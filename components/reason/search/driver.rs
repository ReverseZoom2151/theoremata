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

use super::critic_scorer::CriticScorer;
use super::mcts::{PriorMode, SearchConfig, SelectionMode};
use super::ttc::TtcController;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

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
    /// Whether the goal's **negation** was closed before the goal itself — i.e.
    /// the search found a *disproof* and the goal is false. Only ever `true` when
    /// negation-augmented search is enabled via
    /// [`ProofSearchDriver::with_negator`]; always `false` otherwise. A `refuted`
    /// result is never also `solved`.
    pub refuted: bool,
}

/// One node in the search DAG. Values live on the node (shared across all parents
/// that transpose into it), which is what makes the shared work shared.
struct DagNode<S> {
    state: S,
    closed: bool,
    progress: f64,
    /// Trained-critic V(s) in [0,1]; defaults to `progress` when no critic is
    /// injected, so `critic_weight = 0.0` leaves selection unchanged.
    critic: f64,
    visits: usize,
    value_sum: f64,
    edges: Vec<Edge>,
    expanded: bool,
}

impl<S> DagNode<S> {
    /// Mean backed-up reward `Q` (`0` when unvisited).
    fn mean(&self) -> f64 {
        if self.visits > 0 {
            self.value_sum / self.visits as f64
        } else {
            0.0
        }
    }
}

/// Upper/lower confidence bounds for a child: `mean ± c·√(ln N_parent / N_child)`.
/// An unvisited child has an infinitely wide interval (`+∞` UCB / `-∞` LCB), so it
/// is both the most optimistic action and the hardest subgoal until explored.
fn ucb_lcb(mean: f64, child_visits: usize, parent_visits: usize, c: f64) -> (f64, f64) {
    if child_visits == 0 {
        return (f64::INFINITY, f64::NEG_INFINITY);
    }
    let radius = c * ((parent_visits.max(1) as f64).ln().max(0.0) / child_visits as f64).sqrt();
    (mean + radius, mean - radius)
}

/// AND/OR minimax child selection (Aristotle, `docs/paper-mining/aristotle.md`).
///
/// Edges are grouped by tactic text into *actions* (an action's several edges are
/// its AND-children = the subgoals it produced). Pick the action whose best child
/// has the highest **UCB** (the optimistic OR choice over tactics), then within
/// that action descend into the child with the lowest **LCB** — the hardest
/// subgoal, the one most likely to block the whole action. Deterministic: ties
/// keep the first edge in encounter order.
fn and_or_select_child<S>(
    edges: &[Edge],
    nodes: &[DagNode<S>],
    parent_visits: usize,
    c: f64,
) -> Option<usize> {
    if edges.is_empty() {
        return None;
    }
    // Group edges by tactic, preserving first-seen order for determinism.
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut group_of: Vec<(String, usize)> = Vec::new();
    for e in edges {
        if let Some((_, gi)) = group_of.iter().find(|(t, _)| *t == e.tactic) {
            groups[*gi].push(e.child);
        } else {
            group_of.push((e.tactic.clone(), groups.len()));
            groups.push(vec![e.child]);
        }
    }

    // Highest-UCB action (OR): an action's optimism is its best child's UCB.
    let mut best_group: Option<&Vec<usize>> = None;
    let mut best_ucb = f64::NEG_INFINITY;
    for children in &groups {
        let mut group_ucb = f64::NEG_INFINITY;
        for &ci in children {
            let (ucb, _) = ucb_lcb(nodes[ci].mean(), nodes[ci].visits, parent_visits, c);
            if ucb > group_ucb {
                group_ucb = ucb;
            }
        }
        if group_ucb > best_ucb {
            best_ucb = group_ucb;
            best_group = Some(children);
        }
    }

    // Hardest subgoal (AND): lowest LCB within the chosen action.
    let children = best_group?;
    let mut best_child = None;
    let mut best_lcb = f64::INFINITY;
    for &ci in children {
        let (_, lcb) = ucb_lcb(nodes[ci].mean(), nodes[ci].visits, parent_visits, c);
        if lcb < best_lcb {
            best_lcb = lcb;
            best_child = Some(ci);
        }
    }
    best_child
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
    /// Optional negation seam: given a goal state, produce the state whose
    /// closure *disproves* the goal (the logical negation). When present, the
    /// driver runs negation-augmented search — a disproof competes for the same
    /// budget and, if it closes first, the search returns `refuted`.
    negator: Option<Box<dyn Fn(&E::State) -> Option<E::State>>>,
    /// Optional trained state-value critic, blended into PUCT selection via
    /// `cfg.critic_weight` (the [`super::critic_scorer`] seam). `None` ⇒ nodes
    /// fall back to their `progress` estimate.
    ///
    /// The critic is a **heuristic only**: it may reorder exploration and nothing
    /// else. It is never consulted when deciding whether a node is closed, whether
    /// the search is solved/refuted, or what the driver reports — those come solely
    /// from [`GoalState::is_closed`].
    critic: Option<Arc<dyn CriticScorer>>,
}

/// Read a node's state-value estimate, falling back to `progress`.
///
/// Two guards, both about not letting a bad critic damage the search:
/// * With no critic injected the estimate *is* `progress`, so the blended term is
///   whatever the progress term already contributed and nothing changes.
/// * A critic that returns a non-finite value (`NaN` / `±inf` — the only way an
///   erroring or untrained implementation can signal failure through an `f64`
///   return) is discarded in favour of `progress`. This matters because `NaN`
///   poisons every `score > best_score` comparison in the PUCT loop (all
///   comparisons with `NaN` are false), which would silently collapse selection to
///   "no child chosen" and truncate the descent. Degrading to today's signal is the
///   only safe failure mode. Finite values are clamped to the documented `[0, 1]`
///   contract so an out-of-range critic cannot dominate `q` and `u` outright.
fn critic_estimate<S: GoalState>(
    critic: Option<&Arc<dyn CriticScorer>>,
    state: &S,
    progress: f64,
) -> f64 {
    match critic {
        None => progress,
        Some(c) => {
            let v = c.score(state);
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                progress
            }
        }
    }
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
            negator: None,
            critic: None,
        }
    }

    /// Override the search budget / PUCT tuning.
    pub fn with_config(mut self, cfg: SearchConfig) -> Self {
        self.cfg = cfg;
        self
    }

    /// Inject a trained state-value critic. Combine with a non-zero
    /// `SearchConfig::critic_weight` to fold `V(s)` into PUCT selection.
    ///
    /// The critic only ever reorders *exploration*. It cannot close a node, cannot
    /// make a search `solved` or `refuted`, and never reaches any verdict path —
    /// those are decided exclusively by [`GoalState::is_closed`]. A wrong critic
    /// therefore costs search efficiency, never soundness. Default is no critic, in
    /// which case the driver behaves exactly as it did before this seam existed.
    ///
    /// The critic value reaches the search through exactly **two** places, both of
    /// them exploration-only:
    /// 1. the PUCT priority term, gated on `SearchConfig::critic_weight`; and
    /// 2. the eta-MCTS per-node expansion breadth, gated on
    ///    `SearchConfig::eta_mcts` (which reads `V(s)` as node importance and is
    ///    `None` by default).
    ///
    /// Both are off unless configured, so with no critic injected the driver is
    /// byte-identical for *every* config. Note the corollary for (2): with
    /// `eta_mcts` switched on, injecting a critic changes breadth even at
    /// `critic_weight == 0.0`, because eta-MCTS reads the critic directly rather
    /// than through the weight. That is deliberate and is pinned by
    /// `critic_reaches_expansion_breadth_only_under_eta_mcts`.
    pub fn with_critic(mut self, critic: Arc<dyn CriticScorer>) -> Self {
        self.critic = Some(critic);
        self
    }

    /// Attach a critic only if one was produced, otherwise leave the driver
    /// untouched. This is the production hookup: a construction site pairs it with
    /// [`super::critic_scorer::critic_from_config`], which returns `None` unless
    /// `SearchConfig::critic_weight` is non-zero. So a caller writes a single
    /// unconditional line and still gets byte-identical behaviour by default,
    /// because `None` here is a no-op and the injected-critic gate in
    /// [`run_attempt`](Self::run_attempt) forces `critic_weight` to zero whenever
    /// no critic is present.
    pub fn with_optional_critic(self, critic: Option<Arc<dyn CriticScorer>>) -> Self {
        match critic {
            Some(c) => self.with_critic(c),
            None => self,
        }
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

    /// Enable **negation-augmented search** (Aristotle's falsify-inside-the-budget).
    ///
    /// `negate(goal)` returns the state whose closure *disproves* `goal` (its
    /// logical negation), or `None` if the goal cannot be negated. With this set,
    /// every non-negation node in the DAG is augmented with an extra edge to its
    /// negation, so a disproof competes for the *same* search budget as the proof.
    /// If a negation node closes first, the search stops early and the
    /// [`DriverResult`] is `refuted` (and never `solved`). The seam is injectable
    /// so a real backend (negate the Lean goal) or a mock plugs into the same API.
    pub fn with_negator<F>(mut self, negate: F) -> Self
    where
        F: Fn(&E::State) -> Option<E::State> + 'static,
    {
        self.negator = Some(Box::new(negate));
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
        // Parallel to `nodes`: is this node on the *disproof* (negation) side?
        // A closed disproof node means the goal is refuted, not solved.
        let mut is_neg: Vec<bool> = Vec::new();

        let root_key = root.dedup_key();
        let root_closed = root.is_closed();
        let root_progress = root.progress();
        let root_critic = critic_estimate(self.critic.as_ref(), &root, root_progress);
        // The critic term is gated on a critic actually being injected, not merely
        // on the configured weight. Without one, `critic == progress`, so a
        // non-zero `critic_weight` would silently double-count the progress prior;
        // forcing the weight to zero keeps the no-critic path byte-identical to the
        // pre-seam driver for *every* config.
        let critic_weight = if self.critic.is_some() {
            self.cfg.critic_weight
        } else {
            0.0
        };
        nodes.push(DagNode {
            state: root,
            closed: root_closed,
            progress: root_progress,
            critic: root_critic,
            visits: 0,
            value_sum: 0.0,
            edges: Vec::new(),
            expanded: false,
        });
        is_neg.push(false);
        table.insert(root_key, 0);

        let mut solved = root_closed;
        let mut refuted = false;
        let mut dedup_hits = 0usize;
        let mut edges_created = 0usize;
        let mut iterations = 0usize;
        let node_cap = self.cfg.max_nodes.max(1);
        let base_seed = self.seed;

        for _ in 0..iter_budget.max(1) {
            if solved || refuted {
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
                let best_child = match self.cfg.selection {
                    SelectionMode::AndOrMinimax => and_or_select_child(
                        &nodes[current].edges,
                        &nodes,
                        nodes[current].visits,
                        self.cfg.exploration,
                    ),
                    SelectionMode::Puct => {
                        let n_parent = (nodes[current].visits.max(1) as f64).sqrt();
                        let mut chosen: Option<usize> = None;
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
                            // LeanProgress-style value prior, identical to mcts.rs,
                            // plus the critic as an ADDITIVE term (never a
                            // replacement). Ordering is therefore a pure function of
                            // the scores already stored on the nodes: no wall-clock,
                            // no rng, and no critic call inside the hot loop, so the
                            // determinism contract holds even for a critic that is
                            // expensive or (against its contract) unstable.
                            let score = super::critic_scorer::blend_priority(
                                q,
                                c.progress,
                                self.cfg.progress_weight,
                                c.critic,
                                critic_weight,
                                u,
                            );
                            if score > best_score {
                                best_score = score;
                                chosen = Some(e.child);
                            }
                        }
                        chosen
                    }
                };
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
                    let cur_is_neg = is_neg[current];
                    // Candidate priors: either the progress/heuristic weight each
                    // step carries (default) or Aristotle's empirical sampled-action
                    // distribution (frequency of each action across N samples).
                    let candidates = match self.cfg.prior_mode {
                        PriorMode::Progress => {
                            let seed = mix_seed(base_seed, &nodes[current].state.dedup_key());
                            self.expander.expand(&nodes[current].state, seed)
                        }
                        PriorMode::EmpiricalSampled(n) => Self::empirical_candidates(
                            &mut self.expander,
                            base_seed,
                            &nodes[current].state,
                            n.max(1),
                        ),
                    };
                    let mut edges = Vec::new();
                    // eta-MCTS: optionally size this node's expansion breadth by its
                    // critic (uncertainty) signal; `None` keeps the fixed `expand_k`.
                    // This is the critic's SECOND entry point, and it is deliberately
                    // not gated on `critic_weight`: that weight scales a PUCT term,
                    // whereas eta-MCTS consumes `V(s)` as a raw importance in [0, 1].
                    // With no critic injected `nodes[..].critic == progress`, which is
                    // the exact signal this branch used before the seam existed, so
                    // the no-critic path is unchanged. Breadth is still only *where
                    // to look*, never a verdict.
                    let expand_budget = match &self.cfg.eta_mcts {
                        Some(eta) => super::distance_critic::expansion_budget(
                            nodes[current].critic,
                            expand_k,
                            eta,
                        ),
                        None => expand_k,
                    };
                    for step in candidates.into_iter().take(expand_budget) {
                        let key = step.next.dedup_key();
                        let child = if let Some(&idx) = table.get(&key) {
                            // Transposition: two paths converge onto one node.
                            dedup_hits += 1;
                            if cur_is_neg {
                                is_neg[idx] = true;
                            }
                            idx
                        } else {
                            if nodes.len() >= node_cap {
                                // Node cap reached: stop minting new nodes but keep
                                // any edges into already-known states.
                                continue;
                            }
                            let closed = step.next.is_closed();
                            let progress = step.next.progress();
                            let critic =
                                critic_estimate(self.critic.as_ref(), &step.next, progress);
                            let idx = nodes.len();
                            nodes.push(DagNode {
                                state: step.next,
                                closed,
                                progress,
                                critic,
                                visits: 0,
                                value_sum: 0.0,
                                edges: Vec::new(),
                                expanded: false,
                            });
                            // A subgoal inherits the disproof-side flag of its parent.
                            is_neg.push(cur_is_neg);
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

                    // Negation augmentation: give every proof-side state an extra
                    // edge to its logical negation so a disproof competes for the
                    // same budget. Never negate a disproof node (no double negation).
                    if !cur_is_neg {
                        if let Some(neg_state) =
                            self.negator.as_ref().and_then(|f| f(&nodes[current].state))
                        {
                            let key = neg_state.dedup_key();
                            let child = if let Some(&idx) = table.get(&key) {
                                dedup_hits += 1;
                                is_neg[idx] = true;
                                Some(idx)
                            } else if nodes.len() >= node_cap {
                                None
                            } else {
                                let closed = neg_state.is_closed();
                                let progress = neg_state.progress();
                                let critic =
                                    critic_estimate(self.critic.as_ref(), &neg_state, progress);
                                let idx = nodes.len();
                                nodes.push(DagNode {
                                    state: neg_state,
                                    closed,
                                    progress,
                                    critic,
                                    visits: 0,
                                    value_sum: 0.0,
                                    edges: Vec::new(),
                                    expanded: false,
                                });
                                is_neg.push(true);
                                table.insert(key, idx);
                                Some(idx)
                            };
                            if let Some(child) = child {
                                edges.push(Edge {
                                    tactic: "¬goal (disproof)".into(),
                                    prior: 1.0,
                                    child,
                                });
                                edges_created += 1;
                            }
                        }
                    }

                    nodes[current].edges = edges;
                    nodes[current].expanded = true;
                }
                let start = nodes[current].state.clone();
                Self::rollout(&mut self.expander, base_seed, &start, max_depth)
            };

            if leaf_reward >= 1.0 {
                // A closed leaf on the disproof side means the goal is refuted;
                // on the proof side it means solved.
                if is_neg[current] {
                    refuted = true;
                } else {
                    solved = true;
                }
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
            // The injected disproof edge is not a proof tactic — keep it out of the
            // distilled proof policy / robust-child choice.
            if is_neg[e.child] {
                continue;
            }
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
            refuted,
        }
    }

    /// Aristotle's empirical sampled-action prior: sample the expander `n` times
    /// (each with a distinct, deterministically-derived seed) and set each
    /// action's prior to the frequency with which it was drawn — **not** a fixed
    /// heuristic weight. Actions keep first-seen order (deterministic), and priors
    /// sum to `1`. The representative `next` state is taken from the first sample
    /// that produced the action.
    fn empirical_candidates(
        expander: &mut E,
        base_seed: u64,
        state: &E::State,
        n: usize,
    ) -> Vec<TacticStep<E::State>> {
        let base_key = state.dedup_key();
        let mut order: Vec<String> = Vec::new();
        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut rep: HashMap<String, TacticStep<E::State>> = HashMap::new();
        for i in 0..n {
            let seed = mix_seed(base_seed, &format!("{base_key}#emp#{i}"));
            for step in expander.expand(state, seed) {
                let t = step.tactic.clone();
                if !counts.contains_key(&t) {
                    order.push(t.clone());
                    rep.insert(t.clone(), step);
                }
                *counts.entry(t).or_insert(0) += 1;
            }
        }
        let total: usize = counts.values().sum();
        let mut out = Vec::new();
        if total == 0 {
            return out;
        }
        for t in order {
            let count = counts[&t];
            let mut step = rep
                .remove(&t)
                .expect("representative step for sampled action");
            step.prior = count as f64 / total as f64;
            out.push(step);
        }
        out
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
    use super::super::ttc::TtcConfig;
    use super::*;

    /// A deterministic, table-driven proof state: `key` identifies the state,
    /// `closed` marks proof completion. This is the injectable mock backing every
    /// test — no randomness, everything is a pure function of the table.
    #[derive(Clone)]
    struct MockGoal {
        key: String,
        closed: bool,
        difficulty: f64,
        /// The LeanProgress-style prior. Defaults to `0.0`, matching the
        /// [`GoalState::progress`] default, so tests that do not set it are
        /// unaffected. Tests that need the progress term to actually participate in
        /// the priority arithmetic (the golden) set it explicitly: with progress
        /// pinned at `0` for every state, `progress_weight` multiplies zero and the
        /// selection formula is untestable.
        progress: f64,
    }

    impl MockGoal {
        fn open(key: &str) -> Self {
            Self {
                key: key.into(),
                closed: false,
                difficulty: 0.5,
                progress: 0.0,
            }
        }
        fn closed(key: &str) -> Self {
            Self {
                key: key.into(),
                closed: true,
                difficulty: 0.5,
                progress: 0.0,
            }
        }
        fn with_difficulty(mut self, d: f64) -> Self {
            self.difficulty = d;
            self
        }
        fn with_progress(mut self, p: f64) -> Self {
            self.progress = p;
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
        fn progress(&self) -> f64 {
            self.progress
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
        let mut driver = ProofSearchDriver::new(expander).with_seed(3).with_ttc(ttc);

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

    // ---- Feature 1: negation-augmented search (falsify inside the budget) ----

    #[test]
    fn negation_that_closes_returns_refuted() {
        // The goal "G" itself never closes (no proving edges). Its negation "¬G"
        // closes in one step. With a negator wired in, the disproof competes for
        // the same budget and the search returns `refuted` (goal is false), never
        // `solved`.
        let expander = TableExpander::new()
            // The negated goal proves: ¬G -> closed.
            .edge("not:G", "close", 1.0, MockGoal::closed("false-witness"));
        let mut driver = ProofSearchDriver::new(expander)
            .with_seed(5)
            .with_negator(|g: &MockGoal| Some(MockGoal::open(&format!("not:{}", g.key))));
        let result = driver.run(MockGoal::open("G"));
        assert!(
            result.refuted,
            "the disproof should close and refute the goal"
        );
        assert!(!result.solved, "a refuted goal must never also be solved");
    }

    #[test]
    fn unclosable_negation_leaves_proving_unaffected() {
        // The goal "p2" proves normally (p2 -> p1 -> p0 closed). Its negation
        // never closes (no edges out of any "not:*"). Enabling the negator must
        // not disturb the proof: the goal is solved, not refuted.
        let expander = TableExpander::new()
            .edge("p2", "close", 1.0, MockGoal::open("p1"))
            .edge("p1", "close", 1.0, MockGoal::closed("p0"));
        let mut driver = ProofSearchDriver::new(expander)
            .with_seed(9)
            .with_negator(|g: &MockGoal| Some(MockGoal::open(&format!("not:{}", g.key))));
        let result = driver.run(MockGoal::open("p2"));
        assert!(
            result.solved,
            "an unclosable negation must not block proving"
        );
        assert!(!result.refuted);
        assert_eq!(result.best_tactic.as_deref(), Some("close"));
    }

    #[test]
    fn no_negator_never_refutes() {
        // Without a negator the search behaves exactly as before: refuted stays
        // false even for an unprovable goal.
        let expander = TableExpander::new().edge("q", "stuck", 1.0, MockGoal::open("q"));
        let mut driver = ProofSearchDriver::new(expander)
            .with_seed(1)
            .with_config(SearchConfig {
                max_nodes: 20,
                ..SearchConfig::default()
            });
        let result = driver.run(MockGoal::open("q"));
        assert!(!result.refuted);
        assert!(!result.solved);
    }

    // ---- Feature 2: AND/OR minimax selection ----

    /// Build a leaf DagNode with crafted statistics for selection tests.
    fn stat_node(key: &str, visits: usize, value_sum: f64) -> DagNode<MockGoal> {
        DagNode {
            state: MockGoal::open(key),
            closed: false,
            progress: 0.0,
            critic: 0.0,
            visits,
            value_sum,
            edges: Vec::new(),
            expanded: false,
        }
    }

    #[test]
    fn and_or_selection_picks_highest_ucb_action_then_lowest_lcb_child() {
        // Nodes (index = position): two actions.
        //  action "A": children 1 (mean 0.9, high) and 2 (mean 0.2, hard).
        //  action "B": children 3 (mean 0.5) and 4 (mean 0.5).
        // Same visit count so bounds differ only by mean. Action A has the highest
        // UCB (via child 1's high mean), so it is chosen; within A the lowest-LCB
        // (hardest) child is 2.
        let nodes = vec![
            stat_node("root", 100, 0.0),  // 0: parent (visits used as N_parent)
            stat_node("a-easy", 10, 9.0), // 1: mean 0.9
            stat_node("a-hard", 10, 2.0), // 2: mean 0.2
            stat_node("b1", 10, 5.0),     // 3: mean 0.5
            stat_node("b2", 10, 5.0),     // 4: mean 0.5
        ];
        let edges = vec![
            Edge {
                tactic: "A".into(),
                prior: 1.0,
                child: 1,
            },
            Edge {
                tactic: "A".into(),
                prior: 1.0,
                child: 2,
            },
            Edge {
                tactic: "B".into(),
                prior: 1.0,
                child: 3,
            },
            Edge {
                tactic: "B".into(),
                prior: 1.0,
                child: 4,
            },
        ];
        let chosen = and_or_select_child(&edges, &nodes, nodes[0].visits, 1.41);
        assert_eq!(
            chosen,
            Some(2),
            "must descend into action A's hardest child"
        );
    }

    #[test]
    fn and_or_bounds_order_correctly() {
        // A higher-mean child has a higher UCB and a higher LCB than a lower-mean
        // child at equal visits — the confidence interval just shifts with the mean.
        let (hi_u, hi_l) = ucb_lcb(0.8, 10, 100, 1.41);
        let (lo_u, lo_l) = ucb_lcb(0.2, 10, 100, 1.41);
        assert!(hi_u > lo_u);
        assert!(hi_l > lo_l);
        // Unvisited children have an unbounded interval.
        let (u0, l0) = ucb_lcb(0.0, 0, 100, 1.41);
        assert!(u0.is_infinite() && u0 > 0.0);
        assert!(l0.is_infinite() && l0 < 0.0);
    }

    #[test]
    fn and_or_minimax_mode_still_solves() {
        // The new selection mode is a drop-in: a solvable chain still solves.
        let expander = TableExpander::new()
            .edge("m2", "close", 1.0, MockGoal::open("m1"))
            .edge("m1", "close", 1.0, MockGoal::closed("m0"));
        let mut driver = ProofSearchDriver::new(expander)
            .with_seed(2)
            .with_config(SearchConfig {
                selection: SelectionMode::AndOrMinimax,
                ..SearchConfig::default()
            });
        let result = driver.run(MockGoal::open("m2"));
        assert!(result.solved);
    }

    // ---- Feature 3: empirical sampled-action PUCT prior ----

    /// A seed-sensitive expander: action "a" is always sampled; action "b" is only
    /// sampled when the seed is divisible by 3. Over many samples "a" is drawn far
    /// more often than "b", so its empirical prior must be higher.
    struct SampledExpander;
    impl TacticExpander for SampledExpander {
        type State = MockGoal;
        fn expand(&mut self, state: &MockGoal, seed: u64) -> Vec<TacticStep<MockGoal>> {
            if state.key != "root" {
                return Vec::new();
            }
            let mut steps = vec![TacticStep::new("a", 0.5, MockGoal::open("sa"))];
            if seed % 3 == 0 {
                steps.push(TacticStep::new("b", 0.5, MockGoal::open("sb")));
            }
            steps
        }
    }

    #[test]
    fn empirical_prior_reflects_sample_frequency_and_normalizes() {
        let mut expander = SampledExpander;
        let steps = ProofSearchDriver::<SampledExpander>::empirical_candidates(
            &mut expander,
            123,
            &MockGoal::open("root"),
            120,
        );
        // Both actions appear.
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].tactic, "a");
        assert_eq!(steps[1].tactic, "b");
        // "a" (always sampled) has a strictly higher empirical prior than "b".
        assert!(
            steps[0].prior > steps[1].prior,
            "more-sampled action must get the higher prior ({} vs {})",
            steps[0].prior,
            steps[1].prior
        );
        // Priors normalize to 1.
        let sum: f64 = steps.iter().map(|s| s.prior).sum();
        assert!((sum - 1.0).abs() < 1e-9, "priors must sum to 1 (got {sum})");
    }

    #[test]
    fn empirical_prior_mode_is_deterministic_and_solves() {
        // Two runs with the same seed under the empirical prior mode agree, and a
        // solvable chain still solves.
        let build = || {
            TableExpander::new()
                .edge("e2", "close", 1.0, MockGoal::open("e1"))
                .edge("e1", "close", 1.0, MockGoal::closed("e0"))
        };
        let cfg = SearchConfig {
            prior_mode: PriorMode::EmpiricalSampled(8),
            ..SearchConfig::default()
        };
        let mut d1 = ProofSearchDriver::new(build())
            .with_seed(4)
            .with_config(cfg);
        let mut d2 = ProofSearchDriver::new(build())
            .with_seed(4)
            .with_config(cfg);
        let r1 = d1.run(MockGoal::open("e2"));
        let r2 = d2.run(MockGoal::open("e2"));
        assert!(r1.solved);
        assert_eq!(r1.visit_counts, r2.visit_counts);
    }

    // ---- Feature 4: trained state-value critic folded into PUCT selection ----

    use super::super::critic_scorer::{ConstantCritic, CriticScorer, GoalStateLike};

    /// A critic that rates one named state as maximally close to done and every
    /// other state as maximally far. Deterministic (a pure function of the state
    /// text), as the [`CriticScorer`] contract requires.
    struct KeyCritic(&'static str);
    impl CriticScorer for KeyCritic {
        fn score(&self, state: &dyn GoalStateLike) -> f64 {
            if state.state_text() == self.0 {
                1.0
            } else {
                0.0
            }
        }
    }

    /// A critic that rates every state identically, whatever its value. Stands in
    /// for an untrained head that has collapsed onto a single output.
    struct FlatCritic(f64);
    impl CriticScorer for FlatCritic {
        fn score(&self, _state: &dyn GoalStateLike) -> f64 {
            self.0
        }
    }

    /// A broken critic: `NaN` is the only way a failing implementation can signal
    /// "no estimate" through this trait's `f64` return, and it is also the value
    /// that would poison PUCT's comparisons if it were used raw.
    struct BrokenCritic;
    impl CriticScorer for BrokenCritic {
        fn score(&self, _state: &dyn GoalStateLike) -> f64 {
            f64::NAN
        }
    }

    /// A critic that violates the `[0, 1]` contract with a huge magnitude — enough
    /// to swamp `q` and `u` entirely if it were not clamped.
    struct OutOfRangeCritic;
    impl CriticScorer for OutOfRangeCritic {
        fn score(&self, _state: &dyn GoalStateLike) -> f64 {
            1e18
        }
    }

    /// `DriverResult` has no `PartialEq`, so compare every observable field: this
    /// is the "byte-identical behaviour" check the critic seam must satisfy.
    fn assert_identical(a: &DriverResult, b: &DriverResult, what: &str) {
        assert_eq!(a.solved, b.solved, "{what}: solved");
        assert_eq!(a.refuted, b.refuted, "{what}: refuted");
        assert_eq!(a.best_tactic, b.best_tactic, "{what}: best_tactic");
        assert_eq!(a.root_visits, b.root_visits, "{what}: root_visits");
        assert_eq!(a.iterations, b.iterations, "{what}: iterations");
        assert_eq!(a.nodes_created, b.nodes_created, "{what}: nodes_created");
        assert_eq!(a.edges_created, b.edges_created, "{what}: edges_created");
        assert_eq!(a.dedup_hits, b.dedup_hits, "{what}: dedup_hits");
        assert_eq!(a.visit_counts, b.visit_counts, "{what}: visit_counts");
    }

    /// A root with two equally-prior tactics leading to two distinct dead ends.
    /// Nothing closes, so the search spends its whole budget deciding *where to
    /// look* — which makes the visit split a direct readout of exploration order.
    fn fork_expander() -> TableExpander {
        TableExpander::new()
            .edge("R", "tx", 0.5, MockGoal::open("x"))
            .edge("R", "ty", 0.5, MockGoal::open("y"))
    }

    fn fork_cfg(critic_weight: f64) -> SearchConfig {
        SearchConfig {
            max_nodes: 50,
            critic_weight,
            ..SearchConfig::default()
        }
    }

    /// Run the fork scenario, optionally with a critic injected.
    fn run_fork(critic: Option<Arc<dyn CriticScorer>>, critic_weight: f64) -> DriverResult {
        let mut driver = ProofSearchDriver::new(fork_expander())
            .with_seed(11)
            .with_config(fork_cfg(critic_weight));
        if let Some(c) = critic {
            driver = driver.with_critic(c);
        }
        driver.run(MockGoal::open("R"))
    }

    fn visits_of(result: &DriverResult, tactic: &str) -> usize {
        result
            .visit_counts
            .iter()
            .find(|(t, _)| t == tactic)
            .map(|(_, v)| *v)
            .expect("root tactic present in visit counts")
    }

    /// A branching lattice with two transpositions and no closed state anywhere, so
    /// the search spends its entire budget on *exploration order*. Every observable
    /// field of the result (visit split, node/edge counts, dedup hits) is therefore
    /// a fingerprint of the selection arithmetic: any drift in the priority formula
    /// moves at least one of them.
    /// Distinct per-state progress values so `progress_weight · progress` is a live
    /// term, and a deliberate tie between the two children of `C` so the stable
    /// first-edge tie-break is exercised too.
    fn lattice_goal(key: &str) -> MockGoal {
        let p = match key {
            "B" => 0.30,
            "C" => 0.55,
            "D" => 0.40,
            "E" => 0.65,
            // F ties D exactly, and C's two edges tie on prior, so C's children are
            // fully indistinguishable and the stable first-edge tie-break decides
            // which is descended into. (The tie-break rule itself is pinned by
            // `injected_critic_reorders_exploration`, which reads it off the root.)
            "F" => 0.40,
            "G" => 0.80,
            "H" => 0.20,
            _ => 0.10,
        };
        MockGoal::open(key).with_progress(p)
    }

    fn lattice_expander() -> TableExpander {
        TableExpander::new()
            .edge("A", "l", 0.6, lattice_goal("B"))
            .edge("A", "r", 0.4, lattice_goal("C"))
            .edge("B", "d1", 0.7, lattice_goal("D"))
            .edge("B", "d2", 0.3, lattice_goal("E"))
            .edge("C", "d3", 0.5, lattice_goal("D"))
            .edge("C", "d4", 0.5, lattice_goal("F"))
            .edge("D", "x", 1.0, lattice_goal("G"))
            .edge("E", "y", 1.0, lattice_goal("G"))
            .edge("F", "z", 1.0, lattice_goal("H"))
    }

    fn lattice_cfg() -> SearchConfig {
        SearchConfig {
            max_nodes: 32,
            expand_k: 3,
            max_depth: 6,
            ..SearchConfig::default()
        }
    }

    #[test]
    fn no_critic_path_matches_the_pinned_golden() {
        // THE load-bearing regression test for this seam. `driver.rs` is the main
        // search path, so "the no-critic path still works" is not enough: a silent
        // reordering would pass that and change every downstream proof search.
        // These literals were recorded from the driver and are pinned by hand, so a
        // change to the priority formula that leaks into the no-critic path fails
        // here loudly instead of being invisible.
        let mut driver = ProofSearchDriver::new(lattice_expander())
            .with_seed(11)
            .with_config(lattice_cfg());
        let r = driver.run(MockGoal::open("A"));

        assert!(!r.solved, "golden scenario has no closed state");
        assert!(!r.refuted);
        assert_eq!(r.best_tactic.as_deref(), Some("r"));
        assert_eq!(r.root_visits, 32);
        assert_eq!(r.iterations, 32);
        assert_eq!(r.nodes_created, 8, "distinct states A,B,C,D,E,F,G,H");
        assert_eq!(r.edges_created, 9);
        assert_eq!(r.dedup_hits, 2, "C->D and E->G both transpose");
        assert_eq!(
            r.visit_counts,
            vec![("l".to_string(), 15), ("r".to_string(), 16)],
            "the exact exploration split is the fingerprint of the priority formula"
        );
    }

    #[test]
    fn no_critic_leaves_the_driver_byte_identical() {
        // The seam is default-OFF: with no critic injected the driver must produce
        // exactly the pre-seam result. The strongest available statement of that is
        // that the result does not depend on `critic_weight` AT ALL when no critic
        // exists — the term cannot be reached, so no config can perturb the main
        // search path by accident.
        let baseline = run_fork(None, 0.0);
        for weight in [0.0, 1.0, 25.0, -3.0] {
            let other = run_fork(None, weight);
            assert_identical(
                &baseline,
                &other,
                &format!("no critic injected, critic_weight={weight}"),
            );
        }

        // The same must hold on the richer golden scenario, and there it must also
        // hold with eta-MCTS switched on: that branch reads the per-node critic
        // value directly, so it is the one place where a no-critic run could have
        // drifted without the weight being involved at all.
        let base = |cfg: SearchConfig| {
            ProofSearchDriver::new(lattice_expander())
                .with_seed(11)
                .with_config(cfg)
                .run(MockGoal::open("A"))
        };
        let lattice_baseline = base(lattice_cfg());
        for weight in [0.0, 3.0, 100.0] {
            for eta in [
                None,
                Some(super::super::distance_critic::EtaMctsConfig::default()),
            ] {
                assert_identical(
                    &lattice_baseline,
                    &base(SearchConfig {
                        critic_weight: weight,
                        eta_mcts: eta,
                        ..lattice_cfg()
                    }),
                    &format!("no critic injected, weight={weight} eta={}", eta.is_some()),
                );
            }
        }

        // Injecting a critic while leaving the weight at its `0.0` default is
        // likewise inert *for the PUCT term*, which is the only path `eta_mcts`
        // (`None` by default) leaves open here. See
        // `critic_reaches_expansion_breadth_only_under_eta_mcts` for the one case
        // where injection alone is observable.
        let inert = run_fork(Some(Arc::new(KeyCritic("y"))), 0.0);
        assert_identical(&baseline, &inert, "critic injected at weight 0");

        // And the no-critic path is still reproducible run to run.
        assert_identical(&baseline, &run_fork(None, 0.0), "repeat of the baseline");
    }

    #[test]
    fn critic_reaches_expansion_breadth_only_under_eta_mcts() {
        // Honest pinning of the critic's second entry point. eta-MCTS sizes a node's
        // expansion breadth from its `V(s)`, so with `eta_mcts` enabled an injected
        // critic is observable even at `critic_weight == 0.0`. This is exploration
        // breadth only, but it is a real behaviour change, so it is asserted rather
        // than glossed as "inert".
        let wide = || {
            TableExpander::new()
                .edge("W", "t1", 0.5, MockGoal::open("s1"))
                .edge("W", "t2", 0.5, MockGoal::open("s2"))
                .edge("W", "t3", 0.5, MockGoal::open("s3"))
                .edge("W", "t4", 0.5, MockGoal::open("s4"))
                .edge("W", "t5", 0.5, MockGoal::open("s5"))
        };
        // `expand_k = 2` so the fixed budget genuinely truncates the five candidates.
        let eta_cfg = SearchConfig {
            max_nodes: 32,
            expand_k: 2,
            max_depth: 6,
            eta_mcts: Some(super::super::distance_critic::EtaMctsConfig::default()),
            ..SearchConfig::default()
        };
        let run = |critic: Option<Arc<dyn CriticScorer>>, cfg: SearchConfig| {
            let mut d = ProofSearchDriver::new(wide())
                .with_seed(11)
                .with_config(cfg);
            if let Some(c) = critic {
                d = d.with_critic(c);
            }
            d.run(MockGoal::open("W"))
        };

        // No critic: breadth stays at the fixed `expand_k`, i.e. exactly today's
        // behaviour. This is the property that must never break.
        let no_critic = run(None, eta_cfg);
        assert_eq!(no_critic.edges_created, 2, "no critic, so fixed expand_k");

        // A maximally-confident critic widens the node, at weight 0.
        let widened = run(Some(Arc::new(FlatCritic(1.0))), eta_cfg);
        assert!(
            widened.edges_created > no_critic.edges_created,
            "eta-MCTS must widen a high-V node ({} vs {})",
            widened.edges_created,
            no_critic.edges_created
        );

        // With `eta_mcts` off (the default) the same injection is fully inert.
        let plain = SearchConfig {
            eta_mcts: None,
            ..eta_cfg
        };
        assert_identical(
            &run(None, plain),
            &run(Some(Arc::new(FlatCritic(1.0))), plain),
            "without eta_mcts, a weight-0 critic changes nothing",
        );
    }

    #[test]
    fn critic_is_consulted_only_when_a_node_is_created() {
        // The determinism contract says ordering is a pure function of scores the
        // driver ALREADY holds. That is only true if the critic is never called from
        // inside the selection loop, where the number of calls would depend on the
        // traversal. Pin it: exactly one call per node minted, budget-independent.
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingCritic(AtomicUsize);
        impl CriticScorer for CountingCritic {
            fn score(&self, _state: &dyn GoalStateLike) -> f64 {
                self.0.fetch_add(1, Ordering::SeqCst);
                // A constant, so the search shape matches the no-critic run and the
                // call count is attributable to node creation alone.
                0.5
            }
        }

        for budget in [8usize, 32, 64] {
            let critic = Arc::new(CountingCritic(AtomicUsize::new(0)));
            let mut driver = ProofSearchDriver::new(lattice_expander())
                .with_seed(11)
                .with_config(SearchConfig {
                    max_nodes: budget,
                    critic_weight: 2.0,
                    ..lattice_cfg()
                })
                .with_critic(critic.clone());
            let r = driver.run(MockGoal::open("A"));
            assert_eq!(
                critic.0.load(Ordering::SeqCst),
                r.nodes_created,
                "one critic call per node created (budget {budget}), never per selection step"
            );
        }
    }

    #[test]
    fn injected_critic_reorders_exploration() {
        // Baseline: the two tactics are indistinguishable, so deterministic
        // tie-breaking keeps the first-seen edge ahead.
        let baseline = run_fork(None, 0.0);
        assert!(
            visits_of(&baseline, "tx") >= visits_of(&baseline, "ty"),
            "without a critic, ties must resolve to the first edge ({:?})",
            baseline.visit_counts
        );

        // With a critic that rates "y" as nearly proved, exploration must swing to
        // the "ty" branch. This is the entire point of the seam.
        let guided = run_fork(Some(Arc::new(KeyCritic("y"))), 1.0);
        assert!(
            visits_of(&guided, "ty") > visits_of(&guided, "tx"),
            "the critic-preferred branch must be explored more ({:?})",
            guided.visit_counts
        );
        assert_ne!(
            baseline.visit_counts, guided.visit_counts,
            "the critic must actually change the exploration order"
        );

        // Pointing the critic at the other branch swings it back, so the effect
        // tracks the critic and is not an artifact of enabling the weight.
        let flipped = run_fork(Some(Arc::new(KeyCritic("x"))), 1.0);
        assert!(
            visits_of(&flipped, "tx") > visits_of(&flipped, "ty"),
            "the effect must follow the critic's preference ({:?})",
            flipped.visit_counts
        );

        // Same critic, same seed, same result: the ordering logic stays a pure
        // function of the scores it is handed.
        assert_identical(
            &guided,
            &run_fork(Some(Arc::new(KeyCritic("y"))), 1.0),
            "guided search is reproducible",
        );
    }

    #[test]
    fn constant_critic_does_not_degenerate_the_frontier() {
        // A critic stuck on one output (an untrained head) is the dangerous case: if
        // the critic REPLACED the priority it would flatten every child to the same
        // score and destroy the frontier. Because it is folded in as an additive
        // term, a constant shifts all siblings equally and the ordering is exactly
        // the no-critic ordering — a useless critic costs nothing.
        let baseline = run_fork(None, 0.0);
        for value in [0.0, 0.5, 0.7, 1.0] {
            for weight in [1.0, 10.0] {
                let flat = run_fork(Some(Arc::new(FlatCritic(value))), weight);
                assert_identical(
                    &baseline,
                    &flat,
                    &format!("constant critic value={value} weight={weight}"),
                );
            }
        }

        // `ConstantCritic` (the shipped test double) behaves the same way.
        assert_identical(
            &baseline,
            &run_fork(Some(Arc::new(ConstantCritic(0.42))), 5.0),
            "ConstantCritic must not reshape the frontier",
        );

        // An out-of-contract magnitude is clamped into [0, 1] before it is blended,
        // so it degenerates to the constant case instead of swamping q and u.
        assert_identical(
            &baseline,
            &run_fork(Some(Arc::new(OutOfRangeCritic)), 1.0),
            "an out-of-range critic is clamped, not allowed to dominate",
        );

        // The fork is a deliberate tie, so repeat the check on the lattice, where
        // the no-critic ordering is genuinely non-trivial (distinct priors and
        // distinct progress values). Flattening THAT frontier would be visible.
        let run_lattice = |critic: Option<Arc<dyn CriticScorer>>, weight: f64| {
            let mut d = ProofSearchDriver::new(lattice_expander())
                .with_seed(11)
                .with_config(SearchConfig {
                    critic_weight: weight,
                    ..lattice_cfg()
                });
            if let Some(c) = critic {
                d = d.with_critic(c);
            }
            d.run(MockGoal::open("A"))
        };
        let lattice_baseline = run_lattice(None, 0.0);
        for value in [0.0, 0.5, 1.0] {
            for weight in [1.0, 10.0] {
                assert_identical(
                    &lattice_baseline,
                    &run_lattice(Some(Arc::new(FlatCritic(value))), weight),
                    &format!("constant critic must preserve a non-trivial frontier (value={value} weight={weight})"),
                );
            }
        }
    }

    #[test]
    fn erroring_critic_degrades_to_the_no_critic_behaviour() {
        // A critic that cannot produce an estimate returns NaN. Used raw it would
        // make every `score > best_score` comparison false and silently truncate
        // selection; discarded in favour of `progress` it costs nothing at all.
        let baseline = run_fork(None, 0.0);
        assert_identical(
            &baseline,
            &run_fork(Some(Arc::new(BrokenCritic)), 1.0),
            "a NaN critic must fall back to today's signal",
        );

        // Non-finite infinities are handled by the same guard.
        struct InfCritic(f64);
        impl CriticScorer for InfCritic {
            fn score(&self, _state: &dyn GoalStateLike) -> f64 {
                self.0
            }
        }
        for v in [f64::INFINITY, f64::NEG_INFINITY] {
            assert_identical(
                &baseline,
                &run_fork(Some(Arc::new(InfCritic(v))), 1.0),
                "an infinite critic must fall back to today's signal",
            );
        }

        // A broken critic must not cost the search a proof it would otherwise find.
        let expander = TableExpander::new()
            .edge("b2", "close", 1.0, MockGoal::open("b1"))
            .edge("b1", "close", 1.0, MockGoal::closed("b0"));
        let mut driver = ProofSearchDriver::new(expander)
            .with_seed(6)
            .with_config(SearchConfig {
                critic_weight: 4.0,
                ..SearchConfig::default()
            })
            .with_critic(Arc::new(BrokenCritic));
        let result = driver.run(MockGoal::open("b2"));
        assert!(result.solved, "a broken critic must not break proving");
    }

    #[test]
    fn critic_never_influences_closed_or_accepted_determinations() {
        // A critic is a heuristic, never a verdict. Closure comes from
        // `GoalState::is_closed` alone, so the most confident possible critic cannot
        // manufacture a proof and the most pessimistic cannot deny one.

        // 1. Maximum confidence on an unprovable goal: still not solved.
        let stuck = TableExpander::new().edge("q", "stuck", 1.0, MockGoal::open("q"));
        let mut optimist = ProofSearchDriver::new(stuck)
            .with_seed(1)
            .with_config(SearchConfig {
                max_nodes: 20,
                critic_weight: 100.0,
                ..SearchConfig::default()
            })
            .with_critic(Arc::new(ConstantCritic(1.0)));
        let result = optimist.run(MockGoal::open("q"));
        assert!(
            !result.solved,
            "a critic claiming V=1 everywhere must not close an unprovable goal"
        );
        assert!(!result.refuted, "nor may it manufacture a refutation");

        // 2. Minimum confidence on a provable goal: still solved. The critic can
        //    only make the search look elsewhere first; it cannot reject a proof.
        let chain = TableExpander::new()
            .edge("c2", "close", 1.0, MockGoal::open("c1"))
            .edge("c1", "close", 1.0, MockGoal::closed("c0"));
        let mut pessimist = ProofSearchDriver::new(chain)
            .with_seed(1)
            .with_config(SearchConfig {
                critic_weight: 100.0,
                ..SearchConfig::default()
            })
            .with_critic(Arc::new(ConstantCritic(0.0)));
        let result = pessimist.run(MockGoal::open("c2"));
        assert!(
            result.solved,
            "a critic claiming V=0 everywhere must not block a real proof"
        );

        // 3. The disproof verdict is equally out of reach: a critic that loves the
        //    negation side cannot turn a provable goal into a refuted one, because
        //    `refuted` is gated on a negation node genuinely closing.
        let expander = TableExpander::new()
            .edge("n2", "close", 1.0, MockGoal::open("n1"))
            .edge("n1", "close", 1.0, MockGoal::closed("n0"));
        let mut driver = ProofSearchDriver::new(expander)
            .with_seed(9)
            .with_config(SearchConfig {
                critic_weight: 100.0,
                ..SearchConfig::default()
            })
            .with_critic(Arc::new(KeyCritic("not:n2")))
            .with_negator(|g: &MockGoal| Some(MockGoal::open(&format!("not:{}", g.key))));
        let result = driver.run(MockGoal::open("n2"));
        assert!(!result.refuted, "an unclosable negation stays unrefuted");
        assert!(result.solved, "and the real proof is still found");
    }
}
