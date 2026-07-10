import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.format_filters import (  # noqa: E402
    drop_negative_gradient,
    format_filter,
    has_tactic_block,
    passes_coverage,
    run,
    snippet_coverage,
)


# --- Filter 1: tactic-block presence ---------------------------------------

def test_no_tactic_block_is_dropped():
    # A pure term proof with no `by` / tactic keyword is rejected.
    assert has_tactic_block({"proof": "lt_trans h1 h2"}) is False
    assert has_tactic_block({"final_proof": ""}) is False
    # Accepts a raw string too.
    assert has_tactic_block("fun x => x") is False


def test_tactic_block_present_is_kept():
    assert has_tactic_block({"proof": "by simp"}) is True
    assert has_tactic_block({"proof": "theorem t : p := by\n  linarith"}) is True
    # Lean 3 begin/end block.
    assert has_tactic_block("begin\n  intros,\n  exact h,\nend") is True
    # A line opening with a tactic keyword (no explicit `by`).
    assert has_tactic_block("intro x\nexact h x") is True


# --- Filter 2: snippet coverage --------------------------------------------

def test_snippet_coverage_fraction():
    proof = "by\n  simp\n  linarith"
    # both tactic lines appear in the CoT
    assert snippet_coverage("first simp then linarith closes it", proof) == 1.0
    # only one of two appears -> 0.5
    assert snippet_coverage("just simp", proof) == 0.5
    # none appear -> 0.0
    assert snippet_coverage("some unrelated musing", proof) == 0.0


def test_low_coverage_dropped_high_kept():
    proof = "by\n  norm_num\n  nlinarith [sq_nonneg x]"
    high = {
        "proof": proof,
        "reasoning": "apply norm_num, then nlinarith [sq_nonneg x] finishes",
    }
    low = {"proof": proof, "reasoning": "I think this is obviously true"}
    assert passes_coverage(high, threshold=0.6) is True
    assert passes_coverage(low, threshold=0.6) is False


# --- Filter 3: negative-gradient drop --------------------------------------

def test_all_fail_group_dropped():
    group = [{"reward": 0.0}, {"reward": 0.0}, {"reward": 0.0}]
    assert drop_negative_gradient(group) == []


def test_zero_variance_group_dropped():
    group = [{"reward": 1.0}, {"reward": 1.0}]
    assert drop_negative_gradient(group) == []


def test_below_quantile_dropped():
    group = [
        {"reward": 0.0, "id": "a"},
        {"reward": 0.0, "id": "b"},
        {"reward": 1.0, "id": "c"},
        {"reward": 1.0, "id": "d"},
    ]
    kept = drop_negative_gradient(group, omega=0.5)
    assert [s["id"] for s in kept] == ["c", "d"]


def test_reward_derived_from_verdict():
    group = [
        {"verdict": {"compiled": False, "axioms_ok": True}},
        {"verdict": {"compiled": True, "axioms_ok": True}, "id": "win"},
    ]
    kept = drop_negative_gradient(group)
    assert len(kept) == 1 and kept[0].get("id") == "win"


# --- Composite: keep only clean high-signal samples ------------------------

def _sample(pid, proof, reasoning, reward, sid):
    return {
        "problem": pid,
        "proof": proof,
        "reasoning": reasoning,
        "reward": reward,
        "id": sid,
    }


def test_composite_keeps_only_clean_high_signal():
    good_proof = "by\n  simp\n  linarith"
    good_cot = "we simp then linarith"
    samples = [
        # clean, high-coverage, top-of-group reward -> KEPT
        _sample("P", good_proof, good_cot, 1.0, "keep"),
        # clean, high-coverage, but bottom-of-group reward -> negative_gradient
        _sample("P", good_proof, good_cot, 0.0, "neg"),
        # no tactic block -> dropped early
        _sample("P", "lt_trans h1 h2", good_cot, 1.0, "noblk"),
        # tactic block but reasoning never mentions the tactics -> low_coverage
        _sample("P", good_proof, "nothing relevant here", 1.0, "lowcov"),
    ]
    res = format_filter(samples, coverage_threshold=0.6, omega=0.5)
    kept_ids = [s["id"] for s in res["kept"]]
    assert kept_ids == ["keep"]
    reasons = {d["index"]: d["reason"] for d in res["dropped"]}
    assert reasons[1] == "negative_gradient"
    assert reasons[2] == "no_tactic_block"
    assert reasons[3] == "low_coverage"
    assert res["n_in"] == 4 and res["n_kept"] == 1 and res["n_dropped"] == 3


def test_grouping_is_per_problem():
    # Two problems; each keeps its own top rollout independently.
    proof = "by simp"
    cot = "simp"
    samples = [
        _sample("A", proof, cot, 1.0, "a_hi"),
        _sample("A", proof, cot, 0.0, "a_lo"),
        _sample("B", proof, cot, 1.0, "b_hi"),
        _sample("B", proof, cot, 0.0, "b_lo"),
    ]
    res = format_filter(samples)
    assert sorted(s["id"] for s in res["kept"]) == ["a_hi", "b_hi"]


# --- Determinism -----------------------------------------------------------

def test_determinism():
    good_proof = "by\n  ring\n  norm_num"
    samples = [
        _sample("P", good_proof, "ring then norm_num", float(i % 2), f"s{i}")
        for i in range(8)
    ]
    r1 = format_filter(samples)
    r2 = format_filter(samples)
    assert r1 == r2
    assert [s["id"] for s in r1["kept"]] == [s["id"] for s in r2["kept"]]


# --- run() dispatch --------------------------------------------------------

def test_run_dispatch():
    proof = "by simp"
    samples = [
        {"problem": "P", "proof": proof, "reasoning": "simp", "reward": 1.0, "id": "k"},
        {"problem": "P", "proof": proof, "reasoning": "simp", "reward": 0.0, "id": "d"},
        {"problem": "P", "proof": "term_only h", "reasoning": "simp", "reward": 1.0},
    ]
    res = run({"op": "format_filter", "samples": samples})
    assert res["ok"] is True
    assert [s["id"] for s in res["kept"]] == ["k"]
    assert res["n_dropped"] == 2


def test_run_unknown_op_raises():
    import pytest

    with pytest.raises(ValueError):
        run({"op": "nope"})
