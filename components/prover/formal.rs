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

/// One trait, one impl per system. The default [`verify`](FormalBackend::verify)
/// wires the four layers together, fail-closed: ALL must pass.
pub trait FormalBackend {
    fn system(&self) -> FormalSystem;

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

    /// Default 3+1-layer orchestration (compile → axioms ⊆ whitelist → kernel
    /// re-check → source scan). Fail-closed: the proof is trusted only when all
    /// four layers pass. Reuses the existing [`VerificationReport`] fields.
    fn verify(&self, cfg: &Config, code: &str, stmt: &str) -> Result<VerificationReport> {
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
        // Anti-cheat: the existing mention check, tightened to reject a proof
        // spliced onto a WEAKENED/renamed/trivially-restated statement -- but only
        // on positively-detected weakening, so a non-parsable canonical falls back
        // to the mention check unchanged. `check_entry_signature` dispatches per
        // system: Lean/Rocq/Isabelle/Candle keep the theorem-signature parse, while
        // Agda (`name : Type`) and Metamath (`$p … $=`) gain a real per-system
        // signature check so a proof of a DIFFERENT theorem no longer slips through
        // on the weak lexical `statement_mentioned` substring fallback alone.
        let statement_preserved = statement_mentioned(stmt, code)
            && !matches!(
                crate::prover::statement_preservation::check_entry_signature(system, stmt, code)
                    .verdict,
                crate::prover::statement_preservation::PreservationVerdict::Renamed
                    | crate::prover::statement_preservation::PreservationVerdict::BindersChanged
                    | crate::prover::statement_preservation::PreservationVerdict::ConclusionChanged
                    | crate::prover::statement_preservation::PreservationVerdict::TriviallyRestated
            );
        let lexically_verified =
            kernel_clean && axioms_clean && lexical_clean && statement_preserved;

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
                "whitelist": whitelist,
            }),
        })
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
        (FormalSystem::Agda, mock) => Box::new(crate::prover::backends::external::ExternalBackend::new(cfg, FormalSystem::Agda, mock)),
        (FormalSystem::Metamath, mock) => Box::new(crate::prover::backends::external::ExternalBackend::new(cfg, FormalSystem::Metamath, mock)),
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
            Err(e) => (ProverJobStatus::Error, None, format!("live verify error: {e}")),
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
    fn all_entry_names_lists_declarations_in_order() {
        let code = "theorem foo : True := trivial\nlemma bar : True := trivial\n";
        assert_eq!(all_entry_names(FormalSystem::Lean, code), vec!["foo", "bar"]);
    }

    #[test]
    fn candle_parses_and_round_trips() {
        use std::str::FromStr;
        // Both accepted tags parse to Candle; `hol` stays claimed by Isabelle.
        assert_eq!(FormalSystem::from_str("candle").unwrap(), FormalSystem::Candle);
        assert_eq!(
            FormalSystem::from_str("hol_light").unwrap(),
            FormalSystem::Candle
        );
        assert_eq!(FormalSystem::from_str("hol").unwrap(), FormalSystem::Isabelle);
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
            FormalSystem::Isabelle.foundation_profile().primitive_notions,
            None
        );
        assert_eq!(FormalSystem::Lean.foundation_profile().primitive_notions, None);
        // Every foundation we target can encode all of mathematics...
        for sys in [
            FormalSystem::Lean,
            FormalSystem::Rocq,
            FormalSystem::Isabelle,
            FormalSystem::Candle,
        ] {
            assert!(sys.foundation_profile().all_math, "{sys} should be all-math");
        }
        // ...but Rocq is intuitionistic + choice-free by default, unlike the
        // HOL-family backends.
        let rocq = FormalSystem::Rocq.foundation_profile();
        assert!(!rocq.classical && !rocq.choice);
    }
}
