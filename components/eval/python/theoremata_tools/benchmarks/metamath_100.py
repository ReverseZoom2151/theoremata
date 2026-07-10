"""Metamath Proof Explorer corpus loader.

The loader indexes checked ``.mm`` databases supplied by the user.  The
Metamath 100 page is a catalogue, not a proof database; actual verification is
delegated to the configured Metamath executable/backend.
"""
from __future__ import annotations

import re
import hashlib
from typing import Any

from .resources import find_files, rel
from .schema import make_item

_PROOF = re.compile(r"^\s*([A-Za-z0-9_.-]+)\s+\$p\s+(.+?)\s+\$=", re.MULTILINE)
_INCLUDE = re.compile(r"\$\[\s*([^\s]+)\s*\$\]")


def load_metamath_100() -> list[dict[str, Any]]:
    items: list[dict[str, Any]] = []
    for path in find_files("set.mm", "set.mm/**/*.mm", "metamath*/**/*.mm", "*metamath*/**/*.mm"):
        text = path.read_text(encoding="utf-8", errors="replace")
        includes = sorted(set(_INCLUDE.findall(text)))
        source_sha256 = hashlib.sha256(text.encode("utf-8")).hexdigest()
        for label, statement in _PROOF.findall(text):
            items.append(make_item(
                id=f"metamath:{label}", kind="formalization", informal=f"Metamath theorem {label}",
                formal=statement.strip(), expected={"mode": "metamath_verify", "gold_present": True},
                grading={"track": "formalization", "method": "metamath_verify", "auto_gradable": True},
                provenance={"corpus": "metamath_100", "path": rel(path), "label": label,
                            "includes": includes, "source_sha256": source_sha256},
            ))
    return items
