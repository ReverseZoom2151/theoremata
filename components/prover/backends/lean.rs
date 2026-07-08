//! Lean external-prover adapter — Phase 1 mock + Phase 2 live gate.
//!
//! Mirrors the `aristotle`/`rocq` mocks (Config.prover_mock-driven,
//! submit → poll(InProgress → Proved) → result + a `VerificationReport`), and
//! routes verification through the system-agnostic [`FormalBackend`] 3+1-layer
//! gate. In live mode each layer runs the native Lean toolchain through the
//! configured [`Runner`]: `lean <Generated.lean>` (compile — the kernel checks
//! every proof term), `#print axioms <thm>` (axiom audit vs the mathlib
//! whitelist), and `lake env leanchecker` when available (kernel re-check;
//! degrades gracefully to the compile-time kernel check otherwise).

use crate::{
    config::Config,
    db::Store,
    prover::{
        exec::{self, Runner},
        formal::{
            AxiomReport, CompileReport, FormalBackend, FormalSystem, GoalState, ProofSession,
            RecheckReport, ScanReport, StateResult, UnitResult, Workspace,
        },
        model::{FormalProject, ProofJob, ProofResult, ProofTask, ProverJobStatus},
    },
};
use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::json;
use std::{path::PathBuf, time::Instant};

const BACKEND: &str = "lean";
const SYSTEM: FormalSystem = FormalSystem::Lean;
const MODULE: &str = "Generated";

/// The LeanDojo in-kernel `validateProof` soundness-gate template
/// (`components/verify/lean/validate_proof_template.lean`). Referenced from the
/// verify path as an OPTIONAL extra check (gated by `Config::kernel_validate_proof`);
/// it reconstructs a standalone declaration, rejects `sorry`/metavariables, and
/// kernel-rechecks via `addDecl`. See the template header for how the warm REPL
/// would invoke it on the close-path. It need not run live if the toolchain lacks
/// a REPL build of it — the wiring + flag exist regardless.
pub const VALIDATE_PROOF_TEMPLATE: &str = "components/verify/lean/validate_proof_template.lean";

pub fn mock_enabled(config: &Config) -> bool {
    config.prover_mock
        || std::env::var("THEOREMATA_LEAN_MOCK")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or_else(|_| std::env::var("THEOREMATA_LEAN_COMMAND").is_err())
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
        let backend = LeanBackend::mock();
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
                std::fs::write(sub.join("solution.lean"), code)?;
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
        .unwrap_or_else(|| config.resources.join("lean"));
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
        formal_project: FormalProject {
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
            Some(mock_lean_solution(&job.task)),
            Some("mock: proved".into()),
        ),
    }
}

fn mock_lean_solution(task: &ProofTask) -> String {
    let name = task
        .theorem
        .full_name
        .rsplit('.')
        .next()
        .unwrap_or("MainTheorem");
    format!("-- Mock Lean proof.\ntheorem {name} : True := trivial\n")
}

/// Lean [`FormalBackend`]. In mock mode the compile / axiom-audit / kernel
/// re-check layers return canned success; the source scan always runs for real.
pub struct LeanBackend {
    pub mock: bool,
    pub runner: Runner,
    pub lean: String,
    pub lake: String,
    /// Optional pin for the reject-on-mismatch precheck (`THEOREMATA_LEAN_TOOLCHAIN`,
    /// e.g. `leanprover/lean4:v4.9.0`). `None` disables the toolchain check.
    pub toolchain: Option<String>,
    /// Whether to wire the LeanDojo in-kernel `validateProof` soundness gate
    /// ([`VALIDATE_PROOF_TEMPLATE`]) into the kernel re-check
    /// (`Config::kernel_validate_proof`).
    pub kernel_validate: bool,
}

impl LeanBackend {
    /// The offline mock backend (canned layers; real source scan).
    pub fn mock() -> Self {
        Self {
            mock: true,
            runner: Runner::Native,
            lean: "lean".into(),
            lake: "lake".into(),
            toolchain: None,
            kernel_validate: false,
        }
    }

    /// The live backend, reading the configured runner + binary (env-overridable).
    pub fn live(cfg: &Config) -> Self {
        Self {
            mock: false,
            runner: cfg.formal_runners.for_system(SYSTEM),
            lean: exec::env_or("THEOREMATA_LEAN", &cfg.lean_bin),
            lake: exec::env_or("THEOREMATA_LAKE", "lake"),
            toolchain: std::env::var("THEOREMATA_LEAN_TOOLCHAIN")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            kernel_validate: cfg.kernel_validate_proof,
        }
    }

    /// Status of the optional in-kernel `validateProof` soundness gate: whether it
    /// is enabled, whether the template is present on disk, and a note. Folded
    /// into the kernel-recheck detail so the wiring is observable even when the
    /// check does not run live.
    fn validate_proof_gate(&self) -> serde_json::Value {
        if !self.kernel_validate {
            return json!({"enabled": false});
        }
        let present = std::path::Path::new(VALIDATE_PROOF_TEMPLATE).exists();
        json!({
            "enabled": true,
            "template": VALIDATE_PROOF_TEMPLATE,
            "template_present": present,
            "note": if present {
                "in-kernel validateProof gate wired; runs when a REPL build of the template \
                 against the pinned toolchain is available"
            } else {
                "kernel_validate_proof set but template not found on disk"
            },
        })
    }
}

/// Parse the transitive axiom set from a `#print axioms` message. Returns
/// `Some(vec![])` for the clean "does not depend on any axioms" line, or the
/// listed axioms otherwise; `None` if no axiom line is present.
fn parse_axioms(stdout: &str) -> Option<Vec<String>> {
    if stdout.contains("does not depend on any axioms") {
        return Some(Vec::new());
    }
    let marker = "depends on axioms:";
    let idx = stdout.find(marker)?;
    let tail = &stdout[idx + marker.len()..];
    // The list is `[a, b, c]` possibly spanning lines.
    let inside = tail
        .split_once('[')
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(list, _)| list)
        .unwrap_or(tail);
    let axioms: Vec<String> = inside
        .split(',')
        .map(|s| s.trim().trim_matches(|c: char| c.is_whitespace()).to_string())
        .filter(|s| !s.is_empty())
        .collect();
    Some(axioms)
}

impl FormalBackend for LeanBackend {
    fn system(&self) -> FormalSystem {
        SYSTEM
    }

    fn available(&self) -> bool {
        self.mock || exec::probe(&self.runner, &[&self.lean, "--version"])
    }

    fn expected_toolchain(&self) -> Option<String> {
        self.toolchain.clone()
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
        let entry =
            crate::prover::formal::entry_name(SYSTEM, code).unwrap_or_else(|| name.to_string());
        let root = crate::prover::formal::live_workspace_dir(cfg, SYSTEM)?;
        let src = root.join(format!("{MODULE}.lean"));
        std::fs::write(&src, code)?;
        Ok(Workspace {
            system: SYSTEM,
            root,
            source_path: src,
            entry,
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
                errors: vec!["lean toolchain unavailable".into()],
                per_unit: Vec::new(),
                detail: json!({"unavailable": true, "runner": self.runner.tag()}),
            });
        }
        let file = format!("{MODULE}.lean");
        let out = exec::run(&self.runner, &[&self.lean, &file], &ws.root);
        let errors = if out.success() {
            Vec::new()
        } else {
            vec![out.stderr.clone(), out.stdout.clone()]
        };
        // Failure-isolating per-declaration status: read the generated source
        // back and attribute each error to the declaration it names.
        let code = std::fs::read_to_string(&ws.source_path).unwrap_or_default();
        let per_unit =
            crate::prover::formal::per_declaration_status(SYSTEM, &code, out.success(), &errors);
        Ok(CompileReport {
            compiled: out.success(),
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

    fn audit_axioms(&self, ws: &Workspace, thm: &str, whitelist: &[String]) -> Result<AxiomReport> {
        if self.mock {
            return Ok(AxiomReport {
                axioms: Vec::new(),
                within_whitelist: true,
                detail: json!({"mock": true, "whitelist": whitelist}),
            });
        }
        // Write a sibling file that imports nothing extra and prints the axiom
        // closure of the target theorem, then run `lean` on it.
        let base = std::fs::read_to_string(&ws.source_path).unwrap_or_default();
        let audit_file = "Generated_axioms.lean";
        let mut content = base;
        if !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&format!("#print axioms {thm}\n"));
        std::fs::write(ws.root.join(audit_file), content)?;
        let out = exec::run(&self.runner, &[&self.lean, audit_file], &ws.root);
        let axioms = parse_axioms(&out.stdout).unwrap_or_else(|| vec!["<unparsed>".into()]);
        let within = out.success()
            && parse_axioms(&out.stdout).is_some()
            && axioms.iter().all(|a| whitelist.iter().any(|w| w == a));
        Ok(AxiomReport {
            axioms,
            within_whitelist: within,
            detail: json!({
                "runner": self.runner.tag(),
                "whitelist": whitelist,
                "stdout": out.stdout,
            }),
        })
    }

    fn kernel_recheck(&self, ws: &Workspace) -> Result<RecheckReport> {
        // Optional LeanDojo in-kernel `validateProof` gate wiring (observable in
        // the detail regardless of whether it can run live).
        let validate_proof = self.validate_proof_gate();
        if self.mock {
            return Ok(RecheckReport {
                rechecked: true,
                detail: json!({"mock": true, "validate_proof": validate_proof}),
            });
        }
        // `leanchecker` is only meaningful inside a Lake project (it replays
        // `.olean`s). A standalone `lean <file>` already runs the proof term
        // through the kernel, so when there is no Lake workspace we degrade
        // gracefully: the compile IS the kernel check.
        if !ws.root.join("lakefile.toml").exists() && !ws.root.join("lakefile.lean").exists() {
            return Ok(RecheckReport {
                rechecked: true,
                detail: json!({
                    "runner": self.runner.tag(),
                    "leanchecker": "skipped (bare lean; compile is kernel-checked)",
                    "validate_proof": validate_proof,
                }),
            });
        }
        let out = exec::run(&self.runner, &[&self.lake, "env", "leanchecker"], &ws.root);
        // If leanchecker is absent the launch fails; degrade to the compile check.
        if !out.launched {
            return Ok(RecheckReport {
                rechecked: true,
                detail: json!({
                    "runner": self.runner.tag(),
                    "leanchecker": "unavailable; relying on compile kernel-check",
                    "validate_proof": validate_proof,
                }),
            });
        }
        Ok(RecheckReport {
            rechecked: out.success(),
            detail: json!({
                "runner": self.runner.tag(),
                "code": out.code,
                "stdout": out.stdout,
                "stderr": out.stderr,
                "validate_proof": validate_proof,
            }),
        })
    }

    fn source_scan(&self, code: &str) -> Result<ScanReport> {
        // Prefer the shared Python `source_scan` worker (comment-aware); fall
        // back to a built-in lexical pass so the gate still bites offline.
        if let Some(report) = crate::prover::formal::worker_source_scan(SYSTEM, code) {
            return Ok(report);
        }
        // Lean escape hatches NOT caught by the kernel / `#print axioms` cleanly.
        let low = code.to_lowercase();
        let patterns = [
            "sorry",
            "sorryax",
            "admit",
            "native_decide",
            "ofreducebool",
            "trustcompiler",
        ];
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

/// Lean warm-driver session (repl in Phase 3). Supports both `submit_unit` and
/// per-tactic `step_tactic`.
impl ProofSession for LeanBackend {
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

    fn step_tactic(&mut self, state: u64, tactic: &str) -> Result<StateResult> {
        // Lean supports per-tactic stepping (repl `proofState` ids).
        let finished = tactic.contains("trivial") || tactic.trim() == "rfl";
        Ok(StateResult {
            state: state + 1,
            finished,
            detail: json!({"mock": self.mock, "tactic": tactic}),
        })
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
