"""Tests for the `#print axioms` authoritative soundness gate.

The Mathlib-root cases need the Lean toolchain plus the Mathlib olean cache; if
Lean is unavailable the whole module skips cleanly.
"""
from __future__ import annotations

import os

import pytest

from theoremata_tools.axioms import _parse_axioms, _resolve, check_axioms

MATHLIB_ROOT = os.path.join(
    os.path.dirname(os.path.dirname(os.path.dirname(__file__))),
    "resources",
    "mathlib4-master",
    "mathlib4-master",
)

_lean = _resolve("lean") or _resolve("lake")
requires_lean = pytest.mark.skipif(_lean is None, reason="Lean toolchain not available")
requires_mathlib = pytest.mark.skipif(
    _lean is None or not os.path.isdir(os.path.join(MATHLIB_ROOT, ".lake", "build")),
    reason="Mathlib build/cache not available",
)


def test_parse_bracket_form():
    text = "'seven_prime' depends on axioms: [propext, Classical.choice, Quot.sound]"
    assert _parse_axioms(text) == ["propext", "Classical.choice", "Quot.sound"]


def test_parse_no_axioms():
    assert _parse_axioms("'t' does not depend on any axioms") == []


def test_parse_missing_report():
    assert _parse_axioms("some unrelated error output") is None


@requires_lean
def test_sorry_is_unclean():
    result = check_axioms("theorem foo : 1 = 1 := by sorry", "foo", timeout=180.0)
    assert result["ok"] is True
    assert "sorryAx" in result["axioms"]
    assert "sorryAx" in result["disallowed"]
    assert result["clean"] is False


@requires_lean
def test_no_axioms_is_clean():
    result = check_axioms("theorem t : True := trivial", "t", timeout=180.0)
    assert result["ok"] is True
    assert result["axioms"] == []
    assert result["clean"] is True
    assert result["compiled"] is True


@requires_lean
def test_custom_axiom_is_disallowed():
    source = "axiom bad : False\ntheorem uses_bad : False := bad"
    result = check_axioms(source, "uses_bad", timeout=180.0)
    assert result["ok"] is True
    assert "bad" in result["axioms"]
    assert "bad" in result["disallowed"]
    assert result["clean"] is False


@requires_mathlib
def test_clean_mathlib_theorem():
    source = (
        "import Mathlib.Data.Nat.Prime.Basic\n"
        "theorem seven_prime : Nat.Prime 7 := by decide"
    )
    result = check_axioms(source, "seven_prime", root=MATHLIB_ROOT, timeout=600.0)
    assert result["ok"] is True
    assert result["compiled"] is True
    assert result["clean"] is True
    assert set(result["axioms"]) <= {"propext", "Classical.choice", "Quot.sound"}
    assert result["disallowed"] == []
