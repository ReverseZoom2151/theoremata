"""Tests for proof telemetry (tactic histogram + proven/unproven verdict)."""
from __future__ import annotations

from theoremata_tools.proof_telemetry import (
    analyze,
    has_admit,
    has_sorry,
    mask_lean,
    tactic_histogram,
    verdict,
)


def test_mask_preserves_length_and_newlines():
    src = 'a -- simp\nb /- rw -/ c\n"omega"\n'
    masked = mask_lean(src)
    assert len(masked) == len(src)
    assert masked.count("\n") == src.count("\n")


def test_histogram_counts_real_tactics():
    src = """
theorem t (n : Nat) : n + 0 = n := by
  intro h
  simp
  rw [Nat.add_zero]
  simp
  exact rfl
"""
    hist = tactic_histogram(src)
    assert hist["simp"] == 2
    assert hist["rw"] == 1
    assert hist["exact"] == 1
    assert hist["intro"] == 1


def test_tactics_in_comments_are_ignored():
    src = """
theorem t : True := by
  -- we could use simp or omega or linarith here
  /- a block comment mentioning decide and induction -/
  trivial
"""
    hist = tactic_histogram(src)
    assert "simp" not in hist
    assert "omega" not in hist
    assert "linarith" not in hist
    assert "decide" not in hist
    assert "induction" not in hist
    assert hist.get("trivial") == 1


def test_tactics_in_strings_are_ignored():
    src = 'theorem t : True := by\n  have s := "use simp and omega"\n  trivial\n'
    hist = tactic_histogram(src)
    assert "simp" not in hist
    assert "omega" not in hist
    assert hist.get("have") == 1
    assert hist.get("trivial") == 1


def test_identifier_boundaries_not_miscounted():
    # `simple`, `.simp`, and `decidedly` must not count as tactics.
    src = "theorem t : True := by\n  have simple := decidedly\n  trivial\n"
    hist = tactic_histogram(src)
    assert "simp" not in hist
    assert "decide" not in hist


def test_sorry_and_admit_detection():
    assert has_sorry("theorem t : True := by sorry") is True
    assert has_admit("theorem t : True := by admit") is True
    assert has_sorry("theorem t : True := trivial") is False
    # A `sorry` mentioned only in a comment is not a live placeholder.
    assert has_sorry("theorem t : True := by trivial -- no sorry here") is False


def test_verdict():
    assert verdict("theorem t : True := by simp") == "proven"
    assert verdict("theorem t : True := by sorry") == "unproven"
    assert verdict("theorem t : True := by admit") == "unproven"


def test_analyze_full_record_and_ranking():
    proven = analyze("theorem t : True := by trivial")
    assert proven["status"] == "proven"
    assert proven["has_sorry"] is False
    assert proven["ranking_score"] > 0.0
    assert proven["total_tactic_calls"] == 1

    unproven = analyze("theorem t : True := by sorry")
    assert unproven["status"] == "unproven"
    assert unproven["has_sorry"] is True
    assert unproven["ranking_score"] == 0.0

    # Shorter proven proof ranks above a longer proven proof.
    short = analyze("theorem t : True := by trivial")
    longer = analyze(
        "theorem t : Nat := by\n  simp\n  rw [x]\n  simp\n  omega\n  linarith\n  exact rfl"
    )
    assert short["ranking_score"] > longer["ranking_score"]


def test_native_decide_penalized():
    plain = analyze("theorem t : P := by decide")
    native = analyze("theorem t : P := by native_decide")
    assert native["uses_native_decide"] is True
    assert plain["uses_native_decide"] is False
    # native_decide is penalized relative to an equally short kernel decide proof.
    assert native["ranking_score"] < plain["ranking_score"]
