"""Tests for the Theoremata Lean-workspace scaffolder.

These tests assert file/path/structure only. A full ``lake build`` against
Mathlib is far too slow for CI, so it is never run. An optional, gated
``lake env lean`` smoke test compiles a trivial Mathlib-free module to confirm
the toolchain resolves; it skips cleanly if lake is unavailable or slow.
"""
from __future__ import annotations

import os
import shutil

import pytest

from theoremata_tools import lean_workspace as lw

# Real Mathlib checkout with a full olean cache (only its lean-toolchain and
# directory presence are needed for the fast tests).
MATHLIB_ROOT = os.path.join(
    "C:\\", "Users", "adria", "Downloads", "math-agent",
    "resources", "mathlib4-master", "mathlib4-master",
)


def _fake_mathlib(tmp_path, toolchain="leanprover/lean4:v4.32.0-rc1"):
    """Create a minimal stand-in mathlib dir with just a lean-toolchain."""
    root = tmp_path / "fake_mathlib"
    root.mkdir()
    (root / "lean-toolchain").write_text(toolchain + "\n", encoding="utf-8")
    return root


def _mathlib_root(tmp_path):
    """Prefer the real checkout; fall back to a fake one for the fast tests."""
    if os.path.isdir(MATHLIB_ROOT) and os.path.isfile(
        os.path.join(MATHLIB_ROOT, "lean-toolchain")
    ):
        return MATHLIB_ROOT, _read_toolchain(MATHLIB_ROOT)
    root = _fake_mathlib(tmp_path)
    return str(root), (root / "lean-toolchain").read_text(encoding="utf-8").strip()


def _read_toolchain(root):
    with open(os.path.join(root, "lean-toolchain"), encoding="utf-8") as fh:
        return fh.read().strip()


# ---------------------------------------------------------------------------
# scaffold
# ---------------------------------------------------------------------------


def test_scaffold_writes_expected_files(tmp_path):
    mathlib_root, expected_tc = _mathlib_root(tmp_path)
    target = tmp_path / "ws"

    result = lw.scaffold(str(target), mathlib_root)

    assert result["ok"] is True
    assert result["toolchain"] == expected_tc
    assert result["lib_name"] == "Theoremata"

    lakefile = target / "lakefile.toml"
    toolchain = target / "lean-toolchain"
    root_mod = target / "Theoremata.lean"

    assert lakefile.is_file()
    assert toolchain.is_file()
    assert root_mod.is_file()
    assert (target / "Theoremata").is_dir()

    # Files list points at the real written files.
    for f in (lakefile, toolchain, root_mod):
        assert str(f) in result["files"]


def test_scaffold_toolchain_matches_mathlib(tmp_path):
    mathlib_root, expected_tc = _mathlib_root(tmp_path)
    target = tmp_path / "ws"
    lw.scaffold(str(target), mathlib_root)

    written = (target / "lean-toolchain").read_text(encoding="utf-8").strip()
    assert written == expected_tc


def test_scaffold_lakefile_requires_local_mathlib(tmp_path):
    mathlib_root, _ = _mathlib_root(tmp_path)
    target = tmp_path / "ws"
    lw.scaffold(str(target), mathlib_root)

    content = (target / "lakefile.toml").read_text(encoding="utf-8")
    assert 'name = "mathlib"' in content
    assert "[[require]]" in content
    assert "[[lean_lib]]" in content
    assert 'name = "Theoremata"' in content
    # Absolute path to the mathlib root, forward-slashed.
    assert mathlib_root.replace("\\", "/") in content


def test_scaffold_root_module_minimal(tmp_path):
    mathlib_root, _ = _mathlib_root(tmp_path)
    target = tmp_path / "ws"
    lw.scaffold(str(target), mathlib_root)

    root = (target / "Theoremata.lean").read_text(encoding="utf-8")
    assert "namespace Theoremata" in root
    # Root stays cheap: it must NOT import Mathlib.
    assert "import Mathlib" not in root


def test_scaffold_missing_mathlib_raises(tmp_path):
    with pytest.raises(FileNotFoundError):
        lw.scaffold(str(tmp_path / "ws"), str(tmp_path / "nope"))


# ---------------------------------------------------------------------------
# place_proof
# ---------------------------------------------------------------------------


def test_place_proof_writes_module_and_qualifies(tmp_path):
    mathlib_root, _ = _mathlib_root(tmp_path)
    target = tmp_path / "ws"
    lw.scaffold(str(target), mathlib_root)

    src = "import Mathlib\n\ntheorem my_thm : True := trivial\n"
    res = lw.place_proof(str(target), "MyThm", src)

    assert res["ok"] is True
    assert res["qualified_name"] == "Theoremata.MyThm"
    assert res["module_name"] == "MyThm"

    module_file = target / "Theoremata" / "MyThm.lean"
    assert module_file.is_file()
    assert module_file.read_text(encoding="utf-8") == src
    assert res["module_file"] == str(module_file)


def test_place_proof_strips_dot_lean_suffix(tmp_path):
    mathlib_root, _ = _mathlib_root(tmp_path)
    target = tmp_path / "ws"
    lw.scaffold(str(target), mathlib_root)

    res = lw.place_proof(str(target), "Foo.lean", "theorem t : True := trivial\n")
    assert res["qualified_name"] == "Theoremata.Foo"
    assert (target / "Theoremata" / "Foo.lean").is_file()


def test_place_proof_dotted_name_nests(tmp_path):
    mathlib_root, _ = _mathlib_root(tmp_path)
    target = tmp_path / "ws"
    lw.scaffold(str(target), mathlib_root)

    res = lw.place_proof(str(target), "Proofs.Nested", "theorem t : True := trivial\n")
    assert res["qualified_name"] == "Theoremata.Proofs.Nested"
    assert (target / "Theoremata" / "Proofs" / "Nested.lean").is_file()


def test_place_proof_rejects_bad_name(tmp_path):
    mathlib_root, _ = _mathlib_root(tmp_path)
    target = tmp_path / "ws"
    lw.scaffold(str(target), mathlib_root)

    with pytest.raises(ValueError):
        lw.place_proof(str(target), "../evil", "x")


# ---------------------------------------------------------------------------
# resolve_lake
# ---------------------------------------------------------------------------


def test_resolve_lake_returns_string():
    lake = lw.resolve_lake()
    assert isinstance(lake, str)
    assert lake  # non-empty


# ---------------------------------------------------------------------------
# optional lake smoke test (gated, fast, no Mathlib import)
# ---------------------------------------------------------------------------


@pytest.mark.skipif(
    os.environ.get("THEOREMATA_LAKE_SMOKE") != "1",
    reason="lake smoke test gated behind THEOREMATA_LAKE_SMOKE=1 (slow / env-dependent)",
)
def test_lake_env_lean_trivial_smoke(tmp_path):
    mathlib_root, _ = _mathlib_root(tmp_path)
    if shutil.which(lw.resolve_lake()) is None and lw.resolve_lake() == "lake":
        pytest.skip("lake not available")

    target = tmp_path / "ws"
    lw.scaffold(str(target), mathlib_root)
    # Trivial module WITHOUT importing Mathlib -> fast toolchain check only.
    lw.place_proof(str(target), "Trivial", "theorem t : True := trivial\n")

    result = lw.build(str(target), "Trivial", timeout=120.0)
    if result["timed_out"]:
        pytest.skip("lake env lean timed out in this environment")
    assert result["ok"], result["output"]
