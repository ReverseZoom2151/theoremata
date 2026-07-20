//! Trained state-value critic seam for the MCGS driver (InternLM2.5-StepProver's
//! critic-guided search, adapted — built from the *architecture*, not any vendored
//! source).
//!
//! # The gap this closes
//!
//! The graph driver ([`crate::search::driver`]) ranks a node's children by
//! `score = q + progress_weight·progress + u`, where `progress` is the
//! hand-crafted, unlearned LeanProgress-style heuristic in
//! [`crate::progress`]. Meanwhile the project already has a *trainable* critic /
//! value head — the Monte-Carlo `Q`-backup targets in
//! [`crate::search::process_reward`] and the value model in
//! `theoremata_tools.process_supervision` ([`predict_value`]) — but it is
//! **decoupled** from live search: nothing feeds `V(s)` back into node priority.
//!
//! InternLM2.5-StepProver's insight is that a *trained* state-value critic `V(s)`
//! (how close is this proof state to a completed proof?) guiding best-first /
//! PUCT selection is worth far more than a static heuristic. This module supplies
//! the **injectable seam** to wire such a critic into the driver's node priority
//! without hard-coding a model:
//!
//! * [`CriticScorer`] — the trait the driver consults for each freshly minted
//!   node: `score(state) → V(s) ∈ [0, 1]`, higher meaning "closer to done".
//! * [`HeuristicCritic`] — the default, which delegates to
//!   [`crate::progress::progress_value_from_state`]. With it (or *no* critic at
//!   all) plus the default `critic_weight = 0.0`, the driver's behaviour is
//!   **exactly unchanged** — the critic term is inert.
//! * [`ConstantCritic`] — a deterministic test double returning a fixed value.
//! * [`blend_priority`] — the one arithmetic change the driver needs: the
//!   augmented score `q + progress_weight·progress + critic_weight·critic + u`.
//!
//! # The trained-critic adapter path (documented seam, *not* built here)
//!
//! Training a real `V(s)` is the GPU-gated part and is deliberately **out of
//! scope**. When a trained value head exists, plug it in behind [`CriticScorer`]
//! with a thin adapter — no driver change is needed beyond this seam:
//!
//! ```ignore
//! struct WorkerCritic { model: serde_json::Value } // loaded value-head weights
//! impl CriticScorer for WorkerCritic {
//!     fn score(&self, state: &dyn GoalStateLike) -> f64 {
//!         // 1. featurize the pretty-printed goal state (same features the
//!         //    process_supervision trainer used),
//!         // 2. call the Python worker seam (PythonCheck / exec) that reaches
//!         //    `theoremata_tools.process_supervision.predict_value(model, feats)`,
//!         //    which returns a tanh-bounded V in [-1, 1] with NO torch at query
//!         //    time,
//!         // 3. remap to the critic's [0, 1] contract: (v + 1.0) / 2.0.
//!         let v: f64 = /* predict_value(model, features) */ 0.0;
//!         ((v + 1.0) / 2.0).clamp(0.0, 1.0)
//!     }
//! }
//! ```
//!
//! The training targets are the Monte-Carlo `Q`s from
//! [`crate::search::process_reward::q_targets`]; the query path is the existing
//! Python worker bridge (`tool: "process_supervision"`). This file provides only
//! the trait and the deterministic defaults — everything here is a pure function
//! of its inputs, with no wall-clock and no unseeded randomness.

use super::mcts::SearchConfig;
use super::progress;
use std::sync::Arc;

/// The minimal view of a proof state a critic needs to score it.
///
/// A [`CriticScorer`] is object-safe (`dyn`-usable) so the driver can hold an
/// `Option<Arc<dyn CriticScorer>>`; keeping the state behind this small
/// dyn-compatible trait is what makes that work. The only thing a value estimate
/// needs is a textual view of the goal state — the same normalised pretty-print a
/// real backend already uses as its transposition key.
pub trait GoalStateLike {
    /// A pretty-printed / textual view of the proof state: the turnstile goal
    /// state (`⊢ …` with its hypotheses). A real backend returns its normalised
    /// pretty-print; this is exactly what [`crate::progress`] parses.
    fn state_text(&self) -> String;
}

/// Bridge: every driver [`GoalState`](crate::search::driver::GoalState) is
/// scorable. Its [`dedup_key`](crate::search::driver::GoalState::dedup_key) is a
/// canonical, normalised pretty-print of the goal state (α-equivalent,
/// hypothesis order canonicalised) — precisely the textual view a critic scores —
/// so the driver can pass `&node.state` straight through with no new bound on the
/// expander's state type.
impl<T: super::driver::GoalState> GoalStateLike for T {
    fn state_text(&self) -> String {
        self.dedup_key()
    }
}

/// A state-value critic `V(s)`: estimate how close a proof state is to a
/// completed proof.
///
/// Contract: [`score`](CriticScorer::score) returns a value in `[0, 1]`, higher
/// meaning *closer to done* (`1.0` ⇒ a closed / no-goals state). This is the same
/// orientation as [`crate::progress::progress_value`], so a critic is a drop-in,
/// learned replacement for the static progress heuristic in node priority.
///
/// Implementations MUST be deterministic (a pure function of the state) so search
/// stays reproducible.
pub trait CriticScorer {
    /// Estimate `V(state) ∈ [0, 1]`.
    fn score(&self, state: &dyn GoalStateLike) -> f64;
}

/// The default critic: delegates to the hand-crafted, unlearned LeanProgress-style
/// heuristic ([`crate::progress::progress_value_from_state`]).
///
/// Using this critic reproduces the driver's current progress signal exactly, so
/// installing the seam with `HeuristicCritic` and `critic_weight = 0.0` leaves
/// behaviour byte-for-byte unchanged until a *trained* critic is supplied.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeuristicCritic;

impl CriticScorer for HeuristicCritic {
    fn score(&self, state: &dyn GoalStateLike) -> f64 {
        progress::progress_value_from_state(&state.state_text())
    }
}

/// A deterministic test double returning a fixed value for every state (clamped
/// to the `[0, 1]` contract). Used to exercise the blend ordering without a model.
#[derive(Debug, Clone, Copy)]
pub struct ConstantCritic(pub f64);

impl CriticScorer for ConstantCritic {
    fn score(&self, _state: &dyn GoalStateLike) -> f64 {
        self.0.clamp(0.0, 1.0)
    }
}

/// The augmented PUCT node priority: `q + progress_weight·progress +
/// critic_weight·critic + u`.
///
/// This is the whole arithmetic change the driver's selection loop needs: it
/// currently computes `q + progress_weight·progress + u` inline (driver.rs, PUCT
/// branch), and gains only the `critic_weight·critic` term. When
/// `critic_weight == 0.0` (the intended default of the new
/// `SearchConfig::critic_weight` field) the critic term vanishes and the score is
/// identical to today's — so the wiring is behaviour-preserving until a trained
/// critic and a non-zero weight are supplied.
///
/// The weights are passed explicitly (rather than read from a [`SearchConfig`]
/// field) so this module compiles standalone against the *current* config; once
/// the `critic_weight` field is added to [`SearchConfig`] per the reported edit,
/// the driver calls this as
/// `blend_priority(q, c.progress, cfg.progress_weight, c.critic, cfg.critic_weight, u)`.
#[inline]
pub fn blend_priority(
    q: f64,
    progress: f64,
    progress_weight: f64,
    critic: f64,
    critic_weight: f64,
    u: f64,
) -> f64 {
    q + progress_weight * progress + critic_weight * critic + u
}

/// Convenience: blend reading `progress_weight` from an existing [`SearchConfig`]
/// while taking `critic_weight` explicitly, so callers that already hold a config
/// need not thread `progress_weight` separately. (Once `SearchConfig` gains a
/// `critic_weight` field, a fully-config-driven wrapper can replace this.)
#[inline]
pub fn blend_priority_with_cfg(
    q: f64,
    progress: f64,
    critic: f64,
    critic_weight: f64,
    u: f64,
    cfg: &SearchConfig,
) -> f64 {
    blend_priority(q, progress, cfg.progress_weight, critic, critic_weight, u)
}

/// Build the state-value critic to inject into a [`crate::search::driver`] for a
/// given [`SearchConfig`], or `None` when the critic seam is switched off.
///
/// This is the production gate that keeps the whole seam OFF by default. The
/// driver only reads a critic when one is injected, so returning `None` here
/// leaves search byte-identical to the pre-seam behaviour. The gate is
/// `cfg.critic_weight`:
///
/// * `critic_weight == 0.0` (the [`SearchConfig`] default) ⇒ `None`. Nothing is
///   injected, and even if a future config path also flips on `eta_mcts` the
///   driver still sees no critic, so no config can change the default behaviour.
/// * `critic_weight != 0.0` ⇒ the concrete [`HeuristicCritic`], which is the
///   documented, deterministic default value head. It delegates to the same
///   LeanProgress heuristic the driver already blends, so turning the weight up is
///   an additive, bounded change rather than a new signal. The point of wiring it
///   now is that the seam becomes LIVE end to end, and swapping the trained value
///   head in later is a one-line change to this factory (return the trained
///   `CriticScorer` instead), with no edit to the driver or its callers.
///
/// Returned behind an [`Arc`] because the driver holds `Option<Arc<dyn
/// CriticScorer>>` so one critic can be shared across concurrent searches.
pub fn critic_from_config(cfg: &SearchConfig) -> Option<Arc<dyn CriticScorer>> {
    if cfg.critic_weight == 0.0 {
        None
    } else {
        Some(Arc::new(HeuristicCritic))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal state carrying a pretty-printed goal string. It implements
    /// [`GoalStateLike`] directly (not the driver's `GoalState`), so the blanket
    /// bridge never applies and there is no coherence overlap.
    struct TextState(&'static str);
    impl GoalStateLike for TextState {
        fn state_text(&self) -> String {
            self.0.to_string()
        }
    }

    /// The heuristic critic must equal the raw progress heuristic on the same
    /// text — it is a pure delegation, nothing more.
    #[test]
    fn heuristic_critic_delegates_to_progress() {
        let goal = "n : ℕ\nih : n + 0 = n\n⊢ n + 1 + 0 = n + 1";
        let s = TextState(goal);
        let critic = HeuristicCritic;
        assert_eq!(
            critic.score(&s),
            progress::progress_value_from_state(goal),
            "HeuristicCritic is exactly the progress heuristic"
        );
        // A closed ("no goals") state scores the terminal 1.0.
        assert_eq!(HeuristicCritic.score(&TextState("no goals")), 1.0);
    }

    /// `ConstantCritic` returns its value verbatim (within the contract) and
    /// clamps out-of-range inputs.
    #[test]
    fn constant_critic_returns_fixed_clamped_value() {
        assert_eq!(ConstantCritic(0.3).score(&TextState("⊢ p")), 0.3);
        assert_eq!(ConstantCritic(1.7).score(&TextState("⊢ p")), 1.0);
        assert_eq!(ConstantCritic(-0.4).score(&TextState("⊢ p")), 0.0);
    }

    /// With `critic_weight = 0` the blend is byte-for-byte the current
    /// progress-only priority, regardless of the critic value — this is what makes
    /// the seam behaviour-preserving by default.
    #[test]
    fn blend_with_zero_critic_weight_is_progress_only() {
        let (q, progress_v, pw, u) = (0.4, 0.6, 0.5, 0.2);
        let baseline = q + pw * progress_v + u; // the driver's current formula
        for critic in [0.0, 0.5, 1.0, 0.9137] {
            let blended = blend_priority(q, progress_v, pw, critic, 0.0, u);
            assert!(
                (blended - baseline).abs() < 1e-12,
                "critic_weight=0 must recover progress-only (critic={critic})"
            );
        }
    }

    /// The blend is strictly increasing in the critic term when the weight is
    /// positive (and holding everything else fixed) — a higher `V(s)` can only
    /// raise a node's priority.
    #[test]
    fn blend_is_monotonic_in_critic() {
        let (q, progress_v, pw, cw, u) = (0.1, 0.5, 0.5, 0.7, 0.3);
        let low = blend_priority(q, progress_v, pw, 0.2, cw, u);
        let mid = blend_priority(q, progress_v, pw, 0.5, cw, u);
        let high = blend_priority(q, progress_v, pw, 0.9, cw, u);
        assert!(
            low < mid && mid < high,
            "priority must rise with the critic value"
        );
        // The increment is exactly critic_weight·Δcritic.
        assert!(((mid - low) - cw * (0.5 - 0.2)).abs() < 1e-12);
    }

    /// A `ConstantCritic` produces the expected *ordering* of two nodes that are
    /// otherwise identical: the one the critic rates higher wins the priority.
    #[test]
    fn constant_critic_drives_node_ordering() {
        let cfg = SearchConfig {
            progress_weight: 0.5,
            ..SearchConfig::default()
        };
        let critic_weight = 1.0;
        // Two candidate children with identical q / progress / u, differing only
        // in the critic's verdict.
        let (q, progress_v, u) = (0.2, 0.4, 0.1);
        let hi = ConstantCritic(0.9).score(&TextState("⊢ almost done"));
        let lo = ConstantCritic(0.1).score(&TextState("⊢ far away"));
        let score_hi = blend_priority_with_cfg(q, progress_v, hi, critic_weight, u, &cfg);
        let score_lo = blend_priority_with_cfg(q, progress_v, lo, critic_weight, u, &cfg);
        assert!(
            score_hi > score_lo,
            "the critic-preferred node must rank higher ({score_hi} vs {score_lo})"
        );
        // And with the critic disabled (weight 0) the two tie exactly.
        let tie_hi = blend_priority_with_cfg(q, progress_v, hi, 0.0, u, &cfg);
        let tie_lo = blend_priority_with_cfg(q, progress_v, lo, 0.0, u, &cfg);
        assert!((tie_hi - tie_lo).abs() < 1e-12);
    }

    /// The production gate: no critic at the default weight, the heuristic critic
    /// once the weight is switched on.
    #[test]
    fn critic_from_config_is_gated_on_the_weight() {
        // Default config has critic_weight == 0.0, so the seam stays off.
        assert!(critic_from_config(&SearchConfig::default()).is_none());

        let on = SearchConfig {
            critic_weight: 0.5,
            ..SearchConfig::default()
        };
        let critic = critic_from_config(&on).expect("a non-zero weight injects a critic");
        // The injected critic is the heuristic default: it agrees with the raw
        // progress heuristic on any state text.
        let goal = "n : ℕ\n⊢ n + 0 = n";
        assert_eq!(
            critic.score(&TextState2(goal)),
            progress::progress_value_from_state(goal)
        );
    }

    /// A tiny state for the gate test; separate from `TextState` only to keep each
    /// test's fixtures local and obvious.
    struct TextState2(&'static str);
    impl GoalStateLike for TextState2 {
        fn state_text(&self) -> String {
            self.0.to_string()
        }
    }

    /// Everything is deterministic: repeated scoring / blending of the same inputs
    /// yields identical results (no wall-clock, no rng).
    #[test]
    fn scoring_and_blending_are_deterministic() {
        let s = TextState("case succ\nn : ℕ\n⊢ n + 1 + 0 = n + 1");
        let c = HeuristicCritic;
        assert_eq!(c.score(&s), c.score(&s));
        let a = blend_priority(0.3, 0.5, 0.5, 0.7, 0.4, 0.2);
        let b = blend_priority(0.3, 0.5, 0.5, 0.7, 0.4, 0.2);
        assert_eq!(a, b);
    }
}
