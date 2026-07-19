//! System-agnostic formal-method contract (Phase 0 of the multi-formal-system
//! integration; see `docs/formal-systems/INTEGRATION-PLAN.md`).
//!
//! This module generalizes the previously Lean-hardwired prover layer into a
//! `FormalSystem` tag plus two traits:
//!
//! * [`FormalBackend`] — the 3+1-layer verification gate (compile → axiom/oracle
//!   audit ⊆ whitelist → kernel re-check → source scan), with a fail-closed
//!   default [`FormalBackend::verify`] orchestration shared by every system.
//! * [`ProofSession`] — the warm-driver interface. `submit_unit` (whole
//!   theory/file) is supported by all systems; `step_tactic` is Lean/Rocq only
//!   (Isabelle returns [`SessionError::Unsupported`]).
//!
//! Phase 0 keeps behavior unchanged: only the *contract* is generalized. The
//! concrete per-system producers arrive in later phases (mock backends in
//! Phase 1; real gates in Phase 2; live drivers in Phase 3).

use crate::{
    config::Config,
    db::Store,
    prover::model::{ProofJob, ProofResult, ProverJobStatus, VerificationReport},
};
use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
    time::Instant,
};

/// Which formal system a proof object belongs to. Serialized `snake_case`
/// (`lean` / `rocq` / `isabelle` / `candle` / `agda` / `metamath`) to match the `backend` string
/// dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormalSystem {
    Lean,
    Rocq,
    Isabelle,
    /// Candle — the verified HOL Light kernel running on CakeML. Its kernel
    /// soundness is machine-PROVEN (in HOL4, down to the CakeML-compiled binary),
    /// so its layer-3 kernel re-check carries a stronger guarantee than the
    /// smaller-trusted-checker approach of `leanchecker`/`coqchk`.
    Candle,
    /// Agda, an intuitionistic Martin-Lof dependent type theory checker.
    Agda,
    /// Metamath's tiny substitution kernel and explicit proof language.
    Metamath,
}

impl Default for FormalSystem {
    fn default() -> Self {
        FormalSystem::Lean
    }
}

impl FormalSystem {
    /// Canonical lowercase tag, matching `backend` strings and serde output.
    pub fn as_str(self) -> &'static str {
        match self {
            FormalSystem::Lean => "lean",
            FormalSystem::Rocq => "rocq",
            FormalSystem::Isabelle => "isabelle",
            FormalSystem::Candle => "candle",
            FormalSystem::Agda => "agda",
            FormalSystem::Metamath => "metamath",
        }
    }

    /// Trusted-axiom / oracle whitelist. Anything a proof depends on that is
    /// NOT in this set makes the axiom audit fail-closed.
    ///
    /// * Lean — the three universally-trusted axioms (`propext`,
    ///   `Classical.choice`, `Quot.sound`).
    /// * Rocq — no bare axioms; a clean `Print Assumptions` reads
    ///   `Closed under the global context` (represented here as the single
    ///   sentinel token the audit checks for).
    /// * Isabelle — the empty oracle set (`Thm_Deps.all_oracles = []`).
    /// * Candle — HOL Light's tiny, fixed axiom base: the three mathematical
    ///   axioms `ETA_AX`, `SELECT_AX` (choice), and `INFINITY_AX`. The
    ///   definitional principles (`new_definition` / `new_basic_definition` /
    ///   `new_type_definition`) are conservative and add nothing to this base;
    ///   any OTHER axiom (i.e. a `new_axiom` call) fails the audit.
    pub fn axiom_whitelist(self) -> Vec<String> {
        match self {
            FormalSystem::Lean => vec![
                "propext".into(),
                "Classical.choice".into(),
                "Quot.sound".into(),
            ],
            FormalSystem::Rocq => vec!["Closed under the global context".into()],
            FormalSystem::Isabelle => Vec::new(),
            FormalSystem::Candle => {
                vec!["ETA_AX".into(), "SELECT_AX".into(), "INFINITY_AX".into()]
            }
            FormalSystem::Agda => Vec::new(),
            FormalSystem::Metamath => Vec::new(),
        }
    }

    /// Source-file extension for generated proofs.
    pub fn source_extension(self) -> &'static str {
        match self {
            FormalSystem::Lean => ".lean",
            FormalSystem::Rocq => ".v",
            FormalSystem::Isabelle => ".thy",
            // HOL Light proofs are OCaml scripts executed by the Candle kernel.
            FormalSystem::Candle => ".ml",
            FormalSystem::Agda => ".agda",
            FormalSystem::Metamath => ".mm",
        }
    }

    /// Default corpus imports the model may draw premises from.
    pub fn default_imports(self) -> Vec<String> {
        match self {
            FormalSystem::Lean => vec!["Mathlib".into()],
            FormalSystem::Rocq => vec!["Stdlib".into(), "mathcomp.ssreflect.ssreflect".into()],
            FormalSystem::Isabelle => vec!["Main".into()],
            // HOL Light's standard prelude (loaded by the Candle image).
            FormalSystem::Candle => vec!["hol_light".into()],
            FormalSystem::Agda => vec!["Agda.Builtin".into()],
            FormalSystem::Metamath => vec!["set.mm".into()],
        }
    }

    /// Citable trusted-base facts about the foundation each backend rests on,
    /// used for honest documentation and backend selection. The three capability
    /// flags and the primitive-notion counts follow Freek Wiedijk, "Is ZF a
    /// hack? Comparing the complexity of some (formalist interpretations of)
    /// foundational systems for mathematics" (J. Applied Logic, 2006), whose
    /// Automath encodings (`zfc-etc/*.aut`) give a per-foundation primitive-
    /// notion inventory. `primitive_notions` is `Some(_)` only where our
    /// backend's foundation appears *directly* in that study; `None` where the
    /// backend rests on a relative/descendant of a studied system.
    pub fn foundation_profile(self) -> FoundationProfile {
        match self {
            // Lean 4 — Calculus of Inductive Constructions. Not studied directly;
            // a descendant of CoC/ECC. Classical logic and choice are opt-in
            // axioms (see `axiom_whitelist`), available but not default.
            FormalSystem::Lean => FoundationProfile {
                foundation: "Calculus of Inductive Constructions (type theory)",
                classical: true,
                choice: true,
                all_math: true,
                primitive_notions: None,
                note: "descendant of CoC/ECC (Wiedijk 2006); classical logic + \
                       choice are opt-in axioms, not the default logic",
            },
            // Rocq — CIC as well, but intuitionistic and choice-free by default
            // (`Classical`/choice must be imported explicitly).
            FormalSystem::Rocq => FoundationProfile {
                foundation: "Calculus of Inductive Constructions (type theory)",
                classical: false,
                choice: false,
                all_math: true,
                primitive_notions: None,
                note: "descendant of CoC/ECC (Wiedijk 2006); intuitionistic and \
                       choice-free by default",
            },
            // Isabelle/HOL — Church higher-order logic (same lineage as HOL
            // Light). Wiedijk studied the Pure *meta-logic* (minimal, not
            // all-math) separately, so no direct primitive-notion count for the
            // full object logic.
            FormalSystem::Isabelle => FoundationProfile {
                foundation: "Isabelle/HOL (Church higher-order logic)",
                classical: true,
                choice: true,
                all_math: true,
                primitive_notions: None,
                note: "Church HOL, same lineage as HOL Light; Wiedijk 2006 \
                       studied the minimal Pure meta-logic (not all-math) apart \
                       from the full object logic",
            },
            // Candle — HOL Light exactly, the `holl.aut` context in the study:
            // 25 primitive notions, the simplest *serious* foundation by concept
            // count; the three axioms match our `axiom_whitelist`.
            FormalSystem::Candle => FoundationProfile {
                foundation: "HOL Light (Church higher-order logic)",
                classical: true,
                choice: true,
                all_math: true,
                primitive_notions: Some(25),
                note: "Wiedijk 2006 `holl.aut`: 25 primitive notions, the \
                       simplest serious foundation by concept count; axioms \
                       ETA_AX / SELECT_AX / INFINITY_AX (= our axiom_whitelist)",
            },
            FormalSystem::Agda => FoundationProfile {
                foundation: "Martin-Lof dependent type theory (Agda)",
                classical: false,
                choice: false,
                all_math: true,
                primitive_notions: None,
                note: "constructive dependent type theory; Agda postulates are rejected by the source gate unless explicitly profiled",
            },
            FormalSystem::Metamath => FoundationProfile {
                foundation: "Metamath set theory / ZFC-style axiomatic foundation",
                classical: true,
                choice: true,
                all_math: true,
                primitive_notions: None,
                note: "explicit substitution proofs checked by the Metamath kernel; database foundation depends on the loaded .mm corpus",
            },
        }
    }
}

/// Citable trusted-base profile for a [`FormalSystem`]'s underlying foundation.
/// See [`FormalSystem::foundation_profile`] for provenance (Wiedijk 2006).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundationProfile {
    /// Human-readable name of the underlying foundation.
    pub foundation: &'static str,
    /// Logic is classical by default (vs. intuitionistic).
    pub classical: bool,
    /// The axiom of choice is available by default.
    pub choice: bool,
    /// Rich enough to encode "all of mathematics" (Wiedijk's "all math" column).
    pub all_math: bool,
    /// Primitive-notion count from Wiedijk 2006 where the foundation is studied
    /// directly; `None` for a relative/descendant of a studied system.
    pub primitive_notions: Option<u32>,
    /// Short provenance / caveat note.
    pub note: &'static str,
}

impl fmt::Display for FormalSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for FormalSystem {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "lean" | "lean4" => Ok(FormalSystem::Lean),
            "rocq" | "coq" => Ok(FormalSystem::Rocq),
            "isabelle" | "isabelle/hol" | "hol" => Ok(FormalSystem::Isabelle),
            // `hol` is already claimed by Isabelle above, so Candle takes the
            // explicit tags only.
            "candle" | "hol_light" | "hol-light" | "hollight" => Ok(FormalSystem::Candle),
            "agda" => Ok(FormalSystem::Agda),
            "metamath" | "mm" => Ok(FormalSystem::Metamath),
            other => Err(anyhow::anyhow!("unknown formal system: {other}")),
        }
    }
}

// --- 3+1-layer gate report structs ---------------------------------------

/// A scaffolded, ready-to-build workspace (Lake project / `_CoqProject` /
/// session `ROOT`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub system: FormalSystem,
    pub root: PathBuf,
    pub source_path: PathBuf,
    /// Fully-qualified theorem name the audit/recheck will target.
    pub entry: String,
}

/// Per-declaration compile status (open-atp `_parse_per_file`): a failure in one
/// declaration does not mask the rest, so a partially-good artifact is visible
/// instead of collapsing to one whole-project boolean.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnitStatus {
    /// The declared theorem/lemma/def name.
    pub name: String,
    /// Whether the compiler reported no error referencing this declaration.
    pub ok: bool,
}

/// Layer 2b (build): did the source compile, and what errors if not.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileReport {
    pub compiled: bool,
    pub errors: Vec<String>,
    /// Failure-isolating per-declaration status (open-atp). Empty when the
    /// backend does not break the artifact down (mock / single-unit theories),
    /// so pre-existing serialized reports still load unchanged.
    #[serde(default)]
    pub per_unit: Vec<UnitStatus>,
    #[serde(default)]
    pub detail: Value,
}

/// Result of the reject-on-mismatch PRECHECK (open-atp `check_compatible`): is
/// the project's pinned toolchain/corpus revision compatible with the backend's
/// before any compute is spent?
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrecheckReport {
    pub compatible: bool,
    /// A human-readable reason when incompatible (empty when compatible).
    pub reason: String,
    #[serde(default)]
    pub detail: Value,
}

impl PrecheckReport {
    pub fn ok() -> Self {
        Self {
            compatible: true,
            reason: String::new(),
            detail: Value::Null,
        }
    }

    pub fn reject(reason: impl Into<String>, detail: Value) -> Self {
        Self {
            compatible: false,
            reason: reason.into(),
            detail,
        }
    }
}

/// Layer 2a: the axioms/oracles the proof depends on, and whether that set is
/// ⊆ the whitelist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxiomReport {
    pub axioms: Vec<String>,
    pub within_whitelist: bool,
    #[serde(default)]
    pub detail: Value,
}

/// Layer 2b (kernel): independent kernel re-check (`leanchecker` / `rocqchk` /
/// clean `isabelle build`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecheckReport {
    pub rechecked: bool,
    #[serde(default)]
    pub detail: Value,
}

/// Layer 2c (MANDATORY): lexical soundness scan for escape hatches the audit
/// and kernel re-check cannot see (`native_decide` / `-type-in-type` /
/// `bypass_check` / `quick_and_dirty` / `sorry` / added `axiom`/`oracle`, …).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanReport {
    pub clean: bool,
    pub findings: Vec<String>,
    #[serde(default)]
    pub detail: Value,
}

// --- Tier 0 gate switches -------------------------------------------------

/// Which of the two Tier-0 semantic gates are CONJOINED into
/// [`VerificationReport::lexically_verified`].
///
/// Both are **OFF by default, and that is a correctness requirement, not
/// timidity.** Each gate demands a channel the caller must positively supply:
///
/// * [`crate::prover::hypothesis_audit`] needs the DESIGNATED INPUTS of the task
///   ([`FormalBackend::designated_inputs`]). With none declared, every genuine
///   antecedent of a conditional theorem reads as `Unaccounted`.
/// * [`crate::prover::vacuity`] needs a [`SatisfiabilityWitness`]
///   ([`FormalBackend::satisfiability_witness`]). It is fail-closed by design:
///   *no witness ⇒ not clean*.
///
/// So conjoining either one before its channel is populated would fail EVERY
/// non-trivial goal and halt the pipeline. Until the callers that own the task
/// definition supply these, the reports are computed and published to the gate's
/// JSON `detail` UNCONDITIONALLY — full observability, zero behavior change —
/// and only the conjunction is gated.
///
/// [`SatisfiabilityWitness`]: crate::prover::vacuity::SatisfiabilityWitness
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TierZeroGates {
    /// Conjoin the hypothesis-discharge audit's verdict.
    pub hypothesis_discharge: bool,
    /// Conjoin the vacuous-success guard's verdict.
    pub vacuity: bool,
}

impl TierZeroGates {
    /// Both gates observational only — the behavior-preserving default.
    pub const OFF: Self = Self {
        hypothesis_discharge: false,
        vacuity: false,
    };

    /// Both gates enforcing.
    pub const ON: Self = Self {
        hypothesis_discharge: true,
        vacuity: true,
    };

    /// Read the gates from the environment, in the crate's default-off env-seam
    /// idiom (cf. `config::default_validate_statements` and
    /// `agent::abstain_threshold`): absent / empty / `0`/`false`/`off` means OFF.
    ///
    /// * `THEOREMATA_HYPOTHESIS_GATE` — hypothesis-discharge audit.
    /// * `THEOREMATA_VACUITY_GATE` — vacuous-success guard.
    ///
    /// Superseded by [`TierZeroGates::from_config`]: the flags now live on
    /// `Config` (`hypothesis_gate` / `vacuity_gate`), which is race-free under
    /// the parallel test harness and visible to a config file. Retained for
    /// callers with no `Config` in hand. Deterministic per call: no clock, no RNG.
    pub fn from_env() -> Self {
        Self {
            hypothesis_discharge: env_gate_on("THEOREMATA_HYPOTHESIS_GATE"),
            vacuity: env_gate_on("THEOREMATA_VACUITY_GATE"),
        }
    }

    /// Read the gate flags from [`Config`] — the authoritative source. Prefer
    /// this over [`TierZeroGates::from_env`] wherever a `Config` is in scope.
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            hypothesis_discharge: cfg.hypothesis_gate,
            vacuity: cfg.vacuity_gate,
        }
    }

    /// Whether either gate is enforcing.
    pub fn any(self) -> bool {
        self.hypothesis_discharge || self.vacuity
    }
}

/// Default-OFF truthiness for a gate env var: set to anything other than
/// (empty) / `0` / `false` / `off` turns it on.
fn env_gate_on(var: &str) -> bool {
    match std::env::var(var) {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "off"
        ),
        Err(_) => false,
    }
}

/// One trait, one impl per system. The default [`verify`](FormalBackend::verify)
/// wires the four layers together, fail-closed: ALL must pass.
pub trait FormalBackend {
    fn system(&self) -> FormalSystem;

    /// The positive signal that constitutes a PASSING compile for this backend.
    /// Required (there is deliberately no default): exit status alone is never a
    /// sufficient pass signal, because several real checkers return 0 on a
    /// FAILED check -- Metamath's reference binary, and the HVM/Kind-family tools
    /// surveyed in `docs/resource-mining/new/higher-order-co.md`. Forcing every
    /// backend to declare its signal stops a new backend from silently
    /// inheriting exit-code trust. See [`SuccessSignal`].
    fn compile_success_signal(&self) -> SuccessSignal;

    /// Whether this backend is a MOCK (canned compile/axiom/kernel layers) rather
    /// than a live toolchain. The default is `false` (a real backend); every
    /// mock-capable backend overrides this to return `self.mock`. The default
    /// [`verify`](FormalBackend::verify) stamps the produced report's `live` flag
    /// as `!self.is_mock()`, so a mock verification can NEVER be mistaken for a
    /// live formal certification downstream.
    fn is_mock(&self) -> bool {
        false
    }

    /// Probe whether this backend's toolchain is usable right now. The default is
    /// `true` (mock backends are always available); live backends override this
    /// to actually probe the configured runner, so callers can skip cleanly when
    /// the toolchain is absent.
    fn available(&self) -> bool {
        true
    }

    /// The toolchain/corpus revision this backend is pinned to, when known
    /// (Lean's `lean-toolchain` string / opam switch / Isabelle heap id). The
    /// default is `None` (mock backends are pin-agnostic); live backends override
    /// it so the [`precheck`](FormalBackend::precheck) can reject a mismatch.
    fn expected_toolchain(&self) -> Option<String> {
        None
    }

    /// Reject-on-mismatch PRECHECK (open-atp `check_compatible`), run *before* any
    /// compute: if the project declares a pinned toolchain that differs from this
    /// backend's, fail fast rather than deep in a build. The default compares
    /// [`FormalProject::toolchain`] against [`expected_toolchain`](FormalBackend::expected_toolchain)
    /// via [`precheck_compat`]; a project that declares nothing, or a backend that
    /// pins nothing, is treated as compatible.
    fn precheck(&self, project: Option<&crate::prover::model::FormalProject>) -> PrecheckReport {
        precheck_compat(self.system(), project, self.expected_toolchain().as_deref())
    }

    /// Layer 3: build a project/workspace around `code`.
    fn scaffold(&self, cfg: &Config, code: &str, name: &str) -> Result<Workspace>;

    /// Layer 2b (build): compile the workspace, collecting errors.
    fn compile(&self, ws: &Workspace) -> Result<CompileReport>;

    /// Layer 2a: audit the proof's axiom/oracle dependencies against `whitelist`.
    fn audit_axioms(&self, ws: &Workspace, thm: &str, whitelist: &[String]) -> Result<AxiomReport>;

    /// Layer 2b (kernel): independent kernel re-check of the compiled artifact.
    fn kernel_recheck(&self, ws: &Workspace) -> Result<RecheckReport>;

    /// Layer 2c (MANDATORY): lexical escape-hatch scan of the raw source.
    fn source_scan(&self, code: &str) -> Result<ScanReport>;

    /// **Layer 2d channel — DESIGNATED INPUTS.** The hypotheses this task
    /// legitimately assumes, named either by BINDER name (`"hGlaisher"`) or by
    /// TYPE HEAD (`"Glaisher3"`).
    ///
    /// [`crate::prover::hypothesis_audit`] exists to catch a theorem that is
    /// silently conditional on unproved mathematics carried in its own signature
    /// — a `Prop`-valued argument that is stated and never proved, or an
    /// assumption-bundling `structure` no instance is ever constructed for.
    /// Neither is visible to `#print axioms` or to a `sorry` scan. But a
    /// *genuine* conditional result ("assuming RH, …") has exactly that shape,
    /// and only the party that defined the task can tell the two apart. This is
    /// where they say so.
    ///
    /// The default is EMPTY — nothing is assumed to be a designated input. That
    /// is the safe default for the audit's verdict, and it is also why the
    /// [`TierZeroGates::hypothesis_discharge`] gate must stay off until a
    /// backend/job actually populates this: with an empty allowlist, every real
    /// conditional theorem is `Unaccounted`.
    fn designated_inputs(&self) -> Vec<String> {
        Vec::new()
    }

    /// **Vacuity channel (1/2) — the goal's HYPOTHESIS BUNDLE**, when the caller
    /// can state it.
    ///
    /// `None` (the default) means "this backend does not model the goal's
    /// bundle", and the vacuity check is reported NOT DECLARED rather than run:
    /// we will not synthesize a bundle we cannot parse and then fail closed on
    /// our own guess. A backend/job that CAN state the bundle returns it here and
    /// pairs it with [`satisfiability_witness`](FormalBackend::satisfiability_witness).
    fn hypothesis_bundle(&self, _stmt: &str) -> Option<crate::prover::vacuity::HypothesisBundle> {
        None
    }

    /// **Vacuity channel (2/2) — the SATISFIABILITY WITNESS** for the bundle: a
    /// concrete instance meeting every field.
    ///
    /// [`crate::prover::vacuity`] catches a goal discharged by making its own
    /// hypotheses contradictory (`(h₁ : x > 5) (h₂ : x < 3) : Goal` proves ANY
    /// goal). Such a proof passes the kernel, the axiom audit AND statement
    /// preservation — every existing layer asks "is this derivation sound?", and
    /// it is. The only defense is to exhibit an instance, and satisfiability is
    /// undecidable in general, so the witness must come from the caller.
    ///
    /// The default is `None`, which the vacuity check treats as fail-closed for
    /// any non-trivial bundle — hence the default-off gate.
    fn satisfiability_witness(
        &self,
        _stmt: &str,
    ) -> Option<crate::prover::vacuity::SatisfiabilityWitness> {
        None
    }

    /// Default 3+1-layer orchestration (compile → axioms ⊆ whitelist → kernel
    /// re-check → source scan). Fail-closed: the proof is trusted only when all
    /// four layers pass. Reuses the existing [`VerificationReport`] fields.
    fn verify(&self, cfg: &Config, code: &str, stmt: &str) -> Result<VerificationReport> {
        self.verify_with_gates(cfg, code, stmt, TierZeroGates::from_config(cfg))
    }

    /// [`verify`](FormalBackend::verify) with the Tier-0 gate switches passed
    /// explicitly instead of read from the environment.
    ///
    /// This is the real implementation; `verify` is the env-reading wrapper.
    /// Callers that know their policy (and tests, which must not race on a
    /// process-global env var) should call this directly.
    ///
    /// Whatever the switches, both Tier-0 reports are COMPUTED and written to
    /// `detail` — the observability is unconditional. The switches decide only
    /// whether the verdicts are conjoined into `lexically_verified`.
    fn verify_with_gates(
        &self,
        cfg: &Config,
        code: &str,
        stmt: &str,
        gates: TierZeroGates,
    ) -> Result<VerificationReport> {
        let system = self.system();
        let name = theorem_name_hint(stmt);
        let ws = self.scaffold(cfg, code, &name)?;
        let compile = self.compile(&ws)?;
        let whitelist = system.axiom_whitelist();
        let axioms = self.audit_axioms(&ws, &ws.entry, &whitelist)?;
        let recheck = self.kernel_recheck(&ws)?;
        let scan = self.source_scan(code)?;

        // Layer 2c is mandatory and layers combine conjunctively (fail-closed).
        let axioms_clean = axioms.within_whitelist;
        // Also reject the non-reproducible proof-search SUGGESTION tactics
        // (`apply?`/`exact?`/`rfl?`) — the DeepSeek-Prover-V2 reward-hacking
        // erratum. These never occur in a legit compiled proof (and not at all in
        // the non-Lean backends), so this only ever tightens. `native_decide` is
        // left to config policy (it has legitimate uses); `sorry`/`admit` are
        // already covered by `scan.clean`.
        let suggestion_hatch = crate::prover::statement_preservation::scan_escape_hatches(code)
            .iter()
            .any(|h| matches!(h.rule, "apply?" | "exact?" | "rfl?"));
        let lexical_clean = scan.clean && !suggestion_hatch;
        let kernel_clean = compile.compiled && recheck.rechecked;
        // The signature checker is the authority for LIVE backends: a lexical
        // mention can be smuggled into a string literal while the submitted
        // declaration proves something else. An unparsable or missing declaration
        // is therefore not evidence of preservation. Mocks retain the legacy
        // lexical report for offline test scaffolding only; their reports are
        // permanently `live: false` and cannot certify a node.
        let preservation =
            crate::prover::statement_preservation::check_entry_signature(system, stmt, code);
        let mentioned = statement_mentioned(stmt, code);
        let statement_preserved = mentioned && (preservation.preserved || self.is_mock());

        // --- Tier 0 layer 2d: hypothesis-discharge audit ---------------------
        // Catches a theorem conditional on unproved mathematics carried in its
        // own signature. Non-Lean systems report NOT APPLICABLE (clean, so this
        // never regresses a backend we cannot parse).
        let allowlist = self.designated_inputs();
        let hypotheses = crate::prover::hypothesis_audit::audit_hypotheses(
            system, stmt, code, &allowlist,
        );
        let hypotheses_discharged = hypotheses.clean;

        // --- Tier 0: vacuous-success guard -----------------------------------
        // Catches a goal discharged by making its hypotheses contradictory —
        // sound, kernel-clean, and worthless. Only runs when the caller declared
        // a bundle; an undeclared bundle is NOT DECLARED, never a guessed fail.
        let bundle = self.hypothesis_bundle(stmt);
        let vacuity = bundle.as_ref().map(|b| {
            crate::prover::vacuity::check_vacuity(b, self.satisfiability_witness(stmt).as_ref())
        });
        let bundle_satisfiable = vacuity.as_ref().map_or(true, |v| v.clean);

        // Conjoin ONLY behind the (default-off) switches. With the gates off this
        // expression is byte-for-byte the historical one.
        let lexically_verified = kernel_clean
            && axioms_clean
            && lexical_clean
            && statement_preserved
            && (!gates.hypothesis_discharge || hypotheses_discharged)
            && (!gates.vacuity || bundle_satisfiable);

        Ok(VerificationReport {
            lexically_verified,
            axioms_clean,
            statement_preserved,
            lexical_clean,
            hardening_clean: Some(kernel_clean),
            // A mock backend's canned kernel layers are NOT a live proof: mark the
            // report non-live so no downstream site can upgrade it to
            // `FormallyVerified`.
            live: !self.is_mock(),
            detail: json!({
                "system": system.as_str(),
                "gate": "3+1-layer",
                "compile": compile,
                "axioms": axioms,
                "kernel_recheck": recheck,
                "source_scan": scan,
                "statement_preservation": preservation,
                "whitelist": whitelist,
                // Tier 0 gates: ALWAYS reported, conjoined only when switched on.
                // `enforced` records which verdicts actually moved
                // `lexically_verified`, so a reader can never mistake an
                // observational finding for a gating one.
                "tier0": {
                    "gates": gates,
                    "hypothesis_audit": {
                        "clean": hypotheses_discharged,
                        "enforced": gates.hypothesis_discharge,
                        "designated_inputs": allowlist,
                        "report": hypotheses,
                    },
                    "vacuity": {
                        "declared": vacuity.is_some(),
                        "clean": bundle_satisfiable,
                        "enforced": gates.vacuity,
                        "report": vacuity,
                    },
                },
            }),
        })
    }
}

/// What positively constitutes a passing compile (see
/// [`FormalBackend::compile_success_signal`]). Exit status is never trusted on
/// its own; this makes each backend's success criterion explicit and auditable.
#[derive(Debug, Clone)]
pub enum SuccessSignal {
    /// The checker sets a correct non-zero exit code on failure, so a clean exit
    /// is a trustworthy pass (Lean, Rocq, Isabelle, Candle, and Agda under
    /// `--safe`, which all fail with a non-zero status).
    NonZeroExitIsHonest,
    /// The exit code is unreliable, so a pass requires the combined stdout+stderr
    /// to contain every `must_contain` marker and none of the `must_not_contain`
    /// markers (Metamath: the "All proofs ... verified" sentinel, and no
    /// `?Error`/`?Warning`). `must_not_contain` is matched case-insensitively.
    StdoutSentinel {
        must_contain: &'static [&'static str],
        must_not_contain: &'static [&'static str],
    },
}

impl SuccessSignal {
    /// Evaluate the signal against a run. `launched` is whether the process
    /// actually started; `exit_success` is the backend's exit-code verdict
    /// (`ExecOutcome::success()`: launched, not timed out, exited zero).
    pub fn is_pass(&self, launched: bool, exit_success: bool, stdout: &str, stderr: &str) -> bool {
        match self {
            SuccessSignal::NonZeroExitIsHonest => exit_success,
            SuccessSignal::StdoutSentinel {
                must_contain,
                must_not_contain,
            } => {
                if !launched {
                    return false;
                }
                let combined = format!("{stdout}\n{stderr}");
                let lc = combined.to_lowercase();
                must_contain.iter().all(|m| combined.contains(m))
                    && must_not_contain
                        .iter()
                        .all(|m| !lc.contains(&m.to_lowercase()))
            }
        }
    }
}

// --- warm-driver session contract ----------------------------------------

/// Result of submitting a whole theory/file (`submit_unit`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnitResult {
    pub ok: bool,
    pub messages: Vec<String>,
    #[serde(default)]
    pub detail: Value,
}

/// Opaque proof-state handle for tactic stepping (Lean `proofState` id / Rocq
/// SerAPI state id / Petanque `Run_result`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateResult {
    pub state: u64,
    pub finished: bool,
    #[serde(default)]
    pub detail: Value,
}

/// The pretty-printed goal(s) at a proof state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalState {
    pub goals: Vec<String>,
    #[serde(default)]
    pub detail: Value,
}

/// Errors from a [`ProofSession`], distinguishing the "this system does not
/// support tactic stepping" case (Isabelle) from backend faults.
#[derive(Debug)]
pub enum SessionError {
    /// The operation is not supported by this system (e.g. `step_tactic` on
    /// theory-file-granular Isabelle).
    Unsupported(&'static str),
    /// A backend/driver fault.
    Backend(String),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::Unsupported(what) => write!(f, "unsupported operation: {what}"),
            SessionError::Backend(msg) => write!(f, "session backend error: {msg}"),
        }
    }
}

impl std::error::Error for SessionError {}

/// Generalizes the warm Lean REPL (`lean_session.rs`). Carries both a coarse
/// `submit_unit` (all three systems) and an optional `step_tactic` (Lean/Rocq;
/// Isabelle returns [`SessionError::Unsupported`]).
pub trait ProofSession {
    /// Warm the driver against a project.
    fn start(&mut self, project: &crate::prover::model::FormalProject) -> Result<()>;

    /// Submit a whole theory/file and parse the result (all systems).
    fn submit_unit(&mut self, code: &str) -> Result<UnitResult>;

    /// Advance one tactic from a proof `state` (Lean/Rocq only). Isabelle must
    /// return `Err(SessionError::Unsupported(..))`.
    fn step_tactic(&mut self, state: u64, tactic: &str) -> Result<StateResult>;

    /// Pretty-print the goal(s) at `state`.
    fn goal_state(&self, state: u64) -> Result<GoalState>;
}

// --- shared helpers -------------------------------------------------------

/// Extract a plausible theorem name from a statement header
/// (`theorem foo : …` / `Theorem foo : …` / `lemma foo: …`), falling back to a
/// stable default.
pub(crate) fn theorem_name_hint(stmt: &str) -> String {
    let low = stmt.trim_start();
    for kw in ["theorem", "Theorem", "lemma", "Lemma"] {
        if let Some(rest) = low.strip_prefix(kw) {
            if let Some(name) = rest.split_whitespace().next() {
                let name = name.trim_end_matches([':', '(']);
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
    }
    "MainTheorem".to_string()
}

/// Given the first two chars of a potential block-comment opener, return the
/// two-char closer it expects, or `None` if this is not an opener. Covers the
/// supported systems: `(* *)` (Rocq/Isabelle/HOL), `/- -/` (Lean), `{- -}`
/// (Agda), `$( $)` (Metamath). All delimiters are ASCII, so these comparisons
/// never match a multi-byte char and never split one.
fn block_closer(c: char, next: Option<char>) -> Option<(char, char)> {
    match (c, next) {
        ('(', Some('*')) => Some(('*', ')')),
        ('/', Some('-')) => Some(('-', '/')),
        ('{', Some('-')) => Some(('-', '}')),
        ('$', Some('(')) => Some(('$', ')')),
        _ => None,
    }
}

/// Remove comments across the supported formal systems, replacing every stripped
/// char with a space so token boundaries are preserved and no substring bridges
/// across a removed comment. Handles nestable block comments `(* *)`, `/- -/`,
/// `{- -}`, the non-nesting Metamath `$( $)`, and line comments `--` and `//`.
/// `#` is intentionally left untouched (too ambiguous). Conservative bias: when
/// a delimiter is ambiguous we strip, which only makes the caller stricter.
///
/// **Agda pragma exemption.** `{-# ... #-}` is NOT a comment — it is a pragma,
/// and Agda acts on it (`{-# OPTIONS --allow-unsolved-metas #-}` disables the
/// unsolved-meta check; `{-# COMPILED ... #-}` escapes Agda's semantics into
/// foreign code). Blanket-stripping it would blind the very checks the offline
/// source scan exists to run, so a well-formed pragma is copied through
/// VERBATIM — including any `--` inside it, which is an OPTIONS flag and not a
/// line comment. Only a pragma opener with no matching `#-}` falls back to
/// being treated as an ordinary `{- -}` block comment (strip-on-ambiguity).
/// Nothing here affects Lean (`/- -/`, `--`), Rocq/Isabelle/HOL (`(* *)`) or
/// Metamath (`$( $)`): none of them can produce a `{-#` opener.
///
/// Operates on a `Vec<char>` and only ever compares against ASCII delimiter
/// chars, so it is panic-free on arbitrary UTF-8 (multi-byte chars simply never
/// match a delimiter and are copied through unchanged).
///
/// `pub(crate)` so the per-system entry-signature checks in
/// [`crate::prover::statement_preservation`] can reuse the same multi-system
/// comment stripping (Agda `{- -}` / `--`, Metamath `$( $)`) before parsing.
pub(crate) fn strip_comments(code: &str) -> String {
    let chars: Vec<char> = code.chars().collect();
    let mut out = String::with_capacity(chars.len());
    let mut i = 0usize;
    // Stack of expected block-comment closers, supporting nested block comments.
    let mut stack: Vec<(char, char)> = Vec::new();
    let mut in_line = false;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if in_line {
            if c == '\n' {
                in_line = false;
                out.push('\n');
            } else {
                out.push(' ');
            }
            i += 1;
            continue;
        }
        if let Some(&(a, b)) = stack.last() {
            // Inside a block comment: a matching closer pops; a nested opener
            // pushes; everything else is blanked (newlines preserved).
            if c == a && next == Some(b) {
                stack.pop();
                out.push(' ');
                out.push(' ');
                i += 2;
                continue;
            }
            if let Some(close) = block_closer(c, next) {
                stack.push(close);
                out.push(' ');
                out.push(' ');
                i += 2;
                continue;
            }
            out.push(if c == '\n' { '\n' } else { ' ' });
            i += 1;
            continue;
        }
        // Agda pragma `{-# ... #-}`: not a comment. Copy it through verbatim
        // when it is properly closed; otherwise fall through and treat it as a
        // `{- -}` block comment (conservative bias).
        if c == '{' && next == Some('-') && chars.get(i + 2) == Some(&'#') {
            if let Some(end) = pragma_end(&chars, i + 3) {
                for &pc in &chars[i..end] {
                    out.push(pc);
                }
                i = end;
                continue;
            }
        }
        if let Some(close) = block_closer(c, next) {
            stack.push(close);
            out.push(' ');
            out.push(' ');
            i += 2;
            continue;
        }
        if (c == '-' && next == Some('-')) || (c == '/' && next == Some('/')) {
            in_line = true;
            out.push(' ');
            out.push(' ');
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Index just past the `#-}` closing an Agda pragma whose body starts at
/// `from`, or `None` if the pragma is never closed. Pure lookahead: it consumes
/// nothing, so an unclosed opener stays ambiguous and [`strip_comments`] falls
/// back to stripping it. ASCII-only comparisons, so panic-free on arbitrary
/// UTF-8.
fn pragma_end(chars: &[char], from: usize) -> Option<usize> {
    let mut j = from;
    while j + 2 < chars.len() {
        if chars[j] == '#' && chars[j + 1] == '-' && chars[j + 2] == '}' {
            return Some(j + 3);
        }
        j += 1;
    }
    None
}

// --- shared escape-hatch token table (alias-expanded) ---------------------
//
// WHY THIS EXISTS. Every backend used to carry its own private list of banned
// literals matched with `str::contains`. Two things were wrong with that:
//
//  1. ALIASES. A ban a model can sidestep by renaming is worse than no ban,
//     because it reads as protection. `native_decide` has the config-syntax
//     alias `decide +native`; Rocq's `Admitted` has the tactic form `admit` and
//     the command forms `Parameter` / `Hypothesis` / `Variable` / `Conjecture`
//     that are the SAME escape hatch as `Axiom`; Isabelle's `oracle` has
//     `Thm.add_oracle` and `axiomatization`. Each list had a different subset,
//     so which rename worked depended on which backend you happened to hit.
//  2. SUBSTRING MATCHING. `contains("admit")` also fires on `admits_a_root`,
//     and `contains("sorry")` on `my_sorry'`. Over-rejection is not free: it
//     costs a retry on an artifact that was fine.
//
// So: ONE table, keyed by system, alias-expanded, matched on WORD BOUNDARIES.
// The tables deliberately mirror the CRITICAL rules of the authoritative online
// scanner (`components/verify/python/theoremata_tools/formal_source_scan.py`),
// so the offline fallback and the worker reject the same set instead of the
// offline path being quietly laxer.
//
// BOUNDARY TRADE-OFF (chosen deliberately). A token only needs a boundary on
// the ends that are themselves identifier chars: `admit` must not match inside
// `admits`, but `--unsafe` needs no boundary before the `-`. Identifier chars
// are alphanumeric, `_`, and `'` (Lean admits the prime, so `sorry'` is a
// DIFFERENT name than `sorry`). The cost of this choice is that a token can no
// longer be caught as a substring of a longer alias, so every alias must be
// listed explicitly -- `sorryAx` does NOT match the `sorry` token and is listed
// on its own line. Under-matching is the real hole; the explicit alias list is
// how we pay for the boundary.

/// Identifier char for escape-hatch token boundaries. `'` is included because
/// Lean admits the prime in identifiers.
fn is_hatch_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '\''
}

/// Word-boundary containment of `token` (already lowercase) in `lowered`
/// (already lowercase chars). A boundary is required only on an end whose token
/// char is itself an identifier char, so `--unsafe` / `+native` / `{-# compiled`
/// still match where they legitimately appear.
fn contains_hatch_token(lowered: &[char], token: &str) -> bool {
    let n: Vec<char> = token.chars().collect();
    if n.is_empty() || lowered.len() < n.len() {
        return false;
    }
    let head_is_word = n.first().map_or(false, |&c| is_hatch_word(c));
    let tail_is_word = n.last().map_or(false, |&c| is_hatch_word(c));
    for i in 0..=(lowered.len() - n.len()) {
        if lowered[i..i + n.len()] != n[..] {
            continue;
        }
        let before_ok = !head_is_word || i == 0 || !is_hatch_word(lowered[i - 1]);
        let after_ok =
            !tail_is_word || lowered.get(i + n.len()).map_or(true, |&c| !is_hatch_word(c));
        if before_ok && after_ok {
            return true;
        }
    }
    false
}

/// Lean escape hatches, alias-expanded. `native_decide` and its config-syntax
/// twin `decide +native` are the same trust hole (the compiled evaluator, which
/// the kernel never re-checks); `sorryAx` is the axiom `sorry` elaborates to and
/// does NOT match the `sorry` token under word boundaries; `ofReduceNat` is the
/// sibling of `ofReduceBool`.
const LEAN_HATCH_TOKENS: &[&str] = &[
    "sorry",
    "sorryAx",
    "admit",
    "native_decide",
    // `decide +native` (and `decide +kernel +native`) is `native_decide` under
    // another spelling. Matched as the bare `+native` config flag so the flag
    // order inside the config list does not matter. A preceding identifier char
    // suppresses the match, so the arithmetic `x+native` is not flagged.
    "+native",
    "ofReduceBool",
    "ofReduceNat",
    "trustCompiler",
    // `open private f in …` pulls a PRIVATE declaration into scope under its real
    // name. Private is Lean's only encapsulation boundary, so this is the way to
    // reach an internal lemma a module deliberately did not export, or to shadow
    // a name the surrounding proof is read as using. Neither the kernel nor
    // `#print axioms` says anything about it. `open Foo` on its own is ordinary
    // and stays allowed.
    "open private",
];

/// Rocq escape hatches, alias-expanded. `Admitted` (command), `admit` (tactic)
/// and `Admit Obligations` are one hatch; `Axiom` has the exact synonyms
/// `Axioms` / `Parameter(s)` / `Conjecture(s)` / `Hypothesis`/`Hypotheses` /
/// `Variable(s)`, all of which introduce an undischarged assumption; the three
/// `Unset ... Checking` commands disable kernel checks without an axiom.
const ROCQ_HATCH_TOKENS: &[&str] = &[
    "Admitted",
    "admit",
    "Admit Obligations",
    "Axiom",
    "Axioms",
    "Parameter",
    "Parameters",
    "Conjecture",
    "Conjectures",
    "Hypothesis",
    "Hypotheses",
    "Variable",
    "Variables",
    "-type-in-type",
    "type_in_type",
    "Unset Universe Checking",
    "Unset Guard Checking",
    "Unset Positivity Checking",
    "bypass_check",
];

/// Isabelle escape hatches, alias-expanded. `oracle` no longer catches
/// `Thm.add_oracle` or `oracles` as substrings once boundaries are enforced, so
/// both are listed; `axiomatization` / `axioms` assert facts by fiat exactly as
/// an oracle does.
const ISABELLE_HATCH_TOKENS: &[&str] = &[
    "sorry",
    "oops",
    "quick_and_dirty",
    "skip_proof",
    "oracle",
    "oracles",
    "add_oracle",
    "axiomatization",
    "axioms",
];

/// Agda escape hatches, alias-expanded: `postulate` plus the whole family of
/// checker-disabling `OPTIONS` flags (each one is a rename of the same "turn a
/// check off" move) and `primTrustMe`.
const AGDA_HATCH_TOKENS: &[&str] = &[
    "postulate",
    "--allow-unsolved-metas",
    "--type-in-type",
    "--unsafe",
    "--no-termination-check",
    "--no-positivity-check",
    "--no-coverage-check",
    "primTrustMe",
    "{-# COMPILED",
];

/// The alias-expanded escape-hatch token table for `system`.
///
/// Candle and Metamath return an empty slice on purpose: Candle's hatches are
/// audited structurally by [`crate::prover::axiom_audit::audit_hol_light`] and
/// Metamath's by the backend's own `$a` / `?` / include scan, neither of which
/// is a token list.
pub(crate) fn escape_hatch_tokens(system: FormalSystem) -> &'static [&'static str] {
    match system {
        FormalSystem::Lean => LEAN_HATCH_TOKENS,
        FormalSystem::Rocq => ROCQ_HATCH_TOKENS,
        FormalSystem::Isabelle => ISABELLE_HATCH_TOKENS,
        FormalSystem::Agda => AGDA_HATCH_TOKENS,
        FormalSystem::Candle | FormalSystem::Metamath => &[],
    }
}

/// Which of `system`'s escape-hatch tokens occur in `code`, in table order.
///
/// Matched over COMMENT-STRIPPED source, so this agrees with
/// [`crate::prover::statement_preservation::ESCAPE_HATCH_COMMENT_POLICY`]
/// (`CodeOnly`): a commented hatch is never seen by the kernel and so is not a
/// soundness violation. Matching is case-insensitive (Rocq commands are
/// capitalized, Lean tactics are not) but each finding is reported in the
/// table's canonical spelling.
pub(crate) fn escape_hatch_findings(system: FormalSystem, code: &str) -> Vec<String> {
    // Whitespace is normalized as well as comments stripped, so a multi-word
    // token matches however the source broke the line between its words
    // (`open\n  private foo in …`, `Unset\n  Universe Checking`). Single-token
    // patterns are unaffected: a token cannot span whitespace in the first place.
    let stripped = strip_comments(code).to_lowercase();
    let normalized = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
    let lowered: Vec<char> = normalized.chars().collect();
    escape_hatch_tokens(system)
        .iter()
        .filter(|t| contains_hatch_token(&lowered, &t.to_lowercase()))
        .map(|t| (*t).to_string())
        .collect()
}

/// Cheap lexical "the code is about this statement" check: the statement's
/// leading identifier/head appears in the whitespace-normalized source.
///
/// Comments are stripped from `code` first so a statement hidden in a comment
/// (e.g. `-- theorem foo : goal` or `(* ... goal ... *)`) cannot satisfy the
/// check — a trust-boundary concern for this statement-preservation fallback.
pub(crate) fn statement_mentioned(stmt: &str, code: &str) -> bool {
    let stripped = strip_comments(code);
    let code_norm: String = stripped.split_whitespace().collect();
    let stmt_norm: String = stmt.split_whitespace().collect();
    if stmt_norm.is_empty() {
        return false;
    }
    if code_norm.contains(&stmt_norm) {
        return true;
    }
    // Fall back to the head (before the first `:`), e.g. the theorem name.
    stmt.split(':')
        .next()
        .map(|head| {
            let head_norm: String = head.split_whitespace().collect();
            !head_norm.is_empty() && code_norm.contains(&head_norm)
        })
        .unwrap_or(false)
}

// --- live-gate shared helpers (Phase 2) ----------------------------------

/// Extract the declared entry (theorem/lemma) name from generated source for a
/// system, so the axiom audit targets the real declaration rather than a guess
/// derived from the statement. Falls back to `None` when nothing matches.
pub(crate) fn entry_name(system: FormalSystem, code: &str) -> Option<String> {
    let keywords: &[&str] = match system {
        FormalSystem::Lean => &["theorem", "lemma", "example", "def"],
        FormalSystem::Rocq => &[
            "Theorem",
            "Lemma",
            "Corollary",
            "Proposition",
            "Example",
            "Fact",
            "Remark",
            "Definition",
        ],
        FormalSystem::Isabelle => &["theorem", "lemma", "corollary", "proposition"],
        // HOL Light theorems are OCaml let-bindings: `let FOO = prove(...)`.
        FormalSystem::Candle => &["let"],
        FormalSystem::Agda => &["module", "data", "record", "postulate"],
        FormalSystem::Metamath => &["$p", "$a"],
    };
    for raw in code.lines() {
        let line = raw.trim_start();
        for kw in keywords {
            if let Some(rest) = line.strip_prefix(kw) {
                // Require a separator after the keyword so `theorematic` etc. do
                // not match.
                if !rest.starts_with(|c: char| c.is_whitespace()) {
                    continue;
                }
                let name: String = rest
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '\'' | '.'))
                    .collect();
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }
    }
    None
}

/// Reject-on-mismatch compatibility check (open-atp `check_compatible`). Returns
/// an incompatible report only when BOTH the project and the backend declare a
/// pin AND they disagree (whitespace-insensitive). A missing pin on either side
/// is treated as compatible — we never fail-closed on an *unknown* pin, only on a
/// *known mismatch* (the cheap "do not spend compute on a doomed build" gate).
pub fn precheck_compat(
    system: FormalSystem,
    project: Option<&crate::prover::model::FormalProject>,
    expected: Option<&str>,
) -> PrecheckReport {
    let declared = project
        .and_then(|p| p.toolchain.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let expected = expected.map(str::trim).filter(|s| !s.is_empty());
    match (declared, expected) {
        (Some(d), Some(e)) if d != e => PrecheckReport::reject(
            format!("{system} toolchain mismatch: project pins `{d}` but backend is `{e}`"),
            json!({"system": system.as_str(), "declared": d, "expected": e}),
        ),
        _ => PrecheckReport {
            compatible: true,
            reason: String::new(),
            detail: json!({
                "system": system.as_str(),
                "declared": declared,
                "expected": expected,
            }),
        },
    }
}

/// Every declared entry (theorem/lemma/def/…) name in `code`, in source order —
/// the multi-declaration companion to [`entry_name`], used to attribute compile
/// errors per declaration (open-atp per-file isolation).
pub(crate) fn all_entry_names(system: FormalSystem, code: &str) -> Vec<String> {
    let keywords: &[&str] = match system {
        FormalSystem::Lean => &["theorem", "lemma", "example", "def"],
        FormalSystem::Rocq => &[
            "Theorem",
            "Lemma",
            "Corollary",
            "Proposition",
            "Example",
            "Fact",
            "Remark",
            "Definition",
        ],
        FormalSystem::Isabelle => &["theorem", "lemma", "corollary", "proposition"],
        // HOL Light theorems are OCaml let-bindings: `let FOO = prove(...)`.
        FormalSystem::Candle => &["let"],
        FormalSystem::Agda => &["module", "data", "record", "postulate"],
        FormalSystem::Metamath => &["$p", "$a"],
    };
    let mut out = Vec::new();
    for raw in code.lines() {
        let line = raw.trim_start();
        for kw in keywords {
            if let Some(rest) = line.strip_prefix(kw) {
                if !rest.starts_with(|c: char| c.is_whitespace()) {
                    continue;
                }
                let name: String = rest
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '\'' | '.'))
                    .collect();
                if !name.is_empty() && !out.contains(&name) {
                    out.push(name);
                }
            }
        }
    }
    out
}

/// Failure-isolating per-declaration status (open-atp `_parse_per_file`): mark a
/// declaration failed when any compiler error message mentions its name, else
/// passed. When the whole file compiled, every declaration is `ok`. Declarations
/// not referenced by any error survive even if a *sibling* failed — so a
/// partially-good artifact is visible rather than collapsing to one boolean.
pub(crate) fn per_declaration_status(
    system: FormalSystem,
    code: &str,
    compiled: bool,
    errors: &[String],
) -> Vec<UnitStatus> {
    let names = all_entry_names(system, code);
    let blob = errors.join("\n");
    names
        .into_iter()
        .map(|name| {
            let ok = compiled || !blob.contains(&name);
            UnitStatus { name, ok }
        })
        .collect()
}

/// Create (and `create_dir_all`) a unique, canonicalized workspace directory for
/// a live scaffold under the state dir (`.theoremata/formal/<system>/<uuid>`).
/// Canonicalization yields an absolute path so WSL/Docker runners can translate
/// or bind-mount it.
pub(crate) fn live_workspace_dir(cfg: &Config, system: FormalSystem) -> Result<PathBuf> {
    let base = cfg
        .workspace
        .parent()
        .map(|p| p.join("formal"))
        .unwrap_or_else(|| PathBuf::from(".theoremata/formal"));
    let dir = base
        .join(system.as_str())
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir)?;
    Ok(std::fs::canonicalize(&dir).unwrap_or(dir))
}

/// Run the Python `source_scan` worker (layer 2c) for `system`, returning a
/// [`ScanReport`] when the worker is available and answered, else `None` (the
/// caller then falls back to its built-in lexical patterns).
pub(crate) fn worker_source_scan(system: FormalSystem, code: &str) -> Option<ScanReport> {
    use crate::tools::{PythonCheck, Tool};
    let py = PythonCheck::new();
    if !py.available() {
        return None;
    }
    let result = py
        .run(json!({"tool": "source_scan", "system": system.as_str(), "source": code}))
        .ok()?;
    let v: Value = serde_json::from_str(&result.stdout).ok()?;
    if !v.get("ok")?.as_bool()? {
        return None;
    }
    let output = v.get("output")?;
    let clean = output.get("clean")?.as_bool()?;
    let findings = output
        .get("flags")
        .and_then(Value::as_array)
        .map(|flags| {
            flags
                .iter()
                .filter(|f| f.get("severity").and_then(Value::as_str) == Some("critical"))
                .filter_map(|f| f.get("pattern").and_then(Value::as_str).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    Some(ScanReport {
        clean,
        findings,
        detail: json!({"system": system.as_str(), "worker": output}),
    })
}

/// Build the live or mock [`FormalBackend`] for `system`.
pub fn backend_for(cfg: &Config, system: FormalSystem, mock: bool) -> Box<dyn FormalBackend> {
    match (system, mock) {
        (FormalSystem::Lean, true) => Box::new(crate::prover::lean::LeanBackend::mock()),
        (FormalSystem::Lean, false) => Box::new(crate::prover::lean::LeanBackend::live(cfg)),
        (FormalSystem::Rocq, true) => Box::new(crate::prover::rocq::RocqBackend::mock()),
        (FormalSystem::Rocq, false) => Box::new(crate::prover::rocq::RocqBackend::live(cfg)),
        (FormalSystem::Isabelle, true) => {
            Box::new(crate::prover::isabelle::IsabelleBackend::mock())
        }
        (FormalSystem::Isabelle, false) => {
            Box::new(crate::prover::isabelle::IsabelleBackend::live(cfg))
        }
        (FormalSystem::Candle, true) => {
            Box::new(crate::prover::backends::candle::CandleBackend::mock())
        }
        (FormalSystem::Candle, false) => {
            Box::new(crate::prover::backends::candle::CandleBackend::live(cfg))
        }
        (FormalSystem::Agda, mock) => Box::new(
            crate::prover::backends::external::ExternalBackend::new(cfg, FormalSystem::Agda, mock),
        ),
        (FormalSystem::Metamath, mock) => {
            Box::new(crate::prover::backends::external::ExternalBackend::new(
                cfg,
                FormalSystem::Metamath,
                mock,
            ))
        }
    }
}

fn write_artifact(dir: &Path, rel: &str, value: &impl Serialize) -> Result<()> {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(value)?)?;
    Ok(())
}

/// Drive a non-mock proof job to a terminal state through the real 3+1-layer
/// gate. The candidate proof is taken from `task.stub`; the backend probes its
/// toolchain first and FAILS CLOSED (`Error`) when unavailable — a live gate
/// never silently passes an un-verified proof.
pub fn live_poll(
    store: &Store,
    cfg: &Config,
    mut job: ProofJob,
    backend_name: &str,
    system: FormalSystem,
) -> Result<ProofJob> {
    let started = Instant::now();
    let backend = backend_for(cfg, system, false);
    let code = job.task.stub.clone();

    // Reject-on-mismatch PRECHECK before spending any compute (open-atp): a
    // project whose pinned toolchain disagrees with the backend fails fast.
    let precheck = backend.precheck(Some(&job.task.formal_project));

    let (status, verification, message) = if !precheck.compatible {
        (
            ProverJobStatus::Error,
            None,
            format!("precheck rejected: {}", precheck.reason),
        )
    } else if !backend.available() {
        (
            ProverJobStatus::Error,
            None,
            format!("{system} toolchain unavailable (fail-closed)"),
        )
    } else if let Some(code) = code.clone() {
        match backend.verify(cfg, &code, &job.task.statement) {
            Ok(v) => {
                let ok = v.lexically_verified;
                (
                    if ok {
                        ProverJobStatus::Proved
                    } else {
                        ProverJobStatus::Failed
                    },
                    Some(v),
                    if ok {
                        "live: verified through the 3+1-layer gate".to_string()
                    } else {
                        "live: rejected by the 3+1-layer gate".to_string()
                    },
                )
            }
            Err(e) => (
                ProverJobStatus::Error,
                None,
                format!("live verify error: {e}"),
            ),
        }
    } else {
        (
            ProverJobStatus::Failed,
            None,
            "live job requires a candidate proof in task.stub".to_string(),
        )
    };

    job.status = status;
    job.percent_complete = 100.0;
    job.poll_count += 1;
    job.completed_at = Some(Utc::now());
    job.updated_at = Utc::now();

    let result = ProofResult {
        task_id: job.task.id.clone(),
        job_id: job.id.clone(),
        status,
        formal_code: code,
        counterexample: None,
        verification,
        artifacts_dir: job.artifacts_dir.clone(),
        duration_ms: started.elapsed().as_millis(),
        cost: None,
        message: Some(message),
        provenance: json!({
            "backend": backend_name,
            "system": system.as_str(),
            "mock": false,
            "runner": cfg.formal_runners.for_system(system).tag(),
            "poll_count": job.poll_count,
            "precheck": precheck,
        }),
    };

    if let Some(dir) = &job.artifacts_dir {
        if let Some(c) = &result.formal_code {
            let sub = dir.join(backend_name);
            std::fs::create_dir_all(&sub)?;
            std::fs::write(
                sub.join(format!("solution{}", system.source_extension())),
                c,
            )?;
        }
        write_artifact(dir, "result.json", &result)?;
        if let Some(v) = &result.verification {
            write_artifact(dir, "verifier/report.json", v)?;
        }
    }

    job.result = Some(result);
    store.update_proof_job(&job)?;
    store.event(
        job.project_id.as_deref(),
        None,
        "proof_job.completed",
        backend_name,
        json!({"job_id": job.id, "status": status, "live": true}),
    )?;
    Ok(job)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prover::model::FormalProject;

    fn project_with_toolchain(system: FormalSystem, toolchain: Option<&str>) -> FormalProject {
        FormalProject {
            system,
            root: PathBuf::from("."),
            toolchain: toolchain.map(str::to_owned),
            imports: Vec::new(),
            metadata: json!({}),
        }
    }

    // --- Tier 0 gate plumbing --------------------------------------------
    //
    // A minimal backend whose four classic layers all PASS, so the only thing
    // that can move `lexically_verified` in these tests is the Tier-0 wiring.

    use crate::prover::vacuity::{HypothesisBundle, HypothesisField, SatisfiabilityWitness};

    #[derive(Default)]
    struct GateTestBackend {
        inputs: Vec<String>,
        bundle: Option<HypothesisBundle>,
        witness: Option<SatisfiabilityWitness>,
    }

    impl FormalBackend for GateTestBackend {
        fn system(&self) -> FormalSystem {
            FormalSystem::Lean
        }
        fn compile_success_signal(&self) -> SuccessSignal {
            SuccessSignal::NonZeroExitIsHonest
        }
        // Mock, so `statement_preserved` needs only the lexical mention and the
        // test does not depend on the signature checker's parse.
        fn is_mock(&self) -> bool {
            true
        }
        fn scaffold(&self, _cfg: &Config, _code: &str, name: &str) -> Result<Workspace> {
            Ok(Workspace {
                system: FormalSystem::Lean,
                root: PathBuf::from("."),
                source_path: PathBuf::from("./Generated.lean"),
                entry: name.to_string(),
            })
        }
        fn compile(&self, _ws: &Workspace) -> Result<CompileReport> {
            Ok(CompileReport {
                compiled: true,
                errors: Vec::new(),
                per_unit: Vec::new(),
                detail: json!({}),
            })
        }
        fn audit_axioms(
            &self,
            _ws: &Workspace,
            _thm: &str,
            _whitelist: &[String],
        ) -> Result<AxiomReport> {
            Ok(AxiomReport {
                axioms: Vec::new(),
                within_whitelist: true,
                detail: json!({}),
            })
        }
        fn kernel_recheck(&self, _ws: &Workspace) -> Result<RecheckReport> {
            Ok(RecheckReport {
                rechecked: true,
                detail: json!({}),
            })
        }
        fn source_scan(&self, _code: &str) -> Result<ScanReport> {
            Ok(ScanReport {
                clean: true,
                findings: Vec::new(),
                detail: json!({}),
            })
        }
        fn designated_inputs(&self) -> Vec<String> {
            self.inputs.clone()
        }
        fn hypothesis_bundle(&self, _stmt: &str) -> Option<HypothesisBundle> {
            self.bundle.clone()
        }
        fn satisfiability_witness(&self, _stmt: &str) -> Option<SatisfiabilityWitness> {
            self.witness.clone()
        }
    }

    /// A theorem conditional on a `Prop` that is STATED and never PROVED — the
    /// mechanism-(a) hole the hypothesis audit exists to catch. Every classic
    /// layer passes on it.
    const COND_STMT: &str = "theorem phi3 (hG : Glaisher3) : True";
    const COND_CODE: &str = "\
def Glaisher3 : Prop := True

theorem phi3 (hG : Glaisher3) : True := trivial
";

    /// A non-trivial hypothesis bundle with no witness — the fail-closed case.
    fn unwitnessed_bundle() -> HypothesisBundle {
        HypothesisBundle::new(
            "phi3",
            vec![
                HypothesisField::datum("n", "Nat"),
                HypothesisField::hypothesis("hn", "n > 5"),
            ],
        )
    }

    fn tier0<'a>(report: &'a VerificationReport) -> &'a Value {
        &report.detail["tier0"]
    }

    /// GATES OFF (the default): a proof that would fail BOTH Tier-0 checks
    /// verifies exactly as it does today — and both reports are nonetheless
    /// present in `detail`.
    #[test]
    fn tier0_reports_are_observational_when_the_gates_are_off() {
        let backend = GateTestBackend {
            inputs: Vec::new(),
            bundle: Some(unwitnessed_bundle()),
            witness: None,
        };
        let cfg = Config::default();
        let report = backend
            .verify_with_gates(&cfg, COND_CODE, COND_STMT, TierZeroGates::OFF)
            .expect("verify must succeed");

        // Behavior preserved: the four classic layers decide, and they all pass.
        assert!(
            report.lexically_verified,
            "gates off must not change the verdict: {:#?}",
            report.detail
        );
        assert!(report.axioms_clean);
        assert!(report.lexical_clean);
        assert!(report.statement_preserved);

        // ...yet BOTH findings are visible.
        let t = tier0(&report);
        assert_eq!(t["hypothesis_audit"]["clean"], json!(false));
        assert_eq!(t["hypothesis_audit"]["enforced"], json!(false));
        assert_eq!(t["vacuity"]["declared"], json!(true));
        assert_eq!(t["vacuity"]["clean"], json!(false));
        assert_eq!(t["vacuity"]["enforced"], json!(false));
        // The detail names the culprits, not just a boolean.
        let rendered = t.to_string();
        assert!(rendered.contains("Glaisher3"), "{rendered}");
        assert!(rendered.contains("witness_missing"), "{rendered}");
    }

    /// GATE ON: the same submission fails closed on the hypothesis audit alone.
    #[test]
    fn hypothesis_gate_fails_closed_when_switched_on() {
        let backend = GateTestBackend::default();
        let cfg = Config::default();
        let gates = TierZeroGates {
            hypothesis_discharge: true,
            vacuity: false,
        };
        let report = backend
            .verify_with_gates(&cfg, COND_CODE, COND_STMT, gates)
            .unwrap();
        assert!(
            !report.lexically_verified,
            "an undischarged hypothesis must fail the gate when it is enforced"
        );
        // The classic layers are untouched — this is a NEW, separable verdict.
        assert!(report.axioms_clean && report.lexical_clean && report.statement_preserved);
        assert_eq!(tier0(&report)["hypothesis_audit"]["enforced"], json!(true));
    }

    /// The ALLOWLIST channel: declaring the hypothesis a designated input clears
    /// the same gate on the same submission.
    #[test]
    fn designated_inputs_clear_the_hypothesis_gate() {
        let cfg = Config::default();
        let gates = TierZeroGates {
            hypothesis_discharge: true,
            vacuity: false,
        };
        // By type head...
        let by_head = GateTestBackend {
            inputs: vec!["Glaisher3".to_string()],
            ..Default::default()
        };
        assert!(by_head
            .verify_with_gates(&cfg, COND_CODE, COND_STMT, gates)
            .unwrap()
            .lexically_verified);
        // ...and by binder name.
        let by_binder = GateTestBackend {
            inputs: vec!["hG".to_string()],
            ..Default::default()
        };
        assert!(by_binder
            .verify_with_gates(&cfg, COND_CODE, COND_STMT, gates)
            .unwrap()
            .lexically_verified);
    }

    /// GATE ON: a declared bundle with no witness fails closed.
    #[test]
    fn vacuity_gate_fails_closed_without_a_witness() {
        let backend = GateTestBackend {
            inputs: vec!["Glaisher3".to_string()],
            bundle: Some(unwitnessed_bundle()),
            witness: None,
        };
        let cfg = Config::default();
        let gates = TierZeroGates {
            hypothesis_discharge: false,
            vacuity: true,
        };
        let report = backend
            .verify_with_gates(&cfg, COND_CODE, COND_STMT, gates)
            .unwrap();
        assert!(
            !report.lexically_verified,
            "no witness must fail closed: absence of a witness is not evidence \
             of satisfiability"
        );
    }

    /// The WITNESS channel: supplying a satisfying instance clears the gate.
    #[test]
    fn a_supplied_witness_clears_the_vacuity_gate() {
        let backend = GateTestBackend {
            inputs: vec!["Glaisher3".to_string()],
            bundle: Some(unwitnessed_bundle()),
            witness: Some(
                SatisfiabilityWitness::new("n := 7")
                    .bind("n", json!(7))
                    .claim("hn"),
            ),
        };
        let cfg = Config::default();
        let report = backend
            .verify_with_gates(&cfg, COND_CODE, COND_STMT, TierZeroGates::ON)
            .unwrap();
        assert!(
            report.lexically_verified,
            "a witnessed bundle + allowlisted input must pass: {:#?}",
            report.detail
        );
        assert_eq!(tier0(&report)["vacuity"]["clean"], json!(true));
    }

    /// A witness that does NOT satisfy the bundle is rejected — the channel is a
    /// real check, not a rubber stamp.
    #[test]
    fn a_violating_witness_does_not_clear_the_vacuity_gate() {
        let backend = GateTestBackend {
            inputs: vec!["Glaisher3".to_string()],
            bundle: Some(unwitnessed_bundle()), // hn : n > 5
            witness: Some(
                SatisfiabilityWitness::new("n := 2")
                    .bind("n", json!(2))
                    .claim("hn"),
            ),
        };
        let cfg = Config::default();
        assert!(!backend
            .verify_with_gates(&cfg, COND_CODE, COND_STMT, TierZeroGates::ON)
            .unwrap()
            .lexically_verified);
    }

    /// An UNDECLARED bundle is reported as such and never fails closed on a
    /// bundle we had to guess at — even with the gate enforcing.
    #[test]
    fn an_undeclared_bundle_is_not_declared_rather_than_failed() {
        let backend = GateTestBackend {
            inputs: vec!["Glaisher3".to_string()],
            bundle: None,
            witness: None,
        };
        let cfg = Config::default();
        let report = backend
            .verify_with_gates(&cfg, COND_CODE, COND_STMT, TierZeroGates::ON)
            .unwrap();
        assert!(report.lexically_verified);
        let t = tier0(&report);
        assert_eq!(t["vacuity"]["declared"], json!(false));
        assert_eq!(t["vacuity"]["report"], Value::Null);
    }

    /// The env seam parses default-OFF, and only truthy values switch a gate on.
    #[test]
    fn gate_env_parsing_is_default_off() {
        assert_eq!(TierZeroGates::default(), TierZeroGates::OFF);
        assert!(!TierZeroGates::OFF.any());
        assert!(TierZeroGates::ON.any());

        // Unset (this name is never set by the suite) reads OFF.
        assert!(!env_gate_on("THEOREMATA_GATE_ENV_PROBE_UNSET"));

        // The falsey spellings the crate's other seams accept, plus a truthy one.
        let var = "THEOREMATA_GATE_ENV_PROBE";
        for falsey in ["", " ", "0", "false", "FALSE", "off", " Off "] {
            std::env::set_var(var, falsey);
            assert!(!env_gate_on(var), "`{falsey}` must read as OFF");
        }
        for truthy in ["1", "true", "on", "yes"] {
            std::env::set_var(var, truthy);
            assert!(env_gate_on(var), "`{truthy}` must read as ON");
        }
        std::env::remove_var(var);
    }

    #[test]
    fn precheck_rejects_only_a_known_mismatch() {
        let sys = FormalSystem::Lean;
        // Both sides pinned and disagree -> reject.
        let p = project_with_toolchain(sys, Some("leanprover/lean4:v4.9.0"));
        let r = precheck_compat(sys, Some(&p), Some("leanprover/lean4:v4.10.0"));
        assert!(!r.compatible);
        assert!(r.reason.contains("mismatch"));

        // Agreeing pins -> compatible.
        let r = precheck_compat(sys, Some(&p), Some("leanprover/lean4:v4.9.0"));
        assert!(r.compatible);

        // Unknown on either side -> never fail-closed.
        let none = project_with_toolchain(sys, None);
        assert!(precheck_compat(sys, Some(&none), Some("leanprover/lean4:v4.9.0")).compatible);
        assert!(precheck_compat(sys, Some(&p), None).compatible);
        assert!(precheck_compat(sys, None, None).compatible);
    }

    #[test]
    fn per_declaration_status_isolates_a_single_failure() {
        let code = "theorem good : True := trivial\ntheorem bad : False := by sorry\n";
        let errors = vec!["error: Generated.lean:2:0: unsolved goals in `bad`".to_string()];
        let status = per_declaration_status(FormalSystem::Lean, code, false, &errors);
        assert_eq!(status.len(), 2);
        // `good` is untouched by the error; only `bad` is failed.
        assert!(status.iter().any(|u| u.name == "good" && u.ok));
        assert!(status.iter().any(|u| u.name == "bad" && !u.ok));
    }

    #[test]
    fn per_declaration_status_all_ok_when_compiled() {
        let code = "theorem a : True := trivial\nlemma b : True := trivial\n";
        let status = per_declaration_status(FormalSystem::Lean, code, true, &[]);
        assert_eq!(status.len(), 2);
        assert!(status.iter().all(|u| u.ok));
    }

    #[test]
    fn statement_hidden_in_comment_is_not_matched() {
        let stmt = "theorem foo : goal";
        // Present only inside a Lean `--` line comment -> must NOT match.
        let commented = "-- theorem foo : goal\nexample : True := trivial\n";
        assert!(!statement_mentioned(stmt, commented));
        // Same statement present as real code -> MUST match.
        let real = "theorem foo : goal := by trivial\n";
        assert!(statement_mentioned(stmt, real));
        // Present only inside a `(* ... *)` block comment -> must NOT match.
        let block = "(* theorem foo : goal *)\nexample : True := trivial\n";
        assert!(!statement_mentioned(stmt, block));
        // `//` line comment and `/- -/` / `{- -}` block comments also hidden.
        assert!(!statement_mentioned(stmt, "// theorem foo : goal\n"));
        assert!(!statement_mentioned(stmt, "/- theorem foo : goal -/\n"));
        assert!(!statement_mentioned(stmt, "{- theorem foo : goal -}\n"));
    }

    #[test]
    fn signature_rejects_a_statement_smuggled_only_into_a_string() {
        let report = crate::prover::statement_preservation::check_entry_signature(
            FormalSystem::Lean,
            "theorem wanted : False",
            "theorem decoy : True := trivial\n#check \"theorem wanted : False\"\n",
        );
        assert!(
            !report.preserved,
            "a string literal must not establish statement preservation: {report:?}"
        );
    }

    #[test]
    fn strip_comments_is_panic_free_on_non_ascii_and_deterministic() {
        // Multi-byte chars adjacent to delimiters must not panic or split.
        let code = "theorem β : ∀ x, x = x -- comment with π\n(* café ≤ ∞ *)λ\n";
        let a = strip_comments(code);
        let b = strip_comments(code);
        assert_eq!(a, b, "strip_comments must be deterministic");
        // Real (non-comment) tokens survive; comment content is gone.
        assert!(a.contains('β'));
        assert!(a.contains('λ'));
        assert!(!a.contains("comment"));
        assert!(!a.contains("café"));
        // Length in chars is preserved (every stripped char -> one space).
        assert_eq!(a.chars().count(), code.chars().count());
    }

    #[test]
    fn strip_comments_preserves_agda_pragmas_but_not_agda_comments() {
        let code = "{-# OPTIONS --allow-unsolved-metas #-}\n\
                    {- postulate absurd : ⊥ -}\n\
                    {-# COMPILED foo bar #-}\n\
                    postulate real : Set\n";
        let out = strip_comments(code);
        // Pragmas survive verbatim — including the `--` OPTIONS flag, which is
        // not a line comment inside `{-# ... #-}`.
        assert!(out.contains("{-# OPTIONS --allow-unsolved-metas #-}"));
        assert!(out.contains("{-# COMPILED foo bar #-}"));
        // A genuine `{- ... -}` comment is still removed.
        assert!(!out.contains("absurd"));
        // Real code outside any comment survives.
        assert!(out.contains("postulate real"));
        // Char count preserved, so line numbers stay accurate.
        assert_eq!(out.chars().count(), code.chars().count());
        assert_eq!(out.lines().count(), code.lines().count());
        // An UNCLOSED pragma opener stays ambiguous and is stripped (the
        // conservative direction: it can only make a caller stricter).
        let unclosed = strip_comments("{-# OPTIONS sorry\n");
        assert!(!unclosed.contains("OPTIONS"));
    }

    #[test]
    fn strip_comments_pragma_exemption_does_not_affect_other_systems() {
        // Lean `/- -/` and `--`, Rocq/Isabelle `(* *)`, Metamath `$( $)` are all
        // untouched by the `{-#` exemption: none of them can open one.
        assert!(!strip_comments("/- sorry -/\nexact h\n").contains("sorry"));
        assert!(!strip_comments("(* admit *)\nQed.\n").contains("admit"));
        assert!(!strip_comments("$( $a bad $)\nfoo $p\n").contains("bad"));
        assert!(!strip_comments("theorem t := by -- sorry\n  rfl\n").contains("sorry"));
    }

    /// The shared table's THREE properties, per system: the base token is still
    /// caught, its alias is caught too, and an innocent identifier that merely
    /// contains the token is not. The middle one is the bug this table exists
    /// for -- a ban a rename walks past reads as protection while providing none.
    #[test]
    fn escape_hatch_table_is_alias_expanded_and_boundary_matched() {
        // (system, base token that must STILL be caught, an ALIAS that must NOW
        // be caught, innocent source that must NOT be flagged)
        let cases: &[(FormalSystem, &str, &str, &str)] = &[
            (
                FormalSystem::Lean,
                "theorem t : P := by native_decide\n",
                "theorem t : P := by decide +native\n",
                "theorem t : 2 + 2 = 4 := by decide\ninstance : DecidableEq Foo := decidable_eq_foo\n",
            ),
            (
                FormalSystem::Rocq,
                "Theorem t : True.\nProof.\nAdmitted.\n",
                "Parameter bad : False.\n",
                "Theorem admits_a_root : True.\nProof. exact I. Qed.\n",
            ),
            (
                FormalSystem::Isabelle,
                "lemma t: \"True\"\n  sorry\n",
                "axiomatization bad where bad: \"False\"\n",
                "lemma oracle_free: \"True\" by simp\n",
            ),
            (
                FormalSystem::Agda,
                "postulate absurd : Set\n",
                "{-# OPTIONS --no-termination-check #-}\nthm : Set\n",
                "postulates : Set\npostulates = Set\n",
            ),
        ];
        for (system, base, alias, innocent) in cases {
            assert!(
                !escape_hatch_findings(*system, base).is_empty(),
                "{system}: the base token must still be caught"
            );
            assert!(
                !escape_hatch_findings(*system, alias).is_empty(),
                "{system}: the alias must be caught too -- a rename must not evade the ban"
            );
            assert!(
                escape_hatch_findings(*system, innocent).is_empty(),
                "{system}: innocent identifier flagged: {:?}",
                escape_hatch_findings(*system, innocent)
            );
        }
    }

    /// A commented hatch stays non-gating (ESCAPE_HATCH_COMMENT_POLICY ==
    /// CodeOnly), and a multi-word token matches across a line break because the
    /// scan normalizes whitespace.
    #[test]
    fn escape_hatch_findings_honour_comments_and_line_breaks() {
        assert!(escape_hatch_findings(
            FormalSystem::Lean,
            "-- sorry\n/- native_decide -/\ntheorem t : True := trivial\n"
        )
        .is_empty());
        assert_eq!(
            escape_hatch_findings(FormalSystem::Rocq, "Unset\n  Universe Checking.\n"),
            vec!["Unset Universe Checking".to_string()]
        );
    }

    /// Candle and Metamath are deliberately NOT token-scanned: their hatches are
    /// audited structurally. Stated here so an empty slice reads as a decision
    /// rather than an omission.
    #[test]
    fn candle_and_metamath_are_audited_structurally_not_by_token() {
        assert!(escape_hatch_tokens(FormalSystem::Candle).is_empty());
        assert!(escape_hatch_tokens(FormalSystem::Metamath).is_empty());
    }

    #[test]
    fn all_entry_names_lists_declarations_in_order() {
        let code = "theorem foo : True := trivial\nlemma bar : True := trivial\n";
        assert_eq!(
            all_entry_names(FormalSystem::Lean, code),
            vec!["foo", "bar"]
        );
    }

    #[test]
    fn candle_parses_and_round_trips() {
        use std::str::FromStr;
        // Both accepted tags parse to Candle; `hol` stays claimed by Isabelle.
        assert_eq!(
            FormalSystem::from_str("candle").unwrap(),
            FormalSystem::Candle
        );
        assert_eq!(
            FormalSystem::from_str("hol_light").unwrap(),
            FormalSystem::Candle
        );
        assert_eq!(
            FormalSystem::from_str("hol").unwrap(),
            FormalSystem::Isabelle
        );
        // as_str / Display round-trip.
        assert_eq!(FormalSystem::Candle.as_str(), "candle");
        assert_eq!(FormalSystem::Candle.to_string(), "candle");
        assert_eq!(
            FormalSystem::from_str(FormalSystem::Candle.as_str()).unwrap(),
            FormalSystem::Candle
        );
        // Source extension + a non-empty, fixed axiom base.
        assert_eq!(FormalSystem::Candle.source_extension(), ".ml");
        assert_eq!(FormalSystem::Candle.axiom_whitelist().len(), 3);
    }

    #[test]
    fn backend_for_candle_returns_candle_backend() {
        let cfg = Config::default();
        let backend = backend_for(&cfg, FormalSystem::Candle, true);
        assert_eq!(backend.system(), FormalSystem::Candle);
        // The mock backend is always available, mirroring the siblings.
        assert!(backend.available());
    }

    #[test]
    fn foundation_profiles_carry_citable_facts() {
        // Candle = HOL Light exactly: the study's `holl.aut` context, 25
        // primitive notions, classical + choice + all-math. The only backend
        // whose foundation Wiedijk 2006 studies directly, so the only one with a
        // concrete primitive-notion count.
        let candle = FormalSystem::Candle.foundation_profile();
        assert_eq!(candle.primitive_notions, Some(25));
        assert!(candle.classical && candle.choice && candle.all_math);
        assert_eq!(
            FormalSystem::Isabelle
                .foundation_profile()
                .primitive_notions,
            None
        );
        assert_eq!(
            FormalSystem::Lean.foundation_profile().primitive_notions,
            None
        );
        // Every foundation we target can encode all of mathematics...
        for sys in [
            FormalSystem::Lean,
            FormalSystem::Rocq,
            FormalSystem::Isabelle,
            FormalSystem::Candle,
        ] {
            assert!(
                sys.foundation_profile().all_math,
                "{sys} should be all-math"
            );
        }
        // ...but Rocq is intuitionistic + choice-free by default, unlike the
        // HOL-family backends.
        let rocq = FormalSystem::Rocq.foundation_profile();
        assert!(!rocq.classical && !rocq.choice);
    }
}
