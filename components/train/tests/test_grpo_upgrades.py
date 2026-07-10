import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.grpo import grpo_config  # noqa: E402
from theoremata_tools.grpo_upgrades import (  # noqa: E402
    gdpo_advantages,
    normalize_then_sum,
    run,
    two_grpo_config,
)


# --- 2-GRPO preset ---------------------------------------------------------

def test_two_grpo_sets_group_size_to_2_and_preserves_other_keys():
    base = grpo_config("m", "d")  # ships num_generations=8
    assert base["num_generations"] == 8
    out = two_grpo_config(base)
    # every group-size vocabulary now reads 2
    assert out["num_generations"] == 2
    assert out["group_size"] == 2
    assert out["G"] == 2
    # other keys untouched
    assert out["model"] == "m"
    assert out["learning_rate"] == base["learning_rate"]
    assert out["epsilon_high"] == base["epsilon_high"]
    # documents the savings vs the previous G=8
    assert out["grpo_preset"] == "2-GRPO"
    assert out["rollout_savings_vs_prev"] == 4.0  # 8 / 2


def test_two_grpo_does_not_mutate_input():
    base = grpo_config("m", "d")
    two_grpo_config(base)
    assert base["num_generations"] == 8  # original left intact


# --- normalize-then-sum: hand-computed -------------------------------------

def test_normalize_then_sum_zero_means_each_component_then_sums():
    # correctness=[1,1,0], format=[1,0,0]. Each z-normalized (pop std) then summed.
    comps = {"correctness": [1.0, 1.0, 0.0], "format": [1.0, 0.0, 0.0]}
    out = normalize_then_sum(comps)
    # by symmetry the two channels are mirror images -> sum is antisymmetric
    # correctness norm: [ 0.70710678,  0.70710678, -1.41421356]
    # format      norm: [ 1.41421356, -0.70710678, -0.70710678]
    expected = [2.1213203435596424, 0.0, -2.1213203435596424]
    assert len(out) == 3
    for got, exp in zip(out, expected):
        assert math.isclose(got, exp, abs_tol=1e-9)


def test_each_component_is_zero_mean_after_normalization():
    # a 2-sample sanity check: [3, 7] -> mean 5, pstdev 2 -> [-1, +1]
    out = normalize_then_sum({"a": [3.0, 7.0]})
    assert math.isclose(out[0], -1.0, abs_tol=1e-12)
    assert math.isclose(out[1], 1.0, abs_tol=1e-12)
    assert math.isclose(sum(out), 0.0, abs_tol=1e-12)


# --- advantage collapse: huge-scale channel must NOT dominate --------------

def test_huge_scale_component_does_not_dominate_gdpo():
    big = [100.0, 0.0, 0.0]   # one channel with a huge dynamic range
    small = [0.0, 1.0, 2.0]   # a small channel that orders samples 0<1<2
    comps = {"big": big, "small": small}

    # RAW additive sum: the big channel dwarfs the small one -> sample 0 wins
    # purely because of scale, and 1 vs 2 are nearly indistinguishable.
    raw = [b + s for b, s in zip(big, small)]  # [100, 1, 2]
    assert raw.index(max(raw)) == 0  # big-scale sample dominates the raw blend

    # GDPO normalize-then-sum: both channels rescaled to unit variance first,
    # so the big channel no longer dominates and the small channel's ordering
    # actually decides the top sample.
    adv = gdpo_advantages(comps, center=False)
    assert adv.index(max(adv)) != 0          # big-scale sample no longer wins
    assert adv.index(max(adv)) == 2          # small channel's ordering survives


def test_zero_variance_component_handled_no_crash():
    # 'const' fires identically for every sample -> std 0 -> contributes 0.
    comps = {"const": [0.5, 0.5, 0.5], "signal": [0.0, 1.0, 2.0]}
    out = normalize_then_sum(comps)
    # equals the normalized 'signal' alone: [-1.2247, 0, 1.2247]
    assert math.isclose(out[0], -1.224744871391589, abs_tol=1e-9)
    assert math.isclose(out[1], 0.0, abs_tol=1e-9)
    assert math.isclose(out[2], 1.224744871391589, abs_tol=1e-9)


def test_gdpo_centering_zero_means_the_advantages():
    comps = {"a": [1.0, 2.0, 3.0], "b": [10.0, 0.0, 5.0]}
    centered = gdpo_advantages(comps, center=True)
    assert math.isclose(sum(centered), 0.0, abs_tol=1e-9)


def test_determinism():
    comps = {"correctness": [1.0, 0.0, 1.0, 0.0], "tool": [0.1, 0.0, 0.0, 0.1]}
    a = gdpo_advantages(comps)
    b = gdpo_advantages(comps)
    assert a == b


def test_empty_inputs():
    assert normalize_then_sum({}) == []
    assert gdpo_advantages({}) == []


# --- run() dispatch --------------------------------------------------------

def test_run_dispatch_two_grpo_config():
    out = run({"op": "two_grpo_config", "config": grpo_config("m", "d")})
    assert out["ok"] is True
    assert out["config"]["num_generations"] == 2


def test_run_dispatch_normalize_then_sum():
    out = run({"op": "normalize_then_sum", "components": {"a": [3.0, 7.0]}})
    assert out["ok"] is True
    assert math.isclose(out["blended"][0], -1.0, abs_tol=1e-12)


def test_run_dispatch_gdpo_advantages():
    out = run({"op": "gdpo_advantages", "components": {"a": [1.0, 3.0]}, "center": False})
    assert out["ok"] is True
    assert len(out["advantages"]) == 2


def test_run_unknown_op_returns_not_ok():
    out = run({"op": "nope"})
    assert out["ok"] is False
    assert "unknown op" in out["error"]
