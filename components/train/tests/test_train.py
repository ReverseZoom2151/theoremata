import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.sft_export import (  # noqa: E402
    rationalize,
    star_dataset,
    write_jsonl,
)
from theoremata_tools.grpo import (  # noqa: E402
    goldilocks_keep,
    grpo_config,
    reward_from_verifier,
    train,
)


# --- star_dataset ---------------------------------------------------------

def test_star_dataset_keeps_only_verified_and_axioms_ok():
    records = [
        {"goal": "g1", "proof": "p1", "verified": True, "axioms_ok": True},
        {"goal": "g2", "proof": "p2", "verified": False, "axioms_ok": True},
        {"goal": "g3", "proof": "p3", "verified": True, "axioms_ok": False},
    ]
    out = star_dataset(records)
    assert out["ok"] is True
    assert out["kept"] == 1
    assert out["dropped"] == 2
    assert out["rows"][0]["messages"][0]["content"] == "g1"


def test_star_dataset_axioms_ok_defaults_true():
    # absent axioms_ok means the audit was not run -> not a rejection reason
    out = star_dataset([{"goal": "g", "proof": "p", "verified": True}])
    assert out["kept"] == 1


def test_star_dataset_dedupes_by_goal_and_proof():
    records = [
        {"goal": "g", "proof": "p", "verified": True, "axioms_ok": True},
        {"goal": "g", "proof": "p", "verified": True, "axioms_ok": True},
        {"goal": "g", "proof": "p2", "verified": True, "axioms_ok": True},
    ]
    out = star_dataset(records)
    assert out["kept"] == 2  # (g,p) once + (g,p2)
    assert out["dropped"] == 1  # the duplicate (g,p)


def test_sft_row_shape():
    out = star_dataset([{"goal": "G", "proof": "P", "verified": True}])
    row = out["rows"][0]
    assert row == {
        "messages": [
            {"role": "user", "content": "G"},
            {"role": "assistant", "content": "P"},
        ]
    }


def test_star_dataset_empty():
    out = star_dataset([])
    assert out == {"ok": True, "kept": 0, "dropped": 0, "rows": []}


# --- rationalize ----------------------------------------------------------

def test_rationalize_builds_row_from_sketch():
    row = rationalize({"goal": "hard goal"}, "step1; step2")
    assert row["messages"][0]["content"] == "hard goal"
    assert row["messages"][1]["content"] == "step1; step2"
    assert row["rationalized"] is True


def test_rationalize_prefers_existing_proof():
    row = rationalize({"goal": "g", "proof": "real"}, "sketch")
    assert row["messages"][1]["content"] == "real"


# --- write_jsonl ----------------------------------------------------------

def test_write_jsonl_roundtrip(tmp_path):
    import json

    out = star_dataset([{"goal": "g", "proof": "p", "verified": True}])
    path = tmp_path / "data.jsonl"
    n = write_jsonl(out["rows"], str(path))
    assert n == 1
    lines = path.read_text(encoding="utf-8").splitlines()
    assert json.loads(lines[0]) == out["rows"][0]


# --- grpo_config ----------------------------------------------------------

def test_grpo_config_expected_keys():
    cfg = grpo_config("Qwen/model", "data.jsonl")
    for key in (
        "model",
        "dataset_path",
        "output_dir",
        "num_generations",
        "learning_rate",
        "beta",
        "epsilon",
        "max_steps",
        # DAPO-style knobs
        "epsilon_high",
        "loss_type",
        "mask_truncated_completions",
        "overlong_filter",
    ):
        assert key in cfg, key
    assert cfg["model"] == "Qwen/model"
    assert cfg["dataset_path"] == "data.jsonl"


def test_grpo_config_overrides_applied():
    cfg = grpo_config("m", "d", num_generations=16, learning_rate=5e-7, custom="x")
    assert cfg["num_generations"] == 16
    assert cfg["learning_rate"] == 5e-7
    assert cfg["custom"] == "x"  # unknown keys pass through


# --- reward_from_verifier -------------------------------------------------

def test_reward_from_verifier_maps_stub():
    verifier = lambda c: c == "good"  # noqa: E731
    rewards = reward_from_verifier(["good", "bad", "good"], verifier)
    assert rewards == [1.0, 0.0, 1.0]


# --- goldilocks_keep ------------------------------------------------------

def test_goldilocks_keep_mixed_group():
    assert goldilocks_keep([1.0, 0.0, 1.0]) is True


def test_goldilocks_drop_all_pass():
    assert goldilocks_keep([1.0, 1.0, 1.0]) is False


def test_goldilocks_drop_all_fail():
    assert goldilocks_keep([0.0, 0.0, 0.0]) is False


def test_goldilocks_drop_empty():
    assert goldilocks_keep([]) is False


# --- train (dry run, no TRL) ----------------------------------------------

def test_train_dry_run_returns_would_run_without_trl():
    cfg = grpo_config("m", "d")
    out = train(cfg, dry_run=True)
    assert out["ok"] is True
    assert out["dry_run"] is True
    assert out["would_run"] == cfg
    # dry_run must not have imported trl
    assert "trl" not in sys.modules
