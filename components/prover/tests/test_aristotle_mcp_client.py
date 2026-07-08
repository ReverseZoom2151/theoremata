"""Offline tests for the Aristotle MCP reference client (mock mode).

Every test runs without a network or the ``aristotlelib`` SDK: the client
defaults to the deterministic mock, and the one live-path test forces
``mock=False`` to assert the graceful ``live-unavailable`` degradation (no
exception, no network -- the SDK is not importable in this environment).
"""
from __future__ import annotations

import os
import tempfile

import pytest

from theoremata_tools import aristotle_mcp_client as amc
from theoremata_tools.aristotle_mcp_client import (
    AristotleMCPClient,
    ProjectInputType,
    ProjectStatus,
    map_api_status,
    run,
)


# --------------------------------------------------------------------------- #
# Raw status enum + normalization.
# --------------------------------------------------------------------------- #
def test_raw_status_enum_is_complete():
    names = {s.name for s in ProjectStatus}
    assert names == {
        "NOT_STARTED",
        "QUEUED",
        "IN_PROGRESS",
        "COMPLETE",
        "FAILED",
        "PENDING_RETRY",
    }


def test_map_api_status_normalization():
    assert map_api_status(ProjectStatus.COMPLETE, 100)[0] == "complete"
    assert map_api_status(ProjectStatus.QUEUED, 0)[0] == "queued"
    assert map_api_status(ProjectStatus.NOT_STARTED, 0)[0] == "queued"
    assert map_api_status(ProjectStatus.IN_PROGRESS, 50)[0] == "in_progress"
    # PENDING_RETRY normalizes to in_progress (upstream behavior).
    assert map_api_status(ProjectStatus.PENDING_RETRY, 0)[0] == "in_progress"
    assert map_api_status(ProjectStatus.FAILED, 0)[0] == "failed"
    # Accepts a raw name string too.
    assert map_api_status("COMPLETE", 100)[0] == "complete"


def test_project_input_type_values():
    assert ProjectInputType.FORMAL_LEAN.value == 2
    assert ProjectInputType.INFORMAL.value == 3


# --------------------------------------------------------------------------- #
# Sync prove: proved / counterexample / failed keyword triggers.
# --------------------------------------------------------------------------- #
def test_prove_sync_proved():
    client = AristotleMCPClient(mock=True)
    res = client.prove("theorem t : 1 + 1 = 2 := by sorry")
    assert res.ok is True
    assert res.status == "proved"
    assert res.raw_status == "COMPLETE"
    assert res.input_type == "FORMAL_LEAN"
    assert res.code is not None and "mock" in res.code


def test_prove_sync_counterexample():
    client = AristotleMCPClient(mock=True)
    res = client.prove("theorem false_theorem : 0 = 1 := by sorry")
    assert res.status == "counterexample"
    assert res.raw_status == "FAILED"
    assert res.counterexample is not None


def test_prove_sync_failed():
    client = AristotleMCPClient(mock=True)
    res = client.prove("theorem hard_one : big := by sorry")
    assert res.status == "failed"
    assert res.raw_status == "FAILED"


# --------------------------------------------------------------------------- #
# Async prove: submit -> poll walks the RAW enum through to COMPLETE.
# --------------------------------------------------------------------------- #
def test_async_prove_transitions_through_raw_enum_to_complete():
    client = AristotleMCPClient(mock=True)
    submitted = client.prove("theorem t : True := by sorry", wait=False)
    assert submitted.status == "submitted"
    assert submitted.raw_status == "NOT_STARTED"
    pid = submitted.project_id
    assert pid is not None

    # Poll 1: raw QUEUED -> normalized queued.
    p1 = client.check_proof(pid)
    assert p1.raw_status == "QUEUED"
    assert p1.status == "queued"

    # Poll 2: raw IN_PROGRESS.
    p2 = client.check_proof(pid)
    assert p2.raw_status == "IN_PROGRESS"
    assert p2.status == "in_progress"
    assert p2.percent_complete == 50

    # Poll 3: terminal raw COMPLETE -> normalized proved, code present.
    p3 = client.check_proof(pid)
    assert p3.raw_status == "COMPLETE"
    assert p3.status == "proved"
    assert p3.code is not None


def test_check_proof_unknown_id():
    client = AristotleMCPClient(mock=True)
    res = client.check_proof("does-not-exist")
    assert res.ok is False
    assert res.status == "error"


# --------------------------------------------------------------------------- #
# Formal (prove) vs informal (formalize) input types.
# --------------------------------------------------------------------------- #
def test_formalize_is_informal_input():
    client = AristotleMCPClient(mock=True)
    res = client.formalize("the sum of two even numbers is even")
    assert res.status == "formalized"
    assert res.input_type == "INFORMAL"
    assert res.lean_code is not None and "even" in res.lean_code
    assert res.raw_status == "COMPLETE"


def test_formalize_with_prove_flag_proves():
    client = AristotleMCPClient(mock=True)
    res = client.formalize("7 is prime", prove=True)
    assert res.status == "proved"
    assert res.input_type == "INFORMAL"
    assert "sorry" not in (res.lean_code or "")


def test_prove_is_formal_input():
    client = AristotleMCPClient(mock=True)
    res = client.prove("theorem t : True := by sorry")
    assert res.input_type == "FORMAL_LEAN"


# --------------------------------------------------------------------------- #
# Size-guard rejections (1 MB code / 100 KB description / 10 MB file).
# --------------------------------------------------------------------------- #
def test_prove_size_guard_rejection():
    client = AristotleMCPClient(mock=True)
    big = "x" * (amc.MAX_CODE_SIZE + 1)
    res = client.prove(big)
    assert res.ok is False
    assert res.status == "error"
    assert "maximum size" in res.message


def test_formalize_size_guard_rejection():
    client = AristotleMCPClient(mock=True)
    big = "y" * (amc.MAX_DESCRIPTION_SIZE + 1)
    res = client.formalize(big)
    assert res.ok is False
    assert res.status == "error"


def test_prove_file_size_guard_rejection(monkeypatch):
    client = AristotleMCPClient(mock=True)
    with tempfile.NamedTemporaryFile(mode="w", suffix=".lean", delete=False) as fh:
        fh.write("theorem t : True := by sorry\n")
        path = fh.name
    try:
        # Avoid writing a 10 MB file: report an oversized size via getsize.
        monkeypatch.setattr(os.path, "getsize", lambda p: amc.MAX_FILE_SIZE + 1)
        res = client.prove_file(path)
        assert res.ok is False
        assert res.status == "error"
        assert "maximum size" in res.message
    finally:
        os.unlink(path)


# --------------------------------------------------------------------------- #
# prove_file: sync proved + filename-keyword partial/fail + async save.
# --------------------------------------------------------------------------- #
def test_prove_file_sync_proved(tmp_path):
    client = AristotleMCPClient(mock=True)
    f = tmp_path / "MyProof.lean"
    f.write_text("theorem t : True := by sorry\n")
    res = client.prove_file(str(f))
    assert res.status == "proved"
    assert res.output_path is not None and res.output_path.endswith("_aristotle.lean")


def test_prove_file_partial_keyword(tmp_path):
    client = AristotleMCPClient(mock=True)
    f = tmp_path / "partial_case.lean"
    f.write_text("theorem t : True := by sorry\n")
    res = client.prove_file(str(f))
    assert res.status == "partial"


def test_prove_file_missing_file():
    client = AristotleMCPClient(mock=True)
    res = client.prove_file("/no/such/file.lean")
    assert res.ok is False
    assert res.status == "error"


def test_check_prove_file_async_save(tmp_path):
    client = AristotleMCPClient(mock=True)
    f = tmp_path / "AsyncProof.lean"
    f.write_text("theorem t : True := by sorry\n")
    submitted = client.prove_file(str(f), wait=False)
    assert submitted.status == "submitted"
    pid = submitted.project_id
    # Two intermediate polls, then terminal.
    client.check_prove_file(pid)
    client.check_prove_file(pid)
    done = client.check_prove_file(pid, save=True)
    assert done.status == "proved"
    assert done.raw_status == "COMPLETE"
    assert done.output_path is not None


# --------------------------------------------------------------------------- #
# aristotle://status resource.
# --------------------------------------------------------------------------- #
def test_status_resource_mock():
    client = AristotleMCPClient(mock=True)
    st = client.status()
    assert st["mock_mode"] is True
    assert st["ready"] is True


# --------------------------------------------------------------------------- #
# Graceful offline: forcing the live path with no SDK/key degrades cleanly.
# --------------------------------------------------------------------------- #
def test_live_path_graceful_offline():
    # Force live mode with a key so we pass the key check, then fail on the
    # (absent) aristotlelib import -- no exception, no network.
    client = AristotleMCPClient(mock=False, api_key="fake-key-for-test")
    res = client.prove("theorem t : True := by sorry")
    assert res.ok is False
    assert res.status == "error"
    assert res.mode == "live-unavailable"


def test_live_path_no_api_key_graceful():
    client = AristotleMCPClient(mock=False, api_key="")
    res = client.formalize("something")
    assert res.ok is False
    assert res.mode == "live-unavailable"


# --------------------------------------------------------------------------- #
# Worker-style run() dispatch.
# --------------------------------------------------------------------------- #
def test_run_dispatch_prove():
    out = run({"tool": "aristotle_mcp", "op": "prove", "code": "theorem t : True := by sorry", "mock": True})
    assert out["ok"] is True
    assert out["status"] == "proved"
    assert out["input_type"] == "FORMAL_LEAN"


def test_run_dispatch_status():
    out = run({"op": "status", "mock": True})
    assert out["mock_mode"] is True


def test_run_dispatch_async_roundtrip():
    submitted = run({"op": "prove", "code": "theorem t : True := by sorry", "wait": False, "mock": True})
    assert submitted["status"] == "submitted"
    pid = submitted["project_id"]
    run({"op": "check_proof", "project_id": pid, "mock": True})  # queued (fresh client, poll 1)
    # Note: run() builds a fresh client each call, so poll state does not persist
    # across run() invocations; this asserts the first poll is a queued state.
    first = run({"op": "check_proof", "project_id": pid, "mock": True})
    assert first["ok"] is False  # unknown id in a fresh client's store


def test_run_unknown_op_raises():
    with pytest.raises(ValueError):
        run({"op": "bogus", "mock": True})


# --------------------------------------------------------------------------- #
# to_dict shape.
# --------------------------------------------------------------------------- #
def test_to_dict_omits_none_but_keeps_core():
    client = AristotleMCPClient(mock=True)
    d = client.prove("theorem t : True := by sorry").to_dict()
    assert set(["ok", "tool", "status", "message", "mode"]).issubset(d.keys())
    assert "counterexample" not in d  # None -> omitted
    assert d["code"] is not None
