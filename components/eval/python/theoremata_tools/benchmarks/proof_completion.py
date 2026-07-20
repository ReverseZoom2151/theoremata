"""Proof-completion smoke runner for formalization benchmarks (MiniF2F, etc.).

Grades caller-supplied responses against loaded benchmark items. Does not call
a model — deterministic and testable offline.

Accounting contract (see :mod:`.graders`): a verdict may be UNGRADED, meaning no
gradable verdict was produced at all (typically the statement comparator was
unavailable). Such an item is neither a pass nor a fail. WHY this matters here:
``accuracy`` is a headline number, and dividing correct answers by *all* items
turns "we could not measure this" into "this failed", which reads exactly like a
real result. So the denominator is the GRADED subset, the ungraded count is
reported alongside it, and when nothing at all was graded the accuracy is
``None`` rather than ``0.0``.
"""
from __future__ import annotations

from typing import Any

from .graders import grade
from .loaders import LOADERS


def _is_ungraded(verdict: dict[str, Any]) -> bool:
    """True when the grader declined to render a gradable verdict.

    ``detail["ungraded"]`` is the authoritative signal (graders.py always sets it
    on the detail block); the top-level ``ungraded`` key is checked as well
    because the uniform verdict carries it too and the two must never disagree.
    """
    detail = verdict.get("detail")
    if isinstance(detail, dict) and detail.get("ungraded") is True:
        return True
    return verdict.get("ungraded") is True


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
    ungraded = 0
    for item in items:
        response = responses.get(item["id"], "")
        verdict = grade(item, response)
        item_ungraded = _is_ungraded(verdict)
        if item_ungraded:
            ungraded += 1
        if verdict.get("is_solved"):
            solved += 1
        if verdict.get("is_correct"):
            correct += 1
        results.append(
            {
                "id": item["id"],
                "is_solved": verdict.get("is_solved"),
                "is_correct": verdict.get("is_correct"),
                # Surfaced per item so a reader can locate the ungraded ones
                # instead of having to re-derive them from the detail block.
                "ungraded": item_ungraded,
                "detail": verdict.get("detail"),
            }
        )

    n = len(items)
    n_graded = n - ungraded
    # None, not 0.0: with nothing graded there is no evidence that nothing
    # passed, only an absence of measurement. A 0.0 here would assert the
    # former and is the exact misreport this accounting exists to prevent.
    accuracy = (correct / n_graded) if n_graded else None
    return {
        "benchmark": benchmark,
        "n": n,
        "n_graded": n_graded,
        "n_ungraded": ungraded,
        "solved": solved,
        "correct": correct,
        # REPURPOSED KEY: `accuracy` is now correct/n_graded, not correct/n.
        # It is None when nothing was graded.
        "accuracy": accuracy,
        "accuracy_basis": {
            "denominator": "graded",
            "n_graded": n_graded,
            "n_ungraded": ungraded,
            "n_total": n,
            "note": (
                "accuracy = correct / n_graded. Ungraded items produced no "
                "verdict and are excluded from the denominator but reported as "
                "n_ungraded; they are not failures. accuracy is None when "
                "n_graded is 0."
            ),
        },
        "results": results,
    }