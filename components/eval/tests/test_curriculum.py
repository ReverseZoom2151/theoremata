import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from theoremata_tools.curriculum import (  # noqa: E402
    EASY,
    HARD,
    MEDIUM,
    UNKNOWN,
    annotate_curriculum,
    count_tactic_steps,
    curriculum_tiers,
    difficulty,
    order_by_curriculum,
    run,
)

# A three-tactic Lean proof.
PROOF_3 = """by
  intro n
  simp
  omega"""

PROOF_1 = "by rfl"
PROOF_SORRY = """by
  intro n
  sorry"""


def test_count_tactic_steps_lines():
    assert count_tactic_steps(PROOF_3) == 3
    assert count_tactic_steps(PROOF_1) == 1


def test_count_tactic_steps_semicolon_and_combinator():
    assert count_tactic_steps("by simp; omega") == 2
    assert count_tactic_steps("by constructor <;> simp") == 2


def test_count_tactic_steps_list():
    assert count_tactic_steps(["intro n", "simp", "omega"]) == 3


def test_difficulty_is_exp_of_steps():
    assert difficulty(PROOF_3) == math.exp(3)
    assert difficulty(PROOF_1) == math.exp(1)
    assert difficulty(["a", "b"]) == math.exp(2)


def test_difficulty_sorry_is_inf():
    assert difficulty(PROOF_SORRY) == math.inf
    assert math.isinf(difficulty("by admit"))


def test_difficulty_no_proof_is_none():
    assert difficulty(None) is None
    assert difficulty("") is None
    assert difficulty("   ") is None
    assert difficulty([]) is None


def test_curriculum_tiers_percentile_buckets():
    # exp(1) < exp(3) < exp(5): terciles put them Easy / Medium / Hard.
    diffs = [difficulty("by rfl"), difficulty(PROOF_3), difficulty("by\n a\n b\n c\n d\n e")]
    tiers = curriculum_tiers(diffs)
    assert tiers == [EASY, MEDIUM, HARD]


def test_curriculum_tiers_sorry_and_none():
    diffs = [difficulty(PROOF_1), difficulty(PROOF_SORRY), difficulty(None)]
    tiers = curriculum_tiers(diffs)
    assert tiers[0] == EASY
    assert tiers[1] == HARD  # sorry -> inf -> Hard
    assert tiers[2] == UNKNOWN  # no proof


def test_order_by_curriculum_easy_to_hard():
    items = [
        {"id": "hard", "proof": "by\n a\n b\n c\n d\n e"},   # 5 tactics
        {"id": "easy", "proof": PROOF_1},                     # 1 tactic
        {"id": "medium", "proof": PROOF_3},                   # 3 tactics
    ]
    ordered = order_by_curriculum(items)
    assert [it["id"] for it in ordered] == ["easy", "medium", "hard"]


def test_order_puts_sorry_before_no_proof():
    items = [
        {"id": "noproof"},
        {"id": "sorry", "proof": PROOF_SORRY},
        {"id": "easy", "proof": PROOF_1},
    ]
    ordered = order_by_curriculum(items)
    assert [it["id"] for it in ordered] == ["easy", "sorry", "noproof"]


def test_order_stable_on_ties():
    items = [{"id": f"t{i}", "proof": PROOF_1} for i in range(4)]
    ordered = order_by_curriculum(items)
    assert [it["id"] for it in ordered] == ["t0", "t1", "t2", "t3"]


def test_annotate_curriculum():
    items = [
        {"id": "medium", "proof": PROOF_3},
        {"id": "easy", "proof": PROOF_1},
    ]
    rows = annotate_curriculum(items)
    assert rows[0]["item"]["id"] == "easy"
    assert rows[0]["difficulty"] == math.exp(1)
    assert rows[0]["tier"] in {EASY, MEDIUM, HARD}


def test_run_dispatch():
    out = run({"op": "difficulty", "proof": PROOF_3})
    assert out["difficulty"] == math.exp(3)
    assert out["tactic_steps"] == 3
    out2 = run({"op": "order_by_curriculum", "items": [
        {"id": "b", "proof": PROOF_3}, {"id": "a", "proof": PROOF_1},
    ]})
    assert [it["id"] for it in out2["ordered"]] == ["a", "b"]
