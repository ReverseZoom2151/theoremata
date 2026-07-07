"""ReProver-style accessible-premise filtering for Mathlib retrieval.

A premise is *accessible* from a theorem context when:
1. Its defining module is imported (transitive import closure), and
2. It is not defined *after* the theorem in the same file (same-file order).

This is a lightweight lexical approximation of ReProver's ``Corpus`` visibility
rules — sufficient for retrieval ranking without vendoring the full training stack.
"""
from __future__ import annotations

from typing import Any


def _module_prefixes(module: str) -> set[str]:
    parts = (module or "").split(".")
    return {".".join(parts[: i + 1]) for i in range(len(parts))}


def filter_accessible(
    decls: list[dict[str, Any]],
    *,
    imports: list[str],
    file_path: str | None = None,
    theorem_line: int | None = None,
) -> list[dict[str, Any]]:
    """Return decls visible from ``(imports, file_path, theorem_line)``."""
    import_closure: set[str] = set()
    for imp in imports or ["Init"]:
        import_closure |= _module_prefixes(imp)
        import_closure.add(imp)

    out: list[dict[str, Any]] = []
    for d in decls:
        module = d.get("module") or ""
        if module and module not in import_closure:
            # Allow Mathlib decls when Mathlib is imported.
            if not (
                any(imp.startswith("Mathlib") for imp in import_closure)
                and (module.startswith("Mathlib") or module == "Init")
            ):
                continue
        if file_path and d.get("file") == file_path:
            start = d.get("line_start")
            if (
                theorem_line is not None
                and isinstance(start, int)
                and start > theorem_line
            ):
                continue
        out.append(d)
    return out