import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.reward import (  # noqa: E402
    correctness_reward,
    majority_at_k,
    majority_pass_at_k,
    make_reward_fn,
    pass_at_k,
    reward,
    tool_use_reward,
    used_tool,
)

CLEAN = {"compiled": True, "axioms_ok": True}
DIRTY_AXIOMS = {"compiled": True, "axioms_ok": False}
FAILED = {"compiled": False, "axioms_ok": True}


# --- correctness reward ----------------------------------------------------

def test_correctness_reward_clean_verdict_is_one():
    assert correctness_reward(CLEAN) == 1.0


def test_correctness_reward_failure_is_zero():
    assert correctness_reward(FAILED) == 0.0
    assert correctness_reward(DIRTY_AXIOMS) == 0.0  # axiom gate not clean


def test_correctness_reward_no_verdict_is_none():
    assert correctness_reward(None) is None
    assert correctness_reward({}) is None


# --- composite reward: 1.0 / 0 / None -------------------------------------

def test_reward_one_on_clean_verdict():
    assert reward({"gold": "theorem t : True", "verdict": CLEAN}) == 1.0


def test_reward_zero_on_failure():
    assert reward({"gold": "theorem t : True", "verdict": FAILED}) == 0.0
    assert reward({"gold": "theorem t : True", "verdict": DIRTY_AXIOMS}) == 0.0


def test_reward_none_on_missing_gold():
    assert reward({"verdict": CLEAN}) is None
    assert reward({"gold": None, "verdict": CLEAN}) is None
    assert reward({"gold": "   ", "verdict": CLEAN}) is None  # unparseable gold


def test_reward_none_when_gold_present_but_unchecked():
    assert reward({"gold": "t", "verdict": None}) is None


# --- tool-use shaping (decoupled from correctness) -------------------------

def test_used_tool_detects_successful_calls():
    assert used_tool({"tool_calls": [{"name": "falsify", "ok": True}]}) is True
    assert used_tool({"tools_used": ["falsify"]}) is True
    assert used_tool({"used_falsifier": True}) is True
    assert used_tool({}) is False


def test_used_tool_ignores_errored_calls():
    assert used_tool({"tool_calls": [{"name": "falsify", "error": "boom"}]}) is False
    assert used_tool({"tool_calls": [{"name": "falsify", "ok": False}]}) is False


def test_tool_use_reward_weight():
    assert tool_use_reward({"used_tool": True}) == 0.1
    assert tool_use_reward({"used_tool": True}, weight=0.2) == 0.2
    assert tool_use_reward({}) == 0.0


def test_tool_shaping_added_to_correctness_when_enabled():
    s = {"gold": "t", "verdict": CLEAN, "used_tool": True}
    # correctness-only by default
    assert reward(s) == 1.0
    # shaping enabled adds 0.1 even to a passing proof
    assert reward(s, tool_weight=0.1) == 1.1


def test_tool_shaping_independent_of_correctness():
    # a failing proof that still used a tool legitimately gets the shaping bonus
    s = {"gold": "t", "verdict": FAILED, "used_tool": True}
    assert reward(s, tool_weight=0.1) == 0.1


def test_make_reward_fn_binds_tool_weight():
    fn = make_reward_fn(tool_weight=0.1)
    assert fn({"gold": "t", "verdict": CLEAN, "used_tool": True}) == 1.1
    assert fn({"verdict": CLEAN}) is None


# --- aggregation helpers ---------------------------------------------------

def test_pass_at_k():
    assert pass_at_k([FAILED, FAILED, CLEAN]) == 1.0
    assert pass_at_k([FAILED, DIRTY_AXIOMS]) == 0.0
    assert pass_at_k([None, None]) == 0.0


def test_majority_at_k():
    assert majority_at_k(["42", "42", "7"]) == "42"
    assert majority_at_k([]) is None
    # tie breaks toward earliest-seen
    assert majority_at_k(["a", "b"]) == "a"


def test_majority_at_k_nonhashable():
    # lists aren't hashable; keyed by repr, original object returned
    assert majority_at_k([[1], [1], [2]]) == [1]


def test_majority_pass_at_k():
    passing = {"verdict": CLEAN}
    failing = {"verdict": FAILED}
    assert majority_pass_at_k([passing, passing, failing]) == 1.0
    assert majority_pass_at_k([passing, failing, failing]) == 0.0
    assert majority_pass_at_k([]) == 0.0
