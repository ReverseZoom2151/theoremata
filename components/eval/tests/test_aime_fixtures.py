"""Fixtures for the AIME nl-answer loaders and the Strong-PNT proof-DAG.

Two deliverables are exercised here:

* **AIME24/25/26** — the vendored ``resources/aime2x`` repos ship *only* a PDF
  data card (title / license / citation, empty "Appendix"); they contain **no**
  problems or answers.  We therefore do not fabricate any, and instead read any
  *committed* ``benchmarks/data/aimeXX.jsonl`` fixtures that a maintainer has
  populated with real problems.  Each such fixture must carry ``id`` / ``problem``
  / integer ``answer`` (0-999, the AIME answer range).  When none are present the
  populated-set assertions skip cleanly — the honest current state.
* **Strong-PNT proof-DAG** — a committed JSON fixture parsed from the numbered
  lemma blueprint in ``math-papers/Strong PNT.pdf``; it must parse and be acyclic.

The fixtures are read as committed JSON/JSONL, so no PDF parsing (pypdf) happens
at test time.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

_EVAL = Path(__file__).resolve().parents[1]
for _p in (_EVAL, _EVAL / "python"):
    if str(_p) not in sys.path:
        sys.path.insert(0, str(_p))

from theoremata_tools.benchmarks import load_benchmark  # noqa: E402

_DATA = _EVAL / "python" / "theoremata_tools" / "benchmarks" / "data"
_DAG_FIXTURE = _DATA / "strongpnt_dag.json"

# registry name -> committed fixture file
_AIME_SETS = {
    "aime24": _DATA / "aime24.jsonl",
    "aime25": _DATA / "aime25.jsonl",
    "aime26": _DATA / "aime26.jsonl",
}


def _read_jsonl(path: Path) -> list[dict]:
    return [
        json.loads(ln)
        for ln in path.read_text(encoding="utf-8").splitlines()
        if ln.strip()
    ]


def _populated_sets() -> list[str]:
    return [name for name, p in _AIME_SETS.items() if p.exists()]


# --------------------------------------------------------------------------- #
# AIME committed fixtures
# --------------------------------------------------------------------------- #

@pytest.mark.parametrize("name", sorted(_AIME_SETS))
def test_aime_committed_fixture_wellformed(name: str) -> None:
    """A committed AIME fixture (if present) is real integer-answer data."""
    path = _AIME_SETS[name]
    if not path.exists():
        pytest.skip(
            f"{name}: no committed fixture — vendored source is a PDF data "
            f"card with no problems (nothing extractable, not fabricated)"
        )
    records = _read_jsonl(path)
    assert records, f"{name}: committed fixture is empty"
    seen_ids: set[str] = set()
    for rec in records:
        assert set(rec) >= {"id", "problem", "answer"}, rec
        assert isinstance(rec["problem"], str) and rec["problem"].strip()
        rid = str(rec["id"])
        assert rid and rid not in seen_ids, f"dup id {rid}"
        seen_ids.add(rid)
        ans = int(rec["answer"])  # must coerce to int
        assert 0 <= ans <= 999, f"{name}: answer {ans} out of AIME range"


@pytest.mark.parametrize("name", sorted(_AIME_SETS))
def test_aime_registry_loader(name: str) -> None:
    """The registry loader never raises and never fabricates items.

    For a *populated* set (committed fixture wired into the loader) it must
    return >0 well-formed integer-answer items; otherwise it degrades to ``[]``.
    """
    items = load_benchmark(name)
    assert isinstance(items, list)
    if name not in _populated_sets():
        assert items == [], f"{name}: no source, loader must return [] not {len(items)}"
        return
    # Loader is wired to the committed fixture -> must light up.
    assert items, f"{name}: populated fixture but loader returned 0 items"
    for it in items:
        problem = it.get("informal") or it.get("problem")
        expected = it.get("expected", {})
        ans = int(expected.get("answer"))
        assert problem and 0 <= ans <= 999


def test_at_least_dag_fixture_is_populated() -> None:
    """Guard against a totally empty deliverable: the DAG fixture must exist."""
    assert _DAG_FIXTURE.exists(), "Strong-PNT DAG fixture missing"


# --------------------------------------------------------------------------- #
# Strong-PNT proof-DAG
# --------------------------------------------------------------------------- #

def _load_dag() -> dict:
    return json.loads(_DAG_FIXTURE.read_text(encoding="utf-8"))


def test_strongpnt_dag_parses() -> None:
    dag = _load_dag()
    nodes = dag["nodes"]
    assert isinstance(nodes, dict) and len(nodes) >= 50
    assert dag["n_nodes"] == len(nodes)
    for nid, node in nodes.items():
        assert node["statement"].strip(), f"{nid}: empty statement"
        assert isinstance(node["depends_on"], list)


def test_strongpnt_dag_deps_reference_existing_nodes() -> None:
    nodes = _load_dag()["nodes"]
    for nid, node in nodes.items():
        for dep in node["depends_on"]:
            assert dep in nodes, f"{nid} -> {dep}: dangling dependency"
            assert dep != nid, f"{nid}: self-dependency"


def test_strongpnt_dag_is_acyclic() -> None:
    nodes = _load_dag()["nodes"]
    # Kahn's algorithm: a full topological order exists iff the graph is a DAG.
    indeg = {n: len(nodes[n]["depends_on"]) for n in nodes}
    radj: dict[str, list[str]] = {n: [] for n in nodes}
    for n in nodes:
        for dep in nodes[n]["depends_on"]:
            radj[dep].append(n)
    queue = [n for n, d in indeg.items() if d == 0]
    visited = 0
    while queue:
        cur = queue.pop()
        visited += 1
        for nxt in radj[cur]:
            indeg[nxt] -= 1
            if indeg[nxt] == 0:
                queue.append(nxt)
    assert visited == len(nodes), (
        f"cycle detected: only {visited}/{len(nodes)} nodes topologically ordered"
    )
