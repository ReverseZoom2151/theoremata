//! The slash-command registry and its popup.
//!
//! WHY this module exists: today the chat's slash commands live in three
//! hand-maintained places (the flat parser, the picker, and the help text) that
//! drift out of sync. This collapses them into ONE source of truth, `registry`,
//! from which `/help` and the popup are both generated so they can never
//! disagree again.
//!
//! Dependencies are intentionally just ratatui + crossterm: the popup produces
//! styled lines for the integrator to draw and reports its own height; it never
//! touches the store, the model, or the composer's text buffer.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// One row of the command registry. All fields are static string slices because
/// the command set is fixed at compile time; keeping them `&'static` lets
/// `registry` hand out `&'static` references with no allocation or lifetime
/// juggling for callers.
pub struct CommandSpec {
    /// The full command token the user types, leading slash included (e.g. the
    /// literal "/prove"). Matching and completion both key off this.
    pub name: &'static str,
    /// A one-line argument hint. Angle-bracket and pipe glyphs here are DATA for
    /// display, not markup.
    pub args: &'static str,
    /// A single line of help shown in the popup and in the generated reference.
    pub help: &'static str,
}

/// The single registry of every chat command. WHY a function returning a static
/// slice rather than a `const`: it is the one public entry point other code
/// (help generation, the popup, and the integrator's dispatch) reads, so there
/// is exactly one list to edit when a command is added or removed.
///
/// Grouped logically: session and project setup, then the work actions, then
/// the read-only inspectors, then proposal handling, then help.
pub fn registry() -> &'static [CommandSpec] {
    &REGISTRY
}

static REGISTRY: [CommandSpec; 18] = [
    // Session and project setup.
    CommandSpec {
        name: "/model",
        args: "[name]",
        help: "list local models, or switch the active one",
    },
    CommandSpec {
        name: "/project",
        args: "[name]",
        help: "list projects, or switch to one",
    },
    CommandSpec {
        name: "/new",
        args: "<name> | <thm>",
        help: "create a project and switch to it",
    },
    // Work actions (may run for minutes).
    CommandSpec {
        name: "/prove",
        args: "[sys] <node|statement>",
        help: "formalize, prove, and gate",
    },
    CommandSpec {
        name: "/hammer",
        args: "<sys> <goal>",
        help: "hammer-assisted native proof plus gate",
    },
    CommandSpec {
        name: "/falsify",
        args: "<json> <claim>",
        help: "numeric counterexample search",
    },
    CommandSpec {
        name: "/sweep",
        args: "",
        help: "staleness census for this project",
    },
    CommandSpec {
        name: "/agent",
        args: "",
        help: "run the autonomous loop on this project",
    },
    // Read-only inspectors.
    CommandSpec {
        name: "/graph",
        args: "",
        help: "list every node",
    },
    CommandSpec {
        name: "/obligations",
        args: "",
        help: "obligation nodes with hints",
    },
    CommandSpec {
        name: "/attempts",
        args: "",
        help: "recent solver attempts",
    },
    CommandSpec {
        name: "/events",
        args: "",
        help: "recent trajectory events",
    },
    CommandSpec {
        name: "/verify",
        args: "",
        help: "verification status of every node",
    },
    CommandSpec {
        name: "/status",
        args: "",
        help: "project name and theorem",
    },
    // Proposal handling.
    CommandSpec {
        name: "/proposals",
        args: "",
        help: "pending graph-mutation proposals",
    },
    CommandSpec {
        name: "/approve",
        args: "<id>",
        help: "approve a pending proposal",
    },
    CommandSpec {
        name: "/reject",
        args: "<id> [reason]",
        help: "reject a pending proposal",
    },
    // Help.
    CommandSpec {
        name: "/help",
        args: "",
        help: "this reference",
    },
];

/// The style used to make a command name stand out (in help and the popup).
fn name_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

/// The dim style for argument hints, so the eye lands on the name first.
fn dim_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// Render one registry row as a styled line: bold name, dim args, plain help.
/// Shared by `help_lines` and the popup so a row looks the same everywhere.
fn spec_line(spec: &CommandSpec, selected: bool) -> Line<'static> {
    // Pad the name to a fixed column so the help text lines up into a readable
    // second column regardless of command length.
    let name = format!("{:<13}", spec.name);
    let args = if spec.args.is_empty() {
        String::new()
    } else {
        format!("{} ", spec.args)
    };
    let help = spec.help.to_string();
    let mut spans = vec![
        Span::styled(name, name_style()),
        Span::styled(args, dim_style()),
        Span::raw(help),
    ];
    if selected {
        // A selected row reverses every span so the whole row highlights, which
        // reads clearly on any terminal palette without picking a background.
        for s in &mut spans {
            s.style = s.style.add_modifier(Modifier::REVERSED);
        }
    }
    Line::from(spans)
}

/// The `/help` body, generated from the registry so it can never drift from the
/// commands the popup and dispatch actually know about. Exactly one line per
/// command, in registry order.
pub fn help_lines() -> Vec<Line<'static>> {
    registry().iter().map(|s| spec_line(s, false)).collect()
}

/// The most rows the popup will ever show at once. WHY a cap: with a bare "/"
/// every command matches, and a popup taller than a handful of rows would eat
/// the transcript. Extra matches are simply not drawn.
const MAX_ROWS: usize = 8;

/// The slash-command popup state. It is ACTIVE only while the composer holds a
/// partial command name (starts with '/', no space yet); once a space is typed
/// the user is past the name and into arguments, so the popup steps aside.
pub struct CommandPopup {
    /// True while the composer text is a partial command name.
    active: bool,
    /// The typed text WITHOUT the leading slash, e.g. "pr" for "/pr". Stored so
    /// matching is a pure function of this filter.
    filter: String,
    /// Index into the ranked match list of the highlighted row.
    selected: usize,
}

impl Default for CommandPopup {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandPopup {
    pub fn new() -> Self {
        CommandPopup {
            active: false,
            filter: String::new(),
            selected: 0,
        }
    }

    /// Feed the current composer text on every keystroke. The popup activates
    /// only when the text starts with '/' and contains no space yet: a space
    /// means the command name is finished and the user is typing arguments, so
    /// completing the name no longer makes sense.
    pub fn sync(&mut self, composer_text: &str) {
        let active = composer_text.starts_with('/') && !composer_text.contains(' ');
        // The filter is everything after the leading slash. When inactive we
        // clear it so a stale filter never leaks into a later match query.
        let new_filter = if active {
            composer_text[1..].to_string()
        } else {
            String::new()
        };
        // Reset the selection to the top whenever the filter changes, so the
        // best (top-ranked) match is always the default as the user narrows.
        if new_filter != self.filter {
            self.selected = 0;
        }
        self.active = active;
        self.filter = new_filter;
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Ranked matches for the current partial command, best first. WHY this
    /// ranking (exact, then prefix, then substring, each alphabetical): it is
    /// cheap and predictable and matches how people expect a command menu to
    /// behave, without the surprises of a fuzzy scorer. Typing "/pr" surfaces
    /// the prefix group /project, /proposals, /prove in alphabetical order.
    pub fn matches(&self) -> Vec<&'static CommandSpec> {
        // Compare on the body (name without the slash) against the filter (also
        // slash-stripped) so a substring like "ammer" can match "hammer".
        let f = self.filter.as_str();
        let mut ranked: Vec<(u8, &'static CommandSpec)> = registry()
            .iter()
            .filter_map(|spec| {
                let body = spec.name.strip_prefix('/').unwrap_or(spec.name);
                let rank = if body == f {
                    0u8
                } else if body.starts_with(f) {
                    1
                } else if body.contains(f) {
                    2
                } else {
                    return None;
                };
                Some((rank, spec))
            })
            .collect();
        // Sort by rank first, then alphabetically by name within a rank. This is
        // stable and total, so the order is deterministic for a given filter.
        ranked.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(b.1.name)));
        ranked.into_iter().map(|(_, spec)| spec).collect()
    }

    /// Handle a navigation key while the popup is active. Up and Down move the
    /// selection (wrapping at the ends), Tab and Enter accept the highlighted
    /// command. Returns the accepted command name so the integrator can complete
    /// the composer to it; returns None for a key the popup does not consume.
    pub fn key(&mut self, key: KeyEvent) -> Option<&'static str> {
        let matches = self.matches();
        if matches.is_empty() {
            return None;
        }
        // Keep the selection in range in case the match list shrank since the
        // last keystroke (a narrower filter drops rows).
        if self.selected >= matches.len() {
            self.selected = matches.len() - 1;
        }
        match key.code {
            KeyCode::Up => {
                // Wrap to the bottom when moving up off the top row.
                self.selected = if self.selected == 0 {
                    matches.len() - 1
                } else {
                    self.selected - 1
                };
                None
            }
            KeyCode::Down => {
                // Wrap to the top when moving down off the last row.
                self.selected = (self.selected + 1) % matches.len();
                None
            }
            KeyCode::Tab | KeyCode::Enter => Some(matches[self.selected].name),
            _ => None,
        }
    }

    /// The number of visible rows the popup occupies (matches, capped). The
    /// integrator uses this to reserve height above the composer. When active
    /// with no matches we still show a single empty-state line, so the height is
    /// at least one while active and zero when inactive.
    pub fn height(&self) -> u16 {
        if !self.active {
            return 0;
        }
        let visible = self.matches().len().min(MAX_ROWS);
        visible.max(1) as u16
    }

    /// The popup rendered to styled lines for drawing above the composer, with
    /// the selected row highlighted. The `width` is accepted for the caller's
    /// layout budget; rows are short two-column entries that fit comfortably, so
    /// no per-line truncation is applied here.
    pub fn lines(&self, _width: u16) -> Vec<Line<'static>> {
        if !self.active {
            return Vec::new();
        }
        let matches = self.matches();
        if matches.is_empty() {
            // Honest empty state rather than a blank box, so a mistyped command
            // reads as "no such command" instead of a silent no-op.
            return vec![Line::styled(
                "  no matching command".to_string(),
                dim_style(),
            )];
        }
        let sel = self.selected.min(matches.len() - 1);
        matches
            .iter()
            .take(MAX_ROWS)
            .enumerate()
            .map(|(i, spec)| spec_line(spec, i == sel))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent};

    fn kev(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    #[test]
    fn no_duplicate_command_names() {
        let names: Vec<&str> = registry().iter().map(|s| s.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "registry has a duplicate name");
    }

    #[test]
    fn help_has_one_line_per_command() {
        assert_eq!(help_lines().len(), registry().len());
    }

    #[test]
    fn active_only_before_a_space() {
        let mut p = CommandPopup::new();
        p.sync("/pr");
        assert!(p.is_active());
        p.sync("/prove ");
        assert!(!p.is_active());
        p.sync("hello");
        assert!(!p.is_active());
        p.sync("/");
        assert!(p.is_active());
    }

    #[test]
    fn prefix_group_is_alphabetical() {
        let mut p = CommandPopup::new();
        p.sync("/pr");
        let names: Vec<&str> = p.matches().iter().map(|s| s.name).collect();
        // All three prefix matches present, in alphabetical order.
        let pos = |n: &str| names.iter().position(|x| *x == n);
        assert!(pos("/project").is_some());
        assert!(pos("/proposals").is_some());
        assert!(pos("/prove").is_some());
        assert!(pos("/project") < pos("/proposals"));
        assert!(pos("/proposals") < pos("/prove"));
    }

    #[test]
    fn exact_ranks_first() {
        let mut p = CommandPopup::new();
        p.sync("/prove");
        // "/prove" is an exact body match and must sort ahead of "/proposals".
        assert_eq!(p.matches()[0].name, "/prove");
    }

    #[test]
    fn down_then_enter_returns_second_match() {
        let mut p = CommandPopup::new();
        p.sync("/pr");
        let second = p.matches()[1].name;
        assert_eq!(p.key(kev(KeyCode::Down)), None);
        assert_eq!(p.key(kev(KeyCode::Enter)), Some(second));
    }

    #[test]
    fn selection_wraps() {
        let mut p = CommandPopup::new();
        p.sync("/pr");
        let n = p.matches().len();
        // Up from the top wraps to the last row.
        assert_eq!(p.key(kev(KeyCode::Up)), None);
        let last = p.matches()[n - 1].name;
        assert_eq!(p.key(kev(KeyCode::Tab)), Some(last));
    }

    #[test]
    fn substring_matches_when_no_prefix() {
        let mut p = CommandPopup::new();
        p.sync("/ammer");
        assert_eq!(p.matches()[0].name, "/hammer");
    }
}
