//! The non-blocking event bus and its worker threads.
//!
//! WHY this module is the load-bearing change: the old cockpit called the model
//! ON the UI thread, so a multi-minute local turn froze every key. Here a turn
//! (or a single action) runs on a `std::thread::spawn` worker that talks to the
//! main loop ONLY through an `mpsc` channel of [`UiEvent`]s. The UI thread keeps
//! polling keys and drawing while the worker runs, so Esc/Ctrl-C are live the
//! whole time, not just between action rounds.
//!
//! The worker never shares the main thread's `Store` or provider. It opens its
//! OWN `Store` from the same database path and builds its OWN provider from the
//! same `config.model_command`, and a `Config` clone is moved in. Actions are
//! serialized (one worker in flight at a time, enforced by the main loop), so
//! two SQLite connections never race on a write. This also sidesteps any
//! `Send` requirement on `&dyn ModelProvider`: the worker constructs a fresh,
//! owned provider rather than moving a borrowed one across the boundary.
//!
//! Soundness note (mirrors the engine's own invariant): nothing here can mark a
//! node formally verified. `execute_action` runs REAL functions; `prove`/
//! `hammer` return a report and the honest [`super::cell::verdict_cell`] renders
//! it (a mock or unpreserved result is yellow, never a green check). A failed
//! action becomes a visible error cell, never a false success.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;

use serde_json::{json, Value};

use crate::{
    agent,
    chat::{ChatAction, ChatEngine},
    config::Config,
    db::Store,
    formal::{self, FormalSystem},
    formal_generate,
    model::ModelStreamEvent,
    provider::{CommandProvider, ModelProvider, OfflineProvider},
    tools::{PythonCheck, Tool},
};

use super::cell::{self, Cell};

/// Cap on how many action-execution rounds one natural-language turn may drive.
/// A single model call is minutes against a local 35B and an action may itself
/// call the model, so an uncapped loop would hang for a very long time. The cap
/// plus the mid-turn cancel flag are the whole safety net for the loop.
pub const MAX_ACTION_ROUNDS: usize = 4;

/// Messages the workers post to the main loop. The main loop drains these with
/// `try_recv` every tick and never blocks on them.
pub enum UiEvent {
    /// A model token delta; the main loop appends it to the in-flight streamed
    /// reply preview (the "active" cell).
    StreamDelta(String),
    /// A status/footer update (e.g. "working (prove)").
    Progress(String),
    /// A finished transcript cell to commit (an agent reply, a notice). The
    /// main loop discards the streamed preview before pushing it.
    Cell(Cell),
    /// A finished tool/action result cell (verdict/falsify/sweep/agent_run).
    /// Treated exactly like `Cell` by the main loop; kept as a distinct variant
    /// so the intent (a tool result vs a chat reply) stays legible at the seam.
    ToolCell(Cell),
    /// The whole turn (possibly multi-round) is done; the UI returns to idle.
    TurnDone,
    /// The worker failed. The main loop renders a visible error cell and returns
    /// to idle: a worker error is NEVER a silent or false success.
    Failed(String),
}

/// The owned inputs a worker needs. Everything here is `Send` and owned, so
/// nothing borrowed from the main thread crosses the thread boundary.
pub struct WorkerInputs {
    pub db_path: PathBuf,
    pub config: Config,
    pub project_id: String,
    pub tx: Sender<UiEvent>,
    /// Set by the main loop when the user hits Esc; the worker checks it between
    /// rounds (and stops forwarding stream deltas once set) so a long turn can
    /// be interrupted without waiting for the cap.
    pub cancel: Arc<AtomicBool>,
}

/// Build the same provider the top-level binary builds, but owned by the worker.
fn build_provider(config: &Config) -> Box<dyn ModelProvider> {
    match &config.model_command {
        Some(command) => Box::new(CommandProvider::new(command)),
        None => Box::new(OfflineProvider),
    }
}

/// Open the worker's own store, or report a visible failure and give up.
fn open_store(db_path: &PathBuf, tx: &Sender<UiEvent>) -> Option<Store> {
    match Store::open(db_path) {
        Ok(store) => Some(store),
        Err(e) => {
            let _ = tx.send(UiEvent::Failed(format!("could not open store: {e}")));
            None
        }
    }
}

/// Spawn the agentic natural-language loop on a worker thread.
///
/// This is the migration of the old `agentic_turn`: the model replies (streamed
/// as deltas), may file graph proposals, AND may request closed-set actions.
/// Each action runs, its compact result is appended as a `tool` message, then
/// the model is called again so it can react. Capped at [`MAX_ACTION_ROUNDS`]
/// and interruptible between rounds via the cancel flag.
pub fn spawn_chat(inputs: WorkerInputs, task: String) {
    thread::spawn(move || {
        let WorkerInputs {
            db_path,
            config,
            project_id,
            tx,
            cancel,
        } = inputs;
        let Some(store) = open_store(&db_path, &tx) else {
            let _ = tx.send(UiEvent::TurnDone);
            return;
        };
        let provider = build_provider(&config);
        let engine = ChatEngine {
            store: &store,
            provider: provider.as_ref(),
        };

        let mut task = task;
        let mut record_user = true;
        let mut round = 0usize;
        loop {
            if cancel.load(Ordering::Relaxed) {
                let _ = tx.send(UiEvent::Progress("interrupted; returned to input".into()));
                break;
            }
            // Run one model turn, streaming deltas to the UI. Once cancelled we
            // stop forwarding deltas so the preview freezes immediately.
            let turn = {
                let txc = tx.clone();
                let cancelc = cancel.clone();
                let mut on_event = |ev: ModelStreamEvent| {
                    if cancelc.load(Ordering::Relaxed) {
                        return;
                    }
                    if let ModelStreamEvent::Delta { text } = ev {
                        let _ = txc.send(UiEvent::StreamDelta(text));
                    }
                };
                engine.send_turn(&project_id, &task, record_user, &mut on_event)
            };
            let turn = match turn {
                Ok(t) => t,
                Err(e) => {
                    let _ = tx.send(UiEvent::Failed(format!("agent error: {e}")));
                    return;
                }
            };
            record_user = false;

            // Commit the reply as its own cell. Sending a committed cell tells
            // the main loop to discard the streamed preview, so the text is not
            // doubled: the preview was only a live view of this same reply.
            let _ = tx.send(UiEvent::Cell(cell::agent_cell(&turn.reply)));
            if turn.proposals > 0 {
                let _ = tx.send(UiEvent::Cell(cell::notice_cell(&format!(
                    "{} graph mutation proposal(s) awaiting /approve",
                    turn.proposals
                ))));
            }

            if turn.actions.is_empty() {
                let _ = tx.send(UiEvent::TurnDone);
                return;
            }
            round += 1;
            for action in &turn.actions {
                let _ = tx.send(UiEvent::Progress(format!(
                    "working ({})",
                    action.tool_name()
                )));
                let outcome =
                    execute_action(&store, &config, provider.as_ref(), &project_id, action);
                // Persist a compact tool message so the next model turn can react
                // to the result without re-running the action.
                let _ = store.add_message(
                    &project_id,
                    "tool",
                    &outcome.summary,
                    json!({"tool": action.tool_name(), "ok": outcome.ok}),
                );
                let _ = tx.send(UiEvent::ToolCell(outcome.cell));
            }
            if round >= MAX_ACTION_ROUNDS {
                let _ = tx.send(UiEvent::Progress(format!(
                    "action round cap ({MAX_ACTION_ROUNDS}) reached; stopping"
                )));
                let _ = tx.send(UiEvent::TurnDone);
                return;
            }
            if cancel.load(Ordering::Relaxed) {
                let _ = tx.send(UiEvent::Progress("interrupted; returned to input".into()));
                let _ = tx.send(UiEvent::TurnDone);
                return;
            }
            task = "Continue: react to the tool results now in the conversation. \
                    Request more actions only if they are needed."
                .to_string();
        }
        let _ = tx.send(UiEvent::TurnDone);
    });
}

/// Spawn a single closed-set action (`/prove`, `/hammer`, `/falsify`, `/sweep`)
/// on a worker thread. Unlike the chat loop this does not persist a `tool`
/// message: a direct slash action is a one-shot inspection, matching the old
/// `run_blocking` behaviour that showed output without writing to the
/// conversation.
pub fn spawn_action(inputs: WorkerInputs, action: ChatAction) {
    thread::spawn(move || {
        let WorkerInputs {
            db_path,
            config,
            project_id,
            tx,
            cancel,
        } = inputs;
        let Some(store) = open_store(&db_path, &tx) else {
            let _ = tx.send(UiEvent::TurnDone);
            return;
        };
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(UiEvent::TurnDone);
            return;
        }
        let provider = build_provider(&config);
        let outcome = execute_action(&store, &config, provider.as_ref(), &project_id, &action);
        let _ = tx.send(UiEvent::ToolCell(outcome.cell));
        let _ = tx.send(UiEvent::TurnDone);
    });
}

/// Spawn the autonomous agent loop (the CLI `Agent` path) on a worker thread.
pub fn spawn_agent(inputs: WorkerInputs) {
    thread::spawn(move || {
        let WorkerInputs {
            db_path,
            config,
            project_id,
            tx,
            cancel,
        } = inputs;
        let Some(store) = open_store(&db_path, &tx) else {
            let _ = tx.send(UiEvent::TurnDone);
            return;
        };
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(UiEvent::TurnDone);
            return;
        }
        let provider = build_provider(&config);
        let outcome = run_agent(&store, &config, provider.as_ref(), &project_id);
        let _ = tx.send(UiEvent::ToolCell(outcome.cell));
        let _ = tx.send(UiEvent::TurnDone);
    });
}

/// The result of running one action: a success flag, a compact one-line summary
/// (used for the `tool` conversation message), and the honest transcript cell.
struct ActionOutcome {
    ok: bool,
    summary: String,
    cell: Cell,
}

impl ActionOutcome {
    /// A failed action: fail closed with a visible error cell, never a false
    /// success. The summary and the cell carry the same message.
    fn error(message: String) -> Self {
        let cell = cell::error_cell(&message);
        ActionOutcome {
            ok: false,
            summary: message,
            cell,
        }
    }
}

/// Execute one closed-set [`ChatAction`] against the REAL functions. Returns an
/// [`ActionOutcome`]; NEVER panics or bubbles an error out. This is the single
/// place the cockpit maps the closed enum to a concrete function; no string from
/// the model is ever run as a command, and the cell is built from the real
/// report fields so a mock or unpreserved result cannot render green.
fn execute_action(
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    project_id: &str,
    action: &ChatAction,
) -> ActionOutcome {
    match action {
        ChatAction::Prove { system, target } => {
            let sys = match system.parse::<FormalSystem>() {
                Ok(s) => s,
                Err(e) => return ActionOutcome::error(format!("prove: unknown system: {e}")),
            };
            let statement = resolve_prove_target(store, project_id, target);
            match formal_generate::generate_and_verify(store, config, provider, sys, &statement) {
                Ok((code, report)) => {
                    let cell = cell::verdict_cell(
                        sys.as_str(),
                        &code,
                        report.lexically_verified,
                        report.axioms_clean,
                        report.statement_preserved,
                        report.live,
                    );
                    let summary = format!(
                        "prove [{}] {statement}: compiled={} axioms_clean={} preserved={} live={}",
                        sys.as_str(),
                        report.lexically_verified,
                        report.axioms_clean,
                        report.statement_preserved,
                        report.live,
                    );
                    ActionOutcome {
                        ok: report.lexically_verified,
                        summary,
                        cell,
                    }
                }
                Err(e) => ActionOutcome::error(format!("prove error: {e}")),
            }
        }
        ChatAction::Hammer { system, goal } => {
            let sys = match system.parse::<FormalSystem>() {
                Ok(s) => s,
                Err(e) => return ActionOutcome::error(format!("hammer: unknown system: {e}")),
            };
            // Mirror the CLI HammerProve handler: find a tactic, assemble a
            // native proof, then verify it through the same gate.
            match formal_generate::hammer_prove(config, sys, goal) {
                Some(code) => {
                    let live = formal::backend_for(config, sys, false);
                    let used_live = !config.prover_mock && live.available();
                    let backend = if used_live {
                        live
                    } else {
                        formal::backend_for(config, sys, true)
                    };
                    match backend.verify(config, &code, goal) {
                        Ok(report) => {
                            let cell = cell::verdict_cell(
                                sys.as_str(),
                                &code,
                                report.lexically_verified,
                                report.axioms_clean,
                                report.statement_preserved,
                                report.live,
                            );
                            let summary = format!(
                                "hammer [{}] {goal}: backend={} compiled={} live={}",
                                sys.as_str(),
                                if used_live { "live" } else { "mock" },
                                report.lexically_verified,
                                report.live,
                            );
                            ActionOutcome {
                                ok: report.lexically_verified,
                                summary,
                                cell,
                            }
                        }
                        Err(e) => ActionOutcome::error(format!("hammer verify error: {e}")),
                    }
                }
                None => ActionOutcome::error(
                    "hammer produced no reconstruction (worker unavailable or no proof found)"
                        .into(),
                ),
            }
        }
        ChatAction::Falsify { variables, claim } => {
            // Same worker call the CLI Falsify handler makes.
            let request = json!({
                "tool": "falsify", "variables": variables, "claim": claim,
                "assumptions": "True", "max_cases": 100_000
            });
            match PythonCheck::new().run(request) {
                Ok(res) => {
                    // The worker's {verdict, assignment, checked} come back as
                    // JSON on stdout; `metadata` is only the wrapper. Feed the
                    // parsed output to the honest falsify cell.
                    let value = parse_worker_json(&res.stdout);
                    let verdict = value["verdict"]
                        .as_str()
                        .unwrap_or("inconclusive")
                        .to_string();
                    let cell = cell::falsify_cell(&value);
                    let summary = format!("falsify {claim}: {verdict} ({})", res.summary);
                    ActionOutcome {
                        ok: res.success,
                        summary,
                        cell,
                    }
                }
                Err(e) => ActionOutcome::error(format!("falsify error: {e}")),
            }
        }
        ChatAction::Sweep => {
            match crate::reason::proving::staleness_sweep::sweep(
                store,
                config,
                Some(project_id),
                100_000,
            ) {
                Ok(outcome) => {
                    let c = &outcome.census;
                    let value = json!({
                        "summary": outcome.summary,
                        "fresh": c.fresh,
                        "repair_candidate": c.repair_candidate,
                        "mathematics_moved": c.mathematics_moved,
                        "unknown": c.unknown,
                        "total": c.total,
                    });
                    let cell = cell::sweep_cell(&value);
                    ActionOutcome {
                        ok: true,
                        summary: outcome.summary.clone(),
                        cell,
                    }
                }
                Err(e) => ActionOutcome::error(format!("sweep error: {e}")),
            }
        }
    }
}

/// Run the autonomous agent loop and map its summary to an agent-run cell.
fn run_agent(
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    project_id: &str,
) -> ActionOutcome {
    match (agent::AgentLoop {
        store,
        config,
        provider,
    })
    .run(project_id)
    {
        Ok(summary) => {
            // `certified` is a COUNT of certified nodes; the honest agent-run
            // cell wants a boolean claim, so a run is "certified" iff it
            // certified at least one node.
            let certified = summary.certified > 0;
            let value = json!({
                "run_id": summary.run_id,
                "certified": certified,
                "steps": summary.steps.len(),
            });
            let cell = cell::agent_run_cell(&value);
            let summary_line = format!(
                "agent run {}: certified={} steps={}",
                summary.run_id,
                summary.certified,
                summary.steps.len()
            );
            ActionOutcome {
                ok: certified,
                summary: summary_line,
                cell,
            }
        }
        Err(e) => ActionOutcome::error(format!("agent error: {e}")),
    }
}

/// Resolve a `/prove` target: a numeric index into the node list, or an id
/// prefix (>= 4 chars to avoid ambiguous matches), yields that node's informal
/// statement; anything else is treated as a raw statement to formalize.
fn resolve_prove_target(store: &Store, project_id: &str, target: &str) -> String {
    let target = target.trim();
    let nodes = store.nodes(project_id).unwrap_or_default();
    if let Ok(idx) = target.parse::<usize>() {
        if let Some(n) = nodes.get(idx) {
            return n.statement.clone();
        }
    }
    if target.len() >= 4 {
        if let Some(n) = nodes.iter().find(|n| n.id.starts_with(target)) {
            return n.statement.clone();
        }
    }
    target.to_string()
}

/// Parse a Python worker's JSON output. The worker prints its result object
/// (e.g. `{"verdict": ..., "assignment": ..., "checked": ...}`) to stdout. We
/// try the whole trimmed stream first, then fall back to the last non-empty
/// line (in case the worker emitted an incidental log line before the result),
/// and finally to a neutral `inconclusive` so a parse failure never renders as a
/// success.
fn parse_worker_json(stdout: &str) -> Value {
    let trimmed = stdout.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return v;
    }
    if let Some(last) = trimmed.lines().rev().find(|l| !l.trim().is_empty()) {
        if let Ok(v) = serde_json::from_str::<Value>(last.trim()) {
            return v;
        }
    }
    json!({ "verdict": "inconclusive" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_round_cap_is_small() {
        // The cap must stay small: a call is minutes and an action may itself
        // call the model. Guard against an accidental large value.
        assert!(MAX_ACTION_ROUNDS >= 1 && MAX_ACTION_ROUNDS <= 8);
    }

    #[test]
    fn parse_worker_json_reads_whole_then_last_line() {
        // Clean JSON parses directly.
        let v = parse_worker_json(r#"{"verdict":"counterexample","checked":3}"#);
        assert_eq!(v["verdict"], "counterexample");
        assert_eq!(v["checked"], 3);
        // A leading log line is tolerated: the last JSON line wins.
        let v2 = parse_worker_json(
            "loading sympy...\n{\"verdict\":\"no_counterexample_in_domain\",\"checked\":100000}",
        );
        assert_eq!(v2["verdict"], "no_counterexample_in_domain");
        assert_eq!(v2["checked"], 100000);
        // Garbage degrades to a neutral inconclusive, never a success shape.
        let v3 = parse_worker_json("not json at all");
        assert_eq!(v3["verdict"], "inconclusive");
        assert!(v3["assignment"].is_null());
    }

    #[test]
    fn action_error_is_fail_closed() {
        let outcome = ActionOutcome::error("prove error: boom".into());
        assert!(!outcome.ok);
        assert!(outcome.summary.contains("boom"));
        // The cell renders (an error cell); it must produce at least one line.
        assert!(!outcome.cell.lines(80).is_empty());
    }
}
