"""Tests for the round-trip consistency-validator audit.

The audit is deterministic and validator-agnostic: it exercises an injected
``validate`` callable with taxonomy-keyed perturbations and reports recall /
specificity / pass@k collapse. These tests drive it with mock validators so the
measured numbers are exactly predictable, plus a smoke test against the real
default round-trip validator via ``run()``.
"""
from __future__ import annotations

import pytest

from theoremata_tools.roundtrip_audit import (
    DEFAULT_THRESHOLD,
    PERTURBATION_KINDS,
    audit,
    perturb,
    run,
    taxonomy_of,
)

# A rich faithful statement that carries every perturbation target: an explicit
# ∀, a hypothesis arrow, a bound glyph, a binder constant, a numeric literal.
_NL = "For all x, if x is at least 0 then n is at most x."
_LEAN = "theorem foo (n : Nat) : ∀ x, x ≥ 0 → n ≤ x"

_PAIRS = [(_NL, _LEAN)]


# --------------------------------------------------------------------------- #
# perturb(): every kind is generated, distinct, deterministic, and caught-able
# --------------------------------------------------------------------------- #

def test_every_kind_generated_and_distinct():
    variants = {k: perturb(_LEAN, k) for k in PERTURBATION_KINDS}
    # Each perturbation actually changes the statement.
    for k, v in variants.items():
        assert v != _LEAN, f"{k} did not change the statement"
    # And all five perturbations are distinct from one another.
    assert len(set(variants.values())) == len(PERTURBATION_KINDS)


def test_perturb_is_deterministic():
    for k in PERTURBATION_KINDS:
        assert perturb(_LEAN, k) == perturb(_LEAN, k)


def test_perturb_targets_expected_taxonomy():
    # drop/flip/relation/rename map onto the divergence taxonomy kinds.
    assert taxonomy_of("drop_hypothesis") == "dropped-hypothesis"
    assert taxonomy_of("flip_quantifier") == "quantifier-flip"
    assert taxonomy_of("swap_relation") == "relation-flip"
    assert taxonomy_of("rename_constant") == "lexical-drift"


def test_perturb_specific_edits():
    assert perturb(_LEAN, "flip_quantifier").count("∃") == 1  # ∀ -> ∃
    assert "≤" in perturb(_LEAN, "swap_relation")             # ≥ -> ≤
    assert "→" not in perturb(_LEAN, "drop_hypothesis")       # arrow removed
    assert "zn" in perturb(_LEAN, "rename_constant")          # n -> zn


def test_perturb_unknown_kind_raises():
    with pytest.raises(ValueError):
        perturb(_LEAN, "nonsense")


def test_perturb_inapplicable_returns_unchanged():
    # No hypothesis arrow -> drop_hypothesis is a no-op.
    plain = "theorem t : 2 = 2"
    assert perturb(plain, "drop_hypothesis") == plain


# --------------------------------------------------------------------------- #
# audit(): perfect / lenient mock validators
# --------------------------------------------------------------------------- #

def _perfect_validator(pairs):
    """Returns True iff the lean is one of the known faithful statements."""
    faithful = {lean for _, lean in pairs}
    return lambda nl, lean: lean in faithful


def test_perfect_validator_recall_and_specificity_are_one():
    result = audit(_PAIRS, validate=_perfect_validator(_PAIRS))
    assert result["recall"] == 1.0
    assert result["specificity"] == 1.0
    # Confusion: faithful all TP, perturbed all TN, no FP/FN.
    conf = result["confusion"]
    assert conf["tp"] == 1 and conf["fn"] == 0
    assert conf["fp"] == 0 and conf["tn"] == result["n_perturbations"]
    # Every kind was generated and fully caught.
    for k in PERTURBATION_KINDS:
        pk = result["per_kind"][k]
        assert pk["generated"] == 1
        assert pk["specificity"] == 1.0


def test_lenient_validator_specificity_collapses_to_zero():
    # An always-pass validator is the reward-hack: it keeps recall but catches
    # nothing, so the audit reports specificity 0 (the hack is exposed).
    result = audit(_PAIRS, validate=lambda nl, lean: True)
    assert result["recall"] == 1.0
    assert result["specificity"] == 0.0
    assert result["confusion"]["fp"] == result["n_perturbations"]
    for k in PERTURBATION_KINDS:
        assert result["per_kind"][k]["specificity"] == 0.0


def test_over_eager_validator_drops_recall():
    # A reject-everything validator has specificity 1 but recall 0.
    result = audit(_PAIRS, validate=lambda nl, lean: False)
    assert result["recall"] == 0.0
    assert result["specificity"] == 1.0


# --------------------------------------------------------------------------- #
# pass@k specificity collapse
# --------------------------------------------------------------------------- #

def test_pass_at_k_specificity_collapses_with_k():
    # A partial validator that catches only the first three (strong) kinds and
    # leaks the last two. pass@k specificity must be non-increasing in k and
    # strictly lower at max-k than at k=1.
    caught_kinds = {"drop_hypothesis", "flip_quantifier", "swap_relation"}
    leaked = {perturb(_LEAN, k) for k in PERTURBATION_KINDS if k not in caught_kinds}

    def partial(nl, lean):
        # Pass (fooled) iff this is a faithful statement OR a leaked perturbation.
        return lean == _LEAN or lean in leaked

    result = audit(_PAIRS, validate=partial)
    pak = result["pass_at_k_specificity"]
    ks = sorted(pak)
    values = [pak[k] for k in ks]

    # Monotonically non-increasing.
    assert all(values[i] >= values[i + 1] for i in range(len(values) - 1))
    # And it genuinely collapses: caught at k=1, leaks by max-k.
    assert values[0] == 1.0
    assert values[-1] < values[0]
    # Overall specificity reflects the leak (3 of 5 caught).
    assert result["specificity"] == pytest.approx(3 / 5)


# --------------------------------------------------------------------------- #
# determinism + run() dispatch
# --------------------------------------------------------------------------- #

def test_audit_is_deterministic():
    v = _perfect_validator(_PAIRS)
    assert audit(_PAIRS, validate=v) == audit(_PAIRS, validate=v)


def test_run_dispatch_uses_default_validator():
    # No injected callable -> default round-trip validator (offline lexical).
    out = run({"op": "roundtrip_audit", "pairs": [[_NL, _LEAN]]})
    assert out["op"] == "roundtrip_audit"
    assert out["n_pairs"] == 1
    assert 0.0 <= out["recall"] <= 1.0
    assert out["specificity"] is not None
    assert out["threshold"] == DEFAULT_THRESHOLD
    assert set(out["per_kind"]) == set(PERTURBATION_KINDS)


def test_run_rejects_unknown_op():
    with pytest.raises(ValueError):
        run({"op": "not_a_real_op", "pairs": []})


def test_run_accepts_dict_pairs():
    out = run({"pairs": [{"nl": _NL, "lean": _LEAN}]})
    assert out["n_pairs"] == 1
