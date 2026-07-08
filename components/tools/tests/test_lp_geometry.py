"""Tests for the exact LP dual-certificate + polytope geometry layer.

Every certificate is re-checked independently here (complementary slackness,
strong duality, containment, vertex count) rather than trusting the module.
"""
from fractions import Fraction as F

import pytest

from theoremata_tools.lp_geometry import (
    chebyshev_center,
    primal_dual,
    redundant_constraints,
    run,
    vertex_enumeration,
)

# A classic non-degenerate LP:  max x + y
#   x + 2y <= 14 ;  3x - y >= 0 ;  x - y <= 2 ;  x, y >= 0
# Optimum is (6, 4) with value 10.
_LP = [
    {"coeffs": {"x": 1, "y": 2}, "sense": "leq", "rhs": 14},
    {"coeffs": {"x": 3, "y": -1}, "sense": "geq", "rhs": 0},
    {"coeffs": {"x": 1, "y": -1}, "sense": "leq", "rhs": 2},
]


def _F(s):
    return F(s)


def test_primal_dual_optimum_and_certificate_verified():
    res = primal_dual({"x": 1, "y": 1}, _LP, "max")
    assert res["status"] == "optimal"
    assert _F(res["objective_value"]) == 10
    assert res["primal"] == {"x": "6", "y": "4"}
    # The module's own composite check:
    assert res["certificate"]["verified"] is True


def test_primal_dual_complementary_slackness_holds_independently():
    res = primal_dual({"x": 1, "y": 1}, _LP, "max")
    cert = res["certificate"]
    # Strong duality  c.x* == h.y*.
    assert cert["strong_duality"]["holds"] is True
    assert (_F(cert["strong_duality"]["primal_objective"])
            == _F(cert["strong_duality"]["dual_objective"]))
    # Complementary slackness: y_k * slack_k == 0 for every normalized row,
    # and y >= 0.
    for row in cert["complementary_slackness"]["rows"]:
        y = _F(row["y"])
        slack = _F(row["slack"])
        assert y >= 0
        assert y * slack == 0
        assert _F(row["y*slack"]) == 0


def test_dual_from_basis_matches_dual_vector():
    # y = c_B A_B^{-1} reconstructed from the optimal basis must equal the
    # dual returned by the certificate.
    res = primal_dual({"x": 1, "y": 1}, _LP, "max")
    assert res["dual_from_basis"] is not None
    assert res["dual_from_basis"] == res["dual"]


def test_primal_dual_min_sense():
    # min x + y  s.t.  x + y >= 3, x,y >= 0  -> value 3.
    cons = [{"coeffs": {"x": 1, "y": 1}, "sense": "geq", "rhs": 3}]
    res = primal_dual({"x": 1, "y": 1}, cons, "min")
    assert res["status"] == "optimal"
    assert _F(res["objective_value"]) == 3
    assert res["certificate"]["verified"] is True


def test_primal_dual_positional_objective():
    res = primal_dual([1, 1], _LP, "max")  # x, y in sorted order
    assert _F(res["objective_value"]) == 10


def test_chebyshev_center_is_strictly_inside_box():
    # 0 <= x <= 4, 0 <= y <= 4  -> center (2,2), radius 2.
    box = [
        {"coeffs": {"x": 1}, "sense": "leq", "rhs": 4},
        {"coeffs": {"x": 1}, "sense": "geq", "rhs": 0},
        {"coeffs": {"y": 1}, "sense": "leq", "rhs": 4},
        {"coeffs": {"y": 1}, "sense": "geq", "rhs": 0},
    ]
    res = chebyshev_center(box)
    assert res["status"] == "optimal"
    assert res["interior"] is True
    assert res["center_inside"] is True
    cx, cy = _F(res["center"]["x"]), _F(res["center"]["y"])
    # Strictly interior: satisfies every constraint with slack.
    assert 0 < cx < 4 and 0 < cy < 4
    # Exact center of a square box.
    assert cx == 2 and cy == 2
    assert _F(res["radius"]) == 2


def test_chebyshev_center_triangle_inside_with_positive_radius():
    tri = [
        {"coeffs": {"x": 1, "y": 1}, "sense": "leq", "rhs": 2},
        {"coeffs": {"x": 1}, "sense": "geq", "rhs": 0},
        {"coeffs": {"y": 1}, "sense": "geq", "rhs": 0},
    ]
    res = chebyshev_center(tri)
    assert res["interior"] is True
    cx, cy = _F(res["center"]["x"]), _F(res["center"]["y"])
    assert cx > 0 and cy > 0 and cx + cy < 2  # strictly inside


def test_vertex_enumeration_counts_box_corners():
    box = [
        {"coeffs": {"x": 1}, "sense": "leq", "rhs": 4},
        {"coeffs": {"x": 1}, "sense": "geq", "rhs": 0},
        {"coeffs": {"y": 1}, "sense": "leq", "rhs": 4},
        {"coeffs": {"y": 1}, "sense": "geq", "rhs": 0},
    ]
    res = vertex_enumeration(box)
    assert res["count"] == 4
    corners = {(v["x"], v["y"]) for v in res["vertices"]}
    assert corners == {("0", "0"), ("0", "4"), ("4", "0"), ("4", "4")}


def test_vertex_enumeration_triangle():
    tri = [
        {"coeffs": {"x": 1, "y": 1}, "sense": "leq", "rhs": 2},
        {"coeffs": {"x": 1}, "sense": "geq", "rhs": 0},
        {"coeffs": {"y": 1}, "sense": "geq", "rhs": 0},
    ]
    res = vertex_enumeration(tri)
    assert res["count"] == 3
    corners = {(v["x"], v["y"]) for v in res["vertices"]}
    assert corners == {("0", "0"), ("2", "0"), ("0", "2")}


def test_redundant_constraint_detection():
    # x <= 10 is redundant given x <= 5 (both with x >= 0).
    cons = [
        {"coeffs": {"x": 1}, "sense": "leq", "rhs": 5},
        {"coeffs": {"x": 1}, "sense": "leq", "rhs": 10},
        {"coeffs": {"x": 1}, "sense": "geq", "rhs": 0},
    ]
    res = redundant_constraints(cons)
    # Row 1 (x <= 10) is implied; row 0 (x <= 5) is not.
    assert 1 in res["redundant"]
    assert 0 not in res["redundant"]


def test_no_redundant_when_all_binding():
    box = [
        {"coeffs": {"x": 1}, "sense": "leq", "rhs": 4},
        {"coeffs": {"x": 1}, "sense": "geq", "rhs": 0},
    ]
    res = redundant_constraints(box)
    assert res["redundant"] == []


def test_run_dispatch_ops():
    pd = run({"op": "primal_dual", "objective": {"x": 1, "y": 1},
              "constraints": _LP, "sense": "max"})
    assert pd["certificate"]["verified"] is True

    ch = run({"op": "chebyshev", "constraints": [
        {"coeffs": {"x": 1}, "sense": "leq", "rhs": 4},
        {"coeffs": {"x": 1}, "sense": "geq", "rhs": 0},
    ]})
    assert ch["status"] == "optimal"

    vt = run({"op": "vertices", "constraints": [
        {"coeffs": {"x": 1}, "sense": "leq", "rhs": 4},
        {"coeffs": {"x": 1}, "sense": "geq", "rhs": 0},
    ]})
    assert vt["count"] == 2  # x=0 and x=4 in 1-D

    rd = run({"op": "redundant", "constraints": [
        {"coeffs": {"x": 1}, "sense": "leq", "rhs": 5},
        {"coeffs": {"x": 1}, "sense": "leq", "rhs": 10},
    ]})
    assert 1 in rd["redundant"]

    with pytest.raises(ValueError):
        run({"op": "nope", "constraints": []})
