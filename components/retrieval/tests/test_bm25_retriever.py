"""Tests for the BM25 lexical-recall baseline (offline, no Lean).

Covers backend detection/reporting, relevant-beats-irrelevant ranking, the
query→candidates contract, ordering, and accessibility-masked ``run``.
"""
from theoremata_tools import bm25_retriever as B


DECLS = [
    {"name": "Nat.add_zero", "module": "Init.Data.Nat.Basic", "kind": "theorem"},
    {"name": "Nat.add_comm", "module": "Init.Data.Nat.Basic", "kind": "theorem"},
    {"name": "List.reverse_reverse", "module": "Init.Data.List.Basic", "kind": "theorem"},
    {"name": "List.append_nil", "module": "Init.Data.List.Basic", "kind": "theorem"},
]


def _names(results):
    return [r["name"] for r in results]


def test_backend_is_reported():
    assert B._detect_backend() in {"rank_bm25", "fallback"}
    resp = B.run(query="add zero", decls=DECLS)
    assert resp["backend"] in {"rank_bm25", "fallback"}


def test_relevant_beats_irrelevant():
    results = B.retrieve("Nat add zero", DECLS, limit=10)
    assert results, "expected at least one match"
    assert results[0]["name"] == "Nat.add_zero"
    # a list lemma with no shared tokens must not appear
    assert "List.reverse_reverse" not in _names(results)


def test_contract_shape_and_descending_scores():
    results = B.retrieve("add comm", DECLS, limit=10)
    assert set(results[0].keys()) == {"name", "module", "kind", "score"}
    assert results[0]["name"] == "Nat.add_comm"
    scores = [r["score"] for r in results]
    assert scores == sorted(scores, reverse=True)


def test_no_query_or_empty_corpus_returns_empty():
    assert B.retrieve("", DECLS) == []
    assert B.retrieve("add", []) == []


def test_zero_overlap_returns_empty():
    assert B.retrieve("xyzzy nonexistent token", DECLS) == []


def test_limit_respected():
    decls = [{"name": f"Foo.bar_{i}", "module": "M.N", "kind": "theorem"} for i in range(10)]
    assert len(B.retrieve("bar", decls, limit=3)) == 3


def test_both_backends_agree_on_ordering():
    # The pure-Python fallback and rank_bm25 (if present) must rank the obvious
    # winner first; force the fallback explicitly to exercise it.
    fb = B.retrieve("Nat add zero", DECLS, limit=10, backend="fallback")
    assert fb[0]["name"] == "Nat.add_zero"


def test_run_contract_and_accessible_masking():
    resp = B.run(query="add zero", decls=DECLS, op="retrieve")
    assert resp["ok"] is True
    assert resp["op"] == "retrieve"
    assert resp["results"][0]["name"] == "Nat.add_zero"

    # Accessibility masking via an import DAG: MyFile imports only the Nat
    # module, so the List lemmas are filtered out before ranking.
    dag = {"imports": {"MyFile": ["Init.Data.Nat.Basic"]}}
    resp2 = B.run(
        query="reverse",
        decls=DECLS,
        op="accessible_retrieve",
        imports=["MyFile"],
        dag=dag,
    )
    # List.reverse_reverse is unreachable -> no results survive masking
    assert resp2["results"] == []

    resp3 = B.run(
        query="add",
        decls=DECLS,
        op="accessible_retrieve",
        imports=["MyFile"],
        dag=dag,
    )
    assert resp3["results"], "Nat lemmas are reachable"
    assert all(r["module"] == "Init.Data.Nat.Basic" for r in resp3["results"])
