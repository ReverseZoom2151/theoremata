import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from theoremata_tools.proof_grader import (  # noqa: E402
    CORRECT,
    COMPUTATION_ERROR,
    UNJUSTIFIED_STEP,
    classify_step,
    grade_proof,
    grade_proof_item,
    run,
    split_steps,
)

# A clean, fully-justified proof (every equality holds; no hedging).
CLEAN_PROOF = """Assume n is even, so n = 2k for some integer k.
Then n^2 = 4k^2 = 2*(2k^2), which is divisible by 2.
Since 2 + 2 = 4, the arithmetic checks out.
Therefore n^2 is even, as required."""

# A proof that asserts a key step with no justification ("obviously").
UNJUSTIFIED_PROOF = """Let x be a real number with x > 0.
Obviously the inequality holds for all such x.
Therefore the claim is proved."""


def test_split_steps_lines():
    steps = split_steps("Step one.\nStep two.\nStep three.")
    assert steps == ["Step one.", "Step two.", "Step three."]


def test_split_steps_single_line_sentences():
    steps = split_steps("First we note A. Then B follows.")
    assert len(steps) == 2


def test_classify_flags_unjustified_hedge():
    c = classify_step("Obviously the inequality holds.")
    assert c["status"] == UNJUSTIFIED_STEP


def test_classify_flags_bad_arithmetic():
    c = classify_step("We compute 2 + 2 = 5 and continue.")
    assert c["status"] == COMPUTATION_ERROR


def test_classify_flags_sorry_placeholder():
    c = classify_step("The remaining case is left by sorry.")
    assert c["status"] == UNJUSTIFIED_STEP


def test_classify_clean_step_is_correct():
    c = classify_step("Then n^2 = 4k^2 = 2*(2k^2), divisible by 2.")
    assert c["status"] == CORRECT


def test_grader_flags_unjustified_step_mock_mode(monkeypatch):
    """Core requirement: in offline/mock mode the grader flags an unjustified
    step and does not accept the proof."""
    monkeypatch.setenv("THEOREMATA_MODEL_MOCK", "1")
    res = grade_proof(UNJUSTIFIED_PROOF, mode="step_wise")
    assert res["verdict"] == "flawed"
    assert res["taxonomy_counts"][UNJUSTIFIED_STEP] >= 1
    assert res["score"] < 1.0
    statuses = [s["status"] for s in res["per_step"]]
    assert UNJUSTIFIED_STEP in statuses


def test_grader_accepts_clean_proof(monkeypatch):
    monkeypatch.setenv("THEOREMATA_MODEL_MOCK", "1")
    res = grade_proof(CLEAN_PROOF, mode="step_wise")
    assert res["verdict"] == CORRECT
    assert res["flaw_count"] == 0
    assert res["score"] == 1.0
    assert all(s["status"] == CORRECT for s in res["per_step"])


def test_holistic_mode_binary_score():
    good = grade_proof(CLEAN_PROOF, mode="holistic")
    assert good["score"] == 1.0 and good["verdict"] == CORRECT
    bad = grade_proof(UNJUSTIFIED_PROOF, mode="holistic")
    assert bad["score"] == 0.0 and bad["verdict"] == "flawed"
    assert bad["overall_status"] == UNJUSTIFIED_STEP


def test_empty_proof():
    res = grade_proof("")
    assert res["verdict"] == "empty"
    assert res["score"] is None
    assert res["n_steps"] == 0


def test_return_shape():
    res = grade_proof(CLEAN_PROOF)
    assert set(res) >= {"score", "per_step", "verdict"}
    assert all(set(s) >= {"status", "reason"} for s in res["per_step"])


def test_llm_judge_path_with_injected_judge():
    """An injected judge drives the step-wise verdict (no network)."""

    def fake_judge(problem, steps):
        per = [{"status": CORRECT, "reason": "ok"} for _ in steps]
        per[0] = {"status": COMPUTATION_ERROR, "reason": "bad calc"}
        return {"per_step": per, "verdict": "flawed"}

    res = grade_proof(CLEAN_PROOF, use_llm=True, judge=fake_judge)
    assert res["path"] == "llm_judge"
    assert res["per_step"][0]["status"] == COMPUTATION_ERROR
    assert res["verdict"] == "flawed"


def test_llm_judge_falls_back_to_deterministic():
    """A judge returning no per_step falls back to the deterministic path."""

    def empty_judge(problem, steps):
        return {"per_step": [], "verdict": "unknown"}

    res = grade_proof(UNJUSTIFIED_PROOF, use_llm=True, judge=empty_judge)
    assert res["path"] == "deterministic"
    assert res["verdict"] == "flawed"


def test_default_llm_judge_runs_in_mock_mode(monkeypatch):
    """The default provider-backed judge must not hit the network in mock mode."""
    monkeypatch.setenv("THEOREMATA_MODEL_MOCK", "1")
    res = grade_proof(CLEAN_PROOF, use_llm=True)
    # Mock provider returns a schema-shaped object; grader stays well-formed.
    assert set(res) >= {"score", "per_step", "verdict", "path"}


def test_grade_proof_item_adapter():
    item = {"problem": "Show n^2 even.", "kind": "proof_rubric"}
    verdict = grade_proof_item(item, CLEAN_PROOF)
    assert verdict["is_solved"] is True
    assert verdict["is_correct"] is True
    assert verdict["detail"]["grader"] == "proof_grader"

    bad = grade_proof_item(item, UNJUSTIFIED_PROOF)
    assert bad["is_correct"] is False


def test_run_dispatch():
    out = run({"op": "grade_proof", "proof": UNJUSTIFIED_PROOF})
    assert out["verdict"] == "flawed"
    out2 = run({"op": "classify_step", "step": "Obviously true."})
    assert out2["status"] == UNJUSTIFIED_STEP
