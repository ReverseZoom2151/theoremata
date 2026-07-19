//! Statement-change guard (open-atp / Numina pattern).
//!
//! Snapshot theorem/lemma/def headers before a prover round-trip and verify they
//! are preserved afterward — reject weakened or deleted declarations.
//!
//! Headers alone are NOT enough. A header snapshot stops at `:=` (see
//! [`split_name_sig`]), so a submission that keeps `def A : Nat` intact while
//! rewriting its body from `4` to `5` used to pass the guard untouched. That is
//! a soundness hole and not a cosmetic one: a proof that silently redefines what
//! a definition MEANS proves a different theorem than the one that was asked.
//! So the guard also snapshots DEFINITION BODIES ([`snapshot_definitions`]) and
//! reports any drift.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TheoremHeader {
    pub kind: String,
    pub name: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatementGuardReport {
    pub preserved: bool,
    pub before: Vec<TheoremHeader>,
    pub after: Vec<TheoremHeader>,
    pub missing: Vec<String>,
    pub weakened: Vec<String>,
    /// Definition-body drift, filled in by [`guard_lean_round_trip`]. The fields
    /// carry `#[serde(default)]` so reports serialized before this check existed
    /// still deserialize.
    #[serde(default)]
    pub definitions: DefinitionDriftReport,
}

/// Extract `theorem` / `lemma` / `def` header signatures from Lean source.
pub fn snapshot_headers(lean_src: &str) -> Vec<TheoremHeader> {
    let cleaned = strip_comments(lean_src);
    let mut out = Vec::new();
    for line in cleaned.lines() {
        let trimmed = line.trim();
        let Some((kind, rest)) = parse_decl_prefix(trimmed) else {
            continue;
        };
        let Some((name, signature)) = split_name_sig(rest) else {
            continue;
        };
        out.push(TheoremHeader {
            kind: kind.to_string(),
            name: name.to_string(),
            signature: normalize_ws(signature),
        });
    }
    out
}

pub fn headers_preserved(
    before: &[TheoremHeader],
    after: &[TheoremHeader],
) -> StatementGuardReport {
    let after_map: std::collections::HashMap<_, _> =
        after.iter().map(|h| (h.name.as_str(), h)).collect();
    let mut missing = Vec::new();
    let mut weakened = Vec::new();
    for h in before {
        match after_map.get(h.name.as_str()) {
            None => missing.push(h.name.clone()),
            Some(a) => {
                if !signature_covers(&h.signature, &a.signature) {
                    weakened.push(h.name.clone());
                }
            }
        }
    }
    let preserved = missing.is_empty() && weakened.is_empty();
    StatementGuardReport {
        preserved,
        before: before.to_vec(),
        after: after.to_vec(),
        missing,
        weakened,
        // Header-only callers have no sources to diff, so the definition check
        // is vacuously clean here; `guard_lean_round_trip` fills it in.
        definitions: DefinitionDriftReport::default(),
    }
}

pub fn guard_lean_round_trip(before_src: &str, after_src: &str) -> StatementGuardReport {
    let mut report =
        headers_preserved(&snapshot_headers(before_src), &snapshot_headers(after_src));
    report.definitions = definitions_preserved(
        &snapshot_definitions(before_src),
        &snapshot_definitions(after_src),
    );
    // Definition drift is a statement change, so it must move the one flag every
    // caller reads. `verify.rs` gates `statement_preserved` on `preserved`, and a
    // redefined `def` is exactly as fatal as a weakened hypothesis.
    report.preserved = report.preserved && report.definitions.preserved;
    report
}

pub fn guard_report_json(report: &StatementGuardReport) -> Value {
    serde_json::to_value(report).unwrap_or(Value::Null)
}

/// Outcome of a statement-guard RESTORE pass (open-atp / Numina
/// `restore_initial_statements`): the rewritten source plus the names touched.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatementRestore {
    /// The source with drifted headers restored and deleted declarations
    /// re-appended as `… := by sorry`.
    pub restored_src: String,
    /// Declarations whose header signature had drifted and was rewritten back to
    /// the snapshot (proof body preserved).
    pub rewritten: Vec<String>,
    /// Declarations that had been deleted and were re-appended as an open
    /// obligation (`<header> := by sorry`).
    pub reappended: Vec<String>,
}

/// Restore drifted statements rather than only rejecting them (Numina
/// `StatementTracker.restore_initial_statements`).
///
/// For each header present in the `before` snapshot:
/// * if it survives in `after` but its signature drifted (no longer covered),
///   the after-side header is rewritten to the original signature while keeping
///   whatever proof body the model produced (`:= …`);
/// * if it was deleted, the original header is re-appended as an explicit open
///   obligation `<kind> <name> <signature> := by sorry`, so the statement is
///   never silently dropped.
///
/// Non-drifted declarations and any extra declarations the model added are left
/// untouched.
///
/// This restores HEADERS ONLY. A drifted definition BODY is detected by
/// [`definitions_preserved`] and reported, never rewritten: splicing a body back
/// in would leave the surrounding proof referring to a definition it was not
/// written against, which is a broken file presented as a repaired one.
pub fn restore_statements(before_src: &str, after_src: &str) -> StatementRestore {
    let before = snapshot_headers(before_src);
    let report = guard_lean_round_trip(before_src, after_src);
    let weakened: std::collections::HashSet<&str> =
        report.weakened.iter().map(String::as_str).collect();
    let missing: std::collections::HashSet<&str> =
        report.missing.iter().map(String::as_str).collect();
    let by_name: std::collections::HashMap<&str, &TheoremHeader> =
        before.iter().map(|h| (h.name.as_str(), h)).collect();

    // Rewrite drifted headers in place, preserving the proof body.
    let mut rewritten = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    for line in after_src.lines() {
        let trimmed = line.trim_start();
        if let Some((_kind, rest)) = parse_decl_prefix(trimmed) {
            if let Some((name, _sig)) = split_name_sig(rest) {
                if weakened.contains(name) {
                    if let Some(orig) = by_name.get(name) {
                        let indent = &line[..line.len() - trimmed.len()];
                        // Keep the model's `:= …` body if present, else `by sorry`.
                        let body = line
                            .split_once(":=")
                            .map(|(_, b)| b.trim().to_string())
                            .filter(|b| !b.is_empty())
                            .unwrap_or_else(|| "by sorry".to_string());
                        lines.push(format!(
                            "{indent}{} {} {} := {body}",
                            orig.kind, orig.name, orig.signature
                        ));
                        rewritten.push(name.to_string());
                        continue;
                    }
                }
            }
        }
        lines.push(line.to_string());
    }

    // Re-append deleted declarations as explicit open obligations.
    let mut reappended = Vec::new();
    let mut restored_src = lines.join("\n");
    for h in &before {
        if missing.contains(h.name.as_str()) {
            if !restored_src.ends_with('\n') && !restored_src.is_empty() {
                restored_src.push('\n');
            }
            restored_src.push_str(&format!(
                "{} {} {} := by sorry\n",
                h.kind, h.name, h.signature
            ));
            reappended.push(h.name.clone());
        }
    }

    StatementRestore {
        restored_src,
        rewritten,
        reappended,
    }
}

// ===========================================================================
// Definition-body drift
// ===========================================================================

/// One definition-shaped declaration, captured HEADER AND BODY.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DefinitionBody {
    /// `def`, `abbrev`, `instance`, `structure`, `class`, or `inductive`.
    pub kind: String,
    /// The declared name. Anonymous declarations (`instance : Foo Bar where …`)
    /// get a synthesized positional key; see [`snapshot_definitions`].
    pub name: String,
    /// The whole declaration text (header plus body), comment-stripped and
    /// whitespace-normalized. The FULL text is kept rather than only the part
    /// after `:=` because definition forms disagree about where a body starts:
    /// `def f : T := e`, `def f | 0 => a | n+1 => b`, `structure S where …` and
    /// `instance : C T where …` all carry meaning outside any `:=`. Comparing
    /// the whole region needs no per-form parser and cannot silently look at the
    /// wrong half.
    pub text: String,
}

/// Result of comparing the definition bodies of two sources.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DefinitionDriftReport {
    pub preserved: bool,
    pub before: Vec<DefinitionBody>,
    pub after: Vec<DefinitionBody>,
    /// Definitions present in both sources whose text differs.
    pub changed: Vec<String>,
    /// Definitions present before and absent after.
    pub removed: Vec<String>,
    /// Names declared more than once on either side. The comparison cannot say
    /// WHICH one corresponds to which, so they are reported rather than guessed.
    pub ambiguous: Vec<String>,
}

/// The empty report: nothing was captured, so nothing drifted. This is what
/// [`headers_preserved`] carries (it is given headers, not sources) and what a
/// report serialized before this check existed deserializes to.
impl Default for DefinitionDriftReport {
    fn default() -> Self {
        Self {
            preserved: true,
            before: Vec::new(),
            after: Vec::new(),
            changed: Vec::new(),
            removed: Vec::new(),
            ambiguous: Vec::new(),
        }
    }
}

/// Declaration forms whose BODY carries mathematical meaning and therefore must
/// not drift.
///
/// `theorem` / `lemma` / `example` are deliberately ABSENT: their bodies are
/// PROOFS, and producing a different proof is the entire point of the round
/// trip. Their statements are already guarded by [`snapshot_headers`].
///
/// `axiom` and `opaque` are absent too: they have no body to drift, and their
/// signature is a header the existing check would need to cover (it does not
/// today, and widening `parse_decl_prefix` is outside this change).
const DEFINITION_KINDS: &[&str] = &[
    "def",
    "abbrev",
    "instance",
    "structure",
    "class",
    "inductive",
];

/// Keywords that begin a NEW top-level item and therefore END the body region of
/// the declaration before them. Anything else at any indentation is treated as a
/// continuation of the current body.
///
/// Erring toward a LONGER region is the safe direction: an over-long region can
/// only report drift that is not there (a retry), while an over-short one would
/// stop looking before the changed text (a hole).
const DECL_STARTERS: &[&str] = &[
    "theorem",
    "lemma",
    "def",
    "abbrev",
    "instance",
    "structure",
    "class",
    "inductive",
    "example",
    "axiom",
    "opaque",
    "namespace",
    "section",
    "end",
    "open",
    "import",
    "variable",
    "variables",
    "universe",
    "macro",
    "macro_rules",
    "notation",
    "syntax",
    "elab",
    "set_option",
    "attribute",
    "mutual",
    "initialize",
];

/// Leading modifiers that may sit between the start of a line and its keyword.
const DECL_MODIFIERS: &[&str] = &[
    "private",
    "protected",
    "noncomputable",
    "partial",
    "unsafe",
    "scoped",
    "local",
    "nonrec",
];

/// Extract every definition-shaped declaration with its body from Lean source.
///
/// Anonymous declarations are keyed positionally (`<anonymous instance #0>`), so
/// reordering two anonymous instances reports drift that is not there. That is
/// the fail-closed direction: the alternative, skipping them, would let an
/// anonymous instance be rewritten unseen.
pub fn snapshot_definitions(lean_src: &str) -> Vec<DefinitionBody> {
    let cleaned = strip_comments(lean_src);
    let lines: Vec<&str> = cleaned.lines().collect();
    let mut out: Vec<DefinitionBody> = Vec::new();
    let mut anon = 0usize;

    for (i, line) in lines.iter().enumerate() {
        let indent = indent_width(line);
        let head = strip_decl_modifiers(line.trim_start());
        let Some(kind) = DEFINITION_KINDS.iter().find(|k| starts_with_word(head, k)) else {
            continue;
        };

        // The region runs to the next line that starts a new item at the same or
        // an outer indentation.
        let mut end = lines.len();
        for (j, later) in lines.iter().enumerate().skip(i + 1) {
            let later_head = strip_decl_modifiers(later.trim_start());
            if later_head.is_empty() {
                continue;
            }
            if indent_width(later) <= indent
                && DECL_STARTERS.iter().any(|k| starts_with_word(later_head, k))
            {
                end = j;
                break;
            }
        }

        let name = match decl_name(head, kind) {
            Some(n) => n,
            None => {
                let key = format!("<anonymous {kind} #{anon}>");
                anon += 1;
                key
            }
        };
        out.push(DefinitionBody {
            kind: (*kind).to_string(),
            name,
            // Normalizing whitespace is what makes a REFORMATTED body compare
            // equal to the original: indentation, line breaks and comments all
            // vanish, while every token survives.
            text: normalize_ws(&lines[i..end].join(" ")),
        });
    }
    out
}

/// Compare two definition snapshots. Fails closed: anything the comparison
/// cannot match up confidently is reported, never assumed unchanged.
pub fn definitions_preserved(
    before: &[DefinitionBody],
    after: &[DefinitionBody],
) -> DefinitionDriftReport {
    let mut changed = Vec::new();
    let mut removed = Vec::new();
    let mut ambiguous = Vec::new();
    let mut seen: Vec<&str> = Vec::new();

    for b in before {
        // One report line per name even if the name is declared twice.
        if seen.contains(&b.name.as_str()) {
            continue;
        }
        seen.push(b.name.as_str());

        let befores = before.iter().filter(|d| d.name == b.name).count();
        let matches: Vec<&DefinitionBody> = after.iter().filter(|d| d.name == b.name).collect();
        if befores > 1 || matches.len() > 1 {
            ambiguous.push(b.name.clone());
            continue;
        }
        match matches.first() {
            // A definition the submission dropped is drift too: whatever used it
            // now means something else, or nothing.
            None => removed.push(b.name.clone()),
            Some(a) => {
                if a.text != b.text || a.kind != b.kind {
                    changed.push(b.name.clone());
                }
            }
        }
    }

    DefinitionDriftReport {
        preserved: changed.is_empty() && removed.is_empty() && ambiguous.is_empty(),
        before: before.to_vec(),
        after: after.to_vec(),
        changed,
        removed,
        ambiguous,
    }
}

fn indent_width(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// True when `text` begins with `word` followed by a non-identifier character,
/// so `definition` is not mistaken for `def`.
fn starts_with_word(text: &str, word: &str) -> bool {
    match text.strip_prefix(word) {
        None => false,
        Some(rest) => match rest.chars().next() {
            None => true,
            Some(c) => !(c.is_alphanumeric() || c == '_' || c == '\'' || c == '.'),
        },
    }
}

/// Drop leading `@[…]` attributes and modifier keywords so the declaration
/// keyword is at the front.
///
/// An attribute that spans lines is NOT handled: the `@[` line keeps its
/// brackets and the declaration is not recognized on that line. If the same
/// shape appears on both sides it is simply not compared (an accepted blind
/// spot); if it appears on only one side, the name mismatch reports drift.
fn strip_decl_modifiers(line: &str) -> &str {
    let mut rest = line.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("@[") {
            match after.find(']') {
                Some(idx) => {
                    rest = after[idx + 1..].trim_start();
                    continue;
                }
                None => return rest,
            }
        }
        match DECL_MODIFIERS.iter().find(|m| starts_with_word(rest, m)) {
            Some(m) => rest = rest[m.len()..].trim_start(),
            None => return rest,
        }
    }
}

/// The declared name in `<kind> <name> …`, or `None` when the declaration is
/// anonymous (`instance : Foo Bar where …`).
fn decl_name(head: &str, kind: &str) -> Option<String> {
    let rest = head[kind.len()..].trim_start();
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '\'' || *c == '.')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut in_line = false;
    let mut block_depth = 0u32;
    while let Some(c) = chars.next() {
        if in_line {
            if c == '\n' {
                in_line = false;
                out.push(c);
            } else {
                out.push(' ');
            }
            continue;
        }
        if block_depth > 0 {
            if c == '/' && chars.peek() == Some(&'-') {
                chars.next();
                block_depth += 1;
                out.push(' ');
                out.push(' ');
                continue;
            }
            if c == '-' && chars.peek() == Some(&'/') {
                chars.next();
                block_depth = block_depth.saturating_sub(1);
                out.push(' ');
                out.push(' ');
                continue;
            }
            out.push(if c == '\n' { '\n' } else { ' ' });
            continue;
        }
        if c == '-' && chars.peek() == Some(&'-') {
            chars.next();
            in_line = true;
            out.push(' ');
            out.push(' ');
            continue;
        }
        if c == '/' && chars.peek() == Some(&'-') {
            chars.next();
            block_depth = 1;
            out.push(' ');
            out.push(' ');
            continue;
        }
        out.push(c);
    }
    out
}

fn parse_decl_prefix(line: &str) -> Option<(&str, &str)> {
    for kind in ["theorem", "lemma", "def"] {
        if let Some(rest) = line.strip_prefix(kind) {
            let rest = rest.trim_start();
            if !rest.is_empty() {
                return Some((kind, rest));
            }
        }
    }
    None
}

fn split_name_sig(rest: &str) -> Option<(&str, &str)> {
    let mut name_end = 0usize;
    for (i, ch) in rest.char_indices() {
        if ch.is_whitespace() || ch == '(' || ch == ':' {
            name_end = i;
            break;
        }
    }
    if name_end == 0 {
        return None;
    }
    let name = rest[..name_end].trim();
    let sig = rest[name_end..].split(":=").next().unwrap_or("").trim();
    if name.is_empty() || sig.is_empty() {
        return None;
    }
    Some((name, sig))
}

fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn signature_covers(expected: &str, actual: &str) -> bool {
    let e = normalize_ws(expected);
    let a = normalize_ws(actual);
    e == a || a.contains(&e) || e.contains(&a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_deleted_theorem() {
        let before = "theorem T (n : Nat) : n = n := by sorry";
        let after = "def helper : True := trivial";
        let report = guard_lean_round_trip(before, after);
        assert!(!report.preserved);
        assert_eq!(report.missing, vec!["T".to_string()]);
    }

    #[test]
    fn accepts_unchanged_header() {
        let src = "theorem T (n : Nat) : n = n := by exact rfl";
        let report = guard_lean_round_trip(src, src);
        assert!(report.preserved);
    }

    #[test]
    fn restores_a_weakened_statement_keeping_the_body() {
        // The model weakened the hypothesis but produced a body; restore the
        // original signature and keep its `:= …` body.
        let before = "theorem T (n : Nat) (h : n > 0) : n = n := by sorry";
        let after = "theorem T (n : Nat) : n = n := by exact rfl";
        let restore = restore_statements(before, after);
        assert_eq!(restore.rewritten, vec!["T".to_string()]);
        assert!(restore.reappended.is_empty());
        assert!(restore.restored_src.contains("(h : n > 0)"));
        assert!(restore.restored_src.contains(":= by exact rfl"));
        // The restored source now preserves the original statement.
        let report = guard_lean_round_trip(before, &restore.restored_src);
        assert!(report.preserved);
    }

    #[test]
    fn reappends_a_deleted_statement_as_sorry() {
        let before = "theorem T (n : Nat) : n = n := by sorry";
        let after = "def helper : True := trivial";
        let restore = restore_statements(before, after);
        assert_eq!(restore.reappended, vec!["T".to_string()]);
        assert!(restore.restored_src.contains("theorem T"));
        assert!(restore.restored_src.contains(":= by sorry"));
        // helper is left untouched.
        assert!(restore.restored_src.contains("def helper"));
        // The re-appended statement is now present (as an open obligation).
        let report = guard_lean_round_trip(before, &restore.restored_src);
        assert!(report.preserved);
    }

    // -- definition-body drift ------------------------------------------------

    /// The header snapshot on its own cannot see a body change. This test pins
    /// WHY the definition check has to exist: it is the exact hole.
    #[test]
    fn header_snapshot_alone_is_blind_to_a_body_change() {
        let before = "def A : Nat := 4";
        let after = "def A : Nat := 5";
        assert_eq!(snapshot_headers(before), snapshot_headers(after));
    }

    #[test]
    fn unchanged_definition_passes() {
        let src = "def A : Nat := 4\ntheorem T : A = 4 := by rfl\n";
        let report = guard_lean_round_trip(src, src);
        assert!(report.preserved);
        assert!(report.definitions.preserved);
        assert_eq!(report.definitions.before.len(), 1);
        assert_eq!(report.definitions.before[0].name, "A");
    }

    #[test]
    fn changed_definition_body_is_caught() {
        let before = "def A : Nat := 4\ntheorem T : A = 4 := by rfl\n";
        let after = "def A : Nat := 5\ntheorem T : A = 4 := by rfl\n";
        let report = guard_lean_round_trip(before, after);
        assert!(!report.preserved, "a redefined `def` must not pass");
        assert_eq!(report.definitions.changed, vec!["A".to_string()]);
        assert!(report.missing.is_empty());
        assert!(report.weakened.is_empty());
    }

    /// The `def A := 4` shape has no type ascription, so it never even reached
    /// the header snapshot. The definition check must still see it.
    #[test]
    fn changed_untyped_definition_body_is_caught() {
        let report = guard_lean_round_trip("def A := 4\n", "def A := 5\n");
        assert!(!report.preserved);
        assert_eq!(report.definitions.changed, vec!["A".to_string()]);
    }

    /// Reformatting is not redefinition: comments, indentation and line breaks
    /// are normalized away, so an identical body still passes.
    #[test]
    fn reformatted_identical_definition_passes() {
        let before = "def twice (n : Nat) : Nat := n + n\n";
        let after = "-- a doc line\ndef twice (n : Nat) : Nat :=\n    n + n   /- trailing note -/\n";
        let report = guard_lean_round_trip(before, after);
        assert!(
            report.definitions.preserved,
            "reformatting must not read as drift: {:?}",
            report.definitions.changed
        );
        assert!(report.preserved);
    }

    /// A multi-line body is captured to the next top-level declaration, so a
    /// change on its LAST line is still seen.
    #[test]
    fn multi_line_definition_body_change_is_caught() {
        let before = "def f (n : Nat) : Nat :=\n  let k := n + 1\n  k * 2\n\ntheorem T : True := trivial\n";
        let after = "def f (n : Nat) : Nat :=\n  let k := n + 1\n  k * 3\n\ntheorem T : True := trivial\n";
        let report = guard_lean_round_trip(before, after);
        assert_eq!(report.definitions.changed, vec!["f".to_string()]);
    }

    /// A different PROOF for the same theorem is the expected outcome of a
    /// prover round trip and must not trip the definition check.
    #[test]
    fn changed_theorem_proof_body_does_not_trip_the_definition_check() {
        let before = "def A : Nat := 4\ntheorem T : A = A := by sorry\n";
        let after = "def A : Nat := 4\ntheorem T : A = A := by\n  simp\n  rfl\n";
        let report = guard_lean_round_trip(before, after);
        assert!(
            report.definitions.preserved,
            "theorem bodies are proofs: {:?}",
            report.definitions.changed
        );
        assert!(report.preserved);
        // And the definition next to it was captured, so the check really ran.
        assert_eq!(report.definitions.before.len(), 1);
    }

    /// A deleted definition is drift too.
    #[test]
    fn removed_definition_is_reported() {
        let report = guard_lean_round_trip("def A : Nat := 4\n", "theorem T : True := trivial\n");
        assert!(!report.preserved);
        assert_eq!(report.definitions.removed, vec!["A".to_string()]);
    }

    /// Fail closed: a name declared twice cannot be matched up, so it is
    /// reported rather than assumed unchanged.
    #[test]
    fn duplicate_definition_names_are_ambiguous() {
        let src = "def A : Nat := 4\ndef A : Nat := 4\n";
        let report = guard_lean_round_trip(src, src);
        assert!(!report.preserved);
        assert_eq!(report.definitions.ambiguous, vec!["A".to_string()]);
    }

    /// `abbrev`, `structure` and `instance` carry meaning as well.
    #[test]
    fn other_definition_forms_are_covered() {
        let before = "abbrev B : Nat := 1\nstructure S where\n  x : Nat\n";
        let after = "abbrev B : Nat := 2\nstructure S where\n  x : Int\n";
        let report = guard_lean_round_trip(before, after);
        let mut changed = report.definitions.changed.clone();
        changed.sort();
        assert_eq!(changed, vec!["B".to_string(), "S".to_string()]);
    }

    /// An anonymous instance is keyed positionally rather than skipped, so a
    /// rewrite of its body is still seen.
    #[test]
    fn anonymous_instance_body_change_is_caught() {
        let before = "instance : Inhabited Nat where\n  default := 0\n";
        let after = "instance : Inhabited Nat where\n  default := 1\n";
        let report = guard_lean_round_trip(before, after);
        assert_eq!(
            report.definitions.changed,
            vec!["<anonymous instance #0>".to_string()]
        );
    }

    /// Modifiers and attributes in front of the keyword must not hide a `def`.
    #[test]
    fn modifiers_and_attributes_do_not_hide_a_definition() {
        let before = "@[simp] private noncomputable def A : Nat := 4\n";
        let after = "@[simp] private noncomputable def A : Nat := 5\n";
        let report = guard_lean_round_trip(before, after);
        assert_eq!(report.definitions.changed, vec!["A".to_string()]);
    }

    /// `definition` must not be read as `def`, and nothing here is a definition.
    #[test]
    fn word_boundaries_are_respected() {
        assert!(snapshot_definitions("-- definition of A\ntheorem T : True := trivial\n").is_empty());
        assert!(starts_with_word("def A", "def"));
        assert!(!starts_with_word("definition A", "def"));
    }
}
