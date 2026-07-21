//! Structured checker-error feedback rendering (Tier 1 items 5-6).
//!
//! Today a failed verification hands the model the raw checker blob
//! (`CompileReport::errors`, which for most backends is just
//! `[stderr, stdout]`). Two mined systems do materially better by rendering the
//! failure *positionally* against the submitted source: a few lines of
//! preceding context, the offending region wrapped in explicit
//! `<error>` … `</error>` delimiters, one trailing line, then the message. The
//! system this port follows measured ~2.2 points of benchmark gain from the
//! feedback change alone.
//!
//! This module is a **pure function**: no IO, no clock, no RNG, no process
//! spawning. It takes the raw checker stdout/stderr, the source text that was
//! submitted, and a [`FeedbackConfig`], and returns a
//! [`RenderedFeedback`] carrying both the compact rendered string and the
//! structured [`Diagnostic`]s it parsed.
//!
//! Diagnostic parsing dispatches on [`FormalSystem`] exactly like the sibling
//! prover modules (`statement_preservation`, `axiom_audit`), because the
//! formats genuinely differ:
//!
//! | system    | shape                                                     |
//! |-----------|-----------------------------------------------------------|
//! | Lean      | `Generated.lean:12:5: error: unknown identifier 'foo'`     |
//! | Rocq      | `File "F.v", line 12, characters 4-9:` + `Error: …`        |
//! | Isabelle  | `*** Undefined fact: "foo"` + `*** At command … (line 12 …)`|
//! | Agda      | `Generated.agda:12,5-9: Not in scope: foo`                 |
//! | Metamath  | `?Error on line 12 of file "x.mm": …`                      |
//! | Candle    | (no stable machine format; generic fallback)               |
//!
//! **Fail-soft, never fail-closed.** This module is advisory presentation only
//! — it never participates in a verification verdict. Output it cannot parse
//! degrades to a truncated raw passthrough rather than panicking or, worse,
//! silently dropping the checker's own words.

use crate::prover::formal::FormalSystem;
use serde::{Deserialize, Serialize};

/// The literal marker inserted where the middle of an over-long region is
/// elided. Kept as a `const` because the model is expected to learn it, so it
/// must be byte-identical everywhere it appears.
pub const TRUNCATION_MARKER: &str = "... --[Truncated]-- ...";

/// Opening delimiter wrapping the offending region.
pub const ERROR_OPEN: &str = "<error>";
/// Closing delimiter wrapping the offending region.
pub const ERROR_CLOSE: &str = "</error>";

/// Hard ceiling on lines kept when degrading to raw passthrough, so an
/// unparseable multi-megabyte blob can never blow up a prompt.
const RAW_PASSTHROUGH_LINES: usize = 40;

/// Rendering knobs. [`Default`] matches the ported system's settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackConfig {
    /// Maximum diagnostics rendered in full; the rest are counted and omitted.
    pub max_errors: usize,
    /// Source lines of context shown *before* the offending region.
    pub context_lines_before: usize,
    /// Source lines of context shown *after* the offending region.
    pub context_lines_after: usize,
    /// A region spanning more than this many lines has its middle elided with
    /// [`TRUNCATION_MARKER`].
    pub elide_threshold: usize,
}

impl Default for FeedbackConfig {
    fn default() -> Self {
        Self {
            max_errors: 8,
            context_lines_before: 4,
            context_lines_after: 1,
            elide_threshold: 6,
        }
    }
}

/// Diagnostic severity as reported by the checker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

/// One parsed checker diagnostic. All positions are **1-based** and mirror the
/// checker's own numbering; `None` means the checker did not report that
/// coordinate (which is normal for Isabelle and for Lean's file-level errors).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// The system whose format produced this diagnostic.
    pub system: FormalSystem,
    pub severity: Severity,
    /// 1-based start line in the submitted source.
    pub line: Option<usize>,
    /// 1-based end line; `None` means the region is the single `line`.
    pub end_line: Option<usize>,
    /// 1-based inclusive start column.
    pub col_start: Option<usize>,
    /// 1-based exclusive end column.
    pub col_end: Option<usize>,
    /// The checker's message, with continuation lines joined.
    pub message: String,
    /// **Typed seam, deliberately never populated here.** The proof state at
    /// this error position is by far the highest-value enrichment we could add
    /// to failure feedback — a model told "unsolved goals" plus the actual
    /// hypotheses and goal repairs far more often than one told only the
    /// message. Obtaining it requires walking Lean's infotree from a *live*
    /// REPL (see `verify::lean_session` / `prover::session`), which is IO and
    /// therefore outside this pure module. A caller that already holds a warm
    /// session should fill this in before rendering; [`render_feedback`] will
    /// emit it under a `goal state:` heading when present.
    #[serde(default)]
    pub goal_state_slot: Option<String>,
}

impl Diagnostic {
    fn bare(system: FormalSystem, severity: Severity, message: impl Into<String>) -> Self {
        Self {
            system,
            severity,
            line: None,
            end_line: None,
            col_start: None,
            col_end: None,
            message: message.into(),
            goal_state_slot: None,
        }
    }
}

/// The result of rendering: the prompt-ready string plus the structure behind
/// it, so callers can also route diagnostics somewhere machine-readable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderedFeedback {
    /// The compact feedback string to hand back to the model.
    pub text: String,
    /// Every diagnostic parsed, including those beyond `max_errors`.
    pub diagnostics: Vec<Diagnostic>,
    /// How many diagnostics were parsed but not rendered (the true count).
    pub omitted: usize,
    /// `false` when nothing could be parsed and `text` is a truncated raw
    /// passthrough of the checker output.
    pub parsed: bool,
}

// --- entry points ---------------------------------------------------------

/// Render structured, positional feedback for a failed check.
///
/// `raw` is the checker's combined stdout/stderr (callers typically pass
/// `format!("{stderr}\n{stdout}")`, or `CompileReport::errors.join("\n")`).
/// `source` is the source text that was submitted. Pure and deterministic:
/// the same three inputs always yield a byte-identical `text`.
pub fn render_feedback(
    system: FormalSystem,
    raw: &str,
    source: &str,
    cfg: &FeedbackConfig,
) -> RenderedFeedback {
    let diagnostics = parse_diagnostics(system, raw);
    if diagnostics.is_empty() {
        return RenderedFeedback {
            text: raw_passthrough(raw),
            diagnostics,
            omitted: 0,
            parsed: false,
        };
    }

    let source_lines: Vec<&str> = source.lines().collect();
    let shown = diagnostics.len().min(cfg.max_errors.max(1));
    let omitted = diagnostics.len() - shown;

    let mut out = String::new();
    out.push_str(&format!(
        "{} checker reported {} diagnostic(s):\n",
        system.as_str(),
        diagnostics.len()
    ));
    for (i, d) in diagnostics.iter().take(shown).enumerate() {
        out.push('\n');
        out.push_str(&render_one(&source_lines, d, cfg, i + 1));
    }
    if omitted > 0 {
        out.push_str(&format!("\n... [Omitted {omitted} more errors] ...\n"));
    }
    RenderedFeedback {
        text: out,
        diagnostics,
        omitted,
        parsed: true,
    }
}

/// Parse `raw` into structured diagnostics using `system`'s message format.
/// Returns an empty vec when nothing matched (the caller then degrades to
/// passthrough). Never panics on arbitrary bytes/UTF-8.
pub fn parse_diagnostics(system: FormalSystem, raw: &str) -> Vec<Diagnostic> {
    match system {
        FormalSystem::Lean => parse_lean(raw),
        FormalSystem::Rocq => parse_rocq(raw),
        FormalSystem::Isabelle => parse_isabelle(raw),
        FormalSystem::Agda => parse_agda(raw),
        FormalSystem::Metamath => parse_metamath(raw),
        // Candle runs HOL Light as an OCaml script; failures surface as OCaml
        // exceptions with no stable line:col format. Take whatever positional
        // information the generic scan can find rather than inventing one.
        FormalSystem::Candle => parse_generic(FormalSystem::Candle, raw),
    }
}

// --- per-system parsers ---------------------------------------------------

fn severity_from(word: &str) -> Option<Severity> {
    match word.trim().to_ascii_lowercase().as_str() {
        "error" => Some(Severity::Error),
        "warning" | "warn" => Some(Severity::Warning),
        "info" | "information" | "note" => Some(Severity::Info),
        _ => None,
    }
}

/// Split a `"12"` or `"5-9"` column/line field into (start, end).
fn parse_span(field: &str) -> (Option<usize>, Option<usize>) {
    let field = field.trim();
    match field.split_once('-') {
        Some((a, b)) => (a.trim().parse().ok(), b.trim().parse().ok()),
        None => (field.parse().ok(), None),
    }
}

/// Lean 4: `Generated.lean:12:5: error: unknown identifier 'foo'`, with
/// continuation lines indented or simply un-prefixed until the next header.
fn parse_lean(raw: &str) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = Vec::new();
    for line in raw.lines() {
        let mut matched = None;
        for marker in [": error:", ": warning:", ": info:"] {
            if let Some(idx) = line.find(marker) {
                matched = Some((idx, marker));
                break;
            }
        }
        if let Some((idx, marker)) = matched {
            let head = &line[..idx];
            let message = line[idx + marker.len()..].trim().to_string();
            let severity =
                severity_from(marker.trim_matches(':').trim()).unwrap_or(Severity::Error);
            // head is `path:line:col`; take the last two `:`-separated fields.
            let (line_no, col_start, col_end) = match head.rsplit_once(':') {
                Some((rest, col_field)) => {
                    let (c0, c1) = parse_span(col_field);
                    // Lean 4 reports 1-based LINES but 0-based COLUMNS, while
                    // `Diagnostic` stores 1-based columns. Convert, or every
                    // `<error>` span opens one character early (e.g. on the space
                    // before the offending token).
                    let l = rest
                        .rsplit_once(':')
                        .and_then(|(_, l)| l.trim().parse().ok());
                    (l, c0.map(|c| c + 1), c1.map(|c| c + 1))
                }
                None => (None, None, None),
            };
            out.push(Diagnostic {
                system: FormalSystem::Lean,
                severity,
                line: line_no,
                end_line: None,
                col_start,
                col_end,
                message,
                goal_state_slot: None,
            });
            continue;
        }
        // A header-less `error: …` (Lean emits these for whole-file problems).
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("error:") {
            out.push(Diagnostic::bare(
                FormalSystem::Lean,
                Severity::Error,
                rest.trim(),
            ));
            continue;
        }
        // Otherwise treat a non-empty line as a continuation of the previous
        // diagnostic's message (Lean wraps `unsolved goals` bodies this way).
        if !line.trim().is_empty() {
            if let Some(last) = out.last_mut() {
                last.message.push('\n');
                last.message.push_str(line.trim_end());
            }
        }
    }
    out
}

/// Rocq: a `File "F.v", line 12, characters 4-9:` locator followed by an
/// `Error:` / `Warning:` body on subsequent lines.
fn parse_rocq(raw: &str) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = Vec::new();
    let mut pending: Option<(Option<usize>, Option<usize>, Option<usize>)> = None;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("File \"") && trimmed.contains(", line") {
            let line_no = after_marker(trimmed, ", line ").and_then(leading_usize);
            let (c0, c1) = match after_marker(trimmed, ", characters ") {
                Some(s) => parse_span(s.trim_end_matches(':').trim()),
                None => (None, None),
            };
            pending = Some((line_no, c0, c1));
            continue;
        }
        let body = ["Error:", "Warning:", "Syntax error:"]
            .iter()
            .find_map(|m| trimmed.strip_prefix(m).map(|rest| (*m, rest)));
        if let Some((marker, rest)) = body {
            let severity = if marker == "Warning:" {
                Severity::Warning
            } else {
                Severity::Error
            };
            let (line_no, c0, c1) = pending.take().unwrap_or((None, None, None));
            out.push(Diagnostic {
                system: FormalSystem::Rocq,
                severity,
                line: line_no,
                end_line: None,
                col_start: c0,
                col_end: c1,
                message: rest.trim().to_string(),
                goal_state_slot: None,
            });
            continue;
        }
        if !trimmed.is_empty() {
            if let Some(last) = out.last_mut() {
                last.message.push('\n');
                last.message.push_str(line.trim_end());
            }
        }
    }
    out
}

/// Isabelle: `***`-prefixed error blocks; the position, when present, appears as
/// `(line 12 of "…")` inside one of the block's lines.
fn parse_isabelle(raw: &str) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("***") else {
            continue;
        };
        let rest = rest.trim();
        let line_no = after_marker(rest, "(line ").and_then(leading_usize);
        if let (Some(n), Some(last)) = (line_no, out.last_mut()) {
            // A trailing `At command … (line N of "…")` locates the block that
            // precedes it rather than opening a new diagnostic.
            if last.line.is_none() && rest.starts_with("At command") {
                last.line = Some(n);
                continue;
            }
        }
        if rest.is_empty() {
            continue;
        }
        out.push(Diagnostic {
            system: FormalSystem::Isabelle,
            severity: Severity::Error,
            line: line_no,
            end_line: None,
            col_start: None,
            col_end: None,
            message: rest.to_string(),
            goal_state_slot: None,
        });
    }
    out
}

/// Agda: `Generated.agda:12,5-9: message` or a cross-line
/// `Generated.agda:12,5-14,9: message` range.
fn parse_agda(raw: &str) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = Vec::new();
    for line in raw.lines() {
        let parsed = agda_locator(line);
        if let Some((l0, l1, c0, c1, msg)) = parsed {
            out.push(Diagnostic {
                system: FormalSystem::Agda,
                severity: Severity::Error,
                line: Some(l0),
                end_line: l1,
                col_start: c0,
                col_end: c1,
                message: msg,
                goal_state_slot: None,
            });
            continue;
        }
        if !line.trim().is_empty() {
            if let Some(last) = out.last_mut() {
                last.message.push('\n');
                last.message.push_str(line.trim_end());
            }
        }
    }
    out
}

#[allow(clippy::type_complexity)]
fn agda_locator(
    line: &str,
) -> Option<(usize, Option<usize>, Option<usize>, Option<usize>, String)> {
    // Locate `:<digits>,` — the start of Agda's `line,col` range.
    let idx = line.find(".agda:").map(|i| i + ".agda:".len())?;
    let rest = &line[idx..];
    let end = rest.find(": ")?;
    let range = &rest[..end];
    let message = rest[end + 2..].trim().to_string();
    let (start, stop) = match range.split_once('-') {
        Some((a, b)) => (a, Some(b)),
        None => (range, None),
    };
    let (l0, c0) = start.split_once(',')?;
    let l0: usize = l0.trim().parse().ok()?;
    let c0: Option<usize> = c0.trim().parse().ok();
    let (l1, c1) = match stop {
        // `12,5-14,9` — the tail carries its own line.
        Some(s) => match s.split_once(',') {
            Some((a, b)) => (a.trim().parse().ok(), b.trim().parse().ok()),
            // `12,5-9` — same line, tail is just the end column.
            None => (None, s.trim().parse().ok()),
        },
        None => (None, None),
    };
    Some((l0, l1, c0, c1, message))
}

/// Metamath: `?Error on line 12 of file "x.mm": …` / bare `?Error: …` /
/// `?Warning: …`. The reference binary exits 0 on failure, so these markers are
/// the *only* failure signal (see `ExternalBackend::compile_success_signal`).
fn parse_metamath(raw: &str) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        let severity = if trimmed.starts_with("?Error") {
            Severity::Error
        } else if trimmed.starts_with("?Warning") {
            Severity::Warning
        } else {
            if !trimmed.is_empty() {
                if let Some(last) = out.last_mut() {
                    last.message.push('\n');
                    last.message.push_str(line.trim_end());
                }
            }
            continue;
        };
        let line_no = after_marker(trimmed, " on line ").and_then(leading_usize);
        // Message is whatever follows the first `:` after the marker, else the
        // whole marker line (Metamath is inconsistent about the colon).
        let body = trimmed
            .split_once(": ")
            .map(|(_, rest)| rest.trim().to_string())
            .unwrap_or_else(|| trimmed.to_string());
        out.push(Diagnostic {
            system: FormalSystem::Metamath,
            severity,
            line: line_no,
            end_line: None,
            col_start: None,
            col_end: None,
            message: body,
            goal_state_slot: None,
        });
    }
    out
}

/// Format-agnostic fallback: any line containing `error`/`Error` becomes a
/// diagnostic, with a `line N` position if one is stated.
fn parse_generic(system: FormalSystem, raw: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let low = trimmed.to_ascii_lowercase();
        let severity = if low.contains("error") || low.contains("exception") {
            Severity::Error
        } else if low.contains("warning") {
            Severity::Warning
        } else {
            continue;
        };
        let line_no = after_marker(trimmed, "line ").and_then(leading_usize);
        out.push(Diagnostic {
            system,
            severity,
            line: line_no,
            end_line: None,
            col_start: None,
            col_end: None,
            message: trimmed.to_string(),
            goal_state_slot: None,
        });
    }
    out
}

/// Text following the first occurrence of `marker`, or `None`.
fn after_marker<'a>(haystack: &'a str, marker: &str) -> Option<&'a str> {
    haystack.find(marker).map(|i| &haystack[i + marker.len()..])
}

/// The leading run of ASCII digits parsed as a `usize`.
fn leading_usize(s: &str) -> Option<usize> {
    let digits: String = s
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

// --- rendering ------------------------------------------------------------

/// Elide the middle of `lines` when it exceeds `threshold`, keeping a balanced
/// head and tail around a single [`TRUNCATION_MARKER`]. `threshold == 0` is
/// treated as "never elide" so a misconfiguration cannot erase the region.
fn elide(lines: Vec<String>, threshold: usize) -> Vec<String> {
    if threshold == 0 || lines.len() <= threshold {
        return lines;
    }
    let head = (threshold + 1) / 2;
    let tail = threshold - head;
    let mut out: Vec<String> = lines[..head].to_vec();
    out.push(TRUNCATION_MARKER.to_string());
    if tail > 0 {
        out.extend_from_slice(&lines[lines.len() - tail..]);
    }
    out
}

/// Insert the `<error>`/`</error>` delimiters inside a single line at the
/// 1-based, half-open column span `[c0, c1)`. Operates on `char`s, so it can
/// never split a multi-byte character. Out-of-range columns are clamped.
fn mark_inline(line: &str, c0: usize, c1: Option<usize>) -> String {
    let chars: Vec<char> = line.chars().collect();
    let start = c0.saturating_sub(1).min(chars.len());
    let end = c1
        .map(|c| c.saturating_sub(1))
        .unwrap_or(chars.len())
        .clamp(start, chars.len());
    let pre: String = chars[..start].iter().collect();
    let mid: String = chars[start..end].iter().collect();
    let post: String = chars[end..].iter().collect();
    format!("{pre}{ERROR_OPEN}{mid}{ERROR_CLOSE}{post}")
}

fn gutter(width: usize, n: usize, text: &str) -> String {
    format!("{:>width$} | {}\n", n, text, width = width)
}

fn render_one(source_lines: &[&str], d: &Diagnostic, cfg: &FeedbackConfig, index: usize) -> String {
    let mut out = String::new();
    let where_ = match (d.line, d.col_start) {
        (Some(l), Some(c)) => match d.col_end {
            Some(e) => format!("line {l}, columns {c}-{e}"),
            None => format!("line {l}, column {c}"),
        },
        (Some(l), None) => format!("line {l}"),
        (None, _) => "no source position reported".to_string(),
    };
    out.push_str(&format!(
        "-- {} {} at {} --\n",
        d.severity.as_str(),
        index,
        where_
    ));

    // Only render source context when we have both a position and a source.
    let positioned = d
        .line
        .filter(|_| !source_lines.is_empty())
        .map(|l| l.saturating_sub(1).min(source_lines.len() - 1));

    if let Some(li) = positioned {
        let span_end = d
            .end_line
            .map(|e| e.saturating_sub(1))
            .unwrap_or(li)
            .clamp(li, source_lines.len() - 1);
        let ctx_start = li.saturating_sub(cfg.context_lines_before);
        let ctx_end = (span_end + cfg.context_lines_after).min(source_lines.len() - 1);
        let width = (ctx_end + 1).to_string().len();

        for (n, text) in source_lines.iter().enumerate().take(li).skip(ctx_start) {
            out.push_str(&gutter(width, n + 1, text));
        }

        // Single-line span with columns: delimit the offending region *inside*
        // the line so the model sees exactly the offending characters.
        if li == span_end && d.col_start.is_some() {
            let marked = mark_inline(source_lines[li], d.col_start.unwrap_or(1), d.col_end);
            out.push_str(&gutter(width, li + 1, &marked));
        } else {
            out.push_str(ERROR_OPEN);
            out.push('\n');
            let span: Vec<String> = (li..=span_end)
                .map(|n| {
                    let g = gutter(width, n + 1, source_lines[n]);
                    g.trim_end_matches('\n').to_string()
                })
                .collect();
            for l in elide(span, cfg.elide_threshold) {
                out.push_str(&l);
                out.push('\n');
            }
            out.push_str(ERROR_CLOSE);
            out.push('\n');
        }

        for (n, text) in source_lines
            .iter()
            .enumerate()
            .take(ctx_end + 1)
            .skip(span_end + 1)
        {
            out.push_str(&gutter(width, n + 1, text));
        }
    }

    out.push_str(&format!("message: {}\n", d.message.trim_end()));
    if let Some(goal) = &d.goal_state_slot {
        out.push_str("goal state:\n");
        out.push_str(goal.trim_end());
        out.push('\n');
    }
    out
}

/// Graceful degradation: keep the checker's own words, bounded. Never panics
/// and never returns empty when `raw` had content.
fn raw_passthrough(raw: &str) -> String {
    let lines: Vec<String> = raw
        .lines()
        .map(|l| l.trim_end().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return "[checker produced no output]\n".to_string();
    }
    let mut out = String::from("[unrecognized checker output - raw passthrough, truncated]\n");
    for l in elide(lines, RAW_PASSTHROUGH_LINES) {
        out.push_str(&l);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "import Mathlib\n\ntheorem foo (n : Nat) : n + 0 = n := by\n  induction n with\n  | zero => rfl\n  | succ k ih => exact bogus\n";

    #[test]
    fn single_lean_error_renders_context_and_delimiters() {
        // Line 6 is `  | succ k ih => exact bogus`. `bogus` starts at 1-based
        // column 24, which Lean reports as 0-based column 23.
        let raw = "Generated.lean:6:23: error: unknown identifier 'bogus'";
        let r = render_feedback(FormalSystem::Lean, raw, SRC, &FeedbackConfig::default());
        assert!(r.parsed, "a standard Lean header must parse");
        assert_eq!(r.diagnostics.len(), 1);
        let d = &r.diagnostics[0];
        assert_eq!(d.line, Some(6));
        assert_eq!(d.col_start, Some(24));
        assert_eq!(d.severity, Severity::Error);
        assert!(d.goal_state_slot.is_none(), "the seam stays unpopulated");

        // The offending region is explicitly delimited...
        assert!(r.text.contains(ERROR_OPEN) && r.text.contains(ERROR_CLOSE));
        // ...around the actual offending characters on line 6.
        assert!(
            r.text.contains("<error>bogus</error>"),
            "column span must be delimited inline: {}",
            r.text
        );
        // Four preceding lines of context (default), and the message.
        assert!(r.text.contains("theorem foo"));
        assert!(r.text.contains("| zero => rfl"));
        assert!(r.text.contains("unknown identifier 'bogus'"));
        // Line 1 is outside the 4-line window from line 6.
        assert!(!r.text.contains("import Mathlib"), "{}", r.text);
    }

    #[test]
    fn a_long_span_is_elided_with_the_truncation_marker() {
        let src: String = (1..=40)
            .map(|i| format!("line {i} of the proof\n"))
            .collect();
        // A 20-line region, well past the 6-line elide threshold.
        let raw = "Generated.agda:10,1-30,5: nested module error";
        let r = render_feedback(FormalSystem::Agda, raw, &src, &FeedbackConfig::default());
        assert!(r.parsed);
        let d = &r.diagnostics[0];
        assert_eq!((d.line, d.end_line), (Some(10), Some(30)));
        assert!(
            r.text.contains(TRUNCATION_MARKER),
            "a long span must elide: {}",
            r.text
        );
        // Head and tail of the region survive; the middle does not.
        assert!(r.text.contains("line 10 of the proof"));
        assert!(r.text.contains("line 30 of the proof"));
        assert!(!r.text.contains("line 20 of the proof"), "{}", r.text);
    }

    #[test]
    fn more_than_max_errors_appends_the_true_omitted_count() {
        let raw: String = (1..=11)
            .map(|i| format!("Generated.lean:{i}:1: error: problem {i}\n"))
            .collect();
        let src: String = (1..=11).map(|i| format!("stmt{i}\n")).collect();
        let cfg = FeedbackConfig::default(); // max_errors = 8
        let r = render_feedback(FormalSystem::Lean, &raw, &src, &cfg);
        assert_eq!(r.diagnostics.len(), 11, "all diagnostics stay structured");
        assert_eq!(r.omitted, 3);
        assert!(
            r.text.contains("... [Omitted 3 more errors] ..."),
            "{}",
            r.text
        );
        // Exactly the first 8 are rendered.
        assert!(r.text.contains("problem 8"));
        assert!(!r.text.contains("problem 9"), "{}", r.text);
    }

    #[test]
    fn metamath_error_line_parses() {
        let raw = "?Error on line 12 of file \"Generated.mm\": Proof of \"foo\" does not verify.\nAll proofs in the database were not verified.";
        let src: String = (1..=20).map(|i| format!("step{i} $.\n")).collect();
        let r = render_feedback(
            FormalSystem::Metamath,
            raw,
            &src,
            &FeedbackConfig::default(),
        );
        assert!(r.parsed, "a `?Error` line must parse: {}", r.text);
        assert_eq!(r.diagnostics.len(), 1);
        let d = &r.diagnostics[0];
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.line, Some(12));
        assert!(d.message.contains("does not verify"));
        assert!(r.text.contains("step12 $."));
        // `?Warning` is classified as a warning, not an error.
        let w = parse_diagnostics(FormalSystem::Metamath, "?Warning: proof is incomplete.");
        assert_eq!(w[0].severity, Severity::Warning);
    }

    #[test]
    fn unparseable_garbage_degrades_to_truncated_passthrough() {
        let garbage = "\u{1f4a5} segfault at 0xdeadbeef\n\u{ff}\u{fe}binary noise\n";
        let r = render_feedback(FormalSystem::Lean, garbage, SRC, &FeedbackConfig::default());
        assert!(!r.parsed, "nothing should have parsed");
        assert!(r.diagnostics.is_empty());
        assert!(r.text.contains("raw passthrough, truncated"));
        // The checker's own words are never lost.
        assert!(r.text.contains("segfault at 0xdeadbeef"));

        // A flood is bounded and marked.
        let flood: String = (1..500).map(|i| format!("noise {i}\n")).collect();
        let r2 = render_feedback(FormalSystem::Lean, &flood, SRC, &FeedbackConfig::default());
        assert!(!r2.parsed);
        assert!(r2.text.contains(TRUNCATION_MARKER));
        assert!(r2.text.lines().count() < 60, "passthrough must be bounded");

        // Empty output never panics and never yields an empty string.
        let r3 = render_feedback(FormalSystem::Lean, "", "", &FeedbackConfig::default());
        assert!(!r3.text.is_empty());
    }

    #[test]
    fn out_of_range_positions_clamp_instead_of_panicking() {
        // A line number past the end of the source, and columns past the end of
        // the line, must clamp rather than index out of bounds.
        let raw = "Generated.lean:9999:9999: error: phantom";
        let r = render_feedback(FormalSystem::Lean, raw, SRC, &FeedbackConfig::default());
        assert!(r.parsed);
        assert!(r.text.contains("phantom"));
        // Multi-byte source must not be split mid-character.
        let uni = "theorem β : ∀ x, x = x := by\n  exact λ y => rfl\n";
        let raw2 = "Generated.lean:1:9: error: unicode span";
        let r2 = render_feedback(FormalSystem::Lean, raw2, uni, &FeedbackConfig::default());
        assert!(r2.text.contains(ERROR_OPEN));
    }

    #[test]
    fn rendering_is_byte_stable_for_the_same_input() {
        let raw = "Generated.lean:6:20: error: unknown identifier 'bogus'\nGenerated.lean:4:3: warning: unused variable";
        let cfg = FeedbackConfig::default();
        let a = render_feedback(FormalSystem::Lean, raw, SRC, &cfg);
        let b = render_feedback(FormalSystem::Lean, raw, SRC, &cfg);
        assert_eq!(a.text, b.text, "rendering must be deterministic");
        assert_eq!(a.diagnostics, b.diagnostics);
        assert_eq!(a.text.as_bytes(), b.text.as_bytes());
        // Diagnostics keep the checker's own order (no sorting, no hashing).
        assert_eq!(a.diagnostics[0].line, Some(6));
        assert_eq!(a.diagnostics[1].line, Some(4));
        assert_eq!(a.diagnostics[1].severity, Severity::Warning);
    }

    #[test]
    fn rocq_and_isabelle_locators_parse() {
        let rocq = "File \"Generated.v\", line 12, characters 4-9:\nError: The reference bogus was not found.";
        let d = parse_diagnostics(FormalSystem::Rocq, rocq);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].line, Some(12));
        assert_eq!((d[0].col_start, d[0].col_end), (Some(4), Some(9)));

        let isa =
            "*** Undefined fact: \"bogus\"\n*** At command \"by\" (line 7 of \"Generated.thy\")";
        let d = parse_diagnostics(FormalSystem::Isabelle, isa);
        assert_eq!(d.len(), 1, "the `At command` line locates the block: {d:?}");
        assert_eq!(d[0].line, Some(7));
        assert!(d[0].message.contains("Undefined fact"));
    }

    #[test]
    fn goal_state_slot_renders_when_a_caller_populates_it() {
        let mut d = parse_diagnostics(
            FormalSystem::Lean,
            "Generated.lean:6:20: error: unsolved goals",
        );
        d[0].goal_state_slot = Some("n : Nat\n⊢ n + 0 = n".into());
        let src_lines: Vec<&str> = SRC.lines().collect();
        let text = render_one(&src_lines, &d[0], &FeedbackConfig::default(), 1);
        assert!(text.contains("goal state:"));
        assert!(text.contains("⊢ n + 0 = n"));
    }

    #[test]
    fn elide_keeps_a_balanced_head_and_tail() {
        let lines: Vec<String> = (1..=10).map(|i| i.to_string()).collect();
        let out = elide(lines.clone(), 6);
        assert_eq!(out.len(), 7, "6 kept lines + 1 marker");
        assert_eq!(out[3], TRUNCATION_MARKER);
        assert_eq!(out[0], "1");
        assert_eq!(out[6], "10");
        // Under threshold, and threshold 0, are pass-through.
        assert_eq!(elide(lines.clone(), 20), lines);
        assert_eq!(elide(lines.clone(), 0), lines);
    }
}
