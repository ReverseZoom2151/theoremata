//! Integration tests for proof jobs, AttemptRun, and Aristotle mock backend.

use crate::{
    config::Config,
    db::Store,
    prover::{
        aristotle,
        attempt_run,
        model::{AttemptRunStatus, ProverJobStatus},
        proof_job,
    },
};
use serde_json::json;
use std::path::Path;
use std::sync::Mutex;

/// Serializes tests that mutate the process-global `THEOREMATA_ARISTOTLE_MOCK`
/// env var. `std::env::set_var`/`var` race across the default parallel test
/// threads (a concurrent read can miss the flag and take the live path), so any
/// test touching that env must hold this lock for its duration.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn test_config(dir: &Path) -> Config {
    let mut c = Config::default();
    c.database = dir.join("test.db");
    c.artifacts = dir.join("artifacts");
    c
}

#[test]
fn proof_task_submit_poll_result_mock() {
    let _env = lock_env();
    std::env::set_var("THEOREMATA_ARISTOTLE_MOCK", "1");
    let tmp = tempfile::tempdir().unwrap();
    let config = test_config(tmp.path());
    let store = Store::open(&config.database).unwrap();
    let project = store.create_project("p", "True").unwrap();

    let task = aristotle::build_task(
        Some(project.id.clone()),
        None,
        "True",
        "Theoremata.MockThm",
        &config,
    );
    let job = proof_job::submit(&store, &config, task, None).unwrap();
    assert_eq!(job.status, ProverJobStatus::Submitted);

    let mid = proof_job::poll(&store, &config, &job.id, None).unwrap();
    assert_eq!(mid.status, ProverJobStatus::InProgress);

    let done = proof_job::poll(&store, &config, &job.id, None).unwrap();
    assert_eq!(done.status, ProverJobStatus::Proved);
    assert!(done.result.as_ref().unwrap().lean_code.is_some());
    let verification = done.result.as_ref().unwrap().verification.as_ref().unwrap();
    assert!(verification.lexical_clean);
    assert!(verification.axioms_clean);

    let result = proof_job::result(&store, &job.id).unwrap();
    assert_eq!(result.status, ProverJobStatus::Proved);
}

#[test]
fn attempt_run_start_to_completion_writes_artifacts() {
    let _env = lock_env();
    std::env::set_var("THEOREMATA_ARISTOTLE_MOCK", "1");
    let tmp = tempfile::tempdir().unwrap();
    let config = test_config(tmp.path());
    let store = Store::open(&config.database).unwrap();
    let project = store.create_project("p", "True").unwrap();

    let record = attempt_run::start(
        &store,
        &config,
        &project.id,
        None,
        json!({"statement": "True", "theorem_name": "Theoremata.RunThm"}),
    )
    .unwrap();
    assert_eq!(record.status, AttemptRunStatus::Running);
    assert!(record.artifacts_dir.join("input.json").exists());

    let out = attempt_run::run_to_completion(&store, &config, &record.id, 8, None).unwrap();
    assert_eq!(out.status, AttemptRunStatus::Completed);
    assert!(out.proof_result.is_some());
    assert!(out.artifacts_dir.join("output.json").exists());
    assert!(out.artifacts_dir.join("lean/solution.lean").exists());
    assert!(out.artifacts_dir.join("verifier/report.json").exists());
}

#[test]
fn proof_job_cancel_is_terminal() {
    let _env = lock_env();
    std::env::set_var("THEOREMATA_ARISTOTLE_MOCK", "1");
    let tmp = tempfile::tempdir().unwrap();
    let config = test_config(tmp.path());
    let store = Store::open(&config.database).unwrap();
    let project = store.create_project("p", "True").unwrap();
    let task = aristotle::build_task(Some(project.id), None, "True", "T", &config);
    let job = proof_job::submit(&store, &config, task, None).unwrap();
    let cancelled = proof_job::cancel(&store, &job.id).unwrap();
    assert_eq!(cancelled.status, ProverJobStatus::Cancelled);
}