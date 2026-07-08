"""ReProver-style premise retriever training scaffold (config + data recipe).

Ports the ReProver / LeanDojo-v2 retriever training recipe
(``docs/resource-mining/reverify/LeanDojo-ReProver.md``): a ByT5-small
bi-encoder trained with an **MSE multi-positive** loss over a
``batch x batch*(1+num_negatives)`` label matrix, with **position-gated hard
negatives** (in-file premises earlier than the theorem + random imported ones)
and **in-batch false-negative de-labeling** (flip a negative's label to 1 when it
is another example's positive).

This module owns the *config* and the *data recipe* (hard-negative mining + label
matrix) -- the parts that are validated offline with no torch / GPU. The real
training job builds the ByT5 encoder and optimizes ``F.mse_loss(similarity,
label)``; here ``dry_run`` proves the dataset + config are internally consistent.

Documented hyperparameters (ReProver ``retrieval/confs/cli_lean4_random.yaml``):
``google/byt5-small``, ``lr 1e-4``, ``warmup 2000``, ``num_retrieved 100``,
``max_seq_len 1024``, ``batch_size 8`` / ``eval 64``, ``num_negatives 3``,
``num_in_file_negatives 1``, ``max_steps 800000``, ``seed 3407``, ``bf16-mixed``,
DeepSpeed stage-2, ``gradient_clip_val 1.0``.
"""
from __future__ import annotations

import json
import random
import sys
from typing import Any, Optional, Sequence


def retriever_config(**overrides: Any) -> dict[str, Any]:
    """ReProver ByT5 retriever training config. Documented defaults; overrides
    replace any key. Never imports a trainer."""
    config: dict[str, Any] = {
        "model": "google/byt5-small",
        "loss": "mse_multipositive",
        "learning_rate": 1e-4,
        "warmup_steps": 2000,
        "num_retrieved": 100,
        "max_seq_len": 1024,
        "batch_size": 8,
        "eval_batch_size": 64,
        "num_negatives": 3,
        "num_in_file_negatives": 1,
        "max_steps": 800000,
        "seed": 3407,
        "precision": "bf16-mixed",
        "strategy": "deepspeed_stage_2",
        "gradient_clip_val": 1.0,
        "monitor": "Recall@10_val",
        "early_stopping_patience": 5,
    }
    config.update(overrides)
    return config


def mine_hard_negatives(
    corpus: Sequence[dict[str, Any]],
    theorem_pos: int,
    positives: Sequence[str],
    *,
    num_negatives: int = 3,
    num_in_file_negatives: int = 1,
    rng: Optional[random.Random] = None,
) -> list[str]:
    """Position-gated hard-negative mining (ReProver ``datamodule.py:99-128``).

    Each corpus premise is ``{"name", "end", "in_file"}``. In-file negatives are
    premises defined *earlier* in the same file (``end < theorem_pos``, accessible
    but wrong); out-of-file negatives are the rest (transitively imported).
    Positives are excluded. Sample ``num_in_file_negatives`` from the in-file
    pool and the remainder randomly from the out-of-file pool.
    """
    rng = rng or random.Random(0)
    pos = set(positives)
    in_file = [
        p["name"]
        for p in corpus
        if p.get("in_file") and int(p.get("end", 0)) < theorem_pos and p["name"] not in pos
    ]
    # Out-of-file negatives are transitively-imported premises (not in-file).
    # An in-file premise defined AFTER the theorem is inaccessible and is not a
    # candidate at all (neither in-file nor imported).
    out_file = [
        p["name"] for p in corpus if not p.get("in_file") and p["name"] not in pos
    ]
    k_in = min(num_in_file_negatives, len(in_file))
    negs = rng.sample(in_file, k_in) if k_in else []
    k_out = min(num_negatives - k_in, len(out_file))
    if k_out > 0:
        negs += rng.sample(out_file, k_out)
    return negs


def build_label_matrix(
    batch_positives: Sequence[Sequence[str]],
    batch_negatives: Sequence[Sequence[str]],
) -> dict[str, Any]:
    """Build the ReProver MSE multi-positive label matrix.

    Column layout = all examples' positives (one each, in-batch) followed by all
    examples' explicit hard negatives. ``label[i][j] = 1.0`` iff column ``j``'s
    premise is a positive of example ``i`` -- so an in-batch premise that is
    another example's positive is **de-labeled to 1** for that example too,
    killing false negatives (``datamodule.py:164-173``).

    Returns ``{columns, labels}`` where ``columns`` is the premise per column and
    ``labels`` is the ``batch x columns`` 0/1 matrix.
    """
    n = len(batch_positives)
    # column premises: each example contributes its (first) positive, then negs.
    columns: list[str] = []
    for pos in batch_positives:
        columns.append(pos[0] if pos else "")
    for negs in batch_negatives:
        columns.extend(negs)

    pos_sets = [set(p) for p in batch_positives]
    labels: list[list[float]] = []
    for i in range(n):
        row = [1.0 if col and col in pos_sets[i] else 0.0 for col in columns]
        labels.append(row)
    return {"columns": columns, "labels": labels}


def dry_run(
    examples: Sequence[dict[str, Any]],
    config: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Validate the retriever dataset + config offline.

    ``examples`` are ``{"positives": [...], "corpus": [...], "theorem_pos": int}``.
    Mines negatives for each, builds the batch label matrix, and checks the
    multi-positive invariant (every example has at least one positive column set).
    No torch, no GPU.
    """
    config = config or retriever_config()
    rng = random.Random(config.get("seed", 3407))
    batch_pos: list[list[str]] = []
    batch_neg: list[list[str]] = []
    for ex in examples:
        positives = list(ex.get("positives", []))
        negs = mine_hard_negatives(
            ex.get("corpus", []),
            int(ex.get("theorem_pos", 0)),
            positives,
            num_negatives=config["num_negatives"],
            num_in_file_negatives=config["num_in_file_negatives"],
            rng=rng,
        )
        batch_pos.append(positives)
        batch_neg.append(negs)
    matrix = build_label_matrix(batch_pos, batch_neg)
    row_sums = [sum(r) for r in matrix["labels"]]
    return {
        "ok": True,
        "dry_run": True,
        "reason": "no_trainer_backend",
        "batch_size": len(examples),
        "num_columns": len(matrix["columns"]),
        "positives_per_row": row_sums,
        "all_rows_have_positive": all(s >= 1.0 for s in row_sums) if examples else True,
        "would_run": {"model": config.get("model"), "loss": config.get("loss")},
    }


def run(request: dict[str, Any]) -> dict[str, Any]:
    op = request.get("op", "config")
    if op == "config":
        return retriever_config(**request.get("overrides", {}))
    if op == "mine_negatives":
        return {
            "negatives": mine_hard_negatives(
                request["corpus"],
                int(request["theorem_pos"]),
                request.get("positives", []),
                num_negatives=int(request.get("num_negatives", 3)),
                num_in_file_negatives=int(request.get("num_in_file_negatives", 1)),
                rng=random.Random(request.get("seed", 0)),
            )
        }
    if op == "label_matrix":
        return build_label_matrix(request["batch_positives"], request["batch_negatives"])
    if op == "dry_run":
        return dry_run(request.get("examples", []), request.get("config"))
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
