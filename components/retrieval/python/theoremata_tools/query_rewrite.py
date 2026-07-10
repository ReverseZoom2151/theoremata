"""Query rewriting / expansion for premise retrieval (RAG multi-query pass).

The RAG cascade (:mod:`theoremata_tools.cascade`) retrieves against the *raw*
goal text; on retry :mod:`theoremata_tools.error_keyed_retrieval` re-keys on the
prover error. Neither reformulates the query *semantically*: a goal phrased with
``iff`` never matches a premise indexed under ``if and only if``, and a
conjunctive goal is retrieved as one blurry query rather than its parts. This
module closes that gap (mining ``docs/agentic-patterns-mining/A4``): it rewrites
one query into a small, deterministic *multi-query* set — notation/synonym
variants, Lean/Mathlib naming variants, and sub-queries split off a conjunctive
goal — optionally augmented with a HyDE-style hypothetical document.

Three pieces:

* :func:`expand_query` — pure, deterministic rule-based expansions of a math
  query (no model). An optional injected ``llm_rewrite`` seam appends extra
  paraphrases (mocked in tests); absent, expansion stays fully rule-based.
* :func:`rewrite_for_retrieval` — the original query + its expansions (+ an
  optional injected HyDE hypothetical-document), de-duplicated, ready to feed a
  retriever as a multi-query. This is what prepends to the cascade.
* :func:`run` — the ``query_rewrite`` worker op. It builds the multi-query and,
  through an **injected** ``retrieve`` callable (default wraps the cascade), runs
  each sub-query and merges the hits (first-seen de-dup), so the cascade sees the
  union of the reformulations rather than the raw goal alone. Tests pass mocks,
  so no Lean/Mathlib index is needed to exercise the dispatch.

Determinism / security
----------------------
The query is untrusted text: it is only ever tokenised, substituted against a
fixed synonym table, and re-emitted — never evaluated. All rule-based expansion
and merging is deterministic (first-seen order, stable de-duplication). Standard
library only; the ``llm_rewrite`` / ``hyde`` / ``retrieve`` seams are the sole
(injected, defaulted) dependencies.
"""
from __future__ import annotations

import json
import os
import re
import sys
from typing import Any, Callable

# Records returned by a retriever: list of dicts (e.g. {name, module, score}).
Retriever = Callable[[str], list]
# A rewrite seam: query -> extra query string(s) (paraphrase / hypothetical doc).
Rewriter = Callable[[str], Any]

# --------------------------------------------------------------------------- #
# Synonym / notation table.
# --------------------------------------------------------------------------- #
# Each group is a set of interchangeable surface forms for the *same* math
# concept. When any form appears in a query, every *other* form of the group
# spawns a variant query with that occurrence substituted. Bidirectional by
# construction. Multi-word forms are matched before single-word ones so
# "if and only if" is consumed whole rather than as three words.
_SYNONYM_GROUPS: list[tuple[str, ...]] = [
    ("iff", "if and only if"),
    ("nat", "natural number"),
    ("int", "integer"),
    ("rat", "rational number"),
    ("real", "real number"),
    ("wlog", "without loss of generality"),
    ("comm", "commutative"),
    ("assoc", "associative"),
    ("distrib", "distributive"),
    ("inj", "injective"),
    ("surj", "surjective"),
    ("bij", "bijective"),
    ("add", "addition", "sum"),
    ("mul", "multiplication", "product"),
    ("sub", "subtraction", "difference"),
    ("div", "division", "quotient"),
    ("pos", "positive"),
    ("neg", "negative"),
    ("le", "less than or equal", "at most"),
    ("ge", "greater than or equal", "at least"),
    ("lt", "less than", "strictly less"),
    ("gt", "greater than", "strictly greater"),
    ("mem", "element of", "member of"),
    ("card", "cardinality", "number of elements"),
    ("continuous", "cts"),
]

# Unicode / ASCII math notation <-> spelled-out name. Kept apart from the word
# groups because these match as literal substrings, not word-boundaried tokens.
_NOTATION_GROUPS: list[tuple[str, ...]] = [
    ("↔", "iff"),
    ("→", "implies"),
    ("∀", "forall"),
    ("∃", "exists"),
    ("∧", "and"),
    ("∨", "or"),
    ("¬", "not"),
    ("∈", "in"),
    ("≤", "<="),
    ("≥", ">="),
    ("≠", "!="),
    ("∅", "empty set"),
    ("ℕ", "natural number"),
    ("ℤ", "integer"),
    ("ℚ", "rational number"),
    ("ℝ", "real number"),
]

# Connectives on which a conjunctive goal is split into independently-retrievable
# sub-queries. Longest / most specific first so " and " is not clipped mid-word.
_CONJUNCTION_RE = re.compile(r"\s+and\s+|\s*∧\s*|\s*;\s*|\s*,\s*(?=\S)")

# Collapse runs of whitespace for stable comparison / emission.
_WS_RE = re.compile(r"\s+")


def _as_text(value: Any) -> str:
    """Coerce an untrusted query field to a plain string (never evaluate it)."""
    if value is None:
        return ""
    if isinstance(value, str):
        return value
    if isinstance(value, (list, tuple)):
        return " ".join(_as_text(v) for v in value)
    return str(value)


def _norm(text: str) -> str:
    """Collapse whitespace + trim (surface form preserved apart from spacing)."""
    return _WS_RE.sub(" ", text).strip()


def _dedup(items: list[str]) -> list[str]:
    """First-seen, case/space-insensitive de-duplication preserving surface form.

    Empty / whitespace-only strings are dropped. The comparison key is the
    lowercased whitespace-collapsed string, so ``"Nat  Add"`` and ``"nat add"``
    collapse to one entry (the first spelling wins).
    """
    out: list[str] = []
    seen: set[str] = set()
    for it in items:
        s = _norm(it)
        if not s:
            continue
        key = s.lower()
        if key in seen:
            continue
        seen.add(key)
        out.append(s)
    return out


def _word_variants(query: str) -> list[str]:
    """Word-boundaried synonym substitutions (one variant per replaced form)."""
    variants: list[str] = []
    lowered = query.lower()
    for group in _SYNONYM_GROUPS:
        # Match longer surface forms first so multi-word phrases win.
        for form in sorted(group, key=len, reverse=True):
            pat = re.compile(r"\b" + re.escape(form) + r"\b", re.IGNORECASE)
            if not pat.search(lowered):
                continue
            for other in group:
                if other == form:
                    continue
                variants.append(pat.sub(other, query))
            # Only the first form present in this group drives the substitution,
            # so each group contributes a bounded, deterministic set.
            break
    return variants


def _notation_variants(query: str) -> list[str]:
    """Literal-substring notation <-> spelled-out substitutions."""
    variants: list[str] = []
    for group in _NOTATION_GROUPS:
        for form in sorted(group, key=len, reverse=True):
            if form not in query:
                continue
            for other in group:
                if other == form:
                    continue
                variants.append(query.replace(form, other))
            break
    return variants


def _subqueries(query: str) -> list[str]:
    """Split a conjunctive goal into sub-queries (only when it truly splits)."""
    parts = [_norm(p) for p in _CONJUNCTION_RE.split(query)]
    parts = [p for p in parts if p]
    # A single fragment means there was nothing conjunctive to decompose.
    return parts if len(parts) > 1 else []


def expand_query(query: str, *, llm_rewrite: Rewriter | None = None) -> list[str]:
    """Deterministic rule-based expansions of a math retrieval query.

    Produces, in a fixed order (each de-duplicated, first-appearance preserved):

    1. **Notation variants** — spelled-out <-> symbolic (``↔`` <-> ``iff``,
       ``ℕ`` <-> ``natural number``, ``≤`` <-> ``<=``).
    2. **Synonym / naming variants** — Lean/Mathlib shorthand <-> prose
       (``iff`` <-> ``if and only if``, ``nat`` <-> ``natural number``,
       ``comm`` <-> ``commutative``, ``add`` <-> ``addition``/``sum``).
    3. **Sub-queries** — a conjunctive goal (``... and ...`` / ``∧`` / ``,`` /
       ``;``) decomposed into its independently-retrievable conjuncts.

    The returned list holds the *expansions* only (not the original query);
    :func:`rewrite_for_retrieval` prepends the original. Pure and model-free.

    ``llm_rewrite`` is an optional injected seam: when given, it is called with
    the query and its output (a string or list of strings — e.g. LM paraphrases)
    is appended after the rule-based variants. Absent, expansion is fully
    deterministic and rule-based. The result is always de-duplicated.
    """
    q = _norm(_as_text(query))
    if not q:
        return []

    expansions: list[str] = []
    expansions.extend(_notation_variants(q))
    expansions.extend(_word_variants(q))
    expansions.extend(_subqueries(q))

    if llm_rewrite is not None:
        extra = llm_rewrite(q)
        if isinstance(extra, str):
            expansions.append(extra)
        elif isinstance(extra, (list, tuple)):
            expansions.extend(_as_text(e) for e in extra)

    # Drop any variant that is just the original (substitution matched nothing
    # meaningful) and de-duplicate.
    q_key = q.lower()
    return [e for e in _dedup(expansions) if e.lower() != q_key]


def rewrite_for_retrieval(
    query: str,
    *,
    expansions: bool = True,
    hyde: Rewriter | None = None,
    llm_rewrite: Rewriter | None = None,
) -> list[str]:
    """Build the multi-query fed to the retriever: original + expansions + HyDE.

    Order is stable and meaningful: the **original** query first (highest-signal,
    what the corpus was most likely written against), then the rule-based
    :func:`expand_query` variants, then — if ``hyde`` is supplied — the
    HyDE-style hypothetical document(s) it generates.

    Parameters
    ----------
    expansions : bool
        Include the rule-based synonym/notation/sub-query expansions. When
        ``False`` only the original (+ optional HyDE) is returned.
    hyde : callable, optional
        Injected HyDE seam ``hyde(query) -> str | list[str]``: a *hypothetical
        document* (an imagined premise statement / answer) whose text is
        retrieved against directly. Skipped entirely when ``None`` — no model is
        invoked. Tests pass a mock.
    llm_rewrite : callable, optional
        Forwarded to :func:`expand_query` for LM paraphrase expansion.

    Returns a de-duplicated ``list[str]``; deterministic given deterministic
    seams.
    """
    q = _norm(_as_text(query))
    multi: list[str] = []
    if q:
        multi.append(q)
    if expansions:
        multi.extend(expand_query(q, llm_rewrite=llm_rewrite))
    if hyde is not None and q:
        doc = hyde(q)
        if isinstance(doc, str):
            multi.append(doc)
        elif isinstance(doc, (list, tuple)):
            multi.extend(_as_text(d) for d in doc)
    return _dedup(multi)


# --------------------------------------------------------------------------- #
# Default retriever (wraps the cascade).
# --------------------------------------------------------------------------- #
def _default_retriever(request: dict[str, Any]) -> Retriever:
    """Build a ``retrieve(query) -> list`` over the cascade, threading index
    params. Imported lazily so this module carries no hard dependency on a built
    Lean index when a mock is used.
    """
    from . import cascade  # local import: only needed for the live path

    root = request.get("root")
    imports = request.get("imports")
    passthrough = {
        k: request[k]
        for k in (
            "first_stage",
            "first_k",
            "k",
            "theorem_module",
            "theorem_line",
            "file_module",
            "mask",
            "samples",
            "rebuild",
            "lean_bin",
            "timeout",
            "decls",
            "dag",
        )
        if k in request
    }

    def _retrieve(query: str) -> list:
        res = cascade.run(root=root, imports=imports, query=query, **passthrough)
        return res.get("results", []) if res.get("ok") else []

    return _retrieve


def _result_key(record: Any, fallback: str) -> str:
    """A stable de-dup key for a retrieved record (its name, else a fallback)."""
    if isinstance(record, dict):
        for field in ("name", "id", "ref"):
            v = record.get(field)
            if v:
                return str(v)
    return fallback


# --------------------------------------------------------------------------- #
# Worker entry point.
# --------------------------------------------------------------------------- #
def run(
    request: dict[str, Any],
    retrieve: Retriever | None = None,
    *,
    hyde: Rewriter | None = None,
    llm_rewrite: Rewriter | None = None,
) -> dict[str, Any]:
    """Answer one ``query_rewrite`` request.

    Request::

        {"op": "query_rewrite", "query": "<goal text>",
         "expansions"?: bool, "retrieve"?: bool, ...retrieval params}

    The query is rewritten into a multi-query via :func:`rewrite_for_retrieval`.
    When ``retrieve`` is falsy (the default) the op is a pure rewrite pass: it
    returns the ``queries`` for a caller to feed the cascade itself. When
    retrieval is requested (``request["retrieve"] is True`` or a ``retrieve``
    callable is injected), each sub-query is run through ``retrieve`` (default
    wraps :func:`theoremata_tools.cascade.run`) and the per-query hits are merged
    into one de-duplicated ``results`` list (each tagged with the ``query`` that
    found it), with the full breakdown under ``per_query`` — this is the
    multi-query-retrieval-then-merge that prepends to the cascade. Deterministic
    given deterministic seams.
    """
    if not isinstance(request, dict):
        return {"op": "query_rewrite", "ok": False, "stderr": "request must be an object"}
    op = request.get("op", "query_rewrite")
    if op != "query_rewrite":
        return {"op": op, "ok": False, "stderr": f"unknown op: {op}"}

    query = _norm(_as_text(request.get("query", "")))
    do_expansions = bool(request.get("expansions", True))
    queries = rewrite_for_retrieval(
        query, expansions=do_expansions, hyde=hyde, llm_rewrite=llm_rewrite
    )

    base = {
        "op": "query_rewrite",
        "ok": True,
        "query": query,
        "queries": queries,
        "expansions": [q for q in queries if q.lower() != query.lower()],
    }

    # Pure rewrite pass unless retrieval was explicitly asked for.
    want_retrieval = retrieve is not None or bool(request.get("retrieve", False))
    if not want_retrieval:
        base["count"] = 0
        base["results"] = []
        return base

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

    base["count"] = len(merged)
    base["results"] = merged
    base["per_query"] = per_query
    return base


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
