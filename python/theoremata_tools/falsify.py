"""Bounded counterexample search with explicit domains and assumptions."""
from __future__ import annotations

import itertools
from typing import Any

from .safe_eval import compile_expression, ALLOWED_NAMES


def _domain(spec: dict[str, Any]) -> range:
    start = int(spec.get("start", -20))
    stop = int(spec.get("stop", 21))
    step = int(spec.get("step", 1))
    if step == 0:
        raise ValueError("domain step cannot be zero")
    values = range(start, stop, step)
    if len(values) > 100_000:
        raise ValueError("domain exceeds 100,000 values")
    return values


def search(
    variables: dict[str, dict[str, Any]],
    claim: str,
    assumptions: str = "True",
    max_cases: int = 100_000,
) -> dict[str, Any]:
    names = list(variables)
    domains = [_domain(variables[name]) for name in names]
    assumption_code = compile_expression(assumptions, set(names))
    claim_code = compile_expression(claim, set(names))
    checked = 0
    admissible = 0
    for values in itertools.product(*domains):
        if checked >= max_cases:
            return {
                "verdict": "inconclusive",
                "reason": "case budget exhausted",
                "checked": checked,
                "admissible": admissible,
            }
        checked += 1
        env = dict(zip(names, values))
        scope = {**ALLOWED_NAMES, **env}
        if not eval(assumption_code, {"__builtins__": {}}, scope):
            continue
        admissible += 1
        if not eval(claim_code, {"__builtins__": {}}, scope):
            return {
                "verdict": "counterexample",
                "assignment": env,
                "checked": checked,
                "admissible": admissible,
            }
    return {
        "verdict": "no_counterexample_in_domain",
        "checked": checked,
        "admissible": admissible,
    }
