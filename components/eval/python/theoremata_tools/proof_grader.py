"""Rubric-based, step-wise proof grader (ProofGrader-style).

Mirrors the grading methodology mined in
``docs/resource-mining/new/proofgrader-main.md``. ProofGrader is an
LLM-proof-grading harness that separates candidate generation from evaluator
experiments and offers a small set of grading *workflows*:

    ``single``               single-shot grading;
    ``decompose_then_judge`` decompose proof into steps, then judge;
    ``repeat_and_aggregate`` repeat and aggregate (mean/min/max/median);
    ``reflect_and_revise``   initial report, critique, and final verdict.

This module ports the ``decompose_then_judge`` idea (``mode="step_wise"``) and
``single`` (``mode="holistic"``) with a **structured error taxonomy** and two
independent grading paths:

* a **deterministic** path (default) that flags placeholder ``sorry``/``admit``,
  hedge-word assertions ("clearly", "obviously", …) as *unjustified steps*,
  gap markers as *logical gaps*, and provably-wrong numeric equalities as
  *computation errors* (the arithmetic check reuses
  :func:`theoremata_tools.grader.grade_answer` symbolic equality — the IneqMath
  "exact forms only" rule); and
* an optional **LLM-judge** path (``use_llm=True``) via the mock-capable
  provider (:func:`theoremata_tools.model_provider.generate`), so it runs
  offline under ``THEOREMATA_MODEL_MOCK=1`` and never hits the network in tests.

Every grade returns ``{score, per_step: [{status, reason, ...}], verdict, ...}``.

As the report warns, an LLM grade is a *ranking/triage signal, not a soundness
certificate* — final acceptance still belongs to the Lean compile/sorry/axiom
gate in :mod:`theoremata_tools.benchmarks.graders`.
"""
from __future__ import annotations

import json
import math
import re
import sys
from typing import Any, Callable

from theoremata_tools import grader as base_grader

# --------------------------------------------------------------------------- #
# Error taxonomy
# --------------------------------------------------------------------------- #

CORRECT = "correct"
LOGICAL_GAP = "logical-gap"
UNJUSTIFIED_STEP = "unjustified-step"
COMPUTATION_ERROR = "computation-error"

#: The full status vocabulary. ``correct`` is the only non-flaw status.
ERROR_TAXONOMY = (CORRECT, UNJUSTIFIED_STEP, LOGICAL_GAP, COMPUTATION_ERROR)
FLAW_STATUSES = (UNJUSTIFIED_STEP, LOGICAL_GAP, COMPUTATION_ERROR)

# Ordering used to pick the single worst status for a holistic verdict.
_SEVERITY = {
    CORRECT: 0,
    UNJUSTIFIED_STEP: 1,
    LOGICAL_GAP: 2,
    COMPUTATION_ERROR: 3,
}

_SORRY_TOKENS = ("sorry", "admit", "sorryax")

# Assertions of truth with no supporting argument -> unjustified step.
_HEDGE_PATTERNS = (
    "clearly",
    "obviously",
    "trivially",
    "evidently",
    "it is easy to see",
    "it is trivial",
    "easy to see",
    "one can easily",
    "it follows immediately",
    "immediately follows",
    "left to the reader",
    "left as an exercise",
    "hand-wav",
    "without proof",
    "we omit the proof",
)

# Broken / missing chains of reasoning -> logical gap.
_GAP_PATTERNS = (
    "somehow",
    "by magic",
    "...",
    "todo",
    "fixme",
    "hence the result",  # asserting a conclusion with nothing before it
    "and we are done",
)


# --------------------------------------------------------------------------- #
# Rubric upgrades (from TheoremExplainAgent — geometric mean, banned-reasoning
# anti-refusal guard, <LGTM> convergence sentinel, anchored multi-dimension
# rubric) + ProofGrader's 0-N ordinal scale.
# --------------------------------------------------------------------------- #

#: Clean-judgment sentinel. If a judge emits this, the proof is accepted and the
#: retry/critique loop short-circuits (TEA ``<LGTM>`` contract).
LGTM_SENTINEL = "<LGTM>"

#: Judge cop-out phrases. A judgment containing any of these is a *refusal*, not
#: a grade: it is rejected so a non-answer can't poison the verdict / a retry
#: loop (TEA ``banned_reasonings.txt``, matched case-insensitively).
BANNED_REASONINGS = (
    "evaluation cannot",
    "can't assist",
    "cannot assist",
    "can't provide",
    "cannot provide",
    "can't evaluate",
    "cannot evaluate",
    "cannot be evaluated",
    "cannot be rated",
    "cannot be completed",
    "cannot be assessed",
    "cannot be scored",
    "cannot be conducted",
    "unable to evaluate",
    "unable to provide the evaluation",
    "do not have the capability",
    "do not have the ability",
    "as an ai",
    "i cannot",
)

#: Anchored multi-dimension rubric (1-5 each, 1 = completely fails … 5 = fully
#: meets or exceeds). Dimensions map to proof quality, per the TEA methodology.
RUBRIC_DIMENSIONS = (
    "statement_fidelity",  # proves the actual statement, not a weaker one
    "proof_correctness",   # every step is valid
    "gap_freeness",        # no unjustified leaps / missing cases
    "rigor",               # formal-check-ready, no hand-waving
    "minimality",          # no spurious / circular detours
)

#: Per-status quality in [0,1] used when aggregating with the geometric mean, so
#: a single severe flaw drags the whole score down instead of averaging out.
_STATUS_QUALITY = {
    CORRECT: 1.0,
    UNJUSTIFIED_STEP: 0.5,
    LOGICAL_GAP: 0.3,
    COMPUTATION_ERROR: 0.1,
}


def is_refusal(text: Any) -> bool:
    """True if ``text`` is a judge refusal/punt (contains a banned reasoning)."""
    if text is None:
        return False
    low = str(text).lower()
    return any(bad in low for bad in BANNED_REASONINGS)


def has_lgtm(text: Any) -> bool:
    """True if ``text`` carries the ``<LGTM>`` clean-judgment sentinel."""
    return text is not None and LGTM_SENTINEL.lower() in str(text).lower()


def arithmetic_mean(scores: list[float]) -> float:
    """Plain mean of non-None scores (0.0 if empty)."""
    vals = [float(s) for s in scores if s is not None]
    return sum(vals) / len(vals) if vals else 0.0


def geometric_mean(scores: list[float]) -> float:
    """Geometric mean of non-None scores (0.0 if empty).

    Product-based, so a single near-zero dimension tanks the aggregate — the
    imbalance-punishing aggregation from TEA's ``calculate_geometric_mean``.
    Negative inputs are invalid for a geometric mean and yield 0.0.
    """
    vals = [float(s) for s in scores if s is not None]
    if not vals:
        return 0.0
    prod = 1.0
    for v in vals:
        if v < 0:
            return 0.0
        prod *= v
    return prod ** (1.0 / len(vals))


def aggregate_scores(scores: list[float], method: str = "arithmetic") -> float:
    """Aggregate dimension/step scores by ``"arithmetic"`` or ``"geometric"``."""
    if method == "geometric":
        return geometric_mean(scores)
    if method == "arithmetic":
        return arithmetic_mean(scores)
    raise ValueError(f"unknown aggregation method: {method!r}")


def _to_ordinal(fraction: float, scale_max: int) -> int:
    """Map a [0,1] quality fraction onto an integer 0..scale_max ordinal band."""
    if fraction != fraction:  # NaN
        return 0
    return int(max(0, min(scale_max, round(fraction * scale_max))))


# --------------------------------------------------------------------------- #
# Decomposition
# --------------------------------------------------------------------------- #

_STEP_SPLIT = re.compile(r"[\r\n]+")
_SENT_SPLIT = re.compile(r"(?<=[.;])\s+")
_NUM_PREFIX = re.compile(r"^\s*(?:\(?\d+[.)]|[-*•·])\s*")


def split_steps(proof: str) -> list[str]:
    """Decompose a proof into gradable steps.

    Splits on line breaks first (numbered/bulleted lists and Lean tactic blocks
    are line-oriented); a single-line proof falls back to sentence splitting.
    """
    if not proof:
        return []
    raw = [s.strip() for s in _STEP_SPLIT.split(proof)]
    steps = [_NUM_PREFIX.sub("", s).strip() for s in raw if s.strip()]
    if len(steps) <= 1:
        # Single block: split into sentences instead.
        only = steps[0] if steps else proof.strip()
        steps = [
            _NUM_PREFIX.sub("", s).strip()
            for s in _SENT_SPLIT.split(only)
            if s.strip()
        ]
    return [s for s in steps if s]


def _coerce_steps(proof: str | None, steps: list[str] | None) -> list[str]:
    if steps is not None:
        return [str(s).strip() for s in steps if str(s).strip()]
    return split_steps(proof or "")


# --------------------------------------------------------------------------- #
# Deterministic per-step classification
# --------------------------------------------------------------------------- #

# A numeric-only expression: digits, the four ops, ^, parens, dot, spaces.
# Adjacency guards (``(?<![\w^.])`` / ``(?![\w^.])``) stop us from carving a
# fake ``2 = 4`` out of a symbolic ``n^2 = 4k^2``: a numeric side must not touch
# a letter/underscore/``^``/``.`` on either end.
_NUM_EXPR = r"[0-9][0-9+\-*/^(). ]*[0-9)]|[0-9]"
_EQUATION = re.compile(rf"(?<![\w^.])({_NUM_EXPR})\s*=\s*({_NUM_EXPR})(?![\w^.])")


def _bad_arithmetic(step: str) -> str | None:
    """Return the first provably-wrong numeric equality in ``step``, else None.

    Only *numeric* equalities are checked (so genuine symbolic identities are
    never mis-flagged). Equality is decided by the existing deterministic grader
    using SymPy exact arithmetic, so ``2 + 2 = 5`` is a computation error while
    ``6/3 = 2`` is fine.
    """
    for lhs, rhs in _EQUATION.findall(step):
        lhs, rhs = lhs.strip(), rhs.strip()
        if not any(c.isdigit() for c in lhs) or not any(c.isdigit() for c in rhs):
            continue
        verdict = base_grader.grade_answer(lhs, rhs, "symbolic")
        # Only trust a clean symbolic parse; parse errors are inconclusive.
        if str(verdict.get("method", "")).startswith("symbolic") and not verdict[
            "correct"
        ]:
            return f"{lhs} = {rhs}"
    return None


def classify_step(step: str) -> dict[str, Any]:
    """Deterministically classify a single step into the error taxonomy."""
    low = step.lower()

    for tok in _SORRY_TOKENS:
        if re.search(rf"\b{re.escape(tok)}\b", low):
            return {
                "status": UNJUSTIFIED_STEP,
                "reason": f"placeholder '{tok}' leaves the step unproved",
            }

    bad = _bad_arithmetic(step)
    if bad is not None:
        return {
            "status": COMPUTATION_ERROR,
            "reason": f"numeric equality does not hold: {bad}",
        }

    for pat in _HEDGE_PATTERNS:
        if pat in low:
            return {
                "status": UNJUSTIFIED_STEP,
                "reason": f"claim asserted without justification (hedge: {pat!r})",
            }

    for pat in _GAP_PATTERNS:
        if pat in low:
            return {
                "status": LOGICAL_GAP,
                "reason": f"missing reasoning / gap marker ({pat!r})",
            }

    return {"status": CORRECT, "reason": "no deterministic flaw detected"}


# --------------------------------------------------------------------------- #
# LLM-judge path (mock-mode compatible)
# --------------------------------------------------------------------------- #

# A judge maps (problem, steps) -> {"per_step": [{status, reason}], "verdict"?}.
ProofJudge = Callable[[str, list[str]], dict[str, Any]]

_JUDGE_SCHEMA = {
    "type": "object",
    "required": ["per_step", "verdict"],
    "properties": {
        "per_step": {
            "type": "array",
            "items": {
                "type": "object",
                "required": ["status", "reason"],
                "properties": {
                    "status": {"type": "string", "enum": list(ERROR_TAXONOMY)},
                    "reason": {"type": "string"},
                },
            },
        },
        "verdict": {"type": "string"},
    },
}


def _default_llm_judge(problem: str, steps: list[str]) -> dict[str, Any]:
    """LLM-judge over the decomposed steps via the mock-capable provider.

    Deterministic in mock mode (``THEOREMATA_MODEL_MOCK=1``) so tests never hit
    the network. Returns ``{"per_step": [...], "verdict": ...}``; on any failure
    returns an empty ``per_step`` so the caller can fall back to determinism.
    """
    try:
        from theoremata_tools.model_provider import generate
    except Exception as exc:  # provider component not importable
        return {"per_step": [], "verdict": "unknown", "error": f"judge_unavailable:{exc}"}

    request = {
        "role": "proof_step_judge",
        "task": (
            "You are grading a mathematical proof with a rubric. Decompose the "
            "reasoning and, for EACH step, assign exactly one status from the "
            "error taxonomy: 'correct', 'unjustified-step' (a claim asserted "
            "without justification), 'logical-gap' (a missing link in the "
            "argument), or 'computation-error' (a wrong calculation). Give a "
            "short reason per step, then an overall verdict ('correct' or "
            "'flawed')."
        ),
        "context": {"problem": problem, "steps": steps},
        "output_schema": _JUDGE_SCHEMA,
    }
    try:
        content, model = generate(request)
        content.setdefault("_model", model)
        return content
    except Exception as exc:  # noqa: BLE001
        return {"per_step": [], "verdict": "unknown", "error": f"judge_error:{exc}"}


def _normalize_judge_steps(
    steps: list[str], raw: list[Any]
) -> list[dict[str, Any]]:
    """Coerce an LLM judge's per-step list onto our step list + taxonomy."""
    out: list[dict[str, Any]] = []
    for i, step in enumerate(steps):
        entry = raw[i] if i < len(raw) and isinstance(raw[i], dict) else {}
        status = str(entry.get("status", CORRECT)).strip().lower()
        if status not in ERROR_TAXONOMY:
            status = CORRECT if status in {"ok", "valid", "fine"} else UNJUSTIFIED_STEP
        out.append(
            {
                "step": step,
                "status": status,
                "reason": str(entry.get("reason", "")) or "llm_judge",
            }
        )
    return out


# --------------------------------------------------------------------------- #
# Scoring / verdict
# --------------------------------------------------------------------------- #

def _score(per_step: list[dict[str, Any]]) -> float:
    if not per_step:
        return 0.0
    good = sum(1 for s in per_step if s["status"] == CORRECT)
    return round(good / len(per_step), 6)


def _taxonomy_counts(per_step: list[dict[str, Any]]) -> dict[str, int]:
    counts = {status: 0 for status in ERROR_TAXONOMY}
    for s in per_step:
        counts[s["status"]] = counts.get(s["status"], 0) + 1
    return counts


def _worst_status(per_step: list[dict[str, Any]]) -> str:
    if not per_step:
        return CORRECT
    return max((s["status"] for s in per_step), key=lambda st: _SEVERITY.get(st, 1))


# --------------------------------------------------------------------------- #
# Public entry point
# --------------------------------------------------------------------------- #

def _judge_text(result: dict[str, Any]) -> str:
    """Flatten a judge result to text for sentinel / refusal scanning."""
    parts = [
        str(result.get(k, ""))
        for k in ("raw", "text", "content", "verdict", "reason", "notes")
    ]
    try:
        parts.append(json.dumps(result, default=str))
    except Exception:  # noqa: BLE001
        pass
    return " ".join(p for p in parts if p)


def grade_proof(
    proof: str | None = None,
    *,
    steps: list[str] | None = None,
    mode: str = "step_wise",
    use_llm: bool = False,
    judge: ProofJudge | None = None,
    problem: str = "",
    aggregate: str = "arithmetic",
    scale: int | None = None,
) -> dict[str, Any]:
    """Grade a proof with a rubric + structured error taxonomy.

    Parameters
    ----------
    proof:
        The proof text. Decomposed via :func:`split_steps` unless ``steps`` is
        given.
    steps:
        Pre-decomposed steps (skips text splitting).
    mode:
        ``"step_wise"`` (ProofGrader ``decompose_then_judge``) grades each step
        and scores by the fraction correct; ``"holistic"`` (ProofGrader
        ``single``) collapses to one overall status/verdict with a binary score.
    use_llm / judge:
        When ``use_llm`` is True, grade with ``judge`` (or the default
        mock-capable LLM judge). Any judge failure falls back to the
        deterministic path.
    problem:
        Optional problem statement passed to the LLM judge for context.
    aggregate:
        Step-wise score aggregation: ``"arithmetic"`` (default; fraction of
        correct steps) or ``"geometric"`` (per-step quality geometric mean, so a
        single severe flaw tanks the score — TEA geometric-mean aggregation).
    scale:
        When an int ``N`` (e.g. ``7`` for ProofGrader's 0-7 rubric), also emit an
        integer ``ordinal_score`` in ``[0, N]`` derived from the quality
        fraction, alongside the existing binary/float ``score``.

    Returns
    -------
    ``{score, verdict, mode, path, per_step: [{step, status, reason}],
       taxonomy_counts, flaw_count, n_steps}`` (+ ``ordinal_score``/``scale`` when
       ``scale`` is set, and ``lgtm``/``judge_refused`` flags on the LLM path).
    """
    if mode not in {"step_wise", "holistic"}:
        raise ValueError(f"unknown mode: {mode!r}")

    step_list = _coerce_steps(proof, steps)
    if not step_list:
        empty = {
            "score": None,
            "verdict": "empty",
            "mode": mode,
            "path": "none",
            "per_step": [],
            "taxonomy_counts": _taxonomy_counts([]),
            "flaw_count": 0,
            "n_steps": 0,
        }
        if scale is not None:
            empty["ordinal_score"] = None
            empty["scale"] = int(scale)
        return empty

    path = "deterministic"
    per_step: list[dict[str, Any]] = []
    lgtm = False
    judge_refused = False

    if use_llm:
        jfn = judge or _default_llm_judge
        result = jfn(problem, step_list) or {}
        text = _judge_text(result)
        raw_steps = result.get("per_step") or []
        # Anti-refusal guard: a punting judge is a non-answer, not a grade —
        # drop it and fall back to determinism rather than trusting it.
        if is_refusal(text):
            judge_refused = True
            raw_steps = []
        # <LGTM> convergence sentinel: a clean judgment with no flagged steps
        # short-circuits to acceptance.
        elif has_lgtm(text) and not any(
            isinstance(s, dict) and str(s.get("status")).lower() in FLAW_STATUSES
            for s in raw_steps
        ):
            lgtm = True
            per_step = [
                {"step": s, "status": CORRECT, "reason": "judge returned <LGTM>"}
                for s in step_list
            ]
            path = "llm_judge"
        if not per_step and raw_steps:
            per_step = _normalize_judge_steps(step_list, raw_steps)
            path = "llm_judge"

    if not per_step:  # deterministic path (default, or LLM fallback)
        for step in step_list:
            c = classify_step(step)
            per_step.append({"step": step, "status": c["status"], "reason": c["reason"]})

    counts = _taxonomy_counts(per_step)
    flaw_count = sum(counts[s] for s in FLAW_STATUSES)
    quality = [_STATUS_QUALITY.get(s["status"], 0.0) for s in per_step]

    if mode == "holistic":
        worst = _worst_status(per_step)
        score = 1.0 if worst == CORRECT else 0.0
        verdict = CORRECT if worst == CORRECT else "flawed"
        holistic = {"overall_status": worst}
        quality_fraction = score
    else:  # step_wise
        if aggregate == "geometric":
            score = round(aggregate_scores(quality, "geometric"), 6)
        else:
            score = _score(per_step)
        verdict = CORRECT if flaw_count == 0 else "flawed"
        holistic = {}
        quality_fraction = round(aggregate_scores(quality, aggregate), 6)

    out = {
        "score": score,
        "verdict": verdict,
        "mode": mode,
        "path": path,
        "aggregate": aggregate,
        "per_step": per_step,
        "taxonomy_counts": counts,
        "flaw_count": flaw_count,
        "n_steps": len(step_list),
        **holistic,
    }
    if use_llm:
        out["lgtm"] = lgtm
        out["judge_refused"] = judge_refused
    if scale is not None:
        out["scale"] = int(scale)
        out["ordinal_score"] = _to_ordinal(quality_fraction, int(scale))
    return out


def grade_proof_item(
    item: dict[str, Any],
    response: Any,
    *,
    mode: str = "step_wise",
    use_llm: bool = False,
    judge: ProofJudge | None = None,
) -> dict[str, Any]:
    """Benchmark-registry adapter: grade a proof ``response`` for an ``item``.

    Returns the uniform ``{is_solved, is_correct, detail}`` verdict used by the
    benchmark harness, wrapping :func:`grade_proof`. ``is_correct`` is True only
    when the rubric finds zero flaws (a triage signal, not a soundness proof).
    """
    text = response if isinstance(response, str) else str(
        (response or {}).get("proof", response) if isinstance(response, dict) else response
    )
    problem = str(item.get("problem") or item.get("statement") or "")
    graded = grade_proof(
        text, mode=mode, use_llm=use_llm, judge=judge, problem=problem
    )
    is_solved = graded["n_steps"] > 0
    is_correct = is_solved and graded["verdict"] == CORRECT
    return {
        "is_solved": is_solved,
        "is_correct": is_correct,
        "detail": {
            "track": "proof_rubric",
            "grader": "proof_grader",
            **graded,
        },
    }


# --------------------------------------------------------------------------- #
# JSON dispatch (worker hook) + CLI
# --------------------------------------------------------------------------- #

def run(request: dict[str, Any]) -> dict[str, Any]:
    op = request.get("op", "grade_proof")
    if op == "grade_proof":
        return grade_proof(
            request.get("proof"),
            steps=request.get("steps"),
            mode=request.get("mode", "step_wise"),
            use_llm=bool(request.get("use_llm", False)),
            problem=request.get("problem", ""),
            aggregate=request.get("aggregate", "arithmetic"),
            scale=request.get("scale"),
        )
    if op == "grade_proof_item":
        return grade_proof_item(
            request["item"],
            request.get("response", ""),
            mode=request.get("mode", "step_wise"),
            use_llm=bool(request.get("use_llm", False)),
        )
    if op == "split_steps":
        return {"op": "split_steps", "steps": split_steps(request.get("proof", ""))}
    if op == "classify_step":
        return {"op": "classify_step", **classify_step(request.get("step", ""))}
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
