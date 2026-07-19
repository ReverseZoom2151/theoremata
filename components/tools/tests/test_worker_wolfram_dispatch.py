"""The four Wolfram tools must be REACHABLE through the worker dispatch.

Each module was unit-tested in isolation, but a tool nobody can route to is a
tool that does not exist as far as `theoremata tool '{"tool": ...}'` is
concerned. These tests exercise the dispatch entry point itself.

They also pin the CI condition: with no engine, no wolframscript and no AppID,
every op must return its clean "unavailable" response instead of raising, and
none of them may look like a certification.
"""
from __future__ import annotations

import pytest

from theoremata_tools.worker import dispatch

# Every switch that could make a real engine or endpoint reachable. Cleared so
# the test asserts the degrade path even on a developer machine that has Wolfram
# installed and enabled.
_ENGINE_ENV = (
    "THEOREMATA_WOLFRAM",
    "THEOREMATA_WOLFRAM_ENABLED",
    "THEOREMATA_WOLFRAM_CLOUD_KEY",
    "THEOREMATA_WOLFRAM_APPID",
    "THEOREMATA_WOLFRAM_ALPHA_ENABLED",
)


@pytest.fixture
def no_engine(monkeypatch):
    for name in _ENGINE_ENV:
        monkeypatch.delenv(name, raising=False)


def test_wolfram_link_available(no_engine):
    out = dispatch({"tool": "wolfram_link", "op": "available"})
    assert out["ok"] is True
    assert out["available"] is False
    assert out["reason"]


def test_wolfram_link_evaluate(no_engine):
    out = dispatch({"tool": "wolfram_link", "op": "evaluate", "code": "1 + 1"})
    assert out["unavailable"] is True
    # Restated at the boundary: a raw evaluation is never a checked result.
    assert out["trusted"] is False


def test_wolfram_alpha_available(no_engine):
    out = dispatch({"tool": "wolfram_alpha", "op": "available"})
    assert out["ok"] is True
    assert out["available"] is False


def test_wolfram_alpha_query(no_engine):
    out = dispatch({"tool": "wolfram_alpha", "op": "query", "input": "2+2"})
    assert out["unavailable"] is True


def test_wolfram_falsify_falsify(no_engine):
    out = dispatch({
        "tool": "wolfram_falsify",
        "op": "falsify",
        "variables": ["n"],
        "claim": "n * n >= 0",
    })
    assert out["verdict"] == "unavailable"
    assert out["available"] is False
    # The asymmetry: an unreachable oracle refutes nothing and proves nothing.
    assert out["refuted"] is False
    assert out["proved"] is False
    assert out["trusted"] is False


def test_wolfram_falsify_integer_relation(no_engine):
    out = dispatch({
        "tool": "wolfram_falsify",
        "op": "integer_relation",
        "constants": ["Pi", "1"],
    })
    assert out["verdict"] == "unavailable"
    assert out["status"] == "unproved_conjecture"
    assert out["proved"] is False


@pytest.mark.parametrize("op", ["sos", "nullstellensatz", "sturm"])
def test_wolfram_cert_ops_unavailable(no_engine, op):
    # wolfram_cert lives under components/verify/python; reaching it here also
    # proves the shared `theoremata_tools` namespace package resolves both roots.
    out = dispatch({"tool": "wolfram_cert", "op": op})
    assert out["unavailable"] is True
    assert out["ok"] is False
    # No engine means no document, and therefore no certificate.
    assert out["cert"] is None
    assert out["checked"] is False


def test_wolfram_tools_are_not_meta_tools():
    from theoremata_tools.worker import is_meta_tool_op

    for tool in (
        "wolfram_link",
        "wolfram_alpha",
        "wolfram_recognizer",
        "wolfram_falsify",
        "wolfram_cert",
    ):
        assert is_meta_tool_op(tool) is False


def test_unknown_wolfram_op_still_rejected(no_engine):
    with pytest.raises(ValueError):
        dispatch({"tool": "wolfram_cert", "op": "definitely_not_an_op"})


@pytest.mark.parametrize("op", ["available", "recognize", "triage_then_query"])
def test_wolfram_recognizer_is_reachable_and_degrades(no_engine, op):
    # The triage rung landed after the other four were registered, so this pins
    # that it is actually dispatchable rather than merely built.
    out = dispatch({"tool": "wolfram_recognizer", "op": op, "text": "2+2"})
    assert out.get("trusted", False) is False
    # A routing hint is never a mathematical verdict, in any response.
    for banned in ("verdict", "proved", "verified", "refuted"):
        assert banned not in out
