//! Subgoal extraction from FAILURE, not only from success (`sorry2lemma`).
//!
//! A mined system's `sorry2lemma` pass lifts not just the explicit `sorry`s of a
//! *completed-modulo-holes* proof but the unsolved goals sitting at **error
//! locations** into standalone top-level lemmas. That is strictly more useful
//! than extracting from a proof that compiled: today a partial proof that errors
//! out is discarded whole, even though the positions it failed at are exactly
//! the well-formed subproblems we would want to enqueue as claim-DAG nodes.
//!
//! This module turns a failed attempt into a list of [`ExtractedSubgoal`]s from
//! two sources:
//!
//! * **[`SubgoalOrigin::ExplicitHole`]** — a `sorry` / `admit` / `Admitted` /
//!   `{! !}` / `postulate` / `?` token that survives in the submitted source,
//!   found by scanning the source outside comments.
//! * **[`SubgoalOrigin::ErrorLocation`]** — a position the checker itself
//!   reported, taken from the [`Diagnostic`]s that
//!   [`crate::prover::error_feedback::parse_diagnostics`] already produced. No
//!   second diagnostic type is defined here; this module consumes that one.
//!
//! # THE INVARIANT: an extracted subgoal is a HYPOTHESIS, never an assertion
//!
//! **An [`ExtractedSubgoal`] asserts only that the parent proof did not close
//! this position. It asserts NOTHING about whether the goal is true, provable,
//! well-formed, or even correctly transcribed.** The goal text, when present,
//! came from a checker's *error message* — prose written for a human, not a
//! certified statement — and the parent proof that produced it is by
//! construction a FAILED proof. Every obligation this module emits therefore
//! enters the claim DAG **unproved**, and must never be admitted, discharged,
//! assumed, or counted as evidence on the extractor's say-so. There is no code
//! path here that can mark anything proved: [`to_obligations`] produces
//! [`crate::decompose::Obligation`]s, whose only downstream constructor
//! (`ChildProposal::from_obligation`) fixes child status at `Unproved` by
//! construction. Nothing in this module may ever be changed to bypass that.
//!
//! # Honesty about unrecoverable goal text
//!
//! Goal text is genuinely not recoverable from stdout alone for most systems.
//! Lean prints the goal under `unsolved goals`; a caller holding a warm session
//! may have filled [`Diagnostic::goal_state_slot`]. Rocq, Isabelle, Agda and
//! Metamath print a *message* about the failure and not the proof state. When
//! the text cannot be recovered, this module records the **span with no
//! statement** rather than inventing one — see
//! [`ExtractedSubgoal::statement`] and [`UNRECOVERED_GOAL`]. A fabricated goal
//! would be worse than none: it would enter the DAG looking like a real claim.
//!
//! Pure and deterministic: no IO, no clock, no RNG, no process spawning. The
//! same three inputs always yield the same `Vec` in the same order.

use crate::decompose::Obligation;
use crate::prover::error_feedback::{Diagnostic, Severity};
use crate::prover::formal::FormalSystem;
use serde::{Deserialize, Serialize};

/// The statement text used when the goal could not be recovered. It is a
/// deliberate non-statement: it starts with `[` so no formal-system parser can
/// mistake it for a term, and it names itself. Downstream code that needs a real
/// statement must test [`ExtractedSubgoal::has_statement`] rather than parsing
/// this.
pub const UNRECOVERED_GOAL: &str = "[goal text not recoverable from checker output]";

/// Where an extracted subgoal came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubgoalOrigin {
    /// A hole token literally present in the submitted source.
    ExplicitHole,
    /// A position the checker reported an error at.
    ErrorLocation,
}

impl SubgoalOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            SubgoalOrigin::ExplicitHole => "explicit_hole",
            SubgoalOrigin::ErrorLocation => "error_location",
        }
    }
}

/// A 1-based source region, mirroring [`Diagnostic`]'s coordinate conventions:
/// lines and columns are 1-based, `col_end` is exclusive, and `None` means the
/// coordinate was not reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub start_line: usize,
    pub end_line: usize,
    pub col_start: Option<usize>,
    pub col_end: Option<usize>,
}

/// How the parent proof would invoke the lifted lemma in place of the hole or
/// failing step. This is a **suggestion for the repair prompt**, not something
/// this module splices into source: rewriting the parent is the caller's job and
/// requires a re-verification the extractor cannot perform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconstructedCallsite {
    /// Proposed top-level name for the lifted lemma. Sanitized to
    /// `[A-Za-z0-9_]` and made unique within one extraction.
    pub lemma_name: String,
    /// The tactic/term text that would stand where the hole is, e.g.
    /// `exact subgoal_1` for Lean or `apply subgoal_1.` for Rocq.
    pub invocation: String,
    /// The exact source text being replaced, when it is a literal hole token
    /// (`sorry`, `admit`, …). `None` for an error location, where what should be
    /// replaced is a failing tactic the extractor does not attempt to delimit.
    pub replaces: Option<String>,
    /// The enclosing declaration's name, when one was recoverable by scanning
    /// backwards for a declaration keyword.
    pub parent_decl: Option<String>,
}

/// One lifted subgoal: a position the parent proof did not close.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedSubgoal {
    pub system: FormalSystem,
    pub origin: SubgoalOrigin,
    /// The goal statement text, **only when genuinely recoverable**. `None`
    /// means the span is real but the checker never printed the goal; callers
    /// must not fabricate one. See the module docs.
    pub statement: Option<String>,
    /// The region of the submitted source this came from.
    pub span: SourceSpan,
    /// The checker's message, for an [`SubgoalOrigin::ErrorLocation`].
    pub message: Option<String>,
    /// The source line text at `span.start_line`, for prompt context.
    pub source_line: Option<String>,
    pub callsite: ReconstructedCallsite,
}

impl ExtractedSubgoal {
    /// Whether a real goal statement was recovered. Callers that need a
    /// statement must gate on this instead of unwrapping [`Self::statement`].
    pub fn has_statement(&self) -> bool {
        self.statement.is_some()
    }

    /// A short human/prompt label.
    pub fn title(&self) -> String {
        format!(
            "{} (line {}, {})",
            self.callsite.lemma_name,
            self.span.start_line,
            self.origin.as_str()
        )
    }

    /// This subgoal as a decomposition [`Obligation`].
    ///
    /// The obligation carries no `claim_kind` and no transfer ingredients: the
    /// extractor has no basis to type a claim it may not even have the text of.
    /// When the goal was unrecoverable the statement is [`UNRECOVERED_GOAL`],
    /// which is intentionally unparseable — an obligation that cannot be stated
    /// still records that the position needs proving, and MUST NOT be discharged
    /// until a real statement is attached.
    pub fn to_obligation(&self) -> Obligation {
        Obligation {
            title: self.title(),
            statement: self
                .statement
                .clone()
                .unwrap_or_else(|| UNRECOVERED_GOAL.to_string()),
            claim_kind: None,
            ingredients: Vec::new(),
        }
    }
}

/// Extract every subgoal a failed attempt exposes.
///
/// `source` is the text that was submitted; `diagnostics` are the parsed
/// [`Diagnostic`]s from [`crate::prover::error_feedback::parse_diagnostics`] for
/// the same attempt. Diagnostics whose `system` differs from `system` are
/// ignored rather than reinterpreted under the wrong format.
///
/// A clean proof — no holes, no error diagnostics — yields an empty vec.
///
/// Ordering is deterministic: by start line, then start column (unpositioned
/// last), then origin (`ExplicitHole` before `ErrorLocation`), with ties broken
/// by discovery order (source scan first, then checker order). No hashing, no
/// sorting by anything address- or time-derived.
pub fn extract_subgoals(
    system: FormalSystem,
    source: &str,
    diagnostics: &[Diagnostic],
) -> Vec<ExtractedSubgoal> {
    let lines: Vec<&str> = source.lines().collect();
    let masked = mask_comments(system, &lines);

    let mut raw: Vec<Draft> = Vec::new();
    raw.extend(explicit_holes(system, &masked));
    raw.extend(error_locations(system, &lines, diagnostics));

    // Drop an error location that lands exactly on a hole we already lifted: it
    // is the same position reported twice (e.g. Lean's "declaration uses
    // 'sorry'" alongside the token itself), not two subgoals.
    let hole_positions: Vec<(usize, Option<usize>)> = raw
        .iter()
        .filter(|d| d.origin == SubgoalOrigin::ExplicitHole)
        .map(|d| (d.span.start_line, d.span.col_start))
        .collect();
    raw.retain(|d| {
        d.origin == SubgoalOrigin::ExplicitHole
            || !hole_positions.iter().any(|(l, c)| {
                *l == d.span.start_line && (d.span.col_start.is_none() || *c == d.span.col_start)
            })
    });

    // Stable sort keeps discovery order for equal keys.
    raw.sort_by_key(|d| {
        (
            d.span.start_line,
            d.span.col_start.unwrap_or(usize::MAX),
            d.origin,
        )
    });

    let mut used: Vec<String> = Vec::new();
    raw.into_iter()
        .enumerate()
        .map(|(i, d)| {
            let parent = enclosing_decl(system, &masked, d.span.start_line);
            let lemma_name = unique_name(&mut used, &proposed_name(parent.as_deref(), i + 1));
            let invocation = invocation_for(system, &lemma_name);
            ExtractedSubgoal {
                system,
                origin: d.origin,
                statement: d.statement,
                span: d.span,
                message: d.message,
                source_line: lines
                    .get(d.span.start_line.saturating_sub(1))
                    .map(|l| l.trim_end().to_string()),
                callsite: ReconstructedCallsite {
                    lemma_name,
                    invocation,
                    replaces: d.replaces,
                    parent_decl: parent,
                },
            }
        })
        .collect()
}

/// Every extracted subgoal as a decomposition obligation, in extraction order.
///
/// These enter the claim DAG **unproved**, without exception. See the module
/// docs: the extractor is a source of hypotheses, never of proof.
pub fn to_obligations(subgoals: &[ExtractedSubgoal]) -> Vec<Obligation> {
    subgoals.iter().map(ExtractedSubgoal::to_obligation).collect()
}

// --- internal draft -------------------------------------------------------

struct Draft {
    origin: SubgoalOrigin,
    span: SourceSpan,
    statement: Option<String>,
    message: Option<String>,
    replaces: Option<String>,
}

// --- (a) explicit holes ---------------------------------------------------

/// One hole token for a system. `ident_like` tokens must not match inside a
/// longer identifier (`sorry` must not fire on `sorryAx` or `no_sorrying`);
/// symbolic ones (`{!`, `?`) are matched literally.
///
/// `.` is deliberately NOT an identifier byte for boundary purposes: Rocq and
/// Isabelle terminate every command with one, so `admit.` and `Admitted.` must
/// still match. The cost is that a qualified `Foo.sorry` also matches, which
/// over-extracts rather than under-extracts and so only ever adds an unproved
/// obligation.
struct HoleToken {
    text: &'static str,
    ident_like: bool,
}

fn hole_tokens(system: FormalSystem) -> &'static [HoleToken] {
    match system {
        FormalSystem::Lean => &[HoleToken {
            text: "sorry",
            ident_like: true,
        }],
        // `admit` closes one goal, `Admitted` closes the whole proof
        // incompletely, `give_up` is the SSReflect spelling.
        FormalSystem::Rocq => &[
            HoleToken {
                text: "admit",
                ident_like: true,
            },
            HoleToken {
                text: "Admitted",
                ident_like: true,
            },
            HoleToken {
                text: "give_up",
                ident_like: true,
            },
        ],
        // Isabelle's `sorry` admits the goal; `oops` abandons the proof.
        FormalSystem::Isabelle => &[
            HoleToken {
                text: "sorry",
                ident_like: true,
            },
            HoleToken {
                text: "oops",
                ident_like: true,
            },
        ],
        // Agda interaction holes are `{! !}` (or bare `?`); `postulate` asserts
        // without proof and the source gate already treats it as a hole.
        FormalSystem::Agda => &[
            HoleToken {
                text: "{!",
                ident_like: false,
            },
            HoleToken {
                text: "postulate",
                ident_like: true,
            },
        ],
        // Metamath marks an unknown proof step with a bare `?` token.
        FormalSystem::Metamath => &[HoleToken {
            text: "?",
            ident_like: false,
        }],
        // Candle/HOL Light has no admitted-goal syntax we can rely on; `cheat_tac`
        // is the closest analogue in the HOL family.
        FormalSystem::Candle => &[HoleToken {
            text: "cheat_tac",
            ident_like: true,
        }],
    }
}

fn explicit_holes(system: FormalSystem, masked: &[String]) -> Vec<Draft> {
    let mut out = Vec::new();
    for (i, line) in masked.iter().enumerate() {
        for tok in hole_tokens(system) {
            for byte_idx in find_token(line, tok) {
                let col = line[..byte_idx].chars().count() + 1;
                let width = tok.text.chars().count();
                out.push(Draft {
                    origin: SubgoalOrigin::ExplicitHole,
                    span: SourceSpan {
                        start_line: i + 1,
                        end_line: i + 1,
                        col_start: Some(col),
                        col_end: Some(col + width),
                    },
                    // A hole token carries no goal text. The enclosing
                    // declaration's statement is the PARENT's goal, not this
                    // subgoal's, so using it would be a fabrication.
                    statement: None,
                    message: Some(format!("explicit `{}` hole", tok.text)),
                    replaces: Some(tok.text.to_string()),
                });
            }
        }
    }
    out
}

/// Byte offsets of every occurrence of `tok` in `line`, respecting identifier
/// boundaries for identifier-like tokens.
fn find_token(line: &str, tok: &HoleToken) -> Vec<usize> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let needle = tok.text.as_bytes();
    if needle.is_empty() || needle.len() > bytes.len() {
        return out;
    }
    let mut i = 0usize;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle && line.is_char_boundary(i) {
            let ok = !tok.ident_like
                || (!is_ident_byte(if i == 0 { None } else { Some(bytes[i - 1]) })
                    && !is_ident_byte(bytes.get(i + needle.len()).copied()));
            if ok {
                out.push(i);
                i += needle.len();
                continue;
            }
        }
        i += 1;
    }
    out
}

fn is_ident_byte(b: Option<u8>) -> bool {
    match b {
        Some(b) => b.is_ascii_alphanumeric() || b == b'_' || b == b'\'',
        None => false,
    }
}

// --- (b) error locations --------------------------------------------------

fn error_locations(
    system: FormalSystem,
    lines: &[&str],
    diagnostics: &[Diagnostic],
) -> Vec<Draft> {
    let mut out = Vec::new();
    for d in diagnostics {
        // Never reinterpret another system's format, and never lift a warning or
        // an info note: only an error marks a position the proof failed to
        // close.
        if d.system != system || d.severity != Severity::Error {
            continue;
        }
        let start = match d.line {
            Some(l) if l >= 1 => l,
            // A file-level error with no position is not a subgoal: there is no
            // span to lift. Dropping it is the honest choice.
            _ => continue,
        };
        let start = start.min(lines.len().max(1));
        let end = d.end_line.unwrap_or(start).max(start).min(lines.len().max(1));
        out.push(Draft {
            origin: SubgoalOrigin::ErrorLocation,
            span: SourceSpan {
                start_line: start,
                end_line: end,
                col_start: d.col_start,
                col_end: d.col_end,
            },
            statement: recover_goal_text(d),
            message: Some(d.message.trim_end().to_string()),
            replaces: None,
        });
    }
    out
}

/// Recover the goal statement from a diagnostic, or `None`.
///
/// Two honest sources only:
///
/// 1. [`Diagnostic::goal_state_slot`], when a caller holding a warm session
///    filled it — this is the checker's own pretty-printed proof state.
/// 2. Lean's `unsolved goals` body, which prints the hypotheses and a `⊢` goal
///    inline in the message. The turnstile is required: without it the body is
///    prose, not a goal.
///
/// Rocq, Isabelle, Agda, Metamath and Candle print a *message about* the
/// failure, not the proof state, so nothing is recoverable from stdout alone and
/// this returns `None`. That is a span-only subgoal, by design.
fn recover_goal_text(d: &Diagnostic) -> Option<String> {
    if let Some(goal) = &d.goal_state_slot {
        let g = goal.trim();
        if !g.is_empty() {
            return Some(g.to_string());
        }
    }
    if d.system == FormalSystem::Lean {
        let low = d.message.to_ascii_lowercase();
        if low.contains("unsolved goals") {
            let body: String = d
                .message
                .lines()
                .skip(1)
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string();
            if body.contains('⊢') {
                return Some(body);
            }
        }
    }
    None
}

// --- comments -------------------------------------------------------------

/// Comment delimiters per system: `(block_open, block_close, line)`.
fn comment_syntax(system: FormalSystem) -> (&'static str, &'static str, Option<&'static str>) {
    match system {
        FormalSystem::Lean => ("/-", "-/", Some("--")),
        FormalSystem::Rocq => ("(*", "*)", None),
        FormalSystem::Isabelle => ("(*", "*)", None),
        FormalSystem::Agda => ("{-", "-}", Some("--")),
        FormalSystem::Metamath => ("$(", "$)", None),
        FormalSystem::Candle => ("(*", "*)", None),
    }
}

/// Replace comment content with spaces, preserving every line's length in
/// characters so columns computed against the mask are columns in the original.
///
/// Deliberately simple: it does not model nesting depth or string literals. A
/// `sorry` written inside a string is vanishingly rare compared to one written
/// inside a comment, and over-masking only ever loses a subgoal — it can never
/// invent one.
fn mask_comments(system: FormalSystem, lines: &[&str]) -> Vec<String> {
    let (open, close, line_c) = comment_syntax(system);
    let mut out = Vec::with_capacity(lines.len());
    let mut in_block = false;
    for line in lines {
        let chars: Vec<char> = line.chars().collect();
        let mut kept: Vec<char> = Vec::with_capacity(chars.len());
        let mut i = 0usize;
        while i < chars.len() {
            if in_block {
                if starts_with_at(&chars, i, close) {
                    for _ in 0..close.chars().count() {
                        kept.push(' ');
                    }
                    i += close.chars().count();
                    in_block = false;
                    continue;
                }
                kept.push(' ');
                i += 1;
                continue;
            }
            if starts_with_at(&chars, i, open) {
                for _ in 0..open.chars().count() {
                    kept.push(' ');
                }
                i += open.chars().count();
                in_block = true;
                continue;
            }
            if let Some(lc) = line_c {
                if starts_with_at(&chars, i, lc) {
                    while i < chars.len() {
                        kept.push(' ');
                        i += 1;
                    }
                    break;
                }
            }
            kept.push(chars[i]);
            i += 1;
        }
        out.push(kept.into_iter().collect());
    }
    out
}

fn starts_with_at(chars: &[char], i: usize, pat: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    if i + p.len() > chars.len() {
        return false;
    }
    chars[i..i + p.len()] == p[..]
}

// --- callsite reconstruction ---------------------------------------------

fn decl_keywords(system: FormalSystem) -> &'static [&'static str] {
    match system {
        FormalSystem::Lean => &["theorem", "lemma", "example", "def", "instance"],
        FormalSystem::Rocq => &[
            "Theorem",
            "Lemma",
            "Corollary",
            "Proposition",
            "Definition",
            "Fact",
        ],
        FormalSystem::Isabelle => &["theorem", "lemma", "corollary", "definition"],
        FormalSystem::Agda => &["module", "data", "record"],
        FormalSystem::Metamath => &["$p", "$a"],
        FormalSystem::Candle => &["let", "prove"],
    }
}

/// The name of the declaration enclosing `line` (1-based), found by scanning
/// backwards for the nearest declaration keyword. `None` when none is found or
/// the keyword is not followed by a name.
fn enclosing_decl(system: FormalSystem, masked: &[String], line: usize) -> Option<String> {
    let idx = line.saturating_sub(1).min(masked.len().saturating_sub(1));
    for i in (0..=idx).rev() {
        let l = masked.get(i)?;
        let trimmed = l.trim_start();
        for kw in decl_keywords(system) {
            // Metamath labels PRECEDE the `$p`/`$a` keyword: `mylabel $p … $.`
            if system == FormalSystem::Metamath {
                if let Some(pos) = trimmed.find(kw) {
                    let label = trimmed[..pos].split_whitespace().next_back();
                    if let Some(label) = label.filter(|s| !s.is_empty()) {
                        return Some(label.to_string());
                    }
                }
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix(kw) {
                if !rest.starts_with(|c: char| {
                    c.is_alphanumeric() || c == '_' || c == '\''
                }) {
                    let name = rest
                        .trim_start()
                        .split(|c: char| c.is_whitespace() || "(){}[]:,".contains(c))
                        .find(|s| !s.is_empty());
                    if let Some(name) = name {
                        return Some(name.to_string());
                    }
                }
            }
        }
    }
    None
}

fn proposed_name(parent: Option<&str>, index: usize) -> String {
    match parent {
        Some(p) => format!("{}_subgoal_{index}", sanitize(p)),
        None => format!("subgoal_{index}"),
    }
}

/// Keep `[A-Za-z0-9_]`, collapse everything else to `_`, and ensure the result
/// starts with a letter or `_` so it is a legal identifier in every system here.
fn sanitize(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if s.is_empty() {
        s.push('_');
    }
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s
}

fn unique_name(used: &mut Vec<String>, candidate: &str) -> String {
    let mut name = candidate.to_string();
    let mut n = 2usize;
    while used.iter().any(|u| u == &name) {
        name = format!("{candidate}_{n}");
        n += 1;
    }
    used.push(name.clone());
    name
}

/// How the parent would invoke the lifted lemma, in each system's own syntax.
fn invocation_for(system: FormalSystem, name: &str) -> String {
    match system {
        FormalSystem::Lean => format!("exact {name}"),
        FormalSystem::Rocq => format!("apply {name}."),
        FormalSystem::Isabelle => format!("by (rule {name})"),
        FormalSystem::Agda => name.to_string(),
        // Metamath proofs are label sequences; the lemma's label IS the step.
        FormalSystem::Metamath => name.to_string(),
        FormalSystem::Candle => format!("MATCH_ACCEPT_TAC {name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prover::error_feedback::parse_diagnostics;

    const LEAN_SRC: &str = "import Mathlib\n\ntheorem foo (n : Nat) : n + 0 = n := by\n  induction n with\n  | zero => rfl\n  | succ k ih => sorry\n";

    fn lean_diag(msg: &str, line: usize) -> Diagnostic {
        parse_diagnostics(
            FormalSystem::Lean,
            &format!("Generated.lean:{line}:2: error: {msg}"),
        )
        .remove(0)
    }

    #[test]
    fn an_explicit_sorry_is_extracted_with_its_span() {
        let got = extract_subgoals(FormalSystem::Lean, LEAN_SRC, &[]);
        assert_eq!(got.len(), 1, "{got:?}");
        let s = &got[0];
        assert_eq!(s.origin, SubgoalOrigin::ExplicitHole);
        assert_eq!(s.span.start_line, 6);
        // `  | succ k ih => sorry` — `sorry` starts at 1-based column 18.
        assert_eq!(s.span.col_start, Some(18));
        assert_eq!(s.span.col_end, Some(23));
        assert_eq!(s.callsite.replaces.as_deref(), Some("sorry"));
        assert_eq!(s.callsite.parent_decl.as_deref(), Some("foo"));
        assert_eq!(s.callsite.lemma_name, "foo_subgoal_1");
        assert_eq!(s.callsite.invocation, "exact foo_subgoal_1");
        // A hole token carries no goal text, and none is invented.
        assert!(!s.has_statement());
        assert_eq!(s.to_obligation().statement, UNRECOVERED_GOAL);
    }

    #[test]
    fn a_diagnostic_at_an_error_location_yields_an_error_location_subgoal() {
        let src = "theorem foo : True := by\n  exact bogus\n";
        let d = lean_diag("unknown identifier 'bogus'", 2);
        let got = extract_subgoals(FormalSystem::Lean, src, &[d]);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].origin, SubgoalOrigin::ErrorLocation);
        assert_eq!(got[0].span.start_line, 2);
        assert!(got[0].message.as_deref().unwrap().contains("bogus"));
        assert!(got[0].callsite.replaces.is_none());
        assert_eq!(got[0].source_line.as_deref(), Some("  exact bogus"));
    }

    #[test]
    fn a_clean_proof_yields_none() {
        let src = "theorem foo : True := by\n  trivial\n";
        assert!(extract_subgoals(FormalSystem::Lean, src, &[]).is_empty());
        // Warnings are not failures to close a goal, so they lift nothing.
        let warn = parse_diagnostics(
            FormalSystem::Lean,
            "Generated.lean:2:2: warning: unused variable",
        );
        assert!(extract_subgoals(FormalSystem::Lean, src, &warn).is_empty());
        // A `sorry` inside a comment is not a hole.
        let commented = "theorem foo : True := by\n  -- sorry, this used to be a hole\n  trivial\n/- sorry -/\n";
        assert!(
            extract_subgoals(FormalSystem::Lean, commented, &[]).is_empty(),
            "commented-out holes must not be lifted"
        );
        // Nor is `sorry` inside a longer identifier.
        let ident = "theorem foo : True := by\n  exact sorryAx True false\n";
        assert!(extract_subgoals(FormalSystem::Lean, ident, &[]).is_empty());
    }

    #[test]
    fn unrecoverable_goal_text_yields_a_span_only_entry() {
        // Rocq prints a message ABOUT the failure, never the proof state.
        let src = "Theorem foo : True.\nProof.\n  apply bogus.\nQed.\n";
        let d = parse_diagnostics(
            FormalSystem::Rocq,
            "File \"F.v\", line 3, characters 2-13:\nError: The reference bogus was not found.",
        );
        let got = extract_subgoals(FormalSystem::Rocq, src, &d);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].span.start_line, 3);
        assert_eq!(got[0].span.col_start, Some(2));
        assert!(
            !got[0].has_statement(),
            "no goal text is recoverable; none must be invented"
        );
        assert_eq!(got[0].statement, None);
        assert_eq!(got[0].to_obligation().statement, UNRECOVERED_GOAL);
        assert_eq!(got[0].callsite.invocation, "apply foo_subgoal_1.");

        // Lean's `unsolved goals` body DOES carry the goal, so it is recovered.
        let raw = "Generated.lean:2:2: error: unsolved goals\nn : Nat\n⊢ n + 0 = n";
        let ld = parse_diagnostics(FormalSystem::Lean, raw);
        let got = extract_subgoals(FormalSystem::Lean, "theorem t : True := by\n  skip\n", &ld);
        assert_eq!(got[0].statement.as_deref(), Some("n : Nat\n⊢ n + 0 = n"));
        // ...and a goal-state slot filled by a warm-session caller wins.
        let mut ld2 = ld.clone();
        ld2[0].goal_state_slot = Some("⊢ False".into());
        let got2 = extract_subgoals(FormalSystem::Lean, "theorem t : True := by\n  skip\n", &ld2);
        assert_eq!(got2[0].statement.as_deref(), Some("⊢ False"));
        // An `unsolved goals` message with no turnstile is prose, not a goal.
        let prose = parse_diagnostics(
            FormalSystem::Lean,
            "Generated.lean:2:2: error: unsolved goals\nsee the docs",
        );
        let got3 = extract_subgoals(FormalSystem::Lean, "theorem t : True := by\n  skip\n", &prose);
        assert!(!got3[0].has_statement());
    }

    #[test]
    fn extraction_is_deterministically_ordered() {
        let src = "theorem a : True := by\n  sorry\n\ntheorem b : True := by\n  sorry\n  exact bogus\n";
        let diags = parse_diagnostics(
            FormalSystem::Lean,
            "Generated.lean:6:2: error: unknown identifier 'bogus'\nGenerated.lean:2:2: error: declaration uses 'sorry'",
        );
        let a = extract_subgoals(FormalSystem::Lean, src, &diags);
        let b = extract_subgoals(FormalSystem::Lean, src, &diags);
        assert_eq!(a, b, "extraction must be deterministic");
        let lines: Vec<usize> = a.iter().map(|s| s.span.start_line).collect();
        assert_eq!(lines, vec![2, 5, 6], "sorted by position: {a:?}");
        // The diagnostic that landed exactly on the line-2 hole was deduplicated.
        assert_eq!(a[0].origin, SubgoalOrigin::ExplicitHole);
        assert_eq!(a[2].origin, SubgoalOrigin::ErrorLocation);
        // Lemma names are unique and index by final order.
        let names: Vec<&str> = a.iter().map(|s| s.callsite.lemma_name.as_str()).collect();
        assert_eq!(names, vec!["a_subgoal_1", "b_subgoal_2", "b_subgoal_3"]);
    }

    #[test]
    fn every_obligation_enters_unproved_and_untyped() {
        let got = extract_subgoals(FormalSystem::Lean, LEAN_SRC, &[]);
        let obs = to_obligations(&got);
        assert_eq!(obs.len(), got.len());
        // The extractor never types a claim it may not even have the text of...
        assert!(obs.iter().all(|o| o.claim_kind.is_none()));
        assert!(obs.iter().all(|o| o.ingredients.is_empty()));
        // ...and the only downstream constructor fixes status at Unproved.
        let proposal = crate::decompose::Decomposer::admission_proposal(
            "parent statement",
            &obs,
            &crate::decomposition_admission::DischargeProbe::default(),
        );
        assert!(
            proposal.children.iter().all(|c| !c.status.is_assertion()),
            "an extracted subgoal must never enter as an assertion"
        );
    }

    #[test]
    fn per_system_hole_syntax_dispatches() {
        let cases: Vec<(FormalSystem, &str, &str)> = vec![
            (FormalSystem::Rocq, "Lemma l : True.\nProof.\n  admit.\nAdmitted.\n", "admit"),
            (FormalSystem::Isabelle, "lemma l: \"True\"\n  sorry\n", "sorry"),
            (FormalSystem::Agda, "module M where\npostulate p : Set\n", "postulate"),
            (FormalSystem::Metamath, "mylab $p |- ph $= ? $.\n", "?"),
        ];
        for (system, src, expect) in cases {
            let got = extract_subgoals(system, src, &[]);
            assert!(
                got.iter().any(|s| s.callsite.replaces.as_deref() == Some(expect)),
                "{system:?} must find `{expect}`: {got:?}"
            );
            assert!(got.iter().all(|s| s.origin == SubgoalOrigin::ExplicitHole));
        }
        // Rocq finds BOTH the `admit.` and the `Admitted.`
        let rocq = extract_subgoals(
            FormalSystem::Rocq,
            "Lemma l : True.\nProof.\n  admit.\nAdmitted.\n",
            &[],
        );
        assert_eq!(rocq.len(), 2, "{rocq:?}");
        assert_eq!(rocq[0].span.start_line, 3);
        assert_eq!(rocq[1].span.start_line, 4);
        // Metamath labels precede the keyword.
        let mm = extract_subgoals(FormalSystem::Metamath, "mylab $p |- ph $= ? $.\n", &[]);
        assert_eq!(mm[0].callsite.parent_decl.as_deref(), Some("mylab"));
        assert_eq!(mm[0].callsite.lemma_name, "mylab_subgoal_1");
    }

    #[test]
    fn foreign_and_unpositioned_diagnostics_are_ignored_not_reinterpreted() {
        let src = "theorem foo : True := by\n  trivial\n";
        // A Rocq diagnostic handed to a Lean extraction is dropped.
        let rocq = parse_diagnostics(FormalSystem::Rocq, "Error: something");
        assert!(extract_subgoals(FormalSystem::Lean, src, &rocq).is_empty());
        // A Lean error with no position has no span to lift.
        let bare = parse_diagnostics(FormalSystem::Lean, "error: whole-file failure");
        assert_eq!(bare.len(), 1);
        assert!(bare[0].line.is_none());
        assert!(extract_subgoals(FormalSystem::Lean, src, &bare).is_empty());
        // Out-of-range lines clamp instead of panicking.
        let far = lean_diag("phantom", 9999);
        let got = extract_subgoals(FormalSystem::Lean, src, &[far]);
        assert_eq!(got.len(), 1);
        assert!(got[0].span.start_line <= src.lines().count());
        // Empty source never panics.
        let _ = extract_subgoals(FormalSystem::Lean, "", &[lean_diag("x", 1)]);
    }
}
