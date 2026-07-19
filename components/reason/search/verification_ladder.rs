//! **The verification ladder** (Tier 3, items 20–21): an explicit *cost
//! hierarchy* of checks, plus the validity trichotomy the pipeline currently
//! lacks.
//!
//! Today's falsify-before-prove routing ([`super::super::proving::falsification`])
//! is one hand-wired instance of a general idea: run the *cheap* check first, and
//! only pay for the kernel when the cheap check could not settle the question.
//! This module generalizes that into an ordered ladder —
//! `fast_refute -> cheap_decide -> expensive_kernel` — where every rung is an
//! injected trait seam, so numeric spot checks, random model evaluation,
//! property-based testing, SMT probes, and the real proof kernel all plug into
//! the same control flow.
//!
//! # The soundness rule (enforced by the types)
//!
//! **A cheap rung may only REFUTE or ABSTAIN. It may NEVER verify. Only the
//! kernel rung can produce a positive verdict.**
//!
//! This is not a convention, a lint, or a runtime assertion — it is structural.
//! A [`CheapRung`] returns [`RungVerdict`], whose *only* variants are
//! [`RungVerdict::Refuted`] and [`RungVerdict::Abstain`]; there is no `Verified`
//! variant for it to construct. Only [`KernelRung`] returns [`KernelVerdict`],
//! which is the sole type in this module carrying [`KernelVerdict::Verified`].
//! Consequently an unsound-but-cheap probe is a *legitimate* pre-filter: a
//! bounded numeric sweep that finds no counterexample has said nothing, and the
//! type system will not let it say otherwise. `∀`-claims are refutable by one
//! instance but never provable by finitely many, and the ladder encodes exactly
//! that asymmetry.
//!
//! The ladder short-circuits on the first refutation: once a rung refutes, the
//! kernel is **not consulted** (there is nothing left to prove, and the expensive
//! call is pure waste). Abstentions fall through to the next rung, and an
//! all-abstain ladder ends at the kernel.
//!
//! # Refutation witnesses, not booleans
//!
//! A refuting rung must hand back a **structured** [`RefutationWitness`] — the
//! offending instance, the conflicting hypothesis set — because *the witness is
//! what routes the repair*. "false" tells the repair loop nothing; `n = 4`
//! violating the claim tells it which case the statement forgot, and a conflict
//! set tells it which hypotheses to weaken. A bare boolean throws that signal
//! away at exactly the moment it is most valuable.
//!
//! # The validity trichotomy
//!
//! [`Validity`] is `Correct | Incorrect | Undecodable`. The third case is the one
//! usually collapsed into the second, wrongly: a candidate that failed to
//! **parse** is categorically different from one the checker **rejected**, and
//! they route differently — see [`Validity::routing`]:
//!
//! * [`Validity::Undecodable`] ⇒ [`Routing::Repair`]. The reasoning was never
//!   evaluated. Re-emit / reformat / re-parse; pruning here discards candidates
//!   whose mathematics may have been fine.
//! * [`Validity::Incorrect`] ⇒ [`Routing::Prune`]. The content *was* evaluated
//!   and found wrong. Repairing the format cannot save it; drop the branch (and
//!   feed the witness back).
//! * [`Validity::Correct`] ⇒ [`Routing::Certify`], and only ever from the kernel.
//!
//! [`well_formed_rate`] is the fraction of candidates that at least decode. It is
//! deliberately separate from any accuracy metric: a drop in well-formed rate is
//! an **output-format** regression (a prompt/template/decoder change), while a
//! drop in accuracy at a flat well-formed rate is a **reasoning** regression.
//! Folding undecodables into "incorrect" makes those two failure modes
//! indistinguishable on a dashboard.
//!
//! # Purity
//!
//! No IO, no clock, no RNG in this module. Budgets are abstract step/instance
//! counts, not durations, so a run is reproducible; anything that touches the
//! world lives behind the [`CheapRung`] / [`KernelRung`] / [`Decoder`] seams.
//! With [`LadderConfig::default`] **every cheap rung is disabled**, so a ladder
//! configured with no rungs behaves identically to calling the kernel directly.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Refutation witnesses
// ---------------------------------------------------------------------------

/// What kind of refutation a rung found. Coarse, because the routing decision is
/// coarse; the payload in [`RefutationWitness`] carries the detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WitnessKind {
    /// A concrete assignment falsifying a universally quantified claim (the
    /// numeric-sweep / random-model-evaluation case).
    Counterexample,
    /// A minimal-ish set of mutually unsatisfiable hypotheses (the SMT / clause
    /// case). Routes to hypothesis weakening rather than statement repair.
    ConflictSet,
    /// A property-based test that shrank to a failing input.
    PropertyViolation,
    /// A structural/typing contradiction found without executing anything (e.g.
    /// arity or sort mismatch).
    StructuralConflict,
}

/// A **structured** refutation: what refuted the candidate, and enough detail to
/// route the repair. Never a bare boolean — see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefutationWitness {
    /// Name of the rung that produced this witness (provenance for audit).
    pub rung: String,
    /// The shape of the refutation.
    pub kind: WitnessKind,
    /// One-line human summary, for logs and for prompting the repair model.
    pub summary: String,
    /// The offending instance, as ordered `(variable, value)` bindings. Ordered
    /// (a `Vec`, not a map) so serialization and test assertions are
    /// deterministic.
    pub instance: Vec<(String, String)>,
    /// The conflicting hypotheses / clauses, when the refutation is a conflict
    /// rather than a single instance.
    pub conflict: Vec<String>,
}

impl RefutationWitness {
    /// A counterexample witness from `bindings`.
    pub fn counterexample(
        rung: impl Into<String>,
        summary: impl Into<String>,
        bindings: Vec<(String, String)>,
    ) -> Self {
        Self {
            rung: rung.into(),
            kind: WitnessKind::Counterexample,
            summary: summary.into(),
            instance: bindings,
            conflict: Vec::new(),
        }
    }

    /// A conflict-set witness naming the mutually unsatisfiable hypotheses.
    pub fn conflict_set(
        rung: impl Into<String>,
        summary: impl Into<String>,
        conflict: Vec<String>,
    ) -> Self {
        Self {
            rung: rung.into(),
            kind: WitnessKind::ConflictSet,
            summary: summary.into(),
            instance: Vec::new(),
            conflict,
        }
    }

    /// Whether this witness carries any structured payload at all. A witness with
    /// neither an instance nor a conflict set is degenerate — it refutes but
    /// cannot route a repair — and callers may want to flag it.
    pub fn is_actionable(&self) -> bool {
        !self.instance.is_empty() || !self.conflict.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Verdicts — the soundness rule, in the type system
// ---------------------------------------------------------------------------

/// The verdict a **cheap** rung may return.
///
/// There is deliberately **no** `Verified` variant. A cheap rung is structurally
/// incapable of certifying: it can produce a refutation (with a witness) or it
/// can decline to answer. Adding a positive variant here would silently promote
/// every unsound probe into a certifier — do not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RungVerdict {
    /// Refuted, with the structured witness that routes the repair.
    Refuted(RefutationWitness),
    /// No opinion. The candidate falls through to the next rung.
    Abstain,
}

/// The verdict the **kernel** rung may return. This is the only type in the
/// module with a positive variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum KernelVerdict {
    /// Certified by the kernel. The single source of positive verdicts.
    Verified,
    /// The kernel evaluated the candidate and rejected it (a real error from the
    /// checker, not a parse failure).
    Rejected(String),
    /// The kernel did not settle it — budget exhausted, timeout at the seam,
    /// unavailable toolchain. Explicitly *not* a rejection.
    Abstain,
}

// ---------------------------------------------------------------------------
// Rung seams
// ---------------------------------------------------------------------------

/// Which tier of the ladder a rung sits in, in ascending cost order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RungTier {
    /// Cheapest: refutation-only probes meant to kill obviously-false candidates
    /// (bounded numeric sweeps, random model evaluation, type/arity checks).
    FastRefute,
    /// Middle: heavier decision procedures still run only as refuters
    /// (property-based testing with shrinking, bounded SMT, finite models).
    CheapDecide,
    /// Most expensive: the proof kernel. The only tier that can verify.
    Kernel,
}

impl RungTier {
    /// Stable label for logs and traces.
    pub fn as_str(&self) -> &'static str {
        match self {
            RungTier::FastRefute => "fast_refute",
            RungTier::CheapDecide => "cheap_decide",
            RungTier::Kernel => "expensive_kernel",
        }
    }
}

/// Abstract, clock-free budget for one rung. Counts, not durations, so a run is
/// reproducible. A rung is free to interpret these in its own units; `0` means
/// "unbounded" only if the rung documents it so — the ladder never inspects them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RungBudget {
    /// Maximum internal steps (sweep iterations, solver conflicts, tactic steps).
    pub max_steps: u64,
    /// Maximum candidate instances to evaluate (samples, test cases, models).
    pub max_instances: u64,
}

impl RungBudget {
    /// A budget with both limits set.
    pub fn new(max_steps: u64, max_instances: u64) -> Self {
        Self {
            max_steps,
            max_instances,
        }
    }
}

impl Default for RungBudget {
    /// Modest defaults suitable for a screening probe.
    fn default() -> Self {
        Self {
            max_steps: 1_000,
            max_instances: 100,
        }
    }
}

/// A cheap rung: **refute or abstain**, never verify.
///
/// `C` is the caller's candidate type (a proof source, a goal state, a
/// statement + model). The trait is generic rather than fixed to one struct so
/// the ladder can wrap any stage of the pipeline.
pub trait CheapRung<C: ?Sized> {
    /// Stable rung name, used for provenance in the trace and the witness.
    fn name(&self) -> &str;

    /// Probe `candidate` within `budget`. Returning [`RungVerdict::Abstain`] is
    /// always sound; returning [`RungVerdict::Refuted`] asserts a *real*
    /// refutation backed by the witness.
    fn probe(&self, candidate: &C, budget: &RungBudget) -> RungVerdict;
}

/// The kernel rung: the only seam that can certify.
pub trait KernelRung<C: ?Sized> {
    /// Stable kernel name (e.g. `"lean"`, `"rocq"`, `"isabelle"`).
    fn name(&self) -> &str;

    /// Run the expensive check.
    fn verify(&self, candidate: &C, budget: &RungBudget) -> KernelVerdict;
}

// ---------------------------------------------------------------------------
// Configuration — default is kernel-only
// ---------------------------------------------------------------------------

/// Enable flag + budget for one cheap tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RungConfig {
    /// When `false` (the default) the tier is skipped entirely — its rungs are
    /// never invoked, even if registered.
    pub enabled: bool,
    /// The budget handed to each rung in this tier.
    pub budget: RungBudget,
}

impl RungConfig {
    /// An enabled tier with `budget`.
    pub fn enabled(budget: RungBudget) -> Self {
        Self {
            enabled: true,
            budget,
        }
    }

    /// A disabled tier (the default).
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            budget: RungBudget::default(),
        }
    }
}

impl Default for RungConfig {
    /// Disabled. Opting in to a cheap pre-filter is always explicit.
    fn default() -> Self {
        Self::disabled()
    }
}

/// Per-tier configuration for the ladder.
///
/// [`Default`] disables both cheap tiers, so a default-configured ladder is
/// **exactly** a kernel call: same verdict, same number of kernel invocations,
/// no cheap rung consulted. Callers opt *in* to pre-filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LadderConfig {
    /// Tier 1 policy.
    pub fast_refute: RungConfig,
    /// Tier 2 policy.
    pub cheap_decide: RungConfig,
    /// Budget passed to the kernel. The kernel tier has no enable flag: a ladder
    /// without a kernel would be a ladder that can never verify.
    pub kernel_budget: RungBudget,
}

impl LadderConfig {
    /// The kernel-only policy (identical to [`Default`], named for intent).
    pub fn kernel_only() -> Self {
        Self {
            fast_refute: RungConfig::disabled(),
            cheap_decide: RungConfig::disabled(),
            kernel_budget: RungBudget::default(),
        }
    }

    /// Both cheap tiers enabled with their default budgets.
    pub fn all_rungs() -> Self {
        Self {
            fast_refute: RungConfig::enabled(RungBudget::default()),
            cheap_decide: RungConfig::enabled(RungBudget::default()),
            kernel_budget: RungBudget::default(),
        }
    }

    /// The policy for a tier.
    fn tier(&self, tier: RungTier) -> RungConfig {
        match tier {
            RungTier::FastRefute => self.fast_refute,
            RungTier::CheapDecide => self.cheap_decide,
            RungTier::Kernel => RungConfig::enabled(self.kernel_budget),
        }
    }
}

impl Default for LadderConfig {
    fn default() -> Self {
        Self::kernel_only()
    }
}

// ---------------------------------------------------------------------------
// Outcomes
// ---------------------------------------------------------------------------

/// What the ladder concluded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LadderOutcome {
    /// A cheap rung refuted the candidate; the kernel was never consulted.
    Refuted {
        /// The refuting rung's name.
        rung: String,
        /// Which tier it sat in.
        tier: RungTier,
        /// The structured witness that routes the repair.
        witness: RefutationWitness,
    },
    /// The kernel certified. Reachable only from [`KernelVerdict::Verified`].
    Verified {
        /// The kernel's name.
        kernel: String,
    },
    /// The kernel evaluated and rejected.
    Rejected {
        /// The kernel's name.
        kernel: String,
        /// The kernel's error text.
        reason: String,
    },
    /// Nothing settled it: every cheap rung abstained and so did the kernel.
    Abstained,
}

/// One rung invocation, recorded in order for audit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RungTrace {
    /// The rung's name.
    pub rung: String,
    /// The tier it ran in.
    pub tier: RungTier,
    /// `refuted` | `abstain` | `verified` | `rejected`.
    pub verdict: String,
}

/// A full ladder run: the outcome plus the ordered trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LadderRun {
    /// The conclusion.
    pub outcome: LadderOutcome,
    /// Every rung actually invoked, in ascending-cost order.
    pub trail: Vec<RungTrace>,
    /// Whether the expensive kernel was invoked. `false` whenever a cheap rung
    /// short-circuited — this is the metric that justifies the ladder's cost.
    pub kernel_consulted: bool,
}

impl LadderRun {
    /// Whether this run certified. True only for [`LadderOutcome::Verified`].
    pub fn is_verified(&self) -> bool {
        matches!(self.outcome, LadderOutcome::Verified { .. })
    }

    /// The refutation witness, when a cheap rung refuted.
    pub fn witness(&self) -> Option<&RefutationWitness> {
        match &self.outcome {
            LadderOutcome::Refuted { witness, .. } => Some(witness),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// The ladder
// ---------------------------------------------------------------------------

/// Runs checks in ascending cost order: `fast_refute -> cheap_decide ->
/// expensive_kernel`, short-circuiting on the first refutation.
///
/// Every rung is injected; the ladder itself is pure control flow with no IO,
/// clock, or randomness.
pub struct VerificationLadder<'a, C: ?Sized> {
    fast_refute: Vec<&'a dyn CheapRung<C>>,
    cheap_decide: Vec<&'a dyn CheapRung<C>>,
    kernel: &'a dyn KernelRung<C>,
    config: LadderConfig,
}

impl<'a, C: ?Sized> VerificationLadder<'a, C> {
    /// A ladder with no cheap rungs and the kernel-only config — behaviorally a
    /// direct kernel call.
    pub fn new(kernel: &'a dyn KernelRung<C>) -> Self {
        Self {
            fast_refute: Vec::new(),
            cheap_decide: Vec::new(),
            kernel,
            config: LadderConfig::kernel_only(),
        }
    }

    /// Register tier-1 (cheapest) refuters, tried in the given order.
    pub fn with_fast_refute(mut self, rungs: Vec<&'a dyn CheapRung<C>>) -> Self {
        self.fast_refute = rungs;
        self
    }

    /// Register tier-2 refuters, tried after every tier-1 rung abstains.
    pub fn with_cheap_decide(mut self, rungs: Vec<&'a dyn CheapRung<C>>) -> Self {
        self.cheap_decide = rungs;
        self
    }

    /// Set the per-tier enable flags and budgets. Note that registering rungs
    /// does **not** enable their tier: the config must also enable it, so the
    /// safe kernel-only behavior survives a partial configuration.
    pub fn with_config(mut self, config: LadderConfig) -> Self {
        self.config = config;
        self
    }

    /// The active config.
    pub fn config(&self) -> &LadderConfig {
        &self.config
    }

    /// Run the ladder over `candidate`.
    ///
    /// Order and short-circuiting:
    /// 1. Each enabled `fast_refute` rung, in registration order. First
    ///    [`RungVerdict::Refuted`] ends the run — **the kernel is not called**.
    /// 2. Then each enabled `cheap_decide` rung, same rule.
    /// 3. Only if every cheap rung abstained (or none were enabled) is the
    ///    kernel invoked; its [`KernelVerdict`] becomes the outcome.
    ///
    /// A cheap rung can therefore never cause a `Verified` outcome, and never
    /// suppresses one either — it only ever elides a kernel call that would have
    /// had to reject.
    pub fn run(&self, candidate: &C) -> LadderRun {
        let mut trail = Vec::new();

        for tier in [RungTier::FastRefute, RungTier::CheapDecide] {
            let policy = self.config.tier(tier);
            if !policy.enabled {
                continue;
            }
            let rungs = match tier {
                RungTier::FastRefute => &self.fast_refute,
                _ => &self.cheap_decide,
            };
            for rung in rungs {
                match rung.probe(candidate, &policy.budget) {
                    RungVerdict::Refuted(witness) => {
                        trail.push(RungTrace {
                            rung: rung.name().to_string(),
                            tier,
                            verdict: "refuted".to_string(),
                        });
                        return LadderRun {
                            outcome: LadderOutcome::Refuted {
                                rung: rung.name().to_string(),
                                tier,
                                witness,
                            },
                            trail,
                            // The whole point: the expensive rung is skipped.
                            kernel_consulted: false,
                        };
                    }
                    RungVerdict::Abstain => trail.push(RungTrace {
                        rung: rung.name().to_string(),
                        tier,
                        verdict: "abstain".to_string(),
                    }),
                }
            }
        }

        let kernel_name = self.kernel.name().to_string();
        let verdict = self.kernel.verify(candidate, &self.config.kernel_budget);
        let (outcome, label) = match verdict {
            KernelVerdict::Verified => (
                LadderOutcome::Verified {
                    kernel: kernel_name.clone(),
                },
                "verified",
            ),
            KernelVerdict::Rejected(reason) => (
                LadderOutcome::Rejected {
                    kernel: kernel_name.clone(),
                    reason,
                },
                "rejected",
            ),
            KernelVerdict::Abstain => (LadderOutcome::Abstained, "abstain"),
        };
        trail.push(RungTrace {
            rung: kernel_name,
            tier: RungTier::Kernel,
            verdict: label.to_string(),
        });
        LadderRun {
            outcome,
            trail,
            kernel_consulted: true,
        }
    }
}

// ---------------------------------------------------------------------------
// The validity trichotomy
// ---------------------------------------------------------------------------

/// Why a candidate could not be decoded into something checkable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodeError {
    /// Where decoding failed (`"json"`, `"lean_syntax"`, `"answer_extraction"`).
    pub stage: String,
    /// Human-readable reason, fed to the reformat/repair prompt.
    pub reason: String,
}

impl DecodeError {
    /// Construct a decode failure.
    pub fn new(stage: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            reason: reason.into(),
        }
    }
}

/// A raw generation, after the decode attempt. The `Undecodable` arm is kept
/// distinct all the way through so it can never be silently counted as wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateStatus<T> {
    /// Decoded into a checkable candidate.
    Decoded(T),
    /// Never became checkable.
    Undecodable(DecodeError),
}

impl<T> CandidateStatus<T> {
    /// Whether this candidate at least decoded.
    pub fn is_decoded(&self) -> bool {
        matches!(self, CandidateStatus::Decoded(_))
    }

    /// The decoded candidate, if any.
    pub fn decoded(&self) -> Option<&T> {
        match self {
            CandidateStatus::Decoded(value) => Some(value),
            CandidateStatus::Undecodable(_) => None,
        }
    }
}

/// The decode seam: turns a raw model output into a checkable candidate. Pure by
/// contract; a real implementation may call a parser but must not observe a
/// clock or RNG.
pub trait Decoder<R: ?Sized> {
    /// The checkable candidate type.
    type Candidate;

    /// Attempt to decode. `Err` means *unparseable*, not *wrong*.
    fn decode(&self, raw: &R) -> Result<Self::Candidate, DecodeError>;
}

/// Decode `raw` into a [`CandidateStatus`], preserving the undecodable arm.
pub fn decode_status<R: ?Sized, D: Decoder<R>>(
    decoder: &D,
    raw: &R,
) -> CandidateStatus<D::Candidate> {
    match decoder.decode(raw) {
        Ok(candidate) => CandidateStatus::Decoded(candidate),
        Err(error) => CandidateStatus::Undecodable(error),
    }
}

/// The ternary validity label. **Not** a boolean, because
/// "never parsed" and "parsed and is wrong" demand different follow-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Validity {
    /// Kernel-certified. Only [`KernelVerdict::Verified`] produces this.
    Correct,
    /// Evaluated and found wrong — by the kernel, or refuted by a cheap rung
    /// with a witness.
    Incorrect,
    /// Never became checkable: a parse/format failure, upstream of any
    /// mathematical judgment.
    Undecodable,
}

/// What the pipeline should do next with a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Routing {
    /// Accept and record as certified.
    Certify,
    /// Drop the branch. The mathematics was evaluated and is wrong; reformatting
    /// cannot rescue it.
    Prune,
    /// Re-emit / reformat and try to decode again. Pruning here would discard
    /// candidates whose reasoning was never examined.
    Repair,
}

impl Validity {
    /// Stable label for metrics.
    pub fn as_str(&self) -> &'static str {
        match self {
            Validity::Correct => "correct",
            Validity::Incorrect => "incorrect",
            Validity::Undecodable => "undecodable",
        }
    }

    /// **The routing consequence** — the reason the trichotomy exists.
    ///
    /// `Undecodable ⇒ Repair` (format problem, reasoning unexamined);
    /// `Incorrect ⇒ Prune` (content problem, reasoning examined and wrong);
    /// `Correct ⇒ Certify`.
    pub fn routing(&self) -> Routing {
        match self {
            Validity::Correct => Routing::Certify,
            Validity::Incorrect => Routing::Prune,
            Validity::Undecodable => Routing::Repair,
        }
    }

    /// Whether the candidate at least decoded (i.e. is not `Undecodable`).
    pub fn is_well_formed(&self) -> bool {
        !matches!(self, Validity::Undecodable)
    }
}

/// Classify a candidate given its decode status and (if it decoded) its ladder
/// outcome.
///
/// Returns `None` when nothing settled it — an undecided candidate is *not* a
/// validity label, and forcing it into one would fabricate a judgment. `None`
/// means "still open": retry with a bigger budget or a different kernel.
///
/// Note the ordering: the decode status is examined **first**, so a candidate
/// that never parsed is `Undecodable` regardless of what any checker would have
/// said about it.
pub fn classify<T>(
    status: &CandidateStatus<T>,
    outcome: Option<&LadderOutcome>,
) -> Option<Validity> {
    if let CandidateStatus::Undecodable(_) = status {
        return Some(Validity::Undecodable);
    }
    match outcome? {
        LadderOutcome::Verified { .. } => Some(Validity::Correct),
        LadderOutcome::Refuted { .. } | LadderOutcome::Rejected { .. } => Some(Validity::Incorrect),
        LadderOutcome::Abstained => None,
    }
}

/// Classify from a completed [`LadderRun`] on an already-decoded candidate.
pub fn classify_run(run: &LadderRun) -> Option<Validity> {
    let decoded: CandidateStatus<()> = CandidateStatus::Decoded(());
    classify(&decoded, Some(&run.outcome))
}

/// Fraction of candidates that **at least decode**, in `[0.0, 1.0]`.
///
/// This isolates output-format regressions from reasoning regressions: it moves
/// when the decoder/prompt/template breaks and is flat when only the mathematics
/// gets worse. Empty input yields `0.0` (no evidence of well-formedness), which
/// is the conservative reading for a dashboard.
pub fn well_formed_rate(validities: &[Validity]) -> f64 {
    if validities.is_empty() {
        return 0.0;
    }
    let ok = validities.iter().filter(|v| v.is_well_formed()).count();
    ok as f64 / validities.len() as f64
}

/// [`well_formed_rate`] over decode statuses directly, before any checking has
/// happened — usable as an early format-health signal.
pub fn well_formed_rate_of_statuses<T>(statuses: &[CandidateStatus<T>]) -> f64 {
    if statuses.is_empty() {
        return 0.0;
    }
    let ok = statuses.iter().filter(|s| s.is_decoded()).count();
    ok as f64 / statuses.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A cheap rung that always refutes with a structured witness.
    struct AlwaysRefutes {
        name: &'static str,
        calls: Cell<usize>,
    }

    impl AlwaysRefutes {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                calls: Cell::new(0),
            }
        }
    }

    impl CheapRung<str> for AlwaysRefutes {
        fn name(&self) -> &str {
            self.name
        }
        fn probe(&self, _candidate: &str, _budget: &RungBudget) -> RungVerdict {
            self.calls.set(self.calls.get() + 1);
            RungVerdict::Refuted(RefutationWitness::counterexample(
                self.name,
                "claim fails at n = 4",
                vec![("n".to_string(), "4".to_string())],
            ))
        }
    }

    /// A cheap rung that always abstains.
    struct AlwaysAbstains {
        name: &'static str,
        calls: Cell<usize>,
    }

    impl AlwaysAbstains {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                calls: Cell::new(0),
            }
        }
    }

    impl CheapRung<str> for AlwaysAbstains {
        fn name(&self) -> &str {
            self.name
        }
        fn probe(&self, _candidate: &str, _budget: &RungBudget) -> RungVerdict {
            self.calls.set(self.calls.get() + 1);
            RungVerdict::Abstain
        }
    }

    /// A kernel mock that counts its invocations.
    struct MockKernel {
        verdict: KernelVerdict,
        calls: Cell<usize>,
        last_budget: Cell<Option<RungBudget>>,
    }

    impl MockKernel {
        fn new(verdict: KernelVerdict) -> Self {
            Self {
                verdict,
                calls: Cell::new(0),
                last_budget: Cell::new(None),
            }
        }
    }

    impl KernelRung<str> for MockKernel {
        fn name(&self) -> &str {
            "mock-kernel"
        }
        fn verify(&self, _candidate: &str, budget: &RungBudget) -> KernelVerdict {
            self.calls.set(self.calls.get() + 1);
            self.last_budget.set(Some(*budget));
            self.verdict.clone()
        }
    }

    #[test]
    fn a_refuting_cheap_rung_short_circuits_before_the_kernel_is_consulted() {
        let refuter = AlwaysRefutes::new("numeric-sweep");
        let kernel = MockKernel::new(KernelVerdict::Verified);
        let ladder = VerificationLadder::new(&kernel)
            .with_fast_refute(vec![&refuter])
            .with_config(LadderConfig::all_rungs());

        let run = ladder.run("statement");

        // The whole justification for the ladder: the expensive rung never ran.
        assert_eq!(kernel.calls.get(), 0, "kernel must NOT be consulted");
        assert!(!run.kernel_consulted);
        assert_eq!(refuter.calls.get(), 1);
        assert!(matches!(
            run.outcome,
            LadderOutcome::Refuted {
                tier: RungTier::FastRefute,
                ..
            }
        ));
        // And the refutation carried a structured, repair-routing witness.
        let witness = run.witness().expect("refutation carries a witness");
        assert_eq!(witness.kind, WitnessKind::Counterexample);
        assert_eq!(witness.instance, vec![("n".to_string(), "4".to_string())]);
        assert!(witness.is_actionable());
    }

    #[test]
    fn a_refuting_rung_stops_later_cheap_rungs_too() {
        let first = AlwaysRefutes::new("first");
        let second = AlwaysRefutes::new("second");
        let later = AlwaysRefutes::new("tier2");
        let kernel = MockKernel::new(KernelVerdict::Verified);
        let ladder = VerificationLadder::new(&kernel)
            .with_fast_refute(vec![&first, &second])
            .with_cheap_decide(vec![&later])
            .with_config(LadderConfig::all_rungs());

        let run = ladder.run("statement");

        assert_eq!(first.calls.get(), 1);
        assert_eq!(second.calls.get(), 0, "short-circuit stops the tier");
        assert_eq!(later.calls.get(), 0, "and stops the next tier");
        assert_eq!(kernel.calls.get(), 0);
        assert_eq!(run.trail.len(), 1);
        assert_eq!(run.trail[0].rung, "first");
    }

    #[test]
    fn an_abstaining_cheap_rung_falls_through_to_the_kernel() {
        let fast = AlwaysAbstains::new("sweep");
        let cheap = AlwaysAbstains::new("pbt");
        let kernel = MockKernel::new(KernelVerdict::Verified);
        let ladder = VerificationLadder::new(&kernel)
            .with_fast_refute(vec![&fast])
            .with_cheap_decide(vec![&cheap])
            .with_config(LadderConfig::all_rungs());

        let run = ladder.run("statement");

        assert_eq!(fast.calls.get(), 1);
        assert_eq!(cheap.calls.get(), 1);
        assert_eq!(kernel.calls.get(), 1, "abstention falls through");
        assert!(run.kernel_consulted);
        assert!(run.is_verified());
        // Ascending cost order is recorded in the trail.
        let tiers: Vec<RungTier> = run.trail.iter().map(|t| t.tier).collect();
        assert_eq!(
            tiers,
            vec![RungTier::FastRefute, RungTier::CheapDecide, RungTier::Kernel]
        );
    }

    #[test]
    fn only_the_kernel_can_return_verified() {
        // Structural argument, demonstrated through the API: `RungVerdict` — the
        // ONLY type a `CheapRung` can return — has exactly two variants, neither
        // positive. Exhaustively matching it here fails to compile the moment a
        // positive variant is added, which is the point.
        let cheap: RungVerdict = AlwaysAbstains::new("probe").probe("s", &RungBudget::default());
        match cheap {
            RungVerdict::Refuted(_) => {}
            RungVerdict::Abstain => {}
            // no `Verified` arm exists to write.
        }

        // Every `Verified` outcome the ladder can emit is constructed on exactly
        // one code path: the kernel returning `KernelVerdict::Verified`. With a
        // non-verifying kernel, no arrangement of cheap rungs yields Verified.
        for kernel_verdict in [
            KernelVerdict::Rejected("kernel says no".into()),
            KernelVerdict::Abstain,
        ] {
            let refuter = AlwaysRefutes::new("r");
            let abstainer = AlwaysAbstains::new("a");
            let kernel = MockKernel::new(kernel_verdict);
            let arrangements: Vec<Vec<&dyn CheapRung<str>>> =
                vec![Vec::new(), vec![&refuter], vec![&abstainer]];
            for rungs in arrangements {
                let ladder = VerificationLadder::new(&kernel)
                    .with_fast_refute(rungs)
                    .with_config(LadderConfig::all_rungs());
                assert!(
                    !ladder.run("s").is_verified(),
                    "no cheap rung can manufacture a positive verdict"
                );
            }
        }
    }

    #[test]
    fn undecodable_is_distinguished_from_incorrect_and_routes_differently() {
        let undecodable: CandidateStatus<&str> =
            CandidateStatus::Undecodable(DecodeError::new("lean_syntax", "unbalanced `begin`"));
        let decoded: CandidateStatus<&str> = CandidateStatus::Decoded("theorem t : True");

        let rejected = LadderOutcome::Rejected {
            kernel: "mock-kernel".into(),
            reason: "unknown identifier".into(),
        };
        let refuted = LadderOutcome::Refuted {
            rung: "sweep".into(),
            tier: RungTier::FastRefute,
            witness: RefutationWitness::conflict_set("sweep", "h1 ∧ h2 unsat", vec!["h1".into()]),
        };
        let verified = LadderOutcome::Verified {
            kernel: "mock-kernel".into(),
        };

        // A parse failure is Undecodable no matter what a checker would have said.
        assert_eq!(
            classify(&undecodable, Some(&rejected)),
            Some(Validity::Undecodable)
        );
        assert_eq!(classify(&undecodable, None), Some(Validity::Undecodable));
        // A checked-and-wrong candidate is Incorrect — a different label.
        assert_eq!(classify(&decoded, Some(&rejected)), Some(Validity::Incorrect));
        assert_eq!(classify(&decoded, Some(&refuted)), Some(Validity::Incorrect));
        assert_eq!(classify(&decoded, Some(&verified)), Some(Validity::Correct));
        assert_ne!(Validity::Undecodable, Validity::Incorrect);

        // ...and they route differently: repair vs prune.
        assert_eq!(Validity::Undecodable.routing(), Routing::Repair);
        assert_eq!(Validity::Incorrect.routing(), Routing::Prune);
        assert_eq!(Validity::Correct.routing(), Routing::Certify);
        assert_ne!(
            Validity::Undecodable.routing(),
            Validity::Incorrect.routing()
        );
    }

    #[test]
    fn an_unsettled_candidate_gets_no_validity_label() {
        let decoded: CandidateStatus<&str> = CandidateStatus::Decoded("x");
        assert_eq!(classify(&decoded, Some(&LadderOutcome::Abstained)), None);
        assert_eq!(classify(&decoded, None), None);
    }

    #[test]
    fn classify_run_reads_a_completed_ladder_run() {
        let kernel = MockKernel::new(KernelVerdict::Verified);
        let ladder = VerificationLadder::new(&kernel);
        assert_eq!(classify_run(&ladder.run("s")), Some(Validity::Correct));

        let kernel = MockKernel::new(KernelVerdict::Rejected("nope".into()));
        let ladder = VerificationLadder::new(&kernel);
        assert_eq!(classify_run(&ladder.run("s")), Some(Validity::Incorrect));

        let kernel = MockKernel::new(KernelVerdict::Abstain);
        let ladder = VerificationLadder::new(&kernel);
        assert_eq!(classify_run(&ladder.run("s")), None);
    }

    #[test]
    fn well_formed_rate_computes_correctly() {
        assert_eq!(well_formed_rate(&[]), 0.0);
        assert_eq!(well_formed_rate(&[Validity::Undecodable]), 0.0);
        assert_eq!(well_formed_rate(&[Validity::Correct]), 1.0);
        // 3 of 4 decode: one correct, two incorrect, one undecodable.
        let sample = [
            Validity::Correct,
            Validity::Incorrect,
            Validity::Incorrect,
            Validity::Undecodable,
        ];
        assert_eq!(well_formed_rate(&sample), 0.75);
        // The point of the metric: format health is independent of accuracy.
        let all_wrong = [
            Validity::Incorrect,
            Validity::Incorrect,
            Validity::Incorrect,
            Validity::Incorrect,
        ];
        assert_eq!(
            well_formed_rate(&all_wrong),
            1.0,
            "every candidate decoded; the regression is in reasoning, not format"
        );
    }

    #[test]
    fn well_formed_rate_of_statuses_matches() {
        let statuses: Vec<CandidateStatus<&str>> = vec![
            CandidateStatus::Decoded("a"),
            CandidateStatus::Undecodable(DecodeError::new("json", "trailing comma")),
            CandidateStatus::Decoded("c"),
            CandidateStatus::Decoded("d"),
        ];
        assert_eq!(well_formed_rate_of_statuses(&statuses), 0.75);
        let empty: Vec<CandidateStatus<&str>> = Vec::new();
        assert_eq!(well_formed_rate_of_statuses(&empty), 0.0);
    }

    #[test]
    fn decode_status_preserves_the_undecodable_arm() {
        struct EvenLenDecoder;
        impl Decoder<str> for EvenLenDecoder {
            type Candidate = usize;
            fn decode(&self, raw: &str) -> Result<usize, DecodeError> {
                if raw.len() % 2 == 0 {
                    Ok(raw.len())
                } else {
                    Err(DecodeError::new("len", "odd length"))
                }
            }
        }
        assert!(decode_status(&EvenLenDecoder, "abcd").is_decoded());
        let bad = decode_status(&EvenLenDecoder, "abc");
        assert!(!bad.is_decoded());
        assert_eq!(classify(&bad, None), Some(Validity::Undecodable));
    }

    #[test]
    fn no_rungs_configured_equals_kernel_only() {
        for verdict in [
            KernelVerdict::Verified,
            KernelVerdict::Rejected("boom".into()),
            KernelVerdict::Abstain,
        ] {
            // Reference: call the kernel directly.
            let direct = MockKernel::new(verdict.clone());
            let expected = direct.verify("s", &RungBudget::default());

            // The default ladder with no rungs registered.
            let bare_kernel = MockKernel::new(verdict.clone());
            let bare = VerificationLadder::new(&bare_kernel).run("s");

            // A ladder WITH rungs registered but the default (disabled) config:
            // registration alone must not change behavior.
            let refuter = AlwaysRefutes::new("would-refute");
            let cfg_kernel = MockKernel::new(verdict.clone());
            let configured = VerificationLadder::new(&cfg_kernel)
                .with_fast_refute(vec![&refuter])
                .with_cheap_decide(vec![&refuter])
                .with_config(LadderConfig::default())
                .run("s");

            assert_eq!(bare.outcome, configured.outcome);
            assert!(bare.kernel_consulted && configured.kernel_consulted);
            assert_eq!(bare_kernel.calls.get(), 1);
            assert_eq!(cfg_kernel.calls.get(), 1, "exactly one kernel call, as direct");
            assert_eq!(refuter.calls.get(), 0, "disabled tiers never invoke rungs");
            assert_eq!(bare.trail.len(), 1, "only the kernel appears in the trail");
            assert_eq!(bare.trail[0].tier, RungTier::Kernel);

            // Same verdict as the direct call.
            match (&expected, &bare.outcome) {
                (KernelVerdict::Verified, LadderOutcome::Verified { .. }) => {}
                (KernelVerdict::Rejected(r), LadderOutcome::Rejected { reason, .. }) => {
                    assert_eq!(r, reason)
                }
                (KernelVerdict::Abstain, LadderOutcome::Abstained) => {}
                other => panic!("ladder diverged from the direct kernel call: {other:?}"),
            }
        }
    }

    #[test]
    fn default_config_is_kernel_only_and_budgets_are_passed_through() {
        assert_eq!(LadderConfig::default(), LadderConfig::kernel_only());
        assert!(!LadderConfig::default().fast_refute.enabled);
        assert!(!LadderConfig::default().cheap_decide.enabled);
        assert!(LadderConfig::all_rungs().fast_refute.enabled);

        let kernel = MockKernel::new(KernelVerdict::Abstain);
        let cfg = LadderConfig {
            kernel_budget: RungBudget::new(42, 7),
            ..LadderConfig::default()
        };
        VerificationLadder::new(&kernel).with_config(cfg).run("s");
        assert_eq!(kernel.last_budget.get(), Some(RungBudget::new(42, 7)));
    }

    #[test]
    fn a_disabled_tier_is_skipped_but_an_enabled_one_still_runs() {
        let fast = AlwaysRefutes::new("fast");
        let cheap = AlwaysRefutes::new("cheap");
        let kernel = MockKernel::new(KernelVerdict::Verified);
        let cfg = LadderConfig {
            fast_refute: RungConfig::disabled(),
            cheap_decide: RungConfig::enabled(RungBudget::new(5, 5)),
            kernel_budget: RungBudget::default(),
        };
        let run = VerificationLadder::new(&kernel)
            .with_fast_refute(vec![&fast])
            .with_cheap_decide(vec![&cheap])
            .with_config(cfg)
            .run("s");

        assert_eq!(fast.calls.get(), 0, "disabled tier skipped");
        assert_eq!(cheap.calls.get(), 1, "enabled tier runs");
        assert_eq!(kernel.calls.get(), 0);
        assert!(matches!(
            run.outcome,
            LadderOutcome::Refuted {
                tier: RungTier::CheapDecide,
                ..
            }
        ));
    }

    #[test]
    fn the_ladder_is_deterministic_across_repeated_runs() {
        let abstainer = AlwaysAbstains::new("probe");
        let kernel = MockKernel::new(KernelVerdict::Verified);
        let ladder = VerificationLadder::new(&kernel)
            .with_fast_refute(vec![&abstainer])
            .with_config(LadderConfig::all_rungs());
        let a = ladder.run("s");
        let b = ladder.run("s");
        let c = ladder.run("s");
        assert_eq!(a, b);
        assert_eq!(b, c);
    }
}
