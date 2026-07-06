import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from theoremata_tools.eval_harness import (  # noqa: E402
    contamination_flag,
    evaluate,
    freshness_tier,
    load_problems,
    recalled_answer_smell,
    run,
)


# --- fixtures -------------------------------------------------------------

def _problem_set():
    """Three answer-style records: an integer test item, a symbolic test item,
    and a train-tagged item that must be refused (never scored)."""
    return [
        {
            "data_id": "int-1",
            "problem": "Compute 6 * 7.",
            "type": "bound",
            "answer_kind": "integer",
            "data_split": "test",
            "answer": "42",
            "solution": "Multiplying six by seven yields forty two as the product.",
            "difficulty": "easy",
        },
        {
            "id": "sym-1",
            "problem": "Simplify (x+1)^2.",
            "answer_kind": "symbolic",
            "usage_tag": "test",
            "answer": "x**2 + 2*x + 1",
            "gold_solution": "Expand the binomial square to get x squared plus two x plus one.",
            "difficulty_tier": "medium",
        },
        {
            "data_id": "train-1",
            "problem": "A memorized training item.",
            "type": "integer",
            "data_split": "train",
            "answer": "5",
            "solution": "Trivial.",
            "difficulty": "easy",
        },
    ]


def _generations():
    return {
        # pass@k True; majority (mode "42") correct; averaged = 3/4
        "int-1": ["42", "7", "42", "42"],
        # all correct symbolic variants -> pass, majority, averaged 1.0
        "sym-1": ["x**2+2*x+1", "1 + 2*x + x**2", "x^2+2*x+1"],
        # should never be graded
        "train-1": ["5", "5"],
    }


# --- loading / schema -----------------------------------------------------

def test_load_normalizes_superset_schema():
    probs = load_problems(_problem_set())
    by_id = {p["id"]: p for p in probs}
    keys = {
        "id", "problem", "grade_kind", "answer_kind", "answer", "choices",
        "gold_solution", "lean_stub", "usage_tag", "contamination_risk",
        "difficulty_tier",
    }
    assert keys <= set(by_id["int-1"])
    assert by_id["int-1"]["grade_kind"] == "answer"
    assert by_id["int-1"]["usage_tag"] == "test"       # data_split test
    assert by_id["train-1"]["usage_tag"] == "train"    # data_split train
    assert by_id["int-1"]["difficulty_tier"] == "easy"


def test_load_jsonl_file(tmp_path):
    p = tmp_path / "probs.jsonl"
    p.write_text(
        '{"id": "a", "problem": "x", "answer": "1", "usage_tag": "test"}\n'
        '{"id": "b", "problem": "y", "answer": "2", "usage_tag": "test"}\n',
        encoding="utf-8",
    )
    probs = load_problems(str(p))
    assert [x["id"] for x in probs] == ["a", "b"]


# --- evaluate: metrics ----------------------------------------------------

def test_metrics_pass_majority_averaged():
    probs = load_problems(_problem_set())
    rep = evaluate(probs, _generations())
    per = {p["id"]: p for p in rep["per_problem"]}

    m = per["int-1"]["metrics"]
    assert m["pass_at_k"] is True
    assert m["majority_at_k"] is True
    assert abs(m["averaged_at_k"] - 0.75) < 1e-9

    m2 = per["sym-1"]["metrics"]
    assert m2["pass_at_k"] is True
    assert m2["majority_at_k"] is True
    assert abs(m2["averaged_at_k"] - 1.0) < 1e-9


def test_train_item_refused_not_scored():
    probs = load_problems(_problem_set())
    rep = evaluate(probs, _generations())
    scored_ids = {p["id"] for p in rep["per_problem"]}
    assert "train-1" not in scored_ids
    refused_ids = {r["id"] for r in rep["refused"]}
    assert refused_ids == {"train-1"}
    assert rep["refused"][0]["scored"] is False
    assert rep["n_scored"] == 2


def test_k_truncation():
    probs = load_problems(_problem_set())
    # k=1 keeps only the first generation for int-1 ("42") -> averaged 1.0
    rep = evaluate(probs, _generations(), k=1)
    per = {p["id"]: p for p in rep["per_problem"]}
    assert per["int-1"]["metrics"]["k"] == 1
    assert per["int-1"]["metrics"]["averaged_at_k"] == 1.0


# --- six axes reported separately + standard error ------------------------

def test_six_axes_reported_separately():
    probs = load_problems(_problem_set())
    rep = evaluate(probs, _generations())
    axes = rep["by_axis"]
    assert set(axes) == {
        "discovery", "informal", "formal", "soundness", "efficiency", "novelty"
    }
    # answer-style signals present; lean-only axes explicitly None (not 0)
    assert axes["discovery"] is not None
    assert axes["informal"] is not None
    assert axes["formal"] is None
    assert axes["soundness"] is None
    assert axes["efficiency"] is None
    # never blended into a single scalar
    assert isinstance(axes["discovery"], dict)

    # per-problem six_axis is also kept separate
    per = rep["per_problem"][0]["six_axis"]
    assert set(per) == {
        "discovery", "informal", "formal", "soundness", "efficiency", "novelty"
    }


def test_standard_error_present_everywhere():
    probs = load_problems(_problem_set())
    rep = evaluate(probs, _generations())
    assert "stderr" in rep["overall"]["pass_at_k"]
    assert "stderr" in rep["overall"]["averaged_at_k"]
    assert rep["overall"]["averaged_at_k"]["stderr"] is not None
    # per-tier standard error too
    for tier in rep["by_difficulty_tier"].values():
        assert "stderr" in tier["averaged_at_k"]
    # per-problem sample stderr
    assert "stderr" in rep["per_problem"][0]["metrics"]


# --- contamination controls -----------------------------------------------

def test_contamination_flag_fires_on_gold_echo():
    prob = load_problems(_problem_set())[0]  # int-1, has a gold solution
    echo = "Multiplying six by seven yields forty two as the product."
    hit = contamination_flag(prob, echo)
    assert hit["flagged"] is True
    assert hit["overlap"] >= 0.5
    # an unrelated generation does not flag
    miss = contamination_flag(prob, "The result is simply forty two okay.")
    assert miss["flagged"] is False


def test_contamination_surfaced_in_report():
    probs = load_problems(_problem_set())
    gens = _generations()
    gens["int-1"] = [
        "Multiplying six by seven yields forty two as the product.",
        "42",
    ]
    rep = evaluate(probs, gens)
    assert "int-1" in rep["contamination"]["flagged_ids"]
    per = {p["id"]: p for p in rep["per_problem"]}
    assert per["int-1"]["contamination"]["flagged"] is True
    # novelty axis picks up the contamination signal
    assert rep["by_axis"]["novelty"] is not None


def test_recalled_answer_smell_hook():
    hot = recalled_answer_smell(0.95, 0.1)
    assert hot["flagged"] is True
    cold = recalled_answer_smell(0.95, 0.9)
    assert cold["flagged"] is False
    unknown = recalled_answer_smell(0.95, None)
    assert unknown["flagged"] is None


def test_freshness_tier_heuristic():
    assert freshness_tier({"id": "aime26-p1", "problem": ""}) == "low"
    assert freshness_tier({"id": "aime24-p3", "problem": ""}) == "high"
    assert freshness_tier({"id": "x", "problem": "", "contamination_risk": "medium"}) == "medium"


# --- dispatch -------------------------------------------------------------

def test_run_dispatch_evaluate():
    rep = run({"op": "evaluate", "problems": _problem_set(), "generations": _generations()})
    assert rep["op"] == "evaluate"
    assert rep["n_scored"] == 2
    assert rep["n_refused"] == 1


def test_run_dispatch_load():
    out = run({"op": "load", "records": _problem_set()})
    assert out["n"] == 3
