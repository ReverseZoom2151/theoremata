"""Proof-completion smoke runner for formalization benchmarks (MiniF2F, etc.).

Grades caller-supplied responses against loaded benchmark items. Does not call
a model — deterministic and testable offline.
"""
from __future__ import annotations

from typing import Any

from .graders import grade
from .loaders import LOADERS


def run_proof_completion(
    *,
    benchmark: str = "minif2f_test",
    responses: dict[str, Any] | None = None,
    limit: int | None = None,
) -> dict[str, Any]:
    """Load a formalization benchmark and grade supplied proof responses.

    Parameters
    ----------
    benchmark:
        Registry name (default ``minif2f_test`` for a small smoke split).
    responses:
        ``item_id -> lean proof text`` mapping. Missing ids are graded as empty.
    limit:
        Cap the number of items processed (after load).
    """
    if benchmark not in LOADERS:
        raise KeyError(
            f"unknown benchmark {benchmark!r}; known: {sorted(LOADERS)}"
        )
    items = LOADERS[benchmark]()
    if isinstance(limit, int) and limit >= 0:
        items = items[:limit]
    responses = responses or {}

    results: list[dict[str, Any]] = []
    solved = 0
    correct = 0
    for item in items:
        response = responses.get(item["id"], "")
        verdict = grade(item, response)
        if verdict.get("is_solved"):
            solved += 1
        if verdict.get("is_correct"):
            correct += 1
        results.append(
            {
                "id": item["id"],
                "is_solved": verdict.get("is_solved"),
                "is_correct": verdict.get("is_correct"),
                "detail": verdict.get("detail"),
            }
        )

    n = len(items)
    return {
        "benchmark": benchmark,
        "n": n,
        "solved": solved,
        "correct": correct,
        "accuracy": (correct / n) if n else None,
        "results": results,
    }