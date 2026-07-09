"""Tests for the minimal-dependency traceback + check_good synthesis module.

Covers, offline and deterministically (any seed is passed in):
  * the minimal-support solve returns a genuine minimal subset of source
    equalities (and drops a redundant one);
  * traceback returns a minimal subgraph that drops a redundant premise, and the
    minimal proof still entails the goal via geometry.py's chainer;
  * check_good flags a fact whose proof footprint exceeds its statement footprint
    and returns the EXACT auxiliary points, and does NOT flag a fact whose proof
    stays inside its statement points;
  * harvest yields aux-labelled examples deterministically;
  * the run() worker dispatch matches the direct API.

Run from repo root::

    python -m pytest components/prover/tests/test_geometry_synth2.py -x -q
"""
from __future__ import annotations

import json
from fractions import Fraction

import pytest

from theoremata_tools import geometry, geometry_synth2 as gs2


def _key(premise: dict) -> tuple:
    return geometry._canonical(premise["pred"], list(premise["points"]))


# --------------------------------------------------------------------------- #
# 1. Minimal-support solve: genuine minimum-cardinality subset.
# --------------------------------------------------------------------------- #
def test_minimal_support_drops_redundant_source():
    # cong chain AB=CD, CD=EF, EF=GH ; goal AB=EF needs only the first two.
    sources = [
        gs2._fact_equation(_key({"pred": "cong", "points": ["A", "B", "C", "D"]})),
        gs2._fact_equation(_key({"pred": "cong", "points": ["C", "D", "E", "F"]})),
        gs2._fact_equation(_key({"pred": "cong", "points": ["E", "F", "G", "H"]})),
    ]
    target = gs2._fact_equation(_key({"pred": "cong", "points": ["A", "B", "E", "F"]}))

    support = gs2.minimal_support(sources, target)
    assert support is not None
    # Exactly two sources, and the third (EF=GH) is redundant/excluded.
    assert len(support) == 2
    assert 2 not in support
    # Genuinely minimal: no single source spans the target.
    for i in range(len(sources)):
        assert not gs2._in_span(target, [sources[i]])
    # And the returned subset really spans it.
    assert gs2._in_span(target, [sources[i] for i in support])


def test_minimal_support_perp_perp_gives_parallel():
    # Two perpendiculars to a common line PQ entail the parallel of their feet.
    s1 = gs2._fact_equation(_key({"pred": "perpendicular",
                                  "points": ["R", "F", "P", "Q"]}))
    s2 = gs2._fact_equation(_key({"pred": "perpendicular",
                                  "points": ["S", "G", "P", "Q"]}))
    target = gs2._fact_equation(_key({"pred": "parallel",
                                      "points": ["R", "F", "S", "G"]}))
    support = gs2.minimal_support([s1, s2], target)
    assert support is not None and len(support) == 2
    # Neither perpendicular alone entails the parallel.
    assert not gs2._in_span(target, [s1])
    assert not gs2._in_span(target, [s2])


def test_minimal_support_returns_none_when_unentailed():
    s = gs2._fact_equation(_key({"pred": "cong", "points": ["A", "B", "C", "D"]}))
    target = gs2._fact_equation(_key({"pred": "cong", "points": ["X", "Y", "Z", "W"]}))
    assert gs2.minimal_support([s], target) is None


def test_fact_equation_uses_exact_rationals():
    eq = gs2._fact_equation(_key({"pred": "perpendicular",
                                  "points": ["A", "B", "C", "D"]}))
    assert eq[gs2._CONST] == Fraction(1, 2)
    assert all(isinstance(v, Fraction) for v in eq.values())


# --------------------------------------------------------------------------- #
# 2. Traceback: minimal subgraph drops a redundant premise; proof entails goal.
# --------------------------------------------------------------------------- #
def test_traceback_drops_redundant_premise_and_still_entails():
    premises = [
        {"pred": "parallel", "points": ["A", "B", "C", "D"]},
        {"pred": "parallel", "points": ["C", "D", "E", "F"]},
        {"pred": "parallel", "points": ["E", "F", "G", "H"]},  # redundant for goal
    ]
    goal = {"pred": "parallel", "points": ["A", "B", "E", "F"]}

    res = gs2.traceback(goal, premises)
    assert res["proved"] is True
    assert res["method"] == "ar-minimal-support"
    # The redundant EF||GH premise is dropped.
    min_keys = {_key(p) for p in res["minimal_premises"]}
    assert len(res["minimal_premises"]) == 2
    assert _key({"pred": "parallel", "points": ["E", "F", "G", "H"]}) not in min_keys
    dropped_keys = {_key(p) for p in res["dropped_premises"]}
    assert _key({"pred": "parallel", "points": ["E", "F", "G", "H"]}) in dropped_keys

    # The minimal proof still entails the goal, independently re-proved.
    assert res["entails"] is True
    check = geometry.deductive_prove(res["minimal_premises"], goal)
    assert check["proved"] is True
    # And the goal is NOT provable from the dropped premise alone.
    assert geometry.deductive_prove(res["dropped_premises"], goal)["proved"] is False


def test_traceback_unprovable_goal():
    premises = [{"pred": "parallel", "points": ["A", "B", "C", "D"]}]
    goal = {"pred": "parallel", "points": ["X", "Y", "Z", "W"]}
    res = gs2.traceback(goal, premises)
    assert res["proved"] is False
    assert res["entails"] is False


# --------------------------------------------------------------------------- #
# 3. check_good: flags aux facts with exact aux points; skips within-statement.
# --------------------------------------------------------------------------- #
def test_check_good_flags_aux_and_skips_non_aux():
    premises = [
        # Two perpendiculars to a hidden common line PQ -> parallel(RF, SG),
        # whose proof drags in the auxiliary points P and Q.
        {"pred": "perpendicular", "points": ["R", "F", "P", "Q"]},
        {"pred": "perpendicular", "points": ["S", "G", "P", "Q"]},
        # A midpoint -> cong(MA, MB) & collinear(A,M,B): NO auxiliary points.
        {"pred": "midpoint", "points": ["M", "A", "B"]},
    ]
    flagged = gs2.check_good(premises)

    # The parallel is flagged with EXACTLY the aux points {P, Q}.
    par = [r for r in flagged if r["goal"]["pred"] == "parallel"]
    assert len(par) == 1
    assert par[0]["aux_points"] == ["P", "Q"]
    assert set(par[0]["statement_footprint"]) == {"F", "G", "R", "S"}
    assert set(par[0]["proof_footprint"]) == {"F", "G", "P", "Q", "R", "S"}
    assert par[0]["requires_aux"] is True and par[0]["verified"] is True

    # The midpoint-derived facts (cong / collinear) are NOT flagged: their proof
    # footprint stays within their statement points {M, A, B}.
    flagged_preds = {r["goal"]["pred"] for r in flagged}
    assert "cong" not in flagged_preds
    assert "collinear" not in flagged_preds


def test_check_good_aux_points_are_provably_outside_statement():
    premises = [
        {"pred": "perpendicular", "points": ["R", "F", "P", "Q"]},
        {"pred": "perpendicular", "points": ["S", "G", "P", "Q"]},
    ]
    for rec in gs2.check_good(premises):
        aux = set(rec["aux_points"])
        stmt = set(rec["statement_footprint"])
        assert aux and aux.isdisjoint(stmt)
        assert aux == set(rec["proof_footprint"]) - stmt


# --------------------------------------------------------------------------- #
# 4. harvest: deterministic aux-labelled examples.
# --------------------------------------------------------------------------- #
def test_harvest_yields_aux_examples():
    examples = gs2.harvest(seed=1, n=4)
    assert len(examples) > 0
    for ex in examples:
        assert ex["requires_aux"] is True
        assert ex["aux_points"]                      # non-empty aux set
        assert set(ex["aux_points"]).isdisjoint(ex["statement_footprint"])
        assert ex["verified"] is True
        assert "construction" in ex and "seed" in ex


def test_harvest_is_deterministic():
    a = gs2.harvest(seed=3, n=3)
    b = gs2.harvest(seed=3, n=3)
    assert json.dumps(a, sort_keys=True) == json.dumps(b, sort_keys=True)


def test_harvest_rejects_negative_n():
    with pytest.raises(ValueError):
        gs2.harvest(seed=0, n=-1)


# --------------------------------------------------------------------------- #
# 5. run() worker dispatch.
# --------------------------------------------------------------------------- #
def test_run_traceback_matches_direct():
    premises = [
        {"pred": "cong", "points": ["A", "B", "C", "D"]},
        {"pred": "cong", "points": ["C", "D", "E", "F"]},
        {"pred": "cong", "points": ["X", "Y", "Z", "W"]},  # redundant
    ]
    goal = {"pred": "cong", "points": ["A", "B", "E", "F"]}
    out = gs2.run({"op": "traceback", "goal": goal, "premises": premises})
    assert out["op"] == "traceback"
    assert out["proved"] is True and out["entails"] is True
    assert len(out["minimal_premises"]) == 2


def test_run_check_good_from_seed():
    out = gs2.run({"op": "check_good", "seed": 2})
    assert out["op"] == "check_good"
    assert out["count"] == len(out["examples"]) >= 1


def test_run_harvest():
    out = gs2.run({"op": "harvest", "seed": 5, "n": 2})
    assert out["op"] == "harvest"
    assert out["count"] == len(out["examples"])


def test_run_unknown_op():
    with pytest.raises(ValueError):
        gs2.run({"op": "nope"})
