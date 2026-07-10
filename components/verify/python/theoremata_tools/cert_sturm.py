"""Sturm real-root-counting + composite polynomial-minimax certificates.

Two ``theoremata.cert-log.v1`` certificate kinds, both re-checked offline with
**exact rational arithmetic** (``fractions.Fraction`` via a reused ``_CheckPoly``
polynomial from :mod:`theoremata_tools.cert_log`) — the checker trusts *nothing*
the producer carries and recomputes every claim from the raw polynomial data.

Kind ``sturm`` — real-root counting
----------------------------------
For a **squarefree** univariate polynomial ``p`` and an interval ``(a, b]`` (with
``p(a) != 0``, ``p(b) != 0``), Sturm's theorem gives the number of *distinct*
real roots of ``p`` in ``(a, b]`` as ``V(a) − V(b)``, where ``V(x)`` counts the
sign variations of the **Sturm chain**

    p0 = p,  p1 = p',  p_{k+1} = −rem(p_{k-1}, p_k)

evaluated at ``x`` (zeros dropped).  Squarefreeness ⟺ the chain terminates in a
non-zero constant (``gcd(p, p')`` is constant).  The certificate carries ``p``,
``[a, b]``, the chain (exact-rational coefficient lists), the sign-variation
counts ``V(a)``/``V(b)`` and the claimed root count.  The checker **re-derives the
chain from ``p`` alone** with exact polynomial remainders, recomputes ``V(a)`` and
``V(b)``, and verifies ``count == V(a) − V(b) == claimed``.  A tampered chain (it
no longer matches the one derived from ``p``) or a wrong count is rejected.

Kind ``poly_minimax`` — composite ``|f − p| ≤ K`` on ``[a, b]``
-------------------------------------------------------------
Certifies that a polynomial ``p`` approximates ``f`` to within ``K`` on ``[a, b]``
by composing two independently-checked facts:

1.  a Taylor-model bound ``|f − T| ≤ δ`` on ``[a, b]`` (``T`` a polynomial), which
    the checker re-verifies by **reusing** :func:`theoremata_tools.cert_taylor_model.check`
    (validated ``mpmath.iv`` interval arithmetic); and
2.  a **Sturm-based no-crossing** argument: with ``D = p − T`` (an exact rational
    polynomial) and margin ``m = K − δ > 0``, the polynomial ``Q = m² − D²`` has
    **no root in ``(a, b]``** (Sturm distinct-root count ``== 0``) and ``Q(a) > 0``.
    Together these force ``Q > 0`` on all of ``[a, b]``, i.e. ``|D| < m``, hence
    ``|f − p| ≤ |f − T| + |T − p| < δ + m = K``.

The Sturm root-count is **inlined** here (it does not import a ``sturm`` cert at
runtime).  The exact-polynomial case (``func == "poly"`` with ``T == f``) makes the
whole obligation exact: ``δ = 0`` and ``Q = K² − (p − f)²`` is checked over ``Q``.
A too-small ``K`` (or a too-small ``δ``) is rejected.

Everything is deterministic and offline; all inputs are UNTRUSTED DATA turned into
``valid=False`` with a ``reason`` rather than trusted or executed.

Worker dispatch key: ``cert_sturm`` (see :func:`run`).
"""
from __future__ import annotations

import json
from fractions import Fraction
from typing import Any, Optional, Sequence

from theoremata_tools.cert_log import _CheckPoly  # reuse exact poly arithmetic

FORMAT = "theoremata.cert-log.v1"
KINDS = ("sturm", "poly_minimax")

CAKEML_TARGET = (
    "hol_sturm_check: a HOL-Light/CakeML-verified Sturm real-root-counting and "
    "polynomial-bound checker whose per-step semantics match this module; "
    "consumes theoremata.cert-log.v1 sturm/poly_minimax documents unchanged. "
    "This exact-rational Python checker is the offline stand-in."
)


# --------------------------------------------------------------------------- #
# Exact-rational parsing helpers (numbers travel as strings in the log).
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
# Univariate exact-rational polynomial helpers, built on the reused _CheckPoly
# (n == 1, single variable at index 0; ascending monomial coefficient lists).
# --------------------------------------------------------------------------- #

_V = 0  # the single variable index


def _poly(coeffs: Sequence[Fraction]) -> _CheckPoly:
    """``_CheckPoly`` for ascending monomial coefficients ``[c0, c1, ...]``."""
    return _CheckPoly(1, {(i,): Fraction(c) for i, c in enumerate(coeffs) if c != 0})


def _poly_from_strs(coeffs: Sequence[Any]) -> _CheckPoly:
    return _poly([_frac(c) for c in coeffs])


def _to_coeffs(p: _CheckPoly) -> list[Fraction]:
    """Ascending monomial coefficients (length ``deg + 1``; ``[]`` for zero)."""
    if p.is_zero():
        return []
    d = p.degree_in(_V)
    return [p.terms.get((i,), Fraction(0)) for i in range(d + 1)]


def _lc(p: _CheckPoly) -> Fraction:
    """Leading coefficient (Fraction); ``0`` for the zero polynomial."""
    if p.is_zero():
        return Fraction(0)
    return p.terms.get((p.degree_in(_V),), Fraction(0))


def _deriv(p: _CheckPoly) -> _CheckPoly:
    out: dict[tuple, Fraction] = {}
    for (i,), c in p.terms.items():
        if i >= 1:
            out[(i - 1,)] = c * i
    return _CheckPoly(1, out)


def _eval(p: _CheckPoly, x: Fraction) -> Fraction:
    acc = Fraction(0)
    for (i,), c in p.terms.items():
        acc += c * (x ** i)
    return acc


def _rem(g: _CheckPoly, f: _CheckPoly) -> _CheckPoly:
    """Exact polynomial remainder ``g mod f`` over the rationals.

    Long division with exact ``Fraction`` leading-coefficient quotients (no
    pseudo-division scaling, so signs are never accidentally flipped — essential
    for a correct Sturm sign sequence).  Reuses ``_CheckPoly`` sub/mul/degree.
    """
    if f.is_zero():
        raise ValueError("division by the zero polynomial")
    df = f.degree_in(_V)
    lcf = _lc(f)
    r = _CheckPoly(1, dict(g.terms))
    # Each step strictly lowers deg(r); exact cancellation guarantees termination.
    while (not r.is_zero()) and r.degree_in(_V) >= df:
        dr = r.degree_in(_V)
        term = _CheckPoly(1, {(dr - df,): _lc(r) / lcf})
        r = r - term * f
    return r


def sturm_chain(p: _CheckPoly) -> list[_CheckPoly]:
    """The Sturm chain ``p0=p, p1=p', p_{k+1} = −rem(p_{k-1}, p_k)``.

    Terminates at the last non-zero remainder (a constant multiple of
    ``gcd(p, p')``).  Works for any non-zero ``p`` (distinct-root counting via
    Sturm's theorem is valid even when ``p`` is not squarefree).
    """
    if p.is_zero():
        raise ValueError("Sturm chain of the zero polynomial is undefined")
    chain = [p]
    d = _deriv(p)
    if d.is_zero():           # p is a non-zero constant: no roots, chain = [p]
        return chain
    chain.append(d)
    while True:
        r = _rem(chain[-2], chain[-1])
        if r.is_zero():
            break
        chain.append(-r)
    return chain


def _sign_variations(chain: Sequence[_CheckPoly], x: Fraction) -> int:
    signs: list[int] = []
    for q in chain:
        v = _eval(q, x)
        if v != 0:
            signs.append(1 if v > 0 else -1)
    return sum(1 for i in range(1, len(signs)) if signs[i] != signs[i - 1])


def _root_count(chain: Sequence[_CheckPoly], a: Fraction, b: Fraction) -> int:
    """Distinct real roots of ``chain[0]`` in ``(a, b]`` = ``V(a) − V(b)``."""
    return _sign_variations(chain, a) - _sign_variations(chain, b)


# --------------------------------------------------------------------------- #
# Exporter: kind ``sturm``.
# --------------------------------------------------------------------------- #

def export_sturm_cert(coeffs: Sequence[Any], interval: Sequence[Any], *,
                      var: str = "x", claim: Optional[str] = None) -> dict:
    """Serialize a Sturm real-root-count certificate for ``p`` on ``(a, b]``.

    ``coeffs`` are ascending monomial coefficients of ``p``; ``interval`` is
    ``[a, b]``.  Raises ``ValueError`` if ``p`` is not squarefree or if an endpoint
    is a root of ``p`` (the count would be ill-defined).
    """
    p = _poly_from_strs(coeffs)
    if p.is_zero():
        raise ValueError("p must be non-zero")
    a, b = _frac(interval[0]), _frac(interval[1])
    if a >= b:
        raise ValueError("interval must satisfy a < b")
    if _eval(p, a) == 0 or _eval(p, b) == 0:
        raise ValueError("an endpoint is a root of p; count on (a, b] is ill-defined")
    chain = sturm_chain(p)
    last = chain[-1]
    if last.degree_in(_V) != 0 or last.is_zero():
        raise ValueError("p is not squarefree (Sturm chain does not end in a "
                         "non-zero constant)")
    va = _sign_variations(chain, a)
    vb = _sign_variations(chain, b)
    count = va - vb
    return {
        "format": FORMAT,
        "kind": "sturm",
        "claim": claim or f"p({var}) has {count} distinct real root(s) in "
                          f"({_fs(a)}, {_fs(b)}]",
        "steps": [
            {"op": "sturm_problem", "var": var,
             "coeffs": [_fs(c) for c in _to_coeffs(p)],
             "interval": [_fs(a), _fs(b)], "root_count": int(count)},
            {"op": "sturm_chain",
             "chain": [[_fs(c) for c in _to_coeffs(q)] for q in chain]},
            {"op": "sign_variations", "at_a": int(va), "at_b": int(vb)},
            {"op": "assert_chain"},
            {"op": "assert_root_count"},
        ],
        "meta": {"producer": "cert_sturm.export_sturm_cert",
                 "cakeml_target": CAKEML_TARGET},
    }


# --------------------------------------------------------------------------- #
# Exporter: kind ``poly_minimax``.
# --------------------------------------------------------------------------- #

def export_poly_minimax_cert(
    func: str,
    t_coeffs: Sequence[Any],
    p_coeffs: Sequence[Any],
    domain: Sequence[Any],
    K: Any,
    *,
    delta: Any = "0",
    func_coeffs: Optional[Sequence[Any]] = None,
    subdivisions: int = 128,
    precision_dps: int = 50,
    var: str = "x",
    claim: Optional[str] = None,
) -> dict:
    """Serialize a composite ``|f − p| ≤ K`` certificate on ``[a, b]``.

    ``func``/``func_coeffs`` name ``f`` (a Taylor-model ``mpmath.iv`` function, or
    ``"poly"`` with ``func_coeffs`` = ``f`` in ascending powers of ``x``).
    ``t_coeffs`` are the Taylor polynomial ``T`` in **powers of ``x``** (expansion
    point ``0``); ``p_coeffs`` are the approximant ``p`` in powers of ``x``.
    ``delta`` is a rational bound with ``|f − T| ≤ delta`` on ``[a, b]`` (``0`` when
    ``T == f`` exactly).  The Taylor bound is embedded as a full ``taylor_model``
    sub-certificate that the checker re-verifies via :mod:`cert_taylor_model`.
    """
    from theoremata_tools.cert_taylor_model import export_taylor_model_cert
    a, b = _frac(domain[0]), _frac(domain[1])
    if a >= b:
        raise ValueError("domain must satisfy a < b")
    d = _frac(delta)
    if d < 0:
        raise ValueError("delta must be non-negative")
    taylor_log = export_taylor_model_cert(
        func=func, coeffs=[_fs(c) for c in t_coeffs],
        remainder=[_fs(-d), _fs(d)], domain=[_fs(a), _fs(b)], epsilon=_fs(d),
        expansion_point=0, func_coeffs=(list(func_coeffs) if func_coeffs is not None
                                        else None),
        subdivisions=int(subdivisions), precision_dps=int(precision_dps),
    )
    return {
        "format": FORMAT,
        "kind": "poly_minimax",
        "claim": claim or f"|{func}({var}) - p({var})| <= {_fs(K)} on "
                          f"[{_fs(a)}, {_fs(b)}]",
        "steps": [
            {"op": "poly_minimax_problem", "var": var,
             "p_coeffs": [_fs(c) for c in p_coeffs],
             "domain": [_fs(a), _fs(b)], "K": _fs(K), "delta": _fs(d)},
            {"op": "taylor_bound", "taylor": taylor_log},
            {"op": "assert_taylor_bound"},
            {"op": "assert_no_crossing"},
        ],
        "meta": {"producer": "cert_sturm.export_poly_minimax_cert",
                 "cakeml_target": CAKEML_TARGET},
    }


# --------------------------------------------------------------------------- #
# Reference checker.
# --------------------------------------------------------------------------- #

class _Reject(Exception):
    """Raised to reject a certificate with a human-readable reason."""


def _need(cond: bool, reason: str) -> None:
    if not cond:
        raise _Reject(reason)


# -- sturm handlers ---------------------------------------------------------- #

def _h_sturm_problem(step, ctx):
    var = step.get("var", "x")
    _need(isinstance(step.get("coeffs"), list) and step["coeffs"],
          "sturm_problem: coeffs must be a non-empty list")
    p = _poly_from_strs(step["coeffs"])
    _need(not p.is_zero(), "sturm_problem: p must be non-zero")
    a, b = _frac(step["interval"][0]), _frac(step["interval"][1])
    _need(a < b, "sturm_problem: interval must satisfy a < b")
    _need(_eval(p, a) != 0, "sturm_problem: a is a root of p (count ill-defined)")
    _need(_eval(p, b) != 0, "sturm_problem: b is a root of p (count ill-defined)")
    ctx.update(p=p, a=a, b=b, claimed_count=int(step["root_count"]), var=var)


def _h_sturm_chain(step, ctx):
    _need(isinstance(step.get("chain"), list) and step["chain"],
          "sturm_chain: chain must be a non-empty list")
    carried = [_poly_from_strs(c) for c in step["chain"]]
    # Independently re-derive the chain from p; a tampered chain is rejected.
    derived = sturm_chain(ctx["p"])
    _need(len(carried) == len(derived),
          f"sturm chain length mismatch: carried {len(carried)} != "
          f"derived {len(derived)}")
    for i, (cq, dq) in enumerate(zip(carried, derived)):
        _need(_to_coeffs(cq) == _to_coeffs(dq),
              f"sturm chain entry {i} does not match the chain derived from p")
    last = derived[-1]
    _need(not last.is_zero() and last.degree_in(_V) == 0,
          "p is not squarefree (chain does not end in a non-zero constant)")
    ctx["chain"] = derived


def _h_sign_variations(step, ctx):
    va = _sign_variations(ctx["chain"], ctx["a"])
    vb = _sign_variations(ctx["chain"], ctx["b"])
    _need(int(step["at_a"]) == va,
          f"sign variations at a: carried {step['at_a']} != recomputed {va}")
    _need(int(step["at_b"]) == vb,
          f"sign variations at b: carried {step['at_b']} != recomputed {vb}")
    ctx["va"], ctx["vb"] = va, vb


def _h_assert_chain(step, ctx):
    _need("chain" in ctx, "assert_chain before the chain was derived")


def _h_assert_root_count(step, ctx):
    va = ctx.get("va")
    vb = ctx.get("vb")
    if va is None or vb is None:                 # tolerate missing sign step
        va = _sign_variations(ctx["chain"], ctx["a"])
        vb = _sign_variations(ctx["chain"], ctx["b"])
    count = va - vb
    _need(count == ctx["claimed_count"],
          f"root count mismatch: V(a)-V(b) = {count} != claimed "
          f"{ctx['claimed_count']}")
    ctx["concluded"] = True


# -- poly_minimax handlers --------------------------------------------------- #

def _h_poly_minimax_problem(step, ctx):
    _need(isinstance(step.get("p_coeffs"), list) and step["p_coeffs"],
          "poly_minimax_problem: p_coeffs must be a non-empty list")
    ctx["p_approx"] = _poly_from_strs(step["p_coeffs"])
    a, b = _frac(step["domain"][0]), _frac(step["domain"][1])
    _need(a < b, "poly_minimax_problem: domain must satisfy a < b")
    ctx.update(a=a, b=b, K=_frac(step["K"]), delta=_frac(step["delta"]))
    _need(ctx["delta"] >= 0, "poly_minimax_problem: delta must be non-negative")


def _h_taylor_bound(step, ctx):
    tlog = step.get("taylor")
    _need(isinstance(tlog, dict), "taylor_bound: missing taylor sub-certificate")
    ctx["taylor_log"] = tlog


def _h_assert_taylor_bound(step, ctx):
    from theoremata_tools import cert_taylor_model as tm  # lazy: needs mpmath
    tlog = ctx["taylor_log"]
    res = tm.check(tlog)
    _need(res.get("valid") is True,
          f"embedded taylor_model bound is invalid: {res.get('reason')}")
    # The sub-cert must bound |f - T| by exactly the delta used in the composite,
    # on the same domain, expanded at 0 (so T's coeffs are powers of x).
    tstep = tlog["steps"][0]
    _need(_frac(tstep["epsilon"]) == ctx["delta"],
          "taylor_model epsilon != composite delta")
    _need(_frac(tstep["expansion_point"]) == 0,
          "taylor_model expansion point must be 0 for the composite")
    ta, tb = _frac(tstep["domain"][0]), _frac(tstep["domain"][1])
    _need(ta == ctx["a"] and tb == ctx["b"],
          "taylor_model domain != composite domain")
    ctx["T"] = _poly_from_strs(tstep["coeffs"])


def _h_assert_no_crossing(step, ctx):
    a, b, K, delta = ctx["a"], ctx["b"], ctx["K"], ctx["delta"]
    margin = K - delta
    _need(margin > 0, f"K too small: margin K - delta = {margin} is not positive")
    D = ctx["p_approx"] - ctx["T"]                       # exact residual p - T
    Q = _poly([margin * margin]) - (D * D)              # m^2 - D^2
    # Q(a) > 0 anchors the sign; Sturm-count of roots of Q in (a, b] must be 0.
    _need(_eval(Q, a) > 0,
          "no-crossing fails: |p - T| >= margin at the left endpoint")
    _need(not Q.is_zero(), "no-crossing degenerate: m^2 - D^2 is identically zero")
    chain = sturm_chain(Q)
    roots = _root_count(chain, a, b)
    _need(roots == 0,
          f"no-crossing fails: m^2 - (p-T)^2 has {roots} root(s) in (a, b] "
          f"(so |p - T| reaches the margin; |f - p| <= K not established)")
    # Q(a) > 0 and no roots in (a, b] => Q > 0 on [a, b] => |p - T| < margin =>
    # |f - p| <= |f - T| + |T - p| < delta + margin = K.
    ctx["concluded"] = True


_HANDLERS = {
    "sturm_problem": _h_sturm_problem,
    "sturm_chain": _h_sturm_chain,
    "sign_variations": _h_sign_variations,
    "assert_chain": _h_assert_chain,
    "assert_root_count": _h_assert_root_count,
    "poly_minimax_problem": _h_poly_minimax_problem,
    "taylor_bound": _h_taylor_bound,
    "assert_taylor_bound": _h_assert_taylor_bound,
    "assert_no_crossing": _h_assert_no_crossing,
}

_KIND_OPS = {
    "sturm": {"sturm_problem", "sturm_chain", "sign_variations",
              "assert_chain", "assert_root_count"},
    "poly_minimax": {"poly_minimax_problem", "taylor_bound",
                     "assert_taylor_bound", "assert_no_crossing"},
}


def check(log: Any) -> dict:
    """Independently RE-VERIFY a ``sturm`` / ``poly_minimax`` cert-log document.

    Returns ``{valid, reason, checked_steps, kind, claim}``.  Re-derives the Sturm
    chain / recomputes the composite bound from the raw rationals with exact
    arithmetic; never trusts the producer.  Any malformed, tampered or unsatisfied
    step yields ``valid=False`` with a ``reason`` — the sound boundary.
    """
    checked = 0
    try:
        _need(isinstance(log, dict), "log is not a JSON object")
        _need(log.get("format") == FORMAT, f"unknown format: {log.get('format')!r}")
        kind = log.get("kind")
        _need(kind in KINDS, f"unknown kind: {kind!r}")
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
            except (KeyError, IndexError, TypeError, ValueError,
                    ZeroDivisionError, ArithmeticError) as exc:
                raise _Reject(f"step {i} ({op}): malformed data ({exc})")
            checked += 1

        _need(ctx.get("concluded"), "log reached no verified conclusion step")
        return {"valid": True, "reason": "all steps independently re-verified",
                "checked_steps": checked, "kind": kind, "claim": log.get("claim")}
    except _Reject as exc:
        return {"valid": False, "reason": str(exc), "checked_steps": checked,
                "kind": log.get("kind") if isinstance(log, dict) else None,
                "claim": log.get("claim") if isinstance(log, dict) else None}


# --------------------------------------------------------------------------- #
# Worker dispatch.
# --------------------------------------------------------------------------- #

def run(request: dict) -> dict:
    """Worker entrypoint.  ``request["op"]`` is ``export`` or ``check``.

    * ``export`` with ``kind == "sturm"`` -> :func:`export_sturm_cert`
      (``coeffs``/``interval`` + optional ``var``/``claim``).
    * ``export`` with ``kind == "poly_minimax"`` -> :func:`export_poly_minimax_cert`
      (``func``/``t_coeffs``/``p_coeffs``/``domain``/``K`` + optional
      ``delta``/``func_coeffs``/``subdivisions``/``precision_dps``/``var``/``claim``).
    * ``check`` -> :func:`check` on ``request["log"]``.
    """
    op = request.get("op", "check")
    if op == "check":
        return check(request["log"])
    if op == "export":
        kind = request.get("kind", "sturm")
        if kind == "sturm":
            log = export_sturm_cert(request["coeffs"], request["interval"],
                                    var=request.get("var", "x"),
                                    claim=request.get("claim"))
        elif kind == "poly_minimax":
            log = export_poly_minimax_cert(
                request["func"], request["t_coeffs"], request["p_coeffs"],
                request["domain"], request["K"],
                delta=request.get("delta", "0"),
                func_coeffs=request.get("func_coeffs"),
                subdivisions=int(request.get("subdivisions", 128)),
                precision_dps=int(request.get("precision_dps", 50)),
                var=request.get("var", "x"), claim=request.get("claim"))
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
