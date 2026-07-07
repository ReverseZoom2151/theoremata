"""Tests for the recall->rerank cascade (offline, mock reranker).

Covers stage ordering, accessibility masking inside the cascade, the mock LM
rerank stage lifting the relevant premise, graceful degradation, and the JSON
contract.
"""
from theoremata_tools import cascade as C
from theoremata_tools import reranker as RR


MOCK_ENV = {"THEOREMATA_MODEL_MOCK": "1"}

DECLS = [
    {"name": "Nat.add_zero", "module": "Init.Data.Nat.Basic", "kind": "theorem"},
    {"name": "Nat.add_comm", "module": "Init.Data.Nat.Basic", "kind": "theorem"},
    {"name": "Nat.zero_add", "module": "Init.Data.Nat.Basic", "kind": "theorem"},
    {"name": "List.reverse_reverse", "module": "Init.Data.List.Basic", "kind": "theorem"},
]


def _names(resp):
    return [r["name"] for r in resp["results"]]


def test_cascade_contract_shape():
    resp = C.retrieve_cascade("Nat add zero", DECLS, env=MOCK_ENV, first_k=10, k=3)
    assert resp["ok"] is True
    assert resp["op"] == "cascade"
    assert resp["first_stage"] == "bm25"
    assert set(resp["stages"]) == {"corpus", "accessible", "recall", "returned"}
    assert resp["stages"]["corpus"] == len(DECLS)
    assert resp["count"] == len(resp["results"])


def test_cascade_reranks_relevant_first():
    # First stage (BM25) recalls several Nat lemmas; the mock reranker should
    # keep the goal-relevant `Nat.add_zero` at the top.
    resp = C.retrieve_cascade(
        "Nat.add_zero n + 0 = n", DECLS, env=MOCK_ENV, first_k=10, k=5
    )
    assert resp["model"] == "mock"
    assert resp["degraded"] is False
    assert _names(resp)[0] == "Nat.add_zero"
    # rerank annotations present on results
    assert "affirmative_prob" in resp["results"][0]
    assert resp["results"][0]["rank"] == 0


def test_cascade_masks_before_recall():
    # Only the Nat module is imported; the List lemma must never reach recall.
    dag = {"imports": {"MyFile": ["Init.Data.Nat.Basic"]}}
    resp = C.retrieve_cascade(
        "reverse",
        DECLS,
        imports=["MyFile"],
        dag=dag,
        env=MOCK_ENV,
    )
    assert resp["stages"]["accessible"] == 3      # 3 Nat lemmas
    assert "List.reverse_reverse" not in _names(resp)


def test_cascade_k_limits_output():
    resp = C.retrieve_cascade("Nat add", DECLS, env=MOCK_ENV, first_k=10, k=1)
    assert resp["count"] == 1


def test_cascade_degrades_without_model():
    # No mock env, and no scorer available -> reranker degrades to first-stage
    # order, but the cascade still returns a well-formed response.
    def broken(query, cand):
        raise RuntimeError("model down")

    # Unique query + fresh cache so no earlier mock score is reused.
    resp = C.retrieve_cascade(
        "Nat add zero degrade-probe", DECLS, scorer=broken, first_k=10, k=3,
        cache=RR.ScoreCache(),
    )
    assert resp["ok"] is True
    assert resp["degraded"] is True
    # first-stage (BM25) order is preserved; still a Nat lemma on top
    assert resp["results"][0]["name"].startswith("Nat.")


def test_cascade_hybrid_first_stage():
    resp = C.retrieve_cascade(
        "Nat.add_zero", DECLS, env=MOCK_ENV, first_stage="hybrid", first_k=10, k=3
    )
    assert resp["first_stage"] == "hybrid"
    assert _names(resp)[0] == "Nat.add_zero"


def test_run_entry_with_injected_decls():
    resp = C.run(query="Nat add zero", decls=DECLS, k=2, env=MOCK_ENV)
    assert resp["op"] == "cascade"
    assert resp["count"] == 2
