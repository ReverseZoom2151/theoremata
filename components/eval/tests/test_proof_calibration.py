import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from theoremata_tools.proof_calibration import (  # noqa: E402
    bootstrap_ci,
    calibrate,
    evaluator_disagreement,
    kendall_tau_b,
    mae,
    order_preservation,
    pearson,
    rmse,
    run,
    score_marking_scheme_grader,
    spearman,
    verify_solve_gap,
    within_tolerance,
)


# --------------------------------------------------------------------------- #
# Point-error metrics against hand-computed values
# --------------------------------------------------------------------------- #

def test_mae_rmse_bias_hand_computed():
    pred = [2.0, 4.0, 6.0]
    gold = [1.0, 5.0, 6.0]
    # diffs: +1, -1, 0 -> |.|: 1,1,0 -> mae = 2/3
    assert mae(pred, gold) == 2.0 / 3.0
    # squared: 1,1,0 -> rmse = sqrt(2/3)
    assert math.isclose(rmse(pred, gold), math.sqrt(2.0 / 3.0))


def test_within_tolerance():
    pred = [0.0, 1.0, 5.0]
    gold = [0.0, 3.0, 5.0]
    # |diff|: 0, 2, 0 ; tol=1 -> 2/3 within
    assert within_tolerance(pred, gold, tolerance=1.0) == 2.0 / 3.0


# --------------------------------------------------------------------------- #
# Correlation: perfect / inverse / hand values
# --------------------------------------------------------------------------- #

def test_perfect_positive_correlation():
    pred = [1.0, 2.0, 3.0, 4.0]
    gold = [1.0, 2.0, 3.0, 4.0]
    assert math.isclose(pearson(pred, gold), 1.0)
    assert math.isclose(spearman(pred, gold), 1.0)
    assert math.isclose(kendall_tau_b(pred, gold), 1.0)


def test_perfect_inverse_correlation():
    pred = [4.0, 3.0, 2.0, 1.0]
    gold = [1.0, 2.0, 3.0, 4.0]
    assert math.isclose(pearson(pred, gold), -1.0)
    assert math.isclose(spearman(pred, gold), -1.0)
    assert math.isclose(kendall_tau_b(pred, gold), -1.0)


def test_pearson_hand_value():
    # Classic monotone-but-nonlinear: perfect Spearman, Pearson < 1.
    pred = [1.0, 2.0, 3.0, 4.0]
    gold = [1.0, 4.0, 9.0, 16.0]
    assert math.isclose(spearman(pred, gold), 1.0)
    assert pearson(pred, gold) < 1.0


def test_constant_side_is_nan():
    assert math.isnan(pearson([1.0, 1.0, 1.0], [1.0, 2.0, 3.0]))


# --------------------------------------------------------------------------- #
# Order preservation / pairwise ranking
# --------------------------------------------------------------------------- #

def test_order_preservation_perfect():
    stats = order_preservation([10.0, 20.0, 30.0], [1.0, 2.0, 3.0])
    assert stats["order_acc"] == 1.0
    assert stats["comparable_pairs"] == 3.0
    assert stats["correct_pairs"] == 3.0


def test_order_preservation_one_swap():
    # gold order 1<2<3 ; pred swaps the top two -> pair (2,3) wrong, others right
    stats = order_preservation([1.0, 3.0, 2.0], [1.0, 2.0, 3.0])
    assert stats["comparable_pairs"] == 3.0
    assert stats["correct_pairs"] == 2.0
    assert math.isclose(stats["order_acc"], 2.0 / 3.0)


def test_order_preservation_skips_true_ties():
    # gold ties on first two -> that pair is not comparable
    stats = order_preservation([5.0, 9.0, 1.0], [2.0, 2.0, 3.0])
    assert stats["comparable_pairs"] == 2.0


# --------------------------------------------------------------------------- #
# Bootstrap CIs
# --------------------------------------------------------------------------- #

def test_bootstrap_ci_brackets_point_and_is_deterministic():
    pairs = [(float(i), float(i) + 0.5) for i in range(20)]
    a = bootstrap_ci(pairs, metrics=("mae", "pearson"), num_bootstrap=200, seed=7)
    b = bootstrap_ci(pairs, metrics=("mae", "pearson"), num_bootstrap=200, seed=7)
    assert a == b  # deterministic given seed
    for m in ("mae", "pearson"):
        assert a[m]["lo"] <= a[m]["point"] <= a[m]["hi"] + 1e-9


def test_calibrate_includes_bootstrap_only_when_requested():
    pairs = [(1.0, 1.0), (2.0, 2.0), (3.0, 4.0)]
    assert "bootstrap_ci" not in calibrate(pairs)
    assert "bootstrap_ci" in calibrate(pairs, bootstrap=True, num_bootstrap=50)


# --------------------------------------------------------------------------- #
# Evaluator disagreement
# --------------------------------------------------------------------------- #

def test_evaluator_disagreement():
    # two evaluators; item 0 they agree, item 1 they differ by 4
    scores = {"judgeA": [3.0, 1.0], "judgeB": [3.0, 5.0]}
    d = evaluator_disagreement(scores)
    assert d["num_evaluators"] == 2.0
    assert d["num_items"] == 2.0
    assert d["per_item"][0]["std"] == 0.0
    assert d["per_item"][1]["range"] == 4.0
    assert d["max_std"] == 2.0  # std of {1,5} = 2


# --------------------------------------------------------------------------- #
# Verify-vs-solve gap
# --------------------------------------------------------------------------- #

def test_verify_solve_gap_positive_when_grades_better_than_solves():
    verify = [1.0, 1.0, 1.0, 0.0]  # graded 3/4 correctly
    solve = [1.0, 0.0, 0.0, 0.0]   # solved 1/4
    g = verify_solve_gap(verify, solve)
    assert math.isclose(g["verify_mean"], 0.75)
    assert math.isclose(g["solve_mean"], 0.25)
    assert math.isclose(g["gap"], 0.5)


# --------------------------------------------------------------------------- #
# Full calibrate + worker dispatch
# --------------------------------------------------------------------------- #

def test_calibrate_full_dict_perfect():
    pairs = [(1.0, 1.0), (2.0, 2.0), (3.0, 3.0), (4.0, 4.0)]
    r = calibrate(pairs)
    assert r["count"] == 4.0
    assert r["mae"] == 0.0
    assert r["rmse"] == 0.0
    assert math.isclose(r["pearson"], 1.0)
    assert math.isclose(r["spearman"], 1.0)
    assert math.isclose(r["kendall"], 1.0)
    assert r["order_preservation"] == 1.0


def test_calibrate_accepts_dicts():
    pairs = [{"pred": 2.0, "gold": 1.0}, {"predicted_score": 3.0, "human": 4.0}]
    r = calibrate(pairs)
    assert r["count"] == 2.0


def test_run_dispatch_calibrate_and_disagreement():
    out = run({"op": "calibrate", "pairs": [[1.0, 1.0], [2.0, 2.0], [3.0, 4.0]]})
    assert out["count"] == 3.0
    out2 = run({"op": "verify_solve_gap", "verify_scores": [1.0, 1.0], "solve_scores": [0.0, 1.0]})
    assert math.isclose(out2["gap"], 0.5)


# --------------------------------------------------------------------------- #
# PROOFGRADER calibration: full metric set + marking-scheme grader scoring
# --------------------------------------------------------------------------- #

def test_calibrate_reports_all_paper_metrics_hand_computed():
    # PROOFGRADER's headline metric set: MAE, RMSE, Bias, WTA<=1, Kendall-tau_b,
    # Pearson, Spearman.
    pred = [2.0, 4.0, 6.0]
    gold = [1.0, 5.0, 6.0]
    r = calibrate(pred and [(p, g) for p, g in zip(pred, gold)])
    for key in (
        "mae", "rmse", "bias", "within_tolerance", "within_1_point",
        "kendall", "pearson", "spearman",
    ):
        assert key in r
    # diffs +1, -1, 0 -> MAE 2/3, RMSE sqrt(2/3), Bias 0, all within 1 point.
    assert math.isclose(r["mae"], 2.0 / 3.0)
    assert math.isclose(r["rmse"], math.sqrt(2.0 / 3.0))
    assert math.isclose(r["bias"], 0.0)
    assert r["within_1_point"] == 1.0


def test_score_marking_scheme_grader_vs_gold():
    pred = [7, 1, 4]   # grader's 0-7 grades
    gold = [7, 0, 5]   # expert labels
    r = score_marking_scheme_grader(pred, gold, scale=7)
    assert r["scale"] == 7
    # diffs 0, +1, -1 -> MAE 2/3, Bias 0, all within 1 point (WTA<=1 == 1.0).
    assert math.isclose(r["mae"], 2.0 / 3.0)
    assert math.isclose(r["bias"], 0.0)
    assert r["within_1_point"] == 1.0
    assert "kendall" in r and "pearson" in r and "spearman" in r


def test_run_dispatch_score_marking_scheme_grader():
    out = run(
        {
            "op": "score_marking_scheme_grader",
            "pred_grades": [7, 1, 4],
            "gold_grades": [7, 0, 5],
            "scale": 7,
        }
    )
    assert out["scale"] == 7
    assert out["within_1_point"] == 1.0
