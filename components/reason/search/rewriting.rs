//! First-order term rewriting primitives the search layer lacks: **unification**
//! (#13), **term orderings** (#14) and bounded **Knuth–Bendix completion** (#15).
//!
//! Where [`super::subsumption`] is a deliberately shallow *string/literal-level*
//! α-canonicalizer, this module works on a real first-order [`Term`] model and
//! provides the machinery a saturation / rewriting engine actually needs:
//!
//! * **#13 Unification.** [`unify`] computes a most-general unifier (mgu) by the
//!   classical Robinson algorithm with a sound **occurs-check**, so `x` and
//!   `f(x)` do *not* unify and `f(x,x)` does *not* unify with `f(a,b)`.
//!   [`matches`] is the one-way (pattern → subject) match, and [`subsumes`] is
//!   *true* term subsumption (a general term subsumes its instances, never the
//!   reverse) — a proper replacement for the string test in `subsumption.rs`.
//! * **#14 Term orderings.** [`lpo`] is the Lexicographic Path Order and [`kbo`]
//!   the Knuth–Bendix Order, each a *partial* order parameterised by a symbol
//!   [`Precedence`] (LPO) or [`KboWeights`] (KBO). Because both are genuinely
//!   partial we return `Option<Ordering>` (`None` = incomparable) rather than
//!   forcing a spurious total order. [`multiset_compare`] is the Dershowitz–Manna
//!   multiset extension of any base term order.
//! * **#15 Completion.** [`critical_pairs`] enumerates overlaps between rewrite
//!   rules, [`normal_form`] / [`demodulate`] rewrite a term to a normal form, and
//!   [`complete`] runs a *bounded* Knuth–Bendix completion: it orients each
//!   equation by a supplied ordering and repeatedly joins critical pairs until the
//!   rule set is (locally) confluent or a fixed iteration bound is hit.
//!   [`congruence_closure`] decides a ground equality by union–find + congruence.
//!
//! ## First-principles / dependency note
//!
//! These are small, classic algorithms with no clean drop-in Rust crate that fits
//! this `Term` model, so they are built from first principles and use **std only**
//! (no `rand`, no wall-clock). Every function is a pure function of its inputs:
//! given the same terms / rules / ordering it returns byte-identical results, and
//! every unbounded loop is capped by an explicit, documented iteration bound so
//! completion always terminates (returning a possibly-incomplete but sound rule
//! set) rather than diverging.

use anyhow::{anyhow, Result};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

/// Default cap on rewrite steps when normalising a term to a normal form. Large
/// enough for any small system; guarantees termination on a (buggy) non-normalising
/// rule set instead of looping forever.
const DEFAULT_REWRITE_STEPS: usize = 10_000;

/// Safety cap on the number of rules a [`complete`] run may accumulate before it
/// gives up (returns the rules built so far). Prevents runaway on systems that do
/// not complete finitely.
const MAX_COMPLETION_RULES: usize = 200;

/// A first-order term: either a variable or a function symbol applied to
/// arguments. A **constant** is just `App(symbol, [])`.
///
/// Terms are compared and hashed structurally, so `==` is syntactic equality
/// (identical up to nothing — not even α-renaming; that is what [`unify`] /
/// [`matches`] are for).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Term {
    /// A variable, identified by name.
    Var(String),
    /// A function symbol applied to zero or more argument terms.
    App(String, Vec<Term>),
}

impl Term {
    /// A variable term.
    pub fn var(name: &str) -> Term {
        Term::Var(name.to_string())
    }

    /// A function application `symbol(args...)`.
    pub fn app(symbol: &str, args: Vec<Term>) -> Term {
        Term::App(symbol.to_string(), args)
    }

    /// A constant `symbol` (nullary application).
    pub fn constant(symbol: &str) -> Term {
        Term::App(symbol.to_string(), Vec::new())
    }

    /// True if this term is a variable.
    pub fn is_var(&self) -> bool {
        matches!(self, Term::Var(_))
    }
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Term::Var(x) => write!(f, "{x}"),
            Term::App(sym, args) if args.is_empty() => write!(f, "{sym}"),
            Term::App(sym, args) => {
                write!(f, "{sym}(")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{a}")?;
                }
                write!(f, ")")
            }
        }
    }
}

/// A substitution: a mapping from variable names to terms. `BTreeMap` (not a hash
/// map) so iteration order — and therefore every derived result — is deterministic.
pub type Subst = BTreeMap<String, Term>;

/// Apply `subst` to `term`, resolving bound variables **recursively** (so a
/// binding whose value itself mentions bound variables is fully instantiated).
pub fn apply_subst(subst: &Subst, term: &Term) -> Term {
    match term {
        Term::Var(x) => match subst.get(x) {
            Some(v) => apply_subst(subst, v),
            None => term.clone(),
        },
        Term::App(f, args) => Term::App(
            f.clone(),
            args.iter().map(|a| apply_subst(subst, a)).collect(),
        ),
    }
}

/// Follow top-level variable→term chains in `subst` until the head is no longer a
/// bound variable (does not descend into arguments — that is [`apply_subst`]'s job).
fn resolve(term: &Term, subst: &Subst) -> Term {
    let mut cur = term.clone();
    while let Term::Var(x) = &cur {
        match subst.get(x) {
            Some(v) => cur = v.clone(),
            None => break,
        }
    }
    cur
}

/// The occurs-check: does variable `x` occur anywhere in `term` (resolving bound
/// variables through `subst`)? Guarantees [`unify`] never builds a cyclic term.
fn occurs(x: &str, term: &Term, subst: &Subst) -> bool {
    match term {
        Term::Var(y) => {
            if y == x {
                true
            } else {
                match subst.get(y) {
                    Some(v) => occurs(x, v, subst),
                    None => false,
                }
            }
        }
        Term::App(_, args) => args.iter().any(|a| occurs(x, a, subst)),
    }
}

/// Robinson unification producing a most-general unifier (mgu), or `None` if the
/// terms are not unifiable. Sound: the occurs-check rejects `x = f(x)` and clashing
/// arities/symbols fail.
///
/// The returned [`Subst`] is idempotent under [`apply_subst`]: applying it to both
/// input terms yields syntactically equal results.
pub fn unify(s: &Term, t: &Term) -> Option<Subst> {
    let mut subst = Subst::new();
    if unify_into(s, t, &mut subst) {
        Some(subst)
    } else {
        None
    }
}

fn unify_into(a: &Term, b: &Term, subst: &mut Subst) -> bool {
    let a = resolve(a, subst);
    let b = resolve(b, subst);
    match (&a, &b) {
        (Term::Var(x), Term::Var(y)) if x == y => true,
        (Term::Var(x), _) => {
            if occurs(x, &b, subst) {
                false
            } else {
                subst.insert(x.clone(), b.clone());
                true
            }
        }
        (_, Term::Var(y)) => {
            if occurs(y, &a, subst) {
                false
            } else {
                subst.insert(y.clone(), a.clone());
                true
            }
        }
        (Term::App(f, fa), Term::App(g, ga)) => {
            if f != g || fa.len() != ga.len() {
                return false;
            }
            fa.iter().zip(ga).all(|(x, y)| unify_into(x, y, subst))
        }
    }
}

/// One-way (syntactic) matching: find a substitution σ, defined only on the
/// *pattern*'s variables, such that `pattern σ == term`. The subject `term`'s own
/// variables are treated as opaque constants (never substituted). `None` if no
/// such σ exists.
pub fn matches(pattern: &Term, term: &Term) -> Option<Subst> {
    let mut subst = Subst::new();
    if match_into(pattern, term, &mut subst) {
        Some(subst)
    } else {
        None
    }
}

fn match_into(pattern: &Term, term: &Term, subst: &mut Subst) -> bool {
    match pattern {
        Term::Var(x) => match subst.get(x) {
            Some(bound) => bound == term,
            None => {
                subst.insert(x.clone(), term.clone());
                true
            }
        },
        Term::App(f, pa) => match term {
            Term::App(g, ta) if f == g && pa.len() == ta.len() => {
                pa.iter().zip(ta).all(|(p, t)| match_into(p, t, subst))
            }
            _ => false,
        },
    }
}

/// True subsumption: `general` subsumes `specific` iff some substitution turns
/// `general` into `specific` (`general` is the *more general* term). Sound and
/// directional — a general term subsumes its instances, never the reverse.
pub fn subsumes(general: &Term, specific: &Term) -> bool {
    matches(general, specific).is_some()
}

// ---------------------------------------------------------------------------
// #14 Term orderings: precedence, LPO, KBO, and the multiset extension.
// ---------------------------------------------------------------------------

/// A precedence on function symbols: a total pre-order given by integer ranks,
/// tie-broken by symbol name so it is a genuine **total order** on symbols (LPO/KBO
/// need this to be total-ish on ground terms). Unknown symbols have rank `0`.
#[derive(Clone, Debug, Default)]
pub struct Precedence {
    ranks: BTreeMap<String, i64>,
}

impl Precedence {
    /// An empty precedence: every symbol has rank `0`, so symbols are ordered
    /// purely by name.
    pub fn new() -> Self {
        Precedence::default()
    }

    /// Build a precedence from symbols listed **greatest first**: the first symbol
    /// is the largest, the last the smallest.
    pub fn with_order(symbols: &[&str]) -> Self {
        let n = symbols.len() as i64;
        let mut ranks = BTreeMap::new();
        for (i, s) in symbols.iter().enumerate() {
            ranks.insert((*s).to_string(), n - i as i64);
        }
        Precedence { ranks }
    }

    /// Set a symbol's rank explicitly (higher = greater).
    pub fn set(&mut self, symbol: &str, rank: i64) {
        self.ranks.insert(symbol.to_string(), rank);
    }

    fn rank(&self, symbol: &str) -> i64 {
        *self.ranks.get(symbol).unwrap_or(&0)
    }

    /// Total order on symbols: by rank, then by name.
    fn compare(&self, f: &str, g: &str) -> Ordering {
        self.rank(f).cmp(&self.rank(g)).then_with(|| f.cmp(g))
    }
}

/// Does variable `x` occur anywhere in `term`?
fn occurs_var(x: &str, term: &Term) -> bool {
    match term {
        Term::Var(y) => y == x,
        Term::App(_, args) => args.iter().any(|a| occurs_var(x, a)),
    }
}

/// Lexicographic Path Order comparison (partial): `Some(Greater/Less)` when the
/// terms are LPO-ordered, `Some(Equal)` when syntactically equal, `None` when
/// incomparable. Parameterised by a symbol [`Precedence`].
///
/// LPO is a well-founded simplification order: whenever `lpo(p, l, r) ==
/// Some(Greater)` the rule `l → r` is terminating, which is exactly the test
/// [`complete`] uses to orient equations.
pub fn lpo(prec: &Precedence, s: &Term, t: &Term) -> Option<Ordering> {
    if s == t {
        Some(Ordering::Equal)
    } else if lpo_gt(prec, s, t) {
        Some(Ordering::Greater)
    } else if lpo_gt(prec, t, s) {
        Some(Ordering::Less)
    } else {
        None
    }
}

/// Strict LPO: `s >lpo t`.
fn lpo_gt(prec: &Precedence, s: &Term, t: &Term) -> bool {
    if s == t {
        return false;
    }
    match s {
        // A variable is greater than nothing (only equal to itself, handled above).
        Term::Var(_) => false,
        Term::App(f, ss) => {
            // (1) Subterm case: some argument is >= t.
            if ss.iter().any(|si| si == t || lpo_gt(prec, si, t)) {
                return true;
            }
            match t {
                // (var) s > x iff x occurs (properly) in s.
                Term::Var(x) => occurs_var(x, s),
                Term::App(g, ts) => {
                    if f == g {
                        // (3) Same head: s dominates every t_j and the argument
                        //     tuples are lexicographically ordered.
                        ts.iter().all(|tj| lpo_gt(prec, s, tj)) && lpo_lex_gt(prec, ss, ts)
                    } else if prec.compare(f, g) == Ordering::Greater {
                        // (2) Bigger head: s must dominate every t_j.
                        ts.iter().all(|tj| lpo_gt(prec, s, tj))
                    } else {
                        false
                    }
                }
            }
        }
    }
}

/// Lexicographic lift of `lpo_gt` over equal-length argument tuples.
fn lpo_lex_gt(prec: &Precedence, ss: &[Term], ts: &[Term]) -> bool {
    for (a, b) in ss.iter().zip(ts) {
        if a == b {
            continue;
        }
        return lpo_gt(prec, a, b);
    }
    false
}

/// Knuth–Bendix weighting: a weight per symbol (default `default_weight`), a single
/// `var_weight` (`w0`) for every variable, and a tie-break [`Precedence`]. Kept
/// **admissible** by construction — all weights are `>= 1` and `w0 >= 1` — so the
/// induced [`kbo`] is a reduction order.
#[derive(Clone, Debug)]
pub struct KboWeights {
    weights: BTreeMap<String, u64>,
    default_weight: u64,
    var_weight: u64,
    precedence: Precedence,
}

impl KboWeights {
    /// Uniform weights (every symbol and variable weighs `1`) over `precedence`.
    pub fn new(precedence: Precedence) -> Self {
        KboWeights {
            weights: BTreeMap::new(),
            default_weight: 1,
            var_weight: 1,
            precedence,
        }
    }

    /// Set the weight of a symbol (must stay `>= 1` for admissibility).
    pub fn set_weight(&mut self, symbol: &str, weight: u64) {
        self.weights.insert(symbol.to_string(), weight.max(1));
    }

    fn sym_weight(&self, symbol: &str) -> u64 {
        *self.weights.get(symbol).unwrap_or(&self.default_weight)
    }

    fn weight(&self, term: &Term) -> u64 {
        match term {
            Term::Var(_) => self.var_weight,
            Term::App(f, args) => {
                self.sym_weight(f) + args.iter().map(|a| self.weight(a)).sum::<u64>()
            }
        }
    }
}

/// Count occurrences of each variable in `term`.
fn var_counts(term: &Term, out: &mut BTreeMap<String, u64>) {
    match term {
        Term::Var(x) => *out.entry(x.clone()).or_insert(0) += 1,
        Term::App(_, args) => {
            for a in args {
                var_counts(a, out);
            }
        }
    }
}

/// The KBO variable condition: every variable occurs in `s` at least as often as
/// in `t` (necessary for `s >kbo t`).
fn kbo_var_condition(s: &Term, t: &Term) -> bool {
    let mut cs = BTreeMap::new();
    let mut ct = BTreeMap::new();
    var_counts(s, &mut cs);
    var_counts(t, &mut ct);
    ct.iter()
        .all(|(x, &nt)| cs.get(x).copied().unwrap_or(0) >= nt)
}

/// Knuth–Bendix Order comparison (partial): `Some(Greater/Less)` when KBO-ordered,
/// `Some(Equal)` when syntactically equal, `None` when incomparable.
pub fn kbo(weights: &KboWeights, s: &Term, t: &Term) -> Option<Ordering> {
    if s == t {
        Some(Ordering::Equal)
    } else if kbo_gt(weights, s, t) {
        Some(Ordering::Greater)
    } else if kbo_gt(weights, t, s) {
        Some(Ordering::Less)
    } else {
        None
    }
}

/// Strict KBO: `s >kbo t`.
fn kbo_gt(w: &KboWeights, s: &Term, t: &Term) -> bool {
    if !kbo_var_condition(s, t) {
        return false;
    }
    let ws = w.weight(s);
    let wt = w.weight(t);
    if ws > wt {
        return true;
    }
    if ws < wt {
        return false;
    }
    // Equal weight: break by precedence on heads, then lexicographically.
    match (s, t) {
        (Term::App(f, ss), Term::App(g, ts)) => {
            if w.precedence.compare(f, g) == Ordering::Greater {
                true
            } else if f == g && ss.len() == ts.len() {
                kbo_lex_gt(w, ss, ts)
            } else {
                false
            }
        }
        _ => false,
    }
}

fn kbo_lex_gt(w: &KboWeights, ss: &[Term], ts: &[Term]) -> bool {
    for (a, b) in ss.iter().zip(ts) {
        if a == b {
            continue;
        }
        return kbo_gt(w, a, b);
    }
    false
}

/// Dershowitz–Manna multiset extension of a base term order `cmp`. `m >mul n` iff
/// `n` is obtained from `m` by replacing some (non-empty set of) elements with
/// finitely many strictly smaller ones. Returns `None` when the two multisets are
/// incomparable under `cmp`.
pub fn multiset_compare<F>(cmp: &F, m: &[Term], n: &[Term]) -> Option<Ordering>
where
    F: Fn(&Term, &Term) -> Option<Ordering>,
{
    // Remove the common (syntactically-equal) part greedily.
    let mut mm: Vec<&Term> = m.iter().collect();
    let mut nn: Vec<&Term> = n.iter().collect();
    let mut i = 0;
    while i < mm.len() {
        if let Some(j) = nn.iter().position(|x| *x == mm[i]) {
            mm.remove(i);
            nn.remove(j);
        } else {
            i += 1;
        }
    }
    if mm.is_empty() && nn.is_empty() {
        return Some(Ordering::Equal);
    }
    // m > n: every leftover n-element is strictly below some leftover m-element.
    let gt = !mm.is_empty()
        && nn
            .iter()
            .all(|y| mm.iter().any(|x| cmp(x, y) == Some(Ordering::Greater)));
    let lt = !nn.is_empty()
        && mm
            .iter()
            .all(|x| nn.iter().any(|y| cmp(y, x) == Some(Ordering::Greater)));
    match (gt, lt) {
        (true, false) => Some(Ordering::Greater),
        (false, true) => Some(Ordering::Less),
        _ => None,
    }
}

/// A reduction ordering usable to orient equations in [`complete`]. Implemented by
/// [`LpoOrdering`] and [`KboOrdering`]; a caller may supply any custom order.
pub trait TermOrdering {
    /// Compare two terms; `None` = incomparable.
    fn compare(&self, s: &Term, t: &Term) -> Option<Ordering>;
}

/// LPO as a [`TermOrdering`].
#[derive(Clone, Debug)]
pub struct LpoOrdering {
    pub precedence: Precedence,
}

impl TermOrdering for LpoOrdering {
    fn compare(&self, s: &Term, t: &Term) -> Option<Ordering> {
        lpo(&self.precedence, s, t)
    }
}

/// KBO as a [`TermOrdering`].
#[derive(Clone, Debug)]
pub struct KboOrdering {
    pub weights: KboWeights,
}

impl TermOrdering for KboOrdering {
    fn compare(&self, s: &Term, t: &Term) -> Option<Ordering> {
        kbo(&self.weights, s, t)
    }
}

// ---------------------------------------------------------------------------
// #15 Rewriting + Knuth–Bendix completion.
// ---------------------------------------------------------------------------

/// A rewrite rule `lhs → rhs` (left-to-right). Well-formed rules have
/// `vars(rhs) ⊆ vars(lhs)` so rewriting is closed on ground terms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub lhs: Term,
    pub rhs: Term,
}

impl Rule {
    /// Construct a rule `lhs → rhs`.
    pub fn new(lhs: Term, rhs: Term) -> Rule {
        Rule { lhs, rhs }
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} -> {}", self.lhs, self.rhs)
    }
}

/// Positions of all non-variable (function-application) subterms, root first, in a
/// deterministic pre-order.
fn nonvar_positions(term: &Term) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    collect_positions(term, Vec::new(), &mut out);
    out
}

fn collect_positions(term: &Term, prefix: Vec<usize>, out: &mut Vec<Vec<usize>>) {
    if let Term::App(_, args) = term {
        out.push(prefix.clone());
        for (i, a) in args.iter().enumerate() {
            let mut p = prefix.clone();
            p.push(i);
            collect_positions(a, p, out);
        }
    }
}

/// The subterm of `term` at `pos` (positions produced by [`nonvar_positions`] are
/// always valid).
fn subterm_at<'a>(term: &'a Term, pos: &[usize]) -> &'a Term {
    let mut cur = term;
    for &i in pos {
        match cur {
            Term::App(_, args) => cur = &args[i],
            Term::Var(_) => break,
        }
    }
    cur
}

/// Return `term` with the subterm at `pos` replaced by `new`.
fn replace_at(term: &Term, pos: &[usize], new: Term) -> Term {
    if pos.is_empty() {
        return new;
    }
    match term {
        Term::App(f, args) => {
            let mut na = args.clone();
            let i = pos[0];
            na[i] = replace_at(&args[i], &pos[1..], new);
            Term::App(f.clone(), na)
        }
        Term::Var(_) => term.clone(),
    }
}

/// Rename every variable in `term` by appending `suffix` (used to make two rules'
/// variables disjoint before overlapping them).
fn rename_term(term: &Term, suffix: &str) -> Term {
    match term {
        Term::Var(x) => Term::Var(format!("{x}{suffix}")),
        Term::App(f, args) => Term::App(
            f.clone(),
            args.iter().map(|a| rename_term(a, suffix)).collect(),
        ),
    }
}

fn rename_rule(rule: &Rule, suffix: &str) -> Rule {
    Rule {
        lhs: rename_term(&rule.lhs, suffix),
        rhs: rename_term(&rule.rhs, suffix),
    }
}

/// Rewrite `term` at the root with `rule` if its lhs matches, else `None`.
fn rewrite_top(rule: &Rule, term: &Term) -> Option<Term> {
    matches(&rule.lhs, term).map(|s| apply_subst(&s, &rule.rhs))
}

/// A single leftmost-outermost rewrite step with the first applicable rule, or
/// `None` if `term` is already in normal form w.r.t. `rules`.
fn rewrite_once(rules: &[Rule], term: &Term) -> Option<Term> {
    for r in rules {
        if let Some(t) = rewrite_top(r, term) {
            return Some(t);
        }
    }
    if let Term::App(f, args) = term {
        for i in 0..args.len() {
            if let Some(a2) = rewrite_once(rules, &args[i]) {
                let mut na = args.clone();
                na[i] = a2;
                return Some(Term::App(f.clone(), na));
            }
        }
    }
    None
}

/// Rewrite `term` to a normal form with `rules`, applying at most `max_steps`
/// steps (the bound guarantees termination even on a non-normalising system).
pub fn normal_form(rules: &[Rule], term: &Term, max_steps: usize) -> Term {
    let mut cur = term.clone();
    for _ in 0..max_steps {
        match rewrite_once(rules, &cur) {
            Some(next) => cur = next,
            None => break,
        }
    }
    cur
}

/// Demodulation: rewrite `term` to its normal form with `rules` using the default
/// step bound. (A convenience alias for [`normal_form`] with [`DEFAULT_REWRITE_STEPS`].)
pub fn demodulate(rules: &[Rule], term: &Term) -> Term {
    normal_form(rules, term, DEFAULT_REWRITE_STEPS)
}

/// Enumerate the critical pairs of `rules`: for every way one rule's lhs overlaps
/// (unifies with) a non-variable subterm of another rule's lhs, the two divergent
/// rewrites of the overlap term. Overlaps of a rule with itself skip the root
/// position (which yields only the trivial identity pair). Deterministic order.
pub fn critical_pairs(rules: &[Rule]) -> Vec<(Term, Term)> {
    let mut out = Vec::new();
    for (i, r1) in rules.iter().enumerate() {
        for (j, r2) in rules.iter().enumerate() {
            // Rename r2 apart from r1 so their variables never clash.
            let r2r = rename_rule(r2, &format!("#{j}"));
            let same = i == j;
            for pos in nonvar_positions(&r1.lhs) {
                if same && pos.is_empty() {
                    continue;
                }
                let sub = subterm_at(&r1.lhs, &pos);
                if let Some(sigma) = unify(sub, &r2r.lhs) {
                    // Peak = r1.lhs σ. Reduce via r1 (root) and via r2 (at pos).
                    let left = apply_subst(&sigma, &r1.rhs);
                    let replaced = replace_at(&r1.lhs, &pos, r2r.rhs.clone());
                    let right = apply_subst(&sigma, &replaced);
                    if left != right {
                        out.push((left, right));
                    }
                }
            }
        }
    }
    out
}

/// Orient the equation `l = r` by `ordering` and add it to `rules` (deduplicated;
/// trivial `l = r` with `l == r` is dropped). Errors if the ordering cannot orient
/// the equation in either direction (KB completion "fails" — reported, not panicked).
fn add_oriented_equation(
    rules: &mut Vec<Rule>,
    ordering: &dyn TermOrdering,
    l: Term,
    r: Term,
) -> Result<()> {
    if l == r {
        return Ok(());
    }
    let rule = match ordering.compare(&l, &r) {
        Some(Ordering::Greater) => Rule { lhs: l, rhs: r },
        Some(Ordering::Less) => Rule { lhs: r, rhs: l },
        Some(Ordering::Equal) => return Ok(()),
        None => return Err(anyhow!("completion cannot orient equation: {} = {}", l, r)),
    };
    if !rules.iter().any(|x| *x == rule) {
        rules.push(rule);
    }
    Ok(())
}

/// Bounded Knuth–Bendix completion. Orients each input equation by `ordering`,
/// then repeatedly (up to `max_iters` rounds) computes critical pairs, normalises
/// each pair with the current rules, and orients every non-joinable pair into a
/// new rule — until no new rule is added (locally confluent) or a bound is hit.
///
/// Sound and terminating: every loop is bounded, and the result is always a valid
/// (if possibly not fully confluent, when a bound is reached) rewrite system.
/// Returns `Err` only when an equation cannot be oriented by `ordering`.
pub fn complete(
    equations: &[(Term, Term)],
    ordering: &dyn TermOrdering,
    max_iters: usize,
) -> Result<Vec<Rule>> {
    let mut rules: Vec<Rule> = Vec::new();
    for (l, r) in equations {
        add_oriented_equation(&mut rules, ordering, l.clone(), r.clone())?;
    }

    for _ in 0..max_iters {
        let cps = critical_pairs(&rules);
        let mut changed = false;
        for (s, t) in cps {
            let s2 = normal_form(&rules, &s, DEFAULT_REWRITE_STEPS);
            let t2 = normal_form(&rules, &t, DEFAULT_REWRITE_STEPS);
            if s2 == t2 {
                continue; // already joinable
            }
            let before = rules.len();
            add_oriented_equation(&mut rules, ordering, s2, t2)?;
            if rules.len() != before {
                changed = true;
            }
            if rules.len() > MAX_COMPLETION_RULES {
                return Ok(rules); // safety bound: give up with what we have
            }
        }
        if !changed {
            break; // no new rule ⇒ all critical pairs joinable
        }
    }
    Ok(rules)
}

// ---------------------------------------------------------------------------
// Congruence closure for ground equalities.
// ---------------------------------------------------------------------------

/// An injective structural key for a term (distinguishes variables from same-named
/// constants and encodes symbol + arity + child keys).
fn term_key(t: &Term) -> String {
    match t {
        Term::Var(x) => format!("V:{x}"),
        Term::App(f, args) => {
            let mut s = format!("A:{}/{}", f, args.len());
            for a in args {
                s.push('(');
                s.push_str(&term_key(a));
                s.push(')');
            }
            s
        }
    }
}

fn uf_find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

fn uf_union(parent: &mut [usize], x: usize, y: usize) {
    let a = uf_find(parent, x);
    let b = uf_find(parent, y);
    if a != b {
        parent[a] = b;
    }
}

/// Decide a ground equality by congruence closure: given a set of ground
/// `equations`, is `query.0 = query.1` entailed? Uses union–find over all subterms
/// plus a congruence fixpoint (equal arguments ⇒ equal applications). Deterministic.
///
/// All terms are assumed ground (no variables); variables, if present, are treated
/// as opaque constants.
pub fn congruence_closure(equations: &[(Term, Term)], query: &(Term, Term)) -> bool {
    let mut ids: BTreeMap<String, usize> = BTreeMap::new();
    // For each node: Some((symbol, child ids)) if an application, None if a variable.
    let mut children: Vec<Option<(String, Vec<usize>)>> = Vec::new();

    fn intern(
        t: &Term,
        ids: &mut BTreeMap<String, usize>,
        children: &mut Vec<Option<(String, Vec<usize>)>>,
    ) -> usize {
        let child_ids: Option<(String, Vec<usize>)> = match t {
            Term::App(f, args) => {
                let cs: Vec<usize> = args.iter().map(|a| intern(a, ids, children)).collect();
                Some((f.clone(), cs))
            }
            Term::Var(_) => None,
        };
        let key = term_key(t);
        if let Some(&i) = ids.get(&key) {
            return i;
        }
        let i = children.len();
        children.push(child_ids);
        ids.insert(key, i);
        i
    }

    for (l, r) in equations {
        intern(l, &mut ids, &mut children);
        intern(r, &mut ids, &mut children);
    }
    let q0 = intern(&query.0, &mut ids, &mut children);
    let q1 = intern(&query.1, &mut ids, &mut children);

    let n = children.len();
    let mut parent: Vec<usize> = (0..n).collect();

    for (l, r) in equations {
        let a = ids[&term_key(l)];
        let b = ids[&term_key(r)];
        uf_union(&mut parent, a, b);
    }

    // Congruence fixpoint: if two applications share symbol+arity and all their
    // arguments are already congruent, merge them. Bounded by n merges.
    loop {
        let mut changed = false;
        for i in 0..n {
            for j in (i + 1)..n {
                if let (Some((f, ci)), Some((g, cj))) = (&children[i], &children[j]) {
                    if f == g && ci.len() == cj.len() {
                        let fi = uf_find(&mut parent, i);
                        let fj = uf_find(&mut parent, j);
                        if fi != fj
                            && ci
                                .iter()
                                .zip(cj)
                                .all(|(&x, &y)| uf_find(&mut parent, x) == uf_find(&mut parent, y))
                        {
                            uf_union(&mut parent, i, j);
                            changed = true;
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    uf_find(&mut parent, q0) == uf_find(&mut parent, q1)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Small constructors for readable tests.
    fn v(name: &str) -> Term {
        Term::var(name)
    }
    fn c(name: &str) -> Term {
        Term::constant(name)
    }
    fn app(sym: &str, args: Vec<Term>) -> Term {
        Term::app(sym, args)
    }

    // ---- #13 unification / matching / subsumption ----

    #[test]
    fn unify_succeeds_and_produces_an_mgu() {
        // f(x, a) vs f(b, y) ⇒ x↦b, y↦a; applying the mgu makes both sides equal.
        let s = app("f", vec![v("x"), c("a")]);
        let t = app("f", vec![c("b"), v("y")]);
        let mgu = unify(&s, &t).expect("f(x,a) and f(b,y) must unify");
        assert_eq!(apply_subst(&mgu, &s), apply_subst(&mgu, &t));
        assert_eq!(apply_subst(&mgu, &s), app("f", vec![c("b"), c("a")]));
    }

    #[test]
    fn unify_occurs_check_rejects_cyclic() {
        // x vs f(x) must fail the occurs-check (no finite unifier).
        assert!(unify(&v("x"), &app("f", vec![v("x")])).is_none());
    }

    #[test]
    fn unify_respects_nonlinearity() {
        // f(x,x) vs f(a,b): x cannot be both a and b ⇒ no unifier.
        let s = app("f", vec![v("x"), v("x")]);
        assert!(unify(&s, &app("f", vec![c("a"), c("b")])).is_none());
        // f(x,x) vs f(a,a) unifies with x↦a.
        assert!(unify(&s, &app("f", vec![c("a"), c("a")])).is_some());
    }

    #[test]
    fn unify_clashing_symbols_fail() {
        assert!(unify(&c("a"), &c("b")).is_none());
        assert!(unify(&app("f", vec![v("x")]), &app("g", vec![v("x")])).is_none());
    }

    #[test]
    fn subsumption_is_sound_and_directional() {
        // The general p(x) subsumes the instance p(a); the instance does not
        // subsume the general term.
        let general = app("p", vec![v("x")]);
        let specific = app("p", vec![c("a")]);
        assert!(subsumes(&general, &specific));
        assert!(!subsumes(&specific, &general));

        // Matching binds the pattern variable to the instance's subterm.
        let sigma = matches(&general, &specific).unwrap();
        assert_eq!(sigma.get("x"), Some(&c("a")));

        // A nonlinear pattern p(x,x) subsumes p(a,a) but NOT p(a,b).
        let nl = app("p", vec![v("x"), v("x")]);
        assert!(subsumes(&nl, &app("p", vec![c("a"), c("a")])));
        assert!(!subsumes(&nl, &app("p", vec![c("a"), c("b")])));
    }

    // ---- #14 orderings ----

    /// Group-theory precedence: `*` > `i` (inverse) > `e` (unit).
    fn group_prec() -> Precedence {
        Precedence::with_order(&["*", "i", "e"])
    }

    #[test]
    fn lpo_orients_group_axioms_left_to_right() {
        let p = group_prec();
        // x * e  >  x
        let l1 = app("*", vec![v("x"), c("e")]);
        assert_eq!(lpo(&p, &l1, &v("x")), Some(Ordering::Greater));
        // x * i(x)  >  e
        let l2 = app("*", vec![v("x"), app("i", vec![v("x")])]);
        assert_eq!(lpo(&p, &l2, &c("e")), Some(Ordering::Greater));
        // Reverse directions are strictly Less (consistent orientation).
        assert_eq!(lpo(&p, &v("x"), &l1), Some(Ordering::Less));
        assert_eq!(lpo(&p, &c("e"), &l2), Some(Ordering::Less));
    }

    #[test]
    fn lpo_is_irreflexive_and_total_on_ground() {
        let p = group_prec();
        let t = app("*", vec![c("a"), c("b")]);
        assert_eq!(lpo(&p, &t, &t), Some(Ordering::Equal)); // irreflexive: never Greater/Less
                                                            // Distinct ground terms are comparable (total-ish on ground terms).
        assert_eq!(lpo(&p, &t, &c("a")), Some(Ordering::Greater));
        assert!(lpo(&p, &t, &c("a")).is_some());
        assert!(lpo(&p, &c("a"), &c("b")).is_some());
    }

    #[test]
    fn kbo_orients_group_axioms_left_to_right() {
        let w = KboWeights::new(group_prec());
        let l1 = app("*", vec![v("x"), c("e")]);
        assert_eq!(kbo(&w, &l1, &v("x")), Some(Ordering::Greater));
        let l2 = app("*", vec![v("x"), app("i", vec![v("x")])]);
        assert_eq!(kbo(&w, &l2, &c("e")), Some(Ordering::Greater));
        // Irreflexive; distinct variables are incomparable (KBO is partial).
        assert_eq!(kbo(&w, &l1, &l1), Some(Ordering::Equal));
        assert_eq!(kbo(&w, &v("x"), &v("y")), None);
    }

    #[test]
    fn multiset_extension_dershowitz_manna() {
        let p = Precedence::with_order(&["f"]);
        let cmp = |a: &Term, b: &Term| lpo(&p, a, b);
        // {f(a), b} > {a, b}: f(a) replaces a with a strictly-smaller term.
        let m = [app("f", vec![c("a")]), c("b")];
        let n = [c("a"), c("b")];
        assert_eq!(multiset_compare(&cmp, &m, &n), Some(Ordering::Greater));
        assert_eq!(multiset_compare(&cmp, &n, &m), Some(Ordering::Less));
        // Reordering is invariant ⇒ Equal.
        assert_eq!(
            multiset_compare(&cmp, &[c("a"), c("b")], &[c("b"), c("a")]),
            Some(Ordering::Equal)
        );
    }

    // ---- #15 rewriting / completion / congruence closure ----

    #[test]
    fn critical_pairs_and_completion_reach_confluence() {
        // Complete { f(f(x)) = a }. The self-overlap yields the non-joinable pair
        // (a, f(a)); completion must add f(a) -> a to close it.
        let ord = LpoOrdering {
            precedence: Precedence::with_order(&["f", "a"]),
        };
        let eqs = vec![(app("f", vec![app("f", vec![v("x")])]), c("a"))];
        let rules = complete(&eqs, &ord, 20).expect("this system completes");

        // A new rule was added beyond the single input rule.
        assert!(
            rules.len() >= 2,
            "completion should add f(a) -> a (got {rules:?})"
        );
        assert!(
            rules
                .iter()
                .any(|r| r.lhs == app("f", vec![c("a")]) && r.rhs == c("a")),
            "expected the derived rule f(a) -> a"
        );

        // Confluence witness: every critical pair of the completed set is joinable.
        for (s, t) in critical_pairs(&rules) {
            let s2 = normal_form(&rules, &s, DEFAULT_REWRITE_STEPS);
            let t2 = normal_form(&rules, &t, DEFAULT_REWRITE_STEPS);
            assert_eq!(s2, t2, "critical pair ({s}, {t}) must be joinable");
        }

        // f(f(f(a))) normalises to a.
        let deep = app("f", vec![app("f", vec![app("f", vec![c("a")])])]);
        assert_eq!(normal_form(&rules, &deep, DEFAULT_REWRITE_STEPS), c("a"));
    }

    #[test]
    fn completion_is_deterministic() {
        let ord = LpoOrdering {
            precedence: Precedence::with_order(&["f", "a"]),
        };
        let eqs = vec![(app("f", vec![app("f", vec![v("x")])]), c("a"))];
        let r1 = complete(&eqs, &ord, 20).unwrap();
        let r2 = complete(&eqs, &ord, 20).unwrap();
        assert_eq!(r1, r2, "completion must be byte-identical run to run");
    }

    #[test]
    fn completion_reports_unorientable_equation() {
        // x = y cannot be oriented by any precedence-based order ⇒ Err, not panic.
        let ord = LpoOrdering {
            precedence: Precedence::new(),
        };
        let eqs = vec![(v("x"), v("y"))];
        assert!(complete(&eqs, &ord, 20).is_err());
    }

    #[test]
    fn demodulate_normalises_with_rules() {
        // g(x) -> x collapses nested g's.
        let rules = vec![Rule::new(app("g", vec![v("x")]), v("x"))];
        let t = app("g", vec![app("g", vec![c("a")])]);
        assert_eq!(demodulate(&rules, &t), c("a"));
    }

    #[test]
    fn congruence_closure_decides_ground_equality() {
        // a=b, f(a)=c  ⊢  f(b) = c   (by congruence a~b ⇒ f(a)~f(b)).
        let eqs = vec![(c("a"), c("b")), (app("f", vec![c("a")]), c("c"))];
        assert!(congruence_closure(&eqs, &(app("f", vec![c("b")]), c("c"))));
        // But a = c is NOT entailed.
        assert!(!congruence_closure(&eqs, &(c("a"), c("c"))));

        // Transitivity: a=b, b=d ⊢ a=d.
        let chain = vec![(c("a"), c("b")), (c("b"), c("d"))];
        assert!(congruence_closure(&chain, &(c("a"), c("d"))));
        assert!(!congruence_closure(&chain, &(c("a"), c("z"))));
    }
}
