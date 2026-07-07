"""Corpus-specific parsers for benchmark loaders (Tier 4)."""
from __future__ import annotations

import re
from typing import Any

_THEOREM_HEADER = re.compile(
    r"(?m)^\s*(theorem|lemma|def)\s+([A-Za-z0-9_.«»]+)\s+([^:]*:\s*[^:=]+)",
)
_PROVENANCE_UUID = re.compile(
    r"(?i)request\s*(?:id|uuid)\s*[:=]\s*([0-9a-f-]{36})",
)
_PROVENANCE_LEAN = re.compile(r"(?i)lean\s*(?:version)?\s*[:=]\s*([^\n]+)")
_BLOCK_COMMENT = re.compile(r"/-.*?-/", re.DOTALL)
_LINE_COMMENT = re.compile(r"--.*?$", re.MULTILINE)


def extract_lean_headers(lean_src: str) -> list[dict[str, str]]:
    """Snapshot theorem/lemma/def signature headers (open-atp statement guard)."""
    cleaned = _BLOCK_COMMENT.sub(" ", lean_src)
    cleaned = _LINE_COMMENT.sub(" ", cleaned)
    out: list[dict[str, str]] = []
    for m in _THEOREM_HEADER.finditer(cleaned):
        kind, name, sig = m.group(1), m.group(2), m.group(3).strip()
        out.append({"kind": kind, "name": name, "signature": _normalize_ws(sig)})
    return out


def parse_external_provenance(lean_src: str) -> dict[str, Any]:
    """Best-effort provenance block from Aristotle/Harmonic output headers."""
    head = lean_src[:4000]
    prov: dict[str, Any] = {}
    if m := _PROVENANCE_UUID.search(head):
        prov["request_id"] = m.group(1)
    if m := _PROVENANCE_LEAN.search(head):
        prov["lean_version"] = m.group(1).strip()
    return prov


def extract_problem_comment(lean_src: str) -> str:
    """First large `/- ... -/` block, often the informal problem statement."""
    m = re.search(r"/-\s*(.*?)\s*-/", lean_src, re.DOTALL)
    return (m.group(1).strip() if m else "")[:8000]


def _normalize_ws(s: str) -> str:
    return re.sub(r"\s+", " ", s).strip()