import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.process_supervision import (  # noqa: E402
    REWARD_FAIL,
    REWARD_PASS,
    backup_q,
    graded_revolution_with_process,
    predict_value,
    step_beam_select,
    train_value_head,
    value_targets_from_tree,
)


# --- tree builders ---------------------------------------------------------

def _two_wins_one_loss_tree():
    # root(0) -> A(1); A has three terminal children: pass, pass, fail.
    return {
        "nodes": [
            {"id": 0, "parent": None},
            {"id": 1, "parent": 0, "step_final": True},
            {"id": 2, "parent": 1, "passed": True},
            {"id": 3, "parent": 1, "passed": True},
            {"id": 4, "parent": 1, "passed": False},
        ]
    }


# --- Q-backup --------------------------------------------------------------

def test_backup_q_two_wins_one_loss_is_one_third():
    acc = backup_q(_two_wins_one_loss_tree())
    assert acc[1]["visits"] == 3
    assert abs(acc[1]["q"] - 1.0 / 3.0) < 1e-12
    # root sees the same three simulations.
    assert abs(acc[0]["q"] - 1.0 / 3.0) < 1e-12
    # a leaf's own Q is its terminal reward.
    assert abs(acc[2]["q"] - REWARD_PASS) < 1e-12
    assert abs(acc[4]["q"] - REWARD_FAIL) < 1e-12


def test_passing_gate_gives_positive_q_up_the_path():
    tree = {
        "nodes": [
            {"id": 0, "parent": None},
            {"id": 1, "parent": 0, "step_final": True},
            {"id": 2, "parent": 1, "terminal": 1.0},
        ]
    }
    acc = backup_q(tree)
    assert all(acc[i]["q"] > 0.0 for i in (0, 1, 2))
    assert abs(acc[0]["q"] - 1.0) < 1e-12


# --- value targets ---------------------------------------------------------

def test_value_targets_are_step_final_only_and_ordered():
    tree = {
        "nodes": [
            {"id": 0, "parent": None},  # not step-final
            {"id": 1, "parent": 0, "step_final": True, "features": [1.0]},
            {"id": 2, "parent": 1, "step_final": True, "features": [2.0]},
            {"id": 3, "parent": 2, "passed": True},
            {"id": 4, "parent": 1, "passed": False},
        ]
    }
    vt = value_targets_from_tree(tree)
    ids = [t["node_id"] for t in vt["targets"]]
    assert ids == [1, 2]
    # node 1 sees one win + one loss -> Q = 0; node 2 sees the single win -> Q = 1.
    q_by_id = {t["node_id"]: t["q"] for t in vt["targets"]}
    assert abs(q_by_id[1] - 0.0) < 1e-12
    assert abs(q_by_id[2] - 1.0) < 1e-12
    # features ride along for training.
    assert vt["targets"][1]["features"] == [2.0]


def test_backup_is_deterministic():
    t = _two_wins_one_loss_tree()
    assert value_targets_from_tree(t) == value_targets_from_tree(t)


# --- step-level beam search ------------------------------------------------

def test_step_beam_picks_highest_value_with_id_tiebreak():
    candidates = [
        {"id": 1, "value_estimate": 0.2},
        {"id": 2, "value_estimate": 0.9},
        {"id": 3, "value_estimate": 0.5},
        {"id": 4, "value_estimate": 0.9},  # ties with id 2
    ]
    assert step_beam_select(candidates, 2) == [2, 4]  # tie breaks to smaller id
    assert step_beam_select(candidates, 99) == [2, 4, 3, 1]
    assert step_beam_select(candidates, 0) == []


# --- fallback value head ---------------------------------------------------

def _training_examples():
    # A feature that correlates positively with Q: higher feature -> higher Q.
    return [
        {"features": [x], "q": q}
        for x, q in [(-1.0, -1.0), (-0.5, -0.5), (0.0, 0.0), (0.5, 0.5), (1.0, 1.0)]
    ]


def test_fallback_value_head_fits_and_ranks_high_q_above_low_q():
    model = train_value_head(_training_examples(), backend="fallback")
    assert model["backend"] == "fallback"
    high = predict_value(model, [1.0])
    low = predict_value(model, [-1.0])
    assert high > low
    # predictions stay within the tanh/clip range.
    assert -1.0 <= low <= 1.0 and -1.0 <= high <= 1.0


def test_fallback_is_deterministic():
    ex = _training_examples()
    a = train_value_head(ex, backend="fallback")
    b = train_value_head(ex, backend="fallback")
    assert a["w"] == b["w"] and a["b"] == b["b"]


def test_empty_examples_return_zero_model():
    model = train_value_head([], backend="fallback")
    assert model["backend"] == "empty"
    assert predict_value(model, [1.0, 2.0]) == 0.0


# --- torch value head (gated) ----------------------------------------------

def test_torch_value_head_trains_and_predicts():
    pytest.importorskip("torch")
    model = train_value_head(_training_examples(), backend="torch", seed=0, steps=300)
    assert model["backend"] == "torch"
    assert model["squash"] is True
    # After training on a monotone target, high feature predicts above low feature.
    assert predict_value(model, [1.0]) > predict_value(model, [-1.0])
    # tanh head stays bounded.
    assert -1.0 <= predict_value(model, [5.0]) <= 1.0


# --- graded process hook ---------------------------------------------------

def test_graded_process_feeds_r_score_and_r_meta():
    # A step-final node on a winning path (Q = 1) whose value head agrees (V = 1)
    # earns the maximal graded reward; the formal gate stays the hard oracle.
    tree = {
        "nodes": [
            {"id": 0, "parent": None},
            {"id": 1, "parent": 0, "step_final": True, "value_estimate": 1.0},
            {"id": 2, "parent": 1, "passed": True},
        ]
    }
    out = graded_revolution_with_process([tree])
    assert out["ok"] and out["n_nodes"] == 1
    row = out["rows"][0]
    assert abs(row["q"] - 1.0) < 1e-12
    assert abs(row["r_meta"] - 1.0) < 1e-12  # (q+1)/2
    assert abs(row["r_score"] - 1.0) < 1e-12  # perfect faithfulness
    assert abs(row["reward"] - 1.0) < 1e-12  # R_format * R_score * R_meta


def test_graded_process_penalizes_losing_step_confidence():
    # A losing step (Q = -1) has zero MC win-rate -> R_meta = 0 -> reward 0,
    # regardless of the critic's faithfulness. The soft signal reflects the search.
    tree = {
        "nodes": [
            {"id": 0, "parent": None},
            {"id": 1, "parent": 0, "step_final": True, "value_estimate": -1.0},
            {"id": 2, "parent": 1, "passed": False},
        ]
    }
    row = graded_revolution_with_process([tree])["rows"][0]
    assert abs(row["q"] + 1.0) < 1e-12
    assert abs(row["r_meta"] - 0.0) < 1e-12
    assert abs(row["reward"] - 0.0) < 1e-12
