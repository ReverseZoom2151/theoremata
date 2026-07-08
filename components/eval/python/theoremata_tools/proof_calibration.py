"""Evaluator-calibration metrics for the proof grader/critic.

Ports the *evaluator-quality* layer mined from ProofGrader's
``proofgrader/metrics/compute_evaluator_distances.py`` (see
``docs/resource-mining/reverify/open-atp-proofgrader-memtools.md`` — "the single
highest-value miss"). Given paired ``(predicted_score, gold_score)`` sequences —
where ``predicted_score`` is our critic/grader's number and ``gold_score`` is a
human/expert label (e.g. DeepSeek IMO-ProofBench) or Theoremata's own *formal*
verdict — it measures how well the evaluator tracks ground truth:

* point error: **MAE**, **RMSE**, **bias** (mean signed error), within-tolerance
  accuracy;
* correlation: **Pearson r**, **Spearman rho**, **Kendall tau-b** (tie-aware);
* ranking: **order-preservation / pairwise-ranking accuracy** (does the critic
  put proofs in the same order the gold does);
* uncertainty: **bootstrap percentile confidence intervals** on any metric;
* **evaluator-disagreement**: spread across several evaluators grading the same
  items; and
* the **verify-vs-solve gap**: how much better a model grades (verifies) than it
  solves.

Pure Python by default (self-contained, deterministic — important for the
hand-computed test oracles). ``numpy``/``scipy`` are used *only* opportunistically
for the correlation coefficients when importable, guarded so the fallbacks stay
authoritative and identical.

Public API:
    ``calibrate(pairs, ...) -> {mae, rmse, pearson, spearman, kendall,
        order_preservation, within_tolerance, bias, bootstrap_ci, ...}``
    ``bootstrap_ci(pairs, metrics=..., ...) -> {metric: {lo, hi, point}}``
    ``evaluator_disagreement(evaluator_scores) -> {...}``
    ``verify_solve_gap(verify_scores, solve_scores) -> {...}``
    ``run(request)``  — JSON worker entry (``op`` in
        {``calibrate``, ``bootstrap_ci``, ``disagreement``, ``verify_solve_gap``}).
"""
from __future__ import annotations

import json
import math
import random
import sys
from typing import Any, Iterable, Sequence

Number = float
Pair = tuple[float, float]

# --------------------------------------------------------------------------- #
# Input coercion
# --------------------------------------------------------------------------- #

def _coerce_pairs(pairs: Iterable[Any]) -> tuple[list[float], list[float]]:
    """Split ``pairs`` into aligned ``(predicted, gold)`` float lists.

    Accepts ``(pred, gold)`` tuples/lists or dicts with ``pred``/``gold`` keys
    (aliases: ``predicted``/``predicted_score``/``score`` and
    ``gold``/``gold_score``/``true``/``human``/``label``).
    """
    preds: list[float] = []
    golds: list[float] = []
    for row in pairs:
        if isinstance(row, dict):
            p = _first(row, ("pred", "predicted", "predicted_score", "score", "y_pred"))
            g = _first(row, ("gold", "gold_score", "true", "human", "label", "y_true"))
        else:
            p, g = row  # (pred, gold)
        if p is None or g is None:
            continue
        preds.append(float(p))
        golds.append(float(g))
    return preds, golds


def _first(d: dict, keys: Sequence[str]) -> Any:
    for k in keys:
        if k in d and d[k] is not None:
            return d[k]
    return None


# --------------------------------------------------------------------------- #
# Point-error primitives (stdlib, deterministic)
# --------------------------------------------------------------------------- #

def _mean(values: Sequence[float]) -> float:
    return sum(values) / len(values) if values else float("nan")


def mae(y_pred: Sequence[float], y_true: Sequence[float]) -> float:
    """Mean absolute error ``mean(|pred - true|)``."""
    if not y_pred:
        return float("nan")
    return _mean([abs(p - t) for p, t in zip(y_pred, y_true)])


def rmse(y_pred: Sequence[float], y_true: Sequence[float]) -> float:
    """Root-mean-square error ``sqrt(mean((pred - true)^2))``."""
    if not y_pred:
        return float("nan")
    return math.sqrt(_mean([(p - t) ** 2 for p, t in zip(y_pred, y_true)]))


def bias(y_pred: Sequence[float], y_true: Sequence[float]) -> float:
    """Mean signed error ``mean(pred - true)`` (positive = over-grading)."""
    if not y_pred:
        return float("nan")
    return _mean([p - t for p, t in zip(y_pred, y_true)])


def within_tolerance(
    y_pred: Sequence[float], y_true: Sequence[float], tolerance: float = 1.0
) -> float:
    """Fraction of items with ``|pred - true| <= tolerance``."""
    if not y_pred:
        return float("nan")
    return _mean([1.0 if abs(p - t) <= tolerance else 0.0 for p, t in zip(y_pred, y_true)])


# --------------------------------------------------------------------------- #
# Correlation primitives
# --------------------------------------------------------------------------- #

def pearson(xs: Sequence[float], ys: Sequence[float]) -> float:
    """Pearson product-moment correlation. NaN if a side is constant/empty."""
    n = len(xs)
    if n == 0 or n != len(ys):
        return float("nan")
    mx, my = _mean(xs), _mean(ys)
    num = sum((x - mx) * (y - my) for x, y in zip(xs, ys))
    denx = math.sqrt(sum((x - mx) ** 2 for x in xs))
    deny = math.sqrt(sum((y - my) ** 2 for y in ys))
    if denx == 0 or deny == 0:
        return float("nan")
    return num / (denx * deny)


def _rank(values: Sequence[float]) -> list[float]:
    """1-based ranks with average ranks for ties (fractional ranking)."""
    pairs = sorted((v, i) for i, v in enumerate(values))
    ranks = [0.0] * len(values)
    i = 0
    while i < len(pairs):
        j = i
        while j + 1 < len(pairs) and pairs[j + 1][0] == pairs[i][0]:
            j += 1
        avg_rank = (i + j + 2) / 2.0
        for k in range(i, j + 1):
            ranks[pairs[k][1]] = avg_rank
        i = j + 1
    return ranks


def spearman(xs: Sequence[float], ys: Sequence[float]) -> float:
    """Spearman rank correlation (Pearson on average ranks)."""
    if len(xs) == 0 or len(xs) != len(ys):
        return float("nan")
    return pearson(_rank(xs), _rank(ys))


def kendall_tau_b(xs: Sequence[float], ys: Sequence[float]) -> float:
    """Kendall's tau-b (concordance with tie correction).

    Ported verbatim in spirit from ProofGrader's ``kendall_tau_b``: NaN if the
    lengths mismatch, there are < 2 items, or all pairs tie on one side.
    """
    n = len(xs)
    if n != len(ys) or n < 2:
        return float("nan")
    concordant = discordant = ties_x = ties_y = 0
    for i in range(n - 1):
        xi, yi = xs[i], ys[i]
        for j in range(i + 1, n):
            dx = xi - xs[j]
            dy = yi - ys[j]
            if dx == 0 and dy == 0:
                ties_x += 1
                ties_y += 1
            elif dx == 0:
                ties_x += 1
            elif dy == 0:
                ties_y += 1
            else:
                prod = dx * dy
                if prod > 0:
                    concordant += 1
                elif prod < 0:
                    discordant += 1
    c, d = float(concordant), float(discordant)
    tx, ty = float(ties_x), float(ties_y)
    denom = math.sqrt((c + d + tx) * (c + d + ty))
    if denom == 0.0:
        return float("nan")
    return (c - d) / denom


# --------------------------------------------------------------------------- #
# Ranking / order preservation
# --------------------------------------------------------------------------- #

def order_preservation(y_pred: Sequence[float], y_true: Sequence[float]) -> dict[str, float]:
    """Pairwise order-preservation accuracy of ``y_pred`` w.r.t. ``y_true``.

    Over all pairs ``(i, j)`` that are *comparable* (``y_true[i] != y_true[j]``),
    the pair is correct when the predicted difference has the same sign as the
    true difference. A predicted tie on a comparable pair counts as incorrect
    (matches ProofGrader's ``compute_pairwise_order_stats``).
    """
    n = len(y_true)
    if n != len(y_pred) or n < 2:
        return {
            "num_items": float(n),
            "comparable_pairs": 0.0,
            "correct_pairs": 0.0,
            "order_acc": float("nan"),
        }
    comparable = correct = 0
    for i in range(n):
        ti, pi = y_true[i], y_pred[i]
        for j in range(i + 1, n):
            t_diff = ti - y_true[j]
            if t_diff == 0:
                continue
            comparable += 1
            p_diff = pi - y_pred[j]
            if p_diff != 0 and t_diff * p_diff > 0:
                correct += 1
    acc = (correct / comparable) if comparable > 0 else float("nan")
    return {
        "num_items": float(n),
        "comparable_pairs": float(comparable),
        "correct_pairs": float(correct),
        "order_acc": acc,
    }


# --------------------------------------------------------------------------- #
# Bootstrap confidence intervals
# --------------------------------------------------------------------------- #

_METRIC_FNS = {
    "mae": mae,
    "rmse": rmse,
    "bias": bias,
    "within_tolerance": within_tolerance,
    "pearson": lambda p, t: pearson(p, t),
    "spearman": lambda p, t: spearman(p, t),
    "kendall": lambda p, t: kendall_tau_b(p, t),
    "order_preservation": lambda p, t: order_preservation(p, t)["order_acc"],
}


def _percentile(sorted_values: Sequence[float], q: float) -> float:
    """Linear-interpolated percentile, ``q`` in ``[0, 1]``."""
    if not sorted_values:
        return float("nan")
    n = len(sorted_values)
    if n == 1:
        return float(sorted_values[0])
    pos = (n - 1) * q
    lo = math.floor(pos)
    hi = math.ceil(pos)
    if lo == hi:
        return float(sorted_values[lo])
    frac = pos - lo
    return float(sorted_values[lo] * (1.0 - frac) + sorted_values[hi] * frac)


def bootstrap_ci(
    pairs: Iterable[Any],
    *,
    metrics: Sequence[str] = ("mae", "rmse", "pearson", "spearman"),
    num_bootstrap: int = 1000,
    ci_level: float = 0.95,
    seed: int = 13,
    tolerance: float = 1.0,
) -> dict[str, dict[str, float]]:
    """Paired item-level bootstrap percentile CIs for the named metrics.

    Resamples ``(pred, gold)`` items *with replacement* ``num_bootstrap`` times,
    recomputes each metric, and reports the ``ci_level`` percentile interval plus
    the point estimate on the full sample. Deterministic given ``seed``.
    """
    preds, golds = _coerce_pairs(pairs)
    n = len(preds)
    out: dict[str, dict[str, float]] = {}
    if n == 0:
        return out
    rng = random.Random(seed)
    q_lo = (1.0 - ci_level) / 2.0
    q_hi = 1.0 - q_lo

    def _eval(metric: str, p: Sequence[float], g: Sequence[float]) -> float:
        fn = _METRIC_FNS[metric]
        if metric == "within_tolerance":
            return within_tolerance(p, g, tolerance)
        return fn(p, g)

    samples: dict[str, list[float]] = {m: [] for m in metrics}
    for _ in range(int(num_bootstrap)):
        idx = [rng.randrange(0, n) for _ in range(n)]
        bp = [preds[i] for i in idx]
        bg = [golds[i] for i in idx]
        for m in metrics:
            val = _eval(m, bp, bg)
            if isinstance(val, (int, float)) and not math.isnan(float(val)):
                samples[m].append(float(val))
    for m in metrics:
        vals = sorted(samples[m])
        point = _eval(m, preds, golds)
        if not vals:
            out[m] = {"lo": float("nan"), "hi": float("nan"), "point": point}
            continue
        out[m] = {
            "lo": _percentile(vals, q_lo),
            "hi": _percentile(vals, q_hi),
            "point": point,
        }
    return out


# --------------------------------------------------------------------------- #
# Evaluator disagreement
# --------------------------------------------------------------------------- #

def evaluator_disagreement(evaluator_scores: Any) -> dict[str, Any]:
    """Cross-evaluator disagreement on the same items.

    ``evaluator_scores`` is either a mapping ``{evaluator_name: [scores]}`` or a
    list of score sequences (one per evaluator), all aligned per item. For each
    item it reports the standard deviation and range of the evaluators' scores,
    and summarizes with the mean per-item std/range (ProofGrader's
    ``disagreement_per_item``).
    """
    if isinstance(evaluator_scores, dict):
        names = list(evaluator_scores.keys())
        seqs = [list(evaluator_scores[k]) for k in names]
    else:
        seqs = [list(s) for s in evaluator_scores]
        names = [f"evaluator_{i}" for i in range(len(seqs))]
    if len(seqs) < 2:
        return {
            "num_evaluators": float(len(seqs)),
            "num_items": 0.0,
            "mean_std": float("nan"),
            "mean_range": float("nan"),
            "max_std": float("nan"),
            "per_item": [],
            "evaluators": names,
        }
    n_items = min(len(s) for s in seqs)
    per_item: list[dict[str, float]] = []
    for i in range(n_items):
        col = [s[i] for s in seqs]
        m = _mean(col)
        var = _mean([(v - m) ** 2 for v in col])
        std = math.sqrt(var)
        per_item.append(
            {
                "item": float(i),
                "mean": m,
                "std": std,
                "range": max(col) - min(col),
                "min": min(col),
                "max": max(col),
            }
        )
    stds = [d["std"] for d in per_item]
    ranges = [d["range"] for d in per_item]
    return {
        "num_evaluators": float(len(seqs)),
        "num_items": float(n_items),
        "mean_std": _mean(stds) if stds else float("nan"),
        "mean_range": _mean(ranges) if ranges else float("nan"),
        "max_std": max(stds) if stds else float("nan"),
        "per_item": per_item,
        "evaluators": names,
    }


# --------------------------------------------------------------------------- #
# Verify-vs-solve gap
# --------------------------------------------------------------------------- #

def verify_solve_gap(
    verify_scores: Sequence[float], solve_scores: Sequence[float]
) -> dict[str, float]:
    """The verify-vs-solve gap: does a model grade better than it solves?

    ``verify_scores`` is the evaluator's per-item *verification quality* (e.g. 1
    if it graded the proof within tolerance / labeled it correctly, else 0, or
    any [0,1] quality). ``solve_scores`` is the same model's per-item *solve*
    success on those problems. ``gap = mean(verify) - mean(solve)``; a positive
    gap is the ProofGrader finding that models verify better than they solve.
    """
    if not verify_scores or len(verify_scores) != len(solve_scores):
        return {
            "n": float(len(verify_scores) if verify_scores else 0),
            "verify_mean": float("nan"),
            "solve_mean": float("nan"),
            "gap": float("nan"),
            "correlation": float("nan"),
        }
    vm = _mean(list(verify_scores))
    sm = _mean(list(solve_scores))
    return {
        "n": float(len(verify_scores)),
        "verify_mean": vm,
        "solve_mean": sm,
        "gap": vm - sm,
        "correlation": pearson(list(verify_scores), list(solve_scores)),
    }


# --------------------------------------------------------------------------- #
# Top-level calibration
# --------------------------------------------------------------------------- #

def calibrate(
    pairs: Iterable[Any],
    *,
    tolerance: float = 1.0,
    bootstrap: bool = False,
    bootstrap_metrics: Sequence[str] = ("mae", "rmse", "pearson", "spearman"),
    num_bootstrap: int = 1000,
    ci_level: float = 0.95,
    seed: int = 13,
) -> dict[str, Any]:
    """Calibrate an evaluator against gold labels from ``(pred, gold)`` pairs.

    Returns a flat dict of every calibration metric. ``bootstrap_ci`` is included
    only when ``bootstrap=True`` (it is the expensive part).
    """
    preds, golds = _coerce_pairs(pairs)
    n = len(preds)
    order = order_preservation(preds, golds)
    diffs = [p - t for p, t in zip(preds, golds)]
    result: dict[str, Any] = {
        "count": float(n),
        "mae": mae(preds, golds),
        "rmse": rmse(preds, golds),
        "bias": bias(preds, golds),
        "pearson": pearson(preds, golds),
        "spearman": spearman(preds, golds),
        "kendall": kendall_tau_b(preds, golds),
        "order_preservation": order["order_acc"],
        "comparable_pairs": order["comparable_pairs"],
        "correct_pairs": order["correct_pairs"],
        "within_tolerance": within_tolerance(preds, golds, tolerance),
        # PROOFGRADER's WTA<=1 ("within-1-point agreement"); alias at the default
        # tolerance so the paper's headline metric is always reported by name.
        "within_1_point": within_tolerance(preds, golds, 1.0),
        "tolerance": tolerance,
        "min_diff": min(diffs) if diffs else float("nan"),
        "max_diff": max(diffs) if diffs else float("nan"),
    }
    if bootstrap:
        result["bootstrap_ci"] = bootstrap_ci(
            pairs,
            metrics=bootstrap_metrics,
            num_bootstrap=num_bootstrap,
            ci_level=ci_level,
            seed=seed,
            tolerance=tolerance,
        )
    return result


# --------------------------------------------------------------------------- #
# Marking-scheme grader scoring
# --------------------------------------------------------------------------- #

def score_marking_scheme_grader(
    pred_grades: Sequence[float],
    gold_grades: Sequence[float],
    *,
    scale: int = 7,
    tolerance: float = 1.0,
    bootstrap: bool = False,
    num_bootstrap: int = 1000,
    ci_level: float = 0.95,
    seed: int = 13,
) -> dict[str, Any]:
    """Score the 0-7 marking-scheme grader against an expert-graded gold array.

    ``pred_grades`` are the grader's integer 0-``scale`` grades (e.g. the
    ``grade``/``median_score`` from
    :func:`theoremata_tools.proof_grader.grade_with_marking_scheme`) and
    ``gold_grades`` the aligned expert labels. Returns the full
    :func:`calibrate` metric dict — MAE, RMSE, bias, within-1-point (WTA<=1),
    Kendall-tau_b, Pearson, Spearman, order-preservation — plus ``scale`` and
    ``within_1_point`` (the paper's headline evaluator metric, human ceiling
    ~87.5%).
    """
    pairs = list(zip(pred_grades, gold_grades))
    result = calibrate(
        pairs,
        tolerance=tolerance,
        bootstrap=bootstrap,
        num_bootstrap=num_bootstrap,
        ci_level=ci_level,
        seed=seed,
    )
    result["scale"] = int(scale)
    # within_1_point is already emitted by calibrate; keep it explicit here too.
    result["within_1_point"] = within_tolerance(
        [float(p) for p in pred_grades], [float(g) for g in gold_grades], 1.0
    )
    return result


# --------------------------------------------------------------------------- #
# JSON worker entry
# --------------------------------------------------------------------------- #

def run(request: dict[str, Any]) -> dict[str, Any]:
    """JSON dispatch for the worker (tool key ``proof_calibration``)."""
    op = request.get("op", "calibrate")
    if op == "calibrate":
        return calibrate(
            request.get("pairs", []),
            tolerance=float(request.get("tolerance", 1.0)),
            bootstrap=bool(request.get("bootstrap", False)),
            bootstrap_metrics=tuple(
                request.get("bootstrap_metrics", ("mae", "rmse", "pearson", "spearman"))
            ),
            num_bootstrap=int(request.get("num_bootstrap", 1000)),
            ci_level=float(request.get("ci_level", 0.95)),
            seed=int(request.get("seed", 13)),
        )
    if op == "bootstrap_ci":
        return bootstrap_ci(
            request.get("pairs", []),
            metrics=tuple(request.get("metrics", ("mae", "rmse", "pearson", "spearman"))),
            num_bootstrap=int(request.get("num_bootstrap", 1000)),
            ci_level=float(request.get("ci_level", 0.95)),
            seed=int(request.get("seed", 13)),
            tolerance=float(request.get("tolerance", 1.0)),
        )
    if op == "score_marking_scheme_grader":
        return score_marking_scheme_grader(
            request.get("pred_grades", []),
            request.get("gold_grades", []),
            scale=int(request.get("scale", 7)),
            tolerance=float(request.get("tolerance", 1.0)),
            bootstrap=bool(request.get("bootstrap", False)),
            num_bootstrap=int(request.get("num_bootstrap", 1000)),
            ci_level=float(request.get("ci_level", 0.95)),
            seed=int(request.get("seed", 13)),
        )
    if op == "disagreement":
        return evaluator_disagreement(request.get("evaluator_scores", []))
    if op == "verify_solve_gap":
        return verify_solve_gap(
            request.get("verify_scores", []), request.get("solve_scores", [])
        )
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
