import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

import pytest  # noqa: E402

from theoremata_tools.formalization_meta import (  # noqa: E402
    DIVERGENCE_KINDS,
    PROBLEM_STATUSES,
    SCHEMA_VERSION,
    Divergence,
    DivergenceKind,
    FormalizationMeta,
    MainResult,
    ProblemStatus,
    from_dict,
    run,
    to_dict,
    validate,
)


def _full_document() -> dict:
    """A complete v0.3 document exercising every modeled block."""
    return {
        "version": SCHEMA_VERSION,
        "project": {
            "name": "theoremata-imo-2024",
            "authors": ["A. Formalizer", "B. Reviewer"],
            "license": "Apache-2.0",
        },
        "sources": [
            {
                "title": "IMO 2024 Problem 1",
                "authors": ["IMO Jury"],
                "id": "imo-2024-p1",
                "type": "competition",
            },
            {
                "title": "On a conjecture of Erdos",
                "authors": ["P. Erdos"],
                "id": "arxiv:0000.00000",
                "type": "paper",
            },
        ],
        "status": {
            "scope": "full",
            "sorry_count": 2,
            "sorry_in_definitions": 1,
            "axioms": ["propext", "Classical.choice", "Quot.sound"],
            "main_results": [
                {
                    "declaration": "imo2024_p1",
                    "file": "Theoremata/IMO2024/P1.lean",
                    "sorry_count": 0,
                    "axioms": ["propext", "Classical.choice"],
                },
                {
                    "declaration": "erdos_conjecture_partial",
                    "file": "Theoremata/Erdos/Partial.lean",
                    "sorry_count": 2,
                    "axioms": ["propext", "Quot.sound"],
                },
            ],
        },
        "automation": {
            "methods": [
                {"method": "autonomous", "framework": "Theoremata"},
                {"method": "human", "framework": "Lean 4"},
            ]
        },
        "review": {
            "status": "human + agent",
            "reviewers": ["B. Reviewer", "theoremata-critic"],
            "notes": "statements checked line by line against the informal source",
        },
        "fidelity": {
            "divergences": [
                {
                    "kind": DivergenceKind.ANSWER_BAKED_INTO_STATEMENT,
                    "detail": "the numeric answer 199 appears in the statement",
                    "statement": "imo2024_p1",
                },
                {
                    "kind": DivergenceKind.BACKGROUND_FACT_ASSUMED,
                    "detail": "irrationality of sqrt 2 taken as a hypothesis",
                    "statement": "erdos_conjecture_partial",
                },
            ]
        },
        "alignment": {
            "statements": [
                {
                    "source": "IMO 2024 Problem 1",
                    "lean": "imo2024_p1",
                    "module": "Theoremata.IMO2024.P1",
                    "status": ProblemStatus.PROVED,
                    "note": "answer baked in; see fidelity",
                },
                {
                    "source": "On a conjecture of Erdos",
                    "lean": "erdos_conjecture_partial",
                    "module": "Theoremata.Erdos.Partial",
                    "status": ProblemStatus.UNSOLVED,
                    "note": None,
                },
            ]
        },
    }


# --- round-trip -----------------------------------------------------------

def test_full_document_round_trips_losslessly():
    doc = _full_document()
    out = FormalizationMeta.from_dict(doc).to_dict()
    # ``note: None`` is dropped as "not stated"; everything else survives verbatim
    doc["alignment"]["statements"][1].pop("note")
    assert out == doc


def test_round_trip_is_idempotent():
    meta = FormalizationMeta.from_dict(_full_document())
    once = meta.to_dict()
    twice = FormalizationMeta.from_dict(once).to_dict()
    assert once == twice


def test_json_round_trip():
    meta = FormalizationMeta.from_dict(_full_document())
    assert FormalizationMeta.from_json(meta.to_json()).to_dict() == meta.to_dict()


def test_module_level_wrappers_match_methods():
    doc = _full_document()
    assert to_dict(from_dict(doc)) == FormalizationMeta.from_dict(doc).to_dict()


def test_unknown_top_level_keys_are_preserved():
    doc = _full_document()
    doc["future_block"] = {"something": ["new"]}
    out = FormalizationMeta.from_dict(doc).to_dict()
    assert out["future_block"] == {"something": ["new"]}
    # preserved keys come after the known ones
    assert list(out)[-1] == "future_block"


def test_non_mapping_document_raises():
    with pytest.raises(TypeError):
        FormalizationMeta.from_dict(["not", "a", "mapping"])


# --- validate() -----------------------------------------------------------

def test_validate_accepts_full_document():
    assert validate(_full_document()) == []


def test_validate_catches_missing_required_field():
    doc = _full_document()
    del doc["project"]["name"]
    errors = validate(doc)
    assert [e["path"] for e in errors] == ["project.name"]
    assert errors[0]["code"] == "missing_required"
    assert set(errors[0]) == {"path", "code", "message"}


def test_validate_catches_several_missing_fields_at_once():
    errors = validate({})
    paths = [e["path"] for e in errors]
    assert paths == ["project.name", "project.authors", "sources", "status.scope"]


def test_validate_rejects_negative_counts():
    doc = _full_document()
    doc["status"]["sorry_count"] = -1
    doc["status"]["sorry_in_definitions"] = -3
    codes = {(e["path"], e["code"]) for e in validate(doc)}
    assert ("status.sorry_count", "invalid_value") in codes
    assert ("status.sorry_in_definitions", "invalid_value") in codes


def test_validate_rejects_unknown_divergence_kind():
    doc = _full_document()
    doc["fidelity"]["divergences"][0]["kind"] = "vibes"
    errors = validate(doc)
    assert any(e["code"] == "unknown_divergence_kind" for e in errors)


def test_validate_requires_divergence_detail():
    doc = _full_document()
    doc["fidelity"]["divergences"][0].pop("detail")
    errors = validate(doc)
    assert any(
        e["path"] == "fidelity.divergences[0].detail" and e["code"] == "missing_required"
        for e in errors
    )


def test_validate_rejects_unknown_problem_status():
    doc = _full_document()
    doc["alignment"]["statements"][0]["status"] = "probably"
    errors = validate(doc)
    assert any(e["code"] == "unknown_problem_status" for e in errors)


def test_validate_flags_ledger_axiom_missing_from_repo_summary():
    doc = _full_document()
    doc["status"]["axioms"] = ["propext"]
    errors = validate(doc)
    assert any(e["code"] == "axiom_not_in_repo_set" for e in errors)


def test_validate_requires_declaration_on_ledger_row():
    doc = _full_document()
    doc["status"]["main_results"][0].pop("declaration")
    errors = validate(doc)
    assert any(
        e["path"] == "status.main_results[0].declaration" for e in errors
    )


def test_validate_accepts_a_model_instance():
    assert validate(FormalizationMeta.from_dict(_full_document())) == []


# --- sorry_in_definitions independence ------------------------------------

def test_sorry_in_definitions_is_independent_of_sorry_count():
    doc = _full_document()
    # nothing left to prove, yet a definition still has a hole: legal, and the
    # most dangerous state the schema exists to surface
    doc["status"]["sorry_count"] = 0
    doc["status"]["sorry_in_definitions"] = 1
    doc["status"]["main_results"] = []
    meta = FormalizationMeta.from_dict(doc)
    assert meta.status.sorry_count == 0
    assert meta.status.sorry_in_definitions == 1
    assert meta.validate() == []
    assert meta.to_dict()["status"]["sorry_in_definitions"] == 1


def test_sorry_in_definitions_survives_when_sorry_count_changes():
    doc = _full_document()
    doc["status"]["sorry_in_definitions"] = 4
    for count in (0, 1, 99):
        doc["status"]["sorry_count"] = count
        meta = FormalizationMeta.from_dict(doc)
        assert meta.status.sorry_in_definitions == 4
        assert meta.status.sorry_count == count


def test_zero_sorry_fields_are_serialized_not_dropped():
    meta = FormalizationMeta.from_dict(_full_document())
    meta.status.sorry_count = 0
    meta.status.sorry_in_definitions = 0
    status = meta.to_dict()["status"]
    assert status["sorry_count"] == 0
    assert status["sorry_in_definitions"] == 0


# --- divergence kinds -----------------------------------------------------

def test_every_divergence_kind_serializes_and_round_trips():
    assert len(DIVERGENCE_KINDS) == 7
    doc = _full_document()
    doc["fidelity"]["divergences"] = [
        {"kind": kind, "detail": f"detail for {kind}"} for kind in DIVERGENCE_KINDS
    ]
    meta = FormalizationMeta.from_dict(doc)
    assert [d.kind for d in meta.fidelity.divergences] == list(DIVERGENCE_KINDS)
    out = meta.to_dict()["fidelity"]["divergences"]
    assert [d["kind"] for d in out] == list(DIVERGENCE_KINDS)
    assert meta.validate() == []


def test_answer_baked_into_statement_kind_is_present():
    # the disclosure a real published corpus used this mechanism for
    assert DivergenceKind.ANSWER_BAKED_INTO_STATEMENT in DIVERGENCE_KINDS
    d = Divergence.from_dict(
        {"kind": DivergenceKind.ANSWER_BAKED_INTO_STATEMENT, "detail": "answer given"}
    )
    assert d.to_dict()["kind"] == "answer_baked_into_statement"


def test_problem_statuses_enum():
    assert PROBLEM_STATUSES == ("proved", "disproved", "unsolved")


# --- per-declaration ledger -----------------------------------------------

def test_per_declaration_ledger_round_trips():
    doc = _full_document()
    meta = FormalizationMeta.from_dict(doc)
    ledger = meta.status.main_results
    assert [r.declaration for r in ledger] == [
        "imo2024_p1",
        "erdos_conjecture_partial",
    ]
    assert ledger[0].file == "Theoremata/IMO2024/P1.lean"
    assert ledger[1].sorry_count == 2
    assert ledger[0].axioms == ["propext", "Classical.choice"]
    assert meta.to_dict()["status"]["main_results"] == doc["status"]["main_results"]


def test_ledger_row_axioms_are_per_declaration_not_repo_union():
    meta = FormalizationMeta.from_dict(_full_document())
    rows = meta.status.main_results
    assert rows[0].axioms != rows[1].axioms
    assert set(rows[0].axioms) | set(rows[1].axioms) == set(meta.status.axioms)


def test_empty_ledger_round_trips():
    meta = FormalizationMeta.from_dict({"status": {"main_results": []}})
    assert meta.to_dict()["status"]["main_results"] == []


def test_main_result_defaults():
    r = MainResult.from_dict({})
    assert r.to_dict() == {"sorry_count": 0, "axioms": []}


# --- determinism ----------------------------------------------------------

def test_serialization_key_order_is_deterministic():
    meta = FormalizationMeta.from_dict(_full_document())
    expected = [
        "version",
        "project",
        "sources",
        "status",
        "automation",
        "review",
        "fidelity",
        "alignment",
    ]
    assert list(meta.to_dict()) == expected
    assert list(meta.status.to_dict()) == [
        "scope",
        "sorry_count",
        "sorry_in_definitions",
        "axioms",
        "main_results",
    ]


def test_serialization_order_independent_of_input_order():
    doc = _full_document()
    shuffled = {k: doc[k] for k in reversed(list(doc))}
    assert FormalizationMeta.from_dict(shuffled).to_json() == (
        FormalizationMeta.from_dict(doc).to_json()
    )


def test_repeated_serialization_is_byte_identical():
    meta = FormalizationMeta.from_dict(_full_document())
    assert meta.to_json() == meta.to_json()
    assert meta.to_json() == FormalizationMeta.from_dict(_full_document()).to_json()


def test_list_order_is_preserved_not_sorted():
    doc = _full_document()
    doc["project"]["authors"] = ["Z. Last", "A. First"]
    doc["status"]["axioms"] = ["Quot.sound", "propext", "Classical.choice"]
    out = FormalizationMeta.from_dict(doc).to_dict()
    assert out["project"]["authors"] == ["Z. Last", "A. First"]
    assert out["status"]["axioms"] == ["Quot.sound", "propext", "Classical.choice"]


# --- run() dispatch -------------------------------------------------------

def test_run_default_op_is_validate():
    res = run({"document": _full_document()})
    assert res["op"] == "validate"
    assert res["valid"] is True
    assert res["n_errors"] == 0


def test_run_validate_reports_errors():
    res = run({"op": "validate", "document": {}})
    assert res["valid"] is False
    assert res["n_errors"] == len(res["errors"]) > 0


def test_run_normalize_canonicalizes_order():
    doc = _full_document()
    shuffled = {k: doc[k] for k in reversed(list(doc))}
    assert list(run({"op": "normalize", "document": shuffled})["document"])[0] == "version"


def test_run_schema_reports_identity_and_enums():
    res = run({"op": "schema"})
    assert res["version"] == SCHEMA_VERSION == "0.3"
    assert res["schema"] == "formalization.yaml"
    assert "mathlib-initiative" in res["url"]
    assert res["divergence_kinds"] == list(DIVERGENCE_KINDS)
    assert res["problem_statuses"] == list(PROBLEM_STATUSES)
    assert isinstance(res["yaml_available"], bool)


def test_run_unknown_op_raises():
    with pytest.raises(ValueError):
        run({"op": "nope"})


# --- yaml seam ------------------------------------------------------------

def test_yaml_seam_round_trips_or_raises_actionable_error():
    from theoremata_tools.formalization_meta import yaml_available

    meta = FormalizationMeta.from_dict(_full_document())
    if yaml_available():
        assert FormalizationMeta.from_yaml(meta.to_yaml()).to_dict() == meta.to_dict()
    else:
        with pytest.raises(RuntimeError, match="PyYAML"):
            meta.to_yaml()
        with pytest.raises(RuntimeError, match="PyYAML"):
            FormalizationMeta.from_yaml("version: '0.3'")
