"""Offline (mock-mode) tests for the Rocq interaction-driver client.

No Rocq toolchain is present on the box, so every test exercises the
deterministic mock path (the graceful default when no ``pet``/``sertop`` binary
is discoverable).
"""
from __future__ import annotations

import pytest

from theoremata_tools import rocq_driver as rd


@pytest.fixture(autouse=True)
def _force_mock(monkeypatch):
    """Ensure mock mode regardless of any ambient toolchain/env."""
    monkeypatch.setenv(rd.MOCK_ENV, "1")
    monkeypatch.delenv(rd.PET_ENV, raising=False)
    monkeypatch.delenv(rd.SERTOP_ENV, raising=False)


# --------------------------------------------------------------------------- #
# Detection / graceful fallback.
# --------------------------------------------------------------------------- #
def test_detect_defaults_to_mock():
    mode, backend, binary = rd.detect_backend()
    assert mode == "mock"
    assert backend == "mock"
    assert binary is None


def test_start_reports_mock_mode():
    out = rd.start({"root": "/tmp/gen"})
    assert out["ok"] is True
    session = out["session"]
    assert session["mode"] == "mock"
    assert session["backend"] == "mock"
    assert isinstance(session["id"], str) and session["id"]


# --------------------------------------------------------------------------- #
# submit_unit: proved for a trivially-closing unit.
# --------------------------------------------------------------------------- #
def test_submit_unit_proved_for_trivial_unit():
    session = rd.start()["session"]
    code = "Theorem t : True. Proof. exact I. Qed."
    out = rd.submit_unit(session, code)
    assert out["ok"] is True
    assert out["proved"] is True
    assert out["goals"] == []
    assert out["errors"] == []
    assert out["mode"] == "mock"


def test_submit_unit_not_proved_without_closing_tactic():
    session = rd.start()["session"]
    out = rd.submit_unit(session, "Theorem t : P. Proof.")
    assert out["proved"] is False
    assert out["goals"] == [{"hyps": [], "concl": "True"}]


# --------------------------------------------------------------------------- #
# step_tactic: advances state; closing tactic finishes the proof.
# --------------------------------------------------------------------------- #
def test_step_tactic_advances_state():
    session = rd.start()["session"]
    out = rd.step_tactic(session, 0, "intro n")
    assert out["ok"] is True
    assert out["state"] == 1
    assert out["proof_finished"] is False
    assert out["goals"] == [{"hyps": [], "concl": "True"}]


def test_step_tactic_closing_tactic_finishes():
    session = rd.start()["session"]
    out = rd.step_tactic(session, 3, "exact I.")
    assert out["state"] == 4
    assert out["proof_finished"] is True
    assert out["goals"] == []


# --------------------------------------------------------------------------- #
# goal_state: {goals:[{hyps, concl}]} shape.
# --------------------------------------------------------------------------- #
def test_goal_state_shape():
    session = rd.start()["session"]
    out = rd.goal_state(session, 1)
    assert out["ok"] is True
    assert isinstance(out["goals"], list) and out["goals"]
    goal = out["goals"][0]
    assert set(goal) == {"hyps", "concl"}
    assert isinstance(goal["hyps"], list)
    assert isinstance(goal["concl"], str)


def test_stop_is_idempotent():
    session = rd.start()["session"]
    assert rd.stop(session)["ok"] is True
    # Stopping an unknown/already-stopped session still succeeds.
    assert rd.stop(session)["ok"] is True


# --------------------------------------------------------------------------- #
# Worker-style dispatch surface.
# --------------------------------------------------------------------------- #
def test_run_dispatch_full_cycle():
    started = rd.run({"op": "start"})
    session = started["session"]
    proved = rd.run({"op": "submit_unit", "session": session,
                     "code": "Lemma l : True. Proof. trivial. Qed."})
    assert proved["proved"] is True
    stepped = rd.run({"op": "step_tactic", "session": session,
                      "state": 0, "tactic": "trivial"})
    assert stepped["proof_finished"] is True
    goals = rd.run({"op": "goal_state", "session": session, "state": 0})
    assert "goals" in goals
    assert rd.run({"op": "stop", "session": session})["ok"] is True


def test_run_detect_op():
    out = rd.run({"op": "detect"})
    assert out["mode"] == "mock"


def test_run_missing_session():
    out = rd.run({"op": "submit_unit", "code": "x"})
    assert out["ok"] is False


def test_run_unknown_op():
    out = rd.run({"op": "bogus", "session": {"id": "x", "mode": "mock"}})
    assert out["ok"] is False
