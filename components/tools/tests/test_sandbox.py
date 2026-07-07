"""Tests for the hardened falsifier sandbox (DeepMath-derived robustness).

Covers the four required behaviours:
  * an infinite/runaway snippet is hard-killed within the timeout;
  * a disallowed import returns the self-correcting hint;
  * a legitimate counterexample search still works (through the subprocess);
  * budget exhaustion terminates gracefully.
"""
from __future__ import annotations

import time

import pytest

from theoremata_tools import sandbox
from theoremata_tools.falsify import search
from theoremata_tools.safe_eval import compile_expression, evaluate, evaluate_hardened
from theoremata_tools.sandbox import (
    ImportNotAllowedError,
    StepBudget,
    guard_imports,
    run_in_subprocess,
)


# --- module-level, picklable targets (spawn requires importable callables) ---


def _busy_forever() -> None:
    while True:  # pragma: no cover - killed before it can finish
        pass


def _sleep_forever() -> None:
    time.sleep(3600)  # pragma: no cover - killed before it can finish


def _add(a: int, b: int) -> int:
    return a + b


# --- 1. hard-kill timeout --------------------------------------------------


def test_infinite_loop_is_hard_killed_within_timeout():
    start = time.monotonic()
    result = run_in_subprocess(_busy_forever, timeout=1.0)
    elapsed = time.monotonic() - start
    assert result.timed_out
    assert result.status == sandbox.STATUS_TIMEOUT
    # Killed promptly, not hung: 1s timeout + kill/tree-kill slack.
    assert elapsed < 12.0


def test_sleeping_snippet_is_killed_not_awaited():
    start = time.monotonic()
    result = run_in_subprocess(_sleep_forever, timeout=1.0)
    assert result.timed_out
    # A 3600s sleep must be force-killed, never waited out.
    assert time.monotonic() - start < 12.0


def test_subprocess_runs_legit_callable():
    result = run_in_subprocess(_add, args=(2, 3), timeout=10.0)
    assert result.ok
    assert result.value == 5


def test_falsify_runaway_claim_is_killed_and_reported():
    # A single-case domain, but the claim itself is effectively infinite. The
    # hard-kill sandbox must terminate it and report inconclusive, not hang.
    result = search(
        {"x": {"start": 0, "stop": 1}},
        claim="sum(1 for _ in range(10**15)) >= 0",
        timeout_seconds=1.0,
    )
    assert result["verdict"] == "inconclusive"
    assert "timed out" in result["reason"]


# --- 2. import allow-list --------------------------------------------------


def test_guard_imports_blocks_dynamic_import_with_hint():
    with pytest.raises(ImportNotAllowedError) as exc:
        guard_imports("__import__('os').system('true')")
    assert "Import not allowed: os" in str(exc.value)


def test_guard_imports_blocks_import_statement():
    with pytest.raises(ImportNotAllowedError) as exc:
        guard_imports("import socket")
    assert "Import not allowed: socket" in str(exc.value)


def test_guard_imports_allows_math_libs():
    # Allowed modules do not raise.
    guard_imports("import math")
    guard_imports("from itertools import product")


def test_compile_expression_surfaces_import_hint():
    with pytest.raises(ImportNotAllowedError) as exc:
        compile_expression("__import__('os')")
    assert "Import not allowed: os" in str(exc.value)


def test_evaluate_hardened_reports_import_error():
    result = evaluate_hardened("__import__('os')")
    assert result.status == sandbox.STATUS_IMPORT_ERROR
    assert "Import not allowed: os" in (result.error or "")
    with pytest.raises(ImportNotAllowedError):
        result.unwrap()


# --- 3. legit counterexample search still works ----------------------------


def test_counterexample_search_still_works_through_sandbox():
    result = search({"n": {"start": -5, "stop": 6}}, claim="n * n < 10")
    assert result["verdict"] == "counterexample"
    assert result["assignment"]["n"] * result["assignment"]["n"] >= 10


def test_no_counterexample_search_still_works_through_sandbox():
    result = search(
        {"n": {"start": -50, "stop": 51}},
        assumptions="n % 2 == 0",
        claim="(n * n) % 2 == 0",
    )
    assert result["verdict"] == "no_counterexample_in_domain"


def test_evaluate_hardened_returns_value():
    assert evaluate_hardened("sum(k*k for k in range(5))").unwrap() == 30
    # The in-process evaluate contract is untouched.
    assert evaluate("sum(k*k for k in range(5))") == 30


# --- 4. budget governor ----------------------------------------------------


def test_step_budget_spends_then_refuses():
    budget = StepBudget(total=2)
    assert budget.spend(1)
    assert budget.spend(1)
    assert not budget.spend(1)  # exhausted; refuses without over-spending
    assert budget.exhausted
    assert budget.remaining == 0


def test_budget_exhaustion_terminates_gracefully():
    # Domain has 200 cases but the total budget is tiny: must stop gracefully.
    result = search(
        {"n": {"start": 0, "stop": 200}},
        claim="n < 10",  # would be a counterexample at n=10, but budget stops first
        budget=5,
    )
    assert result["verdict"] == "inconclusive"
    assert result["reason"] == "step budget exhausted"
    assert result["checked"] == 5
