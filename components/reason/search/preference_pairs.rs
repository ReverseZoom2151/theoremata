//! Path/sibling **preference-pair** extraction for a **state-value critic** —
//! the InternLM2.5-StepProver *critic-DPO* pattern (`docs/paper-mining/`,
//! prover-mining adopt-list).
//!
//! [`super::best_first`] already mines **policy** DPO pairs: at one state it
//! prefers the *tactic* that continued the proof over a discarded sibling *tactic*
//! (a `(state, winning_tactic, losing_tactic)` triple). That trains the **policy**
//! — which action to emit. It says nothing about how to *score a state*.
//!
//! InternLM2.5-StepProver adds a second, complementary supervision signal that
//! trains the **critic / value head** `V(s)` instead of the policy. From a solved
//! search it reads two kinds of *state* preferences:
//!
//! * **Path pairs** — walking the winning proof path root→goal, every step moves
//!   *closer* to the closed goal, so a child state is preferred over its parent
//!   (`V(child) > V(parent)`). This is the monotone-progress signal the outcome-
//!   only flywheel and even the [`super::process_reward`] Q-backup do not give a
//!   value head directly: it pins the *ordering* of states along the solution.
//! * **Sibling pairs** — at a branch point, the child that lies *on* the winning
//!   path is preferred over each *off-path* sibling the search also expanded but
//!   that did not lead to the goal (`V(on_path) > V(off_path)`). This teaches the
//!   critic to steer selection toward the productive branch.
//!
//! Each emitted [`PreferencePair`] `{positive_state, negative_state}` **is** a
//! Bradley–Terry training target: the critic is trained so
//! `P(positive ≻ negative) = σ(V(positive) − V(negative))` is driven toward `1`
//! (see [`PreferencePair::bradley_terry_target`]). That is exactly the pairwise
//! objective InternLM2.5-StepProver's critic is fit with.
//!
//! ## Relationship to the rest of `search`
//!
//! * [`super::best_first::dpo_pairs`] → **policy** pairs (tactic ≻ tactic).
//! * [`super::process_reward::q_targets`] → **regression** targets (a scalar `Q`
//!   per step) for the value head.
//! * this module → **preference / ranking** pairs (state ≻ state) for the value
//!   head — the pairwise complement of the pointwise `Q` targets, and the seam a
//!   Bradley–Terry critic trainer (`theoremata_tools.process_supervision`) reads.
//!
//! ## Determinism contract
//!
//! Every function here is a pure function of the input tree: pairs are emitted in
//! node-id order (path pairs first, then sibling pairs), deduplicated by value,
//! with a stable order. There is **no** wall-clock and **no** randomness anywhere.
//! Pair extraction is fully offline; only *training* the critic on the emitted
//! Bradley–Terry pairs is GPU-gated and lives behind the Python trainer.

use super::proof_pool::PoolVerdict;
use crate::db::Store;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Event type under which a mined pair batch is persisted.
const EVENT_TYPE: &str = "search.preference_pairs";

/// One node in a minimal proof-search tree annotated for critic-pair extraction.
///
/// This is the smallest shape the path/sibling math needs — decoupled from the
/// live [`super::driver`] DAG exactly as [`super::process_reward::SearchTree`] is.
/// A real integration projects a finished search (a solved [`super::driver`]
/// result or a [`super::best_first`] proof path) into this shape: one node per
/// visited proof state, `on_winning_path` set for the states on the found
/// solution, `remaining_distance` an optional distance-to-goal estimate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CriticNode<S> {
    /// Position of this node in the tree's arena (its stable id).
    pub id: usize,
    /// Parent node id; `None` only for the root.
    pub parent: Option<usize>,
    /// Child node ids, in insertion order (drives deterministic sibling pairing).
    pub children: Vec<usize>,
    /// The proof state this node holds (opaque payload the critic scores).
    pub state: S,
    /// Whether this node lies on the found winning proof path (root→closed goal).
    pub on_winning_path: bool,
    /// Path length from the root (number of tactics applied). The root is `0`.
    pub depth: usize,
    /// Optional distance-to-goal estimate (smaller ⇒ closer to the closed goal).
    /// When present it corroborates the winning-path direction; the extraction
    /// works from tree structure alone when it is `None`.
    pub remaining_distance: Option<usize>,
}

/// A minimal proof-search tree: an arena of [`CriticNode`]s with parent/child
/// links and a marked winning path. Built with [`add_root`](Self::add_root) /
/// [`add_child`](Self::add_child); consumed by [`extract_preference_pairs`].
#[derive(Debug, Clone, Default)]
pub struct PreferenceTree<S> {
    nodes: Vec<CriticNode<S>>,
}

impl<S> PreferenceTree<S> {
    /// An empty tree. Add the root with [`add_root`](Self::add_root).
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    /// Add the root node (the initial goal) at depth `0`. Panics if a root already
    /// exists. `on_winning_path` marks whether the root is on the solution (it is,
    /// for any solved search).
    pub fn add_root(&mut self, state: S, on_winning_path: bool) -> usize {
        assert!(self.nodes.is_empty(), "root must be the first node");
        let id = 0;
        self.nodes.push(CriticNode {
            id,
            parent: None,
            children: Vec::new(),
            state,
            on_winning_path,
            depth: 0,
            remaining_distance: None,
        });
        id
    }

    /// Add a child state under `parent`. Its depth is `parent.depth + 1`.
    pub fn add_child(&mut self, parent: usize, state: S, on_winning_path: bool) -> usize {
        let depth = self.nodes[parent].depth + 1;
        let id = self.nodes.len();
        self.nodes.push(CriticNode {
            id,
            parent: Some(parent),
            children: Vec::new(),
            state,
            on_winning_path,
            depth,
            remaining_distance: None,
        });
        self.nodes[parent].children.push(id);
        id
    }

    /// Attach a distance-to-goal estimate to a node (smaller ⇒ closer). Returns
    /// `self` for chaining. Optional: extraction is correct without it.
    pub fn set_remaining_distance(&mut self, id: usize, distance: usize) -> &mut Self {
        self.nodes[id].remaining_distance = Some(distance);
        self
    }

    /// All nodes, in id order.
    pub fn nodes(&self) -> &[CriticNode<S>] {
        &self.nodes
    }

    /// Borrow a node by id.
    pub fn node(&self, id: usize) -> &CriticNode<S> {
        &self.nodes[id]
    }

    /// Whether the tree has a marked winning path (any node flagged on-path).
    pub fn has_winning_path(&self) -> bool {
        self.nodes.iter().any(|n| n.on_winning_path)
    }
}

/// A single Bradley–Terry **state** preference mined from a solved search: the
/// critic should score `positive_state` above `negative_state`.
///
/// The pair *is* the training target — see [`bradley_terry_target`].
///
/// [`bradley_terry_target`]: PreferencePair::bradley_terry_target
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreferencePair<S> {
    /// The preferred state — closer to the goal, or on the winning path.
    pub positive_state: S,
    /// The dispreferred state — the ancestor, or the off-path sibling.
    pub negative_state: S,
}

impl<S> PreferencePair<S> {
    fn new(positive_state: S, negative_state: S) -> Self {
        Self {
            positive_state,
            negative_state,
        }
    }

    /// The Bradley–Terry training target for this pair.
    ///
    /// A pairwise preference `positive ≻ negative` is fit by driving the modelled
    /// probability `P(positive ≻ negative) = σ(V(positive) − V(negative))` toward
    /// `1` — equivalently, minimising `−log σ(V(positive) − V(negative))`. The
    /// **target probability is therefore `1.0`**: the label says "the positive
    /// state should win the comparison." This is the only supervision the critic
    /// needs from this pair; the value head `V` and the sigmoid live in the
    /// (GPU-gated) trainer, so this pure extractor just carries the ordered pair
    /// and its unit target.
    pub fn bradley_terry_target(&self) -> f64 {
        1.0
    }
}

/// **Path pairs**: along the winning proof path, prefer each child over its
/// parent (the child is one tactic closer to the closed goal, so `V(child) >
/// V(parent)`).
///
/// Emits one [`PreferencePair`] per winning-path edge — a parent/child both
/// flagged [`on_winning_path`](CriticNode::on_winning_path) — in child-id order.
/// Empty when the tree has no winning path.
///
/// [`on_winning_path`]: CriticNode::on_winning_path
pub fn path_pairs<S: Clone>(tree: &PreferenceTree<S>) -> Vec<PreferencePair<S>> {
    let mut out = Vec::new();
    for child in tree.nodes() {
        if !child.on_winning_path {
            continue;
        }
        let parent = match child.parent {
            Some(p) => &tree.nodes()[p],
            None => continue, // the root has no ancestor to be preferred over
        };
        if !parent.on_winning_path {
            continue;
        }
        // The winning-path child is strictly closer to the goal — corroborated by
        // remaining_distance when both are present, and always true by depth.
        out.push(PreferencePair::new(
            child.state.clone(),
            parent.state.clone(),
        ));
    }
    out
}

/// **Sibling pairs**: at each branch point, prefer the on-winning-path child over
/// each off-path sibling the search also expanded (`V(on_path) > V(off_path)`).
///
/// Emits one [`PreferencePair`] per `(on-path child, off-path sibling)` under a
/// shared parent, in `(parent-id, on-path child-id, off-path sibling-id)` order.
/// Empty when the tree has no winning path or no branch point separates on- and
/// off-path siblings.
pub fn sibling_pairs<S: Clone>(tree: &PreferenceTree<S>) -> Vec<PreferencePair<S>> {
    let mut out = Vec::new();
    for parent in tree.nodes() {
        // Children are already in insertion order; split by winning-path flag.
        let on_path: Vec<usize> = parent
            .children
            .iter()
            .copied()
            .filter(|&c| tree.nodes()[c].on_winning_path)
            .collect();
        let off_path: Vec<usize> = parent
            .children
            .iter()
            .copied()
            .filter(|&c| !tree.nodes()[c].on_winning_path)
            .collect();
        for &pos in &on_path {
            for &neg in &off_path {
                out.push(PreferencePair::new(
                    tree.nodes()[pos].state.clone(),
                    tree.nodes()[neg].state.clone(),
                ));
            }
        }
    }
    out
}

/// Extract the full set of critic preference pairs from a solved search: the
/// [`path_pairs`] (child ≻ ancestor) followed by the [`sibling_pairs`] (on-path ≻
/// off-path), deduplicated by value with a stable order.
///
/// Deterministic: pairs keep path-then-sibling, node-id order, and the first
/// occurrence of any duplicate wins. Empty for a search with no winning path.
/// Each returned pair is a Bradley–Terry target for the critic
/// ([`PreferencePair::bradley_terry_target`]).
pub fn extract_preference_pairs<S: Clone + PartialEq>(
    tree: &PreferenceTree<S>,
) -> Vec<PreferencePair<S>> {
    let mut out: Vec<PreferencePair<S>> = Vec::new();
    for pair in path_pairs(tree).into_iter().chain(sibling_pairs(tree)) {
        if !out.contains(&pair) {
            out.push(pair);
        }
    }
    out
}

/// One search branch offered to [`mine_critic_pairs`]: the proof states from the
/// root down to the branch's last state, plus the verifier's verdict on it.
///
/// The verdict is the load-bearing field. [`CriticNode::on_winning_path`] carries
/// no provenance of its own, so nothing downstream can tell a genuinely verified
/// path from one a caller merely believed in. This adapter therefore refuses to
/// infer "winning" from search structure and derives it only from
/// [`PoolVerdict::Passing`], the repo's existing all-pass verifier verdict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriticBranch {
    /// Proof states root-first, one per node along this branch. States are matched
    /// by string equality, so branches sharing a prefix merge into one path and the
    /// point where they diverge becomes the branch point sibling pairs need.
    pub states: Vec<String>,
    /// The verifier's verdict on this branch.
    pub verdict: PoolVerdict,
}

/// CLI entry point: mine Bradley-Terry critic preference pairs from a set of
/// verified and unverified search branches, and persist the batch.
///
/// A thin adapter: it merges the branches into the [`PreferenceTree`] shape and
/// hands that to the existing [`extract_preference_pairs`]. The pairing math is
/// untouched.
///
/// **Only a [`PoolVerdict::Passing`] branch is treated as a winning path.** This
/// output is training data, so a preferred state that was never actually verified
/// would teach the critic that an unverified proof is good. Anything else
/// (`Pending`, `Suspect`, `Failing`) contributes only as a possible *negative* and
/// is counted in `dropped_unverified`. When no branch passes, no pairs are emitted
/// at all rather than pairs backed by nothing.
///
/// Offline and deterministic: pairs follow input order, no model, no wall-clock.
pub fn mine_critic_pairs(
    store: &Store,
    project_id: &str,
    branches: &[CriticBranch],
) -> Result<serde_json::Value> {
    // A flat arena built first so a node's on-path flag is final before the
    // PreferenceTree is constructed (the tree takes the flag at insertion time).
    struct Raw {
        state: String,
        parent: Option<usize>,
        on_path: bool,
    }
    let mut raw: Vec<Raw> = Vec::new();
    let mut children: Vec<Vec<usize>> = Vec::new();

    let mut verified = 0usize;
    let mut dropped_unverified = 0usize;
    let mut dropped_empty = 0usize;
    let mut dropped_root_mismatch = 0usize;

    for branch in branches {
        if branch.states.is_empty() {
            dropped_empty += 1;
            continue;
        }
        // One tree needs one root: branches from a different initial goal belong to
        // a different search and would fabricate sibling pairs across problems.
        if let Some(root) = raw.first() {
            if root.state != branch.states[0] {
                dropped_root_mismatch += 1;
                continue;
            }
        }
        let is_verified = branch.verdict == PoolVerdict::Passing;
        if is_verified {
            verified += 1;
        } else {
            dropped_unverified += 1;
        }

        if raw.is_empty() {
            raw.push(Raw {
                state: branch.states[0].clone(),
                parent: None,
                on_path: is_verified,
            });
            children.push(Vec::new());
        } else if is_verified {
            raw[0].on_path = true;
        }

        let mut cur = 0usize;
        for state in &branch.states[1..] {
            cur = match children[cur]
                .iter()
                .copied()
                .find(|&c| raw[c].state == *state)
            {
                Some(shared) => shared,
                None => {
                    let id = raw.len();
                    raw.push(Raw {
                        state: state.clone(),
                        parent: Some(cur),
                        on_path: false,
                    });
                    children.push(Vec::new());
                    children[cur].push(id);
                    id
                }
            };
            if is_verified {
                raw[cur].on_path = true;
            }
        }
    }

    // Parents are always created before their children, so arena ids and tree ids
    // coincide and `add_child` can take the raw parent id directly.
    let mut tree: PreferenceTree<String> = PreferenceTree::new();
    for node in &raw {
        match node.parent {
            None => tree.add_root(node.state.clone(), node.on_path),
            Some(p) => tree.add_child(p, node.state.clone(), node.on_path),
        };
    }

    let pairs = extract_preference_pairs(&tree);
    let summary = json!({
        "project_id": project_id,
        "branches_total": branches.len(),
        "branches_verified": verified,
        "dropped_unverified": dropped_unverified,
        "dropped_empty": dropped_empty,
        "dropped_root_mismatch": dropped_root_mismatch,
        "tree_nodes": tree.nodes().len(),
        "pair_count": pairs.len(),
        "pairs": pairs.iter().map(|p| json!({
            "positive_state": p.positive_state,
            "negative_state": p.negative_state,
            "bradley_terry_target": p.bradley_terry_target(),
        })).collect::<Vec<_>>(),
    });
    store.event(
        Some(project_id),
        None,
        EVENT_TYPE,
        "preference_pairs",
        summary.clone(),
    )?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A table-driven proof state identified by a label; equality is by label so
    /// [`extract_preference_pairs`] can dedup deterministically.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct MockState(&'static str);

    /// Build a small solved proof tree with a known winning path:
    ///
    /// ```text
    ///        root(*)
    ///        /     \
    ///     A(*)      B        (A on the winning path, B off-path sibling)
    ///     /   \
    ///  C(*)    D             (C on the winning path, D off-path sibling)
    ///    |
    ///  G(*)                  (closed goal)
    /// ```
    ///
    /// Winning path: root → A → C → G. Off-path: B (sibling of A), D (sibling of C).
    fn solved_tree() -> PreferenceTree<MockState> {
        let mut t = PreferenceTree::new();
        let root = t.add_root(MockState("root"), true);
        let a = t.add_child(root, MockState("A"), true);
        let _b = t.add_child(root, MockState("B"), false);
        let c = t.add_child(a, MockState("C"), true);
        let _d = t.add_child(a, MockState("D"), false);
        let _g = t.add_child(c, MockState("G"), true);
        // Optional distance annotations corroborate the winning-path direction.
        t.set_remaining_distance(root, 3)
            .set_remaining_distance(a, 2)
            .set_remaining_distance(c, 1)
            .set_remaining_distance(_g, 0);
        t
    }

    #[test]
    fn path_pairs_prefer_children_over_ancestors() {
        let t = solved_tree();
        let pairs = path_pairs(&t);
        // One pair per winning-path edge: A≻root, C≻A, G≻C. Child is positive.
        assert_eq!(
            pairs,
            vec![
                PreferencePair::new(MockState("A"), MockState("root")),
                PreferencePair::new(MockState("C"), MockState("A")),
                PreferencePair::new(MockState("G"), MockState("C")),
            ]
        );
        // The child (positive) is always closer to the goal than its ancestor.
        for p in &pairs {
            assert_ne!(p.positive_state, p.negative_state);
        }
    }

    #[test]
    fn sibling_pairs_prefer_on_path_over_off_path() {
        let t = solved_tree();
        let pairs = sibling_pairs(&t);
        // Under root: A(on) ≻ B(off). Under A: C(on) ≻ D(off).
        assert_eq!(
            pairs,
            vec![
                PreferencePair::new(MockState("A"), MockState("B")),
                PreferencePair::new(MockState("C"), MockState("D")),
            ]
        );
    }

    #[test]
    fn extract_combines_path_then_sibling_deduped() {
        let t = solved_tree();
        let all = extract_preference_pairs(&t);
        // Path pairs first (3), then sibling pairs (2); none coincide, so 5 total.
        assert_eq!(
            all,
            vec![
                PreferencePair::new(MockState("A"), MockState("root")),
                PreferencePair::new(MockState("C"), MockState("A")),
                PreferencePair::new(MockState("G"), MockState("C")),
                PreferencePair::new(MockState("A"), MockState("B")),
                PreferencePair::new(MockState("C"), MockState("D")),
            ]
        );
    }

    #[test]
    fn no_pairs_without_a_winning_path() {
        // A tree the search expanded but never solved: nothing is on a winning path.
        let mut t = PreferenceTree::new();
        let root = t.add_root(MockState("root"), false);
        let a = t.add_child(root, MockState("A"), false);
        let _b = t.add_child(root, MockState("B"), false);
        let _c = t.add_child(a, MockState("C"), false);

        assert!(!t.has_winning_path());
        assert!(path_pairs(&t).is_empty());
        assert!(sibling_pairs(&t).is_empty());
        assert!(extract_preference_pairs(&t).is_empty());
    }

    #[test]
    fn dedup_removes_a_repeated_pair() {
        // Two off-path siblings that happen to be the SAME state (a transposition
        // reached by two dead branches). Sibling pairing would emit the identical
        // (on ≻ off) pair twice; extract must dedup it to one.
        let mut t = PreferenceTree::new();
        let root = t.add_root(MockState("root"), true);
        let _on = t.add_child(root, MockState("ON"), true);
        let _dup1 = t.add_child(root, MockState("DEAD"), false);
        let _dup2 = t.add_child(root, MockState("DEAD"), false);

        let raw = sibling_pairs(&t);
        assert_eq!(raw.len(), 2, "raw pairing emits one per off-path sibling");
        let deduped = extract_preference_pairs(&t);
        assert_eq!(
            deduped,
            vec![
                // the winning-path edge (root is on-path in a solved tree)...
                PreferencePair::new(MockState("ON"), MockState("root")),
                // ...then the two identical (ON ≻ DEAD) sibling pairs collapse to one.
                PreferencePair::new(MockState("ON"), MockState("DEAD")),
            ],
            "duplicate sibling pairs collapse to one; the path pair is kept"
        );
    }

    #[test]
    fn extraction_is_deterministic() {
        let a = extract_preference_pairs(&solved_tree());
        let b = extract_preference_pairs(&solved_tree());
        assert_eq!(a, b, "same tree ⇒ byte-identical pairs, same order");
    }

    #[test]
    fn bradley_terry_target_is_unit_probability() {
        // The pair IS the BT target: P(positive ≻ negative) trained toward 1.
        let pair = PreferencePair::new(MockState("closer"), MockState("farther"));
        assert_eq!(pair.bradley_terry_target(), 1.0);
    }

    /// A verified branch `root->A->C->G` and an unverified sibling branch
    /// `root->A->D` (D failed the gate). Only the verified path may seed positives.
    fn mixed_branches() -> Vec<CriticBranch> {
        vec![
            CriticBranch {
                states: vec!["root".into(), "A".into(), "C".into(), "G".into()],
                verdict: PoolVerdict::Passing,
            },
            CriticBranch {
                states: vec!["root".into(), "A".into(), "D".into()],
                verdict: PoolVerdict::Failing,
            },
        ]
    }

    #[test]
    fn entry_point_mines_only_from_verified_branches() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "a solved search").unwrap();

        let summary = mine_critic_pairs(&store, &project.id, &mixed_branches()).unwrap();
        assert_eq!(summary["branches_verified"], 1);
        assert_eq!(summary["dropped_unverified"], 1);
        // Path pairs A>root, C>A, G>C on the verified path, plus sibling C>D at the
        // branch point. D never appears as a positive.
        let pairs = summary["pairs"].as_array().unwrap();
        assert_eq!(summary["pair_count"], 4);
        assert!(pairs.iter().all(|p| p["positive_state"] != "D"));
        assert!(pairs
            .iter()
            .any(|p| p["positive_state"] == "C" && p["negative_state"] == "D"));

        let events = store.events(&project.id, 50).unwrap();
        assert!(events
            .iter()
            .any(|e| e.event_type == "search.preference_pairs"));
    }

    #[test]
    fn entry_point_emits_no_pairs_when_nothing_is_verified() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "an unsolved search").unwrap();

        // A suspect branch is merely attempted, not verified: it must not seed a
        // single preferred state.
        let branches = vec![CriticBranch {
            states: vec!["root".into(), "A".into(), "B".into()],
            verdict: PoolVerdict::Suspect,
        }];
        let summary = mine_critic_pairs(&store, &project.id, &branches).unwrap();
        assert_eq!(summary["branches_verified"], 0);
        assert_eq!(summary["dropped_unverified"], 1);
        assert_eq!(summary["pair_count"], 0);
        assert!(summary["pairs"].as_array().unwrap().is_empty());
    }

    #[test]
    fn entry_point_rejects_branches_from_a_different_root() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "two searches").unwrap();

        let branches = vec![
            CriticBranch {
                states: vec!["root".into(), "A".into()],
                verdict: PoolVerdict::Passing,
            },
            // Different initial goal: must not merge into the first tree.
            CriticBranch {
                states: vec!["other".into(), "Z".into()],
                verdict: PoolVerdict::Passing,
            },
        ];
        let summary = mine_critic_pairs(&store, &project.id, &branches).unwrap();
        assert_eq!(summary["dropped_root_mismatch"], 1);
        // Only the first branch's single edge survives: A > root.
        assert_eq!(summary["pair_count"], 1);
        assert_eq!(summary["pairs"][0]["positive_state"], "A");
    }

    #[test]
    fn entry_point_is_deterministic() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "determinism").unwrap();
        let a = mine_critic_pairs(&store, &project.id, &mixed_branches()).unwrap();
        let b = mine_critic_pairs(&store, &project.id, &mixed_branches()).unwrap();
        assert_eq!(a["pairs"], b["pairs"]);
    }

    #[test]
    fn a_pure_chain_yields_only_path_pairs() {
        // No branch points ⇒ no siblings ⇒ sibling_pairs empty; path pairs still
        // pin the monotone ordering along the solution.
        let mut t = PreferenceTree::new();
        let root = t.add_root(MockState("s2"), true);
        let s1 = t.add_child(root, MockState("s1"), true);
        let _s0 = t.add_child(s1, MockState("s0"), true);

        assert!(sibling_pairs(&t).is_empty());
        assert_eq!(
            extract_preference_pairs(&t),
            vec![
                PreferencePair::new(MockState("s1"), MockState("s2")),
                PreferencePair::new(MockState("s0"), MockState("s1")),
            ]
        );
    }
}
