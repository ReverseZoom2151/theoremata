"""Difficulty curriculum + H0 difficulty pre-filter (triage).

Two durable ideas, kept deliberately separate from the *lifelong* repo-level
policy in :mod:`theoremata_tools.lifelong_curriculum` (which this module reuses
but does not modify):

1. **Difficulty curriculum (LeanDojo-v2 /
   ``docs/paper-mining/leandojo-v2.md``).** Given a dataset of problems, each
   with a difficulty signal (proof length, or an explicit provided score),
   bucket into Easy / Medium / Hard at the **33rd / 67th percentiles of the
   difficulty signal** and emit an **easiest-first** ordering (the curriculum).
   This is the item-level analogue of LeanDojo-v2's "bucket theorems easy /
   medium / hard by proof-step-count percentiles, train easiest-first". Ordering
   is deterministic; ties are broken by problem id.

2. **H0 difficulty pre-filter (Bridge 2010 /
   ``docs/paper-mining/ml-and-automated-theorem-proving.md``).** An optional
   triage that, given problem features + a compute budget, predicts whether a
   problem is "too hard to attempt now" and should be deprioritized/deferred so
   the *total solved per budget* is maximized. A lightweight **learned** model
   is used when scikit-learn is importable, otherwise a **deterministic
   feature-threshold fallback** runs offline with no dependencies. This
   explicitly **trades coverage for total time**: under a tight budget we skip
   the problems predicted hardest (raising throughput-per-budget at the cost of
   some theorems we could have proved with unlimited time), exactly Bridge's H0
   filter which "rejects conjectures predicted too hard to prove, saving total
   time at the cost of some proved theorems".

Both surfaces are pure-Python and deterministic; the learned path is optional
and, once fitted, its weights are stored as plain JSON so estimation needs no
sklearn/GPU. Wire in ``worker.py`` as tool ``'difficulty'`` -> :func:`run`.
"""
from __future__ import annotations

import json
import math
import sys
from typing import Any, Optional, Sequence

# Reuse the mined percentile + proof-length difficulty from the lifelong module
# (import, do not re-implement / re-write). ``difficulty`` returns exp(#steps),
# ``inf`` for a ``sorry``/``admit`` proof, and ``None`` for no proof at all.
from .lifelong_curriculum import (
    EASY,
    HARD,
    MEDIUM,
    _percentile,
    difficulty as _proof_difficulty,
)

MODEL_SCHEMA = "theoremata.h0-difficulty.v1"

_RESERVED = {
    "id", "name", "problem", "goal", "cost", "label", "hard", "too_hard",
    "solved", "success", "closed", "features", "difficulty", "score",
    "proof", "steps", "proof_length", "n_steps",
}


# ---------------------------------------------------------------------------
# Feature / signal extraction
# ---------------------------------------------------------------------------

def _numeric(features: Any) -> dict[str, float]:
    """Keep only real numeric (non-bool) feature values from a mapping."""
    if not isinstance(features, dict):
        return {}
    out: dict[str, float] = {}
    for k, v in features.items():
        if k in _RESERVED:
            continue
        if isinstance(v, bool):
            continue
        if isinstance(v, (int, float)):
            out[str(k)] = float(v)
    return out


def _problem_features(problem: Any) -> dict[str, float]:
    """Numeric features for one problem: an explicit ``features`` mapping if
    present, else any numeric top-level (non-reserved) keys."""
    if isinstance(problem, dict):
        if isinstance(problem.get("features"), dict):
            return _numeric(problem["features"])
        return _numeric(problem)
    return {}


def _problem_id(problem: Any, index: int) -> str:
    if isinstance(problem, dict):
        for k in ("id", "name", "problem", "goal"):
            v = problem.get(k)
            if v is not None:
                return str(v)
    return str(index)


def problem_difficulty(problem: Any) -> float:
    """Difficulty signal for the *curriculum* (LeanDojo-v2 proof-length view).

    An explicit ``score`` / ``difficulty`` / ``proof_length`` / ``n_steps``
    wins; otherwise proof length via :func:`lifelong_curriculum.difficulty`
    (``exp(#steps)``, monotone in step count). A bare number is taken as the
    score directly. Missing / no-proof problems map to ``inf`` so they sort as
    hardest (deferred to the end of the curriculum), deterministically.
    """
    if isinstance(problem, (int, float)) and not isinstance(problem, bool):
        return float(problem)
    if isinstance(problem, dict):
        for k in ("score", "difficulty", "proof_length", "n_steps"):
            v = problem.get(k)
            if v is not None:
                return float(v)
        proof = problem.get("proof", problem.get("steps"))
        if proof is not None:
            d = _proof_difficulty(proof)
            return math.inf if d is None else float(d)
    return math.inf


# ---------------------------------------------------------------------------
# 1. Difficulty curriculum: percentile bucketing + easiest-first ordering
# ---------------------------------------------------------------------------

def bucket_by_percentile(
    difficulties: Sequence[float],
) -> tuple[list[str], float, float]:
    """Bucket finite difficulties Easy/Medium/Hard at the 33rd/67th percentiles.

    Non-finite (``inf``/``nan``) difficulties are always Hard. Returns
    ``(buckets, p33, p67)`` aligned with the input.
    """
    finite = sorted(d for d in difficulties if math.isfinite(d))
    if finite:
        p33 = _percentile(finite, 0.33)
        p67 = _percentile(finite, 0.67)
    else:
        p33 = p67 = math.inf
    buckets: list[str] = []
    for d in difficulties:
        if not math.isfinite(d):
            buckets.append(HARD)
        elif d <= p33:
            buckets.append(EASY)
        elif d <= p67:
            buckets.append(MEDIUM)
        else:
            buckets.append(HARD)
    return buckets, p33, p67


def curriculum(problems: Sequence[Any]) -> dict[str, Any]:
    """Build a difficulty curriculum over ``problems`` (easiest-first).

    Each problem carries a difficulty signal (see :func:`problem_difficulty`).
    Problems are bucketed at the 33rd/67th percentiles and emitted in
    **easiest-first** order; ties broken by problem id for determinism.

    Returns ``{thresholds:{p33,p67}, order:[{id,difficulty,bucket}...],
    buckets:{Easy:[id...],Medium:[...],Hard:[...]}}`` where ``order`` is the
    curriculum (front = easiest) and ``buckets`` groups ids by tier.
    """
    items = [
        {"id": _problem_id(p, i), "difficulty": problem_difficulty(p)}
        for i, p in enumerate(problems)
    ]
    diffs = [it["difficulty"] for it in items]
    tiers, p33, p67 = bucket_by_percentile(diffs)
    for it, tier in zip(items, tiers):
        it["bucket"] = tier
    # easiest-first: ascending difficulty, ties by id. inf sorts last.
    order = sorted(items, key=lambda it: (it["difficulty"], it["id"]))
    buckets: dict[str, list[str]] = {EASY: [], MEDIUM: [], HARD: []}
    for it in order:
        buckets[it["bucket"]].append(it["id"])
    return {
        "thresholds": {"p33": p33, "p67": p67},
        "order": order,
        "buckets": buckets,
    }


# ---------------------------------------------------------------------------
# 2. H0 difficulty pre-filter: estimate + budget-aware triage
# ---------------------------------------------------------------------------

def _sigmoid(x: float) -> float:
    if x >= 0:
        z = math.exp(-x)
        return 1.0 / (1.0 + z)
    z = math.exp(x)
    return z / (1.0 + z)


def _detect_backend() -> str:
    try:
        import sklearn  # noqa: F401
        return "sklearn"
    except Exception:  # noqa: BLE001
        return "fallback"


def _h0_label(example: Any) -> int:
    """Ground-truth 'too hard' label for training. Explicit ``too_hard`` /
    ``hard`` / ``label`` wins; else derived from ``solved``/``success`` (an
    unsolved problem is the hard class)."""
    if isinstance(example, dict):
        for k in ("too_hard", "hard", "label"):
            if example.get(k) is not None:
                v = example[k]
                if isinstance(v, str):
                    return 1 if v.strip().lower() in ("1", "true", "yes", "hard") else 0
                return 1 if v else 0
        for k in ("solved", "success", "closed"):
            if example.get(k) is not None:
                return 0 if example[k] else 1
    return 0


def _fit_sklearn(
    rows: Sequence[dict[str, Any]], keys: Sequence[str]
) -> Optional[tuple[dict[str, float], float]]:
    """Fit logistic ``features -> too_hard``; return ``(weights, intercept)`` or
    ``None`` when single-class / no features (caller falls back)."""
    labels = {r["hard"] for r in rows}
    if len(labels) < 2 or not keys:
        return None
    try:
        from sklearn.linear_model import LogisticRegression
    except Exception:  # noqa: BLE001
        return None
    X = [[r["features"].get(k, 0.0) for k in keys] for r in rows]
    y = [r["hard"] for r in rows]
    clf = LogisticRegression(max_iter=200)
    clf.fit(X, y)
    coefs = clf.coef_[0]
    return {k: float(coefs[i]) for i, k in enumerate(keys)}, float(clf.intercept_[0])


def _fit_fallback(
    rows: Sequence[dict[str, Any]], keys: Sequence[str]
) -> tuple[dict[str, float], float]:
    """Deterministic closed-form margin: mean(feature | hard) - mean(feature |
    easy). Zero for a key with no discriminative signal."""
    hard = [r for r in rows if r["hard"]]
    easy = [r for r in rows if not r["hard"]]
    if not hard or not easy:
        return {k: 0.0 for k in keys}, 0.0
    w: dict[str, float] = {}
    for k in keys:
        mh = sum(r["features"].get(k, 0.0) for r in hard) / len(hard)
        me = sum(r["features"].get(k, 0.0) for r in easy) / len(easy)
        w[k] = mh - me
    return w, 0.0


def train_h0(
    examples: Sequence[Any], *, backend: str = "auto"
) -> dict[str, Any]:
    """Fit an H0 difficulty model on labelled ``examples`` (``{features, ...}``
    with a too-hard label; see :func:`_h0_label`).

    Uses scikit-learn logistic regression when available/importable and the
    labels are two-class, else a deterministic feature-margin fallback. Returns
    a JSON-serializable model ``{schema, lib, features, weights, intercept}``
    that :func:`estimate_difficulty` can query with no sklearn/GPU.
    """
    if backend == "auto":
        backend = _detect_backend()
    rows = [
        {"features": _numeric(e.get("features", e) if isinstance(e, dict) else e),
         "hard": _h0_label(e)}
        for e in examples
    ]
    keys: set[str] = set()
    for r in rows:
        keys.update(r["features"].keys())
    keys_l = sorted(keys)

    lib = backend
    fit: Optional[tuple[dict[str, float], float]] = None
    if backend == "sklearn":
        fit = _fit_sklearn(rows, keys_l)
    if fit is None:
        fit = _fit_fallback(rows, keys_l)
        if backend == "sklearn":
            lib = "fallback"  # requested lib unusable (single-class / absent)
    weights, intercept = fit
    return {
        "schema": MODEL_SCHEMA,
        "lib": lib,
        "features": keys_l,
        "weights": weights,
        "intercept": intercept,
    }


def estimate_difficulty(
    features: Any, model: Optional[dict[str, Any]] = None
) -> float:
    """Estimate how hard a problem is from its features (higher = harder).

    With a fitted ``model`` (from :func:`train_h0`) this is the learned
    probability the problem is too-hard, ``sigmoid(intercept + w . features)``.
    Without a model it is the deterministic feature-threshold fallback: the sum
    of numeric feature values (documented contract: features are oriented so a
    larger value means harder). Empty features -> ``0.0``.
    """
    feats = _numeric(features)
    if model and model.get("weights"):
        w = model["weights"]
        raw = float(model.get("intercept", 0.0)) + sum(
            float(w.get(k, 0.0)) * v for k, v in feats.items()
        )
        return _sigmoid(raw)
    if not feats:
        return 0.0
    return float(sum(feats.values()))


def _hardness(problem: Any, model: Optional[dict[str, Any]]) -> float:
    """Estimated hardness of one triage problem: an explicit ``difficulty`` /
    ``score`` wins, else :func:`estimate_difficulty` on its features."""
    if isinstance(problem, dict):
        for k in ("difficulty", "score"):
            if problem.get(k) is not None:
                return float(problem[k])
    return estimate_difficulty(_problem_features(problem), model)


def triage(
    problems: Sequence[Any],
    budget: float,
    *,
    model: Optional[dict[str, Any]] = None,
    max_difficulty: Optional[float] = None,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """Split ``problems`` into ``(attempt_now, defer)`` under a compute budget.

    Predicts each problem's hardness (learned model or fallback), then greedily
    attempts problems **easiest-first** while the cumulative cost stays within
    ``budget`` (per-problem ``cost`` defaults to ``1.0``). Any problem whose
    estimated hardness exceeds ``max_difficulty`` (when given) is always
    deferred (Bridge's H0 hopeless-skip). Everything not attempted is deferred.

    This **trades coverage for total time**: a tight budget defers the hardest
    problems first (maximizing solved-per-budget), a loose budget attempts more.
    Deterministic: ties in hardness are broken by problem id.

    Returns two lists of ``{id, difficulty, cost}`` dicts (attempt_now, defer).
    """
    scored: list[dict[str, Any]] = []
    for i, p in enumerate(problems):
        cost = 1.0
        if isinstance(p, dict) and p.get("cost") is not None:
            cost = float(p["cost"])
        scored.append({
            "id": _problem_id(p, i),
            "difficulty": _hardness(p, model),
            "cost": cost,
        })
    # easiest-first; ties by id for determinism.
    scored.sort(key=lambda r: (r["difficulty"], r["id"]))

    attempt: list[dict[str, Any]] = []
    defer: list[dict[str, Any]] = []
    spent = 0.0
    for r in scored:
        if max_difficulty is not None and r["difficulty"] > max_difficulty:
            defer.append(r)
            continue
        if spent + r["cost"] <= budget:
            spent += r["cost"]
            attempt.append(r)
        else:
            defer.append(r)
    return attempt, defer


# ---------------------------------------------------------------------------
# Worker dispatch. Wire in worker.py as: tool 'difficulty' -> difficulty.run.
# ---------------------------------------------------------------------------

def run(request: dict[str, Any]) -> dict[str, Any]:
    op = request.get("op", "curriculum")
    if op == "curriculum":
        result = curriculum(request.get("problems", []))
        result["op"] = "curriculum"
        return result
    if op == "triage":
        model = request.get("model")
        if model is None and request.get("examples") is not None:
            model = train_h0(request["examples"], backend=request.get("backend", "auto"))
        md = request.get("max_difficulty")
        attempt, defer = triage(
            request.get("problems", []),
            float(request.get("budget", math.inf)),
            model=model,
            max_difficulty=None if md is None else float(md),
        )
        return {
            "op": "triage",
            "budget": request.get("budget", math.inf),
            "attempt_now": attempt,
            "defer": defer,
            "n_attempt": len(attempt),
            "n_defer": len(defer),
            "lib": (model or {}).get("lib", "fallback"),
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
