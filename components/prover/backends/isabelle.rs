//! Isabelle/HOL external-prover adapter: a mock backend plus a live one.
//!
//! The mock mirrors the `aristotle` mock EXACTLY (Config.prover_mock-driven,
//! submit → poll(InProgress → Proved) → result + a `VerificationReport`), but
//! emits SYSTEM-NATIVE Isabelle theory (`.thy`) proofs and routes verification
//! through the system-agnostic [`FormalBackend`] 3+1-layer gate. Isabelle is
//! theory-file granular (no per-tactic stepping); the driver's `step_tactic`
//! returns [`crate::prover::formal::SessionError::Unsupported`].
//!
//! The LIVE backend runs the gate for real: `isabelle build` supplies both the
//! compile and the kernel-recheck layers, and the oracle gate (layer 2a) runs a
//! generated `thm_oracles` audit session. The source scan runs for real in both
//! modes. Every live layer fails CLOSED: a layer that could not run reports a
//! failure, never a pass, because "we could not check" is not "we checked".

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
/// re-check. The oracle gate (layer 2a) is a real `thm_oracles` query over the
/// full transitive derivation, run in a generated audit session; it fails closed
/// whenever that query cannot be run or its output cannot be parsed, and the
/// source scan (`sorry`/`oops`/`oracle`) runs alongside it, not instead of it.
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

    fn audit_axioms(&self, ws: &Workspace, thm: &str, whitelist: &[String]) -> Result<AxiomReport> {
        if self.mock {
            // The mock's canned clean audit is deliberate offline scaffolding. It
            // is safe ONLY because `is_mock()` forces `VerificationReport.live`
            // to false in `formal.rs::verify`, so no caller can promote it. The
            // `mock: true` marker is repeated in the detail so a persisted report
            // is self-describing even when read out of context.
            return Ok(AxiomReport {
                axioms: Vec::new(),
                within_whitelist: true,
                detail: json!({
                    "mock": true,
                    "live": false,
                    "audit_ran": false,
                    "whitelist": whitelist,
                    "oracles": [],
                }),
            });
        }
        // LIVE oracle audit. Per docs/formal-systems/isabelle.md §3, `thm_oracles`
        // "covers the full graph of transitive dependencies", which is what makes
        // it the faithful analogue of Lean's `#print axioms`: `sorry` is the
        // `Pure.skip_proof` oracle and an unreplayed external ATP reconstruction
        // leaves its own oracle tag, and both taint the derivation transitively.
        // A clean build alone does NOT catch either, so this layer must run its
        // own query rather than defer to `compile`.
        // Both inputs are validated BEFORE the toolchain is probed: a query we
        // cannot even form is already blocked, and checking first keeps these
        // fail-closed paths reachable (and testable) without a live Isabelle.
        //
        // `thm` is interpolated into generated theory source, and it can reach
        // here as `formal::theorem_name_hint`'s output, which is an arbitrary
        // whitespace-delimited token from the STATEMENT. Rejecting anything that
        // is not a plain Isabelle fact name therefore closes a source-injection
        // hole as well: a crafted name containing a cartouche delimiter could
        // otherwise append ML to the audit theory and forge a clean oracle set.
        if !is_fact_name(thm) {
            return Ok(blocked_audit(
                &self.runner.tag(),
                whitelist,
                "the target fact name is not a plain Isabelle identifier; \
                 refusing to build a query from it",
                json!({"thm": thm}),
            ));
        }
        // The audit runs in a SEPARATE session that imports the candidate theory,
        // so the candidate source is never edited (editing it would invalidate
        // the source scan and the statement-preservation check, which both read
        // the submitted text).
        let target_theory = match ws.source_path.file_stem().and_then(|s| s.to_str()) {
            Some(name) if is_fact_name(name) => name.to_string(),
            _ => {
                return Ok(blocked_audit(
                    &self.runner.tag(),
                    whitelist,
                    "candidate theory name could not be derived from the workspace",
                    json!({"source_path": ws.source_path.to_string_lossy()}),
                ))
            }
        };
        if !self.available() {
            return Ok(blocked_audit(
                &self.runner.tag(),
                whitelist,
                "isabelle toolchain unavailable; the oracle set could not be determined",
                json!({"unavailable": true}),
            ));
        }
        let dir = ws.root.join(ORACLE_AUDIT_DIR);
        let scaffolded = std::fs::create_dir_all(&dir)
            .and_then(|_| {
                std::fs::write(
                    dir.join(format!("{ORACLE_AUDIT_THEORY}.thy")),
                    oracle_audit_theory(&target_theory, thm),
                )
            })
            .and_then(|_| std::fs::write(dir.join("ROOT"), oracle_audit_root(&target_theory)));
        if let Err(err) = scaffolded {
            return Ok(blocked_audit(
                &self.runner.tag(),
                whitelist,
                "the oracle-audit session could not be written to disk",
                json!({"io_error": err.to_string()}),
            ));
        }
        // No `-c` (clean): the parent session may legitimately come from the
        // build cache, and the audit theory itself is always new, so its
        // diagnostic always executes. `-v` is required because the `thm_oracles`
        // output is a theory message, not build-summary output.
        let out = exec::run(
            &self.runner,
            &[
                &self.isabelle,
                "build",
                "-v",
                "-o",
                "quick_and_dirty=false",
                "-d",
                ".",
                "-d",
                ORACLE_AUDIT_DIR,
                ORACLE_AUDIT_SESSION,
            ],
            &ws.root,
        );
        // Isabelle splits messages across both streams depending on version and
        // runner, so parse the union; a marker on either stream is the same fact.
        let combined = format!("{}\n{}", out.stdout, out.stderr);
        let parsed = if out.success() {
            parse_oracles(&combined)
        } else {
            // A failed build tells us nothing about the oracle set.
            None
        };
        let (oracles, within) = oracle_verdict(parsed.clone(), whitelist);
        // Every fail-closed report carries its own reason, so a persisted report
        // is diagnosable without re-deriving it from the raw logs.
        let blocked = if parsed.is_some() {
            None
        } else if out.success() {
            Some("the audit session built but its thm_oracles output could not be parsed")
        } else {
            Some("the oracle-audit session failed to build; the oracle set is unknown")
        };
        Ok(AxiomReport {
            axioms: oracles.clone(),
            within_whitelist: within,
            detail: json!({
                "runner": self.runner.tag(),
                "live": true,
                "blocked": blocked,
                // The load-bearing distinction: `audit_ran: false` means the
                // oracle set is UNKNOWN, which is never a clean audit. An empty
                // `oracles` with `audit_ran: true` is the real "no oracles" fact.
                "audit_ran": parsed.is_some(),
                "oracles": oracles,
                "whitelist": whitelist,
                "query": "thm_oracles (full transitive oracle graph)",
                "code": out.code,
                "launched": out.launched,
                "timed_out": out.timed_out,
                "stdout": out.stdout,
                "stderr": out.stderr,
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
        Ok(fallback_source_scan(code))
    }
}

/// Offline lexical fallback for [`IsabelleBackend::source_scan`]: the Isabelle
/// escape hatches NOT caught by `thm_oracles` / a clean build.
///
/// Matched over COMMENT-STRIPPED source so this offline path agrees with the
/// online (worker) path and with
/// [`crate::prover::statement_preservation::ESCAPE_HATCH_COMMENT_POLICY`]
/// (`CodeOnly`): a `(* sorry *)` inside a comment is never seen by the kernel
/// and so is not a soundness violation. This LOOSENS the gate with respect to
/// commented text ONLY — a real `sorry` in code is untouched by stripping and
/// still fails here.
fn fallback_source_scan(code: &str) -> ScanReport {
    // The token list is the SHARED, ALIAS-EXPANDED table in `formal.rs`
    // ([`crate::prover::formal::escape_hatch_tokens`]), matched on word
    // boundaries. `oracle` used to catch `Thm.add_oracle` and `oracles` only
    // because it was a substring match; both are now listed explicitly, and
    // `axiomatization` / `axioms` (the same assert-by-fiat move under another
    // keyword) are listed with them.
    let findings = crate::prover::formal::escape_hatch_findings(SYSTEM, code);
    ScanReport {
        clean: findings.is_empty(),
        findings,
        detail: json!({"system": SYSTEM.as_str(), "fallback": true}),
    }
}

/// Sub-directory, theory and session names of the generated oracle-audit unit.
/// Kept out of the candidate's own session so the submitted source is never
/// rewritten by the audit.
const ORACLE_AUDIT_DIR: &str = "theoremata_oracle_audit";
const ORACLE_AUDIT_THEORY: &str = "Theoremata_Oracle_Audit";
const ORACLE_AUDIT_SESSION: &str = "Theoremata_Oracle_Audit_Session";

/// Delimiters printed around the `thm_oracles` output. They exist so that
/// "the query ran and reported nothing" is DISTINGUISHABLE from "the query never
/// ran": without a pair of markers there is no evidence the diagnostic executed,
/// and silence would otherwise be misread as an empty oracle set.
const ORACLE_BEGIN: &str = "THEOREMATA_ORACLES_BEGIN";
const ORACLE_END: &str = "THEOREMATA_ORACLES_END";

/// A fail-closed [`AxiomReport`]: the oracle set is UNKNOWN, so the audit is not
/// clean. `axioms` stays empty because we learned nothing, and `audit_ran` is
/// false so a reader can tell this apart from a genuine empty oracle set.
fn blocked_audit(
    runner: &str,
    whitelist: &[String],
    reason: &str,
    extra: serde_json::Value,
) -> AxiomReport {
    AxiomReport {
        axioms: Vec::new(),
        within_whitelist: false,
        detail: json!({
            "runner": runner,
            "live": true,
            "audit_ran": false,
            "blocked": reason,
            "whitelist": whitelist,
            "detail": extra,
        }),
    }
}

/// The generated audit theory. It imports the candidate theory and prints the
/// transitive oracle set of `thm` between the markers.
///
/// Only the DOCUMENTED Isar diagnostic `thm_oracles` is used, not the ML API
/// (`Thm_Deps.all_oracles` / `Thm.proof_of`), because the ML oracle tuple shape
/// has changed between Isabelle releases while the Isar command has not.
fn oracle_audit_theory(target_theory: &str, thm: &str) -> String {
    format!(
        "theory {ORACLE_AUDIT_THEORY}\n  imports {target_theory}\nbegin\n\n\
         ML \\<open>writeln \"{ORACLE_BEGIN}\"\\<close>\n\
         thm_oracles {thm}\n\
         ML \\<open>writeln \"{ORACLE_END}\"\\<close>\n\n\
         end\n"
    )
}

/// The session `ROOT` for the audit unit; its parent is the candidate session
/// written by [`IsabelleBackend::scaffold`].
fn oracle_audit_root(target_theory: &str) -> String {
    format!(
        "session {ORACLE_AUDIT_SESSION} = {target_theory}_session +\n  \
         theories\n    {ORACLE_AUDIT_THEORY}\n"
    )
}

/// Parse the oracle set out of an `isabelle build -v` log.
///
/// Returns `None` whenever the output is not evidence of a completed query
/// (missing or out-of-order markers) so the caller can fail closed, and
/// `Some(vec![])` for the genuinely oracle-free case.
fn parse_oracles(output: &str) -> Option<Vec<String>> {
    let lines: Vec<&str> = output.lines().collect();
    let begin = lines.iter().position(|l| l.contains(ORACLE_BEGIN))?;
    // Search for the end AFTER the begin so a stray earlier marker cannot make
    // an unfinished query look complete.
    let end = lines
        .iter()
        .skip(begin + 1)
        .position(|l| l.contains(ORACLE_END))
        .map(|i| i + begin + 1)?;
    let mut oracles = Vec::new();
    for line in &lines[begin + 1..end] {
        // `thm_oracles` prints an `oracles:` label followed by the names; the
        // label is dropped and the names are split on the usual separators.
        let body = line.trim().strip_prefix("oracles:").unwrap_or(line.trim());
        for token in body.split(|c: char| c.is_whitespace() || c == ',') {
            let token = token.trim_matches(|c: char| matches!(c, '"' | '\'' | ';' | '.'));
            if !token.is_empty() {
                oracles.push(token.to_string());
            }
        }
    }
    oracles.sort();
    oracles.dedup();
    Some(oracles)
}

/// Whether `name` is a plain (possibly qualified) Isabelle identifier, i.e. safe
/// to interpolate into generated theory source.
///
/// Deliberately strict: the alternative to rejecting an odd name is emitting it
/// into a theory file, and the only two outcomes there are a build failure or,
/// in the crafted case, attacker-chosen Isar/ML. Neither is worth accepting.
fn is_fact_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '\'' | '.'))
}

/// The whitelist decision, split out so the fail-closed rule is testable without
/// a toolchain: an unknown oracle set (`None`) is NEVER within the whitelist,
/// and a known empty set is.
fn oracle_verdict(parsed: Option<Vec<String>>, whitelist: &[String]) -> (Vec<String>, bool) {
    match parsed {
        None => (Vec::new(), false),
        Some(oracles) => {
            let within = oracles.iter().all(|o| whitelist.iter().any(|w| w == o));
            (oracles, within)
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The offline fallback must implement the SAME comment policy as the
    /// online scan: a commented escape hatch passes, a real one still fails.
    #[test]
    fn offline_fallback_matches_comment_policy() {
        assert!(
            !crate::prover::statement_preservation::commented_escape_hatch_is_a_violation(),
            "this test encodes ESCAPE_HATCH_COMMENT_POLICY == CodeOnly"
        );
        // Commented-out escape hatches: the kernel never sees them -> clean.
        let commented =
            "(* sorry *)\n(* avoid oops / quick_and_dirty here *)\nlemma t: \"True\" by simp\n";
        let report = fallback_source_scan(commented);
        assert!(
            report.clean,
            "commented escape hatch must not gate: {:?}",
            report.findings
        );
        // A REAL one in code still fails, offline as well as online.
        let real = fallback_source_scan("lemma t: \"True\"\n  sorry\n");
        assert!(!real.clean);
        assert!(real.findings.iter().any(|f| f == "sorry"));
        let real2 = fallback_source_scan("lemma t: \"True\"\n  oops\n");
        assert!(!real2.clean);
        assert!(real2.findings.iter().any(|f| f == "oops"));
    }

    /// ALIAS EXPANSION. `Thm.add_oracle` and `axiomatization` assert facts by
    /// fiat exactly as a bare `oracle` does. `add_oracle` and `oracles` used to
    /// be caught only incidentally, as SUBSTRINGS of `oracle`; word-boundary
    /// matching would drop them, so they are listed explicitly and asserted here.
    #[test]
    fn renamed_isabelle_hatches_are_caught() {
        for (code, expected) in [
            ("ML \\<open>Thm.add_oracle\\<close>\n", "add_oracle"),
            (
                "axiomatization bad where bad: \"False\"\n",
                "axiomatization",
            ),
            ("axioms bad: \"False\"\n", "axioms"),
            ("ML \\<open>Thm.oracles\\<close>\n", "oracles"),
        ] {
            let report = fallback_source_scan(code);
            assert!(!report.clean, "alias must be caught: {code:?}");
            assert!(
                report.findings.iter().any(|f| f == expected),
                "expected `{expected}` in {:?}",
                report.findings
            );
        }
    }

    // --- Layer 2a: the live oracle audit ---------------------------------
    //
    // The oracle set IS Isabelle's layer-2 gate: `sorry` is the
    // `Pure.skip_proof` oracle and an unreplayed external ATP reconstruction
    // leaves its own oracle tag, and a clean `isabelle build` catches neither.
    // These tests pin the fail-closed rule without a live Isabelle by driving
    // the pure parse/verdict helpers directly and by driving `audit_axioms`
    // through the paths that return before any process is spawned.

    /// A live backend whose binary cannot exist, so `available()` is false on
    /// every machine, including one that really does have Isabelle installed.
    fn unavailable_live() -> IsabelleBackend {
        IsabelleBackend {
            mock: false,
            runner: Runner::Native,
            isabelle: "theoremata-no-such-isabelle-binary".into(),
        }
    }

    fn workspace(source: &str) -> Workspace {
        Workspace {
            system: SYSTEM,
            root: PathBuf::from("."),
            source_path: PathBuf::from(source),
            entry: "t".into(),
        }
    }

    /// A synthetic `isabelle build -v` log with the audit markers around `body`.
    fn audit_log(body: &str) -> String {
        format!(
            "Running {ORACLE_AUDIT_SESSION} ...\n{ORACLE_BEGIN}\n{body}\n{ORACLE_END}\n\
             Finished {ORACLE_AUDIT_SESSION} (0:00:03 elapsed time)\n"
        )
    }

    /// THE most important assertion in this file. An audit that could not run is
    /// not a clean audit: the oracle set is UNKNOWN, and unknown must never be
    /// reported as within the whitelist. A permissive whitelist cannot rescue it
    /// either, because there is no observed set to compare against.
    #[test]
    fn an_unavailable_toolchain_is_never_a_clean_audit() {
        for whitelist in [
            Vec::new(),
            vec!["Pure.skip_proof".to_string(), "anything".to_string()],
        ] {
            let report = unavailable_live()
                .audit_axioms(&workspace("Scratch.thy"), "t", &whitelist)
                .unwrap();
            assert!(
                !report.within_whitelist,
                "an unavailable toolchain must fail closed: {report:?}"
            );
            assert!(
                report.axioms.is_empty(),
                "nothing was observed, so no oracle may be claimed: {report:?}"
            );
            assert_eq!(report.detail["live"], true);
            assert_eq!(
                report.detail["audit_ran"], false,
                "the report must say the query never ran"
            );
            assert!(
                report.detail["blocked"].is_string(),
                "a blocked audit must carry its reason: {report:?}"
            );
        }
    }

    /// The other two pre-flight refusals, both of which also fail closed. The
    /// fact-name check doubles as an injection guard: `theorem_name_hint` can
    /// hand us an arbitrary token lifted out of the statement.
    #[test]
    fn unusable_audit_inputs_fail_closed() {
        let backend = unavailable_live();
        for (thm, source) in [
            ("t\\<close> ML \\<open>()", "Scratch.thy"),
            ("", "Scratch.thy"),
            ("t", ""),
            ("t", "not a theory name.thy"),
        ] {
            let report = backend.audit_axioms(&workspace(source), thm, &[]).unwrap();
            assert!(
                !report.within_whitelist,
                "unusable input must fail closed (thm={thm:?}, source={source:?}): {report:?}"
            );
            assert_eq!(report.detail["audit_ran"], false);
        }
        // ... while ordinary and qualified fact names are accepted.
        assert!(is_fact_name("t"));
        assert!(is_fact_name("Scratch.my_thm'"));
        assert!(!is_fact_name("2bad"));
    }

    /// A parseable oracle set drawn from the whitelist is clean.
    #[test]
    fn an_oracle_set_inside_the_whitelist_is_clean() {
        let parsed = parse_oracles(&audit_log("oracles: Foo.trusted_bridge"));
        assert_eq!(parsed, Some(vec!["Foo.trusted_bridge".to_string()]));
        let whitelist = vec!["Foo.trusted_bridge".to_string(), "Foo.other".to_string()];
        let (oracles, within) = oracle_verdict(parsed, &whitelist);
        assert_eq!(oracles, vec!["Foo.trusted_bridge".to_string()]);
        assert!(within, "a whitelisted oracle must pass");
    }

    /// An oracle outside the whitelist fails. Note that Isabelle's PRODUCTION
    /// whitelist is empty, so `sorry`'s `Pure.skip_proof` can never pass this
    /// gate: that is exactly the escape hatch the layer exists to catch.
    #[test]
    fn an_oracle_outside_the_whitelist_fails() {
        let parsed = parse_oracles(&audit_log("oracles: Pure.skip_proof"));
        assert_eq!(parsed, Some(vec!["Pure.skip_proof".to_string()]));
        let (oracles, within) = oracle_verdict(parsed.clone(), &["Foo.other".to_string()]);
        assert_eq!(oracles, vec!["Pure.skip_proof".to_string()]);
        assert!(!within, "an unwhitelisted oracle must fail");

        let production = FormalSystem::Isabelle.axiom_whitelist();
        assert!(
            production.is_empty(),
            "Isabelle admits no oracles at all; this test guards that policy"
        );
        assert!(!oracle_verdict(parsed, &production).1);

        // A partially-whitelisted set is still a failure: the rule is subset,
        // not intersection.
        let mixed = parse_oracles(&audit_log("oracles: Foo.other, Pure.skip_proof"));
        assert_eq!(
            mixed,
            Some(vec!["Foo.other".to_string(), "Pure.skip_proof".to_string()])
        );
        assert!(!oracle_verdict(mixed, &["Foo.other".to_string()]).1);
    }

    /// Output that is not evidence of a COMPLETED query fails closed. Without
    /// both markers, in order, there is nothing to distinguish "no oracles" from
    /// "the diagnostic never executed".
    #[test]
    fn unparseable_output_fails_closed() {
        let end_before_begin = format!("{ORACLE_END}\noracles: Pure.skip_proof\n{ORACLE_BEGIN}\n");
        for output in [
            String::new(),
            "Finished Theoremata_Oracle_Audit_Session".to_string(),
            // Truncated: the query started and the session died mid-way.
            format!("{ORACLE_BEGIN}\noracles: Pure.skip_proof\n"),
            // A stray earlier end marker must not close an unfinished query.
            end_before_begin,
        ] {
            assert_eq!(
                parse_oracles(&output),
                None,
                "unparseable output must not parse: {output:?}"
            );
            let (oracles, within) = oracle_verdict(parse_oracles(&output), &[]);
            assert!(oracles.is_empty());
            assert!(!within, "unparseable output must fail closed: {output:?}");
        }
    }

    /// An EMPTY oracle set is clean, and stays DISTINGUISHABLE from "the oracle
    /// set could not be determined". Both report zero oracles, so the empty
    /// `axioms` vector alone cannot tell them apart; the verdict and the
    /// `audit_ran` flag are what carry the difference, and they must not
    /// collapse into one another.
    #[test]
    fn an_empty_oracle_set_is_clean_and_distinct_from_an_unknown_one() {
        let known_empty = parse_oracles(&audit_log(""));
        assert_eq!(
            known_empty,
            Some(Vec::new()),
            "markers with nothing between them ARE the oracle-free fact"
        );
        let unknown: Option<Vec<String>> = None;

        let (empty_oracles, empty_within) = oracle_verdict(known_empty.clone(), &[]);
        let (unknown_oracles, unknown_within) = oracle_verdict(unknown, &[]);

        assert_eq!(empty_oracles, unknown_oracles, "both observe no oracle");
        assert!(empty_within, "a known-empty oracle set is clean");
        assert!(!unknown_within, "an unknown oracle set is never clean");
        assert!(known_empty.is_some(), "audit_ran is true for the empty set");
    }

    /// A mock backend must never emit a report that could pass for a live clean
    /// audit. Its canned pass is safe ONLY under the `formal.rs` discipline that
    /// stamps `VerificationReport.live = !self.is_mock()`, so the mock's own
    /// detail restates both facts and its `audit_ran` stays false.
    #[test]
    fn the_mock_audit_can_never_look_like_a_live_clean_audit() {
        let backend = IsabelleBackend::mock();
        assert!(backend.is_mock(), "formal.rs keys `live` off this");
        let report = backend
            .audit_axioms(&workspace("Scratch.thy"), "t", &[])
            .unwrap();
        assert!(
            report.within_whitelist,
            "the mock's canned pass is offline scaffolding"
        );
        assert_eq!(report.detail["mock"], true);
        assert_eq!(
            report.detail["live"], false,
            "a mock report must never claim to be live"
        );
        assert_eq!(
            report.detail["audit_ran"], false,
            "no oracle query ran, so the mock must not claim one did"
        );
        // The live backend is the mirror image: it claims `live`, and it only
        // ever claims `within_whitelist` off an audit that actually ran.
        let live = unavailable_live()
            .audit_axioms(&workspace("Scratch.thy"), "t", &[])
            .unwrap();
        assert_eq!(live.detail["live"], true);
        assert!(!live.within_whitelist);
    }

    /// The generated audit theory must actually contain the query and both
    /// markers, since the parser's fail-closed rule keys off them.
    #[test]
    fn the_generated_audit_theory_queries_the_target_between_markers() {
        let thy = oracle_audit_theory("Scratch", "my_thm");
        assert!(thy.contains("imports Scratch"));
        assert!(thy.contains("thm_oracles my_thm"));
        let begin = thy.find(ORACLE_BEGIN).expect("begin marker");
        let end = thy.find(ORACLE_END).expect("end marker");
        assert!(begin < end, "markers must bracket the query");
        assert!(
            oracle_audit_root("Scratch").contains("Scratch_session"),
            "the audit session must extend the candidate's own session"
        );
    }

    /// The boundary trade-off, asserted in the OVER-matching direction: a name
    /// that merely CONTAINS a banned token is ordinary Isabelle.
    #[test]
    fn identifiers_containing_a_hatch_token_are_not_flagged() {
        for code in [
            "lemma oracle_free: \"True\" by simp\n",
            "lemma sorry_free: \"True\" by simp\n",
            "lemma oopsie: \"True\" by simp\n",
        ] {
            let report = fallback_source_scan(code);
            assert!(
                report.clean,
                "innocent identifier must not be flagged ({code:?}): {:?}",
                report.findings
            );
        }
    }
}
