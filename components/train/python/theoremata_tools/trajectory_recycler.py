"""Failed-trajectory recycler: mine wasted search for curriculum (HunyuanProver).

A prover that exhausts its budget on a theorem WITHOUT closing it still did real
work: it drove the goal forward through a partial proof and left a concrete,
still-open goal state at the frontier. HunyuanProver's data-synthesis-from-
failures observation is that this last reached goal is itself a well-formed
statement worth learning to prove -- so a failed attempt need not be pure waste;
it can mint fresh, standalone training conjectures.

This is the FAILURE-state sibling of :mod:`curriculum_synth`. Where
``curriculum_synth.subgoal_to_conjectures`` densifies a *succeeded* proof's
``have``-subgoals into a curriculum, this module recycles a *failed* attempt's
reached open goal:

* **form ``"reached_goal"``** -- "prove the reached open goal" as its OWN
  standalone statement. Whatever the search could not finish is exactly a
  gradient-bearing target the next policy should learn.
* **form ``"premise"``** (optional, emitted only when the original goal is
  known) -- "prove the ORIGINAL goal GIVEN the reached state as a lemma": the
  rendered statement ``<reached> -> <original>``. This teaches the "assume the
  frontier, close the theorem" bridge that the failed attempt was one lemma away
  from completing.

Only FAILED attempts with a usable reached state are recycled: a SOLVED attempt
is left to the STaR/harvest path (it is already a positive), and a fully-failed
attempt that never advanced the goal (no reached state) yields nothing.

Data-row contract
-----------------
Each emitted row carries a ``statement`` key (the rendered goal), so it drops
straight into :func:`flywheel.revolution` / :func:`flywheel.label_dataset` as a
problem (which accept ``statement`` / ``problem`` / ``goal``). Rows also carry
``{form, source, origin, index}`` provenance. These are *unproved conjectures*:
they enter the corpus as problems for the GPU-gated verifier/trainer to attempt,
never as SFT positives.

Offline / pure-Python: no model, no GPU, no live-core coupling. Deterministic --
order-preserving, keyed only by input content.
"""
from __future__ import annotations

import json
import sys
from typing import Any, Sequence

__all__ = [
    "recycle_failed_trajectory",
    "recycle_batch",
    "run",
]


# ---------------------------------------------------------------------------
# Attempt-shape probes (tolerant of the several row shapes in this package)
# ---------------------------------------------------------------------------

# keys that may carry the last reached (still-open) goal at the search frontier
_REACHED_KEYS = (
    "reached_goal",
    "open_goal",
    "last_goal",
    "reached_state",
    "goal_state",
    "frontier_goal",
    "reached",
)
# keys that may carry the ORIGINAL theorem statement the attempt targeted
_ORIGIN_KEYS = ("problem", "goal", "statement", "original", "target")


def _as_statement(value: Any) -> str:
    """Coerce a reached-state value (string, or a dict carrying a goal) into its
    statement text. Returns ``""`` when nothing usable is present."""
    if isinstance(value, dict):
        for k in ("statement", "goal", "text", "pretty"):
            v = value.get(k)
            if v:
                return str(v).strip()
        return ""
    if value is None:
        return ""
    return str(value).strip()


def _reached_goal(attempt: dict[str, Any]) -> str:
    """The last reached (still-open) goal of the attempt, or ``""`` if the search
    never advanced to a recordable frontier state."""
    for key in _REACHED_KEYS:
        if key in attempt:
            stmt = _as_statement(attempt[key])
            if stmt:
                return stmt
    return ""


def _origin_goal(attempt: dict[str, Any]) -> str:
    """The original theorem the failed attempt targeted (for the premise form)."""
    for key in _ORIGIN_KEYS:
        if key in attempt:
            stmt = _as_statement(attempt[key])
            if stmt:
                return stmt
    return ""


def _solved(attempt: dict[str, Any]) -> bool:
    """Did the attempt actually close the theorem? A solved attempt is NOT
    recycled here -- it is a positive for the STaR/harvest path. Tolerates
    ``solved`` / ``success`` / ``whole_verified`` / ``verified`` as a bool or a
    ``{compiled, axioms_ok}`` verdict dict."""
    for key in ("solved", "success", "whole_verified", "verified"):
        if key not in attempt:
            continue
        v = attempt[key]
        if isinstance(v, dict):
            if bool(v.get("compiled")) and bool(v.get("axioms_ok", True)):
                return True
        elif bool(v):
            return True
    return False


def _render_premise(reached: str, origin: str) -> str:
    """Render "assume the frontier, close the theorem" as ``reached -> origin``
    (Lean's implication arrow), matching :mod:`curriculum_synth`'s premise
    threading."""
    return f"{reached} -> {origin}"


# ---------------------------------------------------------------------------
# Core: one failed attempt -> new standalone conjecture rows
# ---------------------------------------------------------------------------

def recycle_failed_trajectory(attempt: Any) -> list[dict[str, Any]]:
    """Turn ONE failed proof attempt's reached open goal into new standalone
    training conjectures.

    ``attempt`` is a dict carrying the attempt's partial proof and its last
    reached (still-open) goal state (any of :data:`_REACHED_KEYS`), plus
    optionally the original targeted theorem (:data:`_ORIGIN_KEYS`). Emits:

    * one **``"reached_goal"``** row -- the reached open goal as its own
      statement; and
    * when the original goal is known AND differs from the reached goal, one
      **``"premise"``** row -- ``<reached> -> <original>`` (prove the original
      GIVEN the reached state as a lemma).

    Returns ``[]`` (nothing recycled) when the attempt is not a dict, is SOLVED
    (a positive belongs to the harvest path, not here), or has no usable reached
    state (the search never advanced). Deterministic: output depends only on the
    attempt's content.
    """
    if not isinstance(attempt, dict):
        return []
    if _solved(attempt):
        return []  # solved -> a positive; not a failure to recycle
    reached = _reached_goal(attempt)
    if not reached:
        return []  # never advanced to a recordable frontier -> nothing to mine

    origin = _origin_goal(attempt)
    partial = str(attempt.get("partial_proof", attempt.get("proof", ""))).strip()

    rows: list[dict[str, Any]] = [
        {
            "statement": reached,
            "reached_goal": reached,
            "form": "reached_goal",
            "source": "failed_trajectory",
            "origin": origin,
            "partial_proof": partial,
            "index": 0,
        }
    ]
    # premise form only when the original is known and genuinely distinct
    if origin and origin != reached:
        rows.append(
            {
                "statement": _render_premise(reached, origin),
                "reached_goal": reached,
                "form": "premise",
                "source": "failed_trajectory",
                "origin": origin,
                "partial_proof": partial,
                "index": 1,
            }
        )
    return rows


# ---------------------------------------------------------------------------
# Batch: map over attempts, dedup by rendered statement
# ---------------------------------------------------------------------------

def recycle_batch(attempts: Sequence[Any]) -> list[dict[str, Any]]:
    """Recycle a batch of failed attempts into new conjecture rows, de-duplicated
    by rendered ``statement`` (first occurrence wins; input order preserved).

    Solved attempts and attempts with no reached state contribute nothing.
    """
    rows: list[dict[str, Any]] = []
    seen: set[str] = set()
    for attempt in attempts:
        for row in recycle_failed_trajectory(attempt):
            stmt = row["statement"]
            if stmt in seen:
                continue
            seen.add(stmt)
            rows.append(row)
    return rows


# ---------------------------------------------------------------------------
# Worker dispatch
# ---------------------------------------------------------------------------

def run(request: dict[str, Any]) -> dict[str, Any]:
    """Dispatch the recycler over a JSON request.

    Op ``recycle_trajectory``:

    * a single ``{attempt: {...}}`` -> recycle that attempt; or
    * a batch ``{attempts: [...]}`` -> recycle and dedup the batch.

    Returns ``{ok, rows, n}``.
    """
    op = request.get("op", "recycle_trajectory")
    if op == "recycle_trajectory":
        if "attempts" in request:
            rows = recycle_batch(request.get("attempts", []))
        else:
            rows = recycle_failed_trajectory(request.get("attempt", request))
        return {"ok": True, "rows": rows, "n": len(rows)}
    raise ValueError(f"unknown op: {op}")


def main() -> None:
    if len(sys.argv) >= 2:
        with open(sys.argv[1], encoding="utf-8") as fh:
            request = json.load(fh)
    else:
        request = json.load(sys.stdin)
    print(json.dumps(run(request), indent=2, default=str))
    raise SystemExit(0)


if __name__ == "__main__":
    main()
