"""Tests for the research-to-formal stage catalog."""
from __future__ import annotations

import pytest

from theoremata_tools import stages


def test_twelve_stages_present():
    assert len(stages.STAGES) == 12
    for key, spec in stages.STAGES.items():
        assert spec["key"] == key
        for field in ("title", "purpose", "produces", "prompt_template", "requires"):
            assert field in spec


def test_sequence_is_valid_topological_order():
    order = stages.sequence()
    assert len(order) == 12
    assert set(order) == set(stages.STAGES)
    position = {key: i for i, key in enumerate(order)}
    for key, spec in stages.STAGES.items():
        for dep in spec["requires"]:
            assert position[dep] < position[key], f"{dep} must precede {key}"


def test_render_fills_slots():
    prompt = stages.render("direct_proof", claim="x>0", assumptions="x real")
    assert "x>0" in prompt
    assert "x real" in prompt


def test_render_raises_on_missing_slot():
    with pytest.raises(ValueError):
        stages.render("direct_proof", claim="x>0")  # missing {assumptions}


def test_next_stages_entry_points():
    entry = stages.next_stages([])
    assert "scope_ideate" in entry
    # every returned entry stage has no unmet requirement
    for key in entry:
        assert stages.STAGES[key]["requires"] == []


def test_next_stages_progresses():
    after_scope = stages.next_stages(["scope_ideate", "environment_log"])
    assert "object_identification" in after_scope
    assert "scope_ideate" not in after_scope


def test_evidence_strength_ordering():
    assert stages.stronger_than("lean_checked", "numeric_screen")
    assert stages.stronger_than("prose_proof", "numeric_screen")
    assert not stages.stronger_than("numeric_screen", "lean_checked")
    assert not stages.stronger_than("prose_proof", "prose_proof")


def test_numerics_screen_never_prove_is_encoded():
    # the property-constrained-synthesis stage must state the hard rule
    text = stages.STAGES["property_constrained_synthesis"]["prompt_template"].lower()
    assert "screen" in text and "prove" in text
    # and the two-tolerance epistemics are available
    assert stages.FALSIFIER_TOLERANCES["exact_identity"] < stages.FALSIFIER_TOLERANCES[
        "finite_difference"
    ]


def test_formalization_target():
    target = stages.formalization_target(
        "the norm is invariant under U",
        {"U": "unitary matrix", "psi": "state vector"},
    )
    for key in ("lean_signature_stub", "symbol_dictionary", "sub_targets"):
        assert key in target
    assert target["symbol_dictionary"]["U"] == "unitary matrix"
    assert "sorry" in target["lean_signature_stub"]
    assert len(target["sub_targets"]) == 4


def test_run_dispatch():
    assert stages.run({"op": "sequence"})["sequence"][0] == "scope_ideate"
    rendered = stages.run(
        {"op": "render", "key": "prove_or_disprove", "slots": {"claim": "P", "assumptions": "Q"}}
    )
    assert "P" in rendered["prompt"]
    tgt = stages.run(
        {"op": "formalization_target", "informal_statement": "S", "symbol_dictionary": {"a": "b"}}
    )
    assert tgt["target"]["symbol_dictionary"] == {"a": "b"}
