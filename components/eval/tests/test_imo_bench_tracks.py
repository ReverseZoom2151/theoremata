"""IMO-Bench AnswerBench + GradingBench track tests (Tier 4).

Offline / deterministic. Each loader parses a tiny JSONL fixture (written into a
temporary resource root) into the standard item schema; the graders are checked
directly. Both loaders must return ``[]`` and skip cleanly when the corpus is
absent — never raise. No network is touched.
"""
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
_provider = Path(__file__).resolve().parents[2] / "provider" / "python"
if _provider.exists():
    sys.path.insert(0, str(_provider))

os.environ.setdefault("THEOREMATA_MODEL_MOCK", "1")

from theoremata_tools.benchmarks import (  # noqa: E402
    grade,
    list_benchmarks,
    load_benchmark,
)

_STD_KEYS = {"id", "kind", "informal", "formal", "expected", "provenance", "grading"}


# --------------------------------------------------------------------------- #
# Fixtures
# --------------------------------------------------------------------------- #

_ANSWERBENCH_ROWS = [
    {
        "id": "AB-Alg-001",
        "problem": "Compute the perturbed sum described above.",
        "answer": "1012",
        "answer_type": "integer",
        "category": "Algebra",
        "difficulty": "IMO-Medium",
        "perturbation": "paraphrase",
        "original_problem": "Original IMO 2019 P1.",
        "source": "IMO 2019 P1",
    },
    {
        "id": "AB-NT-002",
        "problem": "Find the exact constant.",
        "answer": "1/2",
        "answer_type": "numeric",
        "category": "Number Theory",
        "difficulty": "IMO-Easy",
        "perturbation": "resubstitute",
    },
    {
        "id": "AB-Comb-003",
        "problem": "List all valid residues.",
        "answer": "{1, 2, 3}",
        "answer_type": "set",
        "category": "Combinatorics",
        "perturbation": "distractor",
    },
]

_GRADINGBENCH_ROWS = [
    {
        "id": "GB-001",
        "problem": "Prove the inequality.",
        "solution": "A fully rigorous and complete proof ...",
        "human_grade": 7,
        "category": "Algebra",
    },
    {
        "id": "GB-002",
        "problem": "Prove the combinatorial identity.",
        "solution": "Some relevant partial results, but mostly wrong ...",
        "human_grade": 1,
        "category": "Combinatorics",
    },
]


def _write_answerbench(root: Path, rows: list[dict]) -> None:
    d = root / "IMO-Bench-main" / "IMO-Bench-main"
    d.mkdir(parents=True, exist_ok=True)
    (d / "answerbench.jsonl").write_text(
        "\n".join(json.dumps(r) for r in rows), encoding="utf-8"
    )


def _write_gradingbench(root: Path, rows: list[dict]) -> None:
    d = root / "IMO-Bench-main" / "IMO-Bench-main"
    d.mkdir(parents=True, exist_ok=True)
    (d / "gradingbench.jsonl").write_text(
        "\n".join(json.dumps(r) for r in rows), encoding="utf-8"
    )


# --------------------------------------------------------------------------- #
# Registry lists both new tracks
# --------------------------------------------------------------------------- #

def test_registry_lists_imo_bench_tracks():
    names = {b["name"] for b in list_benchmarks()}
    assert {"imo_answerbench", "imo_gradingbench"} <= names
    by_name = {b["name"]: b for b in list_benchmarks()}
    assert by_name["imo_answerbench"]["track"] == "nl_answer"
    assert by_name["imo_gradingbench"]["kind"] == "proof_grading"


# --------------------------------------------------------------------------- #
# AnswerBench loader + robust answer-match grader
# --------------------------------------------------------------------------- #

def test_answerbench_loader_parses_fixture(monkeypatch, tmp_path):
    _write_answerbench(tmp_path, _ANSWERBENCH_ROWS)
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    items = load_benchmark("imo_answerbench")
    assert len(items) == 3
    for it in items:
        assert set(it) >= _STD_KEYS
        assert it["kind"] == "nl_answer"
        assert it["grading"]["method"] == "answer_match"
    by_id = {it["id"]: it for it in items}
    alg = by_id["imo_answerbench:AB-Alg-001"]
    assert alg["expected"]["answer"] == "1012"
    assert alg["expected"]["answer_kind"] == "integer"
    assert alg["expected"]["perturbation"] == "paraphrase"
    assert alg["provenance"]["category"] == "Algebra"
    assert by_id["imo_answerbench:AB-Comb-003"]["expected"]["answer_kind"] == "set"


def test_answerbench_grader_accepts_formatting_variants(monkeypatch, tmp_path):
    _write_answerbench(tmp_path, _ANSWERBENCH_ROWS)
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    by_id = {it["id"]: it for it in load_benchmark("imo_answerbench")}

    integer_item = by_id["imo_answerbench:AB-Alg-001"]  # gold 1012
    # boxed / $ / trailing prose variants of the SAME value are accepted
    assert grade(integer_item, r"Therefore the final answer is $\boxed{1012}$.")[
        "is_correct"
    ] is True
    assert grade(integer_item, "The answer is 1012.")["is_correct"] is True
    # a different number is rejected
    assert grade(integer_item, r"\boxed{1013}")["is_correct"] is False

    numeric_item = by_id["imo_answerbench:AB-NT-002"]  # gold 1/2
    # 0.5 IS exactly 1/2 → accepted; an approximation is rejected
    assert grade(numeric_item, r"So the answer is \boxed{0.5}")["is_correct"] is True
    assert grade(numeric_item, r"\boxed{0.49}")["is_correct"] is False

    set_item = by_id["imo_answerbench:AB-Comb-003"]  # gold {1,2,3}
    # order- and brace-insensitive set equivalence
    assert grade(set_item, r"The answer is $\boxed{3, 2, 1}$")["is_correct"] is True
    assert grade(set_item, r"\boxed{1, 2}")["is_correct"] is False


# --------------------------------------------------------------------------- #
# GradingBench loader + autograder-vs-human agreement grader
# --------------------------------------------------------------------------- #

def test_gradingbench_loader_parses_fixture(monkeypatch, tmp_path):
    _write_gradingbench(tmp_path, _GRADINGBENCH_ROWS)
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    items = load_benchmark("imo_gradingbench")
    assert len(items) == 2
    for it in items:
        assert set(it) >= _STD_KEYS
        assert it["kind"] == "proof_grading"
        assert it["grading"]["method"] == "grading_correlation"
    by_id = {it["id"]: it for it in items}
    good = by_id["imo_gradingbench:GB-001"]
    assert good["expected"]["gold_human_rating"] == 7
    assert good["expected"]["gold_bucket"] == "correct"
    assert good["expected"]["proposed_solution"].startswith("A fully rigorous")
    assert by_id["imo_gradingbench:GB-002"]["expected"]["gold_bucket"] == "partial"


def test_gradingbench_grader_agreement(monkeypatch, tmp_path):
    _write_gradingbench(tmp_path, _GRADINGBENCH_ROWS)
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    by_id = {it["id"]: it for it in load_benchmark("imo_gradingbench")}

    correct_item = by_id["imo_gradingbench:GB-001"]  # human 7 → Correct bucket
    # an autograder that also says 7 (boxed / "N out of 7" / dict / label) agrees
    assert grade(correct_item, r"<points>7 out of 7</points>")["is_correct"] is True
    assert grade(correct_item, r"Final grade: \boxed{7}")["is_correct"] is True
    assert grade(correct_item, {"score": 7})["is_correct"] is True
    assert grade(correct_item, "correct")["is_correct"] is True
    # a grader that says 0 disagrees with the human 7
    res = grade(correct_item, r"\boxed{0}")
    assert res["is_correct"] is False
    assert res["detail"]["abs_error"] == 7.0

    partial_item = by_id["imo_gradingbench:GB-002"]  # human 1 → Partial bucket
    assert grade(partial_item, "partial")["is_correct"] is True
    assert grade(partial_item, r"\boxed{1}")["is_correct"] is True
    assert grade(partial_item, r"\boxed{7}")["is_correct"] is False


# --------------------------------------------------------------------------- #
# Graceful degradation — absent corpus returns [] (no raise)
# --------------------------------------------------------------------------- #

def test_imo_bench_tracks_skip_when_absent(monkeypatch, tmp_path):
    # empty resource root → both loaders return [] cleanly
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    assert load_benchmark("imo_answerbench") == []
    assert load_benchmark("imo_gradingbench") == []
