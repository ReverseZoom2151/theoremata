//! LeanDojo-style tactic-session prover backend (mock + live command).

use crate::{
    config::Config,
    db::Store,
    prover::{
        model::{ProofJob, ProofResult, ProofTask, ProverJobStatus, VerificationReport},
        verify::{
            provenance_hash, verify_lean_output, verify_lean_output_hardened, HardeningContext,
        },
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
            .and_then(|code| verify_output(store, config, &job, code).ok());
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
        // Mirror of the aristotle site: the completion branch, where the store,
        // the result and the graph coordinates are all in hand. Written before
        // the result is moved into the job.
        record_artifact_evidence(store, &job, &result)?;
        job.result = Some(result);
    }
    store.update_proof_job(&job)?;
    Ok(job)
}

/// Verify external-prover output, running the deep hardening battery whenever we
/// have somewhere to file its evidence.
///
/// An external prover is the least trusted source in the system, so this is the
/// worst place for the hardening layer to sit inert: the storeless entry point
/// can only ever report `requested_but_could_not_run`. `hardening::harden`
/// writes an evidence row against a graph node, so it needs BOTH a project and a
/// node id.
///
/// When either id is missing there is no node to attach that evidence to, and
/// inventing a placeholder would file a soundness audit against something that
/// does not exist. We fall back to the storeless path instead, which honestly
/// reports could-not-run. That is the truth: we could not run it.
fn verify_output(
    store: &Store,
    config: &Config,
    job: &ProofJob,
    code: &str,
) -> Result<VerificationReport> {
    match (job.task.project_id.as_deref(), job.task.node_id.as_deref()) {
        (Some(project_id), Some(node_id)) => verify_lean_output_hardened(
            &HardeningContext {
                store,
                project_id,
                node_id,
            },
            config,
            code,
            &job.task.statement,
        ),
        _ => verify_lean_output(config, code, &job.task.statement),
    }
}

/// File the artifact-provenance row for a completed external-prover job.
///
/// Called from the completion branch of `poll`, the mirror of the aristotle
/// site: that is the only point where the store, the result and the graph
/// coordinates are all in hand at once.
///
/// Both graph ids are required together. An evidence row hangs off a node, so if
/// either id is absent there is no node to attach it to; inventing a placeholder
/// would file real provenance against a node that does not exist. We write
/// nothing in that case, which is what "this job has no graph coordinates"
/// honestly means.
fn record_artifact_evidence(store: &Store, job: &ProofJob, result: &ProofResult) -> Result<()> {
    let (Some(project_id), Some(node_id)) = (job.project_id.as_deref(), job.node_id.as_deref())
    else {
        return Ok(());
    };
    let output_hash = result.formal_code.as_deref().map(provenance_hash);
    store.add_evidence(
        project_id,
        node_id,
        // Written as a literal, not as `evidence::EXTERNAL_PROVER_ARTIFACT`,
        // because the drift guard in `components/graph/evidence.rs` only counts
        // a statically visible `kind` argument. The test below pins the literal
        // to the declared constant so the two cannot drift apart.
        "external_prover_artifact",
        BACKEND,
        // An audit-trail record that provenance was captured, NOT a verdict on
        // the proof. The session's own claimed status goes in the payload, where
        // it cannot be misread as a gate this row never ran.
        "recorded",
        crate::graph::evidence::external_prover_payload(
            BACKEND,
            job.external_id.as_deref(),
            Some(&provenance_hash(&job.task.statement)),
            output_hash.as_deref(),
            Some(result.duration_ms),
            result.cost,
            json!({
                "job_id": job.id,
                "task_id": result.task_id,
                "claimed_status": result.status,
                "artifacts_dir": job.artifacts_dir,
            }),
        ),
    )?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::NodeKind,
        prover::{
            formal::FormalSystem,
            model::{FormalProject, TheoremIdentity},
        },
    };
    use std::path::{Path, PathBuf};

    const PROOF: &str = "theorem t : True := trivial\n";

    fn config(harden: bool) -> Config {
        let mut config = Config::default();
        config.harden_proofs = harden;
        // No Mathlib checkout under test, so `harden` stops at its own
        // preconditions. That is what makes the hardened path observable here
        // without a Lean toolchain: it reports `skipped`, which only the code
        // path that actually CALLED harden can produce.
        config.lean_project = None;
        config
    }

    fn job_with(store: &Store, ids: (Option<String>, Option<String>)) -> ProofJob {
        let task = ProofTask {
            id: uuid::Uuid::new_v4().to_string(),
            project_id: ids.0,
            node_id: ids.1,
            theorem: TheoremIdentity {
                repo: None,
                commit: None,
                file: None,
                full_name: "t".into(),
                line: None,
            },
            system: FormalSystem::Lean,
            formal_project: FormalProject {
                system: FormalSystem::Lean,
                root: PathBuf::from("."),
                toolchain: None,
                imports: vec![],
                metadata: json!({}),
            },
            statement: "theorem t : True".into(),
            stub: None,
            prompt: None,
            backend: BACKEND.into(),
            metadata: json!({}),
        };
        store
            .create_proof_job(&task, BACKEND, ProverJobStatus::Submitted, None, None, 0.0)
            .unwrap()
    }

    fn graph_ids(store: &Store) -> (Option<String>, Option<String>) {
        let project = store.create_project("p", "t").unwrap();
        let node = store
            .add_node(&project.id, NodeKind::FormalStatement, "f", "s", "test")
            .unwrap();
        (Some(project.id), Some(node.id))
    }

    fn hardening(report: &VerificationReport) -> &Value {
        report
            .detail
            .get("hardening")
            .expect("the hardening block is always present")
    }

    #[test]
    fn hardened_path_is_taken_when_project_and_node_are_present() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let job = job_with(&store, graph_ids(&store));
        let report = verify_output(&store, &config(true), &job, PROOF).unwrap();
        // `skipped` can only come back from a real `harden` call, so this is the
        // proof that the storeless entry point is no longer in the way.
        assert_eq!(
            hardening(&report)
                .get("report")
                .and_then(|r| r.get("outcome"))
                .and_then(Value::as_str),
            Some("skipped"),
            "hardening must actually be invoked when the graph ids exist"
        );
        assert_eq!(report.hardening_clean, Some(false));
    }

    #[test]
    fn missing_ids_fall_back_and_report_could_not_run() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let (project, node) = graph_ids(&store);
        for ids in [(None, node.clone()), (project.clone(), None), (None, None)] {
            let job = job_with(&store, ids);
            let report = verify_output(&store, &config(true), &job, PROOF).unwrap();
            assert_ne!(
                report.hardening_clean,
                Some(true),
                "an unrun check must never read as clean"
            );
            assert_eq!(
                hardening(&report).get("state").and_then(Value::as_str),
                Some("requested_but_could_not_run"),
                "no node to file evidence against means we could not run it"
            );
        }
    }

    #[test]
    fn hardening_off_is_unchanged_on_both_paths() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        for ids in [graph_ids(&store), (None, None)] {
            let job = job_with(&store, ids);
            let report = verify_output(&store, &config(false), &job, PROOF).unwrap();
            assert_eq!(
                report.hardening_clean, None,
                "the default stays off, and an unrequested check rejects nothing"
            );
            assert_eq!(
                hardening(&report).get("state").and_then(Value::as_str),
                Some("not_requested")
            );
        }
    }

    #[test]
    fn the_default_config_does_not_request_hardening() {
        assert!(
            !Config::default().harden_proofs,
            "this change must not flip the default on"
        );
    }

    // --- the external_prover_artifact audit row ---------------------------

    fn completed(job: &ProofJob) -> ProofResult {
        ProofResult {
            task_id: job.task.id.clone(),
            job_id: job.id.clone(),
            status: ProverJobStatus::Proved,
            formal_code: Some(PROOF.into()),
            counterexample: None,
            verification: None,
            artifacts_dir: None,
            duration_ms: 7,
            cost: None,
            message: None,
            provenance: json!({}),
        }
    }

    #[test]
    fn a_row_is_written_when_both_graph_ids_are_present() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let ids = graph_ids(&store);
        let job = job_with(&store, ids.clone());
        record_artifact_evidence(&store, &job, &completed(&job)).unwrap();
        let rows = store
            .evidence_of_kind(
                ids.0.as_deref().unwrap(),
                ids.1.as_deref().unwrap(),
                "external_prover_artifact",
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, BACKEND);
        assert_eq!(
            rows[0].verdict, "recorded",
            "an audit-trail row must not read as a gate verdict"
        );
        assert_eq!(
            rows[0].payload.get("service").and_then(Value::as_str),
            Some(BACKEND),
            "the payload builder in graph::evidence must be the one used"
        );
    }

    #[test]
    fn a_missing_graph_id_writes_no_row_and_does_not_panic() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let (project, node) = graph_ids(&store);
        for ids in [(None, node.clone()), (project.clone(), None), (None, None)] {
            let job = job_with(&store, ids);
            record_artifact_evidence(&store, &job, &completed(&job)).unwrap();
        }
        assert!(
            store
                .project_evidence(project.as_deref().unwrap())
                .unwrap()
                .is_empty(),
            "no node to attach to means no row, never a fabricated node id"
        );
    }

    #[test]
    fn the_emitted_kind_matches_the_declared_constant() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let ids = graph_ids(&store);
        let job = job_with(&store, ids.clone());
        record_artifact_evidence(&store, &job, &completed(&job)).unwrap();
        let rows = store
            .evidence(ids.0.as_deref().unwrap(), ids.1.as_deref().unwrap())
            .unwrap();
        assert_eq!(
            rows.iter()
                .filter(|e| e.evidence_type
                    == crate::graph::evidence::EXTERNAL_PROVER_ARTIFACT)
                .count(),
            1,
            "the literal `kind` and the registry constant must not drift apart"
        );
    }
}
