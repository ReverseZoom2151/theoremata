//! Gap-repair loop for nearly-complete proofs (Aristotle's "automatically repair
//! these gaps") + statement strengthening.
//!
//! Framing (Tao: "the proof contained some minor errors ... Aristotle was able to
//! automatically repair these gaps"): an assembled proof FAILS verification with a
//! *localizable* error. Rather than throw the whole proof away, we
//!
//! 1. **localize** the broken step from the verifier's error span,
//! 2. ask an injected [`Repairer`] for candidate fixes that target *that* step,
//! 3. **re-verify** each candidate and accept the first that passes, and
//! 4. iterate a bounded number of rounds — seeding the next round from the best
//!    still-failing candidate, so a proof broken in several places can be repaired
//!    one step per round.
//!
//! It also supports **strengthening** (NooneAtAll3's example: the model
//! strengthened a small-`C` bound to a large-`C` one on its own): given a proof of
//! a *weaker* statement, an injected [`Adapter`] proposes a modified proof
//! targeting the *stronger* statement, which is accepted only if the STRONG
//! statement's verifier passes.
//!
//! This module mirrors the injectable-seam style of [`crate::optimize`]: the two
//! model-facing seams ([`Repairer`] / [`Adapter`]) are traits and the correctness
//! gate is a `Box<dyn Fn(&str) -> VerifyOutcome>` verifier, so the whole loop runs
//! deterministically under test with mocks. Every seam is a pure function of its
//! inputs plus a threaded `seed` — no wall-clock, no unseeded randomness.
//!
//! **Safety invariant** (as in [`crate::optimize`] and [`crate::sketch`]): a proof
//! is only ever reported as `repaired` / `strengthened` once the verifier has
//! *accepted* it. A proof that still fails is NEVER returned as a success — the
//! report carries the best failing attempt plus the final error instead. All
//! candidate text is treated as UNTRUSTED DATA: it is only ever verifier-checked
//! and returned as text, never executed by this module.

// ---------------------------------------------------------------------------
// Verification outcome (the correctness gate's result)
// ---------------------------------------------------------------------------

/// Where in a proof a failure localizes. Line-indexed (0-based over the proof's
/// `lines()`), so the repair loop can point the repairer at the broken step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// 0-based line index of the failing step within the proof text.
    pub line: usize,
}

impl Span {
    /// A span at 0-based `line`.
    pub fn line(line: usize) -> Self {
        Self { line }
    }
}

/// A localizable verification failure: a human/model-readable `message` plus an
/// optional [`Span`] pinpointing the broken step. `span == None` is a failure the
/// verifier could not localize (the repairer must then reason from the message
/// and whole proof).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyError {
    pub message: String,
    pub span: Option<Span>,
}

impl VerifyError {
    pub fn new(message: impl Into<String>, span: Option<Span>) -> Self {
        Self {
            message: message.into(),
            span,
        }
    }
}

/// The result of running the correctness gate on a proof: either it passes, or it
/// fails with a localizable [`VerifyError`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyOutcome {
    Ok,
    Err(VerifyError),
}

impl VerifyOutcome {
    pub fn is_ok(&self) -> bool {
        matches!(self, VerifyOutcome::Ok)
    }

    /// The error, if this outcome is a failure.
    pub fn err(&self) -> Option<&VerifyError> {
        match self {
            VerifyOutcome::Ok => None,
            VerifyOutcome::Err(e) => Some(e),
        }
    }
}

/// The injected correctness gate. In production this is the formal verifier
/// (Lean/Rocq/…); in tests it is a deterministic mock. Type alias for the boxed
/// closure form the caller constructs; the loop functions accept it by `&dyn`
/// (pass `&*verifier`), exactly like [`crate::optimize::optimize`]'s `verify`.
pub type Verifier = Box<dyn Fn(&str) -> VerifyOutcome>;

// ---------------------------------------------------------------------------
// Repairer / Adapter seams
// ---------------------------------------------------------------------------

/// Proposes candidate fixes for a FAILING proof, targeting the localized failing
/// step (the model seam). In production this is an LLM handed the proof + error;
/// in tests a deterministic mock. MUST be a pure function of `(proof, error,
/// seed)` so runs are reproducible — no wall-clock / unseeded randomness.
///
/// Returned candidates are UNTRUSTED text: each is verifier-checked before it can
/// ever be accepted, and one is never executed by this module. May be empty (no
/// suggestion — the loop then stops making progress and reports failure).
pub trait Repairer {
    /// Candidate whole-proof rewrites that aim to fix the step `error` localizes.
    /// Order is a deterministic tie-break only; the verifier drives acceptance.
    fn repair(&self, proof: &str, error: &VerifyError, seed: u64) -> Vec<String>;
}

/// Proposes candidate proofs of a STRONGER statement, adapted from a proof of a
/// weaker one (the strengthening seam). In production an LLM; in tests a
/// deterministic mock. MUST be a pure function of its inputs plus `seed`.
///
/// Returned candidates are UNTRUSTED text, verifier-checked before acceptance.
pub trait Adapter {
    /// Candidate proofs targeting `strong_statement`, adapted from `weak_proof`
    /// (a proof of `weak_statement`).
    fn adapt(
        &self,
        weak_statement: &str,
        strong_statement: &str,
        weak_proof: &str,
        seed: u64,
    ) -> Vec<String>;
}

// ---------------------------------------------------------------------------
// Config + seeding
// ---------------------------------------------------------------------------

/// Knobs shared by [`repair_proof`] and [`strengthen_proof`]. `rounds` bounds the
/// number of propose→verify iterations; `seed` threads reproducible sampling into
/// the seam, combined with the round index so each round samples differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairConfig {
    /// Maximum propose→verify rounds. `0` = verify only, no repair attempt.
    pub rounds: usize,
    /// Base seed threaded to the [`Repairer`]/[`Adapter`], mixed with the round
    /// index so successive rounds sample differently but reproducibly.
    pub seed: u64,
}

impl Default for RepairConfig {
    fn default() -> Self {
        Self { rounds: 4, seed: 0 }
    }
}

/// Mix the base seed with the round index deterministically (splitmix64 finalizer)
/// so successive rounds sample differently but reproducibly. Mirrors the private
/// `round_seed` in [`crate::optimize`] (kept local — that one is not public).
fn round_seed(base: u64, round: usize) -> u64 {
    let mut z = base.wrapping_add((round as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

// ---------------------------------------------------------------------------
// Localization
// ---------------------------------------------------------------------------

/// The broken step a failure localizes to: its 0-based line index and the trimmed
/// step text. Recorded in the round trace for auditability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailingStep {
    pub line: usize,
    pub text: String,
}

/// Localize the failing step of `proof` from `error`'s span. `None` when the error
/// carries no span, or the span points past the end of the proof — the repairer
/// then reasons from the whole proof and message alone.
pub fn localize_failing_step(proof: &str, error: &VerifyError) -> Option<FailingStep> {
    let span = error.span.as_ref()?;
    let text = proof.lines().nth(span.line)?;
    Some(FailingStep {
        line: span.line,
        text: text.trim().to_string(),
    })
}

// ---------------------------------------------------------------------------
// Repair report + loop
// ---------------------------------------------------------------------------

/// One propose→verify round of [`repair_proof`], for the audit trace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairRound {
    /// The seed handed to the repairer this round (`round_seed(base, round)`).
    pub seed: u64,
    /// The step this round targeted (from the current error's span), if localized.
    pub failing_step: Option<FailingStep>,
    /// How many candidates the repairer proposed this round.
    pub candidates_seen: usize,
    /// Whether a candidate PASSED the verifier this round (loop then stops).
    pub accepted: bool,
}

/// The outcome of [`repair_proof`].
///
/// Invariant: `repaired` is `Some` iff a verifier-passing proof was found; it is
/// NEVER a proof that fails the verifier. On failure, `repaired == None`,
/// `final_error` carries the last verifier error, and `best_attempt` is the
/// closest (last-advanced) still-failing proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairReport {
    /// The original (failing, or already-passing) input proof.
    pub original: String,
    /// The repaired, verifier-passing proof — `Some` iff repair succeeded.
    pub repaired: Option<String>,
    /// The final verifier error — `None` iff repair succeeded.
    pub final_error: Option<VerifyError>,
    /// The closest attempt reached: the accepted proof on success, else the last
    /// still-failing proof carried forward (equals `original` if none advanced).
    pub best_attempt: String,
    /// Per-round audit trace, in order.
    pub rounds: Vec<RepairRound>,
}

impl RepairReport {
    /// Whether the proof was repaired to a verifier-passing state.
    pub fn succeeded(&self) -> bool {
        self.repaired.is_some()
    }

    /// How many propose→verify rounds actually ran.
    pub fn rounds_run(&self) -> usize {
        self.rounds.len()
    }
}

/// Repair a proof that fails verification: localize the broken step, ask the
/// [`Repairer`] for candidates, verify each, accept the first that passes, and
/// iterate up to `config.rounds`. Between rounds the loop advances to the first
/// still-failing candidate that *changed* the proof (making progress on a
/// different step), so a proof broken in several places is repaired one step per
/// round. It stops early when a round proposes no change (convergence).
///
/// * `verify`   — the correctness gate (injected; deterministic in tests). Pass
///   the boxed [`Verifier`] as `&*verifier`, as with [`crate::optimize`].
/// * `repairer` — proposes step-targeted fixes (injected; deterministic in tests).
///
/// If the input already passes, returns success with `repaired == input` and no
/// rounds. NEVER returns a proof that fails `verify` as `repaired`.
pub fn repair_proof(
    proof: &str,
    verify: &dyn Fn(&str) -> VerifyOutcome,
    repairer: &dyn Repairer,
    config: RepairConfig,
) -> RepairReport {
    let mut rounds = Vec::new();

    // Initial verification: an already-passing proof needs no repair.
    let mut current = proof.to_string();
    let mut current_error = match verify(&current) {
        VerifyOutcome::Ok => {
            return RepairReport {
                original: proof.to_string(),
                repaired: Some(current.clone()),
                final_error: None,
                best_attempt: current,
                rounds,
            };
        }
        VerifyOutcome::Err(e) => e,
    };

    for round in 0..config.rounds {
        let seed = round_seed(config.seed, round);
        let failing_step = localize_failing_step(&current, &current_error);
        // Hand the repairer a span-marked view of the error (Kimina-style: the
        // offending line marked with context), without mutating `current_error`
        // so `rounds`/`final_error` stay canonical.
        let marked = crate::prover::statement_preservation::format_error_spans(
            &current,
            &[crate::prover::statement_preservation::LeanError::new(
                current_error.span.as_ref().map(|s| s.line + 1).unwrap_or(0),
                current_error.message.clone(),
            )],
        );
        let error_for_repair = VerifyError::new(marked, current_error.span.clone());
        let candidates = repairer.repair(&current, &error_for_repair, seed);

        // Single verify per candidate: take the first that passes; otherwise
        // remember the first that at least *changed* the proof (to advance).
        let mut passing: Option<String> = None;
        let mut progress: Option<(String, VerifyError)> = None;
        for cand in &candidates {
            match verify(cand) {
                VerifyOutcome::Ok => {
                    passing = Some(cand.clone());
                    break;
                }
                VerifyOutcome::Err(e) => {
                    if progress.is_none() && *cand != current {
                        progress = Some((cand.clone(), e));
                    }
                }
            }
        }

        rounds.push(RepairRound {
            seed,
            failing_step,
            candidates_seen: candidates.len(),
            accepted: passing.is_some(),
        });

        if let Some(p) = passing {
            // Safety invariant: only report a proof the verifier accepted.
            debug_assert!(verify(&p).is_ok(), "accepted proof must pass the verifier");
            return RepairReport {
                original: proof.to_string(),
                repaired: Some(p.clone()),
                final_error: None,
                best_attempt: p,
                rounds,
            };
        }

        // No pass this round: advance to the first still-failing candidate that
        // changed something, to target the next broken step next round. If none
        // changed, the repairer has converged — stop early (bounded anyway).
        match progress {
            Some((cand, err)) => {
                current = cand;
                current_error = err;
            }
            None => break,
        }
    }

    RepairReport {
        original: proof.to_string(),
        repaired: None,
        final_error: Some(current_error),
        best_attempt: current,
        rounds,
    }
}

// ---------------------------------------------------------------------------
// Strengthen report + loop
// ---------------------------------------------------------------------------

/// The outcome of [`strengthen_proof`].
///
/// Invariant: `strengthened` is `Some` iff the candidate PASSED the STRONG
/// statement's verifier; it is never a proof that fails. On failure it is `None`
/// and `final_error` carries the last verifier error (if any candidate was tried).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrengthenReport {
    pub weak_statement: String,
    pub strong_statement: String,
    pub weak_proof: String,
    /// A verified proof of the STRONG statement — `Some` iff strengthening won.
    pub strengthened: Option<String>,
    /// The last verifier error — `None` on success or when no candidate was tried.
    pub final_error: Option<VerifyError>,
    /// Total candidates the adapter proposed across all rounds.
    pub candidates_seen: usize,
    /// How many propose→verify rounds actually ran.
    pub rounds_run: usize,
}

impl StrengthenReport {
    pub fn succeeded(&self) -> bool {
        self.strengthened.is_some()
    }
}

/// Attempt to strengthen a proof of `weak_statement` into a proof of
/// `strong_statement`: each round the injected [`Adapter`] proposes candidate
/// proofs targeting the stronger statement, and each is checked against `verify`
/// — the STRONG statement's verifier. Returns success only when a candidate
/// PASSES the strong verifier; fails cleanly (no false success) otherwise.
///
/// Bounded by `config.rounds`; stops early if a round proposes nothing.
pub fn strengthen_proof(
    weak_statement: &str,
    strong_statement: &str,
    weak_proof: &str,
    adapter: &dyn Adapter,
    verify: &dyn Fn(&str) -> VerifyOutcome,
    config: RepairConfig,
) -> StrengthenReport {
    let mut candidates_seen = 0usize;
    let mut rounds_run = 0usize;
    let mut final_error: Option<VerifyError> = None;

    for round in 0..config.rounds {
        rounds_run += 1;
        let seed = round_seed(config.seed, round);
        let candidates = adapter.adapt(weak_statement, strong_statement, weak_proof, seed);
        candidates_seen += candidates.len();

        for cand in &candidates {
            match verify(cand) {
                VerifyOutcome::Ok => {
                    // Success only if the STRONG statement's proof passes.
                    debug_assert!(verify(cand).is_ok());
                    return StrengthenReport {
                        weak_statement: weak_statement.to_string(),
                        strong_statement: strong_statement.to_string(),
                        weak_proof: weak_proof.to_string(),
                        strengthened: Some(cand.clone()),
                        final_error: None,
                        candidates_seen,
                        rounds_run,
                    };
                }
                VerifyOutcome::Err(e) => final_error = Some(e),
            }
        }

        // Nothing proposed this round: the adapter has converged — stop early.
        if candidates.is_empty() {
            break;
        }
    }

    StrengthenReport {
        weak_statement: weak_statement.to_string(),
        strong_statement: strong_statement.to_string(),
        weak_proof: weak_proof.to_string(),
        strengthened: None,
        final_error,
        candidates_seen,
        rounds_run,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Verifier mocks ------------------------------------------------------

    /// A verifier that FAILS iff any line (trimmed) equals `"BROKEN"`, localizing
    /// to the first such line. Passes once every `BROKEN` line is gone.
    fn broken_line_verifier() -> impl Fn(&str) -> VerifyOutcome {
        |proof: &str| {
            for (i, line) in proof.lines().enumerate() {
                if line.trim() == "BROKEN" {
                    return VerifyOutcome::Err(VerifyError::new(
                        format!("unsolved goal at line {i}"),
                        Some(Span::line(i)),
                    ));
                }
            }
            VerifyOutcome::Ok
        }
    }

    /// A verifier that NEVER passes (localizes to line 0) — for bounded-rounds.
    fn always_fail_verifier() -> impl Fn(&str) -> VerifyOutcome {
        |_: &str| VerifyOutcome::Err(VerifyError::new("never ok", Some(Span::line(0))))
    }

    // -- Repairer mocks ------------------------------------------------------

    /// Replaces the failing line (from the error span) with `"fixed"`.
    struct StepRepairer;
    impl Repairer for StepRepairer {
        fn repair(&self, proof: &str, error: &VerifyError, _seed: u64) -> Vec<String> {
            let Some(span) = &error.span else {
                return Vec::new();
            };
            let mut lines: Vec<String> = proof.lines().map(|s| s.to_string()).collect();
            if span.line >= lines.len() {
                return Vec::new();
            }
            lines[span.line] = "fixed".to_string();
            vec![lines.join("\n")]
        }
    }

    /// Proposes only the unchanged proof — never makes progress (convergence).
    struct NoProgressRepairer;
    impl Repairer for NoProgressRepairer {
        fn repair(&self, proof: &str, _error: &VerifyError, _seed: u64) -> Vec<String> {
            vec![proof.to_string()]
        }
    }

    /// Always proposes a changed-but-still-failing candidate (grows the proof), so
    /// the loop advances every round without ever passing — for bounded-rounds.
    struct ChurnRepairer;
    impl Repairer for ChurnRepairer {
        fn repair(&self, proof: &str, _error: &VerifyError, seed: u64) -> Vec<String> {
            vec![format!("{proof}\n-- churn {seed}")]
        }
    }

    // -- repair_proof: happy path -------------------------------------------

    #[test]
    fn one_step_error_is_repaired_and_reverifies() {
        let proof = "intro x\nBROKEN\nexact h";
        let verify = broken_line_verifier();
        let report = repair_proof(proof, &verify, &StepRepairer, RepairConfig::default());

        assert!(report.succeeded());
        let repaired = report.repaired.as_ref().unwrap();
        assert_eq!(repaired, "intro x\nfixed\nexact h");
        // The reported proof genuinely passes the verifier.
        assert!(verify(repaired).is_ok());
        assert!(report.final_error.is_none());
        // Localized the broken step (line 1) in the trace.
        assert_eq!(report.rounds.len(), 1);
        assert!(report.rounds[0].accepted);
        let step = report.rounds[0].failing_step.as_ref().unwrap();
        assert_eq!(step.line, 1);
        assert_eq!(step.text, "BROKEN");
    }

    #[test]
    fn already_passing_proof_needs_no_repair() {
        let proof = "intro x\nexact h";
        let verify = broken_line_verifier();
        let report = repair_proof(proof, &verify, &StepRepairer, RepairConfig::default());
        assert!(report.succeeded());
        assert_eq!(report.repaired.as_deref(), Some(proof));
        assert!(report.rounds.is_empty()); // verified up-front, no rounds
    }

    #[test]
    fn multi_step_break_is_repaired_one_step_per_round() {
        // Two broken lines: round 1 fixes line 0 (still fails on line 1), the loop
        // advances, round 2 fixes line 1 and passes.
        let proof = "BROKEN\nBROKEN";
        let verify = broken_line_verifier();
        let report = repair_proof(proof, &verify, &StepRepairer, RepairConfig::default());

        assert!(report.succeeded());
        assert_eq!(report.repaired.as_deref(), Some("fixed\nfixed"));
        assert_eq!(report.rounds.len(), 2);
        assert!(!report.rounds[0].accepted);
        assert!(report.rounds[1].accepted);
        // Round 2 targeted line 1 (the second break, after line 0 was fixed).
        assert_eq!(report.rounds[1].failing_step.as_ref().unwrap().line, 1);
    }

    // -- repair_proof: failure never masquerades as success -----------------

    #[test]
    fn unrepairable_proof_returns_failure_with_final_error_not_false_repaired() {
        let proof = "BROKEN";
        let verify = broken_line_verifier();
        // Repairer keeps proposing the identical broken proof: no progress.
        let report = repair_proof(proof, &verify, &NoProgressRepairer, RepairConfig::default());

        assert!(!report.succeeded());
        assert!(report.repaired.is_none());
        let err = report.final_error.as_ref().unwrap();
        assert_eq!(err.span, Some(Span::line(0)));
        // Best attempt is the (still-failing) original — and it genuinely fails.
        assert_eq!(report.best_attempt, proof);
        assert!(!verify(&report.best_attempt).is_ok());
        // Converged after one no-progress round.
        assert_eq!(report.rounds.len(), 1);
        assert!(!report.rounds[0].accepted);
    }

    #[test]
    fn bounded_rounds_are_respected() {
        let proof = "start";
        let verify = always_fail_verifier();
        let cfg = RepairConfig { rounds: 3, seed: 7 };
        let report = repair_proof(proof, &verify, &ChurnRepairer, cfg);

        assert!(!report.succeeded());
        // Exactly `rounds` propose→verify iterations ran (churn always advances).
        assert_eq!(report.rounds.len(), 3);
        assert_eq!(report.rounds_run(), 3);
        assert!(report.final_error.is_some());
    }

    #[test]
    fn zero_rounds_makes_no_repair_attempt() {
        let proof = "BROKEN";
        let verify = broken_line_verifier();
        let cfg = RepairConfig { rounds: 0, seed: 0 };
        let report = repair_proof(proof, &verify, &StepRepairer, cfg);
        assert!(!report.succeeded());
        assert!(report.rounds.is_empty());
        assert_eq!(report.best_attempt, proof);
    }

    // -- repair_proof: seed threading + determinism -------------------------

    /// Fixes the failing line only when the seed's low bit is 0; otherwise returns
    /// a distinct still-failing candidate. Proves the seed reaches the repairer.
    struct SeedSensitiveRepairer;
    impl Repairer for SeedSensitiveRepairer {
        fn repair(&self, proof: &str, error: &VerifyError, seed: u64) -> Vec<String> {
            let Some(span) = &error.span else {
                return Vec::new();
            };
            let mut lines: Vec<String> = proof.lines().map(|s| s.to_string()).collect();
            if span.line >= lines.len() {
                return Vec::new();
            }
            if seed & 1 == 0 {
                lines[span.line] = "fixed".to_string();
            } else {
                lines[span.line] = "BROKEN".to_string(); // unchanged => no progress
            }
            vec![lines.join("\n")]
        }
    }

    #[test]
    fn result_is_deterministic_given_seed() {
        let proof = "a\nBROKEN\nb";
        let verify = broken_line_verifier();
        let cfg = RepairConfig { rounds: 3, seed: 42 };
        let a = repair_proof(proof, &verify, &SeedSensitiveRepairer, cfg);
        let b = repair_proof(proof, &verify, &SeedSensitiveRepairer, cfg);
        assert_eq!(a, b);
    }

    #[test]
    fn seed_is_threaded_into_the_repairer() {
        let proof = "a\nBROKEN\nb";
        let verify = broken_line_verifier();
        // round_seed(base,0) low bit differs for these two bases, flipping the
        // seed-sensitive repairer between "fix" and "no-op" on round 0.
        let even = round_seed(2, 0) & 1;
        let odd = round_seed(1, 0) & 1;
        assert_ne!(even, odd, "test bases must straddle the low-bit branch");
        let fixing_base = if even == 0 { 2 } else { 1 };
        let stuck_base = if even == 0 { 1 } else { 2 };

        let good = repair_proof(
            proof,
            &verify,
            &SeedSensitiveRepairer,
            RepairConfig { rounds: 1, seed: fixing_base },
        );
        let stuck = repair_proof(
            proof,
            &verify,
            &SeedSensitiveRepairer,
            RepairConfig { rounds: 1, seed: stuck_base },
        );
        assert!(good.succeeded());
        assert!(!stuck.succeeded());
    }

    // -- localization helper -------------------------------------------------

    #[test]
    fn localize_extracts_the_failing_line_or_none() {
        let proof = "line0\n  line1  \nline2";
        let err = VerifyError::new("x", Some(Span::line(1)));
        let step = localize_failing_step(proof, &err).unwrap();
        assert_eq!(step.line, 1);
        assert_eq!(step.text, "line1"); // trimmed

        // No span => not localizable.
        assert!(localize_failing_step(proof, &VerifyError::new("x", None)).is_none());
        // Span past end => not localizable.
        assert!(localize_failing_step(proof, &VerifyError::new("x", Some(Span::line(9)))).is_none());
    }

    // -- Adapter mocks + strengthen_proof -----------------------------------

    /// The STRONG statement's verifier: a proof passes iff it contains "STRONG".
    fn strong_verifier() -> impl Fn(&str) -> VerifyOutcome {
        |proof: &str| {
            if proof.contains("STRONG") {
                VerifyOutcome::Ok
            } else {
                VerifyOutcome::Err(VerifyError::new("does not prove the strong statement", None))
            }
        }
    }

    /// Adapts by appending a marker that proves the strong statement.
    struct StrengtheningAdapter;
    impl Adapter for StrengtheningAdapter {
        fn adapt(&self, _weak: &str, _strong: &str, weak_proof: &str, _seed: u64) -> Vec<String> {
            vec![format!("{weak_proof}\n-- generalize C, now STRONG")]
        }
    }

    /// Adapts but never actually reaches the strong statement.
    struct WeakAdapter;
    impl Adapter for WeakAdapter {
        fn adapt(&self, _weak: &str, _strong: &str, weak_proof: &str, _seed: u64) -> Vec<String> {
            vec![format!("{weak_proof}\n-- still only weak")]
        }
    }

    #[test]
    fn strengthen_succeeds_when_adapter_candidate_passes_strong_verifier() {
        let verify = strong_verifier();
        let report = strengthen_proof(
            "C small",
            "C large",
            "by bound with small C",
            &StrengtheningAdapter,
            &verify,
            RepairConfig::default(),
        );
        assert!(report.succeeded());
        let strengthened = report.strengthened.as_ref().unwrap();
        assert!(verify(strengthened).is_ok());
        assert!(strengthened.contains("STRONG"));
        assert!(report.final_error.is_none());
    }

    #[test]
    fn strengthen_fails_cleanly_when_no_candidate_proves_the_strong_statement() {
        let verify = strong_verifier();
        let report = strengthen_proof(
            "C small",
            "C large",
            "by bound with small C",
            &WeakAdapter,
            &verify,
            RepairConfig::default(),
        );
        // Never a false success: the strong statement was never proven.
        assert!(!report.succeeded());
        assert!(report.strengthened.is_none());
        assert!(report.final_error.is_some());
        assert!(report.candidates_seen >= 1);
    }

    #[test]
    fn strengthen_stops_early_when_adapter_proposes_nothing() {
        struct EmptyAdapter;
        impl Adapter for EmptyAdapter {
            fn adapt(&self, _w: &str, _s: &str, _p: &str, _seed: u64) -> Vec<String> {
                Vec::new()
            }
        }
        let verify = strong_verifier();
        let report = strengthen_proof(
            "w",
            "s",
            "p",
            &EmptyAdapter,
            &verify,
            RepairConfig { rounds: 5, seed: 0 },
        );
        assert!(!report.succeeded());
        assert_eq!(report.rounds_run, 1); // stopped after the first empty round
        assert_eq!(report.candidates_seen, 0);
    }

    #[test]
    fn strengthen_is_deterministic_given_seed() {
        let verify = strong_verifier();
        let cfg = RepairConfig { rounds: 3, seed: 99 };
        let a = strengthen_proof("w", "s", "p", &StrengtheningAdapter, &verify, cfg);
        let b = strengthen_proof("w", "s", "p", &StrengtheningAdapter, &verify, cfg);
        assert_eq!(a, b);
    }
}
