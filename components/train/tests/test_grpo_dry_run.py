import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.grpo import (  # noqa: E402
    dry_run_grpo,
    grpo_config,
    linear_temperature,
    run,
)
from theoremata_tools.reward import make_reward_fn  # noqa: E402


# --- temperature annealing -------------------------------------------------

def test_config_carries_temperature_annealing():
    cfg = grpo_config("m", "d")
    assert cfg["temperature_start"] == 1.2
    assert cfg["temperature_end"] == 0.7


def test_linear_temperature_interpolates():
    cfg = grpo_config("m", "d")
    assert linear_temperature(0, 100, cfg) == 1.2
    assert linear_temperature(100, 100, cfg) == 0.7
    assert abs(linear_temperature(50, 100, cfg) - 0.95) < 1e-9
    # clamped + guard
    assert linear_temperature(-5, 100, cfg) == 1.2
    assert linear_temperature(200, 100, cfg) == 0.7
    assert linear_temperature(5, 0, cfg) == 1.2  # total_steps<=0 guard


# --- dry run consumes a tiny synthetic dataset, no GPU ---------------------

def _dataset():
    # a 4-sample group: 2 pass, 1 fail, 1 skipped (missing gold)
    return [
        {"group": "g1", "gold": "t", "verdict": {"compiled": True, "axioms_ok": True}},
        {"group": "g1", "gold": "t", "verdict": {"compiled": True, "axioms_ok": True},
         "used_tool": True},
        {"group": "g1", "gold": "t", "verdict": {"compiled": False, "axioms_ok": True}},
        {"group": "g1", "verdict": {"compiled": True, "axioms_ok": True}},  # no gold -> skip
    ]


def test_dry_run_scores_and_skips_without_gpu():
    cfg = grpo_config("tiny/model", "data.jsonl", max_steps=10)
    out = dry_run_grpo(_dataset(), make_reward_fn(tool_weight=0.1), cfg)
    assert out["ok"] is True
    assert out["dry_run"] is True
    assert out["dataset_size"] == 4
    assert out["scored"] == 3
    assert out["skipped"] == 1  # missing-gold sample dropped, not punished
    # group g1: rewards [1.0, 1.1, 0.0] -> mixed -> Goldilocks keeps it
    assert out["num_groups"] == 1
    assert out["groups_kept"] == 1
    assert out["temperature_schedule"]["start"] == 1.2
    assert out["temperature_schedule"]["end"] == 0.7
    # dry run must not import trl / torch
    assert "trl" not in sys.modules
    assert "torch" not in sys.modules


def test_dry_run_drops_all_pass_group_via_goldilocks():
    ds = [
        {"group": "g", "gold": "t", "verdict": {"compiled": True, "axioms_ok": True}},
        {"group": "g", "gold": "t", "verdict": {"compiled": True, "axioms_ok": True}},
    ]
    out = dry_run_grpo(ds, make_reward_fn(tool_weight=0.0), grpo_config("m", "d"))
    assert out["num_groups"] == 1
    assert out["groups_kept"] == 0  # all-pass -> zero gradient -> dropped


def test_dry_run_loads_jsonl_path(tmp_path):
    import json

    path = tmp_path / "ds.jsonl"
    with open(path, "w", encoding="utf-8") as fh:
        for s in _dataset():
            fh.write(json.dumps(s) + "\n")
    out = dry_run_grpo(str(path), make_reward_fn(), grpo_config("m", "d"))
    assert out["dataset_size"] == 4


def test_dry_run_accepts_harvester_result_dict():
    # a harvester-style {"rows": [...]} where rows carry verdict+gold
    rows = {"rows": _dataset()}
    out = dry_run_grpo(rows, make_reward_fn(), grpo_config("m", "d"))
    assert out["dataset_size"] == 4


def test_run_dispatch_dry_run_op():
    out = run({"op": "dry_run", "dataset": _dataset(), "model": "m", "dataset_path": "d"})
    assert out["ok"] is True
    assert out["scored"] == 3
