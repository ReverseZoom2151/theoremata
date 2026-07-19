"""Tests for the untrusted Wolfram counterexample oracle.

Every test here must pass with NO Wolfram Engine installed, which is the CI
condition: the engine is stubbed by monkeypatching ``available``/``evaluate``
on the module. That also lets us feed the oracle deliberately BOGUS answers,
which is the only way to test the part that actually matters -- that our own
independent recheck catches them.
"""
from __future__ import annotations

from fractions import Fraction

import pytest

from theoremata_tools import wolfram_falsify as wf


def stub(monkeypatch, result: str, *, ok: bool = True, error: str | None = None):
    """Pretend an engine is present and make it return ``result``."""
    monkeypatch.setattr(wf, "available", lambda: True)
    monkeypatch.setattr(
        wf,
        "evaluate",
        lambda code, timeout=30.0: {
            "ok": ok, "result": result, "error": error, "unavailable": False,
        },
    )


def absent(monkeypatch):
    monkeypatch.setattr(wf, "available", lambda: False)

    def _boom(code, timeout=30.0):  # pragma: no cover - must never be reached
        raise AssertionError("evaluate() called while the engine is unavailable")

    monkeypatch.setattr(wf, "evaluate", _boom)


# --- unavailable engine -----------------------------------------------------


def test_falsify_unavailable_is_clean(monkeypatch):
    absent(monkeypatch)
    out = wf.falsify({"variables": ["x"], "claim": "x * x >= 0"})
    assert out["verdict"] == "unavailable"
    assert out["available"] is False
    assert out["refuted"] is False and out["proved"] is False
    assert "assignment" not in out


def test_integer_relation_unavailable_is_clean(monkeypatch):
    absent(monkeypatch)
    out = wf.integer_relation({"constants": ["Pi", "1"]})
    assert out["verdict"] == "unavailable"
    assert out["proved"] is False
    assert out["status"] == "unproved_conjecture"
    assert out.get("coefficients") is None


def test_run_dispatches_both_ops_when_unavailable(monkeypatch):
    absent(monkeypatch)
    for request in (
        {"op": "falsify", "variables": ["x"], "claim": "x > 0"},
        {"op": "integer_relation", "constants": ["Pi"]},
    ):
        assert wf.run(request)["verdict"] == "unavailable"
    with pytest.raises(ValueError):
        wf.run({"op": "nope"})


# --- genuine counterexample, independently confirmed ------------------------


def test_genuine_counterexample_is_reverified_and_reported(monkeypatch):
    """x**2 != 2*x is false at x = 2, and our own recheck confirms it."""
    stub(monkeypatch, "{{x -> 2}}")
    out = wf.falsify({"variables": ["x"], "claim": "x ** 2 != 2 * x"})
    assert out["verdict"] == "counterexample"
    assert out["refuted"] is True
    assert out["independently_verified"] is True
    assert out["assignment"] == {"x": "2"}
    assert out["rejected_witnesses"] == []


def test_rational_counterexample_uses_exact_arithmetic(monkeypatch):
    """x/2 is an integer" fails at x = 3/2; the check must stay exact."""
    stub(monkeypatch, "{{x -> 3/2}}")
    out = wf.falsify({
        "variables": ["x"], "claim": "x * 2 == 3 * x", "domain": "Rationals",
    })
    assert out["verdict"] == "counterexample"
    assert out["assignment"] == {"x": "3/2"}
    assert out["assignment_numerator_denominator"] == {"x": [3, 2]}


def test_witness_violating_assumptions_is_not_a_counterexample(monkeypatch):
    """x > 0 => x >= 1 over the integers is not refuted by x = 0."""
    stub(monkeypatch, "{{x -> 0}}")
    out = wf.falsify({
        "variables": ["x"], "claim": "x >= 1", "assumptions": "x > 0",
    })
    assert out["verdict"] == "inconclusive"
    assert out["refuted"] is False
    assert "assumptions" in out["rejected_witnesses"][0]["reason"]


# --- THE important test: a bogus witness must be caught and discarded --------


def test_bogus_counterexample_is_caught_and_discarded(monkeypatch):
    """The oracle lies: it claims x = 3 refutes x**2 >= 0. It does not.

    Our independent exact recheck evaluates the ORIGINAL claim at x = 3, gets
    True, and therefore must DISCARD the witness. Reporting a refutation here
    would be the unsound failure this whole module exists to prevent.
    """
    stub(monkeypatch, "{{x -> 3}}")
    out = wf.falsify({"variables": ["x"], "claim": "x ** 2 >= 0"})
    assert out["verdict"] != "counterexample"
    assert out["verdict"] == "inconclusive"
    assert out["refuted"] is False
    assert out["independently_verified"] is False
    assert "assignment" not in out  # the bogus witness is NOT reported as one
    assert out["rejected_witnesses"] == [
        {"assignment": {"x": "3"}, "reason": "claim is TRUE at the proposed witness"}
    ]


def test_bogus_multivariable_witness_discarded(monkeypatch):
    stub(monkeypatch, "{{a -> 2, b -> 3}}")
    out = wf.falsify({
        "variables": ["a", "b"], "claim": "(a + b) ** 2 == a**2 + 2*a*b + b**2",
    })
    assert out["verdict"] == "inconclusive"
    assert out["refuted"] is False


def test_inexact_witness_is_discarded(monkeypatch):
    """A machine-precision real cannot be rationalized soundly, so it is dropped."""
    stub(monkeypatch, "{{x -> 1.4142135623730951`}}")
    out = wf.falsify({
        "variables": ["x"], "claim": "x * x != 2", "domain": "Reals",
    })
    assert out["verdict"] != "counterexample"
    assert out["refuted"] is False


def test_float_literal_claim_refuses_to_recheck(monkeypatch):
    """We decline to recheck in floating point rather than risk a wrong call.

    ``wl_claim`` is supplied so the WL-side translation is bypassed and the
    recheck guard itself is what rejects the witness.
    """
    stub(monkeypatch, "{{x -> 1}}")
    out = wf.falsify({
        "variables": ["x"], "claim": "x * 0.1 != 0.5", "wl_claim": "x*(1/10) != 1/2",
    })
    assert out["verdict"] == "inconclusive"
    assert out["refuted"] is False
    assert "exactly checkable" in out["rejected_witnesses"][0]["reason"]


def test_untranslatable_claim_is_inconclusive(monkeypatch):
    stub(monkeypatch, "{}")
    out = wf.falsify({"variables": ["x"], "claim": "math.sqrt(x) > 0"})
    assert out["verdict"] == "inconclusive"
    assert out["refuted"] is False


# --- the asymmetry: nothing found proves nothing ----------------------------


def test_no_counterexample_is_never_a_pass(monkeypatch):
    stub(monkeypatch, "{}")
    out = wf.falsify({"variables": ["x"], "claim": "x ** 2 >= 0"})
    assert out["verdict"] == "no_counterexample_found"
    assert out["refuted"] is False
    assert out["proved"] is False
    assert out["proves"] is None
    assert out["search_exhausted"] is False
    assert out["trusted"] is False
    # No positive vocabulary anywhere in the verdict surface.
    banned = ("verified", "pass", "holds", "proved_true", "valid", "ok")
    assert out["verdict"] not in banned
    assert "NOT verification" in out["note"]


def test_no_positive_verdict_constant_exists():
    """The module deliberately has no verdict meaning 'the claim is true'."""
    verdicts = {
        value for name, value in vars(wf).items()
        if name.startswith("VERDICT_") and isinstance(value, str)
    }
    assert verdicts == {
        "counterexample", "no_counterexample_found", "inconclusive",
        "unavailable", "candidate_relation", "no_relation_found",
    }


def test_bound_and_timeout_are_reported(monkeypatch):
    stub(monkeypatch, "{}")
    out = wf.falsify({
        "variables": ["x"], "claim": "x >= 0",
        "max_instances": 2, "timeout_seconds": 5.0,
    })
    assert out["bound"] == {"max_instances": 2, "timeout_seconds": 5.0}
    assert out["timeout_hit"] is False

    stub(monkeypatch, None, ok=False, error="kernel timeout after 5s")
    out = wf.falsify({"variables": ["x"], "claim": "x >= 0"})
    assert out["timeout_hit"] is True
    assert out["verdict"] == "inconclusive"
    assert out["refuted"] is False


# --- integer relation: conjecture only --------------------------------------


def test_integer_relation_is_marked_unproved(monkeypatch):
    stub(monkeypatch, "{1, 0, -2}")
    out = wf.integer_relation({"constants": ["Log[4]", "1", "Log[2]"]})
    assert out["verdict"] == "candidate_relation"
    assert out["coefficients"] == [1, 0, -2]
    assert out["proved"] is False
    assert out["trusted"] is False
    assert out["status"] == "unproved_conjecture"
    assert "CONJECTURED, unproved" in out["relation"]
    assert "numerical coincidence" in out["note"]


def test_integer_relation_none_found(monkeypatch):
    stub(monkeypatch, "$Failed")
    out = wf.integer_relation({"constants": ["Pi", "1"]})
    assert out["verdict"] == "no_relation_found"
    assert out["coefficients"] is None
    assert out["proved"] is False


def test_integer_relation_wrong_arity_rejected(monkeypatch):
    """A coefficient list that does not match the input arity is not usable."""
    stub(monkeypatch, "{1, -2}")
    out = wf.integer_relation({"constants": ["Pi", "1", "E"]})
    assert out["verdict"] == "no_relation_found"


# --- helper-level units -----------------------------------------------------


@pytest.mark.parametrize("token, expected", [
    ("3", Fraction(3)),
    ("-7", Fraction(-7)),
    ("3/2", Fraction(3, 2)),
    ("-(3/2)", Fraction(-3, 2)),
    ("Rational[5, 4]", Fraction(5, 4)),
    ("2.5", Fraction(5, 2)),
])
def test_parse_exact_accepts_exact_forms(token, expected):
    assert wf.parse_exact(token) == expected


@pytest.mark.parametrize("token", ["Sqrt[2]", "1.5`", "I", "", "1/0", "2*^10"])
def test_parse_exact_rejects_inexact_forms(token):
    with pytest.raises(wf.InexactError):
        wf.parse_exact(token)


def test_recheck_confirms_and_rejects():
    assert wf.recheck({"x": Fraction(2)}, "x ** 2 != 2 * x")["confirmed"] is True
    assert wf.recheck({"x": Fraction(3)}, "x ** 2 >= 0")["confirmed"] is False


def test_recheck_division_stays_exact():
    """Witnesses are coerced to Fraction so '/' cannot silently go to float.

    (10**17 + 1) / 3 * 3 == 10**17 + 1 is TRUE exactly but FALSE in binary
    floating point. Had we rechecked in floats we would have "confirmed" a
    counterexample that does not exist.
    """
    x = 10 ** 17 + 1
    assert (x / 3 * 3 == x) is False  # the float trap this guards against
    result = wf.recheck({"x": Fraction(x)}, "x / 3 * 3 == x")
    assert result["confirmed"] is False  # exactly equal, so the claim holds


def test_to_wolfram_translation():
    assert wf.to_wolfram("x ** 2 >= 0") == "((x ^ 2) >= 0)"
    assert wf.to_wolfram("x > 0 and y < 1") == "((x > 0) && (y < 1))"
    with pytest.raises(ValueError):
        wf.to_wolfram("math.sqrt(x) > 0")
