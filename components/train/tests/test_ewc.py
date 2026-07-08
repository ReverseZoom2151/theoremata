import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.ewc import (  # noqa: E402
    EWCState,
    compute_fisher,
    ewc_penalty,
    run,
)


# --- ewc_penalty math ------------------------------------------------------

def test_ewc_penalty_scalar_params():
    params = {"a": 3.0, "b": 1.0}
    prev = {"a": 1.0, "b": 1.0}
    fisher = {"a": 2.0, "b": 5.0}
    # lambda * sum fisher*(theta-prev)^2 = 0.1 * (2*(2)^2 + 5*(0)^2) = 0.1*8 = 0.8
    assert abs(ewc_penalty(params, prev, fisher, lam=0.1) - 0.8) < 1e-9


def test_ewc_penalty_nested_lists():
    params = {"w": [1.0, 2.0]}
    prev = {"w": [0.0, 0.0]}
    fisher = {"w": [1.0, 0.5]}
    # 1*(1)^2 + 0.5*(2)^2 = 1 + 2 = 3 ; lam=1 -> 3.0
    assert abs(ewc_penalty(params, prev, fisher, lam=1.0) - 3.0) < 1e-9


def test_ewc_penalty_gated_off():
    params = {"a": 5.0}
    prev = {"a": 0.0}
    fisher = {"a": 1.0}
    assert ewc_penalty(params, prev, fisher, lam=0.0) == 0.0  # lambda 0 -> off
    assert ewc_penalty(params, prev, {}, lam=0.1) == 0.0  # no fisher -> off


def test_ewc_penalty_only_shared_names():
    # a fresh param not in prev/fisher is unconstrained
    params = {"a": 1.0, "fresh": 99.0}
    prev = {"a": 0.0}
    fisher = {"a": 1.0}
    assert ewc_penalty(params, prev, fisher, lam=1.0) == 1.0


# --- fisher computation ----------------------------------------------------

def test_compute_fisher_mean_squared_grads():
    grads = [{"a": 2.0}, {"a": 4.0}]
    # mean of squares = (4 + 16)/2 = 10
    fisher = compute_fisher(grads)
    assert abs(fisher["a"] - 10.0) < 1e-9


def test_compute_fisher_dataset_size_denominator():
    grads = [{"a": 2.0}, {"a": 4.0}]
    fisher = compute_fisher(grads, dataset_size=4)
    # (4 + 16)/4 = 5
    assert abs(fisher["a"] - 5.0) < 1e-9


def test_compute_fisher_empty():
    assert compute_fisher([]) == {}


# --- EWCState roundtrip ----------------------------------------------------

def test_ewc_state_from_task_and_penalty():
    params_task1 = {"a": 1.0}
    grads = [{"a": 3.0}]  # fisher = 9
    state = EWCState.from_task(params_task1, grads, lam=0.5)
    assert abs(state.fisher["a"] - 9.0) < 1e-9
    # now params drift to 2.0: penalty = 0.5 * 9 * (2-1)^2 = 4.5
    assert abs(state.penalty({"a": 2.0}) - 4.5) < 1e-9


def test_run_dispatch():
    out = run(
        {
            "op": "penalty",
            "params": {"a": 2.0},
            "prev_params": {"a": 0.0},
            "fisher": {"a": 1.0},
            "lam": 1.0,
        }
    )
    assert out["penalty"] == 4.0
