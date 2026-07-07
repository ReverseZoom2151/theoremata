"""Tests for the per-formal-system SOURCE-SCAN soundness pre-gate."""
from __future__ import annotations

import pytest

from theoremata_tools.formal_source_scan import (
    mask_isabelle,
    mask_lean,
    mask_rocq,
    run,
    scan,
)


def _patterns(result, severity=None):
    return {
        f["pattern"]
        for f in result["flags"]
        if severity is None or f["severity"] == severity
    }


def _critical(result):
    return [f for f in result["flags"] if f["severity"] == "critical"]


# ---------------------------------------------------------------------------
# Masker invariants (length + newline preservation)
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "masker, src",
    [
        (mask_lean, 'a -- sorry\nb /- x -/ c\n"str"\n'),
        (mask_rocq, "a (* sorry *) b\n"),
        (mask_isabelle, "a (* sorry *) ‹text sorry› \"s\"\n"),
    ],
)
def test_masker_preserves_length_and_newlines(masker, src):
    masked = masker(src)
    assert len(masked) == len(src)
    assert [i for i, c in enumerate(src) if c == "\n"] == [
        i for i, c in enumerate(masked) if c == "\n"
    ]


# ---------------------------------------------------------------------------
# Rocq
# ---------------------------------------------------------------------------


def test_rocq_admitted_flagged():
    src = "Theorem t : True.\nProof.\nAdmitted.\n"
    result = scan("rocq", src)
    assert result["clean"] is False
    assert "Admitted" in _patterns(result, "critical")
    flag = next(f for f in result["flags"] if f["pattern"] == "Admitted")
    assert flag["line"] == 3


def test_rocq_admit_tactic_flagged():
    result = scan("rocq", "Theorem t : True.\nProof. admit. Admitted.\n")
    assert result["clean"] is False
    assert "admit" in _patterns(result, "critical")


def test_rocq_global_assumptions_flagged():
    for kw in ("Axiom", "Parameter", "Conjecture", "Hypothesis", "Variable"):
        result = scan("rocq", f"{kw} foo : nat.\n")
        assert result["clean"] is False, kw
        assert "global_assumption" in _patterns(result, "critical"), kw


def test_rocq_disabled_kernel_checks_flagged():
    cases = {
        "-type-in-type": "type_in_type",
        "Unset Universe Checking.": "unset_universe_checking",
        "Unset Guard Checking.": "unset_guard_checking",
        "Unset Positivity Checking.": "unset_positivity_checking",
        "#[bypass_check(guard)]": "bypass_check",
        "Admit Obligations.": "admit_obligations",
    }
    for src, pattern in cases.items():
        result = scan("rocq", src + "\n")
        assert result["clean"] is False, src
        assert pattern in _patterns(result, "critical"), src


def test_rocq_set_bullet_behavior_is_benign():
    result = scan("rocq", 'Set Bullet Behavior "Strict Subproofs".\n')
    assert result["clean"] is True
    assert result["flags"] == []


def test_rocq_admitted_in_comment_not_flagged():
    src = "(* Admitted here would be a bug *)\nTheorem t : True.\nProof. exact I. Qed.\n"
    result = scan("rocq", src)
    assert result["clean"] is True
    assert result["flags"] == []


def test_rocq_nested_comment_not_flagged():
    src = "(* outer (* Axiom bad *) still admit *)\nDefinition d := 0.\n"
    assert scan("rocq", src)["clean"] is True


def test_rocq_token_in_string_not_flagged():
    src = 'Definition s := "Admitted and Axiom"%string.\n'
    assert scan("rocq", src)["clean"] is True


def test_rocq_clean_proof():
    src = (
        "Theorem add_0_r : forall n : nat, n + 0 = n.\n"
        "Proof.\n"
        "  intros n. induction n as [| k IH].\n"
        "  - reflexivity.\n"
        "  - simpl. rewrite IH. reflexivity.\n"
        "Qed.\n"
    )
    result = scan("rocq", src)
    assert result["clean"] is True
    assert result["flags"] == []


# ---------------------------------------------------------------------------
# Isabelle
# ---------------------------------------------------------------------------


def test_isabelle_sorry_flagged():
    result = scan("isabelle", 'lemma foo: "P" \n  sorry\n')
    assert result["clean"] is False
    assert "sorry" in _patterns(result, "critical")


def test_isabelle_apply_rule_sorry_flagged():
    result = scan("isabelle", 'lemma foo: "P" apply (rule sorry) done\n')
    assert result["clean"] is False
    assert "sorry" in _patterns(result, "critical")


def test_isabelle_oops_flagged():
    result = scan("isabelle", 'lemma foo: "P"\n  oops\n')
    assert result["clean"] is False
    assert "oops" in _patterns(result, "critical")


def test_isabelle_quick_and_dirty_flagged():
    result = scan("isabelle", "declare [[quick_and_dirty = true]]\n")
    assert result["clean"] is False
    assert "quick_and_dirty" in _patterns(result, "critical")


def test_isabelle_oracle_and_add_oracle_flagged():
    result = scan("isabelle", 'oracle myrule = ‹fn ct => ct›\n')
    assert result["clean"] is False
    assert "oracle" in _patterns(result, "critical")

    ml = scan("isabelle", 'ML ‹val (_, mk) = Thm.add_oracle (bnd, f) thy›\n')
    # The cartouche is masked, so the ML body's add_oracle is NOT seen — but a
    # bare (unmasked) Thm.add_oracle still is:
    bare = scan("isabelle", "setup Thm.add_oracle stuff\n")
    assert bare["clean"] is False
    assert "add_oracle" in _patterns(bare, "critical")
    # And the masked ML-cartouche form is clean of the oracle token inside it.
    assert "add_oracle" not in _patterns(ml, "critical")


def test_isabelle_axiomatization_and_axioms_flagged():
    assert scan("isabelle", 'axiomatization where bad: "False"\n')["clean"] is False
    assert scan("isabelle", "axioms bad: False\n")["clean"] is False


def test_isabelle_nitpick_quickcheck_are_warnings():
    result = scan("isabelle", 'lemma foo: "P"\n  nitpick\n  quickcheck\n  by auto\n')
    # Informational only: warnings must not break cleanliness.
    assert result["clean"] is True
    assert _patterns(result, "warning") == {"nitpick", "quickcheck"}
    assert _critical(result) == []


def test_isabelle_sorry_in_comment_not_flagged():
    src = '(* sorry is fine in a comment *)\nlemma foo: "x = x" by simp\n'
    result = scan("isabelle", src)
    assert result["clean"] is True
    assert result["flags"] == []


def test_isabelle_sorry_in_cartouche_not_flagged():
    src = 'text ‹this note mentions sorry and oops›\nlemma foo: "x = x" by simp\n'
    assert scan("isabelle", src)["clean"] is True


def test_isabelle_sorry_in_string_not_flagged():
    src = 'lemma foo: "P sorry Q" by blast\n'
    assert scan("isabelle", src)["clean"] is True


def test_isabelle_clean_proof():
    src = (
        "theory Scratch\n"
        "  imports Main\n"
        "begin\n"
        'lemma refl_eq: "x = x"\n'
        "  by simp\n"
        "end\n"
    )
    result = scan("isabelle", src)
    assert result["clean"] is True
    assert result["flags"] == []


# ---------------------------------------------------------------------------
# Lean
# ---------------------------------------------------------------------------


def test_lean_sorry_flagged_with_location():
    result = scan("lean", "theorem t : True := by\n  sorry\n")
    assert result["clean"] is False
    crit = _critical(result)
    assert len(crit) == 1
    assert crit[0]["pattern"] == "sorry"
    assert crit[0]["line"] == 2


def test_lean_sorryAx_and_admit_flagged():
    assert "sorryAx" in _patterns(scan("lean", "#check @sorryAx\n"), "critical")
    assert "admit" in _patterns(scan("lean", "theorem t : True := by admit\n"), "critical")


def test_lean_sorry_does_not_double_match_sorryAx():
    result = scan("lean", "#check @sorryAx\n")
    assert _patterns(result, "critical") == {"sorryAx"}


def test_lean_native_decide_is_critical():
    result = scan("lean", "theorem t : P := by native_decide\n")
    assert result["clean"] is False
    assert "native_decide" in _patterns(result, "critical")


def test_lean_custom_axiom_flagged():
    result = scan("lean", "axiom bad : False\n")
    assert result["clean"] is False
    assert "axiom" in _patterns(result, "critical")


def test_lean_native_trust_axioms_flagged():
    real = scan("lean", "#print axioms Lean.ofReduceBool\n")
    assert "ofReduceBool" in _patterns(real, "critical")
    assert "trustCompiler" in _patterns(
        scan("lean", "example := Lean.trustCompiler\n"), "critical"
    )
    # The same token inside a comment is masked and therefore not flagged.
    commented = scan("lean", "-- uses Lean.ofReduceBool\ntheorem t : True := trivial\n")
    assert commented["clean"] is True


def test_lean_attribute_mechanisms_are_warnings():
    for attr, pat in (
        ("@[implemented_by fastImpl]", "implemented_by"),
        ('@[extern "c_fn"]', "extern"),
        ("@[csimp] theorem e : f = g := rfl", "csimp"),
    ):
        result = scan("lean", attr + "\n")
        assert pat in _patterns(result, "warning"), attr
        # Attributes alone are informational -> still clean.
        assert result["clean"] is True, attr


def test_lean_sorry_prime_and_prefixed_not_flagged():
    src = (
        "def sorry' : Nat := 0\n"
        "def my_sorry : Nat := 1\n"
        "def sorryish : Nat := 2\n"
        "theorem t : True := trivial\n"
    )
    assert scan("lean", src)["clean"] is True


def test_lean_sorry_in_comment_not_flagged():
    assert scan("lean", "theorem t : True := by\n  -- sorry note\n  trivial\n")[
        "clean"
    ] is True
    assert scan("lean", "/- sorry todo -/\ntheorem t : True := trivial\n")[
        "clean"
    ] is True


def test_lean_sorry_in_string_not_flagged():
    src = 'def msg : String := "contains sorry text"\n'
    assert scan("lean", src)["clean"] is True


def test_lean_clean_proof():
    src = (
        "import Mathlib\n\n"
        "theorem even_sq (n : Nat) (h : Even n) : Even (n * n) := by\n"
        "  obtain ⟨k, rfl⟩ := h\n"
        "  exact ⟨2 * k * k, by ring⟩\n"
    )
    result = scan("lean", src)
    assert result["clean"] is True
    assert result["flags"] == []


# ---------------------------------------------------------------------------
# Dispatch + contract
# ---------------------------------------------------------------------------


def test_run_dispatch_matches_scan():
    req = {"system": "lean", "source": "theorem t : True := by sorry\n"}
    assert run(req) == scan("lean", req["source"])


def test_result_contract_shape():
    result = scan("lean", "theorem t : True := by sorry\n")
    assert set(result) == {"clean", "system", "flags"}
    assert result["system"] == "lean"
    for f in result["flags"]:
        assert set(f) == {"pattern", "severity", "line", "snippet"}
        assert f["severity"] in ("critical", "warning")


def test_coq_alias_routes_to_rocq():
    result = scan("coq", "Admitted.\n")
    assert result["system"] == "rocq"
    assert result["clean"] is False


def test_unknown_system_raises():
    with pytest.raises(ValueError):
        scan("agda", "postulate p : Set\n")
