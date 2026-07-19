//! HOL-Light-style primitive-inference proof log + an INDEPENDENT reference
//! checker (BUILD #17).
//!
//! ## Why this exists (the LCF / de Bruijn insight)
//!
//! An LCF prover *is* a de Bruijn proof checker with an *ephemeral* proof: the
//! only way to build a value of the abstract `thm` type is to call a kernel
//! primitive, so a `thm` is a certificate that some sequence of primitive
//! inferences was performed — but that sequence is thrown away. If instead we
//! *persist* each primitive-inference step, we get an independent, replayable
//! proof object: search can happen ANYWHERE (a neural prover, a hammer, a
//! cross-system translation), and trust rests solely on a small checker that
//! re-applies the primitives. This is the "search-anywhere, check-in-kernel"
//! pattern.
//!
//! This module is the Rust side of that idea:
//!
//! * a serde-serializable proof-log FORMAT ([`Proof`] = an ordered list of
//!   [`Step`]s, each naming a primitive rule + its premises by index +
//!   parameters), and
//! * an INDEPENDENT reference CHECKER ([`check_proof`]) that re-derives every
//!   primitive inference of HOL Light's kernel (`fusion.ml`) FROM FIRST
//!   PRINCIPLES — including capture-avoiding substitution for `INST`/`INST_TYPE`
//!   and alpha-correct `ABS`/`BETA` — and returns the final sequent, or an error
//!   if any step is invalid.
//!
//! ## Trust boundary
//!
//! This checker is deliberately `std`-only, written from first principles (only
//! `serde` is pulled in, and only for (de)serialization of the log — never for
//! any logical judgment). It is the offline stand-in for a CakeML-verified
//! checker: HOL Light's kernel has been proven sound in HOL4 down to
//! CakeML-compiled machine code (the `Candle` backend), and the intended upgrade
//! path is to emit this same log format and feed it to that verified checker.
//! Until then, this small audited Rust re-checker is the trust root.
//!
//! ## Emission
//!
//! [`Proof`] round-trips to JSON, so a search backend (e.g. the Candle adapter
//! in [`crate::prover::backends::candle`]) can EMIT this log. Live emission from
//! a running HOL Light image is toolchain-gated (needs the HOL4/PolyML/CakeML
//! stack), exactly like the live Candle gate; this module provides the format
//! and the checker so the offline path is complete and testable without that
//! toolchain.
//!
//! ## Primitive rules implemented
//!
//! `REFL`, `MK_COMB`, `ABS`, `BETA`, `ASSUME`, `EQ_MP`, `DEDUCT_ANTISYM_RULE`,
//! `INST`, `INST_TYPE` — the HOL Light primitive inference rules (minus `TRANS`,
//! which is itself derivable and outside this build's scope). Hypothesis sets are
//! compared up to alpha-equivalence, matching the kernel's `term_union`.

use serde::{Deserialize, Serialize};

// ===========================================================================
// Types: HOL types, terms, sequents.
// ===========================================================================

/// A HOL type: a type variable, or a type operator applied to argument types.
/// Function types are `Tyapp("fun", [dom, rng])`; `bool` is `Tyapp("bool", [])`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HolType {
    /// A type variable, e.g. `A`.
    Tyvar(String),
    /// A type-operator application, e.g. `fun`, `bool`, `list`.
    Tyapp(String, Vec<HolType>),
}

impl HolType {
    /// A type variable named `name`.
    pub fn var(name: &str) -> HolType {
        HolType::Tyvar(name.to_string())
    }

    /// A type-operator application `name(args...)`.
    pub fn app(name: &str, args: Vec<HolType>) -> HolType {
        HolType::Tyapp(name.to_string(), args)
    }

    /// The function type `dom -> rng`.
    pub fn fun(dom: HolType, rng: HolType) -> HolType {
        HolType::Tyapp("fun".to_string(), vec![dom, rng])
    }

    /// The `bool` type.
    pub fn boolean() -> HolType {
        HolType::Tyapp("bool".to_string(), Vec::new())
    }
}

/// A HOL term: variable, constant, application (`Comb`), or abstraction (`Abs`).
/// In a well-formed [`Term::Abs`] the bound position is a [`Term::Var`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Term {
    /// A variable with its type.
    Var(String, HolType),
    /// A constant with its type (e.g. `=` at some instance type).
    Const(String, HolType),
    /// Application `f x`.
    Comb(Box<Term>, Box<Term>),
    /// Abstraction `\v. body` (the first field is the bound [`Term::Var`]).
    Abs(Box<Term>, Box<Term>),
}

impl Term {
    /// A variable term.
    pub fn var(name: &str, ty: HolType) -> Term {
        Term::Var(name.to_string(), ty)
    }

    /// A constant term.
    pub fn constant(name: &str, ty: HolType) -> Term {
        Term::Const(name.to_string(), ty)
    }

    /// The application `f x`.
    pub fn comb(f: Term, x: Term) -> Term {
        Term::Comb(Box::new(f), Box::new(x))
    }

    /// The abstraction `\v. body`.
    pub fn abs(v: Term, body: Term) -> Term {
        Term::Abs(Box::new(v), Box::new(body))
    }

    /// The polymorphic equality constant `=` at operand type `ty`
    /// (type `ty -> ty -> bool`).
    pub fn eq_const(ty: HolType) -> Term {
        Term::Const(
            "=".to_string(),
            HolType::fun(ty.clone(), HolType::fun(ty, HolType::boolean())),
        )
    }
}

/// A sequent: a set of hypotheses (compared up to alpha-equivalence, as HOL
/// Light does) entailing a conclusion. `{h1, ..} |- concl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sequent {
    /// The hypotheses (an alpha-equivalence set; order is derivation order).
    pub hyps: Vec<Term>,
    /// The conclusion.
    pub concl: Term,
}

/// A term-for-variable instantiation entry (for `INST`): replace the free
/// variable [`var`](Instantiation::var) with [`term`](Instantiation::term).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instantiation {
    /// The variable to replace (must be a [`Term::Var`]).
    pub var: Term,
    /// The replacement term (must have the same type as `var`).
    pub term: Term,
}

/// A type-for-type-variable instantiation entry (for `INST_TYPE`): replace the
/// type variable named [`var`](TypeInstantiation::var) with
/// [`ty`](TypeInstantiation::ty).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeInstantiation {
    /// The type-variable name to replace.
    pub var: String,
    /// The replacement type.
    pub ty: HolType,
}

/// One primitive-inference step. Premises are referenced BY INDEX into the
/// enclosing [`Proof::steps`] list; an index must point to a strictly earlier
/// step (no forward or self references).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Step {
    /// `REFL t`: `|- t = t`.
    Refl { term: Term },
    /// `MK_COMB (|- l1 = r1) (|- l2 = r2)`: `|- (l1 l2) = (r1 r2)`.
    MkComb { left: usize, right: usize },
    /// `ABS v (|- l = r)`: `|- (\v. l) = (\v. r)`, provided `v` is not free in
    /// any hypothesis.
    Abs { var: Term, eq: usize },
    /// `BETA ((\v. bod) v)`: `|- (\v. bod) v = bod` (the trivial redex where the
    /// argument IS the bound variable — general beta is derived, not primitive).
    Beta { term: Term },
    /// `ASSUME p` (`p : bool`): `{p} |- p`.
    Assume { term: Term },
    /// `EQ_MP (|- l = r) (|- l')` with `l' ` alpha-equal `l`: `|- r`.
    EqMp { eq: usize, thm: usize },
    /// `DEDUCT_ANTISYM_RULE (A1 |- c1) (A2 |- c2)`:
    /// `(A1 - {c2}) u (A2 - {c1}) |- c1 = c2`.
    DeductAntisym { left: usize, right: usize },
    /// `INST theta (A |- c)`: capture-avoiding term substitution over the whole
    /// sequent.
    Inst {
        subst: Vec<Instantiation>,
        thm: usize,
    },
    /// `INST_TYPE theta (A |- c)`: capture-avoiding type substitution over the
    /// whole sequent.
    InstType {
        subst: Vec<TypeInstantiation>,
        thm: usize,
    },
}

/// A proof log: an ordered list of primitive-inference [`Step`]s. The sequent
/// derived by the LAST step is the theorem the log proves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proof {
    pub steps: Vec<Step>,
}

impl Proof {
    /// Construct a proof from a list of steps.
    pub fn new(steps: Vec<Step>) -> Proof {
        Proof { steps }
    }
}

// ===========================================================================
// Error type (std-only; no anyhow at the trust boundary).
// ===========================================================================

/// A checker rejection: an invalid or ill-formed proof step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckError(pub String);

impl CheckError {
    fn new(msg: impl Into<String>) -> CheckError {
        CheckError(msg.into())
    }
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "proof-log check failed: {}", self.0)
    }
}

impl std::error::Error for CheckError {}

/// The checker's result type.
pub type CheckResult<T> = Result<T, CheckError>;

// ===========================================================================
// Term utilities re-derived from first principles.
// ===========================================================================

/// The type of a term (`type_of` in `fusion.ml`). Fails on an ill-formed term
/// (a non-function operator applied, or an abstraction whose binder is not a
/// variable).
fn type_of(tm: &Term) -> CheckResult<HolType> {
    match tm {
        Term::Var(_, ty) | Term::Const(_, ty) => Ok(ty.clone()),
        Term::Comb(s, _) => match type_of(s)? {
            HolType::Tyapp(ref name, ref args) if name == "fun" && args.len() == 2 => {
                Ok(args[1].clone())
            }
            other => Err(CheckError::new(format!(
                "type_of: operator does not have a function type: {other:?}"
            ))),
        },
        Term::Abs(v, body) => match v.as_ref() {
            Term::Var(_, ty) => Ok(HolType::fun(ty.clone(), type_of(body)?)),
            other => Err(CheckError::new(format!(
                "type_of: abstraction binder is not a variable: {other:?}"
            ))),
        },
    }
}

/// Build `l = r` at the operands' shared type (`safe_mk_eq`). Fails if the two
/// sides do not have the same type.
fn mk_eq(l: &Term, r: &Term) -> CheckResult<Term> {
    let lty = type_of(l)?;
    let rty = type_of(r)?;
    if lty != rty {
        return Err(CheckError::new(format!(
            "mk_eq: type mismatch between sides: {lty:?} vs {rty:?}"
        )));
    }
    let eqc = Term::eq_const(lty);
    Ok(Term::comb(Term::comb(eqc, l.clone()), r.clone()))
}

/// Destructure an equation `l = r`, returning `(l, r)`.
fn dest_eq(tm: &Term) -> CheckResult<(&Term, &Term)> {
    if let Term::Comb(f, r) = tm {
        if let Term::Comb(op, l) = f.as_ref() {
            if let Term::Const(name, _) = op.as_ref() {
                if name == "=" {
                    return Ok((l.as_ref(), r.as_ref()));
                }
            }
        }
    }
    Err(CheckError::new(format!("expected an equation, got {tm:?}")))
}

/// Is variable `v` free in `tm`? (`vfree_in`.)
fn vfree_in(v: &Term, tm: &Term) -> bool {
    match tm {
        Term::Abs(bv, body) => v != bv.as_ref() && vfree_in(v, body),
        Term::Comb(s, t) => vfree_in(v, s) || vfree_in(v, t),
        _ => v == tm,
    }
}

/// The free variables of `tm`, in first-occurrence order (`frees`).
fn frees(tm: &Term) -> Vec<Term> {
    match tm {
        Term::Var(_, _) => vec![tm.clone()],
        Term::Const(_, _) => Vec::new(),
        Term::Comb(s, t) => {
            let mut out = frees(s);
            for x in frees(t) {
                if !out.contains(&x) {
                    out.push(x);
                }
            }
            out
        }
        Term::Abs(v, body) => frees(body)
            .into_iter()
            .filter(|x| x != v.as_ref())
            .collect(),
    }
}

/// Alpha-equivalence of two terms (`aconv` / `alphaorder = 0`). `env` pairs the
/// bound variables encountered so far (innermost last).
fn alpha_eq(env: &mut Vec<(Term, Term)>, s: &Term, t: &Term) -> bool {
    match (s, t) {
        (Term::Var(_, _), Term::Var(_, _)) => {
            // Scan from the innermost binder outward.
            for (b1, b2) in env.iter().rev() {
                let m1 = b1 == s;
                let m2 = b2 == t;
                if m1 && m2 {
                    return true; // corresponding bound occurrences
                }
                if m1 || m2 {
                    return false; // bound at different depths
                }
            }
            s == t // both free: must be identical (name + type)
        }
        (Term::Const(_, _), Term::Const(_, _)) => s == t,
        (Term::Comb(f1, x1), Term::Comb(f2, x2)) => alpha_eq(env, f1, f2) && alpha_eq(env, x1, x2),
        (Term::Abs(v1, b1), Term::Abs(v2, b2)) => {
            // Bound variables must have the same type to be alpha-equivalent.
            let types_match = match (v1.as_ref(), v2.as_ref()) {
                (Term::Var(_, t1), Term::Var(_, t2)) => t1 == t2,
                _ => return s == t,
            };
            if !types_match {
                return false;
            }
            env.push((v1.as_ref().clone(), v2.as_ref().clone()));
            let r = alpha_eq(env, b1, b2);
            env.pop();
            r
        }
        _ => false,
    }
}

/// Top-level alpha-equivalence.
fn aconv(s: &Term, t: &Term) -> bool {
    alpha_eq(&mut Vec::new(), s, t)
}

/// Union of two hypothesis sets up to alpha-equivalence (`term_union`).
fn term_union(a: &[Term], b: &[Term]) -> Vec<Term> {
    let mut out = a.to_vec();
    for t in b {
        if !out.iter().any(|x| aconv(x, t)) {
            out.push(t.clone());
        }
    }
    out
}

/// Remove every alpha-equivalent occurrence of `t` from `list` (`term_remove`).
fn term_remove(t: &Term, list: &[Term]) -> Vec<Term> {
    list.iter().filter(|x| !aconv(x, t)).cloned().collect()
}

/// De-duplicate a hypothesis list up to alpha-equivalence, preserving order
/// (`term_image` set-ification after a substitution).
fn dedup_conv(list: Vec<Term>) -> Vec<Term> {
    let mut out: Vec<Term> = Vec::new();
    for t in list {
        if !out.iter().any(|x| aconv(x, &t)) {
            out.push(t);
        }
    }
    out
}

/// A fresh variable derived from `v` by adding primes to its name until it is
/// not free in any term of `avoid` (`variant`).
fn variant(avoid: &[Term], v: &Term) -> Term {
    if !avoid.iter().any(|t| vfree_in(v, t)) {
        return v.clone();
    }
    match v {
        Term::Var(n, ty) => variant(avoid, &Term::Var(format!("{n}'"), ty.clone())),
        _ => v.clone(),
    }
}

// ===========================================================================
// Capture-avoiding term substitution (INST): `vsubst`.
// ===========================================================================

/// Capture-avoiding substitution of terms for free variables (`vsubst`). Each
/// entry replaces a [`Term::Var`] with a same-typed term. Fails if any target is
/// not a variable or the replacement's type differs.
pub fn vsubst(theta: &[Instantiation], tm: &Term) -> CheckResult<Term> {
    for ins in theta {
        match &ins.var {
            Term::Var(_, vty) => {
                let tty = type_of(&ins.term)?;
                if *vty != tty {
                    return Err(CheckError::new(format!(
                        "INST: replacement type {tty:?} does not match variable type {vty:?}"
                    )));
                }
            }
            other => {
                return Err(CheckError::new(format!(
                    "INST: substitution target is not a variable: {other:?}"
                )));
            }
        }
    }
    Ok(vsubst_rec(theta, tm))
}

/// The recursive core of [`vsubst`]. On entering an abstraction it (1) drops any
/// substitution whose target is the bound variable (it is no longer free), and
/// (2) if a surviving substitution would drag a term in which the bound variable
/// occurs free INTO the scope of that binder — the capture condition — it renames
/// the binder to a fresh [`variant`] first, so no free variable is ever captured.
fn vsubst_rec(ilist: &[Instantiation], tm: &Term) -> Term {
    match tm {
        Term::Var(_, _) => ilist
            .iter()
            .find(|i| &i.var == tm)
            .map(|i| i.term.clone())
            .unwrap_or_else(|| tm.clone()),
        Term::Const(_, _) => tm.clone(),
        Term::Comb(s, t) => Term::comb(vsubst_rec(ilist, s), vsubst_rec(ilist, t)),
        Term::Abs(v, body) => {
            // Substitutions targeting the bound var no longer apply inside.
            let ilist2: Vec<Instantiation> =
                ilist.iter().filter(|i| i.var != **v).cloned().collect();
            if ilist2.is_empty() {
                return tm.clone();
            }
            let body2 = vsubst_rec(&ilist2, body);
            // Capture: some firing substitution replaces a variable that occurs
            // free in the body with a term in which the bound variable is free.
            let needs_rename = ilist2
                .iter()
                .any(|i| vfree_in(v, &i.term) && vfree_in(&i.var, body));
            if needs_rename {
                let fresh = variant(std::slice::from_ref(&body2), v.as_ref());
                let mut ilist3 = ilist2.clone();
                // Also rename the binder occurrences: v -> fresh in the body.
                ilist3.push(Instantiation {
                    var: v.as_ref().clone(),
                    term: fresh.clone(),
                });
                Term::Abs(Box::new(fresh), Box::new(vsubst_rec(&ilist3, body)))
            } else {
                Term::Abs(v.clone(), Box::new(body2))
            }
        }
    }
}

// ===========================================================================
// Capture-avoiding type substitution (INST_TYPE): `inst` with the Clash rule.
// ===========================================================================

/// Substitute types for type variables inside a type (`type_subst`).
fn type_subst(tyin: &[TypeInstantiation], ty: &HolType) -> HolType {
    match ty {
        HolType::Tyvar(n) => tyin
            .iter()
            .find(|t| &t.var == n)
            .map(|t| t.ty.clone())
            .unwrap_or_else(|| ty.clone()),
        HolType::Tyapp(n, args) => HolType::Tyapp(
            n.clone(),
            args.iter().map(|a| type_subst(tyin, a)).collect(),
        ),
    }
}

/// Internal signal that a type instantiation would capture the named variable —
/// HOL Light's `Clash` exception, modeled as an error case.
enum Clash {
    At(Term),
}

/// Capture-avoiding type instantiation over a term (`inst`). Because
/// instantiating types can make two variables that differ only in type become
/// identical, a bound variable can suddenly capture a free one; the `env` tracks
/// each binder's pre/post-instantiation form and a [`Clash`] is raised when an
/// instantiated variable coincides with a DIFFERENT original binder, triggering
/// an alpha-rename of the offending binder.
fn inst_rec(env: &[(Term, Term)], tyin: &[TypeInstantiation], tm: &Term) -> Result<Term, Clash> {
    match tm {
        Term::Var(n, ty) => {
            let ty2 = type_subst(tyin, ty);
            let tm2 = Term::Var(n.clone(), ty2);
            // rev_assocd tm2 env tm: if the instantiated variable is the image of
            // some bound variable, that bound variable must be THIS one.
            let looked = env
                .iter()
                .find(|(_, b)| *b == tm2)
                .map(|(a, _)| a.clone())
                .unwrap_or_else(|| tm.clone());
            if looked == *tm {
                Ok(tm2)
            } else {
                Err(Clash::At(tm2))
            }
        }
        Term::Const(n, ty) => Ok(Term::Const(n.clone(), type_subst(tyin, ty))),
        Term::Comb(f, x) => Ok(Term::comb(inst_rec(env, tyin, f)?, inst_rec(env, tyin, x)?)),
        Term::Abs(y, t) => {
            let y2 = inst_rec(&[], tyin, y)?;
            // (y, y2) :: env  — innermost binder first.
            let mut env2 = Vec::with_capacity(env.len() + 1);
            env2.push((y.as_ref().clone(), y2.clone()));
            env2.extend_from_slice(env);
            match inst_rec(&env2, tyin, t) {
                Ok(t2) => Ok(Term::Abs(Box::new(y2), Box::new(t2))),
                Err(Clash::At(w)) => {
                    if w != y2 {
                        // A clash from a deeper binder; propagate outward.
                        return Err(Clash::At(w));
                    }
                    // The freshly instantiated binder captured a free variable:
                    // rename the binder to a variant fresh for the instantiated
                    // free variables of the body, then redo the abstraction.
                    let mut ifrees: Vec<Term> = Vec::new();
                    for f in frees(t) {
                        ifrees.push(inst_rec(&[], tyin, &f)?);
                    }
                    let fresh = variant(&ifrees, &y2);
                    // z keeps the ORIGINAL (un-instantiated) binder type so the
                    // rename substitution over `t` type-checks.
                    let z = match (&fresh, y.as_ref()) {
                        (Term::Var(nm, _), Term::Var(_, oty)) => Term::Var(nm.clone(), oty.clone()),
                        _ => return Err(Clash::At(fresh)),
                    };
                    let renamed = vsubst_rec(
                        &[Instantiation {
                            var: y.as_ref().clone(),
                            term: z.clone(),
                        }],
                        t,
                    );
                    inst_rec(env, tyin, &Term::Abs(Box::new(z), Box::new(renamed)))
                }
            }
        }
    }
}

/// Capture-avoiding type instantiation over a term (public entry). A `Clash` that
/// escapes to the top means the term was malformed (a captured variable with no
/// enclosing binder) and is reported as an error.
pub fn inst_type(tyin: &[TypeInstantiation], tm: &Term) -> CheckResult<Term> {
    match inst_rec(&[], tyin, tm) {
        Ok(t) => Ok(t),
        Err(Clash::At(w)) => Err(CheckError::new(format!(
            "INST_TYPE: unresolved capture at {w:?}"
        ))),
    }
}

// ===========================================================================
// The checker.
// ===========================================================================

/// Fetch a strictly-earlier premise sequent, rejecting forward/self references.
fn premise<'a>(results: &'a [Sequent], idx: usize, cur: usize) -> CheckResult<&'a Sequent> {
    if idx >= cur {
        return Err(CheckError::new(format!(
            "step {cur}: premise index {idx} is not a strictly-earlier step"
        )));
    }
    results
        .get(idx)
        .ok_or_else(|| CheckError::new(format!("step {cur}: premise index {idx} out of range")))
}

/// Apply one primitive-inference [`Step`], given the sequents derived by the
/// earlier steps, and return the resulting sequent — or an error if the step is
/// not a valid application of its rule.
fn apply_step(step: &Step, results: &[Sequent], cur: usize) -> CheckResult<Sequent> {
    match step {
        // REFL t : |- t = t
        Step::Refl { term } => Ok(Sequent {
            hyps: Vec::new(),
            concl: mk_eq(term, term)?,
        }),

        // MK_COMB (|- l1 = r1) (|- l2 = r2) : |- (l1 l2) = (r1 r2)
        Step::MkComb { left, right } => {
            let s1 = premise(results, *left, cur)?;
            let s2 = premise(results, *right, cur)?;
            let (l1, r1) = dest_eq(&s1.concl)?;
            let (l2, r2) = dest_eq(&s2.concl)?;
            // Type agreement: r1 must be a function whose domain is r2's type.
            let r1_ty = type_of(r1)?;
            let r2_ty = type_of(r2)?;
            let types_agree = matches!(
                &r1_ty,
                HolType::Tyapp(name, args) if name == "fun" && args.len() == 2 && args[0] == r2_ty
            );
            if !types_agree {
                return Err(CheckError::new(format!(
                    "MK_COMB: types do not agree (operator type {r1_ty:?}, argument type {r2_ty:?})"
                )));
            }
            let concl = mk_eq(
                &Term::comb(l1.clone(), l2.clone()),
                &Term::comb(r1.clone(), r2.clone()),
            )?;
            Ok(Sequent {
                hyps: term_union(&s1.hyps, &s2.hyps),
                concl,
            })
        }

        // ABS v (|- l = r) : |- (\v. l) = (\v. r)   (v not free in hyps)
        Step::Abs { var, eq } => {
            if !matches!(var, Term::Var(_, _)) {
                return Err(CheckError::new(format!(
                    "ABS: bound term is not a variable: {var:?}"
                )));
            }
            let s = premise(results, *eq, cur)?;
            if s.hyps.iter().any(|h| vfree_in(var, h)) {
                return Err(CheckError::new(
                    "ABS: the abstracted variable is free in a hypothesis",
                ));
            }
            let (l, r) = dest_eq(&s.concl)?;
            let concl = mk_eq(
                &Term::abs(var.clone(), l.clone()),
                &Term::abs(var.clone(), r.clone()),
            )?;
            Ok(Sequent {
                hyps: s.hyps.clone(),
                concl,
            })
        }

        // BETA ((\v. bod) v) : |- (\v. bod) v = bod
        Step::Beta { term } => match term {
            Term::Comb(f, arg) => match f.as_ref() {
                Term::Abs(v, bod) if arg.as_ref() == v.as_ref() => Ok(Sequent {
                    hyps: Vec::new(),
                    concl: mk_eq(term, bod)?,
                }),
                _ => Err(CheckError::new(
                    "BETA: not a trivial beta-redex ((\\v. bod) v with argument = bound var)",
                )),
            },
            _ => Err(CheckError::new("BETA: term is not an application")),
        },

        // ASSUME p (p : bool) : {p} |- p
        Step::Assume { term } => {
            if type_of(term)? != HolType::boolean() {
                return Err(CheckError::new("ASSUME: term is not a proposition (bool)"));
            }
            Ok(Sequent {
                hyps: vec![term.clone()],
                concl: term.clone(),
            })
        }

        // EQ_MP (|- l = r) (|- l')  with l' alpha l : |- r
        Step::EqMp { eq, thm } => {
            let s_eq = premise(results, *eq, cur)?;
            let s_th = premise(results, *thm, cur)?;
            let (l, r) = dest_eq(&s_eq.concl)?;
            if !aconv(l, &s_th.concl) {
                return Err(CheckError::new(
                    "EQ_MP: the second theorem's conclusion is not alpha-equal to the equation's LHS",
                ));
            }
            Ok(Sequent {
                hyps: term_union(&s_eq.hyps, &s_th.hyps),
                concl: r.clone(),
            })
        }

        // DEDUCT_ANTISYM_RULE (A1 |- c1) (A2 |- c2)
        //   : (A1 - {c2}) u (A2 - {c1}) |- c1 = c2
        Step::DeductAntisym { left, right } => {
            let s1 = premise(results, *left, cur)?;
            let s2 = premise(results, *right, cur)?;
            let asl1 = term_remove(&s2.concl, &s1.hyps);
            let asl2 = term_remove(&s1.concl, &s2.hyps);
            let concl = mk_eq(&s1.concl, &s2.concl)?;
            Ok(Sequent {
                hyps: term_union(&asl1, &asl2),
                concl,
            })
        }

        // INST theta (A |- c) : capture-avoiding term substitution
        Step::Inst { subst, thm } => {
            let s = premise(results, *thm, cur)?;
            let concl = vsubst(subst, &s.concl)?;
            let mut hyps = Vec::with_capacity(s.hyps.len());
            for h in &s.hyps {
                hyps.push(vsubst(subst, h)?);
            }
            Ok(Sequent {
                hyps: dedup_conv(hyps),
                concl,
            })
        }

        // INST_TYPE theta (A |- c) : capture-avoiding type substitution
        Step::InstType { subst, thm } => {
            let s = premise(results, *thm, cur)?;
            let concl = inst_type(subst, &s.concl)?;
            let mut hyps = Vec::with_capacity(s.hyps.len());
            for h in &s.hyps {
                hyps.push(inst_type(subst, h)?);
            }
            Ok(Sequent {
                hyps: dedup_conv(hyps),
                concl,
            })
        }
    }
}

/// Independently CHECK a proof log by re-applying each primitive inference from
/// first principles, returning the final derived [`Sequent`] or a [`CheckError`]
/// if any step is invalid. This is the offline stand-in for a CakeML-verified
/// checker (see the module docs for the upgrade path).
pub fn check_proof(proof: &Proof) -> CheckResult<Sequent> {
    if proof.steps.is_empty() {
        return Err(CheckError::new("empty proof: nothing to derive"));
    }
    let mut results: Vec<Sequent> = Vec::with_capacity(proof.steps.len());
    for (i, step) in proof.steps.iter().enumerate() {
        let seq = apply_step(step, &results, i)?;
        results.push(seq);
    }
    // Non-empty (checked above), so the last result exists.
    Ok(results.pop().expect("non-empty proof has a final step"))
}

// ===========================================================================
// Tests.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- small builders --------------------------------------------------
    fn tya() -> HolType {
        HolType::var("A")
    }
    fn tyb() -> HolType {
        HolType::var("B")
    }
    fn v(name: &str, ty: HolType) -> Term {
        Term::var(name, ty)
    }

    /// REFL derives `|- t = t` with no hypotheses.
    #[test]
    fn refl_derives_reflexive_equation() {
        let x = v("x", tya());
        let proof = Proof::new(vec![Step::Refl { term: x.clone() }]);
        let seq = check_proof(&proof).unwrap();
        assert!(seq.hyps.is_empty());
        assert_eq!(seq.concl, mk_eq(&x, &x).unwrap());
    }

    /// BETA derives `|- (\x. f x) x = f x` (trivial redex).
    #[test]
    fn beta_derives_trivial_redex() {
        let x = v("x", tya());
        let f = v("f", HolType::fun(tya(), tyb()));
        let bod = Term::comb(f.clone(), x.clone());
        let redex = Term::comb(Term::abs(x.clone(), bod.clone()), x.clone());
        let proof = Proof::new(vec![Step::Beta {
            term: redex.clone(),
        }]);
        let seq = check_proof(&proof).unwrap();
        assert!(seq.hyps.is_empty());
        assert_eq!(seq.concl, mk_eq(&redex, &bod).unwrap());
    }

    /// A REFL -> REFL -> MK_COMB -> ABS chain derives `|- (\x. f x) = (\x. f x)`.
    #[test]
    fn mk_comb_and_abs_chain() {
        let x = v("x", tya());
        let f = v("f", HolType::fun(tya(), tyb()));
        let proof = Proof::new(vec![
            Step::Refl { term: f.clone() },     // 0: |- f = f
            Step::Refl { term: x.clone() },     // 1: |- x = x
            Step::MkComb { left: 0, right: 1 }, // 2: |- f x = f x
            Step::Abs {
                var: x.clone(),
                eq: 2,
            }, // 3: |- (\x. f x) = (\x. f x)
        ]);
        let seq = check_proof(&proof).unwrap();
        assert!(seq.hyps.is_empty());
        let fx = Term::comb(f, x.clone());
        let expected = mk_eq(&Term::abs(x.clone(), fx.clone()), &Term::abs(x, fx)).unwrap();
        assert_eq!(seq.concl, expected);
    }

    /// EQ_MP and DEDUCT_ANTISYM_RULE compose: from `{p}|-p` and `{q}|-q` build
    /// `{p,q}|-p=q`, then EQ_MP with `{p}|-p` yields `{p,q}|-q`.
    #[test]
    fn eq_mp_and_deduct_antisym() {
        let p = v("p", HolType::boolean());
        let q = v("q", HolType::boolean());
        let proof = Proof::new(vec![
            Step::Assume { term: p.clone() },          // 0: {p} |- p
            Step::Assume { term: q.clone() },          // 1: {q} |- q
            Step::DeductAntisym { left: 0, right: 1 }, // 2: {p,q} |- p = q
            Step::Assume { term: p.clone() },          // 3: {p} |- p
            Step::EqMp { eq: 2, thm: 3 },              // 4: {p,q} |- q
        ]);
        let seq = check_proof(&proof).unwrap();
        assert_eq!(seq.concl, q);
        assert!(seq.hyps.iter().any(|h| aconv(h, &p)));
        assert!(seq.hyps.iter().any(|h| aconv(h, &q)));
        assert_eq!(seq.hyps.len(), 2);
    }

    /// TAMPERED: a premise index that points forward (>= current step) is
    /// rejected.
    #[test]
    fn tampered_forward_premise_index_rejected() {
        let x = v("x", tya());
        // MK_COMB references step 1 and 2, but there is no step 2 (only 0..=1).
        let proof = Proof::new(vec![
            Step::Refl { term: x.clone() },
            Step::MkComb { left: 0, right: 2 },
        ]);
        let err = check_proof(&proof).unwrap_err();
        assert!(err.0.contains("premise index"), "unexpected error: {err}");
    }

    /// TAMPERED: an illegal BETA (argument is not the bound variable) is
    /// rejected.
    #[test]
    fn illegal_beta_rejected() {
        let x = v("x", tya());
        let y = v("y", tya());
        // (\x. x) y  is a redex, but the primitive BETA requires arg = bound var.
        let bad = Term::comb(Term::abs(x.clone(), x.clone()), y.clone());
        let proof = Proof::new(vec![Step::Beta { term: bad }]);
        assert!(check_proof(&proof).is_err());
    }

    /// ILLEGAL: ABS over a variable that is free in a hypothesis is rejected
    /// (the kernel side-condition).
    #[test]
    fn illegal_abs_over_free_hyp_rejected() {
        let x = v("x", HolType::boolean());
        // {x = x} |- x = x, then ABS x — but x is free in the hypothesis.
        let eq_xx = mk_eq(&x, &x).unwrap();
        let proof = Proof::new(vec![
            Step::Assume { term: eq_xx }, // 0: {x=x} |- x=x
            Step::Abs { var: x, eq: 0 },  // 1: illegal
        ]);
        let err = check_proof(&proof).unwrap_err();
        assert!(err.0.contains("ABS"), "unexpected error: {err}");
    }

    /// INST is genuinely capture-avoiding: instantiating `x := y` in
    /// `|- (\y. x) = (\y. x)` must NOT produce the captured `(\y. y)`; the binder
    /// is renamed to a fresh variant, giving `(\y'. y)`.
    #[test]
    fn inst_is_capture_avoiding() {
        let x = v("x", tya());
        let y = v("y", tya());
        let proof = Proof::new(vec![
            Step::Refl { term: x.clone() }, // 0: |- x = x
            Step::Abs {
                var: y.clone(),
                eq: 0,
            }, // 1: |- (\y. x) = (\y. x)
            Step::Inst {
                subst: vec![Instantiation {
                    var: x.clone(),
                    term: y.clone(),
                }],
                thm: 1,
            }, // 2: capture-avoiding => |- (\y'. y) = (\y'. y)
        ]);
        let seq = check_proof(&proof).unwrap();
        let (lhs, _rhs) = dest_eq(&seq.concl).unwrap();
        match lhs {
            Term::Abs(bound, body) => {
                // The binder was renamed away from `y`...
                assert_ne!(
                    bound.as_ref(),
                    &y,
                    "binder must be renamed to avoid capture, got the captured form"
                );
                // ...and the body is the free `y` (NOT the bound occurrence).
                assert_eq!(body.as_ref(), &y);
                // Sanity: the naive (capturing) result would have been (\y. y).
                let captured = Term::abs(y.clone(), y.clone());
                assert_ne!(lhs, &captured, "checker must not produce the captured term");
            }
            other => panic!("expected an abstraction, got {other:?}"),
        }
    }

    /// INST_TYPE is capture-avoiding: instantiating `A := B` in
    /// `|- (\x:A. x:B) = (\x:A. x:B)` must rename the binder so the (now type-B)
    /// bound `x` does not capture the free `x:B`.
    #[test]
    fn inst_type_is_capture_avoiding() {
        // \x:A. (x:B)  — binder and body share a name but differ in type.
        let bound = v("x", tya());
        let free = v("x", tyb());
        let lam = Term::abs(bound.clone(), free.clone());
        let proof = Proof::new(vec![
            Step::Refl { term: lam }, // 0: |- (\x:A. x:B) = (\x:A. x:B)
            Step::InstType {
                subst: vec![TypeInstantiation {
                    var: "A".to_string(),
                    ty: tyb(),
                }],
                thm: 0,
            }, // 1: A := B, must avoid capture
        ]);
        let seq = check_proof(&proof).unwrap();
        let (lhs, _rhs) = dest_eq(&seq.concl).unwrap();
        match lhs {
            Term::Abs(b, body) => {
                // Binder is now type B but RENAMED away from "x"...
                match b.as_ref() {
                    Term::Var(nm, ty) => {
                        assert_eq!(ty, &tyb());
                        assert_ne!(nm, "x", "binder must be renamed to avoid capture");
                    }
                    other => panic!("expected a variable binder, got {other:?}"),
                }
                // ...and the free occurrence x:B is preserved.
                assert_eq!(body.as_ref(), &free);
            }
            other => panic!("expected an abstraction, got {other:?}"),
        }
    }

    /// The proof log round-trips through JSON, and the deserialized proof checks
    /// to the same sequent.
    #[test]
    fn json_round_trip() {
        let x = v("x", tya());
        let f = v("f", HolType::fun(tya(), tyb()));
        let proof = Proof::new(vec![
            Step::Refl { term: f.clone() },
            Step::Refl { term: x.clone() },
            Step::MkComb { left: 0, right: 1 },
            Step::Abs { var: x, eq: 2 },
        ]);
        let json = serde_json::to_string(&proof).unwrap();
        let back: Proof = serde_json::from_str(&json).unwrap();
        assert_eq!(proof, back);
        assert_eq!(check_proof(&proof).unwrap(), check_proof(&back).unwrap());
        // The derived sequent itself round-trips too.
        let seq = check_proof(&proof).unwrap();
        let seq_json = serde_json::to_string(&seq).unwrap();
        let seq_back: Sequent = serde_json::from_str(&seq_json).unwrap();
        assert_eq!(seq, seq_back);
    }

    /// Checking is deterministic: the same proof yields byte-identical results.
    #[test]
    fn checking_is_deterministic() {
        let x = v("x", tya());
        let y = v("y", tya());
        let proof = Proof::new(vec![
            Step::Refl { term: x.clone() },
            Step::Abs {
                var: y.clone(),
                eq: 0,
            },
            Step::Inst {
                subst: vec![Instantiation { var: x, term: y }],
                thm: 1,
            },
        ]);
        let a = check_proof(&proof).unwrap();
        let b = check_proof(&proof).unwrap();
        assert_eq!(a, b);
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
    }
}
