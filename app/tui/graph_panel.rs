//! The proof-graph side panel: a first-class, togglable RIGHT-hand column that
//! renders the proof-DAG's nodes with their status while you chat.
//!
//! Theoremata is a GRAPH-FIRST prover: the proof-DAG is its core artifact. The
//! redesign made the transcript the full-width spine and demoted the graph to a
//! plain-text dump; this module is the first-class replacement. The integrator
//! toggles it (Ctrl+G) and splits the layout, handing us plain data and a
//! width/height. We keep this module deliberately self-contained: it depends
//! ONLY on `ratatui` and takes plain `PanelNode` data, never a crate-internal
//! type, so the panel stays independent and unit-testable in isolation.
//!
//! The one rule that matters here is the HONESTY RULE (see `status_glyph`): a
//! non-verified node must never read as a green success. Only `formally_verified`
//! earns the green check; every other status is neutral/dim or a warning/failure
//! color. The presentation layer can describe a node's status but can never
//! upgrade it to "verified".

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// One node of the proof-DAG, as plain data (the integrator maps the store's
/// `Node` to this so the panel never touches a crate-internal type).
pub struct PanelNode {
    pub id: String,
    pub title: String,
    pub status: String, // e.g. "formally_verified", "open", "in_progress", "failed"
    pub kind: String,   // e.g. "theorem", "lemma", "obligation", "definition"
}

// ---------------------------------------------------------------------------
// Glyph + color vocabulary. Kept local (small equivalents) rather than importing
// the transcript's `cell.rs` theme, per the module's isolation contract. The
// glyphs match the render study's fixed set so status reads at a glance.
// ---------------------------------------------------------------------------

const VERIFIED: &str = "\u{2714}"; // heavy check ✔, green: the ONLY success glyph
const FAILED: &str = "\u{2717}"; // ballot x ✗, red
const WORKING: &str = "\u{25d0}"; // half circle ◐, cyan: in progress
const BULLET: &str = "\u{2022}"; // bullet •, dim: open / neutral
const UNKNOWN: &str = "?"; // unknown status, dim
const VBAR: &str = "\u{2502}"; // vertical rule │, the panel's left border
const ELLIPSIS: char = '\u{2026}'; // … for truncated titles

fn green_bold() -> Style {
    Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD)
}
fn red() -> Style {
    Style::default().fg(Color::Red)
}
fn cyan() -> Style {
    Style::default().fg(Color::Cyan)
}
fn magenta() -> Style {
    Style::default().fg(Color::Magenta)
}
fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}
fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// The status glyph + its color. THE HONESTY RULE LIVES HERE: only a node whose
/// status is exactly `formally_verified` gets the green check. Everything else
/// is neutral/dim (open, unknown), cyan (in progress), or red (failed). No
/// status string other than `formally_verified` can ever return a green check,
/// so a mock/open/in-progress/failed node can never be misread as a verified
/// success. This mirrors the transcript's VerdictCell honesty gate.
fn status_glyph(status: &str) -> (&'static str, Style) {
    match status {
        "formally_verified" => (VERIFIED, green_bold()),
        "failed" => (FAILED, red()),
        "in_progress" => (WORKING, cyan()),
        "open" => (BULLET, dim()),
        // Anything we do not recognise is neutral, NEVER green. Absence of a
        // known-verified status is not a verification.
        _ => (UNKNOWN, dim()),
    }
}

/// Truncate `s` to at most `max` display columns (char count as a close-enough
/// proxy, avoiding a unicode-width dependency), inserting a trailing ellipsis
/// when it does not fit so a long title can never overflow the column.
fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    match max {
        0 => String::new(),
        1 => ELLIPSIS.to_string(),
        _ => {
            let mut t: String = s.chars().take(max - 1).collect();
            t.push(ELLIPSIS);
            t
        }
    }
}

/// The panel widget: holds a selection cursor and the last-known node count, and
/// renders the node list to styled lines for a right-hand column.
pub struct GraphPanel {
    selected: usize,
    // Last node count seen by `lines`. `lines` takes `&self` (it is called every
    // render frame), yet `select_next` must clamp against the list length without
    // being handed the nodes; a `Cell` lets the render path record the length so
    // navigation can honour it. This models real use: the panel is rendered
    // continuously, so the count is always fresh by the time a key arrives.
    len: std::cell::Cell<usize>,
}

impl Default for GraphPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphPanel {
    pub fn new() -> Self {
        GraphPanel {
            selected: 0,
            len: std::cell::Cell::new(0),
        }
    }

    /// Move the selection up. Clamps at 0. No-op when already at the top (and,
    /// since it only ever decrements, a no-op on an empty list too).
    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Move the selection down. Clamps at len-1. No-op when the node list is
    /// empty (len 0 -> max index 0 -> never advances past 0).
    pub fn select_next(&mut self) {
        let max = self.len.get().saturating_sub(1);
        if self.selected < max {
            self.selected += 1;
        }
    }

    /// The current selection index, clamped to the last-known node count so a
    /// list that shrank never reports an out-of-range cursor.
    pub fn selected(&self) -> usize {
        let max = self.len.get().saturating_sub(1);
        self.selected.min(max)
    }

    /// Render the panel for `nodes` into `width` columns and `height` rows:
    /// a header, one row per node (status glyph + truncated title) with the
    /// selected row highlighted, a scroll window that keeps the selection
    /// visible with a "+K more" marker when nodes exceed the rows, and a short
    /// legend line at the bottom if it fits. Never panics on an empty graph or a
    /// degenerate size.
    pub fn lines(&self, nodes: &[PanelNode], width: u16, height: u16) -> Vec<Line<'static>> {
        // Record the count so navigation can clamp (see the `len` field note).
        self.len.set(nodes.len());

        let h = height as usize;
        let mut out: Vec<Line<'static>> = Vec::new();
        if h == 0 || width == 0 {
            return out;
        }

        // Content area sits to the right of the 2-column border ("│ ").
        let content_w = (width as usize).saturating_sub(2);

        // Header: "PROOF GRAPH  N nodes", bold, truncated to the column.
        let header = truncate(
            &format!("PROOF GRAPH  {} nodes", nodes.len()),
            content_w.max(1),
        );
        out.push(bordered(vec![Span::styled(header, bold())]));
        if out.len() >= h {
            out.truncate(h);
            return out;
        }

        // A legend only earns a row when the panel is tall enough to spare one
        // AND the column is wide enough to read it. Nodes take priority.
        let want_legend = h >= 4 && content_w >= 10;
        let mut body_rows = h - 1;
        if want_legend {
            body_rows -= 1;
        }

        // Body: the empty-graph line, or the scrolled node window.
        let mut body: Vec<Line<'static>> = Vec::new();
        if nodes.is_empty() {
            // Empty graph: a quiet placeholder, never a crash.
            body.push(bordered(vec![Span::styled(
                "no nodes yet".to_string(),
                dim(),
            )]));
        } else {
            body.extend(self.node_window(nodes, content_w, body_rows));
        }

        // Pad so the legend sits at the bottom and the column reads as a
        // distinct panel even when sparsely filled.
        while body.len() < body_rows {
            body.push(Line::from(Span::styled(VBAR.to_string(), dim())));
        }
        body.truncate(body_rows);
        out.extend(body);

        if want_legend {
            out.push(legend_line(content_w));
        }

        out.truncate(h);
        out
    }

    // Render the visible slice of nodes for `rows` rows of node area, keeping the
    // selection inside the window and appending a "+K more" marker when nodes are
    // hidden below the window.
    fn node_window(
        &self,
        nodes: &[PanelNode],
        content_w: usize,
        rows: usize,
    ) -> Vec<Line<'static>> {
        let total = nodes.len();
        let cap = rows.max(1);
        let eff_selected = self.selected.min(total.saturating_sub(1));

        // Decide the window [start, start+visible) and how many nodes remain
        // hidden below it (the marker count).
        let (start, visible, hidden) = if total <= cap {
            (0, total, 0)
        } else {
            // Reserve the last row for the "+K more" marker.
            let vis = cap - 1;
            // Push the selection to the bottom edge once it scrolls past the
            // window, clamped so we never scroll beyond the final page.
            let mut start = if eff_selected >= vis {
                eff_selected + 1 - vis
            } else {
                0
            };
            let max_start = total - vis;
            if start > max_start {
                start = max_start;
            }
            let hidden = total - (start + vis);
            if hidden == 0 {
                // Scrolled to the very bottom: nothing remains below, so reclaim
                // the reserved marker row for one more node rather than print a
                // meaningless "+0 more".
                (total - cap, cap, 0)
            } else {
                (start, vis, hidden)
            }
        };

        let mut lines = Vec::new();
        for (offset, node) in nodes[start..start + visible].iter().enumerate() {
            let idx = start + offset;
            lines.push(self.node_line(node, idx == eff_selected, content_w));
        }
        if hidden > 0 {
            lines.push(bordered(vec![Span::styled(
                format!("+{hidden} more"),
                dim(),
            )]));
        }
        lines
    }

    // One node row: the left border, then a status glyph + space + truncated
    // title. The selected row is highlighted (reverse video) and its content is
    // padded to the full column width so the highlight fills the row.
    fn node_line(&self, node: &PanelNode, selected: bool, content_w: usize) -> Line<'static> {
        let (glyph, gstyle) = status_glyph(&node.status);
        // Glyph (1 col) + space (1 col) leave this much for the title.
        let avail = content_w.saturating_sub(2);
        let title = truncate(&node.title, avail);

        let hl = |s: Style| {
            if selected {
                s.add_modifier(Modifier::REVERSED)
            } else {
                s
            }
        };
        // When selected, pad the title out to fill the content width so the
        // reverse-video block spans the whole column, not just the text.
        let title_text = if selected {
            let pad = avail.saturating_sub(title.chars().count());
            format!("{title}{}", " ".repeat(pad))
        } else {
            title
        };

        Line::from(vec![
            // Border stays un-highlighted so the column rule reads consistently.
            Span::styled(format!("{VBAR} "), dim()),
            Span::styled(glyph.to_string(), hl(gstyle)),
            Span::styled(" ".to_string(), hl(Style::default())),
            Span::styled(title_text, hl(Style::default())),
        ])
    }
}

// Prefix a set of content spans with the panel's left border column.
fn bordered(mut spans: Vec<Span<'static>>) -> Line<'static> {
    let mut out = vec![Span::styled(format!("{VBAR} "), dim())];
    out.append(&mut spans);
    Line::from(out)
}

// The bottom legend: colored glyph + label pairs, added only while they fit the
// column so a narrow panel simply shows fewer items (never overflows).
fn legend_line(content_w: usize) -> Line<'static> {
    let items: [(&str, &str, Style); 4] = [
        (VERIFIED, "verified", green_bold()),
        (BULLET, "open", dim()),
        (WORKING, "working", cyan()),
        (FAILED, "failed", red()),
    ];
    let mut spans = vec![Span::styled(format!("{VBAR} "), dim())];
    let mut used = 0usize;
    for (glyph, label, style) in items {
        // "glyph label  " -> glyph(1) + space(1) + label + two trailing spaces.
        let cost = 1 + 1 + label.chars().count() + 2;
        if used + cost > content_w {
            break;
        }
        spans.push(Span::styled(glyph.to_string(), style));
        spans.push(Span::styled(format!(" {label}  "), dim()));
        used += cost;
    }
    // The proposal accent color is part of our vocabulary but has no legend slot;
    // reference it so the intent (magenta = proposal) stays documented in code.
    let _ = magenta;
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, title: &str, status: &str, kind: &str) -> PanelNode {
        PanelNode {
            id: id.to_string(),
            title: title.to_string(),
            status: status.to_string(),
            kind: kind.to_string(),
        }
    }

    fn all_spans(lines: &[Line<'static>]) -> Vec<Span<'static>> {
        lines.iter().flat_map(|l| l.spans.clone()).collect()
    }

    fn joined(lines: &[Line<'static>]) -> String {
        all_spans(lines)
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn empty_graph_renders_placeholder_no_panic() {
        let panel = GraphPanel::new();
        let lines = panel.lines(&[], 30, 10);
        let text = joined(&lines);
        assert!(text.contains("PROOF GRAPH  0 nodes"));
        assert!(text.contains("no nodes yet"));
        // The placeholder row itself never reads as a green success. (The
        // bottom legend may carry a green "verified" KEY glyph; that is a
        // legend, not a node, so we check the placeholder line specifically.)
        let placeholder = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("no nodes yet")))
            .expect("placeholder line present");
        assert!(placeholder
            .spans
            .iter()
            .all(|s| s.style.fg != Some(Color::Green)));
    }

    #[test]
    fn empty_graph_select_is_noop() {
        let mut panel = GraphPanel::new();
        let _ = panel.lines(&[], 30, 10);
        panel.select_next();
        panel.select_prev();
        assert_eq!(panel.selected(), 0);
    }

    #[test]
    fn verified_node_is_green_check_open_node_is_not() {
        let nodes = vec![
            node("1", "Verified thm", "formally_verified", "theorem"),
            node("2", "Open goal", "open", "lemma"),
        ];
        let panel = GraphPanel::new();
        let lines = panel.lines(&nodes, 40, 12);
        let spans = all_spans(&lines);

        // The verified node carries the green check glyph in green.
        assert!(spans
            .iter()
            .any(|s| s.content.contains(VERIFIED) && s.style.fg == Some(Color::Green)));

        // The open node's glyph is the dim bullet and is NEVER green: an
        // unverified node must not read as a green success (the honesty rule).
        let bullet = spans
            .iter()
            .find(|s| s.content.as_ref() == BULLET)
            .expect("open node bullet present");
        assert_ne!(bullet.style.fg, Some(Color::Green));
        assert!(!bullet.content.contains(VERIFIED));
    }

    #[test]
    fn status_glyph_only_verified_is_green() {
        assert_eq!(status_glyph("formally_verified").1.fg, Some(Color::Green));
        for s in ["open", "in_progress", "failed", "unknown", "definition", ""] {
            assert_ne!(
                status_glyph(s).1.fg,
                Some(Color::Green),
                "status {s:?} must not be green"
            );
        }
        assert_eq!(status_glyph("failed").1.fg, Some(Color::Red));
        assert_eq!(status_glyph("in_progress").1.fg, Some(Color::Cyan));
    }

    #[test]
    fn selection_clamps_at_zero_and_len_minus_one() {
        let nodes: Vec<PanelNode> = (0..5)
            .map(|i| node(&i.to_string(), &format!("n{i}"), "open", "lemma"))
            .collect();
        let mut panel = GraphPanel::new();
        let _ = panel.lines(&nodes, 30, 12); // registers len = 5

        for _ in 0..20 {
            panel.select_next();
        }
        assert_eq!(panel.selected(), 4); // clamps at len-1

        for _ in 0..20 {
            panel.select_prev();
        }
        assert_eq!(panel.selected(), 0); // clamps at 0
    }

    #[test]
    fn more_nodes_than_rows_shows_marker_and_keeps_selection_visible() {
        let nodes: Vec<PanelNode> = (0..20)
            .map(|i| node(&i.to_string(), &format!("node {i}"), "open", "lemma"))
            .collect();
        let mut panel = GraphPanel::new();

        // Small panel: height 6 -> 1 header + legend + a few node rows.
        let lines = panel.lines(&nodes, 30, 6);
        let text = joined(&lines);
        // A "+K more" marker appears because nodes exceed the rows.
        assert!(text.contains(" more"));
        assert!(text.contains('+'));

        // Selection at 0 is visible: node 0's title is rendered.
        assert!(text.contains("node 0"));

        // Drive selection to the end; the selected node stays within the window.
        for _ in 0..25 {
            panel.select_next();
        }
        assert_eq!(panel.selected(), 19);
        let lines2 = panel.lines(&nodes, 30, 6);
        let text2 = joined(&lines2);
        assert!(
            text2.contains("node 19"),
            "selected node must stay within the scroll window, got: {}",
            text2
        );
    }

    #[test]
    fn titles_truncate_to_width() {
        let long = "this is an extremely long theorem title that will not fit";
        let nodes = vec![node("1", long, "open", "theorem")];
        let panel = GraphPanel::new();
        // Narrow column: width 16 -> content 14 -> title area 12.
        let lines = panel.lines(&nodes, 16, 8);
        let spans = all_spans(&lines);
        // Some span carries the ellipsis, and the full title never appears.
        let text = joined(&lines);
        assert!(text.contains(ELLIPSIS));
        assert!(!text.contains(long));
        // No rendered content span exceeds the column width.
        for s in &spans {
            assert!(
                s.content.chars().count() <= 16,
                "span {:?} wider than column",
                s.content
            );
        }
    }

    #[test]
    fn selected_row_is_highlighted() {
        let nodes = vec![
            node("1", "first", "open", "lemma"),
            node("2", "second", "open", "lemma"),
        ];
        let panel = GraphPanel::new();
        let lines = panel.lines(&nodes, 30, 10);
        // The first (selected) node's title span carries the reverse modifier.
        let has_reversed = all_spans(&lines).iter().any(|s| {
            s.content.contains("first") && s.style.add_modifier.contains(Modifier::REVERSED)
        });
        assert!(has_reversed, "selected row should be highlighted");
    }

    #[test]
    fn tiny_height_does_not_panic_and_shows_header() {
        let nodes = vec![node("1", "t", "open", "lemma")];
        let panel = GraphPanel::new();
        for hgt in 0..4u16 {
            let lines = panel.lines(&nodes, 20, hgt);
            assert!(lines.len() <= hgt as usize);
        }
        // Height 1: just the (truncated) header.
        let one = panel.lines(&nodes, 20, 1);
        assert_eq!(one.len(), 1);
        assert!(joined(&one).contains("PROOF GRAPH"));
    }
}
