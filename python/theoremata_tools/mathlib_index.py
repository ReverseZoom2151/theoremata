"""Mathlib retrieval Layer A: an offline module import-DAG index.

Builds a dependency graph over a Lean source tree purely by scanning `import`
lines, exploiting the bijection between a file's path and its module name
(`Mathlib/Algebra/Group/Defs.lean` == module `Mathlib.Algebra.Group.Defs` ==
the identifier written as `import Mathlib.Algebra.Group.Defs`).

Standard-library only; no build, no compiler, no third-party deps. This is the
cheap structured substrate that later layers (env-dump declaration index, warm
`#find` process) refine — it is deliberately source-only so it works on an
unbuilt checkout of 8000+ files.

Ports the idea of Mathlib's own `scripts/dag_traversal.py` (`DAG.from_directories`).
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path
from typing import Any

# Matches a top-of-file import line, tolerating leading whitespace and the
# Lean 4 `public import` form. A line comment (`-- import ...`) never matches
# because the `--` precedes `import`, so it fails the `^\s*import` anchor; block
# comments are handled separately by _iter_imports.
_IMPORT_RE = re.compile(r"^\s*(?:public\s+)?import\s+([A-Za-z0-9_.]+)")


def _iter_imports(path: Path):
    """Yield imported module names from a Lean file, skipping comments.

    Line-granular block-comment tracking (`/- ... -/`) plus stripping any `--`
    line-comment tail keeps `import` tokens that live inside comments from being
    counted, while staying cheap enough for a whole Mathlib tree.
    """
    depth = 0
    with open(path, encoding="utf-8", errors="replace") as fh:
        for raw in fh:
            if depth > 0:
                depth += raw.count("/-") - raw.count("-/")
                if depth < 0:
                    depth = 0
                continue
            code = raw.split("--", 1)[0]
            match = _IMPORT_RE.match(code)
            if match:
                yield match.group(1)
            depth += code.count("/-") - code.count("-/")
            if depth < 0:
                depth = 0


def build_index(root: str, package: str = "Mathlib") -> dict[str, Any]:
    """Scan `root` for `*.lean` files and build the module import DAG.

    Module names are derived relative to `root` (so they include the `package`
    prefix and match `import` identifiers). If `root/package` exists, only that
    subtree is scanned. Returns a dict with `modules` (sorted), `imports`
    (module -> sorted in-tree imports), `imported_by` (reverse adjacency), and
    `external` (module -> sorted imports that are not in-tree, e.g. `Init.*`).
    """
    root_path = Path(root)
    search_root = root_path / package
    if not search_root.is_dir():
        search_root = root_path

    raw_imports: dict[str, list[str]] = {}
    for path in search_root.rglob("*.lean"):
        if not path.is_file():
            continue
        rel = path.relative_to(root_path).with_suffix("")
        module = ".".join(rel.parts)
        raw_imports[module] = list(_iter_imports(path))

    module_set = set(raw_imports)
    imports: dict[str, list[str]] = {}
    external: dict[str, list[str]] = {}
    imported_by: dict[str, set[str]] = {m: set() for m in module_set}

    for module, imps in raw_imports.items():
        in_tree = sorted({i for i in imps if i in module_set})
        ext = sorted({i for i in imps if i not in module_set})
        imports[module] = in_tree
        if ext:
            external[module] = ext
        for target in in_tree:
            imported_by[target].add(module)

    return {
        "package": package,
        "root": str(root_path),
        "modules": sorted(module_set),
        "imports": imports,
        "imported_by": {m: sorted(s) for m, s in imported_by.items()},
        "external": external,
    }


def direct_imports(index: dict[str, Any], module: str) -> list[str]:
    return index["imports"].get(module, [])


def importers(index: dict[str, Any], module: str) -> list[str]:
    return index["imported_by"].get(module, [])


def _closure(adjacency: dict[str, list[str]], module: str) -> list[str]:
    seen: set[str] = set()
    stack = list(adjacency.get(module, []))
    while stack:
        node = stack.pop()
        if node in seen:
            continue
        seen.add(node)
        stack.extend(adjacency.get(node, []))
    return sorted(seen)


def transitive_imports(index: dict[str, Any], module: str) -> list[str]:
    return _closure(index["imports"], module)


def transitive_importers(index: dict[str, Any], module: str) -> list[str]:
    return _closure(index["imported_by"], module)


def search(index: dict[str, Any], substring: str, limit: int = 50) -> list[str]:
    needle = (substring or "").lower()
    hits = [m for m in index["modules"] if needle in m.lower()]
    return hits[:limit]


_MODULE_QUERIES = {
    "direct_imports": direct_imports,
    "transitive_imports": transitive_imports,
    "importers": importers,
    "transitive_importers": transitive_importers,
}


def run(
    root: str,
    query: str = "stats",
    module: str | None = None,
    substring: str | None = None,
    limit: int = 50,
    package: str = "Mathlib",
) -> dict[str, Any]:
    """Build the index and answer one query. Offline / one-shot (no caching yet)."""
    index = build_index(root, package)

    if query == "stats":
        edges = sum(len(v) for v in index["imports"].values())
        ranked = sorted(
            index["imported_by"].items(),
            key=lambda kv: (-len(kv[1]), kv[0]),
        )
        top = [
            {"module": m, "importers": len(imps)}
            for m, imps in ranked[:10]
            if imps
        ]
        return {
            "query": "stats",
            "modules": len(index["modules"]),
            "edges": edges,
            "external_modules": len(index["external"]),
            "most_imported": top,
        }

    if query == "search":
        return {
            "query": "search",
            "substring": substring,
            "matches": search(index, substring or "", limit),
        }

    handler = _MODULE_QUERIES.get(query)
    if handler is None:
        raise ValueError(f"unknown query: {query}")
    if module is None:
        raise ValueError(f"query {query!r} requires a 'module'")
    return {"query": query, "module": module, "result": handler(index, module)}


def main() -> None:
    if len(sys.argv) >= 2:
        with open(sys.argv[1], encoding="utf-8") as fh:
            request = json.load(fh)
    else:
        request = json.load(sys.stdin)
    result = run(
        root=request["root"],
        query=request.get("query", "stats"),
        module=request.get("module"),
        substring=request.get("substring"),
        limit=int(request.get("limit", 50)),
        package=request.get("package", "Mathlib"),
    )
    print(json.dumps(result))
    raise SystemExit(0)


if __name__ == "__main__":
    main()
