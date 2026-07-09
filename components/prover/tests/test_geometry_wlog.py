"""Tests for the WLOG frame-normalization tactic + invariance-lemma registry
(:mod:`theoremata_tools.geometry_wlog`).

Contract:
  * normalizing a triangle to the canonical frame (A at origin, B on +x axis,
    |AB| = 1) preserves a scale-invariant predicate value (collinearity and a
    distance-ratio) exactly -- verified via ``predicate_value``;
  * the registry REFUSES to normalize when a non-invariant predicate is present
    (soundness): an absolute ``distance`` goal blocks the scaling normalization;
    an oriented ``signed_area`` goal blocks reflection;
  * dimension drop applies only when justified (collinear points -> R^1; a
    genuine triangle stays in R^2);
  * determinism: identical inputs give identical outputs.

Run from the repo root::

    python -m pytest components/prover/tests/test_geometry_wlog.py -q
"""
from __future__ import annotations

import pytest

pytest.importorskip("sympy")

import sympy as sp

from theoremata_tools import geometry_wlog as wl


# A generic (numeric) triangle used across the frame-pinning tests.
TRI = {"A": [2, 1], "B": [6, 4], "C": [3, 5]}


# --------------------------------------------------------------------------- #
# 1. Canonical frame preserves invariant predicate values.
# --------------------------------------------------------------------------- #
def test_normalize_triangle_preserves_collinearity_and_ratio():
    # Add a point D on line A-B so collinear(A,B,D) holds (value 0).
    pts = dict(TRI)
    pts["D"] = [4, sp.Rational(5, 2)]  # midpoint of A,B -> collinear
    spec = {"origin": "A", "x_axis": "B", "unit": "B",
            "predicates": ["collinear", "dist_ratio"]}

    res = wl.normalize_frame(pts, spec)
    assert res["refused"] is False
    new = res["points"]

    # collinear(A,B,D) is invariant and must stay exactly 0 before and after.
    before = wl.predicate_value("collinear", ["A", "B", "D"], pts)
    after = wl.predicate_value("collinear", ["A", "B", "D"], new)
    assert before == 0
    assert after == 0

    # A distance ratio |AC|/|AB| is scale-free -> unchanged by the full
    # translate+rotate+scale normalization.
    r_before = wl.predicate_value("dist_ratio", ["A", "C", "A", "B"], pts)
    r_after = wl.predicate_value("dist_ratio", ["A", "C", "A", "B"], new)
    assert sp.simplify(r_before - r_after) == 0

    # The frame really is canonical: A at origin, B at (1, 0).
    assert new["A"] == [0, 0]
    assert new["B"] == [1, 0]


def test_transformation_is_reported_and_orientation_preserving():
    res = wl.normalize_frame(TRI, {"origin": "A", "x_axis": "B",
                                   "predicates": ["collinear"]})
    tr = res["transformation"]
    assert tr["form"] == "x' = M @ (x - t)"
    assert tr["t"] == [2, 1]                # translation is the origin point A
    assert tr["orientation"] == "preserving"  # pure rotation, det = +1
    groups = {s["group"] for s in tr["steps"]}
    assert groups == {"translation", "rotation"}


# --------------------------------------------------------------------------- #
# 2. Soundness gate: refuse on non-invariant predicates.
# --------------------------------------------------------------------------- #
def test_registry_refuses_scaling_under_absolute_distance():
    # An absolute-distance goal is NOT scale-invariant; requesting `unit`
    # (scaling) must be refused.
    spec = {"origin": "A", "x_axis": "B", "unit": "B",
            "predicates": ["collinear", "distance"]}
    res = wl.normalize_frame(TRI, spec)
    assert res["refused"] is True
    assert any(v["group"] == "scaling" and v["predicate"] == "distance"
               for v in res["violations"])
    # Configuration left untouched.
    assert res["points"] == TRI


def test_registry_allows_isometry_under_absolute_distance():
    # Same distance goal, but only translation+rotation (isometry) requested:
    # that IS invariant, so it is allowed.
    spec = {"origin": "A", "x_axis": "B", "predicates": ["distance"]}
    res = wl.normalize_frame(TRI, spec)
    assert res["refused"] is False


def test_registry_refuses_reflection_under_signed_area():
    # signed_area is orientation-dependent: reflection flips its sign, so a
    # reflection normalization must be refused.
    pts = {"A": [0, 0], "B": [1, 0], "C": [0, -1]}  # C below axis
    spec = {"reflect_above": "C", "predicates": ["signed_area"]}
    res = wl.normalize_frame(pts, spec)
    assert res["refused"] is True
    assert any(v["group"] == "reflection" for v in res["violations"])


def test_unknown_predicate_fails_closed():
    # Unknown predicate -> invariant under nothing -> every normalization refused.
    res = wl.normalize_frame(TRI, {"origin": "A", "predicates": ["mystery_pred"]})
    assert res["refused"] is True


def test_gate_helper_direct():
    g_ok = wl.gate(["collinear", "dist_ratio"], ["scaling", "rotation"])
    assert g_ok["ok"] is True
    g_bad = wl.gate(["distance"], ["scaling"])
    assert g_bad["ok"] is False
    assert ("scaling", "distance") in g_bad["violations"]


# --------------------------------------------------------------------------- #
# 3. Dimension drop is guarded by the R^k theorem.
# --------------------------------------------------------------------------- #
def test_dimension_drop_collinear_points():
    line = {"A": [0, 0], "B": [1, 1], "C": [2, 2]}  # all on y = x
    assert wl.min_embedding_dimension(line) == 1
    res = wl.dimension_drop(line, ["collinear"], ambient=2)
    assert res["justified"] is True
    assert res["target_dimension"] == 1


def test_dimension_drop_triangle_stays_2d():
    res = wl.dimension_drop(TRI, ["collinear"], ambient=2)
    assert res["spanned_dimension"] == 2
    assert res["target_dimension"] == 2


def test_dimension_drop_refused_for_non_invariant_pred():
    line = {"A": [0, 0], "B": [1, 1], "C": [2, 2]}
    res = wl.dimension_drop(line, ["mystery_pred"], ambient=2)
    assert res["justified"] is False
    assert res["violations"]


# --------------------------------------------------------------------------- #
# 4. Worker entrypoint + determinism.
# --------------------------------------------------------------------------- #
def test_run_wlog_normalize_op_and_harvests_goal_pred():
    request = {
        "op": "wlog_normalize",
        "points": TRI,
        "spec": {"origin": "A", "x_axis": "B", "unit": "B"},
        "goal": {"pred": "dist_ratio", "points": ["A", "C", "A", "B"]},
    }
    out = wl.run(request)
    assert out["op"] == "geometry_wlog"
    assert out["sub_op"] == "wlog_normalize"
    assert out["refused"] is False
    assert out["points"]["A"] == [0, 0] and out["points"]["B"] == [1, 0]
    # The gate ran using the harvested goal predicate.
    assert out["invariance"]["gated"] is True
    # Output is JSON-friendly (no sympy objects leaked).
    assert "points_expr" not in out


def test_run_refuses_via_goal_pred():
    request = {
        "op": "wlog_normalize",
        "points": TRI,
        "spec": {"origin": "A", "x_axis": "B", "unit": "B"},
        "goal": {"pred": "distance", "points": ["A", "B"]},
    }
    out = wl.run(request)
    assert out["refused"] is True


def test_determinism():
    spec = {"origin": "A", "x_axis": "B", "unit": "B",
            "predicates": ["collinear", "dist_ratio"]}
    a = wl.normalize_frame(TRI, spec)
    b = wl.normalize_frame(TRI, spec)
    assert a["points"] == b["points"]
    assert a["transformation"] == b["transformation"]
