//! The scrolling-transcript CELL model plus its theme.
//!
//! A transcript is a list of typed cells; each renders itself to styled
//! `ratatui` lines at a given width and reports its height. This module is
//! deliberately self-contained: it depends ONLY on `ratatui` and `serde_json`
//! and never on a crate-internal type, so every cell takes plain data. That
//! keeps the presentation layer independent and unit-testable in isolation,
//! and it enforces the one rule that matters here (see `verdict_cell`): the
//! renderer can describe a result but can never manufacture a verification.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

/// A unit in the scrolling transcript. Renders itself to styled lines at a
/// given width and reports how many rows it will occupy.
pub trait HistoryCell: Send {
    fn lines(&self, width: u16) -> Vec<Line<'static>>;
    // Default height is just the rendered line count. Cells re-render from
    // owned source data on each call, so this stays consistent with `lines`.
    fn height(&self, width: u16) -> u16 {
        self.lines(width).len() as u16
    }
}

pub type Cell = Box<dyn HistoryCell>;

// ---------------------------------------------------------------------------
// Theme: the semantic glyph + color vocabulary. Every cell pulls its styling
// from here rather than sprinkling raw colors at call sites, so a future
// light/dark or palette change is a single-file edit. The glyph set is the
// same one the render study fixed: verified/failed/mock/proposal all read at a
// glance and, crucially, the honesty warning has its own distinct color.
// ---------------------------------------------------------------------------
pub mod theme {
    use super::*;

    // Glyph vocabulary. These are plain unicode marks, not markup.
    pub const VERIFIED: &str = "\u{2714}"; // heavy check, green
    pub const FAILED: &str = "\u{2717}"; // ballot x, red
    pub const WARN: &str = "\u{26a0}"; // warning sign, yellow
    pub const PROPOSAL: &str = "\u{25c6}"; // black diamond, magenta
    pub const BULLET: &str = "\u{2022}"; // bullet, dim/info
    pub const REPAIR: &str = "\u{27f3}"; // clockwise arrow, stale/repair
    pub const UNKNOWN: &str = "?"; // unknown bucket
    pub const RULE: &str = "\u{2500}"; // horizontal rule, totals
    pub const USER_PREFIX: &str = "\u{203a} "; // single guillemet, user accent

    // A small spinner cycle for the working state. Reduced-motion callers can
    // just use `BULLET` instead of animating.
    pub const SPINNER: [&str; 8] = [
        "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}",
        "\u{2827}",
    ];

    pub fn spinner_frame(tick: usize) -> &'static str {
        SPINNER[tick % SPINNER.len()]
    }

    // Named semantic styles. A verification tool should read as calm with a
    // few strong status accents, so most body text is plain or dim and only
    // status glyphs carry color.
    pub fn verified() -> Style {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    }
    pub fn failed() -> Style {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    }
    pub fn warn() -> Style {
        // Yellow, NOT bold-green: this is the color of "not a verification".
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    }
    pub fn working() -> Style {
        Style::default().fg(Color::Cyan)
    }
    pub fn proposal() -> Style {
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD)
    }
    pub fn user() -> Style {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    }
    pub fn unknown() -> Style {
        Style::default().fg(Color::DarkGray)
    }
    pub fn error() -> Style {
        Style::default().fg(Color::Red)
    }
    pub fn dim() -> Style {
        Style::default().add_modifier(Modifier::DIM)
    }
    pub fn bold() -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }
    pub fn italic_dim() -> Style {
        Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC)
    }
    pub fn code_span() -> Style {
        // Inline code and proof body read dim so prose stays primary.
        Style::default().add_modifier(Modifier::DIM)
    }
    pub fn code_keyword() -> Style {
        // A light tint for proof keywords; deliberately quiet.
        Style::default().fg(Color::Blue)
    }
    pub fn code_comment() -> Style {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM)
    }

    /// A "label: glyph" status line. Green check when `ok`, red cross when not.
    /// This is the small building block for pass/fail rows; it is NOT how the
    /// verdict header is built (that has three-way honesty logic below).
    pub fn status_line(label: &str, ok: bool) -> Line<'static> {
        let (glyph, style) = if ok {
            (VERIFIED, verified())
        } else {
            (FAILED, failed())
        };
        Line::from(vec![
            Span::styled(glyph.to_string(), style),
            Span::raw(format!(" {label}")),
        ])
    }

    /// A gate row that treats a false value as a yellow warning rather than a
    /// hard red failure. Used for gate layers (mock backend, unpreserved
    /// statement) where "false" means "not a verification", not "broken".
    pub fn gate_line(label: &str, ok: bool, warn_label: &str) -> Line<'static> {
        if ok {
            Line::from(vec![
                Span::styled(VERIFIED.to_string(), verified()),
                Span::raw(format!(" {label}")),
            ])
        } else {
            Line::from(vec![
                Span::styled(WARN.to_string(), warn()),
                Span::raw(format!(" {warn_label}")),
            ])
        }
    }
}

// ---------------------------------------------------------------------------
// Text helpers: width-aware wrapping and the head/collapse truncation marker.
// ---------------------------------------------------------------------------

/// Word-wrap `text` to `width` columns. Newlines are hard breaks; a single
/// word longer than the width is hard-split so nothing overflows the pane.
/// Width is measured in chars (a close enough proxy without pulling in a
/// unicode-width dependency, which the contract does not allow here).
fn wrap_text(text: &str, width: u16) -> Vec<String> {
    let w = (width as usize).max(1);
    let mut out = Vec::new();
    for raw in text.split('\n') {
        if raw.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut cur = String::new();
        for word in raw.split(' ') {
            let wlen = word.chars().count();
            let clen = cur.chars().count();
            if cur.is_empty() {
                if wlen > w {
                    hard_split_into(word, w, &mut out, &mut cur);
                } else {
                    cur.push_str(word);
                }
            } else if clen + 1 + wlen <= w {
                cur.push(' ');
                cur.push_str(word);
            } else {
                out.push(std::mem::take(&mut cur));
                if wlen > w {
                    hard_split_into(word, w, &mut out, &mut cur);
                } else {
                    cur.push_str(word);
                }
            }
        }
        out.push(cur);
    }
    out
}

// Break an over-long word into width-sized chunks, flushing full chunks to
// `out` and leaving the trailing partial chunk in `cur`.
fn hard_split_into(word: &str, w: usize, out: &mut Vec<String>, cur: &mut String) {
    let mut chunk = String::new();
    for ch in word.chars() {
        if chunk.chars().count() == w {
            out.push(std::mem::take(&mut chunk));
        }
        chunk.push(ch);
    }
    *cur = chunk;
}

/// Render long code/output collapsed: wrap to `width`, keep the first
/// `head_max` rows, and if more remain, append a dim `... +K lines` marker
/// instead of dumping the whole blob into the viewport. Visible code rows get
/// a light syntax tint. This is the single truncation helper the render study
/// asked for; both proof code and long sweep bodies flow through it.
pub fn output_lines(text: &str, width: u16, head_max: usize) -> Vec<Line<'static>> {
    let wrapped = wrap_text(text, width);
    let total = wrapped.len();
    let mut lines = Vec::new();
    if total <= head_max {
        for l in wrapped {
            lines.push(tint_code_line(&l));
        }
    } else {
        for l in wrapped.iter().take(head_max) {
            lines.push(tint_code_line(l));
        }
        let k = total - head_max;
        // The collapse marker: never styled as a success, just quiet meta.
        lines.push(Line::from(Span::styled(
            format!("... +{k} lines"),
            theme::dim(),
        )));
    }
    lines
}

// A light, language-agnostic tint for a single proof-code line: comment lines
// dim, a small set of common Lean/Rocq/Isabelle keywords lightly accented,
// everything else plain-dim so code reads as a quiet fenced block.
fn tint_code_line(line: &str) -> Line<'static> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("--") || trimmed.starts_with("(*") || trimmed.starts_with("//") {
        return Line::from(Span::styled(line.to_string(), theme::code_comment()));
    }
    const KEYWORDS: [&str; 16] = [
        "theorem",
        "lemma",
        "def",
        "Proof",
        "Qed",
        "by",
        "exact",
        "intro",
        "intros",
        "apply",
        "have",
        "show",
        "sorry",
        "admit",
        "using",
        "assumption",
    ];
    let first = trimmed
        .split(|c: char| c.is_whitespace())
        .next()
        .unwrap_or("");
    let style = if KEYWORDS.contains(&first) {
        theme::code_keyword()
    } else {
        theme::code_span()
    };
    Line::from(Span::styled(line.to_string(), style))
}

// Wrap a body with a first-line prefix and a hanging (subsequent-line) prefix,
// styling both prefixes dim. Used by user/reasoning/notice-style cells.
fn prefixed_wrapped(
    text: &str,
    width: u16,
    first_prefix: &str,
    subseq_prefix: &str,
    prefix_style: Style,
    body_style: Option<Style>,
) -> Vec<Line<'static>> {
    let indent = first_prefix
        .chars()
        .count()
        .max(subseq_prefix.chars().count());
    let avail = (width as usize).saturating_sub(indent).max(1) as u16;
    let wrapped = wrap_text(text, avail);
    let mut lines = Vec::new();
    for (i, seg) in wrapped.iter().enumerate() {
        let pfx = if i == 0 { first_prefix } else { subseq_prefix };
        let mut spans = Vec::new();
        if !pfx.is_empty() {
            spans.push(Span::styled(pfx.to_string(), prefix_style));
        }
        match body_style {
            Some(s) => spans.push(Span::styled(seg.clone(), s)),
            None => spans.push(Span::raw(seg.clone())),
        }
        lines.push(Line::from(spans));
    }
    if lines.is_empty() {
        lines.push(Line::from(Vec::<Span>::new()));
    }
    lines
}

// Parse inline light markdown (`code` spans and **bold**) within one already
// wrapped segment into styled spans. Markers that happen to straddle a wrap
// boundary simply render literally; that is acceptable for light markdown.
fn inline_spans(s: &str) -> Vec<Span<'static>> {
    let chars: Vec<char> = s.chars().collect();
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut plain = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '`' {
            if let Some(end) = find_from(&chars, i + 1, |c| c == '`') {
                if !plain.is_empty() {
                    out.push(Span::raw(std::mem::take(&mut plain)));
                }
                let code: String = chars[i + 1..end].iter().collect();
                out.push(Span::styled(code, theme::code_span()));
                i = end + 1;
                continue;
            }
        }
        if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            if let Some(end) = find_double_star(&chars, i + 2) {
                if !plain.is_empty() {
                    out.push(Span::raw(std::mem::take(&mut plain)));
                }
                let b: String = chars[i + 2..end].iter().collect();
                out.push(Span::styled(b, theme::bold()));
                i = end + 2;
                continue;
            }
        }
        plain.push(chars[i]);
        i += 1;
    }
    if !plain.is_empty() {
        out.push(Span::raw(plain));
    }
    if out.is_empty() {
        out.push(Span::raw(String::new()));
    }
    out
}

fn find_from(chars: &[char], start: usize, pred: impl Fn(char) -> bool) -> Option<usize> {
    (start..chars.len()).find(|&j| pred(chars[j]))
}

fn find_double_star(chars: &[char], start: usize) -> Option<usize> {
    let mut j = start;
    while j + 1 < chars.len() {
        if chars[j] == '*' && chars[j + 1] == '*' {
            return Some(j);
        }
        j += 1;
    }
    None
}

// Render light markdown: bullet lines (`- ` / `* `) become a dim bullet with a
// hanging indent, other lines get inline bold/code styling.
fn render_markdown(text: &str, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for src in text.split('\n') {
        let trimmed = src.trim_start();
        let bullet = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "));
        let (first_pfx, body, subseq_pfx) = match bullet {
            Some(rest) => (
                format!("{} ", theme::BULLET),
                rest.to_string(),
                "  ".to_string(),
            ),
            None => (String::new(), src.to_string(), String::new()),
        };
        let indent = first_pfx.chars().count();
        let avail = (width as usize).saturating_sub(indent).max(1) as u16;
        let wrapped = wrap_text(&body, avail);
        for (i, seg) in wrapped.iter().enumerate() {
            let pfx = if i == 0 { &first_pfx } else { &subseq_pfx };
            let mut spans = Vec::new();
            if !pfx.is_empty() {
                spans.push(Span::styled(pfx.clone(), theme::dim()));
            }
            spans.extend(inline_spans(seg));
            lines.push(Line::from(spans));
        }
    }
    lines
}

// A field from a JSON value that may be number or string, best-effort to u64.
fn json_u64(v: &Value, key: &str) -> u64 {
    match &v[key] {
        Value::Number(n) => n.as_u64().unwrap_or(0),
        Value::String(s) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn json_str(v: &Value, key: &str) -> Option<String> {
    match &v[key] {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

// Render an assignment value (object {x:1,y:1}, string, or array) as a compact
// "x=1, y=1" string for a counterexample line.
fn render_assignment(v: &Value) -> Option<String> {
    match v {
        Value::Object(map) => {
            if map.is_empty() {
                return None;
            }
            let parts: Vec<String> = map
                .iter()
                .map(|(k, val)| format!("{k}={}", scalar(val)))
                .collect();
            Some(parts.join(", "))
        }
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Array(a) if !a.is_empty() => {
            Some(a.iter().map(scalar).collect::<Vec<_>>().join(", "))
        }
        _ => None,
    }
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Concrete cells. Each owns typed source data and re-renders at width.
// ---------------------------------------------------------------------------

struct UserCell {
    text: String,
}
impl HistoryCell for UserCell {
    fn lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut out = vec![Line::from(Vec::<Span>::new())]; // leading blank for breathing room
        out.extend(prefixed_wrapped(
            &self.text,
            width,
            theme::USER_PREFIX,
            "  ",
            theme::user(),
            None,
        ));
        out
    }
}

pub fn user_cell(text: &str) -> Cell {
    Box::new(UserCell {
        text: text.to_string(),
    })
}

struct AgentCell {
    text: String,
}
impl HistoryCell for AgentCell {
    fn lines(&self, width: u16) -> Vec<Line<'static>> {
        render_markdown(&self.text, width)
    }
}

pub fn agent_cell(text: &str) -> Cell {
    Box::new(AgentCell {
        text: text.to_string(),
    })
}

struct ReasoningCell {
    text: String,
}
impl HistoryCell for ReasoningCell {
    fn lines(&self, width: u16) -> Vec<Line<'static>> {
        prefixed_wrapped(
            &self.text,
            width,
            &format!("{} ", theme::BULLET),
            "  ",
            theme::dim(),
            Some(theme::italic_dim()),
        )
    }
}

pub fn reasoning_cell(text: &str) -> Cell {
    Box::new(ReasoningCell {
        text: text.to_string(),
    })
}

// The flagship cell. The honesty rule lives here and cannot be bypassed: the
// green check is derived from ALL four gates, so no combination of arguments
// can paint a mock or unpreserved result as verified.
struct VerdictCell {
    system: String,
    code: String,
    compiled: bool,
    axioms_clean: bool,
    statement_preserved: bool,
    live: bool,
}

// The overall pass predicate. A verdict is verified ONLY when it compiled with
// clean axioms, the statement was preserved, AND it ran on a live backend.
// This is intentionally a free function so the test pins the exact conjunction.
fn verdict_verified(
    compiled: bool,
    axioms_clean: bool,
    statement_preserved: bool,
    live: bool,
) -> bool {
    compiled && axioms_clean && statement_preserved && live
}

impl VerdictCell {
    // Header glyph/label/style, three-way:
    //  - all gates pass            -> green check, "Proved"
    //  - a hard gate failed        -> red cross,  "Prove failed"
    //  - compiled+clean but mock   -> yellow warn, "Not verified (...)"
    //    or unpreserved (a pass that is not a verification)
    fn header(&self) -> Line<'static> {
        let verified = verdict_verified(
            self.compiled,
            self.axioms_clean,
            self.statement_preserved,
            self.live,
        );
        let subject = format!("  {}", self.system);
        if verified {
            Line::from(vec![
                Span::styled(theme::VERIFIED.to_string(), theme::verified()),
                Span::styled(" Proved".to_string(), theme::bold()),
                Span::styled(subject, theme::dim()),
            ])
        } else if !self.compiled || !self.axioms_clean {
            // A genuine failure: it did not compile or the axioms are dirty.
            Line::from(vec![
                Span::styled(theme::FAILED.to_string(), theme::failed()),
                Span::styled(" Prove failed".to_string(), theme::bold()),
                Span::styled(subject, theme::dim()),
            ])
        } else {
            // Compiled with clean axioms but NOT a verification: mock backend
            // and/or the statement was not preserved. Yellow, never green.
            let mut reasons = Vec::new();
            if !self.live {
                reasons.push("mock");
            }
            if !self.statement_preserved {
                reasons.push("unpreserved");
            }
            let label = format!(" Not verified ({})", reasons.join(", "));
            Line::from(vec![
                Span::styled(theme::WARN.to_string(), theme::warn()),
                Span::styled(label, theme::bold()),
                Span::styled(subject, theme::dim()),
            ])
        }
    }
}

impl HistoryCell for VerdictCell {
    fn lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut out = vec![self.header()];
        // One row per gate layer, indented. compile/axioms are hard pass/fail
        // (red on failure); statement/live are honesty gates (yellow warning
        // on false, because "not preserved" / "mock" are non-verifications,
        // not crashes).
        let indent = |line: Line<'static>| {
            let mut spans = vec![Span::raw("  ".to_string())];
            spans.extend(line.spans);
            Line::from(spans)
        };
        out.push(indent(theme::status_line("compiled", self.compiled)));
        out.push(indent(theme::status_line(
            "axioms clean",
            self.axioms_clean,
        )));
        out.push(indent(theme::gate_line(
            "statement preserved",
            self.statement_preserved,
            "statement not preserved",
        )));
        out.push(indent(theme::gate_line(
            "live backend",
            self.live,
            "mock backend (not live)",
        )));
        // The proof code, collapsed with light tinting. Reserve two columns for
        // the tree marker indent.
        let code_lines = wrap_text(&self.code, width).len();
        out.push(Line::from(Span::styled(
            format!("  \u{2514} proof ({code_lines} lines)"),
            theme::dim(),
        )));
        for l in output_lines(&self.code, width.saturating_sub(2), 10) {
            let mut spans = vec![Span::raw("    ".to_string())];
            spans.extend(l.spans);
            out.push(Line::from(spans));
        }
        out
    }
}

pub fn verdict_cell(
    system: &str,
    code: &str,
    compiled: bool,
    axioms_clean: bool,
    statement_preserved: bool,
    live: bool,
) -> Cell {
    Box::new(VerdictCell {
        system: system.to_string(),
        code: code.to_string(),
        compiled,
        axioms_clean,
        statement_preserved,
        live,
    })
}

// A falsification attempt. Parses {"verdict","assignment","checked"}. The
// honesty rule for this cell: "no counterexample in domain" is NOT a proof and
// must render neutral (a dim bullet), never a green success. A found
// counterexample is a red refutation.
struct FalsifyCell {
    value: Value,
}
impl HistoryCell for FalsifyCell {
    fn lines(&self, width: u16) -> Vec<Line<'static>> {
        let verdict = json_str(&self.value, "verdict").unwrap_or_default();
        let checked = json_u64(&self.value, "checked");
        let assignment = render_assignment(&self.value["assignment"]);

        // Absence of a counterexample: neutral survived line, deliberately not
        // green and not a check glyph, because it is not a proof.
        let neutral = |width: u16| {
            let msg = if checked > 0 {
                format!(
                    "{} no counterexample in domain (checked {checked})",
                    theme::BULLET
                )
            } else {
                format!("{} no counterexample in domain", theme::BULLET)
            };
            prefixed_wrapped(&msg, width, "", "  ", theme::dim(), Some(theme::dim()))
        };

        if verdict == "no_counterexample_in_domain" || verdict == "no_counterexample" {
            return neutral(width);
        }
        match assignment {
            // A concrete counterexample was found: a red refutation.
            Some(a) => {
                vec![Line::from(vec![
                    Span::styled(theme::FAILED.to_string(), theme::failed()),
                    Span::styled(" counterexample ".to_string(), theme::bold()),
                    Span::styled(a, theme::failed()),
                ])]
            }
            // Unknown / inconclusive: still neutral, never green.
            None => neutral(width),
        }
    }
}

pub fn falsify_cell(value: &Value) -> Cell {
    Box::new(FalsifyCell {
        value: value.clone(),
    })
}

// The staleness census. Parses fresh / repair_candidate / mathematics_moved /
// unknown / total. A non-zero MathematicsMoved bucket gets a red accent so the
// eye lands on the thing that regressed.
struct SweepCell {
    value: Value,
}
impl HistoryCell for SweepCell {
    fn lines(&self, _width: u16) -> Vec<Line<'static>> {
        let fresh = json_u64(&self.value, "fresh");
        let repair = json_u64(&self.value, "repair_candidate");
        let moved = json_u64(&self.value, "mathematics_moved");
        let unknown = json_u64(&self.value, "unknown");
        let total = json_u64(&self.value, "total");

        let mut out = Vec::new();
        if let Some(summary) = json_str(&self.value, "summary") {
            out.push(Line::from(vec![
                Span::styled(format!("{} ", theme::BULLET), theme::dim()),
                Span::styled(summary, theme::bold()),
            ]));
        } else {
            out.push(Line::from(vec![
                Span::styled(format!("{} ", theme::BULLET), theme::dim()),
                Span::styled("Staleness sweep".to_string(), theme::bold()),
            ]));
        }

        let row = |glyph: &str, glyph_style: Style, label: &str, count: u64| {
            Line::from(vec![
                Span::raw("  ".to_string()),
                Span::styled(glyph.to_string(), glyph_style),
                Span::raw(format!(" {label:<18}{count:>5}")),
            ])
        };
        out.push(row(theme::VERIFIED, theme::verified(), "Fresh", fresh));
        out.push(row(theme::REPAIR, theme::warn(), "RepairCandidate", repair));
        // MathematicsMoved is the regression bucket: red glyph always, and when
        // non-zero the whole row is red so it cannot be skimmed past.
        if moved > 0 {
            out.push(Line::from(vec![
                Span::raw("  ".to_string()),
                Span::styled(theme::FAILED.to_string(), theme::failed()),
                Span::styled(
                    format!(" {:<18}{:>5}", "MathematicsMoved", moved),
                    theme::error(),
                ),
            ]));
        } else {
            out.push(row(
                theme::FAILED,
                theme::failed(),
                "MathematicsMoved",
                moved,
            ));
        }
        out.push(row(theme::UNKNOWN, theme::unknown(), "Unknown", unknown));
        out.push(Line::from(vec![
            Span::raw("  ".to_string()),
            Span::styled(theme::RULE.to_string(), theme::dim()),
            Span::styled(format!(" {:<18}{:>5}", "total", total), theme::dim()),
        ]));
        out
    }
}

pub fn sweep_cell(value: &Value) -> Cell {
    Box::new(SweepCell {
        value: value.clone(),
    })
}

// An autonomous agent run. Parses {run id, certified, steps}. `certified` here
// is the agent layer's own certification claim, so a certified run may show a
// green check; an uncertified run renders neutral.
struct AgentRunCell {
    value: Value,
}
impl HistoryCell for AgentRunCell {
    fn lines(&self, _width: u16) -> Vec<Line<'static>> {
        let run_id = json_str(&self.value, "run_id")
            .or_else(|| json_str(&self.value, "run"))
            .or_else(|| json_str(&self.value, "id"))
            .unwrap_or_else(|| "?".to_string());
        let short: String = run_id.chars().take(8).collect();
        let certified = self.value["certified"].as_bool().unwrap_or(false);
        let steps = json_u64(&self.value, "steps");

        let header = if certified {
            Line::from(vec![
                Span::styled(theme::VERIFIED.to_string(), theme::verified()),
                Span::styled(" Agent run certified".to_string(), theme::bold()),
                Span::styled(format!("  {short}"), theme::dim()),
            ])
        } else {
            Line::from(vec![
                Span::styled(theme::BULLET.to_string(), theme::dim()),
                Span::styled(
                    " Agent run finished (not certified)".to_string(),
                    theme::bold(),
                ),
                Span::styled(format!("  {short}"), theme::dim()),
            ])
        };
        vec![
            header,
            Line::from(Span::styled(format!("  {steps} steps"), theme::dim())),
        ]
    }
}

pub fn agent_run_cell(value: &Value) -> Cell {
    Box::new(AgentRunCell {
        value: value.clone(),
    })
}

// A pending graph-mutation proposal awaiting approve/reject.
struct ProposalCell {
    id: String,
    summary: String,
}
impl HistoryCell for ProposalCell {
    fn lines(&self, width: u16) -> Vec<Line<'static>> {
        let short: String = self.id.chars().take(8).collect();
        let mut out = vec![Line::from(vec![
            Span::styled(theme::PROPOSAL.to_string(), theme::proposal()),
            Span::styled(format!(" Proposal {short}"), theme::bold()),
        ])];
        out.extend(prefixed_wrapped(
            &self.summary,
            width,
            "  ",
            "  ",
            theme::dim(),
            None,
        ));
        out.push(Line::from(Span::styled(
            "  [a] approve   [r] reject".to_string(),
            theme::dim(),
        )));
        out
    }
}

pub fn proposal_cell(id: &str, summary: &str) -> Cell {
    Box::new(ProposalCell {
        id: id.to_string(),
        summary: summary.to_string(),
    })
}

// The /verify inspector, rendered richly: a bold count header, then one glyphed
// row per node. The honesty rule is enforced here exactly as in `VerdictCell` and
// the graph panel: ONLY a `formally_verified` status earns the green check. A
// rejected node is a red cross; an informally-verified or blocked node is a
// yellow warning (a pass that is NOT a kernel verification); everything else is a
// neutral dim bullet. No status but `formally_verified` can ever read as green.
struct VerifyRow {
    status: String,
    layer: String,
    title: String,
}

struct VerifyCell {
    verified: usize,
    total: usize,
    rows: Vec<VerifyRow>,
}

// The per-node glyph and its style. Free function so the honesty mapping is
// pinned by a test and cannot silently gain a green case.
fn verify_glyph(status: &str) -> (&'static str, Style) {
    match status {
        "formally_verified" => (theme::VERIFIED, theme::verified()),
        "rejected" => (theme::FAILED, theme::failed()),
        // Not a kernel verification, but not a crash either: yellow warning.
        "informally_verified" | "blocked" => (theme::WARN, theme::warn()),
        // proposed / active / superseded / anything unknown: neutral, never green.
        _ => (theme::BULLET, theme::dim()),
    }
}

impl HistoryCell for VerifyCell {
    fn lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut out = vec![Line::from(vec![
            Span::styled(format!("{} ", theme::BULLET), theme::dim()),
            Span::styled(
                format!("{}/{} nodes formally verified", self.verified, self.total),
                theme::bold(),
            ),
        ])];
        for row in &self.rows {
            let (glyph, style) = verify_glyph(&row.status);
            // Reserve room for the glyph, status, and layer columns; truncate the
            // title so a long name never overflows the pane.
            let avail = (width as usize).saturating_sub(34).max(8);
            let title: String = row.title.chars().take(avail).collect();
            out.push(Line::from(vec![
                Span::raw("  ".to_string()),
                Span::styled(glyph.to_string(), style),
                Span::styled(format!(" {:<20}", row.status), theme::dim()),
                Span::styled(format!("{:<9}", row.layer), theme::dim()),
                Span::raw(title),
            ]));
        }
        if self.rows.is_empty() {
            out.push(Line::from(Span::styled(
                "  no nodes yet".to_string(),
                theme::dim(),
            )));
        }
        out.push(Line::from(Vec::<Span>::new()));
        out.push(Line::from(Span::styled(
            "  Use /prove or /agent to drive verification.".to_string(),
            theme::dim(),
        )));
        out
    }
}

/// Build the rich /verify cell from `(status, layer, title)` rows plus the
/// verified/total counts. See `VerifyCell` for the honesty rule it enforces.
pub fn verify_cell(verified: usize, total: usize, rows: Vec<(String, String, String)>) -> Cell {
    Box::new(VerifyCell {
        verified,
        total,
        rows: rows
            .into_iter()
            .map(|(status, layer, title)| VerifyRow {
                status,
                layer,
                title,
            })
            .collect(),
    })
}

struct NoticeCell {
    text: String,
}
impl HistoryCell for NoticeCell {
    fn lines(&self, width: u16) -> Vec<Line<'static>> {
        prefixed_wrapped(
            &self.text,
            width,
            &format!("{} ", theme::BULLET),
            "  ",
            theme::dim(),
            None,
        )
    }
}

pub fn notice_cell(text: &str) -> Cell {
    Box::new(NoticeCell {
        text: text.to_string(),
    })
}

/// The startup card: a bordered box that names the product, states what it is,
/// shows the active model and project, and lists the first few keys. It is
/// pushed as the first transcript cell so it scrolls away as the conversation
/// grows (the Codex session-header pattern), rather than occupying fixed chrome.
struct WelcomeCell {
    model: String,
    project: String,
}
impl HistoryCell for WelcomeCell {
    fn lines(&self, width: u16) -> Vec<Line<'static>> {
        // Inner width: content area between the borders, capped so the card does
        // not sprawl on a wide terminal (an eyeballed, Codex-like ceiling).
        let inner = (width.saturating_sub(4) as usize).clamp(0, 60);
        if inner < 12 {
            // Too narrow to box cleanly; fall back to a plain two-line intro.
            return vec![
                Line::from(Span::styled("Theoremata", theme::bold())),
                Line::from(Span::styled(
                    "an AI mathematician; verify to a kernel",
                    theme::dim(),
                )),
            ];
        }

        // Each content row is (text, style); the border math pads to `inner`.
        let kv = |k: &str, v: &str| -> (String, Style) { (format!("{k:<8}{v}"), theme::dim()) };
        let rows: Vec<(String, Style)> = vec![
            ("Theoremata".to_string(), theme::user()),
            (
                "an AI mathematician: prove conjectures, verify to a".to_string(),
                theme::dim(),
            ),
            (
                "kernel, and keep receipts you can re-check.".to_string(),
                theme::dim(),
            ),
            (String::new(), theme::dim()),
            kv("model", &self.model),
            kv("project", &self.project),
            (String::new(), theme::dim()),
            (
                "type to chat  \u{b7}  / for commands  \u{b7}  Ctrl-C to quit".to_string(),
                theme::dim(),
            ),
        ];

        let mut out = Vec::with_capacity(rows.len() + 2);
        let top = format!("\u{250c}{}\u{2510}", theme::RULE.repeat(inner + 2));
        let bot = format!("\u{2514}{}\u{2518}", theme::RULE.repeat(inner + 2));
        out.push(Line::from(Span::styled(top, theme::dim())));
        for (text, style) in rows {
            // Truncate an over-long row to the inner width, then pad to it so the
            // right border lines up regardless of content length.
            let mut t: String = text.chars().take(inner).collect();
            let pad = inner.saturating_sub(t.chars().count());
            t.push_str(&" ".repeat(pad));
            out.push(Line::from(vec![
                Span::styled("\u{2502} ", theme::dim()),
                Span::styled(t, style),
                Span::styled(" \u{2502}", theme::dim()),
            ]));
        }
        out.push(Line::from(Span::styled(bot, theme::dim())));
        out
    }
}

/// Build the startup welcome card for the given active model and project.
pub fn welcome_cell(model: &str, project: &str) -> Cell {
    Box::new(WelcomeCell {
        model: model.to_string(),
        project: project.to_string(),
    })
}

struct ErrorCell {
    text: String,
}
impl HistoryCell for ErrorCell {
    fn lines(&self, width: u16) -> Vec<Line<'static>> {
        prefixed_wrapped(
            &self.text,
            width,
            &format!("{} ", theme::FAILED),
            "  ",
            theme::error(),
            Some(theme::error()),
        )
    }
}

pub fn error_cell(text: &str) -> Cell {
    Box::new(ErrorCell {
        text: text.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Pull every span across all lines of a cell into a flat list for
    // assertions on glyphs and colors.
    fn all_spans(cell: &Cell, width: u16) -> Vec<Span<'static>> {
        cell.lines(width)
            .into_iter()
            .flat_map(|l| l.spans)
            .collect()
    }

    fn contains_glyph_with_fg(spans: &[Span<'static>], glyph: &str, fg: Color) -> bool {
        spans
            .iter()
            .any(|s| s.content.contains(glyph) && s.style.fg == Some(fg))
    }

    #[test]
    fn fully_green_verdict_shows_green_check() {
        let cell = verdict_cell(
            "lean",
            "theorem t : True := trivial",
            true,
            true,
            true,
            true,
        );
        let header = &cell.lines(80)[0];
        // The header carries the verified glyph in green, bold.
        assert!(header.spans[0].content.contains(theme::VERIFIED));
        assert_eq!(header.spans[0].style.fg, Some(Color::Green));
        assert!(header.spans[0].style.add_modifier.contains(Modifier::BOLD));
        // And it must not be a warning or a failure.
        assert!(!header.spans[0].content.contains(theme::WARN));
        assert!(!header.spans[0].content.contains(theme::FAILED));
    }

    #[test]
    fn mock_verdict_is_yellow_not_green() {
        // Everything passes EXCEPT live: a mock run. Must be yellow warn, and
        // the header must never be a green check.
        let cell = verdict_cell(
            "lean",
            "theorem t : True := trivial",
            true,
            true,
            true,
            false,
        );
        let header = &cell.lines(80)[0];
        assert!(header.spans[0].content.contains(theme::WARN));
        assert_eq!(header.spans[0].style.fg, Some(Color::Yellow));
        // The honesty rule, pinned: the header glyph is not the green check.
        assert!(!header.spans[0].content.contains(theme::VERIFIED));
        assert_ne!(header.spans[0].style.fg, Some(Color::Green));
        // The label names it as not verified / mock.
        let label: String = header.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(label.contains("Not verified"));
        assert!(label.contains("mock"));
    }

    #[test]
    fn unpreserved_verdict_is_yellow_not_green() {
        let cell = verdict_cell("rocq", "Proof. exact I. Qed.", true, true, false, true);
        let header = &cell.lines(80)[0];
        assert!(header.spans[0].content.contains(theme::WARN));
        assert!(!header.spans[0].content.contains(theme::VERIFIED));
        assert_ne!(header.spans[0].style.fg, Some(Color::Green));
        let label: String = header.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(label.contains("unpreserved"));
    }

    #[test]
    fn failed_compile_verdict_is_red() {
        let cell = verdict_cell("lean", "sorry", false, false, false, true);
        let header = &cell.lines(80)[0];
        assert!(header.spans[0].content.contains(theme::FAILED));
        assert_eq!(header.spans[0].style.fg, Some(Color::Red));
        assert!(!header.spans[0].content.contains(theme::VERIFIED));
    }

    #[test]
    fn verdict_predicate_only_all_true() {
        assert!(verdict_verified(true, true, true, true));
        assert!(!verdict_verified(true, true, true, false));
        assert!(!verdict_verified(true, true, false, true));
        assert!(!verdict_verified(false, true, true, true));
        assert!(!verdict_verified(true, false, true, true));
    }

    #[test]
    fn no_counterexample_falsify_is_neutral_not_green() {
        let cell = falsify_cell(&json!({
            "verdict": "no_counterexample_in_domain",
            "checked": 100000
        }));
        let spans = all_spans(&cell, 80);
        // Neutral survived line: a dim bullet, and no green anywhere, and no
        // verified check glyph. Absence of a counterexample is not a proof.
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(joined.contains("no counterexample in domain"));
        assert!(joined.contains("checked 100000"));
        assert!(!joined.contains(theme::VERIFIED));
        assert!(spans.iter().all(|s| s.style.fg != Some(Color::Green)));
    }

    #[test]
    fn refuted_falsify_is_red_counterexample() {
        let cell = falsify_cell(&json!({
            "verdict": "counterexample",
            "assignment": { "x": 1, "y": 1 },
            "checked": 3
        }));
        let spans = all_spans(&cell, 80);
        assert!(contains_glyph_with_fg(&spans, theme::FAILED, Color::Red));
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(joined.contains("counterexample"));
        assert!(joined.contains("x=1"));
        assert!(joined.contains("y=1"));
        // A refutation must never be green.
        assert!(spans.iter().all(|s| s.style.fg != Some(Color::Green)));
    }

    #[test]
    fn sweep_highlights_mathematics_moved_in_red() {
        let cell = sweep_cell(&json!({
            "summary": "swept 16 nodes",
            "fresh": 12, "repair_candidate": 3,
            "mathematics_moved": 1, "unknown": 0, "total": 16
        }));
        let spans = all_spans(&cell, 80);
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(joined.contains("MathematicsMoved"));
        // The regressed row carries red.
        assert!(spans
            .iter()
            .any(|s| s.content.contains("MathematicsMoved") && s.style.fg == Some(Color::Red)));
        assert!(joined.contains("Fresh"));
        assert!(joined.contains("total"));
    }

    #[test]
    fn agent_run_certified_green_uncertified_neutral() {
        let good = agent_run_cell(&json!({"run_id":"abcdef1234","certified":true,"steps":7}));
        let gh = &good.lines(80)[0];
        assert!(gh.spans[0].content.contains(theme::VERIFIED));
        assert_eq!(gh.spans[0].style.fg, Some(Color::Green));

        let bad = agent_run_cell(&json!({"run_id":"abcdef1234","certified":false,"steps":2}));
        let bh = &bad.lines(80)[0];
        assert!(!bh.spans[0].content.contains(theme::VERIFIED));
        assert_ne!(bh.spans[0].style.fg, Some(Color::Green));
    }

    #[test]
    fn output_lines_truncates_with_marker() {
        let text = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = output_lines(&text, 80, 5);
        // 5 head rows plus the collapse marker.
        assert_eq!(lines.len(), 6);
        let marker: String = lines
            .last()
            .unwrap()
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(marker, "... +15 lines");
    }

    #[test]
    fn output_lines_no_marker_when_short() {
        let text = "a\nb\nc";
        let lines = output_lines(text, 80, 5);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn wrap_hard_splits_overlong_word() {
        let w = wrap_text("abcdefghij", 4);
        assert_eq!(w, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn status_line_ok_is_green_check() {
        let l = theme::status_line("compiled", true);
        assert!(l.spans[0].content.contains(theme::VERIFIED));
        assert_eq!(l.spans[0].style.fg, Some(Color::Green));
        let l2 = theme::status_line("compiled", false);
        assert!(l2.spans[0].content.contains(theme::FAILED));
        assert_eq!(l2.spans[0].style.fg, Some(Color::Red));
    }

    #[test]
    fn verify_glyph_only_formally_verified_is_green() {
        // The one green case.
        assert_eq!(verify_glyph("formally_verified").1.fg, Some(Color::Green));
        // Every other status must NOT be green (the honesty rule).
        for s in [
            "proposed",
            "active",
            "blocked",
            "rejected",
            "informally_verified",
            "superseded",
            "",
        ] {
            assert_ne!(
                verify_glyph(s).1.fg,
                Some(Color::Green),
                "status {s:?} must not be green"
            );
        }
        // A rejection reads red; an informal pass reads yellow, never green.
        assert_eq!(verify_glyph("rejected").1.fg, Some(Color::Red));
        assert_eq!(
            verify_glyph("informally_verified").1.fg,
            Some(Color::Yellow)
        );
    }

    #[test]
    fn verify_cell_greens_only_the_verified_node() {
        let cell = verify_cell(
            1,
            2,
            vec![
                (
                    "formally_verified".to_string(),
                    "formal".to_string(),
                    "Proved thm".to_string(),
                ),
                (
                    "active".to_string(),
                    "informal".to_string(),
                    "Open goal".to_string(),
                ),
            ],
        );
        let lines = cell.lines(80);
        // The header reports the count.
        let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(header.contains("1/2 nodes formally verified"));
        // The verified glyph appears in green somewhere.
        let spans = all_spans(&cell, 80);
        assert!(contains_glyph_with_fg(
            &spans,
            theme::VERIFIED,
            Color::Green
        ));
        // The row carrying the "active" status is never green.
        let active_line = cell
            .lines(80)
            .into_iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("active")))
            .expect("active row present");
        assert!(active_line
            .spans
            .iter()
            .all(|s| s.style.fg != Some(Color::Green)));
    }

    #[test]
    fn agent_markdown_styles_bold_and_code() {
        let cell = agent_cell("use **exact** and `trivial`");
        let spans = all_spans(&cell, 80);
        assert!(spans.iter().any(
            |s| s.content.as_ref() == "exact" && s.style.add_modifier.contains(Modifier::BOLD)
        ));
        assert!(spans.iter().any(
            |s| s.content.as_ref() == "trivial" && s.style.add_modifier.contains(Modifier::DIM)
        ));
    }
}
