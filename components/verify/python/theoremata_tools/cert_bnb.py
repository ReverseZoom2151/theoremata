"""Branch-and-bound nonlinear-inequality **certificate** (exporter + reference checker).

A great many analytic obligations reduce to *proving a nonlinear inequality on a
box*: ``f(x) >= 0`` (or ``> 0``) for all ``x`` in a domain ``[lo, hi]`` (possibly
multivariate).  The Flyspeck project discharged thousands of such inequalities
with the classic **branch-and-bound** method: recursively bisect the box, bound
``f`` on each sub-box with validated interval arithmetic, and stop a branch once
the bound proves the sub-box safe.  This module ports that *method* (clean-room
from the math — the Flyspeck ``formal_ineqs`` code is MIT, but nothing is copied)
into Theoremata's ``theoremata.cert-log.v1`` proof-log format (kind
``bnb_inequality``) and ships a **self-contained REFERENCE CHECKER**.

The certificate is a **branch-and-bound RESULT TREE**.  Each node covers a
sub-box and carries a verdict:

* ``pass`` — an interval enclosure of ``f`` over the sub-box has lower bound
  ``>= 0`` (``> 0`` for a strict claim); the leaf is *discharged*.
* ``split`` — the sub-box is partitioned along one coordinate into child
  sub-boxes whose union is exactly the parent; each child is a node.
* ``mono`` — ``f`` is monotone in a coordinate over the sub-box (the interval
  derivative is sign-definite), so the minimum lies on one face; the node reduces
  to that single face child.
* ``false`` — a rational point in the sub-box with ``f < 0``: a native
  *falsification* (the inequality is refuted).

Reuse, not reinvention
----------------------
Per the project principle "reuse existing libs first": ``sympy`` bundles
``mpmath``, and ``mpmath.iv`` already provides *validated* (outward-rounded,
fixed-precision) interval arithmetic and interval elementary functions
(``iv.exp``, ``iv.sin`` …).  The checker evaluates every ``f``-enclosure and every
interval derivative with ``mpmath.iv`` at a fixed ``dps`` — it does **not**
hand-roll interval arithmetic.  Because ``mpmath.iv`` rounds outward, each
recomputed enclosure is a guaranteed *superset* of the true range, so a passing
check is sound.  (The one place we specialise is integer powers, where naive
repeated multiplication would suffer the interval *dependency* problem for even
exponents; :func:`_iv_pow` computes the tight, still-outward-rounded monotone
enclosure.)

Soundness boundary
------------------
:func:`check` is the sound boundary.  Walking the tree it recomputes, from the
raw numbers in the log only:

* for each ``pass`` leaf, the interval enclosure of ``f`` over the sub-box, and
  requires its lower bound ``>= 0`` (``> 0`` if strict) — REJECTS an **unsound
  pass** whose enclosure dips below 0;
* for each ``split``, that the children **exactly tile** the parent box along the
  stated axis (no gap, no overlap, endpoints flush) — REJECTS a **bogus split**
  that leaves an uncovered region;
* for each ``mono``, that the interval derivative is sign-definite and the child
  is the correct face;
* that the **root box equals the declared domain**, so — since splits tile
  exactly and ``mono`` soundly reduces a box to a face — every point of the
  domain lies in some discharged leaf.

If any ``false`` leaf carries a point where the recomputed ``f`` is ``< 0``, the
claim is **refuted** and the witness is reported.  Everything is deterministic
(fixed ``mpmath`` precision; no wall-clock, no RNG) and every input is treated as
UNTRUSTED DATA: malformed structure becomes ``valid=False`` with a ``reason``
rather than an exception.

Honest scope note
-----------------
This is a *fixed-precision interval* reference checker — the offline stand-in.
The trust-critical CHECKER is ours and is pure (mpmath.iv at a fixed ``dps``);
*finding* the branch-and-bound tree is the untrusted search seam and is not part
of the trusted base.  A fully HOL-Light-formalized branch-and-bound checker (à la
Flyspeck ``formal_ineqs``) is the toolchain-gated upgrade; this checker
re-verifies the *result tree*, not the search that produced it.

Worker dispatch key: ``cert_bnb`` (see :func:`run`).
"""
from __future__ import annotations

import itertools
from fractions import Fraction
from typing import Any, Optional

FORMAT = "theoremata.cert-log.v1"
KINDS = ("bnb_inequality",)

# Fixed default working precision (decimal digits) for the interval checker.
DEFAULT_DPS = 30
# Default mincing count per axis for a ``pass`` enclosure (1 == single eval;
# the tree's own splits are the primary tightening device).
DEFAULT_SUBDIVISIONS = 1

# Defensive bounds on an untrusted tree.
MAX_NODES = 200_000
MAX_DEPTH = 2_000
MAX_GRID_CELLS = 50_000

CAKEML_TARGET = (
    "hol_bnb_ineq_check: a HOL-Light/Flyspeck-formal_ineqs-style branch-and-bound "
    "checker whose interval-enclosure semantics match this module; consumes "
    "theoremata.cert-log.v1 bnb_inequality documents unchanged. This mpmath.iv "
    "reference checker is the fixed-precision offline stand-in."
)

_UNARY_FUNCS = ("exp", "log", "sin", "cos", "sqrt", "sinh", "cosh", "tanh", "atan")


# --------------------------------------------------------------------------- #
# Exact-rational helpers (numbers travel as strings in the log).
# --------------------------------------------------------------------------- #

def _frac(x: Any) -> Fraction:
    """Parse a number exactly (via ``str`` to avoid float drift)."""
    if isinstance(x, Fraction):
        return x
    if isinstance(x, bool):  # bools are ints in Python; forbid them as numbers
        raise TypeError("boolean where a rational was expected")
    if isinstance(x, int):
        return Fraction(x)
    return Fraction(str(x))


def _fs(x: Any) -> str:
    """Serialize a number as an exact rational string ``"p"`` or ``"p/q"``."""
    return str(_frac(x))


# --------------------------------------------------------------------------- #
# Expression AST.
#
# A tiny, JSON-serializable expression language over which the checker both
# (a) evaluates interval enclosures with mpmath.iv and (b) computes symbolic
# derivatives with its OWN (clean-room) differentiation rules, so nothing but
# the raw numbers in the log is trusted.  Node forms:
#
#   ["const", "<rational>"]                      a constant
#   ["var", "<name>"]                            a variable
#   ["+", e1, e2, ...]   ["*", e1, e2, ...]      n-ary sum / product
#   ["-", e1, e2]        ["neg", e]              subtraction / negation
#   ["/", e1, e2]                                division
#   ["^", e, "<int>"]                            integer power
#   ["exp", e] ["log", e] ["sin", e] ["cos", e]  unary elementary functions
#   ["sqrt", e] ["sinh", e] ["cosh", e] ["tanh", e] ["atan", e]
# --------------------------------------------------------------------------- #

def _as_expr(node: Any) -> list:
    """Coerce a bare number into a ``const`` node; pass lists through."""
    if isinstance(node, list):
        return node
    return ["const", _fs(node)]


def _norm_expr(node: Any) -> list:
    """Validate + canonicalize an (untrusted) expression AST.

    Rejects unknown tags / malformed arity by raising ``ValueError``; folds
    constants to canonical rational strings.  Used by both the exporter and the
    checker so a tampered expression is caught as malformed data.
    """
    node = _as_expr(node)
    if not node or not isinstance(node[0], str):
        raise ValueError("malformed expression node")
    tag = node[0]
    if tag == "const":
        return ["const", _fs(node[1])]
    if tag == "var":
        return ["var", str(node[1])]
    if tag in ("+", "*"):
        if len(node) < 2:
            raise ValueError(f"{tag} needs >= 1 operand")
        return [tag] + [_norm_expr(a) for a in node[1:]]
    if tag in ("-", "/"):
        if len(node) != 3:
            raise ValueError(f"{tag} is binary")
        return [tag, _norm_expr(node[1]), _norm_expr(node[2])]
    if tag == "neg":
        if len(node) != 2:
            raise ValueError("neg is unary")
        return ["neg", _norm_expr(node[1])]
    if tag == "^":
        if len(node) != 3:
            raise ValueError("^ takes (base, integer-exponent)")
        return ["^", _norm_expr(node[1]), str(int(node[2]))]
    if tag in _UNARY_FUNCS:
        if len(node) != 2:
            raise ValueError(f"{tag} is unary")
        return [tag, _norm_expr(node[1])]
    raise ValueError(f"unknown expression tag {tag!r}")


# --------------------------------------------------------------------------- #
# Interval evaluation (mpmath.iv).
# --------------------------------------------------------------------------- #

def _iv_ctx():
    """Import mpmath's validated interval context (ships with sympy)."""
    from mpmath import iv, mpf  # local import: guarded by importorskip in tests
    return iv, mpf


def _to_iv(iv, x: Fraction):
    """Tight interval enclosing the exact rational ``x`` (outward-rounded)."""
    return iv.mpf(x.numerator) / iv.mpf(x.denominator)


def _lo(interval):
    return interval.a


def _hi(interval):
    return interval.b


def _hull(iv, intervals):
    """Interval hull (union bounding box) of a non-empty list of intervals."""
    lo = intervals[0].a
    hi = intervals[0].b
    for it in intervals[1:]:
        if it.a < lo:
            lo = it.a
        if it.b > hi:
            hi = it.b
    return iv.mpf([lo, hi])


def _iv_pow(iv, X, n: int):
    """Tight, outward-rounded integer power of an interval ``X``.

    Naive repeated multiplication suffers the dependency problem for even
    exponents (``[-2,2]**2`` would widen to ``[-4,4]``).  We evaluate the true
    monotone enclosure on the endpoints (each a *degenerate* interval, so no
    dependency) and keep outward rounding.
    """
    if n == 0:
        return iv.mpf(1)
    if n < 0:
        return iv.mpf(1) / _iv_pow(iv, X, -n)
    a, b = X.a, X.b
    A = iv.mpf(a)
    B = iv.mpf(b)
    An = A
    Bn = B
    for _ in range(n - 1):
        An = An * A
        Bn = Bn * B
    if n % 2 == 1:
        return iv.mpf([An.a, Bn.b])
    if a >= 0:
        return iv.mpf([An.a, Bn.b])
    if b <= 0:
        return iv.mpf([Bn.a, An.b])
    hi = An.b if An.b > Bn.b else Bn.b
    return iv.mpf([iv.mpf(0).a, hi])


def _eval_iv(node: Any, env: dict, iv):
    """Evaluate an expression AST over an interval environment with mpmath.iv."""
    node = _as_expr(node)
    tag = node[0]
    if tag == "const":
        return _to_iv(iv, _frac(node[1]))
    if tag == "var":
        name = str(node[1])
        if name not in env:
            raise ValueError(f"unbound variable {name!r}")
        return env[name]
    if tag == "+":
        acc = iv.mpf(0)
        for a in node[1:]:
            acc = acc + _eval_iv(a, env, iv)
        return acc
    if tag == "*":
        acc = iv.mpf(1)
        for a in node[1:]:
            acc = acc * _eval_iv(a, env, iv)
        return acc
    if tag == "-":
        return _eval_iv(node[1], env, iv) - _eval_iv(node[2], env, iv)
    if tag == "neg":
        return -_eval_iv(node[1], env, iv)
    if tag == "/":
        return _eval_iv(node[1], env, iv) / _eval_iv(node[2], env, iv)
    if tag == "^":
        return _iv_pow(iv, _eval_iv(node[1], env, iv), int(node[2]))
    if tag in _UNARY_FUNCS:
        return getattr(iv, tag)(_eval_iv(node[1], env, iv))
    raise ValueError(f"cannot evaluate expression tag {tag!r}")


# --------------------------------------------------------------------------- #
# Symbolic differentiation (clean-room; produces an AST the checker re-evaluates
# in interval arithmetic to certify a ``mono`` node's derivative sign).
# --------------------------------------------------------------------------- #

def _C(x: Any) -> list:
    return ["const", _fs(x)]


def _is_const(node: list) -> bool:
    return isinstance(node, list) and node and node[0] == "const"


def _mk_neg(a: list) -> list:
    if _is_const(a):
        return _C(-_frac(a[1]))
    return ["neg", a]


def _mk_add(terms: list) -> list:
    flat: list = []
    const = Fraction(0)
    for t in terms:
        t = _as_expr(t)
        if t[0] == "+":
            flat.extend(t[1:])
        else:
            flat.append(t)
    out: list = []
    for t in flat:
        if _is_const(t):
            const += _frac(t[1])
        else:
            out.append(t)
    if const != 0:
        out.append(_C(const))
    if not out:
        return _C(0)
    if len(out) == 1:
        return out[0]
    return ["+"] + out


def _mk_sub(a: list, b: list) -> list:
    return _mk_add([a, _mk_neg(b)])


def _mk_mul(factors: list) -> list:
    flat: list = []
    const = Fraction(1)
    for f in factors:
        f = _as_expr(f)
        if f[0] == "*":
            for g in f[1:]:
                if _is_const(g):
                    const *= _frac(g[1])
                else:
                    flat.append(g)
        elif _is_const(f):
            const *= _frac(f[1])
        else:
            flat.append(f)
    if const == 0:
        return _C(0)
    out: list = []
    if const != 1:
        out.append(_C(const))
    out.extend(flat)
    if not out:
        return _C(1)
    if len(out) == 1:
        return out[0]
    return ["*"] + out


def _mk_div(u: list, v: list) -> list:
    if _is_const(u) and _frac(u[1]) == 0:
        return _C(0)
    if _is_const(v) and _is_const(u):
        return _C(_frac(u[1]) / _frac(v[1]))
    return ["/", u, v]


def _mk_pow(u: list, n: int) -> list:
    if n == 0:
        return _C(1)
    if n == 1:
        return u
    if _is_const(u):
        return _C(_frac(u[1]) ** n)
    return ["^", u, str(n)]


def _diff(node: Any, var: str) -> list:
    """Symbolic partial derivative d(node)/d(var) as a simplified AST."""
    node = _as_expr(node)
    tag = node[0]
    if tag == "const":
        return _C(0)
    if tag == "var":
        return _C(1) if str(node[1]) == var else _C(0)
    if tag == "+":
        return _mk_add([_diff(a, var) for a in node[1:]])
    if tag == "-":
        return _mk_sub(_diff(node[1], var), _diff(node[2], var))
    if tag == "neg":
        return _mk_neg(_diff(node[1], var))
    if tag == "*":
        args = node[1:]
        terms = []
        for i in range(len(args)):
            factors = [args[j] for j in range(len(args)) if j != i]
            factors.append(_diff(args[i], var))
            terms.append(_mk_mul(factors))
        return _mk_add(terms)
    if tag == "/":
        u, v = node[1], node[2]
        du, dv = _diff(u, var), _diff(v, var)
        num = _mk_sub(_mk_mul([du, v]), _mk_mul([u, dv]))
        return _mk_div(num, _mk_pow(v, 2))
    if tag == "^":
        u = node[1]
        n = int(node[2])
        return _mk_mul([_C(n), _mk_pow(u, n - 1), _diff(u, var)])
    if tag == "exp":
        u = node[1]
        return _mk_mul([["exp", u], _diff(u, var)])
    if tag == "log":
        u = node[1]
        return _mk_div(_diff(u, var), u)
    if tag == "sin":
        u = node[1]
        return _mk_mul([["cos", u], _diff(u, var)])
    if tag == "cos":
        u = node[1]
        return _mk_mul([_mk_neg(["sin", u]), _diff(u, var)])
    if tag == "sqrt":
        u = node[1]
        return _mk_div(_diff(u, var), _mk_mul([_C(2), ["sqrt", u]]))
    if tag == "sinh":
        u = node[1]
        return _mk_mul([["cosh", u], _diff(u, var)])
    if tag == "cosh":
        u = node[1]
        return _mk_mul([["sinh", u], _diff(u, var)])
    if tag == "tanh":
        u = node[1]
        return _mk_mul([_mk_sub(_C(1), _mk_pow(["tanh", u], 2)), _diff(u, var)])
    if tag == "atan":
        u = node[1]
        return _mk_div(_diff(u, var), _mk_add([_C(1), _mk_pow(u, 2)]))
    raise ValueError(f"cannot differentiate tag {tag!r}")


# --------------------------------------------------------------------------- #
# Box helpers + mincing enclosure.
# --------------------------------------------------------------------------- #

def _read_box(raw: Any, nvars: int) -> list:
    """Parse a box ``[[lo, hi], ...]`` into a list of ``(Fraction, Fraction)``."""
    if not isinstance(raw, list) or len(raw) != nvars:
        raise ValueError("box has wrong number of coordinates")
    box = []
    for pair in raw:
        if not (isinstance(pair, (list, tuple)) and len(pair) == 2):
            raise ValueError("box coordinate must be [lo, hi]")
        lo, hi = _frac(pair[0]), _frac(pair[1])
        if lo > hi:
            raise ValueError("box coordinate has lo > hi")
        box.append((lo, hi))
    return box


def _box_iv(iv, coord):
    lo, hi = coord
    return iv.mpf([_to_iv(iv, lo).a, _to_iv(iv, hi).b])


def _enclose(expr, variables, box, iv, subdivisions: int):
    """Validated enclosure of ``expr`` over ``box`` (optionally minced).

    With ``subdivisions == 1`` a single interval evaluation; otherwise the box is
    partitioned into a ``subdivisions``-per-axis grid and the per-cell enclosures
    are hulled (mincing beats the interval dependency problem).  Every cell and
    the hull are outward-rounded, so the result is a guaranteed superset of the
    true range of ``expr`` over ``box``.
    """
    d = len(variables)
    if subdivisions <= 1:
        env = {variables[i]: _box_iv(iv, box[i]) for i in range(d)}
        return _eval_iv(expr, env, iv)
    axes = []
    for i in range(d):
        lo, hi = box[i]
        width = hi - lo
        cells = [(lo + width * Fraction(k, subdivisions),
                  lo + width * Fraction(k + 1, subdivisions))
                 for k in range(subdivisions)]
        axes.append(cells)
    pieces = []
    for combo in itertools.product(*axes):
        env = {variables[i]: _box_iv(iv, combo[i]) for i in range(d)}
        pieces.append(_eval_iv(expr, env, iv))
    return _hull(iv, pieces)


def enclosure(expr, variables, box, *, subdivisions=DEFAULT_SUBDIVISIONS,
              dps=DEFAULT_DPS):
    """Public helper: the checker's enclosure of ``expr`` over ``box``.

    ``box`` is ``[[lo, hi], ...]`` (rationals/strings).  Returns ``(lo, hi)`` mpf.
    Producers use this to pick an *honest* branch-and-bound tree (leaves whose
    enclosures the checker will recompute really are ``>= 0``).
    """
    iv, mpf = _iv_ctx()
    iv.dps = int(dps)
    variables = [str(v) for v in variables]
    b = _read_box([[str(l), str(h)] for l, h in box], len(variables))
    E = _enclose(_norm_expr(expr), variables, b, iv, int(subdivisions))
    return mpf(E.a), mpf(E.b)


# --------------------------------------------------------------------------- #
# Node builders (ergonomic construction of a result tree).
# --------------------------------------------------------------------------- #

def _box_ser(box) -> list:
    return [[_fs(lo), _fs(hi)] for lo, hi in box]


def leaf_pass(box) -> dict:
    return {"box": _box_ser(box), "verdict": "pass"}


def node_split(box, coord: int, children: list) -> dict:
    return {"box": _box_ser(box), "verdict": "split",
            "coord": int(coord), "children": list(children)}


def node_mono(box, coord: int, direction: str, child: dict) -> dict:
    if direction not in ("inc", "dec"):
        raise ValueError("direction must be 'inc' or 'dec'")
    return {"box": _box_ser(box), "verdict": "mono",
            "coord": int(coord), "direction": direction, "child": child}


def leaf_false(box, point) -> dict:
    return {"box": _box_ser(box), "verdict": "false",
            "point": [_fs(p) for p in point]}


# --------------------------------------------------------------------------- #
# Exporter.
# --------------------------------------------------------------------------- #

def export_bnb_cert(expr, variables, domain, tree, *, strict: bool = False,
                    precision_dps: int = DEFAULT_DPS,
                    subdivisions: int = DEFAULT_SUBDIVISIONS,
                    claim: Optional[str] = None) -> dict:
    """Serialize a branch-and-bound inequality certificate to a v1 log.

    Parameters
    ----------
    expr:
        The function ``f`` as an expression AST (see the module docstring).
    variables:
        Ordered variable names; ``domain``/every box are indexed in this order.
    domain:
        The root box ``[[lo, hi], ...]``.
    tree:
        The branch-and-bound result tree (root node), built with
        :func:`leaf_pass` / :func:`node_split` / :func:`node_mono` /
        :func:`leaf_false`.
    strict:
        Certify ``f > 0`` (``True``) rather than ``f >= 0`` (``False``).
    """
    variables = [str(v) for v in variables]
    if len(set(variables)) != len(variables):
        raise ValueError("duplicate variable name")
    norm = _norm_expr(expr)
    dom = _read_box([[str(l), str(h)] for l, h in domain], len(variables))
    if int(subdivisions) < 1:
        raise ValueError("subdivisions must be >= 1")
    rel = ">" if strict else ">="
    declare = {
        "op": "declare_problem",
        "vars": variables,
        "expr": norm,
        "domain": _box_ser(dom),
        "strict": bool(strict),
        "precision_dps": int(precision_dps),
        "subdivisions": int(subdivisions),
    }
    return {
        "format": FORMAT,
        "kind": "bnb_inequality",
        "claim": claim or (
            f"f(x) {rel} 0 for all x in "
            f"[{', '.join(f'[{_fs(l)}, {_fs(h)}]' for l, h in dom)}] "
            f"(branch-and-bound over {len(variables)} variable(s))"
        ),
        "steps": [
            declare,
            {"op": "bnb_tree", "root": tree},
        ],
        "meta": {
            "producer": "cert_bnb.export_bnb_cert",
            "cakeml_target": CAKEML_TARGET,
        },
    }


# --------------------------------------------------------------------------- #
# Reference checker.
# --------------------------------------------------------------------------- #

class _Reject(Exception):
    """Raised to reject a certificate with a human-readable reason."""


class _Refuted(Exception):
    """Raised when a verified ``false`` leaf refutes the inequality."""

    def __init__(self, witness: dict, value_hi):
        super().__init__("counterexample found")
        self.witness = witness
        self.value_hi = value_hi


def _need(cond: bool, reason: str) -> None:
    if not cond:
        raise _Reject(reason)


def check(log: Any) -> dict:
    """Independently RE-VERIFY a ``bnb_inequality`` cert-log document.

    Returns ``{valid, reason, checked_nodes, kind, claim}`` (plus ``refuted`` and
    ``witness`` when a ``false`` leaf disproves the claim).  Recomputes every
    enclosure and interval derivative with ``mpmath.iv`` at fixed precision; never
    trusts the producer.  Any malformed, tampered or unsatisfied node yields
    ``valid=False`` with a ``reason`` — the sound boundary.
    """
    counter = {"nodes": 0}
    try:
        _need(isinstance(log, dict), "log is not a JSON object")
        _need(log.get("format") == FORMAT, f"unknown format: {log.get('format')!r}")
        _need(log.get("kind") in KINDS, f"unknown kind: {log.get('kind')!r}")
        steps = log.get("steps")
        _need(isinstance(steps, list) and len(steps) == 2,
              "steps must be [declare_problem, bnb_tree]")
        _need(isinstance(log.get("claim", ""), str), "claim must be a string")

        declare, tree_step = steps[0], steps[1]
        _need(isinstance(declare, dict) and declare.get("op") == "declare_problem",
              "first step must be declare_problem")
        _need(isinstance(tree_step, dict) and tree_step.get("op") == "bnb_tree",
              "second step must be bnb_tree")

        variables = declare.get("vars")
        _need(isinstance(variables, list) and variables
              and all(isinstance(v, str) for v in variables),
              "declare_problem: vars must be a non-empty list of strings")
        _need(len(set(variables)) == len(variables), "duplicate variable name")
        nvars = len(variables)

        try:
            expr = _norm_expr(declare["expr"])
        except (KeyError, ValueError, TypeError) as exc:
            raise _Reject(f"declare_problem: malformed expr ({exc})")
        strict = bool(declare.get("strict", False))

        try:
            domain = _read_box(declare["domain"], nvars)
        except (KeyError, ValueError, TypeError) as exc:
            raise _Reject(f"declare_problem: malformed domain ({exc})")

        dps = DEFAULT_DPS
        if "precision_dps" in declare:
            try:
                dps = int(declare["precision_dps"])
            except (TypeError, ValueError):
                raise _Reject("precision_dps is not an integer")
            _need(2 <= dps <= 400, "precision_dps out of the allowed range [2, 400]")
        subdivisions = int(declare.get("subdivisions", DEFAULT_SUBDIVISIONS))
        _need(1 <= subdivisions <= 1000, "subdivisions out of range [1, 1000]")
        _need(subdivisions ** nvars <= MAX_GRID_CELLS,
              "subdivisions^nvars exceeds the mincing-grid cap")

        iv, mpf = _iv_ctx()
        iv.dps = dps

        # ---- recursive tree verification (nested closures share params) ----- #

        def verify(node: Any, depth: int) -> list:
            counter["nodes"] += 1
            _need(counter["nodes"] <= MAX_NODES, "tree exceeds node cap")
            _need(depth <= MAX_DEPTH, "tree exceeds depth cap")
            _need(isinstance(node, dict), "tree node is not an object")
            box = _read_box(node.get("box"), nvars)
            verdict = node.get("verdict")

            if verdict == "pass":
                E = _enclose(expr, variables, box, iv, subdivisions)
                if strict:
                    _need(_lo(E) > 0,
                          f"unsound pass: enclosure lower bound {_lo(E)} not > 0")
                else:
                    _need(_lo(E) >= 0,
                          f"unsound pass: enclosure lower bound {_lo(E)} < 0")
                return box

            if verdict == "false":
                pt = [_frac(p) for p in node.get("point", [])]
                _need(len(pt) == nvars, "false leaf: point has wrong arity")
                for i in range(nvars):
                    _need(box[i][0] <= pt[i] <= box[i][1],
                          "false leaf: counterexample lies outside its box")
                env = {variables[i]: _box_iv(iv, (pt[i], pt[i])) for i in range(nvars)}
                R = _eval_iv(expr, env, iv)
                _need(_hi(R) < 0,
                      f"false leaf: f at the witness is not < 0 (upper bound {_hi(R)})")
                raise _Refuted({variables[i]: str(pt[i]) for i in range(nvars)}, mpf(R.b))

            if verdict == "split":
                coord = int(node.get("coord", -1))
                _need(0 <= coord < nvars, "split: coord out of range")
                children = node.get("children")
                _need(isinstance(children, list) and children,
                      "split: children must be a non-empty list")
                segs = []
                for ch in children:
                    cbox = verify(ch, depth + 1)
                    for j in range(nvars):
                        if j != coord:
                            _need(cbox[j] == box[j],
                                  "split: child differs from parent off the split axis")
                    _need(box[coord][0] <= cbox[coord][0]
                          and cbox[coord][1] <= box[coord][1],
                          "split: child extends outside the parent box")
                    segs.append(cbox[coord])
                segs.sort()
                _need(segs[0][0] == box[coord][0],
                      "split: children do not start at the parent lower bound")
                _need(segs[-1][1] == box[coord][1],
                      "split: children do not reach the parent upper bound")
                for k in range(len(segs) - 1):
                    _need(segs[k][1] == segs[k + 1][0],
                          "split: gap or overlap between children (parent not covered)")
                return box

            if verdict == "mono":
                coord = int(node.get("coord", -1))
                _need(0 <= coord < nvars, "mono: coord out of range")
                direction = node.get("direction")
                _need(direction in ("inc", "dec"), "mono: direction must be inc/dec")
                dexpr = _diff(expr, variables[coord])
                D = _enclose(dexpr, variables, box, iv, subdivisions)
                if direction == "inc":
                    _need(_lo(D) >= 0,
                          f"mono inc: derivative not >= 0 (lower bound {_lo(D)})")
                    face_val = box[coord][0]
                else:
                    _need(_hi(D) <= 0,
                          f"mono dec: derivative not <= 0 (upper bound {_hi(D)})")
                    face_val = box[coord][1]
                child = node.get("child")
                cbox = verify(child, depth + 1)
                expected = list(box)
                expected[coord] = (face_val, face_val)
                _need(cbox == expected,
                      "mono: child is not the correct face of the parent box")
                return box

            raise _Reject(f"unknown verdict {verdict!r}")

        try:
            root_box = verify(tree_step.get("root"), 0)
        except _Reject:
            raise
        except _Refuted as ref:
            return {"valid": False, "refuted": True, "witness": ref.witness,
                    "reason": (f"inequality REFUTED: counterexample {ref.witness} "
                               f"gives f <= {ref.value_hi} < 0"),
                    "checked_nodes": counter["nodes"], "kind": log.get("kind"),
                    "claim": log.get("claim")}
        except (KeyError, IndexError, TypeError, ValueError,
                ZeroDivisionError, ArithmeticError) as exc:
            raise _Reject(f"malformed tree data ({exc})")

        _need(root_box == domain, "root box does not equal the declared domain")
        return {"valid": True,
                "reason": "branch-and-bound tree re-verified with mpmath.iv; "
                          "every point of the domain lies in a discharged leaf",
                "checked_nodes": counter["nodes"], "kind": log.get("kind"),
                "claim": log.get("claim")}
    except _Reject as exc:
        return {"valid": False, "reason": str(exc),
                "checked_nodes": counter["nodes"],
                "kind": log.get("kind") if isinstance(log, dict) else None,
                "claim": log.get("claim") if isinstance(log, dict) else None}


# --------------------------------------------------------------------------- #
# Worker dispatch.
# --------------------------------------------------------------------------- #

def run(request: dict) -> dict:
    """Worker entrypoint.  ``request["op"]`` is ``export`` or ``check``.

    * ``export`` -> build a bnb_inequality cert-log document from
      ``expr``/``vars``/``domain``/``tree`` (+ optional ``strict``/``precision_dps``/
      ``subdivisions``/``claim``).  Returns ``{"log": <document>}``.
    * ``check`` -> :func:`check` on ``request["log"]``.
    """
    op = request.get("op", "check")
    if op == "check":
        return check(request["log"])
    if op == "export":
        log = export_bnb_cert(
            request["expr"],
            request["vars"],
            request["domain"],
            request["tree"],
            strict=bool(request.get("strict", False)),
            precision_dps=int(request.get("precision_dps", DEFAULT_DPS)),
            subdivisions=int(request.get("subdivisions", DEFAULT_SUBDIVISIONS)),
            claim=request.get("claim"),
        )
        return {"log": log}
    raise ValueError(f"unknown op: {op!r}")


def main() -> None:
    import json
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
