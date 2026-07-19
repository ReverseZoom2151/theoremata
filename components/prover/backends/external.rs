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
use std::{
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
            detail: json!({"runner": self.runner.tag(), "binary": self.binary.clone(), "command": command, "code": out.code, "verified": verified, "stdout": out.stdout, "stderr": out.stderr, "interaction_diagnostics": interaction_diagnostics}),
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
        let filename = ws
            .source_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("Generated");
        let command = self.command(filename);
        let mut secondary = json!(null);
        let mut rechecked = out.success();
        if rechecked {
            if let (FormalSystem::Metamath, Some(binary)) = (self.system, &self.secondary_binary) {
                let filename = ws
                    .source_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Generated");
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
        // turning a checker off — so banning `--allow-unsolved-metas` while
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
