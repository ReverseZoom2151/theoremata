import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.reward import (  # noqa: E402
    ALPHA_SELF_EVAL,
    BETA_SELF_EVAL,
    faithfulness_reward,
    format_reward,
    generator_self_verify_reward,
    snap_score,
    verifier_reward,
)


# --- format reward ---------------------------------------------------------

def test_format_reward_marker_present():
    assert format_reward("... Based on my evaluation, the final overall score should be: \\boxed{1}") == 1.0
    assert format_reward("no markers here") == 0.0
    assert format_reward("") == 0.0
    assert format_reward("custom", markers=["custom"]) == 1.0


# --- snap / faithfulness ---------------------------------------------------

def test_snap_score():
    assert snap_score(0.9) == 1.0
    assert snap_score(0.4) == 0.5
    assert snap_score(0.2) == 0.0
    assert snap_score(None) is None
    assert snap_score(True) is None  # bools rejected


def test_faithfulness_reward_exact_and_off():
    # R_score = 1 - |s' - s|
    assert faithfulness_reward(1.0, 1.0) == 1.0
    assert faithfulness_reward(0.0, 1.0) == 0.0
    assert faithfulness_reward(0.5, 1.0) == 0.5
    assert faithfulness_reward(0.5, 0.0) == 0.5
    assert faithfulness_reward(None, 1.0) is None


# --- verifier reward with meta multiplier ---------------------------------

def test_verifier_reward_meta_multiplier():
    # perfect faithful score, format ok, meta confirms fully -> 1.0
    assert verifier_reward(1.0, 1.0, r_format=1.0, r_meta=1.0) == 1.0
    # meta halves an otherwise-perfect score (analysis half-confirmed)
    assert verifier_reward(1.0, 1.0, r_meta=0.5) == 0.5
    # bad format zeroes everything
    assert verifier_reward(1.0, 1.0, r_format=0.0) == 0.0
    # partial faithfulness * meta
    assert verifier_reward(0.5, 1.0, r_meta=0.5) == 0.25
    assert verifier_reward(None, 1.0) is None


# --- generator self-verify reward -----------------------------------------

def test_generator_self_verify_reward_blend():
    # R = R_format * (alpha*R_Y + beta*R_Z), R_Z = R_score * R_meta
    # perfect proof (R_Y=1), perfect honest self-eval (R_score=1), meta=1
    r = generator_self_verify_reward(1.0, 1.0, 1.0)
    assert abs(r - (ALPHA_SELF_EVAL * 1.0 + BETA_SELF_EVAL * 1.0)) < 1e-9
    assert abs(r - 1.0) < 1e-9


def test_generator_self_verify_penalizes_dishonest_selfeval():
    # proof is actually wrong (gold_score 0) but model self-scored 1.0 (lie):
    # R_Z = 1 - |1 - 0| = 0, and R_Y (proof correctness) is 0 too -> reward 0.
    r = generator_self_verify_reward(0.0, 1.0, 0.0)
    assert r == 0.0


def test_generator_self_verify_rewards_honest_low_selfeval():
    # wrong proof (R_Y=0) but honestly self-scored 0 (matches gold 0):
    # R_Z = 1, so reward = beta * 1 = 0.24 -> honesty is rewarded.
    r = generator_self_verify_reward(0.0, 0.0, 0.0)
    assert abs(r - BETA_SELF_EVAL) < 1e-9


def test_generator_self_verify_meta_gate():
    # meta discounts the self-eval term
    r = generator_self_verify_reward(1.0, 1.0, 1.0, r_meta=0.0)
    assert abs(r - ALPHA_SELF_EVAL) < 1e-9  # only the R_Y term survives


def test_generator_self_verify_none_when_unscorable():
    assert generator_self_verify_reward(1.0, None, 1.0) is None
