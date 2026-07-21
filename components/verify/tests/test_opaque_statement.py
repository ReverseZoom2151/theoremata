"""Tests for `statement_rests_on_opaque_constant`.

The parsing, classification and location logic is exercised with no Lean at all,
so CI (which has no toolchain) still runs something real. The fixture cases,
which are the headline evidence, are gated on a resolvable Lean binary and skip
cleanly otherwise.
"""
from __future__ import annotations

import os

import pytest

from theoremata_tools.opaque_statement import (
    DISCLAIMER,
    VERDICT_NO_FINDING,
    VERDICT_OPAQUE,
    VERDICT_UNKNOWN,
    _resolve,
    check_statement_constants,
    classify,
    locate_declaration,
    parse_probe_output,
    render_probe,
)

FIXTURES = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "fixtures",
    "opaque_statement",
)

_lean = _resolve("lean") or _resolve("lake")
requires_lean = pytest.mark.skipif(_lean is None, reason="Lean toolchain not available")


def _fixture(name: str) -> str:
    with open(os.path.join(FIXTURES, name), encoding="utf-8") as fh:
        return fh.read()


def _line(name: str, kind: str, axioms: str) -> str:
    return f"THEOREMATA_OPAQUE_CONST|{name}|{kind}|{axioms}"


def _report(*lines: str) -> str:
    return "\n".join(["THEOREMATA_OPAQUE_BEGIN", *lines, "THEOREMATA_OPAQUE_END"])


# --- probe rendering -------------------------------------------------------


def test_probe_reads_the_type_and_never_the_value():
    # The entire separation from the axiom audit rests on this one expression.
    probe = render_probe("Foo.bar")
    assert "ci.type.getUsedConstants" in probe
    assert "ci.value" not in probe
    assert "`Foo.bar" in probe


# --- probe parsing ---------------------------------------------------------


def test_parse_full_report():
    text = _report(
        _line("Nat", "inductive", ""),
        _line("Foo.residue", "def", "sorryAx"),
    )
    records = parse_probe_output(text)
    assert records == [
        {"name": "Nat", "kind": "inductive", "axioms": []},
        {"name": "Foo.residue", "kind": "def", "axioms": ["sorryAx"]},
    ]


def test_parse_tolerates_a_lean_location_prefix():
    text = _report("probe.lean:9:0: information: " + _line("Nat", "inductive", ""))
    assert parse_probe_output(text) == [
        {"name": "Nat", "kind": "inductive", "axioms": []}
    ]


def test_parse_multiple_axioms():
    text = _report(_line("Foo.f", "def", "propext,sorryAx, Classical.choice"))
    assert parse_probe_output(text)[0]["axioms"] == [
        "propext",
        "sorryAx",
        "Classical.choice",
    ]


def test_parse_returns_none_without_begin_marker():
    assert parse_probe_output(_line("Foo.f", "def", "sorryAx")) is None


def test_parse_returns_none_without_end_marker():
    text = "THEOREMATA_OPAQUE_BEGIN\n" + _line("Foo.f", "def", "sorryAx")
    assert parse_probe_output(text) is None


def test_parse_returns_none_when_target_missing():
    assert parse_probe_output("THEOREMATA_OPAQUE_MISSING") is None


def test_parse_returns_none_on_malformed_record():
    # A record we cannot read could be the opaque one, or could be the context
    # that makes another one legitimate. Refuse the whole report.
    assert parse_probe_output(_report("THEOREMATA_OPAQUE_CONST|Foo.f|def")) is None
    assert parse_probe_output(_report("THEOREMATA_OPAQUE_CONST||def|")) is None


def test_parse_ignores_unrelated_noise_between_markers():
    text = _report(
        "warning: declaration uses `sorry`",
        _line("Foo.f", "def", "sorryAx"),
    )
    assert [r["name"] for r in parse_probe_output(text)] == ["Foo.f"]


# --- classification: the admitted/abstract boundary ------------------------


def test_classify_flags_only_sorryax():
    records = [
        {"name": "Foo.admitted", "kind": "def", "axioms": ["sorryAx"]},
        {"name": "Foo.sealedSorry", "kind": "opaque", "axioms": ["sorryAx"]},
        {"name": "Foo.declared", "kind": "axiom", "axioms": ["Foo.declared"]},
        {"name": "Foo.sealedReal", "kind": "opaque", "axioms": []},
        {"name": "Foo.real", "kind": "def", "axioms": ["Classical.choice"]},
        {"name": "Group", "kind": "inductive", "axioms": []},
    ]
    assert [r["name"] for r in classify(records)] == [
        "Foo.admitted",
        "Foo.sealedSorry",
    ]


def test_classify_does_not_flag_a_declared_axiom():
    # A deliberate `axiom` is a visible assumption owned by the layer-2 audit,
    # not an admitted placeholder. Flagging it would make this check useless on
    # any axiomatic development.
    records = [{"name": "Foo.ax", "kind": "axiom", "axioms": ["Foo.ax"]}]
    assert classify(records) == []


# --- source location -------------------------------------------------------


def test_locate_declaration_finds_the_definition_site():
    src = "namespace Foo\n\nnoncomputable def residue (f : Nat) : Nat := sorry\n"
    loc = locate_declaration(src, "Foo.residue")
    assert loc["line"] == 3
    assert "residue" in loc["text"]


def test_locate_declaration_handles_attributes_and_opaque():
    src = "@[simp] opaque sealed : Nat := sorry\n"
    assert locate_declaration(src, "Foo.sealed")["line"] == 1


def test_locate_declaration_ignores_comments():
    src = "-- def residue : Nat := 0\n/- def residue -/\ndef residue : Nat := 0\n"
    assert locate_declaration(src, "residue")["line"] == 3


def test_locate_declaration_is_none_when_absent():
    assert locate_declaration("theorem t : True := trivial\n", "Foo.elsewhere") is None


def test_locate_declaration_does_not_match_a_longer_name():
    assert locate_declaration("def residueOf : Nat := 0\n", "residue") is None


# --- withholding: no accusation without evidence ---------------------------


def test_withholds_on_a_non_identifier_theorem_name():
    result = check_statement_constants("theorem t : True := trivial\n", "t; #exit")
    assert result["verdict"] == VERDICT_UNKNOWN
    assert result["opaque_constants"] == []


def test_withholds_when_no_toolchain_is_resolvable(monkeypatch):
    monkeypatch.setattr(
        "theoremata_tools.opaque_statement._resolve", lambda *a, **k: None
    )
    result = check_statement_constants("theorem t : True := trivial\n", "t")
    assert result["verdict"] == VERDICT_UNKNOWN
    assert "no Lean toolchain" in result["withheld_reason"]


def test_no_verdict_string_reads_as_an_endorsement():
    for verdict in (VERDICT_OPAQUE, VERDICT_NO_FINDING, VERDICT_UNKNOWN):
        lowered = verdict.lower()
        for banned in ("sound", "valid", "clean", "verified", "meaningful", "ok"):
            assert banned not in lowered
    assert "does not mean the statement is meaningful" in DISCLAIMER


# --- real Lean, the headline evidence --------------------------------------


@requires_lean
def test_positive_admitted_constants_are_flagged():
    result = check_statement_constants(
        _fixture("positive_admitted.lean"),
        "TheoremataOpaqueFixture.residueTheorem",
    )
    assert result["verdict"] == VERDICT_OPAQUE
    flagged = {c["name"] for c in result["opaque_constants"]}
    assert flagged == {
        "TheoremataOpaqueFixture.windingNumber",
        "TheoremataOpaqueFixture.residue",
        "TheoremataOpaqueFixture.HasContourValue",
    }
    # Actionable: every accusation carries a source line.
    for c in result["opaque_constants"]:
        assert c["defined_at"]["line"] > 0
        assert "sorry" in c["defined_at"]["text"]


@requires_lean
def test_negative_real_definitions_are_not_flagged():
    result = check_statement_constants(
        _fixture("negative_real.lean"), "TheoremataRealFixture.residueTheorem"
    )
    assert result["verdict"] == VERDICT_NO_FINDING
    assert result["opaque_constants"] == []


@requires_lean
def test_abstract_lemma_over_a_typeclass_is_not_flagged():
    result = check_statement_constants(
        _fixture("abstract_control.lean"), "TheoremataAbstractFixture.assoc_four"
    )
    assert result["verdict"] == VERDICT_NO_FINDING
    assert result["opaque_constants"] == []


@requires_lean
def test_declared_axiom_in_a_statement_is_not_flagged():
    result = check_statement_constants(
        _fixture("abstract_control.lean"), "TheoremataAbstractFixture.chosenPoint_self"
    )
    assert result["verdict"] == VERDICT_NO_FINDING
    assert "TheoremataAbstractFixture.chosenPoint" in result["statement_constants"]


@requires_lean
def test_honest_sorry_proof_leaves_the_statement_unaccused():
    # This is the distinction from the axiom audit: `#print axioms` reports
    # [sorryAx] for this file and for positive_admitted.lean alike, and cannot
    # tell an unfinished proof from a contentless statement. We can.
    result = check_statement_constants(
        _fixture("honest_sorry.lean"),
        "TheoremataHonestSorryFixture.double_eq_two_mul",
    )
    assert result["verdict"] == VERDICT_NO_FINDING


@requires_lean
def test_opaque_keyword_does_not_launder_a_sorry():
    result = check_statement_constants(
        _fixture("positive_sealed_opaque.lean"), "TheoremataSealedFixture.seals_agree"
    )
    assert result["verdict"] == VERDICT_OPAQUE
    flagged = {c["name"] for c in result["opaque_constants"]}
    assert flagged == {"TheoremataSealedFixture.admittedSeal"}
    # The honestly sealed constant sits in the same statement and is spared.
    assert "TheoremataSealedFixture.realSeal" in result["statement_constants"]


@requires_lean
def test_unknown_target_withholds_rather_than_accusing():
    result = check_statement_constants(
        _fixture("positive_admitted.lean"), "TheoremataOpaqueFixture.noSuchTheorem"
    )
    assert result["verdict"] == VERDICT_UNKNOWN
    assert result["opaque_constants"] == []


@requires_lean
def test_unelaborable_source_withholds_rather_than_accusing():
    # A missing import is the single most likely way third-party mathematics
    # arrives here, and it must produce silence, not an accusation.
    result = check_statement_constants(
        "theorem t : Nat.NoSuchPredicate 0 := by trivial\n", "t"
    )
    assert result["verdict"] == VERDICT_UNKNOWN
    assert result["opaque_constants"] == []
