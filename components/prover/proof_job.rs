//! Proof-job orchestration: submit → poll → result.

use crate::{
    config::Config,
    db::Store,
    provider::ModelProvider,
    prover::{
        aristotle, isabelle, lean, leandojo, model::{ProofJob, ProofResult, ProofTask}, reprover,
        rocq,
    },
};
use anyhow::{anyhow, Result};

pub fn submit(
    store: &Store,
    config: &Config,
    task: ProofTask,
    artifacts_dir: Option<std::path::PathBuf>,
) -> Result<ProofJob> {
    match task.backend.as_str() {
        "aristotle" => aristotle::submit(store, config, task, artifacts_dir),
        "lean" => lean::submit(store, config, task, artifacts_dir),
        "rocq" => rocq::submit(store, config, task, artifacts_dir),
        "isabelle" => isabelle::submit(store, config, task, artifacts_dir),
        "leandojo" => leandojo::submit(store, config, task, artifacts_dir),
        "reprover" => reprover::submit(store, config, task, artifacts_dir),
        other => Err(anyhow!("unsupported prover backend: {other}")),
    }
}

pub fn poll(
    store: &Store,
    config: &Config,
    job_id: &str,
    provider: Option<&dyn ModelProvider>,
) -> Result<ProofJob> {
    let job = store
        .get_proof_job(job_id)?
        .ok_or_else(|| anyhow!("unknown proof job {job_id}"))?;
    match job.backend.as_str() {
        "aristotle" => aristotle::poll(store, config, job_id),
        "lean" => lean::poll(store, config, job_id),
        "rocq" => rocq::poll(store, config, job_id),
        "isabelle" => isabelle::poll(store, config, job_id),
        "leandojo" => leandojo::poll(store, config, job_id),
        "reprover" => {
            let p = provider.ok_or_else(|| anyhow!("reprover backend requires a model provider"))?;
            reprover::poll_with_provider(store, config, p, job_id)
        }
        other => Err(anyhow!("unsupported prover backend: {other}")),
    }
}

pub fn cancel(store: &Store, job_id: &str) -> Result<ProofJob> {
    let job = store
        .get_proof_job(job_id)?
        .ok_or_else(|| anyhow!("unknown proof job {job_id}"))?;
    match job.backend.as_str() {
        "aristotle" => aristotle::cancel(store, job_id),
        "lean" => lean::cancel(store, job_id),
        "rocq" => rocq::cancel(store, job_id),
        "isabelle" => isabelle::cancel(store, job_id),
        "leandojo" => leandojo::cancel(store, job_id),
        "reprover" => reprover::cancel(store, job_id),
        other => Err(anyhow!("unsupported prover backend: {other}")),
    }
}

pub fn result(store: &Store, job_id: &str) -> Result<ProofResult> {
    let job = store
        .get_proof_job(job_id)?
        .ok_or_else(|| anyhow!("unknown proof job {job_id}"))?;
    job.result
        .clone()
        .ok_or_else(|| anyhow!("job {job_id} has no result yet (status={:?})", job.status))
}

pub fn materialize_result(job: &ProofJob) -> Option<ProofResult> {
    job.result.clone()
}

pub fn any_prover_available(config: &Config, model_ready: bool) -> bool {
    match config.prover_backend.as_str() {
        "aristotle" => {
            aristotle::mock_enabled(config) || std::env::var("THEOREMATA_ARISTOTLE_COMMAND").is_ok()
        }
        "lean" => {
            lean::mock_enabled(config) || std::env::var("THEOREMATA_LEAN_COMMAND").is_ok()
        }
        "rocq" => {
            rocq::mock_enabled(config) || std::env::var("THEOREMATA_ROCQ_COMMAND").is_ok()
        }
        "isabelle" => {
            isabelle::mock_enabled(config) || std::env::var("THEOREMATA_ISABELLE_COMMAND").is_ok()
        }
        "leandojo" => leandojo::available(),
        "reprover" => reprover::available(model_ready),
        _ => false,
    }
}