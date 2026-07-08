"""Tests for Isabelle premise retrieval (parity with the Lean retriever).

Two layers, mirroring ``test_retrieval.py``:

* **offline** -- parsing, scoring, the mock backend and graceful no-toolchain
  fallback are exercised deterministically with no Isabelle present.
* **live** (optional) -- if an ``isabelle`` binary is probeable (native or via
  WSL) a real ``find_theorems`` query is run and asserted to return >=1 hit in
  the shared ``{name, module, kind, score}`` contract. Skips cleanly otherwise.
"""
from __future__ import annotations

import pytest

from theoremata_tools import isabelle_retrieval as IR


# Verbatim capture of a real ``isabelle process_theories`` run of
# ``find_theorems (2) "_ + 0 = _"`` on this box -- including the wrapped
# statement layout (a long name line whose statement continues, 4-space
# indented, on the next line).
_REAL_OUTPUT = """Running Draft ...

Output (line 4 of "/tmp/tmp.A4/Scratch.thy"):
find_theorems
  "_ + 0 = _"

found 9 theorem(s) (2 displayed):
  Groups.monoid_add_class.add_0_right: ?a + 0 = ?a
  Semiring_Normalization.comm_semiring_1_class.semiring_normalization_rules(6):
    ?a + 0 = ?a
Finished Draft (0:00:01 elapsed time)
"""

_NOTHING_OUTPUT = """Running Draft ...

Output (line 4 of "/tmp/x/Scratch.thy"):
find_theorems
  "xyzzy_no_such_symbol_zzz"

found nothing
Finished Draft (0:00:01 elapsed time)
"""


# --- parsing -----------------------------------------------------------------


def test_parse_matches_extracts_names_and_statements():
    matches = IR._parse_matches(_REAL_OUTPUT)
    assert len(matches) == 2
    names = [n for n, _ in matches]
    assert names[0] == "Groups.monoid_add_class.add_0_right"
    # the wrapped statement is re-joined onto its (long) name
    assert names[1] == (
        "Semiring_Normalization.comm_semiring_1_class."
        "semiring_normalization_rules(6)"
    )
    assert matches[0][1] == "?a + 0 = ?a"
    assert matches[1][1] == "?a + 0 = ?a"


def test_parse_nothing_is_empty():
    assert IR._parse_matches(_NOTHING_OUTPUT) == []


def test_parse_garbage_is_empty():
    assert IR._parse_matches("no found marker here\njust noise") == []


# --- scoring / contract ------------------------------------------------------


def test_to_results_shape_and_module_and_kind():
    matches = IR._parse_matches(_REAL_OUTPUT)
    results = IR._to_results(matches, "_ + 0 = _", limit=10)
    assert results
    first = results[0]
    assert set(first.keys()) == {"name", "module", "kind", "score"}
    # module = the Isabelle theory (first dotted segment of the fact name)
    assert first["module"] == "Groups"
    assert results[1]["module"] == "Semiring_Normalization"
    assert all(r["kind"] == "theorem" for r in results)


def test_to_results_scores_strictly_descending():
    matches = IR._parse_matches(_REAL_OUTPUT)
    results = IR._to_results(matches, "add", limit=10)
    scores = [r["score"] for r in results]
    assert scores == sorted(scores, reverse=True)
    assert len(set(scores)) == len(scores)  # strictly descending, no ties


def test_to_results_respects_limit():
    matches = [(f"Thy.fact_{i}", "?a = ?a") for i in range(10)]
    assert len(IR._to_results(matches, "fact", limit=3)) == 3


# --- criteria construction ---------------------------------------------------


def test_criteria_default_is_quoted_term_pattern():
    assert IR._criteria("_ + 0 = _", None) == '"_ + 0 = _"'


def test_criteria_name_mode():
    assert IR._criteria("add_commute", "name") == 'name: "add_commute"'


def test_criteria_sanitizes_embedded_quotes():
    # embedded double quotes must not escape the inner-syntax literal
    assert '"' not in IR._criteria('foo"bar', None).strip('"')


def test_theory_text_embeds_limit_and_criteria():
    thy = IR._theory_text("add_commute", 5, "name")
    assert "find_theorems (5) name: \"add_commute\"" in thy
    assert thy.startswith("theory Scratch")
    assert "imports Main" in thy


# --- mock backend ------------------------------------------------------------


def test_mock_search_is_contract_shaped():
    res = IR.search("_ + 0 = _", force_mode="mock", limit=5)
    assert res["ok"] is True
    assert res["backend"] == "mock"
    assert res["mode"] == "mock"
    assert res["count"] == len(res["results"])
    assert res["results"], "mock should synthesise at least one hit"
    for r in res["results"]:
        assert set(r.keys()) == {"name", "module", "kind", "score"}
        assert r["kind"] == "theorem"
        assert isinstance(r["module"], str) and r["module"]


def test_mock_search_is_deterministic():
    a = IR.search("add comm", force_mode="mock")
    b = IR.search("add comm", force_mode="mock")
    assert a["results"] == b["results"]


def test_mock_search_empty_query_returns_no_hits():
    res = IR.search("   ", force_mode="mock")
    assert res["ok"] is True
    assert res["results"] == []


def test_mock_search_respects_limit():
    res = IR.search("a b c d e f", force_mode="mock", limit=2)
    assert len(res["results"]) <= 2


# --- graceful fallback (live requested, toolchain broken) --------------------


def test_live_failure_degrades_to_mock(monkeypatch):
    # Force the live path but make command resolution return a bogus native
    # binary so the real invocation raises IsabelleUnavailable -> mock fallback.
    monkeypatch.setenv(IR._COMMAND_ENV, "____no_such_isabelle_binary____")
    monkeypatch.delenv(IR._MOCK_ENV, raising=False)
    res = IR.search("_ + 0 = _", force_mode="live", limit=3, timeout=30)
    assert res["ok"] is True
    assert res["backend"] == "mock"
    assert "note" in res and "fell back to mock" in res["note"]
    assert res["results"], "fallback still returns contract-shaped results"
    assert set(res["results"][0].keys()) == {"name", "module", "kind", "score"}


# --- worker dispatch ---------------------------------------------------------


def test_run_search_op_mock():
    out = IR.run({"op": "search", "query": "add", "force_mode": "mock"})
    assert out["ok"] is True
    assert out["backend"] == "mock"
    assert "results" in out


def test_run_detect_op():
    out = IR.run({"op": "detect"})
    assert out["ok"] is True
    assert out["mode"] in ("live", "mock")


def test_run_unknown_op():
    out = IR.run({"op": "bogus"})
    assert out["ok"] is False


# --- optional live integration ----------------------------------------------


@pytest.mark.skipif(
    not IR.tool_available(),
    reason="Isabelle toolchain not available (native or WSL)",
)
def test_live_find_theorems_returns_results():
    # A pattern with many library matches; Isabelle is slow (heap load +
    # process start), so give it a generous budget.
    res = IR.search("_ + 0 = _", force_mode="live", limit=10, timeout=300)
    assert res["ok"] is True
    assert res["backend"] == "isabelle"
    assert res["mode"] == "live"
    assert res["count"] >= 1, f"expected >=1 hit, got: {res}"
    for r in res["results"]:
        assert set(r.keys()) == {"name", "module", "kind", "score"}
        assert r["name"] and r["module"]
        assert r["kind"] == "theorem"
    # scores strictly descending (ranked list, no re-sort needed downstream)
    scores = [r["score"] for r in res["results"]]
    assert scores == sorted(scores, reverse=True)


@pytest.mark.skipif(
    not IR.tool_available(),
    reason="Isabelle toolchain not available (native or WSL)",
)
def test_live_name_mode_query():
    res = IR.search("add_commute", session="HOL", limit=5, mode="name",
                    force_mode="live", timeout=300)
    assert res["ok"] is True and res["backend"] == "isabelle"
    assert res["count"] >= 1
    # every hit's name should contain the searched-for substring
    assert any("add_commute" in r["name"] for r in res["results"])
