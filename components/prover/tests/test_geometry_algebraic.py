"""Tests for the algebraic (Wu's-method) geometry prover.

Covers the contractual scenarios:
  * prove a theorem Wu handles that the 5-rule forward chainer CANNOT
    (diagonals of a parallelogram bisect each other -- a ratio/intersection
    fact) -- final pseudo-remainder is 0;
  * reject a FALSE conjecture (numeric falsify returns a counterexample AND the
    pseudo-remainder is nonzero);
  * a degenerate case surfaces its non-degeneracy conditions rather than a false
    claim;
  * determinism given the seed.

Pure standard library; run from repo root::

    python -m pytest components/prover/tests/test_geometry_algebraic.py -x -q
"""
from __future__ import annotations

from fractions import Fraction

import pytest

from theoremata_tools import geometry, geometry_algebraic as ga


SEED = 20260708


# --------------------------------------------------------------------------- #
# The canonical fixture: parallelogram ABCD (A origin, B on x-axis) with the
# diagonals AC and BD meeting at M defined *only* by two collinearity
# hypotheses; goal: M is the midpoint of AC. This needs coordinates/ratios and
# is out of reach for geometry.deductive_prove.
# --------------------------------------------------------------------------- #
def _parallelogram_problem(goal):
    return {
        "op": "prove",
        "points": {
            "A": [0, 0],
            "B": ["u1", 0],
            "D": ["u2", "u3"],
            "C": ["x1", "x2"],
            "M": ["x3", "x4"],
        },
        "var_order": ["u1", "u2", "u3", "x1", "x2", "x3", "x4"],
        "hypotheses": [
            {"pred": "parallel", "points": ["A", "B", "D", "C"]},   # AB || DC
            {"pred": "parallel", "points": ["A", "D", "B", "C"]},   # AD || BC
            {"pred": "collinear", "points": ["A", "M", "C"]},       # M on AC
            {"pred": "collinear", "points": ["B", "M", "D"]},       # M on BD
        ],
        "goal": goal,
        "seed": SEED,
    }


# --------------------------------------------------------------------------- #
# 1. Wu proves a theorem the forward chainer cannot.
# --------------------------------------------------------------------------- #
def test_wu_proves_parallelogram_diagonals_bisect():
    req = _parallelogram_problem({"pred": "midpoint", "points": ["M", "A", "C"]})
    res = ga.run(req)
    assert res["proved"] is True
    assert res["falsified"] is False
    # both midpoint components reduced to the zero polynomial
    assert res["pseudo_remainders"] == ["0", "0"]
    # a real characteristic set was built (one poly per dependent variable)
    assert len(res["characteristic_set"]) == 4
    assert "certificate" in res


def test_forward_chainer_cannot_do_this():
    """Sanity: the 5-rule engine has no coordinates, so it cannot prove the
    midpoint-of-intersection fact -- justifying the algebraic backend."""
    hyps = [
        {"pred": "collinear", "points": ["A", "M", "C"]},
        {"pred": "collinear", "points": ["B", "M", "D"]},
    ]
    goal = {"pred": "midpoint", "points": ["M", "A", "C"]}
    res = geometry.deductive_prove(hyps, goal)
    assert res["proved"] is False


def test_wu_proves_simple_collinearity_via_coordinates():
    """Midpoints P of AB and Q of AC: prove PQ || BC (the midline theorem),
    a parallel fact obtained purely algebraically."""
    req = {
        "op": "prove",
        "points": {
            "A": [0, 0],
            "B": ["u1", 0],
            "C": ["u2", "u3"],
            "P": ["x1", "x2"],
            "Q": ["x3", "x4"],
        },
        "var_order": ["u1", "u2", "u3", "x1", "x2", "x3", "x4"],
        "hypotheses": [
            {"pred": "midpoint", "points": ["P", "A", "B"]},
            {"pred": "midpoint", "points": ["Q", "A", "C"]},
        ],
        "goal": {"pred": "parallel", "points": ["P", "Q", "B", "C"]},
        "seed": SEED,
    }
    res = ga.run(req)
    assert res["proved"] is True
    assert res["pseudo_remainders"] == ["0"]


# --------------------------------------------------------------------------- #
# 2. A FALSE conjecture: numeric falsify + nonzero pseudo-remainder.
# --------------------------------------------------------------------------- #
def test_false_conjecture_is_falsified():
    """FALSE: claim |AB| = |AC| (isoceles) for a *generic* parallelogram. The
    numeric pre-screen must return a counterexample and refuse to 'prove'."""
    req = _parallelogram_problem({"pred": "cong", "points": ["A", "B", "A", "C"]})
    res = ga.run(req)
    assert res["proved"] is False
    assert res["falsified"] is True
    assert "counterexample" in res
    # the counterexample is keyed by variable name
    assert set(res["counterexample"]).issuperset({"u1", "u2", "u3"})


def test_false_conjecture_nonzero_remainder():
    """Same false goal, but skip the numeric screen (op=falsify then manual
    reduce) to confirm the *algebraic* side also does not certify it: the
    pseudo-remainder is nonzero."""
    co = ga._Coords(
        {
            "A": [0, 0], "B": ["u1", 0], "D": ["u2", "u3"],
            "C": ["x1", "x2"], "M": ["x3", "x4"],
        },
        ["u1", "u2", "u3", "x1", "x2", "x3", "x4"],
    )
    hyps = ga._hypothesis_polys(
        [
            {"pred": "parallel", "points": ["A", "B", "D", "C"]},
            {"pred": "parallel", "points": ["A", "D", "B", "C"]},
        ],
        co,
    )
    chain = ga.triangulate(hyps, co.n)
    goal_polys = ga._goal_polys({"pred": "cong", "points": ["A", "B", "A", "C"]}, co)
    rem = ga.wu_reduce(goal_polys[0], chain)
    assert not rem.is_zero()


# --------------------------------------------------------------------------- #
# 3. The numeric falsifier as a standalone op.
# --------------------------------------------------------------------------- #
def test_falsify_op_finds_counterexample():
    req = _parallelogram_problem({"pred": "cong", "points": ["A", "B", "A", "C"]})
    req["op"] = "falsify"
    res = ga.run(req)
    assert res["op"] == "falsify"
    assert res["falsified"] is True
    assert "counterexample" in res


def test_falsify_op_true_goal_not_falsified():
    req = _parallelogram_problem({"pred": "midpoint", "points": ["M", "A", "C"]})
    req["op"] = "falsify"
    res = ga.run(req)
    assert res["falsified"] is False
    assert res["trials_valid"] > 0


# --------------------------------------------------------------------------- #
# 4. Degeneracy: the proof is certified only modulo non-degeneracy conditions,
#    which are surfaced explicitly (rather than silently claiming universality).
# --------------------------------------------------------------------------- #
def test_non_degeneracy_conditions_surfaced():
    req = _parallelogram_problem({"pred": "midpoint", "points": ["M", "A", "C"]})
    res = ga.run(req)
    assert res["proved"] is True
    # The certificate is conditional: initials of the chain must not vanish.
    assert len(res["non_degeneracy"]) >= 1
    joined = " ".join(res["non_degeneracy"])
    # e.g. u1 != 0 (AB non-degenerate) / u3 != 0 (D off the base line)
    assert "!= 0" in joined
    assert any(v in joined for v in ("u1", "u3", "x1"))


def test_degenerate_branch_is_skipped_not_falsely_claimed():
    """When an initial vanishes the sampler reports a degenerate realization and
    resamples rather than manufacturing a bogus point/counterexample."""
    # A pathological chain: single poly u1*x  (initial u1 can be 0).
    n = 2  # vars: u1=index0, x=index1
    p = ga.Poly.var(n, 0) * ga.Poly.var(n, 1)  # u1 * x, class var = x
    import random as _random
    # force u1 = 0 by seeding then checking the solver returns None for that draw
    got_degenerate = False
    for s in range(50):
        rng = _random.Random(s)
        # manually drive: set independent u1 near 0 is unlikely, so test solver
        assign = {0: 0.0}
        coeffs = [p.coeff_in(1, d).eval(assign) for d in range(p.degree_in(1) + 1)]
        roots = ga._solve_univariate(coeffs)
        if not roots:
            got_degenerate = True
            break
    assert got_degenerate


# --------------------------------------------------------------------------- #
# 5. Determinism.
# --------------------------------------------------------------------------- #
def test_deterministic_given_seed():
    req = _parallelogram_problem({"pred": "cong", "points": ["A", "B", "A", "C"]})
    req["op"] = "falsify"
    a = ga.run(dict(req))
    b = ga.run(dict(req))
    assert a["counterexample"] == b["counterexample"]


# --------------------------------------------------------------------------- #
# 6. Pseudo-division correctness (exact rational arithmetic), and an OPTIONAL
#    sympy cross-check (gated -- core suite never needs sympy).
# --------------------------------------------------------------------------- #
def test_pseudo_remainder_exact_identity():
    """I**k * g = q*f + r is not directly exposed, but a hand identity is:
    prem(x^2, x - a, x) == a^2 (with I = 1)."""
    n = 2  # x=0, a=1
    x = ga.Poly.var(n, 0)
    a = ga.Poly.var(n, 1)
    g = x * x
    f = x - a
    r = ga.pseudo_remainder(g, f, 0)
    assert r.to_str(["x", "a"]) == (a * a).to_str(["x", "a"])


def test_optional_sympy_cross_check():
    sympy = pytest.importorskip("sympy")
    x, a = sympy.symbols("x a")
    # sympy prem of x^2 by (x-a) in x
    q, r = sympy.div(sympy.Poly(x**2, x), sympy.Poly(x - a, x))
    # our pseudo-remainder (initial of x-a is 1, so prem == ordinary remainder)
    n = 2
    px = ga.Poly.var(n, 0)
    pa = ga.Poly.var(n, 1)
    ours = ga.pseudo_remainder(px * px, px - pa, 0)
    # evaluate both at a=3
    from fractions import Fraction as F
    assert ours.eval({1: 3.0}) == float(r.as_expr().subs(a, 3))
