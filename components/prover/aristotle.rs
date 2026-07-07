//! Aristotle / Harmonic external-prover adapter (lean-aristotle-mcp patterns).
//!
//! Mock mode is the default when `THEOREMATA_ARISTOTLE_MOCK=1` or no API key is
//! configured. Live mode shells out to a user-provided command via
//! `THEOREMATA_ARISTOTLE_COMMAND` (JSON request on stdin, JSON response on stdout).

use crate::{
    config::Config,
    db::Store,
    prover::{
        model::{ProofJob, ProofResult, ProverJobStatus, ProofTask},
        verify::verify_lean_output,
    },
};
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    time::Instant,
};

const BACKEND: &str = "aristotle";

pub fn mock_enabled() -> bool {
    std::env::var("THEOREMATA_ARISTOTLE_MOCK")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or_else(|_| std::env::var("THEOREMATA_ARISTOTLE_API_KEY").is_err())
}

pub fn submit(
    store: &Store,
    config: &Config,
    task: ProofTask,
    artifacts_dir: Option<std::path::PathBuf>,
) -> Result<ProofJob> {
    let external_id = if mock_enabled() {
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
        json!({"job_id": job.id, "task_id": task.id, "mock": mock_enabled()}),
    )?;
    if let Some(dir) = &artifacts_dir {
        write_artifact(dir, "task.json", &task)?;
        write_artifact(dir, "submit.json", &json!({"mock": mock_enabled(), "backend": BACKEND}))?;
    }
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
    let (status, percent, lean_code, counterexample, message) = if mock_enabled() {
        advance_mock(&job)?
    } else {
        poll_live(&job)?
    };
    job.status = status;
    job.percent_complete = percent;
    job.poll_count += 1;
    job.updated_at = Utc::now();

    if status.is_terminal() {
        job.completed_at = Some(Utc::now());
        let verification = lean_code
            .as_deref()
            .and_then(|code| verify_lean_output(config, code, &job.task.statement).ok());
        let result = ProofResult {
            task_id: job.task.id.clone(),
            job_id: job.id.clone(),
            status,
            lean_code: lean_code.clone(),
            counterexample,
            verification,
            artifacts_dir: job.artifacts_dir.clone(),
            duration_ms: started.elapsed().as_millis(),
            cost: None,
            message: message.clone(),
            provenance: json!({
                "backend": BACKEND,
                "mock": mock_enabled(),
                "poll_count": job.poll_count,
            }),
        };
        job.result = Some(result.clone());
        if let Some(dir) = &job.artifacts_dir {
            if let Some(code) = &lean_code {
                let lean_dir = dir.join("lean");
                std::fs::create_dir_all(&lean_dir)?;
                std::fs::write(lean_dir.join("solution.lean"), code)?;
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
        .unwrap_or_else(|| config.resources.join("mathlib4-master/mathlib4-master"));
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
        lean_project: crate::prover::model::LeanProject {
            root,
            toolchain: None,
            imports: vec!["Mathlib".into()],
            metadata: json!({}),
        },
        statement: statement.into(),
        stub: None,
        prompt: None,
        backend: BACKEND.into(),
        metadata: json!({}),
    }
}

fn advance_mock(
    job: &ProofJob,
) -> Result<(
    ProverJobStatus,
    f64,
    Option<String>,
    Option<String>,
    Option<String>,
)> {
    match job.poll_count {
        0 => Ok((
            ProverJobStatus::InProgress,
            40.0,
            None,
            None,
            Some("mock: working".into()),
        )),
        _ => {
            let code = mock_lean_solution(&job.task);
            Ok((
                ProverJobStatus::Proved,
                100.0,
                Some(code),
                None,
                Some("mock: proved".into()),
            ))
        }
    }
}

fn mock_lean_solution(task: &ProofTask) -> String {
    let name = task
        .theorem
        .full_name
        .rsplit('.')
        .next()
        .unwrap_or("MainTheorem");
    format!(
        "import Mathlib\n\n/-- Mock Aristotle proof. -/\ntheorem {name} : True := by\n  trivial\n"
    )
}

fn poll_live(
    job: &ProofJob,
) -> Result<(
    ProverJobStatus,
    f64,
    Option<String>,
    Option<String>,
    Option<String>,
)> {
    let cmd = std::env::var("THEOREMATA_ARISTOTLE_COMMAND")
        .context("THEOREMATA_ARISTOTLE_COMMAND is required for live Aristotle mode")?;
    let request = json!({
        "op": "poll",
        "job_id": job.id,
        "external_id": job.external_id,
        "task": job.task,
    });
    let response = run_command(&cmd, &request)?;
    parse_live_response(&response)
}

fn run_command(command: &str, request: &Value) -> Result<Value> {
    let mut child = Command::new("bash")
        .args(["-lc", command])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting Aristotle command: {command}"))?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())?;
    let stdout = child.stdout.take().context("capturing stdout")?;
    let mut lines = Vec::new();
    for line in BufReader::new(stdout).lines() {
        lines.push(line?);
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "Aristotle command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let body = lines.join("\n");
    Ok(serde_json::from_str(&body).unwrap_or(json!({"message": body})))
}

fn parse_live_response(
    response: &Value,
) -> Result<(
    ProverJobStatus,
    f64,
    Option<String>,
    Option<String>,
    Option<String>,
)> {
    let status = match response["status"].as_str().unwrap_or("error") {
        "submitted" => ProverJobStatus::Submitted,
        "queued" => ProverJobStatus::Queued,
        "in_progress" | "running" => ProverJobStatus::InProgress,
        "proved" | "success" => ProverJobStatus::Proved,
        "partial" => ProverJobStatus::Partial,
        "failed" => ProverJobStatus::Failed,
        "counterexample" => ProverJobStatus::Counterexample,
        "cancelled" => ProverJobStatus::Cancelled,
        _ => ProverJobStatus::Error,
    };
    let percent = response["percent_complete"]
        .as_f64()
        .or_else(|| response["percent"].as_f64())
        .unwrap_or(0.0);
    Ok((
        status,
        percent,
        response["code"]
            .as_str()
            .or_else(|| response["lean_code"].as_str())
            .map(str::to_owned),
        response["counterexample"].as_str().map(str::to_owned),
        response["message"].as_str().map(str::to_owned),
    ))
}

fn write_artifact(dir: &std::path::Path, rel: &str, value: &impl serde::Serialize) -> Result<()> {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(value)?)?;
    Ok(())
}