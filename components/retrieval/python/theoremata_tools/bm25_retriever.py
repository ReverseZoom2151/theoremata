"""BM25 lexical-recall baseline for Mathlib premise retrieval (ReProver §BM25).

A **zero-training** first-stage retriever over the premise corpus, mirroring
ReProver's ``retrieval/bm25`` baseline. It ranks declarations for a query purely
by Okapi BM25 term overlap over the tokenised premise text (declaration name +
module), with no embeddings and no learned weights. This is the recall floor the
learned/dense retriever must beat, and the natural first stage for the cascade.

Backends
--------
* ``rank_bm25`` (``BM25Okapi``) when the package is importable — the reference
  implementation.
* A self-contained Okapi BM25 fallback built from ``math`` + ``collections``
  when ``rank_bm25`` is absent. Both backends see the *same* tokenised corpus, so
  the contract is identical; ``run``/``retrieve`` report which one was used via
  the ``backend`` field so the choice is never silent.

Contract
--------
Same query→candidates shape as :func:`theoremata_tools.retrieval.run`::

    {"ok", "op", "query", "root", "imports", "cached", "backend",
     "count", "results": [{"name", "module", "kind", "score"}, ...]}

Standard-library only (plus optional ``rank_bm25``). The premise corpus is the
same cached decl dump used by ``retrieval`` (Layer B), so this reuses the ~40s
Lean cache rather than rebuilding it.
"""
from __future__ import annotations

import json
import math
import os
import sys
from collections import Counter
from typing import Any

from . import accessible_premises
from . import retrieval as _retrieval

# Name tokens carry more signal than module tokens; duplicate them so a query
# matching the declaration name outranks one matching only its module path.
_NAME_REPEAT = 2
_MODULE_REPEAT = 1

# Okapi BM25 parameters (textbook / rank_bm25 defaults).
_K1 = 1.5
_B = 0.75


def _detect_backend() -> str:
    """Return ``"rank_bm25"`` when the package imports, else ``"fallback"``."""
    try:
        import rank_bm25  # noqa: F401
    except Exception:  # noqa: BLE001 - any import failure => fallback
        return "fallback"
    return "rank_bm25"


def premise_tokens(decl: dict[str, Any]) -> list[str]:
    """Tokenised premise text for BM25: weighted name tokens + module tokens.

    Reuses the shared retrieval tokeniser so BM25 and the hybrid ranker split
    identifiers identically (``Nat.succ_le_succ`` -> ``[nat, succ, le, succ]``).
    """
    name = decl.get("name") or ""
    module = decl.get("module") or ""
    toks: list[str] = []
    toks.extend(_retrieval.tokenize(name) * _NAME_REPEAT)
    toks.extend(_retrieval.tokenize(module) * _MODULE_REPEAT)
    return toks


# --------------------------------------------------------------------------- #
# Self-contained Okapi BM25 fallback.
# --------------------------------------------------------------------------- #
class _OkapiBM25:
    """Minimal Okapi BM25 over pre-tokenised documents (rank_bm25-compatible).

    Uses the ``ln(1 + (N - n + 0.5)/(n + 0.5))`` IDF so every term keeps a
    non-negative weight (a term present in every doc still scores > 0), which
    keeps the baseline's ordering well defined on tiny corpora.
    """

    def __init__(self, corpus: list[list[str]]) -> None:
        self.doc_freqs: list[Counter] = []
        self.doc_len: list[int] = []
        df: Counter = Counter()
        total_len = 0
        for doc in corpus:
            tf = Counter(doc)
            self.doc_freqs.append(tf)
            self.doc_len.append(len(doc))
            total_len += len(doc)
            for term in tf:
                df[term] += 1
        self.n_docs = len(corpus)
        self.avgdl = (total_len / self.n_docs) if self.n_docs else 0.0
        self.idf: dict[str, float] = {}
        for term, freq in df.items():
            self.idf[term] = math.log(1.0 + (self.n_docs - freq + 0.5) / (freq + 0.5))

    def get_scores(self, query: list[str]) -> list[float]:
        avgdl = self.avgdl or 1.0
        scores = [0.0] * self.n_docs
        q_terms = set(query)
        for i in range(self.n_docs):
            tf = self.doc_freqs[i]
            dl = self.doc_len[i]
            s = 0.0
            for term in q_terms:
                f = tf.get(term)
                if not f:
                    continue
                denom = f + _K1 * (1.0 - _B + _B * dl / avgdl)
                s += self.idf.get(term, 0.0) * (f * (_K1 + 1.0)) / denom
            scores[i] = s
        return scores


def _build_scorer(corpus: list[list[str]], backend: str):
    """Return an object exposing ``get_scores(query_tokens) -> list[float]``."""
    if backend == "rank_bm25":
        try:
            from rank_bm25 import BM25Okapi

            # rank_bm25 divides by corpus length; guard the empty-corpus case.
            if corpus:
                return BM25Okapi(corpus, k1=_K1, b=_B)
        except Exception:  # noqa: BLE001 - fall through to the pure-Python path
            pass
    return _OkapiBM25(corpus)


# --------------------------------------------------------------------------- #
# Ranking.
# --------------------------------------------------------------------------- #
def retrieve(
    query: str,
    decls: list[dict[str, Any]],
    limit: int = 20,
    *,
    backend: str | None = None,
) -> list[dict[str, Any]]:
    """Rank ``decls`` for ``query`` by BM25 and return the top ``limit``.

    Returns ``[{name, module, kind, score}, ...]`` sorted by descending BM25
    score. Only positively-scored declarations are returned (a zero-overlap
    premise carries no lexical evidence), so a small precise set beats a large
    marginal one — matching ``retrieval.retrieve``'s contract.
    """
    q_terms = _retrieval.tokenize(query)
    if not decls or not q_terms:
        return []
    backend = backend or _detect_backend()
    corpus = [premise_tokens(d) for d in decls]
    scorer = _build_scorer(corpus, backend)
    scores = scorer.get_scores(q_terms)

    ranked: list[tuple[float, int, dict[str, Any]]] = []
    for i, d in enumerate(decls):
        if not (d.get("name")):
            continue
        s = float(scores[i])
        if s > 0.0:
            ranked.append((s, i, d))
    ranked.sort(key=lambda t: (-t[0], t[1]))

    out: list[dict[str, Any]] = []
    for s, _i, d in ranked[: max(0, limit)]:
        out.append(
            {
                "name": d.get("name"),
                "module": d.get("module"),
                "kind": d.get("kind"),
                "score": round(s, 6),
            }
        )
    return out


# --------------------------------------------------------------------------- #
# Entry point (mirrors retrieval.run).
# --------------------------------------------------------------------------- #
def run(
    root: str | None = None,
    imports: list[str] | None = None,
    query: str = "",
    limit: int = 20,
    *,
    op: str = "retrieve",
    rebuild: bool = False,
    lean_bin: str | None = None,
    timeout: float = 300.0,
    theorem_module: str | None = None,
    theorem_line: int | None = None,
    decls: list[dict[str, Any]] | None = None,
    dag: dict[str, Any] | None = None,
    file_module: str | None = None,
) -> dict[str, Any]:
    """Build/load the premise corpus, then answer one BM25 retrieval request.

    Mirrors :func:`retrieval.run`. ``op="retrieve"`` ranks all premises;
    ``op="accessible_retrieve"`` applies import-DAG accessibility masking first.
    Pass ``decls`` directly to rank an in-memory corpus (offline / testing);
    otherwise the cached Lean decl dump is loaded via ``retrieval.build_or_load``.
    """
    backend = _detect_backend()
    cached = False
    root_abs = os.path.abspath(root) if root else None
    imps = list(imports or ["Init"])

    if decls is None:
        index = _retrieval.build_or_load(
            root, imports, rebuild=rebuild, lean_bin=lean_bin, timeout=timeout
        )
        cached = bool(index.get("cached"))
        decls = index.get("decls", [])
        if not index.get("ok") and not decls:
            return {
                "ok": False,
                "op": op,
                "query": query,
                "backend": backend,
                "count": 0,
                "results": [],
                "stderr": index.get("stderr", "index unavailable"),
            }

    if op == "accessible_retrieve":
        decls = accessible_premises.filter_accessible(
            decls,
            imports=imps,
            file_path=theorem_module,
            theorem_line=theorem_line,
            dag=dag,
            file_module=file_module,
        )
    elif op != "retrieve":
        return {"ok": False, "op": op, "backend": backend, "stderr": f"unknown op: {op}"}

    results = retrieve(query, decls, limit=limit, backend=backend)
    return {
        "ok": True,
        "op": op,
        "query": query,
        "root": root_abs,
        "imports": imps,
        "cached": cached,
        "backend": backend,
        "count": len(results),
        "results": results,
    }


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
        limit=int(req.get("limit", 20)),
        op=req.get("op", "retrieve"),
        rebuild=bool(req.get("rebuild", False)),
        lean_bin=req.get("lean_bin"),
        timeout=float(req.get("timeout", 300.0)),
        theorem_module=req.get("theorem_module"),
        theorem_line=req.get("theorem_line"),
        decls=req.get("decls"),
        file_module=req.get("file_module"),
    )
    print(json.dumps(result))
    raise SystemExit(0 if result.get("ok") else 1)


if __name__ == "__main__":
    main()
