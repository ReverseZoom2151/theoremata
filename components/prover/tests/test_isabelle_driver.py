"""Offline (mock-mode) tests for the Isabelle interaction-driver client.

No Isabelle server is reachable on the box, so tests exercise the deterministic
mock path. The key behavioural difference from Rocq: ``step_tactic`` is
unsupported (whole-theory granularity), only ``submit_unit`` is meaningful.
"""
from __future__ import annotations

import pytest

from theoremata_tools import isabelle_driver as isa


@pytest.fixture(autouse=True)
def _force_mock(monkeypatch):
    monkeypatch.setenv(isa.MOCK_ENV, "1")
    monkeypatch.delenv(isa.SERVER_ENV, raising=False)
    monkeypatch.delenv(isa.PASSWORD_ENV, raising=False)


# --------------------------------------------------------------------------- #
# Detection / graceful fallback.
# --------------------------------------------------------------------------- #
def test_detect_defaults_to_mock():
    mode, address, password = isa.detect_backend()
    assert mode == "mock"
    assert address is None
    assert password is None


def test_detect_needs_both_server_and_password(monkeypatch):
    monkeypatch.delenv(isa.MOCK_ENV, raising=False)
    monkeypatch.setenv(isa.SERVER_ENV, "127.0.0.1:4711")
    # No password -> still mock.
    mode, _addr, _pw = isa.detect_backend()
    assert mode == "mock"


def test_start_reports_mock_mode():
    out = isa.start()
    assert out["ok"] is True
    session = out["session"]
    assert session["mode"] == "mock"
    assert session["backend"] == "isabelle-server"
    assert session["logic"] == "HOL"


# --------------------------------------------------------------------------- #
# submit_unit: proved for a trivial theory; tainted by sorry/oops.
# --------------------------------------------------------------------------- #
def test_submit_unit_proved_for_trivial_theory():
    session = isa.start()["session"]
    thy = (
        "theory Scratch\n  imports Main\nbegin\n"
        'lemma my_goal: "True" by auto\nend\n'
    )
    out = isa.submit_unit(session, thy)
    assert out["ok"] is True
    assert out["proved"] is True
    assert out["errors"] == []
    assert out["mode"] == "mock"


def test_submit_unit_sorry_is_not_proved():
    session = isa.start()["session"]
    thy = (
        "theory Scratch\n  imports Main\nbegin\n"
        'lemma my_goal: "P" sorry\nend\n'
    )
    out = isa.submit_unit(session, thy)
    assert out["proved"] is False


def test_submit_unit_sorry_substring_no_false_positive():
    """A word merely containing 'sorry' must not taint the proof."""
    session = isa.start()["session"]
    thy = 'lemma x: "sorryish_lemma = True" by simp'
    out = isa.submit_unit(session, thy)
    assert out["proved"] is True


# --------------------------------------------------------------------------- #
# step_tactic: unsupported at theory granularity.
# --------------------------------------------------------------------------- #
def test_step_tactic_unsupported():
    session = isa.start()["session"]
    out = isa.step_tactic(session, None, "auto")
    assert out["ok"] is False
    assert out["error"] == "unsupported: theory-file granularity"


# --------------------------------------------------------------------------- #
# goal_state / stop shape.
# --------------------------------------------------------------------------- #
def test_goal_state_empty_shape():
    session = isa.start()["session"]
    out = isa.goal_state(session, None)
    assert out["ok"] is True
    assert out["goals"] == []


def test_stop_idempotent():
    session = isa.start()["session"]
    assert isa.stop(session)["ok"] is True
    assert isa.stop(session)["ok"] is True


# --------------------------------------------------------------------------- #
# Worker-style dispatch surface.
# --------------------------------------------------------------------------- #
def test_run_dispatch_cycle():
    started = isa.run({"op": "start"})
    session = started["session"]
    out = isa.run({"op": "submit_unit", "session": session,
                   "code": 'lemma t: "True" by auto'})
    assert out["proved"] is True
    step = isa.run({"op": "step_tactic", "session": session, "tactic": "auto"})
    assert step["ok"] is False
    assert isa.run({"op": "stop", "session": session})["ok"] is True


def test_run_detect_op():
    out = isa.run({"op": "detect"})
    assert out["mode"] == "mock"
    assert out["backend"] == "isabelle-server"


def test_run_missing_session():
    out = isa.run({"op": "submit_unit", "code": "x"})
    assert out["ok"] is False
