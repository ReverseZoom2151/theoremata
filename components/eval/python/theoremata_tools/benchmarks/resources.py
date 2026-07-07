"""Locate benchmark corpora under ``resources/`` at runtime.

Corpora are ingested by *glob*, never hardcoded, so a missing corpus degrades
gracefully (the loader returns ``[]`` and logs a skip). The resource root is,
in priority order:

1. ``$THEOREMATA_RESOURCES`` (explicit override), else
2. ``<repo-root>/resources`` where repo-root is derived from this file's path.

Many corpora are checked out with a *doubled* directory name (e.g.
``FormalQualBench-main/FormalQualBench-main``); callers pass a glob that tolerates
that, so we never assume the nesting depth.
"""
from __future__ import annotations

import glob
import logging
import os
from pathlib import Path

log = logging.getLogger("theoremata.benchmarks")

# this file: components/eval/python/theoremata_tools/benchmarks/resources.py
# parents:   [0]=benchmarks [1]=theoremata_tools [2]=python [3]=eval
#            [4]=components  [5]=<repo root>
_REPO_ROOT = Path(__file__).resolve().parents[5]


def resource_root() -> Path:
    """Return the ``resources/`` directory (may not exist)."""
    override = os.environ.get("THEOREMATA_RESOURCES")
    if override:
        return Path(override)
    return _REPO_ROOT / "resources"


def find_files(*patterns: str) -> list[Path]:
    """Return every path matching any of the ``patterns`` (recursive globs),
    sorted and de-duplicated. Patterns are relative to :func:`resource_root`."""
    root = resource_root()
    seen: dict[str, Path] = {}
    for pat in patterns:
        for hit in glob.glob(str(root / pat), recursive=True):
            p = Path(hit)
            if p.is_file():
                seen[str(p.resolve())] = p
    return [seen[k] for k in sorted(seen)]


def find_dir(*patterns: str) -> Path | None:
    """Return the first directory matching any pattern, else ``None``."""
    root = resource_root()
    for pat in patterns:
        for hit in sorted(glob.glob(str(root / pat), recursive=True)):
            p = Path(hit)
            if p.is_dir():
                return p
    return None


def rel(path: Path) -> str:
    """Best-effort path relative to the repo root for provenance records."""
    try:
        return str(path.resolve().relative_to(_REPO_ROOT))
    except ValueError:
        return str(path)
