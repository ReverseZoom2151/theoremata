//! The Theoremata cockpit: a scrolling transcript of typed cells, a multi-line
//! composer with a slash-command popup, and a non-blocking event bus.
//!
//! The old cockpit was a tabbed three-pane layout that rebuilt panes from the DB
//! every frame and BLOCKED the whole UI thread during a model call. This rebuild
//! keeps the UI live at all times: long work (a chat turn, a prove/hammer/
//! falsify/sweep/agent action) runs on a worker thread (see [`event`]) and talks
//! back through an `mpsc` channel the main loop drains every tick. Keys, scroll,
//! resize and interrupt are all processed every ~50ms regardless of what a
//! worker is doing.
//!
//! Layout, top to bottom: a startup welcome card (product name, active model,
//! project, key hints) that scrolls away as the first transcript cell, the
//! full-width scrolling transcript, a one-row gap, the command popup (when
//! active), the composer in a bordered box, and a one-line status footer. The three sibling
//! leaf modules (`cell`, `composer`, `command_popup`) are treated as fixed,
//! unit-tested dependencies; this module wires them to the store, the model, and
//! the event bus.

mod cell;
mod command_popup;
mod composer;
mod event;
mod graph_panel;
mod mention;

use crate::{
    chat::{ChatAction, ChatEngine},
    config::Config,
    db::Store,
    formal::FormalSystem,
    model::{NodeKind, NodeStatus},
    provider::ModelProvider,
};
use anyhow::Result;
use crossterm::{
    event::{
        poll as poll_event, read as read_event, DisableBracketedPaste, DisableMouseCapture,
        EnableBracketedPaste, EnableMouseCapture, Event as CEvent, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Terminal,
};
use serde_json::Value;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use self::cell::Cell;
use self::command_popup::CommandPopup;
use self::composer::{Composer, Submit};
use self::event::{UiEvent, WorkerInputs};
use self::graph_panel::{GraphPanel, PanelNode};
use self::mention::MentionPopup;

/// The env var the Python adapter reads on every model call; a `/model` switch
/// sets it and the next turn picks it up (the `CommandProvider` spawns the
/// adapter fresh each time).
const MODEL_ENV: &str = "THEOREMATA_MODEL";
/// Prefix litellm needs to route a bare ollama tag to the local ollama server.
const OLLAMA_PREFIX: &str = "ollama_chat/";

/// How many rows a wheel notch scrolls the transcript.
const WHEEL_STEP: usize = 3;

pub fn run(
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    project_id: &str,
) -> Result<()> {
    store.project(project_id)?;
    enable_raw_mode()?;
    let mut out = io::stdout();
    // Mouse capture drives wheel scrolling; bracketed paste lets a pasted
    // statement arrive as one `Event::Paste` instead of a burst of key events.
    execute!(
        out,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    let result = run_loop(&mut terminal, store, config, provider, project_id);
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;
    result
}

/// The mutable UI state. Note what is NOT here: no `Store`, no provider, no
/// `Config`. Those are threaded in as borrows to the loop functions, and the
/// worker gets its own owned copies. `App` is pure presentation plus the bus.
struct App {
    project_id: String,
    composer: Composer,
    popup: CommandPopup,
    /// The @-node mention popup, mutually exclusive with `popup` by trigger char
    /// ('/' vs a trailing '@' token); kept in sync on every composer edit.
    mention: MentionPopup,
    /// The graph-first side panel and whether it is currently shown. `show_graph`
    /// is user intent; whether it actually renders also depends on there being
    /// enough width (see `graph_width`).
    graph: GraphPanel,
    show_graph: bool,
    /// When true AND the panel is shown, Up/Down drive the panel's selection
    /// instead of the composer/popup. Esc or typing returns focus to the
    /// composer. The invariant is that this is never true while `show_graph` is
    /// false (toggling and unfocus keep them consistent).
    graph_focused: bool,
    /// Committed transcript cells, oldest first.
    history: Vec<Cell>,
    /// The in-flight streamed reply preview. `Some` only while a chat worker is
    /// streaming deltas; it is discarded (not committed) when the authoritative
    /// reply cell arrives, so the reply text is never doubled.
    active_stream: Option<String>,
    /// Transcript scroll: `None` sticks to the bottom (auto-follow), `Some(top)`
    /// pins the viewport at row `top` while the user reads history.
    scroll: Option<usize>,
    /// Last drawn transcript total-line count and viewport height, so scroll
    /// keys can clamp without re-rendering every cell.
    last_total: usize,
    last_vh: usize,
    busy: bool,
    status: String,
    /// Draw tick, for the footer spinner animation.
    tick: usize,
    /// Set on Esc while busy; the worker checks it to interrupt between rounds.
    cancel: Arc<AtomicBool>,
    tx: Sender<UiEvent>,
    rx: Receiver<UiEvent>,
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    project_id: &str,
) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut composer = Composer::new();
    composer.set_placeholder("Ask the agent, or /command  (Enter sends, Shift+Enter newline)");
    let mut app = App {
        project_id: project_id.into(),
        composer,
        popup: CommandPopup::new(),
        mention: MentionPopup::new(),
        graph: GraphPanel::new(),
        show_graph: false,
        graph_focused: false,
        history: opening_history(store, project_id),
        active_stream: None,
        scroll: None,
        last_total: 0,
        last_vh: 0,
        busy: false,
        status: idle_status(),
        tick: 0,
        cancel: Arc::new(AtomicBool::new(false)),
        tx,
        rx,
    };
    loop {
        draw(terminal, &mut app, store)?;
        drain_events(&mut app);
        // Short poll so the UI stays responsive AND the spinner animates while a
        // worker runs. Keys are handled every tick, never blocked by a worker.
        if poll_event(Duration::from_millis(50))? {
            match read_event()? {
                CEvent::Key(k) => {
                    if handle_key(&mut app, store, config, provider, k) {
                        break;
                    }
                }
                CEvent::Mouse(m) => match m.kind {
                    MouseEventKind::ScrollUp => scroll_up(&mut app, WHEEL_STEP),
                    MouseEventKind::ScrollDown => scroll_down(&mut app, WHEEL_STEP),
                    _ => {}
                },
                CEvent::Paste(text) => {
                    app.composer.paste(&text);
                    sync_popups(&mut app, store);
                }
                // Resize needs no explicit handling: the next draw re-splits the
                // area and every cell re-renders to the new width (ratatui
                // reflows because cells are width-aware).
                _ => {}
            }
        }
        app.tick = app.tick.wrapping_add(1);
    }
    Ok(())
}

// ===========================================================================
// Event-bus draining (worker -> UI)
// ===========================================================================

/// Drain every pending worker event without blocking. This is the seam that
/// keeps the UI live: the worker posts here and the main loop applies the
/// effects on its own schedule.
fn drain_events(app: &mut App) {
    while let Ok(ev) = app.rx.try_recv() {
        match ev {
            UiEvent::StreamDelta(text) => {
                app.active_stream
                    .get_or_insert_with(String::new)
                    .push_str(&text);
            }
            // A committed reply cell OR a tool result cell: discard the live
            // preview (its content is superseded by the authoritative cell) and
            // commit. Both variants are handled identically on this side.
            UiEvent::Cell(c) | UiEvent::ToolCell(c) => {
                app.active_stream = None;
                app.history.push(c);
            }
            UiEvent::Progress(label) => {
                app.status = label;
            }
            UiEvent::TurnDone => {
                app.active_stream = None;
                app.busy = false;
                app.cancel.store(false, Ordering::Relaxed);
                app.status = idle_status();
                // Ring the terminal bell: a local turn is minutes and the user is
                // very likely looking away. No dependency needed.
                ring_bell();
            }
            UiEvent::Failed(message) => {
                // Fail closed: a worker error becomes a visible error cell, never
                // a false success.
                app.active_stream = None;
                app.history.push(cell::error_cell(&message));
                app.busy = false;
                app.cancel.store(false, Ordering::Relaxed);
                app.status = idle_status();
                ring_bell();
            }
        }
    }
}

fn ring_bell() {
    let mut out = io::stdout();
    let _ = out.write_all(b"\x07");
    let _ = out.flush();
}

// ===========================================================================
// Input handling (UI -> intents)
// ===========================================================================

/// Handle one key. Returns `true` when the app should exit. Global keys
/// (Ctrl-C, Esc, PageUp/Down) are handled first; the rest is routed to the
/// popup (when it is active and the key is a navigation key) or the composer.
fn handle_key(
    app: &mut App,
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    k: KeyEvent,
) -> bool {
    // Windows delivers a Release for every key; acting on it would double every
    // keystroke. Only Press and Repeat are real input.
    if k.kind == KeyEventKind::Release {
        return false;
    }
    match (k.code, k.modifiers) {
        (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => return true,
        // Ctrl+G toggles the graph-first side panel. Showing it also focuses it
        // (so Up/Down browse nodes immediately); hiding it drops focus. Whether
        // the panel actually fits is decided at draw time by `graph_width`.
        (KeyCode::Char('g'), m) if m.contains(KeyModifiers::CONTROL) => {
            app.show_graph = !app.show_graph;
            app.graph_focused = app.show_graph;
            return false;
        }
        (KeyCode::Esc, _) => {
            // While the panel is focused, Esc first returns focus to the composer
            // rather than interrupting/clearing, so the graph is easy to leave.
            if app.graph_focused {
                app.graph_focused = false;
                return false;
            }
            if app.busy {
                // True mid-turn interrupt: the worker checks this between rounds
                // and stops forwarding stream deltas immediately.
                app.cancel.store(true, Ordering::Relaxed);
                app.status = "interrupting… (returns to input at the next round)".into();
            } else if !app.composer.is_empty() {
                app.composer.clear();
                sync_popups(app, store);
            } else {
                // Nothing to cancel or clear: jump back to the live bottom.
                app.scroll = None;
            }
            return false;
        }
        (KeyCode::PageUp, _) => {
            scroll_up(app, app.last_vh.max(1));
            return false;
        }
        (KeyCode::PageDown, _) => {
            scroll_down(app, app.last_vh.max(1));
            return false;
        }
        _ => {}
    }

    // Graph panel focus: while focused, Up/Down drive the panel's selection.
    // A non-nav key returns focus to the composer and is then processed normally
    // (the "typing returns focus" rule), so the user can just start typing.
    if let Some(nav) = graph_nav(app.show_graph, app.graph_focused, k.code) {
        match nav {
            GraphNav::Prev => app.graph.select_prev(),
            GraphNav::Next => app.graph.select_next(),
        }
        return false;
    }
    if app.graph_focused {
        app.graph_focused = false;
    }

    // Route navigation keys to the command popup while it is completing a name.
    if app.popup.is_active() && popup_consumes(k.code) {
        match app.popup.key(k) {
            Some(name) => {
                complete_composer(&mut app.composer, name);
                sync_popups(app, store);
            }
            None => {
                // Up/Down moved the selection (consumed). Enter with no match
                // falls through to a normal submit.
                if k.code == KeyCode::Enter {
                    if let Some(Submit::Line(s)) = app.composer.input(k) {
                        on_submit(app, store, config, provider, s);
                    }
                    sync_popups(app, store);
                }
            }
        }
        return false;
    }

    // Route the same navigation keys to the @-mention popup when IT is active.
    // The two popups are mutually exclusive (a '/'-prefixed name has no '@'
    // trailing token, and vice versa), so this branch never fights the one above.
    if app.mention.is_active() && popup_consumes(k.code) {
        match app.mention.key(k) {
            Some(name) => {
                replace_mention_token(&mut app.composer, &name);
                sync_popups(app, store);
            }
            None => {
                // Up/Down moved the selection (consumed). Enter with nothing to
                // accept falls through to a normal submit.
                if k.code == KeyCode::Enter {
                    if let Some(Submit::Line(s)) = app.composer.input(k) {
                        on_submit(app, store, config, provider, s);
                    }
                    sync_popups(app, store);
                }
            }
        }
        return false;
    }

    // Otherwise the key edits the composer. Any edit may produce a submit.
    if let Some(Submit::Line(s)) = app.composer.input(k) {
        on_submit(app, store, config, provider, s);
    }
    sync_popups(app, store);
    false
}

/// Sync BOTH completion popups to the current composer text. The command popup
/// keys off a leading '/', the mention popup off a trailing '@token'; feeding
/// both here keeps them consistent after every edit from a single call site.
/// The node names are the project's live node titles (a cheap DB read).
fn sync_popups(app: &mut App, store: &Store) {
    let text = app.composer.text();
    app.popup.sync(&text);
    let names = node_names(store, &app.project_id);
    app.mention.sync(&text, &names);
}

/// The project's node titles, for @-mention completion. A read error yields an
/// empty list (the popup simply offers nothing) rather than disrupting input.
fn node_names(store: &Store, project_id: &str) -> Vec<String> {
    store
        .nodes(project_id)
        .map(|ns| ns.into_iter().map(|n| n.title).collect())
        .unwrap_or_default()
}

/// The graph-panel navigation intent for a key, or `None` when the key must keep
/// its normal meaning (history recall / popup nav). A pure decision so the
/// focus-routing rule is unit-testable without a TTY: nav is claimed ONLY while
/// the panel is both shown and focused, and only for Up/Down.
#[derive(Debug, PartialEq, Eq)]
enum GraphNav {
    Prev,
    Next,
}

fn graph_nav(show_graph: bool, focused: bool, code: KeyCode) -> Option<GraphNav> {
    if show_graph && focused {
        match code {
            KeyCode::Up => Some(GraphNav::Prev),
            KeyCode::Down => Some(GraphNav::Next),
            _ => None,
        }
    } else {
        None
    }
}

/// Replace the trailing `@partial` token in the composer with `@<name> ` after a
/// mention is accepted. The composer only edits (no set-text), so we compute the
/// new text purely and reload it via clear+paste, mirroring `complete_composer`.
fn replace_mention_token(composer: &mut Composer, name: &str) {
    let new = replace_trailing_mention(&composer.text(), name);
    composer.clear();
    composer.paste(&new);
}

/// Pure token surgery: swap the trailing whitespace-delimited token (which the
/// mention popup guarantees starts with '@') for `@<name> `. Everything before
/// that token is preserved verbatim. Kept free-standing so it is unit-testable.
fn replace_trailing_mention(text: &str, name: &str) -> String {
    // The mention is active only when the LAST token starts with '@', so there is
    // no trailing whitespace; `trim_end` is a harmless guard.
    let head = text.trim_end();
    let start = head.rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0);
    format!("{}@{name} ", &head[..start])
}

/// Which keys the popup consumes while active: selection movement and accept.
/// A pure decision so the routing is unit-testable without a TTY.
fn popup_consumes(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::Enter
    )
}

/// Complete the composer to a chosen command name, with a trailing space so the
/// popup deactivates and the user can type arguments. The composer has no
/// set-text method by design (it only edits), so we clear and paste.
fn complete_composer(composer: &mut Composer, name: &str) {
    composer.clear();
    composer.paste(&format!("{name} "));
}

/// Act on a submitted line. Records history, echoes a user cell, then either
/// dispatches a slash command or spawns an agentic chat turn. A submit while a
/// worker is in flight is refused (actions are serialized) with a visible note.
fn on_submit(
    app: &mut App,
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    s: String,
) {
    app.composer.push_history(&s);
    // A new submission means the user wants to watch the result: snap to bottom.
    app.scroll = None;
    if app.busy {
        app.history.push(cell::notice_cell(
            "busy: a turn is in flight (press Esc to interrupt)",
        ));
        return;
    }
    app.history.push(cell::user_cell(&s));
    if s.starts_with('/') {
        dispatch_command(app, store, config, provider, &s);
    } else {
        app.busy = true;
        app.status = "working (agent)…".into();
        event::spawn_chat(worker_inputs(app, config), s);
    }
}

/// Build the owned inputs for a worker and reset the cancel flag for the run.
fn worker_inputs(app: &App, config: &Config) -> WorkerInputs {
    app.cancel.store(false, Ordering::Relaxed);
    WorkerInputs {
        db_path: config.database.clone(),
        config: config.clone(),
        project_id: app.project_id.clone(),
        tx: app.tx.clone(),
        cancel: app.cancel.clone(),
    }
}

// ===========================================================================
// Command dispatch
// ===========================================================================

/// Dispatch a slash command. Long-running actions go through the worker/bus;
/// the read-only inspectors and the instant switches (`/model`, `/project`,
/// `/new`, `/approve`, `/reject`) run synchronously and push their result cells
/// straight into the transcript.
fn dispatch_command(
    app: &mut App,
    store: &Store,
    config: &Config,
    provider: &dyn ModelProvider,
    text: &str,
) {
    let mut parts = text.splitn(2, ' ');
    let cmd = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    match cmd {
        // --- Long-running actions: run on the worker so the UI stays live. ---
        "/prove" => {
            let (system, target) = split_leading_system(rest);
            if target.is_empty() {
                app.history.push(cell::notice_cell(
                    "usage: /prove [lean|rocq|isabelle] <node-id|index|statement>",
                ));
                return;
            }
            let action = ChatAction::Prove {
                system: system.as_str().to_string(),
                target: target.to_string(),
            };
            start_action(app, config, action);
        }
        "/hammer" => {
            let mut it = rest.splitn(2, ' ');
            let sys = it.next().unwrap_or("").trim();
            let goal = it.next().unwrap_or("").trim();
            if sys.is_empty() || goal.is_empty() {
                app.history.push(cell::notice_cell(
                    "usage: /hammer <lean|rocq|isabelle> <goal>",
                ));
                return;
            }
            let action = ChatAction::Hammer {
                system: sys.to_string(),
                goal: goal.to_string(),
            };
            start_action(app, config, action);
        }
        "/falsify" => match parse_falsify_args(rest) {
            Ok((variables, claim)) => {
                start_action(app, config, ChatAction::Falsify { variables, claim });
            }
            Err(e) => {
                app.history.push(cell::notice_cell(&format!(
                    "usage: /falsify <variables-json> <claim> ({e})"
                )));
            }
        },
        "/sweep" => start_action(app, config, ChatAction::Sweep),
        "/agent" => {
            app.busy = true;
            app.status = "working (agent loop)…".into();
            event::spawn_agent(worker_inputs(app, config));
        }
        // --- Instant switches / inspectors: run synchronously. ---
        "/model" => handle_model(app, rest),
        "/project" => handle_project(app, store, rest),
        "/new" => handle_new(app, store, rest),
        "/help" => app.history.push(help_cell()),
        "/verify" => inspect_verify(app, store),
        "/graph" | "/obligations" | "/attempts" | "/events" | "/status" => inspect(app, store, cmd),
        "/proposals" => inspect_proposals(app, store),
        "/approve" => handle_approve(app, store, provider, rest),
        "/reject" => handle_reject(app, store, provider, rest),
        other => {
            app.history.push(cell::notice_cell(&format!(
                "unknown command: {other} (try /help)"
            )));
        }
    }
}

/// Mark busy and spawn a single closed-set action on the worker.
fn start_action(app: &mut App, config: &Config, action: ChatAction) {
    app.busy = true;
    app.status = format!("working ({})…", action.tool_name());
    event::spawn_action(worker_inputs(app, config), action);
}

// ===========================================================================
// Synchronous commands (instant DB reads and switches)
// ===========================================================================

/// `/project` with no arg lists projects (marking the current); with a name it
/// switches, rebuilding the transcript from the new project's history.
fn handle_project(app: &mut App, store: &Store, rest: &str) {
    match store.list_projects() {
        Ok(projects) if rest.is_empty() => {
            let mut body = Vec::new();
            for p in &projects {
                let here = if p.id == app.project_id { "* " } else { "  " };
                body.push(format!("{here}{:<20} {}", p.name, p.theorem));
            }
            body.push(String::new());
            body.push("/project <name> to switch    /new <name> | <theorem> to create".into());
            app.history
                .push(plain_cell(&format!("{} project(s)", projects.len()), body));
        }
        Ok(projects) => match projects.iter().find(|p| p.name == rest) {
            Some(p) => {
                app.project_id = p.id.clone();
                app.history = opening_history(store, &app.project_id);
                app.scroll = None;
                app.history.push(cell::notice_cell(&format!(
                    "switched to project '{}'",
                    p.name
                )));
            }
            None => app.history.push(cell::notice_cell(&format!(
                "no project named '{rest}' (try /project)"
            ))),
        },
        Err(e) => app
            .history
            .push(cell::error_cell(&format!("could not list projects: {e}"))),
    }
}

/// `/new <name> | <theorem>`: create a project and switch to it.
fn handle_new(app: &mut App, store: &Store, rest: &str) {
    let (name, theorem) = match rest.split_once('|') {
        Some((n, t)) => (n.trim(), t.trim()),
        None => ("", ""),
    };
    if name.is_empty() || theorem.is_empty() {
        app.history
            .push(cell::notice_cell("usage: /new <name> | <theorem>"));
        return;
    }
    match store.create_project(name, theorem) {
        Ok(p) => {
            app.project_id = p.id;
            app.history = opening_history(store, &app.project_id);
            app.scroll = None;
            app.history.push(cell::notice_cell(&format!(
                "created and switched to '{name}'"
            )));
        }
        Err(e) => app
            .history
            .push(cell::error_cell(&format!("could not create project: {e}"))),
    }
}

/// The read-only inspectors that render as a titled plain cell.
fn inspect(app: &mut App, store: &Store, cmd: &str) {
    let project_id = app.project_id.clone();
    let result: Result<(String, Vec<String>)> = (|| {
        let mut out = Vec::new();
        let title = match cmd {
            "/graph" => {
                let nodes = store.nodes(&project_id)?;
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
                let nodes = store.nodes(&project_id)?;
                let obligations: Vec<_> = nodes
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
                let attempts = store.attempts(&project_id, 30)?;
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
                let events = store.events(&project_id, 40)?;
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
                let p = store.project(&project_id)?;
                out.push(format!("Project:  {}", p.name));
                out.push(format!("Theorem:  {}", p.theorem));
                p.name
            }
            _ => "".into(),
        };
        Ok((title, out))
    })();
    match result {
        Ok((title, body)) => app.history.push(plain_cell(&title, body)),
        Err(e) => app
            .history
            .push(cell::error_cell(&format!("{cmd} error: {e}"))),
    }
}

/// `/verify`: the verification census as a rich, glyphed cell. Reads the nodes
/// once and hands the presentation layer plain `(status, layer, title)` rows plus
/// the verified/total counts; the cell enforces the honesty rule (only
/// `formally_verified` renders green).
fn inspect_verify(app: &mut App, store: &Store) {
    match store.nodes(&app.project_id) {
        Ok(nodes) => {
            let verified = nodes
                .iter()
                .filter(|n| n.status == NodeStatus::FormallyVerified)
                .count();
            let rows: Vec<(String, String, String)> = nodes
                .iter()
                .map(|n| {
                    let layer = if n.formal_statement.is_some() {
                        "formal"
                    } else {
                        "informal"
                    };
                    (n.status.to_string(), layer.to_string(), n.title.clone())
                })
                .collect();
            app.history
                .push(cell::verify_cell(verified, nodes.len(), rows));
        }
        Err(e) => app
            .history
            .push(cell::error_cell(&format!("/verify error: {e}"))),
    }
}

/// `/proposals`: render each pending proposal as its own proposal cell (with the
/// inline approve/reject affordance), or a notice when there are none.
fn inspect_proposals(app: &mut App, store: &Store) {
    match store.proposals(&app.project_id, true) {
        Ok(proposals) if proposals.is_empty() => {
            app.history.push(cell::notice_cell("No pending proposals"));
        }
        Ok(proposals) => {
            for p in &proposals {
                let summary = p.action["action"].as_str().unwrap_or("?").to_string();
                app.history.push(cell::proposal_cell(&p.id, &summary));
            }
        }
        Err(e) => app
            .history
            .push(cell::error_cell(&format!("/proposals error: {e}"))),
    }
}

fn handle_approve(app: &mut App, store: &Store, provider: &dyn ModelProvider, rest: &str) {
    let prefix = rest.split_whitespace().next().unwrap_or("");
    match resolve_proposal(store, &app.project_id, prefix).and_then(|id| {
        ChatEngine { store, provider }
            .approve(&app.project_id, &id)
            .map(|_| id)
    }) {
        Ok(id) => app.history.push(cell::notice_cell(&format!(
            "approved and applied {}",
            &id[..8.min(id.len())]
        ))),
        Err(e) => app
            .history
            .push(cell::error_cell(&format!("/approve error: {e}"))),
    }
}

fn handle_reject(app: &mut App, store: &Store, provider: &dyn ModelProvider, rest: &str) {
    let mut it = rest.splitn(2, ' ');
    let prefix = it.next().unwrap_or("");
    let reason = it.next().unwrap_or("rejected in TUI").trim();
    let reason = if reason.is_empty() {
        "rejected in TUI"
    } else {
        reason
    };
    match resolve_proposal(store, &app.project_id, prefix).and_then(|id| {
        ChatEngine { store, provider }
            .reject(&app.project_id, &id, reason)
            .map(|_| id)
    }) {
        Ok(id) => app.history.push(cell::notice_cell(&format!(
            "rejected {}",
            &id[..8.min(id.len())]
        ))),
        Err(e) => app
            .history
            .push(cell::error_cell(&format!("/reject error: {e}"))),
    }
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

// ===========================================================================
// The /model picker (ollama)
// ===========================================================================

/// `/model` with no arg lists ollama models (marking the current one); with a
/// name it validates against that list and switches by setting [`MODEL_ENV`].
/// We shell out to `ollama list` (the crate has no HTTP client); a name is only
/// accepted if it appeared in that list, so no arbitrary string reaches the env.
fn handle_model(app: &mut App, arg: &str) {
    let current = std::env::var(MODEL_ENV).ok();
    let models = match ollama_models() {
        Ok(m) => m,
        Err(e) => {
            app.history.push(cell::error_cell(&format!(
                "/model: could not run `ollama list`: {e} (is ollama installed and on PATH?)"
            )));
            return;
        }
    };
    if arg.is_empty() {
        let mut body = vec![
            format!(
                "current {MODEL_ENV} = {}",
                current.as_deref().unwrap_or("(unset; adapter default)")
            ),
            String::new(),
            "available ollama models:".into(),
        ];
        let cur_bare = current.as_deref().map(bare_name);
        if models.is_empty() {
            body.push("  (none installed)".into());
        }
        for m in &models {
            let marker = if Some(m.as_str()) == cur_bare {
                " * (current)"
            } else {
                ""
            };
            body.push(format!("  {m}{marker}"));
        }
        body.push(String::new());
        body.push("switch with: /model <name>".into());
        app.history.push(plain_cell("ollama models", body));
        return;
    }
    let bare = bare_name(arg).to_string();
    if !models.iter().any(|m| m == &bare) {
        let mut body = vec![format!("unknown model: {bare}"), "available:".into()];
        body.extend(models.iter().map(|m| format!("  {m}")));
        app.history.push(plain_cell("/model rejected", body));
        return;
    }
    let stored = stored_name(&bare);
    std::env::set_var(MODEL_ENV, &stored);
    app.history.push(cell::notice_cell(&format!(
        "active model set to {stored} (takes effect next turn)"
    )));
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

/// Parse the first column (NAME) out of `ollama list` output, skipping the
/// header row. Pure, so it is unit-tested directly.
fn parse_ollama_list(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .enumerate()
        .filter_map(|(i, line)| {
            if i == 0 {
                return None; // header row: NAME  ID  SIZE  MODIFIED
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

// ===========================================================================
// Argument parsing shared with the old cockpit
// ===========================================================================

/// Split an optional leading formal-system token off a `/prove` argument.
/// Defaults to Lean when the first token is not a system name.
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
/// take the remaining text as the claim. A streaming deserializer lets the
/// variables object contain spaces without a fragile manual split.
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
// Transcript hydration and simple cells
// ===========================================================================

/// Rebuild the transcript from the durable conversation log. Called on launch
/// and on every project switch; thereafter cells are appended in memory as
/// things happen, so a frame no longer re-queries the DB.
/// The transcript a session (or a project switch) opens with: the welcome card
/// first, then any prior conversation. The card names the product, the active
/// model, and the project, so the very first screen explains what this is.
fn opening_history(store: &Store, project_id: &str) -> Vec<Cell> {
    let project = store
        .project(project_id)
        .map(|p| p.name)
        .unwrap_or_else(|_| project_id.to_string());
    let mut cells: Vec<Cell> = vec![cell::welcome_cell(&current_model(), &project)];
    cells.extend(hydrate(store, project_id));
    cells
}

fn hydrate(store: &Store, project_id: &str) -> Vec<Cell> {
    let mut cells: Vec<Cell> = Vec::new();
    if let Ok(messages) = store.messages(project_id, 100) {
        for m in messages {
            let c = match m.role.as_str() {
                "user" => cell::user_cell(&m.content),
                "tool" => cell::notice_cell(&m.content),
                _ => cell::agent_cell(&m.content),
            };
            cells.push(c);
        }
    }
    cells
}

/// A local, deliberately-plain cell for the read-only inspectors: a bold title
/// then the raw lines. Inspectors need not be fancy; they only need to join the
/// transcript and scroll with it.
struct PlainCell {
    title: String,
    body: Vec<String>,
}

impl cell::HistoryCell for PlainCell {
    fn lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut out = vec![Line::from(Span::styled(
            format!("\u{2022} {}", self.title),
            Style::default().add_modifier(Modifier::BOLD),
        ))];
        for l in &self.body {
            out.push(Line::raw(l.clone()));
        }
        out
    }
}

fn plain_cell(title: &str, body: Vec<String>) -> Cell {
    Box::new(PlainCell {
        title: title.to_string(),
        body,
    })
}

/// The `/help` cell: a header plus the command reference generated FROM the
/// single command registry, so it can never drift from what dispatch knows.
fn help_cell() -> Cell {
    let mut lines = vec![Line::from(Span::styled(
        "\u{2022} commands".to_string(),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    lines.extend(command_popup::help_lines());
    lines.push(Line::raw("".to_string()));
    lines.push(Line::from(Span::styled(
        "plain text talks to the agent (it may prove/falsify/hammer/sweep). \
         Enter sends, Shift+Enter newline, Esc interrupts/clears, PageUp/Down or wheel scroll."
            .to_string(),
        Style::default().add_modifier(Modifier::DIM),
    )));
    Box::new(RawCell { lines })
}

/// A cell that renders a fixed set of pre-built lines (used for `/help`).
struct RawCell {
    lines: Vec<Line<'static>>,
}
impl cell::HistoryCell for RawCell {
    fn lines(&self, _width: u16) -> Vec<Line<'static>> {
        self.lines.clone()
    }
}

// ===========================================================================
// Scrolling
// ===========================================================================

/// Scroll the transcript up (toward older content) by `delta` rows, leaving
/// auto-follow. Uses the last drawn geometry to clamp.
fn scroll_up(app: &mut App, delta: usize) {
    let max = app.last_total.saturating_sub(app.last_vh);
    app.scroll = scrolled_up(app.scroll, max, delta);
}

/// Scroll the transcript down (toward newer content); reaching the bottom
/// restores auto-follow (`None`).
fn scroll_down(app: &mut App, delta: usize) {
    let max = app.last_total.saturating_sub(app.last_vh);
    app.scroll = scrolled_down(app.scroll, max, delta);
}

/// Pure scroll math (extracted so it is testable without a TTY). `None` means
/// "stuck to the bottom"; scrolling up from there starts at the bottom (`max`).
fn scrolled_up(scroll: Option<usize>, max: usize, delta: usize) -> Option<usize> {
    let cur = scroll.unwrap_or(max);
    Some(cur.saturating_sub(delta).min(max))
}

/// Scrolling down: past the bottom snaps back to auto-follow (`None`).
fn scrolled_down(scroll: Option<usize>, max: usize, delta: usize) -> Option<usize> {
    let cur = scroll.unwrap_or(max);
    let next = cur + delta;
    if next >= max {
        None
    } else {
        Some(next)
    }
}

// ===========================================================================
// Rendering
// ===========================================================================

fn idle_status() -> String {
    "ready · type to chat, or / for commands".into()
}

fn current_model() -> String {
    std::env::var(MODEL_ENV).unwrap_or_else(|_| "(adapter default)".into())
}

/// The graph panel's width in columns, or 0 when it must NOT be shown. Returns 0
/// unless the panel is requested AND the terminal is wide enough to keep both a
/// readable transcript (>= `TRANSCRIPT_MIN`) and a minimum panel (>= `MIN`). A
/// pure function so the "too narrow, keep it off" rule is unit-testable.
fn graph_width(total: u16, show: bool) -> u16 {
    const MIN: u16 = 24;
    const TRANSCRIPT_MIN: u16 = 30;
    if !show || total < MIN + TRANSCRIPT_MIN {
        return 0;
    }
    // Aim for ~35% of the width, but never below the minimum, and never so wide
    // that the transcript drops under its own minimum.
    let want = (total as u32 * 35 / 100) as u16;
    want.max(MIN).min(total - TRANSCRIPT_MIN)
}

/// How many transcript lines sit BELOW the current viewport, for the scroll-up
/// indicator. Zero while auto-following (`None`) or already at the bottom. Pure,
/// so the indicator math is unit-testable without a TTY.
fn lines_below(scroll: Option<usize>, total: usize, vh: usize) -> usize {
    let max_top = total.saturating_sub(vh);
    match scroll {
        None => 0,
        Some(t) => max_top.saturating_sub(t.min(max_top)),
    }
}

/// Map a store `Node`'s presentation fields to a `PanelNode`. Kept as a small
/// free function taking the enum types (not a whole `Node`) so the mapping, and
/// in particular that status/kind go through their `Display`, is unit-testable
/// without constructing a full `Node`. The honesty rule rides along for free:
/// only `NodeStatus::FormallyVerified` stringifies to "formally_verified", the
/// single status the panel paints green.
fn panel_node(id: &str, title: &str, status: NodeStatus, kind: NodeKind) -> PanelNode {
    PanelNode {
        id: id.to_string(),
        title: title.to_string(),
        status: status.to_string(),
        kind: kind.to_string(),
    }
}

/// Draw one full frame. All layout math and line-building happens up front so we
/// can record the transcript geometry (`last_total`, `last_vh`) for the scroll
/// keys; the closure then renders the pre-built widgets.
fn draw(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    store: &Store,
) -> Result<()> {
    let size = terminal.size()?;
    let full = Rect::new(0, 0, size.width, size.height);

    // Whichever completion popup is active reserves the height; they are mutually
    // exclusive, so at most one is non-zero.
    let popup_h = app.popup.height().max(app.mention.height());
    // The composer is a full bordered box: input height plus a top and bottom
    // border. A one-row gap above the input zone keeps the transcript from
    // butting straight into the box, which was the "squeezed" look.
    let composer_h = app.composer.desired_height(size.width).max(1) + 2;
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),             // transcript (scrolls)
            Constraint::Length(1),          // breathing room above the input zone
            Constraint::Length(popup_h),    // completion popup (0 when inactive)
            Constraint::Length(composer_h), // the composer box
            Constraint::Length(1),          // one-line status footer
        ])
        .split(full);
    let (top_area, popup_area, composer_area, footer_area) =
        (areas[0], areas[2], areas[3], areas[4]);

    // Split the transcript row horizontally when the graph panel is shown AND
    // fits: transcript on the left, a one-column gap, the panel on the right.
    let gw = graph_width(size.width, app.show_graph);
    let (transcript_area, graph_area) = if gw > 0 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(1),     // transcript
                Constraint::Length(1),  // gap
                Constraint::Length(gw), // graph panel
            ])
            .split(top_area);
        (cols[0], Some(cols[2]))
    } else {
        (top_area, None)
    };
    // The panel was requested but the terminal is too narrow: say so in the
    // footer rather than silently dropping it.
    let graph_too_narrow = app.show_graph && gw == 0;

    // Build the whole transcript into one line list at the transcript width.
    // Committed cells never change; only the streamed preview re-renders each
    // frame, so this stays cheap for a modest history.
    let w = transcript_area.width;
    let mut lines: Vec<Line<'static>> = Vec::new();
    for c in &app.history {
        lines.extend(c.lines(w));
    }
    if let Some(s) = &app.active_stream {
        lines.extend(cell::agent_cell(s).lines(w));
    } else if app.busy {
        // A worker is running but not streaming (an action, or before the first
        // token): show a working line so the transcript is not silent.
        lines.push(Line::from(Span::styled(
            format!("{} working…", cell::theme::spinner_frame(app.tick)),
            cell::theme::working(),
        )));
    }

    let total = lines.len();
    let vh = transcript_area.height as usize;
    let max_top = total.saturating_sub(vh);
    let top = match app.scroll {
        None => max_top,
        Some(t) => t.min(max_top),
    };
    app.last_total = total;
    app.last_vh = vh;
    let below = lines_below(app.scroll, total, vh);

    // Build the graph panel lines (a cheap store read; no worker needed). Done
    // outside the draw closure so a read error cannot poison the frame.
    let graph_lines: Vec<Line<'static>> = match graph_area {
        Some(area) => {
            let panel_nodes: Vec<PanelNode> = store
                .nodes(&app.project_id)
                .map(|ns| {
                    ns.iter()
                        .map(|n| panel_node(&n.id, &n.title, n.status, n.kind))
                        .collect()
                })
                .unwrap_or_default();
            app.graph.lines(&panel_nodes, area.width, area.height)
        }
        None => Vec::new(),
    };

    // Exactly one popup contributes lines (the other returns empty when inactive).
    let mut popup_lines = app.popup.lines(popup_area.width);
    popup_lines.extend(app.mention.lines(popup_area.width));
    let composer_lines = app.composer.lines(composer_area.width);
    let footer = footer_line(app, graph_too_narrow);

    terminal.draw(|f| {
        f.render_widget(
            Paragraph::new(lines).scroll((top as u16, 0)),
            transcript_area,
        );
        // The scroll-up indicator overlays the bottom transcript row so the user
        // knows there is more below while reading history.
        if below > 0 && transcript_area.height > 0 {
            let y = transcript_area.y + transcript_area.height - 1;
            let rect = Rect::new(transcript_area.x, y, transcript_area.width, 1);
            let hint = format!("  \u{2193} {below} more below  ");
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(hint, cell::theme::dim()))),
                rect,
            );
        }
        if let Some(area) = graph_area {
            f.render_widget(Paragraph::new(graph_lines), area);
        }
        if popup_h > 0 {
            f.render_widget(Paragraph::new(popup_lines), popup_area);
        }
        f.render_widget(
            Paragraph::new(composer_lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().add_modifier(Modifier::DIM))
                    .title(Span::styled(" message ", cell::theme::dim())),
            ),
            composer_area,
        );
        f.render_widget(footer, footer_area);
    })?;
    Ok(())
}

/// The footer/status line: the active model, the busy/idle state (with a
/// spinner while a worker runs), the current hint, and a compact key-hint strip.
/// The working indicator is truthful because the UI is live during the turn,
/// unlike the old frozen "(blocking; please wait)". `graph_too_narrow` swaps the
/// key hints for a note when the graph panel was toggled on but does not fit.
fn footer_line(app: &App, graph_too_narrow: bool) -> Paragraph<'static> {
    let state = if app.busy {
        Span::styled(
            format!("{} busy", cell::theme::spinner_frame(app.tick)),
            cell::theme::working(),
        )
    } else {
        Span::styled("idle".to_string(), cell::theme::dim())
    };
    // The context hints: the discoverability strip for the panel, popups, and
    // quit, or the "too narrow" note when the graph cannot fit.
    let hints = if graph_too_narrow {
        "  \u{b7}  graph hidden: terminal too narrow".to_string()
    } else {
        "  \u{b7}  Ctrl+G graph  \u{b7}  / commands  \u{b7}  @ nodes  \u{b7}  Ctrl-C quit"
            .to_string()
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" model {} ", current_model()),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled("· ".to_string(), cell::theme::dim()),
        state,
        Span::styled(format!("  · {}", app.status), cell::theme::dim()),
        Span::styled(hints, cell::theme::dim()),
    ]);
    Paragraph::new(line)
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
        assert!(parse_falsify_args(r#"{"n":"int"}"#).is_err());
        assert!(parse_falsify_args(r#"[1,2] x > 0"#).is_err());
    }

    // ---- The new pure logic: key routing and scroll math ----

    #[test]
    fn popup_consumes_only_navigation_keys() {
        assert!(popup_consumes(KeyCode::Up));
        assert!(popup_consumes(KeyCode::Down));
        assert!(popup_consumes(KeyCode::Tab));
        assert!(popup_consumes(KeyCode::Enter));
        // Editing keys are never stolen by the popup; they must reach the composer.
        assert!(!popup_consumes(KeyCode::Char('a')));
        assert!(!popup_consumes(KeyCode::Backspace));
        assert!(!popup_consumes(KeyCode::Left));
        assert!(!popup_consumes(KeyCode::Esc));
    }

    #[test]
    fn scroll_up_from_bottom_starts_at_max_then_clamps_to_zero() {
        // None means "stuck to bottom" (== max). One step up leaves max-delta.
        assert_eq!(scrolled_up(None, 100, 10), Some(90));
        assert_eq!(scrolled_up(Some(5), 100, 10), Some(0));
        // Cannot scroll above the top.
        assert_eq!(scrolled_up(Some(0), 100, 10), Some(0));
    }

    #[test]
    fn scroll_down_past_bottom_restores_autofollow() {
        // From a pinned position, stepping down but not reaching the bottom stays pinned.
        assert_eq!(scrolled_down(Some(80), 100, 10), Some(90));
        // Reaching or passing the bottom snaps back to auto-follow (None).
        assert_eq!(scrolled_down(Some(95), 100, 10), None);
        assert_eq!(scrolled_down(None, 100, 10), None);
    }

    #[test]
    fn plain_cell_renders_title_and_body() {
        let c = plain_cell("2 nodes", vec!["a".into(), "b".into()]);
        let lines = c.lines(80);
        assert_eq!(lines.len(), 3); // title + 2 body lines
        let title: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(title.contains("2 nodes"));
    }

    // ---- The graph-panel focus routing ----

    #[test]
    fn graph_nav_claims_up_down_only_when_shown_and_focused() {
        // Shown and focused: Up/Down become panel navigation.
        assert_eq!(graph_nav(true, true, KeyCode::Up), Some(GraphNav::Prev));
        assert_eq!(graph_nav(true, true, KeyCode::Down), Some(GraphNav::Next));
        // Other keys are never claimed, so typing/enter reach the composer.
        assert_eq!(graph_nav(true, true, KeyCode::Enter), None);
        assert_eq!(graph_nav(true, true, KeyCode::Char('a')), None);
        // Not focused (even if shown): Up/Down keep their normal meaning.
        assert_eq!(graph_nav(true, false, KeyCode::Up), None);
        assert_eq!(graph_nav(true, false, KeyCode::Down), None);
        // Not shown: never claimed regardless of the focus flag.
        assert_eq!(graph_nav(false, true, KeyCode::Up), None);
    }

    // ---- The @-mention token replacement ----

    #[test]
    fn replace_trailing_mention_swaps_last_token() {
        // A mention mid-message: only the trailing @token is replaced.
        assert_eq!(
            replace_trailing_mention("prove @lem", "lemma_3"),
            "prove @lemma_3 "
        );
        // A message that is only a mention.
        assert_eq!(replace_trailing_mention("@lem", "lemma_1"), "@lemma_1 ");
        // A bare '@' completes to the chosen node.
        assert_eq!(replace_trailing_mention("@", "root"), "@root ");
        // Preserves everything before the token verbatim, including other words.
        assert_eq!(
            replace_trailing_mention("see also @foo bar @ba", "baz"),
            "see also @foo bar @baz "
        );
    }

    // ---- The graph-width fit rule ----

    #[test]
    fn graph_width_zero_when_hidden_or_too_narrow() {
        assert_eq!(graph_width(200, false), 0); // hidden: never shows
        assert_eq!(graph_width(40, true), 0); // too narrow: min panel + transcript won't fit
                                              // Wide enough: ~35% of the width, at least the 24-col minimum.
        assert_eq!(graph_width(100, true), 35);
        // Just over the threshold gives the minimum panel, transcript keeps 30.
        assert_eq!(graph_width(54, true), 24);
        // The transcript is never starved below its minimum of 30.
        let gw = graph_width(60, true);
        assert!(gw >= 24 && 60 - gw >= 30);
    }

    // ---- The scroll-indicator math ----

    #[test]
    fn lines_below_zero_at_bottom_positive_when_scrolled_up() {
        // Auto-follow (None) is always at the bottom: nothing below.
        assert_eq!(lines_below(None, 100, 20), 0);
        // max_top = 80. Pinned at 60 leaves 20 rows below.
        assert_eq!(lines_below(Some(60), 100, 20), 20);
        // Pinned at the very bottom: nothing below.
        assert_eq!(lines_below(Some(80), 100, 20), 0);
        // A stale scroll past max clamps, never underflows.
        assert_eq!(lines_below(Some(999), 100, 20), 0);
        // Content shorter than the viewport: nothing below.
        assert_eq!(lines_below(Some(0), 10, 20), 0);
    }

    // ---- The Node -> PanelNode mapping (status/kind via Display) ----

    #[test]
    fn panel_node_maps_status_and_kind_via_display() {
        let n = panel_node(
            "abc",
            "My lemma",
            NodeStatus::FormallyVerified,
            NodeKind::Lemma,
        );
        assert_eq!(n.id, "abc");
        assert_eq!(n.title, "My lemma");
        // The one status the panel paints green stringifies exactly as it expects.
        assert_eq!(n.status, "formally_verified");
        assert_eq!(n.kind, "lemma");
        // A non-verified status must NOT stringify to the verified token, so the
        // panel can never be tricked into greening it.
        let open = panel_node("x", "t", NodeStatus::Active, NodeKind::Obligation);
        assert_eq!(open.status, "active");
        assert_ne!(open.status, "formally_verified");
    }
}
