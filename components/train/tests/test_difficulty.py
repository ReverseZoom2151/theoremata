import math
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.difficulty import (  # noqa: E402
    EASY,
    HARD,
    MEDIUM,
    bucket_by_percentile,
    curriculum,
    estimate_difficulty,
    problem_difficulty,
    run,
    train_h0,
    triage,
)


# --- difficulty signal extraction -----------------------------------------

def test_problem_difficulty_sources():
    assert problem_difficulty({"score": 3.0}) == 3.0
    assert problem_difficulty({"difficulty": 7.0}) == 7.0
    assert problem_difficulty(5) == 5.0
    # proof length -> exp(#steps); 1 step < 3 steps
    assert problem_difficulty({"proof": "a"}) < problem_difficulty({"proof": "a\nb\nc"})
    # no proof / empty -> inf (deferred to hardest)
    assert problem_difficulty({}) == math.inf
    assert problem_difficulty({"proof": ""}) == math.inf


# --- 1. difficulty curriculum: percentile bucketing + easiest-first --------

def test_bucket_by_percentile_terciles():
    diffs = [float(i) for i in range(1, 10)]  # 1..9
    buckets, p33, p67 = bucket_by_percentile(diffs)
    assert buckets[0] == EASY  # shortest -> easy
    assert buckets[-1] == HARD  # longest -> hard
    assert p33 < p67
    assert buckets.count(EASY) >= 3 and buckets.count(HARD) >= 3


def test_bucket_inf_is_hard():
    buckets, _, _ = bucket_by_percentile([1.0, 2.0, 3.0, math.inf])
    assert buckets[-1] == HARD


def test_curriculum_shortest_in_easy_and_easiest_first():
    problems = [
        {"id": "long", "proof": "a\nb\nc\nd\ne\nf"},
        {"id": "short", "proof": "a"},
        {"id": "mid", "proof": "a\nb\nc"},
    ]
    res = curriculum(problems)
    # easiest-first ordering: shortest proof leads
    assert [it["id"] for it in res["order"]] == ["short", "mid", "long"]
    # shortest proof is bucketed Easy, longest Hard
    by_id = {it["id"]: it["bucket"] for it in res["order"]}
    assert by_id["short"] == EASY
    assert by_id["long"] == HARD
    assert "short" in res["buckets"][EASY]


def test_curriculum_ties_broken_by_id_deterministic():
    problems = [{"id": "b", "score": 1.0}, {"id": "a", "score": 1.0}]
    res1 = curriculum(problems)
    res2 = curriculum(problems)
    assert [it["id"] for it in res1["order"]] == ["a", "b"]  # tie -> id order
    assert res1 == res2  # deterministic


# --- 2. H0 pre-filter: estimate + budget-aware triage ----------------------

def test_estimate_difficulty_fallback_monotonic_offline():
    # fallback (no model): sum of numeric features; larger => harder
    assert estimate_difficulty({"size": 1.0}) < estimate_difficulty({"size": 9.0})
    assert estimate_difficulty({}) == 0.0


def test_triage_tight_budget_defers_hardest_first():
    problems = [
        {"id": "p1", "difficulty": 1.0},
        {"id": "p2", "difficulty": 2.0},
        {"id": "p3", "difficulty": 3.0},
        {"id": "p4", "difficulty": 4.0},
    ]
    attempt, defer = triage(problems, budget=2)  # cost 1 each -> 2 attempts
    assert [r["id"] for r in attempt] == ["p1", "p2"]  # easiest attempted
    assert [r["id"] for r in defer] == ["p3", "p4"]  # hardest deferred first


def test_triage_loose_budget_attempts_more():
    problems = [{"id": f"p{i}", "difficulty": float(i)} for i in range(5)]
    tight, _ = triage(problems, budget=1)
    loose_attempt, loose_defer = triage(problems, budget=100)
    assert len(loose_attempt) > len(tight)
    assert loose_defer == []  # loose budget attempts all


def test_triage_max_difficulty_always_defers_hopeless():
    problems = [
        {"id": "easy", "difficulty": 1.0},
        {"id": "hopeless", "difficulty": 99.0},
    ]
    attempt, defer = triage(problems, budget=100, max_difficulty=10.0)
    assert [r["id"] for r in attempt] == ["easy"]
    assert [r["id"] for r in defer] == ["hopeless"]


def test_triage_deterministic_ties_by_id():
    problems = [{"id": "b", "difficulty": 1.0}, {"id": "a", "difficulty": 1.0}]
    a1, d1 = triage(problems, budget=1)
    a2, d2 = triage(problems, budget=1)
    assert [r["id"] for r in a1] == ["a"]  # tie -> id order
    assert (a1, d1) == (a2, d2)


def test_triage_uses_features_via_fallback():
    # no explicit difficulty -> hardness estimated from features (sum)
    problems = [
        {"id": "small", "features": {"size": 1.0}},
        {"id": "big", "features": {"size": 9.0}},
    ]
    attempt, defer = triage(problems, budget=1)
    assert [r["id"] for r in attempt] == ["small"]
    assert [r["id"] for r in defer] == ["big"]


# --- learned H0 model: sklearn gated, fallback offline ---------------------

def test_train_h0_fallback_runs_offline():
    # deterministic margin fallback (no backend). Bigger 'size' => harder.
    examples = [
        {"features": {"size": 1.0}, "solved": True},
        {"features": {"size": 2.0}, "solved": True},
        {"features": {"size": 8.0}, "solved": False},
        {"features": {"size": 9.0}, "solved": False},
    ]
    model = train_h0(examples, backend="fallback")
    assert model["lib"] == "fallback"
    assert model["weights"]["size"] > 0  # hard class has larger size
    # estimate is monotonic in the hard direction
    assert estimate_difficulty({"size": 9.0}, model) > estimate_difficulty({"size": 1.0}, model)


def test_train_h0_sklearn_path():
    pytest.importorskip("sklearn")
    examples = [
        {"features": {"size": 1.0}, "solved": True},
        {"features": {"size": 1.5}, "solved": True},
        {"features": {"size": 2.0}, "solved": True},
        {"features": {"size": 8.0}, "solved": False},
        {"features": {"size": 9.0}, "solved": False},
        {"features": {"size": 9.5}, "solved": False},
    ]
    model = train_h0(examples, backend="sklearn")
    assert model["lib"] == "sklearn"
    assert model["schema"] == "theoremata.h0-difficulty.v1"
    # learned probability-of-too-hard rises with size
    assert estimate_difficulty({"size": 9.0}, model) > estimate_difficulty({"size": 1.0}, model)
    # model is JSON-plain (queryable with no sklearn)
    import json
    json.loads(json.dumps(model))


def test_triage_trains_from_examples_and_uses_model():
    examples = [
        {"features": {"size": 1.0}, "solved": True},
        {"features": {"size": 2.0}, "solved": True},
        {"features": {"size": 8.0}, "solved": False},
        {"features": {"size": 9.0}, "solved": False},
    ]
    model = train_h0(examples, backend="fallback")
    problems = [
        {"id": "small", "features": {"size": 1.0}},
        {"id": "big", "features": {"size": 9.0}},
    ]
    attempt, defer = triage(problems, budget=1, model=model)
    assert [r["id"] for r in attempt] == ["small"]
    assert [r["id"] for r in defer] == ["big"]


# --- 3. run() dispatch -----------------------------------------------------

def test_run_curriculum_op():
    res = run({"op": "curriculum", "problems": [{"id": "x", "score": 5.0},
                                                {"id": "y", "score": 1.0}]})
    assert res["op"] == "curriculum"
    assert [it["id"] for it in res["order"]] == ["y", "x"]


def test_run_triage_op_with_inline_training():
    req = {
        "op": "triage",
        "budget": 1,
        "examples": [
            {"features": {"size": 1.0}, "solved": True},
            {"features": {"size": 9.0}, "solved": False},
        ],
        "problems": [
            {"id": "small", "features": {"size": 1.0}},
            {"id": "big", "features": {"size": 9.0}},
        ],
    }
    res = run(req)
    assert res["op"] == "triage"
    assert res["n_attempt"] == 1
    assert [r["id"] for r in res["attempt_now"]] == ["small"]
    assert [r["id"] for r in res["defer"]] == ["big"]


def test_run_unknown_op_raises():
    with pytest.raises(ValueError):
        run({"op": "nope"})


# keep MEDIUM referenced (tercile middle bucket exists for mid-length proofs)
def test_medium_bucket_populated():
    diffs = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]
    buckets, _, _ = bucket_by_percentile(diffs)
    assert MEDIUM in buckets
