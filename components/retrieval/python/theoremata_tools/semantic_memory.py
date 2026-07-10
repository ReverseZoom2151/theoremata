"""Semantic + episodic retrieval over Theoremata's verified-lemma library.

Closes a gap called out by the agentic-patterns mining (docs/agentic-patterns-mining/A3):
retrieval over our *verified-lemma library* and *past-solve trajectories* was
BM25 / lexical only. This module adds two dense, embedding-based recall paths
that sit alongside the lexical cascade:

* :class:`SemanticLemmaIndex` — a dense index over lemma **statements**. Given a
  proof goal it returns the lemmas whose *meaning* (embedding) is closest, so a
  goal phrased differently from a lemma's name/statement can still surface it.
  This complements :mod:`theoremata_tools.bm25_retriever` (exact term overlap)
  and :func:`theoremata_tools.cascade.retrieve_cascade` (recall→rerank): the
  semantic hits can be *unioned* into the first-stage recall shortlist before the
  LM reranker, widening recall without a model at recall time.

* :class:`EpisodicRecall` — a store of past ``goal -> successful proof/trajectory``
  **episodes**. ``recall(goal, k)`` returns the most similar *previously solved*
  goals and how they were solved, so the agent can condition on "how did I solve
  a similar goal before" — the inference-time trajectory memory that item A3
  asks for. The recalled episodes are meant to be dropped into the agent's
  context as worked exemplars.

The embedding seam
------------------
Both classes take an ``embed(text) -> vector`` seam. The **default is a fully
offline, deterministic hashing bag-of-words vectorizer** (:func:`hashing_embed`)
with cosine similarity — no model, no network, pure standard library — so the
index works out of the box and tests are deterministic. A real sentence /
math embedder (the model-gated upgrade) plugs in by passing ``embed=...``; the
ranking machinery is identical either way.

Standard-library only. No numpy required (a hand-rolled cosine over ``list[float]``).
"""
from __future__ import annotations

import hashlib
import json
import math
import sys
from typing import Any, Callable, Optional, Sequence

from . import retrieval as _retrieval

# An embedder maps text -> a fixed-length dense vector (list of floats).
Vector = list[float]
Embedder = Callable[[str], Vector]

# Default dimensionality of the offline hashing vectorizer. Large enough that
# token hash collisions are rare on lemma-sized vocabularies, small enough to
# stay cheap in pure Python.
DEFAULT_DIM = 256


# --------------------------------------------------------------------------- #
# Offline default embedder: deterministic hashing bag-of-words.
# --------------------------------------------------------------------------- #
def _token_bucket(token: str, dim: int) -> int:
    """Stable hash of a token into ``[0, dim)`` (BLAKE2b — deterministic across
    processes, unlike Python's salted ``hash``)."""
    digest = hashlib.blake2b(token.encode("utf-8"), digest_size=8).digest()
    return int.from_bytes(digest, "big") % dim


def hashing_embed(text: str, *, dim: int = DEFAULT_DIM) -> Vector:
    """Deterministic offline bag-of-words embedding of ``text``.

    Tokenises with the shared retrieval tokeniser (so ``Nat.add_zero`` and a
    query ``"add zero"`` land on the same buckets), hashes each token into a
    fixed-width vector, and L2-normalises the result. Two texts that share
    (sub-)tokens get a high cosine; unrelated texts get ~0. No model, no
    network, fully deterministic — the seam's zero-dependency default.
    """
    vec = [0.0] * dim
    for tok in _retrieval.tokenize(text):
        vec[_token_bucket(tok, dim)] += 1.0
    return _l2_normalize(vec)


def _l2_normalize(vec: Vector) -> Vector:
    norm = math.sqrt(sum(x * x for x in vec))
    if norm <= 0.0:
        return vec
    return [x / norm for x in vec]


def cosine(a: Sequence[float], b: Sequence[float]) -> float:
    """Cosine similarity between two equal-length vectors (0.0 if degenerate)."""
    if len(a) != len(b):
        raise ValueError(f"vector length mismatch: {len(a)} != {len(b)}")
    dot = 0.0
    na = 0.0
    nb = 0.0
    for x, y in zip(a, b):
        dot += x * y
        na += x * x
        nb += y * y
    if na <= 0.0 or nb <= 0.0:
        return 0.0
    return dot / math.sqrt(na * nb)


def _default_embedder(dim: int) -> Embedder:
    """The zero-dependency offline embedder bound to ``dim``."""
    def _embed(text: str) -> Vector:
        return hashing_embed(text, dim=dim)

    return _embed


# --------------------------------------------------------------------------- #
# Semantic lemma index.
# --------------------------------------------------------------------------- #
class SemanticLemmaIndex:
    """Dense embedding index over lemma statements.

    ``add(lemma_id, statement)`` embeds and stores a lemma; ``query(goal, k)``
    returns the ``k`` lemmas whose statement embedding is most cosine-similar to
    the goal, as ``[(lemma_id, score), ...]`` sorted by descending score.

    Parameters
    ----------
    embed : callable, optional
        The ``embed(text) -> vector`` seam. Defaults to the offline
        :func:`hashing_embed` vectorizer (no model). Inject a real embedder here.
    dim : int
        Dimensionality of the *default* offline embedder (ignored when ``embed``
        is injected).
    """

    def __init__(self, embed: Optional[Embedder] = None, *, dim: int = DEFAULT_DIM) -> None:
        self._embed: Embedder = embed if embed is not None else _default_embedder(dim)
        self.dim = dim
        # Parallel arrays keep insertion order stable for deterministic ties.
        self._ids: list[str] = []
        self._statements: list[str] = []
        self._vectors: list[Vector] = []
        self._pos: dict[str, int] = {}

    def __len__(self) -> int:
        return len(self._ids)

    def add(self, lemma_id: str, statement: str) -> None:
        """Index ``statement`` under ``lemma_id`` (re-adding an id overwrites)."""
        vec = self._embed(statement)
        if lemma_id in self._pos:
            i = self._pos[lemma_id]
            self._statements[i] = statement
            self._vectors[i] = vec
            return
        self._pos[lemma_id] = len(self._ids)
        self._ids.append(lemma_id)
        self._statements.append(statement)
        self._vectors.append(vec)

    def add_many(self, items: Sequence[tuple[str, str]]) -> None:
        """Bulk ``add`` from ``(lemma_id, statement)`` pairs."""
        for lemma_id, statement in items:
            self.add(lemma_id, statement)

    def query(self, goal: str, k: int = 5) -> list[tuple[str, float]]:
        """Return the top-``k`` ``(lemma_id, cosine_score)`` for ``goal``.

        Only positively-similar lemmas are returned (a ~0 cosine carries no
        semantic evidence). Ties break on insertion order for determinism.
        """
        if not self._ids or k <= 0:
            return []
        qv = self._embed(goal)
        scored: list[tuple[float, int, str]] = []
        for i, (lemma_id, vec) in enumerate(zip(self._ids, self._vectors)):
            s = cosine(qv, vec)
            if s > 0.0:
                scored.append((s, i, lemma_id))
        scored.sort(key=lambda t: (-t[0], t[1]))
        return [(lemma_id, round(s, 6)) for s, _i, lemma_id in scored[:k]]


# --------------------------------------------------------------------------- #
# Episodic (trajectory) recall.
# --------------------------------------------------------------------------- #
class EpisodicRecall:
    """Memory of past ``goal -> successful proof/trajectory`` episodes.

    ``add(goal, episode)`` records a solved goal and how it was solved (the
    ``episode`` is any JSON-able payload: a proof, a tactic trace, a plan, ...).
    ``recall(goal, k)`` returns the most similar past episodes so the agent can
    reuse a worked exemplar — the inference-time trajectory recall from A3.

    Same ``embed`` seam and cosine ranking as :class:`SemanticLemmaIndex`.
    """

    def __init__(self, embed: Optional[Embedder] = None, *, dim: int = DEFAULT_DIM) -> None:
        self._embed: Embedder = embed if embed is not None else _default_embedder(dim)
        self.dim = dim
        self._goals: list[str] = []
        self._episodes: list[Any] = []
        self._vectors: list[Vector] = []

    def __len__(self) -> int:
        return len(self._goals)

    def add(self, goal: str, episode: Any) -> None:
        """Record a solved ``goal`` and its ``episode`` (proof/trajectory)."""
        self._goals.append(goal)
        self._episodes.append(episode)
        self._vectors.append(self._embed(goal))

    # Convenience alias — reads well at call sites storing a success.
    remember = add

    def recall(self, goal: str, k: int = 3) -> list[dict[str, Any]]:
        """Return the top-``k`` most similar past episodes for ``goal``.

        Each hit is ``{"goal", "episode", "score"}`` (the stored goal, its
        trajectory payload, and the cosine similarity), sorted by descending
        similarity. Ties break on recency-independent insertion order.
        """
        if not self._goals or k <= 0:
            return []
        qv = self._embed(goal)
        scored: list[tuple[float, int]] = []
        for i, vec in enumerate(self._vectors):
            s = cosine(qv, vec)
            if s > 0.0:
                scored.append((s, i))
        scored.sort(key=lambda t: (-t[0], t[1]))
        out: list[dict[str, Any]] = []
        for s, i in scored[:k]:
            out.append(
                {"goal": self._goals[i], "episode": self._episodes[i], "score": round(s, 6)}
            )
        return out


# --------------------------------------------------------------------------- #
# Worker / CLI entry point.
# --------------------------------------------------------------------------- #
def run(request: dict[str, Any]) -> dict[str, Any]:
    """JSON-in/JSON-out dispatch (worker-friendly).

    Operations (``op``):

    * ``semantic_lemma_query`` — build a :class:`SemanticLemmaIndex` from
      ``lemmas`` (a list of ``{"id"|"name", "statement"}`` or ``[id, statement]``
      pairs) and return the top-``k`` for ``goal`` / ``query``::

          {"ok", "op", "count", "results": [{"lemma_id", "score"}, ...]}

    * ``episodic_recall`` — build an :class:`EpisodicRecall` from ``episodes``
      (a list of ``{"goal", "episode"}``) and return the top-``k`` for
      ``goal`` / ``query``::

          {"ok", "op", "count", "results": [{"goal", "episode", "score"}, ...]}

    The embedding is the offline default unless a caller wires an embedder in
    (the seam is a Python injection point, not a JSON one).
    """
    op = request.get("op", "semantic_lemma_query")
    goal = request.get("goal") or request.get("query") or ""
    k = int(request.get("k", 5))
    dim = int(request.get("dim", DEFAULT_DIM))

    if op == "semantic_lemma_query":
        index = SemanticLemmaIndex(dim=dim)
        for item in request.get("lemmas", []):
            if isinstance(item, dict):
                lemma_id = item.get("id") or item.get("name") or item.get("lemma_id")
                statement = item.get("statement", "")
            else:  # [id, statement] pair
                lemma_id, statement = item[0], item[1]
            if lemma_id is None:
                continue
            index.add(str(lemma_id), str(statement))
        hits = index.query(goal, k)
        return {
            "ok": True,
            "op": op,
            "count": len(hits),
            "results": [{"lemma_id": lid, "score": s} for lid, s in hits],
        }

    if op == "episodic_recall":
        recall = EpisodicRecall(dim=dim)
        for item in request.get("episodes", []):
            recall.add(str(item.get("goal", "")), item.get("episode"))
        hits = recall.recall(goal, k)
        return {"ok": True, "op": op, "count": len(hits), "results": hits}

    return {"ok": False, "op": op, "error": f"unknown semantic_memory op: {op}"}


def main() -> None:
    request = json.load(sys.stdin)
    try:
        response = run(request)
    except Exception as exc:  # noqa: BLE001 - surface as JSON
        response = {"ok": False, "error": str(exc)}
    print(json.dumps(response))
    raise SystemExit(0 if response.get("ok") else 1)


if __name__ == "__main__":
    main()
