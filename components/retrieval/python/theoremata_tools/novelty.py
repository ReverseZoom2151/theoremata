"""Novelty / prior-work checker (Erdős #728 motivation).

Given a freshly *proven* result — a statement, and optionally the key
lemmas/methods used — this scans a corpus of already-known results for close
matches and flags "this may already exist". It exists because an AI-derived
argument can turn out to be essentially a known theorem (e.g. Erdős #728, whose
method was later found to be very close to a 2014 Pomerance paper): a cheap
lexical prior-work sweep would have surfaced that overlap *before* claiming
novelty.

Two uses:

* **Warn about non-novelty** — a high-scoring match means the result is
  probably already in the literature, so the ``novelty`` verdict downgrades from
  ``likely_novel`` toward ``likely_known``.
* **Enrich exposition** — even a partial match is useful "related work" the
  write-up should cite.

Scoring reuses the shared retrieval tokeniser
(:func:`theoremata_tools.retrieval.tokenize`) and the same BM25-style idf
weighting from :mod:`theoremata_tools.retrieval`, but wraps them in a
**tf-idf cosine** so the similarity is bounded in ``[0, 1]`` and independent of
corpus size — that boundedness is what lets a fixed verdict threshold be
meaningful (a raw BM25 score is not comparable across corpora).

Contract
--------
``novelty_check`` / ``run`` return::

    {"op", "ok", "novelty", "top_score", "count",
     "matches": [{"id", "title", "ref", "score"}, ...],
     "reason", "advisory": true}

``novelty`` is one of ``{"likely_novel", "possible_overlap", "likely_known"}``.

Security
--------
The statement, methods, and every corpus record are treated as **untrusted
data**: fields are coerced to ``str``, never evaluated, and a corpus given as a
path is read as plain JSON (no code execution). Standard-library only, no
network — the "corpus" is either an in-memory list of records or a local JSON
file — and fully deterministic (stable tie-breaking by record id/index).
"""
from __future__ import annotations

import json
import math
import os
import sys
from collections import Counter
from typing import Any

from . import retrieval as _retrieval

# --------------------------------------------------------------------------- #
# Verdict thresholds on the top (best) similarity, which lives in [0, 1].
# A near-duplicate statement scores ~0.6-1.0; a partial/topical overlap ~0.25-
# 0.6; an unrelated statement well below 0.25. Tuned so a genuine near-dup is
# flagged "likely_known" while incidental shared vocabulary is not.
# --------------------------------------------------------------------------- #
_KNOWN_THRESHOLD = 0.55
_OVERLAP_THRESHOLD = 0.25

_VERDICT_LIKELY_NOVEL = "likely_novel"
_VERDICT_POSSIBLE_OVERLAP = "possible_overlap"
_VERDICT_LIKELY_KNOWN = "likely_known"

# Methods/lemmas are corroborating signal, weighted below the statement itself.
_METHODS_REPEAT = 1
_STATEMENT_REPEAT = 2


def _as_text(value: Any) -> str:
    """Coerce an untrusted field to a plain string (never evaluate it)."""
    if value is None:
        return ""
    if isinstance(value, str):
        return value
    if isinstance(value, (list, tuple)):
        return " ".join(_as_text(v) for v in value)
    return str(value)


def _query_tokens(statement: Any, methods: Any = None) -> list[str]:
    """Weighted token stream for the query result (statement + optional methods)."""
    toks: list[str] = _retrieval.tokenize(_as_text(statement)) * _STATEMENT_REPEAT
    if methods:
        toks += _retrieval.tokenize(_as_text(methods)) * _METHODS_REPEAT
    return toks


def _record_tokens(record: dict[str, Any]) -> list[str]:
    """Tokenise a corpus record's title + statement text."""
    title = _as_text(record.get("title"))
    statement = _as_text(record.get("statement"))
    return _retrieval.tokenize(title) + _retrieval.tokenize(statement)


def _load_corpus(corpus: Any) -> list[dict[str, Any]]:
    """Accept a list of records or a path to a JSON file/array of records.

    Untrusted: a path is read as JSON only. Non-dict entries are dropped so a
    malformed corpus degrades gracefully rather than raising.
    """
    if isinstance(corpus, str):
        if not os.path.exists(corpus):
            return []
        try:
            with open(corpus, encoding="utf-8") as fh:
                corpus = json.load(fh)
        except (OSError, json.JSONDecodeError):
            return []
    if isinstance(corpus, dict):
        # Allow {"records": [...]} wrapper.
        corpus = corpus.get("records", [])
    if not isinstance(corpus, (list, tuple)):
        return []
    return [r for r in corpus if isinstance(r, dict)]


def _build_idf(doc_tokens: list[list[str]]) -> dict[str, float]:
    """BM25-style idf (matching retrieval._build_corpus) over the doc corpus."""
    df: dict[str, int] = {}
    for toks in doc_tokens:
        for term in set(toks):
            df[term] = df.get(term, 0) + 1
    n = len(doc_tokens)
    idf: dict[str, float] = {}
    for term, freq in df.items():
        idf[term] = math.log(1.0 + (n - freq + 0.5) / (freq + 0.5))
    return idf


def _tfidf_vec(tokens: list[str], idf: dict[str, float]) -> dict[str, float]:
    """tf-idf weight vector for a token stream against a shared idf table."""
    vec: dict[str, float] = {}
    for term, tf in Counter(tokens).items():
        w = idf.get(term)
        if w:
            vec[term] = tf * w
    return vec


def _cosine(a: dict[str, float], b: dict[str, float]) -> float:
    """Cosine similarity of two sparse weight vectors, in [0, 1]."""
    if not a or not b:
        return 0.0
    # Iterate the smaller vector for the dot product.
    if len(a) > len(b):
        a, b = b, a
    dot = 0.0
    for term, wa in a.items():
        wb = b.get(term)
        if wb:
            dot += wa * wb
    if dot <= 0.0:
        return 0.0
    na = math.sqrt(sum(w * w for w in a.values()))
    nb = math.sqrt(sum(w * w for w in b.values()))
    if na == 0.0 or nb == 0.0:
        return 0.0
    return dot / (na * nb)


def _verdict(top_score: float) -> str:
    if top_score >= _KNOWN_THRESHOLD:
        return _VERDICT_LIKELY_KNOWN
    if top_score >= _OVERLAP_THRESHOLD:
        return _VERDICT_POSSIBLE_OVERLAP
    return _VERDICT_LIKELY_NOVEL


def _reason(verdict: str, top_score: float, matches: list[dict[str, Any]]) -> str:
    if verdict == _VERDICT_LIKELY_KNOWN and matches:
        top = matches[0]
        return (
            f"Closest known result '{_as_text(top.get('title')) or top.get('id')}' "
            f"scores {top_score:.2f} (>= {_KNOWN_THRESHOLD:.2f}); this result may "
            f"already exist — review before claiming novelty."
        )
    if verdict == _VERDICT_POSSIBLE_OVERLAP and matches:
        top = matches[0]
        return (
            f"Partial overlap with '{_as_text(top.get('title')) or top.get('id')}' "
            f"(score {top_score:.2f}); likely related work worth citing, but not a "
            f"clear duplicate."
        )
    if not matches:
        return "No corpus records to compare against; novelty unverified."
    return (
        f"Top match scores only {top_score:.2f} (< {_OVERLAP_THRESHOLD:.2f}); no "
        f"close prior work found in the provided corpus."
    )


def novelty_check(
    statement: str,
    *,
    corpus: Any,
    methods: Any = None,
    k: int = 5,
) -> dict[str, Any]:
    """Score ``statement`` (+ ``methods``) against ``corpus`` for prior work.

    Returns the top-``k`` closest known results with bounded ``[0, 1]``
    similarity scores, a ``novelty`` verdict thresholded on the best score, and a
    short human-readable ``reason``. Deterministic: ties break by record id then
    corpus position. ``advisory`` is always ``True`` — a ``likely_novel`` verdict
    only means nothing close was found *in this corpus*, never a guarantee.
    """
    records = _load_corpus(corpus)
    q_tokens = _query_tokens(statement, methods)

    # Empty corpus or empty query -> nothing to compare; novel by default.
    if not records or not q_tokens:
        return {
            "op": "novelty",
            "ok": True,
            "novelty": _VERDICT_LIKELY_NOVEL,
            "top_score": 0.0,
            "count": 0,
            "matches": [],
            "reason": _reason(_VERDICT_LIKELY_NOVEL, 0.0, []),
            "advisory": True,
        }

    doc_tokens = [_record_tokens(r) for r in records]
    # idf spans the query too, so a term unique to the query still contributes.
    idf = _build_idf(doc_tokens + [q_tokens])
    q_vec = _tfidf_vec(q_tokens, idf)

    scored: list[tuple[float, str, int, dict[str, Any]]] = []
    for i, (rec, toks) in enumerate(zip(records, doc_tokens)):
        sim = _cosine(q_vec, _tfidf_vec(toks, idf))
        rec_id = _as_text(rec.get("id")) or str(i)
        scored.append((sim, rec_id, i, rec))

    # Deterministic: descending score, then stable by id, then corpus order.
    scored.sort(key=lambda t: (-t[0], t[1], t[2]))

    kk = max(0, int(k))
    matches: list[dict[str, Any]] = []
    for sim, rec_id, _i, rec in scored[:kk]:
        if sim <= 0.0:
            continue
        matches.append(
            {
                "id": rec_id,
                "title": _as_text(rec.get("title")),
                "ref": _as_text(rec.get("ref")),
                "score": round(float(sim), 6),
            }
        )

    top_score = round(float(scored[0][0]), 6) if scored else 0.0
    verdict = _verdict(top_score)
    return {
        "op": "novelty",
        "ok": True,
        "novelty": verdict,
        "top_score": top_score,
        "count": len(matches),
        "matches": matches,
        "reason": _reason(verdict, top_score, matches),
        "advisory": True,
    }


# --------------------------------------------------------------------------- #
# Entry point.
# --------------------------------------------------------------------------- #
def run(request: dict[str, Any]) -> dict[str, Any]:
    """Answer one ``novelty`` request.

    Request: ``{"op": "novelty", "statement", "corpus", "methods"?, "k"?}`` where
    ``corpus`` is a list of ``{id,title,statement,ref}`` records or a path to a
    JSON file of the same. Any other ``op`` is rejected.
    """
    if not isinstance(request, dict):
        return {"op": "novelty", "ok": False, "stderr": "request must be an object"}
    op = request.get("op", "novelty")
    if op != "novelty":
        return {"op": op, "ok": False, "stderr": f"unknown op: {op}"}
    try:
        k = int(request.get("k", 5))
    except (TypeError, ValueError):
        k = 5
    return novelty_check(
        request.get("statement", ""),
        corpus=request.get("corpus", []),
        methods=request.get("methods"),
        k=k,
    )


def main() -> None:
    if len(sys.argv) >= 2 and os.path.exists(sys.argv[1]):
        with open(sys.argv[1], encoding="utf-8") as fh:
            req = json.load(fh)
    else:
        req = json.load(sys.stdin)
    result = run(req)
    print(json.dumps(result))
    raise SystemExit(0 if result.get("ok") else 1)


if __name__ == "__main__":
    main()
