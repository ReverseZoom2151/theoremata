//! Proof-job orchestration: submit → poll → result.

use crate::{
    config::Config,
    db::Store,
    provider::ModelProvider,
    prover::{
        aristotle, external, isabelle, lean, leandojo,
        model::{ProofJob, ProofResult, ProofTask, ProverJobStatus},
        reprover, rocq,
    },
};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
        "agda" | "metamath" => external::submit(store, config, task, artifacts_dir),
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
        "agda" => external::poll(store, config, job_id, crate::prover::formal::FormalSystem::Agda),
        "metamath" => external::poll(store, config, job_id, crate::prover::formal::FormalSystem::Metamath),
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
        "agda" | "metamath" => external::cancel(store, job_id),
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
        "agda" => external::mock_enabled(config, crate::prover::formal::FormalSystem::Agda)
            || std::env::var("THEOREMATA_AGDA_COMMAND").is_ok(),
        "metamath" => external::mock_enabled(config, crate::prover::formal::FormalSystem::Metamath)
            || std::env::var("THEOREMATA_METAMATH_COMMAND").is_ok(),
        "leandojo" => leandojo::available(),
        "reprover" => reprover::available(model_ready),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Checkpoint / sparse-poll / resume state machine
// ---------------------------------------------------------------------------
//
// Long-running external-prover jobs are polled sparsely (with exponential
// backoff) rather than blocked on. This layer lets an interrupted poll loop
// RESUME a job from where it left off instead of restarting: the [`ResumeState`]
// is serializable, so it can be persisted between process runs / poll ticks and
// reloaded. Everything here is pure and deterministic — no wall clock, no rand,
// no sleeping. The scheduler computes the next-poll delay as *data*
// ([`PollDecision::Poll { after }`]); an outer driver decides when to actually
// wait. Poll indices and backoff are threaded through state deterministically so
// the same inputs always yield the same plan.

/// Tuning for the sparse-poll / resume loop. Deterministic: contains no clock.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResumeConfig {
    /// Hard budget: give up once this many polls have been performed without a
    /// terminal status (prevents an unbounded poll loop).
    pub max_polls: u32,
    /// Backoff applied before the first poll (in abstract "ticks", not seconds).
    pub base_backoff: u64,
    /// Ceiling the exponential backoff is clamped to.
    pub max_backoff: u64,
}

impl Default for ResumeConfig {
    fn default() -> Self {
        Self {
            max_polls: 16,
            base_backoff: 1,
            max_backoff: 64,
        }
    }
}

/// Serializable snapshot of *where a job is* in its poll lifecycle. Persist this
/// between poll ticks / process restarts and reload it to resume the job from
/// `last_poll_index` rather than restarting from scratch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResumeState {
    /// The proof-job this state tracks.
    pub job_id: String,
    /// Number of polls already performed (also the index of the *next* poll).
    /// A resumed job continues from here instead of resetting to 0.
    pub last_poll_index: u32,
    /// Backoff (in ticks) to wait before the next poll; grows exponentially.
    pub next_backoff: u64,
    /// Whether the job has reached a terminal status.
    pub terminal: bool,
    /// Last observed backend status (None before the first poll).
    pub last_status: Option<ProverJobStatus>,
    /// Opaque backend-supplied progress marker, carried across resumes so the
    /// backend can continue rather than recompute. Defaults to `null`.
    #[serde(default)]
    pub checkpoint: Value,
}

impl ResumeState {
    /// Fresh state for a never-polled job, primed with the base backoff.
    pub fn new(job_id: impl Into<String>, cfg: &ResumeConfig) -> Self {
        Self {
            job_id: job_id.into(),
            last_poll_index: 0,
            next_backoff: cfg.base_backoff.max(1),
            terminal: false,
            last_status: None,
            checkpoint: Value::Null,
        }
    }
}

/// One poll observation, returned by an injected poller.
#[derive(Debug, Clone, PartialEq)]
pub struct PollResult {
    pub status: ProverJobStatus,
    /// Optional updated checkpoint to persist for the next resume.
    pub checkpoint: Option<Value>,
}

impl PollResult {
    pub fn new(status: ProverJobStatus) -> Self {
        Self {
            status,
            checkpoint: None,
        }
    }
}

/// What the scheduler should do next, computed purely from a [`ResumeState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollDecision {
    /// Fresh start: poll now, waiting `after` ticks first (data, not a sleep).
    Poll { after: u64 },
    /// Continue an in-flight job from poll index `from` (do not restart).
    Resume { from: u32 },
    /// Poll budget exhausted without a terminal status.
    GiveUp,
    /// Job reached a terminal status; nothing more to do.
    Done,
}

/// Terminal outcome of driving a job with [`resume_job`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeOutcomeKind {
    Done,
    GaveUp,
}

/// Final [`ResumeState`] plus the terminal outcome of the driver.
#[derive(Debug, Clone, PartialEq)]
pub struct ResumeOutcome {
    pub kind: ResumeOutcomeKind,
    pub state: ResumeState,
}

/// Pure, deterministic scheduler step: decide what to do next from the current
/// state + config. No I/O, no clock, no sleeping.
///
/// - terminal reached → [`PollDecision::Done`]
/// - budget exhausted (index ≥ `max_polls`) → [`PollDecision::GiveUp`]
/// - never polled (index 0) → [`PollDecision::Poll`] with the current backoff
/// - in-flight (index > 0) → [`PollDecision::Resume`] from `last_poll_index`
pub fn sparse_poll_plan(state: &ResumeState, cfg: &ResumeConfig) -> PollDecision {
    if state.terminal {
        return PollDecision::Done;
    }
    if state.last_poll_index >= cfg.max_polls {
        return PollDecision::GiveUp;
    }
    if state.last_poll_index == 0 {
        PollDecision::Poll {
            after: state.next_backoff,
        }
    } else {
        PollDecision::Resume {
            from: state.last_poll_index,
        }
    }
}

/// Grow the backoff exponentially, clamped to `max_backoff`. Pure helper.
fn advance_backoff(current: u64, cfg: &ResumeConfig) -> u64 {
    current.saturating_mul(2).min(cfg.max_backoff.max(cfg.base_backoff))
}

/// Drive a job to a terminal state (or budget exhaustion) by repeatedly applying
/// [`sparse_poll_plan`] and invoking the injected `poller`. Deterministic and
/// offline: the poller is the only side-effect boundary (mock in tests, real
/// backend in production). Resumes from `state.last_poll_index` rather than
/// restarting, and never reports a false `Done` — an exhausted budget yields
/// [`ResumeOutcomeKind::GaveUp`].
pub fn resume_job(
    mut state: ResumeState,
    poller: &dyn Fn(&ResumeState) -> PollResult,
    cfg: &ResumeConfig,
) -> ResumeOutcome {
    loop {
        match sparse_poll_plan(&state, cfg) {
            PollDecision::Done => {
                return ResumeOutcome {
                    kind: ResumeOutcomeKind::Done,
                    state,
                };
            }
            PollDecision::GiveUp => {
                return ResumeOutcome {
                    kind: ResumeOutcomeKind::GaveUp,
                    state,
                };
            }
            PollDecision::Poll { .. } | PollDecision::Resume { .. } => {
                // The `after`/`from` delay is data the scheduler would honor;
                // here we advance state deterministically without sleeping.
                let observation = poller(&state);
                state.last_poll_index = state.last_poll_index.saturating_add(1);
                state.last_status = Some(observation.status);
                state.terminal = observation.status.is_terminal();
                if let Some(cp) = observation.checkpoint {
                    state.checkpoint = cp;
                }
                state.next_backoff = advance_backoff(state.next_backoff, cfg);
            }
        }
    }
}

#[cfg(test)]
mod resume_tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;

    fn cfg() -> ResumeConfig {
        ResumeConfig {
            max_polls: 8,
            base_backoff: 1,
            max_backoff: 8,
        }
    }

    #[test]
    fn terminates_after_three_polls_resuming_across_polls() {
        let cfg = cfg();
        let calls = Cell::new(0u32);
        // Records the poll index seen on each call to prove we never restart.
        let seen: std::cell::RefCell<Vec<u32>> = std::cell::RefCell::new(vec![]);
        let poller = |s: &ResumeState| {
            calls.set(calls.get() + 1);
            seen.borrow_mut().push(s.last_poll_index);
            if s.last_poll_index >= 2 {
                PollResult::new(ProverJobStatus::Proved)
            } else {
                PollResult::new(ProverJobStatus::InProgress)
            }
        };
        let out = resume_job(ResumeState::new("job-1", &cfg), &poller, &cfg);
        assert_eq!(out.kind, ResumeOutcomeKind::Done);
        assert_eq!(calls.get(), 3, "exactly three polls");
        // Indices strictly increase 0,1,2 — resumed, never restarted from 0.
        assert_eq!(*seen.borrow(), vec![0, 1, 2]);
        assert_eq!(out.state.last_poll_index, 3);
        assert!(out.state.terminal);
        assert_eq!(out.state.last_status, Some(ProverJobStatus::Proved));
    }

    #[test]
    fn backoff_grows_exponentially_and_is_capped() {
        let cfg = cfg();
        // Never terminal, so we exhaust the budget while observing backoff.
        let poller = |_: &ResumeState| PollResult::new(ProverJobStatus::InProgress);
        let out = resume_job(ResumeState::new("job-2", &cfg), &poller, &cfg);
        assert_eq!(out.kind, ResumeOutcomeKind::GaveUp);
        // base=1 doubled each poll, clamped at max_backoff=8.
        assert_eq!(out.state.next_backoff, 8);
    }

    #[test]
    fn pure_backoff_sequence_is_exponential_then_capped() {
        let cfg = cfg();
        let mut b = cfg.base_backoff;
        let mut seq = vec![b];
        for _ in 0..6 {
            b = advance_backoff(b, &cfg);
            seq.push(b);
        }
        assert_eq!(seq, vec![1, 2, 4, 8, 8, 8, 8]);
    }

    #[test]
    fn budget_exhaustion_gives_up_never_false_done() {
        let cfg = ResumeConfig {
            max_polls: 3,
            base_backoff: 1,
            max_backoff: 4,
        };
        let calls = Cell::new(0u32);
        let poller = |_: &ResumeState| {
            calls.set(calls.get() + 1);
            PollResult::new(ProverJobStatus::InProgress)
        };
        let out = resume_job(ResumeState::new("job-3", &cfg), &poller, &cfg);
        assert_eq!(out.kind, ResumeOutcomeKind::GaveUp);
        assert_eq!(calls.get(), 3, "polled exactly up to the budget");
        assert_eq!(out.state.last_poll_index, 3);
        assert!(!out.state.terminal);
    }

    #[test]
    fn resumes_from_persisted_state_at_last_poll_index() {
        let cfg = cfg();
        // Simulate a state reloaded after 2 prior polls (e.g. an interrupted run).
        let mut reloaded = ResumeState::new("job-4", &cfg);
        reloaded.last_poll_index = 2;
        reloaded.next_backoff = 4;
        reloaded.last_status = Some(ProverJobStatus::InProgress);

        // Plan on a mid-flight state must be Resume{from: 2}, not a fresh Poll.
        match sparse_poll_plan(&reloaded, &cfg) {
            PollDecision::Resume { from } => assert_eq!(from, 2),
            other => panic!("expected Resume{{from:2}}, got {other:?}"),
        }

        let first_seen = std::cell::Cell::new(u32::MAX);
        let poller = |s: &ResumeState| {
            if first_seen.get() == u32::MAX {
                first_seen.set(s.last_poll_index);
            }
            if s.last_poll_index >= 3 {
                PollResult::new(ProverJobStatus::Proved)
            } else {
                PollResult::new(ProverJobStatus::InProgress)
            }
        };
        let out = resume_job(reloaded, &poller, &cfg);
        assert_eq!(first_seen.get(), 2, "continued from index 2, did not restart");
        assert_eq!(out.kind, ResumeOutcomeKind::Done);
        assert_eq!(out.state.last_poll_index, 4);
    }

    #[test]
    fn fresh_state_plan_is_poll_with_base_backoff() {
        let cfg = cfg();
        let state = ResumeState::new("job-5", &cfg);
        assert_eq!(sparse_poll_plan(&state, &cfg), PollDecision::Poll { after: 1 });
    }

    #[test]
    fn terminal_state_plan_is_done() {
        let cfg = cfg();
        let mut state = ResumeState::new("job-6", &cfg);
        state.terminal = true;
        state.last_status = Some(ProverJobStatus::Proved);
        assert_eq!(sparse_poll_plan(&state, &cfg), PollDecision::Done);
    }

    #[test]
    fn plan_is_deterministic_same_inputs_same_output() {
        let cfg = cfg();
        let state = ResumeState::new("job-7", &cfg);
        let a = sparse_poll_plan(&state, &cfg);
        let b = sparse_poll_plan(&state, &cfg);
        assert_eq!(a, b);

        // Full-run determinism: identical mock → identical final state.
        let poller = |s: &ResumeState| {
            if s.last_poll_index >= 2 {
                PollResult::new(ProverJobStatus::Failed)
            } else {
                PollResult::new(ProverJobStatus::InProgress)
            }
        };
        let r1 = resume_job(ResumeState::new("job-7", &cfg), &poller, &cfg);
        let r2 = resume_job(ResumeState::new("job-7", &cfg), &poller, &cfg);
        assert_eq!(r1, r2);
        // Failed is terminal → Done outcome (the loop finished the job).
        assert_eq!(r1.kind, ResumeOutcomeKind::Done);
    }

    #[test]
    fn resume_state_round_trips_through_json() {
        let cfg = cfg();
        let mut state = ResumeState::new("job-8", &cfg);
        state.last_poll_index = 5;
        state.next_backoff = 8;
        state.last_status = Some(ProverJobStatus::InProgress);
        state.checkpoint = json!({"cursor": 42});
        let text = serde_json::to_string(&state).unwrap();
        let back: ResumeState = serde_json::from_str(&text).unwrap();
        assert_eq!(state, back);
    }

    #[test]
    fn checkpoint_is_carried_across_polls() {
        let cfg = cfg();
        let poller = |s: &ResumeState| {
            let next = s.last_poll_index + 1;
            let status = if s.last_poll_index >= 1 {
                ProverJobStatus::Proved
            } else {
                ProverJobStatus::InProgress
            };
            PollResult {
                status,
                checkpoint: Some(json!({ "step": next })),
            }
        };
        let out = resume_job(ResumeState::new("job-9", &cfg), &poller, &cfg);
        assert_eq!(out.kind, ResumeOutcomeKind::Done);
        assert_eq!(out.state.checkpoint, json!({"step": 2}));
    }
}
