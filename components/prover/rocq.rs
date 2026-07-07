//! Rocq (formerly Coq) external-prover adapter — Phase 1 mock backend.
//!
//! Mirrors the `aristotle` mock EXACTLY (Config.prover_mock-driven,
//! submit → poll(InProgress → Proved) → result + a `VerificationReport`), but
//! emits SYSTEM-NATIVE Rocq (`.v`) proofs and routes verification through the
//! system-agnostic [`FormalBackend`] 3+1-layer gate. Live mode (Phase 2) will
//! replace the canned layers with `rocq compile` / `Print Assumptions` /
//! `rocqchk`; the source scan already runs for real.

use crate::{
    config::Config,
    db::Store,
    prover::{
        formal::{
            AxiomReport, CompileReport, FormalBackend, FormalSystem, GoalState, ProofSession,
            RecheckReport, ScanReport, StateResult, UnitResult, Workspace,
        },
        model::{FormalProject, ProofJob, ProofResult, ProofTask, ProverJobStatus},
    },
};
use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::json;
use std::{path::PathBuf, time::Instant};

const BACKEND: &str = "rocq";
const SYSTEM: FormalSystem = FormalSystem::Rocq;

pub fn mock_enabled(config: &Config) -> bool {
    // Config flag short-circuits BEFORE any env read, so parallel tests never
    // race on the process-global environment.
    config.prover_mock
        || std::env::var("THEOREMATA_ROCQ_MOCK")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or_else(|_| std::env::var("THEOREMATA_ROCQ_COMMAND").is_err())
}

pub fn submit(
    store: &Store,
    config: &Config,
    task: ProofTask,
    artifacts_dir: Option<std::path::PathBuf>,
) -> Result<ProofJob> {
    let external_id = if mock_enabled(config) {
        Some(format!("mock-{}", &task.id[..8.min(task.id.len())]))
    } else {
        None
    };
    let job = store.create_proof_job(
        &task,
        BACKEND,
        ProverJobStatus::Submitted,
        external_id.as_deref(),
        artifacts_dir.as_deref(),
        0.0,
    )?;
    store.event(
        task.project_id.as_deref(),
        None,
        "proof_job.submitted",
        BACKEND,
        json!({"job_id": job.id, "task_id": task.id, "mock": mock_enabled(config)}),
    )?;
    if let Some(dir) = &artifacts_dir {
        write_artifact(dir, "task.json", &task)?;
        write_artifact(
            dir,
            "submit.json",
            &json!({"mock": mock_enabled(config), "backend": BACKEND}),
        )?;
    }
    Ok(job)
}

pub fn poll(store: &Store, config: &Config, job_id: &str) -> Result<ProofJob> {
    let mut job = store
        .get_proof_job(job_id)?
        .ok_or_else(|| anyhow!("unknown proof job {job_id}"))?;
    if job.status.is_terminal() {
        return Ok(job);
    }
    if !mock_enabled(config) {
        return Err(anyhow!(
            "live Rocq backend is not wired yet (Phase 2); set prover_mock or THEOREMATA_ROCQ_MOCK"
        ));
    }
    let started = Instant::now();
    let (status, percent, formal_code, message) = advance_mock(&job);
    job.status = status;
    job.percent_complete = percent;
    job.poll_count += 1;
    job.updated_at = Utc::now();

    if status.is_terminal() {
        job.completed_at = Some(Utc::now());
        let backend = RocqBackend { mock: true };
        let verification = formal_code
            .as_deref()
            .and_then(|code| backend.verify(config, code, &job.task.statement).ok());
        let result = ProofResult {
            task_id: job.task.id.clone(),
            job_id: job.id.clone(),
            status,
            formal_code: formal_code.clone(),
            counterexample: None,
            verification,
            artifacts_dir: job.artifacts_dir.clone(),
            duration_ms: started.elapsed().as_millis(),
            cost: None,
            message: message.clone(),
            provenance: json!({
                "backend": BACKEND,
                "system": SYSTEM.as_str(),
                "mock": true,
                "poll_count": job.poll_count,
            }),
        };
        job.result = Some(result.clone());
        if let Some(dir) = &job.artifacts_dir {
            if let Some(code) = &formal_code {
                let sub = dir.join(BACKEND);
                std::fs::create_dir_all(&sub)?;
                std::fs::write(sub.join("solution.v"), code)?;
            }
            write_artifact(dir, "result.json", &result)?;
            if let Some(v) = &result.verification {
                write_artifact(dir, "verifier/report.json", v)?;
            }
        }
        store.update_proof_job(&job)?;
        store.event(
            job.project_id.as_deref(),
            None,
            "proof_job.completed",
            BACKEND,
            json!({"job_id": job.id, "status": status, "verified": result.verification.is_some()}),
        )?;
        return Ok(job);
    }

    store.update_proof_job(&job)?;
    store.event(
        job.project_id.as_deref(),
        None,
        "proof_job.polled",
        BACKEND,
        json!({"job_id": job.id, "status": status, "percent": percent}),
    )?;
    Ok(job)
}

pub fn cancel(store: &Store, job_id: &str) -> Result<ProofJob> {
    let mut job = store
        .get_proof_job(job_id)?
        .ok_or_else(|| anyhow!("unknown proof job {job_id}"))?;
    if job.status.is_terminal() {
        return Ok(job);
    }
    job.status = ProverJobStatus::Cancelled;
    job.completed_at = Some(Utc::now());
    job.updated_at = Utc::now();
    store.update_proof_job(&job)?;
    store.event(
        job.project_id.as_deref(),
        None,
        "proof_job.cancelled",
        BACKEND,
        json!({"job_id": job.id}),
    )?;
    Ok(job)
}

pub fn build_task(
    project_id: Option<String>,
    node_id: Option<String>,
    statement: &str,
    theorem_name: &str,
    config: &Config,
) -> ProofTask {
    let root = config
        .lean_project
        .clone()
        .unwrap_or_else(|| config.resources.join("rocq"));
    ProofTask {
        id: uuid::Uuid::new_v4().to_string(),
        project_id,
        node_id,
        theorem: crate::prover::model::TheoremIdentity {
            repo: Some("theoremata".into()),
            commit: None,
            file: None,
            full_name: theorem_name.into(),
            line: None,
        },
        system: SYSTEM,
        formal_project: crate::prover::model::FormalProject {
            system: SYSTEM,
            root,
            toolchain: None,
            imports: SYSTEM.default_imports(),
            metadata: json!({}),
        },
        statement: statement.into(),
        stub: None,
        prompt: None,
        backend: BACKEND.into(),
        metadata: json!({}),
    }
}

fn advance_mock(job: &ProofJob) -> (ProverJobStatus, f64, Option<String>, Option<String>) {
    match job.poll_count {
        0 => (
            ProverJobStatus::InProgress,
            40.0,
            None,
            Some("mock: working".into()),
        ),
        _ => (
            ProverJobStatus::Proved,
            100.0,
            Some(mock_rocq_solution(&job.task)),
            Some("mock: proved".into()),
        ),
    }
}

fn mock_rocq_solution(task: &ProofTask) -> String {
    let name = task
        .theorem
        .full_name
        .rsplit('.')
        .next()
        .unwrap_or("MainTheorem");
    format!("(* Mock Rocq proof. *)\nTheorem {name} : True.\nProof.\n  exact I.\nQed.\n")
}

/// Rocq [`FormalBackend`]. In mock mode the compile / axiom-audit / kernel
/// re-check layers return canned success; the source scan always runs for real.
pub struct RocqBackend {
    pub mock: bool,
}

impl FormalBackend for RocqBackend {
    fn system(&self) -> FormalSystem {
        SYSTEM
    }

    fn scaffold(&self, _cfg: &Config, _code: &str, name: &str) -> Result<Workspace> {
        Ok(Workspace {
            system: SYSTEM,
            root: PathBuf::from("."),
            source_path: PathBuf::from(format!("{name}{}", SYSTEM.source_extension())),
            entry: name.to_string(),
        })
    }

    fn compile(&self, _ws: &Workspace) -> Result<CompileReport> {
        if !self.mock {
            return Err(anyhow!("live Rocq compile not wired yet (Phase 2)"));
        }
        Ok(CompileReport {
            compiled: true,
            errors: Vec::new(),
            detail: json!({"mock": true}),
        })
    }

    fn audit_axioms(&self, _ws: &Workspace, _thm: &str, whitelist: &[String]) -> Result<AxiomReport> {
        if !self.mock {
            return Err(anyhow!("live Rocq axiom audit not wired yet (Phase 2)"));
        }
        Ok(AxiomReport {
            axioms: Vec::new(),
            within_whitelist: true,
            detail: json!({"mock": true, "whitelist": whitelist}),
        })
    }

    fn kernel_recheck(&self, _ws: &Workspace) -> Result<RecheckReport> {
        if !self.mock {
            return Err(anyhow!("live Rocq kernel recheck not wired yet (Phase 2)"));
        }
        Ok(RecheckReport {
            rechecked: true,
            detail: json!({"mock": true}),
        })
    }

    fn source_scan(&self, code: &str) -> Result<ScanReport> {
        // Rocq escape hatches NOT caught by Print Assumptions / rocqchk.
        let low = code.to_lowercase();
        let patterns = [
            "admitted",
            "axiom ",
            "-type-in-type",
            "type_in_type",
            "unset universe checking",
            "bypass_check",
        ];
        let findings: Vec<String> = patterns
            .iter()
            .filter(|p| low.contains(**p))
            .map(|p| (*p).to_string())
            .collect();
        Ok(ScanReport {
            clean: findings.is_empty(),
            findings,
            detail: json!({"system": SYSTEM.as_str()}),
        })
    }
}

/// Rocq warm-driver session (Petanque / SerAPI in Phase 3). Supports both
/// `submit_unit` and per-tactic `step_tactic`.
impl ProofSession for RocqBackend {
    fn start(&mut self, _project: &FormalProject) -> Result<()> {
        Ok(())
    }

    fn submit_unit(&mut self, code: &str) -> Result<UnitResult> {
        let scan = self.source_scan(code)?;
        Ok(UnitResult {
            ok: scan.clean,
            messages: scan.findings,
            detail: json!({"mock": self.mock, "system": SYSTEM.as_str()}),
        })
    }

    fn step_tactic(&mut self, state: u64, tactic: &str) -> Result<StateResult> {
        // Rocq supports per-tactic stepping (SerAPI state ids / Petanque).
        let finished = tactic.contains("Qed") || tactic.trim() == "exact I";
        Ok(StateResult {
            state: state + 1,
            finished,
            detail: json!({"mock": self.mock, "tactic": tactic}),
        })
    }

    fn goal_state(&self, _state: u64) -> Result<GoalState> {
        Ok(GoalState {
            goals: vec!["True".into()],
            detail: json!({"mock": self.mock}),
        })
    }
}

fn write_artifact(dir: &std::path::Path, rel: &str, value: &impl serde::Serialize) -> Result<()> {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(value)?)?;
    Ok(())
}
