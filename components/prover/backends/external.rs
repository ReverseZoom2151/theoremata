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
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use wait_timeout::ChildExt;

const AGDA_INTERACTION_DIAGNOSTICS_ENV: &str = "THEOREMATA_AGDA_INTERACTION_JSON";

/// Most holes a single enrichment pass will ask Agda about.
///
/// A generated file with a hundred holes must not be able to turn an advisory
/// diagnostic into a gate-length job: every extra hole is another round trip
/// through the type checker. Truncation is reported explicitly (see
/// `holes_truncated`) so a reader never mistakes a capped context listing for a
/// complete one.
const AGDA_GOAL_CONTEXT_HOLE_CAP: usize = 16;

/// Wall-clock budget for the WHOLE goal-context enrichment, measured from the
/// start of the `Cmd_load` phase so a slow load cannot be topped up with a fresh
/// allowance. It only ever curtails the NEW goal-context phase; the load phase
/// keeps the limit it already had, because changing that would change
/// diagnostics that exist today. Deliberately far below the batch checker's own
/// limit: the enrichment is optional, so it yields rather than extends the gate.
const AGDA_GOAL_CONTEXT_DEADLINE: Duration = Duration::from_secs(30);

/// Most files the Metamath include walk will open while building the dependency
/// closure. `$[ file $]` includes nest, and a database is free to include a
/// database that includes another. The cap bounds the walk so a pathological or
/// hostile include chain cannot turn the axiom audit into an unbounded file
/// crawl. Hitting the cap means the closure is INCOMPLETE, which the spec calls
/// a failure rather than a successful skip, so it fails the audit closed.
const METAMATH_INCLUDE_CLOSURE_CAP: usize = 64;

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

/// Parsed, advisory-only output from Agda's JSON interaction protocol.
///
/// `Malformed` is deliberately distinct from `Unsupported`: malformed output is
/// a failed diagnostic attempt and exposes no partial goals/errors, while an
/// older Agda that does not implement `--interaction-json` simply leaves the
/// optional enrichment unavailable. Neither state can influence the batch
/// `agda --safe` verdict.
#[derive(Debug, Clone, PartialEq)]
enum AgdaInteractionDiagnostics {
    Ready {
        records: usize,
        goals: Vec<serde_json::Value>,
        errors: Vec<serde_json::Value>,
        warnings: Vec<serde_json::Value>,
    },
    Unsupported {
        reason: String,
    },
    Malformed {
        reason: String,
    },
}

impl AgdaInteractionDiagnostics {
    fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Ready {
                records,
                goals,
                errors,
                warnings,
            } => json!({
                "status": "ready",
                "well_formed": true,
                "records": records,
                "goals": goals,
                "errors": errors,
                "warnings": warnings,
            }),
            Self::Unsupported { reason } => json!({
                "status": "unsupported",
                "well_formed": false,
                "reason": reason,
            }),
            Self::Malformed { reason } => json!({
                "status": "malformed",
                "well_formed": false,
                "fail_closed": true,
                "reason": reason,
                "goals": [],
                "errors": [],
                "warnings": [],
            }),
        }
    }
}

fn agda_interaction_diagnostics_enabled() -> bool {
    std::env::var(AGDA_INTERACTION_DIAGNOSTICS_ENV)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Parse one JSON object per output line, accepting Agda's optional `JSON>`
/// prefix. The parser is strict at the protocol boundary: once output claims to
/// be JSON, malformed JSON or a malformed recognised diagnostic record rejects
/// the complete diagnostic response rather than returning partial information.
fn parse_agda_interaction_output(stdout: &str) -> AgdaInteractionDiagnostics {
    let mut records = 0usize;
    let mut recognised = 0usize;
    let mut goals = Vec::new();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut non_json = Vec::new();

    for (index, raw_line) in stdout.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let (claims_json, payload) = match line.strip_prefix("JSON>") {
            Some(payload) => (true, payload.trim()),
            None => (line.starts_with('{'), line),
        };
        if !claims_json {
            non_json.push(format!("line {}: {line}", index + 1));
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(payload) {
            Ok(value) => value,
            Err(error) => {
                return AgdaInteractionDiagnostics::Malformed {
                    reason: format!("invalid JSON on line {}: {error}", index + 1),
                };
            }
        };
        records += 1;
        let Some(object) = value.as_object() else {
            return AgdaInteractionDiagnostics::Malformed {
                reason: format!("interaction record {} is not an object", records),
            };
        };
        if object.get("kind").and_then(|v| v.as_str()) != Some("DisplayInfo") {
            continue;
        }
        let Some(info) = object.get("info").and_then(|v| v.as_object()) else {
            return AgdaInteractionDiagnostics::Malformed {
                reason: format!("DisplayInfo record {} has no object-valued info", records),
            };
        };

        match info.get("kind").and_then(|v| v.as_str()) {
            Some("AllGoalsWarnings") => {
                recognised += 1;
                let visible = match info.get("visibleGoals").and_then(|v| v.as_array()) {
                    Some(goals) => goals,
                    None => {
                        return AgdaInteractionDiagnostics::Malformed {
                            reason: "AllGoalsWarnings.visibleGoals is not an array".into(),
                        };
                    }
                };
                let invisible = match info.get("invisibleGoals").and_then(|v| v.as_array()) {
                    Some(goals) => goals,
                    None => {
                        return AgdaInteractionDiagnostics::Malformed {
                            reason: "AllGoalsWarnings.invisibleGoals is not an array".into(),
                        };
                    }
                };
                let record_warnings = match info.get("warnings").and_then(|v| v.as_array()) {
                    Some(warnings) => warnings,
                    None => {
                        return AgdaInteractionDiagnostics::Malformed {
                            reason: "AllGoalsWarnings.warnings is not an array".into(),
                        };
                    }
                };
                for goal in visible {
                    match normalise_agda_goal(goal, "visible") {
                        Ok(goal) => goals.push(goal),
                        Err(reason) => {
                            return AgdaInteractionDiagnostics::Malformed { reason };
                        }
                    }
                }
                for goal in invisible {
                    match normalise_agda_goal(goal, "invisible") {
                        Ok(goal) => goals.push(goal),
                        Err(reason) => {
                            return AgdaInteractionDiagnostics::Malformed { reason };
                        }
                    }
                }
                warnings.extend(record_warnings.iter().cloned());
            }
            Some("GoalSpecific") => {
                recognised += 1;
                match normalise_agda_goal_specific(info) {
                    Ok(goal) => goals.push(goal),
                    Err(reason) => {
                        return AgdaInteractionDiagnostics::Malformed { reason };
                    }
                }
            }
            _ => {}
        }

        if info.contains_key("error") || info.contains_key("errors") {
            recognised += 1;
            if let Err(reason) = collect_agda_errors(info, &mut errors) {
                return AgdaInteractionDiagnostics::Malformed { reason };
            }
        }
    }

    if records == 0 {
        let reason = if non_json.is_empty() {
            "interaction mode produced no diagnostics".to_string()
        } else {
            format!(
                "interaction-json is unsupported or produced no JSON ({})",
                non_json.join("; ")
            )
        };
        return AgdaInteractionDiagnostics::Unsupported { reason };
    }
    if !non_json.is_empty() {
        return AgdaInteractionDiagnostics::Malformed {
            reason: format!(
                "non-JSON output was interleaved with interaction records: {}",
                non_json.join("; ")
            ),
        };
    }
    if recognised == 0 {
        return AgdaInteractionDiagnostics::Unsupported {
            reason: format!(
                "parsed {records} JSON record(s), but this Agda version exposed no supported goal/error schema"
            ),
        };
    }
    AgdaInteractionDiagnostics::Ready {
        records,
        goals,
        errors,
        warnings,
    }
}

/// Interpret the transport outcome without consulting its exit code. Agda's
/// interaction protocol is diagnostic-only: launch/timeout/cap failures affect
/// diagnostic availability, while the process status can neither validate nor
/// invalidate the separately computed batch verdict.
fn parse_agda_interaction_outcome(outcome: &exec::ExecOutcome) -> AgdaInteractionDiagnostics {
    if !outcome.launched {
        AgdaInteractionDiagnostics::Unsupported {
            reason: format!("interaction process could not launch: {}", outcome.stderr),
        }
    } else if outcome.timed_out {
        AgdaInteractionDiagnostics::Malformed {
            reason: "interaction diagnostics timed out".into(),
        }
    } else if outcome.output_capped {
        AgdaInteractionDiagnostics::Malformed {
            reason: "interaction diagnostics exceeded the output cap".into(),
        }
    } else {
        parse_agda_interaction_output(&outcome.stdout)
    }
}

fn normalise_agda_goal(
    goal: &serde_json::Value,
    visibility: &str,
) -> std::result::Result<serde_json::Value, String> {
    let object = goal
        .as_object()
        .ok_or_else(|| format!("{visibility} goal is not an object"))?;
    let constraint = object
        .get("constraintObj")
        .and_then(|value| value.as_object())
        .ok_or_else(|| format!("{visibility} goal has no constraintObj object"))?;
    let id = constraint
        .get("id")
        .or_else(|| constraint.get("name"))
        .cloned()
        .ok_or_else(|| format!("{visibility} goal has no id/name"))?;
    let goal_type = object
        .get("type")
        .cloned()
        .ok_or_else(|| format!("{visibility} goal has no type"))?;
    Ok(json!({
        "visibility": visibility,
        "id": id,
        "kind": object.get("kind").cloned().unwrap_or(serde_json::Value::Null),
        "type": goal_type,
        "range": constraint.get("range").cloned().unwrap_or(serde_json::Value::Null),
    }))
}

fn normalise_agda_goal_specific(
    info: &serde_json::Map<String, serde_json::Value>,
) -> std::result::Result<serde_json::Value, String> {
    let point = info
        .get("interactionPoint")
        .and_then(|value| value.as_object())
        .ok_or_else(|| "GoalSpecific has no interactionPoint object".to_string())?;
    let goal_info = info
        .get("goalInfo")
        .and_then(|value| value.as_object())
        .ok_or_else(|| "GoalSpecific has no goalInfo object".to_string())?;
    let id = point
        .get("id")
        .cloned()
        .ok_or_else(|| "GoalSpecific interactionPoint has no id".to_string())?;
    let goal_type = goal_info
        .get("type")
        .cloned()
        .ok_or_else(|| "GoalSpecific goalInfo has no type".to_string())?;
    Ok(json!({
        "visibility": "specific",
        "id": id,
        "type": goal_type,
        "range": point.get("range").cloned().unwrap_or(serde_json::Value::Null),
        "context": goal_info.get("entries").cloned().unwrap_or_else(|| json!([])),
    }))
}

fn collect_agda_errors(
    info: &serde_json::Map<String, serde_json::Value>,
    errors: &mut Vec<serde_json::Value>,
) -> std::result::Result<(), String> {
    if let Some(error) = info.get("error") {
        errors.push(normalise_agda_error(error)?);
    }
    if let Some(many) = info.get("errors") {
        let many = many
            .as_array()
            .ok_or_else(|| "DisplayInfo.errors is not an array".to_string())?;
        for error in many {
            errors.push(normalise_agda_error(error)?);
        }
    }
    Ok(())
}

fn normalise_agda_error(
    error: &serde_json::Value,
) -> std::result::Result<serde_json::Value, String> {
    let object = error
        .as_object()
        .ok_or_else(|| "Agda error is not an object".to_string())?;
    let message = object
        .get("message")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "Agda error has no string message".to_string())?;
    Ok(json!({
        "message": message,
        "range": object.get("range").cloned().unwrap_or(serde_json::Value::Null),
    }))
}

/// Collect the interaction-point ids worth asking Agda about, capped.
///
/// The ids come from the `AllGoalsWarnings` reply we ALREADY parse rather than
/// from a `{! !}` regex over the source (which is what agda-cli does). Two
/// reasons: the reply is Agda's own view of the file, so it cannot disagree with
/// the checker about what a hole is, and it costs no new parser. A source regex
/// additionally mis-reads holes inside comments, string literals, and nested
/// braces, and it cannot see holes that Agda created but the text did not spell.
///
/// Only NUMERIC ids are returned. Agda's invisible goals are unsolved metas
/// identified by name (`_8`); they are not interaction points, so
/// `Cmd_goal_type_context_infer` has nothing to address them with.
///
/// Returns the capped id list plus how many were dropped by the cap.
fn agda_interaction_point_ids(
    diagnostics: &AgdaInteractionDiagnostics,
    cap: usize,
) -> (Vec<u64>, usize) {
    let AgdaInteractionDiagnostics::Ready { goals, .. } = diagnostics else {
        return (Vec::new(), 0);
    };
    let mut ids: Vec<u64> = Vec::new();
    for goal in goals {
        if goal.get("visibility").and_then(|v| v.as_str()) != Some("visible") {
            continue;
        }
        let Some(id) = goal.get("id").and_then(|id| id.as_u64()) else {
            continue;
        };
        // A repeated id would spend budget on an answer we already have.
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let truncated = ids.len().saturating_sub(cap);
    ids.truncate(cap);
    (ids, truncated)
}

/// Build ONE stdin script that reloads the file and then asks for the type and
/// context of each hole.
///
/// The reload is required because this is a fresh process: Agda holds the
/// interaction points of the currently loaded file in memory, so a
/// goal-specific command sent without a preceding `Cmd_load` addresses nothing.
/// Every command goes on the same stream to the same process, so hole count
/// costs round trips inside one type-checker run and never a new process.
///
/// The trailing `x` line is the same terminator the load-only request uses: it
/// is not a valid `IOTCM`, which is how the interaction loop is told there is
/// nothing more to serve.
fn agda_goal_context_request(encoded_filename: &str, ids: &[u64]) -> String {
    let mut request = format!(
        "IOTCM {encoded_filename} None Direct (Cmd_load {encoded_filename} [])\n"
    );
    for id in ids {
        request.push_str(&format!(
            "IOTCM {encoded_filename} None Direct (Cmd_goal_type_context_infer Normalised {id} noRange \"?\")\n"
        ));
    }
    request.push_str("x\n");
    request
}

/// Fold the `GoalSpecific` contexts into the goals the load phase already
/// produced, matching on interaction-point id.
///
/// This only ever ADDS a `context` key to an existing goal object. Nothing is
/// removed, retyped, or reordered, so a caller that ignores `context` sees byte
/// for byte what it saw before the enrichment existed. Returns how many goals
/// gained a context and how many replies matched no known goal.
fn merge_agda_goal_contexts(
    goals: &mut [serde_json::Value],
    enrichment: &AgdaInteractionDiagnostics,
) -> (usize, usize) {
    let AgdaInteractionDiagnostics::Ready { goals: replies, .. } = enrichment else {
        return (0, 0);
    };
    let mut merged = 0usize;
    let mut unmatched = 0usize;
    for reply in replies {
        if reply.get("visibility").and_then(|v| v.as_str()) != Some("specific") {
            continue;
        }
        let Some(context) = reply.get("context") else {
            continue;
        };
        let Some(id) = reply.get("id").and_then(|id| id.as_u64()) else {
            unmatched += 1;
            continue;
        };
        let mut hit = false;
        for goal in goals.iter_mut() {
            if goal.get("id").and_then(|value| value.as_u64()) != Some(id) {
                continue;
            }
            let Some(object) = goal.as_object_mut() else {
                continue;
            };
            object.insert("context".into(), context.clone());
            hit = true;
            merged += 1;
        }
        if !hit {
            unmatched += 1;
        }
    }
    (merged, unmatched)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn interaction_process(
    runner: &Runner,
    argv: &[String],
    workspace: &Path,
) -> std::result::Result<Command, String> {
    if argv.is_empty() {
        return Err("empty interaction command".into());
    }
    match runner {
        Runner::Native => {
            let mut command = Command::new(&argv[0]);
            command.args(&argv[1..]).current_dir(workspace);
            Ok(command)
        }
        Runner::Wsl { distro } => {
            let workspace = exec::to_wsl_path(workspace);
            let command_line = argv
                .iter()
                .map(|arg| shell_quote(arg))
                .collect::<Vec<_>>()
                .join(" ");
            let script = format!("cd {} && {command_line}", shell_quote(&workspace));
            let mut command = Command::new("wsl.exe");
            command.args(["-d", distro.as_str(), "--", "bash", "-lc", script.as_str()]);
            Ok(command)
        }
        Runner::Docker { image } => {
            let mut host = workspace.to_string_lossy().replace('\\', "/");
            if let Some(stripped) = host.strip_prefix("//?/") {
                host = stripped.to_string();
            }
            let mut command = Command::new("docker");
            command
                .args(["run", "--rm", "-i", "-v"])
                .arg(format!("{host}:/work"))
                .args(["-w", "/work"])
                .arg(image)
                .args(argv);
            Ok(command)
        }
    }
}

fn read_interaction_pipe<R: Read>(mut reader: R, cap: usize) -> (String, bool) {
    let mut output = Vec::new();
    let mut capped = false;
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => {
                if output.len() < cap {
                    let take = (cap - output.len()).min(count);
                    output.extend_from_slice(&chunk[..take]);
                    capped |= take < count;
                } else {
                    capped = true;
                }
            }
            Err(_) => break,
        }
    }
    (String::from_utf8_lossy(&output).into_owned(), capped)
}

/// Run an optional interaction command with stdin while retaining the same
/// timeout/output caps as the authoritative batch runner. This is local to the
/// external backend because the normal gate runner intentionally closes stdin.
///
/// `deadline` may only TIGHTEN the configured wall-clock limit, never loosen it:
/// the enrichment phase carries its own, smaller budget, but no caller may buy
/// itself more time than the operator configured.
fn run_interaction_with_input(
    runner: &Runner,
    argv: &[String],
    workspace: &Path,
    input: &str,
    deadline: Option<Duration>,
) -> exec::ExecOutcome {
    let mut command = match interaction_process(runner, argv, workspace) {
        Ok(command) => command,
        Err(error) => {
            return exec::ExecOutcome {
                launched: false,
                code: None,
                stdout: String::new(),
                stderr: error,
                timed_out: false,
                output_capped: false,
            };
        }
    };
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return exec::ExecOutcome {
                launched: false,
                code: None,
                stdout: String::new(),
                stderr: error.to_string(),
                timed_out: false,
                output_capped: false,
            };
        }
    };

    let mut limits = exec::ResourceLimits::from_env();
    if let Some(deadline) = deadline {
        limits.timeout = limits.timeout.min(deadline);
    }
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (stdout_tx, stdout_rx) = std::sync::mpsc::channel();
    let (stderr_tx, stderr_rx) = std::sync::mpsc::channel();
    let cap = limits.max_output_bytes;
    std::thread::spawn(move || {
        let value = stdout
            .map(|pipe| read_interaction_pipe(pipe, cap))
            .unwrap_or_default();
        let _ = stdout_tx.send(value);
    });
    std::thread::spawn(move || {
        let value = stderr
            .map(|pipe| read_interaction_pipe(pipe, cap))
            .unwrap_or_default();
        let _ = stderr_tx.send(value);
    });

    let input_error = child.stdin.take().and_then(|mut stdin| {
        stdin
            .write_all(input.as_bytes())
            .err()
            .map(|error| error.to_string())
    });
    let (timed_out, code) = match child.wait_timeout(limits.timeout) {
        Ok(Some(status)) => (false, status.code()),
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            (true, None)
        }
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return exec::ExecOutcome {
                launched: true,
                code: None,
                stdout: String::new(),
                stderr: error.to_string(),
                timed_out: false,
                output_capped: false,
            };
        }
    };
    let grace = Duration::from_secs(2);
    let (stdout, stdout_capped) = stdout_rx
        .recv_timeout(grace)
        .unwrap_or_else(|_| ("[theoremata] interaction stdout did not drain".into(), true));
    let (mut stderr, stderr_capped) = stderr_rx
        .recv_timeout(grace)
        .unwrap_or_else(|_| ("[theoremata] interaction stderr did not drain".into(), true));
    if let Some(error) = input_error {
        stderr.push_str(&format!(
            "\n[theoremata] failed to write interaction request: {error}"
        ));
    }
    if timed_out {
        stderr.push_str(&format!(
            "\n[theoremata] interaction diagnostics exceeded {}s resource limit",
            limits.timeout.as_secs()
        ));
    }
    exec::ExecOutcome {
        launched: true,
        code,
        stdout,
        stderr,
        timed_out,
        output_capped: stdout_capped || stderr_capped,
    }
}

pub fn mock_enabled(config: &Config, system: FormalSystem) -> bool {
    config.prover_mock
        || match system {
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
        system,
        formal_project: FormalProject {
            system,
            root: config.resources.clone(),
            toolchain: None,
            imports: system.default_imports(),
            metadata: json!({}),
        },
        statement: statement.into(),
        stub: None,
        prompt: None,
        backend: backend_name(system).into(),
        metadata: json!({}),
    }
}

pub fn submit(
    store: &Store,
    config: &Config,
    task: ProofTask,
    artifacts_dir: Option<std::path::PathBuf>,
) -> Result<ProofJob> {
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
        task_id: job.task.id.clone(),
        job_id: job.id.clone(),
        status: job.status,
        formal_code: Some(code.clone()),
        counterexample: None,
        verification,
        artifacts_dir: job.artifacts_dir.clone(),
        duration_ms: 0,
        cost: None,
        message: Some(format!("mock {system} checker completed")),
        provenance: json!({"backend": backend_name(system), "system": system.as_str(), "mock": true}),
    });
    if let Some(dir) = &job.artifacts_dir {
        let sub = dir.join(backend_name(system));
        std::fs::create_dir_all(&sub)?;
        std::fs::write(
            sub.join(format!("solution{}", system.source_extension())),
            &code,
        )?;
        std::fs::write(
            dir.join("result.json"),
            serde_json::to_string_pretty(job.result.as_ref().unwrap())?,
        )?;
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
    let mut job = store
        .get_proof_job(job_id)?
        .ok_or_else(|| anyhow::anyhow!("unknown proof job {job_id}"))?;
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

    /// Run Agda's JSON interaction protocol strictly as an advisory enrichment.
    /// The authoritative verdict remains the separate batch `agda --safe` run
    /// in [`FormalBackend::compile`]. In particular, this function records but
    /// never interprets the interaction process's exit status as success.
    fn agda_interaction_diagnostics(&self, ws: &Workspace) -> serde_json::Value {
        let filename = ws
            .source_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Generated.agda");
        let encoded_filename =
            serde_json::to_string(filename).unwrap_or_else(|_| "\"Generated.agda\"".to_string());
        let request =
            format!("IOTCM {encoded_filename} None Direct (Cmd_load {encoded_filename} [])\nx\n");
        let command = vec![
            self.binary.clone(),
            "--safe".into(),
            "--interaction-json".into(),
        ];
        let started = Instant::now();
        // The load phase keeps the operator-configured limit it has always had:
        // tightening it here would change diagnostics that exist today, which
        // the goal-context work is not entitled to do.
        let outcome = run_interaction_with_input(&self.runner, &command, &ws.root, &request, None);
        let parsed = parse_agda_interaction_outcome(&outcome);
        let mut detail = parsed.to_json();
        // `Cmd_load` answers with goal TYPES only. The hypotheses in scope at
        // each hole need a goal-specific command, so ask for them in a second
        // phase and fold the answers into the goals above. Purely additive: on
        // any failure the diagnostics stay exactly as the load phase left them.
        let enrichment =
            self.agda_goal_context_enrichment(&mut detail, &parsed, &command, ws, started);
        if let Some(object) = detail.as_object_mut() {
            object.insert("goal_context_enrichment".into(), enrichment);
            object.insert("authority".into(), json!("advisory_only"));
            object.insert("verdict_source".into(), json!("batch_agda_safe"));
            object.insert("exit_status_trusted".into(), json!(false));
            object.insert("runner".into(), json!(self.runner.tag()));
            object.insert("command".into(), json!(command));
            object.insert("code".into(), json!(outcome.code));
            object.insert("stderr".into(), json!(outcome.stderr));
            object.insert("stdout".into(), json!(outcome.stdout));
        }
        detail
    }

    /// Second phase of the advisory Agda diagnostics: ask for the CONTEXT
    /// (hypotheses in scope) at each open hole and merge it into `detail`.
    ///
    /// Every failure mode here is silent by construction. An Agda too old to
    /// know `Cmd_goal_type_context_infer`, a reply we cannot parse, a blown
    /// deadline, or a file with no holes at all each return a status record and
    /// leave `detail` byte for byte as the load phase produced it. Nothing on
    /// this path can reach the pass/fail verdict, which is computed from the
    /// separate batch `agda --safe` run before this is ever called.
    fn agda_goal_context_enrichment(
        &self,
        detail: &mut serde_json::Value,
        loaded: &AgdaInteractionDiagnostics,
        command: &[String],
        ws: &Workspace,
        started: Instant,
    ) -> serde_json::Value {
        let (ids, holes_truncated) = agda_interaction_point_ids(loaded, AGDA_GOAL_CONTEXT_HOLE_CAP);
        if ids.is_empty() {
            return json!({
                "status": "skipped",
                "authority": "advisory_only",
                "reason": "the load phase reported no addressable interaction points",
                "holes_queried": 0,
                "holes_truncated": holes_truncated,
            });
        }
        // The deadline covers BOTH phases, so a slow load leaves less room here
        // and can legitimately consume the whole budget.
        let remaining = AGDA_GOAL_CONTEXT_DEADLINE
            .checked_sub(started.elapsed())
            .filter(|left| !left.is_zero());
        let Some(remaining) = remaining else {
            return json!({
                "status": "deadline_exhausted",
                "authority": "advisory_only",
                "reason": "the load phase consumed the goal-context budget",
                "deadline_secs": AGDA_GOAL_CONTEXT_DEADLINE.as_secs(),
                "holes_queried": 0,
                "holes_truncated": holes_truncated + ids.len(),
            });
        };
        let filename = ws
            .source_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Generated.agda");
        let encoded_filename =
            serde_json::to_string(filename).unwrap_or_else(|_| "\"Generated.agda\"".to_string());
        let request = agda_goal_context_request(&encoded_filename, &ids);
        let outcome =
            run_interaction_with_input(&self.runner, command, &ws.root, &request, Some(remaining));
        let replies = parse_agda_interaction_outcome(&outcome);
        let (status, merged, unmatched, reason) = match &replies {
            AgdaInteractionDiagnostics::Ready { .. } => {
                match detail.get_mut("goals").and_then(|g| g.as_array_mut()) {
                    Some(goals) => {
                        let (merged, unmatched) = merge_agda_goal_contexts(goals, &replies);
                        ("ready", merged, unmatched, String::new())
                    }
                    // Malformed load diagnostics expose no goals to merge into;
                    // that state is preserved, never repaired from here.
                    None => (
                        "degraded",
                        0,
                        0,
                        "the load phase exposed no goals to merge into".to_string(),
                    ),
                }
            }
            AgdaInteractionDiagnostics::Unsupported { reason } => {
                ("unsupported", 0, 0, reason.clone())
            }
            AgdaInteractionDiagnostics::Malformed { reason } => ("degraded", 0, 0, reason.clone()),
        };
        json!({
            "status": status,
            "authority": "advisory_only",
            "verdict_source": "batch_agda_safe",
            "exit_status_trusted": false,
            "reason": reason,
            "hole_cap": AGDA_GOAL_CONTEXT_HOLE_CAP,
            "deadline_secs": AGDA_GOAL_CONTEXT_DEADLINE.as_secs(),
            "holes_queried": ids.len(),
            "holes_truncated": holes_truncated,
            "hole_ids": ids,
            "contexts_merged": merged,
            "replies_unmatched": unmatched,
            "code": outcome.code,
            "stderr": outcome.stderr,
        })
    }

    /// Probe the checker for its exact version string.
    ///
    /// Returns the reason for failure rather than a guess: an unknown tool
    /// version is a failure state in the spec, not a blank field, so callers
    /// must be able to say WHY it is missing.
    fn checker_version(&self) -> std::result::Result<String, String> {
        if self.mock {
            return Err("mock backend does not invoke a checker".into());
        }
        let probe = match self.system {
            FormalSystem::Agda => vec![self.binary.as_str(), "--version"],
            // The `metamath` reference binary has no version flag; its help
            // banner is the only place the version appears.
            FormalSystem::Metamath => vec![self.binary.as_str(), "-h"],
            _ => return Err("unsupported system".into()),
        };
        let out = exec::run(&self.runner, &probe, Path::new("."));
        if !out.launched {
            return Err(format!("version probe could not launch: {}", out.stderr));
        }
        let combined = format!("{}\n{}", out.stdout, out.stderr);
        combined
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && line.to_ascii_lowercase().contains("version"))
            .map(str::to_string)
            .ok_or_else(|| {
                format!(
                    "{} exposed no recognisable version line",
                    backend_name(self.system)
                )
            })
    }

    /// The common cross-backend contract from docs/formal-systems: system,
    /// checker identity, source hash, dependency hash, and the resource limits
    /// the run was held to.
    ///
    /// Every field that cannot be computed is emitted as an explicit
    /// null-with-reason (see [`unavailable_field`]) so a reader can never
    /// confuse "we did not record this" with "this was empty".
    fn provenance(&self, ws: Option<&Workspace>) -> serde_json::Value {
        let limits = exec::ResourceLimits::from_env();
        let filename = ws
            .and_then(|ws| ws.source_path.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("Generated");
        let version = match self.checker_version() {
            Ok(version) => json!(version),
            Err(reason) => unavailable_field(&reason),
        };
        let source = ws.map(|ws| std::fs::read(&ws.source_path));
        let source_sha256 = match &source {
            Some(Ok(bytes)) => json!(sha256_hex(bytes)),
            Some(Err(error)) => {
                unavailable_field(&format!("source could not be read: {error}"))
            }
            None => unavailable_field("no workspace was scaffolded for this phase"),
        };
        // Metamath's dependencies are the resolved `$[ file $]` closure. Agda's
        // are a library/module graph this backend does not resolve, so the field
        // is explicitly absent with the reason rather than silently empty.
        let dependency_sha256 = match (self.system, ws, &source) {
            (FormalSystem::Metamath, Some(ws), Some(Ok(bytes))) => {
                let text = String::from_utf8_lossy(bytes).into_owned();
                let stripped = crate::prover::formal::strip_comments(&text);
                let closure = metamath_include_closure(&ws.root, stripped.as_str());
                match closure.dependency_sha256 {
                    Some(sha) => json!(sha),
                    None => unavailable_field(&format!(
                        "include closure is incomplete: {}",
                        closure.unresolved.join("; ")
                    )),
                }
            }
            (FormalSystem::Metamath, _, _) => {
                unavailable_field("the database source could not be read")
            }
            _ => unavailable_field(
                "the Agda module-graph closure is not resolved by this backend",
            ),
        };
        json!({
            "system": self.system.as_str(),
            "mock": self.mock,
            "runner": self.runner.tag(),
            "checker": {
                "binary": self.binary.clone(),
                "version": version,
                "command": self.command(filename),
            },
            "source_sha256": source_sha256,
            "dependency_sha256": dependency_sha256,
            "limits": {
                "timeout_seconds": limits.timeout.as_secs(),
                "max_output_bytes": limits.max_output_bytes,
            },
        })
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
                detail: json!({"mock": true, "provenance": self.provenance(Some(ws))}),
            });
        }
        if !self.available() {
            return Ok(CompileReport {
                compiled: false,
                errors: vec![format!("{} toolchain unavailable", self.system)],
                per_unit: vec![],
                detail: json!({"unavailable": true, "provenance": self.provenance(Some(ws))}),
            });
        }
        let out = self.run_file(ws);
        let filename = ws
            .source_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("Generated");
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
        // Optional structured feedback only. This is intentionally computed
        // AFTER the batch verdict and is never folded back into `verified`.
        // Unsupported interaction mode leaves the backend available; malformed
        // interaction output is represented as a failed diagnostic response.
        let interaction_diagnostics = if self.system == FormalSystem::Agda {
            if agda_interaction_diagnostics_enabled() {
                self.agda_interaction_diagnostics(ws)
            } else {
                json!({
                    "status": "disabled",
                    "authority": "advisory_only",
                    "verdict_source": "batch_agda_safe",
                    "enable_with": AGDA_INTERACTION_DIAGNOSTICS_ENV,
                })
            }
        } else {
            serde_json::Value::Null
        };
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
            detail: json!({"runner": self.runner.tag(), "binary": self.binary.clone(), "command": command, "code": out.code, "verified": verified, "stdout": out.stdout, "stderr": out.stderr, "interaction_diagnostics": interaction_diagnostics, "provenance": self.provenance(Some(ws))}),
        })
    }

    /// Layer 2 of the gate: what does this proof ASSUME?
    ///
    /// This used to return `within_whitelist: true` unconditionally, which made
    /// layer 2 inert for two of six systems while still presenting as a gate.
    /// An audit that cannot run must say so and fail CLOSED; asserting
    /// cleanliness it never established is the one answer it may not give.
    fn audit_axioms(&self, ws: &Workspace, thm: &str, whitelist: &[String]) -> Result<AxiomReport> {
        if self.mock {
            // The mock's canned clean audit is offline scaffolding, and it is
            // safe ONLY because `is_mock()` forces `VerificationReport.live` to
            // false in `formal.rs::verify`, so no caller can promote it. The
            // markers are repeated in the detail so a persisted report stays
            // self-describing when read out of context.
            return Ok(AxiomReport {
                axioms: Vec::new(),
                within_whitelist: true,
                detail: json!({
                    "system": self.system.as_str(),
                    "mock": true,
                    "live": false,
                    "audit_ran": false,
                    "whitelist": whitelist,
                    "provenance": self.provenance(Some(ws)),
                }),
            });
        }
        let provenance = self.provenance(Some(ws));
        if !self.available() {
            return Ok(blocked_audit(
                self.system,
                whitelist,
                "toolchain unavailable; the assumed axiom set could not be determined",
                provenance,
                json!({"unavailable": true}),
            ));
        }
        // The audit reads the SUBMITTED source, never a rewritten copy: editing
        // it would invalidate the source scan and the statement-preservation
        // check, which both read the same text.
        let code = match std::fs::read_to_string(&ws.source_path) {
            Ok(code) => code,
            Err(error) => {
                return Ok(blocked_audit(
                    self.system,
                    whitelist,
                    "the candidate source could not be read",
                    provenance,
                    json!({"source_path": ws.source_path.to_string_lossy(), "error": error.to_string()}),
                ));
            }
        };
        let stripped = crate::prover::formal::strip_comments(&code);
        match self.system {
            FormalSystem::Agda => {
                // Agda has no `#print axioms`, so the assumed set is what the
                // module postulates plus whatever its imports postulate. Local
                // postulates are read off the source; imports are only trusted
                // when they lie inside the DECLARED closure (the builtins that
                // ship with the checker and are covered by `--safe`). Any other
                // import is a module this backend has not read, so the closure
                // is unknown and the audit fails closed rather than reporting a
                // local-only answer as if it were complete. See the
                // `agda_transitive_postulates` note in this file's tests for the
                // module-graph walk that would lift this restriction.
                let axioms = agda_postulate_names(stripped.as_str());
                let imports = agda_imports(stripped.as_str());
                let unresolved: Vec<String> = imports
                    .iter()
                    .filter(|module| !agda_import_inside_declared_closure(module.as_str()))
                    .cloned()
                    .collect();
                let local_clean = axioms.iter().all(|axiom| whitelist.contains(axiom));
                let within = local_clean && unresolved.is_empty();
                Ok(AxiomReport {
                    axioms: axioms.clone(),
                    within_whitelist: within,
                    detail: json!({
                        "system": "agda",
                        "live": true,
                        "audit_ran": true,
                        "target": thm,
                        "whitelist": whitelist,
                        "local_postulates": axioms,
                        "local_postulates_within_whitelist": local_clean,
                        "imports": imports,
                        "declared_closure": FormalSystem::Agda.default_imports(),
                        "imports_outside_declared_closure": unresolved,
                        "transitive_imported_postulates": unavailable_field(
                            "resolving imported postulates needs an Agda module-graph walk this backend does not perform; imports outside the declared closure therefore fail the audit closed",
                        ),
                        "provenance": provenance,
                    }),
                })
            }
            FormalSystem::Metamath => {
                // A `$a` in the CANDIDATE source is a new axiom and widens the
                // trusted base. A `$a` inside an included database is a reviewed
                // database axiom: the spec says to RECORD those, not to reject
                // them, so they are counted per file instead of being folded
                // into the whitelist check. An include that cannot be resolved
                // makes the closure incomplete, which is a failure and not a
                // successful skip.
                let axioms = metamath_axiom_labels(stripped.as_str());
                let closure = metamath_include_closure(&ws.root, stripped.as_str());
                let local_clean = axioms.iter().all(|axiom| whitelist.contains(axiom));
                let within = local_clean && closure.unresolved.is_empty();
                Ok(AxiomReport {
                    axioms: axioms.clone(),
                    within_whitelist: within,
                    detail: json!({
                        "system": "metamath",
                        "live": true,
                        "audit_ran": true,
                        "target": thm,
                        "whitelist": whitelist,
                        "generated_axioms": axioms,
                        "generated_axioms_within_whitelist": local_clean,
                        "database_closure": closure.files,
                        "database_closure_unresolved": closure.unresolved,
                        "database_closure_complete": closure.unresolved.is_empty(),
                        "provenance": provenance,
                    }),
                })
            }
            _ => unreachable!("ExternalBackend only supports Agda and Metamath"),
        }
    }

    fn kernel_recheck(&self, ws: &Workspace) -> Result<RecheckReport> {
        if self.mock {
            return Ok(RecheckReport {
                rechecked: true,
                detail: json!({"mock": true, "live": false, "provenance": self.provenance(Some(ws))}),
            });
        }
        if !self.available() {
            return Ok(RecheckReport {
                rechecked: false,
                detail: json!({"unavailable": true, "provenance": self.provenance(Some(ws))}),
            });
        }
        let out = self.run_file(ws);
        let filename = ws
            .source_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("Generated");
        let command = self.command(filename);
        // SOUNDNESS: this used to read `out.success()`, i.e. the exit code. The
        // Metamath reference binary returns 0 even when `verify proof *` FAILS
        // (see `compile_success_signal`), so the exit code alone would certify a
        // failed re-check. The recheck therefore uses the SAME declared success
        // signal as `compile`: sentinel for Metamath, honest exit for Agda.
        let primary_passed = self.compile_success_signal().is_pass(
            out.launched,
            out.success(),
            &out.stdout,
            &out.stderr,
        );
        let mut secondary = json!(null);
        let mut secondary_passed = None;
        if let (FormalSystem::Metamath, Some(binary)) = (self.system, &self.secondary_binary) {
            let args = [binary.as_str(), filename];
            let second = exec::run(&self.runner, &args, &ws.root);
            // The secondary checker may CORROBORATE or FLAG DISAGREEMENT; it may
            // never substitute for the primary verdict (docs/formal-systems).
            // Its exit code is the only signal an arbitrary third-party checker
            // offers, so a non-zero exit or a failed launch reads as "did not
            // pass" (fail-closed).
            let passed = second.success();
            secondary_passed = Some(passed);
            let agreement = if passed == primary_passed {
                "agree"
            } else {
                "disagree"
            };
            secondary = json!({
                "binary": binary,
                "command": args,
                "code": second.code,
                "stdout": second.stdout,
                "stderr": second.stderr,
                "passed": passed,
                "role": "cross_check_only",
                "agreement": agreement,
            });
        }
        let (rechecked, disagreement) = cross_checked_verdict(primary_passed, secondary_passed);
        Ok(RecheckReport {
            rechecked,
            detail: json!({
                "binary": self.binary.clone(),
                "command": command,
                "code": out.code,
                "stdout": out.stdout,
                "stderr": out.stderr,
                "checker": self.system.as_str(),
                "primary_passed": primary_passed,
                "exit_status_trusted": false,
                "secondary": secondary,
                "secondary_disagreement": disagreement,
                "provenance": self.provenance(Some(ws)),
            }),
        })
    }

    fn source_scan(&self, code: &str) -> Result<ScanReport> {
        if let Some(report) = crate::prover::formal::worker_source_scan(self.system, code) {
            return Ok(report);
        }
        Ok(fallback_source_scan(self.system, code))
    }
}

/// Offline lexical fallback for [`ExternalBackend::source_scan`] (Agda and
/// Metamath).
///
/// Matched over COMMENT-STRIPPED source so this offline path agrees with the
/// online (worker) path and with the authoritative policy in
/// [`crate::prover::statement_preservation`]
/// (`ESCAPE_HATCH_COMMENT_POLICY == CommentPolicy::CodeOnly`): a commented-out
/// escape hatch is never seen by the checker, so it must not gate. This
/// LOOSENS the gate with respect to commented text ONLY — real constructs are
/// untouched by stripping and still fail.
///
/// **Agda pragmas are the subtle case.** `--allow-unsolved-metas` and
/// `{-# COMPILED ... #-}` live inside `{-# ... #-}`, which is a PRAGMA that the
/// checker acts on, not a comment. Blanket stripping would erase both and blind
/// these checks, so [`crate::prover::formal::strip_comments`] exempts pragmas
/// (copying them through verbatim, `--` options included). Metamath's
/// `$( ... $)` needs no special handling — `strip_comments` already covers it.
fn fallback_source_scan(system: FormalSystem, code: &str) -> ScanReport {
    let mut findings = Vec::new();
    if system == FormalSystem::Agda {
        // The token list is the SHARED, ALIAS-EXPANDED table in `formal.rs`
        // ([`crate::prover::formal::escape_hatch_tokens`]), matched on word
        // boundaries. Agda's hatches are a family of RENAMES of one move —
        // turning a checker off -- so banning `--allow-unsolved-metas` while
        // leaving `--no-termination-check`, `--no-positivity-check`,
        // `--no-coverage-check`, `--type-in-type`, `--unsafe` and `primTrustMe`
        // unbanned was protection in name only.
        //
        // `--allow-incomplete-matches` is deliberately NOT in that list. Agda's
        // own documentation lists it among the options `--safe` refuses, so the
        // checker we invoke (`agda --safe`) already rejects any file that asks
        // for it, both on the command line and via an `{-# OPTIONS #-}` pragma.
        // Adding the needle would duplicate a guarantee we already hold rather
        // than close a hole. `--allow-unsolved-metas` is likewise rejected by
        // `--safe` but stays here as defence in depth for the offline scan,
        // which runs on sources no checker has seen yet.
        findings.extend(crate::prover::formal::escape_hatch_findings(system, code));
    } else if system == FormalSystem::Metamath {
        // Metamath's checks are STRUCTURAL (`$a` declarations, `?` placeholder
        // steps, escaping include paths), not a token list, so they stay here.
        let stripped = crate::prover::formal::strip_comments(code);
        findings.extend(metamath_source_findings(stripped.as_str()));
    }
    ScanReport {
        clean: findings.is_empty(),
        findings,
        detail: json!({"system": system.as_str(), "fallback": true}),
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
    if code
        .split(|c: char| c.is_whitespace())
        .any(|tok| tok == "?")
    {
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

// --- provenance and axiom audit -------------------------------------------

/// Lowercase hex of a digest. sha2 0.11's output no longer implements
/// `LowerHex`, so the bytes are formatted explicitly (same helper shape as
/// `graph::db`).
fn hex_lower(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write as _;
    let bytes = bytes.as_ref();
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_lower(Sha256::digest(bytes))
}

/// A field of the common cross-backend contract that could NOT be computed.
///
/// The spec names `source_sha256`, `dependency_sha256` and `limits` as fields
/// every result carries. Omitting one when it cannot be computed would make
/// "not recorded" indistinguishable from "recorded as empty", so an explicit
/// null carrying its reason is emitted instead.
fn unavailable_field(reason: &str) -> serde_json::Value {
    json!({"value": serde_json::Value::Null, "unavailable": true, "reason": reason})
}

/// Names introduced by `postulate` blocks in COMMENT-STRIPPED Agda source.
///
/// Agda's layout rule makes the block everything indented deeper than the
/// `postulate` keyword itself, so the block is delimited by indentation rather
/// than by a terminator token. A line that cannot be split at `:` is recorded
/// verbatim rather than dropped: over-reporting only makes the audit stricter,
/// while dropping an entry would understate the trusted base.
fn agda_postulate_names(stripped: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut block: Option<usize> = None;
    for raw in stripped.lines() {
        let indent = raw.len() - raw.trim_start().len();
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(open_indent) = block {
            if indent > open_indent {
                push_agda_postulate_entry(line, &mut names);
                continue;
            }
            block = None;
        }
        // Word-boundary match: `postulates` is an ordinary identifier.
        let is_keyword = line == "postulate"
            || line
                .strip_prefix("postulate")
                .is_some_and(|rest| rest.starts_with(char::is_whitespace));
        if is_keyword {
            block = Some(indent);
            let inline = line["postulate".len()..].trim();
            if !inline.is_empty() {
                push_agda_postulate_entry(inline, &mut names);
            }
        }
    }
    names
}

fn push_agda_postulate_entry(entry: &str, names: &mut Vec<String>) {
    match entry.split_once(':') {
        Some((lhs, _)) if !lhs.trim().is_empty() => {
            names.extend(lhs.split_whitespace().map(str::to_string));
        }
        // No `:` on this line means a continuation or a shape we do not model.
        // Keep it verbatim so the reader sees exactly what was assumed.
        _ => names.push(entry.to_string()),
    }
}

/// Module names imported by COMMENT-STRIPPED Agda source (`import M`,
/// `open import M ...`).
fn agda_imports(stripped: &str) -> Vec<String> {
    let mut modules = Vec::new();
    for raw in stripped.lines() {
        let mut tokens = raw.split_whitespace().peekable();
        let Some(first) = tokens.next() else { continue };
        let rest = match first {
            "import" => tokens.next(),
            "open" => match tokens.next() {
                Some("import") => tokens.next(),
                _ => None,
            },
            _ => None,
        };
        if let Some(module) = rest {
            let module = module.trim_end_matches(';').to_string();
            if !module.is_empty() && !modules.contains(&module) {
                modules.push(module);
            }
        }
    }
    modules
}

/// Whether an Agda import lies inside the DECLARED project closure.
///
/// The declared closure is `FormalSystem::Agda.default_imports()`, i.e. the
/// builtin modules that ship with the checker we invoke. Under `--safe` those
/// builtins are part of the checker's own trusted base, so they need no separate
/// postulate audit. Anything else is a module this backend has not read, and
/// per the spec an import outside the declared closure fails closed.
fn agda_import_inside_declared_closure(module: &str) -> bool {
    FormalSystem::Agda.default_imports().iter().any(|prefix| {
        module == prefix.as_str() || module.starts_with(&format!("{prefix}."))
    })
}

/// Labels of `$a` axiomatic assertions declared in COMMENT-STRIPPED Metamath
/// source. In Metamath a statement is `label $a ... $.`, so the label is the
/// token immediately before the `$a` keyword.
fn metamath_axiom_labels(stripped: &str) -> Vec<String> {
    let tokens: Vec<&str> = stripped.split_whitespace().collect();
    let mut labels = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if *token != "$a" {
            continue;
        }
        let label = match index.checked_sub(1).and_then(|i| tokens.get(i)) {
            Some(label) if !label.starts_with('$') => (*label).to_string(),
            // An unlabelled `$a` is malformed Metamath, but it still announces
            // an axiom, so it is recorded rather than skipped.
            _ => format!("unlabelled $a at token {index}"),
        };
        labels.push(label);
    }
    labels
}

/// The transitively-resolved `$[ file $]` closure of a Metamath database.
struct MetamathClosure {
    /// One entry per resolved file: name, content hash, and how many `$a`
    /// database axioms it contributes to the trusted base.
    files: Vec<serde_json::Value>,
    /// Includes that could not be read, escaped the workspace, or were cut off
    /// by the cap. Any entry here means the closure is incomplete.
    unresolved: Vec<String>,
    /// Hash over the resolved closure, or `None` when it is incomplete: a hash
    /// of a partial closure would look like a real dependency identity.
    dependency_sha256: Option<String>,
}

/// Walk the include closure of `code` under `root`, transitively and bounded.
///
/// Nested includes are followed because the spec requires the dependency
/// closure to include included files, and a database is free to include another
/// database. Cycles terminate on the visited set; the cap bounds the walk.
fn metamath_include_closure(root: &Path, code: &str) -> MetamathClosure {
    use std::path::Component;
    let mut files = Vec::new();
    let mut unresolved = Vec::new();
    let mut visited: BTreeSet<String> = BTreeSet::new();
    let mut queue: Vec<PathBuf> = metamath_includes(code);
    let mut digest_material: Vec<String> = Vec::new();

    while let Some(include) = queue.pop() {
        let name = include.to_string_lossy().to_string();
        if !visited.insert(name.clone()) {
            continue;
        }
        if visited.len() > METAMATH_INCLUDE_CLOSURE_CAP {
            unresolved.push(format!(
                "{name}: include closure exceeded the {METAMATH_INCLUDE_CLOSURE_CAP}-file cap"
            ));
            continue;
        }
        let escapes = include.is_absolute()
            || include.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            });
        if escapes {
            unresolved.push(format!("{name}: include escapes the workspace"));
            continue;
        }
        let bytes = match std::fs::read(root.join(&include)) {
            Ok(bytes) => bytes,
            Err(error) => {
                unresolved.push(format!("{name}: {error}"));
                continue;
            }
        };
        let sha = sha256_hex(&bytes);
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let stripped = crate::prover::formal::strip_comments(&text);
        let database_axioms = metamath_axiom_labels(stripped.as_str()).len();
        digest_material.push(format!("{name}\u{0}{sha}"));
        files.push(json!({
            "include": name,
            "sha256": sha,
            "database_axioms": database_axioms,
        }));
        queue.extend(metamath_includes(stripped.as_str()));
    }

    digest_material.sort();
    let dependency_sha256 = unresolved
        .is_empty()
        .then(|| sha256_hex(digest_material.join("\n").as_bytes()));
    MetamathClosure {
        files,
        unresolved,
        dependency_sha256,
    }
}

/// Combine a primary re-check verdict with an OPTIONAL secondary checker.
///
/// Returns `(rechecked, disagreement)`. The secondary may corroborate or flag
/// disagreement; it may never substitute for the primary verdict, so it can
/// only ever withhold the re-check, never grant one the primary refused. A
/// disagreement is surfaced rather than silently resolved in favour of whichever
/// checker ran last, because two checkers contradicting each other is exactly
/// the state in which we do not know the proof is good.
fn cross_checked_verdict(primary_passed: bool, secondary_passed: Option<bool>) -> (bool, bool) {
    let disagreement = secondary_passed.is_some_and(|passed| passed != primary_passed);
    (primary_passed && !disagreement, disagreement)
}

/// A fail-closed [`AxiomReport`]: the axiom set is UNKNOWN, so the audit is not
/// clean. `axioms` stays empty because nothing was learned, and `audit_ran` is
/// false so a reader can tell this apart from a genuinely empty axiom set.
fn blocked_audit(
    system: FormalSystem,
    whitelist: &[String],
    reason: &str,
    provenance: serde_json::Value,
    extra: serde_json::Value,
) -> AxiomReport {
    AxiomReport {
        axioms: Vec::new(),
        within_whitelist: false,
        detail: json!({
            "system": system.as_str(),
            "live": true,
            "audit_ran": false,
            "blocked": reason,
            "whitelist": whitelist,
            "provenance": provenance,
            "detail": extra,
        }),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Agda's offline fallback must implement the SAME comment policy as the
    /// online scan: a commented escape hatch passes, a real one still fails.
    /// Crucially, `{-# ... #-}` is a PRAGMA, not a comment, so pragma-based
    /// checks must keep firing.
    #[test]
    fn agda_offline_fallback_matches_comment_policy() {
        assert!(
            !crate::prover::statement_preservation::commented_escape_hatch_is_a_violation(),
            "this test encodes ESCAPE_HATCH_COMMENT_POLICY == CodeOnly"
        );
        let sys = FormalSystem::Agda;
        // Commented-out escape hatches: Agda never acts on them -> clean.
        let commented = "-- postulate absurd : Set\n\
                         {- was: --allow-unsolved-metas, and a {-# COMPILED f g #-} -}\n\
                         thm : Set\nthm = Set\n";
        let report = fallback_source_scan(sys, commented);
        assert!(
            report.clean,
            "commented escape hatch must not gate: {:?}",
            report.findings
        );
        // A REAL postulate in code still fails.
        let real = fallback_source_scan(sys, "postulate absurd : Set\n");
        assert!(!real.clean);
        assert!(real.findings.iter().any(|f| f.starts_with("postulate")));
        // PRAGMAS still fail — they are not comments and are NOT stripped.
        let opts = fallback_source_scan(sys, "{-# OPTIONS --allow-unsolved-metas #-}\nthm : Set\n");
        assert!(!opts.clean, "pragma must not be stripped away");
        assert!(opts
            .findings
            .iter()
            .any(|f| f.starts_with("--allow-unsolved-metas")));
        let compiled = fallback_source_scan(sys, "{-# COMPILED f g #-}\n");
        assert!(!compiled.clean, "pragma must not be stripped away");
        assert!(compiled.findings.iter().any(|f| f.starts_with("{-# COMPILED")));
    }

    /// ALIAS EXPANSION. Every flag here turns a checker off, exactly as
    /// `--allow-unsolved-metas` does, and `primTrustMe` fabricates an equality
    /// proof. Banning one spelling of a move with six other spellings is
    /// protection in name only.
    #[test]
    fn renamed_agda_hatches_are_caught() {
        let sys = FormalSystem::Agda;
        for (code, expected) in [
            ("{-# OPTIONS --type-in-type #-}\nthm : Set\n", "--type-in-type"),
            ("{-# OPTIONS --unsafe #-}\nthm : Set\n", "--unsafe"),
            (
                "{-# OPTIONS --no-termination-check #-}\nthm : Set\n",
                "--no-termination-check",
            ),
            (
                "{-# OPTIONS --no-positivity-check #-}\nthm : Set\n",
                "--no-positivity-check",
            ),
            (
                "{-# OPTIONS --no-coverage-check #-}\nthm : Set\n",
                "--no-coverage-check",
            ),
            ("thm = primTrustMe\n", "primTrustMe"),
        ] {
            let report = fallback_source_scan(sys, code);
            assert!(!report.clean, "alias must be caught: {code:?}");
            assert!(
                report.findings.iter().any(|f| f == expected),
                "expected `{expected}` in {:?}",
                report.findings
            );
        }
    }

    /// The boundary trade-off, asserted in the OVER-matching direction: an
    /// identifier that merely CONTAINS a banned token is ordinary Agda.
    #[test]
    fn identifiers_containing_a_hatch_token_are_not_flagged() {
        let sys = FormalSystem::Agda;
        for code in [
            "postulates : Set\npostulates = Set\n",
            "{-# OPTIONS --safe #-}\nthm : Set\nthm = Set\n",
        ] {
            let report = fallback_source_scan(sys, code);
            assert!(
                report.clean,
                "innocent source must not be flagged ({code:?}): {:?}",
                report.findings
            );
        }
    }

    /// Metamath's offline fallback under the same policy. `$( ... $)` is
    /// already handled by `strip_comments`.
    #[test]
    fn metamath_offline_fallback_matches_comment_policy() {
        let sys = FormalSystem::Metamath;
        let commented = "$( an old draft: badax $a |- ph $. and a ? step $)\n\
                         mp2 $p |- ph $= wph wps mp1 mp3 $.\n";
        let report = fallback_source_scan(sys, commented);
        assert!(
            report.clean,
            "commented escape hatch must not gate: {:?}",
            report.findings
        );
        // Real ones still fail.
        let real = fallback_source_scan(sys, "badax $a |- ph $.\n");
        assert!(!real.clean);
        let incomplete = fallback_source_scan(sys, "foo $p wff ph $= ? $.\n");
        assert!(!incomplete.clean);
    }

    #[test]
    fn agda_interaction_parser_extracts_structured_goals_and_warnings() {
        let output = r#"JSON> {"kind":"DisplayInfo","info":{"kind":"AllGoalsWarnings","visibleGoals":[{"constraintObj":{"id":7,"range":[{"start":{"line":3,"col":5}}]},"kind":"OfType","type":"Nat"}],"invisibleGoals":[{"constraintObj":{"name":"_8","range":[]},"kind":"OfType","type":"Bool"}],"warnings":[{"kind":"UnsolvedMetas","message":"Unsolved metas"}]}}"#;
        let AgdaInteractionDiagnostics::Ready {
            records,
            goals,
            errors,
            warnings,
        } = parse_agda_interaction_output(output)
        else {
            panic!("valid AllGoalsWarnings output must parse")
        };
        assert_eq!(records, 1);
        assert_eq!(goals.len(), 2);
        assert_eq!(goals[0]["visibility"], "visible");
        assert_eq!(goals[0]["id"], 7);
        assert_eq!(goals[0]["type"], "Nat");
        assert_eq!(goals[1]["visibility"], "invisible");
        assert_eq!(goals[1]["id"], "_8");
        assert!(errors.is_empty());
        assert_eq!(warnings[0]["kind"], "UnsolvedMetas");
    }

    #[test]
    fn agda_interaction_parser_extracts_structured_errors() {
        let output = r#"{"kind":"DisplayInfo","info":{"kind":"Error","error":{"message":"Not in scope: x","range":[{"start":{"line":4,"col":2}}]}}}"#;
        let AgdaInteractionDiagnostics::Ready { errors, goals, .. } =
            parse_agda_interaction_output(output)
        else {
            panic!("a structured Agda error is a well-formed diagnostic response")
        };
        assert!(goals.is_empty());
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["message"], "Not in scope: x");
        assert_eq!(errors[0]["range"][0]["start"]["line"], 4);
    }

    #[test]
    fn agda_interaction_parser_fails_closed_on_malformed_output() {
        for output in [
            "JSON> {not-json}",
            r#"{"kind":"DisplayInfo","info":{"kind":"AllGoalsWarnings","visibleGoals":[],"invisibleGoals":[]}}"#,
            "JSON> {\"kind\":\"DisplayInfo\",\"info\":{\"kind\":\"AllGoalsWarnings\",\"visibleGoals\":[],\"invisibleGoals\":[],\"warnings\":[]}}\nnot-json",
        ] {
            let parsed = parse_agda_interaction_output(output);
            assert!(
                matches!(parsed, AgdaInteractionDiagnostics::Malformed { .. }),
                "malformed interaction output must expose no partial diagnostics: {parsed:?}"
            );
            let detail = parsed.to_json();
            assert_eq!(detail["status"], "malformed");
            assert_eq!(detail["fail_closed"], true);
            assert_eq!(detail["goals"], json!([]));
            assert_eq!(detail["errors"], json!([]));
        }
    }

    #[test]
    fn agda_interaction_parser_treats_unsupported_protocol_as_optional() {
        for output in [
            "agda: unrecognized option --interaction-json",
            r#"{"kind":"Status","showImplicitArguments":false}"#,
            "",
        ] {
            assert!(matches!(
                parse_agda_interaction_output(output),
                AgdaInteractionDiagnostics::Unsupported { .. }
            ));
        }
    }

    #[test]
    fn agda_interaction_diagnostics_never_trust_exit_status() {
        let stdout = r#"{"kind":"DisplayInfo","info":{"kind":"AllGoalsWarnings","visibleGoals":[],"invisibleGoals":[],"warnings":[]}}"#;
        let outcome = |code| exec::ExecOutcome {
            launched: true,
            code: Some(code),
            stdout: stdout.into(),
            stderr: String::new(),
            timed_out: false,
            output_capped: false,
        };
        assert_eq!(
            parse_agda_interaction_outcome(&outcome(0)),
            parse_agda_interaction_outcome(&outcome(42)),
            "interaction exit status must not participate in diagnostic parsing"
        );
    }

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
        assert!(!sig.is_pass(
            true,
            true,
            "?Error on line 5: ... proof does not verify.",
            ""
        ));
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
        assert!(!sig.is_pass(
            false,
            false,
            "All proofs in the database were verified.",
            ""
        ));
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
        let findings =
            metamath_source_findings("$[ set.mm $]\nmp2 $p |- ph $= wph wps mp1 mp3 $.\n");
        assert!(
            findings.is_empty(),
            "clean proof must not flag: {findings:?}"
        );
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
            assert!(
                backend
                    .source_scan("$[ set.mm $]\nt $p |- ph $= a b c $.\n")
                    .unwrap()
                    .clean
            );
        }
    }

    // GAP 2 — a MOCK (toolchain-absent) check must NEVER be a LIVE certification
    // (audit invariant #2). Downstream only grants `FormallyVerified` when both
    // `report.lexically_verified && report.live` hold (agent.rs), so the mock
    // report carrying `live == false` is what keeps mock proofs out of it.

    // GAP 3: the goal-context enrichment. `Cmd_load` yields goal TYPES only;
    // the hypotheses at each hole require a goal-specific command. Everything
    // below stays inside the advisory diagnostics: none of it may move the
    // verdict, which comes from the batch `agda --safe` run.

    /// One `AllGoalsWarnings` reply with two real interaction points (7, 9) and
    /// one unsolved meta identified by name.
    fn load_phase_diagnostics() -> AgdaInteractionDiagnostics {
        let output = r#"{"kind":"DisplayInfo","info":{"kind":"AllGoalsWarnings","visibleGoals":[{"constraintObj":{"id":7,"range":[]},"kind":"OfType","type":"Nat"},{"constraintObj":{"id":9,"range":[]},"kind":"OfType","type":"Bool"}],"invisibleGoals":[{"constraintObj":{"name":"_8","range":[]},"kind":"OfType","type":"Set"}],"warnings":[]}}"#;
        parse_agda_interaction_output(output)
    }

    fn load_phase_goals() -> Vec<serde_json::Value> {
        let AgdaInteractionDiagnostics::Ready { goals, .. } = load_phase_diagnostics() else {
            panic!("fixture must parse")
        };
        goals
    }

    #[test]
    fn agda_hole_ids_come_from_the_parsed_load_reply() {
        // Ids are read off Agda's own reply, not regexed out of the source, and
        // named (invisible) metas are not addressable interaction points.
        let (ids, truncated) = agda_interaction_point_ids(&load_phase_diagnostics(), 16);
        assert_eq!(ids, vec![7, 9]);
        assert_eq!(truncated, 0);
        let request = agda_goal_context_request("\"Generated.agda\"", &ids);
        assert!(request.starts_with(
            "IOTCM \"Generated.agda\" None Direct (Cmd_load \"Generated.agda\" [])\n"
        ));
        assert!(request.contains(
            "IOTCM \"Generated.agda\" None Direct (Cmd_goal_type_context_infer Normalised 7 noRange \"?\")\n"
        ));
        assert!(request.contains(
            "IOTCM \"Generated.agda\" None Direct (Cmd_goal_type_context_infer Normalised 9 noRange \"?\")\n"
        ));
        // One process, one stream: every hole is a line, never a new process.
        assert!(request.ends_with("x\n"));
        assert_eq!(request.lines().count(), 2 + ids.len());
    }

    #[test]
    fn agda_hole_cap_truncates_visibly() {
        // A file with a hundred holes must not be able to stretch the gate.
        let visible = (0..100)
            .map(|id| {
                format!(r#"{{"constraintObj":{{"id":{id},"range":[]}},"kind":"OfType","type":"Nat"}}"#)
            })
            .collect::<Vec<_>>()
            .join(",");
        let output = format!(
            r#"{{"kind":"DisplayInfo","info":{{"kind":"AllGoalsWarnings","visibleGoals":[{visible}],"invisibleGoals":[],"warnings":[]}}}}"#
        );
        let parsed = parse_agda_interaction_output(&output);
        let (ids, truncated) = agda_interaction_point_ids(&parsed, AGDA_GOAL_CONTEXT_HOLE_CAP);
        assert_eq!(ids.len(), AGDA_GOAL_CONTEXT_HOLE_CAP);
        assert_eq!(truncated, 100 - AGDA_GOAL_CONTEXT_HOLE_CAP);
        assert!(truncated > 0, "the cap must be reported, not hidden");
    }

    #[test]
    fn agda_goal_specific_reply_supplies_the_hole_context() {
        // This is the reply the load phase can never produce: hypotheses.
        let reply = parse_agda_interaction_output(
            r#"{"kind":"DisplayInfo","info":{"kind":"GoalSpecific","interactionPoint":{"id":7,"range":[]},"goalInfo":{"kind":"GoalType","type":"Nat","entries":[{"originalName":"n","reifiedName":"n","binding":"Nat","inScope":true}]}}}"#,
        );
        let mut goals = load_phase_goals();
        let (merged, unmatched) = merge_agda_goal_contexts(&mut goals, &reply);
        assert_eq!((merged, unmatched), (1, 0));
        assert_eq!(goals[0]["id"], 7);
        assert_eq!(goals[0]["context"][0]["binding"], "Nat");
        // Purely additive: the goal that was not asked about is untouched.
        assert!(goals[1].get("context").is_none());
        // And the original fields survive verbatim.
        assert_eq!(goals[0]["type"], "Nat");
        assert_eq!(goals[0]["visibility"], "visible");
    }

    #[test]
    fn agda_goal_context_degrades_silently() {
        // A malformed reply, an Agda that never heard of the command, and an
        // absent toolchain must each leave the load diagnostics untouched.
        let baseline = load_phase_goals();
        for reply in [
            parse_agda_interaction_output("JSON> {not-json}"),
            parse_agda_interaction_output("agda: unrecognized option --interaction-json"),
            parse_agda_interaction_output(""),
            parse_agda_interaction_outcome(&exec::ExecOutcome {
                launched: false,
                code: None,
                stdout: String::new(),
                stderr: "no such file or directory".into(),
                timed_out: false,
                output_capped: false,
            }),
            parse_agda_interaction_outcome(&exec::ExecOutcome {
                launched: true,
                code: None,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: true,
                output_capped: false,
            }),
        ] {
            let mut goals = baseline.clone();
            let (merged, unmatched) = merge_agda_goal_contexts(&mut goals, &reply);
            assert_eq!((merged, unmatched), (0, 0));
            assert_eq!(
                goals, baseline,
                "a failed enrichment must not perturb the load diagnostics: {reply:?}"
            );
            // Nothing addressable is derived from a failed reply either.
            assert_eq!(
                agda_interaction_point_ids(&reply, AGDA_GOAL_CONTEXT_HOLE_CAP),
                (Vec::<u64>::new(), 0usize)
            );
        }
    }

    #[test]
    fn agda_goal_context_never_participates_in_the_verdict() {
        // The verdict is a function of the BATCH run alone: (launched, exit,
        // stdout, stderr) of `agda --safe`. The enrichment contributes no
        // argument to it, so it is byte-identical whatever the enrichment did.
        let signal = ExternalBackend::new(&Config::default(), FormalSystem::Agda, true)
            .compile_success_signal();
        let verdict = |stdout: &str, code_zero: bool| {
            signal.is_pass(true, code_zero, stdout, "")
        };
        let pass = verdict("", true);
        let fail = verdict("Generated.agda:3,1-4: error", false);
        assert!(pass && !fail, "batch --safe decides pass/fail on its own");

        // Run every enrichment outcome against the same batch result and check
        // the verdict is recomputed identically each time.
        let enrichments = [
            parse_agda_interaction_output(
                r#"{"kind":"DisplayInfo","info":{"kind":"GoalSpecific","interactionPoint":{"id":7,"range":[]},"goalInfo":{"kind":"GoalType","type":"Nat","entries":[{"binding":"Nat"}]}}}"#,
            ),
            parse_agda_interaction_output("JSON> {not-json}"),
            parse_agda_interaction_output(""),
        ];
        for enrichment in &enrichments {
            let mut goals = load_phase_goals();
            merge_agda_goal_contexts(&mut goals, enrichment);
            assert_eq!(verdict("", true), pass);
            assert_eq!(verdict("Generated.agda:3,1-4: error", false), fail);
        }

        // Interaction output that would LOOK like success cannot rescue a
        // failing batch run, which is the exit-0 trap the split design avoids.
        assert!(!verdict("Generated.agda:3,1-4: error", false));
    }

    #[test]
    fn agda_goal_context_is_absent_by_default() {
        // With no Agda on the box the backend is mocked, and a mock compile
        // carries no interaction diagnostics at all: the enrichment cannot
        // change a run on a machine that has no toolchain.
        let cfg = Config::default();
        let backend = ExternalBackend::new(&cfg, FormalSystem::Agda, true);
        let ws = backend
            .scaffold(&cfg, "module Generated where\n", "Generated")
            .expect("mock scaffold");
        let report = backend.compile(&ws).expect("mock compile");
        assert!(report.compiled);
        assert!(
            report.detail.get("interaction_diagnostics").is_none(),
            "mock compile must not reach the interaction path: {:?}",
            report.detail
        );
        assert!(report.detail.get("goal_context_enrichment").is_none());
    }

    // --- Layer 2: the axiom audit ----------------------------------------
    //
    // This layer used to return `within_whitelist: true` unconditionally for
    // both Agda and Metamath, so it presented as a gate while gating nothing.

    /// A backend pointed at a binary that does not exist cannot audit anything,
    /// so it must fail CLOSED and say why, never assert cleanliness.
    #[test]
    fn audit_fails_closed_when_it_cannot_run() {
        for system in [FormalSystem::Agda, FormalSystem::Metamath] {
            let backend = ExternalBackend {
                system,
                mock: false,
                runner: Runner::Native,
                binary: "theoremata-no-such-checker".into(),
                secondary_binary: None,
            };
            let ws = Workspace {
                system,
                root: PathBuf::from("."),
                source_path: PathBuf::from(format!("Generated{}", system.source_extension())),
                entry: "Generated".into(),
            };
            let report = backend
                .audit_axioms(&ws, "Generated", &system.axiom_whitelist())
                .expect("a blocked audit is a report, not an error");
            assert!(
                !report.within_whitelist,
                "an audit that could not run must not claim cleanliness: {report:?}"
            );
            assert_eq!(report.detail["audit_ran"], false);
            assert!(report.detail["blocked"].is_string());
            assert!(report.axioms.is_empty(), "nothing was learned");
        }
    }

    /// The mock audit stays clean (offline scaffolding) but must be marked as
    /// not-live and not-run, so it is distinguishable from a real clean audit.
    #[test]
    fn mock_audit_is_marked_as_not_live_and_not_run() {
        let cfg = Config::default();
        for system in [FormalSystem::Agda, FormalSystem::Metamath] {
            let backend = ExternalBackend::new(&cfg, system, true);
            let ws = backend.scaffold(&cfg, "", "Generated").unwrap();
            let report = backend
                .audit_axioms(&ws, "Generated", &system.axiom_whitelist())
                .unwrap();
            assert!(report.within_whitelist);
            assert_eq!(report.detail["live"], false);
            assert_eq!(report.detail["audit_ran"], false);
            assert_eq!(report.detail["mock"], true);
        }
    }

    #[test]
    fn agda_postulates_and_imports_are_read_off_the_source() {
        let code = "module Generated where\n\
                    open import Agda.Builtin.Unit\n\
                    open import Untrusted.Module\n\
                    postulate\n  \
                      bad : Set\n  \
                      worse also : Set\n\
                    thm : Set\nthm = Set\n";
        let stripped = crate::prover::formal::strip_comments(code);
        let names = agda_postulate_names(stripped.as_str());
        assert_eq!(names, vec!["bad", "worse", "also"]);
        // The block ends at the dedent: `thm` is a declaration, not an axiom.
        assert!(!names.iter().any(|name| name.starts_with("thm")));
        let imports = agda_imports(stripped.as_str());
        assert_eq!(imports, vec!["Agda.Builtin.Unit", "Untrusted.Module"]);
        assert!(agda_import_inside_declared_closure("Agda.Builtin.Unit"));
        assert!(!agda_import_inside_declared_closure("Untrusted.Module"));
        // A commented-out postulate is never seen by the checker.
        let commented =
            crate::prover::formal::strip_comments("-- postulate ghost : Set\nthm : Set\n");
        assert!(agda_postulate_names(commented.as_str()).is_empty());
        // `postulates` is an ordinary identifier, not the keyword.
        let identifier =
            crate::prover::formal::strip_comments("postulates : Set\npostulates = Set\n");
        assert!(agda_postulate_names(identifier.as_str()).is_empty());
    }

    #[test]
    fn metamath_axiom_labels_are_read_off_the_source() {
        let stripped = crate::prover::formal::strip_comments(
            "$( a comment with badax $a $)\n$c wff $.\nid $a |- ph $.\nth $p |- ph $= wph id $.\n",
        );
        assert_eq!(metamath_axiom_labels(stripped.as_str()), vec!["id"]);
    }

    /// An unresolvable include leaves the dependency closure INCOMPLETE, which
    /// the spec calls a failure, so no dependency hash is produced for it.
    #[test]
    fn metamath_include_closure_fails_closed_on_a_missing_include() {
        let tmp = tempfile::tempdir().unwrap();
        let closure = metamath_include_closure(tmp.path(), "$[ absent.mm $]\n");
        assert!(closure.dependency_sha256.is_none());
        assert_eq!(closure.unresolved.len(), 1);
        assert!(closure.files.is_empty());

        // A resolvable closure hashes, and nested includes are followed.
        std::fs::write(tmp.path().join("base.mm"), "ax1 $a |- ph $.\n").unwrap();
        std::fs::write(tmp.path().join("mid.mm"), "$[ base.mm $]\nax2 $a |- ps $.\n").unwrap();
        let closure = metamath_include_closure(tmp.path(), "$[ mid.mm $]\n");
        assert!(closure.unresolved.is_empty());
        assert_eq!(closure.files.len(), 2, "nested include must be walked");
        assert!(closure.dependency_sha256.is_some());
        // Database axioms are RECORDED, not rejected.
        assert!(closure
            .files
            .iter()
            .all(|file| file["database_axioms"] == 1));
        // An escaping include is unresolved rather than followed.
        let escaping = metamath_include_closure(tmp.path(), "$[ ../evil.mm $]\n");
        assert!(escaping.dependency_sha256.is_none());
        assert!(escaping.unresolved[0].contains("escapes"));
    }

    // --- Layer 2b: the kernel re-check ------------------------------------

    /// The exit code is not the signal for Metamath: the binary exits 0 on a
    /// FAILED `verify proof *`. The re-check must use the same declared success
    /// signal as `compile`, which is what this asserts.
    #[test]
    fn metamath_recheck_rejects_exit_zero_with_a_failure_sentinel() {
        let signal = ExternalBackend::new(&Config::default(), FormalSystem::Metamath, true)
            .compile_success_signal();
        // Exit 0 (`exit_success == true`) plus a failure sentinel: NOT a pass.
        assert!(!signal.is_pass(true, true, "?Error on line 5: proof does not verify.", ""));
        // Exit 0 with no sentinel at all is likewise not a pass.
        assert!(!signal.is_pass(true, true, "", ""));
        // Only the explicit success sentinel passes.
        assert!(signal.is_pass(true, true, "All proofs in the database were verified in 0.01 s.", ""));
        // And that verdict, not the exit code, is what the re-check reports.
        assert_eq!(cross_checked_verdict(false, None), (false, false));
    }

    /// A secondary checker may corroborate or flag disagreement; it may never
    /// substitute for the primary verdict.
    #[test]
    fn secondary_checker_never_overwrites_the_primary_verdict() {
        // Corroboration.
        assert_eq!(cross_checked_verdict(true, Some(true)), (true, false));
        assert_eq!(cross_checked_verdict(false, Some(false)), (false, false));
        // The secondary cannot grant a pass the primary refused.
        assert_eq!(
            cross_checked_verdict(false, Some(true)),
            (false, true),
            "a secondary pass must not overwrite a primary failure"
        );
        // Nor can it be silently resolved away when it contradicts a pass.
        assert_eq!(
            cross_checked_verdict(true, Some(false)),
            (false, true),
            "a disagreement must surface and withhold the re-check"
        );
        // With no secondary configured there is nothing to disagree with.
        assert_eq!(cross_checked_verdict(true, None), (true, false));
    }

    // --- The common cross-backend provenance contract ----------------------

    #[test]
    fn provenance_carries_the_common_contract_fields() {
        let cfg = Config::default();
        for system in [FormalSystem::Agda, FormalSystem::Metamath] {
            let backend = ExternalBackend::new(&cfg, system, true);
            let ws = backend.scaffold(&cfg, "", "Generated").unwrap();
            let provenance = backend.provenance(Some(&ws));
            assert_eq!(provenance["system"], system.as_str());
            assert!(provenance["checker"]["command"].is_array());
            assert_eq!(
                provenance["limits"]["timeout_seconds"],
                json!(exec::ResourceLimits::from_env().timeout.as_secs())
            );
            // Fields that cannot be computed are explicit nulls WITH a reason,
            // so "not recorded" never reads as "recorded as empty".
            for field in ["source_sha256", "dependency_sha256"] {
                let value = &provenance[field];
                assert!(
                    value.is_string() || value["unavailable"] == true,
                    "{field} must be a hash or an explicit null-with-reason: {value:?}"
                );
                if value["unavailable"] == true {
                    assert!(
                        value["reason"].as_str().is_some_and(|r| !r.is_empty()),
                        "{field} must say WHY it is unavailable"
                    );
                    assert!(value["value"].is_null());
                }
            }
            assert_eq!(provenance["checker"]["version"]["unavailable"], true);
        }
    }

    /// A real source hash is computed when the source exists, and it is the
    /// hash of the exact bytes the checker was given.
    #[test]
    fn provenance_hashes_the_submitted_source() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("Generated.mm");
        let code = "$c wff $.\n";
        std::fs::write(&source, code).unwrap();
        let backend = ExternalBackend {
            system: FormalSystem::Metamath,
            mock: false,
            runner: Runner::Native,
            binary: "theoremata-no-such-checker".into(),
            secondary_binary: None,
        };
        let ws = Workspace {
            system: FormalSystem::Metamath,
            root: tmp.path().to_path_buf(),
            source_path: source,
            entry: "Generated".into(),
        };
        let provenance = backend.provenance(Some(&ws));
        assert_eq!(provenance["source_sha256"], json!(sha256_hex(code.as_bytes())));
        // No includes: the closure is complete and empty, so it still hashes.
        assert!(provenance["dependency_sha256"].is_string());
    }

    /// TRANSITIVE IMPORTED POSTULATES (spec, Agda section). A postulate in an
    /// imported module is as much an axiom as a local one, but resolving it
    /// needs an Agda module-graph walk (library roots, `.agda-lib` resolution,
    /// interface files) that this whole-file backend does not have. Rather than
    /// half-build it, the audit fails CLOSED on any import outside the declared
    /// closure and records the reason in the report.
    #[test]
    fn agda_transitive_postulates_fail_closed_pending_a_module_graph_walk() {
        let stripped = crate::prover::formal::strip_comments(
            "module Generated where\nopen import Some.Untrusted.Lib\nthm : Set\nthm = Set\n",
        );
        let imports = agda_imports(stripped.as_str());
        assert!(agda_postulate_names(stripped.as_str()).is_empty());
        assert!(
            imports
                .iter()
                .any(|module| !agda_import_inside_declared_closure(module)),
            "an unresolvable import must be visible to the audit"
        );
    }

    #[test]
    fn mock_async_verification_is_not_live() {
        let cfg = crate::config::Config::default();
        let backend = ExternalBackend::new(&cfg, FormalSystem::Metamath, true);
        let report = backend
            .verify(
                &cfg,
                "$c wff |- $.\n$v ph $.\nph $f wff ph $.\n",
                "some statement",
            )
            .expect("mock verify should not error");
        assert!(
            !report.live,
            "a mock verification must never be a live certification"
        );
    }
}
