//! Value-free **best-first** proof search + a **DPO preference-pair extractor**
//! (the BFS-Prover pattern, `docs/paper-mining/` / prover-mining adopt-list).
//!
//! The MCGS driver ([`super::driver`]) runs PUCT with transposition and a
//! process-reward / value prior. That is powerful but needs a *value* signal
//! (progress estimates, rollouts) to steer selection. BFS-Prover shows a second,
//! **value-free** mode is competitive and much simpler: expand states in order of
//! their **length-normalized cumulative path log-probability** under the policy,
//! with *no* value network, *no* rollouts, and *no* backpropagation. A single
//! global priority queue holds the whole frontier; the best-scoring partial proof
//! is always expanded next, until the goal closes or a step budget is hit.
//!
//! This module adds exactly that on top of the driver's existing abstractions:
//!
//! * It reuses [`GoalState`](super::driver::GoalState) unchanged — a state still
//!   knows its [`dedup_key`](super::driver::GoalState::dedup_key) and whether it
//!   [`is_closed`](super::driver::GoalState::is_closed).
//! * The tactic *scorer* is the injected seam. In the real system it is the policy
//!   LLM returning `(tactic, logprob)` candidates for a state (the GPU-gated part);
//!   here it is a [`TacticScorer`] trait so a deterministic mock — or the driver's
//!   own [`TacticExpander`](super::driver::TacticExpander), via the
//!   [`ExpanderScorer`] adapter — plugs into the same search with no changes.
//!
//! ## Length-normalized priority
//!
//! A frontier node reached by tactics `a_1..a_L` from the root has priority
//! `Σ_t log p(a_t | s_t) / L^alpha` with `alpha ∈ [0, 1]` (config, default `0.5`).
//! Because every `log p ≤ 0`, a raw cumulative sum penalizes *deep* paths (more
//! terms ⇒ more negative), biasing the search shallow; dividing by `L^alpha`
//! counters that bias — `alpha = 0` recovers the pure cumulative score (maximal
//! depth penalty), `alpha = 1` is full per-step averaging (no depth penalty), and
//! intermediate `alpha` interpolates. This is the standard beam-search length
//! normalization, here driving a best-first frontier.
//!
//! ## DPO preference pairs
//!
//! A *solved* search is also a supervision signal. Along the found proof path,
//! at each on-path state the tactic that continued toward the closed goal is a
//! **winner**; the sibling tactics the policy also proposed but that the prover
//! rejected — the [`Discard`](super::tactic_outcome::TacticOutcome::Discard)
//! edges (a Lean error / dead end) the search would otherwise throw away — are
//! **losers**. [`dpo_pairs`] emits one `(state, winning_tactic, losing_tactic)`
//! preference triple per such sibling, in deterministic root→leaf order, ready to
//! train the policy with Direct Preference Optimization.
//!
//! ## Determinism contract
//!
//! The search is a pure algorithm: given the same scorer, root, and
//! [`BestFirstConfig`] (including its `seed`) it returns byte-identical results.
//! There is **no** wall-clock and **no** unseeded randomness anywhere — the
//! priority queue breaks ties by insertion order (a total order) and per-state
//! seeds are derived deterministically from the base seed. The search itself is
//! offline and reproducible; the only stochastic, GPU-gated component is the
//! injected policy that scores tactics, which lives entirely behind the
//! [`TacticScorer`] seam.

use super::critic_scorer::{CriticScorer, GoalStateLike};
use super::driver::{GoalState, TacticExpander};
use super::minimize::{minimize_proof_checked, AdjacencyGraph, MinimizeOutcome};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::Arc;

/// Smallest prior treated as non-zero when converting a `[0,1]` prior to a
/// log-probability (avoids `ln(0) = -∞`).
const PRIOR_EPS: f64 = 1e-12;

/// One candidate tactic scored by the injected policy seam: the tactic text, its
/// log-probability `log p(a | s) ≤ 0`, and the state applying it yields.
#[derive(Debug, Clone)]
pub struct ScoredTactic<S> {
    /// The tactic text (opaque to the search).
    pub tactic: String,
    /// `log p(a | s)` under the policy — `≤ 0`. Higher (closer to `0`) ⇒ more
    /// probable ⇒ expanded sooner (after length normalization).
    pub logprob: f64,
    /// The goal state that results from applying `tactic` (error-free edge).
    pub next: S,
    /// Producer-emitted inline priority nudge (the HVM3 `CInc`/`CDec` analogue),
    /// in the same "nats" units as the frontier score. `0.0` = no hint (the
    /// default, behaviour-preserving). Positive ⇒ schedule this branch sooner,
    /// negative ⇒ later, ORTHOGONALLY to `logprob`. It is accumulated along the
    /// path and folded into the priority only when [`BestFirstConfig::hint_weight`]
    /// is non-zero, so it stays inert unless a producer and the config opt in.
    pub prio_hint: f64,
}

impl<S> ScoredTactic<S> {
    pub fn new(tactic: impl Into<String>, logprob: f64, next: S) -> Self {
        Self {
            tactic: tactic.into(),
            logprob,
            next,
            prio_hint: 0.0,
        }
    }

    /// Attach an inline priority hint (see [`ScoredTactic::prio_hint`]).
    pub fn with_hint(mut self, prio_hint: f64) -> Self {
        self.prio_hint = prio_hint;
        self
    }
}

/// The result of scoring a state: the *live* (error-free) candidate tactics the
/// search may descend into, plus the tactics the policy proposed that the prover
/// **discarded** (a Lean error / dead end). The live set drives the frontier; the
/// discarded set is retained only so [`dpo_pairs`] can mine it for losers.
#[derive(Debug, Clone)]
pub struct ScoredExpansion<S> {
    /// Error-free candidate edges, in the policy's proposal order.
    pub live: Vec<ScoredTactic<S>>,
    /// Tactics tried and discarded at this state (the DPO losers). Order is
    /// preserved for deterministic pair extraction.
    pub discarded: Vec<String>,
}

impl<S> ScoredExpansion<S> {
    /// An expansion with live edges and no discarded siblings.
    pub fn live_only(live: Vec<ScoredTactic<S>>) -> Self {
        Self {
            live,
            discarded: Vec::new(),
        }
    }
}

/// The injected policy seam: given a proof state, return the scored candidate
/// tactics (and the discarded siblings). A real policy LLM implements this — the
/// GPU-gated component — as does the deterministic mock in the tests. `seed` is
/// threaded so a sampling policy stays reproducible; deterministic scorers ignore
/// it.
pub trait TacticScorer {
    /// The proof-state type this scorer operates on.
    type State: GoalState;

    /// Score `state` into candidate `(tactic, logprob, next)` edges plus discarded
    /// siblings. An empty `live` set marks a dead end. Implementations MUST be a
    /// pure function of `(state, seed)` — no wall-clock, no unseeded randomness —
    /// so the search is reproducible.
    fn score(&mut self, state: &Self::State, seed: u64) -> ScoredExpansion<Self::State>;
}

/// Adapts any driver [`TacticExpander`] into a [`TacticScorer`], so the value-free
/// best-first search reuses the exact same environment the MCGS driver does. A
/// step's `[0,1]` prior is read as a probability and mapped to `log(prior)`; the
/// expander exposes no discards, so [`ScoredExpansion::discarded`] is empty.
pub struct ExpanderScorer<E>(pub E);

impl<E: TacticExpander> TacticScorer for ExpanderScorer<E> {
    type State = E::State;

    fn score(&mut self, state: &Self::State, seed: u64) -> ScoredExpansion<Self::State> {
        let live = self
            .0
            .expand(state, seed)
            .into_iter()
            .map(|step| ScoredTactic {
                tactic: step.tactic,
                logprob: step.prior.max(PRIOR_EPS).ln(),
                next: step.next,
                prio_hint: 0.0,
            })
            .collect();
        ScoredExpansion::live_only(live)
    }
}

/// Tuning for [`best_first_search`].
#[derive(Debug, Clone, Copy)]
pub struct BestFirstConfig {
    /// Length-normalization exponent `alpha ∈ [0, 1]` for the priority
    /// `Σ log p / L^alpha`. `0` = pure cumulative log-prob (maximal depth
    /// penalty); `1` = per-step average (no depth penalty). Default `0.5`.
    pub alpha: f64,
    /// Hard cap on expansions (states popped and scored). Guarantees termination
    /// even on an infinite state space — the search stops without a false proof.
    pub max_steps: usize,
    /// Base seed threaded into scoring (per-state seeds are derived from it), so a
    /// sampling policy is reproducible.
    pub seed: u64,
    /// Weight on the producer-emitted inline priority hint
    /// ([`ScoredTactic::prio_hint`]), accumulated along the path and added to the
    /// length-normalized score. Default `0.0` keeps the frontier ordering
    /// identical to the policy-only search; raise it to let producers nudge
    /// scheduling orthogonally to log-prob.
    pub hint_weight: f64,
    /// Weight on the trained state-value critic `V(s)` (the
    /// [`super::critic_scorer`] seam), in the **same nats-per-step units as
    /// `logprob`**: the critic's value for each state on the path is accumulated
    /// into the frontier score's numerator, alongside `Σ log p`, and divided by the
    /// same `L^alpha`. See [`frontier_score`] for why it sits in the numerator
    /// rather than being added to the normalized score.
    ///
    /// Default `0.0`, which makes the critic term structurally absent (the
    /// numerator is then literally `cum_logprob`), so the frontier ordering is the
    /// policy-only ordering the search has today. Because a per-step `|log p|` is
    /// typically in the `0.1 .. 3` nats range and `V(s) ∈ [0, 1]`, a
    /// `critic_weight` of order `1` is the natural starting scale: it makes the
    /// critic worth roughly one average tactic's worth of log-probability per step.
    pub critic_weight: f64,
}

impl Default for BestFirstConfig {
    fn default() -> Self {
        Self {
            alpha: 0.5,
            max_steps: 1_000,
            seed: 0,
            hint_weight: 0.0,
            critic_weight: 0.0,
        }
    }
}

/// The length-normalized priority of a frontier node: `Σ log p / L^alpha`. A
/// higher (less negative) score is expanded sooner. `depth == 0` (the root) uses
/// `L = 1` so the root's empty-path score is exactly its cumulative log-prob
/// (`0`).
fn length_normalized_score(cum_logprob: f64, depth: usize, alpha: f64) -> f64 {
    let l = (depth as f64).max(1.0);
    cum_logprob / l.powf(alpha)
}

/// Read a state's critic value for the frontier, or `0.0` when there is no usable
/// one.
///
/// `0.0` is the correct neutral here (unlike the driver and [`super::mcts`], where
/// the fallback is that node's `progress`): the critic contributes an *additive
/// per-step bonus* to the numerator, so "no opinion" must contribute nothing. Two
/// cases fall back:
/// * no critic injected, and
/// * a non-finite value (`NaN` / `±inf`), the only way an untrained or erroring
///   implementation can signal failure through an `f64`. A `NaN` reaching the
///   frontier would be catastrophic here: [`QueueItem`] orders by `total_cmp`,
///   which sorts `NaN` above every finite score, so one poisoned node would jump
///   the whole queue and the pop order would stop reflecting the search at all.
///
/// Finite values are clamped to the documented `[0, 1]` contract, which is what
/// bounds the per-step critic contribution and makes the scale analysis on
/// [`BestFirstConfig::critic_weight`] hold.
fn critic_value<S: GoalState>(critic: Option<&Arc<dyn CriticScorer>>, state: &S) -> f64 {
    match critic {
        None => 0.0,
        Some(c) => {
            let v = c.score(state as &dyn GoalStateLike);
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            }
        }
    }
}

/// The full frontier priority of a node: the length-normalized path score plus the
/// hint term.
///
/// # Where the critic goes, and why it is not simply added on
///
/// The base score is `Σ log p / L^alpha`. Since every `log p ≤ 0` and their
/// magnitudes are roughly i.i.d., `Σ log p` grows linearly in the path length `L`,
/// so the base score's magnitude grows like `L^(1-alpha)`: with the default
/// `alpha = 0.5`, like `√L`. A critic term added *outside* the normalization would
/// be bounded by `critic_weight` at every depth, so it would swamp the base score
/// at `L = 1` and be negligible by `L = 100`. That is a depth-dependent, and
/// therefore meaningless, preference: the same `critic_weight` would mean two
/// different things at two different points in the same search.
///
/// So the critic enters the **numerator**, as a per-step term accumulated along the
/// path exactly like `log p`, and is divided by the same `L^alpha`:
///
/// `(Σ_t log p(a_t|s_t) + critic_weight · Σ_t V(s_t)) / L^alpha  +  hint_weight · Σ_t hint_t`
///
/// Both numerator sums scale linearly in `L`, so their ratio (the critic's
/// influence relative to the policy's) is invariant in depth. `critic_weight` is
/// then a single scale-free number: "how many nats of log-probability one unit of
/// critic value is worth, per step".
///
/// The hint term is left exactly where it was (added outside, unnormalized). That
/// is pre-existing behaviour with its own default-zero knob and changing it is not
/// this seam's business.
///
/// # Byte-identity at `critic_weight == 0.0`
///
/// The zero branch does not compute `cum_logprob + 0.0 * cum_critic`; it evaluates
/// literally `cum_logprob`, the same expression the search used before this seam
/// existed. So there is no float-arithmetic step to reason about at all on the
/// default path.
fn frontier_score(
    cum_logprob: f64,
    cum_critic: f64,
    cum_hint: f64,
    depth: usize,
    cfg: &BestFirstConfig,
) -> f64 {
    let numerator = if cfg.critic_weight == 0.0 {
        cum_logprob
    } else {
        cum_logprob + cfg.critic_weight * cum_critic
    };
    length_normalized_score(numerator, depth, cfg.alpha) + cfg.hint_weight * cum_hint
}

/// One node in the best-first search arena.
struct Node<S> {
    state: S,
    /// Arena index of the parent, `None` for the root.
    parent: Option<usize>,
    /// The tactic applied at the parent to reach this node, `None` for the root.
    tactic_in: Option<String>,
    /// Path length `L` from the root (number of tactics applied).
    depth: usize,
    /// Cumulative `Σ log p(a_t | s_t)` along the path from the root.
    cum_logprob: f64,
    /// Cumulative `Σ prio_hint` along the path — the producer-emitted priority
    /// nudges (see [`ScoredTactic::prio_hint`]), kept separate from `cum_logprob`
    /// so the log-prob signal (used for DPO mining and length normalization)
    /// stays pure. Folded into the frontier score via
    /// [`BestFirstConfig::hint_weight`].
    cum_hint: f64,
    /// Cumulative `Σ V(s_t)` over the states on the path from the root, excluding
    /// the root itself. Kept separate from `cum_logprob` for the same reason
    /// `cum_hint` is: the log-prob signal is what DPO mining and length
    /// normalization are defined on, and a value estimate is not a log-probability.
    /// Folded into the frontier numerator by [`frontier_score`].
    ///
    /// The root is excluded because its value is common to every frontier node and
    /// so cancels out of all comparisons, and excluding it keeps the root's own
    /// score exactly `0.0` as before.
    cum_critic: f64,
    /// Discarded sibling tactics recorded when this node was expanded — the DPO
    /// losers proposed at this state. Empty until the node is expanded.
    discarded: Vec<String>,
}

/// A frontier entry in the priority queue: a length-normalized score, a
/// deterministic insertion sequence for tie-breaking, and the arena node it
/// refers to. The queue is a max-heap on `score`; equal scores break toward the
/// **earlier**-inserted node (FIFO), keeping the search fully deterministic.
#[derive(Clone, Copy)]
struct QueueItem {
    score: f64,
    seq: u64,
    node: usize,
}

impl PartialEq for QueueItem {
    fn eq(&self, other: &Self) -> bool {
        self.score.total_cmp(&other.score) == Ordering::Equal && self.seq == other.seq
    }
}
impl Eq for QueueItem {}
impl Ord for QueueItem {
    fn cmp(&self, other: &Self) -> Ordering {
        // Max-heap on score; on a tie, the smaller `seq` must be "greater" so it
        // pops first (BinaryHeap yields the maximum).
        self.score
            .total_cmp(&other.score)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}
impl PartialOrd for QueueItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// One edge of a reconstructed proof path: the state a tactic was applied to and
/// the tactic that advanced it toward the closed goal.
#[derive(Debug, Clone)]
pub struct ProofStep<S> {
    /// The state the tactic was applied to (the *from* state of the edge).
    pub state: S,
    /// The winning tactic applied at `state`.
    pub tactic: String,
}

/// A single Direct-Preference-Optimization training triple mined from a solved
/// search: at `state`, the policy should prefer `winning_tactic` (it continued the
/// proof) over `losing_tactic` (a discarded sibling that hit a Lean error / dead
/// end).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DpoPair<S> {
    /// The proof state at which the preference holds.
    pub state: S,
    /// The tactic on the solution path (preferred).
    pub winning_tactic: String,
    /// A discarded sibling tactic at the same state (dispreferred).
    pub losing_tactic: String,
}

/// The outcome of a best-first search — the proof path (if solved) plus enough of
/// the search trace for [`dpo_pairs`] to mine preference triples.
pub struct BestFirstOutcome<S> {
    /// Whether a closed (proof-complete) state was reached.
    pub solved: bool,
    /// Expansions performed (states popped and scored), bounded by the budget.
    pub steps: usize,
    /// Dedup keys of the states popped from the frontier, in expansion order —
    /// the observable trace of *which* states the priority ordering visited and
    /// in what order. Deterministic.
    pub order: Vec<String>,
    /// The search arena (all discovered nodes).
    arena: Vec<Node<S>>,
    /// Arena index of the closed node that solved the search, if any.
    solved_node: Option<usize>,
}

impl<S: GoalState> BestFirstOutcome<S> {
    /// Arena indices along the found proof path, root first, closed leaf last.
    /// Empty when the search did not solve.
    fn path_nodes(&self) -> Vec<usize> {
        let mut path = Vec::new();
        let mut cur = self.solved_node;
        while let Some(idx) = cur {
            path.push(idx);
            cur = self.arena[idx].parent;
        }
        path.reverse();
        path
    }

    /// The winning tactics from the root to the closed goal, in order. Empty when
    /// the search did not solve.
    pub fn proof_tactics(&self) -> Vec<String> {
        self.path_nodes()
            .iter()
            .filter_map(|&idx| self.arena[idx].tactic_in.clone())
            .collect()
    }

    /// The proof path as `(from-state, tactic)` edges, root→leaf. Empty when the
    /// search did not solve.
    pub fn proof_path(&self) -> Vec<ProofStep<S>>
    where
        S: Clone,
    {
        let nodes = self.path_nodes();
        let mut out = Vec::new();
        for w in nodes.windows(2) {
            let (parent, child) = (w[0], w[1]);
            if let Some(tactic) = &self.arena[child].tactic_in {
                out.push(ProofStep {
                    state: self.arena[parent].state.clone(),
                    tactic: tactic.clone(),
                });
            }
        }
        out
    }

    /// Project the *whole* search arena into a [`ProofGraph`], collapsing arena
    /// nodes that share a [`dedup_key`](GoalState::dedup_key) onto one graph node.
    ///
    /// Best-first grows a tree (one parent per arena node), but two arena nodes can
    /// hold the same goal state reached by different-length routes: a transposition
    /// the frontier discovered but only expanded once. Merging by dedup_key turns
    /// that tree back into the DAG those transpositions imply, which is precisely
    /// where a *shorter* closing path than the log-prob-ordered line best-first
    /// closed on can live. The projection is edges the scorer actually emitted and
    /// closed flags from [`is_closed`](GoalState::is_closed); it is only a record of
    /// the search, so any path read out of it is a guess until the gate re-checks it.
    ///
    /// First-seen id assignment over the ordered arena (root is index `0`) keeps the
    /// mapping, and therefore the downstream BFS, deterministic.
    fn proof_graph(&self) -> AdjacencyGraph {
        let mut ids: HashMap<String, usize> = HashMap::new();
        for node in &self.arena {
            let next = ids.len();
            ids.entry(node.state.dedup_key()).or_insert(next);
        }
        // The root is arena index 0, so it is the first key seen and holds id 0.
        let mut graph = AdjacencyGraph::new(ids[&self.arena[0].state.dedup_key()]);
        for node in &self.arena {
            let id = ids[&node.state.dedup_key()];
            if node.state.is_closed() {
                graph = graph.close(id);
            }
            // A node carries its incoming edge (parent, tactic) except at the root;
            // re-key the parent so transposed parents fold together too.
            if let (Some(parent), Some(tactic)) = (node.parent, node.tactic_in.as_ref()) {
                let parent_id = ids[&self.arena[parent].state.dedup_key()];
                graph = graph.edge(parent_id, tactic, id);
            }
        }
        graph
    }

    /// Shrink the found proof to a shorter, **gate-re-checked** tactic sequence.
    ///
    /// The soundness boundary is `replay`: it must return `true` only when replaying
    /// that exact tactic subsequence from the root actually closes the goal. This
    /// method never decides solvedness itself: it hands BFS's shortest closing path
    /// (a guess drawn from the recorded arena) to `replay` and returns the
    /// [`MinimizeOutcome`] verbatim, so `accepted`/[`Verified`](super::minimize::MinimizeStatus::Verified)
    /// is populated only when the caller's re-check confirmed the shrink. On an
    /// unsolved search it returns a
    /// [`NoProofFound`](super::minimize::MinimizeStatus::NoProofFound) outcome
    /// without ever consulting `replay`, so no proof is fabricated where none was
    /// found.
    pub fn minimized_proof<F>(&self, replay: F) -> MinimizeOutcome
    where
        F: FnMut(&[String]) -> bool,
    {
        if !self.solved {
            // No solution: hand the checked entry point a graph with no closed node
            // so it reports NoProofFound and leaves the gate untouched. Nothing is
            // ever marked accepted for a search that did not close a goal.
            return minimize_proof_checked(&AdjacencyGraph::new(0), &[], replay);
        }
        let original = self.proof_tactics();
        minimize_proof_checked(&self.proof_graph(), &original, replay)
    }
}

/// Run value-free best-first search from `root`, expanding states in descending
/// length-normalized cumulative log-prob until a closed state is reached or the
/// step budget is exhausted.
///
/// The search maintains a single global priority queue (the frontier) and a set
/// of already-expanded state keys, so a state that is reached by two paths (a
/// transposition) is expanded only once — the graph, not tree, discipline the
/// MCGS driver also follows. No value, rollout, or backpropagation is used: the
/// only signal is the policy log-prob the [`TacticScorer`] returns.
pub fn best_first_search<Sc: TacticScorer>(
    scorer: &mut Sc,
    root: Sc::State,
    cfg: &BestFirstConfig,
) -> BestFirstOutcome<Sc::State> {
    // No critic is constructed and none can be invoked on this path, so the search
    // is the pre-seam search independently of what `cfg.critic_weight` says (the
    // gate inside `best_first_search_with_critic` zeroes it when there is no
    // critic). Every existing caller keeps this signature.
    best_first_search_with_critic(scorer, root, cfg, None)
}

/// [`best_first_search`] with an injectable trained state-value critic folded into
/// the frontier priority: the [`super::critic_scorer`] seam, applied to the
/// production search.
///
/// # What the critic may and may not do
///
/// It changes the ORDER in which states leave the frontier and nothing else. It is
/// read at exactly one place, [`frontier_score`], and its value is never stored on
/// a proof path, never consulted by
/// [`is_closed`](super::driver::GoalState::is_closed), never reaches
/// [`BestFirstOutcome::minimized_proof`] or its `replay` gate, and never enters
/// [`dpo_pairs`]. `solved` is set only when a state the scorer itself produced is
/// popped and reports `is_closed()`. So a wrong critic costs expansions, never
/// soundness, and cannot promote anything toward acceptance.
///
/// # Determinism
///
/// [`critic_value`] guarantees a finite, `[0, 1]`-clamped contribution, so every
/// `QueueItem::score` stays finite and `total_cmp` remains a total order over them.
/// Exactly-equal scores still fall through to the `seq` tie-break (earlier
/// insertion pops first), which is unchanged. A `CriticScorer` is contractually a
/// pure function of the state text, so re-running the same search re-derives the
/// same scores.
///
/// # Safety at the default
///
/// `critic_weight` is forced to `0.0` whenever `critic` is `None`, so config alone
/// can never alter behaviour; and at `critic_weight == 0.0` [`frontier_score`]
/// evaluates the pre-seam expression literally.
pub fn best_first_search_with_critic<Sc: TacticScorer>(
    scorer: &mut Sc,
    root: Sc::State,
    cfg: &BestFirstConfig,
    critic: Option<Arc<dyn CriticScorer>>,
) -> BestFirstOutcome<Sc::State> {
    // Gate the weight on a critic being present. Without this, a non-zero weight
    // with no critic would multiply into an all-zero `cum_critic` and merely look
    // inert; making it explicit means there is one statement to read rather than an
    // invariant to trust.
    let cfg = &BestFirstConfig {
        critic_weight: if critic.is_some() {
            cfg.critic_weight
        } else {
            0.0
        },
        ..*cfg
    };
    let critic = critic.as_ref();
    let mut arena: Vec<Node<Sc::State>> = Vec::new();
    let mut heap: BinaryHeap<QueueItem> = BinaryHeap::new();
    let mut expanded: HashSet<String> = HashSet::new();
    let mut order: Vec<String> = Vec::new();

    arena.push(Node {
        state: root,
        parent: None,
        tactic_in: None,
        depth: 0,
        cum_logprob: 0.0,
        cum_hint: 0.0,
        cum_critic: 0.0,
        discarded: Vec::new(),
    });
    let mut seq = 0u64;
    heap.push(QueueItem {
        score: frontier_score(0.0, 0.0, 0.0, 0, cfg),
        seq,
        node: 0,
    });

    let mut steps = 0usize;
    let mut solved = false;
    let mut solved_node = None;

    while let Some(item) = heap.pop() {
        let node = item.node;
        let key = arena[node].state.dedup_key();
        // Transposition guard: a state already expanded is not re-expanded.
        if expanded.contains(&key) {
            continue;
        }
        order.push(key.clone());

        // A closed state is a proof — detected on pop, so a solution already on
        // the frontier is returned even once the budget is spent.
        if arena[node].state.is_closed() {
            solved = true;
            solved_node = Some(node);
            break;
        }
        // Budget: stop before scoring once the expansion cap is hit.
        if steps >= cfg.max_steps {
            break;
        }
        expanded.insert(key.clone());

        let seed = mix_seed(cfg.seed, &key);
        let expansion = scorer.score(&arena[node].state, seed);
        steps += 1;
        arena[node].discarded = expansion.discarded;

        let parent_depth = arena[node].depth;
        let parent_cum = arena[node].cum_logprob;
        let parent_hint = arena[node].cum_hint;
        let parent_critic = arena[node].cum_critic;
        for st in expansion.live {
            let child_depth = parent_depth + 1;
            let child_cum = parent_cum + st.logprob;
            let child_hint = parent_hint + st.prio_hint;
            // One critic call per generated edge, evaluated before the state moves
            // into the arena. `critic_value` returns a hard `0.0` when no critic is
            // injected, so this is not merely cheap on the default path, it is the
            // constant `0.0` and `cum_critic` stays `0.0` throughout.
            let child_critic = parent_critic + critic_value(critic, &st.next);
            let child = arena.len();
            arena.push(Node {
                state: st.next,
                parent: Some(node),
                tactic_in: Some(st.tactic),
                depth: child_depth,
                cum_logprob: child_cum,
                cum_hint: child_hint,
                cum_critic: child_critic,
                discarded: Vec::new(),
            });
            seq += 1;
            heap.push(QueueItem {
                score: frontier_score(child_cum, child_critic, child_hint, child_depth, cfg),
                seq,
                node: child,
            });
        }
    }

    BestFirstOutcome {
        solved,
        steps,
        order,
        arena,
        solved_node,
    }
}

/// Extract Direct-Preference-Optimization pairs from a solved search.
///
/// Walks the found proof path root→leaf; at each on-path state the tactic that
/// continued the proof is the winner, and every **discarded** sibling recorded at
/// that state (a Lean error / dead end — the edges the search otherwise throws
/// away) becomes a loser. Emits one [`DpoPair`] per `(on-path state, winner,
/// loser)` in deterministic order (path order, then the scorer's discard order).
/// Returns empty for an unsolved search.
pub fn dpo_pairs<S: GoalState + Clone>(outcome: &BestFirstOutcome<S>) -> Vec<DpoPair<S>> {
    let mut out = Vec::new();
    if !outcome.solved {
        return out;
    }
    let nodes = outcome.path_nodes();
    for w in nodes.windows(2) {
        let (parent, child) = (w[0], w[1]);
        let winner = match &outcome.arena[child].tactic_in {
            Some(t) => t.clone(),
            None => continue,
        };
        for loser in &outcome.arena[parent].discarded {
            out.push(DpoPair {
                state: outcome.arena[parent].state.clone(),
                winning_tactic: winner.clone(),
                losing_tactic: loser.clone(),
            });
        }
    }
    out
}

/// Derive a deterministic per-state seed from a base seed and a state's dedup key
/// (FNV-1a). Same `(base, key)` ⇒ same seed, so a sampling scorer behaves
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
    use super::super::driver::TacticStep;
    use super::super::minimize::MinimizeStatus;
    use super::*;
    use std::collections::HashMap;

    // ---- Deterministic mocks -------------------------------------------------

    /// A table-driven proof state: `key` identifies it, `closed` marks completion.
    #[derive(Clone, Debug, PartialEq)]
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

    /// A deterministic scorer: a map from a state key to its live scored tactics
    /// (`tactic`, `logprob`, `next`) and its discarded siblings. Missing keys are
    /// dead ends. The `seed` is accepted but ignored (this mock is deterministic).
    struct TableScorer {
        live: HashMap<String, Vec<ScoredTactic<MockGoal>>>,
        dead: HashMap<String, Vec<String>>,
    }
    impl TableScorer {
        fn new() -> Self {
            Self {
                live: HashMap::new(),
                dead: HashMap::new(),
            }
        }
        fn edge(mut self, from: &str, tactic: &str, logprob: f64, to: MockGoal) -> Self {
            self.live
                .entry(from.into())
                .or_default()
                .push(ScoredTactic::new(tactic, logprob, to));
            self
        }
        fn discard(mut self, from: &str, tactic: &str) -> Self {
            self.dead
                .entry(from.into())
                .or_default()
                .push(tactic.into());
            self
        }
        fn edge_hinted(
            mut self,
            from: &str,
            tactic: &str,
            logprob: f64,
            hint: f64,
            to: MockGoal,
        ) -> Self {
            self.live
                .entry(from.into())
                .or_default()
                .push(ScoredTactic::new(tactic, logprob, to).with_hint(hint));
            self
        }
    }
    impl TacticScorer for TableScorer {
        type State = MockGoal;
        fn score(&mut self, state: &MockGoal, _seed: u64) -> ScoredExpansion<MockGoal> {
            ScoredExpansion {
                live: self.live.get(&state.key).cloned().unwrap_or_default(),
                discarded: self.dead.get(&state.key).cloned().unwrap_or_default(),
            }
        }
    }

    // ---- Length-normalization arithmetic ------------------------------------

    #[test]
    fn alpha_flips_the_priority_of_a_shallow_vs_deep_node() {
        // Shallow X: depth 1, cum = ln(0.6). Deep Y: depth 3, cum = 3·ln(0.8).
        // X has the *higher* (less negative) cumulative log-prob, but Y wins once
        // normalized by depth.
        let x_cum = 0.6f64.ln();
        let y_cum = 3.0 * 0.8f64.ln();
        assert!(x_cum > y_cum, "X has higher raw cumulative log-prob");

        // alpha = 0: pure cumulative ⇒ shallow X is preferred.
        let x0 = length_normalized_score(x_cum, 1, 0.0);
        let y0 = length_normalized_score(y_cum, 3, 0.0);
        assert!(x0 > y0, "alpha=0 must prefer the shallow node");

        // alpha = 1: per-step average ⇒ the deep high-per-step Y is preferred.
        let x1 = length_normalized_score(x_cum, 1, 1.0);
        let y1 = length_normalized_score(y_cum, 3, 1.0);
        assert!(y1 > x1, "alpha=1 must prefer the deep node");
    }

    // ---- Best-first search ---------------------------------------------------

    #[test]
    fn best_first_finds_the_closing_path() {
        // Two competing branches: A closes with high per-step log-prob, B is a
        // low-prob dead end. Best-first must follow A to the closed goal.
        let mut scorer = TableScorer::new()
            .edge("root", "tA", 0.9f64.ln(), MockGoal::open("A"))
            .edge("root", "tB", 0.2f64.ln(), MockGoal::open("B"))
            .edge("A", "aClose", 0.9f64.ln(), MockGoal::closed("cA"));
        let out = best_first_search(
            &mut scorer,
            MockGoal::open("root"),
            &BestFirstConfig::default(),
        );

        assert!(out.solved, "the A branch closes the goal");
        assert_eq!(out.proof_tactics(), vec!["tA", "aClose"]);
        // The closed state was reached; B was never on the winning path.
        assert_eq!(out.proof_path().last().unwrap().tactic, "aClose");
    }

    #[test]
    fn inline_hint_reorders_the_frontier_only_when_weighted() {
        let pos = |order: &[String], k: &str| order.iter().position(|s| s == k).unwrap();
        // "lo" has a far lower log-prob than "hi" but carries a large positive
        // inline hint. Both leaves are dead ends, so both are expanded.
        let build = || {
            TableScorer::new()
                .edge("root", "hi", 0.9f64.ln(), MockGoal::open("Hi"))
                .edge_hinted("root", "lo", 0.1f64.ln(), 10.0, MockGoal::open("Lo"))
        };

        // hint_weight = 0.0 (default): the hint is inert; log-prob decides, so the
        // frontier expands Hi before Lo -- byte-identical to the policy-only search.
        let mut s0 = build();
        let out0 = best_first_search(&mut s0, MockGoal::open("root"), &BestFirstConfig::default());
        assert!(
            pos(&out0.order, "Hi") < pos(&out0.order, "Lo"),
            "with hint_weight=0 the frontier must ignore the hint and follow log-prob"
        );

        // hint_weight > 0: the large positive hint pulls the low-log-prob branch
        // ahead, orthogonally to its policy score.
        let mut s1 = build();
        let cfg = BestFirstConfig {
            hint_weight: 1.0,
            ..BestFirstConfig::default()
        };
        let out1 = best_first_search(&mut s1, MockGoal::open("root"), &cfg);
        assert!(
            pos(&out1.order, "Lo") < pos(&out1.order, "Hi"),
            "a large positive hint with hint_weight>0 must schedule Lo before Hi"
        );
    }

    #[test]
    fn prefers_higher_length_normalized_prior_at_equal_depth() {
        // At equal depth the higher-log-prob sibling is expanded first. From root,
        // "hi" (ln 0.9) and "lo" (ln 0.1) reach two dead-end leaves; the frontier
        // must pop root, then Hi, then Lo.
        let mut scorer = TableScorer::new()
            .edge("root", "hi", 0.9f64.ln(), MockGoal::open("Hi"))
            .edge("root", "lo", 0.1f64.ln(), MockGoal::open("Lo"));
        let out = best_first_search(
            &mut scorer,
            MockGoal::open("root"),
            &BestFirstConfig::default(),
        );

        assert_eq!(out.order, vec!["root", "Hi", "Lo"]);
    }

    /// A state space where a shallow node X and a deep node Y coexist on the
    /// frontier, so `alpha` decides which is popped first (see the arithmetic
    /// test). X: root→X, ln(0.6). Y: root→Y0→Y1→Y2, each ln(0.8). X and Y2 are
    /// dead-end leaves. After Y0,Y1 are expanded, X(depth1) and Y2(depth3) race.
    fn alpha_ordering_scorer() -> TableScorer {
        TableScorer::new()
            .edge("root", "tx", 0.6f64.ln(), MockGoal::open("X"))
            .edge("root", "ty0", 0.8f64.ln(), MockGoal::open("Y0"))
            .edge("Y0", "ty1", 0.8f64.ln(), MockGoal::open("Y1"))
            .edge("Y1", "ty2", 0.8f64.ln(), MockGoal::open("Y2"))
    }

    #[test]
    fn alpha_changes_the_expansion_order() {
        let pos = |order: &[String], k: &str| order.iter().position(|s| s == k).unwrap();

        // alpha = 0 (max depth penalty): shallow X pops before deep Y2.
        let mut s0 = alpha_ordering_scorer();
        let o0 = best_first_search(
            &mut s0,
            MockGoal::open("root"),
            &BestFirstConfig {
                alpha: 0.0,
                ..BestFirstConfig::default()
            },
        );
        assert!(
            pos(&o0.order, "X") < pos(&o0.order, "Y2"),
            "alpha=0 must expand shallow X before deep Y2 (order {:?})",
            o0.order
        );

        // alpha = 1 (no depth penalty): deep Y2 pops before shallow X.
        let mut s1 = alpha_ordering_scorer();
        let o1 = best_first_search(
            &mut s1,
            MockGoal::open("root"),
            &BestFirstConfig {
                alpha: 1.0,
                ..BestFirstConfig::default()
            },
        );
        assert!(
            pos(&o1.order, "Y2") < pos(&o1.order, "X"),
            "alpha=1 must expand deep Y2 before shallow X (order {:?})",
            o1.order
        );
    }

    #[test]
    fn already_closed_root_is_trivially_solved() {
        let mut scorer = TableScorer::new();
        let out = best_first_search(
            &mut scorer,
            MockGoal::closed("done"),
            &BestFirstConfig::default(),
        );
        assert!(out.solved);
        assert_eq!(out.steps, 0, "no expansion needed for a closed root");
        assert!(out.proof_tactics().is_empty());
    }

    #[test]
    fn transposition_state_is_expanded_only_once() {
        // Diamond: root→L→D and root→R→D. The shared state D must be expanded once.
        let mut scorer = TableScorer::new()
            .edge("root", "l", 0.5f64.ln(), MockGoal::open("L"))
            .edge("root", "r", 0.5f64.ln(), MockGoal::open("R"))
            .edge("L", "ld", 0.9f64.ln(), MockGoal::open("D"))
            .edge("R", "rd", 0.9f64.ln(), MockGoal::open("D"));
        let out = best_first_search(
            &mut scorer,
            MockGoal::open("root"),
            &BestFirstConfig::default(),
        );
        let d_count = out.order.iter().filter(|k| *k == "D").count();
        assert_eq!(d_count, 1, "the transposed state D is expanded only once");
    }

    #[test]
    fn budget_bounds_the_search_on_an_infinite_chain() {
        // An unbounded chain n0→n1→n2→… never closes; the budget must stop it.
        let mut scorer = TableScorer::new()
            .edge("n0", "s", 0.9f64.ln(), MockGoal::open("n1"))
            .edge("n1", "s", 0.9f64.ln(), MockGoal::open("n2"))
            .edge("n2", "s", 0.9f64.ln(), MockGoal::open("n3"))
            .edge("n3", "s", 0.9f64.ln(), MockGoal::open("n4"))
            .edge("n4", "s", 0.9f64.ln(), MockGoal::open("n5"));
        let out = best_first_search(
            &mut scorer,
            MockGoal::open("n0"),
            &BestFirstConfig {
                max_steps: 3,
                ..BestFirstConfig::default()
            },
        );
        assert!(!out.solved, "an unclosable chain must not be solved");
        assert!(out.steps <= 3, "expansions must not exceed the budget");
    }

    #[test]
    fn search_is_deterministic() {
        let build = || {
            TableScorer::new()
                .edge("root", "a", 0.7f64.ln(), MockGoal::open("A"))
                .edge("root", "b", 0.6f64.ln(), MockGoal::open("B"))
                .edge("A", "ac", 0.9f64.ln(), MockGoal::closed("cA"))
                .edge("B", "bc", 0.9f64.ln(), MockGoal::closed("cB"))
        };
        let mut s1 = build();
        let mut s2 = build();
        let o1 = best_first_search(&mut s1, MockGoal::open("root"), &BestFirstConfig::default());
        let o2 = best_first_search(&mut s2, MockGoal::open("root"), &BestFirstConfig::default());
        assert_eq!(o1.solved, o2.solved);
        assert_eq!(o1.order, o2.order);
        assert_eq!(o1.steps, o2.steps);
        assert_eq!(o1.proof_tactics(), o2.proof_tactics());
    }

    // ---- ExpanderScorer adapter (reuses the driver's TacticExpander) ---------

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
    fn expander_scorer_adapter_drives_best_first() {
        // A driver TacticExpander (priors in [0,1]) plugs straight into best-first
        // via the adapter, which reads log(prior) as the log-prob.
        let expander = TableExpander::new()
            .edge("g2", "close", 1.0, MockGoal::open("g1"))
            .edge("g1", "close", 1.0, MockGoal::closed("g0"));
        let mut scorer = ExpanderScorer(expander);
        let out = best_first_search(
            &mut scorer,
            MockGoal::open("g2"),
            &BestFirstConfig::default(),
        );
        assert!(out.solved);
        assert_eq!(out.proof_tactics(), vec!["close", "close"]);
    }

    // ---- The trained-critic seam --------------------------------------------

    /// A deterministic critic that rates any state whose key starts with `"Good"`
    /// as close to done. It reads only `state_text()`, the textual contract a real
    /// critic sees, and is a pure function of it.
    struct KeyCritic {
        prefix: &'static str,
    }
    impl CriticScorer for KeyCritic {
        fn score(&self, state: &dyn GoalStateLike) -> f64 {
            if state.state_text().starts_with(self.prefix) {
                1.0
            } else {
                0.0
            }
        }
    }

    /// A critic that returns `NaN`, the failure signal an untrained or erroring
    /// implementation can emit through an `f64`.
    struct NanCritic;
    impl CriticScorer for NanCritic {
        fn score(&self, _state: &dyn GoalStateLike) -> f64 {
            f64::NAN
        }
    }

    /// A branching scorer: from the root, a LOW-log-prob branch through `Good*`
    /// states and a HIGH-log-prob branch through `Bad*` states, both dead ends of
    /// equal depth. Policy alone expands `Bad1` first; a critic that likes `Good*`
    /// must be able to flip that.
    fn split_scorer() -> TableScorer {
        TableScorer::new()
            .edge("root", "bad", 0.9f64.ln(), MockGoal::open("Bad1"))
            .edge("root", "good", 0.1f64.ln(), MockGoal::open("Good1"))
            .edge("Bad1", "bad2", 0.9f64.ln(), MockGoal::open("Bad2"))
            .edge("Good1", "good2", 0.9f64.ln(), MockGoal::open("Good2"))
    }

    fn arc(c: impl CriticScorer + 'static) -> Arc<dyn CriticScorer> {
        Arc::new(c)
    }

    /// The safety property that makes this landable on the production path: at
    /// `critic_weight == 0.0` an injected critic changes NOTHING, whatever it says.
    /// `order` is the full observable trace of which states the frontier popped and
    /// in what sequence, so equality of `order` is equality of the search.
    #[test]
    fn critic_weight_zero_is_identical_to_the_critic_free_search() {
        let cfg = BestFirstConfig::default(); // critic_weight == 0.0
        let mut base_scorer = split_scorer();
        let baseline = best_first_search(&mut base_scorer, MockGoal::open("root"), &cfg);

        for critic in [
            None,
            Some(arc(KeyCritic { prefix: "Good" })),
            Some(arc(KeyCritic { prefix: "Bad" })),
            Some(arc(NanCritic)),
        ] {
            let mut s = split_scorer();
            let got = best_first_search_with_critic(&mut s, MockGoal::open("root"), &cfg, critic);
            assert_eq!(got.order, baseline.order);
            assert_eq!(got.solved, baseline.solved);
            assert_eq!(got.steps, baseline.steps);
        }
    }

    /// With no critic injected, `critic_weight` is inert for EVERY value, so no
    /// config value alone can change the production search.
    #[test]
    fn weight_without_a_critic_is_inert_at_every_value() {
        let mut base_scorer = split_scorer();
        let baseline = best_first_search(
            &mut base_scorer,
            MockGoal::open("root"),
            &BestFirstConfig::default(),
        );
        for w in [0.0, 1.0, 25.0, -25.0] {
            let cfg = BestFirstConfig {
                critic_weight: w,
                ..BestFirstConfig::default()
            };
            let mut s = split_scorer();
            let got = best_first_search_with_critic(&mut s, MockGoal::open("root"), &cfg, None);
            assert_eq!(got.order, baseline.order, "no critic, critic_weight={w}");
        }
    }

    /// The seam is LIVE on the production path: with a critic and a non-zero
    /// weight, the critic's verdict changes which branch the frontier explores
    /// first, and flipping the critic flips the order back. So it is the critic
    /// deciding, not a fixed tie-break.
    #[test]
    fn critic_reorders_the_frontier_when_weighted() {
        let pos = |order: &[String], k: &str| order.iter().position(|s| s == k).unwrap();
        // The two branches differ by `ln(0.9) - ln(0.1)` = about 2.2 nats per step,
        // so a weight of 4.0 is the honest "critic outweighs a 2.2-nat policy gap"
        // setting. A smaller weight would correctly leave the policy in charge,
        // which is the point of the term being on a comparable scale.
        let cfg = BestFirstConfig {
            critic_weight: 4.0,
            ..BestFirstConfig::default()
        };

        // Policy alone prefers the Bad branch (higher log-prob).
        let mut s = split_scorer();
        let policy_only = best_first_search(&mut s, MockGoal::open("root"), &cfg);
        assert!(pos(&policy_only.order, "Bad1") < pos(&policy_only.order, "Good1"));

        // A critic that likes Good* pulls it ahead.
        let mut s = split_scorer();
        let good = best_first_search_with_critic(
            &mut s,
            MockGoal::open("root"),
            &cfg,
            Some(arc(KeyCritic { prefix: "Good" })),
        );
        assert!(
            pos(&good.order, "Good1") < pos(&good.order, "Bad1"),
            "the critic-preferred branch must be explored first (order {:?})",
            good.order
        );

        // Inverting the critic restores the policy's own preference.
        let mut s = split_scorer();
        let bad = best_first_search_with_critic(
            &mut s,
            MockGoal::open("root"),
            &cfg,
            Some(arc(KeyCritic { prefix: "Bad" })),
        );
        assert!(pos(&bad.order, "Bad1") < pos(&bad.order, "Good1"));
    }

    /// The critic term rides in the numerator, so its influence relative to the
    /// policy term does not drift with depth. Concretely: the critic's advantage
    /// over a fixed per-step log-prob gap is the SAME at depth 1 and at depth 8. A
    /// term added outside the `L^alpha` normalization would fail this, because the
    /// base score's magnitude grows like `L^(1-alpha)` while a bounded outside term
    /// does not.
    #[test]
    fn the_critic_term_keeps_its_scale_against_the_base_score_at_every_depth() {
        let cfg = BestFirstConfig {
            critic_weight: 1.0,
            ..BestFirstConfig::default()
        };
        // Per-step log-prob gap of `gap` nats against a per-step critic gap of 1.0.
        let step = 0.5f64.ln();
        let gap = 0.25f64.ln() - step; // the extra nats the weaker sibling loses
        for depth in [1usize, 2, 8, 64] {
            let d = depth as f64;
            // Branch P: better log-prob every step, critic says 0 every step.
            let p = frontier_score(d * step, 0.0, 0.0, depth, &cfg);
            // Branch C: worse log-prob every step, critic says 1 every step.
            let c = frontier_score(d * (step + gap), d, 0.0, depth, &cfg);
            // The critic's net advantage, normalized by the base score's own scale.
            let advantage = (c - p) / d.powf(1.0 - cfg.alpha);
            let expected = 1.0 + gap; // critic_weight * 1.0 + gap, per step
            assert!(
                (advantage - expected).abs() < 1e-9,
                "critic-vs-policy scale must be depth-invariant (depth {depth}: {advantage} vs {expected})"
            );
        }
    }

    /// A critic cannot make anything true. On a search with no closed state, a
    /// critic pinned at the maximum must not flip `solved`, must not invent a proof
    /// path, must not produce DPO pairs, and must not let the minimizer accept
    /// anything (the gate is never even consulted for an unsolved search).
    #[test]
    fn critic_never_decides_that_something_is_proved() {
        let cfg = BestFirstConfig {
            critic_weight: 50.0,
            ..BestFirstConfig::default()
        };
        let mut s = split_scorer(); // no MockGoal::closed anywhere
        let out = best_first_search_with_critic(
            &mut s,
            MockGoal::open("root"),
            &cfg,
            Some(arc(KeyCritic { prefix: "Good" })),
        );
        assert!(!out.solved, "only `is_closed` may declare a solve");
        assert!(out.proof_tactics().is_empty());
        assert!(dpo_pairs(&out).is_empty());

        let mut gate_calls = 0;
        let minimized = out.minimized_proof(|_| {
            gate_calls += 1;
            true // a gate that says yes to everything must still be unreachable
        });
        assert_eq!(
            gate_calls, 0,
            "the gate must not run for an unsolved search"
        );
        assert_eq!(minimized.status, MinimizeStatus::NoProofFound);
        assert!(minimized.accepted.is_none());
    }

    /// A `NaN` critic must be neutralised BEFORE it reaches the ordering.
    /// `QueueItem` orders by `total_cmp`, which sorts `NaN` above every finite
    /// score, so an unneutralised `NaN` would let one node jump the whole queue.
    /// Degrading it to `0.0` recovers the policy-only order exactly.
    #[test]
    fn a_non_finite_critic_cannot_poison_the_frontier() {
        let cfg = BestFirstConfig {
            critic_weight: 3.0,
            ..BestFirstConfig::default()
        };
        let mut base = split_scorer();
        let baseline = best_first_search(&mut base, MockGoal::open("root"), &cfg);
        let mut s = split_scorer();
        let got = best_first_search_with_critic(
            &mut s,
            MockGoal::open("root"),
            &cfg,
            Some(arc(NanCritic)),
        );
        assert_eq!(got.order, baseline.order);
    }

    /// Exactly-equal scores still fall through to the `seq` tie-break: the
    /// earlier-inserted sibling pops first. Two siblings with identical log-prob
    /// and an identical critic verdict tie exactly, and the scorer's proposal order
    /// decides, as it did before the seam existed.
    #[test]
    fn equal_scores_still_break_toward_the_earlier_insertion() {
        let cfg = BestFirstConfig {
            critic_weight: 4.0,
            ..BestFirstConfig::default()
        };
        let build = || {
            TableScorer::new()
                .edge("root", "first", 0.5f64.ln(), MockGoal::open("Good_a"))
                .edge("root", "second", 0.5f64.ln(), MockGoal::open("Good_b"))
        };
        let mut s = build();
        let out = best_first_search_with_critic(
            &mut s,
            MockGoal::open("root"),
            &cfg,
            Some(arc(KeyCritic { prefix: "Good" })),
        );
        assert_eq!(out.order, vec!["root", "Good_a", "Good_b"]);

        // Reversing the proposal order reverses the pop order, confirming it is
        // insertion order and not the key that decides.
        let mut s = TableScorer::new()
            .edge("root", "second", 0.5f64.ln(), MockGoal::open("Good_b"))
            .edge("root", "first", 0.5f64.ln(), MockGoal::open("Good_a"));
        let out = best_first_search_with_critic(
            &mut s,
            MockGoal::open("root"),
            &cfg,
            Some(arc(KeyCritic { prefix: "Good" })),
        );
        assert_eq!(out.order, vec!["root", "Good_b", "Good_a"]);
    }

    /// Determinism: the same critic-steered search re-run gives the same trace.
    #[test]
    fn critic_guided_search_is_reproducible() {
        let cfg = BestFirstConfig {
            critic_weight: 2.0,
            ..BestFirstConfig::default()
        };
        let run = || {
            let mut s = split_scorer();
            best_first_search_with_critic(
                &mut s,
                MockGoal::open("root"),
                &cfg,
                Some(arc(KeyCritic { prefix: "Good" })),
            )
            .order
        };
        assert_eq!(run(), run());
    }

    // ---- DPO preference-pair extraction -------------------------------------

    #[test]
    fn dpo_pairs_pairs_winners_with_discarded_siblings() {
        // Path root -tA-> A -aClose-> cA(closed). At root the policy also proposed
        // two tactics the prover discarded (simp_fails, ring_fails); at A it
        // discarded omega_fails. Each becomes a (winner > loser) pair.
        let mut scorer = TableScorer::new()
            .edge("root", "tA", 0.9f64.ln(), MockGoal::open("A"))
            .edge("root", "tB", 0.2f64.ln(), MockGoal::open("B"))
            .discard("root", "simp_fails")
            .discard("root", "ring_fails")
            .edge("A", "aClose", 0.9f64.ln(), MockGoal::closed("cA"))
            .discard("A", "omega_fails");
        let out = best_first_search(
            &mut scorer,
            MockGoal::open("root"),
            &BestFirstConfig::default(),
        );
        assert!(out.solved);

        let pairs = dpo_pairs(&out);
        // Deterministic root→leaf, discard-order pairs.
        assert_eq!(
            pairs,
            vec![
                DpoPair {
                    state: MockGoal::open("root"),
                    winning_tactic: "tA".into(),
                    losing_tactic: "simp_fails".into(),
                },
                DpoPair {
                    state: MockGoal::open("root"),
                    winning_tactic: "tA".into(),
                    losing_tactic: "ring_fails".into(),
                },
                DpoPair {
                    state: MockGoal::open("A"),
                    winning_tactic: "aClose".into(),
                    losing_tactic: "omega_fails".into(),
                },
            ]
        );
    }

    #[test]
    fn dpo_pairs_empty_without_a_solution() {
        // No closing edge ⇒ unsolved ⇒ no preference pairs even with discards.
        let mut scorer = TableScorer::new()
            .edge("root", "t", 0.5f64.ln(), MockGoal::open("stuck"))
            .discard("root", "bad");
        let out = best_first_search(
            &mut scorer,
            MockGoal::open("root"),
            &BestFirstConfig {
                max_steps: 10,
                ..BestFirstConfig::default()
            },
        );
        assert!(!out.solved);
        assert!(dpo_pairs(&out).is_empty());
    }

    // ---- Gate-re-checked proof minimization ----------------------------------

    /// A solved search whose arena hides a shorter closing route than the line
    /// best-first actually closed on. The high-log-prob two-step line
    /// root -t1-> A -t3-> G(closed) wins the frontier race, but the frontier also
    /// discovered root -t2-> C(closed), a length-one close. Projecting the arena to
    /// a DAG exposes that shortcut to the minimizer's BFS.
    fn shortcut_scorer() -> TableScorer {
        TableScorer::new()
            .edge("root", "t1", 0.99f64.ln(), MockGoal::open("A"))
            .edge("root", "t2", 0.5f64.ln(), MockGoal::closed("C"))
            .edge("A", "t3", 0.99f64.ln(), MockGoal::closed("G"))
    }

    #[test]
    fn minimized_proof_accepts_a_gate_checked_shortcut() {
        let mut scorer = shortcut_scorer();
        let out = best_first_search(
            &mut scorer,
            MockGoal::open("root"),
            &BestFirstConfig::default(),
        );
        assert!(out.solved);
        // Best-first closed on the high-log-prob two-step line, not the shortcut.
        assert_eq!(out.proof_tactics(), vec!["t1", "t3"]);
        let original = out.proof_tactics();

        // A gate that re-checks and accepts the shorter route shrinks the proof;
        // the gate is what makes the shrink emittable, and it saw the candidate.
        let mut seen: Vec<Vec<String>> = Vec::new();
        let outcome = out.minimized_proof(|cand| {
            seen.push(cand.to_vec());
            true
        });
        assert_eq!(outcome.status, MinimizeStatus::Verified);
        assert_eq!(outcome.accepted, Some(vec!["t2".to_string()]));
        assert_eq!(
            seen,
            vec![vec!["t2".to_string()]],
            "the gate must be handed the exact shrunk sequence to re-check"
        );
        assert!(
            outcome.best_safe(&original).len() < original.len(),
            "the accepted shrink is strictly shorter than the closed line"
        );
        assert_eq!(outcome.best_safe(&original), ["t2".to_string()]);
    }

    #[test]
    fn minimized_proof_keeps_the_original_when_the_gate_rejects() {
        let mut scorer = shortcut_scorer();
        let out = best_first_search(
            &mut scorer,
            MockGoal::open("root"),
            &BestFirstConfig::default(),
        );
        let original = out.proof_tactics();

        // A gate that rejects every shorter candidate leaves `accepted` empty, so
        // the caller falls back to the original line that passed a gate upstream.
        let outcome = out.minimized_proof(|_| false);
        assert_eq!(outcome.status, MinimizeStatus::RejectedByGate);
        assert_eq!(outcome.accepted, None);
        assert_eq!(outcome.best_safe(&original), original.as_slice());
    }

    #[test]
    fn minimized_proof_fabricates_nothing_for_an_unsolved_search() {
        // A chain that never closes within budget: the minimizer must report
        // NoProofFound and never consult the gate, so no proof is invented for a
        // search that did not close a goal.
        let mut scorer = TableScorer::new().edge("root", "t", 0.9f64.ln(), MockGoal::open("stuck"));
        let out = best_first_search(
            &mut scorer,
            MockGoal::open("root"),
            &BestFirstConfig {
                max_steps: 5,
                ..BestFirstConfig::default()
            },
        );
        assert!(!out.solved);

        let mut gate_calls = 0;
        let outcome = out.minimized_proof(|_| {
            gate_calls += 1;
            true
        });
        assert_eq!(
            gate_calls, 0,
            "an unsolved search must not consult the gate"
        );
        assert_eq!(outcome.status, MinimizeStatus::NoProofFound);
        assert_eq!(outcome.accepted, None);
        assert_eq!(outcome.candidate, None);
    }
}
