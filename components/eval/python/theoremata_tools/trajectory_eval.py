"""Trajectory-level agentic eval metrics (agentic-patterns-mining H2 / A5).

The existing harness (:mod:`theoremata_tools.eval_harness`) scores *outcomes* —
did the final answer match, pass@k, majority@k — but says nothing about the
*path* the agent took to get there. Two agents can both solve a problem while
one wanders through redundant, error-prone tool calls and the other walks a
near-optimal line. This module scores the path.

Four primitives, all pure-stdlib and deterministic:

* :func:`trajectory_efficiency` — ``L* / L_agent``: how close the agent's step
  count came to the known-optimal (reference) length. ``1.0`` = optimal, less
  than 1 for a longer-than-optimal run.
* :func:`redundancy_rate` — fraction of steps that made no progress (repeated a
  prior state / were explicitly flagged as wasted).
* :func:`tool_use_accuracy` — ``n_ok / (n_ok + n_error)`` over a list of tool-call
  records, i.e. how often the agent's tool invocations succeeded.
* :func:`pass_at_k` — the **unbiased** estimator ``1 - C(n-c, k) / C(n, k)`` for
  the probability that a random size-``k`` subsample of ``n`` samples (``c``
  correct) contains at least one correct sample (Chen et al., 2021; Codex).

:func:`score_trajectory` folds the first three into one per-run record, and
:func:`run` is the JSON dispatch (op ``trajectory_eval``) so ``worker.py`` can
expose it as a tool alongside the outcome-level ``eval`` op.

This module never mutates its inputs and imports no other Theoremata code, so it
is safe to wire into the worker without touching the shared eval files.
"""
from __future__ import annotations

import math
from typing import Any

# --------------------------------------------------------------------------- #
# Efficiency
# --------------------------------------------------------------------------- #

def trajectory_efficiency(agent_len: float, optimal_len: float) -> float:
    """Path efficiency ``L* / L_agent`` (optimal length over agent length).

    Returns ``1.0`` when the agent matched the optimal length, a value in
    ``(0, 1)`` when the agent took more steps than optimal, and is capped at
    ``1.0`` so an agent that (implausibly) beats the reference is not rewarded
    above optimal. Returns ``0.0`` for a non-positive agent length (a run with no
    steps cannot be efficient); a non-positive ``optimal_len`` is treated as no
    reference being available and yields ``0.0``.
    """
    agent = float(agent_len)
    optimal = float(optimal_len)
    if agent <= 0 or optimal <= 0:
        return 0.0
    return min(1.0, optimal / agent)


# --------------------------------------------------------------------------- #
# Redundancy
# --------------------------------------------------------------------------- #

def _step_key(step: Any) -> Any:
    """A hashable identity for a step, used to detect exact repeats.

    A ``dict`` step is keyed by its ``(tool, action, args/state)`` signature when
    present, else by a stable serialization; everything else by its ``repr``.
    """
    if isinstance(step, dict):
        for key in ("state", "signature", "key"):
            if key in step:
                return ("k", repr(step[key]))
        sig = tuple(
            (k, repr(step[k]))
            for k in ("tool", "action", "op", "args", "input", "result")
            if k in step
        )
        if sig:
            return ("sig", sig)
        return ("repr", repr(sorted(step.items(), key=lambda kv: str(kv[0]))))
    return ("repr", repr(step))


def _is_wasted(step: Any) -> bool:
    """A step is wasted when it is explicitly flagged as making no progress.

    Honors, on a dict step, ``progress=False``, ``wasted=True``, ``redundant=True``,
    or ``ok=False`` paired with an absent/False ``progress``.
    """
    if not isinstance(step, dict):
        return False
    if step.get("wasted") is True or step.get("redundant") is True:
        return True
    if "progress" in step and step.get("progress") is False:
        return True
    return False


def redundancy_rate(steps: list[Any]) -> float:
    """Fraction of steps that were repeated or explicitly wasted (no progress).

    A step counts as redundant if either (a) its identity (see :func:`_step_key`)
    has already appeared earlier in the trajectory, or (b) it is explicitly
    flagged as making no progress (``progress=False`` / ``wasted`` / ``redundant``).
    The result is ``redundant_steps / total_steps`` in ``[0, 1]``; an empty
    trajectory has redundancy ``0.0``.
    """
    if not steps:
        return 0.0
    seen: set[Any] = set()
    redundant = 0
    for step in steps:
        key = _step_key(step)
        repeat = key in seen
        if repeat or _is_wasted(step):
            redundant += 1
        seen.add(key)
    return redundant / len(steps)


# --------------------------------------------------------------------------- #
# Tool-use accuracy
# --------------------------------------------------------------------------- #

def _tool_ok(call: Any) -> bool:
    """Interpret one tool-call record as success (True) or error (False).

    Accepts ``{"ok": bool}``, ``{"error": ...}``, ``{"status": "ok"/"error"}``,
    or a bare bool/str verdict. Anything ambiguous is treated as an error so
    accuracy is never inflated by unparseable records.
    """
    if isinstance(call, bool):
        return call
    if isinstance(call, str):
        return call.strip().lower() in {"ok", "success", "succeeded", "pass", "true"}
    if isinstance(call, dict):
        if "ok" in call:
            return bool(call["ok"])
        if "error" in call:
            return not call["error"]
        status = str(call.get("status", "")).strip().lower()
        if status:
            return status in {"ok", "success", "succeeded", "pass"}
    return False


def tool_use_accuracy(tool_calls: list[Any]) -> dict[str, Any]:
    """Success rate over a list of tool-call records.

    Returns ``{"accuracy", "n_ok", "n_error", "n"}`` where ``accuracy`` is
    ``n_ok / n`` (``0.0`` for an empty list — no calls means no demonstrated
    accuracy). Each record is classified by :func:`_tool_ok`.
    """
    n = len(tool_calls)
    n_ok = sum(1 for c in tool_calls if _tool_ok(c))
    n_error = n - n_ok
    return {
        "accuracy": (n_ok / n) if n else 0.0,
        "n_ok": n_ok,
        "n_error": n_error,
        "n": n,
    }


# --------------------------------------------------------------------------- #
# Unbiased pass@k
# --------------------------------------------------------------------------- #

def pass_at_k(n: int, c: int, k: int) -> float:
    """Unbiased pass@k estimator ``1 - C(n-c, k) / C(n, k)``.

    Estimates the probability that a random size-``k`` subsample drawn from ``n``
    generated samples (of which ``c`` are correct) contains at least one correct
    sample. Uses exact integer :func:`math.comb`, so there is no floating-point
    error in the combinatorial term.

    Edge cases: ``c <= 0`` -> ``0.0``; ``c >= n`` (all correct) -> ``1.0``;
    ``k >= n`` collapses to "did any sample pass" (``1.0`` iff ``c > 0``). Raises
    ``ValueError`` for ``n < 0``, ``k < 0``, or ``k > n``.
    """
    if n < 0 or k < 0:
        raise ValueError("n and k must be non-negative")
    if k > n:
        raise ValueError(f"k ({k}) cannot exceed n ({n})")
    c = max(0, min(c, n))
    if c <= 0:
        return 0.0
    if c >= n:
        return 1.0
    if k == 0:
        # an empty subsample never contains a correct sample
        return 0.0
    if n - c < k:
        # every size-k subsample must include at least one correct sample
        return 1.0
    return 1.0 - (math.comb(n - c, k) / math.comb(n, k))


# --------------------------------------------------------------------------- #
# Per-trajectory scoring
# --------------------------------------------------------------------------- #

def score_trajectory(trajectory: dict[str, Any]) -> dict[str, Any]:
    """Score one agent run across the path-level axes.

    ``trajectory`` is a dict with any of:

    * ``steps`` — list of step records (drives redundancy and, if
      ``agent_len`` is absent, the agent length);
    * ``agent_len`` / ``optimal_len`` — explicit lengths for efficiency (else
      ``agent_len`` falls back to ``len(steps)`` and ``optimal_len`` to
      ``optimal_steps``);
    * ``tool_calls`` — list of tool-call records for tool-use accuracy.

    Returns ``{efficiency, redundancy, tool_accuracy, n_steps}`` — the axes are
    reported side by side and never blended into a single scalar.
    """
    steps = list(trajectory.get("steps", []) or [])
    n_steps = len(steps)

    agent_len = trajectory.get("agent_len")
    if agent_len is None:
        agent_len = n_steps
    optimal_len = trajectory.get("optimal_len", trajectory.get("optimal_steps", 0))

    tool_calls = list(trajectory.get("tool_calls", []) or [])

    return {
        "efficiency": trajectory_efficiency(agent_len, optimal_len or 0),
        "redundancy": redundancy_rate(steps),
        "tool_accuracy": tool_use_accuracy(tool_calls),
        "n_steps": n_steps,
    }


# --------------------------------------------------------------------------- #
# JSON dispatch (worker.py hook) + CLI
# --------------------------------------------------------------------------- #

def run(request: dict[str, Any]) -> dict[str, Any]:
    """JSON dispatch for the ``trajectory_eval`` worker op.

    Sub-ops (``op`` field, default ``score_trajectory``):

    * ``efficiency`` -> ``{"efficiency": float}``
    * ``redundancy`` -> ``{"redundancy": float}``
    * ``tool_use_accuracy`` -> the tool-use accuracy block
    * ``pass_at_k`` -> ``{"pass_at_k": float, "n", "c", "k"}``
    * ``score_trajectory`` (default) -> the per-run record, tagged with ``op``.
    """
    op = request.get("op", "score_trajectory")
    if op == "efficiency":
        return {
            "op": op,
            "efficiency": trajectory_efficiency(
                request["agent_len"], request["optimal_len"]
            ),
        }
    if op == "redundancy":
        return {"op": op, "redundancy": redundancy_rate(request.get("steps", []))}
    if op == "tool_use_accuracy":
        return {"op": op, **tool_use_accuracy(request.get("tool_calls", []))}
    if op == "pass_at_k":
        n = int(request["n"])
        c = int(request["c"])
        k = int(request["k"])
        return {"op": op, "pass_at_k": pass_at_k(n, c, k), "n": n, "c": c, "k": k}
    if op == "score_trajectory":
        return {"op": op, **score_trajectory(request.get("trajectory", request))}
    raise ValueError(f"unknown op: {op}")


def main() -> None:  # pragma: no cover - thin CLI shim
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
