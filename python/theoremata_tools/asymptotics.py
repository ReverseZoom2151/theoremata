"""Asymptotic order-of-magnitude reasoning via a logarithmic embedding.

The `estimates` repo's ``log_linarith`` tactic observed that order-of-magnitude
relations between positive quantities become *linear* arithmetic once you take
logs: multiplication becomes addition and raising to a power becomes scalar
multiplication. So a monomial ``x^2 / y`` maps to the linear form
``2*log x - log y`` in the log-magnitude variables, and an asymptotic relation
``lhs REL rhs`` maps to ``(log lhs - log rhs) REL 0``.

This module performs that translation and delegates the actual decision to the
exact standard-library feasibility kernel in :mod:`feasibility`, so it inherits
exactness (``fractions.Fraction`` throughout) and re-checkable Farkas
certificates. No SymPy or Z3 required.
"""
from __future__ import annotations

import json
import sys
from fractions import Fraction
from typing import Any

from .feasibility import feasibility

# Asymptotic relation -> the sense of ``(log lhs - log rhs) SENSE 0``.
#   ~ / =    exact same order of magnitude   -> equality
#   <~ / <=  bounded above by                 -> <=
#   >~ / >=  bounded below by                 -> >=
#   <<       strictly smaller order           -> strict <
#   >>       strictly larger order            -> strict >
_REL_TO_SENSE = {
    "~": "=",
    "=": "=",
    "<~": "<=",
    "<=": "<=",
    ">~": ">=",
    ">=": ">=",
    "<<": "<",
    ">>": ">",
}

# Negation of a goal relation, expressed as an asymptotic relation. Note that
# the negation of ``~``/``=`` is a *disjunction* (``<<`` or ``>>``) and is
# handled specially in :func:`prove_asymptotic`.
_NEGATION = {
    "<<": ">~",   # not (a << b)  <=>  a >~ b
    ">>": "<~",   # not (a >> b)  <=>  a <~ b
    "<~": ">>",   # not (a <~ b)  <=>  a >> b
    "<=": ">>",
    ">~": "<<",   # not (a >~ b)  <=>  a << b
    ">=": "<<",
}


def _monomial_diff(lhs: dict[str, Any], rhs: dict[str, Any]) -> dict[str, Fraction]:
    """``log(lhs) - log(rhs)`` as a coefficient map over log-magnitude variables.

    A monomial is ``{var: exponent}`` (a product of powers); its log is the
    linear form whose coefficient on ``var`` is that exponent.
    """
    coeffs: dict[str, Fraction] = {}
    for v, e in (lhs or {}).items():
        coeffs[v] = coeffs.get(v, Fraction(0)) + Fraction(str(e))
    for v, e in (rhs or {}).items():
        coeffs[v] = coeffs.get(v, Fraction(0)) - Fraction(str(e))
    return {v: c for v, c in coeffs.items() if c != 0}


def _to_constraint(ac: dict[str, Any]) -> dict[str, Any]:
    """Translate one asymptotic constraint into a linear feasibility constraint
    ``(log lhs - log rhs) SENSE 0``."""
    rel = ac["rel"]
    if rel not in _REL_TO_SENSE:
        raise ValueError(f"unknown asymptotic relation: {rel!r}")
    coeffs = _monomial_diff(ac.get("lhs", {}), ac.get("rhs", {}))
    return {
        "coeffs": {v: str(c) for v, c in coeffs.items()},
        "sense": _REL_TO_SENSE[rel],
        "rhs": 0,
    }


def asymptotic_feasibility(constraints: list[dict]) -> dict:
    """Decide whether a system of asymptotic constraints is jointly satisfiable.

    Each constraint is ``{"lhs": monomial, "rel": "<<|<~|~|>~|>>|=|<=|>=",
    "rhs": monomial}`` where a monomial is ``{var: exponent}``. Returns
    ``{"feasible": bool, "variables": [...]}`` plus ``"log_model"`` (a
    satisfying assignment of log-magnitudes) when feasible, or ``"certificate"``
    (a re-checkable Farkas certificate) when not.
    """
    result = feasibility([_to_constraint(c) for c in constraints])
    out: dict[str, Any] = {
        "feasible": result["feasible"],
        "variables": result["variables"],
    }
    if result["feasible"]:
        out["log_model"] = result["model"]
    else:
        out["certificate"] = result["certificate"]
    return out


def prove_asymptotic(hypotheses: list[dict], goal: dict) -> dict:
    """Prove ``goal`` follows from ``hypotheses`` by refutation (log_linarith).

    Negate the goal, add it to the hypotheses, and show the combined system is
    infeasible. The negation of an equality goal is the disjunction ``<<`` OR
    ``>>``, so an equality is proved only when *both* strict augmentations are
    infeasible. Returns ``{"proved": bool, "certificate": ...}`` (the
    certificate is the infeasibility witness(es) when proved).
    """
    hyp = [_to_constraint(c) for c in hypotheses]
    rel = goal["rel"]

    if rel in ("~", "="):
        certs = []
        for neg_rel in ("<<", ">>"):
            neg = {"lhs": goal.get("lhs", {}), "rhs": goal.get("rhs", {}), "rel": neg_rel}
            res = feasibility(hyp + [_to_constraint(neg)])
            if res["feasible"]:
                return {"proved": False, "certificate": None}
            certs.append(res["certificate"])
        return {"proved": True, "certificate": certs}

    if rel not in _NEGATION:
        raise ValueError(f"cannot negate goal relation: {rel!r}")
    neg = {"lhs": goal.get("lhs", {}), "rhs": goal.get("rhs", {}), "rel": _NEGATION[rel]}
    res = feasibility(hyp + [_to_constraint(neg)])
    proved = not res["feasible"]
    return {"proved": proved, "certificate": res["certificate"] if proved else None}


def main() -> None:
    if len(sys.argv) >= 2:
        with open(sys.argv[1], encoding="utf-8") as fh:
            payload = json.load(fh)
    else:
        payload = json.load(sys.stdin)
    op = payload.get("op", "asymptotic_feasibility")
    if op == "asymptotic_feasibility":
        result = asymptotic_feasibility(payload.get("constraints", []))
        ok = result["feasible"]
    elif op == "prove_asymptotic":
        result = prove_asymptotic(payload.get("hypotheses", []), payload["goal"])
        ok = result["proved"]
    else:
        raise ValueError(f"unknown op: {op!r}")
    print(json.dumps(result, indent=2))
    raise SystemExit(0 if ok else 1)


if __name__ == "__main__":
    main()
