import random
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.retriever_train import (  # noqa: E402
    build_label_matrix,
    dry_run,
    mine_hard_negatives,
    retriever_config,
)


# --- config ----------------------------------------------------------------

def test_retriever_config_documented_defaults():
    cfg = retriever_config()
    assert cfg["model"] == "google/byt5-small"
    assert cfg["num_negatives"] == 3
    assert cfg["num_in_file_negatives"] == 1
    assert cfg["loss"] == "mse_multipositive"
    assert cfg["seed"] == 3407


def test_retriever_config_override():
    cfg = retriever_config(num_negatives=5, batch_size=16)
    assert cfg["num_negatives"] == 5
    assert cfg["batch_size"] == 16


# --- hard-negative mining --------------------------------------------------

def test_mine_hard_negatives_position_gated():
    corpus = [
        {"name": "early_in", "end": 5, "in_file": True},   # in-file, before theorem
        {"name": "late_in", "end": 50, "in_file": True},   # in-file, AFTER theorem -> not accessible
        {"name": "imp1", "end": 0, "in_file": False},
        {"name": "imp2", "end": 0, "in_file": False},
    ]
    negs = mine_hard_negatives(
        corpus, theorem_pos=10, positives=[], num_negatives=3, num_in_file_negatives=1,
        rng=random.Random(0),
    )
    assert len(negs) == 3
    # exactly one in-file negative, and it must be the accessible (before) one
    assert "early_in" in negs
    assert "late_in" not in negs  # defined after the theorem -> excluded


def test_mine_hard_negatives_excludes_positives():
    corpus = [
        {"name": "p", "end": 1, "in_file": True},
        {"name": "imp1", "end": 0, "in_file": False},
        {"name": "imp2", "end": 0, "in_file": False},
    ]
    negs = mine_hard_negatives(
        corpus, theorem_pos=10, positives=["p"], num_negatives=2, num_in_file_negatives=1,
        rng=random.Random(0),
    )
    assert "p" not in negs


# --- MSE multi-positive label matrix --------------------------------------

def test_label_matrix_multi_positive_and_false_negative_delabel():
    # example 0 positive = "A", example 1 positive = "B".
    # example 1 also lists "A" as a negative -> but A is ex0's positive; the
    # in-batch column for A must be de-labeled to 1 for ex0 only.
    batch_pos = [["A"], ["B"]]
    batch_neg = [["X"], ["A"]]  # ex1's hard-neg "A" collides with ex0's positive
    out = build_label_matrix(batch_pos, batch_neg)
    cols = out["columns"]
    labels = out["labels"]
    # columns: [A, B, X, A]  (2 positives then 2 negs)
    assert cols == ["A", "B", "X", "A"]
    a0, a1 = cols.index("A"), len(cols) - 1  # first and last A columns
    # ex0's positive A columns both label 1
    assert labels[0][a0] == 1.0
    assert labels[0][a1] == 1.0  # false-negative de-labeled to 1 for ex0
    # ex1's positive is B, not A
    assert labels[1][cols.index("B")] == 1.0
    assert labels[1][a0] == 0.0


# --- dry run ---------------------------------------------------------------

def test_dry_run_every_row_has_positive():
    examples = [
        {
            "positives": ["A"],
            "theorem_pos": 10,
            "corpus": [
                {"name": "e1", "end": 1, "in_file": True},
                {"name": "i1", "end": 0, "in_file": False},
                {"name": "i2", "end": 0, "in_file": False},
            ],
        },
        {
            "positives": ["B"],
            "theorem_pos": 20,
            "corpus": [
                {"name": "e2", "end": 2, "in_file": True},
                {"name": "i3", "end": 0, "in_file": False},
            ],
        },
    ]
    out = dry_run(examples)
    assert out["ok"] is True and out["dry_run"] is True
    assert out["batch_size"] == 2
    assert out["all_rows_have_positive"] is True
    assert all(s >= 1.0 for s in out["positives_per_row"])


# --- dense-index scaffold: hash fallback (always available, no torch) -------

def test_dense_index_hash_fallback_ranks_sanely():
    from theoremata_tools.retriever_train import build_index, query_index

    premises = [
        {"name": "Nat.add_comm", "module": "Mathlib.Nat.Basic"},
        {"name": "Nat.mul_comm", "module": "Mathlib.Nat.Basic"},
        {"name": "List.append_nil", "module": "Mathlib.List.Basic"},
    ]
    index = build_index(premises, dim=64, backend="hash")
    assert index["backend"] == "hash"
    assert index["projection"] is None
    assert len(index["vectors"]) == 3

    res = query_index(index, "add commutative nat", k=2)
    assert res[0]["name"] == "Nat.add_comm"  # best lexical/embedding overlap
    assert len(res) == 2
    assert res[0]["score"] >= res[1]["score"]  # sorted descending


def test_dense_index_persist_roundtrip(tmp_path):
    from theoremata_tools.retriever_train import (
        build_index,
        load_index,
        query_index,
        save_index,
    )

    premises = [
        {"name": "Nat.add_comm", "module": "Mathlib.Nat.Basic"},
        {"name": "List.append_nil", "module": "Mathlib.List.Basic"},
    ]
    index = build_index(premises, dim=32, backend="hash")
    path = str(tmp_path / "index.json")
    save_index(index, path)

    loaded = load_index(path)
    assert loaded["dim"] == 32
    assert loaded["backend"] == "hash"
    assert query_index(loaded, "nat add", k=1)[0]["name"] == "Nat.add_comm"


def test_dense_index_torch_optional():
    import importlib.util

    import pytest

    if importlib.util.find_spec("torch") is None:
        pytest.skip("torch not installed")

    from theoremata_tools.retriever_train import build_index, query_index

    premises = [
        {"name": "Nat.add_comm", "module": "Mathlib.Nat.Basic"},
        {"name": "List.append_nil", "module": "Mathlib.List.Basic"},
    ]
    index = build_index(premises, dim=32, backend="torch")
    assert index["backend"] == "torch"
    assert index["projection"] is not None  # fitted, JSON-serializable matrix
    res = query_index(index, "nat add", k=1)
    assert res and res[0]["name"] in {"Nat.add_comm", "List.append_nil"}
