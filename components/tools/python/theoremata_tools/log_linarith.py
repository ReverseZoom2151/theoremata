"""``LogLinarith`` -- discharge or refute asymptotic goals by log-reduction.

A relation between products/powers of positive quantities becomes *linear* once
you take logs: ``Theta(x**2 / y) REL Theta(1)`` is ``2*log x - log y  REL  0``.
This module:

1. converts each hypothesis and the *negated* goal into an order relation over
   the :mod:`order_of_magnitude` algebra (proof by contradiction);
2. extracts each order relation's monomials into an exponent vector (an
   :class:`~linprog_cert.Inequality`);
3. injects the two soundness gadgets from Tao's ``estimates``:
   * the **integrality gap** -- a positive integer ``N`` gives ``Theta(N) >= 1``;
   * **fixed/bounded** quantities -- ``Theta(k) = 1`` (fixed) / ``<= 1`` (bounded);
4. expands every ``max``/``min`` into a **disjunction** (one branch per "this arg
   is the max/min");
5. calls the exact LP kernel (:mod:`linprog_cert`) on the Cartesian product of
   all disjunction branches. The goal is **proved** iff *every* branch is
   infeasible (each yielding a Farkas certificate); if *some* branch is feasible
   the goal is **refuted** and that feasible point is returned as a concrete
   **asymptotic counterexample** ``Theta(var) = X**value`` for an unbounded
   order ``X`` -- our falsify-before-prove signal.

Public entry: :func:`evaluate` (JSON in / JSON out).
"""
from __future__ import annotations

from fractions import Fraction
from itertools import product
from typing import Any

from sympy import (
    Eq,
    GreaterThan,
    LessThan,
    Ne,
    S,
    StrictGreaterThan,
    StrictLessThan,
    Symbol,
    sympify,
)
from sympy.core.relational import Relational

from .linprog_cert import Inequality, feasibility
from .order_of_magnitude import (
    OrderMax,
    OrderMin,
    OrderMul,
    OrderOfMagnitude,
    OrderPow,
    Theta,
    Undefined,
    asymp,
    gg,
    gtrsim,
    lesssim,
    ll,
)

# --- order-relation representation: a plain (lhs_order, rhs_order, sense) triple.
# sense in {"leq","lt","geq","gt","eq"}; both sides are OrderOfMagnitude objects.

_NEG_SENSE = {  # logical negation of a *goal* order relation (strictness kept)
    "leq": "gt",
    "lt": "geq",
    "geq": "lt",
    "gt": "leq",
}


def extract_monomials(expr) -> dict[Any, Fraction]:
    """Order-of-magnitude expression -> ``{base: exponent}`` (its log vector)."""
    monomials: dict[Any, Fraction] = {}
    if expr == Theta(1):
        return monomials
    if isinstance(expr, OrderMul):
        for arg in expr.args:
            for term, coeff in extract_monomials(arg).items():
                monomials[term] = monomials.get(term, S(0)) + coeff
        return monomials
    if isinstance(expr, OrderPow):
        base, exp = expr.args
        if exp.is_rational:
            for term, coeff in extract_monomials(base).items():
                monomials[term] = coeff * exp
            return monomials
    monomials[expr] = S(1)
    return monomials


def _to_fraction(x) -> Fraction:
    r = S(x)
    return Fraction(int(r.p), int(r.q))


def inequality_of(lhs, rhs, sense: str) -> Inequality:
    """Order relation ``lhs SENSE rhs`` -> exponent-space :class:`Inequality`.

    Divides the two orders (``lhs/rhs``), reads off the monomial exponents and
    keys the LP variables by the string form of each order base.
    """
    coeffs_raw = extract_monomials((lhs / rhs))
    coeffs = {str(base): _to_fraction(exp) for base, exp in coeffs_raw.items()}
    return Inequality(coeffs, sense, 0)


# --- max / min object discovery (for disjunction expansion) ------------------

def _max_objects(expr) -> set:
    if isinstance(expr, OrderMax):
        objs = {expr}
        for a in expr.args:
            objs |= _max_objects(a)
        return objs
    if isinstance(expr, (OrderMin, OrderMul)):
        objs: set = set()
        for a in expr.args:
            objs |= _max_objects(a)
        return objs
    if isinstance(expr, OrderPow):
        return _max_objects(expr.args[0])
    return set()


def _min_objects(expr) -> set:
    if isinstance(expr, OrderMin):
        objs = {expr}
        for a in expr.args:
            objs |= _min_objects(a)
        return objs
    if isinstance(expr, (OrderMax, OrderMul)):
        objs: set = set()
        for a in expr.args:
            objs |= _min_objects(a)
        return objs
    if isinstance(expr, OrderPow):
        return _min_objects(expr.args[0])
    return set()


# --- turning parsed SymPy relations into order relations ---------------------

def _order_pair(rel: Relational):
    """Return (lhs_order, rhs_order, is_native_order) for a parsed relation."""
    a, b = rel.args[0], rel.args[1]
    if isinstance(a, OrderOfMagnitude) or isinstance(b, OrderOfMagnitude):
        la = a if isinstance(a, OrderOfMagnitude) else Theta(a)
        lb = b if isinstance(b, OrderOfMagnitude) else Theta(b)
        return la, lb, True
    return Theta(a), Theta(b), False


def _hypothesis_relations(rel: Relational) -> list[list[tuple]]:
    """A hypothesis relation -> a list of disjunction branches, each a list of
    order-relation triples.

    Strict inequalities between *raw* positive reals are weakened to non-strict
    (all we can conclude asymptotically); relations already stated between
    orders keep their strictness. ``!=`` between orders becomes a disjunction
    ``<`` OR ``>``; ``!=`` between raw reals carries no asymptotic content and is
    dropped.
    """
    la, lb, native = _order_pair(rel)
    if isinstance(la, Undefined) or isinstance(lb, Undefined):
        return []

    if isinstance(rel, Ne):
        if native:
            return [[(la, lb, "lt")], [(la, lb, "gt")]]
        return []  # raw a != b: no asymptotic information
    if isinstance(rel, Eq):
        return [[(la, lb, "eq")]]
    if isinstance(rel, (LessThan, StrictLessThan)):
        sense = "lt" if (native and isinstance(rel, StrictLessThan)) else "leq"
        return [[(la, lb, sense)]]
    if isinstance(rel, (GreaterThan, StrictGreaterThan)):
        sense = "gt" if (native and isinstance(rel, StrictGreaterThan)) else "geq"
        return [[(la, lb, sense)]]
    return []


def _goal_negation_branches(rel: Relational) -> list[list[tuple]]:
    """Negated goal -> disjunction branches of order-relation triples.

    The goal is read as an *asymptotic* statement (raw sides are wrapped in
    ``Theta`` keeping the stated sense as an order sense); its logical negation
    is taken in the order world with strictness preserved. Negating ``=`` gives
    the disjunction ``<`` OR ``>``.
    """
    la, lb, _ = _order_pair(rel)
    if isinstance(la, Undefined) or isinstance(lb, Undefined):
        raise ValueError("goal involves a non-positive / undefined order.")

    if isinstance(rel, Eq):
        return [[(la, lb, "lt")], [(la, lb, "gt")]]
    if isinstance(rel, Ne):
        # goal a != b: negation is a == b
        return [[(la, lb, "eq")]]
    if isinstance(rel, (LessThan, StrictLessThan)):
        base = "lt" if isinstance(rel, StrictLessThan) else "leq"
    elif isinstance(rel, (GreaterThan, StrictGreaterThan)):
        base = "gt" if isinstance(rel, StrictGreaterThan) else "geq"
    else:
        raise ValueError(f"unsupported goal relation: {rel}")
    return [[(la, lb, _NEG_SENSE[base])]]


# --- the core solver ---------------------------------------------------------

def log_linarith(
    hypotheses: list[Relational],
    goal: Relational,
    fixed: set[str] | None = None,
    bounded: set[str] | None = None,
    pos_int_vars: set[str] | None = None,
    split_max: bool = True,
) -> dict:
    """Core decision procedure over already-parsed SymPy relations.

    Returns a JSON-able dict ``{proved, refuted, counterexample?, certificates?,
    branches, details}``.
    """
    fixed = fixed or set()
    bounded = bounded or set()
    pos_int_vars = pos_int_vars or set()

    # inequality_lists: list of disjunctions; each disjunction is a list of
    # (Inequality, order-relation-triple) alternatives.
    disjunctions: list[list[tuple]] = []
    order_relations: list[tuple] = []  # flat, for max/min scanning

    def add_branchset(branches: list[list[tuple]]) -> None:
        # branches: list of alternatives; each alternative is a list of triples.
        # A single hypothesis normally yields one alternative with one triple.
        alt_ineqs: list[tuple] = []
        for alt in branches:
            # An alternative may contain several conjoined triples; but in our
            # construction each alternative holds exactly one triple.
            for (la, lb, sense) in alt:
                order_relations.append((la, lb, sense))
                alt_ineqs.append((inequality_of(la, lb, sense), (la, lb, sense)))
        if alt_ineqs:
            disjunctions.append(alt_ineqs)

    for hyp in hypotheses:
        if isinstance(hyp, Relational):
            add_branchset(_hypothesis_relations(hyp))

    add_branchset(_goal_negation_branches(goal))

    # Integrality gap: positive integer N gives Theta(N) >= Theta(1).
    for name in sorted(pos_int_vars):
        sym = Symbol(name, positive=True, integer=True)
        add_branchset([[(Theta(sym), Theta(1), "geq")]])

    # Fixed / bounded quantities.
    for name in sorted(fixed):
        sym = Symbol(name, positive=True)
        add_branchset([[(Theta(sym), Theta(1), "eq")]])
    for name in sorted(bounded):
        if name in fixed:
            continue
        sym = Symbol(name, positive=True)
        add_branchset([[(Theta(sym), Theta(1), "leq")]])

    # max / min disjunction expansion.
    if split_max:
        max_objs: set = set()
        min_objs: set = set()
        for (la, lb, _s) in order_relations:
            max_objs |= _max_objects(la) | _max_objects(lb)
            min_objs |= _min_objects(la) | _min_objects(lb)
        for M in max_objs:
            eq_alts: list[tuple] = []
            for arg in M.args:
                disjunctions.append([(inequality_of(arg, M, "leq"), (arg, M, "leq"))])
                eq_alts.append((inequality_of(arg, M, "eq"), (arg, M, "eq")))
            if eq_alts:
                disjunctions.append(eq_alts)
        for M in min_objs:
            eq_alts = []
            for arg in M.args:
                disjunctions.append([(inequality_of(arg, M, "geq"), (arg, M, "geq"))])
                eq_alts.append((inequality_of(arg, M, "eq"), (arg, M, "eq")))
            if eq_alts:
                disjunctions.append(eq_alts)

    # Iterate the Cartesian product of disjunction branches.
    branch_reports: list[dict] = []
    for combo in product(*disjunctions):
        ineqs = [ineq for (ineq, _tri) in combo]
        result = feasibility(ineqs)
        if result["feasible"]:
            # A concrete asymptotic counterexample.
            model = result.get("model", {})
            counter = [
                {"order": var, "exponent": val, "reads": f"{var} = X**{val}"}
                for var, val in model.items()
            ]
            return {
                "proved": False,
                "refuted": True,
                "counterexample": {
                    "note": "feasible for an unbounded order of magnitude X",
                    "assignment": counter,
                },
                "branches": len(list(product(*disjunctions))) if disjunctions else 1,
                "details": {
                    "backend": result["backend"],
                    "branch": [str(ineq) for ineq in ineqs],
                },
            }
        branch_reports.append({
            "inequalities": [str(ineq) for ineq in ineqs],
            "certificate": result["certificate"],
        })

    return {
        "proved": True,
        "refuted": False,
        "certificates": branch_reports,
        "branches": len(branch_reports),
        "details": {"method": "every branch infeasible (proof by contradiction)"},
    }


# --- parsing / public entry --------------------------------------------------

_ASSUMPTIONS = {
    "pos_real": dict(positive=True, real=True),
    "nonneg_real": dict(nonnegative=True, real=True),
    "real": dict(real=True),
    "pos_int": dict(positive=True, integer=True),
    "nonneg_int": dict(nonnegative=True, integer=True),
    "int": dict(integer=True),
    "pos_rat": dict(positive=True, rational=True),
}


def _build_namespace(varspec) -> tuple[dict, set[str]]:
    """Build a sympify namespace from a variable spec, returning
    (namespace, positive-integer variable names)."""
    ns: dict[str, Any] = {
        "Theta": Theta,
        "lesssim": lesssim,
        "ll": ll,
        "gg": gg,
        "gtrsim": gtrsim,
        "asymp": asymp,
    }
    from sympy import Abs, Eq as _Eq, Max, Min, Ne as _Ne, Rational, sqrt

    ns.update({"Abs": Abs, "Max": Max, "Min": Min, "Eq": _Eq, "Ne": _Ne,
               "Rational": Rational, "sqrt": sqrt})

    pos_ints: set[str] = set()
    items = varspec.items() if isinstance(varspec, dict) else [
        (v["name"], v.get("type", "pos_real")) for v in (varspec or [])
    ]
    for name, typ in items:
        assumptions = _ASSUMPTIONS.get(typ, _ASSUMPTIONS["pos_real"])
        ns[name] = Symbol(name, **assumptions)
        if assumptions.get("positive") and assumptions.get("integer"):
            pos_ints.add(name)
    return ns, pos_ints


def _parse(expr: str, ns: dict) -> Relational:
    parsed = sympify(expr, locals=ns)
    if not isinstance(parsed, Relational):
        raise ValueError(f"expression is not a relation: {expr!r}")
    return parsed


def evaluate(request: dict) -> dict:
    """JSON-able entry point.

    ``op`` selects the capability (default ``"prove"``):

    * ``"prove"`` / ``"loglinarith"`` -- prove/refute an asymptotic ``goal`` from
      ``hypotheses``. Request keys: ``vars`` (``{name: type}`` or ``[{name,type}]``),
      ``hypotheses`` (list of relation strings), ``goal`` (relation string),
      ``fixed``/``bounded`` (lists of variable names), ``split_max`` (bool).
    * ``"feasibility"`` -- exact LP over explicit ``constraints`` (delegates to
      :mod:`linprog_cert`).
    * ``"normalize"`` -- normalize an order-of-magnitude ``expression`` string.

    Returns ``{status, verdict, certificate|counterexample|..., details}``.
    """
    op = request.get("op", "prove")

    try:
        if op in ("prove", "loglinarith", "refute"):
            ns, pos_ints = _build_namespace(request.get("vars", request.get("variables", {})))
            hyp_strs = request.get("hypotheses", [])
            hypotheses = [_parse(h, ns) for h in hyp_strs]
            goal = _parse(request["goal"], ns)
            fixed = set(request.get("fixed", []))
            bounded = set(request.get("bounded", []))
            split_max = bool(request.get("split_max", True))

            res = log_linarith(
                hypotheses, goal, fixed=fixed, bounded=bounded,
                pos_int_vars=pos_ints, split_max=split_max,
            )
            if res["proved"]:
                return {
                    "status": "ok",
                    "verdict": "proved",
                    "certificate": res["certificates"],
                    "details": {"branches": res["branches"], **res.get("details", {})},
                }
            return {
                "status": "ok",
                "verdict": "refuted",
                "counterexample": res["counterexample"],
                "details": {"branches": res.get("branches"), **res.get("details", {})},
            }

        if op in ("feasibility", "linprog"):
            from .linprog_cert import evaluate as _lp_eval
            return _lp_eval(request)

        if op in ("normalize", "order"):
            ns, _ = _build_namespace(request.get("vars", request.get("variables", {})))
            expr = sympify(request["expression"], locals=ns)
            order = expr if isinstance(expr, OrderOfMagnitude) else Theta(expr)
            return {
                "status": "ok",
                "verdict": "normalized",
                "details": {
                    "input": request["expression"],
                    "order": str(order),
                    "monomials": {str(b): str(e) for b, e in extract_monomials(order).items()},
                    "undefined": isinstance(order, Undefined),
                },
            }

        return {"status": "error", "verdict": "error",
                "details": {"error": f"unknown op: {op!r}"}}
    except Exception as exc:  # pragma: no cover - defensive top-level guard
        return {"status": "error", "verdict": "error", "details": {"error": str(exc)}}
