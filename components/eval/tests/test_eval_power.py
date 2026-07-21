"""Tests for ``theoremata_tools.eval_power``.

The oracles here are KNOWN VALUES, not round trips through the module itself. A
power calculation that is internally consistent and numerically wrong is the
easy failure mode, so the sign-test counts are checked against independently
computable references:

* the textbook normal-approximation sample size for a sign test,
  ``N = ((z_{1 - alpha/2} * 0.5 + z_{power} * sqrt(p(1-p))) / |p - 0.5|) ** 2``
  (Noether's formula for the sign test, standard in nonparametric texts), hand
  evaluated below; and
* the exact binomial answer, recomputed in these tests from ``math.comb`` in a
  deliberately naive way that shares no code with the log-space implementation.
"""
from __future__ import annotations

import math

import pytest

from theoremata_tools.eval_power import (
    DISCLAIMER,
    PowerPlanError,
    design_effect,
    plan,
    ppi_variance_factor,
    required_decisive_pairs,
    sign_test_power,
    tie_inflation_factor,
    run,
)


# --------------------------------------------------------------------------- #
# Independent reference implementation (naive, math.comb, no shared code)
# --------------------------------------------------------------------------- #

def _ref_sf(n: int, c: int, p: float) -> float:
    """P(S >= c) by direct summation of C(n, k) p^k q^(n-k)."""
    return sum(math.comb(n, k) * p**k * (1.0 - p) ** (n - k) for k in range(c, n + 1))


def _ref_cdf(n: int, c: int, p: float) -> float:
    if c < 0:
        return 0.0
    return sum(math.comb(n, k) * p**k * (1.0 - p) ** (n - k) for k in range(0, c + 1))


def _ref_power(n: int, p1: float, alpha: float, sides: int) -> float:
    tail = alpha / 2.0 if sides == 2 else alpha
    c = 0
    for cand in range(n + 1, -1, -1):
        if _ref_sf(n, cand, 0.5) > tail:
            c = cand + 1
            break
    if c > n:
        return 0.0
    pw = _ref_sf(n, c, p1)
    if sides == 2:
        pw += _ref_cdf(n, n - c, p1)
    return pw


# --------------------------------------------------------------------------- #
# Known values: exact sign-test sample sizes
# --------------------------------------------------------------------------- #

EXACT_CASES = [
    # (win_rate, alpha, power, sides, expected_exact_N)
    (0.75, 0.05, 0.80, 2, 30),
    (0.75, 0.05, 0.80, 1, 23),
    (0.70, 0.05, 0.90, 2, 65),
    (0.80, 0.05, 0.90, 2, 28),
    (0.60, 0.05, 0.80, 2, 199),
    (0.65, 0.01, 0.90, 2, 164),
]


@pytest.mark.parametrize("p1,alpha,pw,sides,expected", EXACT_CASES)
def test_known_exact_sign_test_sample_sizes(p1, alpha, pw, sides, expected):
    assert required_decisive_pairs(p1, alpha=alpha, power=pw, sides=sides) == expected


@pytest.mark.parametrize("p1,alpha,pw,sides,expected", EXACT_CASES)
def test_exact_n_is_the_smallest_attaining_target_per_naive_reference(
    p1, alpha, pw, sides, expected
):
    """The reported N attains the target and no smaller N does."""
    assert _ref_power(expected, p1, alpha, sides) >= pw
    for n in range(1, expected):
        assert _ref_power(n, p1, alpha, sides) < pw


def test_sign_test_power_matches_naive_reference():
    for n in (5, 12, 23, 30, 47):
        for p1 in (0.6, 0.75, 0.9):
            got = sign_test_power(n, p1, alpha=0.05, sides=2)
            assert got == pytest.approx(_ref_power(n, p1, 0.05, 2), abs=1e-12)


def test_textbook_normal_approximation_value_brackets_the_exact_answer():
    """Noether's formula gives 29 for p1=0.75, alpha=.05 two-sided, power .80.

    The exact answer is 30. The approximation understating by a little is the
    documented behaviour, so this pins both the reference arithmetic and the
    direction of the discrepancy.
    """
    z_a = 1.959963984540054  # Phi^{-1}(0.975)
    z_b = 0.8416212335729143  # Phi^{-1}(0.80)
    n_approx = math.ceil(
        ((z_a * 0.5 + z_b * math.sqrt(0.75 * 0.25)) / 0.25) ** 2
    )
    assert n_approx == 29
    exact = required_decisive_pairs(0.75, alpha=0.05, power=0.80, sides=2)
    assert exact == 30
    assert exact >= n_approx


def test_normal_approximation_path_is_labelled_when_exact_is_out_of_reach():
    """Forcing exact_max_n low must fall back and SAY it fell back."""
    n, method, achieved = required_decisive_pairs(
        0.55, alpha=0.05, power=0.80, sides=2, exact_max_n=10, _detail=True
    )
    assert method == "normal_approximation"
    assert achieved is None
    assert n == 783  # Noether's formula for p1 = 0.55
    exact = required_decisive_pairs(0.55, alpha=0.05, power=0.80, sides=2)
    assert exact == 786
    assert exact > n


def test_sawtooth_is_real_so_the_docstring_warning_is_earned():
    """Exact power is not monotone in n; 31 is worse than 30 here."""
    assert sign_test_power(30, 0.75) >= 0.80
    assert sign_test_power(31, 0.75) < 0.80


# --------------------------------------------------------------------------- #
# Tie inflation
# --------------------------------------------------------------------------- #

def test_tie_inflation_factor_values():
    assert tie_inflation_factor(0.0) == 1.0
    assert tie_inflation_factor(0.5) == pytest.approx(2.0)
    assert tie_inflation_factor(0.75) == pytest.approx(4.0)


def test_half_the_comparisons_tying_roughly_doubles_required_items():
    base = plan(0.75, tie_rate=0.0)
    tied = plan(0.75, tie_rate=0.5)
    assert base["required_items"] == 30
    assert tied["required_items"] == 60
    assert tied["required_items"] == 2 * base["required_items"]


def test_mined_corpus_non_decisive_rate_inflates_as_expected():
    """694 ties + 433 errors of 2102 leaves 975 decisive: rate 1127/2102."""
    rate = (694 + 433) / 2102
    p = plan(0.6, tie_rate=rate)
    assert p["required_decisive_pairs"] == 199
    assert p["required_items"] == math.ceil(199 / (1.0 - rate))
    # 199 * 2102 / 975 = 429.05..., so 430 items to expect 199 decisive pairs.
    assert p["required_items"] == 430
    assert p["assumptions"]["tie_rate"] == pytest.approx(rate)


# --------------------------------------------------------------------------- #
# Design effect
# --------------------------------------------------------------------------- #

def test_design_effect_kish_values():
    assert design_effect(0.0, 10.0) == 1.0
    assert design_effect(0.0, 1.0) == 1.0
    assert design_effect(0.05, 11.0) == pytest.approx(1.5)
    assert design_effect(1.0, 8.0) == pytest.approx(8.0)
    # A cluster of size 1 is no cluster at all, whatever the icc.
    assert design_effect(0.9, 1.0) == 1.0


def test_zero_icc_leaves_n_unchanged_and_positive_icc_raises_it():
    flat = plan(0.75, icc=0.0, mean_cluster_size=20.0)
    clustered = plan(0.75, icc=0.2, mean_cluster_size=20.0)
    assert flat["required_items"] == 30
    assert clustered["stages"]["design_effect"] == pytest.approx(1.0 + 19 * 0.2)
    assert clustered["required_items"] == math.ceil(30 * (1.0 + 19 * 0.2))
    assert clustered["required_items"] > flat["required_items"]


# --------------------------------------------------------------------------- #
# PPI
# --------------------------------------------------------------------------- #

def test_ppi_variance_factor_values():
    assert ppi_variance_factor(0.0) == 1.0
    assert ppi_variance_factor(0.5) == pytest.approx(0.75)
    assert ppi_variance_factor(0.8) == pytest.approx(0.36)


def test_ppi_reduces_the_budget_and_is_flagged_in_assumptions():
    without = plan(0.6)
    with_ppi = plan(0.6, ppi_correlation=0.8)
    assert without["assumptions"]["ppi_used"] is False
    assert without["assumptions"]["ppi_correlation"] is None
    assert with_ppi["assumptions"]["ppi_used"] is True
    assert with_ppi["assumptions"]["ppi_correlation"] == pytest.approx(0.8)
    assert with_ppi["required_items"] == math.ceil(199 * 0.36)
    assert with_ppi["required_items"] < without["required_items"]


# --------------------------------------------------------------------------- #
# Monotonicity
# --------------------------------------------------------------------------- #

def test_n_grows_as_the_effect_shrinks():
    sizes = [
        required_decisive_pairs(p, alpha=0.05, power=0.80, sides=2)
        for p in (0.85, 0.75, 0.70, 0.65, 0.60)
    ]
    assert sizes == sorted(sizes)
    assert sizes[0] < sizes[-1]


def test_n_grows_as_alpha_tightens():
    sizes = [
        required_decisive_pairs(0.7, alpha=a, power=0.80, sides=2)
        for a in (0.10, 0.05, 0.01, 0.001)
    ]
    assert sizes == sorted(sizes)
    assert sizes[0] < sizes[-1]


def test_n_grows_as_power_rises():
    sizes = [
        required_decisive_pairs(0.7, alpha=0.05, power=pw, sides=2)
        for pw in (0.50, 0.70, 0.80, 0.90, 0.99)
    ]
    assert sizes == sorted(sizes)
    assert sizes[0] < sizes[-1]


def test_two_sided_never_needs_fewer_items_than_one_sided():
    for p1 in (0.6, 0.7, 0.8):
        one = required_decisive_pairs(p1, sides=1)
        two = required_decisive_pairs(p1, sides=2)
        assert two >= one


def test_items_grow_monotonically_in_tie_rate_and_icc():
    tie_sizes = [plan(0.7, tie_rate=t)["required_items"] for t in (0.0, 0.25, 0.5, 0.9)]
    assert tie_sizes == sorted(tie_sizes)
    icc_sizes = [
        plan(0.7, icc=i, mean_cluster_size=10.0)["required_items"]
        for i in (0.0, 0.1, 0.5, 1.0)
    ]
    assert icc_sizes == sorted(icc_sizes)


# --------------------------------------------------------------------------- #
# Refusals: every one raises, none returns a number
# --------------------------------------------------------------------------- #

@pytest.mark.parametrize(
    "kwargs",
    [
        {"win_rate": 0.5},  # zero effect needs infinite N
        {"win_rate": 0.0},
        {"win_rate": 1.0},
        {"win_rate": -0.2},
        {"win_rate": 1.4},
        {"win_rate": float("nan")},
        {"win_rate": "big"},
        {"win_rate": 0.7, "alpha": 0.0},
        {"win_rate": 0.7, "alpha": 1.0},
        {"win_rate": 0.7, "alpha": -0.05},
        {"win_rate": 0.7, "power": 0.0},
        {"win_rate": 0.7, "power": 1.0},
        {"win_rate": 0.7, "power": 1.5},
        {"win_rate": 0.7, "sides": 0},
        {"win_rate": 0.7, "sides": 3},
        {"win_rate": 0.7, "tie_rate": 1.0},  # nothing is ever decisive
        {"win_rate": 0.7, "tie_rate": 1.2},
        {"win_rate": 0.7, "tie_rate": -0.1},
        {"win_rate": 0.7, "tie_rate": float("nan")},
        {"win_rate": 0.7, "icc": -0.01},
        {"win_rate": 0.7, "icc": 1.01},
        {"win_rate": 0.7, "icc": "high"},
        {"win_rate": 0.7, "mean_cluster_size": 0.0},
        {"win_rate": 0.7, "mean_cluster_size": -3.0},
        {"win_rate": 0.7, "ppi_correlation": 1.0},
        {"win_rate": 0.7, "ppi_correlation": 1.5},
        {"win_rate": 0.7, "ppi_correlation": -0.3},
    ],
)
def test_impossible_inputs_raise_instead_of_returning_a_number(kwargs):
    with pytest.raises(PowerPlanError):
        plan(**kwargs)


def test_refusal_messages_name_the_offending_input():
    with pytest.raises(PowerPlanError, match="win_rate"):
        plan(0.5)
    with pytest.raises(PowerPlanError, match="tie_rate"):
        plan(0.7, tie_rate=1.0)
    with pytest.raises(PowerPlanError, match="icc"):
        plan(0.7, icc=2.0)
    with pytest.raises(PowerPlanError, match="mean_cluster_size"):
        plan(0.7, mean_cluster_size=0)
    with pytest.raises(PowerPlanError, match="ppi_correlation"):
        plan(0.7, ppi_correlation=1.0)


def test_power_helper_refuses_bad_n():
    with pytest.raises(PowerPlanError):
        sign_test_power(0, 0.7)
    with pytest.raises(PowerPlanError):
        sign_test_power(-5, 0.7)
    with pytest.raises(PowerPlanError):
        sign_test_power(3.5, 0.7)


def test_power_plan_error_is_a_value_error():
    assert issubclass(PowerPlanError, ValueError)


# --------------------------------------------------------------------------- #
# The output carries its assumptions
# --------------------------------------------------------------------------- #

def test_worked_end_to_end_plan_carries_every_assumption():
    p = plan(
        win_rate=0.62,
        alpha=0.05,
        power=0.80,
        sides=2,
        tie_rate=0.536,
        icc=0.08,
        mean_cluster_size=12.0,
        ppi_correlation=0.6,
        label="critic-v3 vs critic-v2 on IneqMath",
    )

    a = p["assumptions"]
    assert a["test"] == "paired sign test against p = 0.5"
    assert a["alternative_win_rate"] == 0.62
    assert a["effect_size_abs"] == pytest.approx(0.12)
    assert a["alpha"] == 0.05
    assert a["sides"] == 2
    assert a["target_power"] == 0.80
    assert a["tie_rate"] == 0.536
    assert a["icc"] == 0.08
    assert a["mean_cluster_size"] == 12.0
    assert a["ppi_used"] is True
    assert a["ppi_correlation"] == 0.6

    # The whole pipeline is reconstructible from the reported stages.
    n_stat = p["stages"]["sign_test_pairs"]
    assert n_stat == required_decisive_pairs(0.62, alpha=0.05, power=0.80, sides=2)
    assert p["stages"]["ppi_factor"] == pytest.approx(1.0 - 0.36)
    assert p["stages"]["design_effect"] == pytest.approx(1.0 + 11 * 0.08)
    assert p["stages"]["tie_inflation_factor"] == pytest.approx(1.0 / (1.0 - 0.536))
    expected_decisive = math.ceil(
        n_stat * p["stages"]["ppi_factor"] * p["stages"]["design_effect"]
    )
    assert p["required_decisive_pairs"] == expected_decisive
    assert p["required_items"] == math.ceil(
        expected_decisive * p["stages"]["tie_inflation_factor"]
    )
    assert p["label"] == "critic-v3 vs critic-v2 on IneqMath"


def test_plan_is_marked_as_planning_only_everywhere_it_matters():
    p = plan(0.7)
    assert p["planning_only"] is True
    assert p["disclaimer"] == DISCLAIMER
    assert "not evidence" in DISCLAIMER
    assert p["method_is_exact"] is True
    assert p["achieved_power_at_n"] >= 0.80
    assert p["exact_power_is_sawtoothed"] is True
    assert any("not a result" in note for note in p["notes"])


def test_no_public_name_suggests_it_validates_a_finished_result():
    import theoremata_tools.eval_power as mod

    banned = ("significant", "validate", "confirm", "verify", "proves")
    for name in mod.__all__:
        assert not any(b in name.lower() for b in banned)


def test_plan_output_is_json_serializable():
    import json

    json.loads(json.dumps(plan(0.7, ppi_correlation=0.5, tie_rate=0.3)))


# --------------------------------------------------------------------------- #
# Worker entry
# --------------------------------------------------------------------------- #

def test_run_plan_op():
    out = run({"op": "plan", "win_rate": 0.75, "tie_rate": 0.5})
    assert out["required_items"] == 60
    assert out["planning_only"] is True


def test_run_power_op():
    out = run({"op": "power", "n": 30, "win_rate": 0.75})
    assert out["power"] == pytest.approx(sign_test_power(30, 0.75))
    assert out["planning_only"] is True
    assert out["assumptions"]["alpha"] == 0.05


def test_run_rejects_unknown_op_and_bad_n():
    with pytest.raises(PowerPlanError):
        run({"op": "nope"})
    with pytest.raises(PowerPlanError):
        run({"op": "power", "n": "thirty"})
