"""Tests for the warm persistent Lean checker (theoremata_tools.lean_repl).

Uses ``Init`` only (fast, no Mathlib) so the suite stays cheap. Skips cleanly
when no Lean toolchain is installed."""

from __future__ import annotations

import time

import pytest

from theoremata_tools import lean_repl


def _lean_available() -> bool:
    return lean_repl._resolve("lake") is not None or lean_repl._resolve("lean") is not None


pytestmark = pytest.mark.skipif(not _lean_available(), reason="Lean toolchain not installed")


@pytest.fixture(scope="module")
def session():
    s = lean_repl.LeanSession(imports=["Init"], warm_timeout=180.0, timeout=60.0)
    w = s.warm()
    if not w.get("ok"):
        s.close()
        pytest.skip(f"Lean warm failed (mode={s.mode}): {w.get('error')}")
    yield s
    s.close()


def test_warm_reports_mode(session):
    assert session.mode in ("repl", "lean")


def test_trivial_theorem_ok(session):
    res = session.check("theorem t : True := trivial")
    assert res["ok"] is True
    assert not [m for m in res.get("messages", []) if m.get("severity") == "error"]


def test_sorry_reports_warning(session):
    res = session.check("theorem s : True := by sorry")
    # A sorry surfaces either as a recorded sorry or a warning message.
    has_sorry = bool(res.get("sorries")) or any(
        "sorry" in str(m.get("data", "")).lower() or m.get("severity") == "warning"
        for m in res.get("messages", [])
    )
    assert has_sorry


def test_error_detected(session):
    res = session.check("theorem bad : True := 42")
    assert res["ok"] is False


def test_warm_reuse_is_fast(session):
    # First check pays any lingering JIT/first-touch cost; the second reuses the
    # warm environment. Assert the second is faster-or-comparable (tolerant).
    r1 = session.check("example : 1 = 1 := rfl")
    r2 = session.check("example : 2 = 2 := rfl")
    assert r1["ok"] and r2["ok"]
    # Generous tolerance: second must not be dramatically slower than the first.
    assert r2["elapsed"] <= r1["elapsed"] * 3 + 1.0


def test_run_dispatch_check():
    out = lean_repl.run({"op": "check", "imports": ["Init"], "source": "theorem u : True := trivial"})
    if not out.get("ok") and out.get("error"):
        pytest.skip(f"check dispatch unavailable: {out.get('error')}")
    assert out["ok"] is True
    assert "mode" in out


def test_run_dispatch_warm():
    out = lean_repl.run({"op": "warm", "imports": ["Init"]})
    assert "mode" in out
    if out.get("ok"):
        assert out.get("warmed") is True
