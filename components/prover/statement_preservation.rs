//! Canonical-statement PRESERVATION + escape-hatch guard (trust-boundary layer
//! 2c hardening).
//!
//! This is OUR code — a *trust-boundary* checker built from first principles,
//! std-only + `serde_json` (already a dependency of [`crate::prover::formal`]),
//! deterministic and offline. It hardens the 3+1-layer gate against three
//! reward-hacking / anti-cheat failure modes surfaced by the resource mining:
//!
//! * **Statement substitution** (Goedel-Prover's anti-cheat; Numina's
//!   statement-change guard): a model "proves" a *weakened*, *renamed*, or
//!   *trivially-restated* theorem and splices it back onto the canonical name.
//!   [`check_statement_preserved`] confirms the submitted proof declares the SAME
//!   theorem signature (name + binders + conclusion, up to whitespace / a
//!   best-effort alpha-rename) as the canonical statement — fail-closed when it
//!   cannot confirm a match.
//!
//! * **Proof-search escape hatches** (DeepSeek-Prover-V2's reward-hacking
//!   erratum): the interactive *suggestion* tactics `apply?` / `exact?` / `rfl?`
//!   report a "proof" that is non-reproducible (it depended on an editor/UI code
//!   path, not the kernel), alongside the classic `sorry` / `admit` gaps and the
//!   `native_decide` compiled-evaluator trust hole. [`scan_escape_hatches`] flags
//!   each as CRITICAL.
//!
//! * **Opaque repair loops** (Kimina's error-message / infotree formatting): a
//!   verifier error is far more useful to the repair loop when the offending
//!   line is shown in context. [`format_error_spans`] renders Lean errors with
//!   the offending line(s) marked (`>>>`) and ±2 lines of context.
//!
//! It is Lean-focused (the systems whose `?`-suggestion tactics and `sorry` /
//! `native_decide` hatches the erratum concerns), lexical / light-structural, and
//! best-effort: it never claims MORE soundness than it can prove, so an
//! un-confirmable statement match is reported as NOT preserved (fail-closed).
//! Results reuse the [`ScanReport`](crate::prover::formal::ScanReport) shape from
//! layer 2c rather than inventing a new gate-result type where a plug-in view is
//! useful.

use crate::prover::formal::{FormalSystem, ScanReport};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ===========================================================================
// Statement preservation
// ===========================================================================

/// A parsed declaration signature: the pieces the canonical / submitted match
/// compares (`theorem NAME <binders> : <conclusion>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TheoremSig {
    /// Declaration keyword (`theorem` / `lemma` / `example` / `def`).
    pub kind: String,
    /// Declared name (`example` yields the sentinel `"<example>"`).
    pub name: String,
    /// Binder region, whitespace-normalized (everything between the name and the
    /// top-level statement colon: `(n : Nat) (h : n > 0)` …). Empty when none.
    pub binders: String,
    /// Conclusion / goal, whitespace-normalized (everything after the top-level
    /// statement colon, up to `:=`). Empty for a `def` with no ascribed type.
    pub conclusion: String,
}

/// Structured verdict of [`check_statement_preserved`]. Only [`Preserved`] and
/// [`PreservedAlpha`] leave the gate open; every other verdict is fail-closed.
///
/// [`Preserved`]: PreservationVerdict::Preserved
/// [`PreservedAlpha`]: PreservationVerdict::PreservedAlpha
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreservationVerdict {
    /// Name + binders + conclusion match up to whitespace.
    Preserved,
    /// Match up to whitespace AND a best-effort positional alpha-rename of the
    /// bound variables (same structure, renamed binders).
    PreservedAlpha,
    /// A declaration with the canonical signature exists but under a DIFFERENT
    /// name (the canonical name is never actually proved).
    Renamed,
    /// The binder region differs (a hypothesis was added or, more dangerously,
    /// dropped — a possible weakening).
    BindersChanged,
    /// The conclusion / goal differs (a possible weakening or restatement).
    ConclusionChanged,
    /// The conclusion was replaced by a trivial proposition (`True` / `trivial`
    /// / `⊤`) — the canonical goal is not proved at all.
    TriviallyRestated,
    /// The canonical statement itself could not be parsed into a signature
    /// (fail-closed: we cannot confirm anything).
    CanonicalUnparsable,
    /// No declaration in the submission matches the canonical name OR signature.
    SubmittedMissing,
}

impl PreservationVerdict {
    /// Whether this verdict leaves the gate OPEN (statement genuinely preserved).
    pub fn is_preserved(self) -> bool {
        matches!(
            self,
            PreservationVerdict::Preserved | PreservationVerdict::PreservedAlpha
        )
    }

    /// Stable tag for finding strings / JSON detail.
    pub fn tag(self) -> &'static str {
        match self {
            PreservationVerdict::Preserved => "preserved",
            PreservationVerdict::PreservedAlpha => "preserved_alpha",
            PreservationVerdict::Renamed => "renamed",
            PreservationVerdict::BindersChanged => "binders_changed",
            PreservationVerdict::ConclusionChanged => "conclusion_changed",
            PreservationVerdict::TriviallyRestated => "trivially_restated",
            PreservationVerdict::CanonicalUnparsable => "canonical_unparsable",
            PreservationVerdict::SubmittedMissing => "submitted_missing",
        }
    }
}

/// The result of [`check_statement_preserved`], in the [`ScanReport`] idiom
/// (`preserved` / `findings` / `detail`) plus the structured [`verdict`] and the
/// two parsed signatures.
///
/// [`verdict`]: PreservationReport::verdict
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreservationReport {
    /// Fail-closed verdict: `true` iff the submitted proof declares the canonical
    /// statement (up to whitespace / alpha).
    pub preserved: bool,
    /// Structured verdict tag.
    pub verdict: PreservationVerdict,
    /// Human-readable finding lines (empty iff `preserved`).
    pub findings: Vec<String>,
    /// The canonical signature we parsed (`None` if unparsable).
    pub canonical: Option<TheoremSig>,
    /// The submitted signature we matched against (`None` if none found).
    pub submitted: Option<TheoremSig>,
    /// Structured detail for the gate's JSON report.
    pub detail: Value,
}

impl PreservationReport {
    /// Layer-2c view: a [`ScanReport`] the backend `source_scan` can fold in.
    /// `clean` mirrors `preserved` (fail-closed); the finding lines carry over.
    pub fn into_scan_report(self) -> ScanReport {
        let detail = json!({
            "check": "statement_preservation",
            "verdict": self.verdict.tag(),
            "canonical": &self.canonical,
            "submitted": &self.submitted,
        });
        ScanReport {
            clean: self.preserved,
            findings: self.findings,
            detail,
        }
    }
}

/// Confirm the submitted proof proves the ORIGINAL (canonical) statement.
///
/// The declared theorem signature — name + binders + conclusion — of the
/// canonical statement is compared against the matching declaration in
/// `submitted_code`, up to whitespace and a best-effort alpha-rename of the bound
/// variables. A weakened / renamed / trivially-restated statement is flagged and
/// the report is fail-closed (`preserved == false`).
///
/// Matching is lexical / light-structural and deliberately conservative: it never
/// reports `preserved` unless it can actually parse BOTH sides and see the same
/// signature, so an ambiguous or unparsable input fails closed rather than
/// silently passing.
pub fn check_statement_preserved(canonical_statement: &str, submitted_code: &str) -> PreservationReport {
    let Some(canonical) = parse_first_decl(canonical_statement) else {
        return report(
            PreservationVerdict::CanonicalUnparsable,
            vec![
                "canonical statement did not parse into a `theorem`/`lemma` signature \
                 (fail-closed: cannot confirm preservation)"
                    .to_string(),
            ],
            None,
            None,
        );
    };

    let submitted = parse_all_decls(submitted_code);

    // Prefer the declaration that shares the canonical NAME (the model is supposed
    // to prove *that* theorem). Fall back to a same-signature-different-name
    // declaration to distinguish a rename from an outright miss.
    if let Some(by_name) = submitted.iter().find(|d| d.name == canonical.name) {
        let (verdict, findings) = classify(&canonical, by_name);
        return report(verdict, findings, Some(canonical), Some(by_name.clone()));
    }

    // No same-name declaration. Is the canonical STATEMENT proved under another
    // name (a rename cheat), or absent entirely?
    if let Some(renamed) = submitted
        .iter()
        .find(|d| sig_matches(&canonical, d).is_some())
    {
        return report(
            PreservationVerdict::Renamed,
            vec![format!(
                "renamed statement: the canonical theorem `{}` is proved under the \
                 different name `{}` — the canonical name is never established",
                canonical.name, renamed.name
            )],
            Some(canonical),
            Some(renamed.clone()),
        );
    }

    report(
        PreservationVerdict::SubmittedMissing,
        vec![format!(
            "missing statement: no declaration in the submission proves the canonical \
             theorem `{}` (name + signature not found)",
            canonical.name
        )],
        Some(canonical),
        None,
    )
}

/// Classify a same-name canonical/submitted pair into a verdict + findings.
fn classify(canonical: &TheoremSig, submitted: &TheoremSig) -> (PreservationVerdict, Vec<String>) {
    // Exact / alpha match: gate stays open.
    if let Some(alpha) = sig_matches(canonical, submitted) {
        let verdict = if alpha {
            PreservationVerdict::PreservedAlpha
        } else {
            PreservationVerdict::Preserved
        };
        return (verdict, Vec::new());
    }

    // Trivial restatement takes priority — the clearest cheat.
    if is_trivial_conclusion(&submitted.conclusion) && !is_trivial_conclusion(&canonical.conclusion) {
        return (
            PreservationVerdict::TriviallyRestated,
            vec![format!(
                "trivially-restated statement: theorem `{}` now concludes `{}` instead of \
                 the canonical goal `{}` — the original goal is not proved",
                canonical.name,
                norm_ws(&submitted.conclusion),
                norm_ws(&canonical.conclusion)
            )],
        );
    }

    // Binder region changed (dropped/added hypotheses => possible weakening).
    if norm_ws(&canonical.binders) != norm_ws(&submitted.binders) {
        return (
            PreservationVerdict::BindersChanged,
            vec![format!(
                "weakened/altered binders: theorem `{}` binders changed from `{}` to `{}` \
                 (possible weakening — a hypothesis may have been dropped)",
                canonical.name,
                norm_ws(&canonical.binders),
                norm_ws(&submitted.binders)
            )],
        );
    }

    // Otherwise the conclusion drifted.
    (
        PreservationVerdict::ConclusionChanged,
        vec![format!(
            "altered conclusion: theorem `{}` goal changed from `{}` to `{}` \
             (possible weakening/restatement)",
            canonical.name,
            norm_ws(&canonical.conclusion),
            norm_ws(&submitted.conclusion)
        )],
    )
}

/// Whether `canonical` and `submitted` share a signature. `Some(false)` on an
/// exact (whitespace) match, `Some(true)` on an alpha-only match, `None` when they
/// differ. Alpha-matching only rescues a pure bound-variable rename: it requires
/// the same binder COUNT (a different count is a structural change we must flag).
fn sig_matches(canonical: &TheoremSig, submitted: &TheoremSig) -> Option<bool> {
    let cb = norm_ws(&canonical.binders);
    let sb = norm_ws(&submitted.binders);
    let cc = norm_ws(&canonical.conclusion);
    let sc = norm_ws(&submitted.conclusion);
    if cb == sb && cc == sc {
        return Some(false);
    }

    // Best-effort alpha: rename each side's bound variables to positional tokens
    // and compare. Only attempted when the binder counts agree, so a dropped or
    // added hypothesis is never masked as an alpha-rename.
    let cvars = binder_vars(&canonical.binders);
    let svars = binder_vars(&submitted.binders);
    if !cvars.is_empty() && cvars.len() == svars.len() {
        let c_alpha = alpha_canonicalize(&cb, &cc, &cvars);
        let s_alpha = alpha_canonicalize(&sb, &sc, &svars);
        if c_alpha == s_alpha {
            return Some(true);
        }
    }
    None
}

fn report(
    verdict: PreservationVerdict,
    findings: Vec<String>,
    canonical: Option<TheoremSig>,
    submitted: Option<TheoremSig>,
) -> PreservationReport {
    let detail = json!({
        "verdict": verdict.tag(),
        "canonical": &canonical,
        "submitted": &submitted,
    });
    PreservationReport {
        preserved: verdict.is_preserved(),
        verdict,
        findings,
        canonical,
        submitted,
        detail,
    }
}

// ===========================================================================
// Per-system entry-signature preservation (Agda / Metamath)
// ===========================================================================

/// Per-system statement-signature preservation check.
///
/// For Lean / Rocq / Isabelle / Candle this delegates verbatim to
/// [`check_statement_preserved`] (the theorem-signature parser), so their gate
/// behavior is unchanged. For **Agda** and **Metamath** — whose declarations do
/// NOT use the `theorem` / `lemma` keyword the Lean-oriented parser looks for, so
/// they previously fell through to the weak lexical
/// [`statement_mentioned`](crate::prover::formal) substring fallback where a proof
/// of a DIFFERENT theorem could pass merely because the statement text appears in
/// the source — it applies a system-specific signature parse:
///
/// * **Agda**: a declaration is `name : Type`; the statement IS the type. The
///   canonical type of the entry is compared against the type the submission
///   declares for the same entry, up to whitespace. A submission that declares a
///   DIFFERENT type is flagged [`ConclusionChanged`](PreservationVerdict::ConclusionChanged).
/// * **Metamath**: a theorem is `label $p <typecode> <symbols> $= <proof> $.`; the
///   asserted statement is the symbol sequence between `$p` and `$=`. The
///   canonical symbol sequence for `label` is compared against what the
///   submission's `$p` asserts. A `$p` asserting a DIFFERENT statement is flagged
///   [`ConclusionChanged`](PreservationVerdict::ConclusionChanged).
///
/// Conservative by construction: when the canonical statement or the submitted
/// entry cannot be parsed for the given system, the verdict is a *fallback* one
/// ([`CanonicalUnparsable`](PreservationVerdict::CanonicalUnparsable) /
/// [`SubmittedMissing`](PreservationVerdict::SubmittedMissing)) that is NOT in the
/// gate's flagged set, so `verify()` falls back to the lexical mention check
/// rather than rejecting a legitimate proof. Only a POSITIVELY-detected different
/// signature yields a flagged verdict — mirroring the Lean wiring already in
/// `verify()`.
pub fn check_entry_signature(
    system: FormalSystem,
    canonical_statement: &str,
    submitted_code: &str,
) -> PreservationReport {
    match system {
        FormalSystem::Agda => check_agda_signature(canonical_statement, submitted_code),
        FormalSystem::Metamath => check_metamath_signature(canonical_statement, submitted_code),
        // Lean / Rocq / Isabelle / Candle: unchanged theorem-signature path.
        FormalSystem::Lean
        | FormalSystem::Rocq
        | FormalSystem::Isabelle
        | FormalSystem::Candle => check_statement_preserved(canonical_statement, submitted_code),
    }
}

/// A minimal `name : conclusion` signature for the non-Lean systems (no binder
/// region — Agda folds binders into the type; Metamath has none).
fn entry_sig(kind: &str, name: &str, conclusion: &str) -> TheoremSig {
    TheoremSig {
        kind: kind.to_string(),
        name: name.to_string(),
        binders: String::new(),
        conclusion: norm_ws(conclusion),
    }
}

// --- Agda ------------------------------------------------------------------

/// Confirm the submission declares the canonical Agda entry with the SAME type.
fn check_agda_signature(canonical_statement: &str, submitted_code: &str) -> PreservationReport {
    let Some((name, canon_type)) = parse_agda_decl(canonical_statement) else {
        return report(
            PreservationVerdict::CanonicalUnparsable,
            vec![
                "Agda canonical statement did not parse into a `name : Type` signature \
                 (falling back to the lexical mention check)"
                    .to_string(),
            ],
            None,
            None,
        );
    };
    let stripped: Vec<char> = crate::prover::formal::strip_comments(submitted_code)
        .chars()
        .collect();
    let Some(sub_type) = find_agda_type(&stripped, &name) else {
        return report(
            PreservationVerdict::SubmittedMissing,
            vec![format!(
                "Agda entry `{name}` has no `{name} : …` type signature in the submission \
                 (falling back to the lexical mention check)"
            )],
            Some(entry_sig("agda", &name, &canon_type)),
            None,
        );
    };
    if norm_ws(&canon_type) == norm_ws(&sub_type) {
        return report(
            PreservationVerdict::Preserved,
            Vec::new(),
            Some(entry_sig("agda", &name, &canon_type)),
            Some(entry_sig("agda", &name, &sub_type)),
        );
    }
    report(
        PreservationVerdict::ConclusionChanged,
        vec![format!(
            "altered Agda type: entry `{name}` is declared with type `{}` but the canonical \
             statement's type is `{}` — a proof of a DIFFERENT proposition",
            norm_ws(&sub_type),
            norm_ws(&canon_type)
        )],
        Some(entry_sig("agda", &name, &canon_type)),
        Some(entry_sig("agda", &name, &sub_type)),
    )
}

/// Parse a canonical Agda statement `name : Type` into `(name, type)`. Comments
/// are stripped first. `None` when there is no top-level `:` ascription.
fn parse_agda_decl(statement: &str) -> Option<(String, String)> {
    let stripped = crate::prover::formal::strip_comments(statement);
    let chars: Vec<char> = stripped.chars().collect();
    let colon = agda_top_level_colon(&chars)?;
    let name_region: String = chars[..colon].iter().collect();
    let name = name_region.split_whitespace().next()?.to_string();
    let ty = agda_type_region(&chars[colon + 1..]);
    if name.is_empty() || ty.is_empty() {
        return None;
    }
    Some((name, ty))
}

/// Index of the first depth-0 `:` ascription colon (not `:=`), tracking bracket
/// depth so a binder-local `{x : T}` colon is skipped.
fn agda_top_level_colon(chars: &[char]) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < chars.len() {
        match chars[i] {
            '(' | '[' | '{' | '⟨' | '⦃' => depth += 1,
            ')' | ']' | '}' | '⟩' | '⦄' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            ':' if depth == 0 && chars.get(i + 1) != Some(&'=') => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// Extract an Agda type region: from the start of `region` up to the end of the
/// type signature — the first depth-0 de-dented line break (which begins the
/// equation `name = …` or a sibling declaration) or a depth-0 standalone `=`.
/// Indented continuation lines are folded in. Whitespace-normalized.
fn agda_type_region(region: &[char]) -> String {
    let mut depth = 0i32;
    let mut out = String::new();
    let mut i = 0usize;
    while i < region.len() {
        let c = region[i];
        match c {
            '(' | '[' | '{' | '⟨' | '⦃' => depth += 1,
            ')' | ']' | '}' | '⟩' | '⦄' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            '\n' if depth == 0 => {
                // A continuation line must be indented; a de-dented token or a
                // blank line ends the signature.
                match region.get(i + 1) {
                    Some(' ') | Some('\t') => {
                        out.push(' ');
                        i += 1;
                        continue;
                    }
                    _ => break,
                }
            }
            '=' if depth == 0 => {
                let prev = if i > 0 { Some(region[i - 1]) } else { None };
                let next = region.get(i + 1).copied();
                // Skip `==` / `=>` / `<=` / `>=` / `:=` / `!=` operators; a lone
                // `=` at depth 0 is the equation delimiter and ends the type.
                let part_of_op = matches!(prev, Some('=') | Some('<') | Some('>') | Some(':') | Some('!'))
                    || matches!(next, Some('=') | Some('>'));
                if !part_of_op {
                    break;
                }
            }
            _ => {}
        }
        out.push(c);
        i += 1;
    }
    norm_ws(&out)
}

/// Find the type the submission declares for Agda entry `name`: the first
/// whole-token occurrence of `name` immediately followed (spaces/tabs only) by a
/// `:` ascription. Returns the whitespace-normalized type, or `None`.
fn find_agda_type(stripped: &[char], name: &str) -> Option<String> {
    let n: Vec<char> = name.chars().collect();
    if n.is_empty() || stripped.len() < n.len() {
        return None;
    }
    let mut i = 0usize;
    while i + n.len() <= stripped.len() {
        if stripped[i..i + n.len()] == n[..] {
            let before_ok = i == 0 || !is_word(stripped[i - 1]);
            let after = i + n.len();
            let after_ok = stripped.get(after).map_or(true, |&c| !is_word(c));
            if before_ok && after_ok {
                let mut j = after;
                while j < stripped.len() && (stripped[j] == ' ' || stripped[j] == '\t') {
                    j += 1;
                }
                if stripped.get(j) == Some(&':') && stripped.get(j + 1) != Some(&'=') {
                    let ty = agda_type_region(&stripped[j + 1..]);
                    if !ty.is_empty() {
                        return Some(ty);
                    }
                }
            }
        }
        i += 1;
    }
    None
}

// --- Metamath --------------------------------------------------------------

/// Confirm the submission's `$p` for the canonical label asserts the SAME symbol
/// sequence as the canonical statement.
fn check_metamath_signature(canonical_statement: &str, submitted_code: &str) -> PreservationReport {
    let Some((label, canon_syms)) = parse_metamath_assertion(canonical_statement) else {
        return report(
            PreservationVerdict::CanonicalUnparsable,
            vec![
                "Metamath canonical statement did not parse into a `label $p/$a … ` assertion \
                 (falling back to the lexical mention check)"
                    .to_string(),
            ],
            None,
            None,
        );
    };
    let Some(sub_syms) = find_metamath_assertion(submitted_code, &label) else {
        return report(
            PreservationVerdict::SubmittedMissing,
            vec![format!(
                "Metamath label `{label}` is not asserted by a `$p` in the submission \
                 (falling back to the lexical mention check)"
            )],
            Some(entry_sig("metamath", &label, &canon_syms.join(" "))),
            None,
        );
    };
    if canon_syms == sub_syms {
        return report(
            PreservationVerdict::Preserved,
            Vec::new(),
            Some(entry_sig("metamath", &label, &canon_syms.join(" "))),
            Some(entry_sig("metamath", &label, &sub_syms.join(" "))),
        );
    }
    report(
        PreservationVerdict::ConclusionChanged,
        vec![format!(
            "altered Metamath assertion: `$p {label}` asserts `{}` but the canonical statement \
             asserts `{}` — a proof of a DIFFERENT statement",
            sub_syms.join(" "),
            canon_syms.join(" ")
        )],
        Some(entry_sig("metamath", &label, &canon_syms.join(" "))),
        Some(entry_sig("metamath", &label, &sub_syms.join(" "))),
    )
}

/// Parse a canonical Metamath assertion `label $p/$a <symbols> ($= | $.)` into
/// `(label, symbols)`. Comments are stripped first. `None` when no labelled
/// `$p`/`$a` with symbols is present.
fn parse_metamath_assertion(statement: &str) -> Option<(String, Vec<String>)> {
    let stripped = crate::prover::formal::strip_comments(statement);
    let toks: Vec<&str> = stripped.split_whitespace().collect();
    for k in 1..toks.len() {
        if toks[k] == "$p" || toks[k] == "$a" {
            let label = toks[k - 1];
            // The preceding token must be a real label, not another keyword.
            if label.starts_with('$') {
                continue;
            }
            let syms = metamath_symbols(&toks[k + 1..]);
            if !label.is_empty() && !syms.is_empty() {
                return Some((label.to_string(), syms));
            }
        }
    }
    None
}

/// Find the symbol sequence the submission's `$p` for `label` asserts (between
/// `$p` and `$=`/`$.`). Comments are stripped first. `None` when absent.
fn find_metamath_assertion(submitted_code: &str, label: &str) -> Option<Vec<String>> {
    let stripped = crate::prover::formal::strip_comments(submitted_code);
    let toks: Vec<&str> = stripped.split_whitespace().collect();
    for k in 1..toks.len() {
        if toks[k] == "$p" && toks[k - 1] == label {
            let syms = metamath_symbols(&toks[k + 1..]);
            if !syms.is_empty() {
                return Some(syms);
            }
        }
    }
    None
}

/// Collect assertion symbols from the tokens after `$p`/`$a`, stopping at the
/// proof separator `$=` or the statement terminator `$.`.
fn metamath_symbols(rest: &[&str]) -> Vec<String> {
    let mut syms = Vec::new();
    for &t in rest {
        if t == "$=" || t == "$." {
            break;
        }
        syms.push(t.to_string());
    }
    syms
}

// ===========================================================================
// Escape-hatch scan
// ===========================================================================

/// One escape-hatch finding: which construct, where (1-based line), and why. All
/// escape hatches here are CRITICAL (gate-failing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EscapeHatch {
    /// Stable rule id (`sorry` / `admit` / `native_decide` / `apply?` / …).
    pub rule: &'static str,
    /// 1-based source line.
    pub line: usize,
    /// Human-readable explanation.
    pub detail: String,
}

/// The escape-hatch tokens flagged, in fixed order. Each entry is
/// `(literal, rule-id, reason)`.
///
/// * `sorry` / `admit` — an OPEN goal admitted with no proof.
/// * `native_decide` — closes a goal by the *compiled* (native-code) evaluator, a
///   trust hole the kernel does not re-check (a config-level escape hatch).
/// * `apply?` / `exact?` / `rfl?` — interactive SUGGESTION tactics. A "proof" that
///   relies on them is non-reproducible (it exercised an editor/UI code path, not
///   the kernel) — the DeepSeek-Prover-V2 reward-hacking erratum.
const ESCAPE_HATCHES: &[(&str, &str, &str)] = &[
    ("sorry", "sorry", "open goal admitted with `sorry` (no proof)"),
    ("admit", "admit", "open goal admitted with `admit` (no proof)"),
    (
        "native_decide",
        "native_decide",
        "goal closed by the compiled `native_decide` evaluator — a trust hole the \
         kernel does not re-check (config-level escape hatch)",
    ),
    (
        "apply?",
        "apply?",
        "interactive suggestion tactic `apply?` — non-reproducible proof \
         (editor/UI code path, not the kernel; DeepSeek-Prover-V2 erratum)",
    ),
    (
        "exact?",
        "exact?",
        "interactive suggestion tactic `exact?` — non-reproducible proof \
         (editor/UI code path, not the kernel; DeepSeek-Prover-V2 erratum)",
    ),
    (
        "rfl?",
        "rfl?",
        "interactive suggestion tactic `rfl?` — non-reproducible proof \
         (editor/UI code path, not the kernel; DeepSeek-Prover-V2 erratum)",
    ),
];

/// Flag proof-search escape hatches in Lean `code`: `sorry`, `admit`,
/// `native_decide`, and the `apply?` / `exact?` / `rfl?` suggestion tactics.
///
/// Runs over comment/string-stripped source (so `-- sorry` in a comment or a
/// `"sorry"` string literal never false-flags) while preserving line numbers.
/// Deterministic: findings are ordered by `(line, rule)`. Every finding here is
/// CRITICAL — the presence of any makes the gate fail-closed
/// ([`escape_hatches_clean`] is then `false`).
pub fn scan_escape_hatches(code: &str) -> Vec<EscapeHatch> {
    let sanitized: Vec<char> = sanitize_lean(code).chars().collect();
    let chars = &sanitized[..];

    let mut out: Vec<EscapeHatch> = Vec::new();
    for (literal, rule, reason) in ESCAPE_HATCHES {
        for pos in token_positions(chars, literal) {
            out.push(EscapeHatch {
                rule,
                line: line_of(chars, pos),
                detail: (*reason).to_string(),
            });
        }
    }
    out.sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.rule.cmp(b.rule)));
    out
}

/// Whether `code` is free of escape hatches (no CRITICAL findings).
pub fn escape_hatches_clean(code: &str) -> bool {
    scan_escape_hatches(code).is_empty()
}

/// Layer-2c view of the escape-hatch scan: a [`ScanReport`] the backend
/// `source_scan` can fold in. `clean` is `true` iff no escape hatch was found.
pub fn escape_hatch_scan_report(code: &str) -> ScanReport {
    let hatches = scan_escape_hatches(code);
    let findings = hatches
        .iter()
        .map(|h| format!("CRITICAL: {} (line {}): {}", h.rule, h.line, h.detail))
        .collect::<Vec<_>>();
    ScanReport {
        clean: hatches.is_empty(),
        findings,
        detail: json!({
            "check": "escape_hatches",
            "hatches": hatches,
        }),
    }
}

// ===========================================================================
// Error-span formatting (repair loop)
// ===========================================================================

/// A localized Lean error for [`format_error_spans`]: a 1-based source `line` and
/// the compiler `message`. (Lean's own diagnostics are 1-based, e.g.
/// `Generated.lean:2:0: error: …`.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeanError {
    /// 1-based line the error localizes to.
    pub line: usize,
    /// The compiler / verifier message.
    pub message: String,
}

impl LeanError {
    pub fn new(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }
}

/// Render Lean `errors` as a repair message: for each error, the offending line
/// marked with `>>>` plus ±2 lines of context (Kimina infotree style), for the
/// repair loop to hand to the model.
///
/// Deterministic: errors are rendered in `(line, message)` order, one block each.
/// An error whose line is out of range still prints its message (with no context
/// window). Returns an empty string when `errors` is empty.
pub fn format_error_spans(code: &str, errors: &[LeanError]) -> String {
    if errors.is_empty() {
        return String::new();
    }
    let lines: Vec<&str> = code.lines().collect();
    let total = lines.len();

    let mut sorted: Vec<&LeanError> = errors.iter().collect();
    sorted.sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.message.cmp(&b.message)));

    // Width of the largest line number in any rendered window, for aligned gutters.
    let max_no = sorted
        .iter()
        .map(|e| (e.line + 2).min(total.max(1)))
        .max()
        .unwrap_or(1);
    let width = max_no.to_string().len();

    let mut blocks: Vec<String> = Vec::new();
    for err in sorted {
        let mut block = String::new();
        block.push_str(&format!("error at line {}: {}\n", err.line, err.message));

        if err.line == 0 || err.line > total {
            block.push_str("    (line out of range; no source context)\n");
            blocks.push(block);
            continue;
        }
        // 1-based inclusive window [line-2, line+2], clamped to the source.
        let lo = err.line.saturating_sub(2).max(1);
        let hi = (err.line + 2).min(total);
        for n in lo..=hi {
            let marker = if n == err.line { ">>>" } else { "   " };
            block.push_str(&format!(
                "{marker} {:>width$} | {}\n",
                n,
                lines[n - 1],
                width = width
            ));
        }
        blocks.push(block);
    }
    blocks.join("\n")
}

// ===========================================================================
// Lexical helpers
// ===========================================================================

/// A Lean identifier / word char for token-boundary tests (NOT including `.`, so
/// a namespaced access like `Foo.sorry` still boundary-matches `sorry`).
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '\''
}

/// A name char, including `.` for namespaced identifiers (`Nat.add_comm`).
fn is_name(c: char) -> bool {
    is_word(c) || c == '.'
}

/// Replace Lean `--` line comments, nested `/- … -/` block comments, and `"…"`
/// string literals with spaces, preserving newlines and the char count so line
/// numbers and offsets stay aligned. Leaves everything else intact.
fn sanitize_lean(code: &str) -> String {
    let chars: Vec<char> = code.chars().collect();
    let mut out = String::with_capacity(chars.len());
    let mut i = 0usize;
    let mut block_depth = 0usize;
    let mut in_line = false;
    let mut in_string = false;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if in_line {
            out.push(if c == '\n' { '\n' } else { ' ' });
            if c == '\n' {
                in_line = false;
            }
            i += 1;
            continue;
        }
        if block_depth > 0 {
            if c == '/' && next == Some('-') {
                block_depth += 1;
                out.push(' ');
                out.push(' ');
                i += 2;
                continue;
            }
            if c == '-' && next == Some('/') {
                block_depth -= 1;
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
        if c == '-' && next == Some('-') {
            in_line = true;
            out.push(' ');
            out.push(' ');
            i += 2;
            continue;
        }
        if c == '/' && next == Some('-') {
            block_depth = 1;
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

/// Whole-token occurrences of `needle` in `chars`: the char before the match is
/// not a word char, and the char after the LAST word char of the needle is not a
/// word char. A needle ending in a non-word char (e.g. `apply?`) only needs its
/// leading boundary checked, so `apply?` matches but `applying` does not match
/// `apply`.
fn token_positions(chars: &[char], needle: &str) -> Vec<usize> {
    let n: Vec<char> = needle.chars().collect();
    let mut out = Vec::new();
    if n.is_empty() || chars.len() < n.len() {
        return out;
    }
    let last_is_word = n.last().map_or(false, |&c| is_word(c));
    let mut i = 0usize;
    while i + n.len() <= chars.len() {
        if chars[i..i + n.len()] == n[..] {
            let before_ok = i == 0 || !is_word(chars[i - 1]);
            // Trailing boundary only matters when the needle ends in a word char.
            let after_ok = !last_is_word
                || chars.get(i + n.len()).map_or(true, |&c| !is_word(c));
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

/// Collapse all runs of whitespace to a single space and trim.
fn norm_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether a conclusion is a trivial proposition a restatement cheat resolves to.
fn is_trivial_conclusion(conclusion: &str) -> bool {
    matches!(norm_ws(conclusion).as_str(), "True" | "true" | "trivial" | "⊤")
}

// --- declaration parsing --------------------------------------------------

/// Parse the FIRST `theorem`/`lemma`/`example`/`def` declaration in `src` into a
/// signature. `None` when none is found.
fn parse_first_decl(src: &str) -> Option<TheoremSig> {
    parse_all_decls(src).into_iter().next()
}

/// Parse EVERY top-level `theorem`/`lemma`/`example`/`def` declaration in `src`,
/// in source order. Operates over comment/string-stripped source so a keyword in
/// a comment or string never starts a phantom declaration.
fn parse_all_decls(src: &str) -> Vec<TheoremSig> {
    let sanitized: Vec<char> = sanitize_lean(src).chars().collect();
    let chars = &sanitized[..];
    let mut out = Vec::new();
    // Keyword longest-first so `theorem` is tried before nothing shadows it.
    let keywords = ["theorem", "lemma", "example", "def"];
    let mut i = 0usize;
    while i < chars.len() {
        // A declaration keyword must sit on a word boundary.
        let boundary = i == 0 || !is_word(chars[i - 1]);
        if boundary {
            let mut matched: Option<&str> = None;
            for kw in keywords {
                let k: Vec<char> = kw.chars().collect();
                if i + k.len() <= chars.len()
                    && chars[i..i + k.len()] == k[..]
                    && chars.get(i + k.len()).map_or(true, |&c| !is_word(c))
                {
                    matched = Some(kw);
                    break;
                }
            }
            if let Some(kw) = matched {
                let after_kw = i + kw.chars().count();
                if let Some((sig, consumed)) = parse_decl_at(chars, kw, after_kw) {
                    out.push(sig);
                    i = consumed;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// Parse a single declaration whose keyword `kw` ended at `after_kw`. Returns the
/// signature and the index just past the parsed signature (at `:=` / new decl /
/// EOF), or `None` if no name follows.
fn parse_decl_at(chars: &[char], kw: &str, after_kw: usize) -> Option<(TheoremSig, usize)> {
    let mut j = after_kw;
    while j < chars.len() && chars[j].is_whitespace() {
        j += 1;
    }
    // Name (namespaced identifier). `example` may have none.
    let name_start = j;
    while j < chars.len() && is_name(chars[j]) {
        j += 1;
    }
    let name: String = chars[name_start..j].iter().collect();
    let name = if name.is_empty() {
        if kw == "example" {
            "<example>".to_string()
        } else {
            return None;
        }
    } else {
        name
    };

    // Signature region: from here up to the top-level `:=`, the next top-level
    // declaration keyword, or EOF.
    let sig_start = j;
    let sig_end = signature_end(chars, sig_start);
    let sig: &[char] = &chars[sig_start..sig_end];

    // Split binders vs conclusion at the first depth-0 `:` (the statement colon).
    let (binders, conclusion) = split_binders_conclusion(sig);

    Some((
        TheoremSig {
            kind: kw.to_string(),
            name,
            binders: norm_ws(&binders.iter().collect::<String>()),
            conclusion: norm_ws(&conclusion.iter().collect::<String>()),
        },
        sig_end,
    ))
}

/// Index where a declaration's signature ends: the first top-level (bracket depth
/// 0) `:=`, else the first top-level declaration keyword after the start, else the
/// end of input.
fn signature_end(chars: &[char], start: usize) -> usize {
    let keywords = ["theorem", "lemma", "example", "def"];
    let mut depth = 0i32;
    let mut i = start;
    while i < chars.len() {
        match chars[i] {
            '(' | '[' | '{' | '⟨' | '⦃' => depth += 1,
            ')' | ']' | '}' | '⟩' | '⦄' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            ':' if depth == 0 && chars.get(i + 1) == Some(&'=') => return i,
            _ => {
                if depth == 0 && (i == 0 || !is_word(chars[i - 1])) {
                    for kw in keywords {
                        let k: Vec<char> = kw.chars().collect();
                        if i + k.len() <= chars.len()
                            && chars[i..i + k.len()] == k[..]
                            && chars.get(i + k.len()).map_or(true, |&c| !is_word(c))
                        {
                            return i;
                        }
                    }
                }
            }
        }
        i += 1;
    }
    chars.len()
}

/// Split a signature region into `(binders, conclusion)` at the first depth-0 `:`
/// (the statement colon; binder-local `:` sit inside brackets at depth > 0). When
/// there is no depth-0 `:` (e.g. a `def` with no ascription), the whole region is
/// the binders and the conclusion is empty.
fn split_binders_conclusion(sig: &[char]) -> (&[char], &[char]) {
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < sig.len() {
        match sig[i] {
            '(' | '[' | '{' | '⟨' | '⦃' => depth += 1,
            ')' | ']' | '}' | '⟩' | '⦄' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            ':' if depth == 0 => {
                // Guard against a stray `:=` (shouldn't reach here — cut earlier).
                if sig.get(i + 1) != Some(&'=') {
                    return (&sig[..i], &sig[i + 1..]);
                }
            }
            _ => {}
        }
        i += 1;
    }
    (sig, &[])
}

// --- alpha canonicalization -----------------------------------------------

/// The bound-variable names introduced by a binder region, in order (each
/// top-level group's identifiers before its local `:`). Best-effort and lexical.
fn binder_vars(binders: &str) -> Vec<String> {
    let chars: Vec<char> = binders.chars().collect();
    let mut vars: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let open = chars[i];
        let close = match open {
            '(' => Some(')'),
            '{' => Some('}'),
            '[' => Some(']'),
            '⟨' => Some('⟩'),
            '⦃' => Some('⦄'),
            _ => None,
        };
        if let Some(close) = close {
            // Find the matching close at the same depth.
            let mut depth = 1i32;
            let mut k = i + 1;
            while k < chars.len() && depth > 0 {
                if chars[k] == open {
                    depth += 1;
                } else if chars[k] == close {
                    depth -= 1;
                }
                if depth == 0 {
                    break;
                }
                k += 1;
            }
            let inner = &chars[i + 1..k.min(chars.len())];
            // Names are the identifiers before the group's local `:`.
            let name_region: Vec<char> = {
                let mut d = 0i32;
                let mut cut = inner.len();
                for (idx, &c) in inner.iter().enumerate() {
                    match c {
                        '(' | '[' | '{' | '⟨' | '⦃' => d += 1,
                        ')' | ']' | '}' | '⟩' | '⦄' => {
                            if d > 0 {
                                d -= 1;
                            }
                        }
                        ':' if d == 0 => {
                            cut = idx;
                            break;
                        }
                        _ => {}
                    }
                }
                inner[..cut].to_vec()
            };
            for name in split_idents(&name_region) {
                if !vars.contains(&name) {
                    vars.push(name);
                }
            }
            i = k + 1;
            continue;
        }
        i += 1;
    }
    vars
}

/// The whitespace-separated identifiers in a slice (binder names like `n m`).
fn split_idents(chars: &[char]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for &c in chars {
        if is_word(c) {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    // `_` is an anonymous binder, not an alpha-renamable name.
    out.into_iter().filter(|s| s != "_").collect()
}

/// Rename each bound variable in `vars` to a positional token (`#0`, `#1`, …)
/// throughout `binders : conclusion`, yielding an alpha-canonical form. Whole-word
/// replacement only (so `n` does not rewrite inside `natural`).
fn alpha_canonicalize(binders: &str, conclusion: &str, vars: &[String]) -> String {
    let combined = format!("{binders} ⊢ {conclusion}");
    let mut chars: Vec<char> = combined.chars().collect();
    // Map each var to its positional index.
    for (idx, var) in vars.iter().enumerate() {
        let token: Vec<char> = format!("#{idx}").chars().collect();
        chars = replace_word(&chars, var, &token);
    }
    norm_ws(&chars.into_iter().collect::<String>())
}

/// Replace every whole-word occurrence of `needle` in `chars` with `repl`.
fn replace_word(chars: &[char], needle: &str, repl: &[char]) -> Vec<char> {
    let n: Vec<char> = needle.chars().collect();
    if n.is_empty() {
        return chars.to_vec();
    }
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut i = 0usize;
    while i < chars.len() {
        if i + n.len() <= chars.len() && chars[i..i + n.len()] == n[..] {
            let before_ok = i == 0 || !is_word(chars[i - 1]);
            let after_ok = chars.get(i + n.len()).map_or(true, |&c| !is_word(c));
            if before_ok && after_ok {
                out.extend_from_slice(repl);
                i += n.len();
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- statement preservation ---------------------------------------------

    /// A proof on the canonical statement passes (exact match).
    #[test]
    fn canonical_statement_is_preserved() {
        let canonical = "theorem add_comm (a b : Nat) : a + b = b + a";
        let submitted = "theorem add_comm (a b : Nat) : a + b = b + a := by ring";
        let r = check_statement_preserved(canonical, submitted);
        assert!(r.preserved, "exact match must pass: {:?}", r.findings);
        assert_eq!(r.verdict, PreservationVerdict::Preserved);
        assert!(r.findings.is_empty());
        assert!(r.into_scan_report().clean);
    }

    /// Whitespace / newline differences do not defeat preservation.
    #[test]
    fn whitespace_differences_still_preserved() {
        let canonical = "theorem T (n : Nat) : n = n";
        let submitted = "theorem T   (n : Nat)\n    :  n = n := rfl";
        let r = check_statement_preserved(canonical, submitted);
        assert!(r.preserved, "{:?}", r);
        assert_eq!(r.verdict, PreservationVerdict::Preserved);
    }

    /// A pure bound-variable rename is accepted up to alpha.
    #[test]
    fn alpha_renamed_binders_are_preserved() {
        let canonical = "theorem T (n : Nat) : n = n";
        let submitted = "theorem T (m : Nat) : m = m := rfl";
        let r = check_statement_preserved(canonical, submitted);
        assert!(r.preserved, "alpha rename must pass: {:?}", r.findings);
        assert_eq!(r.verdict, PreservationVerdict::PreservedAlpha);
    }

    /// A weakened statement (dropped hypothesis) is flagged, fail-closed.
    #[test]
    fn weakened_statement_is_flagged() {
        let canonical = "theorem T (n : Nat) (h : n > 0) : n ≥ 1";
        let submitted = "theorem T (n : Nat) : n ≥ 1 := by omega";
        let r = check_statement_preserved(canonical, submitted);
        assert!(!r.preserved);
        assert_eq!(r.verdict, PreservationVerdict::BindersChanged);
        assert!(r.findings.iter().any(|f| f.contains("weakening")));
        assert!(!r.into_scan_report().clean);
    }

    /// A renamed statement (same signature, different name) is flagged.
    #[test]
    fn renamed_statement_is_flagged() {
        let canonical = "theorem add_comm (a b : Nat) : a + b = b + a";
        let submitted = "theorem my_lemma (a b : Nat) : a + b = b + a := by ring";
        let r = check_statement_preserved(canonical, submitted);
        assert!(!r.preserved);
        assert_eq!(r.verdict, PreservationVerdict::Renamed);
        assert!(r.findings.iter().any(|f| f.contains("renamed")));
    }

    /// A trivially-restated conclusion (`True`) is flagged.
    #[test]
    fn trivially_restated_statement_is_flagged() {
        let canonical = "theorem T (n : Nat) : n = n + 0";
        let submitted = "theorem T (n : Nat) : True := trivial";
        let r = check_statement_preserved(canonical, submitted);
        assert!(!r.preserved);
        assert_eq!(r.verdict, PreservationVerdict::TriviallyRestated);
    }

    /// An altered conclusion (same binders) is flagged as a restatement.
    #[test]
    fn altered_conclusion_is_flagged() {
        let canonical = "theorem T (n : Nat) : n = n + 0";
        let submitted = "theorem T (n : Nat) : n = n := rfl";
        let r = check_statement_preserved(canonical, submitted);
        assert!(!r.preserved);
        assert_eq!(r.verdict, PreservationVerdict::ConclusionChanged);
    }

    /// A submission that does not declare the canonical theorem at all fails
    /// closed as missing.
    #[test]
    fn missing_statement_fails_closed() {
        let canonical = "theorem T (n : Nat) : n = n";
        let submitted = "def helper : True := trivial";
        let r = check_statement_preserved(canonical, submitted);
        assert!(!r.preserved);
        assert_eq!(r.verdict, PreservationVerdict::SubmittedMissing);
    }

    /// An unparsable canonical statement fails closed (cannot confirm anything).
    #[test]
    fn unparsable_canonical_fails_closed() {
        let r = check_statement_preserved("-- just a comment, no theorem", "theorem T : True := trivial");
        assert!(!r.preserved);
        assert_eq!(r.verdict, PreservationVerdict::CanonicalUnparsable);
    }

    /// The canonical theorem may appear alongside helper lemmas in the submission.
    #[test]
    fn matches_the_named_declaration_among_several() {
        let canonical = "theorem main (n : Nat) : n + 0 = n";
        let submitted = "\
lemma helper (a : Nat) : a = a := rfl
theorem main (n : Nat) : n + 0 = n := by simp
def aux : Nat := 0
";
        let r = check_statement_preserved(canonical, submitted);
        assert!(r.preserved, "{:?}", r);
    }

    // -- per-system entry signature (Agda / Metamath) -----------------------

    /// `check_entry_signature` delegates the Lean/Rocq/Isabelle path to
    /// `check_statement_preserved` unchanged (no regression).
    #[test]
    fn entry_signature_delegates_for_lean_family() {
        let canonical = "theorem T (n : Nat) (h : n > 0) : n ≥ 1";
        let submitted = "theorem T (n : Nat) : n ≥ 1 := by omega";
        for sys in [
            FormalSystem::Lean,
            FormalSystem::Rocq,
            FormalSystem::Isabelle,
            FormalSystem::Candle,
        ] {
            let a = check_entry_signature(sys, canonical, submitted);
            let b = check_statement_preserved(canonical, submitted);
            assert_eq!(a.verdict, b.verdict, "{sys:?} must delegate unchanged");
        }
    }

    /// Agda: a proof declaring the requested `foo : A` passes.
    #[test]
    fn agda_same_type_is_preserved() {
        let canonical = "foo : A";
        let submitted = "foo : A\nfoo = a\n";
        let r = check_entry_signature(FormalSystem::Agda, canonical, submitted);
        assert!(r.preserved, "{:?}", r.findings);
        assert_eq!(r.verdict, PreservationVerdict::Preserved);
    }

    /// Agda: whitespace / indented-continuation differences still preserve.
    #[test]
    fn agda_multiline_type_is_preserved() {
        let canonical = "foo : A -> B -> C";
        let submitted = "foo : A -> B\n      -> C\nfoo x y = c\n";
        let r = check_entry_signature(FormalSystem::Agda, canonical, submitted);
        assert!(r.preserved, "{:?}", r);
        assert_eq!(r.verdict, PreservationVerdict::Preserved);
    }

    /// Agda: a proof declaring a DIFFERENT type (`foo : B`) is flagged.
    #[test]
    fn agda_different_type_is_flagged() {
        let canonical = "foo : A";
        let submitted = "foo : B\nfoo = b\n";
        let r = check_entry_signature(FormalSystem::Agda, canonical, submitted);
        assert!(!r.preserved);
        assert_eq!(r.verdict, PreservationVerdict::ConclusionChanged);
        assert!(r.findings.iter().any(|f| f.contains("DIFFERENT")));
    }

    /// Agda: an unparsable canonical (no `:`) falls back — NOT a flagged verdict,
    /// so `verify()` defers to the mention check rather than rejecting.
    #[test]
    fn agda_unparsable_canonical_falls_back() {
        let r = check_entry_signature(FormalSystem::Agda, "foo", "foo : A\nfoo = a\n");
        assert_eq!(r.verdict, PreservationVerdict::CanonicalUnparsable);
        assert!(!is_flagged(r.verdict));
    }

    /// Agda: a legit proof whose entry the parser cannot locate falls back
    /// (does not spuriously reject).
    #[test]
    fn agda_missing_entry_falls_back() {
        // Canonical parses, but the submission declares only the equation, no sig.
        let r = check_entry_signature(FormalSystem::Agda, "foo : A", "foo = a\n");
        assert_eq!(r.verdict, PreservationVerdict::SubmittedMissing);
        assert!(!is_flagged(r.verdict));
    }

    /// Metamath: a `$p` asserting the requested statement passes.
    #[test]
    fn metamath_same_assertion_is_preserved() {
        let canonical = "th1 $p |- ( ph -> ph ) $= ? $.";
        let submitted = "th1 $p |- ( ph -> ph ) $= wph wph mpd $.";
        let r = check_entry_signature(FormalSystem::Metamath, canonical, submitted);
        assert!(r.preserved, "{:?}", r.findings);
        assert_eq!(r.verdict, PreservationVerdict::Preserved);
    }

    /// Metamath: a `$p` asserting a DIFFERENT statement is flagged.
    #[test]
    fn metamath_different_assertion_is_flagged() {
        let canonical = "th1 $p |- ( ph -> ph ) $= ? $.";
        let submitted = "th1 $p |- ( ph -> ps ) $= wph wps mpd $.";
        let r = check_entry_signature(FormalSystem::Metamath, canonical, submitted);
        assert!(!r.preserved);
        assert_eq!(r.verdict, PreservationVerdict::ConclusionChanged);
        assert!(r.findings.iter().any(|f| f.contains("DIFFERENT")));
    }

    /// Metamath: canonical without a `$p`/`$a` assertion falls back (not flagged).
    #[test]
    fn metamath_unparsable_canonical_falls_back() {
        let r = check_entry_signature(FormalSystem::Metamath, "|- ( ph -> ph )", "th1 $p |- ( ph -> ph ) $= x $.");
        assert_eq!(r.verdict, PreservationVerdict::CanonicalUnparsable);
        assert!(!is_flagged(r.verdict));
    }

    /// Metamath: a commented-out `$p` does not satisfy the check (falls back).
    #[test]
    fn metamath_commented_assertion_falls_back() {
        let canonical = "th1 $p |- ( ph -> ph ) $= ? $.";
        let submitted = "$( th1 $p |- ( ph -> ph ) $= x $. $)\n";
        let r = check_entry_signature(FormalSystem::Metamath, canonical, submitted);
        assert_eq!(r.verdict, PreservationVerdict::SubmittedMissing);
        assert!(!is_flagged(r.verdict));
    }

    /// Non-ASCII input never panics for either non-Lean system.
    #[test]
    fn non_ascii_entry_signature_does_not_panic() {
        let _ = check_entry_signature(FormalSystem::Agda, "β : ∀ x → x ≡ x", "β : ∀ x → x ≡ x\nβ x = refl\n");
        let _ = check_entry_signature(FormalSystem::Agda, "café", "≤ ∞ λ π");
        let _ = check_entry_signature(FormalSystem::Metamath, "τ $p |- ∀ x $= ? $.", "τ $p |- ∀ x $= π $.");
        let _ = check_entry_signature(FormalSystem::Metamath, "", "");
    }

    /// Deterministic across repeated calls for both non-Lean systems.
    #[test]
    fn entry_signature_is_deterministic() {
        let a1 = check_entry_signature(FormalSystem::Agda, "foo : A", "foo : B\nfoo = b\n");
        let a2 = check_entry_signature(FormalSystem::Agda, "foo : A", "foo : B\nfoo = b\n");
        assert_eq!(a1, a2);
        let m1 = check_entry_signature(FormalSystem::Metamath, "t $p |- a $= ? $.", "t $p |- b $= x $.");
        let m2 = check_entry_signature(FormalSystem::Metamath, "t $p |- a $= ? $.", "t $p |- b $= x $.");
        assert_eq!(m1, m2);
    }

    /// Mirror of `verify()`'s flagged set: only these verdicts reject the proof;
    /// every other verdict falls back to the lexical mention check.
    fn is_flagged(v: PreservationVerdict) -> bool {
        matches!(
            v,
            PreservationVerdict::Renamed
                | PreservationVerdict::BindersChanged
                | PreservationVerdict::ConclusionChanged
                | PreservationVerdict::TriviallyRestated
        )
    }

    // -- escape hatches ------------------------------------------------------

    /// A clean proof has no escape-hatch findings.
    #[test]
    fn clean_proof_has_no_escape_hatches() {
        let code = "theorem T (n : Nat) : n = n := by rfl\ntheorem U : True := by trivial\n";
        assert!(escape_hatches_clean(code));
        assert!(scan_escape_hatches(code).is_empty());
        assert!(escape_hatch_scan_report(code).clean);
    }

    /// `sorry` / `admit` are flagged CRITICAL.
    #[test]
    fn sorry_and_admit_are_critical() {
        let code = "theorem T : True := by sorry\ntheorem U : True := by admit\n";
        let hatches = scan_escape_hatches(code);
        assert!(!escape_hatches_clean(code));
        assert!(hatches.iter().any(|h| h.rule == "sorry" && h.line == 1));
        assert!(hatches.iter().any(|h| h.rule == "admit" && h.line == 2));
        let report = escape_hatch_scan_report(code);
        assert!(!report.clean);
        assert!(report.findings.iter().all(|f| f.contains("CRITICAL")));
    }

    /// The `apply?` / `exact?` / `rfl?` suggestion tactics (DeepSeek erratum) are
    /// flagged CRITICAL, and `native_decide` too.
    #[test]
    fn search_tactics_and_native_decide_are_critical() {
        let code = "\
theorem T : True := by apply?
theorem U : True := by exact?
theorem V (n : Nat) : n = n := by rfl?
theorem W : 2 + 2 = 4 := by native_decide
";
        let hatches = scan_escape_hatches(code);
        assert!(!escape_hatches_clean(code));
        for rule in ["apply?", "exact?", "rfl?", "native_decide"] {
            assert!(
                hatches.iter().any(|h| h.rule == rule),
                "expected `{rule}` to be flagged: {hatches:?}"
            );
        }
    }

    /// Escape hatches inside comments / strings do NOT false-flag.
    #[test]
    fn comments_and_strings_are_ignored_by_escape_scan() {
        let code = "\
-- this sorry is only a comment, and native_decide too
/- admit and apply? in a block comment -/
theorem T : String := \"sorry and exact? in a literal\"
";
        assert!(escape_hatches_clean(code), "{:?}", scan_escape_hatches(code));
    }

    /// A tactic name is matched whole-token: `apply` (no `?`) is not flagged, and
    /// `sorry` inside `sorryAx`-like identifiers is not double-matched.
    #[test]
    fn whole_token_matching_avoids_false_positives() {
        // `apply` without `?` is a legitimate tactic — not an escape hatch.
        let code = "theorem T (h : True) : True := by apply h\n";
        assert!(escape_hatches_clean(code));
    }

    // -- error-span formatting ----------------------------------------------

    /// `format_error_spans` marks the offending line with `>>>` and shows ±2
    /// lines of context.
    #[test]
    fn format_error_spans_marks_the_right_line() {
        let code = "line1\nline2\nline3\nline4\nline5";
        let errors = vec![LeanError::new(3, "unsolved goals")];
        let out = format_error_spans(code, &errors);
        assert!(out.contains("error at line 3: unsolved goals"));
        // The offending line is marked, context lines are not.
        assert!(out.contains(">>>"));
        assert!(out.lines().any(|l| l.starts_with(">>>") && l.contains("line3")));
        assert!(out.lines().any(|l| l.starts_with("   ") && l.contains("line2")));
        assert!(out.lines().any(|l| l.starts_with("   ") && l.contains("line4")));
        // ±2 window: line1 and line5 are included, nothing beyond.
        assert!(out.contains("line1"));
        assert!(out.contains("line5"));
    }

    /// Windows clamp at the file edges and out-of-range errors still print.
    #[test]
    fn format_error_spans_clamps_and_handles_out_of_range() {
        let code = "only1\nonly2";
        // Error on line 1: window clamps to [1,2].
        let out = format_error_spans(code, &[LeanError::new(1, "at top")]);
        assert!(out.lines().any(|l| l.starts_with(">>>") && l.contains("only1")));
        assert!(out.contains("only2"));
        // Out-of-range error still renders its message.
        let oor = format_error_spans(code, &[LeanError::new(99, "phantom")]);
        assert!(oor.contains("error at line 99: phantom"));
        assert!(oor.contains("out of range"));
        // Empty errors => empty string.
        assert_eq!(format_error_spans(code, &[]), "");
    }

    /// Multiple errors render in `(line, message)` order.
    #[test]
    fn format_error_spans_orders_errors() {
        let code = "a\nb\nc\nd\ne\nf";
        let errors = vec![
            LeanError::new(5, "second"),
            LeanError::new(2, "first"),
        ];
        let out = format_error_spans(code, &errors);
        let first = out.find("first").unwrap();
        let second = out.find("second").unwrap();
        assert!(first < second, "line-2 error must render before line-5");
    }

    // -- determinism ---------------------------------------------------------

    /// Identical input yields identical results across all three entry points.
    #[test]
    fn results_are_deterministic() {
        let canonical = "theorem T (n : Nat) (h : n > 0) : n ≥ 1";
        let submitted = "\
-- with a sorry and an apply?
theorem T (n : Nat) : n ≥ 1 := by
  apply?
  sorry
";
        let a = check_statement_preserved(canonical, submitted);
        let b = check_statement_preserved(canonical, submitted);
        assert_eq!(a, b);

        let ha = scan_escape_hatches(submitted);
        let hb = scan_escape_hatches(submitted);
        assert_eq!(ha, hb);
        // Findings ordered by line.
        let lines: Vec<usize> = ha.iter().map(|h| h.line).collect();
        let mut sorted = lines.clone();
        sorted.sort();
        assert_eq!(lines, sorted);

        let errors = vec![LeanError::new(3, "e"), LeanError::new(1, "e")];
        assert_eq!(
            format_error_spans(submitted, &errors),
            format_error_spans(submitted, &errors)
        );
    }
}
