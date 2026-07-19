//! Orchestrated test-time-compute controller (plan §14, the 2026 orchestrated-TTC
//! pattern).
//!
//! This module separates the *how much compute, and where* decision from the
//! search itself. Instead of every goal getting a fixed best-of-N width and a
//! fixed MCTS rollout budget, a [`TtcController`] decides — per goal — how much
//! of a bounded global budget to spend, as a function of:
//! * a **difficulty estimate** in `[0, 1]` for the goal (harder ⇒ more compute),
//! * the **budget remaining** in the run (near-exhaustion ⇒ shrink the spend), and
//! * how many **prior attempts** the goal has already had (escalate on retries).
//!
//! The load-bearing property is *pure separation*: [`TtcConfig::allocate`] is a
//! pure function (no clock, no randomness, no interior state) so the allocation
//! policy is exhaustively testable in isolation. [`TtcController`] is the thin
//! stateful wrapper that tracks cumulative spend against a global budget and
//! guarantees the total never exceeds it — every allocation's cost is clamped to
//! the remaining budget before it is charged.
//!
//! The search layer consults this *optionally*: [`crate::search::driver`] runs
//! its existing fixed-`N` behaviour when no controller is attached, and defers
//! width/rollout sizing to the controller when one is.

use serde::Serialize;

/// A compute allocation for a single goal: how wide to sample (best-of-N /
/// MCTS branching), how many rollouts/iterations to run, and the rollout depth
/// cap. Produced by [`TtcConfig::allocate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Allocation {
    /// Best-of-N width / MCTS expansion breadth (the top-`k` prior candidates).
    pub width: usize,
    /// MCTS rollouts / search iterations to spend on this goal.
    pub rollouts: usize,
    /// Rollout depth cap.
    pub max_depth: usize,
}

impl Allocation {
    /// The compute cost charged against the global budget: `width * rollouts`
    /// (the total number of leaf evaluations this allocation authorises).
    pub fn cost(&self) -> u64 {
        (self.width as u64).saturating_mul(self.rollouts as u64)
    }

    /// Whether this allocation authorises no work (budget exhausted).
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.rollouts == 0
    }
}

/// Tuning for the orchestrated-TTC policy. The `base_*` values are what an easy
/// goal (difficulty `0`) with a full budget receives; the `max_*` values are the
/// ceiling a maximally hard goal receives. The policy interpolates between them.
#[derive(Debug, Clone, Copy)]
pub struct TtcConfig {
    /// Total compute units (leaf evaluations) for the whole run.
    pub global_budget: u64,
    /// Width for an easy goal (difficulty `0`).
    pub base_width: usize,
    /// Width ceiling for the hardest goal.
    pub max_width: usize,
    /// Rollouts for an easy goal (difficulty `0`).
    pub base_rollouts: usize,
    /// Rollout ceiling for the hardest goal.
    pub max_rollouts: usize,
    /// Rollout depth cap (constant across difficulties).
    pub max_depth: usize,
    /// How much each prior attempt escalates the effective difficulty (a retry
    /// bump). `0.0` disables retry escalation.
    pub attempt_escalation: f64,
}

impl Default for TtcConfig {
    fn default() -> Self {
        Self {
            global_budget: 10_000,
            base_width: 1,
            max_width: 16,
            base_rollouts: 32,
            max_rollouts: 512,
            max_depth: 12,
            attempt_escalation: 0.15,
        }
    }
}

impl TtcConfig {
    /// The pure allocation policy: decide `(width, rollouts, depth)` for a goal
    /// from its `difficulty` estimate (`[0, 1]`, clamped), the `budget_remaining`
    /// in the run, and how many `prior_attempts` it has had.
    ///
    /// Properties (all exercised by the unit tests):
    /// * **Monotone in difficulty** — harder goals get at least as much width and
    ///   rollouts (before budget clamping).
    /// * **Retry escalation** — `prior_attempts` raises the effective difficulty,
    ///   so a re-attempted goal gets more compute.
    /// * **Budget-bounded** — the returned allocation's [`Allocation::cost`] never
    ///   exceeds `budget_remaining`; a fully exhausted budget yields an empty
    ///   allocation. This is what lets [`TtcController`] guarantee the global cap.
    pub fn allocate(
        &self,
        difficulty: f64,
        budget_remaining: u64,
        prior_attempts: u32,
    ) -> Allocation {
        // Nothing left to spend: authorise no work.
        if budget_remaining == 0 {
            return Allocation {
                width: 0,
                rollouts: 0,
                max_depth: self.max_depth,
            };
        }

        // Effective difficulty escalates with prior attempts (a retry bump).
        let d = difficulty.clamp(0.0, 1.0);
        let escalated =
            (d + self.attempt_escalation.max(0.0) * prior_attempts as f64).clamp(0.0, 1.0);

        // Interpolate width and rollouts from base..=max by the escalated
        // difficulty. `saturating_sub` guards against a mis-configured base > max.
        let width_span = self.max_width.saturating_sub(self.base_width) as f64;
        let roll_span = self.max_rollouts.saturating_sub(self.base_rollouts) as f64;
        let mut width = self.base_width + (width_span * escalated).round() as usize;
        let mut rollouts = self.base_rollouts + (roll_span * escalated).round() as usize;
        width = width.max(1);
        rollouts = rollouts.max(1);

        // Clamp the cost to the remaining budget so it can be charged in full
        // without overspending. Shrink rollouts first, then width; both stay >= 1
        // while any budget remains.
        let cap = budget_remaining;
        if (width as u64).saturating_mul(rollouts as u64) > cap {
            rollouts = ((cap / width as u64).max(1)) as usize;
            let w_cap = (cap / rollouts as u64).max(1) as usize;
            width = width.min(w_cap).max(1);
        }

        Allocation {
            width,
            rollouts,
            max_depth: self.max_depth,
        }
    }
}

/// Stateful wrapper around [`TtcConfig`] that tracks cumulative spend against the
/// global budget. Its invariant: the sum of every charged allocation's cost never
/// exceeds `cfg.global_budget`, because [`TtcConfig::allocate`] clamps each
/// allocation to the [`remaining`](TtcController::remaining) budget before
/// [`take`](TtcController::take) charges it.
#[derive(Debug, Clone)]
pub struct TtcController {
    cfg: TtcConfig,
    spent: u64,
}

impl TtcController {
    /// A fresh controller with `spent = 0`.
    pub fn new(cfg: TtcConfig) -> Self {
        Self { cfg, spent: 0 }
    }

    /// The controller's tuning.
    pub fn config(&self) -> &TtcConfig {
        &self.cfg
    }

    /// Compute units already charged.
    pub fn spent(&self) -> u64 {
        self.spent
    }

    /// Compute units still available (`global_budget - spent`, floored at `0`).
    pub fn remaining(&self) -> u64 {
        self.cfg.global_budget.saturating_sub(self.spent)
    }

    /// Peek at the allocation for a goal *without* charging it — the pure policy
    /// applied to the current remaining budget.
    pub fn allocate(&self, difficulty: f64, prior_attempts: u32) -> Allocation {
        self.cfg
            .allocate(difficulty, self.remaining(), prior_attempts)
    }

    /// Allocate compute for a goal *and charge it* to the running total. The
    /// returned allocation's cost is guaranteed `<= remaining()` at call time, so
    /// the cumulative spend never exceeds the global budget.
    pub fn take(&mut self, difficulty: f64, prior_attempts: u32) -> Allocation {
        let alloc = self.allocate(difficulty, prior_attempts);
        self.spent = self.spent.saturating_add(alloc.cost());
        alloc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harder_goals_get_more_width_and_rollouts() {
        // With a budget large enough that no clamping happens, the allocation is
        // monotone in difficulty.
        let cfg = TtcConfig::default();
        let easy = cfg.allocate(0.1, u64::MAX, 0);
        let hard = cfg.allocate(0.9, u64::MAX, 0);
        assert!(
            hard.width > easy.width,
            "harder goal should get more width ({} vs {})",
            hard.width,
            easy.width
        );
        assert!(hard.rollouts > easy.rollouts);
    }

    #[test]
    fn allocation_is_monotone_across_the_difficulty_range() {
        let cfg = TtcConfig::default();
        let mut prev = cfg.allocate(0.0, u64::MAX, 0);
        for step in 1..=10 {
            let d = step as f64 / 10.0;
            let a = cfg.allocate(d, u64::MAX, 0);
            assert!(
                a.width >= prev.width,
                "width must not decrease with difficulty"
            );
            assert!(a.rollouts >= prev.rollouts);
            prev = a;
        }
    }

    #[test]
    fn prior_attempts_escalate_the_allocation() {
        let cfg = TtcConfig::default();
        let first = cfg.allocate(0.3, u64::MAX, 0);
        let retried = cfg.allocate(0.3, u64::MAX, 2);
        assert!(
            retried.width >= first.width && retried.rollouts >= first.rollouts,
            "a re-attempted goal should get at least as much compute"
        );
        assert!(retried.rollouts > first.rollouts || retried.width > first.width);
    }

    #[test]
    fn budget_exhaustion_shrinks_then_empties_the_allocation() {
        let cfg = TtcConfig::default();
        // Same hard goal, decreasing remaining budget => non-increasing cost.
        let plenty = cfg.allocate(0.9, u64::MAX, 0);
        let tight = cfg.allocate(0.9, 100, 0);
        let scarce = cfg.allocate(0.9, 4, 0);
        assert!(tight.cost() <= plenty.cost());
        assert!(scarce.cost() <= tight.cost());
        // Every non-empty allocation's cost fits inside the remaining budget.
        assert!(tight.cost() <= 100);
        assert!(scarce.cost() <= 4);
        // A fully exhausted budget authorises no work.
        let exhausted = cfg.allocate(0.9, 0, 0);
        assert!(exhausted.is_empty());
        assert_eq!(exhausted.cost(), 0);
    }

    #[test]
    fn allocation_cost_never_exceeds_remaining_budget() {
        // Exhaustive-ish sweep: for a range of budgets and difficulties the pure
        // policy must never authorise more than the remaining budget.
        let cfg = TtcConfig::default();
        for &budget in &[0u64, 1, 3, 7, 50, 500, 9999] {
            for step in 0..=10 {
                let d = step as f64 / 10.0;
                for attempts in 0..4 {
                    let a = cfg.allocate(d, budget, attempts);
                    assert!(
                        a.cost() <= budget,
                        "cost {} exceeded budget {} (d={d}, attempts={attempts})",
                        a.cost(),
                        budget
                    );
                }
            }
        }
    }

    #[test]
    fn controller_total_spend_never_exceeds_the_global_budget() {
        let cfg = TtcConfig {
            global_budget: 1_000,
            ..TtcConfig::default()
        };
        let mut ctrl = TtcController::new(cfg);
        // Hammer it with many hard goals; the running total must stay bounded and
        // remaining must monotonically drain to exactly what was spent.
        for i in 0..100 {
            let difficulty = (i % 10) as f64 / 10.0;
            let alloc = ctrl.take(difficulty, (i % 3) as u32);
            assert!(ctrl.spent() <= 1_000, "spend overran the global budget");
            assert!(alloc.cost() <= 1_000);
        }
        assert!(ctrl.spent() <= 1_000);
        assert_eq!(ctrl.remaining(), 1_000 - ctrl.spent());
        // Once drained, further allocations are empty (no negative / wraparound).
        let mut drain = TtcController::new(TtcConfig {
            global_budget: 10,
            base_width: 4,
            base_rollouts: 4,
            ..TtcConfig::default()
        });
        for _ in 0..50 {
            let _ = drain.take(1.0, 0);
        }
        assert!(drain.spent() <= 10);
        assert!(drain.take(1.0, 0).is_empty());
    }

    #[test]
    fn peeking_does_not_charge_the_budget() {
        let mut ctrl = TtcController::new(TtcConfig::default());
        let _ = ctrl.allocate(0.5, 0);
        assert_eq!(ctrl.spent(), 0, "allocate() must not charge");
        let a = ctrl.take(0.5, 0);
        assert_eq!(ctrl.spent(), a.cost(), "take() charges exactly the cost");
    }
}
