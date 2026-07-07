"""Retrieval Layer B: dump declaration metadata from a Lean environment.

Runs the companion `dump_decls.lean` meta-program via `lean --run` (or, for a
Lake project such as Mathlib, `lake env lean --run` inside the project root),
captures its JSONL output, and parses it into declaration records of the form
``{"name", "kind", "module", "is_axiom"}``. This is the type-aware retrieval
substrate that sits above the source-only import DAG (Layer A).
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from collections import Counter
from pathlib import Path
from typing import Any

_SCRIPT = Path(__file__).resolve().parents[2] / "lean" / "dump_decls.lean"


def _resolve(name: str, override: str | None = None) -> str | None:
    """Locate a Lean toolchain binary: an explicit override, then the process
    PATH, then the default elan install dir (which may be off a non-login PATH
    on Windows)."""
    if override:
        return override
    found = shutil.which(name)
    if found:
        return found
    for ext in ("", ".exe"):
        candidate = os.path.expanduser(os.path.join("~", ".elan", "bin", name + ext))
        if os.path.exists(candidate):
            return candidate
    return None


def dump(
    root: str | None,
    imports: list[str] | None,
    lean_bin: str | None = None,
    timeout: float = 300.0,
) -> dict[str, Any]:
    """Import `imports` (default ``["Init"]``) and dump every non-internal
    declaration. When `root` is a Lake project, resolve imports against its
    build via `lake env lean`; otherwise run bare `lean`. Timeout-guarded with
    SIGTERM->SIGKILL escalation."""
    imports = imports or ["Init"]
    if root:
        lake = _resolve("lake", lean_bin)
        if not lake:
            return {"ok": False, "count": 0, "decls": [], "stderr": "lake not found"}
        cmd = [lake, "env", "lean", "--run", str(_SCRIPT), *imports]
        cwd: str | None = root
    else:
        lean = _resolve("lean", lean_bin)
        if not lean:
            return {"ok": False, "count": 0, "decls": [], "stderr": "lean not found"}
        cmd = [lean, "--run", str(_SCRIPT), *imports]
        cwd = None

    try:
        proc = subprocess.Popen(
            cmd,
            cwd=cwd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            encoding="utf-8",
            errors="replace",
        )
    except FileNotFoundError as exc:
        return {"ok": False, "count": 0, "decls": [], "stderr": str(exc)}

    try:
        out, err = proc.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        proc.terminate()
        try:
            proc.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.communicate()
        return {
            "ok": False,
            "count": 0,
            "decls": [],
            "stderr": f"lean dump timed out after {timeout}s",
        }

    decls: list[dict[str, Any]] = []
    for line in out.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            decls.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return {
        "ok": len(decls) > 0,
        "count": len(decls),
        "decls": decls,
        "stderr": (err or "").strip(),
    }


def by_kind(decls: list[dict[str, Any]], kind: str) -> list[dict[str, Any]]:
    return [d for d in decls if d.get("kind") == kind]


def search(decls: list[dict[str, Any]], substring: str, limit: int = 50) -> list[dict[str, Any]]:
    needle = substring.lower()
    return [d for d in decls if needle in d.get("name", "").lower()][:limit]


def axioms(decls: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [d for d in decls if d.get("is_axiom")]


def run(
    root: str | None = None,
    imports: list[str] | None = None,
    query: str = "dump",
    kind: str | None = None,
    substring: str | None = None,
    limit: int = 50,
    lean_bin: str | None = None,
    timeout: float = 300.0,
) -> dict[str, Any]:
    result = dump(root, imports, lean_bin=lean_bin, timeout=timeout)
    if not result["ok"] or query == "dump":
        return result
    decls = result["decls"]
    if query == "stats":
        return {
            "ok": True,
            "query": "stats",
            "count": result["count"],
            "kinds": dict(Counter(d.get("kind") for d in decls)),
            "axioms": len(axioms(decls)),
        }
    if query == "by_kind":
        matches = by_kind(decls, kind or "theorem")
    elif query == "search":
        matches = search(decls, substring or "", limit)
    elif query == "axioms":
        matches = axioms(decls)
    else:
        return {"ok": False, "count": 0, "decls": [], "stderr": f"unknown query: {query}"}
    return {"ok": True, "query": query, "count": len(matches), "matches": matches[:limit]}


def main() -> None:
    if len(sys.argv) >= 2 and os.path.exists(sys.argv[1]):
        with open(sys.argv[1], encoding="utf-8") as fh:
            req = json.load(fh)
    else:
        req = json.load(sys.stdin)
    result = run(
        root=req.get("root"),
        imports=req.get("imports"),
        query=req.get("query", "dump"),
        kind=req.get("kind"),
        substring=req.get("substring"),
        limit=int(req.get("limit", 50)),
        lean_bin=req.get("lean_bin"),
        timeout=float(req.get("timeout", 300.0)),
    )
    print(json.dumps(result))
    raise SystemExit(0 if result.get("ok") else 1)


if __name__ == "__main__":
    main()
