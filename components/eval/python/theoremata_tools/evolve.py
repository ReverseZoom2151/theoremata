"""Open-problem mode: AlphaEvolve / Co-Scientist generate-debate-evolve loop
(Theoremata plan section 14).

An evolutionary *program database* over candidate proofs / constructions. The
model calls (fast generator) and the strong evaluator (in real use: run Lean and
score by pass + proof-length / generality) are all INJECTED as callables, so the
orchestration skeleton is fully testable offline with no model and no GPU.

Core pieces
-----------
* ``ProgramDatabase`` -- an in-memory population of scored candidates
  ``{id, code, score, generation, parent_id, verdict}`` with quality-and-diversity
  parent sampling.
* ``evolve(...)`` -- the AlphaEvolve/Co-Scientist loop: each round samples parents
  from the database, fans out ``fanout`` children via ``generate``, scores each via
  ``evaluate``, adds them, and keeps the top ``keep`` (multi-objective: higher score,
  then shorter code). Tracks best-so-far with monotonic improvement.
* ``debate(...)`` -- a Chain/Graph-of-Debates helper. ``argue_for`` / ``argue_against``
  are injected, and a candidate is "supported" only when the ``ground`` check (a real
  verifiable sub-result) passes: the plan's "well-supported = verifiable ground truth".

``run(request)`` / ``main()`` follow the JSON-stdio convention (see ``grader.py``) and
accept deterministic generate/evaluate specs so the loop is invocable without a model.
"""
from __future__ import annotations

import json
import os
import sys
from typing import Any, Callable, Optional

# Injected-callable type aliases (documentation only).
GenerateFn = Callable[[list[dict[str, Any]], int], list[str]]
EvaluateFn = Callable[[str], dict[str, Any]]
JudgeFn = Callable[[list[dict[str, Any]]], list[float]]


def _candidate_sort_key(cand: dict[str, Any]) -> tuple[float, int]:
    """Multi-objective ordering key for "best": prefer higher score, then shorter
    code. Returned so ``sorted(..., reverse=True)`` puts the best first."""
    return (float(cand.get("score", 0.0)), -len(cand.get("code", "")))


class ProgramDatabase:
    """A scored population of candidate programs (proofs / constructions).

    Each candidate is a dict ``{id, code, score, generation, parent_id, verdict}``.
    The database assigns stable integer ids and never mutates candidates in place.
    """

    def __init__(self) -> None:
        self._candidates: list[dict[str, Any]] = []
        self._next_id = 0

    def add(self, candidate: dict[str, Any]) -> dict[str, Any]:
        """Insert a candidate, assigning an id when absent. Returns the stored
        record (a normalized copy)."""
        stored = {
            "id": candidate.get("id") if candidate.get("id") is not None else self._next_id,
            "code": candidate.get("code", ""),
            "score": float(candidate.get("score", 0.0)),
            "generation": int(candidate.get("generation", 0)),
            "parent_id": candidate.get("parent_id"),
            "verdict": candidate.get("verdict"),
        }
        if candidate.get("id") is None:
            self._next_id += 1
        else:
            # keep _next_id ahead of any explicitly supplied id
            try:
                self._next_id = max(self._next_id, int(candidate["id"]) + 1)
            except (TypeError, ValueError):
                pass
        self._candidates.append(stored)
        return stored

    def size(self) -> int:
        return len(self._candidates)

    def all(self) -> list[dict[str, Any]]:
        return list(self._candidates)

    def best(self, k: int = 1) -> list[dict[str, Any]]:
        """Return the top ``k`` candidates by (score desc, code-length asc)."""
        ordered = sorted(self._candidates, key=_candidate_sort_key, reverse=True)
        return ordered[: max(0, k)]

    def sample_parents(self, k: int, *, rng: Optional[Any] = None) -> list[dict[str, Any]]:
        """Sample ``k`` parents, favoring higher score while keeping diversity.

        Deterministic and rng-free by construction: candidates are ranked by the
        multi-objective key, then picked by striding across the ranking so that
        both elite and mid-population individuals seed the next generation (an
        island/quality-diversity flavor). This avoids collapsing onto a single
        elite parent, which is what kills exploration in evolutionary search.
        """
        if not self._candidates or k <= 0:
            return []
        ordered = sorted(self._candidates, key=_candidate_sort_key, reverse=True)
        n = len(ordered)
        if k >= n:
            return ordered

        if rng is not None:
            # Optional stochastic mode: weight by rank so higher score is likelier.
            weights = [float(n - i) for i in range(n)]
            chosen: list[dict[str, Any]] = []
            pool = list(range(n))
            pool_w = list(weights)
            for _ in range(k):
                total = sum(pool_w)
                r = rng.random() * total
                acc = 0.0
                idx = 0
                for j, w in enumerate(pool_w):
                    acc += w
                    if r <= acc:
                        idx = j
                        break
                chosen.append(ordered[pool[idx]])
                del pool[idx]
                del pool_w[idx]
            return chosen

        # Deterministic elite + strided-diversity selection.
        picks: list[dict[str, Any]] = [ordered[0]]  # always keep the current elite
        remaining = k - 1
        if remaining > 0:
            # stride across the rest of the ranking
            step = max(1, (n - 1) // remaining)
            i = 1
            while len(picks) < k and i < n:
                picks.append(ordered[i])
                i += step
            # top up from the front if striding fell short
            j = 1
            while len(picks) < k and j < n:
                if ordered[j] not in picks:
                    picks.append(ordered[j])
                j += 1
        return picks[:k]


def evolve(
    seed_candidates: list[dict[str, Any]] | list[str],
    *,
    generate: GenerateFn,
    evaluate: EvaluateFn,
    rounds: int,
    fanout: int,
    keep: int,
    judge: Optional[JudgeFn] = None,
    rng: Optional[Any] = None,
) -> dict[str, Any]:
    """Run the AlphaEvolve / Co-Scientist generate-evaluate-evolve loop.

    Parameters
    ----------
    seed_candidates:
        Initial programs. Each may be a raw code string or a
        ``{code, score?, verdict?}`` dict. Seeds are evaluated if unscored.
    generate(parents, n) -> list[str]:
        Fast model: fans out ``n`` new candidate program strings from ``parents``
        (each parent is a full candidate dict).
    evaluate(code) -> {"score": float, "verdict": str}:
        Strong evaluator. In real use this runs Lean and scores by pass plus a
        proof-length / generality term; here it is injected.
    rounds, fanout, keep:
        Number of generations; children generated per round; population cap kept
        after each round (top ``keep`` by score then shorter code).
    judge(candidates) -> list[float]:
        Optional Co-Scientist Elo-style tournament ranking, used to break ties /
        re-rank the final leaderboard.

    Returns a dict:
        ``{ok, rounds, evaluated, best:{code,score,verdict}, history:[best_score_per_round],
           database_size}``.
    """
    db = ProgramDatabase()
    evaluated = 0

    def _eval(code: str) -> dict[str, Any]:
        nonlocal evaluated
        result = evaluate(code)
        evaluated += 1
        return {
            "score": float(result.get("score", 0.0)),
            "verdict": result.get("verdict"),
        }

    # ---- seed the database ----
    for seed in seed_candidates:
        if isinstance(seed, str):
            code, score, verdict = seed, None, None
        else:
            code = seed.get("code", "")
            score = seed.get("score")
            verdict = seed.get("verdict")
        if score is None:
            scored = _eval(code)
            score, verdict = scored["score"], scored["verdict"]
        db.add(
            {
                "code": code,
                "score": float(score),
                "generation": 0,
                "parent_id": None,
                "verdict": verdict,
            }
        )

    best_so_far = db.best(1)[0] if db.size() else None
    history: list[float] = []

    # ---- evolution rounds ----
    for r in range(1, rounds + 1):
        parents = db.sample_parents(max(1, keep), rng=rng)
        children_code = generate(parents, fanout) or []
        # parent id lineage: attribute each child to a parent round-robin.
        for i, code in enumerate(children_code):
            parent = parents[i % len(parents)] if parents else None
            scored = _eval(code)
            db.add(
                {
                    "code": code,
                    "score": scored["score"],
                    "generation": r,
                    "parent_id": parent["id"] if parent else None,
                    "verdict": scored["verdict"],
                }
            )

        # keep the top-`keep` population (multi-objective).
        survivors = db.best(keep)
        new_db = ProgramDatabase()
        for cand in survivors:
            new_db.add(cand)
        db = new_db

        round_best = db.best(1)[0]
        # best-so-far is monotonic: never regress below a previously seen best.
        if best_so_far is None or _candidate_sort_key(round_best) > _candidate_sort_key(best_so_far):
            best_so_far = round_best
        history.append(float(best_so_far["score"]))

    # ---- optional Elo-style tournament to break ties on the leaderboard ----
    if judge is not None and db.size():
        leaderboard = db.best(db.size())
        elo = judge(leaderboard)
        ranked = sorted(
            zip(leaderboard, elo),
            key=lambda pair: (pair[1], _candidate_sort_key(pair[0])),
            reverse=True,
        )
        top = ranked[0][0]
        # judge only re-ranks among equally-scored best candidates; never let it
        # demote a strictly higher objective score.
        if best_so_far is None or float(top["score"]) >= float(best_so_far["score"]):
            best_so_far = top

    best_out = (
        {
            "code": best_so_far["code"],
            "score": float(best_so_far["score"]),
            "verdict": best_so_far["verdict"],
        }
        if best_so_far is not None
        else None
    )

    return {
        "ok": True,
        "rounds": rounds,
        "evaluated": evaluated,
        "best": best_out,
        "history": history,
        "database_size": db.size(),
    }


def debate(
    candidate: dict[str, Any] | str,
    *,
    argue_for: Callable[[dict[str, Any] | str], str],
    argue_against: Callable[[dict[str, Any] | str], str],
    ground: Callable[[dict[str, Any] | str], bool],
) -> dict[str, Any]:
    """Chain / Graph-of-Debates over a single candidate.

    ``argue_for`` and ``argue_against`` are injected callables that produce
    rationale strings for and against the candidate. Crucially, the verdict is
    NOT decided by the debate rhetoric: a candidate is ``supported`` only when the
    injected ``ground`` check -- a real, verifiable sub-result (e.g. a passing Lean
    lemma) -- returns True. This is the plan's "well-supported = verifiable ground
    truth": arguments contextualize, evidence decides.
    """
    pro = argue_for(candidate)
    con = argue_against(candidate)
    grounded = bool(ground(candidate))
    return {
        "supported": grounded,
        "grounded": grounded,
        "rationale": {
            "for": pro,
            "against": con,
            "verdict": (
                "supported: backed by a passing ground check"
                if grounded
                else "unsupported: no verifiable ground truth"
            ),
        },
    }


# ---------------------------------------------------------------------------
# Deterministic (model-free) generate/evaluate specs for run()/main()
# ---------------------------------------------------------------------------

def _spec_evaluate(spec: dict[str, Any]) -> EvaluateFn:
    """Build a deterministic ``evaluate`` from a JSON spec.

    Supported specs (no model required):
      * ``{"kind": "target_len", "target": N}`` -- score by closeness of
        ``len(code)`` to ``N`` (higher is closer), verdict "pass" on exact match.
      * ``{"kind": "substring", "needle": "..."}`` -- score = fraction of the
        needle contained as a prefix; 1.0 (verdict "pass") when the needle is a
        substring of the code.
      * ``{"kind": "map", "scores": {code: score}}`` -- explicit lookup table.
    """
    kind = spec.get("kind", "target_len")

    if kind == "target_len":
        target = int(spec.get("target", 0))

        def _ev(code: str) -> dict[str, Any]:
            dist = abs(len(code) - target)
            score = 1.0 / (1.0 + dist)
            return {"score": score, "verdict": "pass" if dist == 0 else "fail"}

        return _ev

    if kind == "substring":
        needle = str(spec.get("needle", ""))

        def _ev(code: str) -> dict[str, Any]:
            if needle and needle in code:
                return {"score": 1.0, "verdict": "pass"}
            # partial credit: longest matching prefix of needle
            best = 0
            for i in range(len(needle), 0, -1):
                if needle[:i] in code:
                    best = i
                    break
            score = (best / len(needle)) if needle else 0.0
            return {"score": score, "verdict": "fail"}

        return _ev

    if kind == "map":
        table = {str(k): float(v) for k, v in dict(spec.get("scores", {})).items()}

        def _ev(code: str) -> dict[str, Any]:
            score = table.get(code, 0.0)
            return {"score": score, "verdict": "pass" if score >= 1.0 else "fail"}

        return _ev

    raise ValueError(f"unknown evaluate spec kind: {kind}")


def _spec_generate(spec: dict[str, Any]) -> GenerateFn:
    """Build a deterministic ``generate`` from a JSON spec.

    Supported specs (no model required):
      * ``{"kind": "grow", "toward": "..."}`` -- each child extends a parent one
        character closer to the ``toward`` string (a deterministic hill-climb).
      * ``{"kind": "append", "alphabet": "abc"}`` -- append successive alphabet
        characters to parents.
    """
    kind = spec.get("kind", "grow")

    if kind == "grow":
        toward = str(spec.get("toward", ""))

        def _gen(parents: list[dict[str, Any]], n: int) -> list[str]:
            out: list[str] = []
            for i in range(n):
                parent = parents[i % len(parents)] if parents else {"code": ""}
                code = str(parent.get("code", ""))
                if len(code) < len(toward) and toward.startswith(code):
                    child = toward[: len(code) + 1]
                else:
                    child = toward if toward else code
                out.append(child)
            return out

        return _gen

    if kind == "append":
        alphabet = str(spec.get("alphabet", "abcdefghijklmnopqrstuvwxyz"))

        def _gen(parents: list[dict[str, Any]], n: int) -> list[str]:
            out: list[str] = []
            for i in range(n):
                parent = parents[i % len(parents)] if parents else {"code": ""}
                code = str(parent.get("code", ""))
                ch = alphabet[len(code) % len(alphabet)] if alphabet else ""
                out.append(code + ch)
            return out

        return _gen

    raise ValueError(f"unknown generate spec kind: {kind}")


# ---------------------------------------------------------------------------
# Dispatch / CLI (JSON-stdio convention, see grader.py)
# ---------------------------------------------------------------------------

def run(request: dict[str, Any]) -> dict[str, Any]:
    op = request.get("op", "evolve")
    if op == "evolve":
        generate = _spec_generate(request.get("generate", {"kind": "grow", "toward": ""}))
        evaluate = _spec_evaluate(request.get("evaluate", {"kind": "target_len", "target": 0}))
        return evolve(
            request.get("seed_candidates", request.get("seeds", [])),
            generate=generate,
            evaluate=evaluate,
            rounds=int(request.get("rounds", 1)),
            fanout=int(request.get("fanout", 1)),
            keep=int(request.get("keep", 1)),
        )
    if op == "debate":
        # Deterministic debate: rationale strings are echoed, ground is a literal
        # boolean (in real use, a Lean/verifier sub-result).
        cand = request.get("candidate", "")
        grounded = bool(request.get("ground", False))
        return debate(
            cand,
            argue_for=lambda c: str(request.get("for", "argument in favor")),
            argue_against=lambda c: str(request.get("against", "argument against")),
            ground=lambda c: grounded,
        )
    raise ValueError(f"unknown op: {op}")


def main() -> None:
    if len(sys.argv) >= 2 and os.path.exists(sys.argv[1]):
        with open(sys.argv[1], encoding="utf-8") as fh:
            request = json.load(fh)
    else:
        request = json.load(sys.stdin)
    print(json.dumps(run(request), indent=2, default=str))
    raise SystemExit(0)


if __name__ == "__main__":
    main()
