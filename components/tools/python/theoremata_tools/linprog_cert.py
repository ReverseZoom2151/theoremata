"""Exact rational linear programming with a certificate *either way*.

Given a system of linear (in)equalities over named rational variables this
kernel decides feasibility and always returns a re-checkable certificate:

* **feasible**  -> a concrete rational witness assignment;
* **infeasible** -> nonnegative Farkas/dual multipliers on the constraints whose
  combination is an evident contradiction (``0 <= negative`` or ``0 < 0``).

Two backends. If ``z3`` is importable it is used (primal model for feasibility,
dual solve for Farkas multipliers, mirroring Tao's ``estimates`` ``linprog``).
Otherwise a pure-Python Fourier--Motzkin elimination over
:class:`fractions.Fraction` is used -- exact, no floating point, and it tracks
the multiplier by which every derived row is built from the originals, so the
final contradiction row *is* the Farkas certificate.

Both :mod:`log_linarith` and any linear front end are meant to be thin wrappers
over :func:`feasibility` here.
"""
from __future__ import annotations

from fractions import Fraction
from numbers import Rational
from typing import Any, Iterable, Literal

try:  # pragma: no cover - environment dependent
    import z3 as _z3  # noqa: N813

    _HAS_Z3 = True
except Exception:  # pragma: no cover
    _z3 = None
    _HAS_Z3 = False

Sense = Literal["leq", "lt", "geq", "gt", "eq"]

# Accept a handful of surface spellings for the sense.
_SENSE_ALIASES = {
    "leq": "leq", "<=": "leq", "le": "leq",
    "lt": "lt", "<": "lt",
    "geq": "geq", ">=": "geq", "ge": "geq",
    "gt": "gt", ">": "gt",
    "eq": "eq", "=": "eq", "==": "eq",
}


def _frac(x: Any) -> Fraction:
    """Parse a number exactly (routing through ``str`` avoids float drift)."""
    if isinstance(x, Fraction):
        return x
    if isinstance(x, Rational):
        return Fraction(x.numerator, x.denominator)
    return Fraction(str(x))


class Inequality:
    """A linear constraint ``sum(coeffs[v]*v) SENSE rhs`` with exact rationals.

    ``coeffs`` maps a variable name (any hashable, stringified for reporting) to
    a rational coefficient; ``sense`` is one of ``leq/lt/geq/gt/eq`` (aliases
    ``<=, <, >=, >, =`` accepted); ``rhs`` is a rational.
    """

    __slots__ = ("coeffs", "sense", "rhs")

    def __init__(self, coeffs: dict, sense: str, rhs: Any = 0) -> None:
        self.coeffs = {str(v): _frac(c) for v, c in coeffs.items() if _frac(c) != 0}
        s = _SENSE_ALIASES.get(sense)
        if s is None:
            raise ValueError(f"invalid sense: {sense!r}")
        self.sense: Sense = s  # type: ignore[assignment]
        self.rhs = _frac(rhs)

    def variables(self) -> set[str]:
        return set(self.coeffs.keys())

    def as_dict(self) -> dict[str, Any]:
        return {
            "coeffs": {v: str(c) for v, c in self.coeffs.items()},
            "sense": self.sense,
            "rhs": str(self.rhs),
        }

    def __str__(self) -> str:
        lhs = " + ".join(f"{c}*{v}" for v, c in self.coeffs.items()) or "0"
        op = {"leq": "<=", "lt": "<", "geq": ">=", "gt": ">", "eq": "="}[self.sense]
        return f"{lhs} {op} {self.rhs}"

    __repr__ = __str__


def _coerce(inequalities: Iterable) -> list[Inequality]:
    out: list[Inequality] = []
    for item in inequalities:
        if isinstance(item, Inequality):
            out.append(item)
        elif isinstance(item, dict):
            out.append(
                Inequality(item.get("coeffs", {}), item["sense"], item.get("rhs", 0))
            )
        else:
            raise TypeError(f"cannot interpret constraint: {item!r}")
    return out


def ineq_variables(inequalities: list[Inequality]) -> set[str]:
    vs: set[str] = set()
    for ineq in inequalities:
        vs.update(ineq.variables())
    return vs


# --------------------------------------------------------------------------- #
# Pure-Python Fourier--Motzkin backend (exact, certificate-tracking).
# --------------------------------------------------------------------------- #

class _Row:
    """A normalized row ``sum(coeffs*v) (<=|<) b`` with a multiplier trail.

    ``mult`` maps ``normalized-row id -> nonnegative multiplier``; a
    contradiction row's ``mult`` is therefore a Farkas certificate over the
    normalized ``<=``/``<`` rows.
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
        if not self.is_constant():
            return False
        return self.b <= 0 if self.strict else self.b < 0


def _normalize(inequalities: list[Inequality]) -> tuple[list[_Row], list[dict]]:
    """Turn each constraint into ``<=``/``<`` rows, tracking their provenance."""
    norm_rows: list[dict] = []

    def add(coeffs: dict[str, Fraction], b: Fraction, strict: bool,
            index: int, orientation: int) -> _Row:
        pruned = {k: v for k, v in coeffs.items() if v != 0}
        rid = len(norm_rows)
        norm_rows.append({
            "id": rid, "index": index, "orientation": orientation,
            "coeffs": pruned, "b": b, "strict": strict,
        })
        return _Row(pruned, b, strict, {rid: Fraction(1)})

    rows: list[_Row] = []
    for i, ineq in enumerate(inequalities):
        coeffs = dict(ineq.coeffs)
        b = ineq.rhs
        neg = {k: -v for k, v in coeffs.items()}
        if ineq.sense == "leq":
            rows.append(add(coeffs, b, False, i, +1))
        elif ineq.sense == "lt":
            rows.append(add(coeffs, b, True, i, +1))
        elif ineq.sense == "geq":
            rows.append(add(neg, -b, False, i, -1))
        elif ineq.sense == "gt":
            rows.append(add(neg, -b, True, i, -1))
        elif ineq.sense == "eq":
            rows.append(add(coeffs, b, False, i, +1))
            rows.append(add(neg, -b, False, i, -1))
    return rows, norm_rows


def _combine(u: _Row, l: _Row, v: str) -> _Row:
    """Positively combine an upper row (coeff>0) and lower row (coeff<0) to
    cancel ``v``; the nonnegative combination preserves ``A x <= b`` form."""
    alpha = -l.coeffs[v]  # > 0
    beta = u.coeffs[v]    # > 0
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
    return _Row(coeffs, b, u.strict or l.strict, mult)


def _certificate(row: _Row, norm_rows: list[dict],
                 inequalities: list[Inequality]) -> dict:
    """Assemble a Farkas certificate from a contradiction row's multipliers."""
    terms = []
    for rid, m in sorted(row.mult.items()):
        if m == 0:
            continue
        nr = norm_rows[rid]
        terms.append({
            "index": nr["index"],
            "constraint": str(inequalities[nr["index"]]),
            "orientation": nr["orientation"],
            "multiplier": str(m),
        })
    return {
        "type": "farkas",
        "multipliers": terms,
        "combination": {
            "coeffs": {k: str(v) for k, v in row.coeffs.items()},  # {} => 0
            "strict": row.strict,
            "rhs": str(row.b),
            "reads": f"0 {'<' if row.strict else '<='} {row.b}",
        },
    }


def _choose_value(v: str, bound_rows: list[_Row],
                  assignment: dict[str, Fraction]) -> Fraction:
    lowers: list[tuple[Fraction, bool]] = []
    uppers: list[tuple[Fraction, bool]] = []
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
        return lo
    if lo is not None:
        return lo + 1 if lo_strict else lo
    if hi is not None:
        return hi - 1 if hi_strict else hi
    return Fraction(0)


def _feasibility_fm(inequalities: list[Inequality]) -> dict:
    rows, norm_rows = _normalize(inequalities)
    variables = sorted(ineq_variables(inequalities))

    for r in rows:
        if r.is_contradiction():
            return {"feasible": False, "backend": "pure-python", "variables": variables,
                    "certificate": _certificate(r, norm_rows, inequalities)}

    system = rows
    elim_stack: list[tuple[str, list[_Row]]] = []
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
                return {"feasible": False, "backend": "pure-python",
                        "variables": variables,
                        "certificate": _certificate(r, norm_rows, inequalities)}

    assignment: dict[str, Fraction] = {}
    for v, bound_rows in reversed(elim_stack):
        assignment[v] = _choose_value(v, bound_rows, assignment)

    return {
        "feasible": True,
        "backend": "pure-python",
        "variables": variables,
        "model": {v: str(assignment.get(v, Fraction(0))) for v in variables},
    }


# --------------------------------------------------------------------------- #
# Z3 backend (used only when z3 is importable).
# --------------------------------------------------------------------------- #

def _feasibility_z3(inequalities: list[Inequality]) -> dict:  # pragma: no cover
    variables = sorted(ineq_variables(inequalities))
    zvars = {v: _z3.Real(v) for v in variables}

    def lhs(ineq: Inequality):
        return _z3.Sum(*[zvars[v] * float(c) for v, c in ineq.coeffs.items()]) \
            if ineq.coeffs else _z3.RealVal(0)

    s = _z3.Solver()
    for ineq in inequalities:
        L, r = lhs(ineq), _z3.RealVal(str(ineq.rhs))
        s.add({"leq": L <= r, "lt": L < r, "geq": L >= r,
               "gt": L > r, "eq": L == r}[ineq.sense])

    if s.check() == _z3.sat:
        m = s.model()

        def val(v):
            x = m[zvars[v]]
            if x is None:
                return "0"
            return str(Fraction(int(x.numerator_as_long()), int(x.denominator_as_long())))

        return {"feasible": True, "backend": "z3", "variables": variables,
                "model": {v: val(v) for v in variables}}

    # Dual / Farkas solve.
    duals = {i: _z3.Real(f"dual_{i}") for i in range(len(inequalities))}
    d = _z3.Solver()
    for i, ineq in enumerate(inequalities):
        if ineq.sense in ("leq", "lt"):
            d.add(duals[i] <= 0)
        elif ineq.sense in ("geq", "gt"):
            d.add(duals[i] >= 0)
    for v in variables:
        d.add(_z3.Sum(*[duals[i] * float(ineq.coeffs.get(v, 0))
                        for i, ineq in enumerate(inequalities)]) == 0)
    final = _z3.Sum(*[duals[i] * float(ineq.rhs) for i, ineq in enumerate(inequalities)])
    d.add(final >= 0)
    slt = _z3.Sum(*[duals[i] for i, q in enumerate(inequalities) if q.sense == "lt"])
    sgt = _z3.Sum(*[duals[i] for i, q in enumerate(inequalities) if q.sense == "gt"])
    d.add(final + sgt - slt == 1)

    if d.check() == _z3.sat:
        m = d.model()
        terms = []
        for i, ineq in enumerate(inequalities):
            x = m[duals[i]]
            if x is None:
                continue
            mv = Fraction(int(x.numerator_as_long()), int(x.denominator_as_long()))
            if mv != 0:
                terms.append({"index": i, "constraint": str(ineq),
                              "multiplier": str(mv)})
        return {"feasible": False, "backend": "z3", "variables": variables,
                "certificate": {"type": "farkas", "multipliers": terms}}

    raise ValueError("Farkas lemma violation: neither feasible nor infeasible.")


def feasibility(inequalities: Iterable) -> dict:
    """Decide feasibility of a linear system, returning a certificate either way.

    ``inequalities`` is an iterable of :class:`Inequality` or dicts
    ``{"coeffs": {var: num}, "sense": "leq|lt|geq|gt|eq", "rhs": num}``.

    Returns ``{"feasible", "backend", "variables", ...}`` where the extra key is
    ``"model"`` (a rational witness, values as ``"p/q"``) when feasible, or
    ``"certificate"`` (Farkas multipliers) when infeasible.
    """
    ineqs = _coerce(inequalities)
    if not ineqs:
        return {"feasible": True, "backend": "trivial", "variables": [], "model": {}}
    if _HAS_Z3:
        return _feasibility_z3(ineqs)
    return _feasibility_fm(ineqs)


def evaluate(request: dict) -> dict:
    """JSON-able entry point for the worker.

    Request: ``{"constraints": [ {coeffs, sense, rhs}, ... ]}``. Response:
    ``{status, verdict, certificate|witness, details}``.
    """
    try:
        constraints = request.get("constraints", request.get("inequalities", []))
        result = feasibility(constraints)
    except Exception as exc:  # pragma: no cover - defensive
        return {"status": "error", "verdict": "error", "details": {"error": str(exc)}}

    if result["feasible"]:
        return {
            "status": "ok",
            "verdict": "feasible",
            "witness": result.get("model", {}),
            "details": {"backend": result["backend"], "variables": result["variables"]},
        }
    return {
        "status": "ok",
        "verdict": "infeasible",
        "certificate": result["certificate"],
        "details": {"backend": result["backend"], "variables": result["variables"]},
    }
