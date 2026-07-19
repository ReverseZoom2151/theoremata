//! Isabelle/HOL external-prover adapter — Phase 1 mock backend.
//!
//! Mirrors the `aristotle` mock EXACTLY (Config.prover_mock-driven,
//! submit → poll(InProgress → Proved) → result + a `VerificationReport`), but
//! emits SYSTEM-NATIVE Isabelle theory (`.thy`) proofs and routes verification
//! through the system-agnostic [`FormalBackend`] 3+1-layer gate. Isabelle is
//! theory-file granular (no per-tactic stepping); the driver's `step_tactic`
//! returns [`crate::prover::formal::SessionError::Unsupported`]. Live mode
//! (Phase 2) will replace the canned layers with `isabelle build` /
//! `thm_oracles` / `Thm_Deps`; the source scan already runs for real.

use crate::{
    config::Config,
    db::Store,
    prover::{
        exec::{self, Runner},
        formal::{
            AxiomReport, CompileReport, FormalBackend, FormalSystem, GoalState, ProofSession,
            RecheckReport, ScanReport, SessionError, StateResult, UnitResult, Workspace,
        },
        model::{FormalProject, ProofJob, ProofResult, ProofTask, ProverJobStatus},
    },
};
use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::json;
use std::{path::PathBuf, time::Instant};

const BACKEND: &str = "isabelle";
const SYSTEM: FormalSystem = FormalSystem::Isabelle;

pub fn mock_enabled(config: &Config) -> bool {
    // Config flag short-circuits BEFORE any env read, so parallel tests never
    // race on the process-global environment.
    config.prover_mock
        || std::env::var("THEOREMATA_ISABELLE_MOCK")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or_else(|_| std::env::var("THEOREMATA_ISABELLE_COMMAND").is_err())
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
        // Live path: verify the candidate proof through the real 3+1-layer gate.
        return crate::prover::formal::live_poll(store, config, job, BACKEND, SYSTEM);
    }
    let started = Instant::now();
    let (status, percent, formal_code, message) = advance_mock(&job);
    job.status = status;
    job.percent_complete = percent;
    job.poll_count += 1;
    job.updated_at = Utc::now();

    if status.is_terminal() {
        job.completed_at = Some(Utc::now());
        let backend = IsabelleBackend::mock();
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
                std::fs::write(sub.join("Solution.thy"), code)?;
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
        .unwrap_or_else(|| config.resources.join("isabelle"));
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
            Some(mock_isabelle_solution(&job.task)),
            Some("mock: proved".into()),
        ),
    }
}

fn mock_isabelle_solution(task: &ProofTask) -> String {
    let name = task
        .theorem
        .full_name
        .rsplit('.')
        .next()
        .unwrap_or("MainTheorem");
    format!(
        "theory Solution\n  imports Main\nbegin\n\n\
         (* Mock Isabelle proof. *)\ntheorem {name}: \"True\"\n  by simp\n\nend\n"
    )
}

/// Isabelle [`FormalBackend`]. In mock mode the compile / oracle-audit / kernel
/// re-check layers return canned success; the source scan always runs for real.
/// In live mode the theory is scaffolded with a session `ROOT` and checked with
/// a clean `isabelle build -o quick_and_dirty=false` through the configured
/// [`Runner`] — Isabelle is LCF/kernel-checked, so a clean build IS the kernel
/// re-check; the oracle gate is that clean build combined with the source scan
/// (which catches `sorry`/`oops`/`oracle`).
pub struct IsabelleBackend {
    pub mock: bool,
    pub runner: Runner,
    pub isabelle: String,
}

impl IsabelleBackend {
    /// The offline mock backend (canned layers; real source scan).
    pub fn mock() -> Self {
        Self {
            mock: true,
            runner: Runner::Native,
            isabelle: "isabelle".into(),
        }
    }

    /// The live backend, reading the configured runner + binary (env-overridable).
    pub fn live(cfg: &Config) -> Self {
        Self {
            mock: false,
            runner: cfg.formal_runners.for_system(SYSTEM),
            isabelle: exec::env_or("THEOREMATA_ISABELLE", &cfg.isabelle_bin),
        }
    }

    /// A clean `isabelle build` of the scaffolded session (shared by `compile`
    /// and `kernel_recheck` — the build both elaborates and kernel-checks).
    fn build(&self, ws: &Workspace) -> exec::ExecOutcome {
        exec::run(
            &self.runner,
            &[
                &self.isabelle,
                "build",
                "-o",
                "quick_and_dirty=false",
                "-D",
                ".",
            ],
            &ws.root,
        )
    }
}

impl FormalBackend for IsabelleBackend {
    fn system(&self) -> FormalSystem {
        SYSTEM
    }

    fn compile_success_signal(&self) -> crate::prover::formal::SuccessSignal {
        // Isabelle's batch build sets a correct non-zero exit code on failure.
        crate::prover::formal::SuccessSignal::NonZeroExitIsHonest
    }

    fn is_mock(&self) -> bool {
        self.mock
    }

    fn available(&self) -> bool {
        self.mock || exec::probe(&self.runner, &[&self.isabelle, "version"])
    }

    fn scaffold(&self, cfg: &Config, code: &str, name: &str) -> Result<Workspace> {
        if self.mock {
            return Ok(Workspace {
                system: SYSTEM,
                root: PathBuf::from("."),
                source_path: PathBuf::from(format!("{name}{}", SYSTEM.source_extension())),
                entry: name.to_string(),
            });
        }
        // Determine the theory name; wrap a bare proof body in a Main theory.
        let (thy_name, thy_body) = match theory_name(code) {
            Some(n) => (n, code.to_string()),
            None => (
                "Scratch".to_string(),
                format!("theory Scratch\n  imports Main\nbegin\n\n{code}\n\nend\n"),
            ),
        };
        let root = crate::prover::formal::live_workspace_dir(cfg, SYSTEM)?;
        std::fs::write(root.join(format!("{thy_name}.thy")), &thy_body)?;
        // A minimal session ROOT so `isabelle build -D .` has a unit to check.
        let root_file = format!("session {thy_name}_session = HOL +\n  theories\n    {thy_name}\n");
        std::fs::write(root.join("ROOT"), root_file)?;
        Ok(Workspace {
            system: SYSTEM,
            root,
            source_path: PathBuf::from(format!("{thy_name}.thy")),
            entry: crate::prover::formal::entry_name(SYSTEM, code)
                .unwrap_or_else(|| name.to_string()),
        })
    }

    fn compile(&self, ws: &Workspace) -> Result<CompileReport> {
        if self.mock {
            return Ok(CompileReport {
                compiled: true,
                errors: Vec::new(),
                per_unit: Vec::new(),
                detail: json!({"mock": true}),
            });
        }
        if !self.available() {
            return Ok(CompileReport {
                compiled: false,
                errors: vec!["isabelle toolchain unavailable".into()],
                per_unit: Vec::new(),
                detail: json!({"unavailable": true, "runner": self.runner.tag()}),
            });
        }
        let out = self.build(ws);
        let errors = if out.success() {
            Vec::new()
        } else {
            vec![out.stderr.clone()]
        };
        let code = std::fs::read_to_string(ws.root.join(&ws.source_path)).unwrap_or_default();
        let per_unit =
            crate::prover::formal::per_declaration_status(SYSTEM, &code, out.success(), &errors);
        Ok(CompileReport {
            compiled: self.compile_success_signal().is_pass(
                out.launched,
                out.success(),
                &out.stdout,
                &out.stderr,
            ),
            errors,
            per_unit,
            detail: json!({
                "runner": self.runner.tag(),
                "code": out.code,
                "stdout": out.stdout,
                "stderr": out.stderr,
            }),
        })
    }

    fn audit_axioms(
        &self,
        _ws: &Workspace,
        _thm: &str,
        whitelist: &[String],
    ) -> Result<AxiomReport> {
        if self.mock {
            return Ok(AxiomReport {
                axioms: Vec::new(),
                within_whitelist: true,
                detail: json!({"mock": true, "whitelist": whitelist, "oracles": []}),
            });
        }
        // Per the Isabelle reference (docs/formal-systems/isabelle.md §3): the
        // oracle gate is a clean `isabelle build` (no `sorry`/oracle, done in
        // `compile`) combined with the mandatory source scan. There is no cheap
        // per-theorem oracle-list command over the batch build, so this layer
        // defers to those two; it is non-blocking here.
        Ok(AxiomReport {
            axioms: Vec::new(),
            within_whitelist: true,
            detail: json!({
                "runner": self.runner.tag(),
                "note": "oracle gate = clean build (compile) + source scan; no oracle in whitelist",
                "whitelist": whitelist,
            }),
        })
    }

    fn kernel_recheck(&self, ws: &Workspace) -> Result<RecheckReport> {
        if self.mock {
            return Ok(RecheckReport {
                rechecked: true,
                detail: json!({"mock": true}),
            });
        }
        if !self.available() {
            return Ok(RecheckReport {
                rechecked: false,
                detail: json!({"unavailable": true, "runner": self.runner.tag()}),
            });
        }
        // A fresh clean build re-runs every primitive inference through the LCF
        // kernel — the independent re-check analogue.
        let out = self.build(ws);
        Ok(RecheckReport {
            rechecked: out.success(),
            detail: json!({
                "runner": self.runner.tag(),
                "code": out.code,
                "note": "kernel-checked by isabelle build",
            }),
        })
    }

    fn source_scan(&self, code: &str) -> Result<ScanReport> {
        // Prefer the shared Python `source_scan` worker (comment/cartouche-aware);
        // fall back to a built-in lexical pass so the gate still bites offline.
        if let Some(report) = crate::prover::formal::worker_source_scan(SYSTEM, code) {
            return Ok(report);
        }
        // Isabelle escape hatches NOT caught by thm_oracles / clean build.
        let low = code.to_lowercase();
        let patterns = ["sorry", "oops", "quick_and_dirty", "skip_proof", "oracle"];
        let findings: Vec<String> = patterns
            .iter()
            .filter(|p| low.contains(**p))
            .map(|p| (*p).to_string())
            .collect();
        Ok(ScanReport {
            clean: findings.is_empty(),
            findings,
            detail: json!({"system": SYSTEM.as_str(), "fallback": true}),
        })
    }
}

/// Extract the `theory <Name>` declared in a full `.thy`, or `None` for a bare
/// proof body that must be wrapped.
fn theory_name(code: &str) -> Option<String> {
    for line in code.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("theory") {
            if rest.starts_with(|c: char| c.is_whitespace()) {
                let name: String = rest
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '\''))
                    .collect();
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }
    }
    None
}

/// Isabelle warm-driver session (Isabelle Server in Phase 3). Theory-file
/// granular: `submit_unit` is supported; `step_tactic` returns
/// [`SessionError::Unsupported`].
impl ProofSession for IsabelleBackend {
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

    fn step_tactic(&mut self, _state: u64, _tactic: &str) -> Result<StateResult> {
        // Isabelle is driven at theory granularity; no per-tactic stepping.
        Err(SessionError::Unsupported(
            "Isabelle is theory-file granular; use submit_unit instead of step_tactic",
        )
        .into())
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
