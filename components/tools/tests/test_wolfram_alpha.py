"""Tests for the Wolfram|Alpha client. All run with no AppID and no network,
which is the CI condition."""
from __future__ import annotations

import json

import pytest

from theoremata_tools import wolfram_alpha


@pytest.fixture(autouse=True)
def _clear_env(monkeypatch):
    monkeypatch.delenv(wolfram_alpha.APPID_ENV, raising=False)
    monkeypatch.delenv(wolfram_alpha.ENABLED_ENV, raising=False)


def test_unavailable_without_opt_in_even_with_an_appid(monkeypatch):
    # The query text leaves the machine, so an AppID alone is not consent.
    monkeypatch.setenv(wolfram_alpha.APPID_ENV, "XXXX-YYYY")
    assert wolfram_alpha.available() is False


def test_unavailable_without_an_appid(monkeypatch):
    monkeypatch.setenv(wolfram_alpha.ENABLED_ENV, "1")
    assert wolfram_alpha.available() is False


def test_query_degrades_cleanly_when_unavailable():
    out = wolfram_alpha.query("population of france")
    assert out["unavailable"] is True
    assert out["ok"] is False
    assert out["trusted"] is False


def test_unavailable_reason_distinguishes_off_from_no_appid(monkeypatch):
    off = wolfram_alpha.unavailable_response()["reason"]
    monkeypatch.setenv(wolfram_alpha.ENABLED_ENV, "1")
    no_appid = wolfram_alpha.unavailable_response()["reason"]
    assert off != no_appid
    assert wolfram_alpha.APPID_ENV in no_appid


def _enable(monkeypatch):
    monkeypatch.setenv(wolfram_alpha.ENABLED_ENV, "1")
    monkeypatch.setenv(wolfram_alpha.APPID_ENV, "TEST-APPID")


def test_assumptions_are_always_surfaced(monkeypatch):
    # The load-bearing test. Alpha disambiguated the query, so the caller MUST be
    # told, or it will read an answer to a neighbouring question as its own.
    _enable(monkeypatch)
    payload = {
        "queryresult": {
            "success": True,
            "pods": [
                {
                    "id": "Result",
                    "title": "Result",
                    "subpods": [{"plaintext": "42", "minput": "Total[{40, 2}]"}],
                }
            ],
            "assumptions": {
                "assumption": [
                    {
                        "type": "Clash",
                        "values": [
                            {"desc": "a variable"},
                            {"desc": "the unit of length"},
                        ],
                    }
                ]
            },
        }
    }
    monkeypatch.setattr(
        wolfram_alpha, "_get", lambda url, timeout: (200, json.dumps(payload))
    )
    out = wolfram_alpha.query("x")
    assert out["ok"] is True
    assert out["assumptions"], "an ambiguous query must report its disambiguation"
    assert "a variable" in out["assumptions"][0]
    # The alternative reading is what proves the query WAS ambiguous.
    assert "the unit of length" in out["assumptions"][0]
    assert out["trusted"] is False


def test_assumptions_key_present_even_when_empty(monkeypatch):
    # Always present so a caller cannot forget to check it.
    _enable(monkeypatch)
    payload = {"queryresult": {"success": True, "pods": []}}
    monkeypatch.setattr(
        wolfram_alpha, "_get", lambda url, timeout: (200, json.dumps(payload))
    )
    out = wolfram_alpha.query("2+2")
    assert out["assumptions"] == []


def test_wolfram_input_is_extracted(monkeypatch):
    # minput is Alpha's own code rendering of the query, which is what makes
    # drift inspectable instead of hidden in prose.
    _enable(monkeypatch)
    payload = {
        "queryresult": {
            "success": True,
            "pods": [
                {
                    "id": "Result",
                    "subpods": [{"plaintext": "4", "minput": "Plus[2, 2]"}],
                }
            ],
        }
    }
    monkeypatch.setattr(
        wolfram_alpha, "_get", lambda url, timeout: (200, json.dumps(payload))
    )
    assert wolfram_alpha.query("2+2")["wolfram_input"] == "Plus[2, 2]"


def test_not_understood_is_not_a_pass(monkeypatch):
    # success=false means Alpha did not understand. That is not an empty answer
    # and must not read as one.
    _enable(monkeypatch)
    payload = {"queryresult": {"success": False, "pods": []}}
    monkeypatch.setattr(
        wolfram_alpha, "_get", lambda url, timeout: (200, json.dumps(payload))
    )
    out = wolfram_alpha.query("asdfqwer")
    assert out["ok"] is False
    assert out["understood"] is False


def test_http_error_is_reported_not_swallowed(monkeypatch):
    _enable(monkeypatch)
    monkeypatch.setattr(wolfram_alpha, "_get", lambda url, timeout: (403, "bad appid"))
    out = wolfram_alpha.query("2+2")
    assert out["ok"] is False
    assert out["unavailable"] is False
    assert "403" in out["error"]


def test_network_failure_is_not_unavailable(monkeypatch):
    # A dead network is a transient failure, distinct from "not configured".
    _enable(monkeypatch)
    monkeypatch.setattr(wolfram_alpha, "_get", lambda url, timeout: None)
    out = wolfram_alpha.query("2+2")
    assert out["ok"] is False
    assert out["unavailable"] is False


def test_run_rejects_empty_input():
    assert wolfram_alpha.run({"op": "query", "input": " "})["ok"] is False


def test_run_rejects_unknown_op():
    assert wolfram_alpha.run({"op": "nope"})["ok"] is False
