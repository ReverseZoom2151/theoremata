"""Tests for the statement-level mutation check.

Two tiers, on purpose. The pure tier exercises parsing, scope tracking, plan
refusal and mutant rendering with no Lean at all, so CI still runs something
real on a box with no toolchain. The live tier drives the actual elaborator and
skips, never fails, when Lean or the third-party corpus is absent.
"""

from __future__ import annotations

import os
import re
from pathlib import Path

import pytest

from theoremata_tools.statement_triviality import (
    SENTINELS,
    VERDICT_NOT_SHOWN_TRIVIAL,
    VERDICT_TRIVIAL,
    VERDICT_WITHHELD,
    check_statement_triviality,
    lean_available,
    plan_mutation,
    render_mutant,
)

REPO_ROOT = Path(__file__).resolve().parents[3]
FIXTURES = REPO_ROOT / "components" / "eval" / "fixtures" / "trivial_existential"
PROBE = FIXTURES / "probe.lean"
CONTROL = FIXTURES / "control.lean"
MAXWELL = (
    REPO_ROOT
    / "resources"
    / "MaxwellEquations-main"
    / "MaxwellEquations-main"
    / "proofs"
    / "maxwell_1d.lean"
)
MATHLIB = REPO_ROOT / "resources" / "mathlib4-master" / "mathlib4-master"


# ---------------------------------------------------------------------------
# Pure tier: no Lean process is started anywhere below this line.
# ---------------------------------------------------------------------------

TOY = """\
structure Box where
  lo : Int
  hi : Int

/-- doc -/
def mk (n : Int) : Box :=
  { lo := n, hi := n + 1 }

theorem mkIsOrdered (n : Int) :
    ∃ x : Int, x = (mk n).lo :=
  ⟨(mk n).lo, rfl⟩

theorem later (n : Int) : (mk n).lo = n := rfl
"""


def test_plan_finds_the_definition_the_statement_mentions():
    plan = plan_mutation(TOY, "mkIsOrdered")
    assert plan["verdict"] is None
    assert [d["name"] for d in plan["mutated_defs"]] == ["mk"]
    assert plan["mutated_defs"][0]["fields"] == [("lo", "Int"), ("hi", "Int")]


def test_render_replaces_body_keeps_signature_and_truncates():
    plan = plan_mutation(TOY, "mkIsOrdered")
    out = render_mutant(TOY, plan, 7, with_proof=True)
    # Signature preserved byte for byte, which is what keeps the mutant well
    # typed whenever the original was.
    assert "def mk (n : Int) : Box :=" in out
    # One distinct constant per field slot. This previously asserted the same
    # value for both, which was the co-mutation false-positive documented in
    # test_each_mutated_slot_gets_a_distinct_constant.
    assert "{ lo := (7 : Int), hi := (8 : Int) }" in out
    assert "n + 1" not in out
    # Declarations after the target are dropped so an unrelated later breakage
    # cannot masquerade as evidence.
    assert "theorem later" not in out


def test_render_stage_a_replaces_the_proof_with_sorry():
    plan = plan_mutation(TOY, "mkIsOrdered")
    out = render_mutant(TOY, plan, 7, with_proof=False)
    assert "sorry" in out
    assert "⟨(mk n).lo, rfl⟩" not in out


def test_the_two_sentinels_produce_different_mutants():
    plan = plan_mutation(TOY, "mkIsOrdered")
    a = render_mutant(TOY, plan, SENTINELS[0])
    b = render_mutant(TOY, plan, SENTINELS[1])
    assert a != b


def test_open_namespace_is_reclosed_after_truncation():
    src = "namespace N\n\n" + TOY + "\nend N\n"
    plan = plan_mutation(src, "mkIsOrdered")
    assert plan["trailing_scopes"] == ["N"]
    assert render_mutant(src, plan, 7).rstrip().endswith("end N")


@pytest.mark.parametrize(
    "source, name",
    [
        # No such theorem.
        (TOY, "doesNotExist"),
        # Statement names no same-file definition.
        ("theorem t (n : Int) : n = n := rfl\n", "t"),
    ],
)
def test_withholds_rather_than_accusing(source, name):
    assert plan_mutation(source, name)["verdict"] == VERDICT_WITHHELD


def test_withholds_when_the_proof_has_a_hole():
    src = TOY.replace("⟨(mk n).lo, rfl⟩", "by sorry")
    out = plan_mutation(src, "mkIsOrdered")
    assert out["verdict"] == VERDICT_WITHHELD
    assert "hole" in out["reason"]


def test_withholds_when_a_referenced_definition_is_out_of_shape():
    # A dependent binder with nested parentheses leaves the return-type colon
    # ambiguous, so the definition falls outside the covered class.
    src = TOY.replace("def mk (n : Int) : Box :=", "def mk (n : (Int)) : Box :=")
    out = plan_mutation(src, "mkIsOrdered")
    assert out["verdict"] == VERDICT_WITHHELD
    assert "covered def shape" in out["reason"]


def test_withholds_when_the_return_structure_has_a_non_literal_field():
    src = TOY.replace("  hi : Int", "  hi : List Int")
    out = plan_mutation(src, "mkIsOrdered")
    assert out["verdict"] == VERDICT_WITHHELD
    assert "out of scope" in out["reason"]


def test_withholds_when_only_some_referenced_definitions_are_mutable():
    # `other` is referenced by the statement but returns a type we cannot make
    # a constant of. Mutating only `mk` could indict a theorem that genuinely
    # constrains `other`, so the whole check withholds.
    src = TOY.replace(
        "theorem mkIsOrdered (n : Int) :\n    ∃ x : Int, x = (mk n).lo :=\n  ⟨(mk n).lo, rfl⟩",
        "def other (n : Int) : Int := n\n\n"
        "theorem mkIsOrdered (n : Int) :\n"
        "    ∃ x : Int, x = (mk n).lo + other n :=\n"
        "  ⟨(mk n).lo + other n, rfl⟩",
    )
    out = plan_mutation(src, "mkIsOrdered")
    assert out["verdict"] == VERDICT_WITHHELD


def test_prose_in_block_comments_is_not_mistaken_for_a_declaration():
    # Both shipped fixtures open with a long block comment that discusses
    # theorems in prose. Slicing must ignore it.
    src = "/-\ntheorem notReal : True := trivial\ndef notReal2 : Nat := 0\n-/\n" + TOY
    plan = plan_mutation(src, "mkIsOrdered")
    assert plan["verdict"] is None
    assert [d["name"] for d in plan["mutated_defs"]] == ["mk"]


def test_no_verdict_asserts_the_statement_is_good():
    # The vocabulary itself is the guarantee: the check can accuse or be silent,
    # never bless.
    verdicts = {VERDICT_TRIVIAL, VERDICT_NOT_SHOWN_TRIVIAL, VERDICT_WITHHELD}
    assert verdicts == {"trivial", "not_shown_trivial", "withheld"}


def test_planning_the_shipped_fixtures_needs_no_lean():
    for path in (PROBE, CONTROL):
        plan = plan_mutation(path.read_text(encoding="utf-8"), "spectrumIsOrdered")
        assert plan["verdict"] is None, path
        assert [d["name"] for d in plan["mutated_defs"]] == ["spectrum"]


def test_missing_source_file_withholds(tmp_path):
    out = check_statement_triviality(str(tmp_path / "nope.lean"), "whatever")
    assert out["verdict"] == VERDICT_WITHHELD


# ---------------------------------------------------------------------------
# Live tier: skips honestly when the toolchain or the corpus is absent.
# ---------------------------------------------------------------------------

requires_lean = pytest.mark.skipif(
    not lean_available(), reason="no Lean toolchain on PATH"
)


@requires_lean
def test_probe_is_flagged_trivial(tmp_path):
    out = check_statement_triviality(
        str(PROBE), "spectrumIsOrdered", work_dir=str(tmp_path), timeout=300.0
    )
    assert out["verdict"] == VERDICT_TRIVIAL, out
    assert out["mutated_defs"] == ["spectrum"]
    # Evidence, not inference: every stage really compiled.
    assert all(s["ok"] for s in out["stages"])


@requires_lean
def test_control_is_not_flagged(tmp_path):
    out = check_statement_triviality(
        str(CONTROL), "spectrumIsOrdered", work_dir=str(tmp_path), timeout=300.0
    )
    assert out["verdict"] != VERDICT_TRIVIAL, out
    assert out["verdict"] == VERDICT_NOT_SHOWN_TRIVIAL
    # The mutated statement did elaborate; it was the proof that stopped
    # working. That ordering is what makes the negative result meaningful.
    stage_a = [s for s in out["stages"] if s["stage"] == "A"]
    stage_b = [s for s in out["stages"] if s["stage"] == "B"]
    assert stage_a and all(s["ok"] for s in stage_a)
    assert stage_b and not stage_b[-1]["ok"]


@requires_lean
def test_baseline_failure_withholds(tmp_path):
    # An honest theorem in a file that does not compile must never be accused.
    broken = tmp_path / "broken.lean"
    # The breakage has to precede the target: everything after it is truncated
    # away by design, so a later error is correctly irrelevant.
    src = TOY.replace("theorem mkIsOrdered", "def bad : Int := notAThing\n\ntheorem mkIsOrdered")
    broken.write_text(src, encoding="utf-8")
    out = check_statement_triviality(
        str(broken), "mkIsOrdered", work_dir=str(tmp_path / "w"), timeout=300.0
    )
    assert out["verdict"] == VERDICT_WITHHELD, out
    assert "baseline" in out["reason"]


@requires_lean
def test_breakage_after_the_target_is_truncated_away(tmp_path):
    # The converse of the baseline test: an error in a LATER declaration must
    # not suppress a real finding, because the mutant stops at the target.
    later_broken = tmp_path / "later.lean"
    later_broken.write_text(TOY + "\ndef bad : Int := notAThing\n", encoding="utf-8")
    out = check_statement_triviality(
        str(later_broken), "mkIsOrdered", work_dir=str(tmp_path / "w2"), timeout=300.0
    )
    assert out["verdict"] == VERDICT_TRIVIAL, out


@pytest.mark.skipif(not MAXWELL.is_file(), reason="MaxwellEquations corpus absent")
def test_maxwell_plan_is_inside_the_covered_class():
    # Parsing only. The corpus is gitignored, so this skips in CI, and it never
    # executes corpus content.
    plan = plan_mutation(MAXWELL.read_text(encoding="utf-8"), "xHyperbolicity")
    assert plan["verdict"] is None, plan
    assert [d["name"] for d in plan["mutated_defs"]] == ["xFluxJacobianEigenExprs"]
    assert plan["trailing_scopes"] == ["maxwell_1d"]


@requires_lean
@pytest.mark.skipif(
    not os.environ.get("THEOREMATA_SLOW_LEAN")
    or not MAXWELL.is_file()
    or not (MATHLIB / ".lake" / "build" / "lib" / "lean" / "Mathlib.olean").is_file(),
    # Opt-in: this one imports all of Mathlib, so it is minutes, not seconds.
    reason="set THEOREMATA_SLOW_LEAN=1 and provide the corpus plus a built Mathlib",
)
def test_maxwell_xhyperbolicity(tmp_path):
    out = check_statement_triviality(
        str(MAXWELL),
        "xHyperbolicity",
        work_dir=str(tmp_path),
        lake_workspace=str(MATHLIB),
        timeout=1800.0,
    )
    # Whatever Mathlib does here, the one thing that must hold is that a
    # failure to compile never becomes an accusation.
    assert out["verdict"] in {VERDICT_TRIVIAL, VERDICT_NOT_SHOWN_TRIVIAL, VERDICT_WITHHELD}
    if any(s.get("stage") == "baseline" and not s["ok"] for s in out.get("stages", [])):
        assert out["verdict"] == VERDICT_WITHHELD


# --------------------------------------------------------------------------- #
# Co-mutation regression
# --------------------------------------------------------------------------- #

_TWO_DEF_SOURCE = """\
structure Speeds where
  lo : Int
  hi : Int

def leftSpeeds (n : Int) : Speeds := { lo := n, hi := n + 1 }

def rightSpeeds (n : Int) : Speeds := { lo := n + 2, hi := n + 3 }

theorem speedsRelated (n : Int) :
    (leftSpeeds n).lo <= (rightSpeeds n).hi := by
  simp [leftSpeeds, rightSpeeds]
  omega
"""


def test_each_mutated_slot_gets_a_distinct_constant():
    """Co-mutation false positive, pinned.

    Giving every mutated definition the SAME constant made a relational
    statement hold reflexively, so a substantive theorem was reported trivial.
    It was found on a real corpus theorem (maxwell_1d.xWaveStability, a
    `|mu| >= |lambda|` relation over two definitions) which the checker accused
    before this. Two mutually distinct sentinel RUNS do not defend against it,
    because within each run both sides still move together.

    Distinctness cannot hide a real triviality: a genuinely trivial statement
    is true for any values, so it stays trivial when the values differ.
    """
    plan = plan_mutation(_TWO_DEF_SOURCE, "speedsRelated")
    assert plan["ok"], plan
    assert len(plan["mutated_defs"]) == 2

    for sentinel in SENTINELS:
        mutant = render_mutant(_TWO_DEF_SOURCE, plan, sentinel, with_proof=True)
        constants = re.findall(r":=\s*\((\d+)\s*:", mutant)
        assert len(constants) == 4, mutant
        assert len(set(constants)) == 4, (
            f"every slot must differ, got {constants}; equal constants make a "
            "relational statement hold reflexively"
        )
