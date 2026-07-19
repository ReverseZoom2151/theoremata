//! Distance-Critic label/encoding + eta-MCTS adaptive expansion budget
//! (HunyuanProver, `docs/resource-mining/*` prover mine).
//!
//! Two orthogonal ideas from HunyuanProver's system-2 search, both offline and
//! deterministic here, both with a GPU-gated learned head sitting *behind* a
//! clean seam:
//!
//! 1. **Distance-Critic (DC) label/encoding.** A plain scalar "estimated
//!    remaining steps to close the goal" is a hard regression target for a value
//!    head: the loss is dominated by far-away states and the near/far boundary
//!    (the part that actually gates search) is under-resolved. HunyuanProver's
//!    trick is to make the target *coarse-to-fine*: encode the remaining-distance
//!    estimate as the **root-to-leaf path down a balanced binary tree** over the
//!    range `1..=N`. Each level is one binary decision — "is the goal in the
//!    nearer half or the farther half of the remaining interval?" — so the head
//!    learns a sequence of easy binary classifications that refine from coarse
//!    (near vs far) to fine (exact step count). That is what makes DC learnable.
//!    This module is the pure label codec: [`encode_distance`] (the exact
//!    root-to-leaf path), [`decode_distance`] (its inverse, round-trip exact for
//!    every `steps` in `1..=N`), and [`distance_score`] — the monotonic critic
//!    signal in `[0, 1]` a search reads off the path (nearer ⇒ higher).
//!
//! 2. **eta-MCTS adaptive per-node expansion budget.** Vanilla MCTS expands every
//!    node with the same fixed breadth. HunyuanProver's eta-MCTS instead spends
//!    its expansion budget where it matters: high-importance / high-value-gap
//!    nodes (the ones whose children disagree, so exploring more actually changes
//!    the decision) get *more* children, settled nodes get *fewer*.
//!    [`expansion_budget`] is that pure allocation function — monotonic in
//!    importance and clamped to `[min, max]`.
//!
//! There is **no** wall-clock or unseeded randomness anywhere: every function is
//! a pure function of its inputs, so the labels and budgets are reproducible.
//!
//! ## The trained-head seam (GPU-gated, offline here)
//!
//! The learned DC value head is *not* built here — that is GPU training, gated in
//! the Python trainer. This module manufactures the **labels** it trains on and
//! the **decoder** it uses at inference:
//! * Offline: for each visited proof state, take a Monte-Carlo estimate of the
//!   remaining steps to closure and call [`encode_distance`] → a `Vec<Bit>`. Each
//!   bit is one binary-cross-entropy target for the corresponding tree level. The
//!   head is a stack of `ceil(log2 N)` binary classifiers (a coarse-to-fine head).
//! * Inference: the trained head emits a predicted path; [`distance_score`] turns
//!   it into a scalar critic in `[0, 1]` that feeds PUCT exactly where the
//!   progress/value prior does today ([`crate::search::driver`]'s
//!   `progress_weight * c.progress` term), and [`expansion_budget`] sizes each
//!   node's breadth from that critic's value gap.

use serde::Serialize;

/// One coarse-to-fine decision on the root-to-leaf path of the Distance-Critic
/// balanced binary tree. At each level the remaining-distance interval is split in
/// half: [`Near`](Bit::Near) descends into the nearer half (fewer remaining steps,
/// closer to closing the goal), [`Far`](Bit::Far) into the farther half. The path
/// read most-significant-first is the plain binary index of the leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Bit {
    /// The goal is in the nearer half of the current interval (fewer steps left).
    Near,
    /// The goal is in the farther half of the current interval (more steps left).
    Far,
}

/// Depth of the balanced binary tree covering `1..=n`: `ceil(log2 n)` — the exact
/// number of coarse-to-fine decisions (and hence the path / label length). `n <= 1`
/// needs no decisions (a single leaf), so the depth is `0`.
pub fn tree_depth(n: usize) -> usize {
    if n <= 1 {
        return 0;
    }
    let mut depth = 0usize;
    let mut cap = 1usize;
    while cap < n {
        cap <<= 1;
        depth += 1;
    }
    depth
}

/// Encode an estimated remaining-steps count `steps` (in `1..=n`) as the exact
/// **root-to-leaf path** down the balanced binary tree over `1..=n`.
///
/// This is the offline Distance-Critic label: a `Vec<Bit>` of length
/// [`tree_depth(n)`](tree_depth) = `ceil(log2 n)`, coarse (first bit: near vs far
/// half) to fine (last bit: the exact leaf). Out-of-range `steps` are clamped into
/// `1..=n`. The encoding is a binary search over the index interval `[0, 2^depth)`
/// so it round-trips exactly through [`decode_distance`] for every `steps` in
/// `1..=n`.
pub fn encode_distance(steps: usize, n: usize) -> Vec<Bit> {
    assert!(n >= 1, "distance range 1..=n requires n >= 1");
    let depth = tree_depth(n);
    let target = steps.clamp(1, n) - 1; // 0-based leaf index
    let mut lo = 0usize;
    let mut hi = 1usize << depth; // 2^depth, always >= n
    let mut path = Vec::with_capacity(depth);
    for _ in 0..depth {
        let mid = (lo + hi) / 2;
        if target < mid {
            path.push(Bit::Near);
            hi = mid;
        } else {
            path.push(Bit::Far);
            lo = mid;
        }
    }
    path
}

/// Decode a Distance-Critic path back to its remaining-steps count in `1..=n` —
/// the exact inverse of [`encode_distance`]. Replays the same binary search over
/// `[0, 2^len)`; after consuming the path the interval has collapsed to the single
/// encoded index, and `steps = index + 1` (clamped into `1..=n` for safety).
pub fn decode_distance(path: &[Bit], n: usize) -> usize {
    let mut lo = 0usize;
    let mut hi = 1usize << path.len(); // 2^len
    for &bit in path {
        let mid = (lo + hi) / 2;
        match bit {
            Bit::Near => hi = mid,
            Bit::Far => lo = mid,
        }
    }
    (lo + 1).clamp(1, n.max(1))
}

/// The Distance-Critic scalar signal in `[0, 1]` read off a path: **nearer ⇒
/// higher**. A path of all-[`Near`](Bit::Near) (the closest leaf, `steps == 1`)
/// scores `1.0`; all-[`Far`](Bit::Far) (the farthest leaf) scores `0.0`; the score
/// decreases monotonically as the encoded remaining-step count grows. Depends only
/// on the path (an empty path — the degenerate `n <= 1` tree — is maximally near,
/// `1.0`), so a trained head's predicted path maps straight to a critic value
/// usable like a progress/value prior in PUCT.
pub fn distance_score(path: &[Bit]) -> f64 {
    let depth = path.len();
    if depth == 0 {
        return 1.0;
    }
    // Path MSB-first is the binary leaf index (Near = 0, Far = 1).
    let mut index = 0u64;
    for &bit in path {
        index <<= 1;
        if let Bit::Far = bit {
            index |= 1;
        }
    }
    let max_index = (1u64 << depth) - 1;
    1.0 - (index as f64) / (max_index as f64)
}

/// Tuning for the eta-MCTS adaptive expansion budget. A settled node
/// (importance `0`) receives `base_budget`; a maximally important node receives
/// `base_budget * (1 + importance_gain)`, all clamped into `[min_budget,
/// max_budget]`.
#[derive(Debug, Clone, Copy)]
pub struct EtaMctsConfig {
    /// Floor on the per-node breadth — even a fully settled node expands at least
    /// this many children (guarantees progress).
    pub min_budget: usize,
    /// Ceiling on the per-node breadth — even a maximally important node never
    /// expands more than this (bounds the branching factor / compute).
    pub max_budget: usize,
    /// How strongly importance scales the base budget. `0.0` disables adaptation
    /// (every node gets `base_budget`); larger values spend more on hot nodes.
    pub importance_gain: f64,
}

impl Default for EtaMctsConfig {
    fn default() -> Self {
        // Mirrors the driver's fixed `expand_k` band: a small floor, a modest
        // ceiling, and a gain that roughly triples breadth at peak importance.
        Self {
            min_budget: 1,
            max_budget: 16,
            importance_gain: 2.0,
        }
    }
}

/// eta-MCTS adaptive per-node expansion budget: how many children to expand at a
/// node of the given `node_importance` (typically its value gap in `[0, 1]`).
///
/// Monotonic non-decreasing in `node_importance` (with `importance_gain >= 0`) and
/// always within `[cfg.min_budget, cfg.max_budget]`. A settled node
/// (`node_importance == 0`) gets `base_budget`; importance scales the base up
/// toward the ceiling, so the search spends breadth where the children disagree
/// and conserves it where the outcome is decided. `node_importance` is clamped to
/// `[0, 1]`, so out-of-range inputs are safe.
pub fn expansion_budget(node_importance: f64, base_budget: usize, cfg: &EtaMctsConfig) -> usize {
    // Clamp importance (and guard NaN → 0.0) into [0, 1].
    let imp = if node_importance.is_nan() {
        0.0
    } else {
        node_importance.clamp(0.0, 1.0)
    };
    let gain = cfg.importance_gain.max(0.0);
    let scaled = (base_budget as f64) * (1.0 + gain * imp);
    // Round to the nearest whole child count, then clamp into [min, max].
    let rounded = scaled.round() as i64;
    let (lo, hi) = (
        cfg.min_budget.min(cfg.max_budget),
        cfg.max_budget.max(cfg.min_budget),
    );
    rounded.clamp(lo as i64, hi as i64) as usize
}

/// Map a child value gap — how much a node's best and worst child value estimates
/// disagree — into a `node_importance` in `[0, 1]` for [`expansion_budget`]. This
/// is the concrete eta-MCTS seam: a wide gap (children strongly disagree ⇒
/// exploring more can flip the decision) is important; a zero gap (settled) is
/// not. `scale` is the gap magnitude that saturates to full importance; the map is
/// monotonic non-decreasing in `gap` and clamped to `[0, 1]`.
pub fn importance_from_value_gap(gap: f64, scale: f64) -> f64 {
    if scale <= 0.0 {
        return 0.0;
    }
    (gap.abs() / scale).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode → decode is exact for every step in `1..=N`, across several `N`
    /// (powers of two and non-powers alike).
    #[test]
    fn encode_decode_round_trips_for_all_steps() {
        for &n in &[1usize, 2, 3, 7, 32, 50, 64] {
            for steps in 1..=n {
                let path = encode_distance(steps, n);
                let back = decode_distance(&path, n);
                assert_eq!(back, steps, "round-trip failed for steps={steps}, n={n}");
            }
        }
    }

    /// The path (label) length is exactly `ceil(log2 N)` for every step.
    #[test]
    fn path_length_is_ceil_log2_n() {
        // Spot-check the depth helper against known ceil(log2 N) values.
        assert_eq!(tree_depth(1), 0);
        assert_eq!(tree_depth(2), 1);
        assert_eq!(tree_depth(3), 2);
        assert_eq!(tree_depth(4), 2);
        assert_eq!(tree_depth(5), 3);
        assert_eq!(tree_depth(50), 6);
        assert_eq!(tree_depth(64), 6);

        for &n in &[1usize, 2, 3, 7, 32, 50, 64] {
            let expected = tree_depth(n);
            for steps in 1..=n {
                assert_eq!(
                    encode_distance(steps, n).len(),
                    expected,
                    "path length must be ceil(log2 {n}) for steps={steps}"
                );
            }
        }
    }

    /// `distance_score` is monotonic: fewer remaining steps ⇒ strictly higher
    /// score, spanning the full `[0, 1]` range at the endpoints.
    #[test]
    fn distance_score_is_monotonic_nearer_is_higher() {
        let n = 64;
        let mut prev = f64::INFINITY;
        for steps in 1..=n {
            let s = distance_score(&encode_distance(steps, n));
            assert!(
                s >= 0.0 && s <= 1.0,
                "score {s} out of [0,1] at steps={steps}"
            );
            assert!(
                s < prev,
                "score must strictly decrease as steps grow: steps={steps} score={s} prev={prev}"
            );
            prev = s;
        }
        // Endpoints: nearest is 1.0, farthest is 0.0.
        assert!((distance_score(&encode_distance(1, n)) - 1.0).abs() < 1e-12);
        assert!(distance_score(&encode_distance(n, n)).abs() < 1e-12);
        // The degenerate single-leaf tree is maximally near.
        assert_eq!(distance_score(&[]), 1.0);
    }

    /// The all-Near path scores 1.0 and all-Far scores 0.0 regardless of depth.
    #[test]
    fn distance_score_endpoints_by_construction() {
        for depth in 1..=6usize {
            let near = vec![Bit::Near; depth];
            let far = vec![Bit::Far; depth];
            assert!((distance_score(&near) - 1.0).abs() < 1e-12);
            assert!(distance_score(&far).abs() < 1e-12);
        }
    }

    /// `expansion_budget` grows monotonically with importance and respects the
    /// `[min, max]` bounds at the extremes.
    #[test]
    fn expansion_budget_grows_with_importance_and_is_bounded() {
        let cfg = EtaMctsConfig {
            min_budget: 2,
            max_budget: 16,
            importance_gain: 3.0,
        };
        let base = 4;

        // Sweep importance; budget must be non-decreasing and within bounds.
        let mut prev = 0usize;
        for i in 0..=10 {
            let imp = i as f64 / 10.0;
            let b = expansion_budget(imp, base, &cfg);
            assert!(
                b >= cfg.min_budget && b <= cfg.max_budget,
                "budget {b} out of bounds"
            );
            assert!(b >= prev, "budget must be non-decreasing in importance");
            prev = b;
        }

        // A settled node gets the base budget; peak importance scales up but is
        // capped at max_budget (4 * (1 + 3*1) = 16).
        assert_eq!(expansion_budget(0.0, base, &cfg), 4);
        assert_eq!(expansion_budget(1.0, base, &cfg), 16);
        // A high importance that would exceed the ceiling is clamped.
        assert_eq!(expansion_budget(1.0, 8, &cfg), 16);
        // A base below the floor is lifted to min_budget.
        assert_eq!(expansion_budget(0.0, 1, &cfg), 2);
        // Out-of-range / NaN importance is handled (clamped to [0,1] / 0.0).
        assert_eq!(expansion_budget(-5.0, base, &cfg), 4);
        assert_eq!(expansion_budget(f64::NAN, base, &cfg), 4);
    }

    /// Disabling the gain freezes every node at the base budget (no adaptation).
    #[test]
    fn zero_gain_disables_adaptation() {
        let cfg = EtaMctsConfig {
            min_budget: 1,
            max_budget: 32,
            importance_gain: 0.0,
        };
        for i in 0..=10 {
            assert_eq!(expansion_budget(i as f64 / 10.0, 5, &cfg), 5);
        }
    }

    /// The value-gap → importance seam is monotonic and saturating.
    #[test]
    fn importance_from_value_gap_is_monotonic_and_saturating() {
        let scale = 2.0;
        assert_eq!(importance_from_value_gap(0.0, scale), 0.0);
        assert!((importance_from_value_gap(1.0, scale) - 0.5).abs() < 1e-12);
        assert_eq!(importance_from_value_gap(2.0, scale), 1.0);
        // Saturates past the scale and is sign-insensitive.
        assert_eq!(importance_from_value_gap(5.0, scale), 1.0);
        assert_eq!(importance_from_value_gap(-5.0, scale), 1.0);
        // A non-positive scale yields zero importance (no divide-by-zero).
        assert_eq!(importance_from_value_gap(1.0, 0.0), 0.0);
    }

    /// Everything is deterministic: identical inputs give identical outputs.
    #[test]
    fn codec_and_budget_are_deterministic() {
        let cfg = EtaMctsConfig::default();
        for &n in &[16usize, 64] {
            for steps in 1..=n {
                assert_eq!(encode_distance(steps, n), encode_distance(steps, n));
            }
        }
        for i in 0..=20 {
            let imp = i as f64 / 20.0;
            assert_eq!(
                expansion_budget(imp, 4, &cfg),
                expansion_budget(imp, 4, &cfg)
            );
        }
    }

    /// A full round of the intended pipeline: a Monte-Carlo remaining-steps
    /// estimate becomes a coarse-to-fine label, and the label's score drives a
    /// larger expansion budget when the state is near (high value) than when far.
    #[test]
    fn pipeline_near_state_gets_more_budget_than_far_state() {
        let n = 64;
        let cfg = EtaMctsConfig::default();
        let near_score = distance_score(&encode_distance(2, n)); // ~closing
        let far_score = distance_score(&encode_distance(60, n)); // far away
        assert!(near_score > far_score);
        // Treat the critic score as node importance (a near/high-value node is
        // where more expansion pays off).
        let near_budget = expansion_budget(near_score, 4, &cfg);
        let far_budget = expansion_budget(far_score, 4, &cfg);
        assert!(
            near_budget > far_budget,
            "near state ({near_score}) should out-budget far state ({far_score})"
        );
    }
}
