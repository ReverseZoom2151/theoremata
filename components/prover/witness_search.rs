//! BOUNDED SATISFIABILITY-WITNESS SEARCH — the producer side of the
//! vacuous-success gate ([`crate::prover::vacuity`]).
//!
//! # The central invariant
//!
//! **This searcher may only ever return a witness it has itself CHECKED against
//! EVERY field of the bundle.** There is no partial credit and no benefit of the
//! doubt:
//!
//! * If ANY field — hypothesis or data binder — falls outside the decidable
//!   fragment described below, the answer is
//!   [`NotDecidable`](WitnessSearch::NotDecidable). It is **never** a witness
//!   that covers only the fields we understood.
//! * A partial witness would be actively worse than no witness at all. Vacuity's
//!   own audit ([`check_vacuity`](crate::prover::vacuity::check_vacuity)) accepts
//!   any claim it cannot evaluate on the caller's word, so a witness claiming
//!   fields we never checked would make the gate report "satisfiable" for a
//!   bundle whose UNPARSED field is precisely the contradictory one. That is
//!   exactly the hollow success the gate exists to catch, laundered through the
//!   gate's own defense. Hence: understand every field, or decide nothing.
//!
//! # The other direction: [`NoWitnessInBounds`] is NOT unsatisfiability
//!
//! [`NoWitnessInBounds`](WitnessSearch::NoWitnessInBounds) means "the finite grid
//! this searcher enumerated contained no satisfying assignment". It is **not**
//! evidence — not weak evidence, not a hint — that the bundle is unsatisfiable:
//!
//! * the grid is bounded by [`SearchBounds::max_abs_value`], so `x > 10^6` has a
//!   solution the default search will never see;
//! * it is bounded again by [`SearchBounds::max_candidates`], so a wide bundle is
//!   abandoned part-way (see [`WitnessSearch::exhausted`]);
//! * over `ℚ`/`ℝ` the grid enumerates INTEGERS ONLY, so `0 < x ∧ x < 1` is
//!   satisfiable yet unfound.
//!
//! A caller that treated `NoWitnessInBounds` as "unsatisfiable ⇒ vacuous ⇒
//! reject" would reject perfectly good goals. The only correct treatment is the
//! same as [`NotDecidable`](WitnessSearch::NotDecidable): *we produced no
//! witness*. Refutation is [`refute_bundle`](crate::prover::vacuity::refute_bundle)'s
//! job, not this module's — this module only ever ADDS the affirmative exhibit.
//!
//! # The decidable fragment
//!
//! A bundle is decidable when every field is:
//!
//! **Data binders** — a type naming a domain we can enumerate:
//! `Nat`/`ℕ` (0, 1, 2, …), `Int`/`ℤ` (0, 1, −1, 2, −2, …), `Fin k` for a literal
//! `k` (0 … k−1), or `Rat`/`ℚ`/`Real`/`ℝ` (enumerated as INTEGERS — a sound but
//! incomplete subset: an integer solution really is a rational/real solution).
//! Any other type (`Type`, `α`, `List Nat`, `Fintype β`, …) ⇒ `NotDecidable`.
//!
//! **Hypotheses** — a conjunction (`∧`, `/\`) of, and negations (`¬`, `~`,
//! `Not`) of, these atoms, over integer-linear expressions built from `+`, `-`,
//! `*` (at most one variable per product), integer literals and declared data
//! binders:
//!
//! | form | example |
//! |---|---|
//! | linear comparison `<`, `≤`, `>`, `≥`, `=`, `≠` | `x > 5`, `a = b + 1`, `2 * n <= m - 3` |
//! | divisibility `d ∣ e` (literal `d ≠ 0`) | `3 ∣ n`, `2 ∣ (a + b)` |
//! | parity | `Even n`, `Odd (k + 1)` |
//! | modular equation `e % m = r` | `n % 4 = 1` |
//! | finite-range membership | `x ∈ Finset.range 10`, `k ∈ Set.Icc 1 5`, `Finset.Ico`/`Ioc`/`Ioo` |
//!
//! Everything else — disjunction, implication, quantifiers, `Nat.Prime`,
//! `Nat.Coprime`, function application, non-linear products, a variable not
//! declared as a data binder of the same bundle, a divisibility/parity/modular
//! constraint on a `ℚ`/`ℝ` binder — makes the WHOLE bundle `NotDecidable`.
//!
//! # Determinism and purity
//!
//! No IO, no clock, no RNG, no `HashMap` iteration. Variables are enumerated in
//! sorted binder order as a fixed-radix odometer (last variable varies fastest),
//! candidate values in a fixed per-domain order. Same bundle + same bounds ⇒
//! byte-identical result. `std` + `serde`/`serde_json` only.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

use crate::prover::vacuity::{FieldKind, HypothesisBundle, SatisfiabilityWitness};

// ===========================================================================
// Public API
// ===========================================================================

/// Explicit, caller-visible bounds on the search. Both are hard caps: the
/// searcher never enumerates outside them and never raises them itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchBounds {
    /// Largest absolute value any variable may take. `Nat` ranges over
    /// `0..=max_abs_value`; `Int` over `0, ±1, … ±max_abs_value`.
    pub max_abs_value: i64,
    /// Hard cap on the number of complete assignments EVALUATED. Reaching it
    /// stops the search with [`WitnessSearch::NoWitnessInBounds`] and
    /// `exhausted == false`.
    pub max_candidates: usize,
}

impl SearchBounds {
    /// `max_abs_value = 20`, `max_candidates = 100_000` — enough for the
    /// small-integer bundles that appear in practice, small enough to stay
    /// well under a millisecond.
    pub const DEFAULT: SearchBounds = SearchBounds {
        max_abs_value: 20,
        max_candidates: 100_000,
    };
}

impl Default for SearchBounds {
    fn default() -> Self {
        SearchBounds::DEFAULT
    }
}

/// The THREE-way outcome. These are three different facts and must never be
/// collapsed into a boolean: only the first is a witness, and the other two are
/// *not* interchangeable (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub enum WitnessSearch {
    /// An assignment was found AND verified against every field of the bundle.
    /// The witness binds every data binder and claims every hypothesis.
    WitnessFound(SatisfiabilityWitness),
    /// Every field was understood, but no assignment inside the bounds satisfied
    /// them all. **NOT evidence of unsatisfiability.**
    NoWitnessInBounds {
        /// How many complete assignments were evaluated.
        candidates_tried: usize,
        /// `true` if the whole bounded grid was enumerated; `false` if
        /// [`SearchBounds::max_candidates`] cut the search short.
        exhausted: bool,
    },
    /// At least one field lies outside the decidable fragment, so NO claim is
    /// made about the bundle in either direction.
    NotDecidable {
        /// Which field defeated the parser, and why.
        reason: String,
    },
}

impl WitnessSearch {
    /// The witness, if one was found and verified. `None` for BOTH other
    /// outcomes — this is the mapping a backend's
    /// [`satisfiability_witness`](crate::prover::formal::FormalBackend::satisfiability_witness)
    /// wants, since neither `NoWitnessInBounds` nor `NotDecidable` is a witness
    /// and vacuity fails closed on `None`.
    pub fn into_witness(self) -> Option<SatisfiabilityWitness> {
        match self {
            WitnessSearch::WitnessFound(w) => Some(w),
            _ => None,
        }
    }

    /// Borrowing form of [`into_witness`](WitnessSearch::into_witness).
    pub fn witness(&self) -> Option<&SatisfiabilityWitness> {
        match self {
            WitnessSearch::WitnessFound(w) => Some(w),
            _ => None,
        }
    }

    /// Whether the whole bounded grid was enumerated without success. Only ever
    /// `true` for `NoWitnessInBounds`, and STILL not a proof of unsatisfiability
    /// (the grid is bounded).
    pub fn exhausted(&self) -> bool {
        matches!(
            self,
            WitnessSearch::NoWitnessInBounds {
                exhausted: true,
                ..
            }
        )
    }

    /// Stable snake_case tag for logs and JSON detail.
    pub fn tag(&self) -> &'static str {
        match self {
            WitnessSearch::WitnessFound(_) => "witness_found",
            WitnessSearch::NoWitnessInBounds { .. } => "no_witness_in_bounds",
            WitnessSearch::NotDecidable { .. } => "not_decidable",
        }
    }
}

/// Search for a satisfiability witness with [`SearchBounds::DEFAULT`].
pub fn search_witness(bundle: &HypothesisBundle) -> WitnessSearch {
    search_witness_bounded(bundle, SearchBounds::DEFAULT)
}

/// Search for a satisfiability witness under explicit bounds.
///
/// Honest by construction: returns [`WitnessFound`](WitnessSearch::WitnessFound)
/// only after evaluating the parsed form of EVERY hypothesis to `true` under a
/// single assignment that also gives EVERY data binder a value in its declared
/// domain. See the module docs for the invariant and for why the two failure
/// outcomes are distinct.
pub fn search_witness_bounded(bundle: &HypothesisBundle, bounds: SearchBounds) -> WitnessSearch {
    // ---- 1. Every DATA binder must name a domain we can enumerate. ----------
    let mut domains: BTreeMap<String, Domain> = BTreeMap::new();
    for f in bundle.fields.iter().filter(|f| f.kind == FieldKind::Datum) {
        match parse_domain(&f.text) {
            Some(d) => {
                domains.insert(f.binder.clone(), d);
            }
            None => {
                return not_decidable(format!(
                    "data binder `{}` : {} names a domain this searcher cannot enumerate \
                     (supported: Nat/ℕ, Int/ℤ, Fin <literal>, Rat/ℚ, Real/ℝ). Refusing to \
                     guess a value for it — a witness that skipped this binder would not be \
                     a witness.",
                    f.binder,
                    norm_ws(&f.text)
                ));
            }
        }
    }

    // ---- 2. Every HYPOTHESIS must parse into the decidable fragment. --------
    let mut constraints: Vec<(String, Constraint)> = Vec::new();
    for f in bundle
        .fields
        .iter()
        .filter(|f| f.kind == FieldKind::Hypothesis)
    {
        let text = norm_ws(&f.text);
        let c = match parse_constraint(&text) {
            Ok(c) => c,
            Err(why) => {
                return not_decidable(format!(
                    "hypothesis `{}` : {} is outside the decidable fragment ({why}). The whole \
                     bundle is NotDecidable: a witness covering only the other fields could \
                     certify a bundle whose contradiction lives in THIS field.",
                    f.binder, text
                ));
            }
        };
        // Every variable mentioned must be a declared data binder — otherwise we
        // do not know its domain and cannot enumerate it.
        let mut vars = BTreeSet::new();
        c.collect_vars(&mut vars);
        for v in &vars {
            let Some(dom) = domains.get(v) else {
                return not_decidable(format!(
                    "hypothesis `{}` : {} mentions `{v}`, which is not a data binder of this \
                     bundle, so its domain is unknown and it cannot be instantiated.",
                    f.binder, text
                ));
            };
            if c.needs_integrality() && !dom.is_integral() {
                return not_decidable(format!(
                    "hypothesis `{}` : {} imposes a divisibility/parity/modular constraint on \
                     `{v}`, whose declared domain {} is not integral.",
                    f.binder,
                    text,
                    dom.describe()
                ));
            }
        }
        constraints.push((f.binder.clone(), c));
    }

    // ---- 3. Bounded, deterministic grid search. ----------------------------
    // Odometer over variables in sorted binder order; the LAST variable varies
    // fastest. Candidate lists are fixed per domain. No RNG, no clock.
    let names: Vec<String> = domains.keys().cloned().collect();
    let value_lists: Vec<Vec<i64>> = names
        .iter()
        .map(|n| domains[n].candidates(bounds.max_abs_value))
        .collect();

    // An empty candidate list (e.g. `Fin 0`) means the grid itself is empty.
    if value_lists.iter().any(|v| v.is_empty()) {
        return WitnessSearch::NoWitnessInBounds {
            candidates_tried: 0,
            exhausted: true,
        };
    }

    let mut idx = vec![0usize; names.len()];
    let mut assignment: BTreeMap<String, i64> = BTreeMap::new();
    let mut tried = 0usize;

    loop {
        if tried >= bounds.max_candidates {
            return WitnessSearch::NoWitnessInBounds {
                candidates_tried: tried,
                exhausted: false,
            };
        }

        assignment.clear();
        for (i, name) in names.iter().enumerate() {
            assignment.insert(name.clone(), value_lists[i][idx[i]]);
        }
        tried += 1;

        // THE CHECK: every constraint must evaluate to true under this ONE
        // assignment. An evaluation error (arithmetic overflow) is not a
        // failure of the candidate — it means we cannot decide, so we say so.
        let mut all_hold = true;
        for (binder, c) in &constraints {
            match c.eval(&assignment) {
                Ok(true) => {}
                Ok(false) => {
                    all_hold = false;
                    break;
                }
                Err(why) => {
                    return not_decidable(format!(
                        "hypothesis `{binder}` could not be evaluated ({why}); refusing to \
                         report a witness that was not fully checked."
                    ));
                }
            }
        }

        if all_hold {
            return WitnessSearch::WitnessFound(build_witness(bundle, &assignment));
        }

        // Odometer increment; wrap-around on the leading digit ends the grid.
        let mut pos = names.len();
        let mut carried = true;
        while pos > 0 {
            pos -= 1;
            idx[pos] += 1;
            if idx[pos] < value_lists[pos].len() {
                carried = false;
                break;
            }
            idx[pos] = 0;
        }
        if carried {
            // Includes the zero-variable case: exactly one (empty) assignment.
            return WitnessSearch::NoWitnessInBounds {
                candidates_tried: tried,
                exhausted: true,
            };
        }
    }
}

fn not_decidable(reason: String) -> WitnessSearch {
    WitnessSearch::NotDecidable { reason }
}

/// Build the witness for a VERIFIED assignment: bind every data binder, claim
/// every hypothesis. Both are total over `bundle.fields`, which is what makes
/// vacuity's completeness audit pass.
fn build_witness(bundle: &HypothesisBundle, assignment: &BTreeMap<String, i64>) -> SatisfiabilityWitness {
    let label = if assignment.is_empty() {
        "(no data binders)".to_string()
    } else {
        assignment
            .iter()
            .map(|(k, v)| format!("{k} := {v}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut w = SatisfiabilityWitness::new(label);
    for f in bundle.fields.iter() {
        match f.kind {
            FieldKind::Datum => {
                let v = assignment
                    .get(&f.binder)
                    .copied()
                    .expect("every data binder is a search variable");
                w = w.bind(f.binder.clone(), Value::from(v));
            }
            FieldKind::Hypothesis => {
                w = w.claim(f.binder.clone());
            }
        }
    }
    w
}

// ===========================================================================
// Domains
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Domain {
    /// `Nat` / `ℕ`.
    Nat,
    /// `Int` / `ℤ`.
    Int,
    /// `Fin k` — exactly `0..k`.
    Fin(i64),
    /// `Rat`/`ℚ`/`Real`/`ℝ`, enumerated over INTEGERS ONLY. Sound (an integer is
    /// a rational) but incomplete (misses `1/2`), which is why failure is only
    /// ever `NoWitnessInBounds`.
    IntegerSubsetOfField,
}

impl Domain {
    /// Whether divisibility / parity / `%` are meaningful here.
    fn is_integral(self) -> bool {
        matches!(self, Domain::Nat | Domain::Int | Domain::Fin(_))
    }

    fn describe(self) -> &'static str {
        match self {
            Domain::Nat => "Nat",
            Domain::Int => "Int",
            Domain::Fin(_) => "Fin",
            Domain::IntegerSubsetOfField => "a field (ℚ/ℝ)",
        }
    }

    /// Fixed enumeration order. Deterministic, small values first.
    fn candidates(self, max_abs: i64) -> Vec<i64> {
        let max_abs = max_abs.max(0);
        match self {
            Domain::Nat => (0..=max_abs).collect(),
            Domain::Fin(k) => (0..k.max(0).min(max_abs.saturating_add(1))).collect(),
            Domain::Int | Domain::IntegerSubsetOfField => {
                let mut out = vec![0i64];
                for v in 1..=max_abs {
                    out.push(v);
                    out.push(-v);
                }
                out
            }
        }
    }
}

fn parse_domain(text: &str) -> Option<Domain> {
    let t = strip_outer_parens(&norm_ws(text)).to_string();
    match t.as_str() {
        "Nat" | "ℕ" | "Nat.succ" => return Some(Domain::Nat),
        "Int" | "ℤ" => return Some(Domain::Int),
        "Rat" | "ℚ" | "Real" | "ℝ" => return Some(Domain::IntegerSubsetOfField),
        _ => {}
    }
    if let Some(rest) = t.strip_prefix("Fin ") {
        if let Some(k) = as_int(rest.trim()) {
            if k >= 0 {
                return Some(Domain::Fin(k));
            }
        }
    }
    None
}

// ===========================================================================
// Constraint AST
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cmp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

/// An integer-linear expression: `constant + Σ coeff·var`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Lin {
    constant: i64,
    terms: BTreeMap<String, i64>,
}

impl Lin {
    fn constant(c: i64) -> Self {
        Lin {
            constant: c,
            terms: BTreeMap::new(),
        }
    }

    fn var(name: &str) -> Self {
        let mut terms = BTreeMap::new();
        terms.insert(name.to_string(), 1);
        Lin {
            constant: 0,
            terms,
        }
    }

    fn is_constant(&self) -> bool {
        self.terms.is_empty()
    }

    fn scale(&self, k: i64) -> Result<Lin, String> {
        let mut out = Lin {
            constant: self
                .constant
                .checked_mul(k)
                .ok_or_else(|| "coefficient overflow".to_string())?,
            terms: BTreeMap::new(),
        };
        for (v, c) in &self.terms {
            let nc = c
                .checked_mul(k)
                .ok_or_else(|| "coefficient overflow".to_string())?;
            if nc != 0 {
                out.terms.insert(v.clone(), nc);
            }
        }
        Ok(out)
    }

    fn add(&self, other: &Lin, sign: i64) -> Result<Lin, String> {
        let scaled = other.scale(sign)?;
        let mut out = self.clone();
        out.constant = out
            .constant
            .checked_add(scaled.constant)
            .ok_or_else(|| "constant overflow".to_string())?;
        for (v, c) in scaled.terms {
            let e = out.terms.entry(v).or_insert(0);
            *e = e
                .checked_add(c)
                .ok_or_else(|| "coefficient overflow".to_string())?;
        }
        out.terms.retain(|_, c| *c != 0);
        Ok(out)
    }

    fn mul(&self, other: &Lin) -> Result<Lin, String> {
        if self.is_constant() {
            other.scale(self.constant)
        } else if other.is_constant() {
            self.scale(other.constant)
        } else {
            Err("non-linear product of two variable expressions".to_string())
        }
    }

    fn eval(&self, a: &BTreeMap<String, i64>) -> Result<i128, String> {
        let mut acc = self.constant as i128;
        for (v, c) in &self.terms {
            let val = *a
                .get(v)
                .ok_or_else(|| format!("variable `{v}` is unassigned"))?;
            acc = acc
                .checked_add((*c as i128).checked_mul(val as i128).ok_or("overflow")?)
                .ok_or("overflow")?;
        }
        Ok(acc)
    }

    fn collect_vars(&self, out: &mut BTreeSet<String>) {
        for v in self.terms.keys() {
            out.insert(v.clone());
        }
    }

    /// Guard against absurd coefficients that would make evaluation fragile.
    fn check_magnitude(&self) -> Result<(), String> {
        const LIMIT: i64 = 1_000_000;
        if self.constant.abs() > LIMIT || self.terms.values().any(|c| c.abs() > LIMIT) {
            return Err("coefficient magnitude exceeds the searcher's limit".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Constraint {
    Compare(Lin, Cmp, Lin),
    /// `divisor ∣ expr`, `divisor != 0`.
    Divides { divisor: i64, expr: Lin },
    /// `expr % modulus == residue`, Euclidean (non-negative) residue, matching
    /// Lean's `Nat.mod` / `Int.emod` for a positive modulus.
    ModEq {
        expr: Lin,
        modulus: i64,
        residue: i64,
    },
    Not(Box<Constraint>),
    And(Vec<Constraint>),
}

impl Constraint {
    fn collect_vars(&self, out: &mut BTreeSet<String>) {
        match self {
            Constraint::Compare(a, _, b) => {
                a.collect_vars(out);
                b.collect_vars(out);
            }
            Constraint::Divides { expr, .. } | Constraint::ModEq { expr, .. } => {
                expr.collect_vars(out)
            }
            Constraint::Not(c) => c.collect_vars(out),
            Constraint::And(cs) => cs.iter().for_each(|c| c.collect_vars(out)),
        }
    }

    /// Whether this constraint is only meaningful over an integral domain.
    fn needs_integrality(&self) -> bool {
        match self {
            Constraint::Compare(..) => false,
            Constraint::Divides { .. } | Constraint::ModEq { .. } => true,
            Constraint::Not(c) => c.needs_integrality(),
            Constraint::And(cs) => cs.iter().any(|c| c.needs_integrality()),
        }
    }

    fn eval(&self, a: &BTreeMap<String, i64>) -> Result<bool, String> {
        Ok(match self {
            Constraint::Compare(l, op, r) => {
                let (lv, rv) = (l.eval(a)?, r.eval(a)?);
                match op {
                    Cmp::Lt => lv < rv,
                    Cmp::Le => lv <= rv,
                    Cmp::Gt => lv > rv,
                    Cmp::Ge => lv >= rv,
                    Cmp::Eq => lv == rv,
                    Cmp::Ne => lv != rv,
                }
            }
            Constraint::Divides { divisor, expr } => {
                let v = expr.eval(a)?;
                v.rem_euclid(*divisor as i128) == 0
            }
            Constraint::ModEq {
                expr,
                modulus,
                residue,
            } => {
                let v = expr.eval(a)?;
                let m = *modulus as i128;
                v.rem_euclid(m) == (*residue as i128).rem_euclid(m)
            }
            Constraint::Not(c) => !c.eval(a)?,
            Constraint::And(cs) => {
                for c in cs {
                    if !c.eval(a)? {
                        return Ok(false);
                    }
                }
                true
            }
        })
    }
}

// ===========================================================================
// Parser — refuses everything it does not fully understand
// ===========================================================================

/// Parse a hypothesis text into the decidable fragment, or explain why not.
fn parse_constraint(text: &str) -> Result<Constraint, String> {
    let t = strip_outer_parens(&norm_ws(text)).to_string();
    if t.is_empty() {
        return Err("empty proposition".to_string());
    }
    if !parens_balanced(&t) {
        return Err("unbalanced parentheses".to_string());
    }
    let chars: Vec<char> = t.chars().collect();

    // Connectives we deliberately do NOT model. Bail out loudly.
    for bad in ["∨", "\\/", "→", "->", "↔", "<->", "∀", "∃", "Σ", "λ"] {
        if split_top_level(&chars, &[bad]).len() > 1 {
            return Err(format!("contains the unsupported connective `{bad}`"));
        }
    }

    // Conjunction first (associates the whole proposition).
    let conjuncts = split_top_level(&chars, &["∧", "/\\"]);
    if conjuncts.len() > 1 {
        let mut out = Vec::with_capacity(conjuncts.len());
        for c in conjuncts {
            out.push(parse_constraint(&c)?);
        }
        return Ok(Constraint::And(out));
    }

    parse_atom(&t)
}

fn parse_atom(t: &str) -> Result<Constraint, String> {
    let t = strip_outer_parens(t).trim().to_string();
    let chars: Vec<char> = t.chars().collect();

    // --- negation --------------------------------------------------------
    for p in ["¬", "~"] {
        if let Some(rest) = t.strip_prefix(p) {
            return Ok(Constraint::Not(Box::new(parse_constraint(rest.trim())?)));
        }
    }
    if let Some(rest) = t.strip_prefix("Not ") {
        return Ok(Constraint::Not(Box::new(parse_constraint(rest.trim())?)));
    }

    // --- finite-range membership -----------------------------------------
    if split_top_level(&chars, &["∈"]).len() > 1 {
        return parse_membership(&chars);
    }

    // --- divisibility `d ∣ e` --------------------------------------------
    let dv = split_top_level(&chars, &["∣"]);
    if dv.len() > 1 {
        if dv.len() != 2 {
            return Err("chained divisibility is not supported".to_string());
        }
        let lhs = parse_lin(&dv[0])?;
        if !lhs.is_constant() {
            return Err("divisor must be an integer literal".to_string());
        }
        if lhs.constant == 0 {
            return Err("divisor is zero".to_string());
        }
        let expr = parse_lin(&dv[1])?;
        expr.check_magnitude()?;
        return Ok(Constraint::Divides {
            divisor: lhs.constant,
            expr,
        });
    }

    // --- parity ----------------------------------------------------------
    if let Some(rest) = t.strip_prefix("Even ") {
        let expr = parse_lin(rest)?;
        expr.check_magnitude()?;
        return Ok(Constraint::Divides { divisor: 2, expr });
    }
    if let Some(rest) = t.strip_prefix("Odd ") {
        let expr = parse_lin(rest)?;
        expr.check_magnitude()?;
        return Ok(Constraint::Not(Box::new(Constraint::Divides {
            divisor: 2,
            expr,
        })));
    }

    // --- comparison (and the `%` special case on its left) ----------------
    let (lhs, op, rhs) = split_comparison(&chars)?;

    let lhs_chars: Vec<char> = lhs.chars().collect();
    let modparts = split_top_level(&lhs_chars, &["%"]);
    if modparts.len() > 1 {
        if op != Cmp::Eq {
            return Err("`%` is only supported in an equation `e % m = r`".to_string());
        }
        if modparts.len() != 2 {
            return Err("chained `%` is not supported".to_string());
        }
        let expr = parse_lin(&modparts[0])?;
        let m = parse_lin(&modparts[1])?;
        let r = parse_lin(&rhs)?;
        if !m.is_constant() || m.constant <= 0 {
            return Err("modulus must be a positive integer literal".to_string());
        }
        if !r.is_constant() {
            return Err("residue must be an integer literal".to_string());
        }
        expr.check_magnitude()?;
        return Ok(Constraint::ModEq {
            expr,
            modulus: m.constant,
            residue: r.constant,
        });
    }
    if split_top_level(&rhs.chars().collect::<Vec<_>>(), &["%"]).len() > 1 {
        return Err("`%` on the right of a comparison is not supported".to_string());
    }

    let l = parse_lin(&lhs)?;
    let r = parse_lin(&rhs)?;
    l.check_magnitude()?;
    r.check_magnitude()?;
    Ok(Constraint::Compare(l, op, r))
}

/// `x ∈ Finset.range 10`, `x ∈ Set.Icc 1 5`, …
fn parse_membership(chars: &[char]) -> Result<Constraint, String> {
    let parts = split_top_level(chars, &["∈"]);
    if parts.len() != 2 {
        return Err("chained membership is not supported".to_string());
    }
    let elem = parse_lin(&parts[0])?;
    elem.check_magnitude()?;
    let set = strip_outer_parens(&parts[1]).to_string();

    let body = set
        .strip_prefix("Finset.")
        .or_else(|| set.strip_prefix("Set."))
        .ok_or_else(|| format!("unsupported set `{set}` (need Finset./Set. range|Icc|Ico|Ioc|Ioo)"))?;

    let mut it = body.split_whitespace();
    let ctor = it.next().unwrap_or("");
    let args: Vec<&str> = it.collect();
    let lit = |s: &str| -> Result<i64, String> {
        as_int(strip_outer_parens(s)).ok_or_else(|| format!("`{s}` is not an integer literal"))
    };

    // Each becomes a conjunction of two comparisons. `lo_strict`/`hi_strict`
    // follow the standard Mathlib naming: I(c|o)(c|o) = closed/open.
    let (lo, lo_strict, hi, hi_strict) = match (ctor, args.len()) {
        ("range", 1) => (0, false, lit(args[0])?, true),
        ("Icc", 2) => (lit(args[0])?, false, lit(args[1])?, false),
        ("Ico", 2) => (lit(args[0])?, false, lit(args[1])?, true),
        ("Ioc", 2) => (lit(args[0])?, true, lit(args[1])?, false),
        ("Ioo", 2) => (lit(args[0])?, true, lit(args[1])?, true),
        _ => {
            return Err(format!(
                "unsupported set constructor `{ctor}` with {} argument(s)",
                args.len()
            ))
        }
    };

    Ok(Constraint::And(vec![
        Constraint::Compare(
            Lin::constant(lo),
            if lo_strict { Cmp::Lt } else { Cmp::Le },
            elem.clone(),
        ),
        Constraint::Compare(
            elem,
            if hi_strict { Cmp::Lt } else { Cmp::Le },
            Lin::constant(hi),
        ),
    ]))
}

/// Split at the single top-level comparison operator.
fn split_comparison(chars: &[char]) -> Result<(String, Cmp, String), String> {
    // Longest first so `>=` never reads as `>`.
    const OPS: &[(&str, Cmp)] = &[
        ("≠", Cmp::Ne),
        ("!=", Cmp::Ne),
        (">=", Cmp::Ge),
        ("<=", Cmp::Le),
        ("≥", Cmp::Ge),
        ("≤", Cmp::Le),
        (">", Cmp::Gt),
        ("<", Cmp::Lt),
        ("=", Cmp::Eq),
    ];
    let mut found: Option<(usize, usize, Cmp)> = None;
    let mut depth = 0i32;
    let mut i = 0usize;
    'scan: while i < chars.len() {
        match chars[i] {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            // `:=` and `==` are not comparisons we model.
            if (chars[i] == ':' || chars[i] == '=') && chars.get(i + 1) == Some(&'=') {
                return Err("`:=` / `==` is not a supported comparison".to_string());
            }
            for (lit, op) in OPS {
                let l: Vec<char> = lit.chars().collect();
                if i + l.len() <= chars.len() && chars[i..i + l.len()] == l[..] {
                    if found.is_some() {
                        return Err("chained comparisons are not supported".to_string());
                    }
                    found = Some((i, i + l.len(), *op));
                    i += l.len();
                    continue 'scan;
                }
            }
        }
        i += 1;
    }
    let (s, e, op) = found.ok_or_else(|| {
        "not a comparison, divisibility, parity or membership atom".to_string()
    })?;
    let lhs: String = chars[..s].iter().collect();
    let rhs: String = chars[e..].iter().collect();
    if lhs.trim().is_empty() || rhs.trim().is_empty() {
        return Err("comparison is missing an operand".to_string());
    }
    Ok((lhs.trim().to_string(), op, rhs.trim().to_string()))
}

// --- linear expressions ----------------------------------------------------

fn parse_lin(s: &str) -> Result<Lin, String> {
    let s = strip_outer_parens(s.trim()).trim().to_string();
    if s.is_empty() {
        return Err("empty expression".to_string());
    }
    let chars: Vec<char> = s.chars().collect();

    // Split into signed terms at top level. A `+`/`-` is BINARY only when the
    // previous non-space char exists and is not itself an operator or `(`.
    let mut terms: Vec<(i64, String)> = Vec::new();
    let mut cur = String::new();
    let mut sign = 1i64;
    let mut depth = 0i32;
    let mut prev: Option<char> = None;
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if depth == 0 && (c == '+' || c == '-') {
            let unary = match prev {
                None => true,
                Some(p) => matches!(p, '+' | '-' | '*' | '(' | '%'),
            };
            if !unary {
                terms.push((sign, cur.trim().to_string()));
                cur.clear();
                sign = if c == '-' { -1 } else { 1 };
                prev = Some(c);
                continue;
            }
            if i == 0 || cur.trim().is_empty() {
                // Leading sign of the whole expression / of this term.
                if c == '-' {
                    sign = -sign;
                }
                prev = Some(c);
                continue;
            }
        }
        cur.push(c);
        if !c.is_whitespace() {
            prev = Some(c);
        }
    }
    if cur.trim().is_empty() {
        return Err("dangling operator in expression".to_string());
    }
    terms.push((sign, cur.trim().to_string()));

    let mut acc = Lin::default();
    for (sg, t) in terms {
        let l = parse_term(&t)?;
        acc = acc.add(&l, sg)?;
    }
    Ok(acc)
}

fn parse_term(s: &str) -> Result<Lin, String> {
    let chars: Vec<char> = s.chars().collect();
    let factors = split_top_level(&chars, &["*"]);
    let mut acc = Lin::constant(1);
    for f in &factors {
        if f.trim().is_empty() {
            return Err("empty factor".to_string());
        }
        let l = parse_factor(f)?;
        acc = acc.mul(&l)?;
    }
    Ok(acc)
}

fn parse_factor(s: &str) -> Result<Lin, String> {
    let mut t = s.trim().to_string();
    let mut sign = 1i64;
    while t.starts_with('-') {
        sign = -sign;
        t = t[1..].trim().to_string();
    }
    let stripped = strip_outer_parens(&t).trim().to_string();
    let was_parenthesized = stripped != t;
    let base = if let Some(n) = as_int(&stripped) {
        Lin::constant(n)
    } else if let Some(name) = as_ident(&stripped) {
        Lin::var(&name)
    } else if was_parenthesized {
        // A parenthesized sub-expression: recurse (terminates, since the string
        // strictly shrank).
        parse_lin(&stripped)?
    } else {
        return Err(format!(
            "`{stripped}` is neither an integer literal, a variable, nor a parenthesized \
             linear expression"
        ));
    };
    base.scale(sign)
}

// ===========================================================================
// Lexical helpers (local copies — vacuity's are private to that module)
// ===========================================================================

fn split_top_level(chars: &[char], seps: &[&str]) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut i = 0usize;
    'outer: while i < chars.len() {
        match chars[i] {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            for s in seps {
                let sv: Vec<char> = s.chars().collect();
                if !sv.is_empty()
                    && i + sv.len() <= chars.len()
                    && chars[i..i + sv.len()] == sv[..]
                {
                    parts.push(cur.trim().to_string());
                    cur.clear();
                    i += sv.len();
                    continue 'outer;
                }
            }
        }
        cur.push(chars[i]);
        i += 1;
    }
    parts.push(cur.trim().to_string());
    parts
}

fn parens_balanced(s: &str) -> bool {
    let mut d = 0i32;
    for c in s.chars() {
        match c {
            '(' => d += 1,
            ')' => {
                d -= 1;
                if d < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    d == 0
}

fn as_int(s: &str) -> Option<i64> {
    let s = strip_outer_parens(s.trim()).trim();
    if s.is_empty() {
        return None;
    }
    let body = s.strip_prefix('-').unwrap_or(s);
    if body.is_empty() || !body.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse::<i64>().ok()
}

fn as_ident(s: &str) -> Option<String> {
    let s = strip_outer_parens(s.trim()).trim();
    if s.is_empty() {
        return None;
    }
    let first = s.chars().next()?;
    if !(first.is_alphabetic() || first == '_') {
        return None;
    }
    if !s.chars().all(is_word) {
        return None;
    }
    Some(s.to_string())
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '\''
}

fn strip_outer_parens(s: &str) -> &str {
    let mut cur = s.trim();
    loop {
        let b = cur.as_bytes();
        if b.len() < 2 || b[0] != b'(' || b[b.len() - 1] != b')' {
            return cur;
        }
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

fn norm_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prover::vacuity::{check_vacuity, HypothesisField, WitnessVerdict};

    fn b(goal: &str, fields: Vec<HypothesisField>) -> HypothesisBundle {
        HypothesisBundle::new(goal, fields)
    }

    // --- THE positive case: vacuity itself accepts what we produce ----------

    #[test]
    fn satisfiable_conjunction_yields_a_witness_vacuity_accepts() {
        let bundle = b(
            "thm",
            vec![
                HypothesisField::datum("x", "Int"),
                HypothesisField::datum("y", "Int"),
                HypothesisField::hypothesis("h1", "x > 5"),
                HypothesisField::hypothesis("h2", "x < 12 ∧ y = x + 1"),
                HypothesisField::hypothesis("h3", "2 ∣ y"),
            ],
        );
        let found = search_witness(&bundle);
        let w = match &found {
            WitnessSearch::WitnessFound(w) => w.clone(),
            other => panic!("expected a witness, got {other:?}"),
        };

        // The witness really satisfies the fields.
        let x = w.bindings["x"].as_i64().unwrap();
        let y = w.bindings["y"].as_i64().unwrap();
        assert!(x > 5 && x < 12 && y == x + 1 && y % 2 == 0, "x={x} y={y}");

        // And vacuity's OWN audit accepts it — the whole point.
        let report = check_vacuity(&bundle, Some(&w));
        assert!(report.clean, "vacuity must accept our witness: {report:?}");
        assert_eq!(report.verdict, WitnessVerdict::WitnessSupplied);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn witness_covers_every_field() {
        let bundle = b(
            "thm",
            vec![
                HypothesisField::datum("n", "Nat"),
                HypothesisField::datum("m", "Nat"), // constrained by nothing
                HypothesisField::hypothesis("hn", "n >= 3"),
            ],
        );
        let w = search_witness(&bundle).into_witness().expect("witness");
        assert!(w.bindings.contains_key("n"));
        assert!(w.bindings.contains_key("m"), "unconstrained binder must still be bound");
        assert_eq!(w.claims, vec!["hn".to_string()]);
        assert!(check_vacuity(&bundle, Some(&w)).clean);
    }

    // --- the classic contradiction -----------------------------------------

    #[test]
    fn contradictory_pair_yields_no_witness_in_bounds() {
        let bundle = b(
            "thm_vacuous",
            vec![
                HypothesisField::datum("x", "Int"),
                HypothesisField::hypothesis("h1", "x > 5"),
                HypothesisField::hypothesis("h2", "x < 3"),
            ],
        );
        match search_witness(&bundle) {
            WitnessSearch::NoWitnessInBounds {
                exhausted,
                candidates_tried,
            } => {
                assert!(exhausted, "the whole grid should have been enumerated");
                assert!(candidates_tried > 0);
            }
            other => panic!("expected NoWitnessInBounds, got {other:?}"),
        }
    }

    #[test]
    fn no_witness_in_bounds_produces_no_witness_for_the_backend() {
        let bundle = b(
            "g",
            vec![
                HypothesisField::datum("x", "Nat"),
                HypothesisField::hypothesis("h1", "x > 5"),
                HypothesisField::hypothesis("h2", "x < 3"),
            ],
        );
        assert!(search_witness(&bundle).into_witness().is_none());
    }

    // --- THE key test: one unparseable field poisons the whole bundle -------

    #[test]
    fn one_unparseable_field_yields_not_decidable() {
        // `h1` and `h2` are satisfiable and trivially witnessed by n := 4; a
        // partial witness would have been easy. `h3` is outside the fragment, so
        // the ONLY honest answer is NotDecidable — `h3` could be the
        // contradictory one, and a witness claiming it would launder exactly the
        // hollow success the gate exists to catch.
        let bundle = b(
            "thm",
            vec![
                HypothesisField::datum("n", "Nat"),
                HypothesisField::hypothesis("h1", "n > 2"),
                HypothesisField::hypothesis("h2", "n < 10"),
                HypothesisField::hypothesis("h3", "Nat.Coprime n 9"),
            ],
        );
        match search_witness(&bundle) {
            WitnessSearch::NotDecidable { reason } => {
                assert!(reason.contains("h3"), "reason should name the field: {reason}");
            }
            other => panic!("expected NotDecidable, got {other:?}"),
        }
        assert!(search_witness(&bundle).into_witness().is_none());

        // Sanity: without the unparseable field the very same bundle IS decided.
        let ok = b(
            "thm",
            vec![
                HypothesisField::datum("n", "Nat"),
                HypothesisField::hypothesis("h1", "n > 2"),
                HypothesisField::hypothesis("h2", "n < 10"),
            ],
        );
        assert!(matches!(search_witness(&ok), WitnessSearch::WitnessFound(_)));
    }

    #[test]
    fn unparseable_data_binder_yields_not_decidable() {
        let bundle = b(
            "thm",
            vec![
                HypothesisField::datum("n", "Nat"),
                HypothesisField::datum("l", "List Nat"),
                HypothesisField::hypothesis("hn", "n > 0"),
            ],
        );
        assert!(matches!(
            search_witness(&bundle),
            WitnessSearch::NotDecidable { .. }
        ));
    }

    #[test]
    fn free_variable_not_declared_as_datum_is_not_decidable() {
        // `k` has no declared domain — we will not invent one.
        let bundle = b(
            "thm",
            vec![
                HypothesisField::datum("n", "Nat"),
                HypothesisField::hypothesis("h", "n > k"),
            ],
        );
        match search_witness(&bundle) {
            WitnessSearch::NotDecidable { reason } => assert!(reason.contains('k')),
            other => panic!("expected NotDecidable, got {other:?}"),
        }
    }

    #[test]
    fn disjunction_is_not_decidable() {
        let bundle = b(
            "thm",
            vec![
                HypothesisField::datum("x", "Int"),
                HypothesisField::hypothesis("h", "x > 5 ∨ x < 3"),
            ],
        );
        assert!(matches!(
            search_witness(&bundle),
            WitnessSearch::NotDecidable { .. }
        ));
    }

    #[test]
    fn divisibility_on_a_field_binder_is_not_decidable() {
        let bundle = b(
            "thm",
            vec![
                HypothesisField::datum("x", "ℝ"),
                HypothesisField::hypothesis("h", "2 ∣ x"),
            ],
        );
        assert!(matches!(
            search_witness(&bundle),
            WitnessSearch::NotDecidable { .. }
        ));
    }

    #[test]
    fn nonlinear_product_is_not_decidable() {
        let bundle = b(
            "thm",
            vec![
                HypothesisField::datum("x", "Int"),
                HypothesisField::datum("y", "Int"),
                HypothesisField::hypothesis("h", "x * y = 6"),
            ],
        );
        assert!(matches!(
            search_witness(&bundle),
            WitnessSearch::NotDecidable { .. }
        ));
    }

    // --- fragment coverage --------------------------------------------------

    #[test]
    fn linear_equation_between_variables() {
        let bundle = b(
            "thm",
            vec![
                HypothesisField::datum("a", "Int"),
                HypothesisField::datum("bb", "Int"),
                HypothesisField::hypothesis("h1", "a = bb + 1"),
                HypothesisField::hypothesis("h2", "2 * a - bb >= 7"),
            ],
        );
        let w = search_witness(&bundle).into_witness().expect("witness");
        let a = w.bindings["a"].as_i64().unwrap();
        let bb = w.bindings["bb"].as_i64().unwrap();
        assert_eq!(a, bb + 1);
        assert!(2 * a - bb >= 7);
    }

    #[test]
    fn parity_modulus_and_range_membership() {
        let bundle = b(
            "thm",
            vec![
                HypothesisField::datum("n", "Nat"),
                HypothesisField::hypothesis("h1", "Odd n"),
                HypothesisField::hypothesis("h2", "n % 4 = 1"),
                HypothesisField::hypothesis("h3", "n ∈ Finset.Icc 6 12"),
            ],
        );
        let w = search_witness(&bundle).into_witness().expect("witness");
        let n = w.bindings["n"].as_i64().unwrap();
        assert_eq!(n % 2, 1);
        assert_eq!(n % 4, 1);
        assert!((6..=12).contains(&n));
        assert_eq!(n, 9);
    }

    #[test]
    fn finset_range_and_negation() {
        let bundle = b(
            "thm",
            vec![
                HypothesisField::datum("x", "Nat"),
                HypothesisField::hypothesis("h1", "x ∈ Finset.range 5"),
                HypothesisField::hypothesis("h2", "¬ (x < 3)"),
            ],
        );
        let w = search_witness(&bundle).into_witness().expect("witness");
        let x = w.bindings["x"].as_i64().unwrap();
        assert!((3..5).contains(&x));
    }

    #[test]
    fn fin_domain_is_enumerated() {
        let bundle = b(
            "thm",
            vec![
                HypothesisField::datum("i", "Fin 3"),
                HypothesisField::hypothesis("h", "i >= 2"),
            ],
        );
        let w = search_witness(&bundle).into_witness().expect("witness");
        assert_eq!(w.bindings["i"].as_i64().unwrap(), 2);
    }

    #[test]
    fn a_negated_pair_finds_nothing_rather_than_lying() {
        let bundle = b(
            "thm",
            vec![
                HypothesisField::datum("x", "Int"),
                HypothesisField::hypothesis("h1", "x > 5"),
                HypothesisField::hypothesis("h2", "¬ (x > 5)"),
            ],
        );
        assert!(matches!(
            search_witness(&bundle),
            WitnessSearch::NoWitnessInBounds { .. }
        ));
    }

    #[test]
    fn trivial_bundle_is_witnessed_when_domains_are_known() {
        let bundle = b("g", vec![HypothesisField::datum("n", "Nat")]);
        let w = search_witness(&bundle).into_witness().expect("witness");
        assert_eq!(w.bindings["n"].as_i64().unwrap(), 0);
        assert!(w.claims.is_empty());
        assert!(check_vacuity(&bundle, Some(&w)).clean);
    }

    // --- determinism and bounds --------------------------------------------

    #[test]
    fn enumeration_is_deterministic_across_runs() {
        let bundle = b(
            "thm",
            vec![
                HypothesisField::datum("p", "Int"),
                HypothesisField::datum("q", "Int"),
                HypothesisField::datum("r", "Nat"),
                HypothesisField::hypothesis("h1", "p + q = 4"),
                HypothesisField::hypothesis("h2", "q > 1"),
                HypothesisField::hypothesis("h3", "r >= q"),
            ],
        );
        let a = search_witness(&bundle);
        for _ in 0..5 {
            assert_eq!(a, search_witness(&bundle), "search must be deterministic");
        }
        // And the witness itself is a fixed, reproducible instance.
        let w = a.witness().expect("witness");
        // First hit of the fixed odometer: p slowest, r fastest; Int enumerates
        // 0, 1, -1, 2, -2, … and Nat enumerates 0, 1, 2, ….
        assert_eq!(w.label, "p := 0, q := 4, r := 4");
    }

    #[test]
    fn failure_outcomes_are_deterministic_too() {
        let bundle = b(
            "thm",
            vec![
                HypothesisField::datum("x", "Int"),
                HypothesisField::hypothesis("h1", "x > 5"),
                HypothesisField::hypothesis("h2", "x < 3"),
            ],
        );
        let a = search_witness(&bundle);
        assert_eq!(a, search_witness(&bundle));

        let nd = b(
            "thm",
            vec![HypothesisField::hypothesis("h", "Nat.Prime p")],
        );
        assert_eq!(search_witness(&nd), search_witness(&nd));
    }

    #[test]
    fn the_candidate_bound_is_respected() {
        let bundle = b(
            "thm",
            vec![
                HypothesisField::datum("x", "Int"),
                HypothesisField::datum("y", "Int"),
                HypothesisField::hypothesis("h1", "x > 5"),
                HypothesisField::hypothesis("h2", "x < 3"),
            ],
        );
        let bounds = SearchBounds {
            max_abs_value: 50,
            max_candidates: 10,
        };
        match search_witness_bounded(&bundle, bounds) {
            WitnessSearch::NoWitnessInBounds {
                candidates_tried,
                exhausted,
            } => {
                assert_eq!(candidates_tried, 10, "must stop exactly at the cap");
                assert!(!exhausted, "a truncated search is NOT exhaustive");
            }
            other => panic!("expected NoWitnessInBounds, got {other:?}"),
        }
    }

    #[test]
    fn the_value_bound_is_respected() {
        // `x > 100` is satisfiable, but not inside a ±20 grid. The honest answer
        // is NoWitnessInBounds — which is NOT unsatisfiability.
        let bundle = b(
            "thm",
            vec![
                HypothesisField::datum("x", "Int"),
                HypothesisField::hypothesis("h", "x > 100"),
            ],
        );
        assert!(matches!(
            search_witness(&bundle),
            WitnessSearch::NoWitnessInBounds { exhausted: true, .. }
        ));
        // Widen the bound and the very same bundle is witnessed.
        let wide = SearchBounds {
            max_abs_value: 200,
            max_candidates: 100_000,
        };
        let w = search_witness_bounded(&bundle, wide)
            .into_witness()
            .expect("witness inside the wider bound");
        assert!(w.bindings["x"].as_i64().unwrap() > 100);
    }

    #[test]
    fn every_bound_value_stays_inside_the_declared_domain() {
        let bundle = b(
            "thm",
            vec![
                HypothesisField::datum("n", "Nat"),
                HypothesisField::hypothesis("h", "n <= 3"),
            ],
        );
        let w = search_witness(&bundle).into_witness().expect("witness");
        assert!(w.bindings["n"].as_i64().unwrap() >= 0, "Nat is never negative");
    }

    #[test]
    fn tags_are_stable() {
        assert_eq!(
            WitnessSearch::NoWitnessInBounds {
                candidates_tried: 0,
                exhausted: true
            }
            .tag(),
            "no_witness_in_bounds"
        );
        assert_eq!(
            WitnessSearch::NotDecidable {
                reason: String::new()
            }
            .tag(),
            "not_decidable"
        );
    }
}
