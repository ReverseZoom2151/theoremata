"""Self-contained proof-DAG web viewer.

Reads a project's proof-DAG straight out of the SQLite database that the Rust
core writes (see ``components/graph/db.rs``) and emits a SINGLE self-contained
HTML file -- inline CSS + vanilla JS, no external assets, no CDN, no ``<script
src>``. Open the file in a browser and you get a layered layout of the graph:
nodes coloured by status and kind and marked by taint, edges coloured by kind,
a clickable side panel showing a selected node's statement, and a legend.

Tables / columns read (all from ``db.rs``'s ``CREATE TABLE`` DDL):

* ``projects(id, name, theorem)`` -- the header / title.
* ``nodes(id, kind, status, title, statement, tainted, taint, tier,
  parent_id)`` -- the vertices. ``tainted`` is the legacy 0/1 bool; ``taint``
  is the newer three-valued ``clean|tainted|self_admitted`` text column. We
  reconcile them exactly as the Rust reader does (``db.rs`` ~line 1787): a
  ``taint='clean'`` default is overridden to ``tainted`` when the legacy bit is
  set. The ``taint`` column may be absent on very old DBs -- we introspect
  ``PRAGMA table_info`` and fall back to the bool.
* ``edges(id, source_id, target_id, kind)`` -- the arcs.

SECURITY: every value that originates in the database (project name/theorem,
node title/statement, ids, kinds) is treated as UNTRUSTED DATA. It is NEVER
concatenated into HTML or JS as raw markup. Two defences are used:

1. Text placed directly into the static HTML is run through :func:`html.escape`.
2. The graph model is shipped to the page as a JSON document inside a
   ``<script type="application/json">`` block, with ``<``, ``>`` and ``&``
   unicode-escaped so a malicious statement cannot close the script element.
   The JS reads it with ``JSON.parse`` and only ever assigns it via
   ``textContent`` -- never ``innerHTML`` -- so injected markup is inert.

The output is fully self-contained (no ``http://`` / ``https://`` references,
no external ``src``/``href``) and deterministic: nodes are ordered by id, edges
by id, and no wall-clock is embedded unless a timestamp is supplied by the
caller. Pure standard library (``sqlite3``, ``html``, ``json``).
"""
from __future__ import annotations

import html
import json
import sqlite3
from typing import Any

OP = "graph_viewer"

# --- palettes (single source of truth, also shipped to the page) -----------

#: Fill colour per node status (snake_case, matching ``model.rs::NodeStatus``).
STATUS_COLORS: dict[str, str] = {
    "proposed": "#9e9e9e",
    "active": "#1e88e5",
    "blocked": "#fb8c00",
    "rejected": "#e53935",
    "informally_verified": "#7cb342",
    "formally_verified": "#2e7d32",
    "superseded": "#8e24aa",
}
_STATUS_FALLBACK = "#607d8b"

#: Stroke colour per edge kind (matching ``model.rs::EdgeKind``).
EDGE_COLORS: dict[str, str] = {
    "depends_on": "#546e7a",
    "supports": "#43a047",
    "contradicts": "#e53935",
    "formalizes": "#3949ab",
    "verifies": "#00897b",
    "derived_from": "#6d4c41",
    "supersedes": "#8e24aa",
}
_EDGE_FALLBACK = "#90a4ae";

#: How a node's taint is drawn (border treatment on the node card).
TAINT_STYLES: dict[str, str] = {
    "clean": "",
    "tainted": "tainted",
    "self_admitted": "self-admitted",
}

# Layout geometry (pixels).
_COL_W = 200
_ROW_H = 130
_MARGIN = 48
_NODE_W = 160


# --- DB access -------------------------------------------------------------


def _has_column(conn: sqlite3.Connection, table: str, column: str) -> bool:
    """True if ``table`` has ``column`` (via ``PRAGMA table_info``)."""
    cur = conn.execute(f"PRAGMA table_info({table})")
    return any(row[1] == column for row in cur.fetchall())


def _reconcile_taint(taint_str: str, tainted_bool: int) -> str:
    """Reconcile the three-valued ``taint`` text against the legacy ``tainted``
    bool, mirroring the Rust reader in ``db.rs``: an explicit non-clean taint
    wins; a ``clean`` default is upgraded to ``tainted`` when the old bit is set.
    """
    taint_str = (taint_str or "clean").strip() or "clean"
    if taint_str == "clean" and tainted_bool:
        return "tainted"
    return taint_str


def _load_model(
    db_path: str, project_id: str
) -> dict[str, Any] | None:
    """Read the project's nodes/edges into a plain JSON-able graph model, or
    return ``None`` if the project does not exist. Deterministic ordering:
    nodes by id, edges by id."""
    conn = sqlite3.connect(db_path)
    try:
        conn.row_factory = sqlite3.Row
        proj = conn.execute(
            "SELECT id, name, theorem FROM projects WHERE id = ?",
            (project_id,),
        ).fetchone()
        if proj is None:
            return None

        has_taint = _has_column(conn, "nodes", "taint")
        taint_sel = "taint" if has_taint else "'clean' AS taint"
        node_rows = conn.execute(
            f"SELECT id, kind, status, title, statement, tainted, {taint_sel}, "
            "tier, parent_id FROM nodes WHERE project_id = ? ORDER BY id ASC",
            (project_id,),
        ).fetchall()
        edge_rows = conn.execute(
            "SELECT id, source_id, target_id, kind FROM edges "
            "WHERE project_id = ? ORDER BY id ASC",
            (project_id,),
        ).fetchall()

        nodes = [
            {
                "id": r["id"],
                "kind": r["kind"],
                "status": r["status"],
                "title": r["title"],
                "statement": r["statement"],
                "tier": r["tier"],
                "parent_id": r["parent_id"],
                "taint": _reconcile_taint(r["taint"], r["tainted"]),
            }
            for r in node_rows
        ]
        edges = [
            {
                "source": r["source_id"],
                "target": r["target_id"],
                "kind": r["kind"],
            }
            for r in edge_rows
        ]
        return {
            "project": {
                "id": proj["id"],
                "name": proj["name"],
                "theorem": proj["theorem"],
            },
            "nodes": nodes,
            "edges": edges,
        }
    finally:
        conn.close()


# --- layered layout (deterministic, computed here) -------------------------


def _assign_layers(
    node_ids: list[str], edges: list[dict[str, str]]
) -> dict[str, int]:
    """Assign each node a layer = length of the longest ``depends_on`` chain
    beneath it (leaves/axioms at layer 0). Deterministic and cycle-safe. Only
    ``depends_on`` edges drive layering; other edge kinds are drawn but do not
    move nodes. Unknown edge endpoints are ignored."""
    ids = set(node_ids)
    deps: dict[str, list[str]] = {nid: [] for nid in node_ids}
    for e in edges:
        if e["kind"] == "depends_on" and e["source"] in ids and e["target"] in ids:
            deps[e["source"]].append(e["target"])

    depth: dict[str, int] = {}
    IN_PROGRESS = -1

    def visit(nid: str) -> int:
        cached = depth.get(nid)
        if cached is not None and cached != IN_PROGRESS:
            return cached
        if cached == IN_PROGRESS:  # cycle guard
            return 0
        depth[nid] = IN_PROGRESS
        best = 0
        for tgt in deps[nid]:
            best = max(best, 1 + visit(tgt))
        depth[nid] = best
        return best

    for nid in node_ids:  # already sorted -> deterministic
        visit(nid)
    return depth


def _layout(model: dict[str, Any]) -> dict[str, Any]:
    """Compute pixel positions for every node and the canvas size. Nodes keep
    their id-sorted order within each layer, so the layout is fully
    deterministic. Positions are written back onto each node as ``x``/``y``."""
    nodes = model["nodes"]
    node_ids = [n["id"] for n in nodes]
    depth = _assign_layers(node_ids, model["edges"])
    max_depth = max(depth.values(), default=0)

    # Group by layer; higher depth (roots) drawn at the top.
    layers: dict[int, list[dict[str, Any]]] = {}
    for n in nodes:
        layers.setdefault(depth[n["id"]], []).append(n)

    max_cols = 1
    for d, layer_nodes in layers.items():
        row = max_depth - d  # roots (max depth) -> row 0 (top)
        for col, n in enumerate(layer_nodes):
            n["x"] = _MARGIN + col * _COL_W
            n["y"] = _MARGIN + row * _ROW_H
        max_cols = max(max_cols, len(layer_nodes))

    model["width"] = _MARGIN * 2 + max(max_cols - 1, 0) * _COL_W + _NODE_W
    model["height"] = _MARGIN * 2 + max_depth * _ROW_H + 80
    model["status_colors"] = STATUS_COLORS
    model["edge_colors"] = EDGE_COLORS
    return model


# --- safe embedding --------------------------------------------------------


def _script_safe_json(obj: Any) -> str:
    """``json.dumps`` with ``<``/``>``/``&`` unicode-escaped so the payload
    cannot break out of the surrounding ``<script>`` element. ``ensure_ascii``
    (default) already neutralises U+2028/U+2029 and other non-ASCII."""
    return (
        json.dumps(obj)
        .replace("&", "\\u0026")
        .replace("<", "\\u003c")
        .replace(">", "\\u003e")
    )


# --- HTML rendering --------------------------------------------------------


def _page(title: str, body: str, *, data_json: str | None = None) -> str:
    """Wrap a body in the shared self-contained HTML skeleton (inline CSS/JS)."""
    esc_title = html.escape(title)
    data_block = ""
    if data_json is not None:
        data_block = (
            '<script type="application/json" id="graph-data">'
            f"{data_json}</script>\n"
        )
    return (
        "<!DOCTYPE html>\n"
        '<html lang="en">\n<head>\n<meta charset="utf-8">\n'
        '<meta name="viewport" content="width=device-width, initial-scale=1">\n'
        f"<title>{esc_title}</title>\n"
        f"<style>{_CSS}</style>\n</head>\n<body>\n"
        f"{body}\n{data_block}"
        + (f"<script>{_JS}</script>\n" if data_json is not None else "")
        + "</body>\n</html>\n"
    )


def _empty_page(project: dict[str, Any]) -> str:
    """Valid page for a project whose graph has no nodes."""
    name = html.escape(project["name"])
    theorem = html.escape(project["theorem"])
    body = (
        '<header class="hdr"><h1>Proof DAG</h1>'
        f"<div class=\"proj\">{name}</div>"
        f"<div class=\"thm\">{theorem}</div></header>\n"
        '<main class="empty"><p>This project has no nodes yet.</p></main>'
    )
    return _page(f"Proof DAG: {project['name']}", body)


def _missing_page(project_id: str) -> str:
    """Valid page for a project id that does not exist in the DB."""
    pid = html.escape(project_id)
    body = (
        '<header class="hdr"><h1>Proof DAG</h1></header>\n'
        f'<main class="empty"><p>No such project: <code>{pid}</code></p></main>'
    )
    return _page("Proof DAG: no such project", body)


def render_graph(
    db_path: str, project_id: str, *, timestamp: str | None = None
) -> str:
    """Render the proof-DAG of ``project_id`` (in the SQLite DB at ``db_path``)
    as a single self-contained HTML document string.

    Degrades gracefully: a missing project yields a valid "no such project"
    page; a project with no nodes yields a valid empty page. All database
    content is escaped; the output has no external references. Deterministic
    unless a ``timestamp`` is passed (it is the only wall-clock that can appear).
    """
    model = _load_model(db_path, project_id)
    if model is None:
        return _missing_page(project_id)
    if not model["nodes"]:
        return _empty_page(model["project"])

    _layout(model)
    if timestamp is not None:
        model["generated_at"] = str(timestamp)

    project = model["project"]
    name = html.escape(project["name"])
    theorem = html.escape(project["theorem"])
    stamp = (
        f'<div class="stamp">generated {html.escape(str(timestamp))}</div>'
        if timestamp is not None
        else ""
    )
    body = f"""<header class="hdr">
  <h1>Proof DAG</h1>
  <div class="proj">{name}</div>
  <div class="thm">{theorem}</div>
  {stamp}
</header>
<div class="wrap">
  <div id="stage" class="stage">
    <canvas id="edges"></canvas>
    <div id="nodes"></div>
  </div>
  <aside id="panel" class="panel">
    <h2>Legend</h2>
    <div id="legend"></div>
    <h2>Selected node</h2>
    <div id="detail" class="detail muted">Click a node to see its statement.</div>
  </aside>
</div>"""
    return _page(
        f"Proof DAG: {project['name']}",
        body,
        data_json=_script_safe_json(model),
    )


# --- worker adapter --------------------------------------------------------


def run(request: dict[str, Any]) -> dict[str, Any]:
    """JSON-worker adapter for op ``"graph_viewer"``.

    Request: ``{"db_path": str, "project_id": str, "out_path"?: str,
    "timestamp"?: str}``. When ``out_path`` is given the HTML is written there
    and the path is returned; otherwise the HTML string is returned inline.

    Returns ``{"op": "graph_viewer", "nodes": int, "edges": int,
    "path"?: str, "html"?: str}``.
    """
    db_path = request["db_path"]
    project_id = request["project_id"]
    out_path = request.get("out_path")
    timestamp = request.get("timestamp")

    document = render_graph(db_path, project_id, timestamp=timestamp)

    # Count what we actually rendered (0/0 for missing or empty).
    model = _load_model(db_path, project_id)
    n_nodes = len(model["nodes"]) if model else 0
    n_edges = len(model["edges"]) if model else 0

    result: dict[str, Any] = {"op": OP, "nodes": n_nodes, "edges": n_edges}
    if out_path is not None:
        with open(out_path, "w", encoding="utf-8") as fh:
            fh.write(document)
        result["path"] = out_path
    else:
        result["html"] = document
    return result


# --- inline assets ---------------------------------------------------------

_CSS = """
:root { color-scheme: light dark; }
* { box-sizing: border-box; }
body { margin: 0; font-family: system-ui, -apple-system, Segoe UI, Roboto, sans-serif;
  color: #1a1a1a; background: #f5f6f8; }
.hdr { padding: 12px 20px; background: #263238; color: #eceff1; }
.hdr h1 { margin: 0 0 2px; font-size: 18px; }
.hdr .proj { font-size: 14px; font-weight: 600; }
.hdr .thm { font-size: 13px; opacity: 0.85; white-space: pre-wrap; }
.hdr .stamp { font-size: 11px; opacity: 0.6; margin-top: 4px; }
.wrap { display: flex; align-items: flex-start; gap: 0; }
.stage { position: relative; overflow: auto; flex: 1 1 auto; height: calc(100vh - 88px); }
#edges { position: absolute; top: 0; left: 0; }
#nodes { position: absolute; top: 0; left: 0; }
.node { position: absolute; width: 160px; padding: 6px 8px; border-radius: 8px;
  border: 2px solid rgba(0,0,0,0.35); color: #fff; cursor: pointer; font-size: 12px;
  box-shadow: 0 1px 3px rgba(0,0,0,0.3); overflow: hidden; }
.node .n-kind { font-size: 10px; text-transform: uppercase; letter-spacing: 0.04em;
  opacity: 0.9; }
.node .n-title { font-weight: 600; white-space: nowrap; overflow: hidden;
  text-overflow: ellipsis; }
.node.tainted { border-style: dashed; border-color: #b71c1c; }
.node.self-admitted { border-style: dashed; border-color: #ff6f00; }
.node.selected { outline: 3px solid #ffeb3b; outline-offset: 1px; }
.panel { width: 320px; flex: 0 0 320px; height: calc(100vh - 88px); overflow: auto;
  padding: 12px 16px; background: #fff; border-left: 1px solid #cfd8dc; }
.panel h2 { font-size: 13px; text-transform: uppercase; letter-spacing: 0.04em;
  color: #546e7a; margin: 14px 0 6px; }
.detail .d-title { font-weight: 700; font-size: 14px; margin-bottom: 4px; }
.detail .d-meta { font-size: 12px; color: #607d8b; margin-bottom: 8px; }
.detail .d-stmt { white-space: pre-wrap; font-size: 13px; line-height: 1.4;
  background: #f0f3f5; padding: 8px; border-radius: 6px; }
.detail.muted { color: #90a4ae; }
.legend-row { display: flex; align-items: center; gap: 8px; font-size: 12px;
  margin: 3px 0; }
.legend-swatch { width: 14px; height: 14px; border-radius: 3px; flex: 0 0 auto;
  border: 1px solid rgba(0,0,0,0.3); }
.legend-line { width: 20px; height: 0; border-top: 3px solid; flex: 0 0 auto; }
.empty { padding: 40px; color: #607d8b; }
code { background: rgba(0,0,0,0.08); padding: 1px 4px; border-radius: 3px; }
@media (prefers-color-scheme: dark) {
  body { color: #eceff1; background: #1c1f22; }
  .panel { background: #263238; border-left-color: #37474f; }
  .detail .d-stmt { background: #1c2429; }
}
"""

_JS = r"""
(function () {
  var data = JSON.parse(document.getElementById('graph-data').textContent);
  var STATUS = data.status_colors || {};
  var EDGES = data.edge_colors || {};
  var STATUS_FALLBACK = '#607d8b';
  var EDGE_FALLBACK = '#90a4ae';

  var byId = {};
  data.nodes.forEach(function (n) { byId[n.id] = n; });

  // Canvas edges.
  var canvas = document.getElementById('edges');
  canvas.width = data.width;
  canvas.height = data.height;
  var nodesLayer = document.getElementById('nodes');
  nodesLayer.style.width = data.width + 'px';
  nodesLayer.style.height = data.height + 'px';
  var ctx = canvas.getContext('2d');
  var NW = 160, NH = 44;
  data.edges.forEach(function (e) {
    var s = byId[e.source], t = byId[e.target];
    if (!s || !t) return;
    var x1 = s.x + NW / 2, y1 = s.y + NH / 2;
    var x2 = t.x + NW / 2, y2 = t.y + NH / 2;
    ctx.strokeStyle = EDGES[e.kind] || EDGE_FALLBACK;
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(x1, y1);
    ctx.lineTo(x2, y2);
    ctx.stroke();
    // Arrow head at the target.
    var ang = Math.atan2(y2 - y1, x2 - x1);
    var hx = x2 - Math.cos(ang) * (NH / 2 + 2);
    var hy = y2 - Math.sin(ang) * (NH / 2 + 2);
    ctx.fillStyle = EDGES[e.kind] || EDGE_FALLBACK;
    ctx.beginPath();
    ctx.moveTo(hx, hy);
    ctx.lineTo(hx - Math.cos(ang - 0.4) * 9, hy - Math.sin(ang - 0.4) * 9);
    ctx.lineTo(hx - Math.cos(ang + 0.4) * 9, hy - Math.sin(ang + 0.4) * 9);
    ctx.closePath();
    ctx.fill();
  });

  // Node cards. All text via textContent -> injected markup is inert.
  var selected = null;
  data.nodes.forEach(function (n) {
    var el = document.createElement('div');
    el.className = 'node';
    if (n.taint === 'tainted') el.className += ' tainted';
    else if (n.taint === 'self_admitted') el.className += ' self-admitted';
    el.style.left = n.x + 'px';
    el.style.top = n.y + 'px';
    el.style.background = STATUS[n.status] || STATUS_FALLBACK;
    var kind = document.createElement('div');
    kind.className = 'n-kind';
    kind.textContent = n.kind + (n.taint !== 'clean' ? ' ⚠' : '');
    var title = document.createElement('div');
    title.className = 'n-title';
    title.textContent = n.title;
    el.appendChild(kind);
    el.appendChild(title);
    el.addEventListener('click', function () {
      if (selected) selected.classList.remove('selected');
      selected = el;
      el.classList.add('selected');
      showDetail(n);
    });
    nodesLayer.appendChild(el);
  });

  function showDetail(n) {
    var d = document.getElementById('detail');
    d.className = 'detail';
    d.textContent = '';
    var t = document.createElement('div');
    t.className = 'd-title';
    t.textContent = n.title;
    var meta = document.createElement('div');
    meta.className = 'd-meta';
    meta.textContent = n.kind + ' · ' + n.status + ' · taint: ' + n.taint
      + (n.tier ? ' · ' + n.tier : '');
    var stmt = document.createElement('div');
    stmt.className = 'd-stmt';
    stmt.textContent = n.statement;
    d.appendChild(t);
    d.appendChild(meta);
    d.appendChild(stmt);
  }

  // Legend, built from the shipped palettes.
  var legend = document.getElementById('legend');
  function swatch(color, label, isLine) {
    var row = document.createElement('div');
    row.className = 'legend-row';
    var sw = document.createElement('span');
    sw.className = isLine ? 'legend-line' : 'legend-swatch';
    if (isLine) sw.style.borderTopColor = color; else sw.style.background = color;
    var lb = document.createElement('span');
    lb.textContent = label;
    row.appendChild(sw);
    row.appendChild(lb);
    legend.appendChild(row);
  }
  var h1 = document.createElement('div'); h1.textContent = 'Node status';
  h1.style.fontWeight = '600'; h1.style.margin = '6px 0 2px'; legend.appendChild(h1);
  Object.keys(STATUS).forEach(function (k) { swatch(STATUS[k], k, false); });
  var h2 = document.createElement('div'); h2.textContent = 'Edge kind';
  h2.style.fontWeight = '600'; h2.style.margin = '8px 0 2px'; legend.appendChild(h2);
  Object.keys(EDGES).forEach(function (k) { swatch(EDGES[k], k, true); });
  var h3 = document.createElement('div'); h3.textContent = 'Taint';
  h3.style.fontWeight = '600'; h3.style.margin = '8px 0 2px'; legend.appendChild(h3);
  var tr = document.createElement('div'); tr.className = 'legend-row';
  tr.textContent = 'dashed border = tainted / self-admitted (⚠)';
  legend.appendChild(tr);
})();
"""
