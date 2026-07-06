use crate::{
    chat::ChatEngine,
    db::Store,
    model::{Event, Node, NodeKind, NodeStatus},
    provider::ModelProvider,
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
use std::{io, time::Duration};

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
    /// Detailed output lines from the last slash command; when non-empty they
    /// take over the main pane until cleared with Esc.
    output: Vec<String>,
}

pub fn run(store: &Store, provider: &dyn ModelProvider, project_id: &str) -> Result<()> {
    store.project(project_id)?;
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    let result = run_loop(&mut terminal, store, provider, project_id);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    store: &Store,
    provider: &dyn ModelProvider,
    project_id: &str,
) -> Result<()> {
    let mut app = App {
        project_id: project_id.into(),
        input: String::new(),
        pane: Pane::Chat,
        status: "Ready · Tab changes pane · /help lists commands · Esc clears · Ctrl-C exits"
            .into(),
        selected: 0,
        output: Vec::new(),
    };
    loop {
        let nodes = store.nodes(project_id)?;
        let events = store.events(project_id, 80)?;
        let messages = store.messages(project_id, 100)?;
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
            if !app.output.is_empty() {
                render_command_output(f, cols[0], &app.output);
            } else {
            match app.pane {
                Pane::Chat => {
                    let lines = messages
                        .iter()
                        .flat_map(|m| {
                            let color = if m.role == "user" {
                                Color::Cyan
                            } else {
                                Color::Green
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
                        app.selected = (app.selected + 1).min(nodes.len().saturating_sub(1))
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
                            match slash(store, provider, &app.project_id, &text) {
                                Ok((status, output)) => {
                                    app.status = status;
                                    app.output = output;
                                }
                                Err(e) => {
                                    app.status = format!("Command error: {e}");
                                    app.output.clear();
                                }
                            }
                        } else {
                            app.output.clear();
                            app.status = "Agent working…".into();
                            let engine = ChatEngine { store, provider };
                            match engine.send(&app.project_id, &text) {
                                Ok(_) => {
                                    app.status =
                                        "Response committed to conversation and graph".into()
                                }
                                Err(e) => app.status = format!("Agent error: {e}"),
                            }
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

/// Handle a slash command. Returns a short status line plus detailed output
/// lines (rendered in the main pane until cleared with Esc).
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
                let mark = if a.success { "✓" } else { "×" };
                let node = a
                    .node_id
                    .as_deref()
                    .map(|s| &s[..8.min(s.len())])
                    .unwrap_or("—");
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
            format!("Approved and applied {}", &proposal[..8.min(proposal.len())])
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
            out.push("Run `theoremata run <project>` to drive verification.".into());
            format!("{}/{} nodes formally verified", verified, nodes.len())
        }
        other => {
            out.push("Type /help for the command reference.".into());
            format!("Unknown command: {other}")
        }
    };
    Ok((status, out))
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
                "formally_verified" => "✓",
                "rejected" => "×",
                "blocked" => "!",
                "active" => "◆",
                _ => "·",
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
