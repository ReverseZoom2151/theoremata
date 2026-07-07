"""Two-stage retrieval cascade: lexical/BM25 recall -> LM rerank.

Composes the retrieval stack into ReProver's classic recall→rerank shape:

1. **First stage (recall).** A cheap, high-recall lexical retriever over the
   premise corpus — BM25 (:mod:`theoremata_tools.bm25_retriever`) or the hybrid
   lexical ranker (:mod:`theoremata_tools.retrieval`) — restricted to premises
   that are *accessible* at the theorem position (import-DAG masking via
   :mod:`theoremata_tools.accessible_premises`). This over-fetches ``first_k``
   candidates so recall is high.
2. **Second stage (rerank).** The LM-as-scorer reranker
   (:func:`theoremata_tools.reranker.rerank`) reorders the shortlist by model
   relevance and keeps the top ``k``. Mock-mode compatible
   (``THEOREMATA_MODEL_MOCK=1``) and degrades to first-stage order when no model
   is available.

The cascade is the precision-recall tradeoff done right: a fast lexical net
maximises recall, then an expensive judge is spent only on the shortlist.

``retrieve_cascade`` is the in-memory entry; ``run`` is the JSON-in/JSON-out
worker entry. Both emit a stable contract with per-stage telemetry.
Standard-library only (optional ``rank_bm25`` for the BM25 stage).
"""
from __future__ import annotations

import json
import os
import sys
from typing import Any, Callable, Optional

from . import accessible_premises
from . import bm25_retriever
from . import reranker as _reranker
from . import retrieval as _retrieval


def _first_stage(
    stage: str,
) -> tuple[Callable[..., list[dict[str, Any]]], str]:
    """Resolve the first-stage recall retriever + a label."""
    if stage == "hybrid":
        def _hybrid(query, decls, limit, head_index=None):
            return _retrieval.retrieve(query, decls, head_index=head_index, limit=limit)

        return _hybrid, "hybrid"
    # default: BM25 lexical baseline.
    def _bm25(query, decls, limit, head_index=None):
        return bm25_retriever.retrieve(query, decls, limit=limit)

    return _bm25, "bm25"


def retrieve_cascade(
    query: str,
    decls: list[dict[str, Any]],
    *,
    imports: Optional[list[str]] = None,
    file_module: Optional[str] = None,
    dag: Optional[dict[str, Any]] = None,
    theorem_line: Optional[int] = None,
    theorem_module: Optional[str] = None,
    first_stage: str = "bm25",
    first_k: int = 50,
    k: int = 10,
    head_index: Optional[dict[str, Any]] = None,
    mask: bool = True,
    scorer: Optional[_reranker.Scorer] = None,
    samples: int = 1,
    cache: Optional[_reranker.ScoreCache] = None,
    env: Optional[dict[str, str]] = None,
) -> dict[str, Any]:
    """Run the recall→rerank cascade over an in-memory premise corpus.

    Parameters
    ----------
    query : str
        Proof obligation / goal to retrieve premises for.
    decls : list of ``{name, module, ...}``
        The premise corpus.
    imports, file_module, dag, theorem_line, theorem_module :
        Accessibility-masking inputs, forwarded to
        :func:`accessible_premises.filter_accessible`. Masking is applied when
        ``mask`` is true and ``imports`` (or ``dag``) is given.
    first_stage : {"bm25", "hybrid"}
        First-stage recall retriever.
    first_k : int
        How many candidates the recall stage over-fetches for the reranker.
    k : int
        How many reranked premises to return.
    head_index : dict, optional
        Head-symbol index, used by the ``hybrid`` first stage.
    scorer, samples, cache, env :
        Forwarded to :func:`reranker.rerank` (mock-mode honoured via ``env``).

    Returns
    -------
    dict
        Stable contract::

            {"ok": true, "op": "cascade", "query": ...,
             "first_stage": "bm25"|"hybrid", "reranked": true,
             "degraded": <bool>, "model": <str|null>,
             "stages": {"corpus": n, "accessible": n, "recall": n, "returned": n},
             "count": <n>, "results": [<reranked candidate records>]}
    """
    corpus_n = len(decls)

    # Stage 0: accessibility masking (import-DAG closure + same-file order).
    masked = decls
    if mask and (imports is not None or dag is not None):
        masked = accessible_premises.filter_accessible(
            decls,
            imports=list(imports or ["Init"]),
            file_path=theorem_module,
            theorem_line=theorem_line,
            dag=dag,
            file_module=file_module,
        )

    # Stage 1: first-stage recall (over-fetch first_k).
    retriever, stage_label = _first_stage(first_stage)
    candidates = retriever(query, masked, first_k, head_index=head_index)

    # Stage 2: LM rerank, keep top k.
    reranked = _reranker.rerank(
        query,
        candidates,
        k,
        scorer=scorer,
        samples=samples,
        cache=cache,
        env=env,
    )

    return {
        "ok": True,
        "op": "cascade",
        "query": query,
        "first_stage": stage_label,
        "reranked": not reranked.get("degraded", False),
        "degraded": bool(reranked.get("degraded", False)),
        "reason": reranked.get("reason", ""),
        "model": reranked.get("model"),
        "stages": {
            "corpus": corpus_n,
            "accessible": len(masked),
            "recall": len(candidates),
            "returned": len(reranked.get("results", [])),
        },
        "count": len(reranked.get("results", [])),
        "results": reranked.get("results", []),
    }


def run(
    root: str | None = None,
    imports: list[str] | None = None,
    query: str = "",
    *,
    first_stage: str = "bm25",
    first_k: int = 50,
    k: int = 10,
    theorem_module: str | None = None,
    theorem_line: int | None = None,
    file_module: str | None = None,
    mask: bool = True,
    samples: int = 1,
    rebuild: bool = False,
    lean_bin: str | None = None,
    timeout: float = 300.0,
    decls: list[dict[str, Any]] | None = None,
    dag: dict[str, Any] | None = None,
    env: dict[str, str] | None = None,
) -> dict[str, Any]:
    """JSON entry point: build/load the corpus, then run the cascade.

    Pass ``decls`` to run over an in-memory corpus (offline/testing); otherwise
    the cached Lean decl dump is loaded via ``retrieval.build_or_load`` (reusing
    the same ~40s cache as ``retrieval``/``bm25_retriever``).
    """
    head_index: dict[str, Any] | None = None
    if decls is None:
        index = _retrieval.build_or_load(
            root, imports, rebuild=rebuild, lean_bin=lean_bin, timeout=timeout
        )
        decls = index.get("decls", [])
        head_index = index.get("head_index")
        if not index.get("ok") and not decls:
            return {
                "ok": False,
                "op": "cascade",
                "query": query,
                "count": 0,
                "results": [],
                "stderr": index.get("stderr", "index unavailable"),
            }

    return retrieve_cascade(
        query,
        decls,
        imports=imports,
        file_module=file_module,
        dag=dag,
        theorem_line=theorem_line,
        theorem_module=theorem_module,
        first_stage=first_stage,
        first_k=first_k,
        k=k,
        head_index=head_index,
        mask=mask,
        samples=samples,
        env=env,
    )


def main() -> None:
    if len(sys.argv) >= 2 and os.path.exists(sys.argv[1]):
        with open(sys.argv[1], encoding="utf-8") as fh:
            req = json.load(fh)
    else:
        req = json.load(sys.stdin)
    result = run(
        root=req.get("root"),
        imports=req.get("imports"),
        query=req.get("query", ""),
        first_stage=req.get("first_stage", "bm25"),
        first_k=int(req.get("first_k", 50)),
        k=int(req.get("k", 10)),
        theorem_module=req.get("theorem_module"),
        theorem_line=req.get("theorem_line"),
        file_module=req.get("file_module"),
        mask=bool(req.get("mask", True)),
        samples=int(req.get("samples", 1)),
        rebuild=bool(req.get("rebuild", False)),
        lean_bin=req.get("lean_bin"),
        timeout=float(req.get("timeout", 300.0)),
        decls=req.get("decls"),
    )
    print(json.dumps(result))
    raise SystemExit(0 if result.get("ok") else 1)


if __name__ == "__main__":
    main()
