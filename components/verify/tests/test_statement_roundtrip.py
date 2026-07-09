import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from theoremata_tools.statement_roundtrip import (  # noqa: E402
    ADDED_CONSTRAINT,
    DROPPED_HYPOTHESIS,
    FAITHFUL,
    MISMATCH,
    NEGATION_MISMATCH,
    QUANTIFIER_FLIP,
    RELATION_FLIP,
    SUSPECT,
    detect_divergences,
    roundtrip_validate,
    run,
)

# A faithful pair: the Lean statement encodes exactly what the English says.
FAITHFUL_INFORMAL = (
    "For all natural numbers n, if n is even then n squared is even."
)
FAITHFUL_LEAN = (
    "theorem even_sq (n : Nat) : Even n → Even (n ^ 2) := by sorry"
)

# A pair that FLIPS the quantifier: English says "for all", Lean says "exists".
QFLIP_INFORMAL = "For all real numbers x, x squared is at least zero."
QFLIP_LEAN = "theorem q (x : Real) : ∃ x, x ^ 2 ≥ 0 := by sorry"


def _kinds(result):
    return {d["kind"] for d in result["divergences"]}


# --------------------------------------------------------------------------- #
# Faithful pair
# --------------------------------------------------------------------------- #

def test_faithful_pair_scores_high_and_verdict_faithful():
    res = roundtrip_validate(FAITHFUL_INFORMAL, FAITHFUL_LEAN)
    assert res["verdict"] == FAITHFUL
    assert res["score"] >= 0.80
    assert res["advisory"] is True
    assert res["backend"] == "lexical"
    assert QUANTIFIER_FLIP not in _kinds(res)
    assert RELATION_FLIP not in _kinds(res)


# --------------------------------------------------------------------------- #
# Quantifier flip -> suspect/mismatch and a listed divergence
# --------------------------------------------------------------------------- #

def test_quantifier_flip_is_flagged_and_downgraded():
    res = roundtrip_validate(QFLIP_INFORMAL, QFLIP_LEAN)
    assert res["verdict"] in (SUSPECT, MISMATCH)
    assert QUANTIFIER_FLIP in _kinds(res)
    assert res["score"] < 0.80
    assert res["advisory"] is True


# --------------------------------------------------------------------------- #
# Dropped hypothesis -> lower score, listed divergence
# --------------------------------------------------------------------------- #

def test_dropped_hypothesis_is_flagged():
    informal = (
        "For all integers n, if n is positive and n is even, "
        "then n is at least two."
    )
    # Lean drops the "n is even" hypothesis entirely.
    lean = "theorem t (n : Int) : n > 0 → n ≥ 2 := by sorry"
    res = roundtrip_validate(informal, lean)
    assert DROPPED_HYPOTHESIS in _kinds(res)
    assert res["verdict"] in (SUSPECT, MISMATCH)


# --------------------------------------------------------------------------- #
# Relation direction flip
# --------------------------------------------------------------------------- #

def test_relation_flip_detected():
    divs = detect_divergences(
        "x is at most five", "x is at least five"
    )
    kinds = {d["kind"] for d in divs}
    assert RELATION_FLIP in kinds


# --------------------------------------------------------------------------- #
# Negation mismatch
# --------------------------------------------------------------------------- #

def test_negation_mismatch_detected():
    divs = detect_divergences(
        "the sequence converges", "the sequence does not converge"
    )
    kinds = {d["kind"] for d in divs}
    assert NEGATION_MISMATCH in kinds


# --------------------------------------------------------------------------- #
# Added constraint (present in back-translation, absent from informal)
# --------------------------------------------------------------------------- #

def test_added_constraint_detected():
    informal = "For all integers n, n squared is nonnegative."
    # Lean adds a spurious positivity hypothesis.
    lean = "theorem t (n : Int) : n > 100 → n ^ 2 ≥ 0 := by sorry"
    res = roundtrip_validate(informal, lean)
    assert ADDED_CONSTRAINT in _kinds(res) or DROPPED_HYPOTHESIS in _kinds(res)


# --------------------------------------------------------------------------- #
# Lexical fallback runs with NO model / NO network
# --------------------------------------------------------------------------- #

def test_lexical_fallback_runs_with_no_model():
    res = roundtrip_validate(FAITHFUL_INFORMAL, FAITHFUL_LEAN, model=None)
    assert res["backend"] == "lexical"
    assert "back_translation" in res and res["back_translation"]
    assert isinstance(res["divergences"], list)


def test_model_failure_falls_back_to_lexical():
    def broken_model(informal, lean):
        raise RuntimeError("no network")

    res = roundtrip_validate(FAITHFUL_INFORMAL, FAITHFUL_LEAN, model=broken_model)
    assert res["backend"] == "lexical"
    assert res["advisory"] is True


# --------------------------------------------------------------------------- #
# Injected model backend
# --------------------------------------------------------------------------- #

def test_injected_model_backend_used():
    def fake_model(informal, lean):
        return {
            "back_translation": "For all natural numbers n, n squared is even.",
            "divergences": [
                {"kind": "dropped-hypothesis",
                 "detail": "the evenness of n was dropped"}
            ],
        }

    res = roundtrip_validate(FAITHFUL_INFORMAL, FAITHFUL_LEAN, model=fake_model)
    assert res["backend"] == "model"
    assert DROPPED_HYPOTHESIS in _kinds(res)
    assert res["advisory"] is True


def test_default_model_backend_runs_in_mock_mode(monkeypatch):
    monkeypatch.setenv("THEOREMATA_MODEL_MOCK", "1")
    res = roundtrip_validate(FAITHFUL_INFORMAL, FAITHFUL_LEAN, model=True)
    # Mock provider may or may not yield a usable back_translation; either way
    # the result must be well-formed and advisory.
    assert res["backend"] in ("model", "lexical")
    assert res["advisory"] is True
    assert res["verdict"] in (FAITHFUL, SUSPECT, MISMATCH)


# --------------------------------------------------------------------------- #
# advisory:true is always set
# --------------------------------------------------------------------------- #

def test_advisory_always_true():
    for inf, lean in [
        (FAITHFUL_INFORMAL, FAITHFUL_LEAN),
        (QFLIP_INFORMAL, QFLIP_LEAN),
        ("", ""),
    ]:
        assert roundtrip_validate(inf, lean)["advisory"] is True


def test_note_documents_advisory_never_overrides_formal_gate():
    res = roundtrip_validate(FAITHFUL_INFORMAL, FAITHFUL_LEAN)
    assert "NEVER" in res["note"]
    assert "formal gate" in res["note"].lower()


# --------------------------------------------------------------------------- #
# run() dispatch
# --------------------------------------------------------------------------- #

def test_run_roundtrip_op():
    res = run({
        "op": "roundtrip",
        "informal": FAITHFUL_INFORMAL,
        "lean_statement": FAITHFUL_LEAN,
    })
    assert res["op"] == "roundtrip"
    assert res["verdict"] == FAITHFUL
    assert res["advisory"] is True


def test_run_accepts_lean_alias_key():
    res = run({
        "op": "roundtrip",
        "informal": QFLIP_INFORMAL,
        "lean": QFLIP_LEAN,
    })
    assert QUANTIFIER_FLIP in {d["kind"] for d in res["divergences"]}


def test_run_default_op_is_roundtrip():
    res = run({"informal": "x", "lean_statement": "theorem t : x = x := rfl"})
    assert res["op"] == "roundtrip"


def test_run_unknown_op_raises():
    import pytest
    with pytest.raises(ValueError):
        run({"op": "nope"})


def test_run_model_flag_in_mock_mode(monkeypatch):
    monkeypatch.setenv("THEOREMATA_MODEL_MOCK", "1")
    res = run({
        "op": "roundtrip",
        "informal": FAITHFUL_INFORMAL,
        "lean_statement": FAITHFUL_LEAN,
        "model": True,
    })
    assert res["advisory"] is True
    assert res["backend"] in ("model", "lexical")
