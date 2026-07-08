//! Real proof-search driver over MCTS — the Monte-Carlo *Graph* Search upgrade
//! (plan §14, the AlphaProof pattern with transposition).
//!
//! [`crate::search::mcts`] is a generic, closure-driven MCTS over an abstract
//! *tree*: every expansion mints fresh nodes, so two tactic paths that reach the
//! same proof state become two independent subtrees and the search re-explores
//! the shared work twice. This module drives the same PUCT search but over a
//! *graph*: a transposition table keyed on each goal state's canonical form
//! de-duplicates equivalent subgoals into a single node, turning the search tree
//! into a DAG (Monte-Carlo Graph Search / MCGS). Two paths to the same state now
//! share one node, its visit statistics, and all downstream work.
//!
//! The environment is supplied through injectable traits rather than closures, so
//! a *real* prover backend (Lean/Rocq/Isabelle producing goal states and tactic
//! candidates) or a deterministic *mock* plugs into the same driver:
//! * [`GoalState`] — a proof state that knows its dedup key and whether it is
//!   closed (proof complete), plus optional progress/difficulty signals.
//! * [`TacticExpander`] — given a state, propose candidate `(tactic, prior,
//!   next_state)` steps. This is where a model prior or a prover's tactic
//!   enumeration enters. Expansion is seeded (a `seed` is threaded in) so a
//!   backend that samples stays reproducible — there is **no** wall-clock or
//!   unseeded randomness anywhere in the driver.
//!
//! The driver optionally consults a [`crate::search::ttc::TtcController`] to size
//! its per-goal budget (width/rollouts); with no controller attached it uses the
//! fixed [`SearchConfig`] budget, exactly like [`crate::search::mcts`].

use super::mcts::SearchConfig;
use super::ttc::TtcController;
use serde::Serialize;
use std::collections::HashMap;

/// A proof state the driver searches over. Equivalent states must share a
/// [`dedup_key`](GoalState::dedup_key) so they collapse into one graph node.
pub trait GoalState: Clone {
    /// A canonical key identifying equivalent states — the transposition-table
    /// key. Two states with equal keys are treated as the *same* search node
    /// (the essence of the graph, not tree, in MCGS). A real backend should key
    /// on a normalised pretty-print of the goal state (α-equivalent, hypothesis
    /// order canonicalised); the mock keys on its label.
    fn dedup_key(&self) -> String;

    /// Whether the proof is complete at this state (no open goals ⇒ solved).
    fn is_closed(&self) -> bool;

    /// A LeanProgress-style progress estimate in `[0, 1]` (`1.0` ⇒ closer to
    /// done), folded into PUCT selection via [`SearchConfig::progress_weight`].
    /// Defaults to `0.0`, recovering the value-free MCTS behaviour; a real
    /// backend overrides it with [`crate::progress::progress_value_from_state`].
    fn progress(&self) -> f64 {
        0.0
    }

    /// A difficulty estimate in `[0, 1]` (`1.0` ⇒ hardest) consumed by the
    /// [`TtcController`] to size this goal's compute budget. Defaults to `0.5`.
    fn difficulty(&self) -> f64 {
        0.5
    }
}

/// One candidate tactic and the goal state applying it yields — the unit a
/// [`TacticExpander`] proposes.
#[derive(Debug, Clone)]
pub struct TacticStep<S> {
    /// The tactic text (opaque to the driver).
    pub tactic: String,
    /// Prior probability / weight in `[0, 1]` — how promising this tactic is.
    pub prior: f64,
    /// The goal state that results from applying `tactic`.
    pub next: S,
}

impl<S> TacticStep<S> {
    pub fn new(tactic: impl Into<String>, prior: f64, next: S) -> Self {
        Self {
            tactic: tactic.into(),
            prior,
            next,
        }
    }
}

/// The injectable tactic-expansion backend. Given a proof state, return the
/// candidate steps to try from it. A real prover OR a deterministic mock
/// implements this. `seed` is threaded through so a sampling backend can be
/// reproducible; deterministic backends may ignore it.
pub trait TacticExpander {
    /// The proof-state type this backend operates on.
    type State: GoalState;

    /// Expand `state` into candidate `(tactic, prior, next_state)` steps. An
    /// empty result marks a dead end. Implementations MUST be a pure function of
    /// `(state, seed)` — no wall-clock, no unseeded randomness — so the search is
    /// reproducible.
    fn expand(&mut self, state: &Self::State, seed: u64) -> Vec<TacticStep<Self::State>>;
}

/// The outcome of a driven search.
#[derive(Debug, Clone, Serialize)]
pub struct DriverResult {
    /// Whether a closed (proof-complete) state was reached.
    pub solved: bool,
    /// The most-visited (robust) tactic at the root, if any was expanded.
    pub best_tactic: Option<String>,
    /// How many simulations passed through the root.
    pub root_visits: usize,
    /// Search iterations actually run (bounded by the budget / early stop).
    pub iterations: usize,
    /// Distinct nodes in the DAG after de-duplication (== transposition-table
    /// size). Strictly smaller than a tree search would create whenever paths
    /// converge.
    pub nodes_created: usize,
    /// Total out-edges added across all expansions (edges to a de-duplicated node
    /// still count — they are what makes the graph a DAG, not a tree).
    pub edges_created: usize,
    /// How many times an expansion pointed a new edge at an *existing* node
    /// instead of minting a fresh one — the graph-dedup collapses. `> 0` proves
    /// two paths converged onto one node.
    pub dedup_hits: usize,
    /// `(tactic, visit_count)` for every root child — the distilled policy target.
    pub visit_counts: Vec<(String, usize)>,
}

/// One node in the search DAG. Values live on the node (shared across all parents
/// that transpose into it), which is what makes the shared work shared.
struct DagNode<S> {
    state: S,
    closed: bool,
    progress: f64,
    visits: usize,
    value_sum: f64,
    edges: Vec<Edge>,
    expanded: bool,
}

/// A directed edge: a tactic application from a parent node to a (possibly
/// shared) child node.
struct Edge {
    tactic: String,
    prior: f64,
    child: usize,
}

/// Drives a PUCT graph search (MCGS) using an injectable [`TacticExpander`],
/// optionally sized by a [`TtcController`].
pub struct ProofSearchDriver<E: TacticExpander> {
    expander: E,
    cfg: SearchConfig,
    ttc: Option<TtcController>,
    seed: u64,
}

impl<E: TacticExpander> ProofSearchDriver<E> {
    /// A driver with the default [`SearchConfig`], seed `0`, and no TTC
    /// controller (fixed-budget behaviour identical to [`crate::search::mcts`]).
    pub fn new(expander: E) -> Self {
        Self {
            expander,
            cfg: SearchConfig::default(),
            ttc: None,
            seed: 0,
        }
    }

    /// Override the search budget / PUCT tuning.
    pub fn with_config(mut self, cfg: SearchConfig) -> Self {
        self.cfg = cfg;
        self
    }

    /// Set the base seed threaded into expansion (per-node seeds are derived
    /// deterministically from it, so the whole search is reproducible).
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    /// Attach a test-time-compute controller. When present, the driver asks it
    /// how much width/rollouts to spend on each goal (charging the global budget)
    /// instead of using the fixed [`SearchConfig`] budget.
    pub fn with_ttc(mut self, ttc: TtcController) -> Self {
        self.ttc = Some(ttc);
        self
    }

    /// The remaining global budget, if a controller is attached.
    pub fn ttc_remaining(&self) -> Option<u64> {
        self.ttc.as_ref().map(|t| t.remaining())
    }

    /// Run the graph search from `root`. Equivalent to
    /// [`run_attempt`](Self::run_attempt) with `prior_attempts = 0`.
    pub fn run(&mut self, root: E::State) -> DriverResult {
        self.run_attempt(root, 0)
    }

    /// Run the graph search from `root`, telling any attached [`TtcController`]
    /// how many `prior_attempts` this goal has already had (so retries escalate
    /// the compute allocation).
    pub fn run_attempt(&mut self, root: E::State, prior_attempts: u32) -> DriverResult {
        // Decide the per-goal budget. With a controller, size it from the goal's
        // difficulty and charge the global budget; otherwise fall back to the
        // fixed SearchConfig behaviour.
        let (iter_budget, expand_k, max_depth) = match self.ttc.as_mut() {
            Some(ttc) => {
                let alloc = ttc.take(root.difficulty(), prior_attempts);
                (alloc.rollouts, alloc.width.max(1), alloc.max_depth)
            }
            None => (self.cfg.max_nodes, self.cfg.expand_k, self.cfg.max_depth),
        };

        let mut nodes: Vec<DagNode<E::State>> = Vec::new();
        let mut table: HashMap<String, usize> = HashMap::new();

        let root_key = root.dedup_key();
        let root_closed = root.is_closed();
        let root_progress = root.progress();
        nodes.push(DagNode {
            state: root,
            closed: root_closed,
            progress: root_progress,
            visits: 0,
            value_sum: 0.0,
            edges: Vec::new(),
            expanded: false,
        });
        table.insert(root_key, 0);

        let mut solved = root_closed;
        let mut dedup_hits = 0usize;
        let mut edges_created = 0usize;
        let mut iterations = 0usize;
        let node_cap = self.cfg.max_nodes.max(1);
        let base_seed = self.seed;

        for _ in 0..iter_budget.max(1) {
            if solved {
                break;
            }
            iterations += 1;

            // 1. Selection: descend by PUCT to a leaf / closed / dead-end node.
            //    A visited-set guards against cycles the DAG may contain (a state
            //    that transposes back onto an ancestor).
            let mut path = vec![0usize];
            let mut current = 0usize;
            let mut on_path: Vec<bool> = vec![false; nodes.len()];
            on_path[0] = true;
            let mut depth = 0usize;
            while !nodes[current].closed
                && nodes[current].expanded
                && !nodes[current].edges.is_empty()
                && depth < max_depth
            {
                let n_parent = (nodes[current].visits.max(1) as f64).sqrt();
                let mut best_child: Option<usize> = None;
                let mut best_score = f64::NEG_INFINITY;
                for e in &nodes[current].edges {
                    let c = &nodes[e.child];
                    let q = if c.visits > 0 {
                        c.value_sum / c.visits as f64
                    } else {
                        0.0
                    };
                    let u =
                        self.cfg.exploration * e.prior * n_parent / (1.0 + c.visits as f64);
                    // LeanProgress-style value prior, identical to mcts.rs.
                    let score = q + self.cfg.progress_weight * c.progress + u;
                    if score > best_score {
                        best_score = score;
                        best_child = Some(e.child);
                    }
                }
                let next = match best_child {
                    Some(n) => n,
                    None => break,
                };
                // Cycle guard: never descend into a node already on this path.
                if on_path.get(next).copied().unwrap_or(false) {
                    break;
                }
                current = next;
                if current >= on_path.len() {
                    on_path.resize(current + 1, false);
                }
                on_path[current] = true;
                path.push(current);
                depth += 1;
            }

            // 2/3. Evaluate the leaf: closed ⇒ reward 1.0, else expand (with
            //      transposition) and run a greedy rollout.
            let leaf_reward = if nodes[current].closed {
                1.0
            } else {
                if !nodes[current].expanded {
                    let seed = mix_seed(base_seed, &nodes[current].state.dedup_key());
                    let candidates = self.expander.expand(&nodes[current].state, seed);
                    let mut edges = Vec::new();
                    for step in candidates.into_iter().take(expand_k) {
                        let key = step.next.dedup_key();
                        let child = if let Some(&idx) = table.get(&key) {
                            // Transposition: two paths converge onto one node.
                            dedup_hits += 1;
                            idx
                        } else {
                            if nodes.len() >= node_cap {
                                // Node cap reached: stop minting new nodes but keep
                                // any edges into already-known states.
                                continue;
                            }
                            let closed = step.next.is_closed();
                            let progress = step.next.progress();
                            let idx = nodes.len();
                            nodes.push(DagNode {
                                state: step.next,
                                closed,
                                progress,
                                visits: 0,
                                value_sum: 0.0,
                                edges: Vec::new(),
                                expanded: false,
                            });
                            table.insert(key, idx);
                            idx
                        };
                        edges.push(Edge {
                            tactic: step.tactic,
                            prior: step.prior.max(1e-9),
                            child,
                        });
                        edges_created += 1;
                    }
                    nodes[current].edges = edges;
                    nodes[current].expanded = true;
                }
                let start = nodes[current].state.clone();
                Self::rollout(&mut self.expander, base_seed, &start, max_depth)
            };

            if leaf_reward >= 1.0 {
                solved = true;
            }

            // 4. Backpropagation along the traversed path.
            for &ni in &path {
                nodes[ni].visits += 1;
                nodes[ni].value_sum += leaf_reward;
            }
        }

        // Robust child: the most-visited root tactic.
        let mut visit_counts: Vec<(String, usize)> = Vec::new();
        let mut best_tactic = None;
        let mut best_visits = 0usize;
        for e in &nodes[0].edges {
            let visits = nodes[e.child].visits;
            visit_counts.push((e.tactic.clone(), visits));
            if visits >= best_visits {
                best_visits = visits;
                best_tactic = Some(e.tactic.clone());
            }
        }

        DriverResult {
            solved,
            best_tactic,
            root_visits: nodes[0].visits,
            iterations,
            nodes_created: nodes.len(),
            edges_created,
            dedup_hits,
            visit_counts,
        }
    }

    /// Greedy simulation: follow the highest-prior candidate until a closed state
    /// is reached or the depth cap is hit. Does not mint nodes (it only probes the
    /// expander), so it never affects the DAG. Returns `1.0` on reaching a closed
    /// state, else `0.0`.
    fn rollout(expander: &mut E, base_seed: u64, start: &E::State, max_depth: usize) -> f64 {
        if start.is_closed() {
            return 1.0;
        }
        let mut state = start.clone();
        for _ in 0..max_depth {
            if state.is_closed() {
                return 1.0;
            }
            let seed = mix_seed(base_seed, &state.dedup_key());
            let candidates = expander.expand(&state, seed);
            // Highest-prior candidate; ties keep the first (stable, deterministic).
            let mut best: Option<TacticStep<E::State>> = None;
            for step in candidates {
                match &best {
                    Some(b) if b.prior >= step.prior => {}
                    _ => best = Some(step),
                }
            }
            match best {
                Some(step) => state = step.next,
                None => return 0.0,
            }
        }
        if state.is_closed() {
            1.0
        } else {
            0.0
        }
    }
}

/// Derive a deterministic per-state seed from a base seed and a state's dedup
/// key (FNV-1a). Same `(base, key)` ⇒ same seed, so a sampling expander behaves
/// identically every time it sees the same state — no nondeterminism enters.
fn mix_seed(base: u64, key: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64 ^ base;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::ttc::TtcConfig;

    /// A deterministic, table-driven proof state: `key` identifies the state,
    /// `closed` marks proof completion. This is the injectable mock backing every
    /// test — no randomness, everything is a pure function of the table.
    #[derive(Clone)]
    struct MockGoal {
        key: String,
        closed: bool,
        difficulty: f64,
    }

    impl MockGoal {
        fn open(key: &str) -> Self {
            Self {
                key: key.into(),
                closed: false,
                difficulty: 0.5,
            }
        }
        fn closed(key: &str) -> Self {
            Self {
                key: key.into(),
                closed: true,
                difficulty: 0.5,
            }
        }
        fn with_difficulty(mut self, d: f64) -> Self {
            self.difficulty = d;
            self
        }
    }

    impl GoalState for MockGoal {
        fn dedup_key(&self) -> String {
            self.key.clone()
        }
        fn is_closed(&self) -> bool {
            self.closed
        }
        fn difficulty(&self) -> f64 {
            self.difficulty
        }
    }

    /// A deterministic table-driven expander: a map from a state's key to the
    /// candidate steps out of it. Missing keys are dead ends. The `seed` is
    /// accepted (threaded by the driver) but this mock is deterministic, so it is
    /// ignored — same state always yields the same steps.
    struct TableExpander {
        table: HashMap<String, Vec<TacticStep<MockGoal>>>,
    }

    impl TableExpander {
        fn new() -> Self {
            Self {
                table: HashMap::new(),
            }
        }
        fn edge(mut self, from: &str, tactic: &str, prior: f64, to: MockGoal) -> Self {
            self.table
                .entry(from.into())
                .or_default()
                .push(TacticStep::new(tactic, prior, to));
            self
        }
    }

    impl TacticExpander for TableExpander {
        type State = MockGoal;
        fn expand(&mut self, state: &MockGoal, _seed: u64) -> Vec<TacticStep<MockGoal>> {
            self.table.get(&state.key).cloned().unwrap_or_default()
        }
    }

    #[test]
    fn solvable_mock_closes_all_goals() {
        // A chain g3 -> g2 -> g1 -> g0(closed); each step "close" removes a goal.
        let expander = TableExpander::new()
            .edge("g3", "close", 1.0, MockGoal::open("g2"))
            .edge("g2", "close", 1.0, MockGoal::open("g1"))
            .edge("g1", "close", 1.0, MockGoal::closed("g0"));

        let mut driver = ProofSearchDriver::new(expander).with_seed(7);
        let result = driver.run(MockGoal::open("g3"));

        assert!(result.solved, "the chain to g0 should be solved");
        assert_eq!(result.best_tactic.as_deref(), Some("close"));
        assert!(result.root_visits > 0);
        // Never exceeds the iteration budget.
        assert!(result.iterations <= SearchConfig::default().max_nodes);
    }

    #[test]
    fn already_closed_root_is_trivially_solved() {
        let expander = TableExpander::new();
        let mut driver = ProofSearchDriver::new(expander);
        let result = driver.run(MockGoal::closed("done"));
        assert!(result.solved);
        assert_eq!(result.iterations, 0, "no search needed for a closed root");
    }

    #[test]
    fn graph_dedup_collapses_two_paths_into_one_node() {
        // Diamond: A -> B (l) and A -> C (r); B -> D (d1) and C -> D (d2); D -> E.
        // Nothing is closed, so the search runs its full budget and expands every
        // node. The two edges into D must converge onto a SINGLE node — the MCGS
        // graph collapse — rather than minting D twice as a tree would.
        let expander = TableExpander::new()
            .edge("A", "l", 0.5, MockGoal::open("B"))
            .edge("A", "r", 0.5, MockGoal::open("C"))
            .edge("B", "d1", 1.0, MockGoal::open("D"))
            .edge("C", "d2", 1.0, MockGoal::open("D"))
            .edge("D", "e", 1.0, MockGoal::open("E"));

        let mut driver = ProofSearchDriver::new(expander)
            .with_seed(1)
            .with_config(SearchConfig {
                max_nodes: 200,
                ..SearchConfig::default()
            });
        let result = driver.run(MockGoal::open("A"));

        // Exactly one node per distinct state: A, B, C, D, E.
        assert_eq!(
            result.nodes_created, 5,
            "distinct states A,B,C,D,E must each be one node (got {})",
            result.nodes_created
        );
        // Five edges: A->B, A->C, B->D, C->D, D->E. A tree would have re-created
        // D (and its whole subtree) for the second path.
        assert_eq!(result.edges_created, 5);
        // The second edge into D reused the existing node — the graph collapse.
        assert!(
            result.dedup_hits >= 1,
            "expected at least one transposition hit, got {}",
            result.dedup_hits
        );
    }

    #[test]
    fn seed_is_threaded_and_search_is_reproducible() {
        // Two identical runs with the same seed produce identical statistics —
        // there is no wall-clock / unseeded randomness anywhere.
        let build = || {
            TableExpander::new()
                .edge("s2", "a", 0.6, MockGoal::open("s1"))
                .edge("s2", "b", 0.4, MockGoal::open("s1b"))
                .edge("s1", "c", 1.0, MockGoal::closed("s0"))
        };
        let mut d1 = ProofSearchDriver::new(build()).with_seed(42);
        let mut d2 = ProofSearchDriver::new(build()).with_seed(42);
        let r1 = d1.run(MockGoal::open("s2"));
        let r2 = d2.run(MockGoal::open("s2"));
        assert_eq!(r1.solved, r2.solved);
        assert_eq!(r1.visit_counts, r2.visit_counts);
        assert_eq!(r1.nodes_created, r2.nodes_created);
    }

    #[test]
    fn ttc_controller_sizes_the_budget_and_is_charged() {
        // With a controller attached, the driver still solves, and the global
        // budget is charged (spend reflected in the remaining budget).
        let expander = TableExpander::new()
            .edge("h2", "close", 1.0, MockGoal::open("h1"))
            .edge("h1", "close", 1.0, MockGoal::closed("h0"));
        let ttc = TtcController::new(TtcConfig {
            global_budget: 10_000,
            ..TtcConfig::default()
        });
        let mut driver = ProofSearchDriver::new(expander)
            .with_seed(3)
            .with_ttc(ttc);

        let result = driver.run(MockGoal::open("h2").with_difficulty(0.8));
        assert!(result.solved);
        let remaining = driver.ttc_remaining().unwrap();
        assert!(
            remaining < 10_000,
            "the controller should have charged some budget (remaining {remaining})"
        );
    }

    /// A goal where the greedy rollout is a trap: the high-prior tactic leads to
    /// a dead end, and only the low-prior branch closes. A greedy simulation
    /// always picks the dead end, so *solving needs genuine tree search over many
    /// iterations* — which makes it a clean probe for whether the compute budget
    /// actually gates the search.
    fn rollout_trap() -> TableExpander {
        TableExpander::new()
            .edge("A", "trap", 0.9, MockGoal::open("dead"))
            .edge("A", "win", 0.1, MockGoal::closed("won"))
    }

    #[test]
    fn ttc_exhaustion_starves_the_search() {
        // Funded: a controller with plenty of budget lets the search explore past
        // the high-prior dead end and close via the low-prior branch.
        let funded = TtcController::new(TtcConfig {
            global_budget: 100_000,
            ..TtcConfig::default()
        });
        let mut funded_driver = ProofSearchDriver::new(rollout_trap()).with_ttc(funded);
        assert!(
            funded_driver.run(MockGoal::open("A")).solved,
            "a funded search should escape the greedy-rollout trap"
        );

        // Starved: pre-drain the whole global budget, so the goal gets an empty
        // allocation. The loop's `.max(1)` floor runs a single iteration whose
        // greedy rollout takes the trap — the search cannot close the goal.
        let mut ttc = TtcController::new(TtcConfig {
            global_budget: 4,
            base_width: 4,
            base_rollouts: 4,
            ..TtcConfig::default()
        });
        let _ = ttc.take(1.0, 0); // burn it all
        let mut starved = ProofSearchDriver::new(rollout_trap()).with_ttc(ttc);
        let result = starved.run(MockGoal::open("A"));
        assert!(!result.solved, "an exhausted budget must starve the search");
        assert!(result.iterations <= 1);
    }
}
