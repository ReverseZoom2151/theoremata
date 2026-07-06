"""Tests for the exact rational feasibility checker."""
from __future__ import annotations

from fractions import Fraction

from theoremata_tools.feasibility import feasibility


def c(coeffs, sense, rhs):
    return {"coeffs": coeffs, "sense": sense, "rhs": rhs}


def _check_certificate(cert):
    """A Farkas certificate must combine, with nonnegative multipliers, to a
    contradiction: every variable's aggregate coefficient is 0 and the aggregate
    rhs is < 0 (or <= 0 when some strict row carries positive weight)."""
    agg: dict[str, Fraction] = {}
    total = Fraction(0)
    any_strict = False
    assert cert, "certificate must be non-empty"
    for row in cert:
        m = Fraction(row["multiplier"])
        assert m >= 0
        for var, val in row["coeffs"].items():
            agg[var] = agg.get(var, Fraction(0)) + m * Fraction(val)
        total += m * Fraction(row["rhs"])
        if row["strict"] and m > 0:
            any_strict = True
    assert all(v == 0 for v in agg.values()), f"coeffs did not cancel: {agg}"
    assert total < 0 or (any_strict and total <= 0), f"rhs not contradictory: {total}"


def test_feasible_bounded():
    r = feasibility([c({"x": 1}, ">=", 0), c({"x": 1}, "<=", 1)])
    assert r["feasible"] is True
    x = Fraction(r["model"]["x"])
    assert 0 <= x <= 1


def test_feasible_equality_chain():
    # x = 1 ; y - x = 1  ->  x = 1, y = 2
    r = feasibility([c({"x": 1}, "=", 1), c({"x": -1, "y": 1}, "=", 1)])
    assert r["feasible"] is True
    assert Fraction(r["model"]["x"]) == 1
    assert Fraction(r["model"]["y"]) == 2


def test_infeasible_strict_edge():
    # x < 1 and x >= 1
    r = feasibility([c({"x": 1}, "<", 1), c({"x": 1}, ">=", 1)])
    assert r["feasible"] is False
    _check_certificate(r["certificate"])


def test_feasible_closed_corner():
    # x <= 1 and x >= 1  ->  x = 1
    r = feasibility([c({"x": 1}, "<=", 1), c({"x": 1}, ">=", 1)])
    assert r["feasible"] is True
    assert Fraction(r["model"]["x"]) == 1


def test_feasible_three_vars():
    # x>=0, y>=0, x+y<=2, x<=y  -> some point satisfying all
    cons = [
        c({"x": 1}, ">=", 0),
        c({"y": 1}, ">=", 0),
        c({"x": 1, "y": 1}, "<=", 2),
        c({"x": 1, "y": -1}, "<=", 0),
    ]
    r = feasibility(cons)
    assert r["feasible"] is True
    x = Fraction(r["model"]["x"])
    y = Fraction(r["model"]["y"])
    assert x >= 0 and y >= 0 and x + y <= 2 and x <= y


def test_infeasible_two_var_with_certificate():
    # x + y <= 1 and x + y >= 3
    r = feasibility([c({"x": 1, "y": 1}, "<=", 1), c({"x": 1, "y": 1}, ">=", 3)])
    assert r["feasible"] is False
    _check_certificate(r["certificate"])


def test_exact_fractions():
    # x >= 1/3 and x <= 1/3  ->  x = 1/3 exactly
    r = feasibility([c({"x": 1}, ">=", "1/3"), c({"x": 1}, "<=", "1/3")])
    assert r["feasible"] is True
    assert r["model"]["x"] == "1/3"


def test_exact_fraction_from_scaling():
    # 3x = 1  ->  x = 1/3, no float drift
    r = feasibility([c({"x": 3}, "=", 1)])
    assert r["feasible"] is True
    assert Fraction(r["model"]["x"]) == Fraction(1, 3)
    assert r["model"]["x"] == "1/3"


def test_constant_only_infeasible():
    r = feasibility([c({}, "<=", -1)])
    assert r["feasible"] is False
    _check_certificate(r["certificate"])


def test_constant_only_feasible():
    r = feasibility([c({}, "<=", 1)])
    assert r["feasible"] is True
    assert r["model"] == {}


def test_unbounded_feasible():
    # only an upper bound; region is unbounded below but still feasible
    r = feasibility([c({"x": 1}, "<=", 5)])
    assert r["feasible"] is True
    assert Fraction(r["model"]["x"]) <= 5


def test_decimal_inputs_are_exact():
    # 0.5 parsed exactly as 1/2
    r = feasibility([c({"x": 1}, "=", 0.5)])
    assert r["feasible"] is True
    assert r["model"]["x"] == "1/2"
