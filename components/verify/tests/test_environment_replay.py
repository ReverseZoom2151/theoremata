"""Tests for the anti-tamper environment replay.

Two halves, deliberately separated:

  * The pure-Python parsing half runs everywhere, including CI with no Lean. It
    is where the fail-closed contract is pinned down, because every one of those
    branches is reachable without a toolchain.
  * The live half compiles real fixtures with the real Lean toolchain and
    replays them. It skips honestly when Lean is absent. A skip is not a pass,
    and the parsing half is what keeps the module from being vacuous in CI.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess

import pytest

from theoremata_tools.axioms import _resolve
from theoremata_tools.environment_replay import (
    ERROR_MARKER,
    REPLAY_LEAN_PATH,
    RESULT_MARKER,
    SUMMARY_MARKER,
    parse_replay_output,
    replay_olean,
)

FIXTURES = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "fixtures",
    "environment_replay",
)

_lean = _resolve("lean")
requires_lean = pytest.mark.skipif(_lean is None, reason="Lean toolchain not available")


def _result_line(**kwargs) -> str:
    payload = {
        "olean": "Honest.olean",
        "imports": ["Init"],
        "total_constants": 2,
        "replayed": 2,
        "skipped": [],
    }
    payload.update(kwargs)
    return RESULT_MARKER + " " + json.dumps(payload)


def _summary_line(replayed: int = 2) -> str:
    return SUMMARY_MARKER + " " + json.dumps({"replayed": replayed})


# ---------------------------------------------------------------------------
# Pure parsing. No Lean.
# ---------------------------------------------------------------------------


def test_parse_well_formed_pair():
    parsed = parse_replay_output(_result_line() + "\n" + _summary_line())
    assert parsed is not None
    assert parsed["replayed"] == 2
    assert parsed["imports"] == ["Init"]
    assert parsed["skipped"] == []


def test_parse_ignores_unrelated_chatter():
    text = "\n".join(
        ["info: building", _result_line(), "some trailing note", _summary_line()]
    )
    assert parse_replay_output(text) is not None


def test_parse_rejects_error_marker_even_with_a_good_pair():
    # The error marker is unconditional. If the Lean side said anything went
    # wrong, no amount of well-formed output alongside it is a pass.
    text = "\n".join([_result_line(), _summary_line(), ERROR_MARKER + " boom"])
    assert parse_replay_output(text) is None


def test_parse_rejects_missing_summary():
    # No summary line means the run did not reach the end.
    assert parse_replay_output(_result_line()) is None


def test_parse_rejects_missing_result():
    assert parse_replay_output(_summary_line()) is None


def test_parse_rejects_count_disagreement():
    text = _result_line(replayed=2) + "\n" + _summary_line(3)
    assert parse_replay_output(text) is None


def test_parse_rejects_zero_declarations():
    # The Python twin of the Lean zero-declaration guard: an empty replay is the
    # fail-open shape this whole module exists to refuse.
    text = _result_line(replayed=0) + "\n" + _summary_line(0)
    assert parse_replay_output(text) is None


def test_parse_rejects_two_result_lines():
    text = "\n".join([_result_line(), _result_line(olean="Other.olean"), _summary_line()])
    assert parse_replay_output(text) is None


def test_parse_rejects_unparseable_payload():
    assert parse_replay_output(RESULT_MARKER + " not json\n" + _summary_line()) is None


def test_parse_rejects_missing_field():
    bad = RESULT_MARKER + " " + json.dumps({"olean": "x", "replayed": 1})
    assert parse_replay_output(bad + "\n" + _summary_line(1)) is None


def test_parse_rejects_empty_output():
    assert parse_replay_output("") is None


def test_parse_surfaces_skipped_names():
    parsed = parse_replay_output(
        _result_line(skipped=["Foo.unsafeThing"]) + "\n" + _summary_line()
    )
    assert parsed is not None
    assert parsed["skipped"] == ["Foo.unsafeThing"]


# ---------------------------------------------------------------------------
# Fail-closed wrapper behaviour reachable without Lean.
# ---------------------------------------------------------------------------


def test_missing_olean_is_not_clean(tmp_path):
    res = replay_olean(str(tmp_path / "nope.olean"))
    assert res["ok"] is False
    assert res["clean"] is False
    assert "does not exist" in res["error"]


def test_missing_meta_program_is_not_clean(tmp_path):
    target = tmp_path / "x.olean"
    target.write_bytes(b"")
    res = replay_olean(str(target), replay_lean_path=str(tmp_path / "absent.lean"))
    assert res["ok"] is False
    assert res["clean"] is False


def test_absent_toolchain_is_not_clean(tmp_path, monkeypatch):
    target = tmp_path / "x.olean"
    target.write_bytes(b"")
    monkeypatch.setattr(
        "theoremata_tools.environment_replay._resolve", lambda *a, **k: None
    )
    res = replay_olean(str(target))
    assert res["ok"] is False
    assert res["clean"] is False
    assert "no Lean toolchain" in res["error"]


def test_meta_program_ships_the_markers_python_expects():
    # A rename on either side would otherwise turn every live replay into an
    # unparseable run, which is fail-closed but silently useless.
    with open(REPLAY_LEAN_PATH, encoding="utf-8") as fh:
        text = fh.read()
    assert RESULT_MARKER in text
    assert SUMMARY_MARKER in text
    assert ERROR_MARKER in text


# ---------------------------------------------------------------------------
# Live Lean.
# ---------------------------------------------------------------------------


def _build(workdir: str, module: str, source_name: str | None = None) -> str:
    """Compile one fixture into `workdir` and return its `.olean` path."""
    src = os.path.join(workdir, module + ".lean")
    if source_name:
        shutil.copyfile(os.path.join(FIXTURES, source_name), src)
    olean = os.path.join(workdir, module + ".olean")
    env = dict(os.environ)
    # The drift client imports a sibling fixture, so the work directory itself
    # has to be on the search path at BUILD time as well as at replay time.
    env["LEAN_PATH"] = workdir + os.pathsep + env.get("LEAN_PATH", "")
    proc = subprocess.run(
        [_lean, "-o", olean, src],
        cwd=workdir,
        env=env,
        capture_output=True,
        encoding="utf-8",
        errors="replace",
    )
    assert proc.returncode == 0, f"fixture {module} failed to build:\n{proc.stdout}\n{proc.stderr}"
    return olean


@pytest.fixture(scope="module")
def workdir(tmp_path_factory):
    return str(tmp_path_factory.mktemp("environment_replay"))


@requires_lean
def test_honest_module_replays_clean(workdir):
    olean = _build(workdir, "Honest", "Honest.lean")
    res = replay_olean(olean)
    assert res["ok"] is True, res
    assert res["clean"] is True, res
    assert res["replayed"] == 2
    assert res["skipped"] == []


@requires_lean
def test_unchecked_injection_is_caught(workdir):
    # The headline case: an olean asserting `False` that the kernel never saw.
    # Its axiom closure is empty, so an axiom audit calls it clean; only the
    # replay catches it.
    olean = _build(workdir, "TamperUnchecked", "TamperUnchecked.lean")
    res = replay_olean(olean)
    assert res["clean"] is False, res
    assert "everythingIsFalse" in res["error"], res
    assert "kernel" in res["error"], res


@requires_lean
def test_axiom_audit_is_blind_to_this_tamper(workdir):
    """The reason this layer had to exist, demonstrated rather than argued.

    A downstream module derives `False` from the tampered import, compiles with
    exit code 0, and `#print axioms` reports no axioms for either name. The
    whole existing axiom-based defence sees nothing. Replay is the only layer
    that catches it, which is what the previous test shows.
    """
    _build(workdir, "TamperUnchecked", "TamperUnchecked.lean")
    consumer = os.path.join(workdir, "Consumer.lean")
    with open(consumer, "w", encoding="utf-8") as fh:
        fh.write(
            "import TamperUnchecked\n"
            "theorem exploit : False := Theoremata.Fixture.everythingIsFalse\n"
            "#print axioms exploit\n"
        )
    env = dict(os.environ)
    env["LEAN_PATH"] = workdir + os.pathsep + env.get("LEAN_PATH", "")
    proc = subprocess.run(
        [_lean, consumer],
        cwd=workdir,
        env=env,
        capture_output=True,
        encoding="utf-8",
        errors="replace",
    )
    assert proc.returncode == 0, proc.stderr
    assert "does not depend on any axioms" in proc.stdout, proc.stdout


@requires_lean
def test_zero_declaration_guard_fires(workdir):
    olean = _build(workdir, "Empty", "Empty.lean")
    res = replay_olean(olean)
    assert res["clean"] is False, res
    assert res["replayed"] == 0
    # It ran and answered; it just did not answer "clean".
    assert res["ok"] is True, res
    assert "0 declarations" in res["error"], res


@requires_lean
def test_import_drift_is_caught(workdir):
    """A client olean is honest against its dependency, then the dependency is
    swapped underneath it. The client's bytes never change."""
    drift = os.path.join(workdir, "drift")
    os.makedirs(drift, exist_ok=True)
    _build(drift, "DriftBase", "DriftBase_v1.lean")
    client = _build(drift, "DriftClient", "DriftClient.lean")

    before = replay_olean(client, search_paths=[drift])
    assert before["clean"] is True, before
    assert "DriftBase" in before["imports"], before

    _build(drift, "DriftBase", "DriftBase_v2.lean")
    after = replay_olean(client, search_paths=[drift])
    assert after["clean"] is False, after
    assert "baseValue_is_zero" in after["error"], after


@requires_lean
def test_truncated_olean_crashes_lean_and_still_fails_closed(workdir):
    """`readModuleData` parses a memory-mapped region in native code and a
    truncated `.olean` segfaults it, so the Lean side never gets to report
    anything. The wrapper is the only thing keeping this closed: no summary line
    was printed and the exit code is outside the answered range."""
    honest = _build(workdir, "Honest", "Honest.lean")
    truncated = os.path.join(workdir, "Truncated.olean")
    with open(honest, "rb") as fh:
        head = fh.read(200)
    with open(truncated, "wb") as fh:
        fh.write(head)
    res = replay_olean(truncated)
    assert res["clean"] is False, res
    assert res["ok"] is False, res


@requires_lean
def test_replay_does_not_certify_a_trivial_theorem_as_meaningful(workdir):
    """Honest negative documentation, as a test rather than a comment: a replay
    passing says nothing about WHICH theorem was proved. The clean report on the
    honest fixture carries no statement, no axiom set, and no verdict beyond the
    declaration count."""
    olean = _build(workdir, "Honest", "Honest.lean")
    res = replay_olean(olean)
    assert res["clean"] is True
    assert set(res) == {
        "ok",
        "olean",
        "imports",
        "total_constants",
        "replayed",
        "skipped",
        "clean",
        "error",
    }
    assert "axioms" not in res
    assert "statement" not in res
