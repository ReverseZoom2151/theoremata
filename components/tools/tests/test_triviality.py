"""Tests for the triviality / degenerate-solution detector.

All tests are offline, deterministic, and seeded. The heavy checks run with
``hard_kill=False`` so they stay fast and in-process; one test exercises the
default hard-kill subprocess path to confirm the sandbox wrapper works.
"""
from __future__ import annotations

from theoremata_tools.triviality import OP, run, triviality_check


def _spec(goal, variables, constraints=(), quantifier="exists"):
    return {
        "quantifier": quantifier,
        "variables": variables,
        "constraints": list(constraints),
        "goal": goal,
    }


def _var(name, start, stop, step=1):
    return {"name": name, "domain": {"start": start, "stop": stop, "step": step}}


# --- degenerate witness ----------------------------------------------------


def test_zero_witness_is_flagged_with_concrete_witness():
    # "exists x, y : x*y + x == 0" is trivially solved by x = 0 (any y).
    spec = _spec("x*y + x == 0", [_var("x", -10, 11), _var("y", -10, 11)])
    r = triviality_check(spec, seed=1, hard_kill=False)
    assert r["op"] == OP
    assert r["trivial"] is True
    assert r["kind"] == "degenerate_witness"
    w = r["witness"]
    assert w is not None
    # The reported witness must genuinely satisfy the goal (soundness).
    assert w["x"] * w["y"] + w["x"] == 0
    # And it must actually be degenerate (a zero or minus-one value marker).
    assert r["markers"], "a degenerate witness must carry markers"


def test_unexpectedly_large_boundary_witness_is_flagged():
    # "exists n>=2, k>=1 : n**k > 1_000_000" is trivial: just take both large.
    spec = _spec(
        "n**k > 1000000",
        [_var("n", 2, 61), _var("k", 1, 9)],
        constraints=["n >= 2 and k >= 1"],
    )
    r = triviality_check(spec, seed=7, hard_kill=False)
    assert r["trivial"] is True
    assert r["kind"] == "degenerate_witness"
    w = r["witness"]
    assert w["n"] ** w["k"] > 1_000_000  # verify
    markers = {m for ms in r["markers"].values() for m in ms}
    assert "max_boundary" in markers


# --- genuinely non-trivial -------------------------------------------------


def test_nontrivial_statement_returns_none():
    # Pythagorean triples with x<y: no boundary/zero/one corner solves it, the
    # constraints are satisfiable, and the goal is not a tautology.
    spec = _spec(
        "x*x + y*y == z*z",
        [_var("x", 1, 26), _var("y", 1, 26), _var("z", 1, 26)],
        constraints=["x > 0 and y > 0 and z > 0 and x < y"],
    )
    r = triviality_check(spec, seed=3, hard_kill=False)
    assert r["trivial"] is False
    assert r["kind"] == "none"
    assert r["witness"] is None
    assert r["advisory"] is True


# --- vacuity ---------------------------------------------------------------


def test_unsatisfiable_constraints_are_flagged_vacuous():
    # "x > 5 and x < 3" is unsatisfiable -> vacuous, no witness.
    spec = _spec(
        "x == x",
        [_var("x", -10, 11)],
        constraints=["x > 5 and x < 3"],
        quantifier="forall",
    )
    r = triviality_check(spec, seed=5, hard_kill=False)
    assert r["trivial"] is True
    assert r["kind"] == "vacuous"
    assert r["witness"] is None


def test_tautological_goal_is_flagged_vacuous():
    # Corners are excluded by the constraints, but interior points are
    # admissible and the goal x*x >= 0 holds on every sampled point -> tautology.
    spec = _spec(
        "x*x >= 0",
        [_var("x", -10, 11)],
        constraints=["x > 3 and x < 8"],
    )
    r = triviality_check(spec, seed=11, hard_kill=False)
    assert r["trivial"] is True
    assert r["kind"] == "vacuous"
    assert r["witness"] is not None
    assert 3 < r["witness"]["x"] < 8  # the vacuity witness is admissible


# --- determinism -----------------------------------------------------------


def test_same_seed_same_witness():
    spec = _spec("x*y + x == 0", [_var("x", -10, 11), _var("y", -10, 11)])
    a = triviality_check(spec, seed=42, hard_kill=False)
    b = triviality_check(spec, seed=42, hard_kill=False)
    assert a["witness"] == b["witness"]
    assert a == b


def test_different_seed_still_valid_witness():
    spec = _spec("x*y + x == 0", [_var("x", -10, 11), _var("y", -10, 11)])
    r = triviality_check(spec, seed=999, hard_kill=False)
    assert r["trivial"] is True
    w = r["witness"]
    assert w["x"] * w["y"] + w["x"] == 0  # any seed's witness is sound


# --- hard-kill subprocess path + worker adapter ----------------------------


def test_hard_kill_subprocess_path():
    # Default hard_kill=True routes through the spawn sandbox; result must match.
    spec = _spec("x*y + x == 0", [_var("x", -10, 11), _var("y", -10, 11)])
    r = triviality_check(spec, seed=1)
    assert r["trivial"] is True
    assert r["kind"] == "degenerate_witness"
    assert r["witness"]["x"] * r["witness"]["y"] + r["witness"]["x"] == 0


def test_run_adapter_and_op_name():
    req = {
        "statement_spec": _spec(
            "x*y + x == 0", [_var("x", -10, 11), _var("y", -10, 11)]
        ),
        "seed": 1,
        "hard_kill": False,
    }
    r = run(req)
    assert r["op"] == "triviality"
    assert r["trivial"] is True


def test_untrusted_import_is_rejected():
    # A predicate smuggling an import must be refused by the safe-eval allow-list
    # (surfaces as an exception the worker turns into ok:false).
    spec = _spec("__import__('os').system('echo hi') == 0", [_var("x", -3, 4)])
    try:
        triviality_check(spec, seed=1, hard_kill=False)
    except Exception as exc:  # noqa: BLE001
        assert "not allowed" in str(exc).lower() or "os" in str(exc).lower()
    else:
        raise AssertionError("untrusted import must be rejected")
