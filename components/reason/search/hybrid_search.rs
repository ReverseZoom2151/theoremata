//! Hybrid router: fuse **value-free best-first** search
//! ([`super::best_first`], the BFS-Prover pattern) with **critic-guided MCGS**
//! ([`super::driver`], the AlphaProof/MCGS pattern), plus a **multi-alpha
//! accumulative union** over best-first.
//!
//! Both BFS-Prover and InternLM report the same empirical result: a *hybrid* of a
//! value-free frontier search and a critic/value-guided tree search beats either
//! alone, because the two explore **disjoint** regions of proof space. The
//! value-free search follows raw policy log-prob greedily (cheap, wide, no value
//! signal); the critic-guided search backs value up a DAG and re-allocates
//! compute toward states the critic rates highly (expensive, deep, exploits a
//! learned V(s)). Running both and unioning the solved paths strictly dominates
//! running one.
//!
//! This module is the **routing + union** layer, and it is deliberately *offline
//! and deterministic*. It contains no policy and no critic — those are the
//! GPU-gated seams that live behind [`TacticScorer`] (for best-first) and the
//! driver's `TacticExpander` / `CriticScorer` (for MCGS). What lives here is:
//!
//! * [`multi_alpha_union`] — run [`best_first_search`] once per length-normalization
//!   `alpha`, union the visited states and the solved paths, and return the
//!   shortest solution together with coverage statistics. It *reuses* best-first
//!   verbatim; it does not reimplement search.
//! * [`multi_alpha_union_minimized`]: the same sweep, then a proof shrink gated on
//!   a **caller-supplied** re-check. The re-check is a parameter because nothing in
//!   this module can execute a tactic: the scorer proposes and ranks, it never
//!   verifies, so it must never be the gate. See that function's docs.
//! * [`HybridPlan`] + [`split_budget`] + [`route`] — split a total compute budget
//!   between the best-first side and the critic-guided-driver side from cheap goal
//!   features. A [`super::ttc::TtcController`] is the intended caller (see the
//!   wiring note on [`route`]).
//! * [`run_alpha_sweep_search`]: the production entry point. It is the only item
//!   here that talks to the outside world: it builds a model-backed scorer, runs
//!   the sweep, optionally runs the minimizer behind a real
//!   [`GateReplay`](crate::prover::session::replay::GateReplay), records a store
//!   event, and returns a JSON summary. Everything below it stays pure.
//! * [`run_split`] — a tiny driver-agnostic combinator that runs the best-first
//!   side, then the MCGS side, on their respective budget shares and unions the
//!   outcome. The MCGS side is passed in as a **closure seam** so this module
//!   never has to name the driver's generic `TacticExpander` parameters; a caller
//!   wraps `ProofSearchDriver::run` in the closure.
//!
//! ## Determinism contract
//!
//! Everything here is a pure function of its inputs. There is no wall-clock and no
//! unseeded randomness: [`multi_alpha_union`] iterates `alphas` in the given order
//! and reuses best-first's own deterministic tie-breaking; the union is collected
//! into a [`BTreeSet`] so its enumeration is sorted and stable. Given the same
//! scorer, root, alphas, and budget it returns byte-identical results.

use super::best_first::{
    best_first_search, best_first_search_with_critic, BestFirstConfig, BestFirstOutcome,
    ExpanderScorer, TacticScorer,
};
use super::critic_scorer::{critic_from_config, CriticScorer};
use super::driver::{GoalState, TacticExpander, TacticStep};
use super::mcts::SearchConfig;
use super::minimize::MinimizeOutcome;
use crate::{
    config::Config,
    db::Store,
    model::ModelRequest,
    prover::{
        formal::{backend_for, FormalBackend, FormalSystem},
        session::replay::{self, GateReplay},
    },
    provider::ModelProvider,
};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Multi-alpha accumulative union (over the value-free best-first search)
// ---------------------------------------------------------------------------

/// The result of one best-first pass at a fixed `alpha`, retained so a caller can
/// see which `alpha` covered what.
#[derive(Debug, Clone, PartialEq)]
pub struct AlphaRun {
    /// The length-normalization exponent this pass used.
    pub alpha: f64,
    /// Whether this pass reached a closed goal.
    pub solved: bool,
    /// Expansions (states popped and scored) this pass performed.
    pub steps: usize,
    /// Length of the proof this pass found (`0` when unsolved).
    pub proof_len: usize,
    /// Distinct states this pass visited (its `order.len()`).
    pub visited: usize,
}

/// The fused outcome of a [`multi_alpha_union`] sweep: the shortest solution found
/// across all `alpha` passes plus the coverage the sweep achieved.
#[derive(Debug, Clone, PartialEq)]
pub struct HybridOutcome {
    /// Whether *any* alpha pass solved the goal.
    pub solved: bool,
    /// The shortest solution path (fewest tactics) across all solving passes.
    /// Ties break toward the earlier `alpha` in the input slice. Empty when
    /// unsolved.
    pub proof_tactics: Vec<String>,
    /// The `alpha` that produced [`Self::proof_tactics`], if solved.
    pub best_alpha: Option<f64>,
    /// One entry per input `alpha`, in input order.
    pub runs: Vec<AlphaRun>,
    /// The union of every state key visited across all passes, sorted. Its length
    /// is the accumulative coverage — strictly larger than any single pass's
    /// `visited` whenever different alphas explore disjoint regions.
    pub union_keys: Vec<String>,
}

impl HybridOutcome {
    /// Distinct states covered across the whole sweep (`union_keys.len()`).
    pub fn union_coverage(&self) -> usize {
        self.union_keys.len()
    }
}

/// Run [`best_first_search`] once per `alpha` and accumulate the union.
///
/// Each pass starts from a fresh clone of `root` and uses `per_alpha_budget` as its
/// expansion cap; the same `scorer` (the injected policy seam) backs every pass.
/// The union over passes is what makes this a *multi-alpha accumulative* search:
/// a shallow-biased `alpha` (near `0`, maximal depth penalty) and a
/// depth-neutral `alpha` (near `1`) pop different frontiers, so together they
/// cover states — and can find solutions — that neither reaches alone.
///
/// Deterministic: passes run in `alphas` order, best-first is itself deterministic,
/// and the shortest-solution tie-break is stable (earliest `alpha` wins).
pub fn multi_alpha_union<Sc: TacticScorer>(
    scorer: &mut Sc,
    root: Sc::State,
    alphas: &[f64],
    per_alpha_budget: usize,
) -> HybridOutcome {
    // No critic and a zero weight: the pre-seam sweep, unchanged.
    multi_alpha_union_inner(scorer, root, alphas, per_alpha_budget, 0.0, None).0
}

/// [`multi_alpha_union`] with a trained state-value critic steering every pass.
///
/// The SAME `Arc` is cloned into each alpha pass, so all arms of the sweep see one
/// critic object and therefore identical `V(s)` for identical states. Rebuilding a
/// critic per pass would leave the arms silently incomparable, which would corrupt
/// the union's whole premise (that the passes differ only in `alpha`).
pub fn multi_alpha_union_with_critic<Sc: TacticScorer>(
    scorer: &mut Sc,
    root: Sc::State,
    alphas: &[f64],
    per_alpha_budget: usize,
    critic_weight: f64,
    critic: Option<Arc<dyn CriticScorer>>,
) -> HybridOutcome {
    multi_alpha_union_inner(
        scorer,
        root,
        alphas,
        per_alpha_budget,
        critic_weight,
        critic,
    )
    .0
}

/// The sweep, additionally returning the [`BestFirstOutcome`] of the pass that
/// produced the reported (shortest) solution.
///
/// That outcome is retained only so [`multi_alpha_union_minimized`] can reach its
/// search arena; the arena is what a minimizer's BFS needs, and it cannot be
/// reconstructed from a [`HybridOutcome`], which keeps only the tactic list. It is
/// `None` when no pass solved.
fn multi_alpha_union_inner<Sc: TacticScorer>(
    scorer: &mut Sc,
    root: Sc::State,
    alphas: &[f64],
    per_alpha_budget: usize,
    critic_weight: f64,
    critic: Option<Arc<dyn CriticScorer>>,
) -> (HybridOutcome, Option<BestFirstOutcome<Sc::State>>) {
    let mut runs: Vec<AlphaRun> = Vec::with_capacity(alphas.len());
    let mut union: BTreeSet<String> = BTreeSet::new();
    // Best (shortest) solution so far: (proof_len, tactics, alpha).
    let mut best: Option<(usize, Vec<String>, f64)> = None;
    // The search outcome that produced `best`, kept in lockstep with it.
    let mut best_outcome: Option<BestFirstOutcome<Sc::State>> = None;

    for &alpha in alphas {
        let cfg = BestFirstConfig {
            alpha,
            max_steps: per_alpha_budget,
            seed: 0,
            hint_weight: 0.0,
            critic_weight,
        };
        // With no critic this calls the exact entry point the sweep called before,
        // so the default sweep has no critic-shaped code on its path at all. With a
        // critic, every pass gets a CLONE OF THE SAME Arc, so the arms stay
        // comparable.
        let out = match &critic {
            None => best_first_search(scorer, root.clone(), &cfg),
            Some(c) => best_first_search_with_critic(scorer, root.clone(), &cfg, Some(c.clone())),
        };

        for key in &out.order {
            union.insert(key.clone());
        }

        let tactics = out.proof_tactics();
        let proof_len = tactics.len();
        // Read the per-pass stats out before `out` can be moved into `best_outcome`.
        let (pass_solved, pass_steps, pass_visited) = (out.solved, out.steps, out.order.len());

        if pass_solved {
            // Strictly-shorter wins; equal length keeps the earlier alpha (this
            // pass only replaces on `<`), so the tie-break is deterministic.
            let take = match &best {
                None => true,
                Some((best_len, _, _)) => proof_len < *best_len,
            };
            if take {
                best = Some((proof_len, tactics, alpha));
                best_outcome = Some(out);
            }
        }

        runs.push(AlphaRun {
            alpha,
            solved: pass_solved,
            steps: pass_steps,
            proof_len,
            visited: pass_visited,
        });
    }

    let (solved, proof_tactics, best_alpha) = match best {
        Some((_, tactics, alpha)) => (true, tactics, Some(alpha)),
        None => (false, Vec::new(), None),
    };

    (
        HybridOutcome {
            solved,
            proof_tactics,
            best_alpha,
            runs,
            union_keys: union.into_iter().collect(),
        },
        best_outcome,
    )
}

/// Run the sweep, then shrink the winning proof **behind a caller-supplied
/// re-check**.
///
/// This is [`multi_alpha_union`] plus one extra step: the winning pass's arena is
/// handed to [`BestFirstOutcome::minimized_proof`], which BFS-searches the arena
/// (projected to a DAG, so transpositions expose shortcuts the log-prob-ordered
/// line missed) for a shorter closing sequence and then asks `replay` whether that
/// sequence actually closes the goal. The returned [`MinimizeOutcome`] is
/// `Some` only when a pass solved; it is `None` otherwise, and `replay` is never
/// consulted for an unsolved sweep.
///
/// # `replay` is the entire soundness boundary
///
/// `replay(seq) -> bool` must return `true` **only** if executing exactly `seq`
/// from the root goal against a real proof checker leaves no open goal. Nothing in
/// this module can establish that, which is why it is a parameter rather than
/// something built here:
///
/// * The only capability in scope is [`TacticScorer`], and
///   [`score`](TacticScorer::score) is a *proposer*: it returns candidate tactics
///   with policy log-probabilities and a `next` state it asserts the tactic yields.
///   It never executes anything, so it cannot witness closure.
/// * A state's [`is_closed`](super::driver::GoalState::is_closed) is likewise just
///   a flag on a state the scorer itself produced, not a checker verdict.
///
/// So using the scorer (or an
/// [`ExpanderScorer`](super::best_first::ExpanderScorer)-wrapped expander, or the
/// MCGS critic) as `replay` would be circular: the component that guessed the
/// shrink would be the one confirming it, and a shorter-but-wrong sequence would be
/// promoted to `accepted` and emitted as a proof. Do not do that. A real `replay`
/// closes over a checker: `ProofSession::step_tactic` (`components/prover/formal.rs`)
/// stepped over the sequence, `LeanSession::check`
/// (`components/verify/lean_session.rs`) on the assembled source, or
/// `FormalBackend::verify` (`components/prover/formal.rs`).
///
/// # Cost
///
/// Every call runs one real checker pass, so this entry point is opt-in: plain
/// [`multi_alpha_union`] is unchanged and stays free of prover calls. Callers with
/// no checker wired up should keep using it rather than passing a fake `replay`.
///
/// Emit only [`MinimizeOutcome::accepted`], or fall back through
/// [`MinimizeOutcome::best_safe`] to the original line, which passed a gate
/// upstream. Never emit [`MinimizeOutcome::candidate`] on its own.
pub fn multi_alpha_union_minimized<Sc: TacticScorer, F>(
    scorer: &mut Sc,
    root: Sc::State,
    alphas: &[f64],
    per_alpha_budget: usize,
    replay: F,
) -> (HybridOutcome, Option<MinimizeOutcome>)
where
    F: FnMut(&[String]) -> bool,
{
    multi_alpha_union_minimized_with_critic(scorer, root, alphas, per_alpha_budget, 0.0, None, replay)
}

/// [`multi_alpha_union_minimized`] with a critic steering the sweep.
///
/// The critic reaches the SEARCH only. `replay` remains the entire soundness
/// boundary, and the critic is not passed to it, cannot be it, and does not
/// influence which sequence is handed to it beyond having changed the order the
/// frontier was explored in. Everything the doc on
/// [`multi_alpha_union_minimized`] says about `replay` applies here verbatim: in
/// particular, do not use a critic as `replay`.
#[allow(clippy::too_many_arguments)]
pub fn multi_alpha_union_minimized_with_critic<Sc: TacticScorer, F>(
    scorer: &mut Sc,
    root: Sc::State,
    alphas: &[f64],
    per_alpha_budget: usize,
    critic_weight: f64,
    critic: Option<Arc<dyn CriticScorer>>,
    replay: F,
) -> (HybridOutcome, Option<MinimizeOutcome>)
where
    F: FnMut(&[String]) -> bool,
{
    let (outcome, best_outcome) = multi_alpha_union_inner(
        scorer,
        root,
        alphas,
        per_alpha_budget,
        critic_weight,
        critic,
    );
    // No solving pass ⇒ no arena to shrink and nothing to re-check. Returning None
    // (rather than an empty minimize outcome) keeps "we never ran the gate"
    // distinguishable from "the gate rejected", which the caller logs differently.
    let minimized = best_outcome.map(|out| out.minimized_proof(replay));
    (outcome, minimized)
}

// ---------------------------------------------------------------------------
// Budget routing between the best-first and critic-guided-driver sides
// ---------------------------------------------------------------------------

/// Cheap, model-free features describing a goal, used by [`route`] to split its
/// budget. A real caller fills these from the goal state (its
/// [`difficulty`](super::driver::GoalState::difficulty)) and from whether a trained
/// critic is wired into the driver.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GoalFeatures {
    /// Difficulty estimate in `[0, 1]` (`1.0` ⇒ hardest). Harder goals send more
    /// budget to the critic-guided side, where disjoint deep exploration pays off.
    pub difficulty: f64,
    /// Whether a trained state-value critic is available to steer MCGS. With no
    /// critic the MCGS side has nothing to guide it, so the whole budget goes to
    /// the value-free best-first side.
    pub critic_available: bool,
}

impl GoalFeatures {
    /// Build the features with `critic_available` derived from the ONE factory that
    /// decides whether a critic exists, [`critic_from_config`].
    ///
    /// Setting the flag by hand is how it becomes a lie: a caller can claim a critic
    /// and route budget to an arm that then runs unguided, or deny one that is
    /// actually wired. Deriving it from the same `SearchConfig` the search itself is
    /// built from makes "available" mean exactly "the search will receive one".
    ///
    /// Note this is still only used by [`route`], and [`route`] still has no
    /// production caller. Deriving the flag removes the possibility of it being
    /// wrong; it does not by itself put the routing on the live path.
    pub fn from_config(difficulty: f64, cfg: &SearchConfig) -> Self {
        Self {
            difficulty,
            critic_available: critic_from_config(cfg).is_some(),
        }
    }
}

/// How a total compute budget is partitioned between the value-free best-first
/// search and the critic-guided MCGS driver. `bf_budget + mcgs_budget` always
/// equals the `total` passed to [`split_budget`] / [`route`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HybridPlan {
    /// Expansion budget for the value-free best-first side.
    pub bf_budget: usize,
    /// Iteration budget for the critic-guided MCGS driver side.
    pub mcgs_budget: usize,
}

/// Lower bound on the best-first share when a critic *is* available — keeps the
/// value-free side alive even for the hardest goals so both frontiers are always
/// explored (the whole point of the hybrid).
const BF_RATIO_MIN: f64 = 0.2;
/// Upper bound on the best-first share when a critic is available — keeps the
/// critic-guided side alive even for the easiest goals.
const BF_RATIO_MAX: f64 = 0.8;

/// Partition `total` into `(best_first, mcgs)` by `ratio` — the fraction going to
/// best-first. `ratio` is clamped to `[0, 1]`; best-first gets `round(total *
/// ratio)` (never more than `total`) and MCGS gets the exact remainder, so the two
/// always sum to `total` with no rounding leak.
pub fn split_budget(total: usize, ratio: f64) -> (usize, usize) {
    let r = ratio.clamp(0.0, 1.0);
    let bf = ((total as f64) * r).round() as usize;
    let bf = bf.min(total);
    (bf, total - bf)
}

/// Route a `total_budget` between the two searches from goal features (the method
/// a [`super::ttc::TtcController`] would call to size the hybrid's two arms).
///
/// Policy: the value-free best-first search is the default workhorse. The
/// critic-guided MCGS side only earns budget when a critic is available to steer
/// it, and earns *more* of it as the goal gets harder (higher `difficulty` ⇒ lower
/// best-first ratio), bounded to `[BF_RATIO_MIN, BF_RATIO_MAX]` so neither arm is
/// ever fully starved when both are viable. With no critic, best-first takes the
/// whole budget.
///
/// The returned [`HybridPlan`] always satisfies `bf_budget + mcgs_budget ==
/// total_budget` and each arm `<= total_budget`.
pub fn route(features: GoalFeatures, total_budget: usize) -> HybridPlan {
    let bf_ratio = if !features.critic_available {
        1.0
    } else {
        (1.0 - 0.6 * features.difficulty.clamp(0.0, 1.0)).clamp(BF_RATIO_MIN, BF_RATIO_MAX)
    };
    let (bf_budget, mcgs_budget) = split_budget(total_budget, bf_ratio);
    HybridPlan {
        bf_budget,
        mcgs_budget,
    }
}

// ---------------------------------------------------------------------------
// Driver-agnostic combinator: run best-first, then MCGS, and union
// ---------------------------------------------------------------------------

/// Which arm of the hybrid produced a solution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HybridSource {
    /// No arm solved the goal.
    None,
    /// The value-free best-first search solved it.
    BestFirst,
    /// The critic-guided MCGS driver solved it.
    Mcgs,
}

/// The unioned outcome of running both arms under a [`HybridPlan`].
#[derive(Debug, Clone, PartialEq)]
pub struct HybridSolution {
    /// Whether either arm solved the goal.
    pub solved: bool,
    /// The winning proof path (from whichever arm solved first). Empty when unsolved.
    pub proof_tactics: Vec<String>,
    /// Which arm produced [`Self::proof_tactics`].
    pub source: HybridSource,
}

/// Run the two arms of the hybrid on their [`HybridPlan`] budget shares and union
/// the result, returning the first arm to solve.
///
/// This is the composition seam. The best-first arm is a closure `run_bf(budget) ->
/// Option<proof_tactics>` and the MCGS arm is `run_mcgs(budget) ->
/// Option<proof_tactics>`; a caller builds them from the concrete backends it has:
///
/// ```ignore
/// // Value-free arm: adapt the driver's TacticExpander into a TacticScorer and
/// // run best-first (or run multi_alpha_union) under bf_budget.
/// let run_bf = |budget: usize| {
///     let mut scorer = ExpanderScorer(make_expander());
///     let out = best_first_search(&mut scorer, root.clone(),
///         &BestFirstConfig { max_steps: budget, ..Default::default() });
///     out.solved.then(|| out.proof_tactics())
/// };
/// // Critic-guided arm: run the MCGS driver (with its critic) under mcgs_budget.
/// let run_mcgs = |budget: usize| {
///     let mut driver = ProofSearchDriver::new(make_expander())
///         .with_critic(critic)
///         .with_config(SearchConfig { max_nodes: budget, ..Default::default() });
///     let r = driver.run(root.clone());
///     r.solved.then(|| r.best_tactic.into_iter().collect())
/// };
/// let sol = run_split(&plan, run_bf, run_mcgs);
/// ```
///
/// Keeping the arms as `FnOnce` closures means this module never names the driver's
/// generic parameters; the driver stays a documented seam. Best-first runs first
/// (cheap, value-free), so a goal it already closes never pays for the critic pass.
/// The order is fixed and both closures are pure of the caller's making, so the
/// combinator is deterministic.
pub fn run_split<BF, MC>(plan: &HybridPlan, run_bf: BF, run_mcgs: MC) -> HybridSolution
where
    BF: FnOnce(usize) -> Option<Vec<String>>,
    MC: FnOnce(usize) -> Option<Vec<String>>,
{
    if plan.bf_budget > 0 {
        if let Some(proof_tactics) = run_bf(plan.bf_budget) {
            return HybridSolution {
                solved: true,
                proof_tactics,
                source: HybridSource::BestFirst,
            };
        }
    }
    if plan.mcgs_budget > 0 {
        if let Some(proof_tactics) = run_mcgs(plan.mcgs_budget) {
            return HybridSolution {
                solved: true,
                proof_tactics,
                source: HybridSource::Mcgs,
            };
        }
    }
    HybridSolution {
        solved: false,
        proof_tactics: Vec::new(),
        source: HybridSource::None,
    }
}

// ---------------------------------------------------------------------------
// Production entry point: model-backed sweep, optionally gated by a real checker
// ---------------------------------------------------------------------------

/// Default alpha sweep when a caller supplies none: maximal depth penalty,
/// the best-first default, and no depth penalty. Three passes that pop visibly
/// different frontiers, which is the whole point of the union.
pub const DEFAULT_ALPHAS: [f64; 3] = [0.0, 0.5, 1.0];

/// Hard ceiling on model calls in one sweep. Each distinct goal state costs one
/// provider round trip (results are memoized, so an alpha re-visiting a state is
/// free), and a runaway search would otherwise bill an unbounded number of them.
/// Hitting the cap turns further states into dead ends, which can only make the
/// search report LESS than it otherwise would, never more, so it is safe. It is
/// surfaced in the summary so a truncated run is never mistaken for an exhausted
/// one.
const MAX_MODEL_CALLS: usize = 64;

/// A goal state in the model-driven search: the goal text (which doubles as the
/// transposition key) plus the model's own claim about whether it is closed.
///
/// `closed` is a CLAIM, not a verdict. Nothing in this type has executed a
/// tactic, so a `true` here means only "the proposer said this discharges the
/// goal". That is exactly why the search output is a candidate and why the
/// minimizer's gate is a separate, checker-backed component.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderGoal {
    key: String,
    closed: bool,
}

impl GoalState for ProviderGoal {
    fn dedup_key(&self) -> String {
        self.key.clone()
    }
    fn is_closed(&self) -> bool {
        self.closed
    }
}

/// A [`TacticExpander`] backed by a [`ModelProvider`]: ask the model for the next
/// tactics from a goal, each with a prior and the goal text it claims results.
///
/// This is the same shape as [`crate::search::mcts::TacticMcts::propose_tactics`]
/// (the established model-driven-search pattern in this codebase), extended with
/// the resulting goal, because a *search* needs successor states and a flat tactic
/// list does not provide them. Wrapping it in
/// [`ExpanderScorer`](super::best_first::ExpanderScorer) turns the `[0,1]` priors
/// into the log-probabilities best-first orders its frontier by, so no new scorer
/// type is needed.
///
/// Not deterministic in the strict sense the rest of this module is: the provider
/// is an external process. The memo makes it *consistent within one sweep* (a
/// state is asked about exactly once, so every alpha pass sees the same edges out
/// of it), which is what makes the union across alphas meaningful.
pub struct ProviderExpander<'a> {
    provider: &'a dyn ModelProvider,
    /// The root statement, passed as context on every call so the proposer keeps
    /// sight of what is ultimately being proved.
    statement: String,
    system: FormalSystem,
    /// goal key -> the steps out of it. One provider call per distinct state.
    memo: HashMap<String, Vec<TacticStep<ProviderGoal>>>,
    /// Provider round trips actually made.
    pub calls: usize,
    /// Provider round trips that failed. A failure is a dead end, never a proof.
    pub errors: usize,
    /// Whether [`MAX_MODEL_CALLS`] cut the search short.
    pub call_cap_hit: bool,
}

impl<'a> ProviderExpander<'a> {
    pub fn new(provider: &'a dyn ModelProvider, statement: &str, system: FormalSystem) -> Self {
        Self {
            provider,
            statement: statement.to_string(),
            system,
            memo: HashMap::new(),
            calls: 0,
            errors: 0,
            call_cap_hit: false,
        }
    }

    /// One provider round trip for `goal`. Errors and malformed responses become
    /// an empty step list (a dead end), so a flaky model degrades the search
    /// rather than corrupting it.
    fn ask(&mut self, goal: &str) -> Vec<TacticStep<ProviderGoal>> {
        let request = ModelRequest {
            role: "tactic_step_proposer".into(),
            task: format!(
                "Propose the next candidate {} tactics for the current goal. For each, give a \
                 prior weight in [0,1] (higher = more promising), the goal text that remains \
                 after applying it, and whether it closes the goal outright. Order most \
                 promising first.",
                self.system.as_str()
            ),
            context: json!({
                "statement": self.statement,
                "goal": goal,
                "system": self.system.as_str(),
            }),
            output_schema: json!({
                "type": "object",
                "required": ["steps"],
                "properties": {
                    "steps": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["tactic", "weight", "next_goal", "closes_goal"],
                            "properties": {
                                "tactic": {"type": "string"},
                                "weight": {"type": "number"},
                                "next_goal": {"type": "string"},
                                "closes_goal": {"type": "boolean"}
                            }
                        }
                    }
                }
            }),
        };
        self.calls += 1;
        let response = match self.provider.complete(&request) {
            Ok(r) => r,
            Err(_) => {
                self.errors += 1;
                return Vec::new();
            }
        };
        let mut steps = Vec::new();
        let Some(items) = response.content["steps"].as_array() else {
            return steps;
        };
        for item in items {
            let Some(tactic) = item["tactic"].as_str() else {
                continue;
            };
            if tactic.trim().is_empty() {
                continue;
            }
            let closed = item["closes_goal"].as_bool().unwrap_or(false);
            let next_goal = item["next_goal"].as_str().unwrap_or("").trim().to_string();
            // A closing step often has no residual goal text. Key it on the edge
            // that produced it so two different closing tactics stay two states
            // rather than collapsing onto one via an empty key.
            let key = if next_goal.is_empty() {
                format!("{goal}|{tactic}")
            } else {
                next_goal
            };
            let prior = item["weight"].as_f64().unwrap_or(0.5).clamp(0.0, 1.0);
            steps.push(TacticStep::new(
                tactic.to_string(),
                prior,
                ProviderGoal { key, closed },
            ));
        }
        steps
    }
}

impl TacticExpander for ProviderExpander<'_> {
    type State = ProviderGoal;

    fn expand(&mut self, state: &ProviderGoal, _seed: u64) -> Vec<TacticStep<ProviderGoal>> {
        if let Some(hit) = self.memo.get(&state.key) {
            return hit.clone();
        }
        if self.calls >= MAX_MODEL_CALLS {
            self.call_cap_hit = true;
            return Vec::new();
        }
        let steps = self.ask(&state.key);
        self.memo.insert(state.key.clone(), steps.clone());
        steps
    }
}

/// Run the multi-alpha sweep over a model-backed scorer and, when `minimize` is
/// set AND a live checker exists, shrink the winning line behind a real
/// [`GateReplay`]. Returns a JSON summary and records a `hybrid_search.swept`
/// store event.
///
/// # What the result means
///
/// The sweep itself proves NOTHING. Its "solved" means the proposer's own claimed
/// successor chain reached a state the proposer marked closed; no formal system
/// was consulted. That is why `formally_verified` is `false` and `status` is
/// `"candidate"` for every run in which the gate did not accept something.
///
/// When the gate DOES accept, the claim is precise and it is not nothing:
/// [`GateReplay`] assembled that exact tactic sequence into a source file under
/// this statement and ran the full [`crate::prover::formal::FormalBackend::verify`]
/// gate over it (compile, axiom whitelist, kernel re-check, source scan, statement
/// preservation) on a live, non-mock backend. So `verified_tactics` is a checked
/// proof of `statement` in `system`, and only that field ever is. The sweep's own
/// `candidate_tactics` stays a candidate even then, because it is a different
/// (longer) sequence that was never submitted to the checker.
///
/// # "Did not run" is not "was rejected"
///
/// `GateReplay` refuses every sequence when it has no evidence: mock backend,
/// missing toolchain, unassemblable statement. Reporting that as a rejection would
/// slander a possibly-correct shrink and would read as a search failure. So this
/// function probes the backend FIRST and, with no live checker, never constructs
/// the gate at all: `minimization.ran` is `false` and `skipped_reason` says which
/// precondition was missing. `minimization.status` is populated only when the gate
/// really ran, and `"rejected_by_gate"` therefore always means a live checker
/// looked at the shrink and said no.
///
/// # Cost
///
/// Two separate expenses, both opt-in by the caller:
/// * the sweep spends up to [`MAX_MODEL_CALLS`] provider round trips;
/// * `minimize = true` additionally spends one real checker pass (a full compile)
///   when the search solved and a live backend exists. Pass `false` for a
///   model-only exploration.
#[allow(clippy::too_many_arguments)]
pub fn run_alpha_sweep_search(
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    project_id: Option<&str>,
    statement: &str,
    system: FormalSystem,
    alphas: &[f64],
    per_alpha_budget: usize,
    minimize: bool,
    critic_weight: f64,
) -> Result<Value> {
    let statement = statement.trim();
    if statement.is_empty() {
        bail!("hybrid alpha sweep needs a non-empty statement");
    }
    let alphas: Vec<f64> = if alphas.is_empty() {
        DEFAULT_ALPHAS.to_vec()
    } else {
        alphas.to_vec()
    };
    let budget = per_alpha_budget.max(1);

    // Probe the backend before deciding whether the gate can mean anything. A
    // mock or an absent toolchain is a MISSING PRECONDITION, not a verdict, so it
    // must be detected here rather than inferred from a refusal downstream.
    let backend = backend_for(config, system, config.prover_mock);
    let checker_live = !backend.is_mock() && backend.available();
    drop(backend);

    let root = ProviderGoal {
        key: statement.to_string(),
        closed: false,
    };
    let mut scorer = ExpanderScorer(ProviderExpander::new(provider, statement, system));

    // The production critic gate. `critic_from_config` returns `None` at
    // `critic_weight == 0.0` (the default), so the shipped path builds no critic and
    // the sweep is exactly what it was. A non-zero weight yields the deterministic
    // `HeuristicCritic`; swapping a trained value head in later is a one-line change
    // inside that factory, with nothing here to touch.
    let search_cfg = SearchConfig {
        critic_weight,
        ..SearchConfig::default()
    };
    let critic = critic_from_config(&search_cfg);
    let critic_available = critic.is_some();

    // `skipped_reason` is set on every path that does not run the gate, so the
    // summary can always say WHY rather than leaving a reader to guess.
    let (outcome, minimized, mut skipped_reason) = if !minimize {
        (
            multi_alpha_union_with_critic(
                &mut scorer,
                root,
                &alphas,
                budget,
                critic_weight,
                critic.clone(),
            ),
            None,
            Some("not_requested"),
        )
    } else if !checker_live {
        (
            multi_alpha_union_with_critic(
                &mut scorer,
                root,
                &alphas,
                budget,
                critic_weight,
                critic.clone(),
            ),
            None,
            Some("no_live_checker"),
        )
    } else {
        let mut gate = GateReplay::for_system(config, system, statement);
        let (outcome, minimized) = multi_alpha_union_minimized_with_critic(
            &mut scorer,
            root,
            &alphas,
            budget,
            critic_weight,
            critic.clone(),
            replay::as_closure(&mut gate),
        );
        // `None` here means the sweep never solved, so there was no arena to
        // shrink and the gate was deliberately never consulted.
        let reason = minimized.is_none().then_some("search_found_no_proof");
        (outcome, minimized, reason)
    };

    let gate_ran = minimized.is_some();
    if gate_ran {
        skipped_reason = None;
    }
    let accepted = minimized.as_ref().and_then(|m| m.accepted.clone());
    let formally_verified = accepted.is_some();

    let runs: Vec<Value> = outcome
        .runs
        .iter()
        .map(|r| {
            json!({
                "alpha": r.alpha,
                "solved": r.solved,
                "steps": r.steps,
                "proof_len": r.proof_len,
                "visited": r.visited,
            })
        })
        .collect();

    // Computed outside the `json!` so the fallible conversions are plain
    // statements rather than `?` buried inside a macro argument.
    let minimize_status = match &minimized {
        Some(m) => serde_json::to_value(m.status)?,
        None => Value::Null,
    };
    let minimize_report = match &minimized {
        Some(m) => serde_json::to_value(&m.report)?,
        None => Value::Null,
    };
    let minimization = json!({
        "requested": minimize,
        "checker_live": checker_live,
        "ran": gate_ran,
        "skipped_reason": skipped_reason,
        "status": minimize_status,
        "shrink_accepted": formally_verified,
        "accepted_tactics": accepted.clone(),
        "report": minimize_report,
    });

    let expander = &scorer.0;
    let summary = json!({
        "kind": "hybrid_alpha_sweep",
        // These two keys are the guard against a reader promoting a model-driven
        // search outcome to a proof. They flip only when the real gate accepted.
        "status": if formally_verified { "gate_verified" } else { "candidate" },
        "formally_verified": formally_verified,
        "statement": statement,
        "system": system.as_str(),
        "alphas": alphas,
        "per_alpha_budget": budget,
        "solved": outcome.solved,
        // Always a candidate: this is the proposer's own chain, unchecked.
        "candidate_tactics": outcome.proof_tactics,
        "best_alpha": outcome.best_alpha,
        "union_coverage": outcome.union_keys.len(),
        "runs": runs,
        // Reported so a reader can tell a critic-steered sweep from a policy-only
        // one after the fact. `available` is the factory's verdict, not a wish, so
        // it cannot claim a critic the search did not receive. Neither field says
        // anything about correctness: the critic only reordered exploration.
        "critic": {
            "weight": critic_weight,
            "available": critic_available,
        },
        // The ONLY field that may be emitted as a proof, and only when non-null.
        "verified_tactics": accepted,
        "minimization": minimization,
        "model": {
            "provider": provider.name(),
            "calls": expander.calls,
            "errors": expander.errors,
            "call_cap_hit": expander.call_cap_hit,
        },
        "note": "Search over model-proposed tactics and model-claimed successor \
                 goals. 'solved' means the proposer's chain reached a state it \
                 claimed closed; no formal system verified that. Only \
                 'verified_tactics' (non-null) was re-checked by a live backend \
                 through the full verify gate under this statement.",
    });

    store.event(
        project_id,
        None,
        "hybrid_search.swept",
        "hybrid_search",
        summary.clone(),
    )?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::super::best_first::{ScoredExpansion, ScoredTactic};
    use super::super::driver::GoalState;
    use super::super::minimize::MinimizeStatus;
    use super::*;
    use std::cell::Cell;
    use std::collections::HashMap;

    // ---- Deterministic mocks (self-contained; do not reuse best_first's) -----

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

    /// A table-driven [`TacticScorer`]: state key -> live scored edges. Missing
    /// keys are dead ends. Deterministic; ignores the seed.
    struct TableScorer {
        live: HashMap<String, Vec<ScoredTactic<MockGoal>>>,
    }
    impl TableScorer {
        fn new() -> Self {
            Self {
                live: HashMap::new(),
            }
        }
        fn edge(mut self, from: &str, tactic: &str, logprob: f64, to: MockGoal) -> Self {
            self.live
                .entry(from.into())
                .or_default()
                .push(ScoredTactic::new(tactic, logprob, to));
            self
        }
    }
    impl TacticScorer for TableScorer {
        type State = MockGoal;
        fn score(&mut self, state: &MockGoal, _seed: u64) -> ScoredExpansion<MockGoal> {
            ScoredExpansion::live_only(self.live.get(&state.key).cloned().unwrap_or_default())
        }
    }

    /// A state space with two closing paths chosen by different alphas:
    ///  * shallow: root -[sx (ln 0.7)]-> S(closed)              — depth 1, cum -0.357
    ///  * deep:    root -[dy0]-> D0 -[dy1]-> D1 -[dy2]-> D2(closed), each ln 0.8
    ///
    /// alpha=0 (pure cumulative, depth-penalizing) pops shallow S before descending
    /// the whole deep chain, so it solves via `sx` and never visits D1/D2.
    /// alpha=1 (per-step average, no depth penalty) keeps descending the
    /// high-per-step deep chain and solves via `dy0/dy1/dy2`, never visiting S.
    /// Their visited sets are disjoint beyond the shared prefix, so the union
    /// strictly exceeds either.
    fn two_path_scorer() -> TableScorer {
        TableScorer::new()
            .edge("root", "sx", 0.7f64.ln(), MockGoal::closed("S"))
            .edge("root", "dy0", 0.8f64.ln(), MockGoal::open("D0"))
            .edge("D0", "dy1", 0.8f64.ln(), MockGoal::open("D1"))
            .edge("D1", "dy2", 0.8f64.ln(), MockGoal::closed("D2"))
    }

    #[test]
    fn multi_alpha_union_finds_solution_and_unions_disjoint_coverage() {
        let mut scorer = two_path_scorer();
        let out = multi_alpha_union(&mut scorer, MockGoal::open("root"), &[0.0, 1.0], 50);

        assert!(out.solved, "at least one alpha must close the goal");
        // Both alphas solve, via different-length paths.
        assert_eq!(out.runs.len(), 2);
        assert!(out.runs[0].solved && out.runs[1].solved);
        assert_eq!(
            out.runs[0].proof_len, 1,
            "alpha=0 solves via the shallow path"
        );
        assert_eq!(out.runs[1].proof_len, 3, "alpha=1 solves via the deep path");

        // The shortest solution (shallow, alpha=0) is the reported one.
        assert_eq!(out.proof_tactics, vec!["sx"]);
        assert_eq!(out.best_alpha, Some(0.0));

        // Coverage: the union strictly exceeds either single pass — each alpha
        // missed states the other reached.
        let cov = out.union_coverage();
        assert!(
            cov > out.runs[0].visited && cov > out.runs[1].visited,
            "union {cov} must exceed each pass ({}, {})",
            out.runs[0].visited,
            out.runs[1].visited
        );
        // Concretely: union = {root, S, D0, D1, D2} = 5; alpha=0 missed D1,D2.
        assert_eq!(cov, 5);
        assert_eq!(
            out.union_keys,
            vec!["D0", "D1", "D2", "S", "root"], // BTreeSet ⇒ sorted, stable
        );
    }

    #[test]
    fn multi_alpha_union_covers_a_path_a_single_alpha_misses() {
        // A single alpha=0 pass never visits the deep tail D1/D2...
        let mut single = two_path_scorer();
        let solo = multi_alpha_union(&mut single, MockGoal::open("root"), &[0.0], 50);
        assert!(!solo.union_keys.iter().any(|k| k == "D1" || k == "D2"));

        // ...but adding alpha=1 to the sweep brings them into coverage.
        let mut both = two_path_scorer();
        let sweep = multi_alpha_union(&mut both, MockGoal::open("root"), &[0.0, 1.0], 50);
        assert!(sweep.union_keys.iter().any(|k| k == "D1"));
        assert!(sweep.union_keys.iter().any(|k| k == "D2"));
        assert!(sweep.union_coverage() > solo.union_coverage());
    }

    #[test]
    fn multi_alpha_union_unsolved_when_no_alpha_closes() {
        let mut scorer = TableScorer::new().edge("root", "t", 0.5f64.ln(), MockGoal::open("stuck"));
        let out = multi_alpha_union(&mut scorer, MockGoal::open("root"), &[0.0, 0.5, 1.0], 20);
        assert!(!out.solved);
        assert!(out.proof_tactics.is_empty());
        assert_eq!(out.best_alpha, None);
        assert_eq!(out.runs.len(), 3);
    }

    #[test]
    fn multi_alpha_union_is_deterministic() {
        let run = || {
            let mut s = two_path_scorer();
            multi_alpha_union(&mut s, MockGoal::open("root"), &[0.0, 0.5, 1.0], 50)
        };
        assert_eq!(run(), run());
    }

    // ---- Gate-re-checked minimization over the sweep --------------------------

    /// A space whose winning line is *not* the shortest closing path in the arena.
    /// alpha=0 follows the high-log-prob two-step line root -t1-> A -t3-> G(closed),
    /// but the frontier also discovered root -t2-> C(closed), a length-one close.
    fn shortcut_scorer() -> TableScorer {
        TableScorer::new()
            .edge("root", "t1", 0.99f64.ln(), MockGoal::open("A"))
            .edge("root", "t2", 0.5f64.ln(), MockGoal::closed("C"))
            .edge("A", "t3", 0.99f64.ln(), MockGoal::closed("G"))
    }

    #[test]
    fn minimized_sweep_accepts_a_shortcut_the_replay_confirms() {
        let mut scorer = shortcut_scorer();
        let mut seen: Vec<Vec<String>> = Vec::new();
        let (outcome, minimized) = multi_alpha_union_minimized(
            &mut scorer,
            MockGoal::open("root"),
            &[0.0],
            50,
            |cand| {
                seen.push(cand.to_vec());
                true
            },
        );

        assert!(outcome.solved);
        assert_eq!(outcome.proof_tactics, vec!["t1", "t3"]);

        let m = minimized.expect("a solving sweep yields a minimize outcome");
        assert_eq!(m.status, MinimizeStatus::Verified);
        assert_eq!(m.accepted, Some(vec!["t2".to_string()]));
        assert_eq!(
            seen,
            vec![vec!["t2".to_string()]],
            "the replay must be handed the exact shrunk sequence to re-check"
        );
        assert!(m.best_safe(&outcome.proof_tactics).len() < outcome.proof_tactics.len());
    }

    #[test]
    fn minimized_sweep_keeps_the_original_when_the_replay_rejects() {
        // A replay that refuses the shrink is what a real checker does when the
        // arena's short path does not actually close the goal. Nothing may be
        // accepted in that case.
        let mut scorer = shortcut_scorer();
        let (outcome, minimized) =
            multi_alpha_union_minimized(&mut scorer, MockGoal::open("root"), &[0.0], 50, |_| false);

        let m = minimized.expect("a solving sweep yields a minimize outcome");
        assert_eq!(m.status, MinimizeStatus::RejectedByGate);
        assert_eq!(m.accepted, None, "a rejected shrink is never emittable");
        assert!(!m.status.is_emittable());
        assert_eq!(
            m.candidate,
            Some(vec!["t2".to_string()]),
            "the rejected shrink stays in the report for inspection"
        );
        // The caller falls back to the line the search actually closed on.
        assert_eq!(
            m.best_safe(&outcome.proof_tactics),
            outcome.proof_tactics.as_slice()
        );
    }

    #[test]
    fn minimized_sweep_never_consults_the_replay_when_unsolved() {
        let mut scorer = TableScorer::new().edge("root", "t", 0.5f64.ln(), MockGoal::open("stuck"));
        let mut calls = 0usize;
        let (outcome, minimized) = multi_alpha_union_minimized(
            &mut scorer,
            MockGoal::open("root"),
            &[0.0, 1.0],
            20,
            |_| {
                calls += 1;
                true
            },
        );

        assert!(!outcome.solved);
        assert_eq!(calls, 0, "an unsolved sweep must not spend a checker call");
        assert!(
            minimized.is_none(),
            "no proof was found, so there is nothing to report a status for"
        );
    }

    #[test]
    fn minimized_sweep_shrinks_the_winner_across_alphas() {
        // The sweep's winner is the shortest across alphas; the minimizer then runs
        // against *that* pass's arena, so both stages compose without either one
        // deciding solvedness on its own.
        let mut scorer = two_path_scorer();
        let (outcome, minimized) =
            multi_alpha_union_minimized(&mut scorer, MockGoal::open("root"), &[0.0, 1.0], 50, |_| {
                true
            });
        assert_eq!(outcome.proof_tactics, vec!["sx"], "alpha=0 wins the sweep");
        let m = minimized.expect("solved");
        // Already minimal: the arena's shortest close is the winning line itself.
        assert_eq!(m.accepted, Some(vec!["sx".to_string()]));
        assert_eq!(m.report.tactics_removed, 0);
    }

    #[test]
    fn plain_sweep_matches_the_minimized_sweeps_search_result() {
        // The opt-in entry point must not perturb the search itself: same runs,
        // same coverage, same reported proof as the free path.
        let mut a = shortcut_scorer();
        let plain = multi_alpha_union(&mut a, MockGoal::open("root"), &[0.0, 1.0], 50);
        let mut b = shortcut_scorer();
        let (checked, _) =
            multi_alpha_union_minimized(&mut b, MockGoal::open("root"), &[0.0, 1.0], 50, |_| true);
        assert_eq!(plain, checked);
    }

    // ---- split_budget / route ------------------------------------------------

    #[test]
    fn split_budget_partitions_exactly() {
        assert_eq!(split_budget(100, 0.3), (30, 70));
        assert_eq!(split_budget(100, 0.0), (0, 100));
        assert_eq!(split_budget(100, 1.0), (100, 0));
        // Sum is preserved for an odd total with a rounding split.
        let (bf, mc) = split_budget(101, 0.5);
        assert_eq!(bf + mc, 101);
        // Out-of-range ratios clamp; zero total stays zero.
        assert_eq!(split_budget(50, 1.5), (50, 0));
        assert_eq!(split_budget(50, -1.0), (0, 50));
        assert_eq!(split_budget(0, 0.5), (0, 0));
    }

    #[test]
    fn route_respects_bounds_and_conserves_budget() {
        let total = 1_000usize;
        for &critic in &[true, false] {
            for step in 0..=10 {
                let d = step as f64 / 10.0;
                let plan = route(
                    GoalFeatures {
                        difficulty: d,
                        critic_available: critic,
                    },
                    total,
                );
                assert_eq!(
                    plan.bf_budget + plan.mcgs_budget,
                    total,
                    "budget must be conserved (d={d}, critic={critic})"
                );
                assert!(plan.bf_budget <= total && plan.mcgs_budget <= total);
            }
        }
    }

    #[test]
    fn route_gives_everything_to_best_first_without_a_critic() {
        let plan = route(
            GoalFeatures {
                difficulty: 0.9,
                critic_available: false,
            },
            500,
        );
        assert_eq!(plan.bf_budget, 500);
        assert_eq!(plan.mcgs_budget, 0);
    }

    #[test]
    fn route_shifts_budget_toward_mcgs_as_difficulty_rises() {
        let easy = route(
            GoalFeatures {
                difficulty: 0.1,
                critic_available: true,
            },
            1_000,
        );
        let hard = route(
            GoalFeatures {
                difficulty: 0.9,
                critic_available: true,
            },
            1_000,
        );
        // Harder ⇒ more to the critic-guided side, less to best-first.
        assert!(hard.mcgs_budget > easy.mcgs_budget);
        assert!(hard.bf_budget < easy.bf_budget);
        // Both arms stay alive within the configured ratio bounds.
        assert!(hard.bf_budget >= (BF_RATIO_MIN * 1_000.0) as usize);
        assert!(easy.bf_budget <= (BF_RATIO_MAX * 1_000.0) as usize + 1);
    }

    // ---- The critic seam across the sweep ------------------------------------

    /// A deterministic critic keyed on the state text, plus a call counter so a
    /// test can observe that ONE object served every alpha pass.
    struct CountingCritic {
        prefix: &'static str,
        calls: std::sync::atomic::AtomicUsize,
    }
    impl CriticScorer for CountingCritic {
        fn score(&self, state: &dyn super::super::critic_scorer::GoalStateLike) -> f64 {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if state.state_text().starts_with(self.prefix) {
                1.0
            } else {
                0.0
            }
        }
    }

    /// The default sweep is unchanged: a zero weight with no critic reproduces
    /// `multi_alpha_union` exactly, and so does an INJECTED critic at weight zero.
    #[test]
    fn the_sweep_is_unchanged_at_the_default_critic_weight() {
        let alphas = [0.0, 1.0];
        let mut s = two_path_scorer();
        let baseline = multi_alpha_union(&mut s, MockGoal::open("root"), &alphas, 50);

        let mut s = two_path_scorer();
        let no_critic =
            multi_alpha_union_with_critic(&mut s, MockGoal::open("root"), &alphas, 50, 0.0, None);
        assert_eq!(no_critic, baseline);

        let mut s = two_path_scorer();
        let with_critic = multi_alpha_union_with_critic(
            &mut s,
            MockGoal::open("root"),
            &alphas,
            50,
            0.0,
            Some(Arc::new(CountingCritic {
                prefix: "D",
                calls: std::sync::atomic::AtomicUsize::new(0),
            })),
        );
        assert_eq!(with_critic, baseline);
    }

    /// One critic object serves every alpha pass. If the sweep rebuilt a critic per
    /// pass the arms would be silently incomparable; sharing the `Arc` is what
    /// guarantees identical states score identically across arms.
    #[test]
    fn one_critic_object_serves_every_alpha_pass() {
        let critic = Arc::new(CountingCritic {
            prefix: "D",
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let alphas = [0.0, 0.5, 1.0];
        let mut s = two_path_scorer();
        let out = multi_alpha_union_with_critic(
            &mut s,
            MockGoal::open("root"),
            &alphas,
            50,
            1.0,
            Some(critic.clone()),
        );
        assert_eq!(out.runs.len(), alphas.len());
        // Every pass consulted the SAME object, so the shared counter saw all of
        // them. A per-pass critic would leave this at whatever one pass spent.
        assert!(
            critic.calls.load(std::sync::atomic::Ordering::SeqCst) > 0,
            "the injected critic must actually be consulted"
        );
        // Rerunning with a fresh, equivalent critic gives the identical outcome:
        // the search depends on the critic's VALUES, not on its identity.
        let mut s = two_path_scorer();
        let again = multi_alpha_union_with_critic(
            &mut s,
            MockGoal::open("root"),
            &alphas,
            50,
            1.0,
            Some(Arc::new(CountingCritic {
                prefix: "D",
                calls: std::sync::atomic::AtomicUsize::new(0),
            })),
        );
        assert_eq!(out, again);
    }

    /// A critic reorders the sweep but can never make it claim a proof it did not
    /// find, and the minimizer's gate stays the sole soundness boundary.
    #[test]
    fn critic_cannot_manufacture_a_solution_in_the_sweep() {
        // A state space with no closed state at all.
        let mut s = TableScorer::new()
            .edge("root", "a", 0.5f64.ln(), MockGoal::open("A"))
            .edge("A", "b", 0.5f64.ln(), MockGoal::open("B"));
        let mut gate_calls = 0;
        let (outcome, minimized) = multi_alpha_union_minimized_with_critic(
            &mut s,
            MockGoal::open("root"),
            &[0.0, 1.0],
            50,
            100.0,
            Some(Arc::new(CountingCritic {
                prefix: "",
                calls: std::sync::atomic::AtomicUsize::new(0),
            })),
            |_| {
                gate_calls += 1;
                true
            },
        );
        assert!(!outcome.solved);
        assert!(outcome.proof_tactics.is_empty());
        assert!(minimized.is_none(), "no arena to shrink, so no gate run");
        assert_eq!(gate_calls, 0);
    }

    /// `critic_available` is derived from the one factory that decides whether a
    /// critic exists, so it cannot claim a critic the search would not receive.
    #[test]
    fn critic_available_is_derived_from_the_factory_not_asserted() {
        let off = GoalFeatures::from_config(0.5, &SearchConfig::default());
        assert!(!off.critic_available, "default weight means no critic");
        assert_eq!(route(off, 100).bf_budget, 100);

        let on = GoalFeatures::from_config(
            0.5,
            &SearchConfig {
                critic_weight: 0.5,
                ..SearchConfig::default()
            },
        );
        assert!(on.critic_available);
        assert!(route(on, 100).mcgs_budget > 0);
    }

    // ---- run_split combinator ------------------------------------------------

    #[test]
    fn run_split_returns_best_first_and_skips_mcgs_when_bf_solves() {
        let mcgs_called = Cell::new(false);
        let plan = HybridPlan {
            bf_budget: 100,
            mcgs_budget: 100,
        };
        let sol = run_split(
            &plan,
            |_budget| Some(vec!["bf_proof".to_string()]),
            |_budget| {
                mcgs_called.set(true);
                Some(vec!["mcgs_proof".to_string()])
            },
        );
        assert_eq!(sol.source, HybridSource::BestFirst);
        assert_eq!(sol.proof_tactics, vec!["bf_proof"]);
        assert!(sol.solved);
        assert!(
            !mcgs_called.get(),
            "MCGS arm must be skipped once best-first solves"
        );
    }

    #[test]
    fn run_split_falls_through_to_mcgs_when_best_first_fails() {
        let plan = HybridPlan {
            bf_budget: 100,
            mcgs_budget: 100,
        };
        let sol = run_split(&plan, |_| None, |_| Some(vec!["mcgs_proof".to_string()]));
        assert_eq!(sol.source, HybridSource::Mcgs);
        assert_eq!(sol.proof_tactics, vec!["mcgs_proof"]);
        assert!(sol.solved);
    }

    #[test]
    fn run_split_unsolved_when_neither_arm_solves() {
        let plan = HybridPlan {
            bf_budget: 10,
            mcgs_budget: 10,
        };
        let sol = run_split(&plan, |_| None, |_| None);
        assert!(!sol.solved);
        assert_eq!(sol.source, HybridSource::None);
        assert!(sol.proof_tactics.is_empty());
    }

    #[test]
    fn run_split_skips_a_zero_budget_arm() {
        // best-first gets no budget ⇒ its closure must not run; MCGS carries it.
        let bf_called = Cell::new(false);
        let plan = HybridPlan {
            bf_budget: 0,
            mcgs_budget: 100,
        };
        let sol = run_split(
            &plan,
            |_| {
                bf_called.set(true);
                Some(vec!["bf".to_string()])
            },
            |_| Some(vec!["mcgs".to_string()]),
        );
        assert!(!bf_called.get(), "a zero-budget arm must not be invoked");
        assert_eq!(sol.source, HybridSource::Mcgs);
    }

    // ---- Production entry point ---------------------------------------------

    use crate::model::ModelResponse;
    use std::path::Path;

    const STMT: &str = "theorem t : True";

    /// A proposer that closes `STMT` in one step and knows nothing else. Enough
    /// to drive a solving sweep with zero external dependencies.
    struct ClosingProposer;
    impl ModelProvider for ClosingProposer {
        fn complete(&self, request: &ModelRequest) -> Result<ModelResponse> {
            let goal = request.context["goal"].as_str().unwrap_or("");
            let content = if goal == STMT {
                json!({"steps":[
                    {"tactic":"trivial","weight":0.9,"next_goal":"","closes_goal":true}
                ]})
            } else {
                json!({"steps": []})
            };
            Ok(ModelResponse {
                content,
                model: "test".into(),
                provider: "test".into(),
            })
        }
        fn name(&self) -> &str {
            "test"
        }
    }

    /// A proposer that never closes anything: every goal is a dead end.
    struct StuckProposer;
    impl ModelProvider for StuckProposer {
        fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse> {
            Ok(ModelResponse {
                content: json!({"steps": []}),
                model: "test".into(),
                provider: "test".into(),
            })
        }
        fn name(&self) -> &str {
            "test"
        }
    }

    fn memory_store() -> Store {
        Store::open(Path::new(":memory:")).expect("in-memory store")
    }

    /// A config whose backend is a mock, so no test can ever depend on a Lean or
    /// Rocq toolchain being installed on the host.
    fn mocked_config() -> Config {
        let mut cfg = Config::default();
        cfg.prover_mock = true;
        cfg
    }

    #[test]
    fn entry_reports_a_candidate_not_a_proof() {
        let store = memory_store();
        let summary = run_alpha_sweep_search(
            &store,
            &mocked_config(),
            &ClosingProposer,
            None,
            STMT,
            FormalSystem::Lean,
            &[0.0, 1.0],
            20,
            false,
            0.0,
        )
        .expect("the sweep runs offline");

        assert_eq!(summary["solved"], true);
        assert_eq!(summary["candidate_tactics"], json!(["trivial"]));
        // The search closed a goal the MODEL called closed. That is a candidate.
        assert_eq!(summary["status"], "candidate");
        assert_eq!(summary["formally_verified"], false);
        assert!(
            summary["verified_tactics"].is_null(),
            "nothing may be emittable as a proof without a gate verdict"
        );
        // The expensive path was not asked for, and says so.
        assert_eq!(summary["minimization"]["ran"], false);
        assert_eq!(summary["minimization"]["skipped_reason"], "not_requested");
    }

    #[test]
    fn no_live_checker_reports_minimization_as_not_run_rather_than_rejected() {
        // A mock backend makes GateReplay refuse everything BY DESIGN. That is a
        // missing precondition, not a verdict on the shrink, and never a failure
        // of the search, so the summary must not say "rejected".
        let store = memory_store();
        let summary = run_alpha_sweep_search(
            &store,
            &mocked_config(),
            &ClosingProposer,
            None,
            STMT,
            FormalSystem::Lean,
            &[0.0],
            20,
            true,
            0.0,
        )
        .expect("a missing checker is not an error");

        assert_eq!(summary["solved"], true, "the search itself still succeeded");
        let m = &summary["minimization"];
        assert_eq!(m["requested"], true);
        assert_eq!(m["checker_live"], false);
        assert_eq!(m["ran"], false);
        assert_eq!(m["skipped_reason"], "no_live_checker");
        assert!(
            m["status"].is_null(),
            "no gate ran, so there is no gate status to report"
        );
        assert_ne!(m["status"], "rejected_by_gate");
        assert_eq!(m["shrink_accepted"], false);
        assert!(m["accepted_tactics"].is_null());
        assert_eq!(summary["formally_verified"], false);
    }

    #[test]
    fn an_unsolved_sweep_is_reported_without_tactics() {
        let store = memory_store();
        let summary = run_alpha_sweep_search(
            &store,
            &mocked_config(),
            &StuckProposer,
            None,
            STMT,
            FormalSystem::Lean,
            &[0.0, 0.5, 1.0],
            10,
            true,
            0.0,
        )
        .expect("an unsolved sweep is not an error");

        assert_eq!(summary["solved"], false);
        assert_eq!(summary["candidate_tactics"], json!([]));
        assert!(summary["best_alpha"].is_null());
        assert_eq!(summary["formally_verified"], false);
        assert_eq!(summary["minimization"]["ran"], false);
    }

    #[test]
    fn the_summary_shape_is_stable() {
        let store = memory_store();
        let summary = run_alpha_sweep_search(
            &store,
            &mocked_config(),
            &ClosingProposer,
            None,
            STMT,
            FormalSystem::Lean,
            &[0.0, 1.0],
            20,
            true,
            0.0,
        )
        .expect("sweep runs");

        for key in [
            "kind",
            "status",
            "formally_verified",
            "statement",
            "system",
            "alphas",
            "per_alpha_budget",
            "solved",
            "candidate_tactics",
            "best_alpha",
            "union_coverage",
            "runs",
            "verified_tactics",
            "minimization",
            "model",
            "note",
        ] {
            assert!(summary.get(key).is_some(), "summary must carry `{key}`");
        }
        assert_eq!(summary["kind"], "hybrid_alpha_sweep");
        assert_eq!(summary["system"], "lean");
        assert_eq!(summary["alphas"], json!([0.0, 1.0]));
        assert_eq!(summary["runs"].as_array().unwrap().len(), 2);
        for key in [
            "requested",
            "checker_live",
            "ran",
            "skipped_reason",
            "status",
            "shrink_accepted",
            "accepted_tactics",
            "report",
        ] {
            assert!(
                summary["minimization"].get(key).is_some(),
                "minimization must carry `{key}`"
            );
        }
    }

    #[test]
    fn an_empty_alpha_list_falls_back_to_the_default_sweep() {
        let store = memory_store();
        let summary = run_alpha_sweep_search(
            &store,
            &mocked_config(),
            &ClosingProposer,
            None,
            STMT,
            FormalSystem::Lean,
            &[],
            10,
            false,
            0.0,
        )
        .expect("sweep runs");
        assert_eq!(summary["alphas"], json!(DEFAULT_ALPHAS.to_vec()));
    }

    #[test]
    fn an_empty_statement_is_rejected() {
        let store = memory_store();
        assert!(run_alpha_sweep_search(
            &store,
            &mocked_config(),
            &ClosingProposer,
            None,
            "   ",
            FormalSystem::Lean,
            &[0.0],
            10,
            false,
            0.0,
        )
        .is_err());
    }

    #[test]
    fn each_distinct_goal_costs_at_most_one_provider_call() {
        // Three alpha passes revisit the same root. The memo must keep that at one
        // round trip, both to bound spend and so every pass sees the same edges.
        let mut expander = ProviderExpander::new(&ClosingProposer, STMT, FormalSystem::Lean);
        let root = ProviderGoal {
            key: STMT.to_string(),
            closed: false,
        };
        for _ in 0..3 {
            let steps = expander.expand(&root, 0);
            assert_eq!(steps.len(), 1);
            assert_eq!(steps[0].tactic, "trivial");
            assert!(steps[0].next.is_closed(), "the model claimed a close");
        }
        assert_eq!(expander.calls, 1);
        assert_eq!(expander.errors, 0);
    }

    #[test]
    fn a_failing_provider_is_a_dead_end_not_a_proof() {
        struct Failing;
        impl ModelProvider for Failing {
            fn complete(&self, _r: &ModelRequest) -> Result<ModelResponse> {
                Err(anyhow::anyhow!("model unavailable"))
            }
            fn name(&self) -> &str {
                "failing"
            }
        }
        let store = memory_store();
        let summary = run_alpha_sweep_search(
            &store,
            &mocked_config(),
            &Failing,
            None,
            STMT,
            FormalSystem::Lean,
            &[0.0],
            10,
            true,
            0.0,
        )
        .expect("a model failure degrades the search, it does not abort it");
        assert_eq!(summary["solved"], false);
        assert_eq!(summary["formally_verified"], false);
        assert!(summary["model"]["errors"].as_u64().unwrap() >= 1);
    }
}
