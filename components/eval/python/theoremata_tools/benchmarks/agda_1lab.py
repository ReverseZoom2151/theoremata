"""1Lab/Agda corpus loader.

1Lab is an Agda library rather than a standalone prover.  This loader indexes
Agda modules when a checkout is placed under ``resources/``; it never treats
documentation text as a proof and degrades to an empty corpus when absent.
"""
from __future__ import annotations

import re
import hashlib
from typing import Any

from .resources import find_files, rel
from .schema import AXIOMS_WHITELIST, make_item

_MODULE = re.compile(r"^\s*module\s+([^\s]+)\s+where", re.MULTILINE)
_IMPORT = re.compile(r"^\s*(?:open\s+)?import\s+([^\s;]+)", re.MULTILINE)


def load_1lab() -> list[dict[str, Any]]:
    files = find_files("1lab*/**/*.agda", "*1Lab*/**/*.agda", "agda*/**/*.agda")
    items: list[dict[str, Any]] = []
    for path in files:
        text = path.read_text(encoding="utf-8", errors="replace")
        match = _MODULE.search(text)
        module = match.group(1) if match else path.stem
        imports = sorted(set(_IMPORT.findall(text)))
        source_sha256 = hashlib.sha256(text.encode("utf-8")).hexdigest()
        items.append(make_item(
            id=f"1lab:{module}", kind="formalization",
            informal=f"Agda module {module}", formal=None,
            expected={"mode": "agda_typecheck", "gold_present": False,
                      "axioms_whitelist": list(AXIOMS_WHITELIST)},
            grading={"track": "formalization", "method": "agda_typecheck",
                     "auto_gradable": False},
            provenance={"corpus": "1lab", "path": rel(path), "module": module,
                        "imports": imports, "source_sha256": source_sha256},
        ))
    return items
