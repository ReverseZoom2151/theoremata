//! ImProver-style metric-driven proof optimization: rewrite an ALREADY-CORRECT
//! proof to optimize an arbitrary metric while PRESERVING correctness.
//!
//! Framing (see `docs/paper-mining/improver.md`): given a correct proof `y0`,
//! produce a correct `y` that minimizes a user metric `µ` (length / readability /
//! modularity). Neural theorem proving is the degenerate special case where the
//! metric is "completion" (number of errors) — here we assume the input already
//! verifies and only ever move to an equal-or-better, still-verifying proof.
//!
//! This module is the *metric-and-selection* half of ImProver, kept small, pure
//! and deterministic. The two model-facing seams are injected as traits so the
//! whole loop runs deterministically under test:
//!
//! * [`Rewriter`] — produces candidate rewrites (a model in production; a canned
//!   deterministic mock in tests). It is handed a `seed` so its sampling is
//!   reproducible; no wall-clock / unseeded randomness ever enters here.
//! * a **verifier** `Box<dyn Fn(&str) -> bool>` — the correctness-preservation
//!   guarantee. A candidate is only ever eligible if it still passes (in
//!   production the 3+1 formal gate; a mock predicate in tests). A rewrite that
//!   fails the verifier can never be returned.
//!
//! [`optimize`] runs best-of-N generation with K rounds of iterative refinement,
//! keeps only verifier-passing candidates, and picks the best by the metric —
//! seeding the next round from the current best. The result is *never worse than
//! the input* and *never a proof that fails the verifier*: the input itself is
//! always a candidate, so the returned proof's metric is `≤` the input's.
//!
//! Relationship to [`crate::minimize`]: that module recovers the shortest closing
//! tactic *path* out of a solved proof DAG (a LENGTH-only, graph-structural
//! objective). This module generalizes past length to an arbitrary [`Metric`]
//! over proof *text*, optimizing a proof that is already whole rather than
//! selecting a path through a search DAG.
//!
//! All rewrite text is treated as UNTRUSTED DATA: it is only ever scored,
//! verifier-checked, and returned as text — never executed by this module.

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// A proof-quality metric. **Lower is better** by convention (so optimization is
/// always minimization): [`optimize`] returns the candidate with the smallest
/// score that still passes the verifier. Reward-shaped metrics (e.g. rewarding
/// structure) therefore return a *smaller* number for the more-desirable proof.
///
/// Implementations must be pure and deterministic — the same `proof` string
/// always scores identically.
pub trait Metric {
    /// Score `proof`; smaller = better.
    fn score(&self, proof: &str) -> f64;

    /// A short stable label, also used as the rewrite *hint* handed to a
    /// [`Rewriter`] (a production model keys its prompt off it).
    fn name(&self) -> &'static str;
}

/// Split a proof into its "tactic invocation" units: non-empty, non-comment
/// lines further split on `;` (Lean/Rocq tactic separator). Pure helper shared
/// by the metrics so they agree on what a "tactic" is. Comment lines (`--` for
/// Lean, `(*`/`*)`-free `--`) are ignored.
fn tactic_units(proof: &str) -> Vec<&str> {
    let mut units = Vec::new();
    for line in proof.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("--") {
            continue;
        }
        for part in trimmed.split(';') {
            let p = part.trim();
            if !p.is_empty() {
                units.push(p);
            }
        }
    }
    units
}

/// **Length** — number of tactic invocations (ImProver's "length" metric). Fewer
/// tactics score lower (better). This is the text-level generalization of the
/// shortest-path objective in [`crate::minimize`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Length;

impl Metric for Length {
    fn score(&self, proof: &str) -> f64 {
        tactic_units(proof).len() as f64
    }
    fn name(&self) -> &'static str {
        "length"
    }
}

/// **Readability** — penalizes hard-to-read proofs. A proof scores *worse*
/// (higher) for deep nesting (indentation), over-long lines, and raw term-mode
/// (a proof body with no `by`, i.e. a dense proof term rather than a readable
/// tactic block). All contributions are non-negative, so a flat, short-lined,
/// tactic-style proof scores near `0`.
#[derive(Debug, Clone, Copy)]
pub struct Readability {
    /// Column beyond which a line counts as "too long".
    pub max_line: usize,
}

impl Default for Readability {
    fn default() -> Self {
        // 100 columns is the usual Lean/Mathlib line-length budget.
        Self { max_line: 100 }
    }
}

impl Metric for Readability {
    fn score(&self, proof: &str) -> f64 {
        let mut penalty = 0.0;
        let mut saw_by = false;
        let mut saw_content = false;
        for line in proof.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            saw_content = true;
            if trimmed.contains("by") {
                saw_by = true;
            }
            if trimmed.starts_with("--") {
                // Comments don't add nesting/length pressure the same way.
                continue;
            }
            // Nesting: one penalty unit per two leading spaces (a tab = 2).
            let indent: usize = line
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .map(|c| if c == '\t' { 2 } else { 1 })
                .sum();
            penalty += (indent / 2) as f64;
            // Over-long lines: 0.1 per column past the budget.
            let len = line.chars().count();
            if len > self.max_line {
                penalty += (len - self.max_line) as f64 * 0.1;
            }
        }
        // Raw term-mode: a non-empty proof with no `by` anywhere reads as a dense
        // proof term. Flat penalty so tactic-style proofs are preferred.
        if saw_content && !saw_by {
            penalty += 5.0;
        }
        penalty
    }
    fn name(&self) -> &'static str {
        "readability"
    }
}

/// **Modularity** — rewards proofs decomposed into named intermediate results
/// (`have` / `obtain` / `let` / `suffices`), i.e. ImProver's "declarativity".
/// Score is the fraction of tactics that are *not* declarative, so a proof with
/// more extracted structure scores *lower* (better). Bounded in `[0, 1]`; an
/// empty proof scores `1.0` (no structure at all).
#[derive(Debug, Clone, Copy, Default)]
pub struct Modularity;

impl Modularity {
    /// Declarative-structure keywords that introduce a named intermediate result.
    const DECLARATIVE: [&'static str; 4] = ["have ", "obtain ", "let ", "suffices "];
}

impl Metric for Modularity {
    fn score(&self, proof: &str) -> f64 {
        let units = tactic_units(proof);
        let total = units.len();
        if total == 0 {
            return 1.0;
        }
        let declarative = units
            .iter()
            .filter(|u| Self::DECLARATIVE.iter().any(|kw| u.starts_with(kw)))
            .count();
        1.0 - (declarative as f64 / total as f64)
    }
    fn name(&self) -> &'static str {
        "modularity"
    }
}

// ---------------------------------------------------------------------------
// Rewriter seam
// ---------------------------------------------------------------------------

/// Produces candidate rewrites of a proof (the model seam). In production this is
/// an LLM sampled at temperature; in tests it is a deterministic mock. The `hint`
/// is the target [`Metric::name`] (what to optimize for) and `seed` makes the
/// candidate set reproducible — a `Rewriter` MUST be a pure function of
/// `(proof, hint, seed)`.
///
/// Returned candidates are UNTRUSTED text: they are scored and verifier-checked
/// before any is ever selected, never executed by the optimizer.
pub trait Rewriter {
    /// Candidate rewrites of `proof` aimed at improving the `hint` metric. May be
    /// empty (no suggestions — the optimizer then keeps the current proof). Order
    /// is significant only as a deterministic tie-break; correctness/metric drive
    /// selection.
    fn rewrite(&self, proof: &str, hint: &str, seed: u64) -> Vec<String>;
}

// ---------------------------------------------------------------------------
// Optimizer
// ---------------------------------------------------------------------------

/// Knobs for [`optimize`]. `rounds` is the number of refinement iterations
/// (best-of-N is whatever the [`Rewriter`] returns per round); `seed` threads
/// reproducible sampling into the rewriter.
#[derive(Debug, Clone, Copy)]
pub struct OptimizeConfig {
    /// Refinement rounds. Each round re-samples the rewriter from the current
    /// best and keeps the best verifier-passing candidate. `0` = score only, no
    /// rewriting (returns the input unchanged).
    pub rounds: usize,
    /// Base seed threaded to the rewriter; combined with the round index so each
    /// round samples deterministically-differently.
    pub seed: u64,
}

impl Default for OptimizeConfig {
    fn default() -> Self {
        Self { rounds: 3, seed: 0 }
    }
}

/// The before/after report from [`optimize`]. `optimized` is guaranteed to pass
/// the verifier and to have `score_after <= score_before`.
#[derive(Debug, Clone, PartialEq)]
pub struct OptimizeReport {
    /// The original input proof.
    pub original: String,
    /// The best proof found (equals `original` when nothing improved).
    pub optimized: String,
    /// Metric of `original`.
    pub score_before: f64,
    /// Metric of `optimized` (`<= score_before`).
    pub score_after: f64,
    /// How many refinement rounds actually ran.
    pub rounds_run: usize,
    /// Total candidates the rewriter proposed across all rounds.
    pub candidates_seen: usize,
    /// How many of those passed the verifier.
    pub candidates_accepted: usize,
}

impl OptimizeReport {
    /// Whether a strictly-better proof was found.
    pub fn improved(&self) -> bool {
        self.score_after < self.score_before
    }

    /// The metric delta (positive = improvement, since lower is better).
    pub fn gain(&self) -> f64 {
        self.score_before - self.score_after
    }
}

/// Mix the base seed with the round index deterministically (splitmix64 finalizer)
/// so successive rounds sample differently but reproducibly.
fn round_seed(base: u64, round: usize) -> u64 {
    let mut z = base.wrapping_add((round as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Optimize an already-correct `proof` for `metric`: best-of-N generation with
/// `config.rounds` rounds of refinement, keeping only verifier-passing candidates
/// and always retaining the current best (so the result never regresses and never
/// fails the verifier).
///
/// * `metric`   — the objective (lower = better).
/// * `rewriter` — proposes candidates (injected; deterministic in tests).
/// * `verify`   — correctness gate; a candidate is eligible iff `verify(cand)`.
/// * `config`   — rounds + seed.
///
/// Returns an [`OptimizeReport`] with the before/after scores and the best proof
/// found. The input is treated as correct: even if it somehow fails `verify`, it
/// is still returned as the fallback (there is nothing better to return), so the
/// caller can inspect `score_before`/`score_after`.
pub fn optimize<M: Metric + ?Sized>(
    proof: &str,
    metric: &M,
    rewriter: &dyn Rewriter,
    verify: &dyn Fn(&str) -> bool,
    config: OptimizeConfig,
) -> OptimizeReport {
    let score_before = metric.score(proof);
    let hint = metric.name();

    let mut best = proof.to_string();
    let mut best_score = score_before;
    let mut candidates_seen = 0usize;
    let mut candidates_accepted = 0usize;
    let mut rounds_run = 0usize;

    for round in 0..config.rounds {
        rounds_run += 1;
        let seed = round_seed(config.seed, round);
        let candidates = rewriter.rewrite(&best, hint, seed);
        candidates_seen += candidates.len();

        let mut improved_this_round = false;
        for cand in candidates {
            // Correctness gate FIRST: a rewrite that fails the verifier can never
            // be selected, no matter how good its metric looks.
            if !verify(&cand) {
                continue;
            }
            candidates_accepted += 1;
            let s = metric.score(&cand);
            // Strict `<` so ties keep the incumbent — deterministic and biased
            // toward the earlier/original proof (no churn on equal metric).
            if s < best_score {
                best = cand;
                best_score = s;
                improved_this_round = true;
            }
        }

        // Refinement has converged: a round that yields no improvement will keep
        // re-sampling from the same `best` with a fresh seed, but a deterministic
        // rewriter would just repeat itself — stop early.
        if !improved_this_round {
            break;
        }
    }

    OptimizeReport {
        original: proof.to_string(),
        optimized: best,
        score_before,
        score_after: best_score,
        rounds_run,
        candidates_seen,
        candidates_accepted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Metric unit behavior ------------------------------------------------

    #[test]
    fn length_counts_tactic_units_ignoring_blanks_and_comments() {
        let proof = "by\n  intro x\n  simp; ring\n  -- a comment\n\n";
        // units: "by", "intro x", "simp", "ring" = 4
        assert_eq!(Length.score(proof), 4.0);
    }

    #[test]
    fn readability_orders_proofs_sensibly() {
        let clean = "by\nintro x\nsimp";
        let deeply_nested = "by\n          intro x\n              simp";
        let term_mode = "fun x => Eq.trans h1 h2";
        // Flat tactic proof reads best (lowest).
        assert!(Readability::default().score(clean) < Readability::default().score(deeply_nested));
        // Raw term-mode (no `by`) is penalized above a flat tactic proof.
        assert!(Readability::default().score(clean) < Readability::default().score(term_mode));
    }

    #[test]
    fn readability_penalizes_long_lines() {
        let short = "by\nsimp";
        let long = format!("by\n{}", "a".repeat(200));
        assert!(Readability::default().score(short) < Readability::default().score(&long));
    }

    #[test]
    fn modularity_rewards_declarative_structure() {
        let flat = "by\nsimp\nring\nlinarith";
        let structured = "by\nhave h1 : a = b := rfl\nhave h2 : b = c := rfl\nexact h1.trans h2";
        // More `have`s => lower (better) modularity score.
        assert!(Modularity.score(structured) < Modularity.score(flat));
        // Scores stay in [0,1].
        for s in [Modularity.score(flat), Modularity.score(structured)] {
            assert!((0.0..=1.0).contains(&s));
        }
    }

    #[test]
    fn modularity_empty_proof_is_worst() {
        assert_eq!(Modularity.score(""), 1.0);
    }

    // -- Rewriter mocks ------------------------------------------------------

    /// Returns a fixed set of candidates regardless of input (deterministic).
    struct FixedRewriter(Vec<&'static str>);
    impl Rewriter for FixedRewriter {
        fn rewrite(&self, _proof: &str, _hint: &str, _seed: u64) -> Vec<String> {
            self.0.iter().map(|s| s.to_string()).collect()
        }
    }

    /// Proposes nothing — the optimizer must keep the input.
    struct NoopRewriter;
    impl Rewriter for NoopRewriter {
        fn rewrite(&self, _proof: &str, _hint: &str, _seed: u64) -> Vec<String> {
            Vec::new()
        }
    }

    /// Picks a different candidate depending on the seed's low bit — used to prove
    /// the seed is actually threaded through and that runs are reproducible.
    struct SeedSensitiveRewriter;
    impl Rewriter for SeedSensitiveRewriter {
        fn rewrite(&self, _proof: &str, _hint: &str, seed: u64) -> Vec<String> {
            if seed & 1 == 0 {
                vec!["by\nsimp".to_string()]
            } else {
                vec!["by\nring".to_string()]
            }
        }
    }

    fn accept_all() -> Box<dyn Fn(&str) -> bool> {
        Box::new(|_: &str| true)
    }

    // -- optimize() behavior -------------------------------------------------

    #[test]
    fn optimizing_for_length_returns_shorter_passing_proof() {
        let input = "by\nintro x\nintro y\nsimp\nring\nlinarith"; // 6 tactics
        let rewriter = FixedRewriter(vec!["by\nsimp\nring", "by\ntauto"]);
        let verify = accept_all();
        let report = optimize(input, &Length, &rewriter, &*verify, OptimizeConfig::default());

        assert!(report.improved());
        // Best is the 1-tactic "by\ntauto" (score 2 counting `by`) ... actually
        // "by\ntauto" => 2 units, "by\nsimp\nring" => 3. Shortest wins.
        assert_eq!(report.optimized, "by\ntauto");
        assert!(report.score_after <= report.score_before);
        assert!((verify)(&report.optimized));
    }

    #[test]
    fn a_failing_rewrite_is_never_returned() {
        let input = "by\nintro x\nsimp\nring\nlinarith"; // 5 tactics
        // The SHORTEST candidate is "bad" and must be rejected by the verifier;
        // the optimizer must fall back to the shorter *valid* one.
        let rewriter = FixedRewriter(vec!["BAD", "by\nsimp"]);
        // Reject anything containing "BAD".
        let verify: Box<dyn Fn(&str) -> bool> = Box::new(|p: &str| !p.contains("BAD"));
        let report = optimize(input, &Length, &rewriter, &*verify, OptimizeConfig::default());

        assert_ne!(report.optimized, "BAD");
        assert_eq!(report.optimized, "by\nsimp");
        assert!((verify)(&report.optimized));
        // "BAD" was proposed but not accepted.
        assert!(report.candidates_seen >= 2);
    }

    #[test]
    fn every_rewrite_failing_returns_the_original_unchanged() {
        let input = "by\nsimp";
        let rewriter = FixedRewriter(vec!["by\ntauto", "by\ndecide"]);
        // Verifier rejects EVERY candidate; only the input is trusted.
        let verify: Box<dyn Fn(&str) -> bool> = Box::new(|p: &str| p == "by\nsimp");
        let report = optimize(input, &Length, &rewriter, &*verify, OptimizeConfig::default());

        assert_eq!(report.optimized, input);
        assert_eq!(report.candidates_accepted, 0);
        assert!(!report.improved());
    }

    #[test]
    fn no_metric_improvement_returns_original_unchanged() {
        let input = "by\nsimp"; // 2 tactics
        // All candidates are LONGER (worse length), though verifier-valid.
        let rewriter = FixedRewriter(vec!["by\nintro x\nsimp\nring", "by\nintro x\nsimp"]);
        let verify = accept_all();
        let report = optimize(input, &Length, &rewriter, &*verify, OptimizeConfig::default());

        assert_eq!(report.optimized, input);
        assert_eq!(report.score_after, report.score_before);
        assert!(!report.improved());
        // They were accepted (valid) but none beat the incumbent.
        assert!(report.candidates_accepted >= 1);
    }

    #[test]
    fn noop_rewriter_returns_original() {
        let input = "by\nsimp\nring";
        let report = optimize(
            input,
            &Length,
            &NoopRewriter,
            &*accept_all(),
            OptimizeConfig::default(),
        );
        assert_eq!(report.optimized, input);
        assert_eq!(report.candidates_seen, 0);
        assert!(!report.improved());
        // With no candidates the very first round shows no improvement and stops.
        assert_eq!(report.rounds_run, 1);
    }

    #[test]
    fn result_is_deterministic_given_seed() {
        let input = "by\nintro x";
        let rewriter = SeedSensitiveRewriter;
        let cfg = OptimizeConfig { rounds: 2, seed: 42 };
        let a = optimize(input, &Length, &rewriter, &*accept_all(), cfg);
        let b = optimize(input, &Length, &rewriter, &*accept_all(), cfg);
        assert_eq!(a, b);
    }

    #[test]
    fn optimizing_for_modularity_prefers_declarative_rewrite() {
        let input = "by\nsimp\nring"; // 0 declarative => score 1.0
        let rewriter = FixedRewriter(vec![
            "by\nhave h : a = b := rfl\nexact h", // 1/2 declarative => 0.5
            "by\ntauto",                          // 0 declarative => 1.0
        ]);
        let report = optimize(input, &Modularity, &rewriter, &*accept_all(), OptimizeConfig::default());
        assert_eq!(report.optimized, "by\nhave h : a = b := rfl\nexact h");
        assert!(report.improved());
    }

    #[test]
    fn refinement_runs_multiple_rounds_until_convergence() {
        // A rewriter that shaves one tactic each round: it always drops the first
        // tactic line of whatever it is given. Proves iterative refinement chains.
        struct ShaveRewriter;
        impl Rewriter for ShaveRewriter {
            fn rewrite(&self, proof: &str, _hint: &str, _seed: u64) -> Vec<String> {
                let lines: Vec<&str> = proof.lines().collect();
                if lines.len() <= 1 {
                    return Vec::new(); // nothing left to shave => convergence
                }
                vec![lines[1..].join("\n")]
            }
        }
        let input = "a\nb\nc\nd"; // 4 tactic units
        let report = optimize(
            input,
            &Length,
            &ShaveRewriter,
            &*accept_all(),
            OptimizeConfig { rounds: 10, seed: 1 },
        );
        // Shaved down to a single tactic; refinement chained across rounds.
        assert_eq!(report.optimized, "d");
        assert_eq!(report.score_after, 1.0);
        assert!(report.rounds_run >= 3);
    }
}
