"""Offline, deterministic tests for the conjecture-discovery pipeline.

Core tests exercise the dependency-free fallback (closed-form ridge / logistic +
permutation importance); the sklearn / torch paths are importorskip-gated. On
synthetic data where the invariant depends only on x1 & x3 the pipeline must
detect the relationship, attribute it to x1 & x3, and name {x1, x3}; on pure
noise it must not manufacture a conjecture.
"""
import random
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.conjecture_discovery import (  # noqa: E402
    attribute,
    build_dataset,
    detect_relationship,
    discover,
    propose_conjecture,
    run,
)

NAMES = ["x1", "x2", "x3", "x4", "x5"]


def _signal_data(seed=1, n=200, noise=0.02):
    """y = 3*x1 + 2*x3 + small noise; x2,x4,x5 are irrelevant."""
    rng = random.Random(seed)
    X, y = [], []
    for _ in range(n):
        row = [rng.uniform(-1.0, 1.0) for _ in range(5)]
        X.append(row)
        y.append(3.0 * row[0] + 2.0 * row[2] + rng.gauss(0.0, noise))
    return X, y


def _noise_data(seed=2, n=200):
    """y independent of X (pure noise)."""
    rng = random.Random(seed)
    X = [[rng.uniform(-1.0, 1.0) for _ in range(5)] for _ in range(n)]
    y = [rng.gauss(0.0, 1.0) for _ in range(n)]
    return X, y


# --- 1. build_dataset: injected samplers, deterministic --------------------

def test_build_dataset_named_features_deterministic():
    def sampler(rng):
        return {"a": rng.uniform(0, 1), "b": rng.uniform(0, 1)}

    def feats(o):
        return o  # dict of named features

    def inv(o):
        return 2.0 * o["a"] - o["b"]

    ds1 = build_dataset(sampler, feats, inv, seed=0, n=8)
    ds2 = build_dataset(sampler, feats, inv, seed=0, n=8)
    assert ds1["feature_names"] == ["a", "b"]  # dict keys sorted
    assert len(ds1["X"]) == 8 and len(ds1["y"]) == 8
    assert ds1 == ds2  # seeded -> identical


def test_build_dataset_unnamed_vector_features():
    def sampler(rng):
        return [rng.uniform(-1, 1) for _ in range(3)]

    ds = build_dataset(sampler, lambda o: o, lambda o: o[0], seed=5, n=6)
    assert ds["feature_names"] == ["x1", "x2", "x3"]
    assert all(len(row) == 3 for row in ds["X"])


# --- 2. detect_relationship (fallback, offline) ----------------------------

def test_detect_relationship_true_on_signal_fallback():
    X, y = _signal_data()
    det = detect_relationship(X, y, backend="fallback", seed=0)
    assert det["backend"] == "fallback"
    assert det["task"] == "regression"
    assert det["exists"] is True
    assert det["strength"] > 0.8
    assert det["gap"] > 0.1  # beats permuted-label baseline


def test_detect_relationship_false_on_noise_fallback():
    X, y = _noise_data()
    det = detect_relationship(X, y, backend="fallback", seed=0)
    assert det["exists"] is False
    assert det["strength"] < 0.2  # no held-out signal


def test_detect_relationship_classification_fallback():
    rng = random.Random(3)
    X, y = [], []
    for _ in range(200):
        row = [rng.uniform(-2, 2) for _ in range(4)]
        X.append(row)
        y.append(1.0 if (2.0 * row[0] - 1.5 * row[1]) > 0 else 0.0)
    det = detect_relationship(X, y, backend="fallback", seed=0)
    assert det["task"] == "classification"
    assert det["exists"] is True
    assert det["strength"] > 0.8  # accuracy
    attr = attribute(det["model"], X, y, ["a", "b", "c", "d"], seed=0)
    assert {n for n, _ in attr["importances"][:2]} == {"a", "b"}


# --- 3. attribute: permutation importance ranks the true drivers -----------

def test_attribute_ranks_true_drivers_top():
    X, y = _signal_data()
    det = detect_relationship(X, y, backend="fallback", seed=0)
    attr = attribute(det["model"], X, y, NAMES, seed=0)
    top2 = {n for n, _ in attr["importances"][:2]}
    assert top2 == {"x1", "x3"}
    # irrelevant features have near-zero importance
    imp = dict(attr["importances"])
    assert imp["x1"] > imp["x2"]
    assert imp["x3"] > imp["x4"]
    # linear model -> coefficient saliency also surfaces x1, x3
    assert attr["coef_saliency"] is not None
    assert {n for n, _ in attr["coef_saliency"][:2]} == {"x1", "x3"}


def test_permutation_importance_deterministic():
    X, y = _signal_data()
    det = detect_relationship(X, y, backend="fallback", seed=0)
    a1 = attribute(det["model"], X, y, NAMES, seed=7)
    a2 = attribute(det["model"], X, y, NAMES, seed=7)
    assert a1["importances"] == a2["importances"]


# --- 4. propose_conjecture -------------------------------------------------

def test_propose_conjecture_names_key_features():
    imps = [("x1", 0.9), ("x3", 0.6), ("x2", 0.01), ("x4", 0.0), ("x5", -0.01)]
    prop = propose_conjecture(NAMES, imps, strength=0.95)
    assert set(prop["key_features"]) == {"x1", "x3"}
    assert prop["relationship"] is True
    assert prop["confidence"] > 0.5
    assert prop["form"]["kind"] == "functional_dependence"
    assert "x1" in prop["conjecture"] and "x3" in prop["conjecture"]
    # machine-usable form carries a plain-text statement for novelty
    assert isinstance(prop["form"]["statement"], str)
    assert prop["form"]["target"] == "invariant"


def test_propose_conjecture_no_false_conjecture_on_weak_strength():
    imps = [("x2", 0.02), ("x1", 0.01)]
    prop = propose_conjecture(NAMES, imps, strength=0.05)
    assert prop["relationship"] is False
    assert prop["key_features"] == []
    assert prop["confidence"] < 0.3
    assert prop["form"]["kind"] == "no_relationship"


def test_propose_respects_exists_false():
    imps = [("x1", 0.9), ("x3", 0.6)]
    prop = propose_conjecture(NAMES, imps, strength=0.95, exists=False)
    assert prop["relationship"] is False


# --- 5. end-to-end discover / run ------------------------------------------

def test_discover_end_to_end_signal():
    X, y = _signal_data()
    res = discover(X, y, NAMES, backend="fallback", seed=0)
    assert res["exists"] is True
    assert set(res["key_features"]) == {"x1", "x3"}
    assert res["confidence"] > 0.5
    assert res["form"]["kind"] == "functional_dependence"


def test_discover_end_to_end_noise_emits_no_conjecture():
    X, y = _noise_data()
    res = discover(X, y, NAMES, backend="fallback", seed=0)
    assert res["exists"] is False
    assert res["relationship"] is False
    assert res["key_features"] == []
    assert res["confidence"] < 0.3


def test_run_op_with_prepared_data():
    X, y = _signal_data()
    res = run({
        "op": "conjecture_discovery",
        "X": X, "y": y, "feature_names": NAMES,
        "backend": "fallback", "seed": 0,
    })
    assert res["op"] == "conjecture_discovery"
    assert res["exists"] is True
    assert set(res["key_features"]) == {"x1", "x3"}
    # output is JSON-serializable (no live estimator leaks through)
    import json
    json.loads(json.dumps(res))


def test_run_op_with_injected_samplers():
    def sampler(rng):
        return [rng.uniform(-1, 1) for _ in range(3)]

    def inv(o):
        return 4.0 * o[0]  # depends only on x1

    res = run({
        "op": "conjecture_discovery",
        "object_sampler": sampler,
        "feature_fn": lambda o: o,
        "invariant_fn": inv,
        "n": 200, "backend": "fallback", "seed": 0,
    })
    assert res["exists"] is True
    assert "x1" in res["key_features"]


def test_run_unknown_op_raises():
    with pytest.raises(ValueError):
        run({"op": "nope"})


def test_run_missing_inputs_raises():
    with pytest.raises(ValueError):
        run({"op": "conjecture_discovery"})


# --- 6. optional ML backends (importorskip-gated) --------------------------

def test_detect_relationship_sklearn_path():
    pytest.importorskip("sklearn")
    X, y = _signal_data()
    det = detect_relationship(X, y, backend="sklearn", seed=0)
    assert det["backend"] == "sklearn"
    assert det["exists"] is True
    attr = attribute(det["model"], X, y, NAMES, seed=0)
    assert {n for n, _ in attr["importances"][:2]} == {"x1", "x3"}


def test_detect_relationship_sklearn_classification():
    pytest.importorskip("sklearn")
    rng = random.Random(4)
    X, y = [], []
    for _ in range(200):
        row = [rng.uniform(-2, 2) for _ in range(4)]
        X.append(row)
        y.append(1.0 if (2.0 * row[0] - 1.5 * row[1]) > 0 else 0.0)
    det = detect_relationship(X, y, backend="sklearn", seed=0)
    assert det["backend"] == "sklearn"
    assert det["task"] == "classification"
    assert det["exists"] is True


def test_detect_relationship_torch_path():
    pytest.importorskip("torch")
    X, y = _signal_data()
    det = detect_relationship(X, y, backend="torch", seed=0)
    assert det["backend"] == "torch"
    assert det["exists"] is True
    attr = attribute(det["model"], X, y, NAMES, seed=0)
    assert {n for n, _ in attr["importances"][:2]} == {"x1", "x3"}
