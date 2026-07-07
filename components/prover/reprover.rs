//! ReProver-style retrieval-augmented local prover backend.

use crate::{
    config::Config,
    db::Store,
    model::ModelRequest,
    provider::ModelProvider,
    prover::{
        model::{ProofJob, ProofResult, ProverJobStatus, ProofTask},
        verify::verify_lean_output,
    },
    tools::{PythonCheck, Tool},
};
use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::{json, Value};
use std::time::Instant;

const BACKEND: &str = "reprover";

pub fn available(provider_ready: bool) -> bool {
    provider_ready && PythonCheck::new().available()
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
        ProverJobStatus::Queued,
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

pub fn poll_with_provider(
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    job_id: &str,
) -> Result<ProofJob> {
    let mut job = store
        .get_proof_job(job_id)?
        .ok_or_else(|| anyhow!("unknown proof job {job_id}"))?;
    if job.status.is_terminal() {
        return Ok(job);
    }
    let started = Instant::now();
    if job.poll_count == 0 {
        job.status = ProverJobStatus::InProgress;
        job.percent_complete = 20.0;
        job.poll_count += 1;
        job.updated_at = Utc::now();
        store.update_proof_job(&job)?;
        return Ok(job);
    }

    let premises = fetch_accessible_premises(config, &job.task)?;
    let premise_names: Vec<String> = premises
        .iter()
        .filter_map(|p| p.get("name").and_then(|n| n.as_str()).map(str::to_owned))
        .take(12)
        .collect();

    let response = provider.complete(&ModelRequest {
        role: "reprover_tactic_generator".into(),
        task: "Produce a complete Lean 4 proof using only the accessible premises listed. Never use sorry or admit.".into(),
        context: json!({
            "statement": job.task.statement,
            "theorem": job.task.theorem,
            "accessible_premises": premise_names,
        }),
        output_schema: json!({
            "type":"object",
            "required":["lean"],
            "properties":{"lean":{"type":"string"}}
        }),
    })?;

    let lean_code = response.content["lean"]
        .as_str()
        .unwrap_or("")
        .to_owned();
    let verification = if lean_code.is_empty() {
        None
    } else {
        verify_lean_output(config, &lean_code, &job.task.statement).ok()
    };
    let proved = verification
        .as_ref()
        .map(|v| v.compiles && v.axioms_clean)
        .unwrap_or(false);

    job.status = if proved {
        ProverJobStatus::Proved
    } else {
        ProverJobStatus::Failed
    };
    job.percent_complete = 100.0;
    job.poll_count += 1;
    job.completed_at = Some(Utc::now());
    job.updated_at = Utc::now();
    job.result = Some(ProofResult {
        task_id: job.task.id.clone(),
        job_id: job.id.clone(),
        status: job.status,
        lean_code: if lean_code.is_empty() {
            None
        } else {
            Some(lean_code)
        },
        counterexample: None,
        verification,
        artifacts_dir: job.artifacts_dir.clone(),
        duration_ms: started.elapsed().as_millis(),
        cost: None,
        message: Some(if proved {
            "reprover: proved".into()
        } else {
            "reprover: verification failed".into()
        }),
        provenance: json!({
            "backend": BACKEND,
            "premises": premise_names,
            "model": response.model,
        }),
    });
    store.update_proof_job(&job)?;
    Ok(job)
}

pub fn poll(store: &Store, config: &Config, job_id: &str) -> Result<ProofJob> {
    let job = store
        .get_proof_job(job_id)?
        .ok_or_else(|| anyhow!("unknown proof job {job_id}"))?;
    if job.status.is_terminal() {
        return Ok(job);
    }
    if job.poll_count >= 1 {
        let mut j = job;
        j.status = ProverJobStatus::Failed;
        j.completed_at = Some(Utc::now());
        j.updated_at = Utc::now();
        store.update_proof_job(&j)?;
    }
    store.get_proof_job(job_id)?.ok_or_else(|| anyhow!("job missing"))
}

fn fetch_accessible_premises(config: &Config, task: &ProofTask) -> Result<Vec<Value>> {
    let py = PythonCheck::new();
    let root = task.lean_project.root.clone();
    let result = py.run(json!({
        "tool": "retrieve",
        "root": root,
        "imports": task.lean_project.imports,
        "query": task.statement,
        "limit": 24,
        "op": "accessible_retrieve",
        "theorem_module": task.theorem.file,
        "theorem_line": task.metadata.get("theorem_line"),
    }))?;
    let parsed: Value = serde_json::from_str(&result.stdout).unwrap_or(json!({}));
    Ok(parsed["results"]
        .as_array()
        .cloned()
        .unwrap_or_default())
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