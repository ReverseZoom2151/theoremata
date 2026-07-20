"""Verified-certificate **proof-log** exporter + self-contained **reference checker**.

Theoremata already emits machine-checkable certificates from several producers
(LP/Farkas duals, log-linear/asymptotic Farkas combinations, Wu's-method
pseudo-remainders).  The catch: *only we* validate them.  This module closes
that gap in the spirit of CakeML's "Verified Checkers" programme, which builds
machine-code-correct proof-log checkers and explicitly seeks new proof-log
*formats*.  We give our certificates

1. a **self-describing, transport-neutral proof-log format**
   (``theoremata.cert-log.v1``) — a linear list of typed, independently
   re-checkable *steps* carrying exactly the numeric data a checker needs, and
2. a **self-contained REFERENCE CHECKER** (:func:`check_cert_log`) that
   RE-VERIFIES every step with exact rational arithmetic
   (:class:`fractions.Fraction`) and no floating point, importing **nothing**
   from the producers.

The reference checker is the *offline stand-in* for a CakeML-verified checker.
Because the log is plain self-describing JSON with a fixed, tiny per-step
semantics, the same document is the intended input for a **CakeML-verified
checker binary** once that toolchain is present (see ``CAKEML_TARGET`` below):
the format is deliberately first-order, branch-free-checkable, and closed over
the rationals so it maps onto a verified checker's specification.

Soundness boundary
------------------
The checker is the sound boundary: a *tampered or invalid* certificate MUST be
rejected (``valid=False`` with a ``reason``).  It never trusts a producer's
own "verified" flag — it recomputes ``y^T A``, the Farkas combination, the
pseudo-remainder, etc., from the raw numbers in the log and compares exactly.

Everything here is pure standard library and deterministic.  All inputs
(certificates, logs, ``resources/`` data) are treated as **UNTRUSTED DATA**:
the checker validates structure/types defensively and converts any malformed
input into ``valid=False`` rather than trusting or executing it.

Worker dispatch key: ``cert_log`` (see :func:`run`).
"""
from __future__ import annotations

import json
from fractions import Fraction
from typing import Any, Iterable, Optional

FORMAT = "theoremata.cert-log.v1"

# The upgrade path: this reference checker is the offline stand-in.  A
# CakeML-verified checker binary, generated from a HOL4 specification of the
# per-step semantics below, is the toolchain-gated replacement that validates
# the *same* log documents with a machine-checked soundness guarantee.
CAKEML_TARGET = (
    "cake_certlog_check: a CakeML/HOL4-verified checker whose specification is "
    "the per-step semantics in this module; consumes theoremata.cert-log.v1 "
    "documents unchanged. This Python reference checker is the offline "
    "stand-in until that verified binary/toolchain is available."
)

# The kinds THIS module exports and re-checks.  It is NOT the list of kinds that
# exist in the ``theoremata.cert-log.v1`` namespace: the format is deliberately
# shared by a family of sibling checker modules (``cert_sturm``, ``cert_sos``,
# ...), each of which owns its own kinds and its own independent checker.  Keep
# this tuple in lockstep with ``_KIND_OPS`` below; a test enforces that.
#
# ``fp_rounding`` and ``fp_error_bound`` are floating-point certificate kinds
# owned and re-checked HERE (not by a sibling module), added because the ATP
# mining corpus flagged them as the two highest-confidence new numeric kinds.
# Both are validated with exact rational arithmetic; see their handlers for the
# precise soundness boundary (what is checked exactly vs what is withheld).
KINDS = ("lp_primal_dual", "lp_farkas", "asymptotic", "wu_geometry",
         "subsumption", "fp_rounding", "fp_error_bound")

# Kinds in the SAME proof-log format that are owned (and validated) by a sibling
# module.  Purpose: report such a document as *not checked here, checked over
# there* instead of lumping it in with tampered garbage.  This grants NO
# validation power: this module still refuses to check these documents, and a
# caller must route them to the owning module's checker to learn anything about
# their validity.  A drift-guard test fails if a shipped ``cert_*.py`` exports a
# kind that is absent or misattributed here.
FOREIGN_KIND_OWNERS = {
    "bernstein": "theoremata_tools.cert_bernstein",
    "bezout": "theoremata_tools.cert_bezout",
    "bnb_inequality": "theoremata_tools.cert_bnb",
    "continued_fraction": "theoremata_tools.cert_continued_fraction",
    "flyspeck_lp": "theoremata_tools.cert_flyspeck_lp",
    "herbrand": "theoremata_tools.cert_herbrand",
    "nullstellensatz": "theoremata_tools.cert_nullstellensatz",
    "pocklington_primality": "theoremata_tools.cert_pocklington",
    "poly_minimax": "theoremata_tools.cert_sturm",
    "positivstellensatz": "theoremata_tools.cert_positivstellensatz",
    "pratt_primality": "theoremata_tools.cert_pratt",
    "sos": "theoremata_tools.cert_sos",
    "sturm": "theoremata_tools.cert_sturm",
    "taylor_model": "theoremata_tools.cert_taylor_model",
    "wz": "theoremata_tools.cert_wz",
}

# Outcome statuses of :func:`check_cert_log`.  Only ``STATUS_VERIFIED`` ever
# accompanies ``valid=True``; the other two are distinct flavours of "this
# module is NOT telling you the document is good".
STATUS_VERIFIED = "verified"             # steps re-verified here
STATUS_REJECTED = "rejected"             # malformed/tampered/unsatisfied: refuted
STATUS_UNSUPPORTED = "unsupported_kind"  # a sibling module's kind: UNKNOWN here
# WITHHELD is the honest "I cannot decide this" verdict for a kind we DO own but
# whose specific instance falls outside what this checker can verify exactly (an
# unmodeled rounding mode, or an exact expression using an irrational operation).
# It is fail-closed: withheld carries valid=False, so it is never a pass, but it
# is deliberately distinct from a refutation. A checker that can only partially
# verify must land here rather than fabricate a pass.
STATUS_WITHHELD = "withheld"             # well-formed but undecidable exactly here


# --------------------------------------------------------------------------- #
# Exact-rational helpers (self-contained; no producer imports).
# --------------------------------------------------------------------------- #

def _frac(x: Any) -> Fraction:
    """Parse a number exactly (via ``str`` to avoid float drift)."""
    if isinstance(x, Fraction):
        return x
    if isinstance(x, bool):  # guard: bools are ints in Python
        raise TypeError("boolean where a rational was expected")
    if isinstance(x, int):
        return Fraction(x)
    return Fraction(str(x))


def _fs(x: Fraction) -> str:
    """Serialize a Fraction as ``"p"`` or ``"p/q"``."""
    return str(x)


def _ineq_fields(ineq: Any) -> tuple[dict[str, Fraction], str, Fraction]:
    """Read ``(coeffs, sense, rhs)`` from an Inequality-like object or dict."""
    if isinstance(ineq, dict):
        coeffs = ineq.get("coeffs", {})
        sense = ineq["sense"]
        rhs = ineq.get("rhs", 0)
    else:  # duck-typed linprog_cert.Inequality
        coeffs = getattr(ineq, "coeffs")
        sense = getattr(ineq, "sense")
        rhs = getattr(ineq, "rhs")
    _ALIAS = {"leq": "leq", "<=": "leq", "le": "leq", "lt": "lt", "<": "lt",
              "geq": "geq", ">=": "geq", "ge": "geq", "gt": "gt", ">": "gt",
              "eq": "eq", "=": "eq", "==": "eq"}
    s = _ALIAS.get(str(sense))
    if s is None:
        raise ValueError(f"bad sense {sense!r}")
    return ({str(k): _frac(v) for k, v in dict(coeffs).items()}, s, _frac(rhs))


def _sorted_vars(constraints: list, objective: Optional[dict]) -> list[str]:
    names: set[str] = set()
    for ineq in constraints:
        coeffs, _s, _r = _ineq_fields(ineq)
        names.update(coeffs)
    if objective:
        names.update(str(k) for k in objective)
    return sorted(names)


# --------------------------------------------------------------------------- #
# Exporter: LP dual optimality certificate (lp_geometry.primal_dual).
# --------------------------------------------------------------------------- #

def _normalize_leq(constraints: list, variables: list[str]
                   ) -> tuple[list[list[Fraction]], list[Fraction], list[dict]]:
    """Constraints -> ``G x <= h`` in the SAME order lp_geometry.primal_dual uses.

    ``leq/lt`` kept; ``geq/gt`` negated; ``eq`` split into ``+``/``-`` rows.  The
    resulting row order indexes the producer's dual vector ``y``.
    """
    G: list[list[Fraction]] = []
    h: list[Fraction] = []
    src: list[dict] = []
    for idx, ineq in enumerate(constraints):
        coeffs, sense, rhs = _ineq_fields(ineq)
        row = [coeffs.get(v, Fraction(0)) for v in variables]
        if sense in ("leq", "lt"):
            G.append(row); h.append(rhs); src.append({"index": idx, "orientation": "leq"})
        elif sense in ("geq", "gt"):
            G.append([-v for v in row]); h.append(-rhs)
            src.append({"index": idx, "orientation": "geq(neg)"})
        else:
            G.append(list(row)); h.append(rhs); src.append({"index": idx, "orientation": "eq(+)"})
            G.append([-v for v in row]); h.append(-rhs); src.append({"index": idx, "orientation": "eq(-)"})
    return G, h, src


def export_lp_cert(cert: dict, *, constraints: Iterable,
                   objective: Optional[dict] = None, sense: str = "max",
                   claim: Optional[str] = None) -> dict:
    """Serialize an LP certificate to a cert-log document.

    Accepts either producer cert shape and dispatches on it:

    * ``lp_geometry.primal_dual`` result (``certificate.type ==
      "lp_primal_dual"``) -> a **dual optimality / bound** log carrying the
      normalized ``G x <= h``, the maximised objective ``c``, the dual ``y`` and
      the primal ``x``, with steps a checker re-verifies:
      ``y >= 0``, ``G^T y >= c``, primal feasibility, strong duality
      ``c.x == h.y`` and complementary slackness.
    * ``linprog_cert.feasibility`` INFEASIBLE result (``certificate.type ==
      "farkas"``) -> delegates to :func:`export_lp_farkas_cert`.

    ``constraints`` are the original constraints handed to the producer (dicts
    or Inequality objects); ``objective``/``sense`` mirror the primal_dual call.
    """
    constraints = list(constraints)
    ctype = (cert.get("certificate") or {}).get("type") if isinstance(cert, dict) else None
    if ctype == "farkas" or cert.get("feasible") is False:
        return export_lp_farkas_cert(cert, constraints=constraints, claim=claim)
    if ctype != "lp_primal_dual":
        raise ValueError("unrecognized LP certificate shape for export_lp_cert")

    obj_map = {str(k): _frac(v) for k, v in (objective or {}).items()}
    variables = _sorted_vars(constraints, obj_map or None)
    G, h, src = _normalize_leq(constraints, variables)
    c = [obj_map.get(v, Fraction(0)) for v in variables]
    c_solve = c if sense == "max" else [-v for v in c]  # objective actually bounded

    y = [_frac(v) for v in cert["dual"]]
    x = [_frac(cert["primal"][v]) for v in variables]
    dual_obj = sum(h[k] * y[k] for k in range(len(G)))

    steps = [
        {"op": "lp_problem", "sense": sense, "variables": variables,
         "G": [[_fs(v) for v in row] for row in G], "h": [_fs(v) for v in h],
         "c": [_fs(v) for v in c_solve], "row_src": src,
         "note": "bounds max c.x over {x : G x <= h, x >= 0}"},
        {"op": "dual_vector", "y": [_fs(v) for v in y]},
        {"op": "primal_vector", "x": [_fs(v) for v in x]},
        {"op": "assert_dual_nonneg"},
        {"op": "assert_dual_feasible"},
        {"op": "assert_primal_feasible"},
        {"op": "assert_complementary_slackness"},
        {"op": "assert_strong_duality"},
        {"op": "assert_bound", "bound": _fs(dual_obj)},
    ]
    return {
        "format": FORMAT,
        "kind": "lp_primal_dual",
        "claim": claim or f"{sense} c.x subject to G x <= h, x >= 0 is bounded by h.y",
        "steps": steps,
        "meta": {
            "producer": "lp_geometry.primal_dual",
            "claimed_objective": str(cert.get("objective_value")),
            "cakeml_target": CAKEML_TARGET,
        },
    }


# --------------------------------------------------------------------------- #
# Exporter: LP Farkas infeasibility certificate (linprog_cert.feasibility).
# --------------------------------------------------------------------------- #

def _oriented_row(coeffs: dict[str, Fraction], sense: str, rhs: Fraction,
                  orientation: int) -> tuple[dict[str, Fraction], bool, Fraction]:
    """Reconstruct one normalized ``<=``/``<`` row for the given orientation.

    ``orientation == +1`` keeps the row (``leq``/``lt`` sense); ``-1`` negates it
    (the form used for a ``geq``/``gt`` or the ``-`` half of an ``eq``).  A row is
    strict iff the ORIGINAL sense was strict (``lt``/``gt``).
    """
    strict = sense in ("lt", "gt")
    if orientation == 1:
        return dict(coeffs), strict, rhs
    return {k: -v for k, v in coeffs.items()}, strict, -rhs


def export_lp_farkas_cert(cert: dict, *, constraints: Iterable,
                          claim: Optional[str] = None) -> dict:
    """Serialize a ``linprog_cert.feasibility`` INFEASIBLE (Farkas) cert.

    Rebuilds, from the certificate's ``multipliers`` (each an ``index`` into the
    original constraints plus an ``orientation``) and the original
    ``constraints``, the exact normalized ``<=``/``<`` rows the producer combined,
    and pairs them with the nonnegative multipliers.  A checker re-verifies:
    ``m >= 0``, ``sum m_k a_k == 0`` and ``sum m_k b_k < 0`` (contradiction).
    """
    constraints = list(constraints)
    farkas = cert.get("certificate", cert)
    if farkas.get("type") != "farkas":
        raise ValueError("not a Farkas certificate")
    variables = cert.get("variables") or _sorted_vars(constraints, None)

    rows: list[dict] = []
    m: list[Fraction] = []
    for entry in farkas["multipliers"]:
        idx = int(entry["index"])
        orientation = int(entry["orientation"])
        coeffs, sense, rhs = _ineq_fields(constraints[idx])
        a, strict, b = _oriented_row(coeffs, sense, rhs, orientation)
        rows.append({"a": {k: _fs(v) for k, v in a.items()},
                     "strict": bool(strict), "b": _fs(b)})
        m.append(_frac(entry["multiplier"]))

    steps = [
        {"op": "farkas_system", "variables": list(variables), "rows": rows,
         "note": "each row a.x <= b (or < b) holds for any solution of the system"},
        {"op": "farkas_multipliers", "m": [_fs(v) for v in m]},
        {"op": "assert_multipliers_nonneg"},
        {"op": "assert_combination_zero"},
        {"op": "assert_contradiction"},
    ]
    return {
        "format": FORMAT,
        "kind": "lp_farkas",
        "claim": claim or "the linear system is infeasible (Farkas certificate)",
        "steps": steps,
        "meta": {"producer": "linprog_cert.feasibility",
                 "cakeml_target": CAKEML_TARGET},
    }


# --------------------------------------------------------------------------- #
# Exporter: asymptotic / log-linear certificate (log_linarith).
# --------------------------------------------------------------------------- #

_OPS = ((" <= ", "leq"), (" >= ", "geq"), (" < ", "lt"), (" > ", "gt"), (" = ", "eq"))


def _parse_inequality_str(s: str) -> tuple[dict[str, Fraction], str, Fraction]:
    """Parse a ``linprog_cert.Inequality`` string back to numeric fields.

    Format (deterministic): ``"c1*v1 + c2*v2 OP rhs"`` where each ``ci`` is a
    Fraction spelling (``1``, ``-1``, ``1/2``) and ``vi`` a base name that may
    contain parentheses (e.g. ``Theta(x)``); an all-zero lhs renders as ``"0"``.
    Used only inside the exporter; the emitted log carries clean numeric JSON.
    """
    s = str(s)
    sense = None
    lhs = rhs = ""
    for token, name in _OPS:
        if token in s:
            lhs, rhs = s.split(token, 1)
            sense = name
            break
    if sense is None:
        raise ValueError(f"cannot parse inequality: {s!r}")
    coeffs: dict[str, Fraction] = {}
    lhs = lhs.strip()
    if lhs and lhs != "0":
        for term in lhs.split(" + "):
            term = term.strip()
            if "*" not in term:
                if term in ("0", ""):
                    continue
                raise ValueError(f"cannot parse term: {term!r}")
            cstr, var = term.split("*", 1)
            coeffs[var.strip()] = _frac(cstr.strip())
    return coeffs, sense, _frac(rhs.strip())


def _farkas_steps_from(branch_ineqs: list, farkas: dict) -> tuple[list[dict], list[Fraction]]:
    """Build ``(rows, m)`` for one infeasible branch from its ineqs + Farkas cert."""
    rows: list[dict] = []
    m: list[Fraction] = []
    for entry in farkas["multipliers"]:
        idx = int(entry["index"])
        orientation = int(entry["orientation"])
        coeffs, sense, rhs = _ineq_fields(branch_ineqs[idx])
        a, strict, b = _oriented_row(coeffs, sense, rhs, orientation)
        rows.append({"a": {k: _fs(v) for k, v in a.items()},
                     "strict": bool(strict), "b": _fs(b)})
        m.append(_frac(entry["multiplier"]))
    return rows, m


def export_asymptotic_cert(cert: dict, *, claim: Optional[str] = None) -> dict:
    """Serialize a ``log_linarith`` PROVED asymptotic certificate.

    The asymptotic goal is proved by reducing (via logs) to a linear system over
    order-exponent variables and showing **every disjunction branch is
    infeasible**, each branch carrying a Farkas combination.  This exporter emits
    one ``branch_farkas`` step per branch; the checker re-verifies each branch's
    Farkas contradiction independently.  The goal holds iff *all* branches
    contradict.

    ``cert`` is the ``log_linarith.log_linarith`` result (``proved=True`` with a
    ``certificates`` list of ``{inequalities: [str], certificate: farkas}``), or
    the ``evaluate`` ``"proved"`` payload whose ``certificate`` is that list.
    """
    branches = cert.get("certificates")
    if branches is None and cert.get("verdict") == "proved":
        branches = cert.get("certificate")
    if not branches:
        raise ValueError("no proved-branch certificates to export")

    variables: set[str] = set()
    steps: list[dict] = [None]  # placeholder for the goal step
    for bi, branch in enumerate(branches):
        ineq_strs = branch["inequalities"]
        parsed = [
            (lambda cs: {"coeffs": cs[0], "sense": cs[1], "rhs": cs[2]})(
                _parse_inequality_str(s))
            for s in ineq_strs
        ]
        for p in parsed:
            variables.update(p["coeffs"])
        rows, m = _farkas_steps_from(parsed, branch["certificate"])
        steps.append({"op": "branch_farkas", "index": bi, "rows": rows,
                      "m": [_fs(v) for v in m]})
    steps[0] = {"op": "asymptotic_goal", "variables": sorted(variables),
                "branches": len(branches),
                "note": "goal proved iff every branch is Farkas-infeasible"}
    return {
        "format": FORMAT,
        "kind": "asymptotic",
        "claim": claim or "asymptotic goal proved: every log-linear branch is infeasible",
        "steps": steps,
        "meta": {"producer": "log_linarith.log_linarith",
                 "cakeml_target": CAKEML_TARGET},
    }


# --------------------------------------------------------------------------- #
# Exporter: Wu's-method geometry certificate (geometry_algebraic).
# --------------------------------------------------------------------------- #

def _serialize_poly(p: Any) -> dict:
    """Serialize a geometry_algebraic.Poly (``{monomial-tuple: Fraction}``)."""
    return {"n": int(p.n),
            "terms": [[list(m), _fs(Fraction(c))] for m, c in p.terms.items()]}


def export_geometry_cert(cert: dict, *, points: dict, hypotheses: list,
                         goal: dict, var_order: Optional[list] = None,
                         claim: Optional[str] = None) -> dict:
    """Serialize a Wu's-method PROVED geometry certificate.

    The producer's ``prove`` result carries the characteristic set, goal
    polynomials and pseudo-remainders only as *strings*, which cannot be
    independently re-checked.  So this exporter re-imports the producer's
    low-level polynomial builders to obtain the exact ``Poly`` objects (the
    characteristic chain and the goal polynomials) for the SAME configuration and
    serializes their monomial dictionaries.  The checker then RECOMPUTES the
    successive pseudo-remainder with its own arithmetic and asserts it is exactly
    the zero polynomial.

    ``points``/``hypotheses``/``goal``/``var_order`` are the same inputs handed to
    ``geometry_algebraic.prove``.
    """
    if not cert.get("proved"):
        raise ValueError("only a proved (remainder==0) geometry cert is exportable")
    from theoremata_tools.geometry_algebraic import (  # exporter-only import
        _Coords, _goal_polys, _hypothesis_polys, triangulate,
    )
    co = _Coords(points, var_order)
    chain = triangulate(_hypothesis_polys(list(hypotheses), co), co.n)
    goal_polys = _goal_polys(goal, co)

    nondeg = []
    for p in sorted(chain, key=lambda q: q.class_index()):
        v = p.class_index()
        if v < 0:
            continue
        init = p.leading_coeff_in(v)
        if not init.is_zero() and not init.is_const():
            nondeg.append(_serialize_poly(init))

    steps = [
        {"op": "declare_ring", "nvars": int(co.n), "names": list(co.var_names)},
        {"op": "characteristic_set", "polys": [_serialize_poly(p) for p in chain]},
        {"op": "goal_polynomials", "polys": [_serialize_poly(p) for p in goal_polys]},
        {"op": "non_degeneracy", "conditions": nondeg},
        {"op": "assert_pseudo_remainders_zero"},
    ]
    return {
        "format": FORMAT,
        "kind": "wu_geometry",
        "claim": claim or ("goal is an algebraic consequence of the hypotheses "
                           "modulo the stated non-degeneracy conditions"),
        "steps": steps,
        "meta": {"producer": "geometry_algebraic.prove",
                 "cakeml_target": CAKEML_TARGET},
    }


# --------------------------------------------------------------------------- #
# Exporter: subsumption certificate (optional).
# --------------------------------------------------------------------------- #

def export_subsumption_cert(subsumer: list, subsumed: list,
                            substitution: dict, *,
                            claim: Optional[str] = None) -> dict:
    """Serialize a theta-subsumption relation ``subsumer.theta ⊆ subsumed``.

    ``subsumer``/``subsumed`` are lists of literal strings; ``substitution`` maps
    variable tokens to terms.  The checker re-applies ``theta`` and verifies every
    substituted subsumer literal occurs in ``subsumed`` (sound clause subsumption:
    if it holds, ``subsumer`` subsumes / is at least as general as ``subsumed``).
    """
    return {
        "format": FORMAT,
        "kind": "subsumption",
        "claim": claim or "subsumer theta-subsumes subsumed",
        "steps": [
            {"op": "subsumption_relation",
             "subsumer": [str(x) for x in subsumer],
             "subsumed": [str(x) for x in subsumed],
             "substitution": {str(k): str(v) for k, v in substitution.items()}},
            {"op": "assert_theta_subsumes"},
        ],
        "meta": {"producer": "subsumption", "cakeml_target": CAKEML_TARGET},
    }


# --------------------------------------------------------------------------- #
# Floating-point certificate helpers (fp_rounding, fp_error_bound).
#
# Soundness boundary for these two kinds:
#  * All arithmetic is exact rational (fractions.Fraction). We never touch a
#    hardware/Python float during checking, so nothing here inherits float drift.
#  * ``fp_error_bound`` is checked in FULL: it recomputes |computed - exact| as
#    an exact rational and compares it to the stated bound. No part is asserted.
#  * ``fp_rounding`` is checked EXACTLY in an unbounded-exponent model: it
#    recomputes the correctly-rounded value of the exact expression at the stated
#    significand precision and rounding mode and compares. What is deliberately
#    NOT modeled: IEEE-754 subnormal (gradual underflow), overflow to infinity,
#    and NaN. Those are exponent-range concerns; the rounding of a finite value
#    to a p-bit significand is exact and complete in this model. An instance that
#    needs a mode we do not implement, or an exact expression that uses an
#    irrational operation (sqrt, exp, ...), is WITHHELD, never passed.
# --------------------------------------------------------------------------- #

# Rounding modes implemented exactly here. Any other declared mode is WITHHELD
# (undecided), because passing a rounding rule we do not model would be a
# fabricated verdict.
_FP_MODES = frozenset(
    ("nearest_even", "toward_zero", "toward_pos", "toward_neg", "away_zero")
)

# Exact expression node ops we can evaluate over the rationals.
_FP_EXACT_BINOPS = frozenset(("add", "sub", "mul", "div"))

# Real operations that yield a legitimate rounding claim whose exact value is
# generally irrational. We cannot form them as an exact rational, so a
# certificate whose exact expression uses one is WITHHELD: a fuller checker
# could decide it; this reference checker soundly declines instead of guessing.
_FP_INEXACT_OPS = frozenset(
    ("sqrt", "cbrt", "root", "exp", "log", "ln", "log2", "log10",
     "sin", "cos", "tan", "atan", "asin", "acos", "pi", "e", "pow")
)


def _exact_rat(x: Any) -> Fraction:
    """Parse an fp numeric literal, refusing raw floats.

    A Python ``float`` literal would be silently decimal-rounded by
    ``_frac(str(float))`` and so lose its exact binary identity; fp certificates
    must carry EXACT numbers (integers or string rationals like ``"p/q"``), so a
    float here is rejected rather than approximated.
    """
    if isinstance(x, float):
        raise _Reject("fp certificate carries a float literal; pass an exact "
                      "integer or a string rational instead")
    return _frac(x)


def _eval_fp_exact(node: Any, depth: int = 0) -> Fraction:
    """Evaluate an fp exact expression to an exact rational.

    A node is either a numeric literal or ``{"op": ..., "args": [...]}``. Only
    the four rational field operations are evaluated; a recognized-but-irrational
    operation withholds (see ``_FP_INEXACT_OPS``), and anything else is malformed.
    """
    if depth > 64:
        raise _Reject("fp exact expression nested too deeply")
    if isinstance(node, dict):
        op = node.get("op")
        if op in _FP_INEXACT_OPS:
            # WHY withhold: the value is real but generally irrational; forming it
            # as an exact rational is impossible here, so we decline (not pass).
            raise _Withhold(f"fp exact op {op!r} is not exactly evaluable here")
        args = node.get("args")
        if not isinstance(args, list) or not args:
            raise _Reject("fp exact expression: op needs a non-empty args list")
        if op == "neg":
            return -_eval_fp_exact(args[0], depth + 1)
        if op in _FP_EXACT_BINOPS:
            if len(args) < 2:
                raise _Reject(f"fp exact op {op!r} needs at least two args")
            acc = _eval_fp_exact(args[0], depth + 1)
            for a in args[1:]:
                b = _eval_fp_exact(a, depth + 1)
                if op == "add":
                    acc = acc + b
                elif op == "sub":
                    acc = acc - b
                elif op == "mul":
                    acc = acc * b
                else:  # div
                    if b == 0:
                        raise _Reject("fp exact division by zero")
                    acc = acc / b
            return acc
        raise _Reject(f"fp exact expression: unknown op {op!r}")
    return _exact_rat(node)


def _trailing_zeros(n: int) -> int:
    n = abs(int(n))
    if n == 0:
        return 0
    tz = 0
    while (n & 1) == 0:
        n >>= 1
        tz += 1
    return tz


def _is_p_bit_float(c: Fraction, p: int) -> bool:
    """True iff ``c`` is representable with a ``p``-bit significand.

    Unbounded-exponent model: ``c`` must be a dyadic rational (power-of-two
    denominator) whose significant-bit width is at most ``p``. A correctly-rounded
    result is always such a value, so a ``computed`` field that fails this is
    provably not a rounding result.
    """
    if c == 0:
        return True
    a = -c if c < 0 else c
    n, d = a.numerator, a.denominator
    if d & (d - 1) != 0:            # denominator not a power of two -> not dyadic
        return False
    width = n.bit_length() - _trailing_zeros(n)
    return width <= p


def _floor_log2(a: Fraction) -> int:
    """floor(log2(a)) for exact ``a > 0``; bit-length estimate, then corrected."""
    approx = a.numerator.bit_length() - a.denominator.bit_length()
    while (Fraction(2) ** approx) > a:
        approx -= 1
    while (Fraction(2) ** (approx + 1)) <= a:
        approx += 1
    return approx


def _round_bump(frac: Fraction, m0: int, sign: int, mode: str) -> bool:
    """Whether the p-bit mantissa ``m0`` rounds UP by one, given fractional part.

    ``frac`` is in ``[0, 1)``; ``sign`` is the sign of the value; ``mode`` is one
    of ``_FP_MODES`` (validated by the caller).
    """
    if frac == 0:
        return False
    if mode == "toward_zero":
        return False
    if mode == "away_zero":
        return True
    if mode == "toward_pos":
        return sign > 0
    if mode == "toward_neg":
        return sign < 0
    # nearest_even (round half to even).
    if frac < Fraction(1, 2):
        return False
    if frac > Fraction(1, 2):
        return True
    return (m0 % 2) == 1


def _round_to_precision(e: Fraction, p: int, mode: str) -> Fraction:
    """Correctly round exact ``e`` to a ``p``-bit significand under ``mode``.

    Returns the exact rational value of the rounded float. Unbounded exponent:
    no overflow/underflow, so every finite ``e`` has a well-defined result.
    """
    if e == 0:
        return Fraction(0)
    sign = 1 if e > 0 else -1
    a = e if e > 0 else -e
    # Scale so floor(a / 2^s) has exactly p bits: 2^(p-1) <= a / 2^s < 2^p.
    s = _floor_log2(a) - (p - 1)
    scaled = a / (Fraction(2) ** s)
    # Correct any off-by-one left by the log2 estimate.
    while scaled >= (1 << p):
        s += 1
        scaled = a / (Fraction(2) ** s)
    while scaled < (1 << (p - 1)):
        s -= 1
        scaled = a / (Fraction(2) ** s)
    m0 = scaled.numerator // scaled.denominator
    frac = scaled - m0
    m = m0 + 1 if _round_bump(frac, m0, sign, mode) else m0
    if m == (1 << p):   # rounding carried into a new bit: renormalize.
        m //= 2
        s += 1
    return sign * m * (Fraction(2) ** s)


def _fp_lit(x: Any) -> Any:
    """Serialize an fp literal to an exact string, preserving a float's value.

    Producer-side convenience: a caller may hand us a real ``float`` (e.g. the
    machine result it computed); we snapshot its EXACT binary value via
    ``Fraction(float)`` so the emitted log carries no decimal drift. The checker
    itself never accepts a float (see ``_exact_rat``).
    """
    if isinstance(x, Fraction):
        return _fs(x)
    if isinstance(x, float):
        return _fs(Fraction(x))
    return x


def export_fp_rounding_cert(*, precision: int, mode: str, exact: Any,
                            computed: Any, claim: Optional[str] = None) -> dict:
    """Serialize an fp_rounding certificate.

    Attests: ``computed`` equals the correctly-rounded value of the exact
    expression ``exact`` at ``precision`` significand bits under rounding ``mode``.
    ``exact`` is a numeric literal or an ``{"op", "args"}`` expression tree;
    ``computed`` is the resulting float (its exact value is preserved).
    """
    return {
        "format": FORMAT,
        "kind": "fp_rounding",
        "claim": claim or (f"computed is the correctly-rounded ({mode}, "
                           f"{int(precision)}-bit significand) value of exact"),
        "steps": [
            {"op": "fp_value", "precision": int(precision), "mode": str(mode),
             "exact": _fp_lit(exact) if not isinstance(exact, dict) else exact,
             "computed": _fp_lit(computed)},
            {"op": "assert_correct_rounding"},
        ],
        "meta": {"producer": "fp_rounding", "cakeml_target": CAKEML_TARGET,
                 "checked": ("exact correct rounding in an unbounded-exponent "
                             "model; IEEE subnormal/overflow/NaN not modeled")},
    }


def export_fp_error_bound_cert(*, computed: Any, exact: Any, bound: Any,
                               claim: Optional[str] = None) -> dict:
    """Serialize an fp_error_bound certificate.

    Attests: ``|computed - exact| <= bound``, with all three exact rationals
    (``exact`` may be an ``{"op", "args"}`` expression tree). Fully re-checkable.
    """
    return {
        "format": FORMAT,
        "kind": "fp_error_bound",
        "claim": claim or "absolute error |computed - exact| is within bound",
        "steps": [
            {"op": "fp_error_bound",
             "computed": _fp_lit(computed),
             "exact": _fp_lit(exact) if not isinstance(exact, dict) else exact,
             "bound": _fp_lit(bound)},
            {"op": "assert_error_within_bound"},
        ],
        "meta": {"producer": "fp_error_bound", "cakeml_target": CAKEML_TARGET,
                 "checked": "exact rational recomputation of |computed - exact|"},
    }


# --------------------------------------------------------------------------- #
# Self-contained polynomial arithmetic for the WU checker (no producer import).
# --------------------------------------------------------------------------- #

class _CheckPoly:
    """Minimal multivariate poly over Fraction: ``{exponent-tuple: coeff}``.

    A faithful, independent re-implementation of the arithmetic Wu's method
    needs (pseudo-division), so the checker never trusts the producer's code.
    """

    __slots__ = ("n", "terms")

    def __init__(self, n: int, terms: Optional[dict] = None):
        self.n = n
        self.terms: dict[tuple, Fraction] = {}
        if terms:
            for m, c in terms.items():
                c = Fraction(c)
                if c != 0:
                    self.terms[tuple(m)] = c

    def is_zero(self) -> bool:
        return not self.terms

    def __add__(self, other: "_CheckPoly") -> "_CheckPoly":
        out = dict(self.terms)
        for m, c in other.terms.items():
            nc = out.get(m, Fraction(0)) + c
            if nc == 0:
                out.pop(m, None)
            else:
                out[m] = nc
        return _CheckPoly(self.n, out)

    def __neg__(self) -> "_CheckPoly":
        return _CheckPoly(self.n, {m: -c for m, c in self.terms.items()})

    def __sub__(self, other: "_CheckPoly") -> "_CheckPoly":
        return self + (-other)

    def __mul__(self, other: "_CheckPoly") -> "_CheckPoly":
        out: dict[tuple, Fraction] = {}
        for m1, c1 in self.terms.items():
            for m2, c2 in other.terms.items():
                m = tuple(a + b for a, b in zip(m1, m2))
                nc = out.get(m, Fraction(0)) + c1 * c2
                if nc == 0:
                    out.pop(m, None)
                else:
                    out[m] = nc
        return _CheckPoly(self.n, out)

    def class_index(self) -> int:
        cls = -1
        for m in self.terms:
            for i in range(self.n - 1, cls, -1):
                if m[i] > 0:
                    cls = i
                    break
        return cls

    def degree_in(self, v: int) -> int:
        return max((m[v] for m in self.terms), default=0)

    def coeff_in(self, v: int, d: int) -> "_CheckPoly":
        out: dict[tuple, Fraction] = {}
        for m, c in self.terms.items():
            if m[v] == d:
                mm = list(m); mm[v] = 0
                out[tuple(mm)] = c
        return _CheckPoly(self.n, out)

    def leading_coeff_in(self, v: int) -> "_CheckPoly":
        return self.coeff_in(v, self.degree_in(v))

    def mul_x_pow(self, v: int, k: int) -> "_CheckPoly":
        if k == 0:
            return self
        out: dict[tuple, Fraction] = {}
        for m, c in self.terms.items():
            mm = list(m); mm[v] += k
            out[tuple(mm)] = c
        return _CheckPoly(self.n, out)


def _deserialize_poly(obj: dict, nvars: int) -> _CheckPoly:
    if not isinstance(obj, dict) or "terms" not in obj:
        raise ValueError("malformed polynomial")
    n = int(obj.get("n", nvars))
    if n != nvars:
        raise ValueError("polynomial arity mismatch")
    terms: dict[tuple, Fraction] = {}
    for entry in obj["terms"]:
        exp, coeff = entry
        exp = tuple(int(e) for e in exp)
        if len(exp) != nvars or any(e < 0 for e in exp):
            raise ValueError("malformed monomial")
        terms[exp] = _frac(coeff)
    return _CheckPoly(nvars, terms)


def _pseudo_remainder(g: _CheckPoly, f: _CheckPoly, v: int) -> _CheckPoly:
    df = f.degree_in(v)
    if df == 0:
        raise ValueError("divisor constant in variable")
    lcf = f.leading_coeff_in(v)
    r = g
    span = g.degree_in(v) - df + 2 if g.degree_in(v) >= df else 0
    for _ in range(span):
        dr = r.degree_in(v)
        if r.is_zero() or dr < df:
            break
        lcr = r.leading_coeff_in(v)
        r = (lcf * r) - (lcr * f.mul_x_pow(v, dr - df))
    return r


def _wu_reduce(goal: _CheckPoly, chain: list[_CheckPoly]) -> _CheckPoly:
    r = goal
    for f in sorted(chain, key=lambda p: p.class_index(), reverse=True):
        v = f.class_index()
        if v < 0:
            continue
        if r.degree_in(v) >= f.degree_in(v):
            r = _pseudo_remainder(r, f, v)
        if r.is_zero():
            break
    return r


# --------------------------------------------------------------------------- #
# REFERENCE CHECKER.
# --------------------------------------------------------------------------- #

class _Reject(Exception):
    """Raised to reject a certificate with a human-readable reason."""


class _Unsupported(_Reject):
    """Raised when the kind belongs to a sibling checker, not to this module.

    A subclass of :class:`_Reject` on purpose: whatever else changes, the
    document must still come back with ``valid=False``.  "I do not check this"
    must never leak out as "this is fine".  The separate type exists only so a
    caller can tell *unknown here* from *refuted here* and re-route.
    """

    def __init__(self, kind: str, owner: str):
        super().__init__(
            f"kind {kind!r} is not checked by cert_log; it is owned by {owner}. "
            "This is NOT a verdict on the certificate: route it to that module."
        )
        self.owner = owner


class _Withhold(_Reject):
    """Raised for a kind we OWN whose instance we cannot decide exactly.

    A subclass of :class:`_Reject` on purpose: a withheld document must still come
    back ``valid=False`` (fail-closed, never a fabricated pass). The separate type
    only lets the caller tell "I decline to decide this" from "I refuted this":
    unlike a rejection, a withhold makes no claim that the certificate is wrong.
    """


def _need(cond: bool, reason: str) -> None:
    if not cond:
        raise _Reject(reason)


def _vec(values: Any) -> list[Fraction]:
    _need(isinstance(values, list), "expected a list of rationals")
    return [_frac(v) for v in values]


# -- LP primal/dual handlers ------------------------------------------------- #

def _h_lp_problem(step, ctx):
    variables = step["variables"]
    _need(isinstance(variables, list), "lp_problem: variables must be a list")
    G = [_vec(r) for r in step["G"]]
    h = _vec(step["h"])
    c = _vec(step["c"])
    _need(len(G) == len(h), "lp_problem: |G| != |h|")
    _need(all(len(r) == len(variables) for r in G), "lp_problem: row width != #vars")
    _need(len(c) == len(variables), "lp_problem: |c| != #vars")
    ctx.update(G=G, h=h, c=c, variables=variables, sense=step.get("sense", "max"))


def _h_dual_vector(step, ctx):
    y = _vec(step["y"])
    _need("G" in ctx, "dual_vector before lp_problem")
    _need(len(y) == len(ctx["G"]), "dual_vector length != #rows")
    ctx["y"] = y


def _h_primal_vector(step, ctx):
    x = _vec(step["x"])
    _need(len(x) == len(ctx["variables"]), "primal_vector length != #vars")
    ctx["x"] = x


def _h_assert_dual_nonneg(step, ctx):
    _need(all(v >= 0 for v in ctx["y"]), "dual y has a negative entry (y >= 0 violated)")


def _h_assert_dual_feasible(step, ctx):
    G, c, y = ctx["G"], ctx["c"], ctx["y"]
    n = len(c)
    gTy = [sum(G[k][j] * y[k] for k in range(len(G))) for j in range(n)]
    _need(all(gTy[j] >= c[j] for j in range(n)),
          "dual infeasible: (G^T y)_j >= c_j violated for some j")
    ctx["gTy"] = gTy


def _h_assert_primal_feasible(step, ctx):
    G, h, x = ctx["G"], ctx["h"], ctx["x"]
    _need(all(v >= 0 for v in x), "primal x has a negative entry (x >= 0 violated)")
    for k in range(len(G)):
        lhs = sum(G[k][j] * x[j] for j in range(len(x)))
        _need(lhs <= h[k], f"primal infeasible: row {k} has G_k.x > h_k")


def _h_assert_complementary_slackness(step, ctx):
    G, h, x, y = ctx["G"], ctx["h"], ctx["x"], ctx["y"]
    for k in range(len(G)):
        slack = h[k] - sum(G[k][j] * x[j] for j in range(len(x)))
        _need(y[k] * slack == 0, f"complementary slackness fails at row {k}")


def _h_assert_strong_duality(step, ctx):
    c, x, h, y = ctx["c"], ctx["x"], ctx["h"], ctx["y"]
    cx = sum(c[j] * x[j] for j in range(len(x)))
    hy = sum(h[k] * y[k] for k in range(len(h)))
    _need(cx == hy, f"strong duality fails: c.x = {cx} != h.y = {hy}")
    ctx["concluded"] = True


def _h_assert_bound(step, ctx):
    h, y = ctx["h"], ctx["y"]
    hy = sum(h[k] * y[k] for k in range(len(h)))
    _need(hy == _frac(step["bound"]), f"bound mismatch: h.y = {hy} != {step['bound']}")
    if "x" in ctx:
        cx = sum(ctx["c"][j] * ctx["x"][j] for j in range(len(ctx["x"])))
        _need(cx <= hy, "claimed bound is not an upper bound on c.x")
    ctx["concluded"] = True


# -- Farkas handlers (shared by lp_farkas and asymptotic branches) ----------- #

def _check_farkas(rows: Any, m: Any) -> None:
    """Re-verify a Farkas infeasibility: m>=0, sum m_k a_k == 0, sum m_k b_k < 0."""
    _need(isinstance(rows, list) and isinstance(m, list), "farkas: rows/m must be lists")
    _need(len(rows) == len(m), "farkas: #rows != #multipliers")
    mult = [_frac(v) for v in m]
    _need(all(v >= 0 for v in mult), "farkas: a multiplier is negative")
    combo: dict[str, Fraction] = {}
    b_sum = Fraction(0)
    strict = False
    for row, mk in zip(rows, mult):
        _need(isinstance(row, dict), "farkas: malformed row")
        a = {str(k): _frac(v) for k, v in dict(row.get("a", {})).items()}
        b = _frac(row["b"])
        for var, coeff in a.items():
            combo[var] = combo.get(var, Fraction(0)) + mk * coeff
        b_sum += mk * b
        if bool(row.get("strict")) and mk > 0:
            strict = True
    combo = {k: v for k, v in combo.items() if v != 0}
    _need(not combo, f"farkas: combination is not the zero row (residual {combo})")
    if strict:
        _need(b_sum <= 0, f"farkas: strict combination needs rhs <= 0, got {b_sum}")
    else:
        _need(b_sum < 0, f"farkas: combination needs rhs < 0, got {b_sum}")


def _h_farkas_system(step, ctx):
    ctx["farkas_rows"] = step["rows"]


def _h_farkas_multipliers(step, ctx):
    ctx["farkas_m"] = step["m"]


def _h_assert_multipliers_nonneg(step, ctx):
    _need(all(_frac(v) >= 0 for v in ctx["farkas_m"]), "a multiplier is negative")


def _h_assert_combination_zero(step, ctx):
    # Recompute the combination independently and stash whether it is zero.
    combo: dict[str, Fraction] = {}
    mult = [_frac(v) for v in ctx["farkas_m"]]
    _need(len(mult) == len(ctx["farkas_rows"]), "combination: #rows != #m")
    for row, mk in zip(ctx["farkas_rows"], mult):
        for var, coeff in dict(row.get("a", {})).items():
            combo[str(var)] = combo.get(str(var), Fraction(0)) + mk * _frac(coeff)
    combo = {k: v for k, v in combo.items() if v != 0}
    _need(not combo, f"combination is not the zero row (residual {combo})")


def _h_assert_contradiction(step, ctx):
    # Full independent re-check (also re-verifies nonneg + zero combination).
    _check_farkas(ctx["farkas_rows"], ctx["farkas_m"])
    ctx["concluded"] = True


# -- asymptotic handlers ----------------------------------------------------- #

def _h_asymptotic_goal(step, ctx):
    ctx["expected_branches"] = int(step["branches"])
    ctx["seen_branches"] = 0
    _need(ctx["expected_branches"] >= 1, "asymptotic: need at least one branch")


def _h_branch_farkas(step, ctx):
    _check_farkas(step["rows"], step["m"])
    ctx["seen_branches"] = ctx.get("seen_branches", 0) + 1
    # The goal is proved only once EVERY branch has contradicted.
    if ctx["seen_branches"] >= ctx.get("expected_branches", 0):
        ctx["concluded"] = True


# -- Wu geometry handlers ---------------------------------------------------- #

def _h_declare_ring(step, ctx):
    ctx["nvars"] = int(step["nvars"])
    ctx["names"] = list(step["names"])
    _need(len(ctx["names"]) == ctx["nvars"], "declare_ring: #names != nvars")


def _h_characteristic_set(step, ctx):
    ctx["chain"] = [_deserialize_poly(p, ctx["nvars"]) for p in step["polys"]]


def _h_goal_polynomials(step, ctx):
    ctx["goal_polys"] = [_deserialize_poly(p, ctx["nvars"]) for p in step["polys"]]


def _h_non_degeneracy(step, ctx):
    # Well-formedness: each non-degeneracy initial must be a non-zero polynomial.
    for p in step.get("conditions", []):
        poly = _deserialize_poly(p, ctx["nvars"])
        _need(not poly.is_zero(), "non-degeneracy condition is the zero polynomial")


def _h_assert_pseudo_remainders_zero(step, ctx):
    chain = ctx.get("chain")
    goals = ctx.get("goal_polys")
    _need(chain is not None and goals is not None,
          "pseudo-remainder check before chain/goal declared")
    _need(len(goals) >= 1, "no goal polynomials to reduce")
    for i, g in enumerate(goals):
        rem = _wu_reduce(g, chain)
        _need(rem.is_zero(),
              f"goal polynomial {i} has NONZERO pseudo-remainder "
              f"(#terms={len(rem.terms)}); not an algebraic consequence")
    ctx["concluded"] = True


# -- subsumption handlers ---------------------------------------------------- #

def _apply_theta(literal: str, theta: dict[str, str]) -> str:
    import re
    def repl(match):
        tok = match.group(0)
        return theta.get(tok, tok)
    return re.sub(r"[A-Za-z_][A-Za-z0-9_]*", repl, literal)


def _h_subsumption_relation(step, ctx):
    ctx["subsumer"] = [str(x) for x in step["subsumer"]]
    ctx["subsumed"] = [str(x) for x in step["subsumed"]]
    ctx["theta"] = {str(k): str(v) for k, v in dict(step["substitution"]).items()}


def _h_assert_theta_subsumes(step, ctx):
    target = set(ctx["subsumed"])
    for lit in ctx["subsumer"]:
        sub = _apply_theta(lit, ctx["theta"])
        _need(sub in target,
              f"subsumption fails: literal {lit!r} -> {sub!r} not in subsumed set")
    ctx["concluded"] = True


# -- floating-point handlers ------------------------------------------------- #

def _h_fp_value(step, ctx):
    p = step["precision"]
    # bool is an int subclass; a boolean precision is nonsense, so reject it.
    if not isinstance(p, int) or isinstance(p, bool) or p < 1:
        raise _Reject("fp_rounding: precision must be a positive integer")
    mode = str(step["mode"])
    if mode not in _FP_MODES:
        # Unmodeled rounding rule: decline rather than fabricate a verdict.
        raise _Withhold(f"fp_rounding: rounding mode {mode!r} is not modeled here")
    ctx["fp_precision"] = p
    ctx["fp_mode"] = mode
    ctx["fp_exact"] = step["exact"]        # evaluated at assert time (may withhold)
    ctx["fp_computed"] = step["computed"]


def _h_assert_correct_rounding(step, ctx):
    p = ctx["fp_precision"]
    c = _exact_rat(ctx["fp_computed"])
    # A correctly-rounded result is always representable; if `computed` is not a
    # p-bit float it cannot be that result, so this is a genuine refutation.
    _need(_is_p_bit_float(c, p),
          f"fp_rounding: computed value {c} is not a {p}-bit float")
    e = _eval_fp_exact(ctx["fp_exact"])    # may raise _Withhold for irrational ops
    r = _round_to_precision(e, p, ctx["fp_mode"])
    _need(c == r,
          f"fp_rounding: computed {c} != correctly-rounded value {r} "
          f"of exact {e} at {p} bits ({ctx['fp_mode']})")
    ctx["concluded"] = True


def _h_fp_error_bound(step, ctx):
    ctx["fpe_computed"] = step["computed"]
    ctx["fpe_exact"] = step["exact"]
    ctx["fpe_bound"] = step["bound"]


def _h_assert_error_within_bound(step, ctx):
    b = _exact_rat(ctx["fpe_bound"])
    # A negative bound is meaningless for an absolute error and cannot be met.
    _need(b >= 0, f"fp_error_bound: bound {b} is negative")
    c = _exact_rat(ctx["fpe_computed"])
    e = _eval_fp_exact(ctx["fpe_exact"])   # may raise _Withhold for irrational ops
    err = c - e
    if err < 0:
        err = -err
    _need(err <= b,
          f"fp_error_bound: |computed - exact| = {err} exceeds bound {b}")
    ctx["concluded"] = True


_HANDLERS = {
    "lp_problem": _h_lp_problem,
    "dual_vector": _h_dual_vector,
    "primal_vector": _h_primal_vector,
    "assert_dual_nonneg": _h_assert_dual_nonneg,
    "assert_dual_feasible": _h_assert_dual_feasible,
    "assert_primal_feasible": _h_assert_primal_feasible,
    "assert_complementary_slackness": _h_assert_complementary_slackness,
    "assert_strong_duality": _h_assert_strong_duality,
    "assert_bound": _h_assert_bound,
    "farkas_system": _h_farkas_system,
    "farkas_multipliers": _h_farkas_multipliers,
    "assert_multipliers_nonneg": _h_assert_multipliers_nonneg,
    "assert_combination_zero": _h_assert_combination_zero,
    "assert_contradiction": _h_assert_contradiction,
    "asymptotic_goal": _h_asymptotic_goal,
    "branch_farkas": _h_branch_farkas,
    "declare_ring": _h_declare_ring,
    "characteristic_set": _h_characteristic_set,
    "goal_polynomials": _h_goal_polynomials,
    "non_degeneracy": _h_non_degeneracy,
    "assert_pseudo_remainders_zero": _h_assert_pseudo_remainders_zero,
    "subsumption_relation": _h_subsumption_relation,
    "assert_theta_subsumes": _h_assert_theta_subsumes,
    "fp_value": _h_fp_value,
    "assert_correct_rounding": _h_assert_correct_rounding,
    "fp_error_bound": _h_fp_error_bound,
    "assert_error_within_bound": _h_assert_error_within_bound,
}

# Which ops are legal in which kind (a step from the wrong kind is rejected).
_KIND_OPS = {
    "lp_primal_dual": {"lp_problem", "dual_vector", "primal_vector",
                       "assert_dual_nonneg", "assert_dual_feasible",
                       "assert_primal_feasible", "assert_complementary_slackness",
                       "assert_strong_duality", "assert_bound"},
    "lp_farkas": {"farkas_system", "farkas_multipliers",
                  "assert_multipliers_nonneg", "assert_combination_zero",
                  "assert_contradiction"},
    "asymptotic": {"asymptotic_goal", "branch_farkas"},
    "wu_geometry": {"declare_ring", "characteristic_set", "goal_polynomials",
                    "non_degeneracy", "assert_pseudo_remainders_zero"},
    "subsumption": {"subsumption_relation", "assert_theta_subsumes"},
    "fp_rounding": {"fp_value", "assert_correct_rounding"},
    "fp_error_bound": {"fp_error_bound", "assert_error_within_bound"},
}


def check_cert_log(log: Any) -> dict:
    """Independently RE-VERIFY a cert-log document.

    Returns ``{valid, status, reason, checked_steps, kind, claim,
    claim_verified, verified_statement}``.  Recomputes every assertion from the raw rationals in
    the log with exact arithmetic; it never trusts a producer's own verdict.  Any
    malformed, tampered, or unsatisfied step yields ``valid=False`` with a
    ``reason`` — this is the sound boundary.

    IMPORTANT: ``valid`` attests ONLY to the certificate's mathematical content.
    The ``claim`` field is untrusted producer display text and is **never**
    checked against what the steps prove, so ``claim_verified`` is always
    ``False``; ``verified_statement`` is the machine-derived description of what
    was actually re-verified.  Do not present ``claim`` as a proven statement.

    ``status`` is ``verified`` / ``rejected`` / ``unsupported_kind`` /
    ``withheld``.  ``unsupported_kind`` means the document names a kind owned by a
    sibling checker module (not checked here).  ``withheld`` means a kind we DO
    own whose specific instance falls outside what this checker can decide exactly
    (an unmodeled rounding mode, or an exact expression using an irrational
    operation): it is fail-closed, neither a pass nor a refutation.  Only
    ``verified`` carries ``valid=True``; treat every other status as unchecked.
    """
    checked = 0
    try:
        _need(isinstance(log, dict), "log is not a JSON object")
        _need(log.get("format") == FORMAT, f"unknown format: {log.get('format')!r}")
        kind = log.get("kind")
        if kind not in KINDS:
            # Fail closed either way; only the phrasing/status differs, so that a
            # sibling module's certificate is not misreported as refuted.
            owner = FOREIGN_KIND_OWNERS.get(kind) if isinstance(kind, str) else None
            if owner is not None:
                raise _Unsupported(kind, owner)
            raise _Reject(f"unknown kind: {kind!r}")
        steps = log.get("steps")
        _need(isinstance(steps, list) and steps, "steps must be a non-empty list")
        _need(isinstance(log.get("claim", ""), str), "claim must be a string")
        allowed = _KIND_OPS[kind]

        ctx: dict[str, Any] = {"concluded": False}
        for i, step in enumerate(steps):
            _need(isinstance(step, dict), f"step {i} is not an object")
            op = step.get("op")
            _need(op in _HANDLERS, f"step {i}: unknown op {op!r}")
            _need(op in allowed, f"step {i}: op {op!r} illegal for kind {kind!r}")
            try:
                _HANDLERS[op](step, ctx)
            except _Reject:
                raise
            except (KeyError, IndexError, TypeError, ValueError, ZeroDivisionError) as exc:
                raise _Reject(f"step {i} ({op}): malformed data ({exc})")
            checked += 1

        _need(ctx.get("concluded"), "log reached no verified conclusion step")
        if kind == "asymptotic":
            _need(ctx.get("seen_branches") == ctx.get("expected_branches"),
                  "asymptotic: branch count mismatch")
        # SOUNDNESS: `valid` means ONLY that the certificate's mathematical steps
        # re-verify. The producer-supplied `claim` is free display text that is
        # NEVER checked against what the steps actually prove — a valid LP cert can
        # carry the claim "the Riemann hypothesis is true" and its math still
        # checks. So the claim is returned explicitly as unverified, alongside a
        # machine-derived statement of what WAS checked, so no caller can mistake
        # the free text for a proven fact. (Binding the claim string to the
        # certificate content is a deeper per-kind follow-up.)
        return {"valid": True, "status": STATUS_VERIFIED,
                "reason": "all steps independently re-verified",
                "checked_steps": checked, "kind": kind,
                "claim": log.get("claim"),
                "claim_verified": False,
                "verified_statement": (
                    f"the {kind} certificate's {checked} step(s) are internally "
                    "re-verified with exact arithmetic; this does NOT assert the "
                    "free-text `claim` field"
                )}
    except _Reject as exc:
        # _Withhold is checked first: it is a subclass of _Reject (and disjoint
        # from _Unsupported), and must map to its own fail-closed status.
        withheld = isinstance(exc, _Withhold)
        unsupported = isinstance(exc, _Unsupported)
        if withheld:
            status = STATUS_WITHHELD
        elif unsupported:
            status = STATUS_UNSUPPORTED
        else:
            status = STATUS_REJECTED
        out = {"valid": False, "status": status,
               "reason": str(exc), "checked_steps": checked,
               "kind": log.get("kind") if isinstance(log, dict) else None,
               "claim": log.get("claim") if isinstance(log, dict) else None,
               "claim_verified": False}
        if unsupported:
            # Where to go for an actual verdict.  Naming the owner does not imply
            # the document is well-formed or true; nothing here has been checked.
            out["checker"] = exc.owner
            out["verified_statement"] = (
                "NOTHING was verified here: this kind is checked by another module"
            )
        elif withheld:
            # Well-formed but outside what we can decide exactly: NOT a pass and
            # NOT a refutation. A fuller checker may still validate or refute it.
            out["verified_statement"] = (
                "WITHHELD: this certificate is well-formed but falls outside what "
                "this reference checker can verify exactly; it is neither a pass "
                "nor a refutation"
            )
        return out


# --------------------------------------------------------------------------- #
# Worker dispatch.
# --------------------------------------------------------------------------- #

def run(request: dict) -> dict:
    """Worker entrypoint.  ``request["op"]`` is ``export`` or ``check``.

    * ``export`` -> serialize a producer certificate to a cert-log document.
      Requires ``kind`` and ``cert`` (the producer result); LP/geometry also
      need the original problem inputs (``constraints``/``objective`` or
      ``points``/``hypotheses``/``goal``).  Returns ``{"log": <document>}``.
    * ``check`` -> ``check_cert_log(request["log"])``.
    """
    op = request.get("op", "check")
    if op == "check":
        return check_cert_log(request["log"])
    if op == "export":
        kind = request.get("kind")
        cert = request.get("cert", {})
        if kind in ("lp", "lp_primal_dual", "lp_farkas"):
            log = export_lp_cert(cert, constraints=request["constraints"],
                                 objective=request.get("objective"),
                                 sense=request.get("sense", "max"),
                                 claim=request.get("claim"))
        elif kind == "asymptotic":
            log = export_asymptotic_cert(cert, claim=request.get("claim"))
        elif kind in ("wu_geometry", "geometry"):
            log = export_geometry_cert(cert, points=request["points"],
                                       hypotheses=request.get("hypotheses", []),
                                       goal=request["goal"],
                                       var_order=request.get("var_order"),
                                       claim=request.get("claim"))
        elif kind == "subsumption":
            log = export_subsumption_cert(request["subsumer"], request["subsumed"],
                                          request["substitution"],
                                          claim=request.get("claim"))
        elif kind == "fp_rounding":
            log = export_fp_rounding_cert(precision=request["precision"],
                                          mode=request["mode"],
                                          exact=request["exact"],
                                          computed=request["computed"],
                                          claim=request.get("claim"))
        elif kind == "fp_error_bound":
            log = export_fp_error_bound_cert(computed=request["computed"],
                                             exact=request["exact"],
                                             bound=request["bound"],
                                             claim=request.get("claim"))
        else:
            raise ValueError(f"unknown export kind: {kind!r}")
        return {"log": log}
    raise ValueError(f"unknown op: {op!r}")


def main() -> None:
    import sys
    if len(sys.argv) >= 2:
        with open(sys.argv[1], encoding="utf-8") as fh:
            request = json.load(fh)
    else:
        request = json.load(sys.stdin)
    print(json.dumps(run(request), indent=2, default=str))
    raise SystemExit(0)


if __name__ == "__main__":
    main()
