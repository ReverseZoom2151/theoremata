"""Tests for the Formalizing-100 benchmark (Wiedijk public theorem-name list).

A FORMALIZATION-track benchmark of informal statements to be formalized/proved.
No proofs / no gold formal statements ship with it, so these tests only assert
the loader returns well-formed items (id/name/statement present, ids unique),
that the committed data file parses, and that loading is deterministic.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

_EVAL = Path(__file__).resolve().parents[1]
for _p in (_EVAL, _EVAL / "python"):
    if str(_p) not in sys.path:
        sys.path.insert(0, str(_p))

from theoremata_tools.benchmarks.formalizing_100 import (  # noqa: E402
    load_formalizing_100,
)

_DATA = (
    _EVAL
    / "python"
    / "theoremata_tools"
    / "benchmarks"
    / "data"
    / "formalizing_100.jsonl"
)


def _read_jsonl(path: Path) -> list[dict]:
    return [
        json.loads(ln)
        for ln in path.read_text(encoding="utf-8").splitlines()
        if ln.strip()
    ]


# --------------------------------------------------------------------------- #
# Committed data file
# --------------------------------------------------------------------------- #

def test_data_file_exists_and_parses() -> None:
    assert _DATA.exists(), "formalizing_100.jsonl committed data file is missing"
    rows = _read_jsonl(_DATA)
    assert rows, "data file is empty"
    # a solid subset of the well-known 100 (honest count is reported separately)
    assert len(rows) >= 40, f"expected >= 40 theorems, got {len(rows)}"
    seen: set[str] = set()
    for rec in rows:
        assert set(rec) >= {"id", "name", "statement"}, rec
        assert isinstance(rec["name"], str) and rec["name"].strip()
        assert isinstance(rec["statement"], str) and rec["statement"].strip()
        assert isinstance(rec.get("tags", []), list)
        rid = str(rec["id"])
        assert rid and rid not in seen, f"duplicate id {rid}"
        seen.add(rid)


# --------------------------------------------------------------------------- #
# Loader
# --------------------------------------------------------------------------- #

def test_loader_returns_wellformed_items() -> None:
    items = load_formalizing_100()
    assert isinstance(items, list)
    assert len(items) > 0, "loader returned no items"
    seen: set[str] = set()
    for it in items:
        assert set(it) >= {
            "id", "kind", "informal", "formal", "expected", "provenance", "grading",
        }
        assert it["kind"] == "formalization"
        assert it["id"] and it["id"] not in seen, f"dup id {it['id']}"
        seen.add(it["id"])
        assert it["informal"].strip(), f"{it['id']}: empty statement"
        assert it["expected"]["name"].strip(), f"{it['id']}: empty name"
        # formalization track: informal-only, no gold formal statement / proof
        assert it["formal"] is None
        assert it["expected"]["gold_present"] is False
        assert it["expected"]["mode"] == "to_be_formalized"
        assert it["grading"]["track"] == "formalization"


def test_loader_ids_match_data_rows() -> None:
    rows = _read_jsonl(_DATA)
    items = load_formalizing_100()
    assert len(items) == len(rows)
    expected_ids = {f"formalizing_100:{r['id']}" for r in rows}
    assert {it["id"] for it in items} == expected_ids


def test_loader_is_deterministic() -> None:
    first = load_formalizing_100()
    second = load_formalizing_100()
    assert [it["id"] for it in first] == [it["id"] for it in second]
    assert first == second
