"""Tests for the bulk `collectAxioms` meta-program path.

The splicing, parsing and fail-closed plumbing are pure Python and run with no
Lean at all, so CI still exercises something real. The tests that need a kernel
skip honestly when no toolchain is on PATH.
"""
from __future__ import annotations

import json
import os

import pytest

from theoremata_tools.axioms import (
    AUDIT_LEAN_PATH,
    SPLICE_END_MARKER,
    build_bulk_source,
    check_axioms,
    check_axioms_bulk,
    load_audit_prelude,
    parse_audit_output,
    split_imports,
    _resolve,
)

_lean = _resolve("lean") or _resolve("lake")
requires_lean = pytest.mark.skipif(_lean is None, reason="Lean toolchain not available")


# --------------------------------------------------------------------------
# Pure Python: no Lean required.
# --------------------------------------------------------------------------

def test_audit_lean_file_exists():
    assert os.path.isfile(AUDIT_LEAN_PATH)


def test_prelude_is_spliceable_and_omits_the_executable():
    prelude = load_audit_prelude()
    assert "import Lean" in prelude
    assert "#audit_axioms" in prelude
    assert SPLICE_END_MARKER not in prelude
    # `main` lives below the marker; splicing it would collide with a generated
    # file that defines its own.
    assert "def main" not in prelude


def test_attribution_header_travels():
    with open(AUDIT_LEAN_PATH, encoding="utf-8") as fh:
        head = fh.read(4000)
    for required in (
        "Copyright (c) 2026 The Tau Ceti contributors",
        "Apache",
        "importGraph",
        "Kim Morrison",
        "Paul Lezeau",
        "Robin Arnez",
        "MODIFIED PORT",
    ):
        assert required in head, required


def test_issue_8840_note_is_kept():
    with open(AUDIT_LEAN_PATH, encoding="utf-8") as fh:
        assert "8840" in fh.read()


def test_no_angle_bracket_tags_in_the_lean_file():
    """House rule: no XML-ish tags in code or comments. Lean operators such as
    the functor map are not tags, so match the tag shape rather than the
    characters."""
    import re as _re

    with open(AUDIT_LEAN_PATH, encoding="utf-8") as fh:
        text = fh.read()
    assert not _re.search(r"</?[A-Za-z][A-Za-z0-9_-]*\s*/?>", text)


def test_split_imports_basic():
    imports, body = split_imports("import Mathlib.Tactic\nimport Foo\n\ntheorem t : True := trivial")
    assert imports == ["import Mathlib.Tactic", "import Foo"]
    assert "theorem t" in body
    assert "import" not in body


def test_split_imports_ignores_imports_inside_block_comments():
    src = "/- import NotReal\n-/\nimport Real.One\ntheorem t : True := trivial"
    imports, body = split_imports(src)
    assert imports == ["import Real.One"]
    assert "NotReal" in body


def test_split_imports_stops_after_first_command():
    src = "import A\ntheorem t : True := trivial\nimport B"
    imports, body = split_imports(src)
    assert imports == ["import A"]
    assert "import B" in body


def test_build_bulk_source_hoists_and_dedupes_imports():
    out = build_bulk_source(
        "import Lean\nimport Mathlib.Tactic\ntheorem t : True := trivial",
        ["t"],
        "import Lean\n-- prelude body\n",
    )
    lines = out.splitlines()
    assert lines[0] == "import Lean"
    assert lines[1] == "import Mathlib.Tactic"
    assert lines.count("import Lean") == 1
    assert out.rstrip().endswith("#audit_axioms t")


def test_build_bulk_source_rejects_non_identifiers():
    for bad in ["t; #eval 1", "", "foo bar", "1abc"]:
        with pytest.raises(ValueError):
            build_bulk_source("theorem t : True := trivial", [bad], "import Lean\n")


def test_parse_audit_output_happy():
    text = (
        'THEOREMATA_AXIOM_AUDIT {"decl":"a","axioms":[]}\n'
        'THEOREMATA_AXIOM_AUDIT {"decl":"b","axioms":["propext"]}\n'
        'THEOREMATA_AXIOM_AUDIT_SUMMARY {"audited":2}\n'
    )
    assert parse_audit_output(text) == {"a": [], "b": ["propext"]}


def test_parse_audit_output_rejects_missing_summary():
    assert parse_audit_output('THEOREMATA_AXIOM_AUDIT {"decl":"a","axioms":[]}') is None


def test_parse_audit_output_rejects_zero_declaration_guard():
    assert parse_audit_output("THEOREMATA_AXIOM_AUDIT_ERROR audited 0 declarations") is None


def test_parse_audit_output_rejects_count_mismatch():
    text = (
        'THEOREMATA_AXIOM_AUDIT {"decl":"a","axioms":[]}\n'
        'THEOREMATA_AXIOM_AUDIT_SUMMARY {"audited":5}\n'
    )
    assert parse_audit_output(text) is None


def test_parse_audit_output_rejects_garbage_payload():
    text = (
        "THEOREMATA_AXIOM_AUDIT not-json\n"
        'THEOREMATA_AXIOM_AUDIT_SUMMARY {"audited":1}\n'
    )
    assert parse_audit_output(text) is None


def test_parse_audit_output_ignores_unrelated_chatter():
    text = (
        "warning: declaration uses `sorry`\n"
        'THEOREMATA_AXIOM_AUDIT {"decl":"a","axioms":["sorryAx"]}\n'
        'THEOREMATA_AXIOM_AUDIT_SUMMARY {"audited":1}\n'
    )
    assert parse_audit_output(text) == {"a": ["sorryAx"]}


def test_bulk_with_no_theorems_is_not_clean():
    # The Python twin of the Lean zero-declaration guard. No Lean is invoked.
    out = check_axioms_bulk("theorem t : True := trivial", [])
    assert out["ok"] is False
    assert out["clean"] is False
    assert out["audited"] == 0


def test_bulk_fails_closed_when_no_toolchain(tmp_path):
    out = check_axioms_bulk(
        "theorem t : True := trivial",
        ["t"],
        lean_bin=str(tmp_path / "definitely-not-a-lean-binary"),
    )
    assert out["ok"] is False
    assert out["clean"] is False
    assert out["results"]["t"]["clean"] is False
    assert out["results"]["t"]["compiled"] is False


def test_bulk_fails_closed_on_unsplittable_prelude(tmp_path):
    bad = tmp_path / "bad.lean"
    bad.write_text("import Lean\n", encoding="utf-8")
    out = check_axioms_bulk(
        "theorem t : True := trivial", ["t"], prelude_path=str(bad)
    )
    assert out["ok"] is False
    assert out["clean"] is False
    assert SPLICE_END_MARKER in out["stderr"]


# --------------------------------------------------------------------------
# Real toolchain.
# --------------------------------------------------------------------------

FIXTURE = """theorem bulkClean : True := trivial
axiom bulkBad : False
theorem bulkUsesBad : False := bulkBad
theorem bulkClassical (p : Prop) : p ∨ ¬ p := Classical.em p
axiom bulkSecret : Nat
axiom bulkSecretEq : bulkSecret = 1
theorem bulkUsesSecret : bulkSecret = 1 := bulkSecretEq
theorem bulkSorried : 1 = 1 := by sorry
"""

FIXTURE_NAMES = [
    "bulkClean",
    "bulkUsesBad",
    "bulkClassical",
    "bulkUsesSecret",
    "bulkSorried",
]


@pytest.fixture(scope="module")
def bulk_result():
    return check_axioms_bulk(FIXTURE, FIXTURE_NAMES, timeout=600.0)


@requires_lean
def test_bulk_runs_and_audits_everything(bulk_result):
    assert bulk_result["ok"] is True, bulk_result["stderr"]
    assert bulk_result["audited"] == len(FIXTURE_NAMES)


@requires_lean
def test_bulk_clean_theorem_is_axiom_free(bulk_result):
    r = bulk_result["results"]["bulkClean"]
    assert r["axioms"] == []
    assert r["clean"] is True


@requires_lean
def test_bulk_reports_the_classical_axioms(bulk_result):
    r = bulk_result["results"]["bulkClassical"]
    assert set(r["axioms"]) == {"propext", "Classical.choice", "Quot.sound"}
    assert r["clean"] is True


@requires_lean
def test_bulk_flags_custom_axiom_and_sorry(bulk_result):
    bad = bulk_result["results"]["bulkUsesBad"]
    assert bad["axioms"] == ["bulkBad"]
    assert bad["disallowed"] == ["bulkBad"]
    assert bad["clean"] is False
    sorried = bulk_result["results"]["bulkSorried"]
    assert "sorryAx" in sorried["disallowed"]
    assert sorried["clean"] is False
    assert bulk_result["clean"] is False


@requires_lean
def test_bulk_agrees_with_print_axioms_path(bulk_result):
    """Cross-validation: the meta-program and `#print axioms` must agree on the
    axiom closure for every fixture theorem. A disagreement is a real finding,
    not something to normalize away, so the sets are compared exactly."""
    assert bulk_result["ok"] is True, bulk_result["stderr"]
    disagreements = []
    for name in FIXTURE_NAMES:
        single = check_axioms(FIXTURE, name, timeout=600.0)
        assert single["ok"] is True, single["stderr"]
        mine = sorted(bulk_result["results"][name]["axioms"])
        theirs = sorted(single["axioms"])
        if mine != theirs:
            disagreements.append((name, mine, theirs))
        assert bulk_result["results"][name]["clean"] == single["clean"], name
    assert not disagreements, disagreements


@requires_lean
def test_bulk_fails_closed_on_unknown_theorem_name():
    out = check_axioms_bulk(
        "theorem realOne : True := trivial", ["noSuchTheorem"], timeout=600.0
    )
    assert out["ok"] is False
    assert out["clean"] is False
    assert out["results"]["noSuchTheorem"]["clean"] is False


@requires_lean
def test_lean_zero_declaration_guard_fires(tmp_path):
    """The guard is the fail-open defence: `#audit_axioms` with no targets must
    be a hard error, not a silent clean run."""
    import subprocess

    src = load_audit_prelude() + "\ntheorem guardVictim : True := trivial\n#audit_axioms\n"
    path = tmp_path / "zero.lean"
    path.write_text(src, encoding="utf-8")
    lean = _resolve("lean")
    if lean is None:
        pytest.skip("lean binary not available")
    proc = subprocess.run(
        [lean, str(path)], capture_output=True, encoding="utf-8", errors="replace"
    )
    combined = f"{proc.stdout}\n{proc.stderr}"
    assert proc.returncode != 0
    assert "THEOREMATA_AXIOM_AUDIT_ERROR" in combined
    assert "audited 0 declarations" in combined
    assert parse_audit_output(combined) is None


@requires_lean
def test_cli_bulk_request_round_trips(tmp_path):
    import subprocess
    import sys as _sys

    req = tmp_path / "req.json"
    req.write_text(
        json.dumps({"source": FIXTURE, "theorems": ["bulkClean"], "timeout": 600.0}),
        encoding="utf-8",
    )
    # The subprocess does not inherit conftest's sys.path surgery.
    # `theoremata_tools` is a namespace package, so derive the search path from
    # a concrete submodule rather than the package's (absent) __file__.
    from theoremata_tools import axioms as _axioms_mod

    env = dict(os.environ)
    pkg_parent = os.path.dirname(os.path.dirname(os.path.abspath(_axioms_mod.__file__)))
    env["PYTHONPATH"] = pkg_parent + os.pathsep + env.get("PYTHONPATH", "")
    proc = subprocess.run(
        [_sys.executable, "-m", "theoremata_tools.axioms", str(req)],
        capture_output=True,
        encoding="utf-8",
        errors="replace",
        env=env,
    )
    assert proc.returncode == 0, proc.stderr
    payload = json.loads(proc.stdout.strip().splitlines()[-1])
    assert payload["ok"] is True
    assert payload["results"]["bulkClean"]["clean"] is True
    assert proc.returncode == 0
