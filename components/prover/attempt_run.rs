//! FLARE-style `AttemptRun`: start / cancel / result with artifact directories.

use crate::{
    config::Config,
    db::Store,
    prover::{
        model::{AttemptRunRecord, AttemptRunResult, AttemptRunStatus, ProofTask},
        proof_job,
    },
    provider::ModelProvider,
};
use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Instant;

pub fn artifacts_root(config: &Config, project_id: &str) -> PathBuf {
    config.artifacts.join(project_id)
}

pub fn start(
    store: &Store,
    config: &Config,
    project_id: &str,
    node_id: Option<&str>,
    input: Value,
) -> Result<AttemptRunRecord> {
    store.project(project_id)?;
    let attempt_id = uuid::Uuid::new_v4().to_string();
    let dir = artifacts_root(config, project_id).join(&attempt_id);
    std::fs::create_dir_all(dir.join("lean"))?;
    std::fs::create_dir_all(dir.join("logs"))?;
    std::fs::create_dir_all(dir.join("verifier"))?;
    std::fs::write(
        dir.join("input.json"),
        serde_json::to_string_pretty(&input)?,
    )?;

    let statement = input["statement"]
        .as_str()
        .or_else(|| input["task"].as_str())
        .unwrap_or("True");
    let theorem_name = input["theorem_name"].as_str().unwrap_or("Theoremata.Main");
    let task = ProofTask {
        id: uuid::Uuid::new_v4().to_string(),
        project_id: Some(project_id.into()),
        node_id: node_id.map(str::to_owned),
        theorem: crate::prover::model::TheoremIdentity {
            repo: Some("theoremata".into()),
            commit: None,
            file: None,
            full_name: theorem_name.into(),
            line: None,
        },
        system: crate::prover::formal::FormalSystem::Lean,
        formal_project: crate::prover::model::FormalProject {
            system: crate::prover::formal::FormalSystem::Lean,
            root: config
                .lean_project
                .clone()
                .unwrap_or_else(|| config.resources.join("mathlib4-master/mathlib4-master")),
            toolchain: None,
            imports: vec!["Mathlib".into()],
            metadata: json!({}),
        },
        statement: statement.into(),
        stub: input
            .get("stub")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        prompt: input
            .get("prompt")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        backend: input
            .get("backend")
            .and_then(|v| v.as_str())
            .unwrap_or(&config.prover_backend)
            .to_string(),
        metadata: input.clone(),
    };

    let job = proof_job::submit(store, config, task, Some(dir.clone()))?;
    let record = store.create_attempt_run(
        project_id,
        node_id,
        Some(&job.id),
        AttemptRunStatus::Running,
        &dir,
        &input,
    )?;
    store.event(
        Some(project_id),
        None,
        "attempt_run.started",
        "attempt_run",
        json!({"attempt_id": record.id, "proof_job_id": job.id, "artifacts_dir": dir}),
    )?;
    Ok(record)
}

pub fn cancel(store: &Store, attempt_id: &str) -> Result<AttemptRunRecord> {
    let mut record = store
        .get_attempt_run(attempt_id)?
        .ok_or_else(|| anyhow!("unknown attempt run {attempt_id}"))?;
    if record.status == AttemptRunStatus::Running {
        if let Some(ref job_id) = record.proof_job_id {
            let _ = proof_job::cancel(store, job_id);
        }
        record.status = AttemptRunStatus::Cancelled;
        record.completed_at = Some(Utc::now());
        record.updated_at = Utc::now();
        store.update_attempt_run(&record)?;
        store.event(
            Some(&record.project_id),
            None,
            "attempt_run.cancelled",
            "attempt_run",
            json!({"attempt_id": record.id}),
        )?;
    }
    Ok(record)
}

pub fn result(
    store: &Store,
    config: &Config,
    attempt_id: &str,
    provider: Option<&dyn ModelProvider>,
) -> Result<AttemptRunResult> {
    let mut record = store
        .get_attempt_run(attempt_id)?
        .ok_or_else(|| anyhow!("unknown attempt run {attempt_id}"))?;
    let started = Instant::now();

    if record.status == AttemptRunStatus::Running {
        if let Some(ref job_id) = record.proof_job_id {
            let job = proof_job::poll(store, config, job_id, provider)?;
            if job.status.is_terminal() {
                let output = json!({
                    "proof_job": job,
                    "proof_result": job.result,
                });
                std::fs::write(
                    record.artifacts_dir.join("output.json"),
                    serde_json::to_string_pretty(&output)?,
                )?;
                record.output = Some(output);
                record.status = if job.status == crate::prover::model::ProverJobStatus::Cancelled {
                    AttemptRunStatus::Cancelled
                } else if matches!(
                    job.status,
                    crate::prover::model::ProverJobStatus::Failed
                        | crate::prover::model::ProverJobStatus::Error
                ) {
                    AttemptRunStatus::Failed
                } else {
                    AttemptRunStatus::Completed
                };
                record.completed_at = Some(Utc::now());
                record.duration_ms = Some(started.elapsed().as_millis());
                record.updated_at = Utc::now();
                store.update_attempt_run(&record)?;
            }
        }
    }

    let proof_result = record
        .proof_job_id
        .as_ref()
        .and_then(|id| proof_job::result(store, id).ok());

    Ok(AttemptRunResult {
        id: record.id.clone(),
        status: record.status,
        artifacts_dir: record.artifacts_dir.clone(),
        input: record.input.clone(),
        output: record.output.clone(),
        duration_ms: record.duration_ms,
        cost: record.cost,
        proof_result,
    })
}

/// Drive a running attempt to completion (mock-friendly for tests/CLI).
pub fn run_to_completion(
    store: &Store,
    config: &Config,
    attempt_id: &str,
    max_polls: u32,
    provider: Option<&dyn ModelProvider>,
) -> Result<AttemptRunResult> {
    for _ in 0..max_polls {
        let out = result(store, config, attempt_id, provider)?;
        if out.status != AttemptRunStatus::Running {
            return Ok(out);
        }
    }
    result(store, config, attempt_id, provider)
}
