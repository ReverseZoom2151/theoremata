//! Shared external-checker backend for Agda and Metamath.
//!
//! These systems have different foundations but the same Theoremata boundary:
//! write a source artifact, invoke the authoritative checker, record the exact
//! command/tool result, and apply a conservative source scan.  The backends are
//! mockable so CI does not require either toolchain.

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
use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use std::path::PathBuf;

pub struct ExternalBackend {
    pub system: FormalSystem,
    pub mock: bool,
    pub runner: Runner,
    pub binary: String,
    pub secondary_binary: Option<String>,
}

fn backend_name(system: FormalSystem) -> &'static str {
    system.as_str()
}

pub fn mock_enabled(config: &Config, system: FormalSystem) -> bool {
    config.prover_mock || match system {
        FormalSystem::Agda => std::env::var("THEOREMATA_AGDA_COMMAND").is_err(),
        FormalSystem::Metamath => std::env::var("THEOREMATA_METAMATH_COMMAND").is_err(),
        _ => false,
    }
}

pub fn build_task(
    project_id: Option<String>,
    node_id: Option<String>,
    statement: &str,
    theorem_name: &str,
    config: &Config,
    system: FormalSystem,
) -> ProofTask {
    ProofTask {
        id: uuid::Uuid::new_v4().to_string(), project_id, node_id,
        theorem: crate::prover::model::TheoremIdentity {
            repo: Some("theoremata".into()), commit: None, file: None,
            full_name: theorem_name.into(), line: None,
        },
        system,
        formal_project: FormalProject {
            system, root: config.resources.clone(), toolchain: None,
            imports: system.default_imports(), metadata: json!({}),
        },
        statement: statement.into(), stub: None, prompt: None,
        backend: backend_name(system).into(), metadata: json!({}),
    }
}

pub fn submit(store: &Store, config: &Config, task: ProofTask,
              artifacts_dir: Option<std::path::PathBuf>) -> Result<ProofJob> {
    let mock = mock_enabled(config, task.system);
    let external_id = mock.then(|| format!("mock-{}", &task.id[..8.min(task.id.len())]));
    let job = store.create_proof_job(
        &task,
        backend_name(task.system),
        ProverJobStatus::Submitted,
        external_id.as_deref(),
        artifacts_dir.as_deref(),
        0.0,
    )?;
    store.event(
        task.project_id.as_deref(),
        None,
        "proof_job.submitted",
        backend_name(task.system),
        json!({"job_id": job.id, "task_id": task.id, "mock": mock}),
    )?;
    Ok(job)
}

pub fn poll(
    store: &Store,
    config: &Config,
    job_id: &str,
    system: FormalSystem,
) -> Result<ProofJob> {
    let mut job = store
        .get_proof_job(job_id)?
        .ok_or_else(|| anyhow::anyhow!("unknown proof job {job_id}"))?;
    if job.status.is_terminal() {
        return Ok(job);
    }
    if !mock_enabled(config, system) {
        return crate::prover::formal::live_poll(store, config, job, backend_name(system), system);
    }
    job.poll_count += 1;
    job.updated_at = Utc::now();
    if job.poll_count == 1 {
        job.status = ProverJobStatus::InProgress;
        job.percent_complete = 50.0;
        store.update_proof_job(&job)?;
        store.event(
            job.project_id.as_deref(),
            None,
            "proof_job.progress",
            backend_name(system),
            json!({"job_id": job.id, "status": job.status, "percent_complete": job.percent_complete}),
        )?;
        return Ok(job);
    }
    let code = job.task.stub.clone().unwrap_or_else(|| match system {
        FormalSystem::Agda => "module Generated where\n\nopen import Agda.Builtin.Unit\ngenerated : Agda.Builtin.Unit.\u{22a4}\ngenerated = Agda.Builtin.Unit.tt\n".into(),
        FormalSystem::Metamath => "$c wff |- $.\n$v ph $.\nph $f wff ph $.\n".into(),
        _ => String::new(),
    });
    let backend = ExternalBackend::new(config, system, true);
    let verification = backend.verify(config, &code, &job.task.statement).ok();
    job.status = if verification
        .as_ref()
        .map(|report| report.lexically_verified)
        .unwrap_or(false)
    {
        ProverJobStatus::Proved
    } else {
        ProverJobStatus::Failed
    };
    job.percent_complete = 100.0;
    job.completed_at = Some(Utc::now());
    job.result = Some(ProofResult {
        task_id: job.task.id.clone(), job_id: job.id.clone(), status: job.status,
        formal_code: Some(code.clone()), counterexample: None, verification,
        artifacts_dir: job.artifacts_dir.clone(), duration_ms: 0, cost: None,
        message: Some(format!("mock {system} checker completed")),
        provenance: json!({"backend": backend_name(system), "system": system.as_str(), "mock": true}),
    });
    if let Some(dir) = &job.artifacts_dir {
        let sub = dir.join(backend_name(system));
        std::fs::create_dir_all(&sub)?;
        std::fs::write(sub.join(format!("solution{}", system.source_extension())), &code)?;
        std::fs::write(dir.join("result.json"), serde_json::to_string_pretty(job.result.as_ref().unwrap())?)?;
    }
    store.update_proof_job(&job)?;
    store.event(
        job.project_id.as_deref(),
        None,
        "proof_job.completed",
        backend_name(system),
        json!({"job_id": job.id, "status": job.status, "mock": true}),
    )?;
    Ok(job)
}

pub fn cancel(store: &Store, job_id: &str) -> Result<ProofJob> {
    let mut job = store.get_proof_job(job_id)?.ok_or_else(|| anyhow::anyhow!("unknown proof job {job_id}"))?;
    if !job.status.is_terminal() {
        job.status = ProverJobStatus::Cancelled;
        job.completed_at = Some(Utc::now());
        job.updated_at = Utc::now();
        store.update_proof_job(&job)?;
        store.event(
            job.project_id.as_deref(),
            None,
            "proof_job.cancelled",
            &job.backend,
            json!({"job_id": job.id}),
        )?;
    }
    Ok(job)
}

impl ExternalBackend {
    pub fn new(cfg: &Config, system: FormalSystem, mock: bool) -> Self {
        let (binary, env_name) = match system {
            FormalSystem::Agda => (&cfg.agda_bin, "THEOREMATA_AGDA"),
            FormalSystem::Metamath => (&cfg.metamath_bin, "THEOREMATA_METAMATH"),
            _ => unreachable!("ExternalBackend only supports Agda and Metamath"),
        };
        Self {
            system,
            mock,
            runner: if mock {
                Runner::Native
            } else {
                cfg.formal_runners.for_system(system)
            },
            binary: exec::env_or(env_name, binary),
            secondary_binary: if system == FormalSystem::Metamath {
                std::env::var("THEOREMATA_METAMATH_SECONDARY").ok()
            } else {
                None
            },
        }
    }

    fn command(&self, filename: &str) -> Vec<String> {
        match self.system {
            // Safe mode disables postulates and unsafe options at the checker
            // boundary; source scanning still reports which policy was used.
            FormalSystem::Agda => vec![self.binary.clone(), "--safe".into(), filename.to_string()],
            // Metamath's command-line mode accepts each command as one argv
            // item, avoiding an interactive process that could otherwise hang.
            FormalSystem::Metamath => vec![
                self.binary.clone(),
                format!("read {filename}"),
                "verify proof *".into(),
                "exit".into(),
            ],
            _ => unreachable!(),
        }
    }

    fn run_file(&self, ws: &Workspace) -> exec::ExecOutcome {
        let filename = ws
            .source_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("Generated");
        let args = self.command(filename);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        exec::run(&self.runner, &refs, &ws.root)
    }
}

impl FormalBackend for ExternalBackend {
    fn system(&self) -> FormalSystem {
        self.system
    }

    fn compile_success_signal(&self) -> crate::prover::formal::SuccessSignal {
        match self.system {
            // The `metamath` reference binary returns exit code 0 even when
            // `verify proof *` FAILS (failures only print `?Error` to stdout;
            // its `main()` unconditionally returns 0). Only stdout distinguishes
            // a pass, so require the success sentinel and forbid error/warning
            // markers.
            FormalSystem::Metamath => crate::prover::formal::SuccessSignal::StdoutSentinel {
                must_contain: &["All proofs in the database were verified"],
                must_not_contain: &["?Error", "?Warning", "were not proved", "no source file"],
            },
            // Agda under `--safe` sets a correct non-zero exit on failure.
            _ => crate::prover::formal::SuccessSignal::NonZeroExitIsHonest,
        }
    }

    fn is_mock(&self) -> bool {
        self.mock
    }

    fn available(&self) -> bool {
        if self.mock {
            return true;
        }
        let probe = match self.system {
            FormalSystem::Agda => vec![self.binary.as_str(), "--version"],
            FormalSystem::Metamath => vec![self.binary.as_str(), "-h"],
            _ => unreachable!(),
        };
        exec::probe(&self.runner, &probe)
    }

    fn scaffold(&self, cfg: &Config, code: &str, name: &str) -> Result<Workspace> {
        if self.mock {
            return Ok(Workspace {
                system: self.system,
                root: PathBuf::from("."),
                source_path: PathBuf::from(format!("{name}{}", self.system.source_extension())),
                entry: name.into(),
            });
        }
        let root = crate::prover::formal::live_workspace_dir(cfg, self.system)?;
        let source_path = root.join(format!("Generated{}", self.system.source_extension()));
        std::fs::write(&source_path, code)?;
        if self.system == FormalSystem::Metamath {
            // Metamath resolves `$[ file $]` relative to the current working
            // directory. Copy explicitly referenced resources into the
            // isolated workspace so a proof cannot silently depend on an
            // undeclared host-global database.
            for include in metamath_includes(code) {
                let destination = root.join(&include);
                if destination.exists() {
                    continue;
                }
                let source = cfg.resources.join(&include);
                if !source.is_file() {
                    anyhow::bail!(
                        "Metamath dependency `{}` is not present in the configured resources",
                        include.display()
                    );
                }
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(source, destination)?;
            }
        }
        Ok(Workspace {
            system: self.system,
            root,
            source_path,
            entry: name.into(),
        })
    }

    fn compile(&self, ws: &Workspace) -> Result<CompileReport> {
        if self.mock {
            return Ok(CompileReport {
                compiled: true,
                errors: vec![],
                per_unit: vec![],
                detail: json!({"mock": true}),
            });
        }
        if !self.available() {
            return Ok(CompileReport {
                compiled: false,
                errors: vec![format!("{} toolchain unavailable", self.system)],
                per_unit: vec![],
                detail: json!({"unavailable": true}),
            });
        }
        let out = self.run_file(ws);
        let filename = ws.source_path.file_name().and_then(|s| s.to_str()).unwrap_or("Generated");
        let command = self.command(filename);
        // SOUNDNESS: the Metamath reference binary (`metamath`) returns exit code 0
        // even when `verify proof *` FAILS (failures only print `?Error` to stdout;
        // its `main()` unconditionally returns 0). So the exit code is NOT a
        // reliable pass signal for Metamath, and trusting it (`out.success()`) would
        // mark a failed proof as verified. The per-backend `compile_success_signal`
        // declares the correct positive signal (sentinel for Metamath, honest exit
        // for Agda), so exit status is never trusted on its own (fail-closed).
        let verified = self.compile_success_signal().is_pass(
            out.launched,
            out.success(),
            &out.stdout,
            &out.stderr,
        );
        let errors = if verified {
            vec![]
        } else {
            vec![out.stderr.clone(), out.stdout.clone()]
        };
        let code = std::fs::read_to_string(&ws.source_path).unwrap_or_default();
        Ok(CompileReport {
            compiled: verified,
            per_unit: crate::prover::formal::per_declaration_status(
                self.system,
                &code,
                verified,
                &errors,
            ),
            errors,
            detail: json!({"runner": self.runner.tag(), "binary": self.binary.clone(), "command": command, "code": out.code, "verified": verified, "stdout": out.stdout, "stderr": out.stderr}),
        })
    }

    fn audit_axioms(
        &self,
        _ws: &Workspace,
        _thm: &str,
        whitelist: &[String],
    ) -> Result<AxiomReport> {
        // Agda's trusted boundary is the type checker plus the source policy;
        // Metamath's `$a` declarations belong to the loaded database context.
        Ok(AxiomReport {
            axioms: vec![],
            within_whitelist: true,
            detail: json!({"system": self.system.as_str(), "whitelist": whitelist}),
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
                detail: json!({"unavailable": true}),
            });
        }
        let out = self.run_file(ws);
        let filename = ws.source_path.file_name().and_then(|s| s.to_str()).unwrap_or("Generated");
        let command = self.command(filename);
        let mut secondary = json!(null);
        let mut rechecked = out.success();
        if rechecked {
            if let (FormalSystem::Metamath, Some(binary)) = (self.system, &self.secondary_binary) {
                let filename = ws.source_path.file_name().and_then(|s| s.to_str()).unwrap_or("Generated");
                let args = [binary.as_str(), filename];
                let second = exec::run(&self.runner, &args, &ws.root);
                rechecked = second.success();
                secondary = json!({
                    "binary": binary,
                    "command": args,
                    "code": second.code,
                    "stdout": second.stdout,
                    "stderr": second.stderr,
                    "passed": second.success(),
                });
            }
        }
        Ok(RecheckReport {
            rechecked,
            detail: json!({"binary": self.binary.clone(), "command": command, "code": out.code, "stdout": out.stdout, "stderr": out.stderr, "checker": self.system.as_str(), "secondary": secondary}),
        })
    }

    fn source_scan(&self, code: &str) -> Result<ScanReport> {
        if let Some(report) = crate::prover::formal::worker_source_scan(self.system, code) {
            return Ok(report);
        }
        let mut findings = Vec::new();
        if self.system == FormalSystem::Agda {
            for (needle, reason) in [
                ("postulate", "Agda postulates widen the trusted base"),
                ("--allow-unsolved-metas", "unsolved metas are not proofs"),
                (
                    "{-# COMPILED",
                    "foreign compilation pragma bypasses Agda semantics",
                ),
            ] {
                if code.contains(needle) {
                    findings.push(format!("{needle}: {reason}"));
                }
            }
        } else if self.system == FormalSystem::Metamath {
            findings.extend(metamath_source_findings(code));
        }
        Ok(ScanReport {
            clean: findings.is_empty(),
            findings,
            detail: json!({"system": self.system.as_str(), "fallback": true}),
        })
    }
}

/// Conservative lexical scan for the Metamath trust holes the kernel check
/// cannot see. Over-flagging is safe here: it only makes the gate stricter.
///
/// A generated Metamath proof is trusted only when it merely *discharges* goals
/// against the loaded (reviewed) `set.mm` database via the kernel's proof check.
/// The findings below each mark a construct that bypasses or widens that base:
///   * a bare `$a` axiomatic assertion introduced by the generated proof widens
///     the trusted base (a new axiom, not a reuse of the loaded database);
///   * a `?` placeholder step is an unproven proof — the Metamath analogue of
///     Lean's `sorry`/Coq's `admit` (an `$p ... $= ? $.` incomplete proof);
///   * a `$[ file $]` include that escapes the workspace (absolute path, a `..`
///     parent-dir component, or a drive/root prefix) points at an untrusted,
///     out-of-tree database (path traversal).
fn metamath_source_findings(code: &str) -> Vec<String> {
    use std::path::Component;
    let mut findings = Vec::new();
    // `$a` is a keyword token; any occurrence in the generated source introduces
    // an axiom rather than reusing the loaded database.
    if code.contains("$a") {
        findings.push(
            "$a: generated proof introduces an axiomatic assertion, widening the trusted base"
                .to_string(),
        );
    }
    // In Metamath, `?` is exclusively the incomplete-proof marker, so any bare
    // `?` token disqualifies the proof.
    if code.split(|c: char| c.is_whitespace()).any(|tok| tok == "?") {
        findings.push(
            "?: incomplete `$p ... $= ? $.` proof contains an unproven placeholder step"
                .to_string(),
        );
    }
    // An include that leaves the workspace/set.mm tree is untrusted.
    for include in metamath_includes(code) {
        let escapes = include.is_absolute()
            || include.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            });
        if escapes {
            findings.push(format!(
                "$[ {} $]: include points outside the workspace/set.mm (path traversal / untrusted include)",
                include.display()
            ));
        }
    }
    findings
}

fn metamath_includes(code: &str) -> Vec<PathBuf> {
    let mut includes = Vec::new();
    let mut rest = code;
    while let Some(start) = rest.find("$[") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find("$]") else { break };
        let name = rest[..end].trim();
        if !name.is_empty() {
            includes.push(PathBuf::from(name));
        }
        rest = &rest[end + 2..];
    }
    includes
}

impl ProofSession for ExternalBackend {
    fn start(&mut self, _project: &FormalProject) -> Result<()> {
        Ok(())
    }
    fn submit_unit(&mut self, code: &str) -> Result<UnitResult> {
        let scan = self.source_scan(code)?;
        Ok(UnitResult {
            ok: scan.clean,
            messages: scan.findings,
            detail: json!({"system": self.system.as_str(), "mock": self.mock}),
        })
    }
    fn step_tactic(&mut self, _state: u64, _tactic: &str) -> Result<StateResult> {
        Err(
            SessionError::Unsupported("Agda and Metamath integrations are whole-file checkers")
                .into(),
        )
    }
    fn goal_state(&self, _state: u64) -> Result<GoalState> {
        Ok(GoalState {
            goals: vec![],
            detail: json!({"system": self.system.as_str()}),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Metamath exit-code soundness: a failed `verify proof *` must NOT read as
    // verified even though the binary exits 0. Only the explicit success sentinel
    // (with no error/warning markers) counts.
    #[test]
    fn metamath_verified_requires_the_success_sentinel() {
        // Exercise the real production signal for the Metamath backend, which
        // must NOT read a failed run as verified even though the binary exits 0.
        let sig = ExternalBackend::new(&Config::default(), FormalSystem::Metamath, true)
            .compile_success_signal();
        // Genuine pass (binary exited 0 with the sentinel).
        assert!(sig.is_pass(
            true,
            true,
            "All proofs in the database were verified in 0.01 s.",
            ""
        ));
        // Failure that (like the real binary) still exited 0 -> must be rejected.
        assert!(!sig.is_pass(true, true, "?Error on line 5: ... proof does not verify.", ""));
        // Silent / empty output (missing file, no sentinel) -> rejected.
        assert!(!sig.is_pass(true, true, "", ""));
        assert!(!sig.is_pass(true, true, "No source file was read in.", ""));
        // Warnings (e.g. an incomplete `? ` proof) -> rejected (fail-closed).
        assert!(!sig.is_pass(
            true,
            true,
            "?Warning: proof is incomplete.\nAll proofs in the database were verified.",
            ""
        ));
        // Never launched -> rejected regardless of a clean-looking exit.
        assert!(!sig.is_pass(false, false, "All proofs in the database were verified.", ""));
    }

    // GAP 1 — Metamath source scan. These exercise the pure lexical helper
    // directly, so they are deterministic regardless of whether the Python
    // `source_scan` worker is present.

    #[test]
    fn metamath_placeholder_proof_is_flagged() {
        // A `?` step = an unproven proof (the Metamath `sorry`).
        let findings = metamath_source_findings("$[ set.mm $]\nfoo $p wff ph $= ? $.\n");
        assert!(
            findings.iter().any(|f| f.starts_with("?:")),
            "a `?` placeholder step must be flagged: {findings:?}"
        );
    }

    #[test]
    fn metamath_generated_axiom_is_flagged() {
        // A generated `$a` widens the trusted base beyond the loaded database.
        let findings = metamath_source_findings("badax $a |- ph $.\n");
        assert!(
            findings.iter().any(|f| f.starts_with("$a:")),
            "a generated `$a` axiom must be flagged: {findings:?}"
        );
    }

    #[test]
    fn metamath_outside_include_is_flagged() {
        // `..` parent-dir traversal and absolute/root includes both escape.
        for src in ["$[ ../evil.mm $]\n", "$[ /etc/passwd $]\n"] {
            let findings = metamath_source_findings(src);
            assert!(
                findings.iter().any(|f| f.contains("path traversal")),
                "an out-of-workspace include must be flagged: {src:?} -> {findings:?}"
            );
        }
    }

    #[test]
    fn metamath_clean_proof_passes() {
        // Reuses the loaded database via a normal relative include and a complete
        // `$= ... $.` proof with no placeholder and no new axiom.
        let findings = metamath_source_findings("$[ set.mm $]\nmp2 $p |- ph $= wph wps mp1 mp3 $.\n");
        assert!(findings.is_empty(), "clean proof must not flag: {findings:?}");
    }

    #[test]
    fn metamath_source_scan_flags_and_passes_via_backend() {
        // Same behavior through the backend's `source_scan` entry point (mock).
        let cfg = crate::config::Config::default();
        let backend = ExternalBackend::new(&cfg, FormalSystem::Metamath, true);
        // The built-in fallback only runs when the Python worker is absent; guard
        // the assertions on that so the test stays deterministic in either env.
        if crate::prover::formal::worker_source_scan(FormalSystem::Metamath, "$a |- ph $.\n")
            .is_none()
        {
            assert!(!backend.source_scan("bad $a |- ph $.\n").unwrap().clean);
            assert!(backend
                .source_scan("$[ set.mm $]\nt $p |- ph $= a b c $.\n")
                .unwrap()
                .clean);
        }
    }

    // GAP 2 — a MOCK (toolchain-absent) check must NEVER be a LIVE certification
    // (audit invariant #2). Downstream only grants `FormallyVerified` when both
    // `report.lexically_verified && report.live` hold (agent.rs), so the mock
    // report carrying `live == false` is what keeps mock proofs out of it.

    #[test]
    fn mock_async_verification_is_not_live() {
        let cfg = crate::config::Config::default();
        let backend = ExternalBackend::new(&cfg, FormalSystem::Metamath, true);
        let report = backend
            .verify(&cfg, "$c wff |- $.\n$v ph $.\nph $f wff ph $.\n", "some statement")
            .expect("mock verify should not error");
        assert!(
            !report.live,
            "a mock verification must never be a live certification"
        );
    }
}
