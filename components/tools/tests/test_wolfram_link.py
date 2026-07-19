"""Tests for the Wolfram bridge. All run with NO engine present, which is the
CI condition and the default on any machine that has not opted in."""
from __future__ import annotations

import pytest

from theoremata_tools import wolfram_link


@pytest.fixture(autouse=True)
def _clear_env(monkeypatch):
    """Every test starts from opted-out, so a developer machine with a real
    engine installed cannot change what these assert."""
    monkeypatch.delenv(wolfram_link.WOLFRAM_ENABLED_ENV, raising=False)
    monkeypatch.delenv(wolfram_link.WOLFRAM_BINARY_ENV, raising=False)


def test_unavailable_by_default_even_if_a_binary_exists(monkeypatch):
    # Opt-in is required, not merely a binary on PATH: consulting a licence-gated
    # engine must be a decision, not a side effect of installation.
    monkeypatch.setattr(wolfram_link, "_binary", lambda: "/usr/bin/wolframscript")
    assert wolfram_link.available() is False


def test_enabled_but_no_binary_is_still_unavailable(monkeypatch):
    monkeypatch.setenv(wolfram_link.WOLFRAM_ENABLED_ENV, "1")
    monkeypatch.setattr(wolfram_link, "_binary", lambda: None)
    assert wolfram_link.available() is False


def test_available_needs_both(monkeypatch):
    monkeypatch.setenv(wolfram_link.WOLFRAM_ENABLED_ENV, "1")
    monkeypatch.setattr(wolfram_link, "_binary", lambda: "/usr/bin/wolframscript")
    assert wolfram_link.available() is True


def test_evaluate_degrades_cleanly_without_an_engine():
    # The absent-engine path must never raise: it is the normal case.
    out = wolfram_link.evaluate("1+1")
    assert out["unavailable"] is True
    assert out["ok"] is False
    assert out["result"] is None
    assert "reason" in out


def test_unavailable_reason_says_which_precondition_failed(monkeypatch):
    # "off" and "not installed" are different problems with different fixes, so
    # they must not render identically.
    off = wolfram_link.unavailable_response()["reason"]
    monkeypatch.setenv(wolfram_link.WOLFRAM_ENABLED_ENV, "1")
    monkeypatch.setattr(wolfram_link, "_binary", lambda: None)
    missing = wolfram_link.unavailable_response()["reason"]
    assert off != missing
    assert wolfram_link.WOLFRAM_ENABLED_ENV in off
    assert "PATH" in missing


def test_zero_exit_with_a_failure_marker_is_not_success(monkeypatch):
    # wolframscript reports evaluation failures in-band while exiting 0. Trusting
    # the exit code alone is exactly the trap the formal backends guard against.
    monkeypatch.setenv(wolfram_link.WOLFRAM_ENABLED_ENV, "1")
    monkeypatch.setattr(wolfram_link, "_binary", lambda: "/usr/bin/wolframscript")

    class _Completed:
        returncode = 0
        stdout = "$Failed"
        stderr = ""

    import subprocess

    monkeypatch.setattr(subprocess, "run", lambda *a, **k: _Completed())
    out = wolfram_link.evaluate("Integrate[BadInput]")
    assert out["ok"] is False
    assert out["unavailable"] is False
    assert "failure marker" in out["error"]


def test_run_available_op_reports_the_probe():
    out = wolfram_link.run({"op": "available"})
    assert out["ok"] is True
    assert out["available"] is False
    assert out["reason"]


def test_run_evaluate_marks_the_result_untrusted():
    # Nothing that comes back from Wolfram may be read as checked.
    out = wolfram_link.run({"op": "evaluate", "code": "1+1"})
    assert out["trusted"] is False


def test_run_rejects_an_empty_code_string():
    out = wolfram_link.run({"op": "evaluate", "code": "  "})
    assert out["ok"] is False


def test_run_rejects_an_unknown_op():
    assert wolfram_link.run({"op": "nope"})["ok"] is False
