import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.formalization_reward import (  # noqa: E402
    default_syntax_check,
    formalization_reward,
    is_nl_echo,
    is_trivial_statement,
    run,
    selection_policy,
)

# A well-formed, non-trivial Lean statement (the "good" formalization).
GOOD_LEAN = "theorem add_comm_nat (a b : Nat) : a + b = b + a := by sorry"
NL = "For all natural numbers a and b, a plus b equals b plus a."

# Injected mock checks so no Lean toolchain / LLM judge is needed.
SC_PASS = lambda lean: True          # noqa: E731
SC_FAIL = lambda lean: False         # noqa: E731
CC_PASS = lambda nl, lean: True      # noqa: E731
CC_FAIL = lambda nl, lean: False     # noqa: E731


# --- core SC∧CC rule -------------------------------------------------------

def test_reward_one_when_sc_and_cc_pass():
    out = formalization_reward(NL, GOOD_LEAN, syntax_check=SC_PASS, consistency_check=CC_PASS)
    assert out["reward"] == 1.0
    assert out["sc"] is True and out["cc"] is True
    assert out["trivial"] is False and out["nl_echo"] is False


def test_reward_zero_when_sc_fails():
    out = formalization_reward(NL, GOOD_LEAN, syntax_check=SC_FAIL, consistency_check=CC_PASS)
    assert out["reward"] == 0.0
    assert out["sc"] is False


def test_reward_zero_when_cc_fails():
    out = formalization_reward(NL, GOOD_LEAN, syntax_check=SC_PASS, consistency_check=CC_FAIL)
    assert out["reward"] == 0.0
    assert out["cc"] is False


def test_reward_zero_when_both_fail():
    out = formalization_reward(NL, GOOD_LEAN, syntax_check=SC_FAIL, consistency_check=CC_FAIL)
    assert out["reward"] == 0.0


# --- anti-hack: trivial always-compiling stub ------------------------------

TRIVIAL_TRUE = "theorem t : True := by sorry"
TRIVIAL_REFL = "theorem t (x : Nat) : x = x := by sorry"


def test_trivial_stub_rejected_even_though_sc_and_cc_pass():
    # SC passes (compiles), CC forced to pass — but the statement is vacuous.
    out = formalization_reward(NL, TRIVIAL_TRUE, syntax_check=SC_PASS, consistency_check=CC_PASS)
    assert out["reward"] == 0.0
    assert out["trivial"] is True
    assert "trivial" in out["reason"].lower()


def test_trivial_reflexive_equality_rejected():
    out = formalization_reward(NL, TRIVIAL_REFL, syntax_check=SC_PASS, consistency_check=CC_PASS)
    assert out["reward"] == 0.0
    assert out["trivial"] is True


def test_injected_triviality_check_is_authoritative():
    # GOOD_LEAN is not lexically trivial, but an injected screen flags it.
    out = formalization_reward(
        NL, GOOD_LEAN,
        syntax_check=SC_PASS, consistency_check=CC_PASS,
        triviality_check=lambda lean: True,
    )
    assert out["reward"] == 0.0 and out["trivial"] is True


def test_is_trivial_statement_helper():
    assert is_trivial_statement(TRIVIAL_TRUE) is True
    assert is_trivial_statement(TRIVIAL_REFL) is True
    assert is_trivial_statement(GOOD_LEAN) is False


# --- anti-hack: NL echoed back ---------------------------------------------

def test_nl_echo_rejected_even_though_cc_would_pass():
    # The "Lean statement" is just the NL problem restated as prose.
    out = formalization_reward(NL, NL, syntax_check=SC_PASS, consistency_check=CC_PASS)
    assert out["reward"] == 0.0
    assert out["nl_echo"] is True
    assert "echo" in out["reason"].lower()


def test_nl_echo_near_copy_rejected():
    echoed = "the statement that a + b = b + a for all a b"  # prose, no Lean header
    out = formalization_reward(echoed, echoed, syntax_check=SC_PASS, consistency_check=CC_PASS)
    assert out["reward"] == 0.0 and out["nl_echo"] is True


def test_is_nl_echo_helper():
    assert is_nl_echo(NL, NL) is True
    assert is_nl_echo(NL, GOOD_LEAN) is False


# --- default SC (lexical well-formedness stand-in) -------------------------

def test_default_syntax_check_accepts_wellformed():
    assert default_syntax_check(GOOD_LEAN) is True
    assert default_syntax_check(TRIVIAL_TRUE) is True  # compiles-with-sorry, trivial handled elsewhere


def test_default_syntax_check_rejects_malformed():
    assert default_syntax_check("") is False
    assert default_syntax_check("a + b = b + a") is False          # no header
    assert default_syntax_check("theorem t (a : Nat : a = a") is False  # unbalanced


# --- determinism -----------------------------------------------------------

def test_determinism():
    a = formalization_reward(NL, GOOD_LEAN, syntax_check=SC_PASS, consistency_check=CC_PASS)
    b = formalization_reward(NL, GOOD_LEAN, syntax_check=SC_PASS, consistency_check=CC_PASS)
    assert a == b


# --- selection_policy: SC gates sampling, CC scores only the winner --------

def test_selection_policy_sc_gates_and_cc_scores_winner():
    pool = [TRIVIAL_TRUE, "not lean prose", GOOD_LEAN]
    out = selection_policy(NL, pool, syntax_check=SC_PASS, consistency_check=CC_PASS)
    # Trivial stub (idx 0) and prose echo (idx 1) are dropped by the SC gate;
    # only the real statement survives and is CC-scored.
    assert out["winner_index"] == 2
    assert out["sc_pass"] == [2]
    assert out["reward"] == 1.0


def test_selection_policy_none_survive():
    out = selection_policy(NL, [TRIVIAL_TRUE], syntax_check=SC_PASS, consistency_check=CC_PASS)
    assert out["winner"] is None and out["reward"] == 0.0


# --- run() dispatch --------------------------------------------------------

def test_run_dispatch_formalization_reward():
    out = run({
        "nl_problem": NL,
        "lean_statement": GOOD_LEAN,
        "syntax_check": SC_PASS,
        "consistency_check": CC_PASS,
    })
    assert out["op"] == "formalization_reward" and out["reward"] == 1.0


def test_run_dispatch_selection_policy():
    out = run({
        "op": "selection_policy",
        "nl_problem": NL,
        "lean_statements": [GOOD_LEAN],
        "syntax_check": SC_PASS,
        "consistency_check": CC_PASS,
    })
    assert out["winner_index"] == 0 and out["reward"] == 1.0


def test_run_unknown_op_raises():
    import pytest
    with pytest.raises(ValueError):
        run({"op": "nope"})


# --- default CC path (round-trip-backed) is offline-safe -------------------

def test_default_cc_offline_smoke():
    # No injected CC: exercises make_default_consistency_check's fallback path.
    # Uses the default lexical SC too. Must not raise and must be deterministic.
    out1 = formalization_reward(NL, GOOD_LEAN)
    out2 = formalization_reward(NL, GOOD_LEAN)
    assert out1 == out2
    assert out1["sc"] is True
    assert out1["reward"] in (0.0, 1.0)
