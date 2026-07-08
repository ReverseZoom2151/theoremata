"""Tests for the geometry reasoning vertical.

Covers the three contractual scenarios: a true theorem proved by the deductive
chainer, a numeric check that passes, and a false conjecture rejected by the
numeric falsifier -- plus schema/edge coverage for the ``run`` dispatcher.

Pure standard library; no numpy, no network. Run from repo root::

    python -m pytest components/prover/tests/test_geometry.py -x -q
"""
from __future__ import annotations

import pytest

from theoremata_tools import geometry


SEED = 1234567


# --------------------------------------------------------------------------- #
# 1. A true theorem proved by the forward chainer.
# --------------------------------------------------------------------------- #
def test_prove_perpendicular_transport_chain():
    """AB _|_ CD, CD _|_ EF  =>  AB || EF  (two perpendiculars to a common line),
    then AB || EF, EF || GH  =>  AB || GH  -- a genuine multi-step derivation."""
    hyps = [
        {"pred": "perpendicular", "points": ["A", "B", "C", "D"]},
        {"pred": "perpendicular", "points": ["C", "D", "E", "F"]},
        {"pred": "parallel", "points": ["E", "F", "G", "H"]},
    ]
    goal = {"pred": "parallel", "points": ["A", "B", "G", "H"]}
    result = geometry.deductive_prove(hyps, goal)
    assert result["proved"] is True
    # Requires at least the perp-perp=>parallel step and one transitivity step.
    assert len(result["derivation"]) >= 2
    rules = {step["rule"] for step in result["derivation"]}
    assert "two-perpendiculars-to-a-line-are-parallel" in rules
    assert "parallelism-is-transitive" in rules


def test_prove_midpoint_gives_equal_segments():
    hyps = [{"pred": "midpoint", "points": ["M", "A", "B"]}]
    goal = {"pred": "cong", "points": ["M", "A", "M", "B"]}
    result = geometry.deductive_prove(hyps, goal)
    assert result["proved"] is True
    assert result["derivation"][-1]["rule"].startswith("midpoint")


def test_prove_rejects_unreachable_goal():
    """Sound engine must NOT claim something outside its closure."""
    hyps = [{"pred": "midpoint", "points": ["M", "A", "B"]}]
    goal = {"pred": "perpendicular", "points": ["A", "B", "C", "D"]}
    result = geometry.deductive_prove(hyps, goal)
    assert result["proved"] is False
    assert result["derivation"] is None


# --------------------------------------------------------------------------- #
# 2. A numeric check that passes (a true theorem about the diagram).
# --------------------------------------------------------------------------- #
def test_numeric_check_midpoint_equal_segments_true():
    """M = midpoint(A,B) implies |MA| = |MB| in every realization."""
    construction = [
        {"op": "free", "point": "A"},
        {"op": "free", "point": "B"},
        {"op": "midpoint", "point": "M", "of": ["A", "B"]},
    ]
    goal = {"pred": "cong", "points": ["M", "A", "M", "B"]}
    res = geometry.numeric_check(construction, goal, seed=SEED, trials=30)
    assert res["holds"] is True
    assert res["trials_valid"] >= 25
    assert "counterexample" not in res


def test_numeric_check_thales_right_angle_true():
    """Thales: a point P on the circle with diameter AB sees AB at a right angle.
    Built via circumcenter O of A,B,P; here we place O and A freely, reflect for
    B (diameter), and take P on the circle by reflecting A over a random line
    through O -- guaranteeing |OP| = |OA| = |OB|. Then angle APB = 90."""
    construction = [
        {"op": "free", "point": "O"},
        {"op": "free", "point": "A"},
        {"op": "reflect_point", "point": "B", "of": "A", "center": "O"},
        {"op": "free", "point": "Q"},            # random direction anchor
        {"op": "reflect_line", "point": "P", "of": "A", "over": ["O", "Q"]},
    ]
    goal = {"pred": "perpendicular", "points": ["P", "A", "P", "B"]}
    res = geometry.numeric_check(construction, goal, seed=SEED, trials=25)
    assert res["holds"] is True


# --------------------------------------------------------------------------- #
# 3. A false conjecture rejected by the numeric falsifier.
# --------------------------------------------------------------------------- #
def test_falsify_false_conjecture():
    """FALSE: for a generic midpoint M of AB and a free C, claim A,B,C collinear.
    The falsifier must produce a counterexample realization."""
    construction = [
        {"op": "free", "point": "A"},
        {"op": "free", "point": "B"},
        {"op": "midpoint", "point": "M", "of": ["A", "B"]},
        {"op": "free", "point": "C"},
    ]
    goal = {"pred": "collinear", "points": ["A", "B", "C"]}
    out = geometry.run({
        "op": "falsify", "construction": construction, "goal": goal, "seed": SEED,
    })
    assert out["falsified"] is True
    assert out["counterexample"] is not None
    assert set(out["counterexample"]).issuperset({"A", "B", "C"})


def test_check_rejects_false_conjecture():
    """The same false conjecture: ``check`` must report holds=False."""
    construction = [
        {"op": "free", "point": "A"},
        {"op": "free", "point": "B"},
        {"op": "free", "point": "C"},
        {"op": "midpoint", "point": "M", "of": ["A", "C"]},
    ]
    goal = {"pred": "perpendicular", "points": ["A", "B", "A", "C"]}
    out = geometry.run({
        "op": "check", "construction": construction, "goal": goal, "seed": SEED,
    })
    assert out["holds"] is False
    assert "counterexample" in out


# --------------------------------------------------------------------------- #
# run() dispatch + schema coverage.
# --------------------------------------------------------------------------- #
def test_run_check_true_theorem():
    construction = [
        {"op": "free", "point": "A"},
        {"op": "free", "point": "B"},
        {"op": "midpoint", "point": "M", "of": ["A", "B"]},
    ]
    goal = {"pred": "collinear", "points": ["A", "M", "B"]}
    out = geometry.run({
        "op": "check", "construction": construction, "goal": goal, "seed": SEED,
    })
    assert out["op"] == "check"
    assert out["holds"] is True


def test_run_prove_dispatch():
    out = geometry.run({
        "op": "prove",
        "hypotheses": [{"pred": "midpoint", "points": ["M", "A", "B"]}],
        "goal": {"pred": "collinear", "points": ["A", "M", "B"]},
    })
    assert out["op"] == "prove"
    assert out["proved"] is True


def test_run_unknown_op_raises():
    with pytest.raises(ValueError):
        geometry.run({"op": "nonsense"})


def test_seed_determinism():
    construction = [
        {"op": "free", "point": "A"},
        {"op": "free", "point": "B"},
        {"op": "free", "point": "C"},
    ]
    goal = {"pred": "collinear", "points": ["A", "B", "C"]}
    a = geometry.run({"op": "falsify", "construction": construction,
                      "goal": goal, "seed": 42})
    b = geometry.run({"op": "falsify", "construction": construction,
                      "goal": goal, "seed": 42})
    assert a["counterexample"] == b["counterexample"]


def test_concyclic_true_via_circumcenter():
    """Four points equidistant from a common center are concyclic."""
    construction = [
        {"op": "free", "point": "O"},
        {"op": "free", "point": "A"},
        {"op": "free", "point": "L1"},
        {"op": "free", "point": "L2"},
        {"op": "free", "point": "L3"},
        {"op": "reflect_line", "point": "B", "of": "A", "over": ["O", "L1"]},
        {"op": "reflect_line", "point": "C", "of": "A", "over": ["O", "L2"]},
        {"op": "reflect_line", "point": "D", "of": "A", "over": ["O", "L3"]},
    ]
    goal = {"pred": "concyclic", "points": ["A", "B", "C", "D"]}
    res = geometry.numeric_check(construction, goal, seed=SEED, trials=20)
    assert res["holds"] is True
