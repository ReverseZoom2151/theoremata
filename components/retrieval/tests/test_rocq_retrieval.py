"""Tests for the Rocq (Coq) premise-retrieval module.

Two tiers:

* **offline** (always run) -- force mock mode and assert the shared result
  contract ``{ok, backend, mode, results:[{name, module, kind, score}]}``, the
  parser over captured ``coqc`` Search output, glob parsing, and graceful
  degradation when no toolchain is present.
* **live** (skipped unless a ``coqc`` is reachable natively or via WSL Ubuntu)
  -- run a real ``Search`` and assert >=1 parsed premise in the contract shape.
"""
from __future__ import annotations

import os

import pytest

from theoremata_tools import rocq_retrieval as rr


# --------------------------------------------------------------------------- #
# Helpers.
# --------------------------------------------------------------------------- #
def _assert_contract(res: dict) -> None:
    assert res["ok"] is True
    assert res["backend"] in {"mock", "coqc", "coqc-wsl"}
    assert res["mode"] in {"mock", "live"}
    assert isinstance(res["results"], list)
    for r in res["results"]:
        assert set(r) >= {"name", "module", "kind", "score"}
        assert isinstance(r["name"], str) and r["name"]
        assert isinstance(r["score"], (int, float))


@pytest.fixture
def force_mock(monkeypatch):
    monkeypatch.setenv(rr.MOCK_ENV, "1")
    rr._wsl_probe.cache_clear()
    yield
    rr._wsl_probe.cache_clear()


# --------------------------------------------------------------------------- #
# Offline: mock search.
# --------------------------------------------------------------------------- #
def test_mock_search_relevant_lemma(force_mock):
    res = rr.search("add_comm")
    _assert_contract(res)
    assert res["mode"] == "mock" and res["backend"] == "mock"
    names = [r["name"] for r in res["results"]]
    # The exact-name premise must win the ranking.
    assert names[0] == "Nat.add_comm"
    assert res["results"][0]["module"] == "Nat"


def test_mock_search_pattern_query_returns_results(force_mock):
    # A pure pattern query has little lexical signal; mock still returns the
    # corpus ranked in a stable order.
    res = rr.search("(?n + ?m = ?m + ?n)", mode="pattern")
    _assert_contract(res)
    assert res["count"] == len(res["results"]) >= 1


def test_mock_search_respects_limit(force_mock):
    res = rr.search("nat", limit=3)
    _assert_contract(res)
    assert len(res["results"]) <= 3


def test_mock_scores_sorted_desc(force_mock):
    res = rr.search("le", limit=10)
    scores = [r["score"] for r in res["results"]]
    assert scores == sorted(scores, reverse=True)


def test_run_dispatch_search(force_mock):
    res = rr.run({"op": "search", "query": "mul_comm", "limit": 5})
    _assert_contract(res)
    assert any(r["name"] == "Nat.mul_comm" for r in res["results"])


def test_run_dispatch_detect(force_mock):
    res = rr.run({"op": "detect"})
    assert res["ok"] and res["mode"] == "mock" and res["backend"] == "mock"


def test_run_unknown_op(force_mock):
    res = rr.run({"op": "bogus"})
    assert res["ok"] is False


def test_dump_mock_reports_unavailable(force_mock):
    res = rr.dump_decls("whatever.v")
    assert res["ok"] is False and res["backend"] == "mock"


# --------------------------------------------------------------------------- #
# Offline: parser units (real captured coqc output, no toolchain needed).
# --------------------------------------------------------------------------- #
def test_parse_single_line_entry():
    out = "Nat.add_comm: forall n m : nat, n + m = m + n\n"
    entries, err = rr._parse_search_output(out)
    assert err is None
    assert len(entries) == 1
    assert entries[0]["name"] == "Nat.add_comm"
    assert "n + m = m + n" in entries[0]["type"]


def test_parse_wrapped_type():
    out = (
        "app_length:\n"
        "  forall [A : Type] (l l' : list A), length (l ++ l') = length l + length l'\n"
    )
    entries, err = rr._parse_search_output(out)
    assert len(entries) == 1
    assert entries[0]["name"] == "app_length"
    assert "length (l ++ l')" in entries[0]["type"]


def test_parse_detects_error():
    out = (
        'File "q.v", line 1, characters 0-35:\n'
        "Error: Cannot find a physical path bound to logical path Foo.Bar.\n"
    )
    entries, err = rr._parse_search_output(out)
    assert entries == []
    assert err is not None and "Cannot find" in err


def test_module_and_score_helpers():
    assert rr._module_of("Nat.add_comm") == "Nat"
    assert rr._module_of("le_n") == ""
    # Exact name match outscores a mere token overlap.
    exact = rr._score("Nat.add_comm", "Nat.add_comm", "Nat", 0)
    partial = rr._score("Nat.add_comm", "Nat.add_assoc", "Nat", 1)
    assert exact > partial


def test_rank_entries_contract_shape():
    entries = [
        {"name": "Nat.add_comm", "type": "..."},
        {"name": "Nat.add_assoc", "type": "..."},
    ]
    ranked = rr._rank_entries("add_comm", entries, 10)
    assert ranked[0]["name"] == "Nat.add_comm"
    for r in ranked:
        assert set(r) == {"name", "module", "kind", "score"}


def test_build_vfile_modes():
    body, opam = rr._build_vfile("add_comm", ["Coq.Arith.Arith"], "search")
    assert "Require Import Coq.Arith.Arith." in body
    # Name-shaped queries become substring searches by declaration name.
    assert 'Search "add_comm".' in body
    assert opam is False

    # A genuine pattern is passed through unquoted.
    body, _ = rr._build_vfile("(?n + ?m = ?m + ?n)", ["Coq.Arith.Arith"], "search")
    assert "Search (?n + ?m = ?m + ?n)." in body

    body, _ = rr._build_vfile("?n <= ?n", ["Coq.Arith.Arith"], "pattern")
    assert "SearchPattern (?n <= ?n)." in body

    body, opam = rr._build_vfile("addn _ _", ["mathcomp.ssrnat"], "search")
    assert "From mathcomp Require Import ssrnat." in body
    assert opam is True


def test_parse_glob():
    glob = (
        "DIGEST 2b1f5b685a35d239db094b24da5faf30\n"
        "FFix\n"
        "def 11:15 <> myDef\n"
        "R19:21 Coq.Init.Datatypes <> nat ind\n"
        "prf 37:41 <> myThm\n"
        "prf 83:87 <> myLem\n"
    )
    decls = rr._parse_glob(glob)
    by_name = {d["name"]: d for d in decls}
    assert by_name["Fix.myDef"]["kind"] == "def"
    assert by_name["Fix.myThm"]["kind"] == "theorem"
    assert by_name["Fix.myThm"]["module"] == "Fix"
    assert all(d["is_axiom"] is False for d in decls)


def test_win_to_wsl_path():
    assert rr._win_to_wsl_path(r"C:\Users\x\q.v") == "/mnt/c/Users/x/q.v"


# --------------------------------------------------------------------------- #
# Live: real coqc (native or WSL). Skipped if no toolchain.
# --------------------------------------------------------------------------- #
def _coqc_available() -> bool:
    rr._wsl_probe.cache_clear()
    mode, _backend, _binary = rr.detect_backend()
    return mode == "live"


live = pytest.mark.skipif(
    not _coqc_available(), reason="no coqc reachable (native or WSL Ubuntu)"
)


@live
def test_live_search_returns_parsed_premises():
    res = rr.search("(?n + ?m = ?m + ?n)", mode="pattern",
                    imports=["Coq.Arith.Arith"])
    _assert_contract(res)
    assert res["mode"] == "live"
    assert res["backend"] in {"coqc", "coqc-wsl"}
    assert res["count"] >= 1, f"expected >=1 premise, got {res}"
    # add_comm is the canonical hit for this commutativity pattern.
    names = [r["name"] for r in res["results"]]
    assert any("add_comm" in n for n in names), names


@live
def test_live_search_by_name():
    res = rr.search("le_trans", imports=["Coq.Arith.Arith"])
    _assert_contract(res)
    assert res["count"] >= 1
    assert any("le_trans" in r["name"] for r in res["results"])
