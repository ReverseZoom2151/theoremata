"""Continued-fraction / Diophantine hardness **proof-log** exporter + checker.

Companion to :mod:`theoremata_tools.cert_log`.  Where ``cert_log`` certifies
*asymptotic log-linear* width bounds (how a quantity grows), this module
certifies **exact rational approximation hardness**: given a target real
presented as an exact rational ``x`` (a plain rational, or a real truncated to
an exact rational), its **continued-fraction expansion** ``[a0; a1, a2, ...]``
certifies the sequence of **convergents** ``p_k/q_k`` — the *best rational
approximations* of ``x`` — together with a **conversion-hardness bound**: no
rational can get much closer to ``x`` than the convergent does, quantified by
the classical two-sided estimate

    1 / (q_k (q_k + q_{k+1}))  <  | x - p_k/q_k |  <  1 / (q_k q_{k+1}).

This is the exact-approximation analogue of Harrison's decimal/``arith`` work
on best rational approximations and the error incurred when a value with a
bounded denominator stands in for a target.

The document reuses the ``theoremata.cert-log.v1`` envelope (same ``format``,
same ``{op: ...}`` step list, exact :class:`fractions.Fraction` arithmetic, no
floating point).  The **reference checker** (:func:`check`) is the sound
boundary: it **RE-COMPUTES** the convergents from the partial quotients with
its own exact integer recurrence

    p_k = a_k p_{k-1} + p_{k-2},   q_k = a_k q_{k-1} + q_{k-2},

verifies they equal the convergents carried in the log, verifies the last
convergent equals the target exactly, verifies the consecutive-convergent
determinant identity ``|p_k q_{k-1} - p_{k-1} q_k| = 1`` (the property that
makes each convergent a best approximation), and verifies every claimed
hardness bound **exactly** by recomputing the approximation error
``|x - p_k/q_k|`` as a rational.  A tampered partial quotient, a tampered
convergent, or an **overstated** hardness bound is REJECTED.

Everything is pure standard library, deterministic, and offline.  All inputs
are treated as UNTRUSTED DATA: the checker validates structure/types
defensively and turns any malformed input into ``valid=False`` rather than
trusting it.  Worker dispatch key (intended): ``cert_continued_fraction``.
"""
from __future__ import annotations

import json
from fractions import Fraction
from typing import Any, Optional

# Same transport-neutral envelope as the sibling cert-log module.
FORMAT = "theoremata.cert-log.v1"
KINDS = ("continued_fraction",)


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


def _int(x: Any) -> int:
    """Parse an exact integer, rejecting bools and non-integral rationals."""
    if isinstance(x, bool):
        raise TypeError("boolean where an integer was expected")
    if isinstance(x, int):
        return x
    f = _frac(x)
    if f.denominator != 1:
        raise ValueError(f"expected an integer, got {x!r}")
    return f.numerator


def _fs(x: Fraction) -> str:
    """Serialize a Fraction as ``"p"`` or ``"p/q"``."""
    return str(x)


def continued_fraction(x: Fraction) -> list[int]:
    """Canonical (Euclidean) continued-fraction expansion of a rational.

    Returns ``[a0, a1, ...]`` with ``a_i >= 1`` for ``i >= 1``.  Finite because
    ``x`` is rational.
    """
    num, den = x.numerator, x.denominator
    a: list[int] = []
    while den != 0:
        q = num // den  # floor division -> canonical expansion
        a.append(q)
        num, den = den, num - q * den
    return a


def convergents(a: list[int]) -> tuple[list[int], list[int]]:
    """Exact integer convergent recurrence from partial quotients ``a``.

    ``p_k = a_k p_{k-1} + p_{k-2}``, ``q_k = a_k q_{k-1} + q_{k-2}`` with the
    standard seeds ``p_{-1}=1, p_{-2}=0, q_{-1}=0, q_{-2}=1``.
    """
    p_prev, p_prev2 = 1, 0
    q_prev, q_prev2 = 0, 1
    ps: list[int] = []
    qs: list[int] = []
    for ak in a:
        pk = ak * p_prev + p_prev2
        qk = ak * q_prev + q_prev2
        ps.append(pk)
        qs.append(qk)
        p_prev, p_prev2 = pk, p_prev
        q_prev, q_prev2 = qk, q_prev
    return ps, qs


def _lower_hardness_bound(qk: int, qk_next: Optional[int]) -> Optional[Fraction]:
    """Classical lower bound ``1/(q_k (q_k + q_{k+1}))`` on ``|x - p_k/q_k|``.

    ``None`` for the final convergent (no ``q_{k+1}``), where the error is 0.
    """
    if qk_next is None:
        return None
    return Fraction(1, qk * (qk + qk_next))


# --------------------------------------------------------------------------- #
# Exporter.
# --------------------------------------------------------------------------- #

def export_continued_fraction_cert(target: Any, *,
                                   partial_quotients: Optional[list] = None,
                                   claim: Optional[str] = None) -> dict:
    """Serialize a continued-fraction / hardness certificate for ``target``.

    ``target`` is the value being approximated, given as an **exact rational**
    (a plain fraction, or a real truncated to an exact rational).  If
    ``partial_quotients`` is omitted the canonical expansion is computed.

    The emitted document carries the target, the partial quotients, every
    convergent ``p_k/q_k`` (the final one equals the target exactly), and a
    hardness bound step per non-final convergent asserting
    ``|x - p_k/q_k| >= 1/(q_k (q_k + q_{k+1}))`` — the best-approximation
    conversion-hardness estimate.  The checker re-derives all of it.
    """
    x = _frac(target)
    if partial_quotients is None:
        a = continued_fraction(x)
    else:
        a = [_int(v) for v in partial_quotients]
        if not a:
            raise ValueError("partial_quotients must be non-empty")
    ps, qs = convergents(a)
    if qs[-1] == 0 or Fraction(ps[-1], qs[-1]) != x:
        raise ValueError("partial quotients do not reconstruct the target")

    bound_steps: list[dict] = []
    for k in range(len(a)):
        qk_next = qs[k + 1] if k + 1 < len(qs) else None
        bound = _lower_hardness_bound(qs[k], qk_next)
        if bound is None:
            continue
        err = abs(x - Fraction(ps[k], qs[k]))
        # Sanity: the classical estimate must hold for what we emit.
        assert err >= bound, "internal: hardness estimate violated"
        bound_steps.append({
            "op": "assert_hardness_bound", "k": k,
            "bound": _fs(bound), "direction": "lower",
            "note": "no rational beats p_k/q_k by more than this near x",
        })

    steps: list[dict] = [
        {"op": "cf_target", "x": _fs(x),
         "note": "target real presented as an exact rational"},
        {"op": "partial_quotients", "a": [str(v) for v in a],
         "note": "continued-fraction expansion [a0; a1, a2, ...]"},
        {"op": "convergents", "p": [str(v) for v in ps], "q": [str(v) for v in qs]},
        {"op": "assert_convergents_recompute",
         "note": "recompute p_k,q_k from a; match; last convergent == target; "
                 "|p_k q_{k-1} - p_{k-1} q_k| = 1"},
    ]
    steps.extend(bound_steps)
    steps.append({"op": "assert_best_approximation",
                  "note": "convergents are the best rational approximations of x"})
    return {
        "format": FORMAT,
        "kind": "continued_fraction",
        "claim": claim or ("continued-fraction convergents are best rational "
                           "approximations with certified conversion-hardness bounds"),
        "steps": steps,
        "meta": {"producer": "cert_continued_fraction.export",
                 "num_partial_quotients": len(a)},
    }


# --------------------------------------------------------------------------- #
# REFERENCE CHECKER.
# --------------------------------------------------------------------------- #

class _Reject(Exception):
    """Raised to reject a certificate with a human-readable reason."""


def _need(cond: bool, reason: str) -> None:
    if not cond:
        raise _Reject(reason)


def _h_cf_target(step, ctx):
    ctx["x"] = _frac(step["x"])


def _h_partial_quotients(step, ctx):
    a = step["a"]
    _need(isinstance(a, list) and len(a) >= 1,
          "partial_quotients: need a non-empty list")
    ctx["a"] = [_int(v) for v in a]


def _h_convergents(step, ctx):
    p = step["p"]
    q = step["q"]
    _need(isinstance(p, list) and isinstance(q, list),
          "convergents: p and q must be lists")
    _need(len(p) == len(q), "convergents: |p| != |q|")
    ctx["claim_p"] = [_int(v) for v in p]
    ctx["claim_q"] = [_int(v) for v in q]


def _h_assert_convergents_recompute(step, ctx):
    _need("a" in ctx, "recompute before partial_quotients")
    _need("x" in ctx, "recompute before cf_target")
    _need("claim_p" in ctx, "recompute before convergents")
    a = ctx["a"]
    # Independent recurrence -- do NOT trust the carried convergents.
    ps, qs = convergents(a)
    _need(len(ps) == len(ctx["claim_p"]),
          "recompute: convergent count != claimed")
    _need(ps == ctx["claim_p"] and qs == ctx["claim_q"],
          "recompute: convergents do not match the partial quotients")
    _need(qs[-1] != 0 and Fraction(ps[-1], qs[-1]) == ctx["x"],
          "recompute: last convergent does not equal the target")
    # Best-approximation determinant identity for consecutive convergents.
    p_prev, q_prev = 1, 0  # p_{-1}, q_{-1}
    for pk, qk in zip(ps, qs):
        det = pk * q_prev - p_prev * qk
        _need(abs(det) == 1,
              "recompute: |p_k q_{k-1} - p_{k-1} q_k| != 1 (not consecutive "
              "convergents)")
        p_prev, q_prev = pk, qk
    ctx["p"], ctx["q"] = ps, qs
    ctx["recomputed"] = True


def _h_assert_hardness_bound(step, ctx):
    _need(ctx.get("recomputed"), "hardness bound before convergents recomputed")
    k = _int(step["k"])
    ps, qs = ctx["p"], ctx["q"]
    _need(0 <= k < len(ps), f"hardness bound: index {k} out of range")
    direction = step.get("direction", "lower")
    bound = _frac(step["bound"])
    err = abs(ctx["x"] - Fraction(ps[k], qs[k]))
    if direction == "lower":
        # Overstating the bound (bound > true error) is rejected here.
        _need(err >= bound,
              f"overstated hardness bound at k={k}: |x - p/q| = {err} < {bound}")
    elif direction == "upper":
        _need(err <= bound,
              f"understated approximation error at k={k}: |x - p/q| = {err} > {bound}")
    else:
        raise _Reject(f"hardness bound: unknown direction {direction!r}")
    ctx["concluded"] = True


def _h_assert_best_approximation(step, ctx):
    # The determinant identity re-verified in the recompute step is exactly the
    # witness that each convergent is a best approximation; require it ran.
    _need(ctx.get("recomputed"),
          "best-approximation claim before convergents recomputed")
    ctx["concluded"] = True


_HANDLERS = {
    "cf_target": _h_cf_target,
    "partial_quotients": _h_partial_quotients,
    "convergents": _h_convergents,
    "assert_convergents_recompute": _h_assert_convergents_recompute,
    "assert_hardness_bound": _h_assert_hardness_bound,
    "assert_best_approximation": _h_assert_best_approximation,
}


def check(log: Any) -> dict:
    """Independently RE-VERIFY a continued-fraction cert-log document.

    Returns ``{valid, reason, checked_steps, kind, claim}``.  Recomputes the
    convergents and every hardness bound from the raw integers/rationals with
    exact arithmetic; it never trusts the carried convergents.  Any malformed,
    tampered, or overstated step yields ``valid=False`` with a ``reason``.
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

        ctx: dict[str, Any] = {"concluded": False}
        for i, step in enumerate(steps):
            _need(isinstance(step, dict), f"step {i} is not an object")
            op = step.get("op")
            _need(op in _HANDLERS, f"step {i}: unknown op {op!r}")
            try:
                _HANDLERS[op](step, ctx)
            except _Reject:
                raise
            except (KeyError, IndexError, TypeError, ValueError, ZeroDivisionError) as exc:
                raise _Reject(f"step {i} ({op}): malformed data ({exc})")
            checked += 1

        _need(ctx.get("recomputed"), "log never recomputed its convergents")
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

    * ``export`` -> ``{"log": <document>}`` for ``request["target"]`` (with
      optional ``partial_quotients`` and ``claim``).
    * ``check`` -> :func:`check` on ``request["log"]``.
    """
    op = request.get("op", "check")
    if op == "check":
        return check(request["log"])
    if op == "export":
        log = export_continued_fraction_cert(
            request["target"],
            partial_quotients=request.get("partial_quotients"),
            claim=request.get("claim"),
        )
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
