use crate::{
    chat::ChatEngine,
    db::Store,
    model::{Event, Node},
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
        status: "Ready · Tab changes pane · Ctrl-C exits · /help lists commands".into(),
        selected: 0,
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
                            app.status = slash(store, provider, &app.project_id, &text)?;
                        } else {
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

fn slash(
    store: &Store,
    provider: &dyn ModelProvider,
    project_id: &str,
    text: &str,
) -> Result<String> {
    let mut parts = text.splitn(3, ' ');
    Ok(match parts.next().unwrap_or("") {
        "/help" => {
            "Commands: /help /graph /events /status /proposals /approve ID /reject ID [reason]"
                .into()
        }
        "/graph" => format!("{} graph nodes", store.nodes(project_id)?.len()),
        "/events" => format!("{} recent events", store.events(project_id, 100)?.len()),
        "/status" => {
            let p = store.project(project_id)?;
            format!("{} · {}", p.name, p.theorem)
        }
        "/proposals" => {
            let proposals = store.proposals(project_id, true)?;
            if proposals.is_empty() {
                "No pending proposals".into()
            } else {
                proposals
                    .iter()
                    .map(|proposal| {
                        format!(
                            "{}  {}",
                            &proposal.id[..8],
                            proposal.action["action"].as_str().unwrap_or("?")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" · ")
            }
        }
        "/approve" => {
            let prefix = parts.next().unwrap_or("");
            let proposal = resolve_proposal(store, project_id, prefix)?;
            ChatEngine { store, provider }.approve(project_id, &proposal)?;
            format!("Approved and applied {proposal}")
        }
        "/reject" => {
            let prefix = parts.next().unwrap_or("");
            let reason = parts.next().unwrap_or("rejected in TUI");
            let proposal = resolve_proposal(store, project_id, prefix)?;
            ChatEngine { store, provider }.reject(project_id, &proposal, reason)?;
            format!("Rejected {proposal}")
        }
        other => format!("Unknown command: {other}"),
    })
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
