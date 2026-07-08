import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.selector import (  # noqa: E402
    evaluate,
    load,
    run,
    save,
    select,
    train,
)


def _toy_logs():
    """lean is the historically-best backend (solves fast); rocq is slow;
    isabelle rarely solves."""
    logs = []
    for i in range(4):
        f = {"size": float(i), "horn": 1.0}
        logs.append({"problem": f"p{i}", "backend": "lean", "solved": True, "time": 1.0, "features": f})
        logs.append({"problem": f"p{i}", "backend": "rocq", "solved": True, "time": 8.0, "features": f})
        logs.append({"problem": f"p{i}", "backend": "isabelle", "solved": i == 0, "time": 3.0, "features": f})
    return logs


# --- fallback ranks the historically-best backend first --------------------

def test_fallback_ranks_best_backend_first():
    model = train(_toy_logs(), backend="fallback")
    assert model["lib"] == "fallback"
    ranked = select(model, {"size": 1.0, "horn": 1.0}, budget=10.0)
    assert ranked[0]["backend"] == "lean"  # highest solve frequency
    names = [r["backend"] for r in ranked]
    assert set(names) == {"lean", "rocq", "isabelle"}


# --- select respects the compute budget ------------------------------------

def test_select_respects_budget():
    model = train(_toy_logs(), backend="fallback")
    # tight budget: only lean (1.0s) fits; rocq (8.0s) and isabelle (3.0s) don't
    ranked = select(model, {"size": 1.0, "horn": 1.0}, budget=2.0)
    by = {r["backend"]: r for r in ranked}
    assert by["lean"]["p_solve_within_budget"] == 1.0
    assert by["rocq"]["p_solve_within_budget"] == 0.0  # 8.0s > 2.0 budget
    assert by["isabelle"]["p_solve_within_budget"] == 0.0  # 3.0s > 2.0 budget
    assert ranked[0]["backend"] == "lean"

    # loosen the budget and rocq's solves now count
    ranked2 = select(model, {"size": 1.0, "horn": 1.0}, budget=10.0)
    by2 = {r["backend"]: r for r in ranked2}
    assert by2["rocq"]["p_solve_within_budget"] == 1.0


# --- scored on total-solved-per-budget, not per-backend accuracy -----------

def test_evaluate_total_solved_per_budget():
    model = train(_toy_logs(), backend="fallback")
    res = evaluate(model, _toy_logs(), budget=2.0)
    # under a 2.0s budget only lean solves within budget -> selector solves all 4
    assert res["solved_by_selector"] == 4
    assert res["beats_best_single"] is True


# --- persist / load round-trips --------------------------------------------

def test_persist_load_round_trip(tmp_path):
    model = train(_toy_logs(), backend="fallback")
    path = str(tmp_path / "selector.json")
    save(model, path)
    reloaded = load(path)
    assert reloaded == model
    # selection is identical after a round-trip
    a = select(model, {"size": 1.0, "horn": 1.0}, budget=5.0)
    b = select(reloaded, {"size": 1.0, "horn": 1.0}, budget=5.0)
    assert a == b


# --- feature margin still ranks when frequencies tie -----------------------

def test_feature_margin_breaks_ties():
    # both backends solve the same problems within budget (freq ties), but 'a'
    # succeeds on high-size goals and 'b' on low-size goals -> margin decides.
    # centered feature (the margin is linear through the origin, so the sign of
    # the query feature is what selects the backend).
    logs = []
    for size in (-1.5, -0.5, 0.5, 1.5):
        big = size > 0
        logs.append({"problem": f"p{size}", "backend": "a", "solved": big, "time": 1.0, "features": {"size": size}})
        logs.append({"problem": f"p{size}", "backend": "b", "solved": not big, "time": 1.0, "features": {"size": size}})
    model = train(logs, backend="fallback")
    # a high-size goal should prefer 'a'
    ranked = select(model, {"size": 2.0}, budget=10.0)
    assert ranked[0]["backend"] == "a"
    # a low-size goal should prefer 'b'
    ranked2 = select(model, {"size": -2.0}, budget=10.0)
    assert ranked2[0]["backend"] == "b"


# --- worker run() dispatch --------------------------------------------------

def test_run_train_and_select():
    model = run({"op": "train", "backend": "fallback", "logs": _toy_logs()})
    out = run({"op": "select", "model": model, "problem_features": {"size": 1.0}, "budget": 10.0})
    assert out["ranked"][0]["backend"] == "lean"


# --- optional real-model path, honestly gated ------------------------------

def test_sklearn_path_when_available():
    pytest.importorskip("sklearn")
    model = train(_toy_logs(), backend="sklearn")
    assert model["lib"] in ("sklearn", "fallback")  # fallback if single-class per backend
    ranked = select(model, {"size": 1.0, "horn": 1.0}, budget=10.0)
    assert ranked[0]["backend"] == "lean"
