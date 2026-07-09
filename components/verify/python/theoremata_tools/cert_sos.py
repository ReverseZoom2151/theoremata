"""Sums-of-squares / Positivstellensatz **proof-log** exporter + pure checker.

This is the *nonlinear* sibling of the Farkas certificate in
:mod:`theoremata_tools.cert_log`: instead of certifying that a **linear** system
is infeasible with a nonnegative combination that sums to a contradiction, it
certifies that a **polynomial** ``p`` is nonnegative with a nonnegative
combination of *squares* (and, on an interval, of the boundary form
``(x-a)(b-x)``).  It shares the same transport-neutral proof-log envelope
(``theoremata.cert-log.v1``) but adds one new ``kind``: ``sos``.

Two problem shapes are supported, both re-checked by a self-contained checker
that trusts **nothing** from the generator:

Univariate (``#5``)
    Certify ``p(x) >= 0`` on all of ``R`` via ``p = Sum s_i^2`` (global), or on a
    closed interval ``[a, b]`` via the Positivstellensatz form
    ``p = Sum s_i^2 + (x-a)(b-x) * Sum t_j^2``.  Because ``Sum s_i^2 >= 0``,
    ``(x-a)(b-x) >= 0`` on ``[a, b]`` and ``Sum t_j^2 >= 0``, the identity proves
    nonnegativity on the interval.  The GENERATOR uses sympy squarefree
    decomposition plus an **injected** numeric ``root_finder`` seam whose roots
    are snapped to EXACT rationals with an eps-cushion (so tests are
    deterministic); the emitted certificate and the checker are exact rational.

Multivariate (``#8``)
    Given a claimed Gram decomposition ``p = z^T Q z`` (``z`` a monomial vector,
    ``Q`` a claimed-PSD rational matrix), :func:`check_sos_gram` re-expands the
    identity exactly and tests ``Q`` for positive semidefiniteness with an exact
    rational LDL^T / completing-the-square congruence.  On failure it is a
    *falsify-before-prove* checker: it emits a counterexample point ``u`` with
    ``p(u) < 0`` when one exists.  The multivariate GENERATOR (an SDP) is out of
    scope and left as a documented stub (:data:`generate_multivariate`).

Soundness boundary
------------------
:func:`check` is the sound boundary.  It re-derives the SOS identity with exact
symbolic/rational arithmetic (sympy over ``QQ``) and re-tests PSD with a pure
rational congruence transform; it never trusts the generator's output.  A
tampered decomposition (wrong squares, or a non-PSD Gram) is rejected, and for a
genuinely negative ``p`` the Gram checker returns a witnessing point.

Worker dispatch key: ``cert_sos`` (see :func:`run`).
"""
from __future__ import annotations

import math
from fractions import Fraction
from itertools import product
from typing import Any, Callable, Iterable, Optional

import sympy
from sympy import I, Poly, Rational, Symbol, div, expand, im, re, sympify

FORMAT = "theoremata.cert-log.v1"
KIND = "sos"
KINDS = ("sos",)


# --------------------------------------------------------------------------- #
# Exact helpers.
# --------------------------------------------------------------------------- #

def _R(x: Any) -> Rational:
    """Parse a value to an exact sympy ``Rational`` (via ``str`` to avoid drift)."""
    if isinstance(x, Rational):
        return x
    if isinstance(x, bool):
        raise TypeError("boolean where a rational was expected")
    if isinstance(x, Fraction):
        return Rational(x.numerator, x.denominator)
    if isinstance(x, int):
        return Rational(x)
    return Rational(str(x))


def _sym(name: str) -> Symbol:
    return Symbol(str(name))


class _GenFail(Exception):
    """Raised when the (best-effort, gated) generator cannot certify exactly."""


# --------------------------------------------------------------------------- #
# Polynomial (de)serialization to plain JSON (exact rationals as strings).
# --------------------------------------------------------------------------- #

def _expr_to_polydict(expr: Any, gens: list[Symbol]) -> dict:
    """Serialize a sympy expression as ``{"vars": [...], "terms": [[exps, coeff]]}``."""
    P = Poly(sympify(expr), *gens, domain="QQ")
    terms = [[list(int(e) for e in monom), str(coeff)] for monom, coeff in P.terms()]
    return {"vars": [str(g) for g in gens], "terms": terms}


def _polydict_to_expr(d: Any) -> tuple[Any, list[Symbol]]:
    """Rebuild ``(expr, gens)`` from a poly dict; raises on malformed data."""
    if not isinstance(d, dict) or "vars" not in d or "terms" not in d:
        raise ValueError("malformed polynomial dict")
    gens = [_sym(v) for v in d["vars"]]
    expr = Rational(0)
    for entry in d["terms"]:
        exps, coeff = entry
        exps = [int(e) for e in exps]
        if len(exps) != len(gens) or any(e < 0 for e in exps):
            raise ValueError("malformed monomial")
        term = _R(coeff)
        for g, e in zip(gens, exps):
            term *= g ** e
        expr += term
    return expr, gens


# --------------------------------------------------------------------------- #
# Root-finder seam + rational snapping (used only by the generator).
# --------------------------------------------------------------------------- #

def _default_root_finder(coeffs: list) -> list[complex]:
    """Default numeric root finder: sympy ``nroots`` on the given coefficients.

    ``coeffs`` are highest-degree-first (as from ``Poly.all_coeffs()``).  The
    result is inexact; the generator snaps it to exact rationals.  Injected in
    tests for determinism.
    """
    z = Symbol("_z")
    return [complex(r) for r in Poly(list(coeffs), z).nroots(n=40)]


def _snap(z: complex, eps: float, max_den: int) -> tuple[Rational, Rational]:
    """Snap a numeric root to an exact Gaussian rational within ``eps``."""
    fr = Fraction(z.real).limit_denominator(max_den)
    real = Rational(fr.numerator, fr.denominator)
    if abs(z.imag) < eps:
        imag = Rational(0)
    else:
        fi = Fraction(z.imag).limit_denominator(max_den)
        imag = Rational(fi.numerator, fi.denominator)
    return real, imag


# --------------------------------------------------------------------------- #
# Positive-rational as a sum of rational squares (for a non-square leading coeff).
# --------------------------------------------------------------------------- #

def _rational_sqrt(q: Rational) -> Optional[Rational]:
    p, r = int(q.p), int(q.q)
    sp, sr = math.isqrt(p), math.isqrt(r)
    if sp * sp == p and sr * sr == r:
        return Rational(sp, sr)
    return None


def _four_squares(N: int) -> tuple[int, int, int, int]:
    """Integers ``(a, b, c, d)`` with ``a^2+b^2+c^2+d^2 == N`` (Lagrange)."""
    if N == 0:
        return (0, 0, 0, 0)
    a = 0
    while a * a <= N:
        ra = N - a * a
        b = 0
        while b * b <= ra:
            rb = ra - b * b
            c = 0
            while c * c <= rb:
                rc = rb - c * c
                d = math.isqrt(rc)
                if d * d == rc:
                    return (a, b, c, d)
                c += 1
            b += 1
        a += 1
    raise _GenFail("four-square decomposition failed")  # pragma: no cover


def _sum_of_squares_rational(lc: Rational) -> list[Rational]:
    """Nonzero rationals whose squares sum to positive ``lc``."""
    if lc <= 0:
        raise _GenFail("leading coefficient must be positive")
    s = _rational_sqrt(lc)
    if s is not None:
        return [s]
    num, den = int(lc.p), int(lc.q)
    a, b, c, d = _four_squares(num * den)  # lc = (num*den)/den^2
    return [Rational(v, den) for v in (a, b, c, d) if v != 0]


# --------------------------------------------------------------------------- #
# Univariate GENERATOR: exact SOS from snapped roots.
# --------------------------------------------------------------------------- #

def _global_sos(P: Poly, x: Symbol,
                root_finder: Callable, eps: float, max_den: int) -> list:
    """Return squares ``[s_i]`` with ``Sum s_i^2 == P`` for a globally-nonneg ``P``.

    Raises :class:`_GenFail` if ``P`` is not exactly a sum of rational-coefficient
    squares by this construction (e.g. an odd-multiplicity real root, or an
    irrational root that does not snap back to ``P``).
    """
    if P.is_zero:
        return []
    lc = Rational(P.LC())
    if lc <= 0:
        raise _GenFail("polynomial is not globally nonnegative (leading coeff <= 0)")

    real_roots: list[tuple[Rational, int]] = []
    pos_complex: list[tuple[Rational, Rational, int]] = []
    for fac, mult in P.sqf_list()[1]:
        if fac.degree() == 0:
            continue
        roots = list(root_finder(fac.all_coeffs()))
        snapped = [_snap(complex(z), eps, max_den) for z in roots]
        for ar, ai in snapped:
            if ai == 0:
                real_roots.append((ar, mult))
            elif ai > 0:
                pos_complex.append((ar, ai, mult))

    for _r, m in real_roots:
        if m % 2 != 0:
            raise _GenFail("odd-multiplicity real root: not globally nonnegative")

    # g = prod (x-r)^(m/2) * prod (x-(a+bi))^m ; then |g|^2 = P/lc.
    g = Rational(1)
    for r, m in real_roots:
        g *= (x - r) ** (m // 2)
    for a, b, m in pos_complex:
        g *= (x - (a + b * I)) ** m
    Pg = Poly(expand(g), x)

    re_part = Rational(0)
    im_part = Rational(0)
    for (deg,), coeff in Pg.terms():
        re_part += re(coeff) * x ** deg
        im_part += im(coeff) * x ** deg
    re_part = expand(re_part)
    im_part = expand(im_part)

    squares: list = []
    for c in _sum_of_squares_rational(lc):
        squares.append(expand(c * re_part))
        if im_part != 0:
            squares.append(expand(c * im_part))

    check = expand(sum(s ** 2 for s in squares) - P.as_expr())
    if check != 0:
        raise _GenFail("reconstructed SOS does not match the polynomial exactly")
    return squares


def generate_univariate_sos(p: Any, x: Symbol, *, interval=None,
                            root_finder: Optional[Callable] = None,
                            eps: float = 1e-9, max_den: int = 10 ** 6) -> dict:
    """Best-effort EXACT univariate SOS / Positivstellensatz generator.

    Returns ``{"squares": [...], "multiplier_squares": [...], "interval": (a,b)|None}``
    with sympy expressions.  For an interval it first tries a global SOS
    (``multiplier_squares == []``); failing that, it peels the boundary form
    ``(x-a)(b-x)`` when it divides ``p`` exactly with a globally-nonneg quotient.
    Raises :class:`_GenFail` when no exact rational certificate is found by these
    constructions (the honest generation boundary; the checker is exact).
    """
    rf = root_finder or _default_root_finder
    x = _sym(str(x)) if not isinstance(x, Symbol) else x
    P = Poly(sympify(p), x, domain="QQ")

    if interval is None:
        return {"squares": _global_sos(P, x, rf, eps, max_den),
                "multiplier_squares": [], "interval": None}

    a, b = _R(interval[0]), _R(interval[1])
    try:  # Pattern A: p is globally nonnegative on all of R.
        squares = _global_sos(P, x, rf, eps, max_den)
        return {"squares": squares, "multiplier_squares": [], "interval": (a, b)}
    except _GenFail:
        pass
    # Pattern B: (x-a)(b-x) | p exactly and quotient is globally SOS.
    mult = Poly((x - a) * (b - x), x, domain="QQ")
    quo, rem = div(P, mult)
    if rem.is_zero:
        tsq = _global_sos(quo, x, rf, eps, max_den)
        return {"squares": [], "multiplier_squares": tsq, "interval": (a, b)}
    raise _GenFail("no exact interval SOS certificate found (needs SDP)")


# The multivariate SOS generator is an SDP problem and is intentionally NOT
# implemented here: a numerically-solved Gram matrix must be rounded/projected to
# an EXACT rational PSD matrix (a nontrivial rationalization step).  It needs an
# optional solver extra (e.g. CSDP, or cvxpy + SCS) plus exact PSD rounding.  The
# CHECKER below is pure and complete on its own, so a Gram matrix produced by any
# external solver can be validated without trusting the solver.
generate_multivariate: Optional[Callable] = None


# --------------------------------------------------------------------------- #
# Exact rational PSD test with a falsifying direction (completing the square).
# --------------------------------------------------------------------------- #

def _psd_witness(Q: list[list[Rational]]) -> tuple[bool, Optional[list[Rational]]]:
    """Exact PSD test for a symmetric rational matrix via a congruence transform.

    Returns ``(True, None)`` if ``Q`` is PSD, else ``(False, v)`` with a rational
    vector ``v`` such that ``v^T Q v < 0``.  Pure rational arithmetic: reduces the
    quadratic form by completing the square, tracking the Q-orthogonal directions
    in the ORIGINAL basis (columns of ``B``) so a bad pivot yields a witness.
    """
    n = len(Q)
    M = [[_R(Q[i][j]) for j in range(n)] for i in range(n)]
    B = [[Rational(1) if i == j else Rational(0) for j in range(n)] for i in range(n)]
    remaining = list(range(n))

    while remaining:
        neg = [p for p in remaining if M[p][p] < 0]
        if neg:
            p = neg[0]
            return False, [B[i][p] for i in range(n)]
        pos = [p for p in remaining if M[p][p] > 0]
        if pos:
            p = pos[0]
            col = [M[i][p] for i in range(n)]  # snapshot p-th row/col (symmetric)
            piv = M[p][p]
            for j in remaining:
                if j == p:
                    continue
                f = col[j] / piv
                for i in range(n):
                    B[i][j] -= f * B[i][p]
            for i in remaining:
                if i == p:
                    continue
                for j in remaining:
                    if j == p:
                        continue
                    M[i][j] -= col[i] * col[j] / piv
            remaining.remove(p)
            continue
        # All remaining diagonal entries are zero: a nonzero off-diagonal among
        # them is an indefinite 2x2 block [[0,b],[b,0]] -> negative direction.
        for ai in range(len(remaining)):
            for bi in range(ai + 1, len(remaining)):
                i, j = remaining[ai], remaining[bi]
                if M[i][j] != 0:
                    t = -Rational(1) / (2 * M[i][j])
                    return False, [B[k][i] + t * B[k][j] for k in range(n)]
        return True, None  # remaining block is entirely zero -> PSD
    return True, None


# --------------------------------------------------------------------------- #
# Multivariate Gram checker (identity + PSD, falsify-before-prove).
# --------------------------------------------------------------------------- #

def _monomial_expr(gens: list[Symbol], exps: list[int]) -> Any:
    term = Rational(1)
    for g, e in zip(gens, exps):
        term *= g ** int(e)
    return term


def _find_negative_point(expr: Any, gens: list[Symbol], monomials: list,
                         direction: Optional[list[Rational]]) -> Optional[dict]:
    """Find an exact rational point ``u`` with ``expr(u) < 0``; else ``None``.

    First tries the PSD-failure ``direction`` read off the degree-1 monomials,
    then a small exact rational grid.  Evaluation is exact (sympy over ``QQ``).
    """
    candidates: list[dict] = []
    if direction is not None:
        unit = {}
        for idx, mono in enumerate(monomials):
            if sum(mono) == 1:
                unit[list(mono).index(1)] = idx
        pt = {g: (direction[unit[k]] if k in unit else Rational(0))
              for k, g in enumerate(gens)}
        candidates.append(pt)
    for combo in product((-2, -1, 0, 1, 2), repeat=len(gens)):
        candidates.append({g: Rational(c) for g, c in zip(gens, combo)})

    for pt in candidates:
        val = sympify(expr).xreplace(pt)
        if val.is_number and Rational(val) < 0:
            return {"point": {str(g): str(v) for g, v in pt.items()},
                    "value": str(Rational(val))}
    return None


def check_sos_gram(p: Any, gens: Iterable[Symbol], monomials: list,
                   Q: list) -> dict:
    """Verify a Gram SOS decomposition ``p == z^T Q z`` with ``Q`` PSD.

    Returns ``{"valid": bool, "reason": str, "witness": {...}|None}``.  Rejects a
    Gram that does not reproduce ``p`` exactly ("identity mismatch") or that is
    not PSD; in the not-PSD case it attaches a counterexample point ``u`` with
    ``p(u) < 0`` when one exists (native falsify-before-prove).
    """
    gens = [(_sym(str(g)) if not isinstance(g, Symbol) else g) for g in gens]
    n = len(monomials)
    if not (isinstance(Q, list) and len(Q) == n and all(len(row) == n for row in Q)):
        return {"valid": False, "reason": "Gram matrix shape != #monomials",
                "witness": None}
    Qr = [[_R(Q[i][j]) for j in range(n)] for i in range(n)]
    for i in range(n):
        for j in range(n):
            if Qr[i][j] != Qr[j][i]:
                return {"valid": False, "reason": "Gram matrix is not symmetric",
                        "witness": None}

    z = [_monomial_expr(gens, list(m)) for m in monomials]
    quad = sum(Qr[i][j] * z[i] * z[j] for i in range(n) for j in range(n))
    p_expr = sympify(p)
    if expand(p_expr - quad) != 0:
        return {"valid": False,
                "reason": "identity mismatch: p != z^T Q z", "witness": None}

    psd, direction = _psd_witness(Qr)
    if psd:
        return {"valid": True, "reason": "p = z^T Q z with Q PSD (exact)",
                "witness": None}
    witness = _find_negative_point(p_expr, gens, [list(m) for m in monomials],
                                   direction)
    return {"valid": False, "reason": "Gram matrix is not PSD", "witness": witness}


# --------------------------------------------------------------------------- #
# Exporters -> theoremata.cert-log.v1, kind "sos".
# --------------------------------------------------------------------------- #

def export_univariate_sos_cert(p: Any, *, x: Any, interval=None,
                               squares=None, multiplier_squares=None,
                               root_finder: Optional[Callable] = None,
                               claim: Optional[str] = None) -> dict:
    """Serialize a univariate SOS / interval-Positivstellensatz certificate.

    If ``squares`` is not supplied, calls :func:`generate_univariate_sos`.  Emits
    steps carrying ``p``, the ``interval`` and the SOS terms, closed by
    ``assert_sos_identity``.
    """
    x = _sym(str(x)) if not isinstance(x, Symbol) else x
    if squares is None:
        gen = generate_univariate_sos(p, x, interval=interval,
                                      root_finder=root_finder)
        squares = gen["squares"]
        multiplier_squares = gen["multiplier_squares"]
        interval = gen["interval"]
    multiplier_squares = multiplier_squares or []

    iv = None
    if interval is not None:
        iv = [str(_R(interval[0])), str(_R(interval[1]))]

    steps = [
        {"op": "sos_problem", "vars": [str(x)],
         "poly": _expr_to_polydict(p, [x]), "interval": iv,
         "note": "p = Sum s_i^2 + (x-a)(b-x) Sum t_j^2 proves p>=0 on [a,b]"},
        {"op": "sos_terms",
         "squares": [_expr_to_polydict(s, [x]) for s in squares],
         "multiplier_squares": [_expr_to_polydict(t, [x]) for t in multiplier_squares]},
        {"op": "assert_sos_identity"},
    ]
    return {
        "format": FORMAT, "kind": KIND,
        "claim": claim or ("p(x) >= 0 on [a,b]" if iv else "p(x) >= 0 on R"),
        "steps": steps,
        "meta": {"producer": "cert_sos.univariate", "mode": "univariate"},
    }


def export_gram_sos_cert(p: Any, *, gens: Iterable[Symbol], monomials: list,
                         Q: list, claim: Optional[str] = None) -> dict:
    """Serialize a multivariate Gram SOS certificate ``p = z^T Q z``."""
    gens = [(_sym(str(g)) if not isinstance(g, Symbol) else g) for g in gens]
    steps = [
        {"op": "sos_gram_problem", "vars": [str(g) for g in gens],
         "poly": _expr_to_polydict(p, gens),
         "monomials": [[int(e) for e in m] for m in monomials],
         "note": "z_i = prod vars^monomials[i]; claim p = z^T Q z with Q PSD"},
        {"op": "gram_matrix", "Q": [[str(_R(v)) for v in row] for row in Q]},
        {"op": "assert_gram_identity"},
        {"op": "assert_gram_psd"},
    ]
    return {
        "format": FORMAT, "kind": KIND,
        "claim": claim or "p >= 0 (sum of squares via PSD Gram matrix)",
        "steps": steps,
        "meta": {"producer": "cert_sos.gram", "mode": "gram"},
    }


def export_sos_cert(p: Any, *, x: Any = None, gens: Iterable[Symbol] = None,
                    interval=None, squares=None, multiplier_squares=None,
                    monomials=None, gram=None,
                    root_finder: Optional[Callable] = None,
                    claim: Optional[str] = None) -> dict:
    """Dispatch to the univariate or Gram exporter based on the arguments."""
    if monomials is not None or gram is not None:
        return export_gram_sos_cert(p, gens=gens or [], monomials=monomials or [],
                                    Q=gram or [], claim=claim)
    return export_univariate_sos_cert(p, x=x, interval=interval, squares=squares,
                                      multiplier_squares=multiplier_squares,
                                      root_finder=root_finder, claim=claim)


# --------------------------------------------------------------------------- #
# REFERENCE CHECKER.
# --------------------------------------------------------------------------- #

class _Reject(Exception):
    """Raised to reject a certificate with a human-readable reason."""


def _need(cond: bool, reason: str) -> None:
    if not cond:
        raise _Reject(reason)


def _check_univariate(steps: list, log: dict) -> dict:
    _need(len(steps) >= 3, "univariate sos: need problem, terms, and identity steps")
    prob, terms, concl = steps[0], steps[1], steps[2]
    _need(prob.get("op") == "sos_problem", "first step must be sos_problem")
    _need(terms.get("op") == "sos_terms", "second step must be sos_terms")
    _need(concl.get("op") == "assert_sos_identity",
          "third step must be assert_sos_identity")

    p_expr, gens = _polydict_to_expr(prob["poly"])
    _need(len(gens) == 1, "univariate sos: expected exactly one variable")
    x = gens[0]

    squares = [_polydict_to_expr(s)[0] for s in terms["squares"]]
    msq = [_polydict_to_expr(t)[0] for t in terms.get("multiplier_squares", [])]

    iv = prob.get("interval")
    if iv is None:
        _need(not msq, "global SOS must not carry multiplier squares (no interval)")
        multiplier = Rational(0)
    else:
        _need(isinstance(iv, list) and len(iv) == 2, "interval must be [a, b]")
        a, b = _R(iv[0]), _R(iv[1])
        _need(a <= b, "interval must have a <= b")
        multiplier = (x - a) * (b - x)

    rhs = sum(s ** 2 for s in squares) + multiplier * sum(t ** 2 for t in msq)
    _need(expand(p_expr - rhs) == 0,
          "SOS identity does not hold: p != Sum s^2 + (x-a)(b-x) Sum t^2")
    return {"valid": True,
            "reason": "SOS identity re-verified exactly", "checked_steps": 3,
            "kind": KIND, "claim": log.get("claim"), "witness": None}


def _check_gram(steps: list, log: dict) -> dict:
    _need(len(steps) >= 4, "gram sos: need problem, matrix, identity, psd steps")
    prob, gram_step, id_step, psd_step = steps[0], steps[1], steps[2], steps[3]
    _need(prob.get("op") == "sos_gram_problem", "first step must be sos_gram_problem")
    _need(gram_step.get("op") == "gram_matrix", "second step must be gram_matrix")
    _need(id_step.get("op") == "assert_gram_identity",
          "third step must be assert_gram_identity")
    _need(psd_step.get("op") == "assert_gram_psd",
          "fourth step must be assert_gram_psd")

    p_expr, gens = _polydict_to_expr(prob["poly"])
    monomials = prob["monomials"]
    _need(isinstance(monomials, list) and monomials, "monomials must be a list")
    for m in monomials:
        _need(len(m) == len(gens), "monomial arity != #vars")
    Q = gram_step["Q"]

    res = check_sos_gram(p_expr, gens, monomials, Q)
    return {"valid": res["valid"], "reason": res["reason"], "checked_steps": 4,
            "kind": KIND, "claim": log.get("claim"), "witness": res.get("witness")}


def check(log: Any) -> dict:
    """Independently RE-VERIFY an SOS cert-log document (the sound boundary).

    Returns ``{valid, reason, checked_steps, kind, claim, witness}``.  Rebuilds
    the SOS identity with exact arithmetic and re-tests PSD purely; a tampered
    decomposition (wrong squares or non-PSD Gram) yields ``valid=False``, with a
    ``p(u) < 0`` witness for a non-PSD Gram when one exists.
    """
    try:
        _need(isinstance(log, dict), "log is not a JSON object")
        _need(log.get("format") == FORMAT, f"unknown format: {log.get('format')!r}")
        _need(log.get("kind") == KIND, f"unknown kind: {log.get('kind')!r}")
        steps = log.get("steps")
        _need(isinstance(steps, list) and steps, "steps must be a non-empty list")
        _need(isinstance(log.get("claim", ""), str), "claim must be a string")

        op0 = steps[0].get("op") if isinstance(steps[0], dict) else None
        if op0 == "sos_problem":
            return _check_univariate(steps, log)
        if op0 == "sos_gram_problem":
            return _check_gram(steps, log)
        raise _Reject(f"unknown leading op {op0!r} for kind {KIND!r}")
    except _Reject as exc:
        return {"valid": False, "reason": str(exc), "checked_steps": 0,
                "kind": log.get("kind") if isinstance(log, dict) else None,
                "claim": log.get("claim") if isinstance(log, dict) else None,
                "witness": None}
    except (KeyError, IndexError, TypeError, ValueError, ZeroDivisionError) as exc:
        return {"valid": False, "reason": f"malformed cert ({exc})",
                "checked_steps": 0,
                "kind": log.get("kind") if isinstance(log, dict) else None,
                "claim": log.get("claim") if isinstance(log, dict) else None,
                "witness": None}


# --------------------------------------------------------------------------- #
# Worker dispatch.
# --------------------------------------------------------------------------- #

def run(request: dict) -> dict:
    """Worker entrypoint.  ``request["op"]`` is ``export`` or ``check``.

    * ``export`` -> build a cert-log document.  ``mode`` (or the presence of
      ``monomials``/``gram``) selects univariate vs Gram.  Univariate needs
      ``p`` and ``x`` (+ optional ``interval``); Gram needs ``p``, ``gens``,
      ``monomials`` and ``gram``.  Returns ``{"log": <document>}``.
    * ``check`` -> :func:`check` on ``request["log"]``.
    """
    op = request.get("op", "check")
    if op == "check":
        return check(request["log"])
    if op == "export":
        mode = request.get("mode")
        if mode == "gram" or request.get("monomials") is not None:
            log = export_gram_sos_cert(request["p"], gens=request["gens"],
                                       monomials=request["monomials"],
                                       Q=request["gram"], claim=request.get("claim"))
        else:
            log = export_univariate_sos_cert(
                request["p"], x=request["x"], interval=request.get("interval"),
                squares=request.get("squares"),
                multiplier_squares=request.get("multiplier_squares"),
                claim=request.get("claim"))
        return {"log": log}
    raise ValueError(f"unknown op: {op!r}")
