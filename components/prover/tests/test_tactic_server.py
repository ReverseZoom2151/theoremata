"""Offline tests for the LeanCopilot-style tactic server.

All tests run without network / litellm: the mock-mode provider path uses
``THEOREMATA_MODEL_MOCK=1`` and the richer cases inject a deterministic
in-process provider.
"""
from __future__ import annotations

import json
import threading
import urllib.request

import pytest

from theoremata_tools import tactic_server as ts


# --------------------------------------------------------------------------- #
# Fake providers (deterministic, offline).
# --------------------------------------------------------------------------- #
def _provider_with_tactics(tactics):
    def _provider(request):
        return {"tactics": tactics}, "fake"

    return _provider


def _embed_provider(vector):
    def _provider(request):
        return {"embedding": vector}, "fake"

    return _provider


# --------------------------------------------------------------------------- #
# generate: scores from logprob / score / default, dedup keeps max.
# --------------------------------------------------------------------------- #
def test_generate_scores_from_logprob_and_default():
    provider = _provider_with_tactics(
        [
            {"tactic": "simp", "logprob": 0.0},   # exp(0) == 1.0
            {"tactic": "rfl", "score": 0.4},
            {"tactic": "ring"},                    # default 1.0
        ]
    )
    out = ts.generate("m", "goal", provider=provider)
    scored = {c["output"]: c["score"] for c in out}
    assert scored["simp"] == pytest.approx(1.0)
    assert scored["rfl"] == pytest.approx(0.4)
    assert scored["ring"] == pytest.approx(ts.DEFAULT_SCORE)


def test_generate_dedup_keeps_max_score():
    provider = _provider_with_tactics(
        [
            {"tactic": "simp", "score": 0.2},
            {"tactic": "simp", "score": 0.9},   # duplicate -> keep max
            {"tactic": "simp", "score": 0.5},
        ]
    )
    out = ts.generate("m", "goal", provider=provider)
    assert out == [{"output": "simp", "score": pytest.approx(0.9)}]


def test_generate_orders_by_descending_score():
    provider = _provider_with_tactics(
        [
            {"tactic": "a", "score": 0.1},
            {"tactic": "b", "score": 0.9},
            {"tactic": "c", "score": 0.5},
        ]
    )
    out = ts.generate("m", "goal", provider=provider)
    assert [c["output"] for c in out] == ["b", "c", "a"]


def test_generate_mock_provider_returns_valid_candidates(monkeypatch):
    """Real mock path (THEOREMATA_MODEL_MOCK=1) yields a scored candidate list."""
    monkeypatch.setenv("THEOREMATA_MODEL_MOCK", "1")
    out = ts.generate("theoremata", "⊢ True")
    assert isinstance(out, list) and out
    for cand in out:
        assert isinstance(cand["output"], str) and cand["output"]
        assert isinstance(cand["score"], float)


# --------------------------------------------------------------------------- #
# rank_candidates: self-reference + bare aesop filtering.
# --------------------------------------------------------------------------- #
def test_rank_filters_bare_aesop_but_keeps_qualified_aesop():
    state = {"theorem_name": "Nat.add_comm"}
    out = ts.rank_candidates(
        state,
        [
            {"output": "aesop", "score": 0.9},
            {"output": "aesop (config := ...)", "score": 0.3},
            {"output": "simp", "score": 0.5},
        ],
    )
    outputs = [c["output"] for c in out]
    assert "aesop" not in outputs
    assert "aesop (config := ...)" in outputs
    assert "simp" in outputs


def test_rank_filters_self_reference():
    state = {"full_name": "Group.my_lemma"}
    out = ts.rank_candidates(
        state,
        [
            {"output": "exact my_lemma", "score": 0.9},   # self reference
            {"output": "apply Group.my_lemma", "score": 0.8},  # self reference
            {"output": "exact other_lemma", "score": 0.4},
        ],
    )
    outputs = [c["output"] for c in out]
    assert outputs == ["exact other_lemma"]


def test_rank_self_reference_no_substring_false_positive():
    state = {"theorem_name": "foo"}
    out = ts.rank_candidates(state, [{"output": "exact foobar", "score": 0.5}])
    assert [c["output"] for c in out] == ["exact foobar"]


def test_rank_accepts_bare_strings_and_dedups():
    state = {"theorem_name": "T"}
    out = ts.rank_candidates(state, ["simp", "simp", "aesop", "rfl"])
    outputs = [c["output"] for c in out]
    assert outputs.count("simp") == 1
    assert "aesop" not in outputs
    assert "rfl" in outputs


# --------------------------------------------------------------------------- #
# encode: deterministic offline vector; real provider vector passes through.
# --------------------------------------------------------------------------- #
def test_encode_deterministic_and_stable():
    provider = _embed_provider([0.0])  # degenerate -> deterministic fallback
    v1 = ts.encode("m", "hello", provider=provider)
    v2 = ts.encode("m", "hello", provider=provider)
    v3 = ts.encode("m", "world", provider=provider)
    assert v1 == v2
    assert v1 != v3
    assert len(v1) == ts.EMBED_DIM
    assert all(isinstance(x, float) for x in v1)


def test_encode_passes_through_real_vector():
    provider = _embed_provider([0.1, 0.2, 0.3])
    assert ts.encode("m", "x", provider=provider) == [0.1, 0.2, 0.3]


# --------------------------------------------------------------------------- #
# HTTP shim round-trip on an ephemeral localhost port.
# --------------------------------------------------------------------------- #
def _post(url, payload):
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url, data=data, headers={"Content-Type": "application/json"}, method="POST"
    )
    with urllib.request.urlopen(req, timeout=5) as resp:
        return resp.status, json.loads(resp.read().decode("utf-8"))


@pytest.fixture()
def running_server():
    provider = _provider_with_tactics(
        [{"tactic": "simp", "score": 0.7}, {"tactic": "aesop", "score": 0.9}]
    )
    server = ts.make_server("localhost", 0, provider=provider)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    host, port = server.server_address[:2]
    try:
        yield f"http://{host}:{port}"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def test_http_generate_roundtrip(running_server):
    status, body = _post(
        running_server + "/generate",
        {"name": "m", "input": "⊢ p ∧ q", "prefix": ""},
    )
    assert status == 200
    outputs = body["outputs"]
    assert {"output": "simp", "score": pytest.approx(0.7)} in outputs
    assert {"output": "aesop", "score": pytest.approx(0.9)} in outputs
    # schema shape: list of {output, score}
    for cand in outputs:
        assert set(cand) == {"output", "score"}


def test_http_encode_roundtrip(running_server):
    status, body = _post(running_server + "/encode", {"name": "m", "input": "Nat"})
    assert status == 200
    assert isinstance(body["outputs"], list)
    assert len(body["outputs"]) == ts.EMBED_DIM
    assert all(isinstance(x, float) for x in body["outputs"])


def test_http_unknown_path_404(running_server):
    with pytest.raises(urllib.error.HTTPError) as exc:
        _post(running_server + "/nope", {})
    assert exc.value.code == 404


# --------------------------------------------------------------------------- #
# Worker-style dispatch surface.
# --------------------------------------------------------------------------- #
def test_run_dispatch_generate(monkeypatch):
    monkeypatch.setenv("THEOREMATA_MODEL_MOCK", "1")
    out = ts.run({"op": "generate", "name": "m", "input": "⊢ True"})
    assert isinstance(out["outputs"], list) and out["outputs"]


def test_run_dispatch_rank():
    out = ts.run(
        {
            "op": "rank",
            "state": {"theorem_name": "T"},
            "candidates": ["aesop", "simp", "simp"],
        }
    )
    outputs = [c["output"] for c in out["outputs"]]
    assert outputs == ["simp"]


def test_run_unknown_op_raises():
    with pytest.raises(ValueError):
        ts.run({"op": "bogus"})
