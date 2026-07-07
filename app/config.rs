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
