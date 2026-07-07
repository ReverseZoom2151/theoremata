//! LeanDojo-style tactic-session prover backend (mock + live command).

use crate::{
    config::Config,
    db::Store,
    prover::{
        model::{ProofJob, ProofResult, ProverJobStatus, ProofTask},
        verify::verify_lean_output,
    },
    tools::{PythonCheck, Tool},
};
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde_json::{json, Value};
use std::time::Instant;

const BACKEND: &str = "leandojo";

pub fn available() -> bool {
    PythonCheck::new().available()
}

fn worker_run(request: Value) -> Result<Value> {
    let py = PythonCheck::new();
    if !py.available() {
        anyhow::bail!("python worker unavailable");
    }
    let mut payload = request;
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("tool".into(), json!("leandojo"));
    }
    let result = py.run(payload)?;
    serde_json::from_str(&result.stdout).context("parsing leandojo worker output")
}

pub fn submit(
    store: &Store,
    config: &Config,
    task: ProofTask,
    artifacts_dir: Option<std::path::PathBuf>,
) -> Result<ProofJob> {
    let job = store.create_proof_job(
        &task,
        BACKEND,
        ProverJobStatus::Submitted,
        None,
        artifacts_dir.as_deref(),
        0.0,
    )?;
    store.event(
        task.project_id.as_deref(),
        None,
        "proof_job.submitted",
        BACKEND,
        json!({"job_id": job.id, "task_id": task.id}),
    )?;
    let _ = config;
    Ok(job)
}

pub fn poll(store: &Store, config: &Config, job_id: &str) -> Result<ProofJob> {
    let mut job = store
        .get_proof_job(job_id)?
        .ok_or_else(|| anyhow!("unknown proof job {job_id}"))?;
    if job.status.is_terminal() {
        return Ok(job);
    }
    let started = Instant::now();
    let session = worker_run(json!({
        "op": "initialize",
        "theorem": job.task.theorem,
        "statement": job.task.statement,
        "imports": job.task.formal_project.imports,
    }))?;
    if !session.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        job.status = ProverJobStatus::Error;
        job.completed_at = Some(Utc::now());
        job.updated_at = Utc::now();
        store.update_proof_job(&job)?;
        return Ok(job);
    }

    if job.poll_count == 0 {
        job.status = ProverJobStatus::InProgress;
        job.percent_complete = 50.0;
        job.poll_count += 1;
        job.updated_at = Utc::now();
        store.update_proof_job(&job)?;
        return Ok(job);
    }

    let tactic = worker_run(json!({
        "op": "run_tactic",
        "theorem": job.task.theorem,
        "statement": job.task.statement,
        "state_id": session["state_id"],
        "tactic": "trivial",
    }))?;
    let status_str = tactic["status"].as_str().unwrap_or("error");
    let formal_code = tactic["lean_code"].as_str().map(str::to_owned);
    let status = match status_str {
        "proved" => ProverJobStatus::Proved,
        "in_progress" => ProverJobStatus::InProgress,
        _ => ProverJobStatus::Failed,
    };
    job.status = status;
    job.percent_complete = if status == ProverJobStatus::Proved {
        100.0
    } else {
        75.0
    };
    job.poll_count += 1;
    job.updated_at = Utc::now();

    if status.is_terminal() {
        job.completed_at = Some(Utc::now());
        let verification = formal_code
            .as_deref()
            .and_then(|code| verify_lean_output(config, code, &job.task.statement).ok());
        let result = ProofResult {
            task_id: job.task.id.clone(),
            job_id: job.id.clone(),
            status,
            formal_code,
            counterexample: None,
            verification,
            artifacts_dir: job.artifacts_dir.clone(),
            duration_ms: started.elapsed().as_millis(),
            cost: None,
            message: tactic["status"].as_str().map(str::to_owned),
            provenance: json!({"backend": BACKEND, "session": session, "tactic": tactic}),
        };
        job.result = Some(result);
    }
    store.update_proof_job(&job)?;
    Ok(job)
}

pub fn cancel(store: &Store, job_id: &str) -> Result<ProofJob> {
    let mut job = store
        .get_proof_job(job_id)?
        .ok_or_else(|| anyhow!("unknown proof job {job_id}"))?;
    if !job.status.is_terminal() {
        job.status = ProverJobStatus::Cancelled;
        job.completed_at = Some(Utc::now());
        job.updated_at = Utc::now();
        store.update_proof_job(&job)?;
    }
    Ok(job)
}