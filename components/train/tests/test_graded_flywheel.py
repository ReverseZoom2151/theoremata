import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.flywheel import (  # noqa: E402
    canonical_proof,
    graded_revolution,
    majority_meta_confirm,
    run,
)
from theoremata_tools.reward import (  # noqa: E402
    graded_generator_reward,
    graded_verifier_reward,
)


# --- graded verifier/generator reward primitive: R = R_format.R_score.R_meta

def test_graded_verifier_reward_zero_on_format_fail():
    assert graded_verifier_reward(0, 1.0, 1.0) == 0.0
    assert graded_verifier_reward(False, 1.0, 1.0) == 0.0


def test_graded_verifier_reward_multiplicative():
    assert graded_verifier_reward(1, 1.0, 1.0) == 1.0
    assert graded_verifier_reward(1, 0.5, 1.0) == 0.5
    assert graded_verifier_reward(1, 1.0, 0.5) == 0.5
    assert graded_verifier_reward(1, 0.5, 0.5) == 0.25
    # out-of-range terms are clipped into [0, 1]
    assert graded_verifier_reward(1, 2.0, 1.0) == 1.0
    assert graded_verifier_reward(1, -1.0, 1.0) == 0.0


def test_graded_generator_reward_matches_product():
    assert graded_generator_reward(1, 0.5, 0.5) == 0.25
    assert graded_generator_reward(0, 1.0, 1.0) == 0.0


# --- majority_meta_confirm flips on >= ceil(N/2) agreement ------------------

def test_majority_meta_confirm_flips_on_ceil_half():
    # N=4 -> need 2 issue-reporting passes to flip away from 1.0
    assert majority_meta_confirm([1.0, 1.0, 1.0, 1.0]) == 1.0
    assert majority_meta_confirm([0.0, 1.0, 1.0, 1.0]) == 1.0   # only 1 < ceil(4/2)=2
    assert majority_meta_confirm([0.0, 0.0, 1.0, 1.0]) == 0.0   # 2 >= 2 -> flip
    # lowest confirmed issue score wins
    assert majority_meta_confirm([0.5, 0.0, 1.0]) == 0.0        # 2 issues of 3, need 2
    # N=3 -> ceil(3/2)=2
    assert majority_meta_confirm([0.5, 1.0, 1.0]) == 1.0        # only 1 issue
    assert majority_meta_confirm([]) == 1.0                     # empty is correct


def test_majority_meta_confirm_accepts_dicts():
    passes = [
        {"score": 0.0, "reports_issue": True},
        {"score": 0.0, "reports_issue": True},
        {"score": 1.0},
    ]
    assert majority_meta_confirm(passes) == 0.0


# --- graded_revolution auto-labels a hard proof offline --------------------

def test_graded_revolution_auto_labels_hard_proof():
    # generator emits the canonical (formal-pass) proof + a distractor the
    # ground-truth pattern oracle rejects; a SOFT graded verifier accepts the
    # distractor, so it gets soft auto-labeled (verification-compute scaling).
    def gen(statement):
        return [canonical_proof(statement), "clever-but-informal proof"]

    soft = lambda p, pr: 1.0  # noqa: E731 -- soft verifier says everything is correct

    out = graded_revolution(
        [{"statement": "x = x"}],
        generator=gen,
        graded_verifier=soft,
        n_graded=4,
    )
    # hard positives unchanged: exactly the canonical proof
    assert out["n_verified"] == 1
    assert out["sft_rows"][0]["meta"]["label"] == 1.0

    # the distractor was soft auto-labeled, NOT promoted into the hard SFT set
    assert out["n_auto_labeled"] == 1
    al = out["auto_labeled"][0]
    assert al["proof"] == "clever-but-informal proof"
    assert al["soft_label"] == 1.0
    assert al["confirmed"] is True
    assert al["reward"] == 1.0  # R_format(1) * R_score(1) * R_meta(1)
    assert al["provenance"]["oracle"] == "graded_soft"


def test_graded_revolution_soft_verifier_flags_hard_proof():
    # a soft verifier that flags the distractor as fatally flawed -> soft_label 0.0
    def gen(statement):
        return [canonical_proof(statement), "bogus"]

    soft = lambda p, pr: {"score": 0.0, "reports_issue": True}  # noqa: E731

    out = graded_revolution([{"statement": "y = y"}], generator=gen, graded_verifier=soft, n_graded=3)
    assert out["n_auto_labeled"] == 1
    assert out["auto_labeled"][0]["soft_label"] == 0.0
    assert out["auto_labeled"][0]["confirmed"] is True


def test_graded_revolution_run_op_offline():
    out = run({"op": "graded_revolution", "problems": [{"statement": "a = a"}], "n_candidates": 3})
    assert out["ok"] is True
    assert out["n_verified"] == 1
    assert "n_auto_labeled" in out
    assert "sft_rows" not in out  # summary omits inline rows unless with_rows
