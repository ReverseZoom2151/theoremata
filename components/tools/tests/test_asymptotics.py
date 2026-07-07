"""Tests for the asymptotic (log_linarith) tactic."""
from theoremata_tools.asymptotics import asymptotic_feasibility, prove_asymptotic


def rel(lhs, r, rhs):
    return {"lhs": lhs, "rel": r, "rhs": rhs}


X, Y, Z = {"x": 1}, {"y": 1}, {"z": 1}


def test_transitivity_strict():
    # x >> y and y >> z entail x >> z
    hyps = [rel(X, ">>", Y), rel(Y, ">>", Z)]
    assert prove_asymptotic(hyps, rel(X, ">>", Z))["proved"] is True


def test_transitivity_same_order():
    # x ~ y and y ~ z entail x ~ z (equality goal: both strict negations fail)
    hyps = [rel(X, "~", Y), rel(Y, "~", Z)]
    out = prove_asymptotic(hyps, rel(X, "~", Z))
    assert out["proved"] is True
    assert len(out["certificate"]) == 2  # one witness per disjunct


def test_bounded_from_strict():
    # x << y entails x <~ y
    assert prove_asymptotic([rel(X, "<<", Y)], rel(X, "<~", Y))["proved"] is True


def test_contradictory_system_infeasible():
    # x << y and x >~ y cannot both hold
    out = asymptotic_feasibility([rel(X, "<<", Y), rel(X, ">~", Y)])
    assert out["feasible"] is False
    assert out["certificate"]  # a Farkas witness is returned


def test_consistent_system_feasible():
    out = asymptotic_feasibility([rel(X, ">>", Y), rel(Y, ">>", Z)])
    assert out["feasible"] is True
    assert set(out["variables"]) == {"x", "y", "z"}
    assert "log_model" in out


def test_invalid_entailment_not_proved():
    # x >> y says nothing about z, so x >> z does not follow
    assert prove_asymptotic([rel(X, ">>", Y)], rel(X, ">>", Z))["proved"] is False


def test_monomial_powers():
    # given x >> 1 (log x > 0), prove x^2 >> x  (2 log x - log x = log x > 0)
    hyps = [rel({"x": 1}, ">>", {})]
    goal = rel({"x": 2}, ">>", {"x": 1})
    assert prove_asymptotic(hyps, goal)["proved"] is True


def test_monomial_ratio():
    # x ~ y  entails  x / y ~ 1   (log x - log y = 0)
    hyps = [rel({"x": 1}, "~", {"y": 1})]
    goal = rel({"x": 1, "y": -1}, "~", {})
    assert prove_asymptotic(hyps, goal)["proved"] is True


def test_strict_order_does_not_prove_equality():
    # x >> y is consistent with x > y, so it does not entail x ~ y
    assert prove_asymptotic([rel(X, ">>", Y)], rel(X, "~", Y))["proved"] is False


def test_self_relation_strict_infeasible():
    # x << x is false: 0 < 0
    assert asymptotic_feasibility([rel(X, "<<", X)])["feasible"] is False


def test_exact_fractional_exponents():
    # sqrt(x) ~ y  and  x ~ z^? ; check fractional exponents survive exactly.
    # from x^(1/2) ~ y prove x ~ y^2
    hyps = [rel({"x": "1/2"}, "~", {"y": 1})]
    goal = rel({"x": 1}, "~", {"y": 2})
    assert prove_asymptotic(hyps, goal)["proved"] is True
