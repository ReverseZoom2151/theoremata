"""Bounded counterexample search with explicit domains and assumptions.

The search itself runs inside the hard-kill sandbox (see :mod:`.sandbox`): the
model-emitted ``claim``/``assumptions`` expressions are evaluated in a child
process that is force-killed on timeout, and a single :class:`StepBudget`
governs total work across the whole cartesian product (decoupled from how many
variables/domains there are). The public return contract is unchanged -- the
Rust caller (``components/reason/falsification.rs``) still reads
``output.verdict`` and optional ``output.assignment``.
"""
from __future__ import annotations

import itertools
from typing import Any

from .safe_eval import compile_expression, ALLOWED_NAMES
from .sandbox import (
    DEFAULT_TIMEOUT_SECONDS,
    StepBudget,
    run_in_subprocess,
)

#: Default total step budget for one falsification. Decouples "how many
#: variables/domains" from "how much total work"; large enough not to disturb
#: existing bounded searches, low enough to force graceful termination on a
#: pathological product.
DEFAULT_STEP_BUDGET = 1_000_000


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


def _search_impl(
    variables: dict[str, dict[str, Any]],
    claim: str,
    assumptions: str,
    max_cases: int,
    budget_total: int,
) -> dict[str, Any]:
    """Core bounded search. Runs inside the sandbox child process.

    Consumes one unit of a shared :class:`StepBudget` per case so that an
    over-large product terminates gracefully (``inconclusive``) instead of
    silently running to an implicit per-turn cap.
    """
    names = list(variables)
    domains = [_domain(variables[name]) for name in names]
    assumption_code = compile_expression(assumptions, set(names))
    claim_code = compile_expression(claim, set(names))
    budget = StepBudget(total=min(int(max_cases), int(budget_total)))
    checked = 0
    admissible = 0
    for values in itertools.product(*domains):
        if not budget.spend(1):
            return {
                "verdict": "inconclusive",
                "reason": "step budget exhausted",
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


def search(
    variables: dict[str, dict[str, Any]],
    claim: str,
    assumptions: str = "True",
    max_cases: int = 100_000,
    *,
    budget: int = DEFAULT_STEP_BUDGET,
    timeout_seconds: float = DEFAULT_TIMEOUT_SECONDS,
    hard_kill: bool = True,
) -> dict[str, Any]:
    """Search for a counterexample to ``claim`` over integer ``variables``.

    By default the search runs in a hard-kill child process so a pathological
    (effectively infinite) expression is terminated within ``timeout_seconds``
    and reported as ``inconclusive`` rather than hanging the worker. Compile /
    spec errors still propagate as exceptions (the JSON worker turns them into
    ``{"ok": false, ...}``), preserving the pre-hardening behaviour.

    Return shape is unchanged: a dict carrying ``verdict`` (one of
    ``counterexample`` | ``no_counterexample_in_domain`` | ``inconclusive``) plus
    ``checked``/``admissible`` and, for a counterexample, ``assignment``.
    """
    impl_args = (variables, claim, assumptions, max_cases, budget)
    if not hard_kill:
        return _search_impl(*impl_args)

    result = run_in_subprocess(
        _search_impl, args=impl_args, timeout=timeout_seconds
    )
    if result.timed_out:
        return {
            "verdict": "inconclusive",
            "reason": f"execution timed out after {timeout_seconds}s",
            "checked": 0,
            "admissible": 0,
        }
    # ok -> the search dict; import/other error -> raise (worker -> ok:false).
    return result.unwrap()
