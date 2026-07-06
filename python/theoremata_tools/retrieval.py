"""In-the-loop Mathlib retrieval (plan §8).

Combines the three offline retrieval layers into a single, cached, ranked
lookup for a proof obligation:

* Layer A — import DAG (``mathlib_index``): structural substrate (not needed
  here directly, but the same source tree feeds Layers B/C).
* Layer B — declaration dump (``decl_index.dump``): the authoritative list of
  ``{name, kind, module, is_axiom}`` records.
* Layer C — head-symbol buckets (``head_index.build_head_index``): groups
  declarations by the head symbol of their conclusion.

``retrieve`` ranks the declarations for a natural-language / lemma-name query
with a hybrid lexical score: a BM25-style term-overlap score over the tokenised
declaration name (+ module), an exact-identifier bonus so precise lookups such
as ``Nat.succ_le_succ`` win decisively, and a head-bucket bonus when the query's
apparent head symbol matches the declaration's conclusion head.

Standard-library only — no embedding models, no third-party deps. The expensive
``lean``/``lake`` dumps (~40s) are cached to ``<root>/.theoremata/cache`` keyed
on the toolchain + imports so they are not repeated.
"""
from __future__ import annotations

import hashlib
import json
import math
import os
import re
import sys
from pathlib import Path
from typing import Any

from . import decl_index
from . import head_index

# --------------------------------------------------------------------------
# Tokenisation
# --------------------------------------------------------------------------

# Split identifiers on `.`, `_`, whitespace and digit runs, then split each
# fragment on camelCase boundaries. E.g. `Nat.succ_le_succ` ->
# ["nat", "succ", "le", "succ"], `List.getLast?` -> ["list", "get", "last"].
_SEP_RE = re.compile(r"[.\s_?!¬∀∃→↔∧∨=≤<≥>≠∈∉∣≡,:;()\[\]{}⟨⟩·|/\\+\-*^~]+")
_CAMEL_RE = re.compile(r"[A-Z]?[a-z]+|[A-Z]+(?![a-z])|[0-9]+")


def tokenize(text: str) -> list[str]:
    """Lowercased lexical tokens from an identifier or query string.

    Splits on `.`/`_`/whitespace/punctuation, then on camelCase boundaries, so
    both ``Nat.succ_le_succ`` and ``succLeSucc`` yield comparable token streams.
    """
    if not text:
        return []
    out: list[str] = []
    for frag in _SEP_RE.split(text):
        if not frag:
            continue
        for piece in _CAMEL_RE.findall(frag):
            piece = piece.lower()
            if piece:
                out.append(piece)
    return out


def _normalize_ident(text: str) -> str:
    """Collapse an identifier/query to its ordered token signature for exact
    comparison (so ``Nat.succ_le_succ`` == a query of the same, modulo case)."""
    return " ".join(tokenize(text))


def _last_segment(name: str) -> str:
    return name.rsplit(".", 1)[-1] if name else name


# --------------------------------------------------------------------------
# Scoring
# --------------------------------------------------------------------------

# Field weights: a term matched in the declaration name counts far more than one
# matched only in its module path.
_NAME_WEIGHT = 3
_MODULE_WEIGHT = 1

# BM25 parameters (textbook defaults).
_K1 = 1.5
_B = 0.75

# Additive bonuses layered on top of the BM25 lexical score.
_EXACT_FULL_BONUS = 25.0      # query tokens == full decl name tokens
_EXACT_LAST_BONUS = 12.0      # query tokens == last segment of decl name
_EXACT_TOKEN_BONUS = 2.0      # per query token present verbatim as a name token
_HEAD_BONUS = 6.0             # query head symbol == decl conclusion head


def _build_corpus(decls: list[dict[str, Any]]) -> dict[str, Any]:
    """Precompute per-decl weighted term frequencies + corpus IDF for BM25."""
    doc_tfs: list[dict[str, int]] = []
    doc_lens: list[int] = []
    df: dict[str, int] = {}
    for d in decls:
        name = d.get("name") or ""
        module = d.get("module") or ""
        tf: dict[str, int] = {}
        for tok in tokenize(name):
            tf[tok] = tf.get(tok, 0) + _NAME_WEIGHT
        for tok in tokenize(module):
            tf[tok] = tf.get(tok, 0) + _MODULE_WEIGHT
        doc_tfs.append(tf)
        doc_lens.append(sum(tf.values()))
        for tok in tf:
            df[tok] = df.get(tok, 0) + 1
    n = len(decls)
    avgdl = (sum(doc_lens) / n) if n else 0.0
    idf: dict[str, float] = {}
    for tok, freq in df.items():
        # BM25 idf with +1 floor so a term present in every doc still scores > 0.
        idf[tok] = math.log(1.0 + (n - freq + 0.5) / (freq + 0.5))
    return {"doc_tfs": doc_tfs, "doc_lens": doc_lens, "idf": idf, "avgdl": avgdl}


def _bm25(q_terms: list[str], tf: dict[str, int], doc_len: int, corpus: dict[str, Any]) -> float:
    avgdl = corpus["avgdl"] or 1.0
    idf = corpus["idf"]
    score = 0.0
    for term in q_terms:
        f = tf.get(term)
        if not f:
            continue
        denom = f + _K1 * (1.0 - _B + _B * doc_len / avgdl)
        score += idf.get(term, 0.0) * (f * (_K1 + 1.0)) / denom
    return score


def _head_reverse(head_index_obj: dict[str, Any] | None) -> dict[str, str]:
    rev: dict[str, str] = {}
    if not head_index_obj:
        return rev
    for head, names in (head_index_obj.get("heads") or {}).items():
        for nm in names:
            rev[nm] = head
    return rev


# Head symbols that are too generic to carry retrieval signal on their own.
_TRIVIAL_HEADS = {"", "Prop", "Type", "Sort"}


def _query_head(query: str, head_index_obj: dict[str, Any] | None) -> str:
    """Best-effort head symbol of the query, so a query shaped like a goal
    (``a ≤ b``) or naming a head bucket can prefer matching-head decls."""
    if not head_index_obj:
        return ""
    heads = head_index_obj.get("heads") or {}
    # If the query literally names an existing bucket, use it verbatim.
    q = query.strip()
    if q in heads:
        return q
    guessed = head_index.head_symbol(query)
    if guessed in _TRIVIAL_HEADS:
        return ""
    return guessed


def retrieve(
    query: str,
    decls: list[dict[str, Any]],
    head_index: dict[str, Any] | None = None,
    limit: int = 20,
) -> list[dict[str, Any]]:
    """Rank ``decls`` for ``query`` and return the top ``limit`` records.

    Returns ``[{name, module, kind, score}, ...]`` sorted by descending score.
    Only declarations with a positive score are returned, so a small precise set
    beats a large marginal one.
    """
    q_terms = tokenize(query)
    if not decls or not q_terms:
        return []
    q_norm = _normalize_ident(query)
    q_term_set = set(q_terms)

    corpus = _build_corpus(decls)
    head_rev = _head_reverse(head_index)
    q_head = _query_head(query, head_index)

    scored: list[tuple[float, int, dict[str, Any]]] = []
    for i, d in enumerate(decls):
        name = d.get("name") or ""
        if not name:
            continue
        tf = corpus["doc_tfs"][i]
        base = _bm25(q_terms, tf, corpus["doc_lens"][i], corpus)
        score = base

        # Exact-identifier precision bonuses.
        name_norm = _normalize_ident(name)
        if name_norm and name_norm == q_norm:
            score += _EXACT_FULL_BONUS
        elif _normalize_ident(_last_segment(name)) == q_norm and q_norm:
            score += _EXACT_LAST_BONUS
        name_tokens = set(tokenize(name))
        exact_hits = len(q_term_set & name_tokens)
        score += _EXACT_TOKEN_BONUS * exact_hits

        # Head-symbol bucket bonus.
        if q_head and head_rev.get(name) == q_head:
            score += _HEAD_BONUS

        if score > 0.0:
            scored.append((score, i, d))

    scored.sort(key=lambda t: (-t[0], t[1]))
    results: list[dict[str, Any]] = []
    for score, _i, d in scored[: max(0, limit)]:
        results.append(
            {
                "name": d.get("name"),
                "module": d.get("module"),
                "kind": d.get("kind"),
                "score": round(float(score), 6),
            }
        )
    return results


# --------------------------------------------------------------------------
# Index cache
# --------------------------------------------------------------------------


def _toolchain_fingerprint(root: str | None) -> str:
    """Hash of the pinned toolchain + resolved manifest, so a cache entry is
    invalidated when the Lean version or Mathlib revision changes."""
    parts: list[str] = []
    if root:
        for fname in ("lean-toolchain", "lake-manifest.json"):
            p = Path(root) / fname
            try:
                parts.append(p.read_text(encoding="utf-8", errors="replace"))
            except OSError:
                parts.append("")
    return hashlib.sha256("".join(parts).encode("utf-8")).hexdigest()


def cache_key(root: str | None, imports: list[str] | None) -> str:
    """Stable cache key over ``(root, sorted imports, toolchain fingerprint)``."""
    imports_tuple = tuple(sorted(imports or ["Init"]))
    root_abs = os.path.abspath(root) if root else ""
    payload = json.dumps(
        {
            "root": root_abs,
            "imports": imports_tuple,
            "toolchain": _toolchain_fingerprint(root),
        },
        sort_keys=True,
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()[:20]


def cache_dir(root: str | None) -> Path:
    """``<root>/.theoremata/cache`` (or CWD-based when no root is given)."""
    base = Path(root) if root else Path.cwd()
    return base / ".theoremata" / "cache"


def _cache_path(root: str | None, imports: list[str] | None) -> Path:
    return cache_dir(root) / f"decl_head_{cache_key(root, imports)}.json"


def build_or_load(
    root: str | None,
    imports: list[str] | None,
    *,
    rebuild: bool = False,
    lean_bin: str | None = None,
    timeout: float = 300.0,
) -> dict[str, Any]:
    """Return ``{decls, head_index, ...}`` for ``(root, imports)``, cached.

    On a cache hit the ~40s Lean dump is skipped entirely; on a miss the decl
    dump (Layer B) and head-symbol index (Layer C) are built and persisted under
    ``<root>/.theoremata/cache``. Falls back to rebuilding (and returns the
    partial result without caching) when the toolchain is unavailable.
    """
    path = _cache_path(root, imports)
    if not rebuild:
        try:
            with open(path, encoding="utf-8") as fh:
                cached = json.load(fh)
            cached["cached"] = True
            return cached
        except (OSError, json.JSONDecodeError):
            pass

    # Layer B: authoritative declaration records (name/kind/module/is_axiom).
    decl_result = decl_index.dump(root, imports, lean_bin=lean_bin, timeout=timeout)
    decls = decl_result.get("decls", [])

    # Layer C: head-symbol buckets. build_head_index needs pretty-printed types,
    # which the decl dump omits, so obtain them via head_index's type dump.
    head_obj: dict[str, Any] = {"heads": {}, "conclusions": {}, "count": 0}
    types_result = head_index.dump_types(root, imports, lean_bin=lean_bin, timeout=timeout)
    if types_result.get("ok"):
        head_obj = head_index.build_head_index(types_result.get("decls", []))

    result: dict[str, Any] = {
        "ok": bool(decl_result.get("ok")),
        "root": os.path.abspath(root) if root else None,
        "imports": list(imports or ["Init"]),
        "key": cache_key(root, imports),
        "count": len(decls),
        "decls": decls,
        "head_index": head_obj,
        "cached": False,
        "stderr": decl_result.get("stderr", ""),
    }

    if result["ok"] and decls:
        try:
            path.parent.mkdir(parents=True, exist_ok=True)
            with open(path, "w", encoding="utf-8") as fh:
                json.dump(result, fh)
        except OSError:
            pass
    return result


# --------------------------------------------------------------------------
# Entry points
# --------------------------------------------------------------------------


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
) -> dict[str, Any]:
    """Build/load the cached index, then answer one retrieval request.

    ``op="retrieve"`` ranks declarations for ``query``; ``op="warm"`` just
    prebuilds (and caches) the index.
    """
    index = build_or_load(
        root, imports, rebuild=rebuild, lean_bin=lean_bin, timeout=timeout
    )

    if op == "warm":
        return {
            "ok": index["ok"],
            "op": "warm",
            "root": index["root"],
            "imports": index["imports"],
            "key": index["key"],
            "count": index["count"],
            "cached": index["cached"],
            "heads": len((index.get("head_index") or {}).get("heads", {})),
            "stderr": index.get("stderr", ""),
        }

    if op != "retrieve":
        return {"ok": False, "stderr": f"unknown op: {op}"}

    if not index["ok"] and not index["decls"]:
        return {
            "ok": False,
            "op": "retrieve",
            "query": query,
            "count": 0,
            "results": [],
            "stderr": index.get("stderr", "index unavailable"),
        }

    results = retrieve(
        query, index["decls"], head_index=index.get("head_index"), limit=limit
    )
    return {
        "ok": True,
        "op": "retrieve",
        "query": query,
        "root": index["root"],
        "imports": index["imports"],
        "cached": index["cached"],
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
    )
    print(json.dumps(result))
    raise SystemExit(0 if result.get("ok") else 1)


if __name__ == "__main__":
    main()
