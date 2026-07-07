"""Exact rational linear-arithmetic feasibility via Fourier--Motzkin elimination.

A standard-library-only decision procedure for systems of linear inequalities
over named rational variables. It answers whether the system is satisfiable
over the rationals; when feasible it returns a satisfying assignment, and when
infeasible it returns a Farkas-style certificate (nonnegative multipliers on
the normalized constraints that combine to ``0 <= negative`` / ``0 < 0``).

This is the standard-library analogue of the `estimates` repo's Z3-backed
``linprog.feasibility``: exact (no float drift, everything is
``fractions.Fraction``) and producing certificates a caller can re-check
independently. It is a linear-arithmetic oracle, not a general SMT solver.
"""
from __future__ import annotations

import json
import sys
from fractions import Fraction
from typing import Any


def _frac(x: Any) -> Fraction:
    """Parse a number exactly. Routing through ``str`` makes ``0.1`` -> 1/10
    and accepts ``"1/3"``, ``2``, ``0.5`` alike without binary-float drift."""
    if isinstance(x, Fraction):
        return x
    return Fraction(str(x))


class Row:
    """A single normalized constraint ``sum(coeffs[v] * v) (<= | <) b``.

    ``mult`` maps a normalized-row id to the nonnegative rational multiplier by
    which this row is built from the originals, so a contradiction row's
    ``mult`` is a Farkas certificate over the normalized rows.
    """

    __slots__ = ("coeffs", "b", "strict", "mult")

    def __init__(self, coeffs: dict[str, Fraction], b: Fraction, strict: bool,
                 mult: dict[int, Fraction]):
        self.coeffs = {k: v for k, v in coeffs.items() if v != 0}
        self.b = b
        self.strict = strict
        self.mult = {k: v for k, v in mult.items() if v != 0}

    def is_constant(self) -> bool:
        return not self.coeffs

    def is_contradiction(self) -> bool:
        # Constant row ``0 <= b`` is false iff b < 0; ``0 < b`` iff b <= 0.
        if not self.is_constant():
            return False
        return self.b <= 0 if self.strict else self.b < 0


def _normalize(constraints: list[dict]) -> tuple[list[Row], list[dict]]:
    """Convert user constraints into ``<=``/``<`` rows and record the normalized
    rows (for certificate reporting). Every row starts with multiplier 1 on its
    own id."""
    norm_rows: list[dict] = []

    def add(coeffs: dict[str, Fraction], b: Fraction, strict: bool,
            index: int, sense: str) -> Row:
        pruned = {k: v for k, v in coeffs.items() if v != 0}
        rid = len(norm_rows)
        norm_rows.append({
            "id": rid, "index": index, "sense": sense,
            "coeffs": pruned, "b": b, "strict": strict,
        })
        return Row(pruned, b, strict, {rid: Fraction(1)})

    rows: list[Row] = []
    for i, con in enumerate(constraints):
        coeffs = {k: _frac(v) for k, v in con.get("coeffs", {}).items()}
        b = _frac(con.get("rhs", 0))
        sense = con["sense"]
        neg = {k: -v for k, v in coeffs.items()}
        if sense == "<=":
            rows.append(add(coeffs, b, False, i, "<="))
        elif sense == "<":
            rows.append(add(coeffs, b, True, i, "<"))
        elif sense == ">=":
            rows.append(add(neg, -b, False, i, ">="))
        elif sense == ">":
            rows.append(add(neg, -b, True, i, ">"))
        elif sense == "=":
            rows.append(add(coeffs, b, False, i, "="))
            rows.append(add(neg, -b, False, i, "="))
        else:
            raise ValueError(f"unknown sense: {sense!r}")
    return rows, norm_rows


def _combine(u: Row, l: Row, v: str) -> Row:
    """Eliminate ``v`` from an upper-bound row ``u`` (coeff > 0) and a
    lower-bound row ``l`` (coeff < 0) by a positive combination that cancels
    ``v``. Both rows are ``A x <= b``, so the result is too."""
    a_uv = u.coeffs[v]
    a_lv = l.coeffs[v]
    alpha = -a_lv  # > 0
    beta = a_uv    # > 0
    coeffs: dict[str, Fraction] = {}
    for w in set(u.coeffs) | set(l.coeffs):
        c = alpha * u.coeffs.get(w, Fraction(0)) + beta * l.coeffs.get(w, Fraction(0))
        if c != 0:
            coeffs[w] = c
    b = alpha * u.b + beta * l.b
    mult: dict[int, Fraction] = {}
    for k, m in u.mult.items():
        mult[k] = mult.get(k, Fraction(0)) + alpha * m
    for k, m in l.mult.items():
        mult[k] = mult.get(k, Fraction(0)) + beta * m
    return Row(coeffs, b, u.strict or l.strict, mult)


def _certificate(row: Row, norm_rows: list[dict]) -> list[dict]:
    cert = []
    for rid, m in sorted(row.mult.items()):
        if m == 0:
            continue
        nr = norm_rows[rid]
        cert.append({
            "index": nr["index"],
            "sense": nr["sense"],
            "coeffs": {k: str(v) for k, v in nr["coeffs"].items()},
            "rhs": str(nr["b"]),
            "strict": nr["strict"],
            "multiplier": str(m),
        })
    return cert


def _choose_value(v: str, bound_rows: list[Row],
                  assignment: dict[str, Fraction]) -> Fraction:
    """Pick a rational for ``v`` inside the interval carved out by the rows that
    constrain it, given already-assigned later variables."""
    lowers: list[tuple[Fraction, bool]] = []  # v > val (strict) / v >= val
    uppers: list[tuple[Fraction, bool]] = []  # v < val (strict) / v <= val
    for r in bound_rows:
        cv = r.coeffs.get(v)
        if not cv:
            continue
        residual = r.b
        for w, cw in r.coeffs.items():
            if w == v:
                continue
            residual -= cw * assignment[w]
        bound = residual / cv
        if cv > 0:
            uppers.append((bound, r.strict))
        else:
            lowers.append((bound, r.strict))

    lo = hi = None
    lo_strict = hi_strict = False
    if lowers:
        lo = max(val for val, _ in lowers)
        lo_strict = any(s for val, s in lowers if val == lo)
    if uppers:
        hi = min(val for val, _ in uppers)
        hi_strict = any(s for val, s in uppers if val == hi)

    if lo is not None and hi is not None:
        if lo < hi:
            return (lo + hi) / 2
        return lo  # lo == hi with both non-strict (feasibility guarantees this)
    if lo is not None:
        return lo + 1 if lo_strict else lo
    if hi is not None:
        return hi - 1 if hi_strict else hi
    return Fraction(0)


def feasibility(constraints: list[dict]) -> dict:
    """Decide feasibility of a linear system over the rationals.

    Each constraint is ``{"coeffs": {var: number|str}, "sense":
    "<="|"<"|"="|">="|">", "rhs": number|str}``. Returns
    ``{"feasible": bool, "variables": [...], "model": {var: "p/q"}}`` when
    feasible, or ``{"feasible": False, "variables": [...], "certificate":
    [...]}`` when not. All rationals are exact and serialized as ``"p/q"``.
    """
    rows, norm_rows = _normalize(constraints)
    variables = sorted({k for nr in norm_rows for k in nr["coeffs"]})

    # Early constant contradictions (e.g. ``0 <= -1``).
    for r in rows:
        if r.is_contradiction():
            return {"feasible": False, "variables": variables,
                    "certificate": _certificate(r, norm_rows)}

    system = rows
    elim_stack: list[tuple[str, list[Row]]] = []
    for v in variables:
        with_v = [r for r in system if r.coeffs.get(v)]
        without_v = [r for r in system if not r.coeffs.get(v)]
        uppers = [r for r in with_v if r.coeffs[v] > 0]
        lowers = [r for r in with_v if r.coeffs[v] < 0]
        combined = [_combine(u, l, v) for u in uppers for l in lowers]
        elim_stack.append((v, with_v))
        system = without_v + combined
        for r in system:
            if r.is_contradiction():
                return {"feasible": False, "variables": variables,
                        "certificate": _certificate(r, norm_rows)}

    for r in system:
        if r.is_contradiction():
            return {"feasible": False, "variables": variables,
                    "certificate": _certificate(r, norm_rows)}

    assignment: dict[str, Fraction] = {}
    for v, bound_rows in reversed(elim_stack):
        assignment[v] = _choose_value(v, bound_rows, assignment)

    return {
        "feasible": True,
        "variables": variables,
        "model": {v: str(assignment.get(v, Fraction(0))) for v in variables},
    }


def main() -> None:
    if len(sys.argv) >= 2:
        with open(sys.argv[1], encoding="utf-8") as fh:
            payload = json.load(fh)
    else:
        payload = json.load(sys.stdin)
    result = feasibility(payload.get("constraints", []))
    print(json.dumps(result, indent=2))
    raise SystemExit(0 if result["feasible"] else 1)


if __name__ == "__main__":
    main()
