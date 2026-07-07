"""Tests for the retrieval eval harness (R@1/R@10/MRR) with hand-checked numbers."""
import pytest

from theoremata_tools import retrieval_eval as E


# --- per-example primitives --------------------------------------------------


def test_recall_at_k_basic():
    retrieved = ["a", "b", "c", "d"]
    gold = ["b", "d"]
    assert E.recall_at_k(retrieved, gold, 1) == 0.0        # top1 = {a}
    assert E.recall_at_k(retrieved, gold, 2) == 0.5        # top2 = {a,b} -> 1/2
    assert E.recall_at_k(retrieved, gold, 4) == 1.0        # both found


def test_recall_empty_gold_is_one():
    assert E.recall_at_k(["a"], [], 1) == 1.0


def test_reciprocal_rank():
    assert E.reciprocal_rank(["x", "gold", "y"], ["gold"]) == pytest.approx(1 / 2)
    assert E.reciprocal_rank(["gold"], ["gold"]) == 1.0
    assert E.reciprocal_rank(["x", "y"], ["gold"]) == 0.0


# --- corpus-level metrics with exact expected values -------------------------


def test_evaluate_exact_metrics():
    # Two examples, hand-computed:
    #  ex1: gold={g1}, retrieved=[g1, x, y, ...]      -> R@1=1, R@10=1, RR=1
    #  ex2: gold={g2}, retrieved=[x, x, x, x, g2, ...] -> R@1=0, R@10=1, RR=1/5
    examples = [
        {"id": 1, "retrieved": ["g1", "a", "b"], "gold": ["g1"]},
        {"id": 2, "retrieved": ["a", "b", "c", "d", "g2"], "gold": ["g2"]},
    ]
    m = E.evaluate(examples, ks=[1, 10])
    assert m["R@1"] == pytest.approx((1.0 + 0.0) / 2)      # 0.5
    assert m["R@10"] == pytest.approx((1.0 + 1.0) / 2)     # 1.0
    assert m["MRR"] == pytest.approx((1.0 + 0.2) / 2)      # 0.6
    assert m["n"] == 2
    assert m["recall"]["1"] == pytest.approx(0.5)


def test_evaluate_multi_gold_recall_fraction():
    # gold has 2 premises; only 1 in top-2 -> R@2 = 0.5
    examples = [{"retrieved": ["g1", "x", "g2"], "gold": ["g1", "g2"]}]
    m = E.evaluate(examples, ks=[2])
    assert m["R@2"] == pytest.approx(0.5)
    # first gold hit at rank 1 -> RR = 1
    assert m["MRR"] == 1.0


def test_perfect_and_zero_retrievers():
    examples = [{"retrieved": ["g"], "gold": ["g"]}]
    assert E.evaluate(examples, ks=[1])["R@1"] == 1.0
    examples = [{"retrieved": ["nope"], "gold": ["g"]}]
    m = E.evaluate(examples, ks=[1])
    assert m["R@1"] == 0.0
    assert m["MRR"] == 0.0


# --- predictions-style input -------------------------------------------------


def test_evaluate_predictions_inline_gold():
    predictions = [
        {"id": "t1", "retrieved_premises": ["g1", "a"], "all_pos_premises": ["g1"]},
        {"id": "t2", "retrieved_premises": ["a", "g2"], "all_pos_premises": ["g2"]},
    ]
    m = E.evaluate_predictions(predictions, ks=[1, 10])
    assert m["R@1"] == pytest.approx(0.5)   # t1 hit@1, t2 miss@1
    assert m["MRR"] == pytest.approx((1.0 + 0.5) / 2)


def test_evaluate_predictions_external_gold_map():
    predictions = [{"id": "t1", "retrieved_premises": ["a", "b", "g"]}]
    gold = {"t1": ["g"]}
    m = E.evaluate_predictions(predictions, gold, ks=[1, 10])
    assert m["R@10"] == 1.0
    assert m["MRR"] == pytest.approx(1 / 3)


def test_gold_records_with_full_name():
    # Gold premises given as records (ReProver's Premise shape) resolve by name.
    examples = [{"retrieved": [{"name": "g"}], "gold": [{"full_name": "g"}]}]
    assert E.evaluate(examples, ks=[1])["R@1"] == 1.0


def test_run_dispatch():
    assert E.run(examples=[{"retrieved": ["g"], "gold": ["g"]}])["R@1"] == 1.0
    assert E.run(predictions=[{"id": 1, "retrieved_premises": ["g"], "all_pos_premises": ["g"]}])["MRR"] == 1.0
    assert E.run()["ok"] is False
