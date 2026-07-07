"""Difficulty scoring + Easy/Medium/Hard curriculum ordering (LeanAgent-style).

Ports the difficulty/curriculum idea mined in
``docs/resource-mining/new/LeanAgent.md``: LeanAgent "computes difficulty from
proof-step counts, treats sorry as infinite difficulty, bins by percentile, and
sorts repositories by easy theorem counts." Here we make it a small typed policy
module (per the report's P1 "make it a typed policy module rather than global
script logic"):

* :func:`difficulty` -- ``exp(#tactic_steps)``; ``inf`` if the proof contains a
  ``sorry``/``admit`` placeholder; ``None`` if there is no proof at all.
* :func:`curriculum_tiers` -- percentile-bucket a list of difficulties into
  ``Easy`` / ``Medium`` / ``Hard`` (terciles over the finite values; ``inf`` is
  always ``Hard``; ``None`` is ``unknown``).
* :func:`order_by_curriculum` -- sort a benchmark item list easy -> hard, to
  feed curriculum-ordered runs and MCTS budgeting.

The exponential makes step count dominate: a 2-tactic proof (``e^2 ~ 7.4``) is
an order of magnitude easier than a 5-tactic one (``e^5 ~ 148``), which matches
the intuition that each extra required tactic multiplies search difficulty.
"""
from __future__ import annotations

import json
import math
import re
import sys
from typing import Any, Callable, Iterable

EASY = "Easy"
MEDIUM = "Medium"
HARD = "Hard"
UNKNOWN = "unknown"
TIERS = (EASY, MEDIUM, HARD)

_SORRY_TOKENS = ("sorry", "admit", "sorryax")

# Strip Lean comments before counting tactic steps.
_LINE_COMMENT = re.compile(r"--[^\n]*")
_BLOCK_COMMENT = re.compile(r"/-.*?-/", re.DOTALL)

# Tactic separators: newlines, ``;`` and Lean's ``<;>`` combinator.
_SEP = re.compile(r"<;>|;|[\r\n]+")

# Structural-only tokens that are not themselves tactic steps.
_STRUCTURAL = {"by", ":=", ":= by", "{", "}", "·", "•", "(", ")", "[", "]", "do", "=>"}


def _as_text(proof_or_steps: Any) -> str | None:
    """Join a step list into text, or return the string, or None when absent."""
    if proof_or_steps is None:
        return None
    if isinstance(proof_or_steps, (list, tuple)):
        joined = "\n".join(str(s) for s in proof_or_steps if str(s).strip())
        return joined or None
    text = str(proof_or_steps)
    return text if text.strip() else None


def _has_sorry(text: str) -> bool:
    low = text.lower()
    return any(re.search(rf"\b{re.escape(tok)}\b", low) for tok in _SORRY_TOKENS)


def count_tactic_steps(proof_or_steps: Any) -> int:
    """Count tactic steps in a proof (or take ``len`` of an explicit step list).

    Comments are stripped; the body is split on newlines, ``;`` and ``<;>``;
    empty and purely-structural tokens (``by``, braces, focus dots) are dropped.
    """
    if isinstance(proof_or_steps, (list, tuple)):
        return sum(1 for s in proof_or_steps if str(s).strip())
    text = _as_text(proof_or_steps)
    if text is None:
        return 0
    text = _BLOCK_COMMENT.sub(" ", text)
    text = _LINE_COMMENT.sub(" ", text)
    # Drop a leading `theorem ... := by` / `by` header token so only tactics count.
    text = re.sub(r":=\s*by\b", "\n", text)
    count = 0
    for tok in _SEP.split(text):
        tok = tok.strip()
        if not tok or tok in _STRUCTURAL:
            continue
        # A bare `by` or focus dot with nothing else is structural.
        if tok in {"by"} or set(tok) <= {"·", "•", "{", "}"}:
            continue
        count += 1
    return count


def difficulty(proof_or_steps: Any) -> float | None:
    """Difficulty ``= exp(#tactic_steps)``.

    * ``None`` when there is no proof (``None`` / empty string / empty list).
    * ``math.inf`` when the proof still contains a ``sorry``/``admit`` placeholder
      (an unproved obligation is maximally hard).
    * otherwise ``math.exp(count_tactic_steps(...))``.
    """
    text = _as_text(proof_or_steps)
    if text is None:
        return None
    if _has_sorry(text):
        return math.inf
    return math.exp(count_tactic_steps(proof_or_steps))


# --------------------------------------------------------------------------- #
# Percentile bucketing
# --------------------------------------------------------------------------- #

def _percentile(sorted_vals: list[float], q: float) -> float:
    """Linear-interpolation percentile (q in [0, 1]) over pre-sorted values."""
    if not sorted_vals:
        return math.nan
    if len(sorted_vals) == 1:
        return sorted_vals[0]
    pos = q * (len(sorted_vals) - 1)
    lo = math.floor(pos)
    hi = math.ceil(pos)
    if lo == hi:
        return sorted_vals[int(pos)]
    frac = pos - lo
    return sorted_vals[lo] * (1 - frac) + sorted_vals[hi] * frac


def curriculum_tiers(difficulties: Iterable[float | None]) -> list[str]:
    """Bucket difficulties into Easy/Medium/Hard by tercile percentiles.

    Finite difficulties are split at the 33rd/66th percentiles of the finite
    population. ``inf`` is always ``Hard``; ``None`` is ``unknown``. Ties on a
    threshold fall into the lower tier (``<=``).
    """
    diffs = list(difficulties)
    finite = sorted(d for d in diffs if d is not None and math.isfinite(d))
    if finite:
        t1 = _percentile(finite, 1 / 3)
        t2 = _percentile(finite, 2 / 3)
    else:
        t1 = t2 = math.inf

    out: list[str] = []
    for d in diffs:
        if d is None:
            out.append(UNKNOWN)
        elif not math.isfinite(d):
            out.append(HARD)
        elif d <= t1:
            out.append(EASY)
        elif d <= t2:
            out.append(MEDIUM)
        else:
            out.append(HARD)
    return out


# --------------------------------------------------------------------------- #
# Item ordering
# --------------------------------------------------------------------------- #

def _default_proof_key(item: Any) -> Any:
    """Best-effort extraction of a proof/step payload from a benchmark item."""
    if isinstance(item, str) or isinstance(item, (list, tuple)):
        return item
    if not isinstance(item, dict):
        return None
    expected = item.get("expected") or {}
    for src in (
        item.get("proof"),
        item.get("steps"),
        item.get("reference_proof"),
        item.get("solution_proof"),
        expected.get("proof"),
        expected.get("reference_proof"),
        item.get("formal"),
        expected.get("formal_statement"),
    ):
        if src:
            return src
    return None


def _sort_key(diff: float | None) -> tuple[int, float]:
    """Sort key: finite (asc) < inf < no-proof(None) last, all stable."""
    if diff is None:
        return (2, 0.0)
    if not math.isfinite(diff):
        return (1, 0.0)
    return (0, diff)


def order_by_curriculum(
    items: list[Any],
    key: Callable[[Any], Any] | None = None,
) -> list[Any]:
    """Return ``items`` sorted easy -> hard by proof difficulty (stable).

    ``key`` extracts the proof/steps payload from each item (default:
    :func:`_default_proof_key`). Items with no proof sort last; ``sorry``-bearing
    items sort just before them (after all finite difficulties).
    """
    key = key or _default_proof_key
    decorated = [(difficulty(key(it)), i, it) for i, it in enumerate(items)]
    decorated.sort(key=lambda t: (_sort_key(t[0]), t[1]))
    return [it for _, _, it in decorated]


def annotate_curriculum(
    items: list[Any],
    key: Callable[[Any], Any] | None = None,
) -> list[dict[str, Any]]:
    """Annotate each item with its difficulty + tier, in easy->hard order.

    Tiers are computed over the whole population (so they are stable regardless
    of ordering), then rows are returned sorted easy -> hard.
    """
    key = key or _default_proof_key
    diffs = [difficulty(key(it)) for it in items]
    tiers = curriculum_tiers(diffs)
    rows = [
        {"item": it, "difficulty": d, "tier": t}
        for it, d, t in zip(items, diffs, tiers)
    ]
    # Stable easy->hard sort; the enumerate index breaks ties in input order.
    decorated = list(enumerate(rows))
    decorated.sort(key=lambda pair: (_sort_key(pair[1]["difficulty"]), pair[0]))
    return [row for _, row in decorated]


# --------------------------------------------------------------------------- #
# JSON dispatch (worker hook) + CLI
# --------------------------------------------------------------------------- #

def run(request: dict[str, Any]) -> dict[str, Any]:
    op = request.get("op", "difficulty")
    if op == "difficulty":
        return {
            "op": "difficulty",
            "difficulty": difficulty(request.get("proof", request.get("steps"))),
            "tactic_steps": count_tactic_steps(
                request.get("proof", request.get("steps"))
            ),
        }
    if op == "curriculum_tiers":
        diffs = request.get("difficulties")
        if diffs is None:
            diffs = [difficulty(p) for p in request.get("proofs", [])]
        return {"op": "curriculum_tiers", "tiers": curriculum_tiers(diffs)}
    if op == "order_by_curriculum":
        items = request.get("items", [])
        ordered = order_by_curriculum(items)
        return {
            "op": "order_by_curriculum",
            "n": len(ordered),
            "ordered": ordered,
            "difficulties": [difficulty(_default_proof_key(it)) for it in ordered],
        }
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
