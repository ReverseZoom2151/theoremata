import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.progress_sft import (  # noqa: E402
    build_progress_dataset,
    dry_run,
    label_path,
    progress_config,
    sft_row,
)


# --- label format ----------------------------------------------------------

def test_sft_row_format():
    row = sft_row("x > 0", 3)
    assert row["prompt"] == "---\nSTATE_AFTER: x > 0\n\n---\nSTEPS_TO_NO_GOALS:"
    assert row["completion"] == " 3"


# --- label_path: steps + relative_progress = 1 - i/L -----------------------

def test_label_path_steps_and_relative():
    # path of 3 states + terminal: s0 -> s1 -> s2 -> "no goals" ; L = 3
    path = ["s0", "s1", "s2", "no goals"]
    rows = label_path(path, target="steps")
    # s0 is 3 steps away, s1 -> 2, s2 -> 1 ; terminal skipped
    assert [r["steps_to_no_goals"] for r in rows] == [3, 2, 1]
    # relative_progress = 1 - i/L
    assert abs(rows[0]["relative_progress"] - (1 - 3 / 3)) < 1e-5  # 0.0
    assert abs(rows[1]["relative_progress"] - (1 - 2 / 3)) < 1e-5
    assert abs(rows[2]["relative_progress"] - (1 - 1 / 3)) < 1e-5


def test_label_path_relative_target_label():
    path = ["s0", "s1", "no goals"]  # L = 2
    rows = label_path(path, target="relative")
    # labels are the relative_progress floats
    assert rows[0]["label"] == rows[0]["relative_progress"]
    assert abs(rows[0]["label"] - 0.0) < 1e-9
    assert abs(rows[1]["label"] - 0.5) < 1e-9


def test_label_path_trivial_empty():
    assert label_path(["no goals"]) == []
    assert label_path([]) == []


# --- dataset from traces ---------------------------------------------------

def test_build_from_states_trace():
    traces = [{"states": ["s0", "s1", "no goals"]}]
    ds = build_progress_dataset(traces, target="steps")
    assert len(ds["rows"]) == 2
    assert ds["rows"][0]["completion"] == " 2"
    assert ds["rows"][1]["completion"] == " 1"


def test_build_from_edges_trace_appends_terminal():
    traces = [{"edges": [{"state_before": "s0", "state_after": "s1"}], "proved": True}]
    ds = build_progress_dataset(traces, target="steps")
    # path becomes s0 -> s1 -> "no goals" ; L=2
    assert [r["meta"]["steps_to_no_goals"] for r in ds["rows"]] == [2, 1]


def test_build_skips_unlabelable():
    ds = build_progress_dataset([{"states": ["no goals"]}], target="steps")
    assert ds["rows"] == []
    assert ds["skipped"] == 1


# --- config + dry run ------------------------------------------------------

def test_progress_config_defaults_and_overrides():
    cfg = progress_config(max_epochs=8)
    assert cfg["model"] == "deepseek-ai/deepseek-coder-1.3b-base"
    assert cfg["learning_rate"] == 1e-6
    assert cfg["max_epochs"] == 8


def test_dry_run_validates_completions():
    traces = [{"states": ["s0", "s1", "no goals"]}]
    out = dry_run(traces, progress_config(target="relative"))
    assert out["ok"] is True and out["dry_run"] is True
    assert out["num_rows"] == 2
    assert out["target"] == "relative"


# --- chat-SFT: one honest step consuming the flywheel's JSONL --------------

def test_sft_finetune_dry_run_offline():
    from theoremata_tools.progress_sft import sft_finetune

    rows = [
        {"messages": [{"role": "user", "content": "a = a"},
                      {"role": "assistant", "content": "by simp"}]},
        {"messages": [{"role": "user", "content": "b = b"},
                      {"role": "assistant", "content": "by rfl"}]},
    ]
    out = sft_finetune(rows, backend="dry_run")
    assert out["ok"] is True
    assert out["trained"] is False  # honestly labelled not-a-real-train
    assert out["num_examples"] == 2
    assert out["num_tokens"] > 0
    assert out["uniform_baseline_loss"] >= 0.0


def test_flywheel_jsonl_is_valid_sft_input(tmp_path):
    # #1's output is a valid #3 input: shared JSONL schema, end-to-end offline.
    from theoremata_tools.flywheel import revolution
    from theoremata_tools.progress_sft import read_sft_jsonl, sft_finetune

    path = str(tmp_path / "sft.jsonl")
    rev = revolution([{"statement": "p"}, {"statement": "q"}], jsonl_path=path)
    assert rev["written"] == 2

    rows = read_sft_jsonl(path)
    assert all("messages" in r for r in rows)

    out = sft_finetune(path, backend="dry_run")
    assert out["num_examples"] == 2
    assert out["trained"] is False
    assert out["schema"] == rev["schema"]
