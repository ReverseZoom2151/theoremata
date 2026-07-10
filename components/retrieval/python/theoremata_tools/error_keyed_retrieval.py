"""Error-identifier-keyed retrieval (Seed-Prover / Delta-Prover architecture).

Seed-Prover keys its lemma retrieval on *what the proof attempt actually needs*:
when the formal prover (Lean) rejects an attempt, the error names the missing
pieces — an ``unknown identifier``, an ``unknown constant``, a named lemma left
in an ``unsolved goals`` dump, or a term in a ``type mismatch``. Rather than
retrieve against the original goal text (which the prover already had), we mine
the error for those unresolved names and retrieve against *them*, so the next
attempt is handed exactly the premises the last one lacked.

Two pieces:

* :func:`error_keyed_query` — a pure, deterministic extractor turning a raw Lean
  error string into an ordered, de-duplicated list of retrieval queries.
* :func:`run` — the ``error_keyed_retrieval`` worker op. It extracts the keyed
  queries and runs each through an **injected** ``retrieve`` callable
  (``retrieve(query) -> list[record]``), merging the per-query hits. The default
  callable wraps the existing :mod:`theoremata_tools.retrieval` worker; tests
  pass a mock, so no Lean/Mathlib index is needed to exercise the dispatch.

Security / determinism
----------------------
The Lean error is **untrusted data**: it is only ever scanned by fixed regexes
and re-emitted as query strings, never evaluated. Extraction and merging are
fully deterministic (first-seen order, stable de-duplication). Standard-library
only; the retriever is the sole (injected, defaulted) dependency.
"""
from __future__ import annotations

import json
import os
import re
import sys
from typing import Any, Callable

# Records returned by a retriever: list of dicts (e.g. {name, module, kind, score}).
Retriever = Callable[[str], list]

# --------------------------------------------------------------------------- #
# Extraction patterns.
# --------------------------------------------------------------------------- #

# Explicitly named unknowns, e.g.  unknown identifier 'foo' / unknown constant
# 'Nat.foo' / unknown namespace 'Bar' / unknown tactic 'grind'. Lean quotes the
# name with straight quotes, double quotes, or backticks — accept all three.
_QUOTED_UNKNOWN_RE = re.compile(
    r"unknown\s+(?:identifier|constant|namespace|tactic|declaration)\s+"
    r"['\"`]([^'\"`]+)['\"`]",
    re.IGNORECASE,
)

# Qualified dotted names anywhere in the message: Nat.succ_le_succ, List.getLast?.
# Each dotted segment must start with a letter/underscore so projections like
# `x.1` are not captured. The ASCII apostrophe is NOT an identifier char here: it
# doubles as Lean's quote delimiter, so including it would overrun `'name'`.
_DOTTED_RE = re.compile(
    r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_?]*)+"
)

# Bare snake_case identifiers with at least one underscore — the shape of an
# un-namespaced lemma/def name (add_comm, succ_le_succ). Bound variables are
# almost always single letters, so this is a precise-enough lemma signal. A match
# immediately preceded by '.' is a dotted-name segment and is skipped (the dotted
# tier already captured the qualified name).
_SNAKE_RE = re.compile(r"[A-Za-z][A-Za-z0-9]*(?:_[A-Za-z0-9]+)+")


def _as_text(value: Any) -> str:
    """Coerce an untrusted error field to a plain string (never evaluate it)."""
    if value is None:
        return ""
    if isinstance(value, str):
        return value
    if isinstance(value, (list, tuple)):
        return "\n".join(_as_text(v) for v in value)
    return str(value)


def _add(acc: list[str], name: str) -> None:
    """Append ``name`` to ``acc`` if non-empty and not already present."""
    name = name.strip()
    if name and name not in acc:
        acc.append(name)


def error_keyed_query(lean_error: str) -> list[str]:
    """Extract retrieval queries from a Lean error message.

    Mines, in tiers (each de-duplicated, preserving first appearance):

    1. explicitly named unknowns (``unknown identifier/constant/namespace 'X'``);
    2. qualified dotted names anywhere (``Nat.succ_le_succ``) — e.g. the lemma
       named inside an ``unsolved goals`` dump or a ``type mismatch`` term;
    3. bare snake_case lemma-shaped identifiers (``add_comm``), skipping any that
       are just the tail segment of a dotted name already captured.

    Returns an ordered, de-duplicated ``list[str]`` — deterministic for a given
    input. Empty when nothing identifier-like is found.
    """
    text = _as_text(lean_error)
    if not text:
        return []

    queries: list[str] = []

    # Tier 1: explicitly quoted unknown names.
    for m in _QUOTED_UNKNOWN_RE.finditer(text):
        _add(queries, m.group(1))

    # Tier 2: qualified dotted names.
    for m in _DOTTED_RE.finditer(text):
        _add(queries, m.group(0))

    # Tier 3: bare snake_case lemma names (skip dotted-name tail segments).
    for m in _SNAKE_RE.finditer(text):
        start = m.start()
        if start > 0 and text[start - 1] == ".":
            continue
        _add(queries, m.group(0))

    return queries


# --------------------------------------------------------------------------- #
# Default retriever (wraps the existing retrieval worker).
# --------------------------------------------------------------------------- #
def _default_retriever(request: dict[str, Any]) -> Retriever:
    """Build a ``retrieve(query) -> list`` callable over the existing retrieval
    worker, threading through the request's index params. Imported lazily so this
    module carries no hard dependency on a built Lean index when a mock is used.
    """
    from . import retrieval  # local import: only needed for the live path

    root = request.get("root")
    imports = request.get("imports")
    limit = _int(request.get("limit"), 20)
    retrieve_op = request.get("retrieve_op", "retrieve")
    lean_bin = request.get("lean_bin")
    timeout = _float(request.get("timeout"), 300.0)
    theorem_module = request.get("theorem_module")
    theorem_line = request.get("theorem_line")

    def _retrieve(query: str) -> list:
        res = retrieval.run(
            root=root,
            imports=imports,
            query=query,
            limit=limit,
            op=retrieve_op,
            lean_bin=lean_bin,
            timeout=timeout,
            theorem_module=theorem_module,
            theorem_line=theorem_line,
        )
        return res.get("results", []) if res.get("ok") else []

    return _retrieve


def _int(value: Any, default: int) -> int:
    try:
        return int(value)
    except (TypeError, ValueError):
        return default


def _float(value: Any, default: float) -> float:
    try:
        return float(value)
    except (TypeError, ValueError):
        return default


def _result_key(record: Any, fallback: str) -> str:
    """A stable de-dup key for a retrieved record (its name, else its repr)."""
    if isinstance(record, dict):
        for field in ("name", "id", "ref"):
            v = record.get(field)
            if v:
                return str(v)
    return fallback


# --------------------------------------------------------------------------- #
# Worker entry point.
# --------------------------------------------------------------------------- #
def run(request: dict[str, Any], retrieve: Retriever | None = None) -> dict[str, Any]:
    """Answer one ``error_keyed_retrieval`` request.

    Request::

        {"op": "error_keyed_retrieval", "error": "<lean error>",
         "limit"?: int, "root"?, "imports"?, ...retrieval params}

    The Lean error is mined by :func:`error_keyed_query`; each keyed query is run
    through ``retrieve`` (injected; default wraps
    :func:`theoremata_tools.retrieval.run`). Per-query hits are merged into one
    de-duplicated ``results`` list (each tagged with the ``query`` that found it),
    with the full per-query breakdown under ``per_query``. Deterministic given a
    deterministic retriever.
    """
    if not isinstance(request, dict):
        return {
            "op": "error_keyed_retrieval",
            "ok": False,
            "stderr": "request must be an object",
        }
    op = request.get("op", "error_keyed_retrieval")
    if op != "error_keyed_retrieval":
        return {"op": op, "ok": False, "stderr": f"unknown op: {op}"}

    lean_error = request.get("error")
    if lean_error is None:
        lean_error = request.get("lean_error", "")
    queries = error_keyed_query(_as_text(lean_error))

    # No identifier-like content: nothing to retrieve against — return cleanly
    # without touching the (possibly unavailable) index.
    if not queries:
        return {
            "op": "error_keyed_retrieval",
            "ok": True,
            "queries": [],
            "count": 0,
            "results": [],
            "per_query": {},
        }

    retriever = retrieve if retrieve is not None else _default_retriever(request)

    per_query: dict[str, list] = {}
    merged: list[dict[str, Any]] = []
    seen: set[str] = set()
    for q in queries:
        hits = retriever(q) or []
        per_query[q] = hits
        for i, rec in enumerate(hits):
            key = _result_key(rec, f"{q}#{i}")
            if key in seen:
                continue
            seen.add(key)
            tagged = dict(rec) if isinstance(rec, dict) else {"value": rec}
            tagged.setdefault("query", q)
            merged.append(tagged)

    return {
        "op": "error_keyed_retrieval",
        "ok": True,
        "queries": queries,
        "count": len(merged),
        "results": merged,
        "per_query": per_query,
    }


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
