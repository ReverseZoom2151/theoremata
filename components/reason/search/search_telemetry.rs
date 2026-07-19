//! Proof-length / diversity **telemetry** over a set of found proofs or search
//! runs (the BFS-Prover pattern, `docs/paper-mining/` / prover-mining adopt-list).
//!
//! BFS-Prover reports that a healthy value-free search **deepens** round over
//! round — the proofs it finds get longer as the policy learns to push past the
//! shallow lemmas — while **not** collapsing onto a single mode (the flywheel
//! keeps finding *varied* proofs, not the same one over and over). Those are two
//! measurable health signals of the search/flywheel loop, and this module turns a
//! bag of found proofs into exactly those numbers so the driver / flywheel can
//! assert them.
//!
//! A "proof" here is a **tactic sequence** — a `Vec<String>`, precisely what
//! [`super::best_first::BestFirstOutcome::proof_tactics`] returns and what the
//! MCGS driver's robust-child walk yields. Nothing about the goal state, the
//! prover backend, or the model is needed: telemetry is a pure function of the
//! tactic strings, so it is cheap, offline, and deterministic.
//!
//! ## What is measured
//!
//! * [`proof_length_stats`] — count / mean / median / min / max proof length plus
//!   a full length **histogram** (`length ⇒ #proofs`). "Lengths deepen" is read
//!   off the mean/median rising across rounds.
//! * [`diversity`] — three mode-collapse guards over the *same* bag: the
//!   `distinct_ratio` (fraction of proofs that are unique sequences), the
//!   `mean_pairwise_jaccard` (how much any two proofs share their tactic
//!   vocabulary — `1.0` ⇒ everything identical ⇒ collapse), and the
//!   `distinct_tactics` count (the breadth of the tactic vocabulary explored).
//! * [`round_over_round`] — [`proof_length_stats`] per round, so a caller can
//!   assert a deepening trend (mean length rising) directly.
//!
//! ## Determinism contract
//!
//! Every function is a pure, order-insensitive fold over the input using exact
//! integer set / multiset operations (no floating-point accumulation of counts,
//! no hashing-order dependence in any returned value — histograms and tactic sets
//! are `BTreeMap`/`BTreeSet`, so iteration order is the natural key order). There
//! is **no** wall-clock and **no** randomness anywhere: the same proofs always
//! yield byte-identical stats.

use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Summary statistics over the *lengths* (tactic counts) of a bag of proofs.
///
/// Lengths are the number of tactics in each proof (`proof.len()`). On an empty
/// bag every field is zero / empty — there is nothing to summarise, and callers
/// can treat `count == 0` as "no data" without a panic.
#[derive(Debug, Clone, PartialEq)]
pub struct ProofStats {
    /// Number of proofs summarised.
    pub count: usize,
    /// Arithmetic mean proof length. `0.0` when `count == 0`.
    pub mean_length: f64,
    /// Median proof length (mean of the two central lengths for an even count).
    /// `0.0` when `count == 0`.
    pub median_length: f64,
    /// Shortest proof length. `0` when `count == 0`.
    pub min_length: usize,
    /// Longest proof length. `0` when `count == 0`.
    pub max_length: usize,
    /// `length ⇒ how many proofs have that length`, in ascending length order
    /// (a `BTreeMap`, so iteration is deterministic).
    pub length_histogram: BTreeMap<usize, usize>,
}

impl ProofStats {
    /// The all-zero stats for an empty bag of proofs.
    fn empty() -> Self {
        Self {
            count: 0,
            mean_length: 0.0,
            median_length: 0.0,
            min_length: 0,
            max_length: 0,
            length_histogram: BTreeMap::new(),
        }
    }
}

/// Compute length statistics over `proofs` (each a tactic sequence).
///
/// The mean is exact (`sum / count`), the median is taken from the sorted length
/// multiset (averaging the two central values for an even count), and the
/// histogram counts proofs per length. An empty input yields
/// [`ProofStats::empty`].
pub fn proof_length_stats(proofs: &[Vec<String>]) -> ProofStats {
    if proofs.is_empty() {
        return ProofStats::empty();
    }

    let mut lengths: Vec<usize> = proofs.iter().map(|p| p.len()).collect();
    lengths.sort_unstable();

    let count = lengths.len();
    let sum: usize = lengths.iter().sum();
    let mean_length = sum as f64 / count as f64;

    // Median from the sorted lengths: exact central value, or the mean of the two
    // central values for an even count.
    let median_length = if count % 2 == 1 {
        lengths[count / 2] as f64
    } else {
        let hi = count / 2;
        (lengths[hi - 1] + lengths[hi]) as f64 / 2.0
    };

    // min/max: the ends of the sorted vector (count > 0 guaranteed above).
    let min_length = lengths[0];
    let max_length = lengths[count - 1];

    let mut length_histogram: BTreeMap<usize, usize> = BTreeMap::new();
    for &l in &lengths {
        *length_histogram.entry(l).or_insert(0) += 1;
    }

    ProofStats {
        count,
        mean_length,
        median_length,
        min_length,
        max_length,
        length_histogram,
    }
}

/// Mode-collapse telemetry over a bag of proofs — how *varied* the proofs are.
///
/// All three fields fall as the search collapses onto a single proof and rise as
/// it explores a genuinely diverse set of proofs.
#[derive(Debug, Clone, PartialEq)]
pub struct DiversityReport {
    /// Fraction of proofs that are **unique** tactic sequences:
    /// `#distinct_sequences / #proofs`, in `(0, 1]`. `1.0` ⇒ every proof is
    /// different; a low value ⇒ the same proof recurs (mode collapse). `0.0` on
    /// an empty bag.
    pub distinct_ratio: f64,
    /// Mean Jaccard similarity of the *tactic sets* over all unordered proof
    /// pairs, in `[0, 1]`. `1.0` ⇒ every pair shares the exact same tactic
    /// vocabulary (identical proofs collapse to `1.0`); lower ⇒ proofs draw on
    /// different tactics. Defined as `1.0` for fewer than two proofs (no pair to
    /// compare — trivially self-similar).
    pub mean_pairwise_jaccard: f64,
    /// Number of **distinct tactic strings** across every proof — the breadth of
    /// the tactic vocabulary the search actually used.
    pub distinct_tactics: usize,
}

impl DiversityReport {
    /// The report for an empty bag: no proofs, no shared vocabulary, no tactics.
    fn empty() -> Self {
        Self {
            distinct_ratio: 0.0,
            // No pair to compare: trivially self-similar, matching the <2-proof rule.
            mean_pairwise_jaccard: 1.0,
            distinct_tactics: 0,
        }
    }
}

/// Exact Jaccard similarity `|A ∩ B| / |A ∪ B|` of two tactic **sets**. Two empty
/// sets are defined as identical (`1.0`) — two zero-tactic proofs share the same
/// (empty) vocabulary.
fn jaccard(a: &BTreeSet<&str>, b: &BTreeSet<&str>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let inter = a.intersection(b).count();
    let union = a.union(b).count();
    // `union == 0` only when both are empty, handled above.
    inter as f64 / union as f64
}

/// Compute the diversity / mode-collapse telemetry for `proofs`.
///
/// `distinct_ratio` uses the exact set of distinct full tactic *sequences*;
/// `mean_pairwise_jaccard` averages the exact set Jaccard over the tactic
/// *vocabularies* of every unordered pair; `distinct_tactics` is the exact size
/// of the union of all tactic strings. An empty input yields
/// [`DiversityReport::empty`].
pub fn diversity(proofs: &[Vec<String>]) -> DiversityReport {
    if proofs.is_empty() {
        return DiversityReport::empty();
    }

    // distinct_ratio: dedup on the full sequence (a proof's identity).
    let distinct_sequences: HashSet<&[String]> = proofs.iter().map(|p| p.as_slice()).collect();
    let distinct_ratio = distinct_sequences.len() as f64 / proofs.len() as f64;

    // distinct_tactics: the exact union of every tactic string used anywhere.
    let vocabulary: BTreeSet<&str> = proofs
        .iter()
        .flat_map(|p| p.iter().map(String::as_str))
        .collect();
    let distinct_tactics = vocabulary.len();

    // mean_pairwise_jaccard: average Jaccard of tactic sets over all i<j pairs.
    let sets: Vec<BTreeSet<&str>> = proofs
        .iter()
        .map(|p| p.iter().map(String::as_str).collect())
        .collect();
    let mean_pairwise_jaccard = if sets.len() < 2 {
        // A single (or empty) bag has no pair to compare: trivially self-similar.
        1.0
    } else {
        let mut sum = 0.0;
        let mut pairs = 0usize;
        for i in 0..sets.len() {
            for j in (i + 1)..sets.len() {
                sum += jaccard(&sets[i], &sets[j]);
                pairs += 1;
            }
        }
        sum / pairs as f64
    };

    DiversityReport {
        distinct_ratio,
        mean_pairwise_jaccard,
        distinct_tactics,
    }
}

/// Per-round length statistics: [`proof_length_stats`] applied to each round of
/// found proofs, preserving round order.
///
/// A round is the bag of proofs a search / flywheel iteration produced; the
/// returned `Vec` is parallel to `rounds`, so `result[r]` summarises round `r`.
/// Callers assert a **deepening** trend by checking that `mean_length` (or
/// `median_length`) is non-decreasing across the returned stats, and that
/// diversity holds by pairing this with [`diversity`] per round.
pub fn round_over_round(rounds: &[Vec<Vec<String>>]) -> Vec<ProofStats> {
    rounds.iter().map(|r| proof_length_stats(r)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a proof (tactic sequence) from string literals.
    fn proof(tactics: &[&str]) -> Vec<String> {
        tactics.iter().map(|s| s.to_string()).collect()
    }

    /// A proof of a given length whose tactics are all `t` — a length knob for the
    /// stats tests that keeps the vocabulary trivial.
    fn proof_of_len(n: usize) -> Vec<String> {
        vec!["t".to_string(); n]
    }

    // ---- proof_length_stats --------------------------------------------------

    #[test]
    fn length_stats_known_odd_set() {
        // Lengths {2, 4, 9}: mean 5, median 4 (central), min 2, max 9.
        let proofs = vec![proof_of_len(2), proof_of_len(4), proof_of_len(9)];
        let s = proof_length_stats(&proofs);
        assert_eq!(s.count, 3);
        assert!((s.mean_length - 5.0).abs() < 1e-12);
        assert!((s.median_length - 4.0).abs() < 1e-12);
        assert_eq!(s.min_length, 2);
        assert_eq!(s.max_length, 9);
        let expected: BTreeMap<usize, usize> = [(2, 1), (4, 1), (9, 1)].into_iter().collect();
        assert_eq!(s.length_histogram, expected);
    }

    #[test]
    fn length_stats_even_set_averages_two_central_medians() {
        // Lengths {1, 2, 2, 5}: mean 2.5, median (2+2)/2 = 2, histogram has 2×len-2.
        let proofs = vec![
            proof_of_len(1),
            proof_of_len(2),
            proof_of_len(2),
            proof_of_len(5),
        ];
        let s = proof_length_stats(&proofs);
        assert_eq!(s.count, 4);
        assert!((s.mean_length - 2.5).abs() < 1e-12);
        assert!((s.median_length - 2.0).abs() < 1e-12);
        assert_eq!(s.min_length, 1);
        assert_eq!(s.max_length, 5);
        let expected: BTreeMap<usize, usize> = [(1, 1), (2, 2), (5, 1)].into_iter().collect();
        assert_eq!(s.length_histogram, expected);
    }

    #[test]
    fn length_stats_empty_is_all_zero() {
        let s = proof_length_stats(&[]);
        assert_eq!(s, ProofStats::empty());
        assert_eq!(s.count, 0);
        assert_eq!(s.mean_length, 0.0);
        assert_eq!(s.median_length, 0.0);
        assert!(s.length_histogram.is_empty());
    }

    #[test]
    fn length_stats_is_order_insensitive() {
        // The same multiset of lengths in two orders yields identical stats.
        let a = vec![proof_of_len(3), proof_of_len(1), proof_of_len(3)];
        let b = vec![proof_of_len(3), proof_of_len(3), proof_of_len(1)];
        assert_eq!(proof_length_stats(&a), proof_length_stats(&b));
    }

    // ---- diversity -----------------------------------------------------------

    #[test]
    fn identical_proofs_collapse_low_ratio_full_jaccard() {
        // Three copies of the same proof: only one distinct sequence, and every
        // pair shares the exact tactic vocabulary => jaccard 1.0.
        let p = proof(&["intro", "simp"]);
        let proofs = vec![p.clone(), p.clone(), p];
        let d = diversity(&proofs);
        assert!(
            (d.distinct_ratio - 1.0 / 3.0).abs() < 1e-12,
            "1 distinct / 3"
        );
        assert!(
            (d.mean_pairwise_jaccard - 1.0).abs() < 1e-12,
            "identical => jaccard 1"
        );
        assert_eq!(d.distinct_tactics, 2, "vocabulary {{intro, simp}}");
    }

    #[test]
    fn varied_proofs_raise_ratio_and_lower_jaccard() {
        // Three fully distinct proofs with disjoint vocabularies: every sequence is
        // unique (ratio 1.0) and no pair shares a tactic (jaccard 0.0).
        let proofs = vec![
            proof(&["a1", "a2"]),
            proof(&["b1", "b2"]),
            proof(&["c1", "c2"]),
        ];
        let d = diversity(&proofs);
        assert!(
            (d.distinct_ratio - 1.0).abs() < 1e-12,
            "all unique => ratio 1"
        );
        assert!(
            d.mean_pairwise_jaccard.abs() < 1e-12,
            "disjoint => jaccard 0"
        );
        assert_eq!(d.distinct_tactics, 6);

        // And this is strictly more diverse than the collapsed bag on both axes.
        let collapsed = {
            let p = proof(&["a1", "a2"]);
            diversity(&[p.clone(), p.clone(), p])
        };
        assert!(d.distinct_ratio > collapsed.distinct_ratio);
        assert!(d.mean_pairwise_jaccard < collapsed.mean_pairwise_jaccard);
    }

    #[test]
    fn partial_overlap_gives_intermediate_jaccard() {
        // Two proofs sharing one of two tactics each: |∩|=1, |∪|=3 => jaccard 1/3.
        let proofs = vec![proof(&["shared", "x"]), proof(&["shared", "y"])];
        let d = diversity(&proofs);
        assert!((d.mean_pairwise_jaccard - 1.0 / 3.0).abs() < 1e-12);
        assert!((d.distinct_ratio - 1.0).abs() < 1e-12, "sequences differ");
        assert_eq!(d.distinct_tactics, 3, "vocabulary {{shared, x, y}}");
    }

    #[test]
    fn diversity_empty_and_singleton() {
        let empty = diversity(&[]);
        assert_eq!(empty.distinct_ratio, 0.0);
        assert_eq!(empty.distinct_tactics, 0);
        assert!((empty.mean_pairwise_jaccard - 1.0).abs() < 1e-12);

        // A single proof: ratio 1.0, no pair => jaccard defined as 1.0.
        let one = diversity(&[proof(&["only"])]);
        assert!((one.distinct_ratio - 1.0).abs() < 1e-12);
        assert!((one.mean_pairwise_jaccard - 1.0).abs() < 1e-12);
        assert_eq!(one.distinct_tactics, 1);
    }

    #[test]
    fn diversity_dedups_on_full_sequence_not_multiset() {
        // Same tactics, different order => two distinct sequences (order matters for
        // sequence identity) but identical tactic sets (jaccard 1.0).
        let proofs = vec![proof(&["p", "q"]), proof(&["q", "p"])];
        let d = diversity(&proofs);
        assert!(
            (d.distinct_ratio - 1.0).abs() < 1e-12,
            "reordered => distinct"
        );
        assert!(
            (d.mean_pairwise_jaccard - 1.0).abs() < 1e-12,
            "same set => jaccard 1"
        );
    }

    // ---- round_over_round ----------------------------------------------------

    #[test]
    fn round_over_round_returns_per_round_stats_with_deepening_trend() {
        // Three rounds whose proofs get longer: mean lengths 1 -> 2 -> 4. The
        // returned stats are parallel to the rounds and show a strictly deepening
        // mean, exactly the BFS-Prover round-over-round signal.
        let rounds = vec![
            vec![proof_of_len(1), proof_of_len(1)], // mean 1
            vec![proof_of_len(2), proof_of_len(2)], // mean 2
            vec![proof_of_len(3), proof_of_len(5)], // mean 4
        ];
        let stats = round_over_round(&rounds);
        assert_eq!(stats.len(), 3);
        assert!((stats[0].mean_length - 1.0).abs() < 1e-12);
        assert!((stats[1].mean_length - 2.0).abs() < 1e-12);
        assert!((stats[2].mean_length - 4.0).abs() < 1e-12);
        // Strictly deepening across rounds.
        assert!(stats[0].mean_length < stats[1].mean_length);
        assert!(stats[1].mean_length < stats[2].mean_length);
        // Per-round stats match a direct call on that round.
        assert_eq!(stats[1], proof_length_stats(&rounds[1]));
    }

    #[test]
    fn round_over_round_handles_empty_rounds() {
        let stats = round_over_round(&[]);
        assert!(stats.is_empty());

        // A round with no proofs summarises to empty stats in place.
        let rounds = vec![vec![], vec![proof_of_len(2)]];
        let stats = round_over_round(&rounds);
        assert_eq!(stats[0], ProofStats::empty());
        assert_eq!(stats[1].count, 1);
    }

    // ---- determinism ---------------------------------------------------------

    #[test]
    fn telemetry_is_deterministic() {
        let build = || {
            vec![
                proof(&["intro", "simp", "ring"]),
                proof(&["intro", "omega"]),
                proof(&["intro", "simp", "ring"]),
                proof(&["nlinarith"]),
            ]
        };
        let a = build();
        let b = build();
        assert_eq!(proof_length_stats(&a), proof_length_stats(&b));
        assert_eq!(diversity(&a), diversity(&b));

        let ra = vec![build(), build()];
        let rb = vec![build(), build()];
        assert_eq!(round_over_round(&ra), round_over_round(&rb));
    }
}
