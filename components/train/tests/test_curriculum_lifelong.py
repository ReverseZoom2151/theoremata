import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.lifelong_curriculum import (  # noqa: E402
    ResumableCurriculum,
    bin_theorems,
    difficulty,
    sort_repositories,
    theorem_identifier,
)


# --- difficulty ------------------------------------------------------------

def test_difficulty_exp_sorry_none():
    assert difficulty(["a", "b"]) == math.exp(2)
    assert difficulty("foo\nbar\nsorry") == math.inf
    assert difficulty("") is None
    assert difficulty(None) is None
    assert difficulty([]) is None


# --- percentile binning ----------------------------------------------------

def test_bin_theorems_percentile_terciles():
    # difficulties 1..9 -> 33rd/67th percentile split into three tiers
    diffs = [float(i) for i in range(1, 10)]
    buckets = bin_theorems(diffs)
    assert buckets.count("Easy") >= 3
    assert buckets.count("Hard") >= 3
    # smallest is Easy, largest is Hard
    assert buckets[0] == "Easy"
    assert buckets[-1] == "Hard"


def test_bin_theorems_inf_is_hard():
    buckets = bin_theorems([1.0, 2.0, 3.0, math.inf])
    assert buckets[-1] == "Hard"


def test_bin_theorems_round_robin_none_distribution():
    # 3 None (no-proof) items are spread round-robin Easy/Medium/Hard
    diffs = [1.0, 2.0, 3.0, None, None, None]
    buckets = bin_theorems(diffs)
    none_buckets = buckets[3:]
    assert none_buckets == ["Easy", "Medium", "Hard"]  # round-robin order


def test_bin_theorems_all_none():
    # no finite difficulties: Nones still distribute round-robin
    buckets = bin_theorems([None, None, None, None])
    assert buckets == ["Easy", "Medium", "Hard", "Easy"]


# --- repo ordering by easy-count ------------------------------------------

def test_sort_repositories_by_easy_count_desc():
    repos = [
        {"name": "hard_repo", "theorems": [{"difficulty": 9.0}, {"difficulty": 8.0}]},
        {"name": "easy_repo", "theorems": [{"difficulty": 1.0}, {"difficulty": 1.5}]},
    ]
    ordered = sort_repositories(repos)
    # easy_repo has more Easy theorems -> comes first
    assert ordered[0]["name"] == "easy_repo"
    assert ordered[0]["easy_count"] >= ordered[1]["easy_count"]


def test_sort_repositories_uses_proof_payload():
    repos = [
        {"name": "r1", "theorems": [{"proof": "a"}]},  # 1 step -> easy end
        {"name": "r2", "theorems": [{"proof": "a\nb\nc\nd\ne"}]},  # 5 steps -> hard
    ]
    ordered = sort_repositories(repos)
    assert ordered[0]["name"] == "r1"


# --- resumable loop --------------------------------------------------------

def test_theorem_identifier_hashable_and_stable():
    t = {"full_name": "T", "file_path": "f.lean", "start": [1, 2], "end": [3, 4]}
    key = theorem_identifier(t)
    assert key == ("T", "f.lean", (1, 2), (3, 4))
    assert hash(key)  # hashable


def test_resumable_dedup_and_checkpoint_roundtrip():
    cur = ResumableCurriculum()
    thms = [
        {"full_name": "A", "file_path": "f", "start": [1, 0], "end": [2, 0]},
        {"full_name": "B", "file_path": "f", "start": [3, 0], "end": [4, 0]},
        {"full_name": "A", "file_path": "f", "start": [1, 0], "end": [2, 0]},  # dup
    ]
    fresh = cur.filter_new(thms)
    assert len(fresh) == 2  # dup dropped

    # checkpoint -> restore -> already-seen theorems are skipped
    ckpt = cur.checkpoint()
    restored = ResumableCurriculum.restore(ckpt)
    assert restored.filter_new(thms) == []  # all already encountered


def test_resumable_skips_across_runs():
    cur = ResumableCurriculum()
    t = {"full_name": "A", "file_path": "f", "start": [1, 0], "end": [2, 0]}
    assert cur.seen(t) is False
    cur.mark(t)
    assert cur.seen(t) is True
