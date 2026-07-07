"""Offline tests for the unified hammer adapters (mock mode).

All tests run without any live toolchain: the default mode is mock whenever the
tool is absent (which it is in this environment), and we also force ``mode="mock"``
explicitly to be robust even if a driver happens to be on ``PATH``.
"""
from __future__ import annotations

import os
import re

import pytest

from theoremata_tools import hammer


TRIVIAL = "1 + 1 = 2"
NONSENSE = "False"


# --------------------------------------------------------------------------- #
# System normalization.
# --------------------------------------------------------------------------- #
def test_normalize_system_aliases():
    assert hammer.normalize_system("coq") == "rocq"
    assert hammer.normalize_system("HOL") == "isabelle"
    assert hammer.normalize_system("lean4") == "lean"


def test_normalize_system_unknown_raises():
    with pytest.raises(ValueError):
        hammer.normalize_system("mizar")


# --------------------------------------------------------------------------- #
# Per-system: reconstruction for a trivial goal, failure for an unprovable one.
# --------------------------------------------------------------------------- #
@pytest.mark.parametrize(
    "system,tool,tactic",
    [
        ("isabelle", "sledgehammer", "by (metis)"),
        ("rocq", "coqhammer:sauto", "sauto"),
        ("lean", "aesop", "aesop"),
    ],
)
def test_trivial_goal_returns_reconstruction(system, tool, tactic):
    out = hammer.run_hammer(system, TRIVIAL, mode="mock")
    assert out["ok"] is True
    assert out["system"] == system
    assert out["tool"] == tool
    assert out["success"] is True
    assert out["reconstructed_tactic"] == tactic
    assert out["kernel_checked"] is True
    assert out["mode"] == "mock"


@pytest.mark.parametrize("system", ["isabelle", "rocq", "lean"])
def test_unprovable_goal_fails(system):
    out = hammer.run_hammer(system, NONSENSE, mode="mock")
    assert out["ok"] is True
    assert out["success"] is False
    assert out["reconstructed_tactic"] is None
    # kernel_checked stays True: a reconstruction (if any) is always checked.
    assert out["kernel_checked"] is True
    assert out["mode"] == "mock"


@pytest.mark.parametrize("system", ["isabelle", "rocq", "lean"])
def test_kernel_checked_always_true(system):
    for goal in (TRIVIAL, NONSENSE, ""):
        assert hammer.run_hammer(system, goal, mode="mock")["kernel_checked"] is True


# --------------------------------------------------------------------------- #
# Rocq: pure tier (no ATP) vs full-ATP tier split.
# --------------------------------------------------------------------------- #
def test_rocq_pure_tier_has_no_provers():
    out = hammer.run_hammer("rocq", TRIVIAL, mode="mock")  # default tier=pure
    assert out["tier"] == "pure"
    assert out["provers_tried"] == []
    assert out["tool"] == "coqhammer:sauto"
    assert out["reconstructed_tactic"] == "sauto"


def test_rocq_full_tier_fires_atps():
    out = hammer.run_hammer(
        "rocq", TRIVIAL, mode="mock", context={"tier": "full"}
    )
    assert out["tier"] == "full"
    assert out["provers_tried"] == hammer._ROCQ_ATP_PROVERS
    assert out["tool"] == "coqhammer:hammer"
    assert out["reconstructed_tactic"] == "hauto"
    assert out["success"] is True


def test_rocq_best_option_pure_tier():
    out = hammer.run_hammer(
        "rocq", TRIVIAL, mode="mock", context={"best": True}
    )
    assert out["reconstructed_tactic"] == "best"


# --------------------------------------------------------------------------- #
# Isabelle: prover battery + falsifier message on failure.
# --------------------------------------------------------------------------- #
def test_isabelle_prover_battery():
    out = hammer.run_hammer("isabelle", TRIVIAL, mode="mock")
    assert out["provers_tried"] == hammer._ISABELLE_PROVERS


def test_isabelle_failure_mentions_falsifier():
    out = hammer.run_hammer("isabelle", NONSENSE, mode="mock")
    assert "nitpick" in out["message"] or "quickcheck" in out["message"]


# --------------------------------------------------------------------------- #
# Lean: white-box (no external provers), notes Duper/LeanHammer.
# --------------------------------------------------------------------------- #
def test_lean_is_white_box_and_notes_external_atp():
    out = hammer.run_hammer("lean", TRIVIAL, mode="mock")
    assert out["provers_tried"] == []
    assert "Duper" in out["message"] or "LeanHammer" in out["message"]


# --------------------------------------------------------------------------- #
# Mock provability heuristic + explicit override.
# --------------------------------------------------------------------------- #
def test_context_provable_override_forces_success():
    out = hammer.run_hammer(
        "lean", NONSENSE, mode="mock", context={"provable": True}
    )
    assert out["success"] is True
    assert out["reconstructed_tactic"] == "aesop"


def test_context_provable_override_forces_failure():
    out = hammer.run_hammer(
        "lean", TRIVIAL, mode="mock", context={"provable": False}
    )
    assert out["success"] is False


def test_empty_goal_is_unprovable():
    assert hammer.run_hammer("rocq", "", mode="mock")["success"] is False


def test_state_dict_goal_extraction():
    state = {"goals": ["⊢ 1 + 1 = 2"], "hyps": []}
    out = hammer.run_hammer("isabelle", state, mode="mock")
    assert out["success"] is True


# --------------------------------------------------------------------------- #
# Mode resolution + graceful fallback.
# --------------------------------------------------------------------------- #
def test_auto_mode_falls_back_to_mock_offline(monkeypatch):
    # Ensure no live command is discoverable, and no env override is set.
    monkeypatch.setattr(hammer, "_command_for", lambda system: None)
    monkeypatch.delenv("THEOREMATA_HAMMER_MODE", raising=False)
    out = hammer.run_hammer("lean", TRIVIAL)  # mode=None -> auto
    assert out["mode"] == "mock"
    assert out["requested_mode"] is None


def test_forced_real_falls_back_to_mock_when_unavailable(monkeypatch):
    monkeypatch.setattr(hammer, "_command_for", lambda system: None)
    out = hammer.run_hammer("rocq", TRIVIAL, mode="real")
    # Real backend raised HammerUnavailable -> mock fallback, noted in message.
    assert out["mode"] == "mock"
    assert out["requested_mode"] == "real"
    assert "real mode unavailable" in out["message"]
    assert out["success"] is True  # trivial goal still reconstructs in mock


def test_env_var_forces_mock(monkeypatch):
    monkeypatch.setenv("THEOREMATA_HAMMER_MODE", "mock")
    # Even if a command were available, env forces mock.
    monkeypatch.setattr(hammer, "_command_for", lambda system: "fake-binary")
    out = hammer.run_hammer("isabelle", TRIVIAL)
    assert out["mode"] == "mock"


def test_tool_available_probe(monkeypatch):
    monkeypatch.setattr(hammer, "_command_for", lambda system: "isabelle")
    assert hammer.tool_available("isabelle") is True
    monkeypatch.setattr(hammer, "_command_for", lambda system: None)
    assert hammer.tool_available("isabelle") is False


# --------------------------------------------------------------------------- #
# Worker-style dispatch surface (tool == "hammer").
# --------------------------------------------------------------------------- #
def test_run_dispatch_reads_request():
    out = hammer.run(
        {"tool": "hammer", "system": "rocq", "goal": TRIVIAL, "mode": "mock"}
    )
    assert out["system"] == "rocq"
    assert out["success"] is True


def test_run_dispatch_requires_system():
    with pytest.raises(ValueError):
        hammer.run({"tool": "hammer", "goal": TRIVIAL})


def test_run_dispatch_passes_context():
    out = hammer.run(
        {
            "tool": "hammer",
            "system": "rocq",
            "goal": TRIVIAL,
            "mode": "mock",
            "context": {"tier": "full"},
        }
    )
    assert out["tier"] == "full"
    assert out["provers_tried"] == hammer._ROCQ_ATP_PROVERS


# --------------------------------------------------------------------------- #
# Sledgehammer output parsing (offline, deterministic).
# --------------------------------------------------------------------------- #
def test_parse_sledgehammer_strips_millisecond_timing():
    out = "Sledgehammering...\ne: Try this: by auto (0.3 ms)\n"
    assert hammer._parse_sledgehammer(out) == "by auto"


def test_parse_sledgehammer_using_form():
    out = "e: Try this: using one_add_one by blast (0.4 ms)\n"
    assert hammer._parse_sledgehammer(out) == "using one_add_one by blast"


def test_parse_sledgehammer_metis_and_seconds():
    out = "vampire: Try this: by (metis add.commute one_add_one) (1.2 s)\n"
    assert hammer._parse_sledgehammer(out) == "by (metis add.commute one_add_one)"


def test_parse_sledgehammer_no_reconstruction():
    assert hammer._parse_sledgehammer("e found a proof...\nDuplicate proof\n") is None


def test_parse_sledgehammer_takes_first():
    out = (
        "e: Try this: by simp (1 ms)\n"
        "e: Try this: by linarith (11 ms)\n"
    )
    assert hammer._parse_sledgehammer(out) == "by simp"


# --------------------------------------------------------------------------- #
# WSL path translation (Windows-only; the real toolchains live in WSL Ubuntu).
# --------------------------------------------------------------------------- #
@pytest.mark.skipif(os.name != "nt", reason="Windows->WSL path form")
def test_win_to_wsl_path():
    assert hammer._win_to_wsl_path(r"C:\Users\x\Scratch.thy") == (
        "/mnt/c/Users/x/Scratch.thy"
    )


# --------------------------------------------------------------------------- #
# Real-mode gating: rocq (CoqHammer absent) and lean (no aesop project) must
# degrade to mock with a clear, actionable note -- never raise.
# --------------------------------------------------------------------------- #
def test_rocq_real_notes_missing_coqhammer(monkeypatch):
    # A coqc exists, but the CoqHammer plugin does not -> mock + opam hint.
    monkeypatch.setattr(
        hammer, "_command_for", lambda s: "coqc" if s == "rocq" else None
    )
    monkeypatch.setattr(
        hammer, "_coqhammer_plugin_available", lambda cmd, full: False
    )
    out = hammer.run_hammer("rocq", TRIVIAL, mode="real")
    assert out["mode"] == "mock"
    assert out["requested_mode"] == "real"
    assert "opam install coq-hammer" in out["message"]
    assert out["tier"] == "pure"
    assert out["success"] is True  # trivial goal still reconstructs in mock


def test_lean_real_gated_without_project(monkeypatch):
    monkeypatch.setattr(
        hammer, "_command_for", lambda s: "lean" if s == "lean" else None
    )
    monkeypatch.delenv("THEOREMATA_LEAN_PROJECT", raising=False)
    out = hammer.run_hammer("lean", TRIVIAL, mode="real")
    assert out["mode"] == "mock"
    assert "real mode unavailable" in out["message"]
    assert "THEOREMATA_LEAN_PROJECT" in out["message"] or "aesop" in out["message"]


# --------------------------------------------------------------------------- #
# LIVE Sledgehammer: runs only when Isabelle is probeable (else skipped). Asserts
# a kernel-checked `by (...)`-style reconstruction comes back for a trivial goal.
# --------------------------------------------------------------------------- #
@pytest.mark.skipif(
    not hammer.tool_available("isabelle"),
    reason="Isabelle/Sledgehammer not available on this machine",
)
def test_live_sledgehammer_reconstructs_trivial_goal():
    out = hammer.run_hammer(
        "isabelle",
        "1 + 1 = (2::nat)",
        mode="real",
        timeout=30,
        context={"provers": ["e"]},  # E is bundled with Isabelle2025-2
    )
    assert out["ok"] is True
    assert out["mode"] == "live"
    assert out["success"] is True
    assert out["kernel_checked"] is True
    tactic = out["reconstructed_tactic"]
    assert tactic and re.search(r"\bby\b", tactic), tactic
