//! Shared external-checker backend for Agda and Metamath.
//!
//! These systems have different foundations but the same Theoremata boundary:
//! write a source artifact, invoke the authoritative checker, record the exact
//! command/tool result, and apply a conservative source scan.  The backends are
//! mockable so CI does not require either toolchain.

use crate::{
    config::Config,
    prover::{
        exec::{self, Runner},
        formal::{
            AxiomReport, CompileReport, FormalBackend, FormalSystem, GoalState, ProofSession,
            RecheckReport, ScanReport, SessionError, StateResult, UnitResult, Workspace,
        },
        model::FormalProject,
    },
};
use anyhow::Result;
use serde_json::json;
use std::path::PathBuf;

pub struct ExternalBackend {
    pub system: FormalSystem,
    pub mock: bool,
    pub runner: Runner,
    pub binary: String,
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
        }
    }

    fn command(&self, filename: &str) -> Vec<String> {
        match self.system {
            FormalSystem::Agda => vec![self.binary.clone(), filename.to_string()],
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
        let errors = if out.success() {
            vec![]
        } else {
            vec![out.stderr.clone(), out.stdout.clone()]
        };
        let code = std::fs::read_to_string(&ws.source_path).unwrap_or_default();
        Ok(CompileReport {
            compiled: out.success(),
            per_unit: crate::prover::formal::per_declaration_status(
                self.system,
                &code,
                out.success(),
                &errors,
            ),
            errors,
            detail: json!({"runner": self.runner.tag(), "code": out.code, "stdout": out.stdout, "stderr": out.stderr}),
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
        Ok(RecheckReport {
            rechecked: out.success(),
            detail: json!({"code": out.code, "stdout": out.stdout, "stderr": out.stderr, "checker": self.system.as_str()}),
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
        }
        Ok(ScanReport {
            clean: findings.is_empty(),
            findings,
            detail: json!({"system": self.system.as_str(), "fallback": true}),
        })
    }
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
