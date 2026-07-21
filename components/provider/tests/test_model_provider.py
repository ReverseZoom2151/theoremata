"""Unit tests for the model provider adapter.

Pure-function and mock-mode tests only -- these never import litellm and never
touch the network.
"""
from __future__ import annotations

import pytest

from theoremata_tools import model_provider as mp


# --------------------------------------------------------------------------- #
# JSON extraction.
# --------------------------------------------------------------------------- #
def test_extract_fenced_json():
    text = '```json\n{"reply": "hi", "mutations": []}\n```'
    assert mp.extract_json_object(text) == {"reply": "hi", "mutations": []}


def test_extract_bare_json():
    text = '{"lean": "theorem t : True := trivial"}'
    assert mp.extract_json_object(text) == {"lean": "theorem t : True := trivial"}


def test_extract_embedded_json():
    text = 'Sure! Here is the result:\n{"obligations": []}\nHope that helps.'
    assert mp.extract_json_object(text) == {"obligations": []}


def test_extract_json_with_nested_braces_and_strings():
    text = 'noise {"a": {"b": 1}, "s": "has } brace"} trailing'
    assert mp.extract_json_object(text) == {"a": {"b": 1}, "s": "has } brace"}


def test_extract_unbalanced_raises():
    with pytest.raises(ValueError):
        mp.extract_json_object("no json here at all")


def test_extract_truncated_object_raises():
    with pytest.raises(ValueError):
        mp.extract_json_object('{"reply": "unterminated')


# --------------------------------------------------------------------------- #
# Reasoning-model replies: a leading think block precedes the JSON. The tokens
# below are literal model output (data), not markup of ours.
# --------------------------------------------------------------------------- #
THINK_OPEN = "<think>"  # kept out of prose so no angle-bracket "tag" appears in code
THINK_CLOSE = "</think>"


def test_extract_think_block_then_fenced_json():
    # Qwen-style: chain-of-thought (with distracting braces) then fenced JSON.
    text = (
        f"{THINK_OPEN}Let me reason. Maybe the answer is {{oops: not json}} or "
        f"{{a: 1}}. I will now answer.{THINK_CLOSE}\n"
        '```json\n{"reply": "hi", "mutations": []}\n```'
    )
    assert mp.extract_json_object(text) == {"reply": "hi", "mutations": []}


def test_extract_think_block_then_bare_object():
    text = f"{THINK_OPEN}deliberating {{x}}{THINK_CLOSE}\n{{\"lean\": \"trivial\"}}"
    assert mp.extract_json_object(text) == {"lean": "trivial"}


def test_extract_think_block_with_leading_whitespace():
    # Some models emit a newline before the think span.
    text = f"\n\n  {THINK_OPEN}hmm{THINK_CLOSE}{{\"obligations\": []}}"
    assert mp.extract_json_object(text) == {"obligations": []}


def test_extract_unterminated_think_raises():
    # Truncation: think span opens and is never closed, no JSON survives.
    with pytest.raises(ValueError):
        mp.extract_json_object(f"{THINK_OPEN}reasoning got cut off with a {{ brace")


def test_extract_think_block_then_prose_then_object():
    text = (
        f"{THINK_OPEN}internal notes {{k: v}}{THINK_CLOSE}\n"
        'Here is my answer:\n{"reply": "ok", "mutations": []}\nDone.'
    )
    assert mp.extract_json_object(text) == {"reply": "ok", "mutations": []}


def test_think_block_does_not_swallow_answer_braces_in_reasoning():
    # The reasoning contains a full JSON-looking object; extraction must ignore
    # it and return the real post-think answer, proving we strip before scanning.
    text = (
        f"{THINK_OPEN}A tempting wrong object: "
        '{"reply": "WRONG", "mutations": ["nope"]}'
        f"{THINK_CLOSE}\n"
        '{"reply": "RIGHT", "mutations": []}'
    )
    assert mp.extract_json_object(text) == {"reply": "RIGHT", "mutations": []}


def test_prose_preamble_then_bare_object_no_think():
    # Reasoning models sometimes skip the think span but still add a preamble.
    text = 'Here is my answer:\n{"reply": "ok", "mutations": []}'
    assert mp.extract_json_object(text) == {"reply": "ok", "mutations": []}


# --------------------------------------------------------------------------- #
# Required-key validation.
# --------------------------------------------------------------------------- #
def test_missing_required_keys_reports_absent():
    schema = {"type": "object", "required": ["reply", "mutations"]}
    assert mp.missing_required_keys({"reply": "ok"}, schema) == ["mutations"]


def test_missing_required_keys_none_when_present():
    schema = {"type": "object", "required": ["reply"]}
    assert mp.missing_required_keys({"reply": "ok"}, schema) == []


def test_validate_raises_on_missing():
    schema = {"type": "object", "required": ["obligations"]}
    with pytest.raises(ValueError):
        mp.validate_against_schema({}, schema)


def test_validate_noop_for_non_object_schema():
    # No exception for absent/non-object schemas.
    mp.validate_against_schema({}, None)
    mp.validate_against_schema({}, {"type": "string"})


# --------------------------------------------------------------------------- #
# Role -> model env routing.
# --------------------------------------------------------------------------- #
def test_role_env_routing_specific_beats_global():
    env = {
        "THEOREMATA_MODEL": "anthropic/global",
        "THEOREMATA_MODEL_PROOF_DECOMPOSER": "openai/gpt-role",
    }
    assert mp.model_for_role("proof_decomposer", env) == "openai/gpt-role"


def test_role_env_routing_falls_back_to_global():
    env = {"THEOREMATA_MODEL": "anthropic/global"}
    assert mp.model_for_role("lean_formalizer", env) == "anthropic/global"


def test_role_env_routing_default_when_unset():
    assert mp.model_for_role("anything", {}) == mp.DEFAULT_MODEL


def test_role_env_suffix_non_alnum():
    assert mp._role_env_suffix("mathematical_research_orchestrator") == (
        "MATHEMATICAL_RESEARCH_ORCHESTRATOR"
    )
    assert mp._role_env_suffix("lean-formalizer!") == "LEAN_FORMALIZER"


def test_fallback_models_parsing():
    env = {"THEOREMATA_MODEL_FALLBACK": "a/one, b/two ,, c/three"}
    assert mp.fallback_models(env) == ["a/one", "b/two", "c/three"]


# --------------------------------------------------------------------------- #
# Mock-mode generation for each role.
# --------------------------------------------------------------------------- #
MOCK_ENV = {"THEOREMATA_MODEL_MOCK": "1"}


def test_generate_mock_proof_decomposer():
    request = {
        "role": "proof_decomposer",
        "task": "decompose",
        "context": {},
        "output_schema": {"type": "object", "required": ["obligations"]},
    }
    content, model = mp.generate(request, env=MOCK_ENV)
    assert model == "mock"
    assert mp.missing_required_keys(content, request["output_schema"]) == []
    assert isinstance(content["obligations"], list)
    assert content["obligations"][0]["title"] == "Mock"


def test_generate_mock_lean_formalizer():
    request = {
        "role": "lean_formalizer",
        "task": "formalize",
        "context": {},
        "output_schema": {"type": "object", "required": ["lean"]},
    }
    content, model = mp.generate(request, env=MOCK_ENV)
    assert model == "mock"
    assert "theorem theoremata_mock" in content["lean"]
    assert mp.missing_required_keys(content, request["output_schema"]) == []


def test_generate_mock_orchestrator():
    request = {
        "role": "mathematical_research_orchestrator",
        "task": "chat",
        "context": {},
        "output_schema": {"type": "object", "required": ["reply", "mutations"]},
    }
    content, model = mp.generate(request, env=MOCK_ENV)
    assert model == "mock"
    assert content["reply"] == "[mock] ok"
    assert content["mutations"] == []
    assert mp.missing_required_keys(content, request["output_schema"]) == []


def test_generate_mock_respects_extra_required_key():
    request = {
        "role": "mathematical_research_orchestrator",
        "task": "chat",
        "context": {},
        "output_schema": {
            "type": "object",
            "required": ["reply", "mutations", "confidence"],
        },
    }
    content, _ = mp.generate(request, env=MOCK_ENV)
    assert "confidence" in content
    assert mp.missing_required_keys(content, request["output_schema"]) == []


def test_build_messages_shape():
    request = {
        "role": "proof_decomposer",
        "task": "do it",
        "context": {"goal": "x"},
        "output_schema": {"type": "object", "required": ["obligations"]},
    }
    messages = mp.build_messages(request)
    assert messages[0]["role"] == "system"
    assert "proof_decomposer" in messages[0]["content"]
    assert messages[1]["role"] == "user"
    assert "goal" in messages[1]["content"]
