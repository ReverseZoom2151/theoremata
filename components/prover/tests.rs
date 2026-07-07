//! Integration tests for proof jobs, AttemptRun, and Aristotle mock backend.

use crate::{
    config::Config,
    db::Store,
    prover::{
        aristotle,
        attempt_run,
        formal::{FormalSystem, ProofSession, SessionError},
        isabelle,
        model::{AttemptRunStatus, ProverJobStatus},
        proof_job,
        rocq,
    },
};
use serde_json::json;
use std::path::Path;
use std::str::FromStr;

fn test_config(dir: &Path) -> Config {
    let mut c = Config::default();
    c.database = dir.join("test.db");
    c.artifacts = dir.join("artifacts");
    // Force mock via config, NOT the process-global env — `std::env::set_var`
    // races against every other test's `env::var` read under parallel execution.
    c.prover_mock = true;
    c
}

#[test]
fn proof_task_submit_poll_result_mock() {
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
    assert!(done.result.as_ref().unwrap().formal_code.is_some());
    let verification = done.result.as_ref().unwrap().verification.as_ref().unwrap();
    assert!(verification.lexical_clean);
    assert!(verification.axioms_clean);

    let result = proof_job::result(&store, &job.id).unwrap();
    assert_eq!(result.status, ProverJobStatus::Proved);
}

#[test]
fn attempt_run_start_to_completion_writes_artifacts() {
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
fn rocq_submit_poll_result_mock() {
    let tmp = tempfile::tempdir().unwrap();
    let config = test_config(tmp.path());
    let store = Store::open(&config.database).unwrap();
    let project = store.create_project("p", "True").unwrap();

    let task = rocq::build_task(
        Some(project.id.clone()),
        None,
        "True",
        "Theoremata.RocqThm",
        &config,
    );
    assert_eq!(task.backend, "rocq");
    assert_eq!(task.system, FormalSystem::Rocq);

    let job = proof_job::submit(&store, &config, task, None).unwrap();
    assert_eq!(job.status, ProverJobStatus::Submitted);

    let mid = proof_job::poll(&store, &config, &job.id, None).unwrap();
    assert_eq!(mid.status, ProverJobStatus::InProgress);

    let done = proof_job::poll(&store, &config, &job.id, None).unwrap();
    assert_eq!(done.status, ProverJobStatus::Proved);
    let result = done.result.as_ref().unwrap();
    let code = result.formal_code.as_ref().unwrap();
    assert!(code.contains("Qed."), "rocq mock must emit a .v proof: {code}");
    let v = result.verification.as_ref().unwrap();
    assert!(v.lexically_verified);
    assert!(v.axioms_clean);
    assert!(v.lexical_clean);
}

#[test]
fn isabelle_submit_poll_result_mock() {
    let tmp = tempfile::tempdir().unwrap();
    let config = test_config(tmp.path());
    let store = Store::open(&config.database).unwrap();
    let project = store.create_project("p", "True").unwrap();

    let task = isabelle::build_task(
        Some(project.id.clone()),
        None,
        "True",
        "Theoremata.IsabelleThm",
        &config,
    );
    assert_eq!(task.backend, "isabelle");
    assert_eq!(task.system, FormalSystem::Isabelle);

    let job = proof_job::submit(&store, &config, task, None).unwrap();
    assert_eq!(job.status, ProverJobStatus::Submitted);

    let mid = proof_job::poll(&store, &config, &job.id, None).unwrap();
    assert_eq!(mid.status, ProverJobStatus::InProgress);

    let done = proof_job::poll(&store, &config, &job.id, None).unwrap();
    assert_eq!(done.status, ProverJobStatus::Proved);
    let result = done.result.as_ref().unwrap();
    let code = result.formal_code.as_ref().unwrap();
    assert!(code.contains("by simp"), "isabelle mock must emit a .thy proof: {code}");
    let v = result.verification.as_ref().unwrap();
    assert!(v.lexically_verified);
    assert!(v.axioms_clean);
    assert!(v.lexical_clean);
}

#[test]
fn isabelle_step_tactic_is_unsupported() {
    let mut backend = isabelle::IsabelleBackend { mock: true };
    let err = backend.step_tactic(0, "by simp").unwrap_err();
    assert!(
        matches!(err.downcast_ref::<SessionError>(), Some(SessionError::Unsupported(_))),
        "Isabelle step_tactic must return SessionError::Unsupported, got {err}"
    );
    // submit_unit IS supported and accepts the clean mock proof.
    let unit = backend.submit_unit("theorem t: \"True\" by simp").unwrap();
    assert!(unit.ok);
}

#[test]
fn rocq_step_tactic_is_supported() {
    let mut backend = rocq::RocqBackend { mock: true };
    let step = backend.step_tactic(0, "exact I").unwrap();
    assert!(step.finished);
    assert_eq!(step.state, 1);
}

#[test]
fn source_scan_rejects_escape_hatches() {
    use crate::prover::formal::FormalBackend;
    let rocq_b = rocq::RocqBackend { mock: true };
    assert!(!rocq_b.source_scan("Theorem t: True. Admitted.").unwrap().clean);
    let isa_b = isabelle::IsabelleBackend { mock: true };
    assert!(!isa_b.source_scan("theorem t: \"True\" sorry").unwrap().clean);
}

#[test]
fn formal_system_from_str_and_display() {
    assert_eq!(FormalSystem::from_str("coq").unwrap(), FormalSystem::Rocq);
    assert_eq!(FormalSystem::from_str("Isabelle/HOL").unwrap(), FormalSystem::Isabelle);
    assert_eq!(FormalSystem::Lean.to_string(), "lean");
    assert_eq!(FormalSystem::default(), FormalSystem::Lean);
}

#[test]
fn lean_project_alias_deserializes() {
    // Old serialized ProofTask used `lean_project` and had no `system` field.
    let legacy = json!({
        "id": "t1",
        "project_id": null,
        "node_id": null,
        "theorem": {"repo": null, "commit": null, "file": null, "full_name": "T", "line": null},
        "lean_project": {"root": "/tmp/x", "toolchain": null, "imports": ["Mathlib"], "metadata": {}},
        "statement": "True",
        "stub": null,
        "prompt": null,
        "backend": "aristotle",
        "metadata": {}
    });
    let task: crate::prover::model::ProofTask = serde_json::from_value(legacy).unwrap();
    assert_eq!(task.system, FormalSystem::Lean);
    assert_eq!(task.formal_project.imports, vec!["Mathlib".to_string()]);

    // Old ProofResult used `lean_code`.
    let legacy_res = json!({
        "task_id": "t1", "job_id": "j1", "status": "proved",
        "lean_code": "theorem T : True := trivial",
        "counterexample": null, "verification": null, "artifacts_dir": null,
        "duration_ms": 1, "cost": null, "message": null, "provenance": {}
    });
    let res: crate::prover::model::ProofResult = serde_json::from_value(legacy_res).unwrap();
    assert!(res.formal_code.unwrap().contains("trivial"));
}

#[test]
fn proof_job_cancel_is_terminal() {
    let tmp = tempfile::tempdir().unwrap();
    let config = test_config(tmp.path());
    let store = Store::open(&config.database).unwrap();
    let project = store.create_project("p", "True").unwrap();
    let task = aristotle::build_task(Some(project.id), None, "True", "T", &config);
    let job = proof_job::submit(&store, &config, task, None).unwrap();
    let cancelled = proof_job::cancel(&store, &job.id).unwrap();
    assert_eq!(cancelled.status, ProverJobStatus::Cancelled);
}