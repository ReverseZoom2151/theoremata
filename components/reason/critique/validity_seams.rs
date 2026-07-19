//! Production adapters for the three [`crate::statement_validity`] seams.
//!
//! [`crate::statement_validity`] is deliberately pure: it performs no model
//! calls, no process spawning and no IO, and every capability it needs arrives
//! as an injected trait object. That purity is why, wired into the agent loop
//! with nothing attached, every check reports
//! [`CheckStatus::Skipped`](crate::statement_validity::CheckStatus::Skipped).
//! This module supplies the missing implementations — and nothing else. Each
//! type here is a THIN WRAPPER over a handle the caller owns:
//!
//! | Seam | Adapter | Wraps |
//! |---|---|---|
//! | [`StatementJudge`] | [`ModelJudge`] | `&dyn ModelProvider` |
//! | [`NegationProver`] | [`FalsifierNegation`] | the existing [`crate::falsification::Falsifier`] |
//! | [`TrivialProver`] | [`CheapTrivialProver`] | a [`CheapProofAttempt`] (e.g. [`BackendCheapProof`]) |
//!
//! No global state, no statics, no clock, no RNG lives here. Nothing in this
//! file spawns a process on its own account — process work happens only inside
//! the wrapped component (the provider command, the falsifier's Python worker,
//! the formal backend's compiler), exactly as it would if the caller invoked
//! that component directly.
//!
//! # ⚠️ POLARITY — READ THIS BEFORE TOUCHING [`FalsifierNegation`] OR [`CheapTrivialProver`]
//!
//! For the negation and triviality seams, [`ProofOutcome::Proved`] means the
//! statement is **BAD**. This is inverted relative to every other "proved" in
//! the codebase and it is the single easiest thing here to get backwards.
//! Getting it backwards does not fail loudly — it silently REJECTS EVERY GOOD
//! STATEMENT and ACCEPTS EVERY BROKEN ONE.
//!
//! * [`FalsifierNegation`]: a found counterexample is evidence that the
//!   statement is FALSE, i.e. evidence FOR its negation ⇒ `Proved` ⇒ the
//!   negation check FAILS the candidate ⇒ the formalization is broken.
//! * [`CheapTrivialProver`]: a two-tactic proof closing the goal means the
//!   statement is DEGENERATE ⇒ `Proved` ⇒ the triviality check FAILS the
//!   candidate.
//!
//! In both cases `NotProved` (the attempt genuinely ran and found nothing) is
//! the GOOD outcome that passes the check.
//!
//! # Errors never pass
//!
//! Every adapter maps failure of its wrapped handle to a NON-passing answer:
//!
//! * [`ModelJudge`] maps any provider error, any non-`true` `faithful` field and
//!   any unparseable response to a DISSENTING [`JudgeVerdict`]. One dissent
//!   breaks unanimity, so a broken judge can only ever cause a `Reject`, never
//!   an `Accept`.
//! * [`FalsifierNegation`] and [`CheapTrivialProver`] map errors to
//!   [`ProofOutcome::Inconclusive`], which the checks report as `Skipped` —
//!   never `Passed`.
//!
//! Note the asymmetry: the judge fails to a check-FAILURE while the two prover
//! seams fail to a check-SKIP. That is intentional and follows the polarity. For
//! the judge, "no evidence of faithfulness" is precisely what a dissent encodes.
//! For the provers, an error is not evidence that the negation is unprovable nor
//! that the statement is non-trivial, so it must not be reported as `NotProved`
//! (which would PASS the check on the strength of a crash).

use crate::{
    config::Config,
    falsification::{FalsifyVerdict, Falsifier},
    model::ModelRequest,
    prover::formal::FormalBackend,
    provider::ModelProvider,
    statement_validity::{JudgeVerdict, NegationProver, ProofOutcome, StatementJudge, TrivialProver},
};
use anyhow::Result;
use serde_json::json;

// ---------------------------------------------------------------------------
// 1. The judge seam
// ---------------------------------------------------------------------------

/// The model role used for judge requests, so routing/telemetry can see them.
pub const JUDGE_ROLE: &str = "statement_validity_judge";

/// [`StatementJudge`] backed by a [`ModelProvider`].
///
/// Asks the model whether `formal` faithfully encodes `informal` — a
/// FAITHFULNESS question, not a truth question. The model is explicitly told
/// that a true-but-different statement is UNFAITHFUL.
///
/// # `sample_idx` genuinely varies the request
///
/// [`crate::statement_validity::UnanimityCheck`] draws `N` samples and requires
/// unanimity, which is only meaningful if the samples are INDEPENDENT. If every
/// sample sent a byte-identical request, a caching or deterministic provider
/// would return `N` copies of one opinion and "unanimity" would be a tautology.
/// So `sample_idx` is threaded into the request in two places: the `context` (as
/// `sample_idx` and `seed`) and the `task` text. Distinct indices therefore
/// produce distinct request payloads and distinct cache keys.
///
/// This does not by itself guarantee statistical independence — that depends on
/// the provider honouring the seam (see the unverified-assumptions note in the
/// adapter's docs). It guarantees only that the seam has done everything it can
/// on this side of the boundary.
pub struct ModelJudge<'a> {
    provider: &'a dyn ModelProvider,
}

impl<'a> ModelJudge<'a> {
    pub fn new(provider: &'a dyn ModelProvider) -> Self {
        Self { provider }
    }

    /// The request for one sample. Separated out so tests can assert that two
    /// sample indices produce genuinely different payloads.
    pub fn request(&self, informal: &str, formal: &str, sample_idx: u64) -> ModelRequest {
        ModelRequest {
            role: JUDGE_ROLE.into(),
            task: format!(
                "Independent judge sample #{sample_idx}. Decide whether the FORMAL statement \
                 faithfully encodes the INFORMAL one. Faithful means same quantifier order, same \
                 hypotheses (none added, none dropped, none weakened), same conclusion, same \
                 types and bounds. A statement that is TRUE but says something different is NOT \
                 faithful. A statement whose hypotheses are unsatisfiable is NOT faithful. Judge \
                 this sample on its own; do not assume any previous sample's answer. Answer with \
                 `faithful` (boolean) and a one-line `reason`."
            ),
            context: json!({
                "informal": informal,
                "formal": formal,
                // Threaded so the seam owns sampling/seeding; the pure module
                // holds no RNG and supplies only a counter.
                "sample_idx": sample_idx,
                "seed": sample_idx,
            }),
            output_schema: json!({
                "type": "object",
                "required": ["faithful", "reason"],
                "properties": {
                    "faithful": {"type": "boolean"},
                    "reason": {"type": "string"}
                }
            }),
        }
    }
}

impl StatementJudge for ModelJudge<'_> {
    /// FAIL-CLOSED: only an explicit `faithful: true` is a faithful vote. A
    /// provider error, a missing field, a non-boolean field or a garbled
    /// response all become a DISSENT, which breaks unanimity. A broken judge can
    /// never manufacture an `Accept`.
    fn judge(&self, informal: &str, formal: &str, sample_idx: u64) -> JudgeVerdict {
        let request = self.request(informal, formal, sample_idx);
        let response = match self.provider.complete(&request) {
            Ok(response) => response,
            Err(error) => {
                return JudgeVerdict::unfaithful(format!(
                    "sample {sample_idx}: judge provider error, counted as DISSENT (an error is \
                     never a pass): {error}"
                ));
            }
        };
        let reason = response.content["reason"]
            .as_str()
            .unwrap_or("(no reason given)")
            .to_string();
        match response.content["faithful"].as_bool() {
            Some(true) => JudgeVerdict::faithful(format!("sample {sample_idx}: {reason}")),
            Some(false) => JudgeVerdict::unfaithful(format!("sample {sample_idx}: {reason}")),
            None => JudgeVerdict::unfaithful(format!(
                "sample {sample_idx}: judge response had no boolean `faithful` field, counted as \
                 DISSENT (an unreadable answer is never a pass)"
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// 2. The negation seam
// ---------------------------------------------------------------------------

/// The falsification capability [`FalsifierNegation`] consumes.
///
/// This exists so the adapter depends on the *behaviour* of
/// [`crate::falsification::Falsifier`] rather than on its construction, which
/// keeps the adapter unit-testable with canned verdicts (the real falsifier
/// needs a live model provider AND the Python worker to produce a
/// counterexample). [`Falsifier`] implements it by direct delegation — this
/// REUSES the existing falsifier, it does not reimplement one.
pub trait Falsify {
    fn falsify(&self, statement: &str) -> Result<FalsifyVerdict>;
}

impl Falsify for Falsifier<'_> {
    fn falsify(&self, statement: &str) -> Result<FalsifyVerdict> {
        Falsifier::falsify(self, statement)
    }
}

/// The falsifier verdict string that means a refuting assignment was actually
/// found. Every other string — including `no_counterexample_in_domain` — is a
/// non-result for our purposes.
const COUNTEREXAMPLE: &str = "counterexample";

/// Which text the falsifier is pointed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FalsifyTarget {
    /// Falsify the FORMAL statement. The default: the formal statement is the
    /// artifact being screened, and a counterexample to *it* is what condemns
    /// the formalization.
    Formal,
    /// Falsify the INFORMAL statement. Useful when the formal syntax is opaque
    /// to the falsifier's spec-generation step; note this then screens the
    /// SOURCE claim rather than its encoding.
    Informal,
}

/// [`NegationProver`] backed by the existing model-derived falsifier.
///
/// # ⚠️ POLARITY (the whole point of this adapter)
///
/// The seam is called `prove_negation`. A counterexample to `S` IS evidence for
/// `¬S`. So:
///
/// * falsifier found a `counterexample` ⇒ [`ProofOutcome::Proved`]
///   ⇒ the negation check FAILS the candidate ⇒ the formalization is broken.
/// * **everything else** ⇒ [`ProofOutcome::Inconclusive`].
///
/// "Everything else" deliberately includes `no_counterexample_in_domain`, and
/// this is the subtle part. A bounded numeric sweep that found nothing is NOT a
/// proof that `¬S` is unprovable — it searched a finite box. Returning
/// [`ProofOutcome::NotProved`] there would make the negation check report
/// `Passed`, i.e. it would launder "we looked in a small box and saw nothing"
/// into "this statement is sound". A numeric screen never proves anything (see
/// [`crate::falsification`]'s own module note), so it may only ever produce the
/// positive signal, never the negative one. `NotProved` is therefore
/// UNREACHABLE from this adapter, by design.
///
/// Errors likewise map to `Inconclusive`, never `NotProved`.
pub struct FalsifierNegation<'a> {
    falsifier: &'a dyn Falsify,
    target: FalsifyTarget,
}

impl<'a> FalsifierNegation<'a> {
    /// Wrap a falsifier, screening the FORMAL statement.
    pub fn new(falsifier: &'a dyn Falsify) -> Self {
        Self {
            falsifier,
            target: FalsifyTarget::Formal,
        }
    }

    /// Point the falsifier at the informal statement instead.
    pub fn on_informal(mut self) -> Self {
        self.target = FalsifyTarget::Informal;
        self
    }

    /// The configured target.
    pub fn target(&self) -> FalsifyTarget {
        self.target
    }
}

impl NegationProver for FalsifierNegation<'_> {
    fn prove_negation(&self, informal: &str, formal: &str) -> ProofOutcome {
        let statement = match self.target {
            FalsifyTarget::Formal => formal,
            FalsifyTarget::Informal => informal,
        };
        match self.falsifier.falsify(statement) {
            // A refuting assignment exists ⇒ the statement is false ⇒ its
            // NEGATION has evidence ⇒ Proved ⇒ the check FAILS the candidate.
            Ok(verdict) if verdict.verdict == COUNTEREXAMPLE => ProofOutcome::Proved,
            // not_applicable / no_counterexample_in_domain / inconclusive /
            // no_model / unavailable / error — none of these establish that the
            // negation is UNprovable. Never NotProved.
            Ok(_) => ProofOutcome::Inconclusive,
            Err(_) => ProofOutcome::Inconclusive,
        }
    }
}

// ---------------------------------------------------------------------------
// 3. The triviality seam
// ---------------------------------------------------------------------------

/// One cheap, short-budget proof attempt.
///
/// `Ok(true)` = this tactic closed the goal. `Ok(false)` = it ran and did not.
/// `Err` = the attempt could not be made at all (toolchain missing, timeout,
/// scaffold failure) and carries NO information either way.
pub trait CheapProofAttempt {
    fn try_close(&self, formal: &str, tactic: &str) -> Result<bool>;
}

/// A short, deliberately weak tactic budget. If one of these closes a statement
/// we intended to be substantive, the statement lost its content in
/// formalization. Kept tiny on purpose: this is a triviality probe, not a proof
/// search, and a strong budget here would start rejecting genuinely provable
/// (but interesting) statements.
pub const DEFAULT_TRIVIAL_TACTICS: &[&str] = &["rfl", "trivial", "simp", "decide"];

/// [`TrivialProver`] backed by an injected cheap-proof attempt.
///
/// # ⚠️ POLARITY
///
/// [`ProofOutcome::Proved`] means the statement is **BAD** (degenerate), not
/// good. A cheap proof indicates vacuous hypotheses — an unsatisfiable premise
/// proves anything — or a trivially-true conclusion. Reading `Proved` as "this
/// statement is fine" would invert the entire triviality check.
///
/// # Outcome mapping
///
/// * any tactic closes the goal ⇒ `Proved` (degenerate; check FAILS it)
/// * every tactic ran and none closed ⇒ `NotProved` (has content; check PASSES)
/// * any tactic ERRORED ⇒ `Inconclusive`, immediately.
///
/// The error case short-circuits and does not fall back to `NotProved` on the
/// tactics that did run: if the toolchain is broken, "no cheap proof was found"
/// is an artifact of the breakage, not evidence of non-triviality. Reporting
/// `NotProved` there would PASS the check on the strength of a crash.
pub struct CheapTrivialProver<'a> {
    attempt: &'a dyn CheapProofAttempt,
    tactics: Vec<String>,
}

impl<'a> CheapTrivialProver<'a> {
    /// Wrap an attempt with [`DEFAULT_TRIVIAL_TACTICS`].
    pub fn new(attempt: &'a dyn CheapProofAttempt) -> Self {
        Self {
            attempt,
            tactics: DEFAULT_TRIVIAL_TACTICS
                .iter()
                .map(|t| (*t).to_string())
                .collect(),
        }
    }

    /// Wrap an attempt with an explicit tactic budget. An EMPTY budget makes
    /// every probe [`ProofOutcome::Inconclusive`] rather than a vacuous
    /// `NotProved`: probing nothing establishes nothing.
    pub fn with_tactics(attempt: &'a dyn CheapProofAttempt, tactics: Vec<String>) -> Self {
        Self { attempt, tactics }
    }

    /// The configured budget.
    pub fn tactics(&self) -> &[String] {
        &self.tactics
    }
}

impl TrivialProver for CheapTrivialProver<'_> {
    fn prove_trivially(&self, _informal: &str, formal: &str) -> ProofOutcome {
        if self.tactics.is_empty() {
            // No probe was made, so nothing was established. NOT a pass.
            return ProofOutcome::Inconclusive;
        }
        for tactic in &self.tactics {
            match self.attempt.try_close(formal, tactic) {
                // Closed by a one-liner ⇒ DEGENERATE ⇒ the check FAILS it.
                Ok(true) => return ProofOutcome::Proved,
                Ok(false) => continue,
                // The probe could not be run; a failed probe is not evidence of
                // non-triviality.
                Err(_) => return ProofOutcome::Inconclusive,
            }
        }
        ProofOutcome::NotProved
    }
}

/// A [`CheapProofAttempt`] backed by a real [`FormalBackend`].
///
/// Builds `<statement> := by <tactic>` and runs the backend's own verification
/// path. Purely a wrapper: all compiling/spawning is the backend's, unchanged.
///
/// # A MOCK backend is always inconclusive
///
/// If the backend is a mock ([`FormalBackend::is_mock`]) or the report is not
/// `live`, this returns an error, which [`CheapTrivialProver`] maps to
/// `Inconclusive`. That is the safe direction *given the inverted polarity*: a
/// mock backend's canned "verified" would otherwise read as `Proved` and REJECT
/// a perfectly good statement as degenerate.
pub struct BackendCheapProof<'a> {
    backend: &'a dyn FormalBackend,
    config: &'a Config,
}

impl<'a> BackendCheapProof<'a> {
    pub fn new(backend: &'a dyn FormalBackend, config: &'a Config) -> Self {
        Self { backend, config }
    }
}

/// Splice a one-tactic proof onto a statement: everything up to the first `:=`
/// is kept as the statement head, and `:= by <tactic>` replaces any body.
///
/// Exposed for testing; it is plain string surgery with no IO.
pub fn with_tactic_body(formal: &str, tactic: &str) -> String {
    let head = match formal.find(":=") {
        Some(idx) => &formal[..idx],
        None => formal,
    };
    format!("{} := by {}", head.trim_end(), tactic)
}

impl CheapProofAttempt for BackendCheapProof<'_> {
    fn try_close(&self, formal: &str, tactic: &str) -> Result<bool> {
        if self.backend.is_mock() {
            anyhow::bail!(
                "cheap-proof probe declined: backend is a MOCK, and a canned 'proved' would \
                 wrongly mark this statement degenerate"
            );
        }
        let code = with_tactic_body(formal, tactic);
        let report = self.backend.verify(self.config, &code, formal)?;
        if !report.live {
            anyhow::bail!("cheap-proof probe declined: verification report is not live");
        }
        Ok(report.lexically_verified && report.axioms_clean && report.statement_preserved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelResponse;
    use serde_json::Value;
    use std::cell::RefCell;

    const INFORMAL: &str = "every even integer has an even square";
    const FORMAL: &str = "theorem t (n : Int) (h : Even n) : Even (n * n) := by sorry";

    // -- mock provider --------------------------------------------------------

    /// Records every request it receives and replays a scripted answer.
    struct RecordingProvider {
        answers: Vec<Value>,
        seen: RefCell<Vec<ModelRequest>>,
    }
    impl RecordingProvider {
        fn new(answers: Vec<Value>) -> Self {
            Self {
                answers,
                seen: RefCell::new(Vec::new()),
            }
        }
    }
    impl ModelProvider for RecordingProvider {
        fn complete(&self, request: &ModelRequest) -> Result<ModelResponse> {
            let idx = self.seen.borrow().len();
            self.seen.borrow_mut().push(request.clone());
            Ok(ModelResponse {
                content: self.answers.get(idx).cloned().unwrap_or(json!({})),
                model: "test".into(),
                provider: "mock".into(),
            })
        }
        fn name(&self) -> &str {
            "mock"
        }
    }

    struct ErrorProvider;
    impl ModelProvider for ErrorProvider {
        fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse> {
            anyhow::bail!("provider exploded")
        }
        fn name(&self) -> &str {
            "mock-error"
        }
    }

    // -- ModelJudge -----------------------------------------------------------

    #[test]
    fn sample_idx_varies_the_request_so_samples_are_independent() {
        let provider = RecordingProvider::new(vec![]);
        let judge = ModelJudge::new(&provider);
        let a = judge.request(INFORMAL, FORMAL, 0);
        let b = judge.request(INFORMAL, FORMAL, 1);
        assert_ne!(a.task, b.task, "the index reaches the prompt text");
        assert_eq!(a.context["sample_idx"], 0);
        assert_eq!(b.context["sample_idx"], 1);
        assert_eq!(b.context["seed"], 1);
        assert_ne!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap(),
            "distinct samples must not send byte-identical requests, or unanimity is vacuous"
        );
        // ...and the statement under judgement is carried unchanged.
        assert_eq!(a.context["informal"], INFORMAL);
        assert_eq!(a.context["formal"], FORMAL);
    }

    #[test]
    fn each_drawn_sample_reaches_the_provider_with_its_own_index() {
        let provider = RecordingProvider::new(vec![
            json!({"faithful": true, "reason": "ok"}),
            json!({"faithful": true, "reason": "ok"}),
        ]);
        let judge = ModelJudge::new(&provider);
        let _ = judge.judge(INFORMAL, FORMAL, 0);
        let _ = judge.judge(INFORMAL, FORMAL, 1);
        let seen = provider.seen.borrow();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].context["sample_idx"], 0);
        assert_eq!(seen[1].context["sample_idx"], 1);
        assert_eq!(seen[0].role, JUDGE_ROLE);
    }

    #[test]
    fn a_faithful_answer_is_a_faithful_vote() {
        let provider =
            RecordingProvider::new(vec![json!({"faithful": true, "reason": "matches exactly"})]);
        let verdict = ModelJudge::new(&provider).judge(INFORMAL, FORMAL, 0);
        assert!(verdict.faithful);
        assert!(verdict.reason.contains("matches exactly"));
    }

    #[test]
    fn a_provider_error_is_never_a_passing_verdict() {
        let verdict = ModelJudge::new(&ErrorProvider).judge(INFORMAL, FORMAL, 7);
        assert!(
            !verdict.faithful,
            "an error must never manufacture a faithful vote"
        );
        assert!(verdict.reason.contains("error"));
    }

    #[test]
    fn an_unreadable_answer_is_never_a_passing_verdict() {
        // Missing field, wrong type, and empty object all dissent.
        for content in [
            json!({"reason": "forgot the boolean"}),
            json!({"faithful": "yes"}),
            json!({}),
        ] {
            let provider = RecordingProvider::new(vec![content.clone()]);
            let verdict = ModelJudge::new(&provider).judge(INFORMAL, FORMAL, 0);
            assert!(!verdict.faithful, "non-boolean `faithful` must dissent");
        }
    }

    #[test]
    fn an_explicit_dissent_is_carried_with_its_reason() {
        let provider = RecordingProvider::new(vec![
            json!({"faithful": false, "reason": "quantifier order swapped"}),
        ]);
        let verdict = ModelJudge::new(&provider).judge(INFORMAL, FORMAL, 2);
        assert!(!verdict.faithful);
        assert!(verdict.reason.contains("quantifier order swapped"));
    }

    // -- FalsifierNegation ----------------------------------------------------

    struct MockFalsify {
        verdict: String,
        fail: bool,
        seen: RefCell<Vec<String>>,
    }
    impl MockFalsify {
        fn verdict(verdict: &str) -> Self {
            Self {
                verdict: verdict.into(),
                fail: false,
                seen: RefCell::new(Vec::new()),
            }
        }
        fn failing() -> Self {
            Self {
                verdict: String::new(),
                fail: true,
                seen: RefCell::new(Vec::new()),
            }
        }
    }
    impl Falsify for MockFalsify {
        fn falsify(&self, statement: &str) -> Result<FalsifyVerdict> {
            self.seen.borrow_mut().push(statement.into());
            if self.fail {
                anyhow::bail!("falsifier exploded");
            }
            Ok(FalsifyVerdict {
                applicable: true,
                verdict: self.verdict.clone(),
                assignment: None,
                spec: Value::Null,
                details: Value::Null,
            })
        }
    }

    #[test]
    fn a_found_counterexample_proves_the_negation() {
        // POLARITY: counterexample ⇒ the statement is false ⇒ its NEGATION has
        // evidence ⇒ Proved ⇒ the negation check REJECTS the formalization.
        let f = MockFalsify::verdict("counterexample");
        assert_eq!(
            FalsifierNegation::new(&f).prove_negation(INFORMAL, FORMAL),
            ProofOutcome::Proved,
        );
    }

    #[test]
    fn a_clean_falsifier_run_is_inconclusive_not_not_proved() {
        // The critical case. A bounded sweep that found nothing has NOT shown the
        // negation is unprovable — NotProved here would PASS the check and
        // launder a finite search into a soundness claim.
        let f = MockFalsify::verdict("no_counterexample_in_domain");
        let outcome = FalsifierNegation::new(&f).prove_negation(INFORMAL, FORMAL);
        assert_eq!(outcome, ProofOutcome::Inconclusive);
        assert_ne!(
            outcome,
            ProofOutcome::NotProved,
            "absence of a counterexample is not proof that the negation fails"
        );
    }

    #[test]
    fn every_other_falsifier_verdict_is_inconclusive() {
        for v in [
            "not_applicable",
            "inconclusive",
            "no_model",
            "unavailable",
            "error",
            "",
        ] {
            let f = MockFalsify::verdict(v);
            assert_eq!(
                FalsifierNegation::new(&f).prove_negation(INFORMAL, FORMAL),
                ProofOutcome::Inconclusive,
                "verdict {v:?} must not be read as evidence either way"
            );
        }
    }

    #[test]
    fn a_falsifier_error_is_inconclusive_never_not_proved() {
        let f = MockFalsify::failing();
        assert_eq!(
            FalsifierNegation::new(&f).prove_negation(INFORMAL, FORMAL),
            ProofOutcome::Inconclusive,
        );
    }

    #[test]
    fn the_formal_statement_is_screened_by_default() {
        let f = MockFalsify::verdict("inconclusive");
        let neg = FalsifierNegation::new(&f);
        assert_eq!(neg.target(), FalsifyTarget::Formal);
        neg.prove_negation(INFORMAL, FORMAL);
        assert_eq!(*f.seen.borrow(), vec![FORMAL.to_string()]);

        let g = MockFalsify::verdict("inconclusive");
        FalsifierNegation::new(&g)
            .on_informal()
            .prove_negation(INFORMAL, FORMAL);
        assert_eq!(*g.seen.borrow(), vec![INFORMAL.to_string()]);
    }

    // -- CheapTrivialProver ---------------------------------------------------

    struct MockAttempt {
        /// Tactic that closes the goal, if any.
        closes: Option<&'static str>,
        fail: bool,
        seen: RefCell<Vec<String>>,
    }
    impl MockAttempt {
        fn closing(tactic: &'static str) -> Self {
            Self {
                closes: Some(tactic),
                fail: false,
                seen: RefCell::new(Vec::new()),
            }
        }
        fn never_closes() -> Self {
            Self {
                closes: None,
                fail: false,
                seen: RefCell::new(Vec::new()),
            }
        }
        fn failing() -> Self {
            Self {
                closes: None,
                fail: true,
                seen: RefCell::new(Vec::new()),
            }
        }
    }
    impl CheapProofAttempt for MockAttempt {
        fn try_close(&self, _formal: &str, tactic: &str) -> Result<bool> {
            self.seen.borrow_mut().push(tactic.into());
            if self.fail {
                anyhow::bail!("no toolchain");
            }
            Ok(self.closes == Some(tactic))
        }
    }

    #[test]
    fn a_trivial_proof_is_proved_meaning_the_statement_is_degenerate() {
        // POLARITY: Proved ⇒ DEGENERATE ⇒ the triviality check REJECTS it.
        let a = MockAttempt::closing("trivial");
        assert_eq!(
            CheapTrivialProver::new(&a).prove_trivially(INFORMAL, "theorem t : True"),
            ProofOutcome::Proved,
        );
        // It stopped as soon as a tactic closed — no wasted budget.
        assert_eq!(*a.seen.borrow(), vec!["rfl", "trivial"]);
    }

    #[test]
    fn a_statement_no_cheap_tactic_closes_is_not_proved() {
        let a = MockAttempt::never_closes();
        assert_eq!(
            CheapTrivialProver::new(&a).prove_trivially(INFORMAL, FORMAL),
            ProofOutcome::NotProved,
        );
        assert_eq!(a.seen.borrow().len(), DEFAULT_TRIVIAL_TACTICS.len());
    }

    #[test]
    fn an_attempt_error_is_inconclusive_never_not_proved() {
        let a = MockAttempt::failing();
        let outcome = CheapTrivialProver::new(&a).prove_trivially(INFORMAL, FORMAL);
        assert_eq!(outcome, ProofOutcome::Inconclusive);
        assert_ne!(
            outcome,
            ProofOutcome::NotProved,
            "a broken toolchain is not evidence that the statement has content"
        );
        // Short-circuits on the first error rather than grinding the budget.
        assert_eq!(a.seen.borrow().len(), 1);
    }

    #[test]
    fn an_empty_tactic_budget_probes_nothing_and_establishes_nothing() {
        let a = MockAttempt::never_closes();
        assert_eq!(
            CheapTrivialProver::with_tactics(&a, Vec::new()).prove_trivially(INFORMAL, FORMAL),
            ProofOutcome::Inconclusive,
        );
        assert!(a.seen.borrow().is_empty());
    }

    #[test]
    fn the_tactic_budget_is_configurable() {
        let a = MockAttempt::closing("omega");
        let prover = CheapTrivialProver::with_tactics(&a, vec!["omega".into()]);
        assert_eq!(prover.tactics(), ["omega".to_string()]);
        assert_eq!(
            prover.prove_trivially(INFORMAL, FORMAL),
            ProofOutcome::Proved
        );
    }

    // -- statement splicing ---------------------------------------------------

    #[test]
    fn a_tactic_body_replaces_any_existing_proof() {
        assert_eq!(
            with_tactic_body("theorem t : True := by sorry", "trivial"),
            "theorem t : True := by trivial"
        );
        assert_eq!(
            with_tactic_body("theorem t : True", "trivial"),
            "theorem t : True := by trivial"
        );
    }

    // -- the adapters compose with the pure stack -----------------------------

    #[test]
    fn the_wired_stack_accepts_a_good_statement_and_rejects_a_bad_one() {
        use crate::statement_validity::{StatementValidity, StatementVerdict, ValidityCfg};

        let cfg = ValidityCfg {
            judge_samples: 2,
            ..ValidityCfg::default()
        };

        // Good: unanimous judge, no counterexample... but note the falsifier can
        // only ever be Inconclusive, so a fully-wired stack is Indeterminate, not
        // Accept. That is the honest answer — a numeric screen cannot certify.
        let provider = RecordingProvider::new(vec![
            json!({"faithful": true, "reason": "ok"}),
            json!({"faithful": true, "reason": "ok"}),
        ]);
        let judge = ModelJudge::new(&provider);
        let f = MockFalsify::verdict("no_counterexample_in_domain");
        let neg = FalsifierNegation::new(&f);
        let a = MockAttempt::never_closes();
        let triv = CheapTrivialProver::new(&a);
        let report = StatementValidity::new(cfg)
            .with_judge(&judge)
            .with_negation_prover(&neg)
            .with_trivial_prover(&triv)
            .screen(INFORMAL, FORMAL);
        assert_eq!(report.verdict, StatementVerdict::Indeterminate);
        assert!(report.failed().is_empty(), "nothing condemned this candidate");

        // Bad: a counterexample condemns the formalization.
        let provider = RecordingProvider::new(vec![
            json!({"faithful": true, "reason": "ok"}),
            json!({"faithful": true, "reason": "ok"}),
        ]);
        let judge = ModelJudge::new(&provider);
        let f = MockFalsify::verdict("counterexample");
        let neg = FalsifierNegation::new(&f);
        let report = StatementValidity::new(cfg)
            .with_judge(&judge)
            .with_negation_prover(&neg)
            .with_trivial_prover(&triv)
            .screen(INFORMAL, FORMAL);
        assert_eq!(report.verdict, StatementVerdict::Reject);
        assert!(report.verdict.blocks_attempt());
    }

    #[test]
    fn a_broken_provider_can_only_reject_never_accept() {
        use crate::statement_validity::{StatementValidity, StatementVerdict};
        let judge = ModelJudge::new(&ErrorProvider);
        let report = StatementValidity::default()
            .with_judge(&judge)
            .screen(INFORMAL, FORMAL);
        assert_ne!(report.verdict, StatementVerdict::Accept);
        assert!(!report.verdict.is_accept());
    }
}
