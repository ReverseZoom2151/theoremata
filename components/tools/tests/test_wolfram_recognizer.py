"""Tests for the Fast Query Recognizer triage step. All run with no AppID and no
network, which is the CI condition."""
from __future__ import annotations

import json

import pytest

from theoremata_tools import wolfram_alpha, wolfram_recognizer


@pytest.fixture(autouse=True)
def _clear_env(monkeypatch):
    monkeypatch.delenv(wolfram_recognizer.APPID_ENV, raising=False)
    monkeypatch.delenv(wolfram_recognizer.ENABLED_ENV, raising=False)


def _enable(monkeypatch):
    monkeypatch.setenv(wolfram_recognizer.ENABLED_ENV, "1")
    monkeypatch.setenv(wolfram_recognizer.APPID_ENV, "TEST-APPID")


def _classification(monkeypatch, payload):
    monkeypatch.setattr(
        wolfram_recognizer, "_get", lambda url, timeout: (200, json.dumps(payload))
    )


# Any key that could be read as a claim about the mathematics. None of these may
# ever appear in any response this module produces.
_VERDICT_KEYS = {
    "verdict",
    "proved",
    "proven",
    "verified",
    "refuted",
    "disproved",
    "true",
    "false",
    "valid",
    "holds",
    "confidence",
    "probability",
}


def _assert_no_verdict(response, path="response"):
    assert isinstance(response, dict)
    assert response.get("trusted") is False, f"{path} must be marked untrusted"
    for key, value in response.items():
        assert key.lower() not in _VERDICT_KEYS, f"{path}.{key} reads as a verdict"
        if isinstance(value, dict) and "trusted" in value:
            _assert_no_verdict(value, f"{path}.{key}")


def test_env_vars_are_shared_with_the_expensive_client():
    # Imported rather than redefined, so the opt-in cannot drift such that triage
    # runs against a service the operator never consented to.
    assert wolfram_recognizer.APPID_ENV is wolfram_alpha.APPID_ENV
    assert wolfram_recognizer.ENABLED_ENV is wolfram_alpha.ENABLED_ENV


def test_unavailable_without_opt_in_even_with_an_appid(monkeypatch):
    monkeypatch.setenv(wolfram_recognizer.APPID_ENV, "XXXX-YYYY")
    assert wolfram_recognizer.available() is False


def test_recognize_degrades_cleanly_when_unavailable():
    out = wolfram_recognizer.recognize("is every group of order 6 solvable")
    assert out["unavailable"] is True
    assert out["ok"] is False
    assert out["trusted"] is False
    # Unavailable is NOT "not accepted". Null, not False.
    assert out["worth_querying"] is None
    _assert_no_verdict(out)


def test_unavailable_is_distinct_from_not_accepted(monkeypatch):
    unavailable = wolfram_recognizer.recognize("x")
    _enable(monkeypatch)
    _classification(monkeypatch, {"query": {"accepted": "false"}})
    rejected = wolfram_recognizer.recognize("x")
    assert unavailable["worth_querying"] is None
    assert rejected["worth_querying"] is False
    assert unavailable["routing_hint"] != rejected["routing_hint"]


def test_accepted_classification_is_parsed(monkeypatch):
    _enable(monkeypatch)
    _classification(
        monkeypatch,
        {
            "query": {
                "accepted": "true",
                "domain": "Math",
                "resultsignificancescore": "90",
                "timing": "2.5",
                "summarybox": "summarybox/data/abc",
            }
        },
    )
    out = wolfram_recognizer.recognize("integrate x^2")
    assert out["ok"] is True
    assert out["worth_querying"] is True
    assert out["routing_hint"] == "likely_answerable"
    assert out["domain"] == "Math"
    # Named for what it is: relevance of Alpha's own answer, not P(theorem true).
    assert out["relevance_score_0_100"] == 90.0
    assert out["recognizer_timing_ms"] == 2.5
    assert out["summarybox"] == "summarybox/data/abc"
    _assert_no_verdict(out)


def test_rejected_triage_skips_the_expensive_path(monkeypatch):
    _enable(monkeypatch)
    _classification(monkeypatch, {"query": {"accepted": "false", "domain": "Other"}})

    def _must_not_run(*args, **kwargs):  # pragma: no cover - failure path
        raise AssertionError("the expensive query must not be sent after a reject")

    monkeypatch.setattr(wolfram_alpha, "query", _must_not_run)

    out = wolfram_recognizer.triage_then_query("let G be a finite simple group")
    assert out["expensive_path_skipped"] is True
    # The load-bearing assertion: skipped, not searched-and-empty.
    assert "result" not in out
    assert "never sent" in out["skip_reason"]
    assert "not a finding" in out["skip_reason"]
    _assert_no_verdict(out)


def test_accepted_triage_runs_the_expensive_path(monkeypatch):
    _enable(monkeypatch)
    _classification(monkeypatch, {"query": {"accepted": "true"}})
    monkeypatch.setattr(
        wolfram_alpha,
        "query",
        lambda text, podids=None, timeout=None: {
            "ok": True,
            "unavailable": False,
            "assumptions": [],
            "trusted": False,
        },
    )
    out = wolfram_recognizer.triage_then_query("2+2")
    assert out["expensive_path_skipped"] is False
    assert out["result"]["ok"] is True
    _assert_no_verdict(out)


def test_triage_is_unavailable_when_not_configured(monkeypatch):
    def _must_not_run(*args, **kwargs):  # pragma: no cover - failure path
        raise AssertionError("nothing may be sent while unavailable")

    monkeypatch.setattr(wolfram_alpha, "query", _must_not_run)
    out = wolfram_recognizer.triage_then_query("2+2")
    assert out["unavailable"] is True
    assert out["expensive_path_skipped"] is True
    assert "unavailable" in out["skip_reason"]
    _assert_no_verdict(out)


def test_network_failure_gives_no_hint_not_a_rejection(monkeypatch):
    # A dead network says nothing about the query, so it must not masquerade as
    # a negative classification.
    _enable(monkeypatch)
    monkeypatch.setattr(wolfram_recognizer, "_get", lambda url, timeout: None)
    out = wolfram_recognizer.recognize("2+2")
    assert out["ok"] is False
    assert out["unavailable"] is False
    assert out["worth_querying"] is None
    assert out["routing_hint"] == "no_hint"


def test_no_hint_still_queries_by_default(monkeypatch):
    _enable(monkeypatch)
    monkeypatch.setattr(wolfram_recognizer, "_get", lambda url, timeout: None)
    monkeypatch.setattr(
        wolfram_alpha,
        "query",
        lambda text, podids=None, timeout=None: {"ok": True, "trusted": False},
    )
    assert wolfram_recognizer.triage_then_query("2+2")["expensive_path_skipped"] is False


def test_no_hint_can_be_configured_to_skip(monkeypatch):
    _enable(monkeypatch)
    monkeypatch.setattr(wolfram_recognizer, "_get", lambda url, timeout: (500, "boom"))
    out = wolfram_recognizer.triage_then_query("2+2", query_when_no_hint=False)
    assert out["expensive_path_skipped"] is True
    assert "result" not in out


def test_unparseable_payload_degrades_to_no_hint(monkeypatch):
    _enable(monkeypatch)
    monkeypatch.setattr(wolfram_recognizer, "_get", lambda url, timeout: (200, "not json at all"))
    out = wolfram_recognizer.recognize("2+2")
    assert out["worth_querying"] is None
    assert out["routing_hint"] == "no_hint"


def test_rejection_carries_no_relevance_score_to_misuse(monkeypatch):
    # A high score on a rejected query must not become a "probably true" signal.
    _enable(monkeypatch)
    _classification(
        monkeypatch, {"query": {"accepted": "false", "resultsignificancescore": "99"}}
    )
    out = wolfram_recognizer.recognize("the Riemann hypothesis is true")
    assert out["worth_querying"] is False
    # The score is present but explicitly named as relevance of Alpha's answer.
    assert out["relevance_score_0_100"] == 99.0
    assert "confidence" not in out
    assert "true" not in out
    _assert_no_verdict(out)


def test_flat_payload_shape_is_tolerated(monkeypatch):
    _enable(monkeypatch)
    _classification(monkeypatch, {"accepted": True, "domain": "Math"})
    assert wolfram_recognizer.recognize("2+2")["worth_querying"] is True


def test_run_available_op():
    out = wolfram_recognizer.run({"op": "available"})
    assert out["ok"] is True
    assert out["available"] is False
    assert out["reason"]


def test_run_rejects_empty_input():
    assert wolfram_recognizer.run({"op": "recognize", "input": " "})["ok"] is False


def test_run_rejects_unknown_op():
    assert wolfram_recognizer.run({"op": "nope"})["ok"] is False
