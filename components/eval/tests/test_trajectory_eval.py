import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

import pytest  # noqa: E402

from theoremata_tools.trajectory_eval import (  # noqa: E402
    pass_at_k,
    redundancy_rate,
    run,
    score_trajectory,
    tool_use_accuracy,
    trajectory_efficiency,
)


# --- efficiency -----------------------------------------------------------

def test_efficiency_optimal_when_agent_equals_optimal():
    assert trajectory_efficiency(5, 5) == 1.0


def test_efficiency_less_than_one_when_longer():
    assert trajectory_efficiency(10, 5) == pytest.approx(0.5)
    assert trajectory_efficiency(20, 5) < trajectory_efficiency(10, 5)


def test_efficiency_capped_at_one_when_agent_shorter():
    # beating the reference does not score above optimal
    assert trajectory_efficiency(3, 5) == 1.0


def test_efficiency_degenerate_lengths_are_zero():
    assert trajectory_efficiency(0, 5) == 0.0
    assert trajectory_efficiency(5, 0) == 0.0


# --- redundancy -----------------------------------------------------------

def test_redundancy_zero_for_all_distinct():
    steps = [{"tool": "a"}, {"tool": "b"}, {"tool": "c"}]
    assert redundancy_rate(steps) == 0.0


def test_redundancy_detects_repeated_steps():
    steps = [{"tool": "a"}, {"tool": "a"}, {"tool": "b"}, {"tool": "a"}]
    # two of the four steps are exact repeats of an earlier step
    assert redundancy_rate(steps) == pytest.approx(0.5)


def test_redundancy_detects_no_progress_flag():
    steps = [
        {"tool": "a", "progress": True},
        {"tool": "b", "progress": False},
        {"tool": "c", "wasted": True},
    ]
    assert redundancy_rate(steps) == pytest.approx(2 / 3)


def test_redundancy_empty_is_zero():
    assert redundancy_rate([]) == 0.0


# --- tool-use accuracy ----------------------------------------------------

def test_tool_use_accuracy_counts_ok_and_error():
    calls = [
        {"tool": "x", "ok": True},
        {"tool": "y", "ok": False},
        {"tool": "z", "ok": True},
    ]
    res = tool_use_accuracy(calls)
    assert res["n_ok"] == 2
    assert res["n_error"] == 1
    assert res["n"] == 3
    assert res["accuracy"] == pytest.approx(2 / 3)


def test_tool_use_accuracy_error_key_and_status():
    calls = [
        {"tool": "x", "error": None},
        {"tool": "y", "error": "boom"},
        {"tool": "z", "status": "ok"},
        {"tool": "w", "status": "error"},
    ]
    res = tool_use_accuracy(calls)
    assert res["n_ok"] == 2
    assert res["n_error"] == 2


def test_tool_use_accuracy_empty():
    res = tool_use_accuracy([])
    assert res == {"accuracy": 0.0, "n_ok": 0, "n_error": 0, "n": 0}


# --- unbiased pass@k ------------------------------------------------------

def test_pass_at_k_known_formula():
    # n=10, c=3, k=1 -> probability a single draw is correct = 3/10
    assert pass_at_k(10, 3, 1) == pytest.approx(0.3)


def test_pass_at_k_zero_correct():
    assert pass_at_k(10, 0, 5) == 0.0


def test_pass_at_k_all_correct():
    assert pass_at_k(10, 10, 3) == 1.0


def test_pass_at_k_matches_bruteforce():
    from itertools import combinations

    def brute(n, c, k):
        idx = range(n)
        correct = set(range(c))
        subs = list(combinations(idx, k))
        hits = sum(1 for s in subs if correct & set(s))
        return hits / len(subs)

    for n in range(1, 9):
        for c in range(0, n + 1):
            for k in range(1, n + 1):
                assert pass_at_k(n, c, k) == pytest.approx(brute(n, c, k))


def test_pass_at_k_k_exceeds_n_raises():
    with pytest.raises(ValueError):
        pass_at_k(5, 2, 6)


def test_pass_at_k_negative_raises():
    with pytest.raises(ValueError):
        pass_at_k(-1, 0, 0)


def test_pass_at_k_clamps_c_over_n():
    assert pass_at_k(5, 99, 2) == 1.0


# --- score_trajectory -----------------------------------------------------

def test_score_trajectory_combines_axes():
    traj = {
        "steps": [{"tool": "a"}, {"tool": "a"}, {"tool": "b"}, {"tool": "c"}],
        "optimal_len": 2,
        "tool_calls": [{"ok": True}, {"ok": False}],
    }
    res = score_trajectory(traj)
    assert res["n_steps"] == 4
    # agent_len defaults to len(steps)=4, optimal=2 -> 0.5
    assert res["efficiency"] == pytest.approx(0.5)
    # one repeated step out of four
    assert res["redundancy"] == pytest.approx(0.25)
    assert res["tool_accuracy"]["accuracy"] == pytest.approx(0.5)


def test_score_trajectory_explicit_agent_len_overrides_steps():
    traj = {"steps": [{"tool": "a"}], "agent_len": 8, "optimal_len": 4}
    res = score_trajectory(traj)
    assert res["efficiency"] == pytest.approx(0.5)
    assert res["n_steps"] == 1


def test_score_trajectory_deterministic():
    traj = {
        "steps": [{"tool": "a"}, {"tool": "a"}, {"tool": "b"}],
        "optimal_len": 2,
        "tool_calls": [{"ok": True}, {"ok": True}, {"ok": False}],
    }
    assert score_trajectory(traj) == score_trajectory(traj)


# --- run() dispatch -------------------------------------------------------

def test_run_default_op_is_score_trajectory():
    res = run({"trajectory": {"steps": [{"tool": "a"}], "optimal_len": 1}})
    assert res["op"] == "score_trajectory"
    assert res["efficiency"] == 1.0
    assert res["n_steps"] == 1


def test_run_pass_at_k():
    res = run({"op": "pass_at_k", "n": 10, "c": 3, "k": 1})
    assert res["pass_at_k"] == pytest.approx(0.3)
    assert (res["n"], res["c"], res["k"]) == (10, 3, 1)


def test_run_efficiency_and_redundancy_and_tools():
    assert run({"op": "efficiency", "agent_len": 4, "optimal_len": 2})["efficiency"] == pytest.approx(0.5)
    assert run({"op": "redundancy", "steps": [{"t": 1}, {"t": 1}]})["redundancy"] == pytest.approx(0.5)
    tr = run({"op": "tool_use_accuracy", "tool_calls": [{"ok": True}, {"ok": False}]})
    assert tr["n_ok"] == 1 and tr["n_error"] == 1


def test_run_unknown_op_raises():
    with pytest.raises(ValueError):
        run({"op": "nope"})
