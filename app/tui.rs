use crate::{
    agent,
    chat::{ChatAction, ChatEngine, ChatTurn},
    config::Config,
    db::Store,
    formal::{self, FormalSystem},
    formal_generate,
    model::{Event, ModelStreamEvent, Node, NodeKind, NodeStatus},
    provider::ModelProvider,
    tools::{PythonCheck, Tool},
};
use anyhow::Result;
use crossterm::{
    event::{self, Event as CEvent, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Tabs, Wrap},
    Terminal,
};
use serde_json::{json, Value};
use std::{io, time::Duration};

/// Cap on how many action-execution rounds one natural-language turn may drive.
/// A single model call is minutes against a local 35B and an action may itself
/// call the model, so an uncapped loop would hang for a very long time. The cap
/// plus the between-rounds interrupt below are the whole safety net for the loop.
const MAX_ACTION_ROUNDS: usize = 4;

#[derive(Clone, Copy)]
enum Pane {
    Chat,
    Graph,
    Events,
}

struct App {
    project_id: String,
    input: String,
    pane: Pane,
    status: String,
    selected: usize,
    /// Detailed output lines from the last slash command / action; when
    /// non-empty they take over the main pane until cleared with Esc.
    output: Vec<String>,
    /// Live model-stream buffer. `Some` only during an in-flight model call, so
    /// the pane can show reply deltas as they arrive instead of a frozen screen.
    stream: Option<String>,
}

pub fn run(
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    project_id: &str,
) -> Result<()> {
    store.project(project_id)?;
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    let result = run_loop(&mut terminal, store, config, provider, project_id);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    project_id: &str,
) -> Result<()> {
    let mut app = App {
        project_id: project_id.into(),
        input: String::new(),
        pane: Pane::Chat,
        status: "Ready · type to chat (the model can prove/falsify/hammer/sweep) · \
                 /help lists commands · Tab changes pane · Esc clears · Ctrl-C exits"
            .into(),
        selected: 0,
        output: Vec::new(),
        stream: None,
    };
    loop {
        draw(terminal, store, &app)?;
        if event::poll(Duration::from_millis(150))? {
            if let CEvent::Key(k) = event::read()? {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                match (k.code, k.modifiers) {
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,
                    (KeyCode::Esc, _) => {
                        app.output.clear();
                        app.status = "Command output cleared".into();
                    }
                    (KeyCode::Tab, _) => {
                        app.pane = match app.pane {
                            Pane::Chat => Pane::Graph,
                            Pane::Graph => Pane::Events,
                            Pane::Events => Pane::Chat,
                        }
                    }
                    (KeyCode::Up, _) => app.selected = app.selected.saturating_sub(1),
                    (KeyCode::Down, _) => {
                        let count = store.nodes(&app.project_id).map(|n| n.len()).unwrap_or(0);
                        app.selected = (app.selected + 1).min(count.saturating_sub(1))
                    }
                    (KeyCode::Backspace, _) => {
                        app.input.pop();
                    }
                    (KeyCode::Enter, _) => {
                        let text = app.input.trim().to_owned();
                        app.input.clear();
                        if text.is_empty() {
                            continue;
                        }
                        if text.starts_with('/') {
                            handle_slash(terminal, store, config, provider, &mut app, &text);
                        } else {
                            agentic_turn(terminal, store, config, provider, &mut app, &text);
                        }
                    }
                    (KeyCode::Char(ch), m) if !m.contains(KeyModifiers::CONTROL) => {
                        app.input.push(ch)
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

// ===========================================================================
// Rendering
// ===========================================================================

/// Draw one full frame from the current store + app state. Called both by the
/// main loop and, during a model call, by the streaming callback, so a
/// multi-minute call keeps repainting (reply deltas + a spinner status) instead
/// of looking frozen.
fn draw(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    store: &Store,
    app: &App,
) -> Result<()> {
    let nodes = store.nodes(&app.project_id)?;
    let events = store.events(&app.project_id, 80)?;
    let messages = store.messages(&app.project_id, 100)?;
    terminal.draw(|f| {
        let root = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(10),
                Constraint::Length(3),
            ])
            .split(f.area());
        let tab = match app.pane {
            Pane::Chat => 0,
            Pane::Graph => 1,
            Pane::Events => 2,
        };
        let title = Tabs::new(vec!["CHAT", "PROOF GRAPH", "TRAJECTORY"])
            .select(tab)
            .block(Block::default().borders(Borders::ALL).title(" THEOREMATA "))
            .highlight_style(
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            );
        f.render_widget(title, root[0]);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(68), Constraint::Percentage(32)])
            .split(root[1]);
        // Priority: a live stream, then captured command output, then the pane.
        if let Some(buf) = app.stream.as_ref() {
            f.render_widget(
                Paragraph::new(buf.as_str())
                    .wrap(Wrap { trim: false })
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(" Model (streaming)… "),
                    ),
                cols[0],
            );
        } else if !app.output.is_empty() {
            render_command_output(f, cols[0], &app.output);
        } else {
            match app.pane {
                Pane::Chat => {
                    let lines = messages
                        .iter()
                        .flat_map(|m| {
                            let color = match m.role.as_str() {
                                "user" => Color::Cyan,
                                "tool" => Color::Yellow,
                                _ => Color::Green,
                            };
                            vec![
                                Line::from(Span::styled(
                                    format!("{} ›", m.role.to_uppercase()),
                                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                                )),
                                Line::raw(m.content.clone()),
                                Line::raw(""),
                            ]
                        })
                        .collect::<Vec<_>>();
                    f.render_widget(
                        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title(" Conversation "),
                        ),
                        cols[0],
                    );
                }
                Pane::Graph => render_nodes(f, cols[0], &nodes, app.selected),
                Pane::Events => render_events(f, cols[0], &events),
            }
        }
        render_inspector(f, cols[1], &nodes, app.selected, &app.status);
        f.render_widget(
            Paragraph::new(format!("> {}", app.input)).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Message or /command "),
            ),
            root[2],
        );
    })?;
    Ok(())
}

// ===========================================================================
// Slash commands
// ===========================================================================

/// Route a slash command. Action commands (`/model`, `/prove`, `/hammer`,
/// `/falsify`, `/sweep`, `/agent`) need `config`/`terminal` and can block for
/// minutes, so they are handled here; the ten read-only commands fall through
/// to [`slash`].
fn handle_slash(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    app: &mut App,
    text: &str,
) {
    let mut parts = text.splitn(2, ' ');
    let cmd = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    match cmd {
        "/model" => {
            let (status, output) = handle_model(rest);
            app.status = status;
            app.output = output;
        }
        "/prove" => {
            let (system, target) = split_leading_system(rest);
            if target.is_empty() {
                app.status = "usage: /prove [lean|rocq|isabelle] <node-id|index|statement>".into();
                return;
            }
            let action = ChatAction::Prove {
                system: system.as_str().to_string(),
                target: target.to_string(),
            };
            run_blocking(terminal, store, app, "lean", |app| {
                execute_action(store, config, provider, &app.project_id, &action)
            });
        }
        "/hammer" => {
            let mut it = rest.splitn(2, ' ');
            let sys = it.next().unwrap_or("").trim();
            let goal = it.next().unwrap_or("").trim();
            if sys.is_empty() || goal.is_empty() {
                app.status = "usage: /hammer <lean|rocq|isabelle> <goal>".into();
                return;
            }
            let action = ChatAction::Hammer {
                system: sys.to_string(),
                goal: goal.to_string(),
            };
            run_blocking(terminal, store, app, "hammer", |app| {
                execute_action(store, config, provider, &app.project_id, &action)
            });
        }
        "/falsify" => match parse_falsify_args(rest) {
            Ok((variables, claim)) => {
                let action = ChatAction::Falsify { variables, claim };
                run_blocking(terminal, store, app, "python", |app| {
                    execute_action(store, config, provider, &app.project_id, &action)
                });
            }
            Err(e) => {
                app.status = format!("usage: /falsify <variables-json> <claim> ({e})");
            }
        },
        "/sweep" => {
            let action = ChatAction::Sweep;
            run_blocking(terminal, store, app, "sweep", |app| {
                execute_action(store, config, provider, &app.project_id, &action)
            });
        }
        "/agent" => {
            run_blocking(terminal, store, app, "agent", |app| {
                run_agent(store, config, provider, &app.project_id)
            });
        }
        // Everything else: the read-only reference commands.
        _ => match slash(store, provider, &app.project_id, text) {
            Ok((status, output)) => {
                app.status = status;
                app.output = output;
            }
            Err(e) => {
                app.status = format!("Command error: {e}");
                app.output.clear();
            }
        },
    }
}

/// Set a "working" status, repaint, then run a blocking action and capture its
/// output. Honest about being blocking: a single in-flight call cannot be
/// interrupted (raw mode delivers Ctrl-C as a key only when we poll, and the
/// child process holds this thread), so no false "async" claim is made.
fn run_blocking(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    store: &Store,
    app: &mut App,
    label: &str,
    f: impl FnOnce(&App) -> (bool, Vec<String>),
) {
    app.output.clear();
    app.status = format!("working ({label})… (blocking; please wait)");
    let _ = draw(terminal, store, app);
    let (ok, lines) = f(app);
    app.output = lines;
    app.status = if ok {
        format!("{label}: done")
    } else {
        format!("{label}: failed (see output)")
    };
}

/// Handle a read-only slash command. Returns a short status line plus detailed
/// output lines (rendered in the main pane until cleared with Esc).
fn slash(
    store: &Store,
    provider: &dyn ModelProvider,
    project_id: &str,
    text: &str,
) -> Result<(String, Vec<String>)> {
    let mut parts = text.splitn(3, ' ');
    let cmd = parts.next().unwrap_or("");
    let mut out = Vec::new();
    let status = match cmd {
        "/help" => {
            out.push("Read-only:".into());
            out.push("/help                  this command reference".into());
            out.push("/graph                 every node (id · kind · status · title)".into());
            out.push("/obligations           obligation nodes with hints and lemmas".into());
            out.push("/attempts              recent solver attempts".into());
            out.push("/events                recent trajectory events".into());
            out.push("/proposals             pending graph-mutation proposals".into());
            out.push("/approve <id>          approve a pending proposal".into());
            out.push("/reject <id> [reason]  reject a pending proposal".into());
            out.push("/verify                verification status of every node".into());
            out.push("/status                project name and theorem".into());
            out.push(String::new());
            out.push("Actions (do real work, may block for minutes):".into());
            out.push("/model [name]          list ollama models / switch active model".into());
            out.push("/prove [sys] <target>  formalize+prove+gate a node/index/statement".into());
            out.push("/hammer <sys> <goal>   hammer-assisted native proof + gate".into());
            out.push("/falsify <json> <claim> numeric counterexample search".into());
            out.push("/sweep                 staleness census for this project".into());
            out.push("/agent                 run the autonomous loop on this project".into());
            out.push(String::new());
            out.push(
                "Plain text talks to the agent, which may itself prove/falsify/hammer/sweep."
                    .into(),
            );
            out.push("Esc clears output · Tab cycles panes · Ctrl-C exits".into());
            "Command reference".into()
        }
        "/graph" => {
            let nodes = store.nodes(project_id)?;
            for n in &nodes {
                out.push(format!(
                    "{}  {:<16} {:<20} {}",
                    &n.id[..8.min(n.id.len())],
                    n.kind,
                    n.status,
                    n.title
                ));
            }
            format!("{} graph nodes", nodes.len())
        }
        "/obligations" => {
            let nodes = store.nodes(project_id)?;
            let obligations: Vec<&Node> = nodes
                .iter()
                .filter(|n| n.kind == NodeKind::Obligation)
                .collect();
            for n in &obligations {
                out.push(format!("[{}] {}", n.status, n.title));
                if let Some(hint) = &n.strategy_hint {
                    out.push(format!("      hint: {hint}"));
                }
                if !n.suggested_lemmas.is_empty() {
                    out.push(format!("      lemmas: {}", n.suggested_lemmas.join(", ")));
                }
            }
            if obligations.is_empty() {
                out.push("No obligation nodes yet".into());
            }
            format!("{} obligations", obligations.len())
        }
        "/attempts" => {
            let attempts = store.attempts(project_id, 30)?;
            for a in &attempts {
                let mark = if a.success { "ok" } else { "x" };
                let node = a
                    .node_id
                    .as_deref()
                    .map(|s| &s[..8.min(s.len())])
                    .unwrap_or("-");
                out.push(format!(
                    "{mark} {:<18} node={node} {}",
                    a.actor,
                    a.created_at.format("%H:%M:%S")
                ));
            }
            if attempts.is_empty() {
                out.push("No attempts recorded yet".into());
            }
            format!("{} recent attempts", attempts.len())
        }
        "/events" => {
            let events = store.events(project_id, 40)?;
            for e in events.iter().rev() {
                out.push(format!(
                    "{}  {:<24} {}",
                    e.created_at.format("%H:%M:%S"),
                    e.event_type,
                    e.actor
                ));
            }
            format!("{} recent events", events.len())
        }
        "/status" => {
            let p = store.project(project_id)?;
            out.push(format!("Project:  {}", p.name));
            out.push(format!("Theorem:  {}", p.theorem));
            p.name
        }
        "/proposals" => {
            let proposals = store.proposals(project_id, true)?;
            for proposal in &proposals {
                out.push(format!(
                    "{}  {}",
                    &proposal.id[..8.min(proposal.id.len())],
                    proposal.action["action"].as_str().unwrap_or("?")
                ));
            }
            if proposals.is_empty() {
                out.push("No pending proposals".into());
            }
            format!("{} pending proposals", proposals.len())
        }
        "/approve" => {
            let prefix = parts.next().unwrap_or("");
            let proposal = resolve_proposal(store, project_id, prefix)?;
            ChatEngine { store, provider }.approve(project_id, &proposal)?;
            format!(
                "Approved and applied {}",
                &proposal[..8.min(proposal.len())]
            )
        }
        "/reject" => {
            let prefix = parts.next().unwrap_or("");
            let reason = parts.next().unwrap_or("rejected in TUI");
            let proposal = resolve_proposal(store, project_id, prefix)?;
            ChatEngine { store, provider }.reject(project_id, &proposal, reason)?;
            format!("Rejected {}", &proposal[..8.min(proposal.len())])
        }
        "/verify" => {
            let nodes = store.nodes(project_id)?;
            let verified = nodes
                .iter()
                .filter(|n| n.status == NodeStatus::FormallyVerified)
                .count();
            for n in &nodes {
                let layer = if n.formal_statement.is_some() {
                    "formal"
                } else {
                    "informal"
                };
                out.push(format!("{:<20} {:<9} {}", n.status, layer, n.title));
            }
            out.push(String::new());
            out.push("Use /prove or /agent to drive verification.".into());
            format!("{}/{} nodes formally verified", verified, nodes.len())
        }
        other => {
            out.push("Type /help for the command reference.".into());
            format!("Unknown command: {other}")
        }
    };
    Ok((status, out))
}

// ===========================================================================
// Agentic natural-language loop
// ===========================================================================

/// A plain typed line drives an agentic loop: the model replies, may file graph
/// proposals, AND may request closed-set actions. We run each action, append a
/// compact result as a `tool` message, then call the model again so it can
/// react. Capped at [`MAX_ACTION_ROUNDS`] and interruptible between rounds.
///
/// Soundness: the model's `reply` is text, never a verdict. Actions run REAL
/// functions; `prove`/`hammer` return a [`VerificationReport`] but do not write
/// graph status, and proposed `set_status: formally_verified` mutations are
/// rejected by the engine. So the chat can never fake a formal verification.
fn agentic_turn(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    app: &mut App,
    text: &str,
) {
    let mut task = text.to_string();
    let mut record_user = true;
    let mut round = 0usize;
    loop {
        let turn = match run_chat_turn(terminal, store, provider, app, &task, record_user) {
            Ok(t) => t,
            Err(e) => {
                app.status = format!("Agent error: {e}");
                return;
            }
        };
        record_user = false;
        let mut shown = vec![turn.reply.clone()];
        if turn.proposals > 0 {
            shown.push(String::new());
            shown.push(format!(
                "[{} graph mutation proposal(s) awaiting /approve]",
                turn.proposals
            ));
        }
        app.output = shown;
        if turn.actions.is_empty() {
            app.status = "Response committed to conversation and graph".into();
            return;
        }
        round += 1;
        for action in &turn.actions {
            app.status = format!("working ({})… (blocking call)", action.tool_name());
            let _ = draw(terminal, store, app);
            let (ok, lines) = execute_action(store, config, provider, &app.project_id, action);
            let summary = compact_result(action.tool_name(), ok, &lines);
            let _ = store.add_message(
                &app.project_id,
                "tool",
                &summary,
                json!({"tool":action.tool_name(),"ok":ok}),
            );
        }
        if round >= MAX_ACTION_ROUNDS {
            app.status = format!("Action round cap ({MAX_ACTION_ROUNDS}) reached; stopping.");
            return;
        }
        // Interruptible between rounds: a pending Esc / Ctrl-C stops the loop
        // and returns to input. A single in-flight call is not preemptible.
        if pending_interrupt() {
            app.status = "Interrupted; returned to input.".into();
            return;
        }
        task = "Continue: react to the tool results now in the conversation. \
                Request more actions only if they are needed."
            .to_string();
    }
}

/// Run one model turn with live streaming into the pane. The streaming callback
/// borrows `app` and `terminal` mutably; `project`/`task` are cloned locals so
/// the engine call and the callback do not alias the same borrow.
fn run_chat_turn(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    store: &Store,
    provider: &dyn ModelProvider,
    app: &mut App,
    task: &str,
    record_user: bool,
) -> Result<ChatTurn> {
    let project = app.project_id.clone();
    let task = task.to_owned();
    app.stream = Some(String::new());
    app.output.clear();
    let engine = ChatEngine { store, provider };
    let mut ticks: usize = 0;
    let outcome = {
        let app_cell = &mut *app;
        let mut on_event = |ev: ModelStreamEvent| {
            if let ModelStreamEvent::Delta { text } = ev {
                if let Some(buf) = app_cell.stream.as_mut() {
                    buf.push_str(&text);
                }
            }
            ticks = ticks.wrapping_add(1);
            app_cell.status = format!("working (model) {}", spinner(ticks));
            let _ = draw(terminal, store, app_cell);
        };
        engine.send_turn(&project, &task, record_user, &mut on_event)
    };
    app.stream = None;
    outcome
}

/// True if an Esc or Ctrl-C keypress is already queued. Non-blocking: it drains
/// pending events without waiting, so it only sees keys pressed during the
/// preceding action, which is exactly the between-rounds interrupt we want.
fn pending_interrupt() -> bool {
    let mut hit = false;
    while event::poll(Duration::from_millis(0)).unwrap_or(false) {
        if let Ok(CEvent::Key(k)) = event::read() {
            if k.kind == KeyEventKind::Press {
                match (k.code, k.modifiers) {
                    (KeyCode::Esc, _) => hit = true,
                    (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => hit = true,
                    _ => {}
                }
            }
        }
    }
    hit
}

fn spinner(tick: usize) -> char {
    const FRAMES: [char; 4] = ['|', '/', '-', '\\'];
    FRAMES[tick % FRAMES.len()]
}

// ===========================================================================
// Action execution (the closed set the chat + slash commands share)
// ===========================================================================

/// Execute one closed-set [`ChatAction`] against the REAL functions. Returns
/// (success, output-lines) and NEVER panics or bubbles an error out: any
/// failure becomes a visible error line so a failed action can never read as a
/// false success. This is the single place the cockpit maps the closed enum to
/// a concrete function; no string from the model is ever run as a command.
fn execute_action(
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    project_id: &str,
    action: &ChatAction,
) -> (bool, Vec<String>) {
    match action {
        ChatAction::Prove { system, target } => {
            let sys = match system.parse::<FormalSystem>() {
                Ok(s) => s,
                Err(e) => return (false, vec![format!("prove: unknown system: {e}")]),
            };
            let statement = resolve_prove_target(store, project_id, target);
            match formal_generate::generate_and_verify(store, config, provider, sys, &statement) {
                Ok((code, report)) => {
                    let ok = report.lexically_verified;
                    let mut out = vec![
                        format!("prove [{}] statement: {statement}", sys.as_str()),
                        format!("compiled:            {}", report.lexically_verified),
                        format!("axioms_clean:        {}", report.axioms_clean),
                        format!("statement_preserved: {}", report.statement_preserved),
                        format!("live:                {}", report.live),
                        String::new(),
                        "generated code:".into(),
                    ];
                    out.extend(code.lines().map(str::to_string));
                    (ok, out)
                }
                Err(e) => (false, vec![format!("prove error: {e}")]),
            }
        }
        ChatAction::Hammer { system, goal } => {
            let sys = match system.parse::<FormalSystem>() {
                Ok(s) => s,
                Err(e) => return (false, vec![format!("hammer: unknown system: {e}")]),
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
                            let ok = report.lexically_verified;
                            let mut out = vec![
                                format!("hammer [{}] goal: {goal}", sys.as_str()),
                                format!("backend:  {}", if used_live { "live" } else { "mock" }),
                                format!("compiled: {}", report.lexically_verified),
                                format!("live:     {}", report.live),
                                String::new(),
                                "assembled proof:".into(),
                            ];
                            out.extend(code.lines().map(str::to_string));
                            (ok, out)
                        }
                        Err(e) => (false, vec![format!("hammer verify error: {e}")]),
                    }
                }
                None => (
                    false,
                    vec![
                        "hammer produced no reconstruction (worker unavailable or no proof found)"
                            .into(),
                    ],
                ),
            }
        }
        ChatAction::Falsify { variables, claim } => {
            // Same worker call the CLI Falsify handler makes.
            let request = json!({
                "tool":"falsify","variables":variables,"claim":claim,
                "assumptions":"True","max_cases":100_000
            });
            match PythonCheck::new().run(request) {
                Ok(res) => {
                    let out = vec![
                        format!("falsify claim: {claim}"),
                        format!("worker success: {}", res.success),
                        res.summary.clone(),
                        format!("verdict: {}", res.metadata),
                    ];
                    (res.success, out)
                }
                Err(e) => (false, vec![format!("falsify error: {e}")]),
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
                    (
                        true,
                        vec![
                            outcome.summary.clone(),
                            format!("Fresh:            {}", c.fresh),
                            format!("RepairCandidate:  {}", c.repair_candidate),
                            format!("MathematicsMoved: {}", c.mathematics_moved),
                            format!("Unknown:          {}", c.unknown),
                            format!("total:            {}", c.total),
                        ],
                    )
                }
                Err(e) => (false, vec![format!("sweep error: {e}")]),
            }
        }
    }
}

/// Run the autonomous agent loop on the project (the CLI `Agent` command path).
fn run_agent(
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    project_id: &str,
) -> (bool, Vec<String>) {
    match (agent::AgentLoop {
        store,
        config,
        provider,
    })
    .run(project_id)
    {
        Ok(summary) => (
            true,
            vec![
                format!("agent run: {}", summary.run_id),
                format!("certified: {}", summary.certified),
                format!("steps:     {}", summary.steps.len()),
            ],
        ),
        Err(e) => (false, vec![format!("agent error: {e}")]),
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

/// Compact an action's output into a single conversation `tool` message so the
/// next model turn can react without swelling the context. Length-capped.
fn compact_result(tool: &str, ok: bool, lines: &[String]) -> String {
    let body: String = lines
        .iter()
        .filter(|l| !l.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" | ");
    let body: String = body.chars().take(700).collect();
    format!("[{tool} {}] {body}", if ok { "ok" } else { "failed" })
}

/// Split an optional leading formal-system token off a `/prove` argument.
/// Returns (system, remaining-target). Defaults to Lean when the first token is
/// not a system name (so the whole string is the target).
fn split_leading_system(rest: &str) -> (FormalSystem, &str) {
    let mut it = rest.splitn(2, ' ');
    let first = it.next().unwrap_or("");
    if let Ok(sys) = first.parse::<FormalSystem>() {
        (sys, it.next().unwrap_or("").trim())
    } else {
        (FormalSystem::Lean, rest.trim())
    }
}

/// Parse `/falsify <variables-json> <claim>`: read the first JSON value, then
/// take the remaining text as the claim. Using a streaming deserializer lets
/// the variables object contain spaces without a fragile manual split.
fn parse_falsify_args(rest: &str) -> Result<(Value, String)> {
    let mut de = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
    let variables = match de.next() {
        Some(Ok(v)) => v,
        _ => anyhow::bail!("expected a JSON variables object first"),
    };
    let offset = de.byte_offset();
    let claim = rest[offset..].trim().to_string();
    if !variables.is_object() {
        anyhow::bail!("variables must be a JSON object");
    }
    if claim.is_empty() {
        anyhow::bail!("missing claim after the variables object");
    }
    Ok((variables, claim))
}

// ===========================================================================
// Model picker (ollama)
// ===========================================================================

/// The env var the Python adapter reads (via `model_for_role`) on EVERY call.
/// The `CommandProvider` spawns the adapter fresh each turn, so a `set_var` here
/// takes effect on the next model call with no restart.
const MODEL_ENV: &str = "THEOREMATA_MODEL";
/// Prefix litellm needs to route a bare ollama tag to the local ollama server.
const OLLAMA_PREFIX: &str = "ollama_chat/";

/// `/model` with no arg lists ollama models (marking the current one); with a
/// name it validates against that list and switches the active model. Returns
/// (status, output-lines). We shell out to `ollama list` (chosen over the HTTP
/// /api/tags endpoint because the crate has no HTTP client and adding a
/// dependency is out of scope); a name is only accepted if `ollama list`
/// returned it, so no arbitrary string ever reaches the env var.
fn handle_model(arg: &str) -> (String, Vec<String>) {
    let current = std::env::var(MODEL_ENV).ok();
    let models = match ollama_models() {
        Ok(m) => m,
        Err(e) => {
            return (
                format!("/model: could not list ollama models: {e}"),
                vec![
                    format!("could not run `ollama list`: {e}"),
                    "Is ollama installed and on PATH?".into(),
                ],
            )
        }
    };
    if arg.is_empty() {
        let mut out = vec![
            format!(
                "current {MODEL_ENV} = {}",
                current.as_deref().unwrap_or("(unset; adapter default)")
            ),
            String::new(),
            "available ollama models (from `ollama list`):".into(),
        ];
        let cur_bare = current.as_deref().map(bare_name);
        if models.is_empty() {
            out.push("  (none installed)".into());
        }
        for m in &models {
            let marker = if Some(m.as_str()) == cur_bare {
                " * (current)"
            } else {
                ""
            };
            out.push(format!("  {m}{marker}"));
        }
        out.push(String::new());
        out.push("switch with: /model <name>".into());
        return ("ollama model list".into(), out);
    }
    // Switch: validate the bare name against the list before touching the env.
    let bare = bare_name(arg).to_string();
    if !models.iter().any(|m| m == &bare) {
        let mut out = vec![format!("unknown model: {bare}"), "available:".into()];
        out.extend(models.iter().map(|m| format!("  {m}")));
        return (format!("/model rejected unknown model: {bare}"), out);
    }
    let stored = stored_name(&bare);
    std::env::set_var(MODEL_ENV, &stored);
    (
        format!("active model set to {stored} (takes effect next turn)"),
        vec![format!("{MODEL_ENV} = {stored}")],
    )
}

/// Query installed ollama models via `ollama list`.
fn ollama_models() -> Result<Vec<String>> {
    let output = std::process::Command::new("ollama")
        .arg("list")
        .output()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "`ollama list` exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(parse_ollama_list(&String::from_utf8_lossy(&output.stdout)))
}

/// Parse the first column (NAME) out of `ollama list` tabular output, skipping
/// the header row. Pure, so it is unit-tested directly.
fn parse_ollama_list(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .enumerate()
        .filter_map(|(i, line)| {
            if i == 0 {
                // Header row: NAME  ID  SIZE  MODIFIED.
                return None;
            }
            let name = line.split_whitespace().next()?;
            if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            }
        })
        .collect()
}

/// Strip the `ollama_chat/` routing prefix to get the bare ollama tag.
fn bare_name(name: &str) -> &str {
    name.strip_prefix(OLLAMA_PREFIX).unwrap_or(name)
}

/// The value stored in the env var: always prefixed so litellm routes it to the
/// local ollama server, whether the user typed the prefix or not.
fn stored_name(bare: &str) -> String {
    format!("{OLLAMA_PREFIX}{}", bare_name(bare))
}

fn render_command_output(f: &mut ratatui::Frame, area: ratatui::layout::Rect, output: &[String]) {
    let lines: Vec<Line> = output.iter().map(|l| Line::raw(l.clone())).collect();
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Command output · Esc to clear "),
        ),
        area,
    );
}

fn resolve_proposal(store: &Store, project_id: &str, prefix: &str) -> Result<String> {
    let matches = store
        .proposals(project_id, true)?
        .into_iter()
        .filter(|proposal| proposal.id.starts_with(prefix))
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        anyhow::bail!("proposal prefix must match exactly one pending proposal");
    }
    Ok(matches[0].id.clone())
}
fn render_nodes(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    nodes: &[Node],
    selected: usize,
) {
    let items = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let mark = match n.status.to_string().as_str() {
                "formally_verified" => "V",
                "rejected" => "x",
                "blocked" => "!",
                "active" => "*",
                _ => ".",
            };
            let mut item = ListItem::new(format!("{mark} {:<18} {}", n.kind, n.title));
            if i == selected {
                item = item.style(
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                );
            }
            item
        })
        .collect::<Vec<_>>();
    f.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Proof graph · {} nodes ", nodes.len())),
        ),
        area,
    );
}
fn render_events(f: &mut ratatui::Frame, area: ratatui::layout::Rect, events: &[Event]) {
    let items = events
        .iter()
        .rev()
        .map(|e| {
            ListItem::new(format!(
                "{}  {:<22} {}",
                e.created_at.format("%H:%M:%S"),
                e.event_type,
                e.actor
            ))
        })
        .collect::<Vec<_>>();
    f.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Event trajectory "),
        ),
        area,
    );
}
fn render_inspector(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    nodes: &[Node],
    selected: usize,
    status: &str,
) {
    let body =
        nodes
            .get(selected)
            .map(|n| {
                format!(
        "{}\n\nTYPE      {}\nSTATUS    {}\nTAINTED   {}\nPROVENANCE {}\n\n{}\n\nFORMAL\n{}",
        n.title,n.kind,n.status,n.tainted,n.provenance,n.statement,
        n.formal_statement.as_deref().unwrap_or("Not formalized")
    )
            })
            .unwrap_or_else(|| "No node selected".into());
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(5)])
        .split(area);
    f.render_widget(
        Paragraph::new(body)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(" Inspector ")),
        rows[0],
    );
    f.render_widget(
        Paragraph::new(status)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title(" System ")),
        rows[1],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ollama_list_skips_header_takes_names() {
        let stdout = "NAME              ID            SIZE      MODIFIED\n\
                      qwen3:32b         abc123        20 GB     2 days ago\n\
                      llama3.1:8b       def456        4.7 GB    5 days ago\n";
        let models = parse_ollama_list(stdout);
        assert_eq!(models, vec!["qwen3:32b", "llama3.1:8b"]);
    }

    #[test]
    fn parse_ollama_list_handles_empty() {
        assert!(parse_ollama_list("NAME  ID  SIZE  MODIFIED\n").is_empty());
        assert!(parse_ollama_list("").is_empty());
    }

    #[test]
    fn model_name_prefix_normalization() {
        // Bare and prefixed inputs both store WITH the prefix; bare strips it.
        assert_eq!(stored_name("qwen3:32b"), "ollama_chat/qwen3:32b");
        assert_eq!(
            stored_name("ollama_chat/qwen3:32b"),
            "ollama_chat/qwen3:32b"
        );
        assert_eq!(bare_name("ollama_chat/qwen3:32b"), "qwen3:32b");
        assert_eq!(bare_name("qwen3:32b"), "qwen3:32b");
    }

    #[test]
    fn split_leading_system_defaults_to_lean() {
        let (sys, target) = split_leading_system("1 + 1 = 2");
        assert_eq!(sys, FormalSystem::Lean);
        assert_eq!(target, "1 + 1 = 2");

        let (sys, target) = split_leading_system("rocq forall n, n = n");
        assert_eq!(sys, FormalSystem::Rocq);
        assert_eq!(target, "forall n, n = n");

        let (sys, target) = split_leading_system("isabelle 1 + 1 = (2::nat)");
        assert_eq!(sys, FormalSystem::Isabelle);
        assert_eq!(target, "1 + 1 = (2::nat)");
    }

    #[test]
    fn parse_falsify_args_splits_json_and_claim() {
        let (vars, claim) = parse_falsify_args(r#"{"n": "int"} n * n >= 0"#).unwrap();
        assert!(vars.is_object());
        assert_eq!(claim, "n * n >= 0");
        // Missing claim and non-object both error rather than run a bad request.
        assert!(parse_falsify_args(r#"{"n":"int"}"#).is_err());
        assert!(parse_falsify_args(r#"[1,2] x > 0"#).is_err());
    }

    #[test]
    fn compact_result_is_capped_and_labeled() {
        let lines = vec!["a".to_string(), String::new(), "b".to_string()];
        assert_eq!(compact_result("prove", true, &lines), "[prove ok] a | b");
        assert_eq!(
            compact_result("sweep", false, &["x".to_string()]),
            "[sweep failed] x"
        );
        let long = vec!["z".repeat(2000)];
        assert!(compact_result("prove", true, &long).len() < 800);
    }

    #[test]
    fn action_round_cap_is_small() {
        // The cap must stay small: a call is minutes and an action may itself
        // call the model. Guard against an accidental large value.
        assert!(MAX_ACTION_ROUNDS >= 1 && MAX_ACTION_ROUNDS <= 8);
    }
}
