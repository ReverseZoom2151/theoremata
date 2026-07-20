//! Staleness classification for previously-verified results (plan Phase 1).
//!
//! When the formal library moves underneath us, two things can have happened to
//! a result we once recorded as green, and today every system in the field
//! reports them identically:
//!
//! * a name moved, a lemma was relocated, or a tactic changed behaviour, so the
//!   mathematics is intact and the SCRIPT needs patching, or
//! * the mathematics itself moved, so the old green must be WITHDRAWN. Patching
//!   the script here would launder a claim that no longer says what it said.
//!
//! `docs/research/repair.md` section 3.2 is the source for the discriminator
//! used here: re-elaborate the pinned statement under the new environment and
//! compare the resulting type. The review found that operation checkable and
//! apparently unnamed as a technique; the human-supplied substitutes in the
//! field (Mathlib deprecation attributes, Rocq changelog markers, Coq overlays)
//! are assertions, not checks. See also `docs/PLAN-MATH-AT-SCALE.md` Phase 1.2.
//!
//! ## What this module is, and is not
//!
//! It is the pure decision logic: given an environment fingerprint comparison
//! and a re-elaboration outcome, decide what a result now is. It does NO IO.
//! That is deliberate on two counts. The fingerprint comes from the resolved
//! cache identity (plan Phase 0.1), which is being reshaped concurrently, and
//! the re-elaboration needs a live toolchain. Taking both as inputs makes the
//! decision testable today and wireable later without touching this file.
//!
//! ## The one invariant
//!
//! FAIL TOWARD [`StalenessVerdict::Unknown`], NEVER TOWARD
//! [`StalenessVerdict::Fresh`]. A result we could not assess is not fresh. The
//! collapse of unknown into fresh is the entire bug class this module exists to
//! prevent: `repair.md` section 4 records that the stale green is the field's
//! normal state precisely because nothing downgrades a claim when its
//! environment moves.
//!
//! ## Why the artifact class matters
//!
//! `repair.md` section 3.3: certificates do not rot, statements do. A
//! self-contained certificate over exact rationals (an SOS witness, an LRAT
//! refutation, a rational interval bound) replays as long as its checker
//! exists, and is immune to renames and tactic drift. So its PROOF needs no
//! recheck; only the MEANING of its statement is at risk. A tactic script is a
//! program against an API and rots on any drift, so it needs both. That routing
//! is where most of the saved work is (plan Phase 1.3).
//!
//! The residual risk the certificate does NOT cover is the miniF2F
//! `algebra_5778` case: Mathlib routed nth roots through `rpow`, the formal
//! text did not change by one character, and the statement became a different
//! mathematical claim. A certificate is still a valid object there and the
//! thing it supports has silently changed. That case must classify as
//! [`StalenessVerdict::MathematicsMoved`], and it is the reason the statement
//! recheck is unconditional across both artifact classes.

use std::collections::BTreeMap;

// ===========================================================================
// Inputs: what a caller must supply
// ===========================================================================

/// Fingerprint of the environment a result was elaborated against.
///
/// Opaque on purpose. This module only ever compares two of them for equality,
/// so whatever the cache identity ends up hashing (lake-manifest content hash,
/// toolchain, resolved package paths per plan Phase 0.1) is none of its
/// business. Comparison is exact: a fingerprint scheme that is coarser than the
/// environment is a soundness problem in the fingerprint, not here.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EnvironmentFingerprint(String);

impl EnvironmentFingerprint {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What kind of artifact backs a recorded verdict.
///
/// This is the routing input from `repair.md` 3.3. It changes how much has to
/// be rechecked, and nothing else: it never changes whether a result is stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArtifactClass {
    /// A self-contained certificate plus a checker whose correctness does not
    /// depend on the library: exact-rational bounds, Positivstellensatz or SOS
    /// witnesses, LRAT refutations. Replayable indefinitely, immune to renames.
    SelfContainedCertificate,
    /// A tactic script: a program against lemma names, simp sets, instance
    /// resolution and tactic implementations. Rots when any of those move.
    TacticScript,
    /// A stored kernel-checkable proof term. Sturdier than a script (it
    /// references declarations and their types, not an API surface) but not
    /// library-independent the way a certificate is, so its proof still has to
    /// be replayed. Treated as script-like for routing, and kept as its own
    /// variant so the census can tell the two apart.
    ProofTerm,
}

impl ArtifactClass {
    /// Whether the stored proof itself has to be rechecked when the environment
    /// moved, or whether only the statement does.
    ///
    /// `repair.md` 3.3: only the self-contained certificate earns the cheap
    /// route, and it earns it because its checker does not consult the library.
    pub fn recheck_scope(self) -> RecheckScope {
        match self {
            ArtifactClass::SelfContainedCertificate => RecheckScope::StatementOnly,
            ArtifactClass::TacticScript | ArtifactClass::ProofTerm => {
                RecheckScope::StatementAndProof
            }
        }
    }
}

/// How much work a recheck of this result implies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RecheckScope {
    /// Re-elaborate the pinned statement. The proof is durable and is not
    /// replayed.
    StatementOnly,
    /// Re-elaborate the pinned statement AND replay or re-run the proof.
    StatementAndProof,
}

/// The result of trying to re-elaborate the pinned statement text under the
/// CURRENT environment.
///
/// The distinction between [`Rejected`](Self::Rejected) and
/// [`Unavailable`](Self::Unavailable) is load-bearing and is the one place a
/// caller can silently break the invariant of this module. `Rejected` means the
/// elaborator ran and refused the statement: that is a real, checked negative
/// result, and it means the mathematics moved (a constant vanished, an instance
/// no longer resolves, a notation no longer parses). `Unavailable` means we
/// never got an answer: no toolchain, a timeout, a crash, a statement that
/// cannot be elaborated in isolation because its local definitions moved too.
/// No answer is not a negative answer, so it yields `Unknown`.
///
/// A caller that reports a timeout as `Rejected` will withdraw good results,
/// which is loud and recoverable. A caller that reports a rejection as
/// `Unavailable` gets `Unknown`, which is also safe. Neither mistake can
/// produce a `Fresh`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReelaborationOutcome {
    /// The statement elaborated. Carries the resulting type, in whatever
    /// normalized form the caller compares with (pretty-printed elaborated
    /// type, a hash of it, whatever the pin stores). This module compares the
    /// two strings exactly and does not attempt any notion of definitional
    /// equality: that check belongs to the toolchain, and a false "different"
    /// costs a withdrawal while a false "same" costs a laundered green.
    Elaborated { statement_type: String },
    /// The elaborator ran and refused the statement.
    Rejected { detail: String },
    /// We could not obtain an answer at all.
    Unavailable { reason: String },
}

/// A previously-verified result, as far as staleness is concerned.
///
/// `verified_against` and `pinned_statement_type` are exactly the two things
/// plan Phase 0 adds to the claim record (0.1 the resolved-environment
/// fingerprint, 0.2 the pinned elaborated statement type). `pinned_statement_type`
/// is an `Option` because results recorded before that pin exists cannot be
/// discriminated: they must read as `Unknown`, never as `Fresh` and never as a
/// repair task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedResult {
    /// Stable identifier of the claim, used only for reporting.
    pub id: String,
    pub artifact: ArtifactClass,
    /// The environment this result was actually elaborated against.
    pub verified_against: EnvironmentFingerprint,
    /// The elaborated type of the pinned statement at verification time.
    pub pinned_statement_type: Option<String>,
}

impl VerifiedResult {
    pub fn new(
        id: impl Into<String>,
        artifact: ArtifactClass,
        verified_against: EnvironmentFingerprint,
        pinned_statement_type: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            artifact,
            verified_against,
            pinned_statement_type,
        }
    }
}

// ===========================================================================
// Verdict
// ===========================================================================

/// Why a result could not be assessed.
///
/// Every variant here is a reason to look again, never a reason to trust the
/// old green. They are kept distinct so a sweep can report what is blocking it
/// rather than lumping everything into one bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnknownReason {
    /// The current environment could not be resolved or fingerprinted, so there
    /// is nothing to compare against. Not fresh: we do not know that the
    /// environment is the same, we only failed to look.
    EnvironmentUnresolved { detail: String },
    /// The environment moved and no pinned statement type was stored, so the
    /// discriminator cannot run. Plan Phase 0.2 is the fix; until then these
    /// results are simply unassessable.
    NoPinnedStatementType,
    /// The environment moved and re-elaboration produced no answer.
    ReelaborationUnavailable { reason: String },
}

/// Why a green is being withdrawn rather than repaired.
///
/// Carried by [`StalenessVerdict::MathematicsMoved`] so the withdrawal has a
/// citable cause in the audit trail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WithdrawalCause {
    /// The statement still elaborates, to a DIFFERENT type. The formal text may
    /// be byte-identical; that is the miniF2F `algebra_5778` shape
    /// (`repair.md` 3.2), and it is exactly the case a naive repair loop masks.
    StatementTypeChanged { pinned: String, current: String },
    /// The statement no longer elaborates at all under the new environment.
    StatementNoLongerElaborates { detail: String },
}

/// What a previously-verified result now is.
///
/// `#[must_use]`: dropping a verdict on the floor is how a stale green stays
/// green.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "a staleness verdict that is computed and discarded leaves a possibly stale green in place"]
pub enum StalenessVerdict {
    /// The environment fingerprint matches. Nothing to do.
    ///
    /// This is the ONLY variant that means "still good", and it is reachable
    /// only from an exact fingerprint match against a resolved current
    /// environment. Every other path in [`assess`] lands elsewhere.
    Fresh,
    /// The environment moved and the pinned statement re-elaborates to the SAME
    /// type. The mathematics is intact, so any failure is a rename, a moved
    /// lemma, or tactic drift. This, and only this, is a repair task.
    RepairCandidate(RepairPlan),
    /// The environment moved and the statement's meaning moved with it. The old
    /// verdict must be WITHDRAWN, not patched.
    ///
    /// `repair.md` 1.2 (REPLica): roughly 75 percent of human "proof fixes"
    /// were fixes to the specification or program, not the proof. Automating a
    /// script repair here optimizes the minority case and papers over the
    /// majority one.
    MathematicsMoved(Withdrawal),
    /// The result could not be assessed. Distinct from `Fresh`, permanently.
    Unknown(UnknownReason),
}

/// The repair work implied by a [`StalenessVerdict::RepairCandidate`].
///
/// Only constructible by [`assess`], so a `RepairPlan` in hand is evidence that
/// the statement re-elaborated to the same type. That is the point: a caller
/// cannot manufacture a repair task for a result whose mathematics moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairPlan {
    id: String,
    artifact: ArtifactClass,
    scope: RecheckScope,
    /// The type both the pin and the re-elaboration agree on.
    confirmed_statement_type: String,
}

impl RepairPlan {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn artifact(&self) -> ArtifactClass {
        self.artifact
    }

    /// How much has to be rechecked. `StatementOnly` for a self-contained
    /// certificate, and the statement recheck has already succeeded by the time
    /// a `RepairPlan` exists, so a `StatementOnly` plan implies no work at all
    /// beyond re-pinning the environment. See [`RepairPlan::is_already_satisfied`].
    pub fn scope(&self) -> RecheckScope {
        self.scope
    }

    /// The elaborated statement type, confirmed unchanged.
    pub fn confirmed_statement_type(&self) -> &str {
        &self.confirmed_statement_type
    }

    /// True when nothing further needs running: the artifact is a self-contained
    /// certificate, so its proof is durable, and its statement has just been
    /// confirmed to mean the same thing. The result can be re-pinned to the new
    /// environment without touching the prover. This is the saved work from
    /// plan Phase 1.3.
    pub fn is_already_satisfied(&self) -> bool {
        self.scope == RecheckScope::StatementOnly
    }
}

/// The withdrawal implied by a [`StalenessVerdict::MathematicsMoved`].
///
/// Deliberately carries no repair affordance. There is no method on this type
/// that yields a script to patch, because patching is the wrong action: the
/// theorem no longer says what it said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Withdrawal {
    id: String,
    artifact: ArtifactClass,
    cause: WithdrawalCause,
}

impl Withdrawal {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn artifact(&self) -> ArtifactClass {
        self.artifact
    }

    pub fn cause(&self) -> &WithdrawalCause {
        &self.cause
    }

    /// One-line explanation suitable for an audit record.
    pub fn explain(&self) -> String {
        match &self.cause {
            WithdrawalCause::StatementTypeChanged { pinned, current } => format!(
                "{}: statement re-elaborates to a different type (pinned `{}`, now `{}`); \
                 the verdict is withdrawn, not repaired",
                self.id, pinned, current
            ),
            WithdrawalCause::StatementNoLongerElaborates { detail } => format!(
                "{}: statement no longer elaborates ({}); the verdict is withdrawn, not repaired",
                self.id, detail
            ),
        }
    }
}

/// The action a caller is permitted to take, given a verdict.
///
/// This is the loud API surface the design asks for. A caller that wants repair
/// work must go through [`StalenessVerdict::into_action`] and match on
/// [`StalenessAction`], where `Withdraw` and `Escalate` are separate arms that
/// cannot be mistaken for repair. There is no `unwrap_repair`, no `Into<RepairPlan>`,
/// and no `Default`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "the whole point of classifying staleness is to act on the classification"]
pub enum StalenessAction {
    /// Fresh. Do nothing.
    None,
    /// The mathematics is intact. Repair, within the scope of the plan.
    Repair(RepairPlan),
    /// The mathematics moved. Withdraw the green. Do not repair.
    Withdraw(Withdrawal),
    /// Unassessable. Escalate, and in the meantime the result is not verified.
    Escalate(UnknownReason),
}

impl StalenessVerdict {
    /// True only for [`StalenessVerdict::Fresh`].
    ///
    /// Written as an explicit match rather than a `!matches!(.., Unknown)` so
    /// that adding a future variant is a compile error here instead of a silent
    /// new road to "fresh".
    pub fn is_fresh(&self) -> bool {
        match self {
            StalenessVerdict::Fresh => true,
            StalenessVerdict::RepairCandidate(_)
            | StalenessVerdict::MathematicsMoved(_)
            | StalenessVerdict::Unknown(_) => false,
        }
    }

    /// True when the result may no longer be treated as verified. Everything
    /// that is not `Fresh` qualifies, including `Unknown`.
    pub fn needs_attention(&self) -> bool {
        !self.is_fresh()
    }

    /// True only for [`StalenessVerdict::RepairCandidate`]. `MathematicsMoved`
    /// answers false here, which is the whole point.
    pub fn is_repairable(&self) -> bool {
        matches!(self, StalenessVerdict::RepairCandidate(_))
    }

    /// The withdrawal, if this verdict is one. `None` for every other variant.
    pub fn withdrawal(&self) -> Option<&Withdrawal> {
        match self {
            StalenessVerdict::MathematicsMoved(w) => Some(w),
            _ => None,
        }
    }

    /// Convert to the exhaustive action enum. This is the intended consumption
    /// path: it forces a caller to write a `Withdraw` arm.
    pub fn into_action(self) -> StalenessAction {
        match self {
            StalenessVerdict::Fresh => StalenessAction::None,
            StalenessVerdict::RepairCandidate(plan) => StalenessAction::Repair(plan),
            StalenessVerdict::MathematicsMoved(w) => StalenessAction::Withdraw(w),
            StalenessVerdict::Unknown(r) => StalenessAction::Escalate(r),
        }
    }

    /// Short census bucket name, used by [`StalenessReport`].
    pub fn bucket(&self) -> &'static str {
        match self {
            StalenessVerdict::Fresh => "fresh",
            StalenessVerdict::RepairCandidate(_) => "repair_candidate",
            StalenessVerdict::MathematicsMoved(_) => "mathematics_moved",
            StalenessVerdict::Unknown(_) => "unknown",
        }
    }
}

// ===========================================================================
// The decision
// ===========================================================================

/// Classify one previously-verified result.
///
/// `current_environment` is `None` when the environment could not be resolved
/// or fingerprinted at all. `reelaboration` is `None` when re-elaboration was
/// not attempted, which is the normal case for a result whose fingerprint still
/// matches: there is no point paying for it. If the fingerprint moved and no
/// re-elaboration was supplied, the answer is `Unknown`, never `Fresh`.
///
/// Order of the checks is the invariant. Every early return that cannot
/// establish freshness returns `Unknown` or a stronger verdict, and `Fresh` is
/// returned from exactly one place.
pub fn assess(
    result: &VerifiedResult,
    current_environment: Option<&EnvironmentFingerprint>,
    reelaboration: Option<&ReelaborationOutcome>,
) -> StalenessVerdict {
    // 1. Can we even see the current environment? If not, we have learned
    //    nothing. Failing to look is not the same as looking and finding no
    //    change, and conflating the two is the stale-green bug (`repair.md` 4).
    let current = match current_environment {
        Some(fp) => fp,
        None => {
            return StalenessVerdict::Unknown(UnknownReason::EnvironmentUnresolved {
                detail: "current environment fingerprint could not be resolved".to_string(),
            })
        }
    };

    // 2. Fingerprint match. Sound and coarse, exactly as plan Phase 1.1 says:
    //    build-trace style comparison over-approximates staleness (a docstring
    //    edit upstream marks us stale) but never under-approximates it, so a
    //    match is the only thing that licenses `Fresh`.
    if result.verified_against == *current {
        return StalenessVerdict::Fresh;
    }

    // 3. The environment moved. From here the result is NOT fresh under any
    //    branch; the only question is which non-fresh verdict it earns.
    let pinned = match result.pinned_statement_type.as_deref() {
        Some(t) => t,
        // Plan Phase 0.2 was not in force when this result was recorded. No pin
        // means no discriminator, and no discriminator means we cannot claim
        // the mathematics survived. Not a repair candidate: repairing a script
        // whose statement we never pinned is exactly how an LLM loop weakens a
        // theorem until it compiles (`repair.md` 1.6).
        None => return StalenessVerdict::Unknown(UnknownReason::NoPinnedStatementType),
    };

    let outcome = match reelaboration {
        Some(o) => o,
        None => {
            return StalenessVerdict::Unknown(UnknownReason::ReelaborationUnavailable {
                reason: "re-elaboration was not attempted".to_string(),
            })
        }
    };

    match outcome {
        // 4. The discriminator (`repair.md` 3.2, plan Phase 1.2).
        ReelaborationOutcome::Elaborated { statement_type } => {
            if statement_type == pinned {
                StalenessVerdict::RepairCandidate(RepairPlan {
                    id: result.id.clone(),
                    artifact: result.artifact,
                    scope: result.artifact.recheck_scope(),
                    confirmed_statement_type: statement_type.clone(),
                })
            } else {
                // The `algebra_5778` shape: the source text can be identical
                // and the elaborated type different, because the library
                // redefined what the notation means. Loudest verdict we have.
                StalenessVerdict::MathematicsMoved(Withdrawal {
                    id: result.id.clone(),
                    artifact: result.artifact,
                    cause: WithdrawalCause::StatementTypeChanged {
                        pinned: pinned.to_string(),
                        current: statement_type.clone(),
                    },
                })
            }
        }
        // 5. A checked negative. The elaborator ran and refused: a constant it
        //    named is gone, an instance no longer resolves, a notation no
        //    longer parses. The statement as written no longer denotes the
        //    thing it denoted, so the green does not survive.
        ReelaborationOutcome::Rejected { detail } => {
            StalenessVerdict::MathematicsMoved(Withdrawal {
                id: result.id.clone(),
                artifact: result.artifact,
                cause: WithdrawalCause::StatementNoLongerElaborates {
                    detail: detail.clone(),
                },
            })
        }
        // 6. No answer. Plan "honest risks": a statement that depends on local
        //    definitions which also moved degrades the discriminator to
        //    unknown, and unknown must stay distinct from clean.
        ReelaborationOutcome::Unavailable { reason } => {
            StalenessVerdict::Unknown(UnknownReason::ReelaborationUnavailable {
                reason: reason.clone(),
            })
        }
    }
}

/// Whether a result is worth spending a re-elaboration on at all.
///
/// A caller sweeping a large store uses this to skip the expensive step for
/// results whose fingerprint still matches. It answers `false` ONLY on an exact
/// match against a resolved environment, so an unresolvable environment still
/// costs the check rather than being waved through.
pub fn needs_reelaboration(
    result: &VerifiedResult,
    current_environment: Option<&EnvironmentFingerprint>,
) -> bool {
    match current_environment {
        Some(fp) => result.verified_against != *fp,
        None => true,
    }
}

// ===========================================================================
// Census
// ===========================================================================

/// One classified result, kept alongside its verdict for reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssessedResult {
    pub id: String,
    pub artifact: ArtifactClass,
    pub verdict: StalenessVerdict,
}

/// The honest census of plan Phase 1.4: a sweep marks nodes and reports the
/// split. It is not a re-prove loop, and it deliberately does not aggregate
/// `MathematicsMoved` and `RepairCandidate` into a single "broken" number,
/// because that aggregation is the reporting failure the whole phase is about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StalenessReport {
    entries: Vec<AssessedResult>,
}

/// Counts by bucket. `fresh + repair_candidate + mathematics_moved + unknown`
/// always equals the number of assessed results.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Census {
    pub fresh: usize,
    pub repair_candidate: usize,
    pub mathematics_moved: usize,
    pub unknown: usize,
}

impl Census {
    pub fn total(&self) -> usize {
        self.fresh + self.repair_candidate + self.mathematics_moved + self.unknown
    }

    /// Everything that is no longer a usable green. `Unknown` is counted here,
    /// which is the point: an unassessable result is not a verified one.
    pub fn not_verified(&self) -> usize {
        self.repair_candidate + self.mathematics_moved + self.unknown
    }
}

impl StalenessReport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one classification.
    pub fn record(&mut self, id: impl Into<String>, artifact: ArtifactClass, verdict: StalenessVerdict) {
        self.entries.push(AssessedResult {
            id: id.into(),
            artifact,
            verdict,
        });
    }

    /// Classify and record in one step.
    pub fn assess_and_record(
        &mut self,
        result: &VerifiedResult,
        current_environment: Option<&EnvironmentFingerprint>,
        reelaboration: Option<&ReelaborationOutcome>,
    ) {
        let verdict = assess(result, current_environment, reelaboration);
        self.record(result.id.clone(), result.artifact, verdict);
    }

    pub fn entries(&self) -> &[AssessedResult] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn census(&self) -> Census {
        let mut c = Census::default();
        for e in &self.entries {
            match e.verdict {
                StalenessVerdict::Fresh => c.fresh += 1,
                StalenessVerdict::RepairCandidate(_) => c.repair_candidate += 1,
                StalenessVerdict::MathematicsMoved(_) => c.mathematics_moved += 1,
                StalenessVerdict::Unknown(_) => c.unknown += 1,
            }
        }
        c
    }

    /// Every withdrawal in the sweep. These are the loud ones: each is a green
    /// that has to come down.
    pub fn withdrawals(&self) -> Vec<&Withdrawal> {
        self.entries
            .iter()
            .filter_map(|e| e.verdict.withdrawal())
            .collect()
    }

    /// Every repair task in the sweep, with its scope.
    pub fn repair_plans(&self) -> Vec<&RepairPlan> {
        self.entries
            .iter()
            .filter_map(|e| match &e.verdict {
                StalenessVerdict::RepairCandidate(p) => Some(p),
                _ => None,
            })
            .collect()
    }

    /// Repair candidates that need no prover work at all, because their
    /// artifact is a self-contained certificate and their statement already
    /// re-elaborated to the same type. This is the measured version of plan
    /// Phase 1.3's claim about where the saved work is.
    pub fn statement_only_repairs(&self) -> usize {
        self.repair_plans()
            .iter()
            .filter(|p| p.is_already_satisfied())
            .count()
    }

    /// Non-fresh counts split by artifact class, so a sweep can say which kinds
    /// of artifact are carrying the drift.
    pub fn drift_by_artifact(&self) -> BTreeMap<ArtifactClass, usize> {
        let mut m = BTreeMap::new();
        for e in &self.entries {
            if e.verdict.needs_attention() {
                *m.entry(e.artifact).or_insert(0) += 1;
            }
        }
        m
    }

    /// Human-readable one-liner for a CLI sweep.
    pub fn summary(&self) -> String {
        let c = self.census();
        format!(
            "{} assessed: {} fresh, {} repair candidates, {} withdrawn (mathematics moved), {} unknown",
            c.total(),
            c.fresh,
            c.repair_candidate,
            c.mathematics_moved,
            c.unknown
        )
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn env(s: &str) -> EnvironmentFingerprint {
        EnvironmentFingerprint::new(s)
    }

    fn script_result(id: &str, pinned: Option<&str>) -> VerifiedResult {
        VerifiedResult::new(
            id,
            ArtifactClass::TacticScript,
            env("mathlib@old"),
            pinned.map(|s| s.to_string()),
        )
    }

    fn cert_result(id: &str, pinned: Option<&str>) -> VerifiedResult {
        VerifiedResult::new(
            id,
            ArtifactClass::SelfContainedCertificate,
            env("mathlib@old"),
            pinned.map(|s| s.to_string()),
        )
    }

    fn elaborated(t: &str) -> ReelaborationOutcome {
        ReelaborationOutcome::Elaborated {
            statement_type: t.to_string(),
        }
    }

    #[test]
    fn matching_fingerprint_is_fresh() {
        let r = script_result("t1", Some("T"));
        let v = assess(&r, Some(&env("mathlib@old")), None);
        assert_eq!(v, StalenessVerdict::Fresh);
        assert!(v.is_fresh());
        assert!(!v.needs_attention());
    }

    #[test]
    fn same_type_under_new_environment_is_a_repair_candidate() {
        let r = script_result("t2", Some("forall x : Real, 0 <= x"));
        let v = assess(
            &r,
            Some(&env("mathlib@new")),
            Some(&elaborated("forall x : Real, 0 <= x")),
        );
        assert!(v.is_repairable());
        assert!(v.needs_attention());
        assert!(v.withdrawal().is_none());
        match v.into_action() {
            StalenessAction::Repair(p) => {
                assert_eq!(p.id(), "t2");
                assert_eq!(p.scope(), RecheckScope::StatementAndProof);
            }
            other => panic!("expected repair, got {other:?}"),
        }
    }

    #[test]
    fn different_type_under_new_environment_is_mathematics_moved() {
        let r = script_result("t3", Some("pinned type"));
        let v = assess(&r, Some(&env("mathlib@new")), Some(&elaborated("other type")));
        assert!(!v.is_repairable());
        assert!(v.withdrawal().is_some());
        match v.into_action() {
            StalenessAction::Withdraw(w) => {
                assert!(matches!(
                    w.cause(),
                    WithdrawalCause::StatementTypeChanged { .. }
                ));
            }
            other => panic!("expected withdrawal, got {other:?}"),
        }
    }

    #[test]
    fn statement_that_no_longer_elaborates_is_mathematics_moved() {
        let r = script_result("t4", Some("pinned type"));
        let v = assess(
            &r,
            Some(&env("mathlib@new")),
            Some(&ReelaborationOutcome::Rejected {
                detail: "unknown identifier 'Real.rpow_natCast'".to_string(),
            }),
        );
        let w = v.withdrawal().expect("withdrawal");
        assert!(matches!(
            w.cause(),
            WithdrawalCause::StatementNoLongerElaborates { .. }
        ));
        assert!(w.explain().contains("withdrawn, not repaired"));
    }

    #[test]
    fn every_unassessable_path_is_unknown_and_never_fresh() {
        // No resolvable environment.
        let a = assess(&script_result("u1", Some("T")), None, Some(&elaborated("T")));
        // Environment moved, nothing pinned.
        let b = assess(
            &script_result("u2", None),
            Some(&env("mathlib@new")),
            Some(&elaborated("T")),
        );
        // Environment moved, re-elaboration not attempted.
        let c = assess(&script_result("u3", Some("T")), Some(&env("mathlib@new")), None);
        // Environment moved, re-elaboration produced no answer.
        let d = assess(
            &script_result("u4", Some("T")),
            Some(&env("mathlib@new")),
            Some(&ReelaborationOutcome::Unavailable {
                reason: "toolchain timeout".to_string(),
            }),
        );

        for v in [&a, &b, &c, &d] {
            assert!(matches!(v, StalenessVerdict::Unknown(_)), "got {v:?}");
            assert!(!v.is_fresh(), "unknown must never read as fresh: {v:?}");
            assert!(v.needs_attention());
            assert!(!v.is_repairable());
            assert!(v.withdrawal().is_none());
        }

        assert!(matches!(
            a,
            StalenessVerdict::Unknown(UnknownReason::EnvironmentUnresolved { .. })
        ));
        assert!(matches!(
            b,
            StalenessVerdict::Unknown(UnknownReason::NoPinnedStatementType)
        ));
        assert!(matches!(
            c,
            StalenessVerdict::Unknown(UnknownReason::ReelaborationUnavailable { .. })
        ));
        assert!(matches!(
            d,
            StalenessVerdict::Unknown(UnknownReason::ReelaborationUnavailable { .. })
        ));
    }

    #[test]
    fn an_unresolvable_environment_does_not_read_as_fresh_even_on_an_equal_pin() {
        // The trap: the result was verified against exactly the environment we
        // would have found, but we could not resolve it, so we do not know that.
        let r = script_result("u5", Some("T"));
        assert!(!assess(&r, None, None).is_fresh());
        assert!(needs_reelaboration(&r, None));
    }

    #[test]
    fn certificate_and_script_with_the_same_staleness_route_differently() {
        let cur = env("mathlib@new");
        let same = elaborated("same type");

        let cert = assess(&cert_result("c1", Some("same type")), Some(&cur), Some(&same));
        let script = assess(&script_result("s1", Some("same type")), Some(&cur), Some(&same));

        // Same staleness classification.
        assert!(cert.is_repairable() && script.is_repairable());

        let (cert_plan, script_plan) = match (cert.into_action(), script.into_action()) {
            (StalenessAction::Repair(a), StalenessAction::Repair(b)) => (a, b),
            other => panic!("expected two repairs, got {other:?}"),
        };

        // Different routing: the certificate needs only its statement rechecked,
        // which already happened, so it is done. `repair.md` 3.3.
        assert_eq!(cert_plan.scope(), RecheckScope::StatementOnly);
        assert!(cert_plan.is_already_satisfied());
        assert_eq!(script_plan.scope(), RecheckScope::StatementAndProof);
        assert!(!script_plan.is_already_satisfied());
    }

    #[test]
    fn a_certificate_whose_statement_moved_is_still_withdrawn() {
        // The residual rot a certificate does NOT protect against: its proof is
        // durable, the MEANING of its statement is not.
        let v = assess(
            &cert_result("c2", Some("pinned type")),
            Some(&env("mathlib@new")),
            Some(&elaborated("different type")),
        );
        assert!(!v.is_repairable());
        assert_eq!(
            v.withdrawal().map(|w| w.artifact()),
            Some(ArtifactClass::SelfContainedCertificate)
        );
    }

    #[test]
    fn algebra_5778_identical_text_different_elaborated_type_is_not_a_repair_task() {
        // miniF2F `algebra_5778`: Mathlib routed nth roots through `rpow`, the
        // formal statement text did not change by one character, and the claim
        // became a different (and unprovable) one. `repair.md` 3.2.
        const STATEMENT_TEXT: &str = "theorem algebra_5778 (x : Real) : x ^ ((1:Real)/3) = 2";

        let pinned_type = "forall (x : Real), Real.nnrpow x (1/3) = 2";
        let current_type = "forall (x : Real), Real.rpow x (1/3) = 2";
        assert_ne!(pinned_type, current_type);

        let r = VerifiedResult::new(
            STATEMENT_TEXT,
            ArtifactClass::TacticScript,
            env("mathlib@2023"),
            Some(pinned_type.to_string()),
        );
        let v = assess(
            &r,
            Some(&env("mathlib@2026")),
            Some(&elaborated(current_type)),
        );

        assert!(
            !v.is_repairable(),
            "identical source text must not license a script repair when the type moved"
        );
        match v {
            StalenessVerdict::MathematicsMoved(w) => match w.cause() {
                WithdrawalCause::StatementTypeChanged { pinned, current } => {
                    assert_eq!(pinned, pinned_type);
                    assert_eq!(current, current_type);
                }
                other => panic!("expected a type change, got {other:?}"),
            },
            other => panic!("expected MathematicsMoved, got {other:?}"),
        }
    }

    #[test]
    fn needs_reelaboration_only_skips_an_exact_match() {
        let r = script_result("n1", Some("T"));
        assert!(!needs_reelaboration(&r, Some(&env("mathlib@old"))));
        assert!(needs_reelaboration(&r, Some(&env("mathlib@new"))));
        assert!(needs_reelaboration(&r, None));
    }

    #[test]
    fn census_counts_each_bucket_and_totals_correctly() {
        let old = env("mathlib@old");
        let new = env("mathlib@new");
        let mut report = StalenessReport::new();

        // 2 fresh.
        report.assess_and_record(&script_result("f1", Some("T")), Some(&old), None);
        report.assess_and_record(&cert_result("f2", Some("T")), Some(&old), None);
        // 3 repair candidates, one of them a certificate (statement only).
        report.assess_and_record(&script_result("r1", Some("T")), Some(&new), Some(&elaborated("T")));
        report.assess_and_record(&script_result("r2", Some("T")), Some(&new), Some(&elaborated("T")));
        report.assess_and_record(&cert_result("r3", Some("T")), Some(&new), Some(&elaborated("T")));
        // 1 mathematics moved.
        report.assess_and_record(&script_result("m1", Some("T")), Some(&new), Some(&elaborated("U")));
        // 2 unknown.
        report.assess_and_record(&script_result("k1", None), Some(&new), Some(&elaborated("T")));
        report.assess_and_record(&script_result("k2", Some("T")), None, None);

        let c = report.census();
        assert_eq!(c.fresh, 2);
        assert_eq!(c.repair_candidate, 3);
        assert_eq!(c.mathematics_moved, 1);
        assert_eq!(c.unknown, 2);
        assert_eq!(c.total(), 8);
        assert_eq!(c.total(), report.len());
        assert_eq!(c.not_verified(), 6);

        assert_eq!(report.withdrawals().len(), 1);
        assert_eq!(report.repair_plans().len(), 3);
        assert_eq!(report.statement_only_repairs(), 1);

        let drift = report.drift_by_artifact();
        assert_eq!(drift.get(&ArtifactClass::TacticScript), Some(&5));
        assert_eq!(drift.get(&ArtifactClass::SelfContainedCertificate), Some(&1));

        assert!(report.summary().contains("8 assessed"));
    }

    #[test]
    fn census_never_folds_unknown_into_fresh() {
        let mut report = StalenessReport::new();
        for (i, outcome) in [
            ReelaborationOutcome::Unavailable {
                reason: "no toolchain".to_string(),
            },
            ReelaborationOutcome::Unavailable {
                reason: "timeout".to_string(),
            },
        ]
        .into_iter()
        .enumerate()
        {
            let r = script_result(&format!("u{i}"), Some("T"));
            report.assess_and_record(&r, Some(&env("mathlib@new")), Some(&outcome));
        }
        let c = report.census();
        assert_eq!(c.fresh, 0);
        assert_eq!(c.unknown, 2);
        assert_eq!(c.not_verified(), 2);
    }

    #[test]
    fn proof_terms_route_like_scripts_but_are_counted_separately() {
        let r = VerifiedResult::new(
            "p1",
            ArtifactClass::ProofTerm,
            env("mathlib@old"),
            Some("T".to_string()),
        );
        let v = assess(&r, Some(&env("mathlib@new")), Some(&elaborated("T")));
        match v.into_action() {
            StalenessAction::Repair(p) => {
                assert_eq!(p.scope(), RecheckScope::StatementAndProof);
                assert_eq!(p.artifact(), ArtifactClass::ProofTerm);
                assert_eq!(p.confirmed_statement_type(), "T");
            }
            other => panic!("expected repair, got {other:?}"),
        }
    }

    #[test]
    fn buckets_are_stable_names() {
        assert_eq!(StalenessVerdict::Fresh.bucket(), "fresh");
        assert_eq!(
            StalenessVerdict::Unknown(UnknownReason::NoPinnedStatementType).bucket(),
            "unknown"
        );
    }
}
