//! VACUOUS-SUCCESS guard + UNUSED-HYPOTHESIS advisory (Tier 0 gate hardening).
//!
//! This is OUR code — a *trust-boundary* checker built from first principles,
//! std-only + `serde`/`serde_json` (already dependencies of
//! [`crate::prover::formal`]), pure, deterministic, and offline: no IO, no clock,
//! no RNG. It closes a confirmed hole in the 3+1-layer gate.
//!
//! # (1) Vacuous success — the hole
//!
//! An agent can discharge a goal by making its hypothesis bundle
//! **contradictory**. `theorem t (h₁ : x > 5) (h₂ : x < 3) : Goal` is provable by
//! `omega` / `exact absurd …` for ANY `Goal`. Such a proof:
//!
//! * passes the **kernel** (it really is a valid derivation),
//! * passes the **axiom audit** (no `sorry`, no new axiom, no `mk_thm`),
//! * passes **statement preservation** (the canonical signature is untouched),
//!
//! and is nevertheless worthless — the theorem is *vacuously* true and says
//! nothing. In the spec language this is "a hollow success that must be treated
//! as a spec failure". No downstream layer can see it, because every existing
//! layer asks "is this derivation sound?" and the derivation IS sound.
//!
//! The defense, applied BEFORE proof search begins: confirm the hypothesis
//! bundle is **satisfiable** by exhibiting at least one concrete instance meeting
//! every field — a [`SatisfiabilityWitness`]. Satisfiability of an arbitrary
//! first-order bundle is undecidable, so we do NOT attempt to decide it. We
//! demand the caller *exhibit* a witness, and we fail closed on its absence:
//! **no witness ⇒ not clean** ([`VacuityReport::clean`] is `false`).
//!
//! # (2) Refutation heuristics are one-directional — read this
//!
//! [`refute_bundle`] contains cheap syntactic detectors (contradictory numeric
//! bounds on one variable, a `False`/`Empty`/`⊥` hypothesis, a proposition and
//! its literal negation both present). They are **SOUND IN THE REFUTING
//! DIRECTION ONLY**:
//!
//! * A [`Contradiction`] returned here is strong evidence the bundle is
//!   unsatisfiable (modulo the caveats on each detector), so we reject.
//! * An EMPTY result is **not** a satisfiability proof, not a hint of one, and is
//!   never treated as one anywhere in this module. It means only "these three
//!   cheap patterns did not fire". The bundle may still be wildly unsatisfiable.
//!
//! Consequently a bundle is only ever `clean` because a caller-supplied witness
//! survived checking — never because refutation came up empty. Any future edit
//! that lets an empty refutation set stand in for a witness re-opens exactly the
//! hole this module exists to close.
//!
//! # (3) Unused hypothesis — the dual, and why it does NOT gate
//!
//! A proof that never uses a stated hypothesis is EITHER a *stronger* theorem
//! than requested (the assumption was unnecessary — a good outcome worth
//! surfacing) OR a *mis-formalized* statement (the hypothesis was meant to
//! constrain something and, as written, constrains nothing — a bad outcome). The
//! two are indistinguishable from the proof alone, and both are common in real
//! corpora (a mined repo showed both coprimality hypotheses unused in its main
//! theorem, and 20+ instances overall).
//!
//! Because the good case is genuinely good, [`detect_unused_hypotheses`] is
//! **advisory metadata and MUST NOT gate**. Nothing in this module folds it into
//! [`VacuityReport::clean`].
//!
//! It is also a *cheap text-level approximation*: "the binder name never appears
//! as a whole token in the proof body". **Lean's own `linter.unusedSectionVars`
//! (and `#print axioms` / the elaborator's own usage information) is the
//! AUTHORITATIVE source.** This pass has no elaborator, so it:
//!
//! * misses a hypothesis used implicitly (via `omega`, `simp_all`, `tauto`,
//!   `assumption`, `aesop`, or any tactic that consults the whole local context
//!   without naming it) — reporting it as unused when it is used;
//! * misses a hypothesis "used" only in dead code;
//! * cannot see through `rename_i`, anonymous constructor patterns, or `‹…›`
//!   anonymous hypothesis access.
//!
//! Treat its output as a review queue, never as a verdict.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

// ===========================================================================
// Hypothesis bundles
// ===========================================================================

/// What role a bundle field plays. This decides what a witness must supply for
/// it: a [`Datum`](FieldKind::Datum) needs a concrete binding, a
/// [`Hypothesis`](FieldKind::Hypothesis) needs an explicit claim that the
/// instance satisfies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    /// A data binder — the thing being quantified over (`(n : Nat)`, `(x : ℝ)`).
    /// A witness must give it a concrete value.
    Datum,
    /// A propositional hypothesis (`(h : n > 0)`). A witness must explicitly
    /// claim its instance satisfies it.
    Hypothesis,
}

/// One field of a goal's hypothesis bundle: the binder name, its type/proposition
/// text, and its role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HypothesisField {
    /// Binder name as written (`n`, `h₁`, `hx`).
    pub binder: String,
    /// The binder's type or proposition, as written.
    pub text: String,
    /// Whether this field is data to instantiate or a proposition to satisfy.
    pub kind: FieldKind,
}

impl HypothesisField {
    /// A data binder (`(n : Nat)`) — a witness must bind it.
    pub fn datum(binder: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            binder: binder.into(),
            text: text.into(),
            kind: FieldKind::Datum,
        }
    }

    /// A propositional hypothesis (`(h : n > 0)`) — a witness must claim it.
    pub fn hypothesis(binder: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            binder: binder.into(),
            text: text.into(),
            kind: FieldKind::Hypothesis,
        }
    }
}

/// A goal's hypothesis bundle: the goal it belongs to plus its ordered fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HypothesisBundle {
    /// Identifier of the goal / theorem this bundle guards (for the report).
    pub goal: String,
    /// The bundle's fields, in declaration order.
    pub fields: Vec<HypothesisField>,
}

impl HypothesisBundle {
    pub fn new(goal: impl Into<String>, fields: Vec<HypothesisField>) -> Self {
        Self {
            goal: goal.into(),
            fields,
        }
    }

    /// A bundle is TRIVIAL when it states no proposition to satisfy: there is
    /// nothing that could be vacuously contradictory, so no witness is demanded.
    ///
    /// Data binders alone do not make a bundle non-trivial — `∀ n : Nat, P n`
    /// carries no assumption that could be self-contradictory. (An *empty* type
    /// as a data binder would, but that is a type-level emptiness question this
    /// pass cannot see; callers who care should state it as a hypothesis field.)
    pub fn is_trivial(&self) -> bool {
        !self
            .fields
            .iter()
            .any(|f| f.kind == FieldKind::Hypothesis)
    }

    /// Propositional fields, in order.
    pub fn hypotheses(&self) -> impl Iterator<Item = &HypothesisField> {
        self.fields
            .iter()
            .filter(|f| f.kind == FieldKind::Hypothesis)
    }

    /// Data binders, in order.
    pub fn data(&self) -> impl Iterator<Item = &HypothesisField> {
        self.fields.iter().filter(|f| f.kind == FieldKind::Datum)
    }
}

// ===========================================================================
// Satisfiability witness
// ===========================================================================

/// A caller-supplied CONCRETE INSTANCE claimed to satisfy every field of a
/// [`HypothesisBundle`] — the exhibit that makes the bundle non-vacuous.
///
/// This is a *claim*, not a proof. [`check_vacuity`] audits the claim as far as
/// a pure syntactic pass can (every datum bound, every hypothesis claimed, no
/// simple numeric constraint numerically violated by the bound values, no
/// syntactic contradiction in the bundle) and rejects it when the audit fails. A
/// surviving witness is evidence, not certainty: a hypothesis this pass cannot
/// evaluate is taken on the caller's word.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SatisfiabilityWitness {
    /// Human label for the instance (`"n := 7, x := 2.5"`), for the report.
    pub label: String,
    /// Concrete value for each data binder. `BTreeMap` so serialization and
    /// finding order are deterministic.
    pub bindings: BTreeMap<String, Value>,
    /// The binder names of the propositional fields this instance is claimed to
    /// satisfy. Must cover every [`FieldKind::Hypothesis`] field.
    pub claims: Vec<String>,
}

impl SatisfiabilityWitness {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            bindings: BTreeMap::new(),
            claims: Vec::new(),
        }
    }

    /// Bind a data binder to a concrete value.
    pub fn bind(mut self, binder: impl Into<String>, value: Value) -> Self {
        self.bindings.insert(binder.into(), value);
        self
    }

    /// Claim this instance satisfies the named propositional field.
    pub fn claim(mut self, binder: impl Into<String>) -> Self {
        let b = binder.into();
        if !self.claims.iter().any(|c| c == &b) {
            self.claims.push(b);
        }
        self
    }

    /// Numeric view of a binding, when it is a JSON number.
    fn numeric(&self, binder: &str) -> Option<f64> {
        self.bindings.get(binder)?.as_f64()
    }
}

/// Verdict on the witness for a bundle. Only [`WitnessSupplied`] leaves the gate
/// open.
///
/// [`WitnessSupplied`]: WitnessVerdict::WitnessSupplied
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WitnessVerdict {
    /// A witness was supplied and survived the audit (or the bundle is trivial,
    /// with nothing to witness).
    WitnessSupplied,
    /// No witness was supplied for a NON-TRIVIAL bundle. Fail closed: absence of
    /// a witness is never evidence of satisfiability.
    WitnessMissing,
    /// A witness was supplied but the audit rejected it — an incomplete instance,
    /// a numerically violated constraint, or a syntactically contradictory
    /// bundle no instance could satisfy.
    WitnessRejected,
}

impl WitnessVerdict {
    /// Whether this verdict leaves the gate OPEN.
    pub fn is_clean(self) -> bool {
        matches!(self, WitnessVerdict::WitnessSupplied)
    }

    /// Stable snake_case tag for finding strings / JSON detail.
    pub fn tag(self) -> &'static str {
        match self {
            WitnessVerdict::WitnessSupplied => "witness_supplied",
            WitnessVerdict::WitnessMissing => "witness_missing",
            WitnessVerdict::WitnessRejected => "witness_rejected",
        }
    }
}

// ===========================================================================
// Contradiction (refutation) heuristics — REFUTING DIRECTION ONLY
// ===========================================================================

/// A syntactic contradiction found in a bundle. See the module docs: finding one
/// is evidence of unsatisfiability; finding NONE proves nothing whatsoever.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contradiction {
    /// Stable rule id (`numeric_bounds` / `false_hypothesis` /
    /// `literal_negation`).
    pub rule: &'static str,
    /// The binder names of the fields involved, in bundle order.
    pub fields: Vec<String>,
    /// Human-readable explanation.
    pub detail: String,
}

/// Cheap syntactic refutation of a hypothesis bundle.
///
/// **One-directional.** A non-empty result is evidence the bundle is
/// unsatisfiable — the theorem would be vacuously true and any proof of it
/// hollow. An empty result is NOT a satisfiability proof and must never be read
/// as one; it only means none of the three patterns below fired.
///
/// Detectors, all deterministic and ordered `(rule, fields)`:
///
/// 1. **`numeric_bounds`** — a variable carries simple literal bounds that cannot
///    be jointly met (`x > 5` with `x < 3`; `x = 1` with `x = 2`; `x ≥ 4` with
///    `x ≤ 4` is fine, `x > 4` with `x ≤ 4` is not). Only whole hypotheses of the
///    exact shape `<ident> <op> <number>` or `<number> <op> <ident>` are read, so
///    a compound hypothesis (`x > 5 ∨ y < 3`) is simply not analyzed rather than
///    mis-analyzed. Reals are assumed — no integrality reasoning, so `x > 1` with
///    `x < 2` is NOT refuted even over `Nat`.
/// 2. **`false_hypothesis`** — a field whose proposition is literally `False`,
///    `Empty`, `⊥`, `PEmpty`, or `Fin 0`. Nothing satisfies it.
/// 3. **`literal_negation`** — two fields whose propositions are `P` and a
///    literal negation of `P` (`¬P`, `¬ P`, `~P`, `Not P`, `¬(P)`), compared up
///    to whitespace. Purely syntactic: it cannot see semantic negations
///    (`x > 5` vs `x ≤ 5` is caught by detector 1 only when both are literal
///    bounds).
pub fn refute_bundle(bundle: &HypothesisBundle) -> Vec<Contradiction> {
    let mut out: Vec<Contradiction> = Vec::new();

    // (2) False / Empty as a hypothesis.
    for f in bundle.hypotheses() {
        if is_false_prop(&f.text) {
            out.push(Contradiction {
                rule: "false_hypothesis",
                fields: vec![f.binder.clone()],
                detail: format!(
                    "hypothesis `{}` is the empty proposition `{}` — no instance satisfies it, \
                     so the theorem is vacuously true and any proof of it is hollow",
                    f.binder,
                    norm_ws(&f.text)
                ),
            });
        }
    }

    // (3) A proposition and its literal negation both assumed.
    let props: Vec<(&str, String)> = bundle
        .hypotheses()
        .map(|f| (f.binder.as_str(), norm_ws(&f.text)))
        .collect();
    for i in 0..props.len() {
        for j in 0..props.len() {
            if i == j {
                continue;
            }
            if let Some(inner) = strip_negation(&props[j].1) {
                if inner == props[i].1 {
                    // Emit once per unordered pair (the positive field first).
                    out.push(Contradiction {
                        rule: "literal_negation",
                        fields: vec![props[i].0.to_string(), props[j].0.to_string()],
                        detail: format!(
                            "hypotheses `{}` and `{}` assume `{}` and its literal negation `{}` \
                             — the bundle is contradictory and the theorem vacuously true",
                            props[i].0, props[j].0, props[i].1, props[j].1
                        ),
                    });
                }
            }
        }
    }

    // (1) Contradictory literal numeric bounds on one variable.
    out.extend(refute_numeric_bounds(bundle));

    out.sort_by(|a, b| a.rule.cmp(b.rule).then_with(|| a.fields.cmp(&b.fields)));
    out.dedup();
    out
}

/// Interval accumulated for one variable from simple literal comparisons.
#[derive(Debug, Clone)]
struct Interval {
    /// Greatest lower bound seen, with strictness and the field that set it.
    lo: Option<(f64, bool, String)>,
    /// Least upper bound seen, with strictness and the field that set it.
    hi: Option<(f64, bool, String)>,
}

impl Interval {
    fn new() -> Self {
        Self { lo: None, hi: None }
    }

    /// Tighten the lower bound (`strict` = `>` rather than `≥`).
    fn push_lo(&mut self, v: f64, strict: bool, field: &str) {
        let replace = match &self.lo {
            None => true,
            Some((cur, cur_strict, _)) => v > *cur || (v == *cur && strict && !*cur_strict),
        };
        if replace {
            self.lo = Some((v, strict, field.to_string()));
        }
    }

    /// Tighten the upper bound (`strict` = `<` rather than `≤`).
    fn push_hi(&mut self, v: f64, strict: bool, field: &str) {
        let replace = match &self.hi {
            None => true,
            Some((cur, cur_strict, _)) => v < *cur || (v == *cur && strict && !*cur_strict),
        };
        if replace {
            self.hi = Some((v, strict, field.to_string()));
        }
    }

    /// Whether the accumulated bounds are jointly unsatisfiable over the reals.
    fn is_empty(&self) -> bool {
        match (&self.lo, &self.hi) {
            (Some((lo, lo_strict, _)), Some((hi, hi_strict, _))) => {
                lo > hi || (lo == hi && (*lo_strict || *hi_strict))
            }
            _ => false,
        }
    }
}

/// Detector 1: contradictory literal numeric bounds. See [`refute_bundle`].
fn refute_numeric_bounds(bundle: &HypothesisBundle) -> Vec<Contradiction> {
    // Insertion-ordered accumulation keyed by variable, then sorted for
    // determinism.
    let mut vars: BTreeMap<String, Interval> = BTreeMap::new();
    for f in bundle.hypotheses() {
        let Some(cmp) = parse_simple_comparison(&f.text) else {
            continue;
        };
        let iv = vars.entry(cmp.var.clone()).or_insert_with(Interval::new);
        match cmp.op {
            CmpOp::Gt => iv.push_lo(cmp.value, true, &f.binder),
            CmpOp::Ge => iv.push_lo(cmp.value, false, &f.binder),
            CmpOp::Lt => iv.push_hi(cmp.value, true, &f.binder),
            CmpOp::Le => iv.push_hi(cmp.value, false, &f.binder),
            CmpOp::Eq => {
                iv.push_lo(cmp.value, false, &f.binder);
                iv.push_hi(cmp.value, false, &f.binder);
            }
        }
    }

    let mut out = Vec::new();
    for (var, iv) in &vars {
        if !iv.is_empty() {
            continue;
        }
        let (lo, lo_strict, lo_field) = iv.lo.clone().expect("empty interval has both bounds");
        let (hi, hi_strict, hi_field) = iv.hi.clone().expect("empty interval has both bounds");
        let mut fields = vec![lo_field.clone(), hi_field.clone()];
        fields.sort();
        fields.dedup();
        out.push(Contradiction {
            rule: "numeric_bounds",
            fields,
            detail: format!(
                "variable `{var}` is constrained to the empty range (`{}` from `{lo_field}` and \
                 `{}` from `{hi_field}`) — no instance satisfies the bundle, so the theorem is \
                 vacuously true",
                fmt_bound(var, if lo_strict { ">" } else { "≥" }, lo),
                fmt_bound(var, if hi_strict { "<" } else { "≤" }, hi),
            ),
        });
    }
    out
}

fn fmt_bound(var: &str, op: &str, v: f64) -> String {
    format!("{var} {op} {}", fmt_num(v))
}

/// Render a bound value without a trailing `.0` for integral values, so findings
/// read `x > 5` rather than `x > 5.0`.
fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// The comparison operators the simple-comparison parser understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CmpOp {
    Gt,
    Ge,
    Lt,
    Le,
    Eq,
}

impl CmpOp {
    /// The operator with its operands swapped (`a < b` ⇔ `b > a`), used to
    /// normalize `5 < x` into `x > 5`.
    fn flip(self) -> Self {
        match self {
            CmpOp::Gt => CmpOp::Lt,
            CmpOp::Ge => CmpOp::Le,
            CmpOp::Lt => CmpOp::Gt,
            CmpOp::Le => CmpOp::Ge,
            CmpOp::Eq => CmpOp::Eq,
        }
    }
}

/// A parsed `<var> <op> <literal>` constraint.
#[derive(Debug, Clone)]
struct SimpleComparison {
    var: String,
    op: CmpOp,
    value: f64,
}

/// Parse a hypothesis that is EXACTLY a simple literal comparison —
/// `<ident> <op> <number>` or `<number> <op> <ident>`, optionally parenthesized
/// as a whole. Anything else (conjunctions, disjunctions, arithmetic on either
/// side, two variables) yields `None` and is left unanalyzed: the detector's job
/// is to refute confidently or say nothing.
fn parse_simple_comparison(text: &str) -> Option<SimpleComparison> {
    let normalized = norm_ws(text);
    let t = strip_outer_parens(&normalized);
    let toks = tokenize_comparison(t)?;
    let (lhs, op, rhs) = toks;

    if let (Some(var), Some(value)) = (as_ident(&lhs), as_number(&rhs)) {
        return Some(SimpleComparison { var, op, value });
    }
    if let (Some(value), Some(var)) = (as_number(&lhs), as_ident(&rhs)) {
        return Some(SimpleComparison {
            var,
            op: op.flip(),
            value,
        });
    }
    None
}

/// Split a normalized comparison into `(lhs, op, rhs)` at its single top-level
/// operator. `None` when there is not exactly one.
fn tokenize_comparison(t: &str) -> Option<(String, CmpOp, String)> {
    // Longest operators first so `>=` is not read as `>`.
    const OPS: &[(&str, CmpOp)] = &[
        (">=", CmpOp::Ge),
        ("<=", CmpOp::Le),
        ("≥", CmpOp::Ge),
        ("≤", CmpOp::Le),
        (">", CmpOp::Gt),
        ("<", CmpOp::Lt),
        ("=", CmpOp::Eq),
    ];
    let chars: Vec<char> = t.chars().collect();
    let mut found: Option<(usize, usize, CmpOp)> = None;
    let mut i = 0usize;
    while i < chars.len() {
        // `≠`, `!=`, `==` and `:=` are not simple comparisons we model; bail out
        // rather than mis-read them.
        if chars[i] == '≠' {
            return None;
        }
        if (chars[i] == '!' || chars[i] == '=' || chars[i] == ':') && chars.get(i + 1) == Some(&'=')
        {
            return None;
        }
        let mut matched = false;
        for (lit, op) in OPS {
            let l: Vec<char> = lit.chars().collect();
            if i + l.len() <= chars.len() && chars[i..i + l.len()] == l[..] {
                if found.is_some() {
                    // More than one operator: not a simple comparison.
                    return None;
                }
                found = Some((i, i + l.len(), *op));
                i += l.len();
                matched = true;
                break;
            }
        }
        if !matched {
            i += 1;
        }
    }
    let (start, end, op) = found?;
    let lhs: String = chars[..start].iter().collect();
    let rhs: String = chars[end..].iter().collect();
    Some((lhs.trim().to_string(), op, rhs.trim().to_string()))
}

/// The token as a bare identifier, if it is exactly one.
fn as_ident(s: &str) -> Option<String> {
    let s = strip_outer_parens(s.trim());
    if s.is_empty() {
        return None;
    }
    let mut chars = s.chars();
    let first = chars.next()?;
    if !(first.is_alphabetic() || first == '_') {
        return None;
    }
    if !s.chars().all(is_word) {
        return None;
    }
    Some(s.to_string())
}

/// The token as a bare numeric literal, if it is exactly one (an optional sign
/// and a decimal literal).
fn as_number(s: &str) -> Option<f64> {
    let s = strip_outer_parens(s.trim());
    if s.is_empty() {
        return None;
    }
    let body = s.strip_prefix('-').unwrap_or(s);
    if body.is_empty() || !body.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    s.parse::<f64>().ok()
}

/// Strip one layer of fully-enclosing parentheses, repeatedly.
fn strip_outer_parens(s: &str) -> &str {
    let mut cur = s.trim();
    loop {
        let bytes = cur.as_bytes();
        if bytes.len() < 2 || bytes[0] != b'(' || bytes[bytes.len() - 1] != b')' {
            return cur;
        }
        // Confirm the leading `(` closes at the very end (not `(a) + (b)`).
        let mut depth = 0i32;
        let mut closes_at_end = false;
        for (idx, c) in cur.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        closes_at_end = idx + c.len_utf8() == cur.len();
                        break;
                    }
                }
                _ => {}
            }
        }
        if !closes_at_end {
            return cur;
        }
        cur = cur[1..cur.len() - 1].trim();
    }
}

/// Whether a proposition is literally the empty one.
fn is_false_prop(text: &str) -> bool {
    matches!(
        strip_outer_parens(&norm_ws(text)),
        "False" | "false" | "Empty" | "PEmpty" | "⊥" | "Fin 0" | "0 = 1" | "1 = 0"
    )
}

/// If `text` is a literal negation, the proposition negated (whitespace-
/// normalized). Handles `¬P`, `¬ P`, `~P`, `Not P`, and `¬(P)`.
fn strip_negation(text: &str) -> Option<String> {
    let t = strip_outer_parens(&norm_ws(text)).to_string();
    for prefix in ["¬", "~"] {
        if let Some(rest) = t.strip_prefix(prefix) {
            let inner = strip_outer_parens(rest.trim());
            if !inner.is_empty() {
                return Some(norm_ws(inner));
            }
        }
    }
    if let Some(rest) = t.strip_prefix("Not ") {
        let inner = strip_outer_parens(rest.trim());
        if !inner.is_empty() {
            return Some(norm_ws(inner));
        }
    }
    None
}

// ===========================================================================
// Vacuity report
// ===========================================================================

/// Severity of a vacuity finding. `Critical` findings make the report un-clean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VacuitySeverity {
    Critical,
    Warning,
}

impl VacuitySeverity {
    pub fn tag(self) -> &'static str {
        match self {
            VacuitySeverity::Critical => "CRITICAL",
            VacuitySeverity::Warning => "WARNING",
        }
    }
}

/// One vacuity finding: what fired, on which fields, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VacuityFinding {
    pub severity: VacuitySeverity,
    /// Stable rule id (`witness_missing` / `numeric_bounds` / …).
    pub rule: &'static str,
    /// Binder names involved, in deterministic order.
    pub fields: Vec<String>,
    pub detail: String,
}

/// The result of [`check_vacuity`], in the [`ScanReport`] idiom (`clean` /
/// `findings` / `detail`) plus the structured verdict and contradictions.
///
/// [`ScanReport`]: crate::prover::formal::ScanReport
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VacuityReport {
    /// Fail-closed: `true` only when the bundle is trivial, or a supplied witness
    /// survived the audit. A non-trivial bundle with NO witness is `false`.
    pub clean: bool,
    /// Structured witness verdict.
    pub verdict: WitnessVerdict,
    /// The goal this report is about.
    pub goal: String,
    /// Syntactic contradictions found (refuting direction only — see
    /// [`refute_bundle`]).
    pub contradictions: Vec<Contradiction>,
    /// Structured findings, ordered `(rule, fields)`.
    pub findings: Vec<VacuityFinding>,
    /// Structured detail for the gate's JSON report.
    pub detail: Value,
}

impl VacuityReport {
    /// Human-readable finding lines, `SEVERITY: rule [fields]: detail`.
    pub fn finding_strings(&self) -> Vec<String> {
        self.findings
            .iter()
            .map(|f| {
                format!(
                    "{}: {} [{}]: {}",
                    f.severity.tag(),
                    f.rule,
                    f.fields.join(", "),
                    f.detail
                )
            })
            .collect()
    }

    /// Layer-2c view: a [`ScanReport`](crate::prover::formal::ScanReport) a
    /// backend `source_scan` (or the gate) can fold in conjunctively. `clean`
    /// carries over verbatim (fail-closed).
    pub fn into_scan_report(self) -> crate::prover::formal::ScanReport {
        let findings = self.finding_strings();
        crate::prover::formal::ScanReport {
            clean: self.clean,
            findings,
            detail: json!({
                "check": "vacuity",
                "goal": self.goal,
                "verdict": self.verdict.tag(),
                "contradictions": self.contradictions,
            }),
        }
    }
}

/// The vacuous-success gate: confirm a goal's hypothesis bundle is satisfiable
/// BEFORE proof search begins.
///
/// Fail-closed by construction:
///
/// * A **trivial** bundle (no propositional field) is clean — there is nothing
///   that could be vacuously contradictory.
/// * A non-trivial bundle with **no witness** is [`WitnessMissing`] and NOT
///   clean. Absence of a witness is never evidence of satisfiability.
/// * A witness is [`WitnessRejected`] (NOT clean) when the bundle is
///   syntactically refuted, when a data binder is unbound, when a propositional
///   field is unclaimed, or when a bound value numerically violates a simple
///   literal constraint the instance is claimed to satisfy.
/// * Otherwise [`WitnessSupplied`] and clean.
///
/// Pure and deterministic: same inputs ⇒ identical report, no IO/clock/RNG.
///
/// [`WitnessMissing`]: WitnessVerdict::WitnessMissing
/// [`WitnessRejected`]: WitnessVerdict::WitnessRejected
/// [`WitnessSupplied`]: WitnessVerdict::WitnessSupplied
pub fn check_vacuity(
    bundle: &HypothesisBundle,
    witness: Option<&SatisfiabilityWitness>,
) -> VacuityReport {
    let contradictions = refute_bundle(bundle);
    let mut findings: Vec<VacuityFinding> = Vec::new();

    // A syntactic refutation is CRITICAL regardless of what the caller claims:
    // no instance can satisfy a bundle we can refute.
    for c in &contradictions {
        findings.push(VacuityFinding {
            severity: VacuitySeverity::Critical,
            rule: c.rule,
            fields: c.fields.clone(),
            detail: c.detail.clone(),
        });
    }

    let verdict = if bundle.is_trivial() && contradictions.is_empty() {
        WitnessVerdict::WitnessSupplied
    } else {
        match witness {
            None => {
                if contradictions.is_empty() {
                    findings.push(VacuityFinding {
                        severity: VacuitySeverity::Critical,
                        rule: "witness_missing",
                        fields: bundle.hypotheses().map(|f| f.binder.clone()).collect(),
                        detail: format!(
                            "no satisfiability witness supplied for goal `{}`: the hypothesis \
                             bundle is non-trivial and was never shown to admit a concrete \
                             instance. An unsatisfiable bundle makes the theorem vacuously true \
                             — a hollow success. Failing closed (absence of a witness is not \
                             evidence of satisfiability).",
                            bundle.goal
                        ),
                    });
                }
                WitnessVerdict::WitnessMissing
            }
            Some(w) => {
                findings.extend(audit_witness(bundle, w));
                if findings
                    .iter()
                    .any(|f| f.severity == VacuitySeverity::Critical)
                {
                    WitnessVerdict::WitnessRejected
                } else {
                    WitnessVerdict::WitnessSupplied
                }
            }
        }
    };

    findings.sort_by(|a, b| a.rule.cmp(b.rule).then_with(|| a.fields.cmp(&b.fields)));
    findings.dedup();

    let clean = verdict.is_clean()
        && !findings
            .iter()
            .any(|f| f.severity == VacuitySeverity::Critical);

    let detail = json!({
        "check": "vacuity",
        "goal": bundle.goal,
        "verdict": verdict.tag(),
        "trivial_bundle": bundle.is_trivial(),
        "contradictions": contradictions,
        "witness": witness,
        "findings": findings,
    });

    VacuityReport {
        clean,
        verdict,
        goal: bundle.goal.clone(),
        contradictions,
        findings,
        detail,
    }
}

/// Audit a supplied witness against the bundle: completeness (every datum bound,
/// every hypothesis claimed) and, where the constraint is a simple literal
/// comparison over a bound numeric value, arithmetic consistency.
fn audit_witness(bundle: &HypothesisBundle, w: &SatisfiabilityWitness) -> Vec<VacuityFinding> {
    let mut findings = Vec::new();

    for f in bundle.data() {
        if !w.bindings.contains_key(&f.binder) {
            findings.push(VacuityFinding {
                severity: VacuitySeverity::Critical,
                rule: "witness_incomplete",
                fields: vec![f.binder.clone()],
                detail: format!(
                    "witness `{}` gives no concrete value for data binder `{}` : {} — the \
                     instance is incomplete and does not exhibit satisfiability",
                    w.label,
                    f.binder,
                    norm_ws(&f.text)
                ),
            });
        }
    }

    for f in bundle.hypotheses() {
        if !w.claims.iter().any(|c| c == &f.binder) {
            findings.push(VacuityFinding {
                severity: VacuitySeverity::Critical,
                rule: "witness_incomplete",
                fields: vec![f.binder.clone()],
                detail: format!(
                    "witness `{}` does not claim to satisfy hypothesis `{}` : {} — every field \
                     must be met by the SAME instance",
                    w.label,
                    f.binder,
                    norm_ws(&f.text)
                ),
            });
            continue;
        }
        // Where we can actually evaluate the claim, do so.
        if let Some(cmp) = parse_simple_comparison(&f.text) {
            if let Some(actual) = w.numeric(&cmp.var) {
                let holds = match cmp.op {
                    CmpOp::Gt => actual > cmp.value,
                    CmpOp::Ge => actual >= cmp.value,
                    CmpOp::Lt => actual < cmp.value,
                    CmpOp::Le => actual <= cmp.value,
                    CmpOp::Eq => actual == cmp.value,
                };
                if !holds {
                    findings.push(VacuityFinding {
                        severity: VacuitySeverity::Critical,
                        rule: "witness_violates_field",
                        fields: vec![f.binder.clone()],
                        detail: format!(
                            "witness `{}` binds `{}` := {} which does NOT satisfy hypothesis \
                             `{}` : {}",
                            w.label,
                            cmp.var,
                            fmt_num(actual),
                            f.binder,
                            norm_ws(&f.text)
                        ),
                    });
                }
            }
        }
    }

    findings
}

// ===========================================================================
// Unused hypotheses — ADVISORY ONLY, never gating
// ===========================================================================

/// Classification of an unused hypothesis. Advisory: see the module docs on why
/// this never gates and why Lean's `linter.unusedSectionVars` is authoritative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnusedClass {
    /// Every variable the hypothesis constrains IS otherwise used by the proof,
    /// yet the hypothesis itself is not — the proof plausibly establishes a
    /// STRONGER theorem than requested (the assumption was unnecessary). Worth
    /// surfacing as a result, not a defect.
    PossiblyStronger,
    /// The hypothesis constrains a variable the proof never touches either — the
    /// whole clause is inert, which is the signature of a MIS-FORMALIZED
    /// statement (it does not say what the author meant).
    PossiblyMisformalized,
    /// Not distinguishable at text level: an instance-implicit / typeclass /
    /// universe-style binder, or a hypothesis with no extractable variables.
    Unknown,
}

impl UnusedClass {
    pub fn tag(self) -> &'static str {
        match self {
            UnusedClass::PossiblyStronger => "possibly_stronger",
            UnusedClass::PossiblyMisformalized => "possibly_misformalized",
            UnusedClass::Unknown => "unknown",
        }
    }
}

/// One hypothesis whose binder name never appears in the proof body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnusedHypothesis {
    /// The binder name that never appears.
    pub binder: String,
    /// The hypothesis text, whitespace-normalized.
    pub text: String,
    /// Advisory classification.
    pub class: UnusedClass,
    /// Human-readable explanation, including the standing caveat.
    pub detail: String,
}

/// Report hypotheses of `bundle` whose binder name never appears as a whole token
/// in `proof_body`.
///
/// **ADVISORY ONLY — this must never gate.** Nothing here feeds
/// [`VacuityReport::clean`]. An unused hypothesis is either a stronger theorem
/// (good) or a mis-formalization (bad), and text alone cannot decide which; the
/// [`UnusedClass`] is a triage hint, not a verdict.
///
/// **Lean's `linter.unusedSectionVars` is the authoritative source.** This is a
/// cheap text-level approximation with no elaborator: any tactic that consults
/// the local context without naming a hypothesis (`omega`, `simp_all`, `tauto`,
/// `assumption`, `aesop`, `decide`, `linarith` with no arguments, …) uses
/// hypotheses this pass will report as unused. Callers SHOULD treat a report as
/// a review queue and prefer the linter's answer whenever a toolchain is
/// available.
///
/// Deterministic: results follow bundle declaration order.
pub fn detect_unused_hypotheses(
    bundle: &HypothesisBundle,
    proof_body: &str,
) -> Vec<UnusedHypothesis> {
    let body: Vec<char> = proof_body.chars().collect();
    let mut out = Vec::new();

    for f in bundle.hypotheses() {
        if occurs_as_token(&body, &f.binder) {
            continue;
        }
        let vars = mentioned_idents(&f.text);
        // Variables of this hypothesis that are DATA binders of the same bundle.
        let data_vars: Vec<String> = bundle
            .data()
            .map(|d| d.binder.clone())
            .filter(|d| vars.iter().any(|v| v == d))
            .collect();

        let class = if data_vars.is_empty() {
            UnusedClass::Unknown
        } else if data_vars.iter().all(|d| occurs_as_token(&body, d)) {
            UnusedClass::PossiblyStronger
        } else {
            UnusedClass::PossiblyMisformalized
        };

        let detail = match class {
            UnusedClass::PossiblyStronger => format!(
                "hypothesis `{}` : {} is never referenced by the proof, though the variables it \
                 constrains are — the proof may establish a STRONGER theorem than requested. \
                 Advisory only; confirm with Lean's `linter.unusedSectionVars` (a context-\
                 consulting tactic may use it without naming it).",
                f.binder,
                norm_ws(&f.text)
            ),
            UnusedClass::PossiblyMisformalized => format!(
                "hypothesis `{}` : {} is never referenced by the proof, and neither are the \
                 variables it constrains — the clause is inert, suggesting a MIS-FORMALIZED \
                 statement. Advisory only; confirm with Lean's `linter.unusedSectionVars`.",
                f.binder,
                norm_ws(&f.text)
            ),
            UnusedClass::Unknown => format!(
                "hypothesis `{}` : {} is never referenced by the proof; it constrains no data \
                 binder of this bundle, so stronger-vs-mis-formalized cannot be distinguished \
                 at text level. Advisory only; confirm with Lean's \
                 `linter.unusedSectionVars`.",
                f.binder,
                norm_ws(&f.text)
            ),
        };

        out.push(UnusedHypothesis {
            binder: f.binder.clone(),
            text: norm_ws(&f.text),
            class,
            detail,
        });
    }

    out
}

/// Whether `needle` occurs in `chars` as a whole token (not inside a longer
/// identifier). `.` is NOT a word char, so `h` matches inside `h.symm`.
fn occurs_as_token(chars: &[char], needle: &str) -> bool {
    let n: Vec<char> = needle.chars().collect();
    if n.is_empty() || chars.len() < n.len() {
        return false;
    }
    let mut i = 0usize;
    while i + n.len() <= chars.len() {
        if chars[i..i + n.len()] == n[..] {
            let before_ok = i == 0 || !is_word(chars[i - 1]);
            let after_ok = chars.get(i + n.len()).map_or(true, |&c| !is_word(c));
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// The distinct identifier-like tokens in `text`, in order of first appearance.
/// Numeric literals are excluded.
fn mentioned_idents(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if is_word(c) {
            cur.push(c);
        } else if !cur.is_empty() {
            push_ident(&mut out, std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        push_ident(&mut out, cur);
    }
    out
}

fn push_ident(out: &mut Vec<String>, s: String) {
    let starts_ok = s
        .chars()
        .next()
        .map_or(false, |c| c.is_alphabetic() || c == '_');
    if starts_ok && !out.contains(&s) {
        out.push(s);
    }
}

// ===========================================================================
// Shared lexical helpers
// ===========================================================================

/// A word char for token-boundary tests (NOT including `.`, so `h` still
/// boundary-matches inside `h.symm`). Subscripted binders like `h₁` are
/// alphanumeric in Unicode and therefore word chars.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '\'' || c == '₀' || c == '₁' || c == '₂'
}

/// Collapse all runs of whitespace to a single space and trim.
fn norm_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json as jv;

    fn plausible_bundle() -> HypothesisBundle {
        HypothesisBundle::new(
            "thm_pos",
            vec![
                HypothesisField::datum("n", "Nat"),
                HypothesisField::hypothesis("hn", "n > 5"),
            ],
        )
    }

    // --- refutation: contradictory numeric bounds --------------------------

    #[test]
    fn contradictory_numeric_bounds_are_refuted() {
        let bundle = HypothesisBundle::new(
            "thm_vacuous",
            vec![
                HypothesisField::datum("x", "Nat"),
                HypothesisField::hypothesis("h1", "x > 5"),
                HypothesisField::hypothesis("h2", "x < 3"),
            ],
        );
        let cs = refute_bundle(&bundle);
        assert!(
            cs.iter().any(|c| c.rule == "numeric_bounds"),
            "x > 5 with x < 3 must be refuted: {cs:?}"
        );

        // Even WITH a witness the bundle is rejected — no instance can exist.
        let w = SatisfiabilityWitness::new("x := 7")
            .bind("x", jv!(7))
            .claim("h1")
            .claim("h2");
        let report = check_vacuity(&bundle, Some(&w));
        assert!(!report.clean, "refuted bundle can never be clean");
        assert_eq!(report.verdict, WitnessVerdict::WitnessRejected);
        assert!(report
            .finding_strings()
            .iter()
            .any(|s| s.contains("CRITICAL") && s.contains("numeric_bounds")));
        assert!(!report.into_scan_report().clean);
    }

    #[test]
    fn touching_bounds_are_refuted_only_when_strict() {
        // x ≥ 4 and x ≤ 4 is satisfiable (x = 4) — must NOT be refuted.
        let ok = HypothesisBundle::new(
            "g",
            vec![
                HypothesisField::hypothesis("a", "x >= 4"),
                HypothesisField::hypothesis("b", "x <= 4"),
            ],
        );
        assert!(refute_bundle(&ok).is_empty(), "x ≥ 4 ∧ x ≤ 4 is satisfiable");

        // x > 4 and x ≤ 4 is not.
        let bad = HypothesisBundle::new(
            "g",
            vec![
                HypothesisField::hypothesis("a", "x > 4"),
                HypothesisField::hypothesis("b", "x <= 4"),
            ],
        );
        assert!(bad_has(&bad, "numeric_bounds"));
    }

    #[test]
    fn conflicting_equalities_are_refuted() {
        let bundle = HypothesisBundle::new(
            "g",
            vec![
                HypothesisField::hypothesis("a", "x = 1"),
                HypothesisField::hypothesis("b", "x = 2"),
            ],
        );
        assert!(bad_has(&bundle, "numeric_bounds"));
    }

    #[test]
    fn flipped_comparison_is_normalized() {
        // `5 < x` ⇔ `x > 5`, contradicting `x < 3`.
        let bundle = HypothesisBundle::new(
            "g",
            vec![
                HypothesisField::hypothesis("a", "5 < x"),
                HypothesisField::hypothesis("b", "x < 3"),
            ],
        );
        assert!(bad_has(&bundle, "numeric_bounds"));
    }

    #[test]
    fn compound_hypotheses_are_left_unanalyzed() {
        // Not a simple comparison: the detector must stay silent rather than
        // guess. (Silence is NOT a satisfiability claim.)
        let bundle = HypothesisBundle::new(
            "g",
            vec![
                HypothesisField::hypothesis("a", "x > 5 ∨ x < 3"),
                HypothesisField::hypothesis("b", "x < 3"),
            ],
        );
        assert!(refute_bundle(&bundle).is_empty());
    }

    // --- refutation: False / negation --------------------------------------

    #[test]
    fn false_hypothesis_is_refuted() {
        for prop in ["False", "⊥", "Empty", "(False)"] {
            let bundle = HypothesisBundle::new(
                "g",
                vec![HypothesisField::hypothesis("hf", prop)],
            );
            let cs = refute_bundle(&bundle);
            assert!(
                cs.iter().any(|c| c.rule == "false_hypothesis"),
                "`{prop}` must be refuted: {cs:?}"
            );
            let w = SatisfiabilityWitness::new("anything").claim("hf");
            assert!(!check_vacuity(&bundle, Some(&w)).clean);
        }
    }

    #[test]
    fn literal_negation_pair_is_refuted() {
        let bundle = HypothesisBundle::new(
            "g",
            vec![
                HypothesisField::hypothesis("hp", "Prime p"),
                HypothesisField::hypothesis("hnp", "¬ Prime p"),
            ],
        );
        let cs = refute_bundle(&bundle);
        assert!(
            cs.iter().any(|c| c.rule == "literal_negation"),
            "P with ¬P must be refuted: {cs:?}"
        );
    }

    #[test]
    fn unrelated_negation_is_not_refuted() {
        let bundle = HypothesisBundle::new(
            "g",
            vec![
                HypothesisField::hypothesis("hp", "Prime p"),
                HypothesisField::hypothesis("hnq", "¬ Prime q"),
            ],
        );
        assert!(refute_bundle(&bundle).is_empty());
    }

    // --- witness: clean, missing (fail closed), rejected --------------------

    #[test]
    fn plausible_bundle_with_witness_is_clean() {
        let bundle = plausible_bundle();
        let w = SatisfiabilityWitness::new("n := 7")
            .bind("n", jv!(7))
            .claim("hn");
        let report = check_vacuity(&bundle, Some(&w));
        assert!(report.clean, "witnessed bundle must be clean: {report:?}");
        assert_eq!(report.verdict, WitnessVerdict::WitnessSupplied);
        assert!(report.findings.is_empty());
        assert!(report.into_scan_report().clean);
    }

    #[test]
    fn bundle_without_witness_is_not_clean() {
        // THE fail-closed case: nothing is syntactically wrong with `n > 5`, but
        // no instance was exhibited, so the gate stays shut.
        let bundle = plausible_bundle();
        let report = check_vacuity(&bundle, None);
        assert!(
            !report.clean,
            "absence of a witness must fail closed, got: {report:?}"
        );
        assert_eq!(report.verdict, WitnessVerdict::WitnessMissing);
        assert!(report
            .findings
            .iter()
            .any(|f| f.rule == "witness_missing" && f.severity == VacuitySeverity::Critical));
    }

    #[test]
    fn empty_refutation_never_implies_satisfiable() {
        // Guard rail on the module's core invariant: a bundle we cannot refute is
        // still not clean without a witness.
        let bundle = HypothesisBundle::new(
            "riemann",
            vec![HypothesisField::hypothesis("h", "RiemannHypothesis")],
        );
        assert!(refute_bundle(&bundle).is_empty());
        assert!(!check_vacuity(&bundle, None).clean);
    }

    #[test]
    fn trivial_bundle_needs_no_witness() {
        let bundle = HypothesisBundle::new("g", vec![HypothesisField::datum("n", "Nat")]);
        assert!(bundle.is_trivial());
        let report = check_vacuity(&bundle, None);
        assert!(report.clean);
        assert_eq!(report.verdict, WitnessVerdict::WitnessSupplied);
    }

    #[test]
    fn incomplete_witness_is_rejected() {
        let bundle = plausible_bundle();
        // Data binder `n` unbound.
        let w = SatisfiabilityWitness::new("?").claim("hn");
        let r = check_vacuity(&bundle, Some(&w));
        assert!(!r.clean);
        assert_eq!(r.verdict, WitnessVerdict::WitnessRejected);
        assert!(r.findings.iter().any(|f| f.rule == "witness_incomplete"));

        // Hypothesis `hn` unclaimed.
        let w = SatisfiabilityWitness::new("n := 7").bind("n", jv!(7));
        let r = check_vacuity(&bundle, Some(&w));
        assert!(!r.clean);
        assert!(r.findings.iter().any(|f| f.rule == "witness_incomplete"));
    }

    #[test]
    fn witness_violating_a_field_is_rejected() {
        let bundle = plausible_bundle(); // hn : n > 5
        let w = SatisfiabilityWitness::new("n := 2")
            .bind("n", jv!(2))
            .claim("hn");
        let r = check_vacuity(&bundle, Some(&w));
        assert!(!r.clean, "n := 2 does not satisfy n > 5");
        assert_eq!(r.verdict, WitnessVerdict::WitnessRejected);
        assert!(r
            .findings
            .iter()
            .any(|f| f.rule == "witness_violates_field"));
    }

    #[test]
    fn check_is_deterministic() {
        let bundle = HypothesisBundle::new(
            "g",
            vec![
                HypothesisField::datum("x", "Int"),
                HypothesisField::hypothesis("h1", "x > 5"),
                HypothesisField::hypothesis("h2", "x < 3"),
                HypothesisField::hypothesis("h3", "False"),
            ],
        );
        let a = check_vacuity(&bundle, None);
        let b = check_vacuity(&bundle, None);
        assert_eq!(a, b);
        // Findings are ordered by (rule, fields).
        let keys: Vec<_> = a.findings.iter().map(|f| (f.rule, f.fields.clone())).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }

    // --- unused hypotheses (advisory) --------------------------------------

    #[test]
    fn used_hypothesis_is_not_flagged() {
        let bundle = plausible_bundle();
        let body = "by exact Nat.lt_of_lt_of_le (by norm_num) hn";
        assert!(
            detect_unused_hypotheses(&bundle, body).is_empty(),
            "a referenced hypothesis must not be flagged"
        );
    }

    #[test]
    fn hypothesis_used_via_projection_is_not_flagged() {
        let bundle = plausible_bundle();
        // `hn.le` references `hn` — `.` is not a word char, so the token matches.
        assert!(detect_unused_hypotheses(&bundle, "by exact hn.le").is_empty());
    }

    #[test]
    fn prefix_collision_does_not_count_as_use() {
        let bundle = plausible_bundle(); // binder `hn`
        // `hnx` is a different identifier; `hn` is still unused.
        let found = detect_unused_hypotheses(&bundle, "by simpa using hnx");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].binder, "hn");
    }

    #[test]
    fn unused_hypothesis_classified_possibly_stronger() {
        // `n` is used by the proof, but the assumption `hn : n > 5` is not — the
        // result is plausibly stronger than requested.
        let bundle = plausible_bundle();
        let found = detect_unused_hypotheses(&bundle, "by simp [Nat.succ_le, n]");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].binder, "hn");
        assert_eq!(found[0].class, UnusedClass::PossiblyStronger);
        assert!(found[0].detail.contains("linter.unusedSectionVars"));
    }

    #[test]
    fn unused_hypothesis_classified_possibly_misformalized() {
        // Neither the coprimality hypothesis nor the variables it constrains are
        // touched — the clause is inert (the real-corpus pattern).
        let bundle = HypothesisBundle::new(
            "thm",
            vec![
                HypothesisField::datum("a", "Nat"),
                HypothesisField::datum("b", "Nat"),
                HypothesisField::hypothesis("hab", "Nat.Coprime a b"),
            ],
        );
        let found = detect_unused_hypotheses(&bundle, "by rfl");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].class, UnusedClass::PossiblyMisformalized);
    }

    #[test]
    fn unused_hypothesis_with_no_bundle_variables_is_unknown() {
        let bundle = HypothesisBundle::new(
            "thm",
            vec![HypothesisField::hypothesis("inst", "DecidableEq α")],
        );
        let found = detect_unused_hypotheses(&bundle, "by rfl");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].class, UnusedClass::Unknown);
    }

    #[test]
    fn unused_detection_never_gates() {
        // A bundle with a valid witness stays clean even though its hypothesis is
        // unused by the proof: advisory metadata must not affect `clean`.
        let bundle = plausible_bundle();
        let w = SatisfiabilityWitness::new("n := 7")
            .bind("n", jv!(7))
            .claim("hn");
        let report = check_vacuity(&bundle, Some(&w));
        assert!(report.clean);
        assert!(!detect_unused_hypotheses(&bundle, "by rfl").is_empty());
        // The report carries no unused-hypothesis finding at all.
        assert!(report.findings.is_empty());
    }

    fn bad_has(bundle: &HypothesisBundle, rule: &str) -> bool {
        refute_bundle(bundle).iter().any(|c| c.rule == rule)
    }
}
