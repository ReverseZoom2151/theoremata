"""Flyspeck **modified-dual** LP certificate: exporter + self-contained checker.

Background
----------
The Flyspeck project (Hales et al.) discharged **43,078** linear programs to
finish the proof of the Kepler conjecture.  Each LP is a system ``A x <= b`` and
the goal is a *bound* (or plain *infeasibility*).  An off-the-shelf LP solver
answers in floating point, so its dual solution ``y`` is inexact and cannot be
trusted by a proof kernel.  Flyspeck's trick — the **modified dual** — turns that
inexact dual into a *machine-checkable, search-free* certificate:

1. **Repair.**  Round the untrusted floating-point multipliers to nearby exact
   rationals (``fractions.Fraction``), clamping any negative multiplier to ``0``.
   Because the true optimal dual of a rational LP is itself rational with small
   denominators, this rational reconstruction recovers it *exactly* — and the
   residual combination coefficient on every structural variable snaps to
   **exactly zero**.
2. **Verify by one weighted sum.**  With the repaired rational ``y >= 0`` the
   claim reduces to a *single* rational-weighted summation of the rows — **no
   search, no pivoting**.  Weak duality does the rest::

       c . x  =  (Σ_k y_k A_k) . x  =  Σ_k y_k (A_k . x)  <=  Σ_k y_k b_k = β

   valid for every ``x`` with ``A x <= b`` (each ``y_k >= 0`` and
   ``A_k . x <= b_k``).  For a pure infeasibility proof the objective is ``0``,
   the combination is the zero row, and ``Σ_k y_k b_k < 0`` is the contradiction
   ``0 <= negative``.

This mirrors the Farkas / dual shapes already emitted by
:mod:`theoremata_tools.linprog_cert` and :mod:`theoremata_tools.cert_log`, and
uses the same self-describing ``theoremata.cert-log.v1`` proof-log envelope.

Soundness boundary
------------------
The **repair (generator) is untrusted** — it is any LP solver plus rational
rounding.  The **checker** (:func:`check`) is the sound boundary: it re-derives
``y >= 0``, recomputes ``Σ_k y_k A_k`` with exact :class:`fractions.Fraction`
arithmetic and requires it to equal the target combination *exactly* (structural
residuals exactly ``0``), and recomputes the bound by its own weighted sum.  Any
tampered certificate — a perturbed multiplier, a non-zero leftover coefficient,
or a wrong bound — is REJECTED with ``valid=False`` and a reason.  Everything is
pure standard library, exact, deterministic, and offline.

Worker dispatch key: ``cert_flyspeck_lp`` (see :func:`run`).
"""
from __future__ import annotations

import json
from fractions import Fraction
from typing import Any, Iterable, Optional

FORMAT = "theoremata.cert-log.v1"
KIND = "flyspeck_lp"

# Default denominator ceiling for the rational reconstruction of the (untrusted,
# floating-point) dual.  The optimal dual of a rational LP is rational with small
# denominators, so this recovers it exactly while ignoring float noise.
DEFAULT_MAX_DENOMINATOR = 10 ** 6


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
    if isinstance(x, float):
        # Exact binary value of the float; callers repair() away the noise.
        return Fraction(x)
    return Fraction(str(x))


def _fs(x: Fraction) -> str:
    """Serialize a Fraction as ``"p"`` or ``"p/q"``."""
    return str(x)


def _row_fields(row: Any) -> tuple[dict[str, Fraction], Fraction]:
    """Read ``(a, b)`` from a row given as a dict (``a``/``coeffs`` + ``b``/``rhs``)."""
    if not isinstance(row, dict):
        raise TypeError(f"cannot interpret LP row: {row!r}")
    coeffs = row.get("a", row.get("coeffs", {}))
    b = row.get("b", row.get("rhs", 0))
    return ({str(k): _frac(v) for k, v in dict(coeffs).items()}, _frac(b))


def _sorted_vars(rows: list, objective: Optional[dict]) -> list[str]:
    names: set[str] = set()
    for row in rows:
        a, _b = _row_fields(row)
        names.update(a)
    if objective:
        names.update(str(k) for k in objective)
    return sorted(names)


def repair_dual(dual: Iterable, *, max_denominator: int = DEFAULT_MAX_DENOMINATOR
                ) -> list[Fraction]:
    """The **modified-dual repair**: untrusted multipliers -> exact rational ``y >= 0``.

    Each (possibly floating-point) multiplier is rounded to the nearest rational
    whose denominator divides no more than ``max_denominator`` (via
    :meth:`Fraction.limit_denominator`), and any negative result is clamped to
    ``0``.  Deterministic.  This is a *heuristic* reconstruction on the untrusted
    side — its output is only ever accepted if the checker's exact recomputation
    confirms it, so a bad reconstruction can never be unsound (it is rejected).
    """
    out: list[Fraction] = []
    for v in dual:
        f = _frac(v).limit_denominator(max_denominator)
        out.append(f if f > 0 else Fraction(0))
    return out


# --------------------------------------------------------------------------- #
# Exporter.
# --------------------------------------------------------------------------- #

def export_flyspeck_lp_cert(*, rows: Iterable, dual: Iterable,
                            objective: Optional[dict] = None,
                            variables: Optional[list] = None,
                            max_denominator: int = DEFAULT_MAX_DENOMINATOR,
                            claim: Optional[str] = None) -> dict:
    """Export a Flyspeck modified-dual LP certificate as a cert-log document.

    ``rows`` is the system ``A x <= b`` (each row a dict ``{"a": {var: num},
    "b": num}``; ``coeffs``/``rhs`` also accepted).  ``dual`` is the **untrusted**
    dual solution (floats accepted) — it is repaired to exact rationals via
    :func:`repair_dual`.  ``objective`` is the linear objective ``c`` whose value
    we bound (``max c.x``); omit it (or pass empty) for a plain infeasibility
    certificate, where the combination must be the zero row and the weighted rhs
    is negative.

    The emitted steps carry the repaired multipliers ``y >= 0``, the rows, the
    objective, and the claimed bound ``β = Σ_k y_k b_k``.  The checker
    re-verifies each independently.
    """
    rows = list(rows)
    obj_map = {str(k): _frac(v) for k, v in (objective or {}).items()}
    mode = "bound" if obj_map else "infeasible"

    variables = ([str(v) for v in variables] if variables is not None
                 else _sorted_vars(rows, obj_map or None))

    A: list[dict[str, Fraction]] = []
    b: list[Fraction] = []
    for row in rows:
        a, bb = _row_fields(row)
        A.append(a)
        b.append(bb)

    y = repair_dual(dual, max_denominator=max_denominator)
    if len(y) != len(A):
        raise ValueError(f"dual length {len(y)} != #rows {len(A)}")

    bound = sum((y[k] * b[k] for k in range(len(A))), Fraction(0))

    ser_rows = [{"a": {k: _fs(v) for k, v in A[k].items()}, "b": _fs(b[k])}
                for k in range(len(A))]

    if mode == "bound":
        default_claim = "max c.x subject to A x <= b is bounded above by the weighted rhs"
    else:
        default_claim = "the linear system A x <= b is infeasible (modified-dual Farkas cert)"

    steps = [
        {"op": "flyspeck_lp_problem", "mode": mode, "variables": variables,
         "rows": ser_rows, "objective": {k: _fs(v) for k, v in obj_map.items()},
         "note": "rows are A_k . x <= b_k; goal bounds max c.x (c=0 => infeasibility)"},
        {"op": "dual_multipliers", "y": [_fs(v) for v in y]},
        {"op": "assert_dual_nonneg"},
        {"op": "assert_combination_exact"},
        {"op": "assert_bound", "bound": _fs(bound)},
    ]
    return {
        "format": FORMAT,
        "kind": KIND,
        "claim": claim or default_claim,
        "steps": steps,
        "meta": {
            "producer": "cert_flyspeck_lp.export_flyspeck_lp_cert",
            "method": "flyspeck-modified-dual",
            "mode": mode,
            "max_denominator": int(max_denominator),
            "note": ("untrusted dual repaired to exact rationals; verification is a "
                     "single rational-weighted summation, no search"),
        },
    }


# --------------------------------------------------------------------------- #
# REFERENCE CHECKER.
# --------------------------------------------------------------------------- #

class _Reject(Exception):
    """Raised to reject a certificate with a human-readable reason."""


def _need(cond: bool, reason: str) -> None:
    if not cond:
        raise _Reject(reason)


def _h_problem(step, ctx):
    mode = step.get("mode")
    _need(mode in ("bound", "infeasible"), f"bad mode {mode!r}")
    variables = step["variables"]
    _need(isinstance(variables, list), "variables must be a list")
    rows = step["rows"]
    _need(isinstance(rows, list) and rows, "rows must be a non-empty list")
    A: list[dict[str, Fraction]] = []
    b: list[Fraction] = []
    for row in rows:
        _need(isinstance(row, dict), "malformed row")
        a = {str(k): _frac(v) for k, v in dict(row.get("a", {})).items()}
        A.append(a)
        b.append(_frac(row["b"]))
    obj = {str(k): _frac(v) for k, v in dict(step.get("objective", {})).items()}
    if mode == "infeasible":
        _need(not obj, "infeasible mode must carry an empty objective")
    ctx.update(mode=mode, variables=variables, A=A, b=b, objective=obj)


def _h_dual(step, ctx):
    y = step["y"]
    _need(isinstance(y, list), "y must be a list")
    y = [_frac(v) for v in y]
    _need(len(y) == len(ctx["A"]), f"dual length {len(y)} != #rows {len(ctx['A'])}")
    ctx["y"] = y


def _h_assert_nonneg(step, ctx):
    _need(all(v >= 0 for v in ctx["y"]),
          "dual y has a negative entry (y >= 0 violated)")


def _h_assert_combination_exact(step, ctx):
    A, y, obj = ctx["A"], ctx["y"], ctx["objective"]
    combo: dict[str, Fraction] = {}
    for k, ak in enumerate(A):
        for var, coeff in ak.items():
            combo[var] = combo.get(var, Fraction(0)) + y[k] * coeff
    # residual = (Σ y_k A_k) - c  must be the zero row (structural coeffs snap to 0).
    residual: dict[str, Fraction] = {}
    for var in set(combo) | set(obj):
        r = combo.get(var, Fraction(0)) - obj.get(var, Fraction(0))
        if r != 0:
            residual[var] = r
    _need(not residual,
          f"combination is not exact: Σ y.A - c has non-zero coeffs {{"
          + ", ".join(f"{k}: {v}" for k, v in sorted(residual.items())) + "}")
    ctx["combo"] = combo


def _h_assert_bound(step, ctx):
    b, y, mode = ctx["b"], ctx["y"], ctx["mode"]
    bound = sum((y[k] * b[k] for k in range(len(b))), Fraction(0))
    claimed = _frac(step["bound"])
    _need(bound == claimed,
          f"bound mismatch: Σ y.b = {bound} != claimed {claimed}")
    if mode == "infeasible":
        _need(bound < 0,
              f"infeasibility needs Σ y.b < 0 (contradiction 0 <= {bound}); "
              f"got {bound}")
    ctx["bound"] = bound
    ctx["concluded"] = True


_HANDLERS = {
    "flyspeck_lp_problem": _h_problem,
    "dual_multipliers": _h_dual,
    "assert_dual_nonneg": _h_assert_nonneg,
    "assert_combination_exact": _h_assert_combination_exact,
    "assert_bound": _h_assert_bound,
}

_ALLOWED_OPS = set(_HANDLERS)


def check(log: Any) -> dict:
    """Independently RE-VERIFY a ``flyspeck_lp`` cert-log document.

    Returns ``{valid, reason, checked_steps, kind, claim}``.  Recomputes ``y >= 0``,
    the exact combination ``Σ y.A`` (structural residuals must be exactly ``0``),
    and the bound ``Σ y.b`` with :class:`fractions.Fraction`; never trusts the
    generator.  Any malformed, tampered, or unsatisfied step yields
    ``valid=False`` with a ``reason`` — the sound boundary.
    """
    checked = 0
    try:
        _need(isinstance(log, dict), "log is not a JSON object")
        _need(log.get("format") == FORMAT, f"unknown format: {log.get('format')!r}")
        _need(log.get("kind") == KIND, f"unknown kind: {log.get('kind')!r}")
        steps = log.get("steps")
        _need(isinstance(steps, list) and steps, "steps must be a non-empty list")
        _need(isinstance(log.get("claim", ""), str), "claim must be a string")

        ctx: dict[str, Any] = {"concluded": False}
        for i, step in enumerate(steps):
            _need(isinstance(step, dict), f"step {i} is not an object")
            op = step.get("op")
            _need(op in _ALLOWED_OPS, f"step {i}: unknown op {op!r}")
            try:
                _HANDLERS[op](step, ctx)
            except _Reject:
                raise
            except (KeyError, IndexError, TypeError, ValueError, ZeroDivisionError) as exc:
                raise _Reject(f"step {i} ({op}): malformed data ({exc})")
            checked += 1

        _need(ctx.get("concluded"), "log reached no verified conclusion (assert_bound)")
        return {"valid": True, "reason": "all steps independently re-verified",
                "checked_steps": checked, "kind": KIND, "claim": log.get("claim")}
    except _Reject as exc:
        return {"valid": False, "reason": str(exc), "checked_steps": checked,
                "kind": log.get("kind") if isinstance(log, dict) else None,
                "claim": log.get("claim") if isinstance(log, dict) else None}


# --------------------------------------------------------------------------- #
# Worker dispatch.
# --------------------------------------------------------------------------- #

def run(request: dict) -> dict:
    """Worker entrypoint.  ``request["op"]`` is ``export`` or ``check``.

    * ``export`` -> serialize a repaired-dual LP certificate.  Requires ``rows``
      and ``dual``; optional ``objective``, ``variables``, ``max_denominator``,
      ``claim``.  Returns ``{"log": <document>}``.
    * ``check`` -> ``check(request["log"])``.
    """
    op = request.get("op", "check")
    if op == "check":
        return check(request["log"])
    if op == "export":
        log = export_flyspeck_lp_cert(
            rows=request["rows"],
            dual=request["dual"],
            objective=request.get("objective"),
            variables=request.get("variables"),
            max_denominator=request.get("max_denominator", DEFAULT_MAX_DENOMINATOR),
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
