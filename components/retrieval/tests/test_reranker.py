"""Tests for the LM-as-scorer retrieval reranker (Tier 3, item 15).

All tests run fully offline: the built-in MOCK scorer
(``THEOREMATA_MODEL_MOCK=1``) and injected scorers stand in for the model, so no
network / litellm is required.
"""
import math

import pytest

from theoremata_tools import reranker as RR


MOCK_ENV = {"THEOREMATA_MODEL_MOCK": "1"}


def _names(resp):
    return [r["name"] for r in resp["results"]]


# --- yes/no parsing ----------------------------------------------------------


@pytest.mark.parametrize(
    "text,expected",
    [
        (True, True),
        (False, False),
        ("yes", True),
        ("No", False),
        ("Yes, because it rewrites the goal.", True),
        ("no - unrelated lemma", False),
        ({"relevant": True}, True),
        ({"relevant": False}, False),
        ("maybe", None),
        ("", None),
        (None, None),
    ],
)
def test_parse_yes_no(text, expected):
    assert RR.parse_yes_no(text) is expected


# --- logprob -> probability --------------------------------------------------


def test_affirmative_probability_from_logprobs_prefers_yes():
    # P(yes) >> P(no) -> prob near 1.
    p = RR.affirmative_probability_from_logprobs({"Yes": math.log(0.9), "No": math.log(0.1)})
    assert p == pytest.approx(0.9, abs=1e-6)


def test_affirmative_probability_from_logprobs_none_when_absent():
    assert RR.affirmative_probability_from_logprobs({"foo": -1.0}) is None


# --- core reranking: relevant beats irrelevant (mock mode) -------------------


def _candidates():
    return [
        # deliberately placed FIRST so identity order would be "wrong"
        {"name": "List.reverse_reverse", "module": "Init.Data.List", "kind": "theorem", "score": 9.0},
        {"name": "Nat.add_zero", "module": "Init.Data.Nat", "kind": "theorem", "score": 1.0},
    ]


def test_relevant_ranked_above_irrelevant_mock_env():
    resp = RR.rerank("Nat add zero: n + 0 = n", _candidates(), env=MOCK_ENV)
    assert resp["ok"] is True
    assert resp["degraded"] is False
    assert resp["method"] == "generative_classifier"
    assert resp["model"] == "mock"
    # the clearly-relevant lemma must be lifted above the unrelated one
    assert _names(resp)[0] == "Nat.add_zero"
    top, bottom = resp["results"][0], resp["results"][1]
    assert top["affirmative_prob"] > bottom["affirmative_prob"]
    assert top["relevant"] is True
    assert top["rank"] == 0 and bottom["rank"] == 1


def test_relevant_ranked_above_irrelevant_injected_scorer():
    # An injected scorer decouples the test from the mock heuristic entirely.
    def scorer(query, cand):
        prob = 0.95 if "add_zero" in cand["name"] else 0.05
        return {"affirmative_prob": prob, "relevant": prob >= 0.5,
                "method": "custom", "model": "unit", "samples": 1}

    resp = RR.rerank("prove n + 0 = n", _candidates(), scorer=scorer)
    assert _names(resp)[0] == "Nat.add_zero"
    assert resp["results"][0]["score_method"] == "custom"
    assert resp["model"] == "unit"


# --- contract / shape --------------------------------------------------------


def test_contract_shape_and_preserved_fields():
    resp = RR.rerank("add zero", _candidates(), env=MOCK_ENV)
    assert set(resp) >= {"ok", "op", "method", "query", "model", "degraded", "reason", "count", "results"}
    assert resp["op"] == "rerank"
    r = resp["results"][0]
    # original retrieval fields preserved + rerank annotations added
    assert set(r) >= {"name", "module", "kind", "score", "base_rank", "base_score",
                      "affirmative_prob", "relevant", "score_method", "rank"}
    assert r["base_score"] == r["score"]


def test_k_limits_output():
    cands = [{"name": f"L.thm_{i}", "module": "M", "kind": "theorem", "score": float(i)} for i in range(5)]
    resp = RR.rerank("thm", cands, k=2, env=MOCK_ENV)
    assert resp["count"] == 2
    assert len(resp["results"]) == 2


def test_ranks_are_contiguous_and_sorted_by_prob():
    resp = RR.rerank("Nat add zero", _candidates(), env=MOCK_ENV)
    probs = [r["affirmative_prob"] for r in resp["results"]]
    assert probs == sorted(probs, reverse=True)
    assert [r["rank"] for r in resp["results"]] == list(range(len(resp["results"])))


# --- graceful identity fallback ----------------------------------------------


def test_identity_fallback_when_scorer_raises():
    def broken(query, cand):
        raise RuntimeError("model unavailable")

    cands = _candidates()
    resp = RR.rerank("anything", cands, scorer=broken)
    assert resp["ok"] is True            # never raises to the caller
    assert resp["degraded"] is True
    assert resp["method"] == "identity"
    assert "model unavailable" in resp["reason"]
    # input order preserved
    assert _names(resp) == [c["name"] for c in cands]


def test_identity_fallback_no_model_configured(monkeypatch):
    # No mock env, and force the provider scorer to be unavailable at call time
    # (simulating litellm/model absence) -> degrade to identity, not an error.
    monkeypatch.delenv("THEOREMATA_MODEL_MOCK", raising=False)

    def _boom(*a, **k):
        raise RuntimeError("litellm not installed")

    monkeypatch.setattr(RR, "make_provider_scorer", lambda **k: _boom)
    cands = _candidates()
    resp = RR.rerank("anything", cands)  # uses real default resolver
    assert resp["degraded"] is True
    assert resp["method"] == "identity"
    assert _names(resp) == [c["name"] for c in cands]


def test_empty_inputs_are_identity_not_degraded():
    assert RR.rerank("", _candidates())["method"] == "identity"
    resp = RR.rerank("q", [])
    assert resp["method"] == "identity"
    assert resp["degraded"] is False
    assert resp["results"] == []


# --- caching -----------------------------------------------------------------


def test_cache_avoids_rescoring():
    calls = {"n": 0}

    def counting_scorer(query, cand):
        calls["n"] += 1
        prob = 0.9 if "add_zero" in cand["name"] else 0.2
        return {"affirmative_prob": prob, "relevant": prob >= 0.5,
                "method": "custom", "model": "unit", "samples": 1}

    cache = RR.ScoreCache()
    cands = _candidates()
    RR.rerank("goal G", cands, scorer=counting_scorer, cache=cache)
    assert calls["n"] == len(cands)
    # second call with the same (query, candidates) hits the cache -> no new scoring
    RR.rerank("goal G", cands, scorer=counting_scorer, cache=cache)
    assert calls["n"] == len(cands)
    # a different query re-scores
    RR.rerank("different goal", cands, scorer=counting_scorer, cache=cache)
    assert calls["n"] == 2 * len(cands)


def test_run_entrypoint_matches_rerank():
    resp = RR.run("Nat add zero", _candidates(), 1, env=MOCK_ENV)
    assert resp["op"] == "rerank"
    assert resp["count"] == 1
    assert resp["results"][0]["name"] == "Nat.add_zero"
