//! MCTS-Q process supervision — turning ONE terminal correctness bit into DENSE
//! per-node targets (plan item 7; AlphaMath *Process Supervision without Process*,
//! `docs/paper-mining/alphamath-almost-zero.md`, and the Super_MARIO mine,
//! `docs/resource-mining/new/super-mario.md`).
//!
//! Our outcome-only flywheel ([`crate::search::driver`] +
//! `theoremata_tools.flywheel`) labels a whole proof with a single terminal bit:
//! the formal 3+1 gate (Lean/Rocq/Isabelle compile + `#print axioms` closure +
//! kernel typecheck + soundness scan) either passes (`+1`) or fails (`-1`). That
//! trains a policy on positives but leaves the **critic / value head untrained** —
//! there is no per-step signal.
//!
//! AlphaMath's trick closes that gap with *no* step-level annotation: run search
//! over the proof tree, back-propagate each terminal `±1` from the gate, and read
//! off every intermediate node's Monte-Carlo `Q = value_sum / visits` — the mean
//! terminal reward of the simulations passing through it. Those `Q`s become the
//! regression targets for a value head (trained in
//! `theoremata_tools.process_supervision`). Our terminal signal is a *real*
//! verifier verdict, which is strictly stronger (less critic noise) than
//! AlphaMath's answer-string equivalence.
//!
//! This module is the pure, deterministic **Q-backup + target-extraction** core:
//! a mock/injectable [`SearchTree`] decoupled from the live MCGS driver, the
//! [`backup_q`] Monte-Carlo backup, [`q_targets`] step-final regression targets,
//! and [`step_beam_select`] — AlphaMath's backup-free step-level beam selection
//! (SBS), the cheap inference-time approximation of MCTS that ranks frontier
//! nodes by their direct value estimate.
//!
//! There is **no** wall-clock or unseeded randomness anywhere here: every
//! function is a pure function of the tree, so the labels are reproducible.

use serde::Serialize;

/// Terminal reward for a leaf whose proof **passed** the formal 3+1 gate.
/// AlphaMath's `positive_reward`; for us a real verifier verdict, not
/// answer-equivalence.
pub const REWARD_PASS: f64 = 1.0;

/// Terminal reward for a leaf whose proof **failed** the gate (or a structural
/// dead end). AlphaMath's `negative_reward`.
pub const REWARD_FAIL: f64 = -1.0;

/// Map a formal-gate boolean verdict to its `±1` terminal reward — the ONLY
/// external signal the whole backup consumes.
pub fn gate_reward(passed: bool) -> f64 {
    if passed {
        REWARD_PASS
    } else {
        REWARD_FAIL
    }
}

/// One node in the search tree over proof states.
///
/// A node is either an internal reasoning step or a **terminal leaf** carrying a
/// `±1` gate reward. Backup accounting ([`visits`](TreeNode::visits) /
/// [`value_sum`](TreeNode::value_sum)) is filled in by [`backup_q`]; the raw
/// [`value_estimate`](TreeNode::value_estimate) is the value head's *direct*
/// prediction, used by the backup-free [`step_beam_select`].
#[derive(Debug, Clone, Serialize)]
pub struct TreeNode {
    /// Position of this node in the tree's arena (its stable id).
    pub id: usize,
    /// Parent node id; `None` only for the root.
    pub parent: Option<usize>,
    /// Child node ids in insertion order.
    pub children: Vec<usize>,
    /// Terminal reward for a leaf (`Some(±1)` from the gate); `None` for an
    /// internal node. A node with `Some(_)` is where a real simulation ends.
    pub terminal: Option<f64>,
    /// Whether this node is a **step boundary** — the analog of AlphaMath's
    /// `</step>` token where the value head reads `V`. [`q_targets`] emits a
    /// regression target only at step-final nodes.
    pub step_final: bool,
    /// The value head's *direct* estimate of this state in `[-1, 1]`
    /// (`tanh`-bounded). Used by [`step_beam_select`]; irrelevant to the Q-backup.
    pub value_estimate: f64,
    /// Number of simulations (terminal leaves in this node's subtree) that passed
    /// through this node — filled by [`backup_q`].
    pub visits: usize,
    /// Sum of the terminal rewards of those simulations — filled by [`backup_q`].
    /// The Monte-Carlo `Q` is [`value_sum`](TreeNode::value_sum) / `visits`.
    pub value_sum: f64,
}

impl TreeNode {
    /// Mean backed-up reward `Q = value_sum / visits` (`0.0` when unvisited).
    /// AlphaMath's `q_value()`.
    pub fn q(&self) -> f64 {
        if self.visits > 0 {
            self.value_sum / self.visits as f64
        } else {
            0.0
        }
    }

    /// A terminal leaf carries a gate reward.
    pub fn is_terminal(&self) -> bool {
        self.terminal.is_some()
    }
}

/// A per-node regression target: the backed-up `Q` at a step-final node, the
/// label a value head is trained to predict. `features` is left to the caller /
/// the Python trainer (it lives on the proof-state text), so this pure core only
/// carries the `(node_id, q)` pair.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct QTarget {
    pub node_id: usize,
    pub q: f64,
}

/// A mock / injectable search tree, **decoupled from** [`crate::search::driver`].
///
/// The live MCGS driver owns its own `DagNode` arena wired to a real prover; this
/// is the minimal shape the process-supervision math needs — an arena of
/// [`TreeNode`]s with parent/child links and `±1` terminal leaves — so the
/// Q-backup can be built and unit-tested without a model or Lean. A real
/// integration would project a finished driver DAG into this shape (one
/// `TreeNode` per proof state, terminal reward = the formal-gate verdict).
#[derive(Debug, Clone, Default, Serialize)]
pub struct SearchTree {
    nodes: Vec<TreeNode>,
}

impl SearchTree {
    /// An empty tree. Add the root with [`add_root`](Self::add_root).
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    fn push(&mut self, parent: Option<usize>, terminal: Option<f64>) -> usize {
        let id = self.nodes.len();
        self.nodes.push(TreeNode {
            id,
            parent,
            children: Vec::new(),
            terminal,
            step_final: false,
            value_estimate: 0.0,
            visits: 0,
            value_sum: 0.0,
        });
        if let Some(p) = parent {
            self.nodes[p].children.push(id);
        }
        id
    }

    /// Add the root node (the initial goal). Panics if a root already exists.
    pub fn add_root(&mut self) -> usize {
        assert!(self.nodes.is_empty(), "root must be the first node");
        self.push(None, None)
    }

    /// Add an internal (non-terminal) reasoning-step node under `parent`.
    pub fn add_node(&mut self, parent: usize) -> usize {
        self.push(Some(parent), None)
    }

    /// Add a **terminal leaf** under `parent` carrying the formal gate's verdict:
    /// `passed == true` ⇒ `+1`, else `-1`. This is the only place a `±1` enters.
    pub fn add_leaf(&mut self, parent: usize, passed: bool) -> usize {
        self.push(Some(parent), Some(gate_reward(passed)))
    }

    /// Mark a node as a step boundary (so [`q_targets`] emits its `Q`). Returns
    /// `self` for chaining. AlphaMath reads `V` at the step-final token.
    pub fn mark_step_final(&mut self, id: usize) -> &mut Self {
        self.nodes[id].step_final = true;
        self
    }

    /// Set a node's direct value-head estimate in `[-1, 1]` (for
    /// [`step_beam_select`]). Returns `self` for chaining.
    pub fn set_value_estimate(&mut self, id: usize, v: f64) -> &mut Self {
        self.nodes[id].value_estimate = v;
        self
    }

    /// All nodes, in id order.
    pub fn nodes(&self) -> &[TreeNode] {
        &self.nodes
    }

    /// Borrow a node by id.
    pub fn node(&self, id: usize) -> &TreeNode {
        &self.nodes[id]
    }

    /// The backed-up `Q` of a node (`0.0` before [`backup_q`] or when unvisited).
    pub fn q(&self, id: usize) -> f64 {
        self.nodes[id].q()
    }

    /// Leaf nodes (no children) — the search frontier a step-beam ranks over.
    pub fn leaves(&self) -> Vec<usize> {
        self.nodes
            .iter()
            .filter(|n| n.children.is_empty())
            .map(|n| n.id)
            .collect()
    }
}

/// Monte-Carlo Q-backup: turn terminal `±1` gate verdicts into a mean reward on
/// every ancestor.
///
/// Each terminal leaf is one simulation whose reward is its gate verdict. Walking
/// leaf → root, every node on the path gets `visits += 1` and `value_sum +=
/// reward`, exactly AlphaMath's `update_recursive`. Afterwards a node's
/// `Q = value_sum / visits` is the average terminal reward of all simulations
/// through it — so a node on two winning and one losing path scores `Q = (1 + 1 −
/// 1) / 3 = 1/3`. Pure and deterministic: prior accounting is reset first, then
/// leaves are processed in id order (order is irrelevant to the sums anyway).
pub fn backup_q(tree: &mut SearchTree) {
    for n in &mut tree.nodes {
        n.visits = 0;
        n.value_sum = 0.0;
    }
    // Snapshot terminal (leaf_id, reward) pairs first so the walk can mutably
    // borrow the arena without aliasing.
    let terminals: Vec<(usize, f64)> = tree
        .nodes
        .iter()
        .filter_map(|n| n.terminal.map(|r| (n.id, r)))
        .collect();
    for (leaf, reward) in terminals {
        let mut cur = Some(leaf);
        while let Some(id) = cur {
            tree.nodes[id].visits += 1;
            tree.nodes[id].value_sum += reward;
            cur = tree.nodes[id].parent;
        }
    }
}

/// Extract the per-node value-head regression targets after [`backup_q`]: the
/// backed-up `Q` at every **step-final**, visited node, in id order (so the
/// output is deterministic). These `(node_id, q)` pairs are the dense process
/// supervision — one label per reasoning step, manufactured from a single
/// terminal bit with no human/GPT annotation.
pub fn q_targets(tree: &SearchTree) -> Vec<QTarget> {
    tree.nodes
        .iter()
        .filter(|n| n.step_final && n.visits > 0)
        .map(|n| QTarget {
            node_id: n.id,
            q: n.q(),
        })
        .collect()
}

/// AlphaMath **Step-level Beam Search (SBS)** selection, backup-free.
///
/// The production-friendly approximation of MCTS: instead of building a full tree
/// and backing up, score each frontier candidate by its value head's *direct*
/// estimate `V(s)` and keep the best `beam_width`. Given the candidate nodes,
/// return their ids ranked by [`value_estimate`](TreeNode::value_estimate)
/// descending, truncated to `beam_width`. Deterministic: ties break toward the
/// smaller id (stable, encounter order preserved). A `beam_width` of `0` returns
/// nothing; a width past the candidate count returns all of them, still ranked.
// Inference-time entry point: exercised by tests now, called for real once a live
// driver DAG is projected into a `SearchTree` frontier.
#[allow(dead_code)]
pub fn step_beam_select(candidates: &[TreeNode], beam_width: usize) -> Vec<usize> {
    let mut ranked: Vec<&TreeNode> = candidates.iter().collect();
    // Sort by value desc, then id asc. `total_cmp` gives a deterministic order
    // even with NaNs (none expected) — no reliance on partial_cmp unwraps.
    ranked.sort_by(|a, b| {
        b.value_estimate
            .total_cmp(&a.value_estimate)
            .then(a.id.cmp(&b.id))
    });
    ranked.into_iter().take(beam_width).map(|n| n.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Q backs up correctly: a node on **2 winning + 1 losing** path gets
    /// `Q = 1/3` — the canonical AlphaMath backup check.
    #[test]
    fn q_backs_up_two_wins_one_loss_to_one_third() {
        // root(0) -> A(1); A has three terminal children: pass, pass, fail.
        let mut t = SearchTree::new();
        let root = t.add_root();
        let a = t.add_node(root);
        t.mark_step_final(a);
        let _w1 = t.add_leaf(a, true);
        let _w2 = t.add_leaf(a, true);
        let _l1 = t.add_leaf(a, false);

        backup_q(&mut t);

        // A: visits = 3, value_sum = 1 + 1 - 1 = 1, Q = 1/3.
        assert_eq!(t.node(a).visits, 3);
        assert!((t.q(a) - 1.0 / 3.0).abs() < 1e-12, "Q(A) = {}", t.q(a));
        // The root sees the same three simulations.
        assert_eq!(t.node(root).visits, 3);
        assert!((t.q(root) - 1.0 / 3.0).abs() < 1e-12);
        // A leaf's own Q is just its terminal reward.
        assert!((t.q(_w1) - 1.0).abs() < 1e-12);
        assert!((t.q(_l1) + 1.0).abs() < 1e-12);
    }

    /// A proof that passes the (mock) formal gate yields a `+1` leaf, and the
    /// reward is positive all the way up the path to the root.
    #[test]
    fn passing_gate_gives_positive_q_up_the_path() {
        // A stand-in for the formal 3+1 gate: a proof "compiles" iff it ends in
        // `qed`. Pure and deterministic — no Lean process, just the verdict shape.
        fn mock_gate(proof: &str) -> bool {
            proof.trim_end().ends_with("qed")
        }

        let mut t = SearchTree::new();
        let root = t.add_root();
        let step1 = t.add_node(root);
        let step2 = t.add_node(step1);
        t.mark_step_final(step1).mark_step_final(step2);
        // The single simulation ends in a gate PASS.
        let leaf = t.add_leaf(step2, mock_gate("intro; simp; qed"));

        backup_q(&mut t);

        assert_eq!(t.node(leaf).terminal, Some(REWARD_PASS));
        assert!(t.node(leaf).is_terminal());
        // Exactly one node in the whole arena is terminal (the single leaf).
        assert_eq!(t.nodes().iter().filter(|n| n.is_terminal()).count(), 1);
        for id in [root, step1, step2, leaf] {
            assert!(t.q(id) > 0.0, "Q({id}) should be positive, got {}", t.q(id));
        }
        // With one passing simulation every node's Q is exactly +1.
        assert!((t.q(root) - 1.0).abs() < 1e-12);
    }

    /// A failing gate drives Q negative up the path — the mirror image.
    #[test]
    fn failing_gate_gives_negative_q_up_the_path() {
        let mut t = SearchTree::new();
        let root = t.add_root();
        let step = t.add_node(root);
        let leaf = t.add_leaf(step, false); // gate FAIL -> -1

        backup_q(&mut t);

        assert_eq!(t.node(leaf).terminal, Some(REWARD_FAIL));
        assert!((t.q(root) + 1.0).abs() < 1e-12);
        assert!((t.q(step) + 1.0).abs() < 1e-12);
    }

    /// `q_targets` emits a label only at step-final, visited nodes, in id order.
    #[test]
    fn q_targets_are_step_final_only_and_ordered() {
        let mut t = SearchTree::new();
        let root = t.add_root(); // NOT step-final
        let a = t.add_node(root); // step-final
        let b = t.add_node(a); // step-final
        t.mark_step_final(a).mark_step_final(b);
        let _w = t.add_leaf(b, true);
        let _l = t.add_leaf(a, false);

        backup_q(&mut t);
        let targets = q_targets(&t);

        // Only a and b — not the root, not the leaves.
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].node_id, a);
        assert_eq!(targets[1].node_id, b);
        // a is on one win + one loss -> Q = 0; b on the single win -> Q = 1.
        assert!((targets[0].q - 0.0).abs() < 1e-12, "Q(a) = {}", targets[0].q);
        assert!((targets[1].q - 1.0).abs() < 1e-12, "Q(b) = {}", targets[1].q);
    }

    /// Step-beam picks the top-value frontier nodes (backup-free), deterministic
    /// tie-break by id.
    #[test]
    fn step_beam_picks_highest_value_nodes() {
        let mut t = SearchTree::new();
        let root = t.add_root();
        let n1 = t.add_node(root);
        let n2 = t.add_node(root);
        let n3 = t.add_node(root);
        let n4 = t.add_node(root);
        t.set_value_estimate(n1, 0.2)
            .set_value_estimate(n2, 0.9)
            .set_value_estimate(n3, 0.5)
            .set_value_estimate(n4, 0.9); // ties with n2

        // The frontier is exactly the tree's leaves (all four children of root).
        let leaf_ids = t.leaves();
        assert_eq!(leaf_ids, vec![n1, n2, n3, n4]);
        let frontier: Vec<TreeNode> = leaf_ids.iter().map(|&i| t.node(i).clone()).collect();
        let top2 = step_beam_select(&frontier, 2);

        // n2 and n4 share the top value 0.9; tie breaks toward the smaller id.
        assert_eq!(top2, vec![n2, n4]);
        // Width past the candidate count returns all, still ranked.
        let all = step_beam_select(&frontier, 99);
        assert_eq!(all, vec![n2, n4, n3, n1]);
        // Zero width selects nothing.
        assert!(step_beam_select(&frontier, 0).is_empty());
    }

    /// The whole pipeline is deterministic: two identical backups produce
    /// identical targets, and re-running backup is idempotent (accounting reset).
    #[test]
    fn backup_and_targets_are_deterministic() {
        let build = || {
            let mut t = SearchTree::new();
            let root = t.add_root();
            let a = t.add_node(root);
            let b = t.add_node(a);
            t.mark_step_final(a).mark_step_final(b);
            t.add_leaf(b, true);
            t.add_leaf(b, false);
            t.add_leaf(a, true);
            t
        };
        let mut t1 = build();
        let mut t2 = build();
        backup_q(&mut t1);
        backup_q(&mut t2);
        assert_eq!(q_targets(&t1), q_targets(&t2));

        // Idempotent: a second backup on the same tree yields the same Q.
        let before = q_targets(&t1);
        backup_q(&mut t1);
        assert_eq!(before, q_targets(&t1));
    }

    #[test]
    fn gate_reward_maps_verdict_to_pm_one() {
        assert_eq!(gate_reward(true), REWARD_PASS);
        assert_eq!(gate_reward(false), REWARD_FAIL);
    }
}
