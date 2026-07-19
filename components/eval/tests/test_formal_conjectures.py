"""Tests for the ``formal_conjectures`` open-target loader.

Two halves, and both matter:

* **Corpus-independent.** Vocabulary and parser tests run on inline fixtures, so
  they assert real behaviour in CI where ``resources/`` is absent. These are the
  tests that pin the ``expect_open`` distinction.
* **Corpus-conditional.** When the vendored checkout is present, assert the loader
  emits only ``research open`` statements and never mislabels a ``research
  solved`` one -- the failure mode that would make the whole distinction a lie.

The absent-corpus path is the CI condition: ``resources/`` is gitignored.
"""
import os
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
_provider = Path(__file__).resolve().parents[2] / "provider" / "python"
if _provider.exists():
    sys.path.insert(0, str(_provider))

os.environ.setdefault("THEOREMATA_MODEL_MOCK", "1")

from theoremata_tools.benchmarks import list_benchmarks, load_benchmark  # noqa: E402
from theoremata_tools.benchmarks.adversarial import (  # noqa: E402
    EXPECT_ACCEPT,
    EXPECT_ACCEPT_CONDITIONAL,
    EXPECT_REJECT,
    EXPECTED_VERDICTS,
)
from theoremata_tools.benchmarks.formal_conjectures import (  # noqa: E402
    CATEGORY_OPEN,
    CATEGORY_SOLVED,
    CLAIMED_PROOF,
    EXPECT_OPEN,
    OPEN_TARGET_VERDICTS,
    TRACK,
    _ATTR_DECL,
    _ams,
    _category,
    _declaration_block,
    _source_area,
    _split_statement,
    load_formal_conjectures,
)
from theoremata_tools.benchmarks.resources import find_dir  # noqa: E402

_CORPUS_GLOB = "formal-conjectures-main"


def _corpus_present() -> bool:
    return find_dir(_CORPUS_GLOB, f"{_CORPUS_GLOB}/**") is not None


# --------------------------------------------------------------------------- #
# The fourth verdict
# --------------------------------------------------------------------------- #

def test_expect_open_is_a_fourth_verdict_not_an_alias():
    """It must not be confusable with any adversarial verdict."""
    assert EXPECT_OPEN == "expect_open"
    assert EXPECT_OPEN not in EXPECTED_VERDICTS
    assert EXPECT_OPEN not in {
        EXPECT_ACCEPT, EXPECT_REJECT, EXPECT_ACCEPT_CONDITIONAL
    }
    # The vocabulary grew by exactly one.
    assert OPEN_TARGET_VERDICTS == EXPECTED_VERDICTS | {EXPECT_OPEN}
    assert len(OPEN_TARGET_VERDICTS) == 4


def test_registry_registers_the_open_conjecture_track():
    entry = [b for b in list_benchmarks() if b["name"] == "formal_conjectures"]
    assert entry, "formal_conjectures is not registered"
    assert entry[0]["track"] == TRACK == "open_conjecture"
    assert entry[0]["kind"] == "open_target"


def test_loader_never_raises_via_registry():
    assert isinstance(load_benchmark("formal_conjectures"), list)


# --------------------------------------------------------------------------- #
# Absent corpus (the CI condition)
# --------------------------------------------------------------------------- #

def test_absent_corpus_returns_empty(monkeypatch, tmp_path):
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    assert load_formal_conjectures() == []
    assert load_benchmark("formal_conjectures") == []


# --------------------------------------------------------------------------- #
# Parser units (corpus-independent)
# --------------------------------------------------------------------------- #

_OPEN_SRC = """\
import FormalConjecturesUtil

/-!
# Collatz conjecture

*Reference:* [Wikipedia](https://en.wikipedia.org/wiki/Collatz_conjecture)
-/

namespace CollatzConjecture

def collatzStep (n : Nat) : Nat := if n % 2 = 0 then n / 2 else 3 * n + 1

/--
Does every positive integer reach 1?
-/
@[category research open, AMS 11 37]
theorem collatz_conjecture (n : Nat) (hn : n > 0) : exists m, collatzStep^[m] n = 1 := by
  sorry

/--
A solved variant, whose proof lives elsewhere.
-/
@[category research solved, AMS 11]
theorem collatz_conjecture.variants.small : True := by
  sorry

@[category test, AMS 11]
theorem sanity : 1 = 1 := by rfl

end CollatzConjecture
"""


def test_attribute_parsing_reads_category_and_ams():
    found = [
        (_category(m.group("attrs")), _ams(m.group("attrs")), m.group("name"))
        for m in _ATTR_DECL.finditer(_OPEN_SRC)
    ]
    assert found == [
        (CATEGORY_OPEN, [11, 37], "collatz_conjecture"),
        (CATEGORY_SOLVED, [11], "collatz_conjecture.variants.small"),
        ("test", [11], "sanity"),
    ]


def test_split_statement_strips_the_goal_marker():
    m = next(_ATTR_DECL.finditer(_OPEN_SRC))
    block = _declaration_block(_OPEN_SRC, m.start("kw"))
    statement, goal_marker = _split_statement(block)
    assert goal_marker is True
    assert "sorry" not in statement
    assert statement.startswith("theorem collatz_conjecture")
    assert statement.rstrip().endswith("= 1")


def test_split_statement_keeps_inner_assignment():
    """`let x := ...` inside a statement must not be mistaken for the proof."""
    block = "theorem t : let x := 3; x = 3 := by\n  sorry"
    statement, goal_marker = _split_statement(block)
    assert goal_marker is True
    assert "let x := 3" in statement


def test_source_area_reads_the_top_level_directory():
    p = Path("resources/formal-conjectures-main/formal-conjectures-main")
    assert _source_area(p / "FormalConjectures" / "Wikipedia" / "X.lean") == "Wikipedia"
    assert (
        _source_area(p / "FormalConjectures" / "ErdosProblems" / "1.lean")
        == "ErdosProblems"
    )


def test_loader_emits_only_open_from_a_synthetic_corpus(monkeypatch, tmp_path):
    """A `research solved` statement is also sorry-bearing and must NOT be emitted.

    This is the assertion that keeps the distinction honest: if the loader keyed
    off `sorry` instead of the category attribute, all three declarations below
    would come through as open targets.
    """
    root = tmp_path / "formal-conjectures-main" / "formal-conjectures-main"
    d = root / "FormalConjectures" / "Wikipedia"
    d.mkdir(parents=True)
    (d / "Collatz.lean").write_text(_OPEN_SRC, encoding="utf-8")
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))

    items = load_formal_conjectures()
    assert len(items) == 1
    it = items[0]
    assert it["id"] == "formal_conjectures:Wikipedia:Collatz:collatz_conjecture"
    assert it["kind"] == "statement_target"
    assert it["expected"]["verdict"] == EXPECT_OPEN
    assert it["expected"]["status"] == CATEGORY_OPEN
    assert it["expected"]["failure_mode"] == CLAIMED_PROOF
    assert it["expected"]["goal_marker_sorry"] is True
    assert it["expected"]["requires_answer"] is False
    assert it["grading"]["track"] == TRACK
    assert it["grading"]["expected_verdict"] == EXPECT_OPEN
    assert it["provenance"]["untrusted"] is True
    assert it["provenance"]["source_area"] == "Wikipedia"
    assert it["provenance"]["ams"] == [11, 37]
    # The formal field is a proof OBLIGATION, not an artifact with a hole in it.
    assert "sorry" not in it["formal"]
    # Corpus prose is fenced as data.
    assert "UNTRUSTED CORPUS EXCERPT" in it["informal"]


def test_answer_elaborator_is_flagged(monkeypatch, tmp_path):
    src = (
        "/-- Does P hold? -/\n"
        "@[category research open, AMS 5]\n"
        "theorem q : answer(sorry) <-> True := by\n  sorry\n"
    )
    d = tmp_path / "formal-conjectures-main" / "FormalConjectures" / "Other"
    d.mkdir(parents=True)
    (d / "Q.lean").write_text(src, encoding="utf-8")
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    items = load_formal_conjectures()
    assert len(items) == 1
    assert items[0]["expected"]["requires_answer"] is True


def test_open_with_a_formal_proof_pointer_is_not_treated_as_open(
    monkeypatch, tmp_path
):
    """Self-contradictory tagging fails safe: we do not call it open."""
    src = (
        '@[category research open, AMS 11, formal_proof using lean4 at "http://x"]\n'
        "theorem contradictory : True := by\n  sorry\n"
    )
    d = tmp_path / "formal-conjectures-main" / "FormalConjectures" / "Other"
    d.mkdir(parents=True)
    (d / "C.lean").write_text(src, encoding="utf-8")
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    assert load_formal_conjectures() == []


# --------------------------------------------------------------------------- #
# Corpus-conditional
# --------------------------------------------------------------------------- #

@pytest.mark.skipif(not _corpus_present(), reason="formal-conjectures absent")
def test_real_corpus_loads_open_targets():
    items = load_formal_conjectures()
    assert len(items) > 100, "expected hundreds of open conjectures"
    ids = [it["id"] for it in items]
    assert len(ids) == len(set(ids)), "item ids must be unique"
    for it in items:
        assert it["expected"]["verdict"] == EXPECT_OPEN
        assert it["expected"]["status"] == CATEGORY_OPEN
        assert it["provenance"]["category_attribute"] == CATEGORY_OPEN
        assert it["provenance"]["lean_toolchain"] == "leanprover/lean4:v4.27.0"
        assert set(it) >= {
            "id", "kind", "informal", "formal", "expected", "provenance", "grading",
        }


@pytest.mark.skipif(not _corpus_present(), reason="formal-conjectures absent")
def test_real_corpus_statements_carry_no_residual_sorry():
    """`formal` is the obligation; the goal marker is stripped and recorded."""
    items = load_formal_conjectures()
    leaked = [it["id"] for it in items if "sorry" in it["formal"].replace(
        "answer(sorry)", ""
    )]
    assert not leaked[:5], f"residual sorry in formal: {leaked[:5]}"
    marked = sum(1 for it in items if it["expected"]["goal_marker_sorry"])
    assert marked > 0.9 * len(items), "most open targets should end in `sorry`"


@pytest.mark.skipif(not _corpus_present(), reason="formal-conjectures absent")
def test_real_corpus_covers_several_source_areas():
    areas = {it["provenance"]["source_area"] for it in load_formal_conjectures()}
    assert {"ErdosProblems", "Wikipedia"} <= areas
