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
//! * [`HybridPlan`] + [`split_budget`] + [`route`] — split a total compute budget
//!   between the best-first side and the critic-guided-driver side from cheap goal
//!   features. A [`super::ttc::TtcController`] is the intended caller (see the
//!   wiring note on [`route`]).
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

use super::best_first::{best_first_search, BestFirstConfig, TacticScorer};
use std::collections::BTreeSet;

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
    let mut runs: Vec<AlphaRun> = Vec::with_capacity(alphas.len());
    let mut union: BTreeSet<String> = BTreeSet::new();
    // Best (shortest) solution so far: (proof_len, tactics, alpha).
    let mut best: Option<(usize, Vec<String>, f64)> = None;

    for &alpha in alphas {
        let cfg = BestFirstConfig {
            alpha,
            max_steps: per_alpha_budget,
            seed: 0,
            hint_weight: 0.0,
        };
        let out = best_first_search(scorer, root.clone(), &cfg);

        for key in &out.order {
            union.insert(key.clone());
        }

        let tactics = out.proof_tactics();
        let proof_len = tactics.len();

        if out.solved {
            // Strictly-shorter wins; equal length keeps the earlier alpha (this
            // pass only replaces on `<`), so the tie-break is deterministic.
            let take = match &best {
                None => true,
                Some((best_len, _, _)) => proof_len < *best_len,
            };
            if take {
                best = Some((proof_len, tactics.clone(), alpha));
            }
        }

        runs.push(AlphaRun {
            alpha,
            solved: out.solved,
            steps: out.steps,
            proof_len,
            visited: out.order.len(),
        });
    }

    let (solved, proof_tactics, best_alpha) = match best {
        Some((_, tactics, alpha)) => (true, tactics, Some(alpha)),
        None => (false, Vec::new(), None),
    };

    HybridOutcome {
        solved,
        proof_tactics,
        best_alpha,
        runs,
        union_keys: union.into_iter().collect(),
    }
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

#[cfg(test)]
mod tests {
    use super::super::best_first::{ScoredExpansion, ScoredTactic};
    use super::super::driver::GoalState;
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
}
