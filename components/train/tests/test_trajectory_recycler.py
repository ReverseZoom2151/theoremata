import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.trajectory_recycler import (  # noqa: E402
    recycle_batch,
    recycle_failed_trajectory,
    run,
)


# --- recycle_failed_trajectory: reached goal + premise form ----------------

def test_failed_attempt_yields_reached_goal_and_premise():
    attempt = {
        "problem": "a < c",
        "reached_goal": "b < c",
        "partial_proof": "have h1 : a < b := by linarith",
        "solved": False,
    }
    rows = recycle_failed_trajectory(attempt)
    # reached-goal form + premise form
    assert len(rows) == 2
    reached, premise = rows
    assert reached["form"] == "reached_goal"
    assert reached["statement"] == "b < c"
    assert premise["form"] == "premise"
    # premise threads the reached state as a lemma toward the original goal
    assert premise["statement"] == "b < c -> a < c"
    assert premise["origin"] == "a < c"
    assert all(r["source"] == "failed_trajectory" for r in rows)
    # partial proof carried for provenance
    assert reached["partial_proof"] == "have h1 : a < b := by linarith"


def test_reached_state_from_dict_and_no_origin_skips_premise():
    # reached state carried as a dict; no original goal known -> only the
    # reached-goal row (premise form needs the original)
    attempt = {"reached_state": {"statement": "P x"}, "verified": False}
    rows = recycle_failed_trajectory(attempt)
    assert len(rows) == 1
    assert rows[0]["form"] == "reached_goal"
    assert rows[0]["statement"] == "P x"


def test_premise_skipped_when_origin_equals_reached():
    # no forward progress made: reached == original -> no useful premise bridge
    attempt = {"problem": "G", "open_goal": "G"}
    rows = recycle_failed_trajectory(attempt)
    assert len(rows) == 1
    assert rows[0]["form"] == "reached_goal"


# --- nothing to recycle ----------------------------------------------------

def test_fully_failed_attempt_with_no_reached_state_yields_nothing():
    attempt = {"problem": "hard", "solved": False, "partial_proof": ""}
    assert recycle_failed_trajectory(attempt) == []


def test_solved_attempt_is_not_recycled():
    # bool verdict
    assert recycle_failed_trajectory(
        {"problem": "P", "reached_goal": "Q", "solved": True}
    ) == []
    # structured verdict dict
    assert recycle_failed_trajectory(
        {"problem": "P", "reached_goal": "Q", "verified": {"compiled": True, "axioms_ok": True}}
    ) == []


def test_non_dict_attempt_yields_nothing():
    assert recycle_failed_trajectory("not a dict") == []
    assert recycle_failed_trajectory(None) == []


# --- recycle_batch: dedup + determinism ------------------------------------

def test_recycle_batch_dedups_by_statement():
    attempts = [
        {"problem": "a < c", "reached_goal": "b < c"},
        {"problem": "a < c", "reached_goal": "b < c"},  # exact duplicate
        {"problem": "x", "reached_goal": "y"},
        {"problem": "z", "solved": True, "reached_goal": "w"},  # solved -> skipped
    ]
    rows = recycle_batch(attempts)
    stmts = [r["statement"] for r in rows]
    # first attempt -> "b < c" + "b < c -> a < c"; second is a dup (dropped);
    # third -> "y" + "y -> x"; solved attempt contributes nothing
    assert stmts == ["b < c", "b < c -> a < c", "y", "y -> x"]


def test_recycle_batch_is_deterministic():
    attempts = [
        {"problem": "a", "reached_goal": "r1"},
        {"problem": "b", "reached_goal": "r2"},
    ]
    assert recycle_batch(attempts) == recycle_batch(attempts)


# --- run() dispatch --------------------------------------------------------

def test_run_recycle_single_attempt():
    res = run({"op": "recycle_trajectory", "attempt": {"problem": "G", "reached_goal": "H"}})
    assert res["ok"] is True
    assert res["n"] == 2
    assert {r["form"] for r in res["rows"]} == {"reached_goal", "premise"}


def test_run_recycle_batch():
    res = run(
        {
            "op": "recycle_trajectory",
            "attempts": [
                {"problem": "G", "reached_goal": "H"},
                {"problem": "G", "reached_goal": "H"},  # dup
            ],
        }
    )
    assert res["ok"] is True
    assert res["n"] == 2  # deduped across the batch


def test_run_defaults_op_and_unknown_op_raises():
    # default op recycles the request itself as the attempt
    res = run({"problem": "G", "reached_goal": "H"})
    assert res["ok"] is True
    assert res["n"] == 2
    try:
        run({"op": "nope"})
        assert False, "expected ValueError"
    except ValueError:
        pass
