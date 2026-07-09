"""Taylor-model approximation-bound **certificate** (exporter + reference checker).

A *Taylor model* of an analytic function ``f`` on a domain ``[a, b]`` is a pair
``(T, Δ)``:

* a polynomial ``T`` (the truncated Taylor expansion about a point ``z0``), and
* an **interval remainder** ``Δ`` with the rigorous guarantee

      |f(x) − T(x)| ≤ Δ           for all x ∈ [a, b].

Given that guarantee, an analytic obligation ``|f − p| ≤ ε`` (``p`` a polynomial
approximant) is discharged by a purely *polynomial + interval* computation: bound
the residual ``f − T`` by ``Δ`` and check ``Δ ⊆ [−ε, ε]``.  This module emits such
a certificate in Theoremata's ``theoremata.cert-log.v1`` proof-log format (kind
``taylor_model``) and ships a **self-contained REFERENCE CHECKER** that RE-VERIFIES
the enclosure with **validated interval arithmetic** and rejects any tampered cert.

Reuse, not reinvention
----------------------
Per the project principle "reuse existing libs first": ``sympy`` bundles
``mpmath``, and ``mpmath.iv`` already provides *validated* (outward-rounded,
fixed-precision) interval arithmetic and interval-valued elementary functions
(``iv.exp``, ``iv.sin`` …).  The checker evaluates every enclosure with
``mpmath.iv`` at a fixed ``dps`` — it does **not** hand-roll interval arithmetic.
Because ``mpmath.iv`` rounds outward, the residual interval the checker computes is
a guaranteed *superset* of the true residual range, so a passing check is sound.

Modified Taylor model
---------------------
The **modified** variant carries a *relative* remainder: the residual is written
``f(x) − T(x) = (x − z0)^{n+1} · g(x)`` with ``g ⊆ Δ`` on the domain, so the
certified enclosure of the residual is ``(x − z0)^{n+1} · Δ`` rather than a flat
``Δ``.  This is the standard device for removable-discontinuity / near-singular
handling (e.g. ``sin(x)/x`` near ``0``), where a flat remainder is hopeless but the
factored one stays tight.  The checker handles both variants with the same code
path (``modified`` flag).

Soundness boundary
------------------
:func:`check` is the sound boundary.  It recomputes, from the raw numbers in the
log only:

1. the residual enclosure ``R = f(X) − T(X)`` (``X = [a,b]``) with ``mpmath.iv``;
2. that the claimed remainder encloses it: ``R ⊆ Δ_enc`` — this REJECTS a
   **too-tight / unsound remainder** ``Δ``;
3. that the remainder implies the claimed ε: ``Δ_enc ⊆ [−ε, ε]`` and
   ``R ⊆ [−ε, ε]`` — this REJECTS a **wrong (too-small) ε**.

All endpoint comparisons pick the *conservative* interval end so that ulp-level
rounding can only make the checker reject, never wrongly accept.  Everything is
deterministic (fixed ``mpmath`` precision; no wall-clock, no RNG) and every input
is treated as UNTRUSTED DATA: malformed structure becomes ``valid=False`` with a
``reason`` rather than an exception.

Worker dispatch key: ``cert_taylor_model`` (see :func:`run`).

Honest scope note
-----------------
This is a *fixed-precision interval* reference checker — the offline stand-in.  A
fully HOL-Light-formalized Taylor model (à la the CoqInterval / HOL-Light interval
tactics, with a machine-checked bound on the remainder derivation itself) is the
toolchain-gated upgrade; this checker re-verifies the *enclosure*, not the
symbolic derivation of ``Δ``.
"""
from __future__ import annotations

from fractions import Fraction
from typing import Any, Optional

FORMAT = "theoremata.cert-log.v1"
KINDS = ("taylor_model",)

# Fixed default working precision (decimal digits) for the interval checker.
# Deterministic: the same document always checks the same way.
DEFAULT_DPS = 50

# Domain subdivision count for the residual enclosure.  A single interval
# evaluation of ``f(X) - T(X)`` suffers the interval-arithmetic *dependency
# problem* (it cannot see that ``f`` and ``T`` are correlated), grossly
# overestimating the residual.  Evaluating over a partition of ``[a,b]`` and
# taking the hull tightens the enclosure toward the true range as the pieces
# shrink — the standard "mincing" device.  Deterministic (fixed count).
DEFAULT_SUBDIVISIONS = 128

CAKEML_TARGET = (
    "hol_taylor_model_check: a HOL-Light/CoqInterval-formalized Taylor-model "
    "checker whose interval-enclosure semantics match this module; consumes "
    "theoremata.cert-log.v1 taylor_model documents unchanged. This mpmath.iv "
    "reference checker is the fixed-precision offline stand-in."
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
# Exact polynomial helpers (used to build/verify the residual polynomial in the
# exact ``func == "poly"`` case, where interval arithmetic's dependency problem
# would otherwise spuriously widen ``f(X) − T(X)`` away from the true 0).
# --------------------------------------------------------------------------- #

def _binom(n: int, k: int) -> int:
    if k < 0 or k > n:
        return 0
    num = 1
    for i in range(k):
        num = num * (n - i) // (i + 1)
    return num


def _shift_poly(coeffs: list[Fraction], z0: Fraction) -> list[Fraction]:
    """Re-express ``sum a_k x^k`` in powers of ``u = x − z0``.

    Returns coefficients ``b_j`` with ``f(u + z0) = sum b_j u^j`` (exact).
    """
    n = len(coeffs)
    out = [Fraction(0)] * n
    for k, ak in enumerate(coeffs):
        if ak == 0:
            continue
        # a_k (u + z0)^k = a_k sum_j C(k,j) z0^(k-j) u^j
        for j in range(k + 1):
            out[j] += ak * _binom(k, j) * (z0 ** (k - j))
    return out


def _poly_sub(p: list[Fraction], q: list[Fraction]) -> list[Fraction]:
    """Coefficient-wise ``p − q`` (ascending powers), padded to equal length."""
    m = max(len(p), len(q))
    p = p + [Fraction(0)] * (m - len(p))
    q = q + [Fraction(0)] * (m - len(q))
    return [p[i] - q[i] for i in range(m)]


# --------------------------------------------------------------------------- #
# Exporter.
# --------------------------------------------------------------------------- #

_SUPPORTED_FUNCS = ("poly", "exp", "sin", "cos", "log", "sqrt", "atan",
                    "sinh", "cosh", "tanh")


def export_taylor_model_cert(
    func: str,
    coeffs: list,
    remainder: list,
    domain: list,
    epsilon: Any,
    *,
    expansion_point: Any = 0,
    order: Optional[int] = None,
    modified: bool = False,
    func_coeffs: Optional[list] = None,
    precision_dps: int = DEFAULT_DPS,
    subdivisions: int = DEFAULT_SUBDIVISIONS,
    claim: Optional[str] = None,
) -> dict:
    """Serialize a Taylor-model approximation-bound certificate to a v1 log.

    Parameters
    ----------
    func:
        Name of ``f``: an ``mpmath.iv`` elementary function
        (``exp``/``sin``/``cos``/``log``/``sqrt``/``atan``/``sinh``/``cosh``/``tanh``)
        or ``"poly"`` (then ``func_coeffs`` gives ``f`` in ascending powers of ``x``).
    coeffs:
        Coefficients of ``T`` in **ascending powers of** ``(x − z0)``.
    remainder:
        The interval remainder ``Δ`` as ``[lo, hi]``.  For a ``modified`` model this
        bounds the *relative* remainder ``g`` (residual ``= (x−z0)^{order+1}·g``).
    domain:
        ``[a, b]``.
    epsilon:
        The claimed approximation bound ``ε`` (so the cert asserts ``|f − T| ≤ ε``).
    expansion_point:
        ``z0`` (default ``0``).
    order:
        Degree ``n`` of ``T`` (default ``len(coeffs) − 1``); the modified remainder
        power is ``n + 1``.
    modified:
        Emit the modified (relative-remainder) variant.
    """
    if func not in _SUPPORTED_FUNCS:
        raise ValueError(f"unsupported func {func!r}; expected one of {_SUPPORTED_FUNCS}")
    if func == "poly" and func_coeffs is None:
        raise ValueError("func='poly' requires func_coeffs (f in powers of x)")
    if not (isinstance(remainder, (list, tuple)) and len(remainder) == 2):
        raise ValueError("remainder must be a 2-element interval [lo, hi]")
    if not (isinstance(domain, (list, tuple)) and len(domain) == 2):
        raise ValueError("domain must be a 2-element interval [a, b]")

    Tc = [_frac(c) for c in coeffs]
    lo, hi = _frac(remainder[0]), _frac(remainder[1])
    if lo > hi:
        raise ValueError("remainder lo > hi")
    a, b = _frac(domain[0]), _frac(domain[1])
    if a > b:
        raise ValueError("domain a > b")
    n = order if order is not None else len(Tc) - 1

    step: dict[str, Any] = {
        "op": "taylor_model",
        "func": str(func),
        "expansion_point": _fs(expansion_point),
        "order": int(n),
        "coeffs": [_fs(c) for c in Tc],
        "remainder": [_fs(lo), _fs(hi)],
        "modified": bool(modified),
        "domain": [_fs(a), _fs(b)],
        "epsilon": _fs(epsilon),
        "precision_dps": int(precision_dps),
        "subdivisions": int(subdivisions),
    }
    if int(subdivisions) < 1:
        raise ValueError("subdivisions must be >= 1")
    if func == "poly":
        step["func_coeffs"] = [_fs(c) for c in func_coeffs]

    return {
        "format": FORMAT,
        "kind": "taylor_model",
        "claim": claim or (
            f"|{func}(x) - T(x)| <= {_fs(epsilon)} for x in "
            f"[{_fs(a)}, {_fs(b)}] (Taylor model about {_fs(expansion_point)}"
            f"{', modified' if modified else ''})"
        ),
        "steps": [
            step,
            {"op": "assert_remainder_encloses"},
            {"op": "assert_epsilon_bound"},
        ],
        "meta": {
            "producer": "cert_taylor_model.export_taylor_model_cert",
            "variant": "modified" if modified else "standard",
            "cakeml_target": CAKEML_TARGET,
        },
    }


# --------------------------------------------------------------------------- #
# Reference checker (validated interval arithmetic via mpmath.iv).
# --------------------------------------------------------------------------- #

class _Reject(Exception):
    """Raised to reject a certificate with a human-readable reason."""


def _need(cond: bool, reason: str) -> None:
    if not cond:
        raise _Reject(reason)


def _iv_ctx():
    """Import mpmath's validated interval context (ships with sympy)."""
    from mpmath import iv, mpf  # local import: guarded by importorskip in tests
    return iv, mpf


def _to_iv(iv, x: Fraction):
    """Tight interval enclosing the exact rational ``x`` (outward-rounded)."""
    return iv.mpf(x.numerator) / iv.mpf(x.denominator)


def _horner(iv, coeffs: list[Fraction], U):
    """Evaluate ``sum c_k U^k`` (ascending) in interval arithmetic via Horner."""
    acc = iv.mpf(0)
    for c in reversed(coeffs):
        acc = acc * U + _to_iv(iv, c)
    return acc


_IV_FUNC_NAMES = ("exp", "sin", "cos", "log", "sqrt", "atan",
                  "sinh", "cosh", "tanh")


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


def _residual_interval(iv, step: dict, a: Fraction, b: Fraction, z0: Fraction):
    """Validated enclosure of the residual ``f(x) − T(x)`` over ``[a, b]``.

    For ``func == 'poly'`` the residual is formed **exactly** as a polynomial in
    ``u = x − z0`` and evaluated once, so identical ``f`` and ``T`` yield the true
    ``[0, 0]`` (interval arithmetic would otherwise not cancel dependent terms).

    For elementary ``f`` the residual ``iv.f(X) − T(U)`` is evaluated over a
    partition of ``[a, b]`` into ``subdivisions`` pieces and hulled — mincing
    beats the dependency problem, driving the enclosure toward the true range.
    Outward rounding keeps every piece (and the hull) sound.
    """
    Tc = [_frac(c) for c in step["coeffs"]]
    func = step["func"]
    if func == "poly":
        fc = [_frac(c) for c in step["func_coeffs"]]
        f_in_u = _shift_poly(fc, z0)          # f re-expressed in u = x - z0
        res_coeffs = _poly_sub(f_in_u, Tc)    # exact residual polynomial
        U = iv.mpf([_to_iv(iv, a).a, _to_iv(iv, b).b]) - _to_iv(iv, z0)
        return _horner(iv, res_coeffs, U)
    _need(func in _IV_FUNC_NAMES, f"unknown func {func!r}")
    fn = getattr(iv, func)
    n = int(step.get("subdivisions", DEFAULT_SUBDIVISIONS))
    _need(n >= 1, "subdivisions must be >= 1")
    z0_iv = _to_iv(iv, z0)
    pieces = []
    width = b - a
    for k in range(n):
        left = a + width * Fraction(k, n)
        right = a + width * Fraction(k + 1, n)
        Xk = iv.mpf([_to_iv(iv, left).a, _to_iv(iv, right).b])
        pieces.append(fn(Xk) - _horner(iv, Tc, Xk - z0_iv))
    return _hull(iv, pieces)


def residual_enclosure(func, coeffs, domain, *, expansion_point=0,
                       modified=False, order=None, func_coeffs=None,
                       subdivisions=DEFAULT_SUBDIVISIONS, dps=DEFAULT_DPS):
    """Public helper: the checker's residual enclosure as ``(lo, hi)`` mpf.

    Producers use this to pick an *honest* remainder ``Δ`` (one that genuinely
    encloses the residual the checker will recompute) and an ``ε`` above it.
    Returns the enclosure of ``f − T`` (standard) — for the modified variant the
    caller divides out ``(x − z0)^{order+1}`` to bound the relative remainder.
    """
    iv, mpf = _iv_ctx()
    iv.dps = int(dps)
    a, b = _frac(domain[0]), _frac(domain[1])
    step = {
        "func": func, "coeffs": [_fs(c) for c in coeffs],
        "subdivisions": int(subdivisions),
    }
    if func == "poly":
        step["func_coeffs"] = [_fs(c) for c in func_coeffs]
    R = _residual_interval(iv, step, a, b, _frac(expansion_point))
    return mpf(R.a), mpf(R.b)


def _enclosure_interval(iv, step: dict, U):
    """The certified residual enclosure ``Δ_enc``.

    Standard: ``Δ``.  Modified: ``(x − z0)^{order+1} · Δ`` (relative remainder).
    """
    lo, hi = _frac(step["remainder"][0]), _frac(step["remainder"][1])
    delta = iv.mpf([_to_iv(iv, lo).a, _to_iv(iv, hi).b])
    if not step.get("modified"):
        return delta
    power = int(step["order"]) + 1
    _need(power >= 1, "modified model needs order >= 0")
    return (U ** power) * delta


def _lo(x):
    """Lower endpoint of an interval as a plain (real) mpf."""
    return x.a


def _hi(x):
    """Upper endpoint of an interval as a plain (real) mpf."""
    return x.b


def _h_taylor_model(step, ctx):
    iv, _mpf = ctx["_iv"], ctx["_mpf"]
    func = step.get("func")
    _need(isinstance(func, str), "taylor_model: func must be a string")
    _need(isinstance(step.get("coeffs"), list) and step["coeffs"],
          "taylor_model: coeffs must be a non-empty list")
    if func == "poly":
        _need(isinstance(step.get("func_coeffs"), list) and step["func_coeffs"],
              "taylor_model: func='poly' needs func_coeffs")
    a, b = _frac(step["domain"][0]), _frac(step["domain"][1])
    _need(a <= b, "taylor_model: domain a > b")
    z0 = _frac(step["expansion_point"])
    dlo, dhi = _frac(step["remainder"][0]), _frac(step["remainder"][1])
    _need(dlo <= dhi, "taylor_model: remainder lo > hi")
    _need(int(step["order"]) >= 0, "taylor_model: order must be >= 0")
    nsub = int(step.get("subdivisions", DEFAULT_SUBDIVISIONS))
    _need(1 <= nsub <= 100000, "taylor_model: subdivisions out of range [1, 100000]")

    X = iv.mpf([_to_iv(iv, a).a, _to_iv(iv, b).b])
    U = X - _to_iv(iv, z0)
    ctx["R"] = _residual_interval(iv, step, a, b, z0)      # true-residual enclosure
    ctx["Denc"] = _enclosure_interval(iv, step, U)         # claimed enclosure Δ_enc
    ctx["eps"] = _to_iv(iv, _frac(step["epsilon"]))


def _h_assert_remainder_encloses(step, ctx):
    """R ⊆ Δ_enc: the claimed remainder actually encloses the residual.

    Rejects a **too-tight / unsound** remainder.  Conservative endpoints: compare
    against the *inner* estimate of ``Δ_enc`` so ulp rounding can only reject.
    """
    R, D = ctx["R"], ctx["Denc"]
    _need(_hi(D) >= _hi(R) and _lo(D) <= _lo(R),
          f"remainder too tight: residual {[_lo(R), _hi(R)]} not inside "
          f"claimed Delta_enc {[_lo(D), _hi(D)]}")


def _h_assert_epsilon_bound(step, ctx):
    """Δ_enc ⊆ [−ε, ε] and R ⊆ [−ε, ε]: the remainder implies the claimed ε.

    Rejects a **wrong (too-small) ε**.  Uses the lower estimate of ε as its
    magnitude so ulp rounding can only reject.
    """
    R, D, eps = ctx["R"], ctx["Denc"], ctx["eps"]
    e = _lo(eps)                       # conservative magnitude of epsilon
    _need(e >= 0, "epsilon must be non-negative")
    _need(_hi(D) <= e and _lo(D) >= -e,
          f"epsilon too small: Delta_enc {[_lo(D), _hi(D)]} not within +/- {e}")
    _need(_hi(R) <= e and _lo(R) >= -e,
          f"epsilon too small: residual {[_lo(R), _hi(R)]} not within +/- {e}")
    ctx["concluded"] = True


_HANDLERS = {
    "taylor_model": _h_taylor_model,
    "assert_remainder_encloses": _h_assert_remainder_encloses,
    "assert_epsilon_bound": _h_assert_epsilon_bound,
}

_ALLOWED_OPS = set(_HANDLERS)


def check(log: Any) -> dict:
    """Independently RE-VERIFY a ``taylor_model`` cert-log document.

    Returns ``{valid, reason, checked_steps, kind, claim}``.  Recomputes the
    residual enclosure and both bounds with ``mpmath.iv`` at fixed precision; never
    trusts the producer.  Any malformed, tampered or unsatisfied step yields
    ``valid=False`` with a ``reason`` — the sound boundary.
    """
    checked = 0
    try:
        _need(isinstance(log, dict), "log is not a JSON object")
        _need(log.get("format") == FORMAT, f"unknown format: {log.get('format')!r}")
        _need(log.get("kind") in KINDS, f"unknown kind: {log.get('kind')!r}")
        steps = log.get("steps")
        _need(isinstance(steps, list) and steps, "steps must be a non-empty list")
        _need(isinstance(log.get("claim", ""), str), "claim must be a string")

        iv, mpf = _iv_ctx()
        # Fix precision deterministically from the (untrusted, bounded) log.
        dps = DEFAULT_DPS
        if isinstance(steps[0], dict) and "precision_dps" in steps[0]:
            try:
                dps = int(steps[0]["precision_dps"])
            except (TypeError, ValueError):
                raise _Reject("precision_dps is not an integer")
            _need(2 <= dps <= 400, "precision_dps out of the allowed range [2, 400]")
        iv.dps = dps

        ctx: dict[str, Any] = {"concluded": False, "_iv": iv, "_mpf": mpf}
        for i, step in enumerate(steps):
            _need(isinstance(step, dict), f"step {i} is not an object")
            op = step.get("op")
            _need(op in _ALLOWED_OPS, f"step {i}: unknown/illegal op {op!r}")
            try:
                _HANDLERS[op](step, ctx)
            except _Reject:
                raise
            except (KeyError, IndexError, TypeError, ValueError,
                    ZeroDivisionError, ArithmeticError) as exc:
                raise _Reject(f"step {i} ({op}): malformed data ({exc})")
            checked += 1

        _need(ctx.get("concluded"), "log reached no verified conclusion step")
        return {"valid": True, "reason": "Taylor-model enclosure re-verified with mpmath.iv",
                "checked_steps": checked, "kind": log.get("kind"),
                "claim": log.get("claim")}
    except _Reject as exc:
        return {"valid": False, "reason": str(exc), "checked_steps": checked,
                "kind": log.get("kind") if isinstance(log, dict) else None,
                "claim": log.get("claim") if isinstance(log, dict) else None}


# --------------------------------------------------------------------------- #
# Worker dispatch.
# --------------------------------------------------------------------------- #

def run(request: dict) -> dict:
    """Worker entrypoint.  ``request["op"]`` is ``export`` or ``check``.

    * ``export`` -> build a taylor_model cert-log document from
      ``func``/``coeffs``/``remainder``/``domain``/``epsilon`` (+ optional
      ``expansion_point``/``order``/``modified``/``func_coeffs``/``precision_dps``/
      ``claim``).  Returns ``{"log": <document>}``.
    * ``check`` -> :func:`check` on ``request["log"]``.
    """
    op = request.get("op", "check")
    if op == "check":
        return check(request["log"])
    if op == "export":
        log = export_taylor_model_cert(
            request["func"],
            request["coeffs"],
            request["remainder"],
            request["domain"],
            request["epsilon"],
            expansion_point=request.get("expansion_point", 0),
            order=request.get("order"),
            modified=bool(request.get("modified", False)),
            func_coeffs=request.get("func_coeffs"),
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
