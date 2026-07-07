"""Verified-programming smoke runner (BRIDGE-style; structural grading only)."""
from __future__ import annotations

from typing import Any

from .graders import grade
from .loaders import LOADERS


def run_verified_programming(
    *,
    benchmark: str = "bridge178",
    responses: dict[str, Any] | None = None,
    limit: int | None = None,
) -> dict[str, Any]:
    if benchmark not in LOADERS:
        raise KeyError(f"unknown benchmark {benchmark!r}; known: {sorted(LOADERS)}")
    items = LOADERS[benchmark]()
    if isinstance(limit, int) and limit >= 0:
        items = items[:limit]
    responses = responses or {}
    results = []
    solved = correct = 0
    for item in items:
        verdict = grade(item, responses.get(item["id"], ""))
        if verdict.get("is_solved"):
            solved += 1
        if verdict.get("is_correct"):
            correct += 1
        results.append({"id": item["id"], **verdict})
    n = len(items)
    return {
        "benchmark": benchmark,
        "n": n,
        "solved": solved,
        "correct": correct,
        "accuracy": (correct / n) if n else None,
        "results": results,
    }