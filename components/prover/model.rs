//! Proof-task contracts (open-atp / LeanDojo / lean-aristotle-mcp).

use crate::prover::formal::FormalSystem;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// LeanDojo-style theorem identity: `(repo, commit, file, full_name, position)`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TheoremIdentity {
    pub repo: Option<String>,
    pub commit: Option<String>,
    pub file: Option<String>,
    pub full_name: String,
    /// 1-based line of the declaration in `file`, when known.
    pub line: Option<u32>,
}

/// A complete project context for a proof task, generalized across formal
/// systems (open-atp `LeanProject` → `FormalProject`). `system` defaults to
/// [`FormalSystem::Lean`] so pre-existing serialized data (which had no such
/// field) still loads unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormalProject {
    #[serde(default)]
    pub system: FormalSystem,
    pub root: PathBuf,
    /// `lean-toolchain` / opam switch / Isabelle version.
    pub toolchain: Option<String>,
    #[serde(default)]
    pub imports: Vec<String>,
    #[serde(default)]
    pub metadata: Value,
}

/// Input contract for any prover backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofTask {
    pub id: String,
    pub project_id: Option<String>,
    pub node_id: Option<String>,
    pub theorem: TheoremIdentity,
    /// Which formal system this task targets (defaults to Lean for back-compat).
    #[serde(default)]
    pub system: FormalSystem,
    #[serde(alias = "lean_project")]
    pub formal_project: FormalProject,
    pub statement: String,
    pub stub: Option<String>,
    pub prompt: Option<String>,
    pub backend: String,
    #[serde(default)]
    pub metadata: Value,
}

/// External-prover / async job lifecycle (lean-aristotle-mcp taxonomy).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProverJobStatus {
    Submitted,
    Queued,
    InProgress,
    Proved,
    Partial,
    Failed,
    Counterexample,
    Cancelled,
    Error,
}

impl ProverJobStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Proved
                | Self::Partial
                | Self::Failed
                | Self::Counterexample
                | Self::Cancelled
                | Self::Error
        )
    }
}

/// Local verification summary attached to a [`ProofResult`].
///
/// NOTE: `lexically_verified` is the best-available *lexical* screen (soundness
/// scan + no `sorry`/`admit`) — it does NOT mean the Lean compiled. Authoritative
/// certification runs the real compile + `#print axioms` gate in the agent loop
/// (`verify_source`); this report is a cheap trust-but-verify pre-screen for
/// external-prover output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationReport {
    pub lexically_verified: bool,
    pub axioms_clean: bool,
    pub statement_preserved: bool,
    pub lexical_clean: bool,
    #[serde(default)]
    pub hardening_clean: Option<bool>,
    /// Whether this report was produced by a LIVE prover toolchain, as opposed to
    /// a MOCK backend (canned kernel layers). Conservative serde default `false`:
    /// unknown provenance is NOT live, so a report can never be mistaken for a
    /// live certification. The default [`FormalBackend::verify`] sets this to
    /// `!self.is_mock()`. A mock pass is at most *informal* — never
    /// [`FormallyVerified`](crate::model::NodeStatus::FormallyVerified).
    #[serde(default)]
    pub live: bool,
    #[serde(default)]
    pub detail: Value,
}

/// Output contract for any prover backend (open-atp `ProofResult`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofResult {
    pub task_id: String,
    pub job_id: String,
    pub status: ProverJobStatus,
    #[serde(alias = "lean_code")]
    pub formal_code: Option<String>,
    pub counterexample: Option<String>,
    pub verification: Option<VerificationReport>,
    pub artifacts_dir: Option<PathBuf>,
    pub duration_ms: u128,
    pub cost: Option<f64>,
    pub message: Option<String>,
    #[serde(default)]
    pub provenance: Value,
}

/// Persisted proof-job row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofJob {
    pub id: String,
    pub project_id: Option<String>,
    pub node_id: Option<String>,
    pub backend: String,
    pub status: ProverJobStatus,
    pub task: ProofTask,
    pub result: Option<ProofResult>,
    pub external_id: Option<String>,
    pub percent_complete: f64,
    pub artifacts_dir: Option<PathBuf>,
    pub poll_count: u32,
    pub submitted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// FLARE-style attempt lifecycle.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttemptRunStatus {
    Running,
    Completed,
    Cancelled,
    Failed,
}

/// Persisted attempt-run row with artifact directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptRunRecord {
    pub id: String,
    pub project_id: String,
    pub node_id: Option<String>,
    pub proof_job_id: Option<String>,
    pub status: AttemptRunStatus,
    pub artifacts_dir: PathBuf,
    pub input: Value,
    pub output: Option<Value>,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u128>,
    pub cost: Option<f64>,
}

/// API response for `AttemptRun::result`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptRunResult {
    pub id: String,
    pub status: AttemptRunStatus,
    pub artifacts_dir: PathBuf,
    pub input: Value,
    pub output: Option<Value>,
    pub duration_ms: Option<u128>,
    pub cost: Option<f64>,
    pub proof_result: Option<ProofResult>,
}
