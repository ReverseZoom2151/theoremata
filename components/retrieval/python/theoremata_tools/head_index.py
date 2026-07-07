"""Retrieval Layer C: a head-symbol index over Lean declarations.

Approximates Mathlib's ``#find`` (``Mathlib/Tactic/Find.lean``): bucket every
declaration by the head symbol of its *conclusion* — the target reached after
stripping leading binders (``∀``/``Π``) and hypotheses (``… →``). Retrieval by
head symbol is the type-aware layer that sits on top of the declaration dump
(Layer B), so a goal of shape ``_ ≤ _`` can be answered with the lemmas whose
conclusion head is ``LE.le``.

The head extraction is a purely textual heuristic over pretty-printed types, so
the core logic is testable without Lean; ``run`` additionally shells out to the
companion ``dump_types.lean`` meta-program to obtain the types.
"""
from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
from collections import Counter
from pathlib import Path
from typing import Any

_SCRIPT = Path(__file__).resolve().parents[2] / "lean" / "dump_types.lean"

# Outermost logical connective / relation, lowest precedence first, so the first
# top-level match is the head of the conclusion. Maps Lean notation to the
# underlying declaration head that `#find` would index by.
_RELATIONS: list[tuple[str, str]] = [
    ("↔", "Iff"),
    ("∨", "Or"),
    ("∧", "And"),
    ("=", "Eq"),
    ("≠", "Ne"),
    ("≤", "LE.le"),
    ("<", "LT.lt"),
    ("≥", "GE.ge"),
    (">", "GT.gt"),
    ("∈", "Membership.mem"),
    ("∉", "Membership.mem"),
    ("∣", "Dvd.dvd"),
    ("≡", "HEq"),
]

_OPEN = "([{⟨"
_CLOSE = ")]}⟩"
_IDENT = re.compile(r"[A-Za-z_À-￿][\w.'À-￿]*")


def _top_level_find(s: str, sub: str) -> int | None:
    """Index of the first occurrence of `sub` at bracket depth zero, else None."""
    depth = 0
    i = 0
    n = len(s)
    m = len(sub)
    while i < n:
        c = s[i]
        if c in _OPEN:
            depth += 1
        elif c in _CLOSE:
            depth = max(0, depth - 1)
        elif depth == 0 and s.startswith(sub, i):
            return i
        i += 1
    return None


def _strip_outer_parens(c: str) -> str:
    while len(c) >= 2 and c[0] == "(" and c[-1] == ")":
        depth = 0
        matched = True
        for i, ch in enumerate(c):
            if ch in _OPEN:
                depth += 1
            elif ch in _CLOSE:
                depth -= 1
                if depth == 0 and i != len(c) - 1:
                    matched = False
                    break
        if matched:
            c = c[1:-1].strip()
        else:
            break
    return c


def conclusion(type_str: str) -> str:
    """Strip leading dependent binders and hypotheses to reach the target."""
    s = type_str.strip()
    for _ in range(1000):
        s = s.strip()
        if not s:
            break
        if s[0] in "∀Π":
            comma = _top_level_find(s, ",")
            if comma is not None:
                s = s[comma + 1 :]
                continue
            break
        arrow = _top_level_find(s, "→")
        if arrow is not None:
            s = s[arrow + len("→") :]
            continue
        break
    return s.strip()


def head_symbol(type_str: str) -> str:
    """The head symbol of the conclusion of a pretty-printed Lean type."""
    c = conclusion(type_str)
    if not c:
        return ""
    if c[0] == "¬":
        return "Not"
    if c[0] == "∃":
        return "Exists"
    if c[0] == "Σ":
        return "Sigma"
    c = _strip_outer_parens(c)
    for op, head in _RELATIONS:
        if _top_level_find(c, f" {op} ") is not None:
            return head
    c2 = c.lstrip("(").strip()
    m = _IDENT.match(c2)
    if m:
        return m.group(0)
    return c2.split()[0] if c2.split() else c


def build_head_index(decls: list[dict[str, Any]]) -> dict[str, Any]:
    """Bucket declarations (each carrying a `type` string) by conclusion head."""
    heads: dict[str, list[str]] = {}
    conclusions: dict[str, str] = {}
    for d in decls:
        ty = d.get("type")
        name = d.get("name")
        if not ty or not name:
            continue
        head = head_symbol(ty)
        heads.setdefault(head, []).append(name)
        conclusions[name] = conclusion(ty)
    return {"heads": heads, "conclusions": conclusions, "count": len(conclusions)}


def by_head(index: dict[str, Any], head: str, limit: int = 50) -> list[str]:
    return index.get("heads", {}).get(head, [])[:limit]


def search_conclusion(decls: list[dict[str, Any]], pattern: str, limit: int = 50) -> list[str]:
    needle = pattern.lower()
    out = []
    for d in decls:
        ty = d.get("type", "")
        if needle in conclusion(ty).lower():
            out.append(d.get("name"))
            if len(out) >= limit:
                break
    return out


def _resolve(name: str, override: str | None = None) -> str | None:
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


def dump_types(
    root: str | None,
    imports: list[str] | None,
    lean_bin: str | None = None,
    timeout: float = 300.0,
) -> dict[str, Any]:
    """Run `dump_types.lean` to obtain per-declaration `{name,kind,module,type}`."""
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
            cmd, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            encoding="utf-8", errors="replace",
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
        return {"ok": False, "count": 0, "decls": [], "stderr": f"lean dump timed out after {timeout}s"}
    decls: list[dict[str, Any]] = []
    for line in out.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            decls.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return {"ok": len(decls) > 0, "count": len(decls), "decls": decls, "stderr": (err or "").strip()}


def run(
    root: str | None = None,
    imports: list[str] | None = None,
    query: str = "stats",
    head: str | None = None,
    pattern: str | None = None,
    limit: int = 50,
    lean_bin: str | None = None,
    timeout: float = 300.0,
) -> dict[str, Any]:
    dumped = dump_types(root, imports, lean_bin=lean_bin, timeout=timeout)
    if not dumped["ok"]:
        return dumped
    decls = dumped["decls"]
    index = build_head_index(decls)
    if query == "stats":
        buckets = sorted(
            ((h, len(ns)) for h, ns in index["heads"].items()),
            key=lambda x: x[1], reverse=True,
        )
        return {
            "ok": True, "query": "stats", "count": index["count"],
            "heads": len(index["heads"]),
            "largest": [{"head": h, "size": s} for h, s in buckets[:limit]],
        }
    if query == "by_head":
        matches = by_head(index, head or "", limit)
    elif query == "search_conclusion":
        matches = search_conclusion(decls, pattern or "", limit)
    else:
        return {"ok": False, "stderr": f"unknown query: {query}"}
    return {"ok": True, "query": query, "count": len(matches), "matches": matches}


def main() -> None:
    if len(sys.argv) >= 2 and os.path.exists(sys.argv[1]):
        with open(sys.argv[1], encoding="utf-8") as fh:
            req = json.load(fh)
    else:
        req = json.load(sys.stdin)
    result = run(
        root=req.get("root"),
        imports=req.get("imports"),
        query=req.get("query", "stats"),
        head=req.get("head"),
        pattern=req.get("pattern"),
        limit=int(req.get("limit", 50)),
        lean_bin=req.get("lean_bin"),
        timeout=float(req.get("timeout", 300.0)),
    )
    print(json.dumps(result))
    raise SystemExit(0 if result.get("ok") else 1)


if __name__ == "__main__":
    main()
