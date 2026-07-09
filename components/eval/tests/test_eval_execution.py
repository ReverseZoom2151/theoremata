"""Tests for the live eval-execution harness (offline / deterministic).

The compile/run step is behind an injected ``Runner`` seam; here it is always a
deterministic mock. No candidate content is ever executed in-process — the mock
runner is the sole execution boundary.
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from theoremata_tools.benchmarks import make_item  # noqa: E402
from theoremata_tools.eval_execution import (  # noqa: E402
    ERROR,
    FAIL,
    PASS,
    build_artifact,
    execute_track,
    make_outcome,
    outcome_table_runner,
    run,
    table_reviser,
)


# --------------------------------------------------------------------------- #
# fixtures
# --------------------------------------------------------------------------- #

def _proof_item(pid: str) -> dict:
    return make_item(
        id=pid,
        kind="formalization",
        informal=f"informal for {pid}",
        formal=f"theorem {pid} : True := by sorry",
        expected={
            "formal_statement": f"theorem {pid} : True",
            "lean_name": pid,
            "axioms_whitelist": ["propext"],
        },
        grading={"track": "formalization", "method": "comparator_or_statement"},
    )


def _program_item(pid: str, n_oracles: int = 3) -> dict:
    inputs = [{"x": i} for i in range(n_oracles)]
    expected_outputs = [i * 2 for i in range(n_oracles)]
    return make_item(
        id=pid,
        kind="verified_programming",
        informal=f"double the input ({pid})",
        expected={
            "lean_signatures": [f"def {pid} (x : Nat) : Nat"],
            "function_name": pid,
            "arguments": ["x"],
            "oracle_tests": {
                "inputs": inputs,
                "expected_outputs": expected_outputs,
                "bind": "kwargs",
                "arguments": ["x"],
            },
        },
        grading={"track": "verified_programming", "method": "signature_and_oracle"},
    )


# --------------------------------------------------------------------------- #
# 1. proof-compile track: 2 pass / 1 fail -> pass_rate 2/3
# --------------------------------------------------------------------------- #

def test_proof_track_scores_two_of_three():
    examples = [_proof_item("p1"), _proof_item("p2"), _proof_item("p3")]
    candidates = {"p1": "proof-1", "p2": "proof-2", "p3": "proof-3"}

    passing = {"p1", "p2"}

    def runner(artifact):
        # the harness hands us a proof artifact; we (the mock compiler) decide
        assert artifact["kind"] == "proof"
        if artifact["id"] in passing:
            return {"status": "pass", "detail": "compiled"}
        return {"status": "fail", "detail": "compile error"}

    report = execute_track(examples, candidates, runner, track="proof")

    assert report["track"] == "proof"
    assert report["n"] == 3
    assert report["n_pass"] == 2
    assert report["n_fail"] == 1
    assert report["n_error"] == 0
    assert report["pass_rate"] == 2 / 3
    statuses = {it["id"]: it["status"] for it in report["items"]}
    assert statuses == {"p1": PASS, "p2": PASS, "p3": FAIL}
    # order preserved
    assert [it["id"] for it in report["items"]] == ["p1", "p2", "p3"]


# --------------------------------------------------------------------------- #
# 2. program-oracle track: per-oracle results surfaced
# --------------------------------------------------------------------------- #

def test_program_track_reports_per_oracle_results():
    ex = _program_item("prog1", n_oracles=3)
    candidates = {"prog1": "def prog1 (x) := x + x"}

    def runner(artifact):
        # A program artifact carries the oracle tests bound by kwarg name.
        assert artifact["kind"] == "program"
        oracle = artifact["oracle_tests"]
        inputs = oracle["inputs"]
        expected = oracle["expected_outputs"]
        # Deterministic mock "execution": compute 2*x for each named-kwarg input,
        # but make the LAST oracle mismatch so we exercise a per-row failure.
        rows = []
        for i, (inp, exp) in enumerate(zip(inputs, expected)):
            got = inp["x"] * 2
            if i == len(inputs) - 1:
                got = got + 1  # deliberate mismatch on the last oracle
            rows.append({"input": inp, "expected": exp, "got": got, "pass": got == exp})
        ok = all(r["pass"] for r in rows)
        return {
            "status": "pass" if ok else "fail",
            "detail": {"n_oracles": len(rows)},
            "oracle_results": rows,
        }

    report = execute_track(examples=[ex], candidates=candidates, runner=runner, track="program")

    assert report["n"] == 1
    item = report["items"][0]
    assert item["id"] == "prog1"
    assert item["status"] == FAIL  # last oracle mismatched
    assert "oracle_results" in item
    assert len(item["oracle_results"]) == 3
    assert [r["pass"] for r in item["oracle_results"]] == [True, True, False]
    assert report["n_fail"] == 1


def test_program_track_all_oracles_pass():
    ex = _program_item("prog2", n_oracles=4)

    def runner(artifact):
        oracle = artifact["oracle_tests"]
        rows = [
            {"input": inp, "expected": exp, "got": inp["x"] * 2, "pass": inp["x"] * 2 == exp}
            for inp, exp in zip(oracle["inputs"], oracle["expected_outputs"])
        ]
        return {"status": "pass", "detail": "all ok", "oracle_results": rows}

    report = execute_track([ex], {"prog2": "code"}, runner, track="program")
    assert report["n_pass"] == 1
    assert report["pass_rate"] == 1.0
    assert len(report["items"][0]["oracle_results"]) == 4
    assert all(r["pass"] for r in report["items"][0]["oracle_results"])


# --------------------------------------------------------------------------- #
# 3. repair loop: a failed item is re-run once and flips to pass
# --------------------------------------------------------------------------- #

def test_repair_hook_flips_fail_to_pass():
    ex = _proof_item("r1")
    candidates = {"r1": "broken-proof"}

    def runner(artifact):
        # only the repaired candidate compiles
        if artifact["candidate"] == "fixed-proof":
            return {"status": "pass", "detail": "compiled after repair"}
        return {"status": "fail", "detail": "compile error"}

    def reviser(artifact, outcome):
        assert outcome["status"] == "fail"
        return "fixed-proof"

    report = execute_track([ex], candidates, runner, track="proof", reviser=reviser)

    item = report["items"][0]
    assert item["status"] == PASS
    assert item["repaired"] is True
    assert item["attempts"] == 2
    assert report["n_pass"] == 1


def test_repair_hook_none_leaves_failure():
    ex = _proof_item("r2")

    def runner(artifact):
        return {"status": "fail", "detail": "no"}

    def reviser(artifact, outcome):
        return None  # reviser declines to propose a fix

    report = execute_track([ex], {"r2": "x"}, runner, track="proof", reviser=reviser)
    item = report["items"][0]
    assert item["status"] == FAIL
    assert item["repaired"] is False
    assert item["attempts"] == 1


def test_repair_reruns_only_once_even_if_still_failing():
    ex = _proof_item("r3")
    calls = {"n": 0}

    def runner(artifact):
        calls["n"] += 1
        return {"status": "fail", "detail": "still bad"}

    def reviser(artifact, outcome):
        return "still-broken"

    report = execute_track([ex], {"r3": "x"}, runner, track="proof", reviser=reviser)
    assert report["items"][0]["status"] == FAIL
    assert report["items"][0]["attempts"] == 2
    assert calls["n"] == 2  # original + exactly one repair re-run


# --------------------------------------------------------------------------- #
# 4. a raising runner is caught as status "error" (harness never crashes)
# --------------------------------------------------------------------------- #

def test_runner_exception_becomes_error_status():
    examples = [_proof_item("e1"), _proof_item("e2")]

    def runner(artifact):
        if artifact["id"] == "e1":
            raise RuntimeError("compiler segfault")
        return {"status": "pass", "detail": "ok"}

    report = execute_track(examples, {}, runner, track="proof")

    assert report["n_error"] == 1
    assert report["n_pass"] == 1
    by_id = {it["id"]: it for it in report["items"]}
    assert by_id["e1"]["status"] == ERROR
    assert by_id["e1"]["detail"]["reason"] == "runner_raised"
    assert "compiler segfault" in by_id["e1"]["detail"]["exception"]


def test_malformed_runner_return_is_error():
    ex = _proof_item("m1")

    def runner(artifact):
        return "not a dict"

    report = execute_track([ex], {}, runner, track="proof")
    assert report["items"][0]["status"] == ERROR


# --------------------------------------------------------------------------- #
# 5. determinism
# --------------------------------------------------------------------------- #

def test_determinism():
    examples = [_proof_item(f"d{i}") for i in range(5)]
    passing = {"d0", "d2", "d4"}

    def runner(artifact):
        return {"status": "pass" if artifact["id"] in passing else "fail", "detail": ""}

    r1 = execute_track(examples, {}, runner, track="proof")
    r2 = execute_track(examples, {}, runner, track="proof")
    assert r1 == r2
    assert r1["n_pass"] == 3


# --------------------------------------------------------------------------- #
# 6. candidate content is NEVER exec'd in-process (runner seam is the boundary)
# --------------------------------------------------------------------------- #

def test_candidate_is_never_executed_in_process():
    # A candidate that would blow up if it were ever exec/eval'd in-process.
    hostile = "raise AssertionError('candidate code was executed in-process!')"
    ex = _proof_item("safe1")
    seen = {}

    def runner(artifact):
        # The harness passed the raw candidate through as opaque DATA.
        seen["candidate"] = artifact["candidate"]
        return {"status": "pass", "detail": "treated as data"}

    report = execute_track([ex], {"safe1": hostile}, runner, track="proof")

    assert report["items"][0]["status"] == PASS
    # exact string round-trips to the seam, unmodified and unexecuted
    assert seen["candidate"] == hostile


def test_build_artifact_does_not_execute_candidate():
    ex = _program_item("b1")
    hostile = "__import__('os').system('rm -rf /')"
    art = build_artifact(ex, hostile, track="program")
    assert art["candidate"] == hostile
    assert art["kind"] == "program"
    # loader oracle fields are consumed into the artifact
    assert art["oracle_tests"]["arguments"] == ["x"]
    assert art["function_name"] == "b1"


# --------------------------------------------------------------------------- #
# 7. run() op eval_execution via the offline outcome table
# --------------------------------------------------------------------------- #

def test_run_op_with_outcome_table():
    examples = [_proof_item("j1"), _proof_item("j2"), _proof_item("j3")]
    request = {
        "op": "eval_execution",
        "track": "proof",
        "examples": examples,
        "candidates": {"j1": "a", "j2": "b", "j3": "c"},
        "outcomes": {
            "j1": make_outcome(PASS, "ok"),
            "j2": make_outcome(PASS, "ok"),
            "j3": make_outcome(FAIL, "bad"),
        },
    }
    out = run(request)
    assert out["op"] == "eval_execution"
    assert out["n_pass"] == 2
    assert out["n_fail"] == 1
    assert out["pass_rate"] == 2 / 3


def test_run_op_offline_repair_table():
    ex = _proof_item("j4")
    request = {
        "op": "eval_execution",
        "track": "proof",
        "examples": [ex],
        "candidates": {"j4": "orig"},
        "outcomes": {"j4": make_outcome(FAIL, "bad")},
        "repairs": {"j4": "fixed"},
        "repair_outcomes": {"j4": make_outcome(PASS, "fixed ok")},
    }
    out = run(request)
    assert out["items"][0]["status"] == PASS
    assert out["items"][0]["repaired"] is True


def test_run_op_missing_outcome_is_error():
    ex = _proof_item("j5")
    out = run({"op": "eval_execution", "track": "proof", "examples": [ex], "candidates": {}})
    assert out["items"][0]["status"] == ERROR


def test_run_op_rejects_unknown_op():
    import pytest

    with pytest.raises(ValueError):
        run({"op": "not_eval_execution"})


# --------------------------------------------------------------------------- #
# helper-factory sanity
# --------------------------------------------------------------------------- #

def test_outcome_table_runner_and_reviser_factories():
    runner = outcome_table_runner(
        {"x": make_outcome(FAIL, "no")}, {"x": make_outcome(PASS, "yes")}
    )
    assert runner({"id": "x", "attempt": 0})["status"] == FAIL
    assert runner({"id": "x", "attempt": 1})["status"] == PASS
    assert runner({"id": "missing", "attempt": 0})["status"] == ERROR

    reviser = table_reviser({"x": "fix"})
    assert reviser({"id": "x"}, {}) == "fix"
    assert reviser({"id": "y"}, {}) is None


def test_empty_examples_pass_rate_none():
    report = execute_track([], {}, lambda a: make_outcome(PASS), track="proof")
    assert report["n"] == 0
    assert report["pass_rate"] is None
