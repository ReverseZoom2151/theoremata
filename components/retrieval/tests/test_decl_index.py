"""Tests for retrieval Layer B (Lean declaration dump).

Exercises the real Lean toolchain over `Init` (always present, no Mathlib and
no fixture compilation needed), which contains real declarations of every kind
plus the core axioms `propext` / `Quot.sound` / `Classical.choice`. Skips
cleanly if no Lean toolchain is available.
"""
from __future__ import annotations

import os
import shutil

import pytest

from theoremata_tools import decl_index


def _lean_available() -> bool:
    if shutil.which("lean"):
        return True
    for ext in ("", ".exe"):
        if os.path.exists(os.path.expanduser(f"~/.elan/bin/lean{ext}")):
            return True
    return False


pytestmark = pytest.mark.skipif(
    not _lean_available(), reason="Lean toolchain not available (install via elan)"
)


@pytest.fixture(scope="module")
def init_dump():
    result = decl_index.dump(None, ["Init"])
    assert result["ok"], f"dump failed: {result.get('stderr')}"
    return result


def test_dump_has_many_declarations(init_dump):
    assert init_dump["count"] > 1000
    names = {d["name"] for d in init_dump["decls"]}
    assert "Nat.succ_le_succ" in names


def test_record_shape(init_dump):
    d = init_dump["decls"][0]
    assert set(d) >= {"name", "kind", "module", "is_axiom"}
    assert isinstance(d["is_axiom"], bool)


def test_kinds_are_correct(init_dump):
    by_name = {d["name"]: d for d in init_dump["decls"]}
    assert by_name["Nat.succ_le_succ"]["kind"] == "theorem"
    assert by_name["Nat.succ_le_succ"]["module"].startswith("Init")
    # a def and a constructor from the prelude
    assert by_name["And.intro"]["kind"] == "constructor"


def test_axioms_are_flagged(init_dump):
    ax = {d["name"] for d in decl_index.axioms(init_dump["decls"])}
    assert {"propext", "Quot.sound", "Classical.choice"} <= ax
    by_name = {d["name"]: d for d in init_dump["decls"]}
    assert by_name["propext"]["is_axiom"] is True
    assert by_name["propext"]["kind"] == "axiom"


def test_query_helpers(init_dump):
    decls = init_dump["decls"]
    thms = decl_index.by_kind(decls, "theorem")
    assert thms and all(d["kind"] == "theorem" for d in thms)
    hits = decl_index.search(decls, "succ_le")
    assert any("succ_le" in d["name"].lower() for d in hits)


def test_run_stats():
    r = decl_index.run(imports=["Init"], query="stats")
    assert r["ok"] and r["query"] == "stats"
    assert r["axioms"] >= 3
    assert r["kinds"].get("theorem", 0) > 0
    assert r["kinds"].get("axiom", 0) >= 3


def test_fixture_module_roundtrip(tmp_path):
    """Best-effort: compile a tiny fixture module and dump it. Skips (does not
    fail) if importing a freshly-compiled source module isn't wired in this
    environment — the Init-based tests are the authoritative coverage."""
    lean = decl_index._resolve("lean")
    src = tmp_path / "Fix.lean"
    src.write_text(
        "def myDef : Nat := 5\n"
        "theorem myThm : (1 : Nat) = 1 := rfl\n"
        "axiom myAx : Nat\n",
        encoding="utf-8",
    )
    import subprocess

    olean = tmp_path / "Fix.olean"
    try:
        proc = subprocess.run(
            [lean, "-o", str(olean), str(src)],
            capture_output=True,
            encoding="utf-8",
            errors="replace",
            timeout=120,
        )
    except Exception as exc:  # pragma: no cover - environment dependent
        pytest.skip(f"fixture compile unavailable: {exc}")
    if proc.returncode != 0 or not olean.exists():
        pytest.skip(f"fixture compile failed: {proc.stderr.strip()[:200]}")

    env = dict(os.environ)
    env["LEAN_PATH"] = str(tmp_path) + os.pathsep + env.get("LEAN_PATH", "")
    dump_script = str(decl_index._SCRIPT)
    proc = subprocess.run(
        [lean, "--run", dump_script, "Fix"],
        capture_output=True,
        encoding="utf-8",
        errors="replace",
        env=env,
        timeout=120,
    )
    import json

    decls = []
    for line in proc.stdout.splitlines():
        line = line.strip()
        if line:
            try:
                decls.append(json.loads(line))
            except json.JSONDecodeError:
                pass
    by_name = {d["name"]: d for d in decls}
    if not {"myDef", "myThm", "myAx"} <= set(by_name):
        pytest.skip("freshly-compiled fixture module not importable in this environment")
    assert by_name["myDef"]["kind"] == "def"
    assert by_name["myThm"]["kind"] == "theorem"
    assert by_name["myAx"]["kind"] == "axiom"
    assert by_name["myAx"]["is_axiom"] is True
