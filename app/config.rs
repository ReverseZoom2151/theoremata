use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub database: PathBuf,
    pub workspace: PathBuf,
    /// Per-attempt artifact root (FLARE-style inputs/lean/logs/verifier output).
    #[serde(default = "default_artifacts")]
    pub artifacts: PathBuf,
    pub resources: PathBuf,
    pub model_command: Option<String>,
    pub max_iterations: u32,
    pub command_timeout_seconds: u64,
    /// A Lake project that provides Mathlib. When set (and present), Lean checks
    /// run inside it so `import Mathlib` resolves against its build/cache.
    #[serde(default = "default_lean_project")]
    pub lean_project: Option<PathBuf>,
    /// When true, low-risk model-proposed graph mutations (adding an ordinary
    /// node or edge) are auto-approved; consequential ones (status changes,
    /// formal statements, formal/verified nodes) always require a human.
    #[serde(default)]
    pub auto_approve_safe: bool,
    /// Run the LeanParanoia hardening step on certification. Off by default:
    /// the first `lake build` in a fresh workspace resolves Mathlib's transitive
    /// git dependencies over the network and has no timeout.
    #[serde(default)]
    pub harden_proofs: bool,
    /// How finely the decomposer cuts a statement into obligations (and how much
    /// hidden-helper fan-out to budget). Defaults to `medium` (~1.8x).
    #[serde(default)]
    pub node_granularity: crate::model::Granularity,
    /// Number of CONSECUTIVE clean verifier passes required before a node is
    /// certified (AgentMathOlympiadMedalist's noisy-verifier hedge). The streak
    /// resets to zero on any failed pass. Defaults to 3.
    #[serde(default = "default_k_consecutive_clean")]
    pub k_consecutive_clean: u32,
    /// Default external prover backend for Route::Prove (`aristotle`, `leandojo`, `reprover`).
    #[serde(default = "default_prover_backend")]
    pub prover_backend: String,
    /// Max sparse polls when the agent drives an AttemptRun to completion.
    #[serde(default = "default_prover_max_polls")]
    pub prover_max_polls: u32,
    /// Force mock prover mode without touching the process env (used by tests to
    /// avoid `std::env::set_var` races under parallel execution).
    #[serde(default)]
    pub prover_mock: bool,
    /// Per-formal-system runner map (Native / Wsl / Docker). Defaults on this
    /// machine: `lean = Native`, `rocq = Wsl{Ubuntu}`, `isabelle = Wsl{Ubuntu}`.
    /// Any system can be re-pointed at another runner purely through config.
    #[serde(default)]
    pub formal_runners: crate::prover::exec::FormalRunners,
    /// Binary for the live Lean gate (`lean <file>`). Env: `THEOREMATA_LEAN`.
    #[serde(default = "default_lean_bin")]
    pub lean_bin: String,
    /// Binary for the live Rocq compile (`coqc`). Env: `THEOREMATA_COQC`.
    #[serde(default = "default_coqc_bin")]
    pub coqc_bin: String,
    /// Binary for the live Rocq kernel re-check (`coqchk`). Env: `THEOREMATA_COQCHK`.
    #[serde(default = "default_coqchk_bin")]
    pub coqchk_bin: String,
    /// Binary for the live Isabelle build. Env: `THEOREMATA_ISABELLE`.
    #[serde(default = "default_isabelle_bin")]
    pub isabelle_bin: String,
    /// Binary for the live Candle (verified HOL Light on CakeML) gate
    /// (`candle <file>.ml`). Env: `THEOREMATA_CANDLE`.
    #[serde(default = "default_candle_bin")]
    pub candle_bin: String,
    /// Which formal system the agent targets when generating proofs for
    /// `Route::Prove`/`Route::Formalize`. Defaults to Lean (existing behavior);
    /// set to `rocq`/`isabelle` to route through the per-system generator.
    #[serde(default)]
    pub target_system: crate::prover::formal::FormalSystem,
    /// Wire the LeanDojo in-kernel `validateProof` soundness gate
    /// (`components/verify/lean/validate_proof_template.lean`) into the Lean
    /// verify path as an optional extra check. Off by default: it needs a REPL
    /// build of the template against the pinned toolchain, which is not always
    /// present. When on (and the template + toolchain are available), the Lean
    /// backend reconstructs a standalone declaration and kernel-rechecks it,
    /// rejecting a `sorry`/metavariable-carrying "proof" the tactic outcome would
    /// otherwise trust. The wiring + flag exist even when the check cannot run.
    #[serde(default)]
    pub kernel_validate_proof: bool,
    /// Run the advisory statement-VALIDATION faithfulness stage. Promoted from the
    /// `THEOREMATA_VALIDATE_STATEMENTS` env var to a Config field (mirroring
    /// `prover_mock`) so the RUNTIME path reads this field, not the process env —
    /// tests set it directly and no longer race on a global env var. The
    /// env-derived default preserves existing env-based usage: OFF unless the env
    /// is set to a truthy value at Config construction.
    #[serde(default = "default_validate_statements")]
    pub validate_statements: bool,
    /// Aletheia abstention threshold in `(0, 1]`. Promoted from
    /// `THEOREMATA_ABSTAIN_THRESHOLD` to a Config field. `None` (the default, and
    /// the meaning of an absent/unparseable env) keeps the exact certify-or-fail
    /// behaviour; a value makes the certify gate DECLINE (abstain) on any
    /// uncertified node whose confidence is below it. Runtime reads this field.
    #[serde(default = "default_abstain_threshold")]
    pub abstain_threshold: Option<f64>,
    /// Whether the scored proof-pool + critic meta-verification certification gate
    /// is active. Promoted from `THEOREMATA_POOL_META_GATE` to a Config field. ON
    /// by default (env-derived: anything but an explicit `0`/`false`/`off`).
    /// Runtime reads this field, not the env.
    #[serde(default = "default_pool_meta_gate")]
    pub pool_meta_gate: bool,
}

/// Env-derived default for [`Config::validate_statements`]. Read ONCE at Config
/// construction (mirrors `statement_validation::validation_enabled`): absent /
/// empty / `0`/`false`/`off` means OFF.
fn default_validate_statements() -> bool {
    match std::env::var("THEOREMATA_VALIDATE_STATEMENTS") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "off"
        ),
        Err(_) => false,
    }
}

/// Env-derived default for [`Config::abstain_threshold`]. Read ONCE at Config
/// construction (mirrors the old `agent::abstain_threshold` free fn): absent /
/// unparseable / non-positive means `None` (abstention OFF).
fn default_abstain_threshold() -> Option<f64> {
    std::env::var("THEOREMATA_ABSTAIN_THRESHOLD")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|t| *t > 0.0)
}

/// Env-derived default for [`Config::pool_meta_gate`]. Read ONCE at Config
/// construction (mirrors `certification::gate_enabled`): ON unless the env is an
/// explicit `0`/`false`/`off`.
fn default_pool_meta_gate() -> bool {
    match std::env::var("THEOREMATA_POOL_META_GATE") {
        Ok(v) => !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off"),
        Err(_) => true,
    }
}

fn default_lean_bin() -> String {
    "lean".into()
}

fn default_coqc_bin() -> String {
    "coqc".into()
}

fn default_coqchk_bin() -> String {
    "coqchk".into()
}

fn default_isabelle_bin() -> String {
    // Generic default: `isabelle` on PATH. Point at a specific bundle via the
    // `THEOREMATA_ISABELLE` env var or `isabelle_bin` in the local config.
    "isabelle".into()
}

fn default_candle_bin() -> String {
    // Generic default: `candle` on PATH. Point at a specific build via the
    // `THEOREMATA_CANDLE` env var or `candle_bin` in the local config.
    "candle".into()
}

fn default_k_consecutive_clean() -> u32 {
    3
}

fn default_prover_backend() -> String {
    "aristotle".into()
}

fn default_prover_max_polls() -> u32 {
    8
}

fn default_lean_project() -> Option<PathBuf> {
    Some(PathBuf::from("resources/mathlib4-master/mathlib4-master"))
}

fn default_artifacts() -> PathBuf {
    PathBuf::from(".theoremata/artifacts")
}

impl Default for Config {
    fn default() -> Self {
        Self {
            database: PathBuf::from(".theoremata/theoremata.db"),
            workspace: PathBuf::from(".theoremata/workspaces"),
            artifacts: default_artifacts(),
            resources: PathBuf::from("resources"),
            model_command: std::env::var("THEOREMATA_MODEL_COMMAND").ok(),
            max_iterations: 3,
            command_timeout_seconds: 60,
            lean_project: default_lean_project(),
            auto_approve_safe: false,
            harden_proofs: false,
            node_granularity: crate::model::Granularity::default(),
            k_consecutive_clean: default_k_consecutive_clean(),
            prover_backend: default_prover_backend(),
            prover_max_polls: default_prover_max_polls(),
            prover_mock: false,
            formal_runners: crate::prover::exec::FormalRunners::default(),
            lean_bin: default_lean_bin(),
            coqc_bin: default_coqc_bin(),
            coqchk_bin: default_coqchk_bin(),
            isabelle_bin: default_isabelle_bin(),
            candle_bin: default_candle_bin(),
            target_system: crate::prover::formal::FormalSystem::default(),
            kernel_validate_proof: false,
            validate_statements: default_validate_statements(),
            abstain_threshold: default_abstain_threshold(),
            pool_meta_gate: default_pool_meta_gate(),
        }
    }
}

impl Config {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let path = path.unwrap_or_else(|| Path::new(".theoremata/config.json"));
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        Ok(serde_json::from_str(&raw)
            .with_context(|| format!("parsing config {}", path.display()))?)
    }

    pub fn initialize(&self) -> Result<()> {
        if let Some(parent) = self.database.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::create_dir_all(&self.workspace)?;
        fs::create_dir_all(&self.artifacts)?;
        let path = Path::new(".theoremata/config.json");
        if !path.exists() {
            fs::write(path, serde_json::to_string_pretty(self)?)?;
        }
        Ok(())
    }
}
