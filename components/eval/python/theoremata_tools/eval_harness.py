"""Theoremata evaluation harness (plan section 11).

Runs the existing six-axis grader (``theoremata_tools.grader``) over benchmark
problem sets that carry *provided* candidate generations, and reports
pass@k / majority@k / averaged@k with the six axes kept SEPARATE (never folded
into one blended scalar) plus contamination controls.

This module never calls a model. It grades a caller-supplied
``generations[problem_id] -> [candidate answer, ...]`` mapping, so evaluation is
deterministic and testable offline.

Key ideas
---------
* ``load_problems`` normalizes heterogeneous benchmark records (IneqMath JSON,
  a generic AIME-style JSONL, FormalQualBench Lean stubs) into one superset
  record schema.
* ``evaluate`` refuses to score anything whose ``usage_tag != "test"`` (honors a
  strict test-only contract), grades the rest through the grader, and folds each
  problem into a per-problem ``six_axis`` record.
* Aggregates carry a standard error ``std(ddof=1)/sqrt(n)`` and are broken down
  by ``difficulty_tier`` and by the six axes independently.
* Contamination controls: n-gram overlap against the gold solution, a
  "recalled answer" smell test (high answer-acc + low step-acc), and a
  ``freshness_tier`` heuristic.
"""
from __future__ import annotations

import json
import math
import os
import re
import statistics
import sys
from collections import Counter
from typing import Any

from .grader import grade_samples, six_axis

AXES = ("discovery", "informal", "formal", "soundness", "efficiency", "novelty")

# ---------------------------------------------------------------------------
# Loading / normalization
# ---------------------------------------------------------------------------

_INT_RE = re.compile(r"^[+-]?\d+$")


def _looks_integer(value: Any) -> bool:
    if value is None:
        return False
    s = str(value).strip()
    # strip a leading option label like "(B) " and $...$ wrappers
    s = re.sub(r"^\([A-Fa-f]\)\s*", "", s)
    s = s.replace("$", "").strip()
    return bool(_INT_RE.match(s))


def _join_solution(sol: Any) -> str | None:
    if sol is None:
        return None
    if isinstance(sol, list):
        return "\n\n".join(str(s) for s in sol)
    return str(sol)


def freshness_tier(problem: dict[str, Any]) -> str:
    """Contamination-freshness heuristic for a normalized problem.

    Newer competitions are less likely to be in pretraining corpora. Returns one
    of ``"low"`` / ``"medium"`` / ``"high"`` risk. Honors an explicit
    ``contamination_risk`` field first, then sniffs the id/problem text for a
    competition year (AIME 2026 = low, 2024/2025 = high)."""
    explicit = problem.get("contamination_risk")
    if explicit in {"low", "medium", "high"}:
        return explicit
    hay = f"{problem.get('id', '')} {problem.get('source', '')}".lower()
    # look at the problem body too, but weight the id/source first
    body = f"{hay} {str(problem.get('problem', ''))[:200]}".lower()
    if any(t in body for t in ("aime26", "aime 26", "2026")):
        return "low"
    if any(t in body for t in ("aime25", "aime 25", "2025", "aime24", "aime 24", "2024")):
        return "high"
    return "medium"


def _normalize_record(raw: dict[str, Any]) -> dict[str, Any]:
    """Fold one raw benchmark record into the superset schema."""
    rid = (
        raw.get("id")
        or raw.get("data_id")
        or raw.get("problem_id")
        or raw.get("uid")
    )
    if rid is None:
        rid = f"anon-{abs(hash(json.dumps(raw, sort_keys=True, default=str))) % (10**8)}"
    rid = str(rid)

    problem = raw.get("problem") or raw.get("question") or raw.get("statement") or ""
    raw_type = raw.get("type")
    lean_stub = raw.get("lean_stub") or raw.get("lean") or raw.get("formal_statement")

    # grade_kind: answer-style vs proof-style
    grade_kind = raw.get("grade_kind")
    if grade_kind not in {"answer", "proof"}:
        if lean_stub or raw_type == "proof":
            grade_kind = "proof"
        else:
            grade_kind = "answer"

    answer = raw.get("answer")
    if answer is None:
        answer = raw.get("gold") or raw.get("final_answer") or raw.get("solution_answer")

    # answer_kind drives the grader routing (integer|relation|symbolic)
    answer_kind = raw.get("answer_kind")
    if answer_kind not in {"integer", "relation", "symbolic"}:
        if raw_type == "relation":
            answer_kind = "relation"
        elif raw_type == "bound":
            answer_kind = "symbolic"
        elif _looks_integer(answer):
            answer_kind = "integer"
        else:
            answer_kind = "symbolic"

    # usage_tag: train|test|retrieval (honor test-only downstream)
    usage_tag = raw.get("usage_tag")
    if usage_tag not in {"train", "test", "retrieval"}:
        split = str(raw.get("data_split") or raw.get("split") or "").lower()
        if split in {"train", "training"}:
            usage_tag = "train"
        elif split in {"retrieval", "corpus", "few_shot", "fewshot"}:
            usage_tag = "retrieval"
        elif split in {"test", "dev", "val", "validation", "eval"}:
            usage_tag = "test"
        else:
            usage_tag = "test"  # default: benchmark items are scored unless told otherwise

    difficulty_tier = raw.get("difficulty_tier") or raw.get("difficulty") or ""
    difficulty_tier = str(difficulty_tier).strip() or "unknown"

    record = {
        "id": rid,
        "problem": problem,
        "grade_kind": grade_kind,
        "answer_kind": answer_kind,
        "answer": None if answer is None else str(answer),
        "choices": raw.get("choices"),
        "gold_solution": _join_solution(raw.get("gold_solution") or raw.get("solution")),
        "lean_stub": lean_stub,
        "usage_tag": usage_tag,
        "contamination_risk": raw.get("contamination_risk"),
        "difficulty_tier": difficulty_tier,
        "source": raw.get("source"),
    }
    # fill contamination_risk from the freshness heuristic when unset
    if record["contamination_risk"] not in {"low", "medium", "high"}:
        record["contamination_risk"] = freshness_tier(record)
    return record


def load_problems(path_or_records: Any, fmt: str = "auto") -> list[dict[str, Any]]:
    """Load and normalize a problem set.

    ``path_or_records`` may be an in-memory list of dicts, or a path to a
    ``.json`` (list, or ``{"records"/"data": [...]}``) or ``.jsonl`` file.
    ``fmt`` is ``"auto"`` (detect by extension/content), ``"json"``, ``"jsonl"``,
    or ``"records"``.
    """
    if isinstance(path_or_records, (list, tuple)):
        raw_records = list(path_or_records)
    elif isinstance(path_or_records, dict):
        raw_records = path_or_records.get("records") or path_or_records.get("data") or []
    else:
        path = str(path_or_records)
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
        chosen = fmt
        if chosen == "auto":
            if path.lower().endswith(".jsonl"):
                chosen = "jsonl"
            elif path.lower().endswith(".json"):
                chosen = "json"
            else:
                stripped = text.lstrip()
                chosen = "json" if stripped[:1] in "[{" and "\n{" not in stripped[:200] else "jsonl"
        if chosen == "jsonl":
            raw_records = [json.loads(ln) for ln in text.splitlines() if ln.strip()]
        else:
            loaded = json.loads(text)
            if isinstance(loaded, dict):
                raw_records = loaded.get("records") or loaded.get("data") or [loaded]
            else:
                raw_records = loaded
    return [_normalize_record(dict(r)) for r in raw_records]


# ---------------------------------------------------------------------------
# Contamination controls
# ---------------------------------------------------------------------------

_WORD_RE = re.compile(r"[a-z0-9]+")


def _ngrams(text: str, n: int) -> set[tuple[str, ...]]:
    toks = _WORD_RE.findall(text.lower())
    if len(toks) < n:
        return {tuple(toks)} if toks else set()
    return {tuple(toks[i : i + n]) for i in range(len(toks) - n + 1)}


def contamination_flag(
    problem: dict[str, Any],
    generation: str,
    n: int = 5,
    threshold: float = 0.5,
) -> dict[str, Any]:
    """N-gram overlap between a generation and the gold solution.

    Flags a generation whose fraction of ``n``-grams also present in the gold
    solution meets ``threshold`` (a sign the model reproduced the reference
    solution verbatim rather than deriving it)."""
    gold = problem.get("gold_solution") or ""
    gen_grams = _ngrams(generation or "", n)
    gold_grams = _ngrams(gold, n)
    if not gen_grams or not gold_grams:
        return {"flagged": False, "overlap": 0.0, "n": n, "reason": "insufficient_text"}
    shared = gen_grams & gold_grams
    overlap = len(shared) / len(gen_grams)
    flagged = overlap >= threshold
    return {
        "flagged": bool(flagged),
        "overlap": round(overlap, 4),
        "n": n,
        "threshold": threshold,
        "reason": "high_gold_overlap" if flagged else "ok",
    }


def recalled_answer_smell(
    answer_acc: float | None,
    step_acc: float | None,
    ans_hi: float = 0.8,
    step_lo: float = 0.34,
) -> dict[str, Any]:
    """Smell test: high answer accuracy paired with low step/solution accuracy
    suggests a recalled (memorized) final answer rather than a genuine
    derivation. Returns ``flagged: None`` when step accuracy is unknown."""
    if answer_acc is None or step_acc is None:
        return {"flagged": None, "reason": "step_acc_unavailable"}
    flagged = answer_acc >= ans_hi and step_acc <= step_lo
    return {
        "flagged": bool(flagged),
        "answer_acc": round(float(answer_acc), 4),
        "step_acc": round(float(step_acc), 4),
        "reason": "answer_without_steps" if flagged else "ok",
    }


# ---------------------------------------------------------------------------
# Aggregation helpers
# ---------------------------------------------------------------------------

def _agg(values: list[float]) -> dict[str, Any]:
    """Mean + standard error (std(ddof=1)/sqrt(n)) for a list of numbers."""
    n = len(values)
    if n == 0:
        return {"mean": None, "stderr": None, "n": 0}
    mean = sum(values) / n
    stderr = (statistics.stdev(values) / math.sqrt(n)) if n >= 2 else 0.0
    return {"mean": round(mean, 6), "stderr": round(stderr, 6), "n": n}


def _rate_block(bools: list[bool]) -> dict[str, Any]:
    return _agg([1.0 if b else 0.0 for b in bools])


# ---------------------------------------------------------------------------
# Evaluate
# ---------------------------------------------------------------------------

def evaluate(
    problems: list[dict[str, Any]],
    generations: dict[str, list[str]],
    k: int | None = None,
) -> dict[str, Any]:
    """Grade provided generations against a normalized problem set.

    ``generations`` maps problem id -> list of candidate answer strings. Problems
    with ``usage_tag != "test"`` are refused (flagged, not scored). Proof-style
    problems are not gradable offline and are recorded as skipped.
    """
    problems = [p if "answer_kind" in p else _normalize_record(p) for p in problems]

    per_problem: list[dict[str, Any]] = []
    refused: list[dict[str, Any]] = []
    skipped: list[dict[str, Any]] = []
    flagged_ids: list[str] = []
    recall_ids: list[str] = []

    for prob in problems:
        pid = prob["id"]
        preds_all = list(generations.get(pid, []))
        eff_k = len(preds_all) if k is None else min(k, len(preds_all))
        preds = preds_all[:eff_k]

        # Honor the test-only contract: never score train/retrieval items.
        if prob["usage_tag"] != "test":
            refused.append(
                {
                    "id": pid,
                    "usage_tag": prob["usage_tag"],
                    "scored": False,
                    "reason": "usage_tag_not_test",
                }
            )
            continue

        if prob["grade_kind"] != "answer":
            skipped.append(
                {
                    "id": pid,
                    "grade_kind": prob["grade_kind"],
                    "scored": False,
                    "reason": "proof_grading_requires_lean_not_offline",
                }
            )
            continue

        if not preds:
            skipped.append(
                {"id": pid, "scored": False, "reason": "no_generations_provided"}
            )
            continue

        sample_res = grade_samples(prob["answer"] or "", preds, prob["answer_kind"])

        # Contamination: worst-case overlap across the candidate generations.
        contam_per = [contamination_flag(prob, g) for g in preds]
        contam = max(contam_per, key=lambda c: c["overlap"])
        if contam["flagged"]:
            flagged_ids.append(pid)

        # Recalled-answer smell (step accuracy unknown offline unless supplied).
        step_acc = prob.get("step_acc")
        smell = recalled_answer_smell(sample_res["averaged_at_k"], step_acc)
        if smell.get("flagged"):
            recall_ids.append(pid)

        # Fold into the six axes (kept separate, never blended).
        attempt = {
            "solved": sample_res["pass_at_k"],
            "pass_k": sample_res["pass_at_k"],
            "majority_k": sample_res["majority_at_k"],
            "answer_correct": sample_res["pass_at_k"],
            "contamination_flag": contam["flagged"],
        }
        axes = six_axis(attempt)

        per_problem.append(
            {
                "id": pid,
                "difficulty_tier": prob["difficulty_tier"],
                "contamination_risk": prob["contamination_risk"],
                "freshness_tier": freshness_tier(prob),
                "answer_kind": prob["answer_kind"],
                "metrics": {
                    "k": sample_res["k"],
                    "pass_at_k": sample_res["pass_at_k"],
                    "majority_at_k": sample_res["majority_at_k"],
                    "averaged_at_k": sample_res["averaged_at_k"],
                    "stderr": sample_res["stderr"],
                    "majority_answer": sample_res["majority_answer"],
                },
                "six_axis": axes,
                "contamination": contam,
                "recall_smell": smell,
            }
        )

    # ---- overall aggregates (each metric standing alone) ----
    pass_bools = [p["metrics"]["pass_at_k"] for p in per_problem]
    maj_bools = [p["metrics"]["majority_at_k"] for p in per_problem]
    avg_vals = [p["metrics"]["averaged_at_k"] for p in per_problem]
    overall = {
        "pass_at_k": _rate_block(pass_bools),
        "majority_at_k": _rate_block(maj_bools),
        "averaged_at_k": _agg(avg_vals),
    }

    # ---- breakdown by difficulty tier ----
    by_tier: dict[str, Any] = {}
    tiers = sorted({p["difficulty_tier"] for p in per_problem})
    for tier in tiers:
        rows = [p for p in per_problem if p["difficulty_tier"] == tier]
        by_tier[tier] = {
            "pass_at_k": _rate_block([r["metrics"]["pass_at_k"] for r in rows]),
            "majority_at_k": _rate_block([r["metrics"]["majority_at_k"] for r in rows]),
            "averaged_at_k": _agg([r["metrics"]["averaged_at_k"] for r in rows]),
        }

    # ---- breakdown by the six axes, SEPARATELY ----
    by_axis = _aggregate_axes(per_problem)

    return {
        "op": "evaluate",
        "k": k,
        "n_problems": len(problems),
        "n_scored": len(per_problem),
        "n_refused": len(refused),
        "n_skipped": len(skipped),
        "overall": overall,
        "by_difficulty_tier": by_tier,
        "by_axis": by_axis,
        "contamination": {
            "n_flagged": len(flagged_ids),
            "flagged_ids": flagged_ids,
            "recall_smell_ids": recall_ids,
        },
        "refused": refused,
        "skipped": skipped,
        "per_problem": per_problem,
    }


def _aggregate_axes(per_problem: list[dict[str, Any]]) -> dict[str, Any]:
    """Aggregate each of the six axes on its own — never collapsed into one
    number. Axes with no signal across the set are reported as ``None``."""
    result: dict[str, Any] = {axis: None for axis in AXES}

    discovery_solved = [
        p["six_axis"]["discovery"]["solved"]
        for p in per_problem
        if p["six_axis"].get("discovery") is not None
    ]
    discovery_pass = [
        p["six_axis"]["discovery"]["pass_k"]
        for p in per_problem
        if p["six_axis"].get("discovery") is not None
    ]
    discovery_maj = [
        p["metrics"]["majority_at_k"] for p in per_problem
    ]
    if discovery_solved:
        result["discovery"] = {
            "solved_rate": _rate_block([bool(x) for x in discovery_solved]),
            "pass_at_k_rate": _rate_block([bool(x) for x in discovery_pass]),
            "majority_at_k_rate": _rate_block([bool(x) for x in discovery_maj]),
        }

    informal_ans = [
        p["six_axis"]["informal"]["answer_correct"]
        for p in per_problem
        if p["six_axis"].get("informal") is not None
        and p["six_axis"]["informal"].get("answer_correct") is not None
    ]
    if informal_ans:
        result["informal"] = {
            "answer_correct_rate": _rate_block([bool(x) for x in informal_ans]),
        }

    novelty_flags = [
        p["six_axis"]["novelty"]["contamination_flag"]
        for p in per_problem
        if p["six_axis"].get("novelty") is not None
        and p["six_axis"]["novelty"].get("contamination_flag") is not None
    ]
    if novelty_flags:
        result["novelty"] = {
            "contamination_flag_rate": _rate_block([bool(x) for x in novelty_flags]),
        }

    # formal / soundness / efficiency have no offline answer-style signal; they
    # stay None so the reader sees the axis explicitly, not a fake zero.
    return result


# ---------------------------------------------------------------------------
# Dispatch / CLI
# ---------------------------------------------------------------------------

def run(request: dict[str, Any]) -> dict[str, Any]:
    op = request.get("op", "evaluate")
    if op == "load":
        problems = load_problems(request.get("path", request.get("records")), request.get("fmt", "auto"))
        return {"op": "load", "n": len(problems), "problems": problems}
    if op == "evaluate":
        problems = request.get("problems")
        if isinstance(problems, str):
            problems = load_problems(problems, request.get("fmt", "auto"))
        else:
            problems = load_problems(problems or [])
        return evaluate(problems, request.get("generations", {}), request.get("k"))
    if op == "freshness_tier":
        return {"tier": freshness_tier(request["problem"])}
    if op == "contamination_flag":
        return contamination_flag(
            request["problem"], request["generation"],
            int(request.get("n", 5)), float(request.get("threshold", 0.5)),
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
