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

use crate::{config::Config, prover::model::VerificationReport};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fmt,
    path::PathBuf,
    str::FromStr,
};

/// Which formal system a proof object belongs to. Serialized `snake_case`
/// (`lean` / `rocq` / `isabelle`) to match the `backend` string dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormalSystem {
    Lean,
    Rocq,
    Isabelle,
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
    pub fn axiom_whitelist(self) -> Vec<String> {
        match self {
            FormalSystem::Lean => vec![
                "propext".into(),
                "Classical.choice".into(),
                "Quot.sound".into(),
            ],
            FormalSystem::Rocq => vec!["Closed under the global context".into()],
            FormalSystem::Isabelle => Vec::new(),
        }
    }

    /// Source-file extension for generated proofs.
    pub fn source_extension(self) -> &'static str {
        match self {
            FormalSystem::Lean => ".lean",
            FormalSystem::Rocq => ".v",
            FormalSystem::Isabelle => ".thy",
        }
    }

    /// Default corpus imports the model may draw premises from.
    pub fn default_imports(self) -> Vec<String> {
        match self {
            FormalSystem::Lean => vec!["Mathlib".into()],
            FormalSystem::Rocq => vec!["Stdlib".into(), "mathcomp.ssreflect.ssreflect".into()],
            FormalSystem::Isabelle => vec!["Main".into()],
        }
    }
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

/// Layer 2b (build): did the source compile, and what errors if not.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileReport {
    pub compiled: bool,
    pub errors: Vec<String>,
    #[serde(default)]
    pub detail: Value,
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
        let lexical_clean = scan.clean;
        let kernel_clean = compile.compiled && recheck.rechecked;
        let statement_preserved = statement_mentioned(stmt, code);
        let lexically_verified =
            kernel_clean && axioms_clean && lexical_clean && statement_preserved;

        Ok(VerificationReport {
            lexically_verified,
            axioms_clean,
            statement_preserved,
            lexical_clean,
            hardening_clean: Some(kernel_clean),
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

/// Cheap lexical "the code is about this statement" check: the statement's
/// leading identifier/head appears in the whitespace-normalized source.
pub(crate) fn statement_mentioned(stmt: &str, code: &str) -> bool {
    let code_norm: String = code.split_whitespace().collect();
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
