import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import math  # noqa: E402

from theoremata_tools.proof_grader import (  # noqa: E402
    CORRECT,
    COMPUTATION_ERROR,
    LOGICAL_GAP,
    UNJUSTIFIED_STEP,
    aggregate_scores,
    arithmetic_mean,
    classify_step,
    generate_marking_scheme,
    geometric_mean,
    grade_proof,
    grade_proof_item,
    grade_with_marking_scheme,
    has_lgtm,
    is_refusal,
    run,
    score_band,
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


# --------------------------------------------------------------------------- #
# Rubric upgrades: geometric mean, banned-reasonings, <LGTM>, ordinal scale
# --------------------------------------------------------------------------- #

def test_geometric_mean_punishes_a_weak_dimension_vs_arithmetic():
    # One near-failing dimension among strong ones.
    dims = [1.0, 1.0, 1.0, 0.1]
    a = aggregate_scores(dims, "arithmetic")
    g = aggregate_scores(dims, "geometric")
    assert math.isclose(a, 0.775)          # (1+1+1+0.1)/4
    assert g < a                            # geometric drags the aggregate down
    assert math.isclose(g, (0.1) ** 0.25)  # product^(1/4) = 0.1^0.25
    # Balanced input: the two aggregations agree.
    assert math.isclose(arithmetic_mean([0.5, 0.5]), geometric_mean([0.5, 0.5]))


def test_geometric_mean_edge_cases():
    assert geometric_mean([]) == 0.0
    assert geometric_mean([2.0, None, 8.0]) == 4.0  # sqrt(16), None skipped
    assert geometric_mean([-1.0, 2.0]) == 0.0       # negative invalid -> 0.0


def test_grade_proof_geometric_aggregation_lower_than_arithmetic():
    # A proof with one computation error (quality 0.1) among correct steps.
    proof = "We have a = b by hypothesis.\nThen 2 + 2 = 5 as computed.\nHence done by algebra."
    arith = grade_proof(proof, aggregate="arithmetic")
    geo = grade_proof(proof, aggregate="geometric")
    assert geo["score"] < arith["score"]
    assert geo["aggregate"] == "geometric"


def test_banned_reasoning_is_rejected_and_falls_back():
    def refusing_judge(problem, steps):
        return {"per_step": [], "verdict": "I cannot evaluate this proof."}

    res = grade_proof(UNJUSTIFIED_PROOF, use_llm=True, judge=refusing_judge)
    assert res["judge_refused"] is True
    assert res["path"] == "deterministic"   # refusal dropped, determinism used
    assert res["verdict"] == "flawed"


def test_is_refusal_helper():
    assert is_refusal("Sorry, I cannot provide the evaluation.")
    assert is_refusal("As an AI, I am unable to evaluate.")
    assert not is_refusal("Step 2 is unjustified.")


def test_lgtm_sentinel_short_circuits_to_accept():
    def lgtm_judge(problem, steps):
        return {"per_step": [], "verdict": "<LGTM>"}

    res = grade_proof(CLEAN_PROOF, use_llm=True, judge=lgtm_judge)
    assert res["lgtm"] is True
    assert res["verdict"] == CORRECT
    assert res["score"] == 1.0
    assert all(s["status"] == CORRECT for s in res["per_step"])
    assert has_lgtm("all good <LGTM>")


def test_lgtm_ignored_when_judge_also_flags_a_flaw():
    def mixed_judge(problem, steps):
        per = [{"status": CORRECT, "reason": "ok"} for _ in steps]
        per[0] = {"status": LOGICAL_GAP, "reason": "gap"}
        return {"per_step": per, "verdict": "<LGTM> but see step 1"}

    res = grade_proof(CLEAN_PROOF, use_llm=True, judge=mixed_judge)
    assert res["lgtm"] is False
    assert res["verdict"] == "flawed"


def test_ordinal_scale_0_to_7():
    clean = grade_proof(CLEAN_PROOF, scale=7)
    assert clean["scale"] == 7
    assert clean["ordinal_score"] == 7  # all steps correct -> top band
    flawed = grade_proof(UNJUSTIFIED_PROOF, scale=7)
    assert 0 <= flawed["ordinal_score"] < 7
    empty = grade_proof("", scale=7)
    assert empty["ordinal_score"] is None


def test_ordinal_scale_partial_credit_band():
    # 3 of 4 steps correct (one computation error) -> mid ordinal, not 0 or max.
    proof = "a = b holds.\n2 + 2 = 5 is wrong.\nc = d holds.\ne = f holds."
    res = grade_proof(proof, scale=7, aggregate="arithmetic")
    assert res["taxonomy_counts"][COMPUTATION_ERROR] == 1
    assert 0 < res["ordinal_score"] < 7


# --------------------------------------------------------------------------- #
# Marking-scheme-conditioned grading (PROOFGRADER / IMO-Bench)
# --------------------------------------------------------------------------- #

# A reference solution with distinct content words per step (main idea = the
# longest "Factor ..." step).
REFERENCE = """Assume n is even, so write n = 2k for some integer k.
Substitute to get n squared equals 4k squared.
Factor 4k squared as 2 times 2k squared, which exhibits a factor of two.
Therefore n squared is even, completing the argument."""

# Strong candidate: mirrors the reference, every step justified.
STRONG_CANDIDATE = """Assume n is even, so write n = 2k for an integer k.
Substitute to obtain n squared equals 4k squared.
Factor 4k squared as 2 times 2k squared, exhibiting a factor of two.
Therefore n squared is even."""

# Weak candidate: asserts the key step ("obviously"), covers almost no checkpoint.
WEAK_CANDIDATE = """Assume n is even.
Obviously n squared is even for all such n.
Therefore the claim is proved."""

PROBLEM = "Prove that if n is even then n^2 is even."


def _offline_model(problem, reference):
    """Force the deterministic template scheme (no provider/network)."""
    raise RuntimeError("offline: use template fallback")


def test_marking_scheme_from_reference_is_structured():
    scheme = generate_marking_scheme(PROBLEM, REFERENCE, model=_offline_model)
    assert scheme["source"] == "template"
    assert scheme["max_points"] == 7
    cps = scheme["checkpoints"]
    assert cps, "expected non-empty checkpoints"
    # Every checkpoint is structured: integer points + a description string.
    for cp in cps:
        assert isinstance(cp["points"], int)
        assert isinstance(cp["description"], str) and cp["description"]
    # Points never exceed the scale, and the main idea earns >= 4 (paper rule).
    assert sum(cp["points"] for cp in cps) <= 7
    assert max(cp["points"] for cp in cps) >= 4
    # The other two sections are present and non-empty.
    assert scheme["zero_credit"]
    assert scheme["deductions"]


def test_marking_scheme_injected_model_is_used():
    custom = {
        "max_points": 7,
        "checkpoints": [
            {"points": 5, "description": "core pigeonhole argument"},
            {"points": 2, "description": "boundary case handled"},
        ],
        "zero_credit": ["restating the claim"],
        "deductions": [{"penalty": 1, "condition": "minor gap"}],
    }
    scheme = generate_marking_scheme(PROBLEM, REFERENCE, model=lambda p, r: custom)
    assert [cp["points"] for cp in scheme["checkpoints"]] == [5, 2]
    assert scheme["source"] == "model"


def test_scheme_grading_orders_strong_above_weak_on_0_7():
    strong = grade_with_marking_scheme(
        PROBLEM, STRONG_CANDIDATE, reference=REFERENCE, model=_offline_model
    )
    weak = grade_with_marking_scheme(
        PROBLEM, WEAK_CANDIDATE, reference=REFERENCE, model=_offline_model
    )
    assert 0 <= weak["grade"] <= 7 and 0 <= strong["grade"] <= 7
    assert strong["grade"] > weak["grade"]
    assert strong["scale"] == 7
    # Bands: strong reaches the top, weak stays in the lower half.
    assert strong["band"] == "fully-correct"
    assert weak["band"] in {"incorrect", "partial"}


def test_median_of_5_is_robust_to_one_outlier():
    def outlier_grader(problem, candidate, scheme, reference, sample):
        # Four samples say 3, one hallucinates a 7.
        return {"score": 7 if sample == 4 else 3, "assessment": f"s{sample}"}

    res = grade_with_marking_scheme(
        PROBLEM,
        WEAK_CANDIDATE,
        reference=REFERENCE,
        model=_offline_model,
        grader=outlier_grader,
        n_samples=5,
    )
    assert res["n_samples"] == 5
    assert sorted(res["sample_scores"]) == [3, 3, 3, 3, 7]
    assert res["grade"] == 3  # median unmoved by the single outlier


def test_no_context_over_scores_weak_relative_to_scheme():
    """The paper's ~1.7pt finding: no-context grading over-scores a weak proof
    relative to scheme-conditioned grading (assert direction, not magnitude)."""
    no_context = grade_proof(WEAK_CANDIDATE, scale=7)["ordinal_score"]
    scheme_conditioned = grade_with_marking_scheme(
        PROBLEM, WEAK_CANDIDATE, reference=REFERENCE, model=_offline_model
    )["grade"]
    assert no_context > scheme_conditioned


def test_scheme_grade_is_advisory_never_vetoes_formal():
    res = grade_with_marking_scheme(
        PROBLEM, WEAK_CANDIDATE, reference=REFERENCE, model=_offline_model
    )
    assert res["advisory"] is True
    assert "formal" in res["note"].lower()
    assert "never veto" in res["note"].lower()


def test_score_band_mapping():
    assert score_band(0) == "incorrect"
    assert score_band(2) == "partial"
    assert score_band(5) == "nearly-complete"
    assert score_band(7) == "fully-correct"


def test_run_marking_scheme_ops(monkeypatch):
    monkeypatch.setenv("THEOREMATA_MODEL_MOCK", "1")
    gen = run({"op": "generate_marking_scheme", "problem": PROBLEM, "reference": REFERENCE})
    assert gen["op"] == "generate_marking_scheme"
    assert gen["marking_scheme"]["checkpoints"]
    # Grade with an explicit scheme so the op is fully offline/deterministic.
    scheme = generate_marking_scheme(PROBLEM, REFERENCE, model=_offline_model)
    graded = run(
        {
            "op": "grade_with_marking_scheme",
            "problem": PROBLEM,
            "candidate": STRONG_CANDIDATE,
            "reference": REFERENCE,
            "scheme": scheme,
            "n_samples": 5,
        }
    )
    assert graded["op"] == "grade_with_marking_scheme"
    assert 0 <= graded["grade"] <= 7
    assert graded["n_samples"] == 5
    assert graded["advisory"] is True
