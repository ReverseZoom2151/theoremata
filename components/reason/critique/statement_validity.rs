//! Statement-validity **filter stack** (Tier 2 item 9) — the anti-reward-hacking
//! machinery that surrounds an LLM judge.
//!
//! Two independently-mined systems reached the same conclusion: an LLM judge
//! **alone** is not a sufficient gate on whether a formalized statement is worth
//! proving. A single judge sample is cheap to fool and cheap to please — it is
//! exactly the surface a statement-generator learns to reward-hack (emit a
//! vacuous, weakened, or self-contradictory statement that a judge waves through
//! and a prover then closes in one tactic, harvesting a "success").
//!
//! This module implements the filter stack that wraps the judge:
//!
//! 1. [`UnanimityCheck`] — multi-sample judging. `N` INDEPENDENT judge verdicts
//!    are drawn through the [`StatementJudge`] seam and **unanimity** is required
//!    to pass. This was found to *significantly reduce false positives with
//!    little impact on true positives*: a bad statement rarely survives every
//!    sample, while a good statement is agreed on by all of them. The full
//!    [`VoteSplit`] is reported, never just the boolean.
//! 2. [`NegationCheck`] — attempt to prove the **negation** of the statement via
//!    the [`NegationProver`] seam. Success means the formalization is broken (the
//!    formal statement is false, so any "proof" of it would be a proof of a
//!    contradictory or mis-transcribed claim). This reuses our existing
//!    falsify-before-prove primitive ([`crate::falsification`]) as a **curation**
//!    filter — a use we did not previously have; there it screens a *branch*,
//!    here it screens a *candidate statement* before any budget is spent.
//! 3. [`TrivialityCheck`] — attempt a short/cheap proof via the [`TrivialProver`]
//!    seam. If a trivial proof closes the goal, the statement is DEGENERATE:
//!    vacuous hypotheses (an unsatisfiable premise makes anything provable) or a
//!    trivially-true conclusion (`: True`, `n = n`). This is the
//!    prover-backed counterpart of the lexical smell in
//!    [`crate::critic::short_proof_for_hard_target`].
//!
//! [`StatementValidity`] composes all three behind one facade with a fail-closed
//! combined verdict and a per-check enable flag.
//!
//! # INVARIANT: this is NOT a second soundness authority
//!
//! The formal verification gate ([`crate::prover::formal`], registered as
//! [`crate::guardrails::Policy::OutputSoundness`]) remains the **SOLE** authority
//! on whether a PROOF is valid. Nothing here can certify a proof, overturn a gate
//! verdict, or make an unverified result trusted. This module screens only
//! whether a **STATEMENT is worth attempting** — a budget/curation decision taken
//! *before* proof search begins. A [`StatementVerdict::Reject`] means "do not
//! spend proof budget on this candidate"; it never means "this is false" and it
//! never marks anything verified. Accordingly
//! [`StatementValidity::is_soundness_authority`] is permanently `false`.
//!
//! # Purity
//!
//! Every external capability is an INJECTED TRAIT seam, matching the crate's
//! style elsewhere. This file performs no model calls, no process spawning, no
//! IO, no clock reads and no RNG: given the same seam answers it produces the
//! same report, so it is fully unit-testable with mocks.
//!
//! # Skipped is never Passed
//!
//! A check whose seam is absent, or which is disabled by config, reports
//! [`CheckStatus::Skipped`] — visibly distinct from [`CheckStatus::Passed`] in
//! the report and in the combined verdict, which degrades to
//! [`StatementVerdict::Indeterminate`] rather than silently accepting. Defaults
//! are behavior-preserving: all seams are optional, so a caller that injects
//! nothing gets an all-`Skipped` report and an `Indeterminate` verdict, which
//! does not block anything.

use serde::Serialize;

// ---------------------------------------------------------------------------
// Seams
// ---------------------------------------------------------------------------

/// One judge sample's opinion on whether `formal` faithfully encodes `informal`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JudgeVerdict {
    /// True iff this sample judged the formalization faithful to the informal
    /// source.
    pub faithful: bool,
    /// The sample's one-line reason (kept for audit; never parsed for control
    /// flow).
    pub reason: String,
}

impl JudgeVerdict {
    /// A faithful vote with a reason.
    pub fn faithful(reason: impl Into<String>) -> Self {
        Self {
            faithful: true,
            reason: reason.into(),
        }
    }

    /// A dissenting (unfaithful) vote with a reason.
    pub fn unfaithful(reason: impl Into<String>) -> Self {
        Self {
            faithful: false,
            reason: reason.into(),
        }
    }
}

/// SEAM: an LLM (or any) judge of statement faithfulness, sampled `sample_idx`
/// times. Implementations MUST treat each `sample_idx` as an INDEPENDENT draw —
/// the whole point of the unanimity filter is that correlated samples do not
/// reduce false positives. `sample_idx` is supplied by this module (a plain
/// counter) precisely so the seam owns any seeding; no RNG lives here.
pub trait StatementJudge {
    fn judge(&self, informal: &str, formal: &str, sample_idx: u64) -> JudgeVerdict;
}

/// The outcome of a proof attempt made by a screening seam. Deliberately
/// three-valued: an attempt that neither closed nor cleanly failed (timeout,
/// backend unavailable, parse error) is [`ProofOutcome::Inconclusive`] and must
/// NOT be read as evidence either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofOutcome {
    /// The attempt closed the goal.
    Proved,
    /// The attempt ran to completion and did not close the goal.
    NotProved,
    /// The attempt could not be completed (timeout / unavailable / error).
    Inconclusive,
}

/// SEAM: attempts to prove the NEGATION of a candidate formal statement. A
/// [`ProofOutcome::Proved`] here is a hard signal that the formalization is
/// broken. Implementations typically wrap the falsify-before-prove primitive or
/// a short backend run on the negated goal.
pub trait NegationProver {
    fn prove_negation(&self, informal: &str, formal: &str) -> ProofOutcome;
}

/// SEAM: attempts a SHORT/CHEAP proof of the candidate statement (a couple of
/// closing tactics, a tiny budget). A [`ProofOutcome::Proved`] means the
/// statement is degenerate and not worth real proof budget.
pub trait TrivialProver {
    fn prove_trivially(&self, informal: &str, formal: &str) -> ProofOutcome;
}

// ---------------------------------------------------------------------------
// Report types
// ---------------------------------------------------------------------------

/// The registry of checks in this stack, in fixed evaluation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    /// Multi-sample judge agreement.
    Unanimity,
    /// Negation-provability screen.
    Negation,
    /// Triviality (degenerate-statement) screen.
    Triviality,
}

impl Check {
    /// Fixed, deterministic order.
    pub const ALL: [Check; 3] = [Check::Unanimity, Check::Negation, Check::Triviality];

    /// Stable snake_case id for observability payloads.
    pub fn id(self) -> &'static str {
        match self {
            Check::Unanimity => "unanimity",
            Check::Negation => "negation",
            Check::Triviality => "triviality",
        }
    }

    /// One-line description of WHAT this check screens for.
    pub fn screens(self) -> &'static str {
        match self {
            Check::Unanimity => {
                "N independent judge samples must UNANIMOUSLY call the formalization faithful; \
                 unanimity significantly reduces false positives with little impact on true positives"
            }
            Check::Negation => {
                "if the NEGATION of the statement is provable, the formalization is broken \
                 (falsify-before-prove reused as a curation filter)"
            }
            Check::Triviality => {
                "if a short/cheap proof closes the statement, it is degenerate — vacuous \
                 hypotheses or a trivially-true conclusion"
            }
        }
    }
}

/// The status of a single check. `Skipped` is deliberately a distinct variant
/// from `Passed`: a check that did not run has produced NO evidence of validity
/// and must never be counted as if it had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    /// The check ran and the statement survived it.
    Passed,
    /// The check ran and the statement failed it.
    Failed,
    /// The check did not run (seam absent, or disabled by config, or the seam
    /// returned an inconclusive outcome). NOT a pass.
    Skipped,
}

impl CheckStatus {
    /// True only for [`CheckStatus::Passed`]. Provided so no caller can
    /// accidentally spell "not failed" and thereby treat `Skipped` as a pass.
    pub fn is_pass(self) -> bool {
        matches!(self, CheckStatus::Passed)
    }

    /// True only for [`CheckStatus::Failed`].
    pub fn is_fail(self) -> bool {
        matches!(self, CheckStatus::Failed)
    }

    /// True only for [`CheckStatus::Skipped`].
    pub fn is_skipped(self) -> bool {
        matches!(self, CheckStatus::Skipped)
    }

    /// Stable snake_case tag.
    pub fn tag(self) -> &'static str {
        match self {
            CheckStatus::Passed => "passed",
            CheckStatus::Failed => "failed",
            CheckStatus::Skipped => "skipped",
        }
    }
}

/// How the judge samples split. Reported verbatim so a caller can see a 4-1
/// split (a near-miss worth surfacing) differently from a 0-5 rout.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct VoteSplit {
    /// Samples that judged the formalization faithful.
    pub faithful: u32,
    /// Samples that dissented.
    pub dissenting: u32,
}

impl VoteSplit {
    /// Total samples drawn.
    pub fn total(&self) -> u32 {
        self.faithful + self.dissenting
    }

    /// Unanimous FOR: at least one sample, and no dissent. A zero-sample split
    /// is NOT unanimous — an empty vote proves nothing.
    pub fn is_unanimous_for(&self) -> bool {
        self.total() > 0 && self.dissenting == 0
    }

    /// Human-readable `faithful/total` summary.
    pub fn summary(&self) -> String {
        format!("{}/{} judge samples faithful", self.faithful, self.total())
    }
}

/// The result of one check: its status, a human reason, and (for the unanimity
/// check) the vote split plus the individual sample verdicts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckReport {
    pub check: Check,
    pub status: CheckStatus,
    /// Why this status was reached (always populated, including for skips —
    /// a skip always says WHY it was skipped).
    pub reason: String,
    /// Populated only by [`Check::Unanimity`]; `None` elsewhere.
    pub votes: Option<VoteSplit>,
    /// The raw judge samples backing `votes`, in draw order. Empty for other
    /// checks.
    pub samples: Vec<JudgeVerdict>,
}

impl CheckReport {
    fn skipped(check: Check, reason: impl Into<String>) -> Self {
        Self {
            check,
            status: CheckStatus::Skipped,
            reason: reason.into(),
            votes: None,
            samples: Vec::new(),
        }
    }

    fn simple(check: Check, status: CheckStatus, reason: impl Into<String>) -> Self {
        Self {
            check,
            status,
            reason: reason.into(),
            votes: None,
            samples: Vec::new(),
        }
    }
}

/// The combined, fail-closed verdict over the whole stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatementVerdict {
    /// Every ENABLED check ran and passed. The statement is worth attempting.
    Accept,
    /// At least one check FAILED. Do not spend proof budget on this candidate.
    /// This is a curation decision, never a claim that the statement is false
    /// and never a soundness verdict.
    Reject,
    /// No check failed, but at least one did not run (absent seam, disabled, or
    /// an inconclusive seam outcome), so the stack has not established validity.
    /// Fail-closed: this is NOT an accept. Callers decide their own policy
    /// (proceed with a warning, or escalate) — the default caller behaviour is
    /// to proceed, which is what makes an all-seams-absent configuration
    /// behavior-preserving.
    Indeterminate,
}

impl StatementVerdict {
    /// Stable snake_case tag.
    pub fn tag(self) -> &'static str {
        match self {
            StatementVerdict::Accept => "accept",
            StatementVerdict::Reject => "reject",
            StatementVerdict::Indeterminate => "indeterminate",
        }
    }

    /// The only verdict that positively clears a statement.
    pub fn is_accept(self) -> bool {
        matches!(self, StatementVerdict::Accept)
    }

    /// Whether the stack advises SKIPPING proof search on this candidate. Only
    /// an outright `Reject` does — an `Indeterminate` never blocks work, it just
    /// records that nothing was established.
    pub fn blocks_attempt(self) -> bool {
        matches!(self, StatementVerdict::Reject)
    }
}

/// The full stack report: the combined verdict plus every check's own report, in
/// [`Check::ALL`] order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidityReport {
    pub verdict: StatementVerdict,
    pub checks: Vec<CheckReport>,
}

impl ValidityReport {
    /// Look up one check's report.
    pub fn check(&self, check: Check) -> Option<&CheckReport> {
        self.checks.iter().find(|c| c.check == check)
    }

    /// The status of one check (`Skipped` if, impossibly, it is absent — never a
    /// pass).
    pub fn status(&self, check: Check) -> CheckStatus {
        self.check(check)
            .map(|c| c.status)
            .unwrap_or(CheckStatus::Skipped)
    }

    /// The checks that failed, in order.
    pub fn failed(&self) -> Vec<Check> {
        self.checks
            .iter()
            .filter(|c| c.status.is_fail())
            .map(|c| c.check)
            .collect()
    }

    /// The checks that did not run, in order.
    pub fn skipped(&self) -> Vec<Check> {
        self.checks
            .iter()
            .filter(|c| c.status.is_skipped())
            .map(|c| c.check)
            .collect()
    }

    /// The judge vote split, when the unanimity check ran.
    pub fn votes(&self) -> Option<VoteSplit> {
        self.check(Check::Unanimity).and_then(|c| c.votes)
    }

    /// One human-readable line per check, for logs/events.
    pub fn reasons(&self) -> Vec<String> {
        self.checks
            .iter()
            .map(|c| format!("[{}] {}: {}", c.check.id(), c.status.tag(), c.reason))
            .collect()
    }

    /// Fold the per-check statuses into the fail-closed combined verdict:
    /// any `Failed` ⇒ `Reject`; else any `Skipped` (or nothing ran at all) ⇒
    /// `Indeterminate`; else `Accept`.
    fn combine(checks: &[CheckReport]) -> StatementVerdict {
        if checks.iter().any(|c| c.status.is_fail()) {
            return StatementVerdict::Reject;
        }
        if checks.is_empty() || checks.iter().any(|c| c.status.is_skipped()) {
            return StatementVerdict::Indeterminate;
        }
        StatementVerdict::Accept
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Per-check enable flags and the judge sample count.
///
/// Defaults are behavior-preserving: each check is ENABLED, but every seam is
/// optional, so a caller who injects no seams gets three `Skipped` reports and
/// an `Indeterminate` verdict — no behaviour change, and no accidental pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ValidityCfg {
    pub unanimity_enabled: bool,
    pub negation_enabled: bool,
    pub triviality_enabled: bool,
    /// How many INDEPENDENT judge samples the unanimity check draws. `0`
    /// degenerates to a skip (an empty vote establishes nothing) rather than to
    /// a vacuous unanimity.
    pub judge_samples: u32,
}

impl Default for ValidityCfg {
    fn default() -> Self {
        Self {
            unanimity_enabled: true,
            negation_enabled: true,
            triviality_enabled: true,
            // Odd, small default: enough independent draws for unanimity to bite
            // without a large judging bill.
            judge_samples: 3,
        }
    }
}

// ---------------------------------------------------------------------------
// The checks
// ---------------------------------------------------------------------------

/// Multi-sample judge agreement. Draws `samples` INDEPENDENT verdicts (sample
/// indices `0..samples`, supplied to the seam so seeding lives outside this
/// module) and requires UNANIMITY to pass.
///
/// Every sample is drawn — the check never short-circuits on the first dissent —
/// so that the reported [`VoteSplit`] is always the true split rather than a
/// truncated one.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnanimityCheck {
    pub samples: u32,
}

impl UnanimityCheck {
    pub fn new(samples: u32) -> Self {
        Self { samples }
    }

    /// Run the check. `judge: None` ⇒ [`CheckStatus::Skipped`].
    pub fn run(
        &self,
        judge: Option<&dyn StatementJudge>,
        informal: &str,
        formal: &str,
    ) -> CheckReport {
        let Some(judge) = judge else {
            return CheckReport::skipped(
                Check::Unanimity,
                "no StatementJudge seam injected; no judge evidence was gathered (NOT a pass)",
            );
        };
        if self.samples == 0 {
            return CheckReport::skipped(
                Check::Unanimity,
                "judge_samples = 0; an empty vote establishes nothing (NOT a pass)",
            );
        }
        let mut samples = Vec::with_capacity(self.samples as usize);
        let mut votes = VoteSplit::default();
        for idx in 0..self.samples {
            let verdict = judge.judge(informal, formal, u64::from(idx));
            if verdict.faithful {
                votes.faithful += 1;
            } else {
                votes.dissenting += 1;
            }
            samples.push(verdict);
        }
        let (status, reason) = if votes.is_unanimous_for() {
            (
                CheckStatus::Passed,
                format!(
                    "unanimous: {} — every independent sample judged the formalization faithful",
                    votes.summary()
                ),
            )
        } else {
            let dissent: Vec<&str> = samples
                .iter()
                .filter(|s| !s.faithful)
                .map(|s| s.reason.as_str())
                .collect();
            (
                CheckStatus::Failed,
                format!(
                    "not unanimous: {} — {} sample(s) dissented ({}). Unanimity is required \
                     because it significantly reduces false positives with little impact on \
                     true positives.",
                    votes.summary(),
                    votes.dissenting,
                    dissent.join("; ")
                ),
            )
        };
        CheckReport {
            check: Check::Unanimity,
            status,
            reason,
            votes: Some(votes),
            samples,
        }
    }
}

/// Negation screen: if the NEGATION of the statement is provable, the
/// formalization is broken.
#[derive(Debug, Clone, Copy, Default)]
pub struct NegationCheck;

impl NegationCheck {
    /// Run the check. `prover: None` ⇒ [`CheckStatus::Skipped`]. An
    /// [`ProofOutcome::Inconclusive`] attempt is also a skip: a negation attempt
    /// that timed out is not evidence that the statement is sound.
    pub fn run(
        &self,
        prover: Option<&dyn NegationProver>,
        informal: &str,
        formal: &str,
    ) -> CheckReport {
        let Some(prover) = prover else {
            return CheckReport::skipped(
                Check::Negation,
                "no NegationProver seam injected; the negation was never attempted (NOT a pass)",
            );
        };
        match prover.prove_negation(informal, formal) {
            ProofOutcome::Proved => CheckReport::simple(
                Check::Negation,
                CheckStatus::Failed,
                "the NEGATION of this statement was proved — the formalization is broken \
                 (the formal statement is false, so any proof of it would prove a mis-transcribed \
                 or contradictory claim). Do not spend proof budget on it; re-formalize.",
            ),
            ProofOutcome::NotProved => CheckReport::simple(
                Check::Negation,
                CheckStatus::Passed,
                "the negation attempt ran to completion and did not close — no refutation found",
            ),
            ProofOutcome::Inconclusive => CheckReport::skipped(
                Check::Negation,
                "the negation attempt was inconclusive (timeout / unavailable / error); \
                 an unfinished refutation is NOT evidence of validity",
            ),
        }
    }
}

/// Triviality screen: if a short/cheap proof closes the statement, it is
/// degenerate.
#[derive(Debug, Clone, Copy, Default)]
pub struct TrivialityCheck;

impl TrivialityCheck {
    /// Run the check. `prover: None` ⇒ [`CheckStatus::Skipped`]. An
    /// [`ProofOutcome::Inconclusive`] attempt is also a skip.
    ///
    /// Note the polarity: here `Proved` is the FAILING outcome. A cheap proof of
    /// a statement we intended to be substantive means the statement lost its
    /// content in formalization.
    pub fn run(
        &self,
        prover: Option<&dyn TrivialProver>,
        informal: &str,
        formal: &str,
    ) -> CheckReport {
        let Some(prover) = prover else {
            return CheckReport::skipped(
                Check::Triviality,
                "no TrivialProver seam injected; triviality was never probed (NOT a pass)",
            );
        };
        match prover.prove_trivially(informal, formal) {
            ProofOutcome::Proved => CheckReport::simple(
                Check::Triviality,
                CheckStatus::Failed,
                "a short/cheap proof closed this statement — it is DEGENERATE: either the \
                 hypotheses are vacuous (an unsatisfiable premise proves anything) or the \
                 conclusion is trivially true. Proving it would establish nothing about the \
                 intended claim.",
            ),
            ProofOutcome::NotProved => CheckReport::simple(
                Check::Triviality,
                CheckStatus::Passed,
                "the cheap-proof attempt did not close the goal — the statement has content",
            ),
            ProofOutcome::Inconclusive => CheckReport::skipped(
                Check::Triviality,
                "the cheap-proof attempt was inconclusive (timeout / unavailable / error); \
                 a failed-to-run probe is NOT evidence of non-triviality",
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Facade
// ---------------------------------------------------------------------------

/// The composed statement-validity filter stack.
///
/// Holds only its config and borrowed, OPTIONAL seams — no state, no IO, no
/// clock, no RNG. Build it with [`StatementValidity::new`] and attach seams with
/// the `with_*` builders; anything not attached yields a `Skipped` check.
///
/// Reminder of the module invariant: this facade owns NO soundness decision.
/// [`StatementValidity::is_soundness_authority`] returns `false` permanently;
/// the formal gate ([`crate::prover::formal`]) remains the sole authority on
/// proof validity. This only screens whether a statement is worth attempting.
pub struct StatementValidity<'a> {
    cfg: ValidityCfg,
    judge: Option<&'a dyn StatementJudge>,
    negation: Option<&'a dyn NegationProver>,
    trivial: Option<&'a dyn TrivialProver>,
}

impl Default for StatementValidity<'_> {
    fn default() -> Self {
        Self::new(ValidityCfg::default())
    }
}

impl<'a> StatementValidity<'a> {
    /// A stack with no seams attached: every check reports `Skipped`.
    pub fn new(cfg: ValidityCfg) -> Self {
        Self {
            cfg,
            judge: None,
            negation: None,
            trivial: None,
        }
    }

    /// Attach the multi-sample judge seam.
    pub fn with_judge(mut self, judge: &'a dyn StatementJudge) -> Self {
        self.judge = Some(judge);
        self
    }

    /// Attach the negation-prover seam.
    pub fn with_negation_prover(mut self, prover: &'a dyn NegationProver) -> Self {
        self.negation = Some(prover);
        self
    }

    /// Attach the trivial-prover seam.
    pub fn with_trivial_prover(mut self, prover: &'a dyn TrivialProver) -> Self {
        self.trivial = Some(prover);
        self
    }

    /// The active configuration.
    pub fn cfg(&self) -> ValidityCfg {
        self.cfg
    }

    /// This stack is NOT and never becomes a soundness authority — see the
    /// module docs. Kept as a callable so tests and callers can assert it,
    /// mirroring [`crate::guardrails::Policy::is_soundness_authority`].
    pub fn is_soundness_authority(&self) -> bool {
        false
    }

    /// Screen a candidate `formal` statement against its `informal` source.
    ///
    /// Runs every ENABLED check in [`Check::ALL`] order, then folds the statuses
    /// fail-closed: any failure ⇒ [`StatementVerdict::Reject`]; otherwise any
    /// skip ⇒ [`StatementVerdict::Indeterminate`]; only an all-passed stack ⇒
    /// [`StatementVerdict::Accept`].
    ///
    /// Every check runs (no short-circuit on the first failure) so the report is
    /// complete and each independent problem with a candidate is surfaced in one
    /// pass — the same "assume and continue" discipline the adversarial critic
    /// uses for justification gaps.
    pub fn screen(&self, informal: &str, formal: &str) -> ValidityReport {
        let mut checks = Vec::with_capacity(Check::ALL.len());

        checks.push(if self.cfg.unanimity_enabled {
            UnanimityCheck::new(self.cfg.judge_samples).run(self.judge, informal, formal)
        } else {
            CheckReport::skipped(
                Check::Unanimity,
                "disabled by config (unanimity_enabled = false); NOT a pass",
            )
        });

        checks.push(if self.cfg.negation_enabled {
            NegationCheck.run(self.negation, informal, formal)
        } else {
            CheckReport::skipped(
                Check::Negation,
                "disabled by config (negation_enabled = false); NOT a pass",
            )
        });

        checks.push(if self.cfg.triviality_enabled {
            TrivialityCheck.run(self.trivial, informal, formal)
        } else {
            CheckReport::skipped(
                Check::Triviality,
                "disabled by config (triviality_enabled = false); NOT a pass",
            )
        });

        let verdict = ValidityReport::combine(&checks);
        ValidityReport { verdict, checks }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- mock seams -----------------------------------------------------------

    /// A judge whose per-sample answers are scripted; sample `i` reads
    /// `answers[i]` (out-of-range samples default to faithful).
    struct ScriptedJudge {
        answers: Vec<bool>,
        calls: std::cell::RefCell<Vec<u64>>,
    }
    impl ScriptedJudge {
        fn new(answers: &[bool]) -> Self {
            Self {
                answers: answers.to_vec(),
                calls: std::cell::RefCell::new(Vec::new()),
            }
        }
    }
    impl StatementJudge for ScriptedJudge {
        fn judge(&self, _informal: &str, _formal: &str, sample_idx: u64) -> JudgeVerdict {
            self.calls.borrow_mut().push(sample_idx);
            let ok = self
                .answers
                .get(sample_idx as usize)
                .copied()
                .unwrap_or(true);
            if ok {
                JudgeVerdict::faithful(format!("sample {sample_idx} agrees"))
            } else {
                JudgeVerdict::unfaithful(format!("sample {sample_idx} sees a quantifier swap"))
            }
        }
    }

    struct FixedNegation(ProofOutcome);
    impl NegationProver for FixedNegation {
        fn prove_negation(&self, _informal: &str, _formal: &str) -> ProofOutcome {
            self.0
        }
    }

    struct FixedTrivial(ProofOutcome);
    impl TrivialProver for FixedTrivial {
        fn prove_trivially(&self, _informal: &str, _formal: &str) -> ProofOutcome {
            self.0
        }
    }

    const INFORMAL: &str = "every even integer has an even square";
    const FORMAL: &str = "theorem t (n : Int) (h : Even n) : Even (n * n) := by ...";

    /// A stack with all three seams healthy (unanimous judge, no refutation, no
    /// cheap proof).
    fn all_good() -> (ScriptedJudge, FixedNegation, FixedTrivial) {
        (
            ScriptedJudge::new(&[true, true, true]),
            FixedNegation(ProofOutcome::NotProved),
            FixedTrivial(ProofOutcome::NotProved),
        )
    }

    // -- unanimity ------------------------------------------------------------

    #[test]
    fn unanimous_judges_pass() {
        let judge = ScriptedJudge::new(&[true, true, true]);
        let report = UnanimityCheck::new(3).run(Some(&judge), INFORMAL, FORMAL);
        assert_eq!(report.status, CheckStatus::Passed);
        assert_eq!(
            report.votes,
            Some(VoteSplit {
                faithful: 3,
                dissenting: 0
            })
        );
        // Exactly N independent samples were drawn, with distinct indices.
        assert_eq!(*judge.calls.borrow(), vec![0, 1, 2]);
    }

    #[test]
    fn a_single_dissenting_judge_fails() {
        // 2 of 3 agree — a majority, but NOT unanimity.
        let judge = ScriptedJudge::new(&[true, false, true]);
        let report = UnanimityCheck::new(3).run(Some(&judge), INFORMAL, FORMAL);
        assert_eq!(
            report.status,
            CheckStatus::Failed,
            "a majority is not enough; unanimity is required"
        );
        assert!(report.reason.contains("not unanimous"));
        // The dissenter's reason is carried through for audit.
        assert!(report.reason.contains("quantifier swap"));
    }

    #[test]
    fn the_vote_split_is_reported_accurately() {
        let judge = ScriptedJudge::new(&[true, false, false, true, false]);
        let report = UnanimityCheck::new(5).run(Some(&judge), INFORMAL, FORMAL);
        let votes = report.votes.expect("unanimity check reports a split");
        assert_eq!(votes.faithful, 2);
        assert_eq!(votes.dissenting, 3);
        assert_eq!(votes.total(), 5);
        assert!(!votes.is_unanimous_for());
        assert_eq!(votes.summary(), "2/5 judge samples faithful");
        // Every sample is retained in draw order — the check never
        // short-circuits, so the split is the TRUE split.
        assert_eq!(report.samples.len(), 5);
        let faithful_flags: Vec<bool> = report.samples.iter().map(|s| s.faithful).collect();
        assert_eq!(faithful_flags, vec![true, false, false, true, false]);
        assert_eq!(*judge.calls.borrow(), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn zero_samples_is_a_skip_not_a_vacuous_pass() {
        let judge = ScriptedJudge::new(&[]);
        let report = UnanimityCheck::new(0).run(Some(&judge), INFORMAL, FORMAL);
        assert_eq!(report.status, CheckStatus::Skipped);
        assert!(!report.status.is_pass());
        assert!(judge.calls.borrow().is_empty(), "no samples drawn");
        assert!(!VoteSplit::default().is_unanimous_for());
    }

    // -- negation -------------------------------------------------------------

    #[test]
    fn a_successful_negation_proof_fails_the_statement() {
        let neg = FixedNegation(ProofOutcome::Proved);
        let report = NegationCheck.run(Some(&neg), INFORMAL, FORMAL);
        assert_eq!(report.status, CheckStatus::Failed);
        assert!(report.reason.contains("NEGATION"));
    }

    #[test]
    fn a_failed_negation_attempt_passes_and_inconclusive_skips() {
        let not_proved = FixedNegation(ProofOutcome::NotProved);
        assert_eq!(
            NegationCheck.run(Some(&not_proved), INFORMAL, FORMAL).status,
            CheckStatus::Passed
        );
        let inconclusive = FixedNegation(ProofOutcome::Inconclusive);
        let report = NegationCheck.run(Some(&inconclusive), INFORMAL, FORMAL);
        assert_eq!(report.status, CheckStatus::Skipped);
        assert!(!report.status.is_pass(), "a timeout is never a pass");
    }

    // -- triviality -----------------------------------------------------------

    #[test]
    fn a_trivial_proof_fails_the_statement() {
        let triv = FixedTrivial(ProofOutcome::Proved);
        let report = TrivialityCheck.run(Some(&triv), INFORMAL, FORMAL);
        assert_eq!(report.status, CheckStatus::Failed);
        assert!(report.reason.contains("DEGENERATE"));
    }

    #[test]
    fn a_statement_no_cheap_proof_closes_passes_triviality() {
        let triv = FixedTrivial(ProofOutcome::NotProved);
        assert_eq!(
            TrivialityCheck.run(Some(&triv), INFORMAL, FORMAL).status,
            CheckStatus::Passed
        );
        let inconclusive = FixedTrivial(ProofOutcome::Inconclusive);
        assert_eq!(
            TrivialityCheck
                .run(Some(&inconclusive), INFORMAL, FORMAL)
                .status,
            CheckStatus::Skipped
        );
    }

    // -- facade ---------------------------------------------------------------

    #[test]
    fn all_checks_passing_accepts() {
        let (judge, neg, triv) = all_good();
        let stack = StatementValidity::new(ValidityCfg::default())
            .with_judge(&judge)
            .with_negation_prover(&neg)
            .with_trivial_prover(&triv);
        let report = stack.screen(INFORMAL, FORMAL);
        assert_eq!(report.verdict, StatementVerdict::Accept);
        assert!(report.failed().is_empty());
        assert!(report.skipped().is_empty());
        assert!(report.checks.iter().all(|c| c.status.is_pass()));
        // Checks appear in the registry order.
        let ids: Vec<&str> = report.checks.iter().map(|c| c.check.id()).collect();
        assert_eq!(ids, vec!["unanimity", "negation", "triviality"]);
    }

    #[test]
    fn absent_seams_yield_skipped_never_passed() {
        // No seams at all: the behavior-preserving default.
        let stack = StatementValidity::default();
        let report = stack.screen(INFORMAL, FORMAL);
        assert_eq!(report.skipped(), Check::ALL.to_vec());
        assert!(
            report.checks.iter().all(|c| !c.status.is_pass()),
            "an unrun check must NEVER read as passed"
        );
        // The combined verdict reflects the skips: not an accept, not a reject.
        assert_eq!(report.verdict, StatementVerdict::Indeterminate);
        assert!(!report.verdict.is_accept());
        assert!(
            !report.verdict.blocks_attempt(),
            "indeterminate never blocks work — absent seams are behavior-preserving"
        );
        // Every skip explains itself.
        assert!(report.checks.iter().all(|c| !c.reason.is_empty()));
        assert!(report.votes().is_none());
    }

    #[test]
    fn one_absent_seam_downgrades_an_otherwise_passing_stack() {
        let (judge, neg, _) = all_good();
        let stack = StatementValidity::default()
            .with_judge(&judge)
            .with_negation_prover(&neg);
        let report = stack.screen(INFORMAL, FORMAL);
        assert_eq!(report.status(Check::Unanimity), CheckStatus::Passed);
        assert_eq!(report.status(Check::Negation), CheckStatus::Passed);
        assert_eq!(report.status(Check::Triviality), CheckStatus::Skipped);
        assert_eq!(
            report.verdict,
            StatementVerdict::Indeterminate,
            "two passes plus a skip is NOT an accept"
        );
    }

    #[test]
    fn a_failure_rejects_even_when_other_checks_are_skipped() {
        let triv = FixedTrivial(ProofOutcome::Proved);
        let stack = StatementValidity::default().with_trivial_prover(&triv);
        let report = stack.screen(INFORMAL, FORMAL);
        assert_eq!(report.verdict, StatementVerdict::Reject);
        assert!(report.verdict.blocks_attempt());
        assert_eq!(report.failed(), vec![Check::Triviality]);
        // A failure does not suppress the other checks' reports.
        assert_eq!(report.checks.len(), 3);
    }

    #[test]
    fn every_check_runs_so_all_problems_surface_in_one_pass() {
        let judge = ScriptedJudge::new(&[true, false, true]);
        let neg = FixedNegation(ProofOutcome::Proved);
        let triv = FixedTrivial(ProofOutcome::Proved);
        let stack = StatementValidity::default()
            .with_judge(&judge)
            .with_negation_prover(&neg)
            .with_trivial_prover(&triv);
        let report = stack.screen(INFORMAL, FORMAL);
        assert_eq!(report.verdict, StatementVerdict::Reject);
        assert_eq!(report.failed(), Check::ALL.to_vec());
        assert_eq!(report.reasons().len(), 3);
    }

    #[test]
    fn disabling_a_check_skips_it_rather_than_passing_it() {
        let (judge, neg, triv) = all_good();
        let cfg = ValidityCfg {
            negation_enabled: false,
            ..ValidityCfg::default()
        };
        let stack = StatementValidity::new(cfg)
            .with_judge(&judge)
            .with_negation_prover(&neg)
            .with_trivial_prover(&triv);
        let report = stack.screen(INFORMAL, FORMAL);
        assert_eq!(report.status(Check::Negation), CheckStatus::Skipped);
        assert!(report
            .check(Check::Negation)
            .unwrap()
            .reason
            .contains("disabled by config"));
        assert_eq!(report.verdict, StatementVerdict::Indeterminate);
    }

    #[test]
    fn a_disabled_failing_check_cannot_reject() {
        // Triviality WOULD fail, but it is turned off: it must not gate.
        let (judge, neg, _) = all_good();
        let triv = FixedTrivial(ProofOutcome::Proved);
        let cfg = ValidityCfg {
            triviality_enabled: false,
            ..ValidityCfg::default()
        };
        let stack = StatementValidity::new(cfg)
            .with_judge(&judge)
            .with_negation_prover(&neg)
            .with_trivial_prover(&triv);
        let report = stack.screen(INFORMAL, FORMAL);
        assert!(report.failed().is_empty());
        assert_eq!(report.verdict, StatementVerdict::Indeterminate);
    }

    #[test]
    fn screening_is_deterministic_and_pure() {
        let (judge, neg, triv) = all_good();
        let stack = StatementValidity::default()
            .with_judge(&judge)
            .with_negation_prover(&neg)
            .with_trivial_prover(&triv);
        let a = stack.screen(INFORMAL, FORMAL);
        let b = stack.screen(INFORMAL, FORMAL);
        assert_eq!(a, b, "same seam answers ⇒ identical report");
    }

    #[test]
    fn the_stack_is_never_a_soundness_authority() {
        let stack = StatementValidity::default();
        assert!(
            !stack.is_soundness_authority(),
            "the formal gate remains the SOLE authority on proof validity"
        );
        // Its strongest verdict only advises skipping an ATTEMPT.
        assert!(StatementVerdict::Reject.blocks_attempt());
        assert!(!StatementVerdict::Reject.is_accept());
    }

    #[test]
    fn every_check_documents_what_it_screens() {
        for c in Check::ALL {
            assert!(!c.id().is_empty());
            assert!(!c.screens().is_empty(), "{} must document WHAT", c.id());
        }
        // Status tags are distinct — Skipped is visibly not Passed.
        assert_ne!(CheckStatus::Skipped.tag(), CheckStatus::Passed.tag());
    }
}
