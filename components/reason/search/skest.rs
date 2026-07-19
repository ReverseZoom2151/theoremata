//! SKEST — Shared-Knowledge Ensemble Search over Trees (AlphaGeometry2's SKEST).
//!
//! AlphaGeometry2 runs not one search but an *ensemble* of heterogeneous beam
//! searches that SHARE problem-relevant proved facts ACROSS the concurrent trees:
//! a subgoal closed by one tree becomes instantly reusable by all its siblings,
//! mid-run. This module builds that mechanism for our general MCGS.
//!
//! ## What we already had — and the gap this fills
//!
//! Our stack has two kinds of goal reuse, and SKEST is a *third*, orthogonal one:
//! * [`crate::search::driver`] — a **per-tree transposition table**: within ONE
//!   search, two tactic paths that reach the same canonical goal collapse onto one
//!   DAG node. Scope: a single search tree, a single run.
//! * [`crate::search::goal_cache`] — a **cross-RUN persistent cache**: a subgoal
//!   proven in one search (or process) is durably stored (a [`crate::db::Store`]
//!   table) and reused by the *next* search. Scope: across runs, on disk.
//! * **SKEST (this file)** — a **cross-TREE, in-run, in-memory** pool: while N
//!   trees of a single ensemble run *concurrently* (deterministically interleaved),
//!   a fact tree A proves at round k is visible to tree B at round k+1. Scope: the
//!   sibling trees of one ensemble run, RAM only, live. This is the sharing the
//!   transposition table (one tree) and the goal cache (one run at a time, via a
//!   store) both structurally cannot do.
//!
//! Soundness is inherited from [`crate::search::subsumption`], exactly as the goal
//! cache uses it: a shared hit is honoured only when a sibling's SOLVED goal is
//! *more general than* (subsumes) the current one — reusing a proof of a
//! more-general goal is valid; the reverse would be unsound and is never done.
//!
//! ## Determinism
//!
//! There is no wall-clock and no unseeded randomness. A base seed is threaded in;
//! each tree gets a distinct *derived* seed ([`derive_tree_seed`]) so the ensemble
//! is heterogeneous (different expansion order / prior per tree). The trees are NOT
//! real OS threads — they are stepped in a fixed, seeded round-robin, so the whole
//! run (including which tree publishes a fact before which sibling reuses it) is
//! bit-for-bit reproducible.
//!
//! ## Reused, never edited
//!
//! We only *call* public APIs of the search layer: the [`GoalState`] /
//! [`TacticExpander`] / [`TacticStep`] traits from [`crate::search::driver`], the
//! [`SearchConfig`] tuning struct from [`crate::search::mcts`], and
//! [`CanonicalGoal`] / [`subsumes_str`] from [`crate::search::subsumption`].

use super::driver::{GoalState, TacticExpander, TacticStep};
use super::mcts::SearchConfig;
use super::subsumption::{self, CanonicalGoal};
use crate::db::Store;
use anyhow::{bail, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// A deterministic FNV-1a mix of a base seed and a string — the same primitive the
/// driver uses to derive per-node seeds, re-implemented here so nothing in the
/// driver has to be touched. Same `(base, s)` ⇒ same output.
fn mix64(base: u64, s: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64 ^ base;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The distinct seed for tree `i` of an ensemble with base `seed`. Exposed so a
/// caller (or a heterogeneous `expander_factory`) can specialise a tree by its
/// derived seed. Distinct per `i`, deterministic in `(seed, i)`.
pub fn derive_tree_seed(seed: u64, i: usize) -> u64 {
    mix64(seed, &format!("skest-tree-{i}"))
}

// ---------------------------------------------------------------------------
// SharedFacts — the in-memory pool of solved goals shared across sibling trees.
// ---------------------------------------------------------------------------

/// An in-memory pool of SOLVED goal states shared across the trees of one ensemble
/// run. Facts are keyed on the canonical goal ([`CanonicalGoal::parse`]`.key()`),
/// exactly like the transposition table and the goal cache, so α-equivalent /
/// hypothesis-reordered restatements share a slot. Each fact carries provenance:
/// which tree proved it.
///
/// Two lookup modes, both sound (see the module docs):
/// * [`is_solved`](Self::is_solved) — an *exact* canonical-key hit (a sibling
///   solved this very goal, up to α/reorder).
/// * [`solved_via_subsumption`](Self::solved_via_subsumption) — a sibling solved a
///   *more general* goal that SUBSUMES this one; its proof proves this goal. Only
///   the cached-general → query-specific direction is ever a hit.
pub struct SharedFacts {
    /// Canonical key → provenance (tree index that first proved it).
    exact: HashMap<String, usize>,
    /// Raw (pre-canonical) solved goal strings + provenance, in publish order —
    /// scanned by [`solved_via_subsumption`](Self::solved_via_subsumption). The raw
    /// string is what [`subsumes_str`](subsumption::subsumes_str) parses.
    raw: Vec<(String, usize)>,
    /// Cross-tree reuses recorded (a sibling's fact closed a node here).
    reuses: usize,
    /// When `false`, the pool is inert: publishes are dropped and every lookup
    /// misses. This is the honest control for "would sharing have helped?".
    enabled: bool,
}

/// A serialisable snapshot of a [`SharedFacts`] pool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SharedFactsStats {
    /// Distinct canonical facts recorded.
    pub published: usize,
    /// Cross-tree reuses recorded.
    pub reuses: usize,
}

impl SharedFacts {
    /// A live, sharing pool.
    pub fn new() -> Self {
        Self {
            exact: HashMap::new(),
            raw: Vec::new(),
            reuses: 0,
            enabled: true,
        }
    }

    /// An inert pool: publishes are no-ops and all lookups miss. Used as the
    /// no-sharing control so the *same* ensemble machinery runs with sharing off.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::new()
        }
    }

    /// Record `state` as SOLVED with provenance `tree`. Idempotent on the canonical
    /// key: republishing an already-known fact keeps the first provenance and does
    /// not double-count [`published`](Self::published). A no-op on a disabled pool.
    pub fn publish<S: GoalState>(&mut self, tree: usize, state: &S) {
        if !self.enabled {
            return;
        }
        let raw = state.dedup_key();
        let key = CanonicalGoal::parse(&raw).key();
        if self.exact.contains_key(&key) {
            return;
        }
        self.exact.insert(key, tree);
        self.raw.push((raw, tree));
    }

    /// Exact canonical-key hit: a sibling solved *this* goal (up to α/reorder).
    pub fn is_solved<S: GoalState>(&self, state: &S) -> bool {
        self.exact_provenance(state).is_some()
    }

    /// Subsumption hit: a sibling solved a goal that SUBSUMES `state` — i.e. the
    /// sibling's goal is more general (weaker premises, same conclusion), so its
    /// proof proves `state`. SOUND direction only: cached-general subsumes query;
    /// a more-*specific* solved goal is never a hit here.
    pub fn solved_via_subsumption<S: GoalState>(&self, state: &S) -> bool {
        self.subsuming_provenance(state).is_some()
    }

    /// Provenance of an exact canonical hit, if any.
    fn exact_provenance<S: GoalState>(&self, state: &S) -> Option<usize> {
        if !self.enabled {
            return None;
        }
        let key = CanonicalGoal::parse(&state.dedup_key()).key();
        self.exact.get(&key).copied()
    }

    /// Provenance of a subsumption hit, if any. SOUNDNESS: the stored (general)
    /// fact must subsume the query (specific) — never the reverse.
    fn subsuming_provenance<S: GoalState>(&self, state: &S) -> Option<usize> {
        if !self.enabled {
            return None;
        }
        let query = state.dedup_key();
        for (fact, tree) in &self.raw {
            if subsumption::subsumes_str(fact, &query) {
                return Some(*tree);
            }
        }
        None
    }

    /// Whether `state` is provable from the pool by a fact proved by a tree OTHER
    /// than `self_tree` — the genuine cross-tree case — and (if so) whether via an
    /// exact or a subsumption hit. Exact is checked first (cheap, indexed).
    fn cross_tree_source<S: GoalState>(&self, state: &S, self_tree: usize) -> Option<ReuseKind> {
        if let Some(t) = self.exact_provenance(state) {
            if t != self_tree {
                return Some(ReuseKind::Exact);
            }
        }
        if let Some(t) = self.subsuming_provenance(state) {
            if t != self_tree {
                return Some(ReuseKind::Subsumption);
            }
        }
        None
    }

    /// Record that a cross-tree reuse fired.
    fn record_reuse(&mut self) {
        self.reuses += 1;
    }

    /// Distinct canonical facts published.
    pub fn published(&self) -> usize {
        self.exact.len()
    }

    /// Cross-tree reuses recorded.
    pub fn reuses(&self) -> usize {
        self.reuses
    }

    /// A serialisable snapshot.
    pub fn stats(&self) -> SharedFactsStats {
        SharedFactsStats {
            published: self.published(),
            reuses: self.reuses(),
        }
    }
}

impl Default for SharedFacts {
    fn default() -> Self {
        Self::new()
    }
}

/// How a shared hit was found (for the reuse counter's bookkeeping).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReuseKind {
    Exact,
    Subsumption,
}

// ---------------------------------------------------------------------------
// TreeSearch — one steppable best-first search tree in the ensemble.
// ---------------------------------------------------------------------------

/// One node of a tree's search graph. `parent` is the first discoverer (used only
/// to reconstruct the solution path for publishing), so the graph is explored as a
/// best-first tree with a per-tree transposition table on top.
struct TreeNode<S> {
    state: S,
    dedup_key: String,
    parent: Option<usize>,
    depth: usize,
    /// Best-first priority: `prior + progress_weight · progress` of the edge/state.
    priority: f64,
}

/// A single steppable best-first search over an injected [`TacticExpander`].
///
/// The frontier is ordered by the driver's progress/prior signal (highest first;
/// ties broken by a seed-derived hash so heterogeneous trees explore equal-priority
/// frontiers in *different* orders). A per-tree transposition table on
/// [`GoalState::dedup_key`] de-duplicates states within the tree.
///
/// A node counts as SOLVED when it is popped and either [`GoalState::is_closed`],
/// or the shared pool reports [`SharedFacts::is_solved`], or
/// [`SharedFacts::solved_via_subsumption`] — the last two being cross-tree reuse.
/// On solving, the solution-path states are published to the pool so siblings
/// benefit mid-run. Deterministic given its seed.
pub struct TreeSearch<E: TacticExpander> {
    expander: E,
    seed: u64,
    tree_id: usize,
    cfg: SearchConfig,
    nodes: Vec<TreeNode<E::State>>,
    /// Indices of not-yet-expanded nodes (the best-first frontier).
    frontier: Vec<usize>,
    /// Per-tree transposition table: dedup_key → node index.
    table: HashMap<String, usize>,
    solved: bool,
    /// Root node index (always 0).
    root: usize,
    steps: usize,
    /// Nodes actually expanded (expander invoked) — the "work" metric.
    states_explored: usize,
    /// Cross-tree reuses this tree benefited from.
    reuses: usize,
    /// Dedup keys in pop order — inspectable to show heterogeneous ordering.
    trace: Vec<String>,
}

impl<E: TacticExpander> TreeSearch<E> {
    /// Build a tree rooted at `root`, identified as tree `tree_id`, seeded `seed`.
    pub fn new(expander: E, root: E::State, seed: u64, tree_id: usize, cfg: SearchConfig) -> Self {
        let key = root.dedup_key();
        let priority = 1.0 + cfg.progress_weight * root.progress();
        let mut table = HashMap::new();
        table.insert(key.clone(), 0usize);
        Self {
            expander,
            seed,
            tree_id,
            cfg,
            nodes: vec![TreeNode {
                state: root,
                dedup_key: key,
                parent: None,
                depth: 0,
                priority,
            }],
            frontier: vec![0],
            table,
            solved: false,
            root: 0,
            steps: 0,
            states_explored: 0,
            reuses: 0,
            trace: Vec::new(),
        }
    }

    /// Pick the best frontier node: highest priority, ties broken by the smallest
    /// seed-mixed hash of the node's dedup key (so a different seed ⇒ a different
    /// tie-break ⇒ a different exploration order). Returns its position *in the
    /// frontier vector* for O(1) swap-removal.
    fn pick_best(&self) -> Option<usize> {
        let mut best_pos: Option<usize> = None;
        let mut best_pr = f64::NEG_INFINITY;
        let mut best_tie = u64::MAX;
        for (pos, &ni) in self.frontier.iter().enumerate() {
            let pr = self.nodes[ni].priority;
            let tie = mix64(self.seed, &self.nodes[ni].dedup_key);
            if pr > best_pr || (pr == best_pr && tie < best_tie) {
                best_pr = pr;
                best_tie = tie;
                best_pos = Some(pos);
            }
        }
        best_pos
    }

    /// Advance this tree by one step: expand the single best frontier node, unless
    /// that node is already solved (its own closure or a sibling's shared fact), in
    /// which case the tree's root is solved and the solution path is published.
    ///
    /// Live sharing: any fact published here is visible to sibling trees stepped
    /// after this call — including later in the same round-robin round.
    pub fn step(&mut self, shared: &mut SharedFacts) {
        if self.solved {
            return;
        }
        let pos = match self.pick_best() {
            Some(p) => p,
            None => return, // frontier exhausted
        };
        let cur = self.frontier.swap_remove(pos);
        self.steps += 1;
        self.trace.push(self.nodes[cur].dedup_key.clone());

        // Solved? Own closure first (never counts as reuse); then the shared pool.
        let closed = self.nodes[cur].state.is_closed();
        let shared_hit = !closed
            && (shared.is_solved(&self.nodes[cur].state)
                || shared.solved_via_subsumption(&self.nodes[cur].state));
        if closed || shared_hit {
            if shared_hit {
                // Count only genuine CROSS-tree reuse (a sibling's fact, not our own).
                if shared
                    .cross_tree_source(&self.nodes[cur].state, self.tree_id)
                    .is_some()
                {
                    self.reuses += 1;
                    shared.record_reuse();
                }
            }
            self.solved = true;
            self.publish_solution_path(shared, cur);
            return;
        }

        // Otherwise expand this node (the one unit of real work a step does).
        self.expand(cur, shared);
    }

    /// Expand `cur`: query the seeded expander, add the top-`expand_k` children with
    /// per-tree transposition, and eagerly publish any child that is already closed
    /// (a proven fact siblings can reuse immediately, even before it is popped).
    fn expand(&mut self, cur: usize, shared: &mut SharedFacts) {
        self.states_explored += 1;
        let depth = self.nodes[cur].depth;
        if depth >= self.cfg.max_depth {
            return; // depth cap: stop growing this branch (budget still bounds steps)
        }
        let seed = mix64(self.seed, &self.nodes[cur].dedup_key);
        let candidates = self.expander.expand(&self.nodes[cur].state, seed);
        for step in candidates.into_iter().take(self.cfg.expand_k.max(1)) {
            let key = step.next.dedup_key();
            if self.table.contains_key(&key) {
                continue; // within-tree transposition: already have this state
            }
            let closed = step.next.is_closed();
            let priority = step.prior + self.cfg.progress_weight * step.next.progress();
            let idx = self.nodes.len();
            // Eagerly publish a freshly-discovered closed subgoal so a sibling can
            // reuse it before this tree gets around to popping it.
            if closed {
                shared.publish(self.tree_id, &step.next);
            }
            self.nodes.push(TreeNode {
                state: step.next,
                dedup_key: key.clone(),
                parent: Some(cur),
                depth: depth + 1,
                priority,
            });
            self.table.insert(key, idx);
            self.frontier.push(idx);
        }
    }

    /// Publish every state on the root→`leaf` path as a solved fact (each is a goal
    /// from which the proof completes), with this tree's provenance.
    fn publish_solution_path(&mut self, shared: &mut SharedFacts, leaf: usize) {
        let mut cur = Some(leaf);
        while let Some(idx) = cur {
            // Clone out to satisfy the borrow checker (publish borrows `shared` mut).
            let state = self.nodes[idx].state.clone();
            shared.publish(self.tree_id, &state);
            cur = self.nodes[idx].parent;
        }
        let _ = self.root; // root is index 0; the walk terminates at parent == None
    }

    /// Whether this tree has solved its root.
    pub fn solved(&self) -> bool {
        self.solved
    }

    /// Whether this tree can make no further progress (empty frontier, unsolved).
    pub fn is_exhausted(&self) -> bool {
        !self.solved && self.frontier.is_empty()
    }

    /// This tree's derived seed.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Steps taken (nodes popped).
    pub fn steps(&self) -> usize {
        self.steps
    }

    /// Nodes expanded (expander invoked) — the work metric.
    pub fn states_explored(&self) -> usize {
        self.states_explored
    }

    /// Cross-tree reuses this tree benefited from.
    pub fn reuses(&self) -> usize {
        self.reuses
    }

    /// Dedup keys in pop order (heterogeneous ordering is visible here).
    pub fn trace(&self) -> &[String] {
        &self.trace
    }
}

// ---------------------------------------------------------------------------
// skest_search — the ensemble orchestrator.
// ---------------------------------------------------------------------------

/// Per-tree budget + ensemble shape for [`skest_search`].
#[derive(Debug, Clone, Copy)]
pub struct SkestConfig {
    /// Number of heterogeneous trees in the ensemble.
    pub trees: usize,
    /// Global step budget: total `step()`s across all trees (round-robin), the
    /// ensemble's whole compute allowance.
    pub max_steps: usize,
    /// Per-tree search tuning (expand width, progress weight, depth cap). Reused
    /// from the driver so a tree here behaves like a driver frontier.
    pub search: SearchConfig,
    /// Whether facts are shared. `false` is the honest no-sharing control: the same
    /// ensemble runs with an inert pool.
    pub share: bool,
}

impl Default for SkestConfig {
    fn default() -> Self {
        Self {
            trees: 4,
            max_steps: 1024,
            search: SearchConfig::default(),
            share: true,
        }
    }
}

/// Per-tree outcome inside a [`SkestResult`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TreeStat {
    pub tree: usize,
    pub seed: u64,
    pub steps: usize,
    pub states_explored: usize,
    pub reuses: usize,
    pub solved: bool,
}

/// The outcome of a SKEST ensemble run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkestResult {
    /// Whether any tree solved the root.
    pub solved: bool,
    /// Which tree solved it first, if any.
    pub winner_tree: Option<usize>,
    /// The shared pool's final stats.
    pub shared: SharedFactsStats,
    /// Total cross-tree reuses across the ensemble (== `shared.reuses`).
    pub cross_tree_reuses: usize,
    /// Total nodes expanded across the ensemble (the ensemble's work).
    pub states_explored: usize,
    /// Per-tree breakdown.
    pub per_tree: Vec<TreeStat>,
}

/// Run a SKEST ensemble: build `config.trees` HETEROGENEOUS trees (each from
/// `expander_factory(derived_seed)` with a distinct [`derive_tree_seed`], hence a
/// different expansion order / prior), share ONE [`SharedFacts`] pool across them,
/// and interleave their `step()`s in a DETERMINISTIC round-robin until one tree
/// solves the root or the global `max_steps` budget is exhausted.
///
/// Sharing is live: a fact tree A publishes while it steps is visible to tree B
/// when B steps next in the round-robin. Nothing here uses wall-clock time or
/// unseeded randomness, and the trees are not OS threads — so the interleaving, and
/// therefore the whole result, is bit-for-bit reproducible for a given `seed`.
pub fn skest_search<E, F>(
    expander_factory: F,
    root: E::State,
    config: SkestConfig,
    seed: u64,
) -> SkestResult
where
    E: TacticExpander,
    F: Fn(u64) -> E,
{
    let mut shared = if config.share {
        SharedFacts::new()
    } else {
        SharedFacts::disabled()
    };
    let n = config.trees.max(1);

    let mut trees: Vec<TreeSearch<E>> = Vec::with_capacity(n);
    for i in 0..n {
        let ts = derive_tree_seed(seed, i);
        trees.push(TreeSearch::new(
            expander_factory(ts),
            root.clone(),
            ts,
            i,
            config.search,
        ));
    }

    let mut winner: Option<usize> = None;
    let budget = config.max_steps.max(1);
    let mut step = 0usize;
    while step < budget {
        let i = step % n;
        step += 1;
        if !trees[i].solved && !trees[i].is_exhausted() {
            trees[i].step(&mut shared);
            if trees[i].solved {
                winner = Some(i);
                break;
            }
        }
        // All trees done (solved or stuck): stop early, no point spinning the budget.
        if trees.iter().all(|t| t.solved || t.is_exhausted()) {
            break;
        }
    }

    let per_tree = trees
        .iter()
        .map(|t| TreeStat {
            tree: t.tree_id,
            seed: t.seed(),
            steps: t.steps(),
            states_explored: t.states_explored(),
            reuses: t.reuses(),
            solved: t.solved(),
        })
        .collect();
    let states_explored = trees.iter().map(|t| t.states_explored()).sum();

    SkestResult {
        solved: winner.is_some(),
        winner_tree: winner,
        cross_tree_reuses: shared.reuses(),
        states_explored,
        shared: shared.stats(),
        per_tree,
    }
}

// ---------------------------------------------------------------------------
// CLI entry point.
//
// `skest_search` is generic over `TacticExpander`, so a command line cannot call
// it without a concrete proof-state space. No production tactic backend is wired
// into this repository yet, so the entry point runs the ensemble over an EXPLICIT
// goal graph the caller supplies: goal texts as nodes, tactic edges between them,
// and a set of already-closed goals. That is the smallest input from which the
// cross-tree sharing this module exists to demonstrate can actually be exercised;
// a real prover backend that implements `TacticExpander` plugs into the same
// `skest_search` later without touching this file.
// ---------------------------------------------------------------------------

/// One goal state in a caller-supplied goal graph. The `key` is the goal text and
/// doubles as the dedup key (so the subsumption machinery parses it exactly as it
/// parses a real backend's pretty-printed goal). `closed` marks proof completion.
#[derive(Clone)]
pub struct GraphGoal {
    key: String,
    closed: bool,
}

impl GoalState for GraphGoal {
    fn dedup_key(&self) -> String {
        self.key.clone()
    }
    fn is_closed(&self) -> bool {
        self.closed
    }
}

/// A tactic edge in the goal graph: applying `tactic` (with weight `prior`) to the
/// goal `from` yields the goal `to`.
///
/// `Deserialize` so a CLI dispatch arm can accept a goal graph as JSON.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GraphEdge {
    pub from: String,
    pub tactic: String,
    pub prior: f64,
    pub to: String,
}

/// A deterministic goal-graph expander built from an adjacency table. Pure in
/// `(state, seed)`: it ignores the seed and returns the recorded successors, so
/// every tree in the ensemble sees the same edges and only the seeded frontier
/// tie-break makes them heterogeneous.
#[derive(Clone)]
pub struct GoalGraphExpander {
    edges: HashMap<String, Vec<(String, f64, String)>>,
    closed: HashSet<String>,
}

impl GoalGraphExpander {
    fn goal(&self, key: &str) -> GraphGoal {
        GraphGoal {
            key: key.to_string(),
            closed: self.closed.contains(key),
        }
    }
}

impl TacticExpander for GoalGraphExpander {
    type State = GraphGoal;
    fn expand(&mut self, state: &GraphGoal, _seed: u64) -> Vec<TacticStep<GraphGoal>> {
        self.edges
            .get(&state.key)
            .map(|succ| {
                succ.iter()
                    .map(|(tactic, prior, to)| {
                        TacticStep::new(tactic.clone(), *prior, self.goal(to))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Run a SKEST ensemble over an explicit goal graph and record the outcome,
/// returning a JSON summary for the CLI.
///
/// The result is a SEARCH OUTCOME over a caller-asserted graph, not a checked
/// proof. "Solved" here means the ensemble reached a goal the CALLER marked
/// closed (or one subsumed by such a goal); the closedness is an input, not a
/// verdict from Lean, Rocq, or Isabelle. The summary therefore carries
/// `status: "candidate"` and `formally_verified: false` unconditionally so
/// nothing downstream can read the ensemble's "solved" as a formal proof.
///
/// Deterministic in `(closed_goals, edges, root, config, seed)`.
pub fn run_skest_search(
    store: &Store,
    project_id: Option<&str>,
    closed_goals: Vec<String>,
    edges: Vec<GraphEdge>,
    root: String,
    config: SkestConfig,
    seed: u64,
) -> Result<Value> {
    if root.is_empty() {
        bail!("skest search needs a non-empty root goal");
    }
    let closed: HashSet<String> = closed_goals.into_iter().collect();
    let mut adjacency: HashMap<String, Vec<(String, f64, String)>> = HashMap::new();
    for e in &edges {
        if e.from.is_empty() || e.to.is_empty() {
            bail!("skest graph edges must have non-empty endpoints");
        }
        adjacency
            .entry(e.from.clone())
            .or_default()
            .push((e.tactic.clone(), e.prior, e.to.clone()));
    }

    let root_goal = GraphGoal {
        key: root.clone(),
        closed: closed.contains(&root),
    };
    // Each tree is the same graph; heterogeneity comes from the per-tree seed's
    // frontier tie-break, exactly as in `skest_search`'s contract.
    let factory = |_derived_seed: u64| GoalGraphExpander {
        edges: adjacency.clone(),
        closed: closed.clone(),
    };
    let result = skest_search(factory, root_goal, config, seed);

    let summary = json!({
        "kind": "skest_ensemble_search",
        // A search over an asserted graph surfaces a candidate. These two keys are
        // the guard against a reader promoting the ensemble's "solved" to a proof.
        "status": "candidate",
        "formally_verified": false,
        "root": root,
        "seed": seed,
        "trees": config.trees,
        "max_steps": config.max_steps,
        "share": config.share,
        "solved": result.solved,
        "winner_tree": result.winner_tree,
        "cross_tree_reuses": result.cross_tree_reuses,
        "states_explored": result.states_explored,
        "published_facts": result.shared.published,
        "per_tree": result.per_tree,
        "note": "Search over a caller-supplied goal graph. 'solved' means a \
                 caller-marked-closed goal was reached; no formal system verified \
                 anything here.",
    });

    store.event(
        project_id,
        None,
        "skest.searched",
        "skest",
        summary.clone(),
    )?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::super::driver::{GoalState, TacticExpander, TacticStep};
    use super::*;
    use std::collections::HashMap;

    // ---- Deterministic mock goal + expanders --------------------------------

    /// A table-driven proof state: `key` is the goal text (also the dedup key),
    /// `closed` marks proof completion. Pure — no randomness.
    #[derive(Clone)]
    struct MockGoal {
        key: String,
        closed: bool,
    }
    impl MockGoal {
        fn open(key: &str) -> Self {
            Self {
                key: key.into(),
                closed: false,
            }
        }
        fn closed(key: &str) -> Self {
            Self {
                key: key.into(),
                closed: true,
            }
        }
    }
    impl GoalState for MockGoal {
        fn dedup_key(&self) -> String {
            self.key.clone()
        }
        fn is_closed(&self) -> bool {
            self.closed
        }
    }

    /// A deterministic table expander: a map from goal key → candidate steps.
    #[derive(Clone)]
    struct TableExpander {
        table: HashMap<String, Vec<(String, f64, MockGoal)>>,
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
                .push((tactic.into(), prior, to));
            self
        }
    }
    impl TacticExpander for TableExpander {
        type State = MockGoal;
        fn expand(&mut self, state: &MockGoal, _seed: u64) -> Vec<TacticStep<MockGoal>> {
            self.table
                .get(&state.key)
                .map(|v| {
                    v.iter()
                        .map(|(t, p, s)| TacticStep::new(t.clone(), *p, s.clone()))
                        .collect()
                })
                .unwrap_or_default()
        }
    }

    fn cfg(trees: usize, share: bool) -> SkestConfig {
        SkestConfig {
            trees,
            max_steps: 512,
            search: SearchConfig::default(),
            share,
        }
    }

    // ---- SharedFacts soundness ---------------------------------------------

    #[test]
    fn subsumption_reuse_fires_general_over_specific_but_never_reverse() {
        // General fact "⊢ P" (no hypotheses) proved by tree 0.
        let mut pool = SharedFacts::new();
        pool.publish(0, &MockGoal::closed("⊢ P"));

        // A specific query "H ⊢ P" (extra hypothesis, same conclusion) is subsumed
        // by the general fact: its proof proves the query.
        assert!(pool.solved_via_subsumption(&MockGoal::open("H ⊢ P")));
        // Exact hit for the very same goal too.
        assert!(pool.is_solved(&MockGoal::open("⊢ P")));

        // The REVERSE must never fire: a pool holding only the SPECIFIC fact must
        // not mark the more-general goal solved.
        let mut pool2 = SharedFacts::new();
        pool2.publish(0, &MockGoal::closed("H ⊢ P"));
        assert!(
            !pool2.solved_via_subsumption(&MockGoal::open("⊢ P")),
            "a specific solved goal must NOT falsely mark a more-general one solved"
        );
        assert!(!pool2.is_solved(&MockGoal::open("⊢ P")));
    }

    #[test]
    fn disabled_pool_never_hits_and_never_publishes() {
        let mut pool = SharedFacts::disabled();
        pool.publish(0, &MockGoal::closed("⊢ P"));
        assert_eq!(pool.published(), 0);
        assert!(!pool.is_solved(&MockGoal::open("⊢ P")));
        assert!(!pool.solved_via_subsumption(&MockGoal::open("H ⊢ P")));
    }

    // ---- The reuse scenario -------------------------------------------------
    //
    // Two heterogeneous trees over a shared root R:
    //  * tree 0 (mode A): R -(a)-> M -(a2)-> Sg,   Sg = closed general goal "⊢ P".
    //  * tree 1 (mode B): R -(b)-> Xs,             Xs = OPEN specific goal "H ⊢ P"
    //                                              (a dead end on its own).
    // Xs is only solvable by SUBSUMPTION of the general Sg tree 0 proves. Because
    // tree 0 reaches Sg via the longer M path, it PUBLISHES Sg (on expanding M)
    // before it pops Sg — and tree 1, popping Xs the very next step, reuses it and
    // wins. Only tree 1 benefits, and it benefits cross-tree.
    fn mode_a() -> TableExpander {
        TableExpander::new()
            .edge("R", "a", 1.0, MockGoal::open("M"))
            .edge("M", "a2", 1.0, MockGoal::closed("⊢ P"))
    }
    fn mode_b() -> TableExpander {
        TableExpander::new().edge("R", "b", 1.0, MockGoal::open("H ⊢ P"))
        // "H ⊢ P" has no edges: a dead end unless a sibling proves a subsuming goal.
    }
    /// Factory that specialises each tree by its derived seed.
    fn reuse_factory(seed: u64) -> impl Fn(u64) -> TableExpander {
        let s0 = derive_tree_seed(seed, 0);
        move |ts: u64| if ts == s0 { mode_a() } else { mode_b() }
    }

    #[test]
    fn subgoal_proved_by_one_tree_is_reused_by_the_winner() {
        let seed = 7;
        let result = skest_search(reuse_factory(seed), MockGoal::open("R"), cfg(2, true), seed);

        assert!(result.solved, "the ensemble should solve via reuse");
        assert!(
            result.cross_tree_reuses > 0,
            "a sibling's fact must have been reused (got {})",
            result.cross_tree_reuses
        );
        let winner = result.winner_tree.expect("a winner");
        assert_eq!(winner, 1, "tree 1 (mode B) wins by reusing tree 0's fact");
        // The WINNER is the tree that benefited from the reuse.
        assert!(
            result.per_tree[winner].reuses > 0,
            "the winner must be the tree that reused the shared fact"
        );
        // The winner never closed a goal itself — it relied entirely on the sibling.
        assert!(result.per_tree[1].states_explored >= 1);
    }

    #[test]
    fn sharing_solves_with_fewer_expansions_than_no_sharing() {
        let seed = 7;
        let shared = skest_search(reuse_factory(seed), MockGoal::open("R"), cfg(2, true), seed);
        let control = skest_search(
            reuse_factory(seed),
            MockGoal::open("R"),
            cfg(2, false),
            seed,
        );

        assert!(shared.solved, "sharing run must solve");
        assert!(
            shared.states_explored < control.states_explored,
            "sharing must pay off: {} expansions with sharing vs {} without",
            shared.states_explored,
            control.states_explored
        );
        assert_eq!(
            control.cross_tree_reuses, 0,
            "the control never shares, so it can never reuse"
        );
    }

    // ---- Heterogeneity ------------------------------------------------------

    #[test]
    fn heterogeneous_seeds_explore_in_different_orders() {
        // A root with three EQUAL-prior children: ordering is decided purely by the
        // seed tie-break, so two differently-seeded trees pop them differently.
        let build = || {
            TableExpander::new()
                .edge("root", "x", 0.5, MockGoal::open("cx"))
                .edge("root", "y", 0.5, MockGoal::open("cy"))
                .edge("root", "z", 0.5, MockGoal::open("cz"))
        };
        let mut shared = SharedFacts::new();
        let mut t_a = TreeSearch::new(
            build(),
            MockGoal::open("root"),
            derive_tree_seed(1, 0),
            0,
            SearchConfig::default(),
        );
        let mut t_b = TreeSearch::new(
            build(),
            MockGoal::open("root"),
            derive_tree_seed(1, 1),
            1,
            SearchConfig::default(),
        );
        // Pop root (expands the 3 children), then pop the three children in order.
        for _ in 0..4 {
            t_a.step(&mut shared);
            t_b.step(&mut shared);
        }
        assert_ne!(
            t_a.trace(),
            t_b.trace(),
            "different derived seeds must yield different exploration orders"
        );
    }

    // ---- No false solves ----------------------------------------------------

    #[test]
    fn unsolvable_in_budget_returns_false_never_a_false_solve() {
        // No closed state anywhere, no negation, nothing to subsume: the ensemble
        // must exhaust and report solved = false.
        let factory = |_seed: u64| {
            TableExpander::new()
                .edge("g", "loop", 1.0, MockGoal::open("g")) // self-loop, deduped away
                .edge("g", "step", 0.5, MockGoal::open("h")) // dead end
        };
        let result = skest_search(factory, MockGoal::open("g"), cfg(3, true), 5);
        assert!(!result.solved, "an unsolvable goal must not be solved");
        assert_eq!(result.winner_tree, None);
        assert_eq!(result.cross_tree_reuses, 0, "nothing to reuse");
    }

    #[test]
    fn genuinely_solvable_chain_solves() {
        // A plain solvable chain with no sharing needed: g2 -> g1 -> g0(closed).
        let factory = |_seed: u64| {
            TableExpander::new()
                .edge("g2", "c", 1.0, MockGoal::open("g1"))
                .edge("g1", "c", 1.0, MockGoal::closed("g0"))
        };
        let result = skest_search(factory, MockGoal::open("g2"), cfg(2, true), 3);
        assert!(result.solved);
        assert!(result.winner_tree.is_some());
    }

    // ---- Determinism --------------------------------------------------------

    #[test]
    fn same_seed_gives_identical_result_and_counters() {
        let seed = 7;
        let r1 = skest_search(reuse_factory(seed), MockGoal::open("R"), cfg(2, true), seed);
        let r2 = skest_search(reuse_factory(seed), MockGoal::open("R"), cfg(2, true), seed);
        assert_eq!(r1, r2, "same seed ⇒ identical result + counters");
    }

    // ---- CLI entry point ----------------------------------------------------

    use std::path::Path;

    fn store_with_project() -> (Store, String) {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store.create_project("p", "skest entry point").unwrap();
        (store, project.id)
    }

    fn edge(from: &str, tactic: &str, prior: f64, to: &str) -> GraphEdge {
        GraphEdge {
            from: from.into(),
            tactic: tactic.into(),
            prior,
            to: to.into(),
        }
    }

    #[test]
    fn entry_point_solves_a_chain_but_reports_only_a_candidate() {
        let (store, project) = store_with_project();
        let summary = run_skest_search(
            &store,
            Some(&project),
            vec!["g0".into()],
            vec![edge("g2", "c", 1.0, "g1"), edge("g1", "c", 1.0, "g0")],
            "g2".into(),
            cfg(2, true),
            3,
        )
        .unwrap();

        assert_eq!(summary["solved"], json!(true));
        // The framing keys are what stop the ensemble's "solved" reading as proof.
        assert_eq!(summary["status"], json!("candidate"));
        assert_eq!(summary["formally_verified"], json!(false));
    }

    #[test]
    fn entry_point_emits_a_store_event() {
        let (store, project) = store_with_project();
        run_skest_search(
            &store,
            Some(&project),
            vec!["g0".into()],
            vec![edge("g1", "c", 1.0, "g0")],
            "g1".into(),
            cfg(2, true),
            1,
        )
        .unwrap();
        let events = store.events(&project, 100).unwrap();
        assert!(events.iter().any(|e| e.event_type == "skest.searched"));
    }

    #[test]
    fn entry_point_solves_a_subsumption_graph_and_stays_advisory() {
        // The trait-level reuse test relies on two trees with DIFFERENT edge sets
        // (one reaches the general fact, the other only the specific dead end), so
        // reuse is forced. The entry point deliberately shares ONE graph across all
        // trees, with heterogeneity coming only from the per-tree seed tie-break,
        // so it cannot reproduce that forced-reuse construction: every tree here
        // can reach the general goal directly. What this test pins is what the
        // entry point actually guarantees -- the ensemble solves, the reuse counter
        // is reported (whether or not it fired this seed), and nothing is dressed
        // up as formally verified. Forced cross-tree reuse is covered at the trait
        // level by subgoal_proved_by_one_tree_is_reused_by_the_winner.
        let (store, project) = store_with_project();
        let edges = vec![
            edge("R", "a", 1.0, "M"),
            edge("M", "a2", 1.0, "⊢ P"),
            edge("R", "b", 1.0, "H ⊢ P"),
        ];
        let summary = run_skest_search(
            &store,
            Some(&project),
            vec!["⊢ P".into()],
            edges,
            "R".into(),
            cfg(2, true),
            7,
        )
        .unwrap();
        assert_eq!(summary["solved"], json!(true));
        // Present and non-negative: a search outcome, not a proof of reuse.
        assert!(summary["cross_tree_reuses"].as_u64().is_some());
        assert_eq!(summary["formally_verified"], json!(false));
    }

    #[test]
    fn entry_point_unsolvable_graph_reports_not_solved_never_a_false_solve() {
        let (store, project) = store_with_project();
        // No goal is marked closed, so nothing can be solved.
        let summary = run_skest_search(
            &store,
            Some(&project),
            Vec::new(),
            vec![edge("g", "step", 0.5, "h")],
            "g".into(),
            cfg(3, true),
            5,
        )
        .unwrap();
        assert_eq!(summary["solved"], json!(false));
        assert_eq!(summary["winner_tree"], json!(null));
        assert_eq!(summary["formally_verified"], json!(false));
    }

    #[test]
    fn entry_point_rejects_empty_root_or_edge_endpoints() {
        let (store, project) = store_with_project();
        assert!(run_skest_search(
            &store,
            Some(&project),
            Vec::new(),
            Vec::new(),
            String::new(),
            cfg(2, true),
            0,
        )
        .is_err());
        assert!(run_skest_search(
            &store,
            Some(&project),
            Vec::new(),
            vec![edge("g", "t", 1.0, "")],
            "g".into(),
            cfg(2, true),
            0,
        )
        .is_err());
    }

    #[test]
    fn entry_point_is_deterministic_for_a_fixed_seed() {
        let (store, project) = store_with_project();
        let run = || {
            run_skest_search(
                &store,
                Some(&project),
                vec!["g0".into()],
                vec![edge("g2", "c", 1.0, "g1"), edge("g1", "c", 1.0, "g0")],
                "g2".into(),
                cfg(2, true),
                42,
            )
            .unwrap()
        };
        assert_eq!(run(), run());
    }
}
