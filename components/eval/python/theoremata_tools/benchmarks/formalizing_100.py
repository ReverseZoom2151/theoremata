"""Formalizing-100 benchmark loader (formalization track).

Freek Wiedijk's public "Formalizing 100 Theorems" list is a canonical roster of
100 famous theorem *titles* used to compare proof assistants
(https://www.cs.ru.nl/~freek/100/). Only the public theorem-name list is used
here; every informal restatement below is our own brief paraphrase. No vendored
third-party statement text or proof source is copied.

This is a FORMALIZATION-track benchmark: each item is an informal statement that
is *to be formalized and proved*. There is no gold formal statement and no proof
shipped with the corpus, so items are not auto-gradable by statement
preservation — grading requires the full formalize+prove pipeline (the loader
marks ``gold_present=False`` and ``mode="to_be_formalized"`` accordingly).

The data lives in a committed ``data/formalizing_100.jsonl`` beside this module
(one row per theorem: ``{id, name, statement, tags}``). The loader degrades to
``[]`` (logging a skip) when the file is absent — never raising.
"""
from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import Any

from .schema import AXIOMS_WHITELIST, make_item

log = logging.getLogger("theoremata.benchmarks")

_DATA_FILE = Path(__file__).parent / "data" / "formalizing_100.jsonl"


def _read_rows(path: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line:
            continue
        rec = json.loads(line)
        if isinstance(rec, dict):
            rows.append(rec)
    return rows


def load_formalizing_100() -> list[dict[str, Any]]:
    """Wiedijk Formalizing-100 informal statements as formalization items.

    Deterministic: rows are emitted in file order and de-duplicated by ``id``.
    Returns ``[]`` (logging a skip) when the committed data file is absent.
    """
    if not _DATA_FILE.exists():
        log.info("benchmark %-24s loaded=0 skipped=0 (data file absent)", "formalizing_100")
        return []

    items: list[dict[str, Any]] = []
    skipped = 0
    seen: set[str] = set()
    for rec in _read_rows(_DATA_FILE):
        rid = str(rec.get("id") or "").strip()
        name = str(rec.get("name") or "").strip()
        statement = str(rec.get("statement") or "").strip()
        if not rid or not name or not statement or rid in seen:
            skipped += 1
            continue
        seen.add(rid)
        tags = rec.get("tags") or []
        if not isinstance(tags, list):
            tags = [str(tags)]
        items.append(
            make_item(
                id=f"formalizing_100:{rid}",
                kind="formalization",
                informal=statement,
                formal=None,  # no gold formal statement ships with this corpus
                expected={
                    "mode": "to_be_formalized",
                    "gold_present": False,
                    "name": name,
                    "tags": tags,
                    "axioms_whitelist": list(AXIOMS_WHITELIST),
                },
                grading={
                    "track": "formalization",
                    "method": "formalize_and_prove",
                    "task": "formalization",
                    "auto_gradable": False,
                },
                provenance={
                    "corpus": "formalizing_100",
                    "source": "Freek Wiedijk, Formalizing 100 Theorems (public theorem-name list)",
                    "theorem_id": rid,
                    "name": name,
                    "tags": tags,
                    "path": str(_DATA_FILE.name),
                },
            )
        )
    log.info(
        "benchmark %-24s loaded=%d skipped=%d (Wiedijk 100-theorems formalization track)",
        "formalizing_100",
        len(items),
        skipped,
    )
    return items
