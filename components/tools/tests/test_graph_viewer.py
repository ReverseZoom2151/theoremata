"""Tests for the self-contained proof-DAG web viewer.

All offline and deterministic: each test builds a tiny SQLite DB whose schema
mirrors the real one in ``components/graph/db.rs`` (projects / nodes / edges,
including the legacy ``tainted`` bool and the three-valued ``taint`` column),
renders it, and asserts the emitted HTML is a single self-contained document
with every piece of DB content HTML/JS-escaped.
"""
from __future__ import annotations

import sqlite3

from theoremata_tools.graph_viewer import OP, render_graph, run

# A title that is a live XSS payload if ever rendered as raw markup.
XSS_TITLE = "<img src=x onerror=alert(1)>"


def _make_db(path, *, project_id="p1", with_taint_col=True, nodes=True):
    """Create a minimal DB matching the real projects/nodes/edges schema."""
    conn = sqlite3.connect(path)
    taint_col = ", taint TEXT NOT NULL DEFAULT 'clean'" if with_taint_col else ""
    conn.executescript(
        f"""
        CREATE TABLE projects (
          id TEXT PRIMARY KEY, name TEXT NOT NULL, theorem TEXT NOT NULL,
          created_at TEXT NOT NULL, updated_at TEXT NOT NULL
        );
        CREATE TABLE nodes (
          id TEXT PRIMARY KEY, project_id TEXT NOT NULL,
          kind TEXT NOT NULL, status TEXT NOT NULL, title TEXT NOT NULL,
          statement TEXT NOT NULL, formal_statement TEXT,
          provenance TEXT NOT NULL DEFAULT 'test', content_hash TEXT NOT NULL DEFAULT 'h',
          tainted INTEGER NOT NULL DEFAULT 0,
          tier TEXT NOT NULL DEFAULT 'spine', parent_id TEXT,
          created_at TEXT NOT NULL DEFAULT 't', updated_at TEXT NOT NULL DEFAULT 't'
          {taint_col}
        );
        CREATE TABLE edges (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          project_id TEXT NOT NULL, source_id TEXT NOT NULL, target_id TEXT NOT NULL,
          kind TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT 't'
        );
        """
    )
    conn.execute(
        "INSERT INTO projects (id, name, theorem, created_at, updated_at) "
        "VALUES (?, ?, ?, 't', 't')",
        (project_id, "Demo & <Project>", "For all n, P(n) <=> Q(n)"),
    )
    if nodes:
        # (id, kind, status, title, statement, tainted, taint)
        rows = [
            ("n1", "conjecture", "active", "Main conjecture",
             "The main claim: a & b < c", 0, "clean"),
            ("n2", "lemma", "formally_verified", XSS_TITLE,
             "Lemma body </script><b>x</b>", 0, "clean"),
            ("n3", "obligation", "rejected", "Bad step", "1 = 2", 1, "clean"),
            ("n4", "lemma", "blocked", "Admitted gap", "sorry", 0, "self_admitted"),
        ]
        for nid, kind, status, title, stmt, tainted, taint in rows:
            if with_taint_col:
                conn.execute(
                    "INSERT INTO nodes (id, project_id, kind, status, title, "
                    "statement, tainted, taint) VALUES (?,?,?,?,?,?,?,?)",
                    (nid, project_id, kind, status, title, stmt, tainted, taint),
                )
            else:
                conn.execute(
                    "INSERT INTO nodes (id, project_id, kind, status, title, "
                    "statement, tainted) VALUES (?,?,?,?,?,?,?)",
                    (nid, project_id, kind, status, title, stmt, tainted),
                )
        edges = [
            ("n1", "n2", "depends_on"),
            ("n1", "n3", "depends_on"),
            ("n2", "n4", "supports"),
        ]
        for src, tgt, kind in edges:
            conn.execute(
                "INSERT INTO edges (project_id, source_id, target_id, kind) "
                "VALUES (?,?,?,?)",
                (project_id, src, tgt, kind),
            )
    conn.commit()
    conn.close()


def _assert_self_contained(h):
    """No external references of any kind."""
    assert "http://" not in h
    assert "https://" not in h
    assert "<script src" not in h
    assert 'src="http' not in h
    assert "<link" not in h
    assert 'href="http' not in h
    assert "//cdn" not in h


# --- core rendering --------------------------------------------------------


def test_renders_single_self_contained_document(tmp_path):
    db = tmp_path / "g.db"
    _make_db(db)
    h = render_graph(str(db), "p1")
    assert h.startswith("<!DOCTYPE html>")
    assert h.rstrip().endswith("</html>")
    _assert_self_contained(h)
    # Inline assets present, no external ones.
    assert "<style>" in h and "<script>" in h


def test_xss_title_is_escaped_not_raw(tmp_path):
    db = tmp_path / "g.db"
    _make_db(db)
    h = render_graph(str(db), "p1")
    # The raw payload must never appear as live markup.
    assert XSS_TITLE not in h
    assert "<img src=x onerror=alert(1)>" not in h
    # It survives only in escaped form inside the JSON data island.
    assert "\\u003cimg src=x onerror=alert(1)\\u003e" in h
    # A statement that tries to close the script tag is also neutralised.
    assert "</script><b>x</b>" not in h
    assert "\\u003c/script\\u003e" in h


def test_project_header_html_escaped(tmp_path):
    db = tmp_path / "g.db"
    _make_db(db)
    h = render_graph(str(db), "p1")
    # The project name/theorem contain & and <> -> must be entity-escaped.
    assert "Demo &amp; &lt;Project&gt;" in h
    assert "Demo & <Project>" not in h


def test_contains_all_node_ids_and_edges(tmp_path):
    db = tmp_path / "g.db"
    _make_db(db)
    h = render_graph(str(db), "p1")
    for nid in ("n1", "n2", "n3", "n4"):
        assert f'"{nid}"' in h
    # Edges shipped in the model (source/target pairs).
    assert '"source": "n1"' in h
    assert '"target": "n4"' in h
    assert "depends_on" in h and "supports" in h


def test_taint_reconciliation(tmp_path):
    db = tmp_path / "g.db"
    _make_db(db)
    h = render_graph(str(db), "p1")
    # n3 has tainted=1 with taint='clean' -> reconciled to 'tainted'.
    assert '"tainted"' in h
    # n4 carries the explicit three-valued self_admitted taint.
    assert '"self_admitted"' in h


def test_deterministic(tmp_path):
    db = tmp_path / "g.db"
    _make_db(db)
    a = render_graph(str(db), "p1")
    b = render_graph(str(db), "p1")
    assert a == b


def test_timestamp_is_only_wallclock(tmp_path):
    db = tmp_path / "g.db"
    _make_db(db)
    plain = render_graph(str(db), "p1")
    stamped = render_graph(str(db), "p1", timestamp="2026-07-09T00:00:00Z")
    assert plain != stamped
    assert "2026-07-09T00:00:00Z" in stamped
    _assert_self_contained(stamped)


def test_works_without_taint_column(tmp_path):
    # Old DBs lack the `taint` column; the bool must still drive the marker.
    db = tmp_path / "old.db"
    _make_db(db, with_taint_col=False)
    h = render_graph(str(db), "p1")
    _assert_self_contained(h)
    assert '"tainted"' in h  # n3's legacy bool reconciled


# --- graceful degradation --------------------------------------------------


def test_missing_project_valid_page(tmp_path):
    db = tmp_path / "g.db"
    _make_db(db)
    h = render_graph(str(db), "does-not-exist")
    assert h.startswith("<!DOCTYPE html>")
    assert h.rstrip().endswith("</html>")
    assert "no such project" in h.lower()
    _assert_self_contained(h)


def test_missing_project_id_is_escaped(tmp_path):
    db = tmp_path / "g.db"
    _make_db(db)
    h = render_graph(str(db), "<script>alert(1)</script>")
    assert "<script>alert(1)</script>" not in h
    assert "&lt;script&gt;alert(1)&lt;/script&gt;" in h


def test_empty_graph_valid_page(tmp_path):
    db = tmp_path / "g.db"
    _make_db(db, nodes=False)
    h = render_graph(str(db), "p1")
    assert h.startswith("<!DOCTYPE html>")
    assert h.rstrip().endswith("</html>")
    assert "no nodes" in h.lower()
    _assert_self_contained(h)


# --- worker adapter --------------------------------------------------------


def test_run_returns_html_inline(tmp_path):
    db = tmp_path / "g.db"
    _make_db(db)
    r = run({"db_path": str(db), "project_id": "p1"})
    assert r["op"] == OP == "graph_viewer"
    assert r["nodes"] == 4
    assert r["edges"] == 3
    assert r["html"].startswith("<!DOCTYPE html>")
    assert "path" not in r


def test_run_writes_out_path(tmp_path):
    db = tmp_path / "g.db"
    _make_db(db)
    out = tmp_path / "viewer.html"
    r = run({"db_path": str(db), "project_id": "p1", "out_path": str(out)})
    assert r["op"] == "graph_viewer"
    assert r["path"] == str(out)
    assert "html" not in r
    written = out.read_text(encoding="utf-8")
    assert written.startswith("<!DOCTYPE html>")
    _assert_self_contained(written)


def test_run_missing_project_counts_zero(tmp_path):
    db = tmp_path / "g.db"
    _make_db(db)
    r = run({"db_path": str(db), "project_id": "nope"})
    assert r["nodes"] == 0 and r["edges"] == 0
    assert "no such project" in r["html"].lower()
