"""Tests for the AlphaGeometry-style synthetic-data engine (geometry_synth).

Verifies the four-ingredient recipe end to end:
  * seeded sampling is deterministic (same seed => byte-identical example);
  * the emitted proof genuinely derives the goal via geometry.py's chainer;
  * aux_holes == proof-dependency set MINUS goal-statement-dependency set
    (the dependency difference), computed independently from geometry.py's proof;
  * auxiliary holes are genuinely necessary (goal unprovable without them);
  * a batch yields distinct examples across seeds;
  * a no-auxiliary example has empty aux_holes and proves from its premises alone.

Pure standard library; no numpy, no network. Run from repo root::

    python -m pytest components/prover/tests/test_geometry_synth.py -x -q
"""
from __future__ import annotations

import json

import pytest

from theoremata_tools import geometry, geometry_synth


def _key(premise: dict) -> tuple:
    """Order-insensitive identity of a premise/goal for set comparisons."""
    return geometry._canonical(premise["pred"], list(premise["points"]))


def _proof_dependency_leaves(premises: list[dict], goal: dict) -> set:
    """Independently recover the proof-dependency set from geometry.py's OWN
    public proof: the premise facts that appear as a proof ``from`` but are never
    themselves a derived ``fact`` (i.e. the leaves of geometry's derivation)."""
    res = geometry.deductive_prove(premises, goal)
    assert res["proved"] is True
    derived_descr = {step["fact"] for step in res["derivation"]}
    used_descr = set()
    for step in res["derivation"]:
        for parent in step["from"]:
            if parent not in derived_descr:
                used_descr.add(parent)
    # Map premise descriptions back to premise dicts.
    by_descr = {geometry._describe(_key(p)): p for p in premises}
    return {_key(by_descr[d]) for d in used_descr}


# --------------------------------------------------------------------------- #
# 1. Determinism.
# --------------------------------------------------------------------------- #
def test_same_seed_is_deterministic():
    a = geometry_synth.build_example(7, prefer="aux")
    b = geometry_synth.build_example(7, prefer="aux")
    assert json.dumps(a, sort_keys=True) == json.dumps(b, sort_keys=True)


# --------------------------------------------------------------------------- #
# 2. The emitted proof genuinely derives the goal via geometry.py's chainer.
# --------------------------------------------------------------------------- #
@pytest.mark.parametrize("seed", [1, 2, 3, 11, 42, 100])
def test_proof_derives_goal_via_geometry(seed):
    ex = geometry_synth.build_example(seed, prefer="aux")
    # premises + aux_holes == the proof-dependency set; geometry re-proves it.
    hyps = ex["premises"] + ex["aux_holes"]
    res = geometry.deductive_prove(hyps, ex["goal"])
    assert res["proved"] is True
    # The emitted proof is non-empty and its final step concludes the goal.
    assert ex["proof_len"] >= 1
    assert ex["proof"][-1]["fact"] == geometry._describe(_key(ex["goal"]))
    # Numeric realization (when available) confirms the goal actually holds.
    assert ex["numeric_ok"] in (True, None)


# --------------------------------------------------------------------------- #
# 3. Dependency difference: aux_holes == proof-deps MINUS statement-deps.
# --------------------------------------------------------------------------- #
@pytest.mark.parametrize("seed", [1, 2, 3, 11, 42, 100])
def test_aux_is_the_dependency_difference(seed):
    ex = geometry_synth.build_example(seed, prefer="aux")
    goal_points = set(ex["goal"]["points"])

    # Proof-dependency set recovered INDEPENDENTLY from geometry.py's own proof.
    used = ex["premises"] + ex["aux_holes"]
    proof_deps = _proof_dependency_leaves(used, ex["goal"])
    # Statement-dependency set = used premises entirely inside the goal's points.
    stmt_deps = {_key(p) for p in used if set(p["points"]) <= goal_points}

    expected_aux = proof_deps - stmt_deps
    got_aux = {_key(p) for p in ex["aux_holes"]}
    assert got_aux == expected_aux
    # And every aux hole indeed names a point the goal never mentions.
    for hole in ex["aux_holes"]:
        assert not set(hole["points"]) <= goal_points


# --------------------------------------------------------------------------- #
# 4. Auxiliary holes are genuinely necessary.
# --------------------------------------------------------------------------- #
def test_aux_holes_are_necessary():
    ex = geometry_synth.build_example(3, prefer="aux")
    assert ex["aux_holes"], "expected an auxiliary-construction example"
    # With the holes: provable. Without them: NOT provable (that's the hole).
    assert geometry.deductive_prove(
        ex["premises"] + ex["aux_holes"], ex["goal"])["proved"] is True
    assert geometry.deductive_prove(
        ex["premises"], ex["goal"])["proved"] is False


# --------------------------------------------------------------------------- #
# 5. Batch yields distinct examples across seeds.
# --------------------------------------------------------------------------- #
def test_batch_distinct_examples():
    out = geometry_synth.run({"op": "batch", "seed": 1000, "n": 6})
    assert out["count"] == 6
    assert len(out["examples"]) == 6
    blobs = {json.dumps(e, sort_keys=True) for e in out["examples"]}
    assert len(blobs) == 6  # all distinct
    # Distinct in content too (goal + premises), not merely the seed field.
    contents = {json.dumps({"g": e["goal"], "p": e["premises"],
                            "a": e["aux_holes"]}, sort_keys=True)
                for e in out["examples"]}
    assert len(contents) == 6


def test_batch_matches_individual_seeds():
    out = geometry_synth.run({"op": "batch", "seed": 50, "n": 3})
    for i, ex in enumerate(out["examples"]):
        solo = geometry_synth.build_example(50 + i, prefer="aux")
        assert json.dumps(ex, sort_keys=True) == json.dumps(solo, sort_keys=True)


# --------------------------------------------------------------------------- #
# 6. A no-auxiliary example has empty aux_holes and proves from premises alone.
# --------------------------------------------------------------------------- #
@pytest.mark.parametrize("seed", [1, 5, 9, 42])
def test_no_aux_example_has_empty_holes(seed):
    ex = geometry_synth.build_example(seed, prefer="no_aux")
    assert ex["aux_holes"] == []
    # Provable directly from the given premises (nothing was moved into a hole).
    res = geometry.deductive_prove(ex["premises"], ex["goal"])
    assert res["proved"] is True
    # Every premise lies within the goal's own points (statement-dependency).
    goal_points = set(ex["goal"]["points"])
    for p in ex["premises"]:
        assert set(p["points"]) <= goal_points


# --------------------------------------------------------------------------- #
# 7. run() dispatch + schema coverage.
# --------------------------------------------------------------------------- #
def test_run_sample_schema():
    out = geometry_synth.run({"op": "sample", "seed": 8})
    assert out["op"] == "sample"
    ex = out["example"]
    for field in ("seed", "goal", "premises", "aux_holes", "proof",
                  "used_premises", "all_premises", "construction", "verified"):
        assert field in ex
    assert ex["verified"] is True
    assert ex["goal"]["pred"] and ex["goal"]["points"]


def test_run_unknown_op_raises():
    with pytest.raises(ValueError):
        geometry_synth.run({"op": "nonsense", "seed": 1})


def test_multi_step_proof_is_reachable():
    """The engine can mint proofs deeper than one step (perp-perp=>|| then
    ||-transitivity). Search a few seeds for a >=2-step proof."""
    depths = [geometry_synth.build_example(s, prefer="deepest")["proof_len"]
              for s in range(30)]
    assert max(depths) >= 2
