"""Retrieval evaluation harness — R@1 / R@10 / MRR (ReProver ``retrieval/evaluate.py``).

Scores any premise retriever against **gold** (provenance-labelled) premises.
Given, per example, a *ranked* list of retrieved premise names and the set of
gold premises actually used in the ground-truth proof, it computes:

* **R@k** — recall at ``k``: fraction of the gold premises that appear in the top
  ``k`` retrieved, averaged over examples. Reported for each ``k`` (default 1, 10).
* **MRR** — mean reciprocal rank of the *first* gold premise hit (``1/rank``,
  ``0`` when no gold premise is retrieved), averaged over examples.

This mirrors ReProver: it evaluates a retriever's ranked output independently of
the downstream prover, so retrieval quality is a separate CI/eval track from
proof Pass@1.

Two input shapes are supported:

* **examples** — ``[{"retrieved": [names...], "gold": [names...]}, ...]``.
* **predictions-style** — a list of prediction records keyed by an id, plus a
  ``gold`` map ``{id: [names...]}``; each prediction carries
  ``retrieved_premises`` (ranked). This matches ReProver's ``predict.pickle``
  layout so exported predictions can be scored directly.

Standard-library only.
"""
from __future__ import annotations

import json
import os
import sys
from typing import Any, Iterable, Sequence

DEFAULT_KS = (1, 10)


# --------------------------------------------------------------------------- #
# Per-example metrics.
# --------------------------------------------------------------------------- #
def _name_of(p: Any) -> str:
    """Premise identity: a bare name string, or a record's ``full_name``/``name``."""
    if isinstance(p, str):
        return p
    if isinstance(p, dict):
        return str(p.get("full_name") or p.get("name") or "")
    return str(p)


def recall_at_k(retrieved: Sequence[Any], gold: Iterable[Any], k: int) -> float:
    """Fraction of ``gold`` premises present in the top-``k`` of ``retrieved``.

    Returns ``1.0`` for an example with no gold premises (nothing to miss), as in
    ReProver's convention, so such examples neither help nor hurt the average.
    """
    gold_set = {_name_of(g) for g in gold if _name_of(g)}
    if not gold_set:
        return 1.0
    topk = {_name_of(r) for r in list(retrieved)[: max(0, k)]}
    hit = len(gold_set & topk)
    return hit / len(gold_set)


def reciprocal_rank(retrieved: Sequence[Any], gold: Iterable[Any]) -> float:
    """``1 / (rank of first gold hit)``; ``0.0`` when no gold premise is found."""
    gold_set = {_name_of(g) for g in gold if _name_of(g)}
    if not gold_set:
        return 0.0
    for i, r in enumerate(retrieved):
        if _name_of(r) in gold_set:
            return 1.0 / (i + 1)
    return 0.0


# --------------------------------------------------------------------------- #
# Corpus-level evaluation.
# --------------------------------------------------------------------------- #
def evaluate(
    examples: Sequence[dict[str, Any]],
    ks: Sequence[int] = DEFAULT_KS,
    *,
    retrieved_key: str = "retrieved",
    gold_key: str = "gold",
) -> dict[str, Any]:
    """Compute R@k (for each ``k``) and MRR over ``examples``.

    Each example is ``{retrieved_key: [ranked names], gold_key: [gold names]}``.
    Returns a stable metrics dict::

        {"ok", "n", "ks", "R@1", "R@10", ..., "MRR",
         "recall": {k: value}, "per_example": [...]}
    """
    ks = list(ks)
    n = len(examples)
    recall_sums = {k: 0.0 for k in ks}
    mrr_sum = 0.0
    per_example: list[dict[str, Any]] = []

    for ex in examples:
        retrieved = ex.get(retrieved_key) or []
        gold = ex.get(gold_key) or []
        rec = {k: recall_at_k(retrieved, gold, k) for k in ks}
        rr = reciprocal_rank(retrieved, gold)
        for k in ks:
            recall_sums[k] += rec[k]
        mrr_sum += rr
        per_example.append(
            {
                "id": ex.get("id"),
                "recall": {str(k): round(rec[k], 6) for k in ks},
                "rr": round(rr, 6),
                "num_gold": len({_name_of(g) for g in gold if _name_of(g)}),
            }
        )

    recall = {k: (recall_sums[k] / n if n else 0.0) for k in ks}
    mrr = mrr_sum / n if n else 0.0

    out: dict[str, Any] = {
        "ok": True,
        "n": n,
        "ks": ks,
        "recall": {str(k): round(recall[k], 6) for k in ks},
        "MRR": round(mrr, 6),
        "per_example": per_example,
    }
    for k in ks:
        out[f"R@{k}"] = round(recall[k], 6)
    return out


def evaluate_predictions(
    predictions: Sequence[dict[str, Any]],
    gold: dict[str, Iterable[Any]] | None = None,
    ks: Sequence[int] = DEFAULT_KS,
    *,
    id_key: str = "id",
    retrieved_key: str = "retrieved_premises",
    gold_key: str = "all_pos_premises",
) -> dict[str, Any]:
    """Score a ReProver-style ``predictions`` list.

    Each prediction has an id, a ranked ``retrieved_premises`` list, and either
    an inline gold list (``all_pos_premises``) or an entry in the ``gold`` map
    keyed by id. Normalises to :func:`evaluate`'s example shape.
    """
    examples: list[dict[str, Any]] = []
    for pred in predictions:
        pid = pred.get(id_key)
        retrieved = pred.get(retrieved_key) or []
        if gold is not None and pid in gold:
            gold_list = gold[pid]
        else:
            gold_list = pred.get(gold_key) or []
        examples.append({"id": pid, "retrieved": retrieved, "gold": gold_list})
    return evaluate(examples, ks)


# --------------------------------------------------------------------------- #
# Entry point.
# --------------------------------------------------------------------------- #
def run(
    examples: Sequence[dict[str, Any]] | None = None,
    predictions: Sequence[dict[str, Any]] | None = None,
    gold: dict[str, Iterable[Any]] | None = None,
    ks: Sequence[int] | None = None,
) -> dict[str, Any]:
    """JSON-in/JSON-out: evaluate either ``examples`` or ``predictions``."""
    ks = list(ks) if ks else list(DEFAULT_KS)
    if predictions is not None:
        return evaluate_predictions(predictions, gold, ks)
    if examples is not None:
        return evaluate(examples, ks)
    return {"ok": False, "stderr": "provide 'examples' or 'predictions'"}


def main() -> None:
    if len(sys.argv) >= 2 and os.path.exists(sys.argv[1]):
        with open(sys.argv[1], encoding="utf-8") as fh:
            req = json.load(fh)
    else:
        req = json.load(sys.stdin)
    result = run(
        examples=req.get("examples"),
        predictions=req.get("predictions"),
        gold=req.get("gold"),
        ks=req.get("ks"),
    )
    print(json.dumps(result))
    raise SystemExit(0 if result.get("ok") else 1)


if __name__ == "__main__":
    main()
