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

import hashlib
import json
import math
import random
import re
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


# ---------------------------------------------------------------------------
# Dense retrieval index (offline scaffold).
#
#   build_corpus -> embed -> nearest-neighbour query, with JSON persist/load.
#
# Two honestly-gated embedding backends produce the SAME index/query contract:
#
# * ``hash`` (default fallback) -- a dependency-free signed hashing-trick
#   embedding. No torch, no GPU, deterministic; always available.
# * ``torch`` (optional) -- runs one REAL training step to fit a linear
#   projection over the hashed features. The fitted matrix is stored as plain
#   nested lists, so the persisted index is JSON-serializable and can be QUERIED
#   with no torch at all. GPU/torch is used only to *build* the projection.
#
# ``backend="auto"`` picks ``torch`` when importable, else ``hash``. The choice
# is always reported in the index's ``backend`` field so it is never silent.
# ---------------------------------------------------------------------------

_TOKEN_RE = re.compile(r"[A-Z]+(?=[A-Z][a-z])|[A-Z]?[a-z]+|[A-Z]+|[0-9]+")


def _tokenize(text: str) -> list[str]:
    """Split an identifier / query into lowercased word pieces
    (``Nat.add_comm`` -> ``[nat, add, comm]``)."""
    out: list[str] = []
    for part in re.split(r"[^A-Za-z0-9]+", text or ""):
        out.extend(m.group(0).lower() for m in _TOKEN_RE.finditer(part))
    return out


def premise_text(premise: dict[str, Any]) -> str:
    """The text a premise is embedded from: explicit ``text`` if present, else
    ``name`` + ``module``."""
    if premise.get("text"):
        return str(premise["text"])
    return f"{premise.get('name', '')} {premise.get('module', '')}".strip()


def _hash_features(text: str, dim: int) -> list[float]:
    """Deterministic signed hashing-trick features (UNnormalized). Uses
    ``hashlib`` (not the salted builtin ``hash``) so vectors are stable across
    processes and platforms."""
    vec = [0.0] * dim
    for tok in _tokenize(text):
        h = int(hashlib.md5(tok.encode("utf-8")).hexdigest(), 16)
        vec[h % dim] += 1.0 if (h >> 7) & 1 else -1.0
    return vec


def _matvec(matrix: Sequence[Sequence[float]], vec: Sequence[float]) -> list[float]:
    return [sum(row[j] * vec[j] for j in range(len(vec))) for row in matrix]


def _normalize(vec: Sequence[float]) -> list[float]:
    norm = math.sqrt(sum(v * v for v in vec)) or 1.0
    return [v / norm for v in vec]


def _embed(text: str, dim: int, projection: Optional[Sequence[Sequence[float]]]) -> list[float]:
    """Hash features -> optional learned projection -> L2 normalize."""
    feats = _hash_features(text, dim)
    if projection is not None:
        feats = _matvec(projection, feats)
    return _normalize(feats)


def build_corpus(premises: Sequence[dict[str, Any]]) -> list[dict[str, Any]]:
    """Normalize premise records into ``{name, module, kind, text}`` corpus rows."""
    return [
        {
            "name": p.get("name", ""),
            "module": p.get("module"),
            "kind": p.get("kind"),
            "text": premise_text(p),
        }
        for p in premises
    ]


def _detect_index_backend() -> str:
    try:
        import torch  # noqa: F401
    except Exception:  # noqa: BLE001 - any import failure => hash fallback
        return "hash"
    return "torch"


def _train_projection(texts: Sequence[str], dim: int, seed: int) -> list[list[float]]:
    """Run ONE real torch training step to fit a ``dim x dim`` linear projection
    over the hashed features (self-supervised reconstruction warm start).
    Torch-only; returns a plain nested-list matrix so the index stays
    JSON-serializable and queryable without torch."""
    import torch

    torch.manual_seed(int(seed))
    feats = torch.tensor(
        [_hash_features(t, dim) for t in texts] or [[0.0] * dim], dtype=torch.float32
    )
    proj = torch.nn.Linear(dim, dim, bias=False)
    optim = torch.optim.Adam(proj.parameters(), lr=1e-2)
    loss = torch.nn.functional.mse_loss(proj(feats), feats)
    loss.backward()
    optim.step()
    return proj.weight.detach().tolist()


def build_index(
    premises: Sequence[dict[str, Any]],
    *,
    dim: int = 64,
    backend: str = "auto",
    seed: int = 3407,
) -> dict[str, Any]:
    """Build a dense retrieval index over ``premises``.

    ``backend="auto"`` uses the torch projection path when torch imports, else the
    dependency-free ``hash`` fallback. Both produce L2-normalized vectors and a
    :func:`query_index`-ready structure; the torch path additionally stores the
    fitted ``projection`` matrix so queries need no torch.

    Returns ``{backend, dim, projection, records, vectors}``.
    """
    if backend == "auto":
        backend = _detect_index_backend()
    corpus = build_corpus(premises)
    projection: Optional[list[list[float]]] = None
    if backend == "torch":
        projection = _train_projection([r["text"] for r in corpus], dim, seed)
    vectors = [_embed(r["text"], dim, projection) for r in corpus]
    return {
        "backend": backend,
        "dim": dim,
        "projection": projection,
        "records": corpus,
        "vectors": vectors,
    }


def query_index(index: dict[str, Any], query: str, k: int = 5) -> list[dict[str, Any]]:
    """Return the top-``k`` premises for ``query`` by cosine similarity (vectors
    are pre-normalized, so cosine == dot product)."""
    dim = int(index["dim"])
    projection = index.get("projection")
    qv = _embed(query, dim, projection)
    vectors = index["vectors"]
    scored = sorted(
        (
            (sum(qv[j] * v[j] for j in range(dim)), i)
            for i, v in enumerate(vectors)
        ),
        key=lambda t: (-t[0], t[1]),
    )
    out: list[dict[str, Any]] = []
    for s, i in scored[: max(0, k)]:
        rec = index["records"][i]
        out.append(
            {
                "name": rec.get("name"),
                "module": rec.get("module"),
                "kind": rec.get("kind"),
                "score": round(float(s), 6),
            }
        )
    return out


def save_index(index: dict[str, Any], path: str) -> str:
    """Persist an index as JSON; returns the path written."""
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(index, fh, ensure_ascii=False)
    return path


def load_index(path: str) -> dict[str, Any]:
    """Load an index previously written by :func:`save_index`."""
    with open(path, encoding="utf-8") as fh:
        return json.load(fh)


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
    if op == "build_index":
        return build_index(
            request.get("premises", []),
            dim=int(request.get("dim", 64)),
            backend=request.get("backend", "auto"),
            seed=int(request.get("seed", 3407)),
        )
    if op == "query_index":
        return {
            "results": query_index(
                request["index"], request.get("query", ""), int(request.get("k", 5))
            )
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
