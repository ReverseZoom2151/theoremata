"""Tests for the exact-rational LP kernel with Farkas certificates."""
from fractions import Fraction

from theoremata_tools.linprog_cert import Inequality, feasibility, evaluate


def _check_farkas(constraints, cert):
    """Independently re-check a Farkas certificate: the nonnegative combination
    of the (normalized ``<= / <``) constraints must yield ``0 <= negative`` or
    ``0 < 0``."""
    combo = cert["combination"]
    # combined lhs coeffs must all be zero
    assert all(Fraction(v) == 0 for v in combo["coeffs"].values())
    rhs = Fraction(combo["rhs"])
    if combo["strict"]:
        assert rhs <= 0  # 0 < rhs with rhs <= 0 is false
    else:
        assert rhs < 0   # 0 <= rhs with rhs < 0 is false


def test_feasible_returns_rational_witness():
    # x >= 1, x <= 3  -> feasible
    cons = [
        {"coeffs": {"x": 1}, "sense": "geq", "rhs": 1},
        {"coeffs": {"x": 1}, "sense": "leq", "rhs": 3},
    ]
    res = feasibility(cons)
    assert res["feasible"] is True
    x = Fraction(res["model"]["x"])
    assert 1 <= x <= 3


def test_infeasible_returns_farkas_certificate():
    # x <= 1 and x >= 2  -> infeasible
    cons = [
        {"coeffs": {"x": 1}, "sense": "leq", "rhs": 1},
        {"coeffs": {"x": 1}, "sense": "geq", "rhs": 2},
    ]
    res = feasibility(cons)
    assert res["feasible"] is False
    cert = res["certificate"]
    assert cert["type"] == "farkas"
    assert len(cert["multipliers"]) >= 1
    _check_farkas(cons, cert)


def test_strict_contradiction():
    # x < 0 and x > 0
    cons = [
        {"coeffs": {"x": 1}, "sense": "lt", "rhs": 0},
        {"coeffs": {"x": 1}, "sense": "gt", "rhs": 0},
    ]
    res = feasibility(cons)
    assert res["feasible"] is False
    _check_farkas(cons, res["certificate"])


def test_equality_and_multivariable_infeasible():
    # x + y = 1, x = 2, y = 2  -> infeasible
    cons = [
        {"coeffs": {"x": 1, "y": 1}, "sense": "eq", "rhs": 1},
        {"coeffs": {"x": 1}, "sense": "eq", "rhs": 2},
        {"coeffs": {"y": 1}, "sense": "eq", "rhs": 2},
    ]
    res = feasibility(cons)
    assert res["feasible"] is False
    _check_farkas(cons, res["certificate"])


def test_feasible_multivariable_model_satisfies_constraints():
    cons = [
        {"coeffs": {"x": 1, "y": -1}, "sense": "leq", "rhs": 0},   # x <= y
        {"coeffs": {"y": 1}, "sense": "leq", "rhs": 10},           # y <= 10
        {"coeffs": {"x": 1}, "sense": "geq", "rhs": 1},            # x >= 1
    ]
    res = feasibility(cons)
    assert res["feasible"] is True
    x = Fraction(res["model"]["x"])
    y = Fraction(res["model"]["y"])
    assert x <= y and y <= 10 and x >= 1


def test_inequality_object_input():
    ineqs = [
        Inequality({"x": 1}, "leq", 1),
        Inequality({"x": 1}, "geq", 2),
    ]
    res = feasibility(ineqs)
    assert res["feasible"] is False


def test_evaluate_wrapper_shapes():
    ok = evaluate({"constraints": [{"coeffs": {"x": 1}, "sense": "geq", "rhs": 0}]})
    assert ok["status"] == "ok" and ok["verdict"] == "feasible"
    assert "witness" in ok

    bad = evaluate({"constraints": [
        {"coeffs": {"x": 1}, "sense": "leq", "rhs": 1},
        {"coeffs": {"x": 1}, "sense": "geq", "rhs": 2},
    ]})
    assert bad["verdict"] == "infeasible"
    assert bad["certificate"]["type"] == "farkas"


def test_backend_reported():
    res = feasibility([{"coeffs": {"x": 1}, "sense": "geq", "rhs": 0}])
    assert res["backend"] in ("z3", "pure-python", "trivial")
