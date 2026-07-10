//! HOL Light axiom-base / definitional-extension AUDITOR (build #16).
//!
//! This is a *trust-boundary* checker: a lexical + light-structural pass over
//! HOL Light OCaml proof scripts (`.ml`) that flags the ways a script can widen
//! the trusted base or bypass the kernel. It is OUR code, std-only + `serde_json`
//! (already a dependency of [`crate::prover::formal`]), from first principles —
//! never an external tool — so it can run offline as part of the Candle backend's
//! `source_scan` / axiom-audit layers even when no toolchain is present.
//!
//! The bug classes it targets (all flagged by the resource mining) are:
//!
//! * **`mk_thm`** — fabricates a `thm` value with *no* kernel proof, bypassing the
//!   LCF kernel entirely. Always CRITICAL.
//! * **`new_axiom`** (and axiom-introducing likes) not on the whitelist — asserts
//!   a theorem by fiat, widening HOL Light's tiny fixed axiom base. CRITICAL
//!   unless the bound name is one of the trusted axioms in `whitelist`
//!   (`ETA_AX` / `SELECT_AX` / `INFINITY_AX`).
//! * **Bad definitions** — a definitional extension where a *type variable*
//!   appears in the definiens (RHS) but not in the defined constant (LHS). This is
//!   the classic HOL soundness side-condition; violating it lets one derive a
//!   contradiction. CRITICAL. (Best-effort/lexical: we compare the type variables
//!   syntactically present on each side of the definitional equation. A constant
//!   whose polymorphism is carried only by an *inferred* type — with no annotation
//!   on the LHS — can therefore be over-flagged; documented as a conservative,
//!   fail-closed heuristic.)
//! * **`INST` / `INST_TYPE`** capture — variable/type-variable capture during
//!   instantiation. Genuine capture is undecidable lexically and HOL Light's own
//!   `INST`/`INST_TYPE` are capture-avoiding, so these are WARNINGS flagged for
//!   manual review, not gate-failing CRITICALs.
//!
//! Results reuse the existing [`ScanReport`] (layer 2c) and [`AxiomReport`]
//! (layer 2a) shapes from [`crate::prover::formal`] rather than inventing new
//! gate-result types: [`AxiomAudit::into_scan_report`] plugs straight into the
//! Candle backend's `source_scan`, and [`AxiomAudit::to_axiom_report`] into its
//! `audit_axioms`. CRITICAL findings make the report un-clean (fail-closed);
//! WARNINGs are surfaced but do not by themselves fail the gate.

use crate::prover::formal::{AxiomReport, ScanReport};
use serde_json::json;

/// Severity of an audit finding. CRITICAL findings fail the gate (an actual
/// unsoundness / kernel bypass); WARNINGs are surfaced for review only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Critical,
    Warning,
}

impl Severity {
    /// Uppercase tag used in human-readable finding strings and JSON detail.
    pub fn tag(self) -> &'static str {
        match self {
            Severity::Critical => "CRITICAL",
            Severity::Warning => "WARNING",
        }
    }
}

/// A single audit finding: what rule fired, where (1-based line), and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    /// Stable rule identifier (`mk_thm` / `new_axiom` / `bad_definition` /
    /// `INST` / `INST_TYPE`).
    pub rule: &'static str,
    /// 1-based source line of the flagged construct.
    pub line: usize,
    /// Human-readable explanation.
    pub detail: String,
}

impl Finding {
    pub fn critical(rule: &'static str, line: usize, detail: String) -> Self {
        Self { severity: Severity::Critical, rule, line, detail }
    }

    pub fn warning(rule: &'static str, line: usize, detail: String) -> Self {
        Self { severity: Severity::Warning, rule, line, detail }
    }
}

/// The result of [`audit_hol_light`]: the ordered findings plus the axiom names
/// the script introduces via `new_axiom` (for the [`AxiomReport`] view).
#[derive(Debug, Clone)]
pub struct AxiomAudit {
    /// Findings, deterministically ordered by `(line, rule)`.
    pub findings: Vec<Finding>,
    /// Every axiom name introduced by a `new_axiom` binding (allowed or not),
    /// sorted + de-duplicated.
    pub axioms: Vec<String>,
}

impl AxiomAudit {
    /// A proof is clean iff it has NO CRITICAL findings. WARNINGs (possible
    /// capture) are surfaced but do not fail the gate.
    pub fn is_clean(&self) -> bool {
        !self.findings.iter().any(|f| f.severity == Severity::Critical)
    }

    /// Number of CRITICAL findings.
    pub fn critical_count(&self) -> usize {
        self.findings.iter().filter(|f| f.severity == Severity::Critical).count()
    }

    /// Number of WARNING findings.
    pub fn warning_count(&self) -> usize {
        self.findings.iter().filter(|f| f.severity == Severity::Warning).count()
    }

    /// One human-readable line per finding: `CRITICAL: rule (line N): detail`.
    fn finding_strings(&self) -> Vec<String> {
        self.findings
            .iter()
            .map(|f| format!("{}: {} (line {}): {}", f.severity.tag(), f.rule, f.line, f.detail))
            .collect()
    }

    fn findings_json(&self) -> serde_json::Value {
        serde_json::Value::Array(
            self.findings
                .iter()
                .map(|f| {
                    json!({
                        "severity": f.severity.tag(),
                        "rule": f.rule,
                        "line": f.line,
                        "detail": f.detail,
                    })
                })
                .collect(),
        )
    }

    /// Layer 2c view: a [`ScanReport`] the Candle backend's `source_scan` can
    /// return directly. `clean` reflects CRITICAL findings only (fail-closed);
    /// both criticals and warnings appear in `findings`, and the structured
    /// finding list lives in `detail`.
    pub fn into_scan_report(self) -> ScanReport {
        let clean = self.is_clean();
        let findings = self.finding_strings();
        let detail = json!({
            "system": "candle",
            "auditor": "hol_light_axiom_audit",
            "critical": self.critical_count(),
            "warning": self.warning_count(),
            "findings": self.findings_json(),
            "axioms": self.axioms,
        });
        ScanReport { clean, findings, detail }
    }

    /// Layer 2a view: an [`AxiomReport`] the Candle backend's `audit_axioms` can
    /// return. `within_whitelist` reflects CRITICAL findings (an undue
    /// `new_axiom`, `mk_thm`, or bad definition takes the proof out of the
    /// trusted base); `axioms` are the names the script introduces.
    pub fn to_axiom_report(&self, whitelist: &[String]) -> AxiomReport {
        AxiomReport {
            axioms: self.axioms.clone(),
            within_whitelist: self.is_clean(),
            detail: json!({
                "auditor": "hol_light_axiom_audit",
                "critical": self.critical_count(),
                "warning": self.warning_count(),
                "findings": self.findings_json(),
                "whitelist": whitelist,
            }),
        }
    }
}

/// Audit a HOL Light OCaml proof script against `whitelist` (the trusted axiom
/// names, e.g. `ETA_AX` / `SELECT_AX` / `INFINITY_AX`). Deterministic: the same
/// input always yields the same ordered findings.
pub fn audit_hol_light(code: &str, whitelist: &[String]) -> AxiomAudit {
    // Work over comment/string-stripped source (newlines + length preserved, so
    // line numbers stay accurate) so `(* mk_thm *)` in a comment or a `"mk_thm"`
    // string literal does not false-flag. Backquoted `` `...` `` HOL terms are
    // preserved — definitions live there.
    let sanitized: Vec<char> = sanitize(code).chars().collect();
    let chars = &sanitized[..];

    let mut findings: Vec<Finding> = Vec::new();
    let mut axioms: Vec<String> = Vec::new();

    // (1) mk_thm — fabricated theorem, kernel bypass. Always CRITICAL.
    for pos in word_positions(chars, "mk_thm") {
        findings.push(Finding::critical(
            "mk_thm",
            line_of(chars, pos),
            "fabricated theorem: `mk_thm` builds a `thm` with no kernel proof, \
             bypassing the LCF kernel"
                .to_string(),
        ));
    }

    // (1b) CHEAT_TAC — HOL Light's `tactics.ml` defines it as
    // `ACCEPT_TAC(mk_thm([],w))`, i.e. a NAMED `mk_thm` wrapper that closes any
    // goal without proof. A raw `mk_thm` grep misses it, so flag it explicitly.
    for pos in word_positions(chars, "CHEAT_TAC") {
        findings.push(Finding::critical(
            "cheat_tac",
            line_of(chars, pos),
            "fabricated theorem: `CHEAT_TAC` is `ACCEPT_TAC(mk_thm([],w))` — it \
             closes the goal with an unproven theorem, bypassing the kernel"
                .to_string(),
        ));
    }

    // (2) new_axiom — undue axiom unless the bound name is whitelisted.
    for pos in word_positions(chars, "new_axiom") {
        let name = binding_name(chars, pos);
        if let Some(n) = &name {
            axioms.push(n.clone());
        }
        let whitelisted = name
            .as_ref()
            .map_or(false, |n| whitelist.iter().any(|w| w == n));
        if !whitelisted {
            let who = name.clone().unwrap_or_else(|| "<anonymous>".to_string());
            findings.push(Finding::critical(
                "new_axiom",
                line_of(chars, pos),
                format!(
                    "undue axiom `{who}` widens HOL Light's fixed axiom base \
                     (not in the trusted whitelist)"
                ),
            ));
        }
    }

    // (3) Bad definitions — a type variable in the definiens but not the constant.
    for intro in ["new_definition", "new_basic_definition", "define"] {
        for pos in word_positions(chars, intro) {
            let Some((lo, hi)) = backquoted_after(chars, pos + intro.chars().count()) else {
                continue;
            };
            let term = &chars[lo..hi];
            let Some((lhs, rhs)) = split_def(term) else {
                continue;
            };
            let lhs_vars = type_vars(lhs);
            let rhs_vars = type_vars(rhs);
            let cname = const_name(lhs);
            for v in &rhs_vars {
                if !lhs_vars.contains(v) {
                    findings.push(Finding::critical(
                        "bad_definition",
                        line_of(chars, pos),
                        format!(
                            "unsound definitional extension: type variable `{v}` occurs in \
                             the definiens but not in the defined constant `{cname}`"
                        ),
                    ));
                }
            }
        }
    }

    // (4) INST / INST_TYPE — possible capture. Best-effort WARNING only.
    for rule in ["INST_TYPE", "INST"] {
        for pos in word_positions(chars, rule) {
            findings.push(Finding::warning(
                rule,
                line_of(chars, pos),
                format!(
                    "`{rule}` instantiation: confirm no free variable/type variable is \
                     captured (best-effort; HOL Light's `{rule}` is capture-avoiding — \
                     flagged for manual review)"
                ),
            ));
        }
    }

    // Deterministic order: by line, then rule name.
    findings.sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.rule.cmp(b.rule)));
    axioms.sort();
    axioms.dedup();

    AxiomAudit { findings, axioms }
}

// --- lexical helpers ------------------------------------------------------

/// HOL Light / OCaml identifier char (primes and underscores included).
fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '\''
}

/// Replace OCaml `(* ... *)` comments (nested) and `"..."` string literals with
/// spaces, preserving newlines and the overall char count so byte-free char
/// offsets and line numbers stay aligned with the original. Backquoted HOL terms
/// are left intact.
fn sanitize(code: &str) -> String {
    let chars: Vec<char> = code.chars().collect();
    let mut out = String::with_capacity(chars.len());
    let mut i = 0usize;
    let mut depth = 0usize; // (* *) nesting depth
    let mut in_string = false;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if depth > 0 {
            if c == '*' && next == Some(')') {
                depth -= 1;
                out.push(' ');
                out.push(' ');
                i += 2;
                continue;
            }
            if c == '(' && next == Some('*') {
                depth += 1;
                out.push(' ');
                out.push(' ');
                i += 2;
                continue;
            }
            out.push(if c == '\n' { '\n' } else { ' ' });
            i += 1;
            continue;
        }
        if in_string {
            if c == '\\' && next.is_some() {
                out.push(' ');
                out.push(' ');
                i += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
                out.push(' ');
                i += 1;
                continue;
            }
            out.push(if c == '\n' { '\n' } else { ' ' });
            i += 1;
            continue;
        }
        if c == '(' && next == Some('*') {
            depth += 1;
            out.push(' ');
            out.push(' ');
            i += 2;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(' ');
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Byte-free whole-token occurrences of `needle` in `chars`: matches only where
/// the char before and after is not an identifier char (so `INST` does not match
/// inside `INST_TYPE`, nor `mk_thm` inside `mk_thmx`).
fn word_positions(chars: &[char], needle: &str) -> Vec<usize> {
    let n: Vec<char> = needle.chars().collect();
    let mut out = Vec::new();
    if n.is_empty() || chars.len() < n.len() {
        return out;
    }
    let mut i = 0usize;
    while i + n.len() <= chars.len() {
        if chars[i..i + n.len()] == n[..] {
            let before_ok = i == 0 || !is_ident(chars[i - 1]);
            let after_ok = chars.get(i + n.len()).map_or(true, |&c| !is_ident(c));
            if before_ok && after_ok {
                out.push(i);
                i += n.len();
                continue;
            }
        }
        i += 1;
    }
    out
}

/// 1-based line number of char index `idx`.
fn line_of(chars: &[char], idx: usize) -> usize {
    1 + chars[..idx.min(chars.len())].iter().filter(|&&c| c == '\n').count()
}

/// The `let NAME =` bound directly before `pos` within the same `;;`-terminated
/// statement, if any (the axiom name a `new_axiom` is assigned to).
fn binding_name(chars: &[char], pos: usize) -> Option<String> {
    // Statement start: just after the nearest preceding `;;`, else 0.
    let mut start = 0usize;
    let mut j = pos;
    while j >= 2 {
        if chars[j - 1] == ';' && chars[j - 2] == ';' {
            start = j;
            break;
        }
        j -= 1;
    }
    let seg = &chars[start..pos];
    let lets = word_positions(seg, "let");
    let lp = *lets.last()?;
    let mut k = lp + 3;
    while k < seg.len() && seg[k].is_whitespace() {
        k += 1;
    }
    let mut name = String::new();
    while k < seg.len() && is_ident(seg[k]) {
        name.push(seg[k]);
        k += 1;
    }
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Inner range `(start, end)` of the first backquoted `` `...` `` HOL term at or
/// after `from`, if any.
fn backquoted_after(chars: &[char], from: usize) -> Option<(usize, usize)> {
    let open = (from..chars.len()).find(|&i| chars[i] == '`')?;
    let close = (open + 1..chars.len()).find(|&i| chars[i] == '`')?;
    Some((open + 1, close))
}

/// Split a definitional term at its first top-level (paren/bracket depth 0) `=`,
/// yielding `(lhs, rhs)` — the defined constant and the definiens. `None` if
/// there is no top-level `=`.
fn split_def(term: &[char]) -> Option<(&[char], &[char])> {
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < term.len() {
        match term[i] {
            '(' | '[' => depth += 1,
            ')' | ']' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            '=' if depth == 0 => return Some((&term[..i], &term[i + 1..])),
            _ => {}
        }
        i += 1;
    }
    None
}

/// Distinct type variables (`'a`, `'b`, ...) syntactically present in `term`. A
/// leading `'` is a type variable only when it is NOT attached to a preceding
/// identifier (which would make it a primed variable name like `x'`).
fn type_vars(term: &[char]) -> Vec<String> {
    let mut vars: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < term.len() {
        if term[i] == '\'' {
            let attached = i > 0 && (term[i - 1].is_alphanumeric() || term[i - 1] == '_');
            if !attached {
                let mut k = i + 1;
                if k < term.len() && term[k].is_alphabetic() {
                    let mut name = String::from("'");
                    while k < term.len() && (term[k].is_alphanumeric() || term[k] == '_') {
                        name.push(term[k]);
                        k += 1;
                    }
                    if !vars.contains(&name) {
                        vars.push(name);
                    }
                    i = k;
                    continue;
                }
            }
        }
        i += 1;
    }
    vars
}

/// The defined constant's name: the first identifier in the LHS of a definition.
fn const_name(lhs: &[char]) -> String {
    let mut i = 0usize;
    while i < lhs.len() && !(lhs[i].is_alphabetic() || lhs[i] == '_') {
        i += 1;
    }
    let mut s = String::new();
    while i < lhs.len() && is_ident(lhs[i]) {
        s.push(lhs[i]);
        i += 1;
    }
    if s.is_empty() {
        "<unknown>".to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn whitelist() -> Vec<String> {
        vec!["ETA_AX".into(), "SELECT_AX".into(), "INFINITY_AX".into()]
    }

    /// A clean proof — only sound (annotated-polymorphic and monomorphic)
    /// definitions and whitelisted constants — passes with no findings.
    #[test]
    fn clean_proof_passes() {
        let code = "\
let TR = TRUTH;;
let MYADD = new_definition `myadd x y = x + y`;;
let ID_DEF = new_definition `(myid:'a->'a) = \\x. x`;;
";
        let audit = audit_hol_light(code, &whitelist());
        assert!(audit.is_clean(), "clean proof must pass: {:?}", audit.findings);
        assert_eq!(audit.critical_count(), 0);
        assert!(audit.into_scan_report().clean);
    }

    /// `mk_thm([], ...)` fabricates a theorem: CRITICAL, gate un-clean.
    #[test]
    fn mk_thm_is_critical() {
        let code = "let FAKE = mk_thm([], `p /\\ ~p`);;\n";
        let audit = audit_hol_light(code, &whitelist());
        assert!(!audit.is_clean());
        assert!(audit.findings.iter().any(|f| f.rule == "mk_thm" && f.severity == Severity::Critical));
        let report = audit.into_scan_report();
        assert!(!report.clean);
        assert!(report.findings.iter().any(|s| s.contains("CRITICAL") && s.contains("mk_thm")));
    }

    #[test]
    fn cheat_tac_is_critical() {
        // CHEAT_TAC = ACCEPT_TAC(mk_thm([],w)) — a named mk_thm wrapper a raw
        // `mk_thm` grep misses.
        let code = "let BOGUS = prove(`p /\\ ~p`, CHEAT_TAC);;\n";
        let audit = audit_hol_light(code, &whitelist());
        assert!(!audit.is_clean(), "CHEAT_TAC must fail the audit");
        assert!(audit
            .findings
            .iter()
            .any(|f| f.rule == "cheat_tac" && f.severity == Severity::Critical));
    }

    /// A `new_axiom` whose bound name is NOT whitelisted is flagged CRITICAL.
    #[test]
    fn undue_new_axiom_is_flagged() {
        let code = "let BAD_AX = new_axiom `!x. P x`;;\n";
        let audit = audit_hol_light(code, &whitelist());
        assert!(!audit.is_clean());
        assert!(audit.findings.iter().any(|f| f.rule == "new_axiom" && f.severity == Severity::Critical));
        assert!(audit.axioms.contains(&"BAD_AX".to_string()));
        // The AxiomReport view agrees: outside the whitelist.
        assert!(!audit.to_axiom_report(&whitelist()).within_whitelist);
    }

    /// A whitelisted axiom re-declared via `new_axiom` (bootstrapping the trusted
    /// base) is allowed — no CRITICAL.
    #[test]
    fn whitelisted_axiom_is_allowed() {
        let code = "let ETA_AX = new_axiom `!t:A->B. (\\x. t x) = t`;;\n";
        let audit = audit_hol_light(code, &whitelist());
        assert!(audit.is_clean(), "whitelisted axiom must be allowed: {:?}", audit.findings);
        assert!(audit.axioms.contains(&"ETA_AX".to_string()));
        assert!(audit.to_axiom_report(&whitelist()).within_whitelist);
    }

    /// A definition with a type variable in the definiens (RHS) but not in the
    /// defined constant (LHS) is an unsound extension: CRITICAL.
    #[test]
    fn bad_definition_is_flagged() {
        let code = "let BADDEF = new_definition `baddef = (?x:'a. F)`;;\n";
        let audit = audit_hol_light(code, &whitelist());
        assert!(!audit.is_clean());
        let f = audit
            .findings
            .iter()
            .find(|f| f.rule == "bad_definition")
            .expect("bad definition must be flagged");
        assert_eq!(f.severity, Severity::Critical);
        assert!(f.detail.contains("'a"));
        assert!(f.detail.contains("baddef"));
    }

    /// `INST` / `INST_TYPE` are WARNINGs (possible capture), surfaced but NOT
    /// gate-failing — a proof using only them stays clean.
    #[test]
    fn inst_is_warning_only() {
        let code = "let X = INST_TYPE [`:bool`,`:'a`] (INST [`0`,`x:num`] th);;\n";
        let audit = audit_hol_light(code, &whitelist());
        assert!(audit.is_clean(), "INST warnings must not fail the gate");
        assert!(audit.warning_count() >= 2);
        assert!(audit.findings.iter().any(|f| f.rule == "INST_TYPE" && f.severity == Severity::Warning));
        assert!(audit.findings.iter().any(|f| f.rule == "INST" && f.severity == Severity::Warning));
        // Warnings still surface in the ScanReport findings but keep it clean.
        let report = audit.into_scan_report();
        assert!(report.clean);
        assert!(report.findings.iter().any(|s| s.contains("WARNING")));
    }

    /// Comments and string literals must NOT trigger findings.
    #[test]
    fn comments_and_strings_are_ignored() {
        let code = "\
(* mk_thm here is only a comment, and new_axiom too *)
let MSG = \"mk_thm and new_axiom in a string\";;
let TR = TRUTH;;
";
        let audit = audit_hol_light(code, &whitelist());
        assert!(audit.is_clean(), "comments/strings must not flag: {:?}", audit.findings);
    }

    /// `INST` inside `INST_TYPE` is not double-counted (whole-token matching).
    #[test]
    fn inst_not_matched_inside_inst_type() {
        let code = "let X = INST_TYPE [] th;;\n";
        let audit = audit_hol_light(code, &whitelist());
        assert_eq!(audit.findings.iter().filter(|f| f.rule == "INST").count(), 0);
        assert_eq!(audit.findings.iter().filter(|f| f.rule == "INST_TYPE").count(), 1);
    }

    /// Deterministic: identical input yields identical findings (rules + lines).
    #[test]
    fn audit_is_deterministic() {
        let code = "\
let A = new_axiom `!x. Q x`;;
let B = mk_thm([], `T`);;
let C = INST [`0`,`n:num`] th;;
let D = new_definition `dd = (@y:'b. T)`;;
";
        let a = audit_hol_light(code, &whitelist());
        let b = audit_hol_light(code, &whitelist());
        let key = |x: &AxiomAudit| {
            x.findings
                .iter()
                .map(|f| (f.severity, f.rule, f.line))
                .collect::<Vec<_>>()
        };
        assert_eq!(key(&a), key(&b));
        // Sanity: findings ordered by line.
        let lines: Vec<usize> = a.findings.iter().map(|f| f.line).collect();
        let mut sorted = lines.clone();
        sorted.sort();
        assert_eq!(lines, sorted);
    }
}
