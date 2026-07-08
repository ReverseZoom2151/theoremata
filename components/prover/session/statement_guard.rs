//! Statement-change guard (open-atp / Numina pattern).
//!
//! Snapshot theorem/lemma/def headers before a prover round-trip and verify they
//! are preserved afterward — reject weakened or deleted declarations.

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

pub fn headers_preserved(before: &[TheoremHeader], after: &[TheoremHeader]) -> StatementGuardReport {
    let after_map: std::collections::HashMap<_, _> = after
        .iter()
        .map(|h| (h.name.as_str(), h))
        .collect();
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
    }
}

pub fn guard_lean_round_trip(before_src: &str, after_src: &str) -> StatementGuardReport {
    headers_preserved(&snapshot_headers(before_src), &snapshot_headers(after_src))
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
}