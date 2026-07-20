"""Integrity tests for the BRIDGE-style verified-programming runner.

The track grades structurally and executes no oracle. These tests exist so the
report can never quietly start reading as a verification result again: they
assert the not-verified markers are present and that no key in the response (or
in any per-item row) is named in a way a downstream report generator could
render as a verification verdict.
"""
import os
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

os.environ.setdefault("THEOREMATA_MODEL_MOCK", "1")

from theoremata_tools.benchmarks.registry import run as benchmark_run  # noqa: E402
from theoremata_tools.benchmarks.verified_programming import (  # noqa: E402
    GRADING_MODE,
    NOT_VERIFIED,
    run_verified_programming,
)

# Minimal BRIDGE row: real signature + real oracle I/O, so the "oracle exists
# but was not executed" accounting is exercised.
_BRIDGE_RECORD = {
    "task_id": "t1",
    "dataset_id": "bridge178",
    "problem_statement": "Return the minimum number of pushes.",
    "python": {
        "function_name": "minimumPushes",
        "function_signature": "def minimumPushes(word: str) -> int:\n    pass",
    },
    "lean": {
        "function_name": "minimumPushes",
        "function_signature": "def minimumPushes (word : String) : Int",
        "arguments": ["word"],
        "argument_types": ["String"],
    },
    "tests": {"inputs": [{"word": "abcde"}, {"word": "b"}], "expected_outputs": [5, 1]},
}

# Any key whose bare name would let a reader (or a template) treat the number as
# a verified pass rate. `structural_pass*` is deliberately not in this set.
_VERDICT_LOOKING_KEYS = {
    "accuracy",
    "correct",
    "is_correct",
    "pass_rate",
    "passed",
    "solved",
    "is_solved",
    "verified_count",
    "proved",
    "compile_pass",
}


@pytest.fixture()
def bridge_corpus(monkeypatch, tmp_path):
    import json

    d = tmp_path / "BRIDGE-main" / "BRIDGE-main" / "datasets"
    d.mkdir(parents=True)
    (d / "bridge178.jsonl").write_text(json.dumps(_BRIDGE_RECORD), encoding="utf-8")
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    return tmp_path


def test_report_is_marked_not_verified(bridge_corpus):
    report = run_verified_programming(benchmark="bridge178")
    assert report["n"] == 1
    assert report["verified"] is False
    assert report["is_verification_result"] is False
    assert report["verification_status"] == NOT_VERIFIED
    assert report["grading_mode"] == GRADING_MODE
    # the marker must be human-readable too, not only machine-readable
    assert "NOT A VERIFICATION RESULT" in report["disclaimer"]


def test_no_top_level_key_reads_as_a_verification_verdict(bridge_corpus):
    report = run_verified_programming(benchmark="bridge178")
    assert not (_VERDICT_LOOKING_KEYS & set(report))
    # the score itself must be named for what it measures
    assert "structural_pass_rate" in report
    assert "structural_pass" in report


def test_per_item_rows_are_marked_too(bridge_corpus):
    report = run_verified_programming(benchmark="bridge178")
    row = report["results"][0]
    assert not (_VERDICT_LOOKING_KEYS & set(row))
    assert row["verified"] is False
    assert row["verification_status"] == NOT_VERIFIED
    assert row["oracle_executed"] is False


def test_structural_pass_does_not_imply_a_correct_program(bridge_corpus):
    """The whole reason the rename matters: a constant-returning stub with the
    right signature is a structural pass and would fail every oracle case."""
    stub = "```lean\ndef minimumPushes (word : String) : Int := 0\n```"
    report = run_verified_programming(
        benchmark="bridge178", responses={"bridge178:t1": stub}
    )
    assert report["structural_pass"] == 1
    assert report["structural_pass_rate"] == 1.0
    # ... and it is still explicitly not a verification result
    assert report["verified"] is False
    assert report["results"][0]["structural_pass"] is True
    assert report["results"][0]["verified"] is False


def test_oracle_gap_is_reported_as_a_number(bridge_corpus):
    report = run_verified_programming(benchmark="bridge178")
    assert report["oracle_available"] == 1
    assert report["oracle_executed"] == 0
    assert "did NOT execute" in report["oracle_note"]


def test_absent_corpus_skips_cleanly_and_stays_marked(monkeypatch, tmp_path):
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path / "empty"))
    report = run_verified_programming(benchmark="bridge178")
    assert report["n"] == 0
    assert report["corpus_present"] is False
    assert report["structural_pass_rate"] is None
    assert report["verified"] is False
    assert report["results"] == []


def test_unknown_benchmark_raises():
    with pytest.raises(KeyError):
        run_verified_programming(benchmark="does_not_exist")


def test_registry_dispatch_preserves_the_markers(bridge_corpus):
    """The registry is the surface every downstream consumer sees; the markers
    must survive it rather than being stripped by the op wrapper."""
    out = benchmark_run({"op": "verified_programming", "benchmark": "bridge178"})
    assert out["op"] == "verified_programming"
    assert out["verified"] is False
    assert out["verification_status"] == NOT_VERIFIED
    assert not (_VERDICT_LOOKING_KEYS & set(out))
