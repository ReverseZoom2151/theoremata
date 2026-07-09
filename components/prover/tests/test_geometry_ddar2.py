"""Tests for the unified DDAR2 geometry engine (:mod:`theoremata_tools.geometry_ddar2`).

Contractual scenarios (all offline, deterministic, seeded, stdlib-only core):

  * :class:`ElimCore` proves a linear **angle chase** closes and returns the exact
    witnessing linear combination (certificate);
  * a **constant-value angle** goal (angle = pi/3) is proved via "fix" rows -- a
    theorem the plain 5-rule chainer in :mod:`geometry` cannot even state;
  * **ratio** and **length** constant goals (rconst / lconst) proved through the
    prime-factored log-distance sub-lattice;
  * an **additive-position** (distseq) chase closes;
  * a **FALSE** goal is inconclusive (not disproved) unless numerically falsified;
  * determinism given the seed;
  * stdlib-only core (an OPTIONAL numpy cross-check is gated behind importorskip).

Run from the repo root::

    python -m pytest components/prover/tests/test_geometry_ddar2.py -x -q
"""
from __future__ import annotations

from fractions import Fraction

import pytest

from theoremata_tools import geometry, geometry_ddar2 as dd2


SEED = 20260709


# --------------------------------------------------------------------------- #
# 1. ElimCore: exact incremental RREF + certificate.
# --------------------------------------------------------------------------- #
def test_elimcore_entails_returns_certificate():
    core = dd2.ElimCore()
    x, y, z = ("v", "x"), ("v", "y"), ("v", "z")
    # x - y = 0 ; y - z = 0  =>  x - z = 0 with cert [+1, +1]
    core.add({x: Fraction(1), y: Fraction(-1)}, "e1")
    core.add({y: Fraction(1), z: Fraction(-1)}, "e2")
    cert = core.entails({x: Fraction(1), z: Fraction(-1)})
    assert cert is not None
    assert sorted((dd2._fmt_frac(c), l) for c, l in cert) == [("1", "e1"), ("1", "e2")]


def test_elimcore_non_entailed_returns_none():
    core = dd2.ElimCore()
    x, y, z = ("v", "x"), ("v", "y"), ("v", "z")
    core.add({x: Fraction(1), y: Fraction(-1)}, "e1")
    assert core.entails({x: Fraction(1), z: Fraction(-1)}) is None


def test_elimcore_detects_inconsistency():
    core = dd2.ElimCore()
    x = ("v", "x")
    core.add({x: Fraction(1), dd2.ONE: Fraction(-1)}, "e1")   # x = 1
    core.add({x: Fraction(1), dd2.ONE: Fraction(-2)}, "e2")   # x = 2  => 1 = 2
    assert core.inconsistent is True


def test_elimcore_clone_is_independent():
    core = dd2.ElimCore()
    x, y = ("v", "x"), ("v", "y")
    core.add({x: Fraction(1), y: Fraction(-1)}, "e1")
    clone = core.clone()
    clone.add({y: Fraction(1), dd2.ONE: Fraction(-3)}, "e2")   # y = 3 in clone only
    assert clone.entails({x: Fraction(1), dd2.ONE: Fraction(-3)}) is not None
    assert core.entails({x: Fraction(1), dd2.ONE: Fraction(-3)}) is None


# --------------------------------------------------------------------------- #
# 2. Angle chase the five-rule chainer cannot do -- with a certificate.
# --------------------------------------------------------------------------- #
def test_directed_angle_addition_certificate():
    """<AOB=<XPY and <BOC=<YPZ  =>  <AOC=<XPZ (directed-angle additivity)."""
    req = {
        "op": "prove",
        "hypotheses": [
            {"pred": "eqangle", "points": ["A", "O", "B", "X", "P", "Y"]},
            {"pred": "eqangle", "points": ["B", "O", "C", "Y", "P", "Z"]},
        ],
        "goal": {"pred": "eqangle", "points": ["A", "O", "C", "X", "P", "Z"]},
        "seed": SEED,
    }
    res = dd2.run(req)
    assert res["proved"] is True and res["falsified"] is False
    assert res["ar_system"] == "angle"
    assert [c["coeff"] for c in res["certificate"]] == ["1", "1"]
    # anchor: geometry.py's five rules cannot chase angles at all
    assert geometry.deductive_prove(req["hypotheses"], req["goal"])["proved"] is False


def test_parallel_transitive_via_ar():
    req = {
        "op": "prove",
        "hypotheses": [
            {"pred": "parallel", "points": ["A", "B", "C", "D"]},
            {"pred": "parallel", "points": ["C", "D", "E", "F"]},
        ],
        "goal": {"pred": "parallel", "points": ["A", "B", "E", "F"]},
        "seed": SEED,
    }
    assert dd2.run(req)["proved"] is True


def test_perpendicular_pi_over_two():
    """AB||CD and AB _|_ EF  =>  CD _|_ EF (angle system, pi/2 constant)."""
    req = {
        "op": "prove",
        "hypotheses": [
            {"pred": "parallel", "points": ["A", "B", "C", "D"]},
            {"pred": "perpendicular", "points": ["A", "B", "E", "F"]},
        ],
        "goal": {"pred": "perpendicular", "points": ["C", "D", "E", "F"]},
        "seed": SEED,
    }
    assert dd2.run(req)["proved"] is True


# --------------------------------------------------------------------------- #
# 3. Constant-value predicates (the item-3 build): the five-rule chainer cannot
#    even STATE these; DDAR2 proves them via "fix" rows.
# --------------------------------------------------------------------------- #
def test_constant_angle_sum_proves_sixty_degrees():
    """<ABX = 30deg and <XBC = 30deg  =>  <ABC = 60deg  (= pi/3)."""
    req = {
        "op": "prove",
        "hypotheses": [
            {"pred": "aconst", "points": ["A", "B", "X"], "deg": 30},
            {"pred": "aconst", "points": ["X", "B", "C"], "deg": 30},
        ],
        "goal": {"pred": "aconst", "points": ["A", "B", "C"], "const": "pi/3"},
        "seed": SEED,
    }
    res = dd2.run(req)
    assert res["proved"] is True
    assert res["ar_system"] == "angle"
    # certificate is the sum of the two fix rows
    assert [c["coeff"] for c in res["certificate"]] == ["1", "1"]


def test_constant_angle_wrong_value_inconclusive():
    """Same hypotheses, but a wrong constant (pi/2) is NOT proved -- and, with no
    construction, reported inconclusive, never falsely proved."""
    req = {
        "op": "prove",
        "hypotheses": [
            {"pred": "aconst", "points": ["A", "B", "X"], "deg": 30},
            {"pred": "aconst", "points": ["X", "B", "C"], "deg": 30},
        ],
        "goal": {"pred": "aconst", "points": ["A", "B", "C"], "const": "pi/2"},
        "seed": SEED,
    }
    res = dd2.run(req)
    assert res["proved"] is False
    assert res["falsified"] is False
    assert res.get("inconclusive") is True


def test_s_angle_fix_row_proves_aconst():
    """s_angle pins <(AB,BX)=pi/3; goal aconst reads it straight off the fix row."""
    req = {
        "op": "prove",
        "hypotheses": [{"pred": "s_angle", "points": ["A", "B", "X"], "const": "pi/3"}],
        "goal": {"pred": "s_angle", "points": ["A", "B", "X"], "const": "pi/3"},
        "seed": SEED,
    }
    assert dd2.run(req)["proved"] is True


def test_ratio_constant_chain():
    """|AB|/|CD| = 3/2 and |CD|/|EF| = 2  =>  |AB|/|EF| = 3  (log-distance)."""
    req = {
        "op": "prove",
        "hypotheses": [
            {"pred": "rconst", "points": ["A", "B", "C", "D"], "const": "3/2"},
            {"pred": "rconst", "points": ["C", "D", "E", "F"], "const": "2"},
        ],
        "goal": {"pred": "rconst", "points": ["A", "B", "E", "F"], "const": "3"},
        "seed": SEED,
    }
    res = dd2.run(req)
    assert res["proved"] is True
    assert res["ar_system"] == "length"


def test_length_constant_from_cong_and_lconst():
    """|XA| = 5 and |XA| = |YB|  =>  |YB| = 5  (prime-factored length constant)."""
    req = {
        "op": "prove",
        "hypotheses": [
            {"pred": "lconst", "points": ["X", "A"], "const": "5"},
            {"pred": "cong", "points": ["X", "A", "Y", "B"]},
        ],
        "goal": {"pred": "lconst", "points": ["Y", "B"], "const": "5"},
        "seed": SEED,
    }
    assert dd2.run(req)["proved"] is True


def test_ratio_constant_wrong_value_inconclusive():
    req = {
        "op": "prove",
        "hypotheses": [
            {"pred": "rconst", "points": ["A", "B", "C", "D"], "const": "3/2"},
            {"pred": "rconst", "points": ["C", "D", "E", "F"], "const": "2"},
        ],
        "goal": {"pred": "rconst", "points": ["A", "B", "E", "F"], "const": "4"},
        "seed": SEED,
    }
    assert dd2.run(req)["proved"] is False


# --------------------------------------------------------------------------- #
# 4. Additive-position group (distseq).
# --------------------------------------------------------------------------- #
def test_distseq_additive_chase():
    """AB = CD and CD = EF (signed)  =>  AB = EF, in the additive group."""
    req = {
        "op": "prove",
        "hypotheses": [
            {"pred": "distseq", "terms": [[1, ["A", "B"]], [-1, ["C", "D"]]]},
            {"pred": "distseq", "terms": [[1, ["C", "D"]], [-1, ["E", "F"]]]},
        ],
        "goal": {"pred": "distseq", "terms": [[1, ["A", "B"]], [-1, ["E", "F"]]]},
        "seed": SEED,
    }
    res = dd2.run(req)
    assert res["proved"] is True
    assert res["ar_system"] == "position"


# --------------------------------------------------------------------------- #
# 5. DD still works through the joint engine (reuses geometry's five rules).
# --------------------------------------------------------------------------- #
def test_dd_only_midpoint_collinear():
    req = {
        "op": "prove",
        "hypotheses": [{"pred": "midpoint", "points": ["M", "A", "B"]}],
        "goal": {"pred": "collinear", "points": ["A", "M", "B"]},
        "seed": SEED,
    }
    res = dd2.run(req)
    assert res["proved"] is True
    assert res["derivation"]


# --------------------------------------------------------------------------- #
# 6. FALSE goals: inconclusive without a construction; falsified with one.
# --------------------------------------------------------------------------- #
_FREE_TRIANGLE = [
    {"op": "free", "point": "A"},
    {"op": "free", "point": "B"},
    {"op": "free", "point": "C"},
]


def test_unreachable_goal_is_inconclusive_not_disproved():
    req = {
        "op": "prove",
        "hypotheses": [{"pred": "parallel", "points": ["A", "B", "C", "D"]}],
        "goal": {"pred": "perpendicular", "points": ["A", "B", "E", "F"]},
        "seed": SEED,
    }
    res = dd2.run(req)
    assert res["proved"] is False and res["falsified"] is False
    assert res.get("inconclusive") is True


def test_numeric_screen_falsifies_false_goal():
    req = {
        "op": "prove",
        "construction": _FREE_TRIANGLE,
        "hypotheses": [],
        "goal": {"pred": "cong", "points": ["A", "B", "A", "C"]},
        "seed": SEED,
    }
    res = dd2.run(req)
    assert res["proved"] is False
    assert res["falsified"] is True
    assert "counterexample" in res


def test_falsify_op():
    req = {
        "op": "falsify",
        "construction": _FREE_TRIANGLE,
        "goal": {"pred": "cong", "points": ["A", "B", "A", "C"]},
        "seed": SEED,
    }
    res = dd2.run(req)
    assert res["falsified"] is True
    assert set(res["counterexample"]) == {"A", "B", "C"}


def test_check_op_true_goal_holds():
    construction = [
        {"op": "free", "point": "A"},
        {"op": "free", "point": "B"},
        {"op": "midpoint", "point": "M", "of": ["A", "B"]},
    ]
    res = dd2.run({
        "op": "check",
        "construction": construction,
        "goal": {"pred": "collinear", "points": ["A", "M", "B"]},
        "seed": SEED,
    })
    assert res["holds"] is True


# --------------------------------------------------------------------------- #
# 7. Determinism.
# --------------------------------------------------------------------------- #
def test_deterministic_certificate():
    req = {
        "op": "prove",
        "hypotheses": [
            {"pred": "aconst", "points": ["A", "B", "X"], "deg": 30},
            {"pred": "aconst", "points": ["X", "B", "C"], "deg": 30},
        ],
        "goal": {"pred": "aconst", "points": ["A", "B", "C"], "const": "pi/3"},
        "seed": SEED,
    }
    assert dd2.run(dict(req))["certificate"] == dd2.run(dict(req))["certificate"]


def test_deterministic_falsify():
    req = {
        "op": "falsify",
        "construction": _FREE_TRIANGLE,
        "goal": {"pred": "cong", "points": ["A", "B", "A", "C"]},
        "seed": SEED,
    }
    assert dd2.run(dict(req))["counterexample"] == dd2.run(dict(req))["counterexample"]


# --------------------------------------------------------------------------- #
# 8. Constant parsing + prime-factored log constants (stdlib) and optional numpy.
# --------------------------------------------------------------------------- #
def test_angle_const_parsing():
    assert dd2._angle_const({"deg": 60}) == Fraction(1, 3)
    assert dd2._angle_const({"const": "pi/3"}) == Fraction(1, 3)
    assert dd2._angle_const({"const": "2pi/3"}) == Fraction(2, 3)
    assert dd2._angle_const({"const": "7pi/30"}) == Fraction(7, 30)
    assert dd2._angle_const({"const": "1/2"}) == Fraction(1, 2)


def test_log_const_prime_factoring_is_exact():
    comb: dict = {}
    dd2._add_log_const(comb, Fraction(6, 4), 1)   # log(3/2) = log3 - log2
    assert comb[dd2._logp(3)] == Fraction(1)
    assert comb[dd2._logp(2)] == Fraction(-1)


def test_optional_numpy_cross_check():
    np = pytest.importorskip("numpy")
    core = dd2.ElimCore()
    x, y = ("v", "x"), ("v", "y")
    # 2x + y = 5 ; x + 3y = 10  (add ONE atom for the constant side)
    core.add({x: Fraction(2), y: Fraction(1), dd2.ONE: Fraction(-5)}, "e1")
    core.add({x: Fraction(1), y: Fraction(3), dd2.ONE: Fraction(-10)}, "e2")
    # solve: x = (5*3-10)/(2*3-1)=1, y=(2*10-5)/5=3
    assert core.entails({x: Fraction(1), dd2.ONE: Fraction(-1)}) is not None
    assert core.entails({y: Fraction(1), dd2.ONE: Fraction(-3)}) is not None
    ref = np.linalg.solve(np.array([[2.0, 1.0], [1.0, 3.0]]), np.array([5.0, 10.0]))
    assert abs(ref[0] - 1.0) < 1e-9 and abs(ref[1] - 3.0) < 1e-9
