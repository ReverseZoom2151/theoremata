import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import pytest  # noqa: E402

from theoremata_tools.evolve import (  # noqa: E402
    ProgramDatabase,
    debate,
    evolve,
    run,
)


# --- deterministic injected callables -------------------------------------

TARGET = "theorem"


def make_evaluate(target=TARGET):
    """Score a candidate by closeness to ``target`` (higher = better).

    Exact match -> 1.0 (verdict "pass"); otherwise 1/(1+edit-ish distance).
    """

    def evaluate(code):
        # crude closeness: matching prefix length minus length penalty
        match = 0
        for a, b in zip(code, target):
            if a == b:
                match += 1
            else:
                break
        dist = (len(target) - match) + abs(len(code) - len(target))
        score = 1.0 / (1.0 + dist)
        return {"score": score, "verdict": "pass" if dist == 0 else "fail"}

    return evaluate


def make_generate(target=TARGET):
    """Deterministically mutate parents one step toward ``target``."""

    def generate(parents, n):
        out = []
        for i in range(n):
            parent = parents[i % len(parents)] if parents else {"code": ""}
            code = parent.get("code", "")
            if len(code) < len(target) and target.startswith(code):
                out.append(target[: len(code) + 1])
            else:
                out.append(target)
        return out

    return generate


# --- ProgramDatabase ------------------------------------------------------

def test_database_add_and_size():
    db = ProgramDatabase()
    a = db.add({"code": "a", "score": 0.1})
    b = db.add({"code": "bb", "score": 0.9})
    assert db.size() == 2
    assert a["id"] != b["id"]
    assert a["generation"] == 0
    assert b["parent_id"] is None


def test_database_best_orders_by_score_then_length():
    db = ProgramDatabase()
    db.add({"code": "aaaa", "score": 0.5})
    db.add({"code": "bb", "score": 0.9})
    db.add({"code": "cccc", "score": 0.9})  # ties bb on score, longer code
    top = db.best(2)
    assert top[0]["code"] == "bb"  # same score as cccc but shorter wins
    assert top[1]["code"] == "cccc"
    assert db.best(1)[0]["score"] == 0.9


def test_database_best_k_bounds():
    db = ProgramDatabase()
    db.add({"code": "x", "score": 0.2})
    assert db.best(5) == db.best(1)  # k larger than population
    assert db.best(0) == []


def test_sample_parents_favors_high_score_and_keeps_diversity():
    db = ProgramDatabase()
    for i in range(6):
        db.add({"code": "c" * (i + 1), "score": i / 10.0})
    parents = db.sample_parents(3)
    assert len(parents) == 3
    # elite (highest score) is always included
    assert parents[0]["score"] == 0.5
    # diversity: not all three are the very top three consecutive elites
    scores = {p["score"] for p in parents}
    assert len(scores) == 3
    assert min(scores) < 0.5  # reached below the single elite


def test_sample_parents_edge_cases():
    db = ProgramDatabase()
    assert db.sample_parents(3) == []  # empty db
    db.add({"code": "a", "score": 1.0})
    assert db.sample_parents(0) == []
    assert len(db.sample_parents(5)) == 1  # k exceeds population


# --- evolve ---------------------------------------------------------------

def test_evolve_improves_best_score_monotonically():
    res = evolve(
        ["t"],
        generate=make_generate(),
        evaluate=make_evaluate(),
        rounds=6,
        fanout=4,
        keep=3,
    )
    hist = res["history"]
    assert len(hist) == 6
    # best-so-far never regresses
    assert all(hist[i] <= hist[i + 1] for i in range(len(hist) - 1))
    # and it strictly improves overall
    assert hist[-1] > hist[0]


def test_evolve_reaches_target_and_returns_best():
    res = evolve(
        [""],
        generate=make_generate(),
        evaluate=make_evaluate(),
        rounds=len(TARGET) + 2,
        fanout=3,
        keep=2,
    )
    assert res["ok"] is True
    assert res["best"]["code"] == TARGET
    assert res["best"]["score"] == 1.0
    assert res["best"]["verdict"] == "pass"


def test_evolve_respects_keep_and_fanout():
    keep = 2
    fanout = 5
    rounds = 3
    res = evolve(
        ["t"],
        generate=make_generate(),
        evaluate=make_evaluate(),
        rounds=rounds,
        fanout=fanout,
        keep=keep,
    )
    # population is capped at keep after every round
    assert res["database_size"] == keep
    # each round: 1 seed evaluated + fanout children per round
    assert res["evaluated"] == 1 + rounds * fanout
    assert res["rounds"] == rounds


def test_evolve_seed_with_precomputed_score_not_reevaluated():
    calls = {"n": 0}

    def evaluate(code):
        calls["n"] += 1
        return {"score": 0.5, "verdict": "fail"}

    def generate(parents, n):
        return ["child"] * n

    evolve(
        [{"code": "seed", "score": 0.9, "verdict": "pass"}],
        generate=generate,
        evaluate=evaluate,
        rounds=1,
        fanout=2,
        keep=2,
    )
    # seed already scored -> only the 2 children are evaluated
    assert calls["n"] == 2


def test_evolve_judge_breaks_ties():
    # all candidates score equally; judge picks the one it Elo-ranks highest.
    def evaluate(code):
        return {"score": 0.5, "verdict": "pass"}

    def generate(parents, n):
        return [f"cand{i}" for i in range(n)]

    def judge(candidates):
        # rank "cand2" highest if present, else first
        return [10.0 if c["code"] == "cand2" else 1.0 for c in candidates]

    res = evolve(
        ["seed"],
        generate=generate,
        evaluate=evaluate,
        rounds=1,
        fanout=3,
        keep=5,
        judge=judge,
    )
    assert res["best"]["code"] == "cand2"


def test_evolve_judge_cannot_demote_higher_score():
    # a strictly higher objective score must not be overridden by the judge.
    def evaluate(code):
        return {"score": 1.0 if code == "winner" else 0.1, "verdict": "pass"}

    def generate(parents, n):
        return ["winner"] + ["loser"] * (n - 1)

    def judge(candidates):
        # perversely rank the loser highest
        return [100.0 if c["code"] == "loser" else 0.0 for c in candidates]

    res = evolve(
        ["seed"],
        generate=generate,
        evaluate=evaluate,
        rounds=1,
        fanout=3,
        keep=5,
        judge=judge,
    )
    assert res["best"]["code"] == "winner"
    assert res["best"]["score"] == 1.0


# --- debate ---------------------------------------------------------------

def test_debate_supported_only_when_ground_true():
    supported = debate(
        "candidate proof",
        argue_for=lambda c: "it type-checks",
        argue_against=lambda c: "it might have a gap",
        ground=lambda c: True,
    )
    assert supported["supported"] is True
    assert "for" in supported["rationale"] and "against" in supported["rationale"]

    unsupported = debate(
        "candidate proof",
        argue_for=lambda c: "sounds convincing",
        argue_against=lambda c: "no lemma backs it",
        ground=lambda c: False,
    )
    assert unsupported["supported"] is False


def test_debate_ground_decides_regardless_of_rhetoric():
    # even a one-sided persuasive "for" cannot make it supported without ground.
    res = debate(
        "x",
        argue_for=lambda c: "overwhelmingly true!!!",
        argue_against=lambda c: "",
        ground=lambda c: False,
    )
    assert res["supported"] is False


# --- run() dispatch (model-free specs) ------------------------------------

def test_run_evolve_target_len_spec():
    res = run(
        {
            "op": "evolve",
            "seed_candidates": [""],
            "generate": {"kind": "grow", "toward": "abcd"},
            "evaluate": {"kind": "target_len", "target": 4},
            "rounds": 5,
            "fanout": 2,
            "keep": 2,
        }
    )
    assert res["ok"] is True
    assert res["best"]["code"] == "abcd"
    assert res["best"]["score"] == 1.0
    assert all(
        res["history"][i] <= res["history"][i + 1]
        for i in range(len(res["history"]) - 1)
    )


def test_run_debate_spec():
    yes = run({"op": "debate", "candidate": "c", "ground": True})
    assert yes["supported"] is True
    no = run({"op": "debate", "candidate": "c", "ground": False})
    assert no["supported"] is False


def test_run_unknown_op():
    with pytest.raises(ValueError):
        run({"op": "nope"})
