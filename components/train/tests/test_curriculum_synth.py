import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.curriculum_synth import (  # noqa: E402
    beam_self_filter,
    consistency_check,
    mine_cold_start_positives,
    parse_haves,
    run,
    subgoal_to_conjectures,
)


# --- parse_haves -----------------------------------------------------------

def test_parse_haves_named_and_anonymous():
    proof = (
        "have h1 : a < b := by linarith\n"
        "  have : b < c := by exact hbc\n"
        "have h3 : a < c := lt_trans h1 hbc\n"
        "exact h3"
    )
    haves = parse_haves(proof)
    assert [h["name"] for h in haves] == ["h1", None, "h3"]
    assert [h["statement"] for h in haves] == ["a < b", "b < c", "a < c"]


def test_parse_haves_statement_with_colon():
    haves = parse_haves("have h : ∀ x : Nat, x ≥ 0 := by simp")
    assert haves == [{"name": "h", "statement": "∀ x : Nat, x ≥ 0"}]


# --- subgoal_to_conjectures: both forms + premises -------------------------

def test_subgoal_expansion_yields_both_forms_with_premises():
    proof = (
        "have h1 : a < b := by linarith\n"
        "have h2 : b < c := by linarith\n"
        "exact lt_trans h1 h2"
    )
    thms = subgoal_to_conjectures({"goal": "a < c", "proof": proof})
    # two subgoals -> four conjectures (alone + with_premises each)
    assert len(thms) == 4
    forms = [(t["form"], t["subgoal"], t["premises"]) for t in thms]
    assert forms[0] == ("alone", "a < b", [])
    assert forms[1] == ("with_premises", "a < b", [])  # first has no preceding
    assert forms[2] == ("alone", "b < c", [])
    assert forms[3] == ("with_premises", "b < c", ["a < b"])  # carries predecessor
    # rendered statement threads premises via '->' so it is a ready flywheel problem
    assert thms[3]["statement"] == "a < b -> b < c"
    assert thms[2]["statement"] == "b < c"
    assert all("source" in t and t["source"] == "a < c" for t in thms)


def test_subgoal_expansion_accepts_bare_string():
    thms = subgoal_to_conjectures("have h1 : p := by trivial")
    assert len(thms) == 2
    assert thms[0]["source"] == ""


# --- mine_cold_start_positives ---------------------------------------------

def test_positive_mining_keeps_whole_fail_subgoals_solved():
    attempts = [
        {  # KEEP: whole failed, all subgoals solved
            "problem": "prove P",
            "whole_verified": False,
            "subgoals": [
                {"name": "h1", "statement": "a", "solved": True, "proof": "by simp"},
                {"name": "h2", "statement": "b", "solved": True, "proof": "by simp"},
            ],
        },
        {  # DROP: whole already verified
            "problem": "prove Q",
            "whole_verified": True,
            "subgoals": [{"statement": "c", "solved": True}],
        },
        {  # DROP: a subgoal is unsolved
            "problem": "prove R",
            "whole_verified": False,
            "subgoals": [
                {"statement": "d", "solved": True},
                {"statement": "e", "solved": False},
            ],
        },
        {  # DROP: no subgoals
            "problem": "prove S",
            "whole_verified": False,
            "subgoals": [],
        },
    ]
    rows = mine_cold_start_positives(attempts)
    assert len(rows) == 1
    row = rows[0]
    assert row["prompt"] == "prove P"
    assert row["meta"]["kind"] == "cold_start_positive"
    assert row["meta"]["n_subgoals"] == 2
    # completion reassembled from the solved subgoal proofs
    assert "have h1 : a := by simp" in row["completion"]
    assert "have h2 : b := by simp" in row["completion"]


def test_positive_mining_respects_assembled_override_and_dict_verdict():
    attempts = [
        {
            "goal": "G",
            "verified": {"compiled": False, "axioms_ok": True},
            "assembled_proof": "by decide",
            "subgoals": [{"statement": "x", "solved": {"compiled": True}}],
        }
    ]
    rows = mine_cold_start_positives(attempts)
    assert len(rows) == 1
    assert rows[0]["completion"] == "by decide"


# --- consistency_check -----------------------------------------------------

def test_consistency_check_flags_dropped_have_lemma():
    declared = [{"name": "h1", "statement": "a < b"}, {"name": "h2", "statement": "b < c"}]
    kept = "have h1 : a < b := by linarith\nhave h2 : b < c := by linarith\nexact _"
    dropped = "have h1 : a < b := by linarith\nexact sorry"  # h2 abandoned
    assert consistency_check(declared, kept) is True
    assert consistency_check(declared, dropped) is False


def test_consistency_check_string_declarations_and_empty():
    assert consistency_check(["h1", "h2"], "have h1 ... have h2 ...") is True
    assert consistency_check(["h1", "h2"], "have h1 ...") is False
    assert consistency_check([], "anything") is True


# --- beam_self_filter ------------------------------------------------------

def test_beam_self_filter_drops_easy_keeps_hard_deterministic():
    problems = ["easy", "hard", "flaky", "never"]
    solved_flags = [
        [True, True, True],    # easy: consistently solved -> drop
        [False, False, False],  # hard: never solved -> keep
        [True, False, True],   # flaky: not consistent -> keep
        False,                  # never: bare False -> keep
    ]
    kept = beam_self_filter(problems, solved_flags, keep_hard=True)
    assert kept == ["hard", "flaky", "never"]
    # determinism: identical inputs -> identical output, order preserved
    assert beam_self_filter(problems, solved_flags, keep_hard=True) == kept
    # inverse mode keeps exactly the easy ones
    assert beam_self_filter(problems, solved_flags, keep_hard=False) == ["easy"]


def test_beam_self_filter_missing_flags_treated_as_hard():
    problems = ["a", "b", "c"]
    kept = beam_self_filter(problems, [True], keep_hard=True)
    assert kept == ["b", "c"]  # a is easy (solved), b/c have no flag -> hard


# --- run() dispatch --------------------------------------------------------

def test_run_subgoal_to_conjectures():
    res = run({"op": "subgoal_to_conjectures", "proof": {"goal": "g", "proof": "have h : p := by simp"}})
    assert res["ok"] is True
    assert res["n"] == 2
    assert {t["form"] for t in res["theorems"]} == {"alone", "with_premises"}


def test_run_mine_cold_start_positives():
    res = run(
        {
            "op": "mine_cold_start_positives",
            "attempts": [
                {"problem": "P", "whole_verified": False, "subgoals": [{"statement": "a", "solved": True}]}
            ],
        }
    )
    assert res["ok"] is True
    assert res["kept"] == 1


def test_run_consistency_check():
    res = run({"op": "consistency_check", "declared_haves": ["h1", "h2"], "final_proof": "have h1 ..."})
    assert res["ok"] is True
    assert res["consistent"] is False
    assert res["dropped"] == ["h2"]


def test_run_beam_self_filter():
    res = run(
        {
            "op": "beam_self_filter",
            "problems": ["e", "h"],
            "solved_flags": [True, False],
            "keep_hard": True,
        }
    )
    assert res["ok"] is True
    assert res["kept"] == 1
    assert res["dropped"] == 1
    assert res["problems"] == ["h"]


def test_run_unknown_op_raises():
    try:
        run({"op": "nope"})
        assert False, "expected ValueError"
    except ValueError:
        pass
