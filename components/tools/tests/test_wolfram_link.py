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
    monkeypatch.delenv(wolfram_link.CLOUD_KEY_ENV, raising=False)


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


# --- cloud transport -------------------------------------------------------


def test_local_binary_wins_over_a_cloud_key(monkeypatch):
    # A local kernel keeps the expression on the machine. An oracle we already do
    # not trust is still better run without shipping the goal to a third party.
    monkeypatch.setenv(wolfram_link.WOLFRAM_ENABLED_ENV, "1")
    monkeypatch.setenv(wolfram_link.CLOUD_KEY_ENV, "k")
    monkeypatch.setattr(wolfram_link, "_binary", lambda: "/usr/bin/wolframscript")
    assert wolfram_link.transport() == "local"


def test_cloud_key_alone_is_a_usable_transport(monkeypatch):
    monkeypatch.setenv(wolfram_link.WOLFRAM_ENABLED_ENV, "1")
    monkeypatch.setenv(wolfram_link.CLOUD_KEY_ENV, "k")
    monkeypatch.setattr(wolfram_link, "_binary", lambda: None)
    assert wolfram_link.transport() == "cloud"
    assert wolfram_link.available() is True


def test_no_transport_without_binary_or_key(monkeypatch):
    monkeypatch.setenv(wolfram_link.WOLFRAM_ENABLED_ENV, "1")
    monkeypatch.setattr(wolfram_link, "_binary", lambda: None)
    assert wolfram_link.transport() is None
    assert wolfram_link.available() is False


def _cloud(monkeypatch, payload):
    import json as _json
    monkeypatch.setenv(wolfram_link.WOLFRAM_ENABLED_ENV, "1")
    monkeypatch.setenv(wolfram_link.CLOUD_KEY_ENV, "k")
    monkeypatch.setattr(wolfram_link, "_binary", lambda: None)

    class _Resp:
        def read(self):
            return _json.dumps(payload).encode()
        def __enter__(self):
            return self
        def __exit__(self, *a):
            return False

    monkeypatch.setattr(wolfram_link.urllib.request, "urlopen", lambda *a, **k: _Resp())


def test_cloud_success_reports_its_transport(monkeypatch):
    _cloud(monkeypatch, {"success": True, "result": "4", "code": 200})
    out = wolfram_link.evaluate("2+2")
    assert out["ok"] is True
    assert out["result"] == "4"
    assert out["transport"] == "cloud"


def test_cloud_elided_output_is_refused(monkeypatch):
    # A truncated expression parses as a valid SHORTER one rather than as an
    # error, so elision has to be caught here or a clipped certificate looks fine.
    _cloud(monkeypatch, {"success": True, "result": "{1, 2, << 40 >>, 99}", "code": 200})
    out = wolfram_link.evaluate("Range[100]")
    assert out["ok"] is False
    assert "elided" in out["error"]


def test_cloud_in_band_failure_marker_is_caught(monkeypatch):
    # success:true at the protocol level is not proof the evaluation worked, the
    # same trap as exit 0 locally.
    _cloud(monkeypatch, {"success": True, "result": "$Failed", "code": 200})
    out = wolfram_link.evaluate("Integrate[Bad]")
    assert out["ok"] is False
    assert "failure marker" in out["error"]


def test_cloud_reported_failure_is_not_a_result(monkeypatch):
    _cloud(monkeypatch, {"success": False, "result": None, "code": 501})
    out = wolfram_link.evaluate("???")
    assert out["ok"] is False
    assert out["unavailable"] is False
