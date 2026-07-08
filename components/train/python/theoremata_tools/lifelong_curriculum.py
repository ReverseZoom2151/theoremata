"""LeanAgent lifelong-learning curriculum: repo binning + resumable prove-loop.

Extends the item-level difficulty/tier curriculum (in the ``eval`` component)
with LeanAgent's *real* training-time policy
(``docs/resource-mining/reverify/LeanCopilot-LeanAgent-LeanProgress.md``,
``leanagent.py:842-909``):

* **Percentile binning** -- compute 33rd/67th percentile thresholds over all
  *finite* difficulties and bin each theorem Easy / Medium / Hard. ``sorry``-bearing
  proofs are ``inf`` (always Hard); proofs with no steps are ``None`` (deferred).
* **Round-robin distribution of no-proof items** -- the ``None`` /
  ``To_Distribute`` theorems are spread round-robin across the three real buckets
  (not dumped into one), so unproved theorems seed every difficulty tier.
* **Easy-count repo ordering** -- repositories are sorted by their count of Easy
  theorems, descending, so the agent starts where it has the most footholds.

Plus LeanAgent's **resumable-loop primitives** (``leanagent.py:639-739``): a
``theorem_identifier`` dedup key, a pickled/JSON encountered-set, and
checkpoint/restore, so a lifelong run restarts safely without re-processing.

Difficulty here is ``exp(#steps)`` with ``sorry -> inf`` and ``no proof -> None``,
matching the mined formula; this module owns the *policy*, not the tokenizer.
"""
from __future__ import annotations

import json
import math
import re
import sys
from dataclasses import dataclass, field
from typing import Any, Iterable, Optional, Sequence

EASY = "Easy"
MEDIUM = "Medium"
HARD = "Hard"
BUCKETS = (EASY, MEDIUM, HARD)

_SORRY = re.compile(r"\b(sorry|admit)\b", re.IGNORECASE)
_SEP = re.compile(r"<;>|;|[\r\n]+")


def _count_steps(proof_or_steps: Any) -> int:
    if isinstance(proof_or_steps, (list, tuple)):
        return sum(1 for s in proof_or_steps if str(s).strip())
    text = str(proof_or_steps)
    text = re.sub(r":=\s*by\b", "\n", text)
    return sum(1 for tok in _SEP.split(text) if tok.strip() and tok.strip() != "by")


def difficulty(proof_or_steps: Any) -> Optional[float]:
    """``exp(#steps)``; ``inf`` if the proof contains ``sorry``/``admit``;
    ``None`` if there is no proof at all (empty / whitespace / empty list)."""
    if proof_or_steps is None:
        return None
    if isinstance(proof_or_steps, (list, tuple)):
        if not any(str(s).strip() for s in proof_or_steps):
            return None
        text = "\n".join(str(s) for s in proof_or_steps)
    else:
        text = str(proof_or_steps)
        if not text.strip():
            return None
    if _SORRY.search(text):
        return math.inf
    return math.exp(_count_steps(proof_or_steps))


def _percentile(sorted_vals: list[float], q: float) -> float:
    """numpy-style linear-interpolation percentile, ``q`` in ``[0, 1]``."""
    if not sorted_vals:
        return math.nan
    if len(sorted_vals) == 1:
        return sorted_vals[0]
    pos = q * (len(sorted_vals) - 1)
    lo = math.floor(pos)
    hi = math.ceil(pos)
    if lo == hi:
        return sorted_vals[int(pos)]
    return sorted_vals[lo] * (hi - pos) + sorted_vals[hi] * (pos - lo)


def bin_theorems(difficulties: Sequence[Optional[float]]) -> list[str]:
    """LeanAgent percentile binning + round-robin None distribution.

    Finite difficulties are split at the 33rd/67th percentiles into
    Easy/Medium/Hard; ``inf`` is always Hard. ``None`` (no-proof) theorems are
    then distributed **round-robin** across Easy/Medium/Hard (in input order),
    so every bucket receives a share. Returns a bucket per input, aligned.
    """
    diffs = list(difficulties)
    finite = sorted(d for d in diffs if d is not None and math.isfinite(d))
    if finite:
        p33 = _percentile(finite, 0.33)
        p67 = _percentile(finite, 0.67)
    else:
        p33 = p67 = math.inf

    out: list[Optional[str]] = []
    for d in diffs:
        if d is None:
            out.append(None)  # To_Distribute, filled below
        elif not math.isfinite(d):
            out.append(HARD)
        elif d <= p33:
            out.append(EASY)
        elif d <= p67:
            out.append(MEDIUM)
        else:
            out.append(HARD)

    # round-robin the To_Distribute (None) theorems across the three buckets.
    rr = 0
    for i, b in enumerate(out):
        if b is None:
            out[i] = BUCKETS[rr % 3]
            rr += 1
    return [b for b in out]  # type: ignore[misc]


@dataclass
class RepoCurriculum:
    """A repository's theorems with their assigned buckets and easy-count."""

    name: str
    buckets: list[str]

    @property
    def easy_count(self) -> int:
        return self.buckets.count(EASY)


def _theorem_proof(theorem: Any) -> Any:
    if isinstance(theorem, dict):
        if "difficulty" in theorem:
            return theorem  # marker; handled by caller
        return theorem.get("proof", theorem.get("steps"))
    return theorem


def _theorem_difficulty(theorem: Any) -> Optional[float]:
    if isinstance(theorem, dict) and "difficulty" in theorem:
        d = theorem["difficulty"]
        return None if d is None else float(d)
    return difficulty(_theorem_proof(theorem))


def sort_repositories(repos: Sequence[dict[str, Any]]) -> list[dict[str, Any]]:
    """Sort repositories easy-first by their count of Easy theorems (descending).

    Each repo is ``{"name", "theorems": [...]}`` where a theorem carries either a
    ``proof``/``steps`` payload or a precomputed ``difficulty``. Bins are computed
    **globally** across all repos' theorems (one shared percentile scale), then
    repos are ordered by ``easy_count`` desc, ties broken by name for stability.

    Returns the repos annotated: ``{name, theorems, buckets, easy_count}``.
    """
    all_diffs: list[Optional[float]] = []
    spans: list[tuple[str, int, int]] = []  # (name, start, end) into all_diffs
    for repo in repos:
        start = len(all_diffs)
        for t in repo.get("theorems", []):
            all_diffs.append(_theorem_difficulty(t))
        spans.append((repo["name"], start, len(all_diffs)))

    buckets = bin_theorems(all_diffs)
    annotated: list[dict[str, Any]] = []
    for repo, (name, start, end) in zip(repos, spans):
        repo_buckets = buckets[start:end]
        annotated.append(
            {
                "name": name,
                "theorems": repo.get("theorems", []),
                "buckets": repo_buckets,
                "easy_count": repo_buckets.count(EASY),
            }
        )
    annotated.sort(key=lambda r: (-r["easy_count"], r["name"]))
    return annotated


# ---------------------------------------------------------------------------
# Resumable prove-loop primitives (LeanAgent lifelong learning)
# ---------------------------------------------------------------------------

def theorem_identifier(theorem: dict[str, Any]) -> tuple[str, str, Any, Any]:
    """LeanAgent dedup key ``(full_name, file_path, start, end)``. Positions are
    coerced to tuples so the key is hashable regardless of list/tuple input."""

    def _pos(p: Any) -> Any:
        return tuple(p) if isinstance(p, (list, tuple)) else p

    return (
        str(theorem.get("full_name", "")),
        str(theorem.get("file_path", "")),
        _pos(theorem.get("start")),
        _pos(theorem.get("end")),
    )


@dataclass
class ResumableCurriculum:
    """A restart-safe curriculum cursor: dedups already-encountered theorems and
    checkpoints its encountered-set (LeanAgent ``prove_sorry_theorems``)."""

    encountered: set[tuple] = field(default_factory=set)

    def seen(self, theorem: dict[str, Any]) -> bool:
        return theorem_identifier(theorem) in self.encountered

    def mark(self, theorem: dict[str, Any]) -> None:
        self.encountered.add(theorem_identifier(theorem))

    def filter_new(self, theorems: Iterable[dict[str, Any]]) -> list[dict[str, Any]]:
        """Return only theorems not yet encountered, marking them as encountered.
        Also dedups within the same call."""
        fresh: list[dict[str, Any]] = []
        for t in theorems:
            if self.seen(t):
                continue
            self.mark(t)
            fresh.append(t)
        return fresh

    def checkpoint(self) -> dict[str, Any]:
        """Serialize the encountered-set to a JSON-safe checkpoint."""
        return {"encountered": [list(k) for k in sorted(self.encountered, key=repr)]}

    @classmethod
    def restore(cls, state: dict[str, Any]) -> "ResumableCurriculum":
        enc = {
            tuple(tuple(x) if isinstance(x, list) else x for x in k)
            for k in state.get("encountered", [])
        }
        return cls(encountered=enc)


def run(request: dict[str, Any]) -> dict[str, Any]:
    op = request.get("op", "bin")
    if op == "bin":
        diffs = request.get("difficulties")
        if diffs is None:
            diffs = [difficulty(p) for p in request.get("proofs", [])]
        return {"op": "bin", "buckets": bin_theorems(diffs)}
    if op == "sort_repositories":
        return {"op": "sort_repositories", "repos": sort_repositories(request["repos"])}
    if op == "filter_new":
        cur = ResumableCurriculum.restore(request.get("state", {}))
        fresh = cur.filter_new(request.get("theorems", []))
        return {"op": "filter_new", "fresh": fresh, "checkpoint": cur.checkpoint()}
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
