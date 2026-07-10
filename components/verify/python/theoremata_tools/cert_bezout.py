"""Bezout / GCD-cofactor **certificate** exporter + self-contained **reference
checker**, in the ``theoremata.cert-log.v1`` spirit (mirrors ``cert_pratt.py``).

A *Bezout certificate* proves ``g = gcd(a, b)`` by exhibiting cofactors
``u, v`` with

    u * a + v * b == g

together with the facts that ``g`` is a common divisor (``g | a`` and
``g | b``) and that ``g`` is **normalized** (positive integer, or a monic
polynomial).  These three facts *prove* ``g = gcd(a, b)``:

* any common divisor ``d`` of ``a`` and ``b`` divides the integer combination
  ``u*a + v*b = g``, hence ``d | g`` (so ``g`` is the *greatest* common
  divisor, up to units), and
* ``g`` itself is a common divisor, and
* normalization (``g > 0`` / monic) pins down the canonical representative
  among the unit multiples.

A ``divides`` sub-kind certifies the simpler relation ``a | b`` via an exact
quotient ``q`` with ``a * q == b``.

Two halves, mirroring ``cert_pratt.py``:

* **Generator** (``export_bezout_cert`` / ``export_divides_cert``) may use the
  extended Euclidean algorithm (integers) or :mod:`sympy` (polynomial gcd and
  cofactors).  It is the *trusted producer*.
* **Checker** (``check``) is the *sound boundary*: it re-verifies the Bezout
  identity, both divisibilities and the normalization with EXACT arithmetic
  (Python ``int`` / :class:`fractions.Fraction`), importing **nothing** from
  the generator and using **no** sympy.  Its own univariate-polynomial
  arithmetic (add/mul/exact division) re-checks the polynomial domain.  A
  tampered cofactor, a claimed ``g`` that does not divide ``a`` or ``b``, a
  non-normalized ``g`` or a bad divides-quotient is REJECTED.

Everything is deterministic and offline.  All inputs are treated as UNTRUSTED
DATA: the checker validates structure/types defensively and turns any
malformed input into ``valid=False`` rather than trusting it.

Worker dispatch key: ``cert_bezout`` (see :func:`run`).
"""
from __future__ import annotations

import json
from fractions import Fraction
from typing import Any, Optional

# Same transport-neutral proof-log envelope as cert_log.py / cert_pratt.py.
FORMAT = "theoremata.cert-log.v1"

# This module's proof-log KIND (independent of cert_log.py's KINDS tuple).
KINDS = ("bezout",)

_DOMAINS = ("int", "poly")


# --------------------------------------------------------------------------- #
# Exact helpers (self-contained; no producer imports).
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


# -- Self-contained univariate polynomial arithmetic over Q ------------------ #
# Polynomials are coefficient lists LOW-to-HIGH degree, i.e. ``[c0, c1, c2]``
# means ``c0 + c1*x + c2*x**2``.  Normalized = trailing (high-degree) zeros
# stripped; the zero polynomial is ``[]``.

def _p_norm(coeffs: list) -> list:
    c = [Fraction(x) for x in coeffs]
    while c and c[-1] == 0:
        c.pop()
    return c


def _p_is_zero(c: list) -> bool:
    return not _p_norm(c)


def _p_eq(a: list, b: list) -> bool:
    return _p_norm(a) == _p_norm(b)


def _p_add(a: list, b: list) -> list:
    n = max(len(a), len(b))
    out = []
    for i in range(n):
        ai = a[i] if i < len(a) else Fraction(0)
        bi = b[i] if i < len(b) else Fraction(0)
        out.append(Fraction(ai) + Fraction(bi))
    return _p_norm(out)


def _p_mul(a: list, b: list) -> list:
    a = _p_norm(a)
    b = _p_norm(b)
    if not a or not b:
        return []
    out = [Fraction(0)] * (len(a) + len(b) - 1)
    for i, ai in enumerate(a):
        for j, bj in enumerate(b):
            out[i + j] += ai * bj
    return _p_norm(out)


def _p_divmod(num: list, den: list) -> tuple[list, list]:
    """Exact quotient/remainder of ``num`` by ``den`` over Q; ``den`` nonzero."""
    den = _p_norm(den)
    if not den:
        raise ZeroDivisionError("polynomial division by zero")
    r = _p_norm(num)
    dd = len(den) - 1
    lc = den[-1]
    quotient: dict[int, Fraction] = {}
    while r and len(r) - 1 >= dd:
        deg = len(r) - 1
        coeff = r[-1] / lc
        shift = deg - dd
        quotient[shift] = coeff
        newr = list(r)
        for i, dc in enumerate(den):
            newr[shift + i] -= coeff * dc
        r = _p_norm(newr)
    qmax = max(quotient) if quotient else -1
    qlist = [quotient.get(i, Fraction(0)) for i in range(qmax + 1)]
    return _p_norm(qlist), r


def _p_divides(divisor: list, dividend: list) -> bool:
    """True iff ``divisor`` exactly divides ``dividend`` over Q."""
    if _p_is_zero(divisor):
        return _p_is_zero(dividend)  # 0 | x iff x == 0
    _q, rem = _p_divmod(dividend, divisor)
    return _p_is_zero(rem)


def _p_is_monic(c: list) -> bool:
    c = _p_norm(c)
    return bool(c) and c[-1] == Fraction(1)


# --------------------------------------------------------------------------- #
# Generator (trusted producer).
# --------------------------------------------------------------------------- #

def egcd(a: int, b: int) -> tuple[int, int, int]:
    """Extended Euclid: return ``(g, u, v)`` with ``u*a + v*b == g``, ``g >= 0``.

    ``g == gcd(a, b)`` normalized to be nonnegative (the canonical positive
    representative for a non-both-zero input).
    """
    old_r, r = int(a), int(b)
    old_s, s = 1, 0
    old_t, t = 0, 1
    while r != 0:
        q = old_r // r
        old_r, r = r, old_r - q * r
        old_s, s = s, old_s - q * s
        old_t, t = t, old_t - q * t
    g, u, v = old_r, old_s, old_t
    if g < 0:
        g, u, v = -g, -u, -v
    return g, u, v


def _poly_bezout(a: list, b: list) -> tuple[list, list, list]:
    """Monic polynomial gcd + cofactors via sympy: ``u*a + v*b == g``.

    Inputs/outputs are coefficient lists LOW-to-HIGH over Q.  Generator-only:
    the checker never imports sympy.
    """
    import sympy  # generator-only import; the checker never needs it

    x = sympy.symbols("x")

    def _to_poly(coeffs: list):
        c = _p_norm(coeffs)
        if not c:
            return sympy.Poly(0, x, domain="QQ")
        # sympy.Poly wants coeffs HIGH-to-LOW.
        return sympy.Poly([sympy.Rational(str(v)) for v in reversed(c)], x,
                          domain="QQ")

    def _from_poly(p) -> list:
        coeffs = [Fraction(str(v)) for v in reversed(p.all_coeffs())]
        return _p_norm(coeffs)

    fa, fb = _to_poly(a), _to_poly(b)
    if fa.is_zero and fb.is_zero:
        raise ValueError("gcd(0, 0) is undefined for a Bezout certificate")
    u, v, g = fa.gcdex(fb)  # u*fa + v*fb == g, g monic gcd
    return _from_poly(g), _from_poly(u), _from_poly(v)


def _envelope(steps: list, claim: str, meta: dict) -> dict:
    return {"format": FORMAT, "kind": "bezout", "claim": claim,
            "steps": steps, "meta": meta}


def export_bezout_cert(a: Any, b: Any, *, domain: str = "int",
                       claim: Optional[str] = None) -> dict:
    """Serialize a Bezout / GCD-cofactor certificate for ``(a, b)``.

    ``domain == "int"`` -> integers via :func:`egcd`.  ``domain == "poly"`` ->
    univariate polynomials over Q (coefficient lists LOW-to-HIGH) via sympy.
    The single ``bezout_relation`` step carries ``a, b, g, u, v``; a following
    ``assert_gcd`` step is the conclusion the checker re-verifies.
    """
    if domain == "int":
        ai, bi = int(a), int(b)
        g, u, v = egcd(ai, bi)
        step = {"op": "bezout_relation", "domain": "int",
                "a": ai, "b": bi, "g": g, "u": u, "v": v}
        default = f"gcd({ai}, {bi}) = {g} = ({u})*{ai} + ({v})*{bi}"
    elif domain == "poly":
        g, u, v = _poly_bezout(list(a), list(b))
        step = {"op": "bezout_relation", "domain": "poly",
                "a": [_fs(_frac(c)) for c in _p_norm(list(a))],
                "b": [_fs(_frac(c)) for c in _p_norm(list(b))],
                "g": [_fs(c) for c in g],
                "u": [_fs(c) for c in u],
                "v": [_fs(c) for c in v]}
        default = "u*a + v*b = g = gcd(a, b) (monic), univariate over Q"
    else:
        raise ValueError(f"unknown domain: {domain!r}")

    return _envelope(
        [step, {"op": "assert_gcd"}],
        claim or default,
        {"producer": "cert_bezout.export_bezout_cert", "domain": domain},
    )


def export_divides_cert(a: Any, b: Any, *, domain: str = "int",
                        claim: Optional[str] = None) -> dict:
    """Serialize a ``divides`` certificate: ``a | b`` via a quotient ``q``.

    Raises ``ValueError`` if ``a`` does NOT divide ``b`` (a non-divisor is
    rejected at export).  The ``divides_relation`` step carries ``a, b, q`` with
    ``a * q == b``; ``assert_divides`` is the conclusion.
    """
    if domain == "int":
        ai, bi = int(a), int(b)
        if ai == 0:
            if bi != 0:
                raise ValueError(f"0 does not divide {bi}")
            q = 0
        else:
            if bi % ai != 0:
                raise ValueError(f"{ai} does not divide {bi}")
            q = bi // ai
        step = {"op": "divides_relation", "domain": "int",
                "a": ai, "b": bi, "q": q}
        default = f"{ai} | {bi} (quotient {q}: {ai}*{q} = {bi})"
    elif domain == "poly":
        ap, bp = _p_norm(list(a)), _p_norm(list(b))
        if _p_is_zero(ap):
            if not _p_is_zero(bp):
                raise ValueError("zero polynomial does not divide a nonzero one")
            q = []
        else:
            q, rem = _p_divmod(bp, ap)
            if not _p_is_zero(rem):
                raise ValueError("a does not divide b (nonzero remainder)")
        step = {"op": "divides_relation", "domain": "poly",
                "a": [_fs(c) for c in ap], "b": [_fs(c) for c in bp],
                "q": [_fs(c) for c in q]}
        default = "a | b (univariate over Q): a*q = b"
    else:
        raise ValueError(f"unknown domain: {domain!r}")

    return _envelope(
        [step, {"op": "assert_divides"}],
        claim or default,
        {"producer": "cert_bezout.export_divides_cert", "domain": domain},
    )


# --------------------------------------------------------------------------- #
# REFERENCE CHECKER (exact; independent of the generator; no sympy).
# --------------------------------------------------------------------------- #

class _Reject(Exception):
    """Raised to reject a certificate with a human-readable reason."""


def _need(cond: bool, reason: str) -> None:
    if not cond:
        raise _Reject(reason)


def _as_int(x: Any, what: str) -> int:
    _need(isinstance(x, int) and not isinstance(x, bool), f"{what} must be an int")
    return x


def _as_poly(x: Any, what: str) -> list:
    _need(isinstance(x, list), f"{what} must be a coefficient list")
    try:
        return _p_norm([_frac(c) for c in x])
    except (TypeError, ValueError, ZeroDivisionError) as exc:
        raise _Reject(f"{what}: malformed coefficient ({exc})")


def _read_domain(step: dict) -> str:
    dom = step.get("domain", "int")
    _need(dom in _DOMAINS, f"unknown domain: {dom!r}")
    return dom


def _h_bezout_relation(step: dict, ctx: dict) -> None:
    dom = _read_domain(step)
    ctx["subkind"] = "bezout"
    ctx["domain"] = dom
    if dom == "int":
        ctx["a"] = _as_int(step.get("a"), "a")
        ctx["b"] = _as_int(step.get("b"), "b")
        ctx["g"] = _as_int(step.get("g"), "g")
        ctx["u"] = _as_int(step.get("u"), "u")
        ctx["v"] = _as_int(step.get("v"), "v")
    else:
        ctx["a"] = _as_poly(step.get("a"), "a")
        ctx["b"] = _as_poly(step.get("b"), "b")
        ctx["g"] = _as_poly(step.get("g"), "g")
        ctx["u"] = _as_poly(step.get("u"), "u")
        ctx["v"] = _as_poly(step.get("v"), "v")


def _h_assert_gcd(step: dict, ctx: dict) -> None:
    _need(ctx.get("subkind") == "bezout", "assert_gcd before bezout_relation")
    dom = ctx["domain"]
    a, b, g, u, v = ctx["a"], ctx["b"], ctx["g"], ctx["u"], ctx["v"]
    if dom == "int":
        # (1) Bezout identity, exact.
        _need(u * a + v * b == g, f"Bezout relation fails: {u}*{a}+{v}*{b} != {g}")
        # (3) normalization: g > 0 (canonical), or the degenerate gcd(0,0)=0.
        if g == 0:
            _need(a == 0 and b == 0,
                  "g = 0 only certifies gcd(0, 0); a or b is nonzero")
        else:
            _need(g > 0, f"g = {g} is not normalized (must be positive)")
            # (2) g is a common divisor.
            _need(a % g == 0, f"g = {g} does not divide a = {a}")
            _need(b % g == 0, f"g = {g} does not divide b = {b}")
    else:
        # (1) Bezout identity over Q, exact polynomial arithmetic.
        lhs = _p_add(_p_mul(u, a), _p_mul(v, b))
        _need(_p_eq(lhs, g), "Bezout relation fails: u*a + v*b != g")
        # (3) normalization: g monic, or gcd(0,0)=0.
        if _p_is_zero(g):
            _need(_p_is_zero(a) and _p_is_zero(b),
                  "g = 0 only certifies gcd(0, 0); a or b is nonzero")
        else:
            _need(_p_is_monic(g), "g is not normalized (must be monic)")
            # (2) g is a common divisor.
            _need(_p_divides(g, a), "g does not divide a")
            _need(_p_divides(g, b), "g does not divide b")
    ctx["concluded"] = True


def _h_divides_relation(step: dict, ctx: dict) -> None:
    dom = _read_domain(step)
    ctx["subkind"] = "divides"
    ctx["domain"] = dom
    if dom == "int":
        ctx["a"] = _as_int(step.get("a"), "a")
        ctx["b"] = _as_int(step.get("b"), "b")
        ctx["q"] = _as_int(step.get("q"), "q")
    else:
        ctx["a"] = _as_poly(step.get("a"), "a")
        ctx["b"] = _as_poly(step.get("b"), "b")
        ctx["q"] = _as_poly(step.get("q"), "q")


def _h_assert_divides(step: dict, ctx: dict) -> None:
    _need(ctx.get("subkind") == "divides", "assert_divides before divides_relation")
    dom = ctx["domain"]
    a, b, q = ctx["a"], ctx["b"], ctx["q"]
    if dom == "int":
        _need(a * q == b, f"a | b fails: {a}*{q} = {a * q} != b = {b}")
    else:
        _need(_p_eq(_p_mul(a, q), b), "a | b fails: a*q != b")
    ctx["concluded"] = True


_HANDLERS = {
    "bezout_relation": _h_bezout_relation,
    "assert_gcd": _h_assert_gcd,
    "divides_relation": _h_divides_relation,
    "assert_divides": _h_assert_divides,
}

# Legal op set for kind "bezout" (both sub-kinds share the envelope).
_ALLOWED_OPS = set(_HANDLERS)


def check(log: Any) -> dict:
    """Independently RE-VERIFY a Bezout / divides cert-log document.

    Returns ``{valid, reason, checked_steps, kind, claim, subkind, domain}``.
    Recomputes the Bezout identity, both divisibilities and the normalization
    with exact ``int`` / :class:`fractions.Fraction` arithmetic; never trusts
    the generator.  Any malformed, tampered or unsatisfied step yields
    ``valid=False`` with a ``reason`` — this is the sound boundary.
    """
    checked = 0
    try:
        _need(isinstance(log, dict), "log is not a JSON object")
        _need(log.get("format") == FORMAT, f"unknown format: {log.get('format')!r}")
        kind = log.get("kind")
        _need(kind in KINDS, f"unknown kind: {kind!r}")
        _need(isinstance(log.get("claim", ""), str), "claim must be a string")
        steps = log.get("steps")
        _need(isinstance(steps, list) and steps, "steps must be a non-empty list")

        ctx: dict[str, Any] = {"concluded": False}
        for i, step in enumerate(steps):
            _need(isinstance(step, dict), f"step {i} is not an object")
            op = step.get("op")
            _need(op in _HANDLERS, f"step {i}: unknown op {op!r}")
            _need(op in _ALLOWED_OPS, f"step {i}: op {op!r} illegal for kind {kind!r}")
            try:
                _HANDLERS[op](step, ctx)
            except _Reject:
                raise
            except (KeyError, IndexError, TypeError, ValueError, ZeroDivisionError) as exc:
                raise _Reject(f"step {i} ({op}): malformed data ({exc})")
            checked += 1

        _need(ctx.get("concluded"), "log reached no verified conclusion step")
        return {"valid": True, "reason": "Bezout certificate independently re-verified",
                "checked_steps": checked, "kind": kind, "claim": log.get("claim"),
                "subkind": ctx.get("subkind"), "domain": ctx.get("domain")}
    except _Reject as exc:
        return {"valid": False, "reason": str(exc), "checked_steps": checked,
                "kind": log.get("kind") if isinstance(log, dict) else None,
                "claim": log.get("claim") if isinstance(log, dict) else None,
                "subkind": None, "domain": None}


# --------------------------------------------------------------------------- #
# Worker dispatch.
# --------------------------------------------------------------------------- #

def run(request: dict) -> dict:
    """Worker entrypoint (dispatch key ``cert_bezout``).

    * ``export`` -> serialize a certificate.  ``subkind`` is ``bezout``
      (default) or ``divides``; ``domain`` is ``int`` (default) or ``poly``;
      ``a``/``b`` are the operands.  Returns ``{"log": <document>}``.
    * ``check``  -> ``check(request["log"])``.
    """
    op = request.get("op", "check")
    if op == "check":
        return check(request["log"])
    if op == "export":
        sub = request.get("subkind", "bezout")
        domain = request.get("domain", "int")
        claim = request.get("claim")
        if sub == "bezout":
            log = export_bezout_cert(request["a"], request["b"],
                                     domain=domain, claim=claim)
        elif sub == "divides":
            log = export_divides_cert(request["a"], request["b"],
                                      domain=domain, claim=claim)
        else:
            raise ValueError(f"unknown subkind: {sub!r}")
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
