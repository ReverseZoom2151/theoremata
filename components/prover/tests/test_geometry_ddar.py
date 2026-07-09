"""Tests for the DD+AR geometry engine (:mod:`theoremata_tools.geometry_ddar`).

Contractual scenarios (all offline, deterministic, seeded, stdlib-only core):

  * a theorem provable by **angle chasing / AR** that geometry.py's five rules
    CANNOT prove (directed-angle addition, and eqangle transitivity) -- proved
    with an explicit AR linear-combination certificate;
  * the **inscribed-angle** rule: concyclic => directed eqangle (a new sound DD
    rule feeding AR), with the directed identity cross-checked numerically;
  * a **DD-only** theorem (midpoint => collinear) still proved by the joint
    engine, with a derivation;
  * a length/ratio chase closed by AR (cong transitivity) with a certificate;
  * a **FALSE** conjecture rejected by the numeric falsify screen;
  * determinism given the seed;
  * stdlib-only core (an OPTIONAL numpy cross-check is gated behind importorskip).

Run from the repo root::

    python -m pytest components/prover/tests/test_geometry_ddar.py -x -q
"""
from __future__ import annotations

import math
from fractions import Fraction

import pytest

from theoremata_tools import geometry, geometry_ddar as dd


SEED = 20260709


# --------------------------------------------------------------------------- #
# 1. AR proves angle-chase theorems the five-rule chainer cannot.
# --------------------------------------------------------------------------- #
def test_ar_directed_angle_addition_certificate():
    """Directed-angle additivity: <AOB=<XPY and <BOC=<YPZ  =>  <AOC=<XPZ.

    Pure angle chasing: AR closes it by adding the two hypothesis relations.
    geometry.py has no angle rule at all, so it cannot reach this."""
    req = {
        "op": "prove",
        "hypotheses": [
            {"pred": "eqangle", "points": ["A", "O", "B", "X", "P", "Y"]},
            {"pred": "eqangle", "points": ["B", "O", "C", "Y", "P", "Z"]},
        ],
        "goal": {"pred": "eqangle", "points": ["A", "O", "C", "X", "P", "Z"]},
        "seed": SEED,
    }
    res = dd.run(req)
    assert res["proved"] is True
    assert res["falsified"] is False
    assert "ar_certificate" in res
    # certificate is the sum of the two hypothesis relations (coeff +1 each)
    coeffs = sorted(c["coeff"] for c in res["ar_certificate"])
    assert coeffs == ["1", "1"]
    assert len(res["ar_certificate"]) == 2


def test_five_rule_chainer_cannot_chase_angles():
    """Sanity anchor: geometry.deductive_prove cannot prove the same theorem
    (nor even eqangle transitivity) -- justifying the AR layer."""
    hyps = [
        {"pred": "eqangle", "points": ["A", "O", "B", "X", "P", "Y"]},
        {"pred": "eqangle", "points": ["B", "O", "C", "Y", "P", "Z"]},
    ]
    goal = {"pred": "eqangle", "points": ["A", "O", "C", "X", "P", "Z"]}
    assert geometry.deductive_prove(hyps, goal)["proved"] is False


def test_ar_eqangle_transitivity():
    """<1=<2 and <2=<3  =>  <1=<3, again beyond the five rules."""
    req = {
        "op": "prove",
        "hypotheses": [
            {"pred": "eqangle", "points": ["A", "O", "B", "X", "P", "Y"]},
            {"pred": "eqangle", "points": ["X", "P", "Y", "M", "N", "K"]},
        ],
        "goal": {"pred": "eqangle", "points": ["A", "O", "B", "M", "N", "K"]},
        "seed": SEED,
    }
    res = dd.run(req)
    assert res["proved"] is True
    assert "ar_certificate" in res
    assert geometry.deductive_prove(req["hypotheses"], req["goal"])["proved"] is False


def test_ar_length_ratio_chase_certificate():
    """cong is transitive via the AR *length* (log-length) system."""
    req = {
        "op": "prove",
        "hypotheses": [
            {"pred": "cong", "points": ["A", "B", "C", "D"]},
            {"pred": "cong", "points": ["C", "D", "E", "F"]},
        ],
        "goal": {"pred": "cong", "points": ["A", "B", "E", "F"]},
        "seed": SEED,
    }
    res = dd.run(req)
    assert res["proved"] is True
    assert "ar_certificate" in res
    assert res["ar_length_relations"] >= 2


# --------------------------------------------------------------------------- #
# 2. Inscribed-angle: a new sound DD rule (concyclic => directed eqangle) that
#    feeds AR. The directed identity is cross-checked numerically for soundness.
# --------------------------------------------------------------------------- #
def test_inscribed_angle_rule_proves_eqangle():
    req = {
        "op": "prove",
        "hypotheses": [{"pred": "concyclic", "points": ["A", "B", "C", "D"]}],
        "goal": {"pred": "eqangle", "points": ["A", "C", "B", "A", "D", "B"]},
        "seed": SEED,
    }
    res = dd.run(req)
    assert res["proved"] is True
    # reachable both as an AR relation and as a recorded DD derivation
    assert "ar_certificate" in res
    assert "derivation" in res
    # the five-rule engine cannot do this
    assert geometry.deductive_prove(req["hypotheses"], req["goal"])["proved"] is False


def test_inscribed_angle_directed_identity_is_sound():
    """Independent numeric witness that concyclic => directed <ACB = <ADB
    (mod pi). Four points on a circle at fixed angles; deterministic."""
    def slope(p, q):
        return math.atan2(q[1] - p[1], q[0] - p[0]) % math.pi

    pts = {name: (math.cos(a), math.sin(a))
           for name, a in zip("ABCD", (0.3, 1.1, 2.0, 3.5))}
    lhs = (slope(pts["C"], pts["B"]) - slope(pts["C"], pts["A"])) % math.pi
    rhs = (slope(pts["D"], pts["B"]) - slope(pts["D"], pts["A"])) % math.pi
    assert abs(lhs - rhs) < 1e-9


def test_ddar_interleave_two_circles():
    """DD feeds AR across two concyclic hypotheses (interleaving joint closure)."""
    req = {
        "op": "prove",
        "hypotheses": [
            {"pred": "concyclic", "points": ["A", "B", "C", "D"]},
            {"pred": "concyclic", "points": ["A", "B", "E", "F"]},
        ],
        "goal": {"pred": "eqangle", "points": ["A", "E", "B", "A", "F", "B"]},
        "seed": SEED,
    }
    assert dd.run(req)["proved"] is True


# --------------------------------------------------------------------------- #
# 3. DD-only theorems still proved by the joint engine (reusing geometry's rules)
# --------------------------------------------------------------------------- #
def test_dd_only_midpoint_collinear():
    req = {
        "op": "prove",
        "hypotheses": [{"pred": "midpoint", "points": ["M", "A", "B"]}],
        "goal": {"pred": "collinear", "points": ["A", "M", "B"]},
        "seed": SEED,
    }
    res = dd.run(req)
    assert res["proved"] is True
    assert res["derivation"]
    assert res["derivation"][0]["rule"].startswith("midpoint")


def test_dd_perp_perp_parallel():
    """Two lines perpendicular to a common line are parallel (a geometry.py rule,
    still available through the joint closure)."""
    req = {
        "op": "prove",
        "hypotheses": [
            {"pred": "perpendicular", "points": ["A", "B", "E", "F"]},
            {"pred": "perpendicular", "points": ["C", "D", "E", "F"]},
        ],
        "goal": {"pred": "parallel", "points": ["A", "B", "C", "D"]},
        "seed": SEED,
    }
    assert dd.run(req)["proved"] is True


# --------------------------------------------------------------------------- #
# 4. Numeric falsify-before-prove rejects FALSE conjectures fast.
# --------------------------------------------------------------------------- #
_FREE_TRIANGLE = [
    {"op": "free", "point": "A"},
    {"op": "free", "point": "B"},
    {"op": "free", "point": "C"},
]


def test_prove_numeric_screen_rejects_false_goal():
    """A generic triangle is not isoceles: the numeric screen must catch it and
    refuse to hand the goal to the symbolic engine."""
    req = {
        "op": "prove",
        "construction": _FREE_TRIANGLE,
        "hypotheses": [],
        "goal": {"pred": "cong", "points": ["A", "B", "A", "C"]},
        "seed": SEED,
    }
    res = dd.run(req)
    assert res["proved"] is False
    assert res["falsified"] is True
    assert "counterexample" in res


def test_falsify_op_finds_counterexample():
    req = {
        "op": "falsify",
        "construction": _FREE_TRIANGLE,
        "goal": {"pred": "cong", "points": ["A", "B", "A", "C"]},
        "seed": SEED,
    }
    res = dd.run(req)
    assert res["falsified"] is True
    assert set(res["counterexample"]) == {"A", "B", "C"}


def test_check_op_true_goal_holds():
    construction = [
        {"op": "free", "point": "A"},
        {"op": "free", "point": "B"},
        {"op": "midpoint", "point": "M", "of": ["A", "B"]},
    ]
    res = dd.run({
        "op": "check",
        "construction": construction,
        "goal": {"pred": "collinear", "points": ["A", "M", "B"]},
        "seed": SEED,
    })
    assert res["holds"] is True


# --------------------------------------------------------------------------- #
# 5. A goal that is neither in the DD closure nor the AR row space is reported
#    inconclusive (never a false disproof).
# --------------------------------------------------------------------------- #
def test_unreachable_goal_is_inconclusive_not_disproved():
    req = {
        "op": "prove",
        "hypotheses": [{"pred": "parallel", "points": ["A", "B", "C", "D"]}],
        "goal": {"pred": "perpendicular", "points": ["A", "B", "E", "F"]},
        "seed": SEED,
    }
    res = dd.run(req)
    assert res["proved"] is False
    assert res["falsified"] is False
    assert res.get("inconclusive") is True


# --------------------------------------------------------------------------- #
# 6. Determinism.
# --------------------------------------------------------------------------- #
def test_deterministic_falsify():
    req = {
        "op": "falsify",
        "construction": _FREE_TRIANGLE,
        "goal": {"pred": "cong", "points": ["A", "B", "A", "C"]},
        "seed": SEED,
    }
    a = dd.run(dict(req))
    b = dd.run(dict(req))
    assert a["counterexample"] == b["counterexample"]


def test_deterministic_prove_certificate():
    req = {
        "op": "prove",
        "hypotheses": [
            {"pred": "eqangle", "points": ["A", "O", "B", "X", "P", "Y"]},
            {"pred": "eqangle", "points": ["B", "O", "C", "Y", "P", "Z"]},
        ],
        "goal": {"pred": "eqangle", "points": ["A", "O", "C", "X", "P", "Z"]},
        "seed": SEED,
    }
    assert dd.run(dict(req))["ar_certificate"] == dd.run(dict(req))["ar_certificate"]


# --------------------------------------------------------------------------- #
# 7. Exact rational linear solver (stdlib core) + OPTIONAL numpy cross-check.
# --------------------------------------------------------------------------- #
def test_solve_exact_particular_solution():
    # c1*[1,0] + c2*[1,1] = [3,2]  =>  c2=2, c1=1
    mat = [[Fraction(1), Fraction(1)], [Fraction(0), Fraction(1)]]
    rhs = [Fraction(3), Fraction(2)]
    sol = dd._solve_exact(mat, rhs, 2)
    assert sol == [Fraction(1), Fraction(2)]


def test_solve_exact_detects_inconsistency():
    # [1]*c = 1 and [0]*c = 1 is inconsistent
    mat = [[Fraction(1)], [Fraction(0)]]
    rhs = [Fraction(1), Fraction(1)]
    assert dd._solve_exact(mat, rhs, 1) is None


def test_optional_numpy_cross_check():
    np = pytest.importorskip("numpy")
    mat = [[Fraction(2), Fraction(1)], [Fraction(1), Fraction(3)]]
    rhs = [Fraction(5), Fraction(10)]
    sol = dd._solve_exact(mat, rhs, 2)
    assert sol is not None
    a = np.array([[2.0, 1.0], [1.0, 3.0]])
    b = np.array([5.0, 10.0])
    ref = np.linalg.solve(a, b)
    assert all(abs(float(sol[i]) - ref[i]) < 1e-9 for i in range(2))
