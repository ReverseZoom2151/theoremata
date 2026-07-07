"""Compilable lemma cache — a DB-free store of reusable proved lemmas.

Ported/adapted from mathcode's `lib_search.py` `Stored.lean` schema. Each proved
lemma is persisted as a ``-- @stored-theorem``-delimited block inside a single
``Stored.lean``-style file. Because every block is real Lean, the aggregate file
*compiles*: the formalizer can ``import`` it and reuse a lemma with a plain
``exact``/``apply`` instead of re-deriving it. Around that compilable payload we
carry a little metadata (provenance, timestamp, and a normalized statement key
used for lookup / de-duplication).

On-disk block schema (one per entry)::

    -- @stored-theorem <Name>
    -- Original: <original name>
    -- Source:   <where it came from>
    -- Proved:   <ISO-8601 timestamp>
    -- Key:      <normalized statement key>
    theorem <Name> <statement> :=
      <proof>
    -- @end-stored-theorem

JSON-able entry contract (``dict`` with ``str`` values)::

    {
      "name":        str,   # Lean theorem name (unique within the file)
      "statement":   str,   # signature between the name and ':=' (binders + ': type')
      "proof":       str,   # proof term / tactic block after ':=' (may start with 'by')
      "original_name": str, # upstream name this was distilled from ("" if n/a)
      "source":      str,   # provenance (problem id, module, url, ...)
      "proved_at":   str,   # ISO-8601 UTC timestamp
      "key":         str,   # normalized statement key (see `normalize_key`)
    }

This module is standard-library only and imports no other Theoremata code, so it
is safe to use anywhere in the harness.
"""
from __future__ import annotations

import datetime as _dt
import json
import os
import re
import sys
from typing import Any

# Delimiters for a stored-theorem block. Kept identical in spirit to mathcode so
# the on-disk format is interoperable with its `lib_search` reader.
_BEGIN = "-- @stored-theorem"
_END = "-- @end-stored-theorem"

STORED_THEOREM_BLOCK_RE = re.compile(
    r"-- @stored-theorem[ \t]+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\b[^\n]*\n"
    r"(?:--[ \t]*Original:[ \t]*(?P<original>[^\n]*)\n)?"
    r"(?:--[ \t]*Source:[ \t]*(?P<source>[^\n]*)\n)?"
    r"(?:--[ \t]*Proved:[ \t]*(?P<proved>[^\n]*)\n)?"
    r"(?:--[ \t]*Key:[ \t]*(?P<key>[^\n]*)\n)?"
    r"theorem[ \t]+(?P=name)\s+(?P<type_and_body>.*?)\n"
    r"-- @end-stored-theorem",
    re.DOTALL,
)


def _now() -> str:
    return _dt.datetime.now(_dt.timezone.utc).replace(microsecond=0).isoformat()


def normalize_key(statement: str) -> str:
    """Return a whitespace-normalized key for a lemma statement.

    Collapses all runs of whitespace to a single space and strips a trailing
    ``:=`` (in case a full signature-with-assign was passed). This is the key on
    which we look up / de-duplicate: two lemmas with the same normalized
    statement are considered the same reusable fact.
    """
    text = statement.strip()
    text = re.sub(r":=\s*$", "", text).strip()
    text = re.sub(r"\s+", " ", text)
    return text


def make_entry(
    name: str,
    statement: str,
    proof: str,
    *,
    original_name: str = "",
    source: str = "",
    proved_at: str | None = None,
    key: str | None = None,
) -> dict[str, str]:
    """Build a JSON-able cache entry (does not touch disk).

    `statement` is everything between the theorem name and ``:=`` (binders plus
    ``: <type>``); `proof` is the term or tactic block that follows ``:=``.
    """
    if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
        raise ValueError(f"{name!r} is not a legal Lean identifier")
    statement = statement.strip()
    proof = proof.strip()
    if not statement:
        raise ValueError("statement must be non-empty")
    if not proof:
        raise ValueError("proof must be non-empty")
    return {
        "name": name,
        "statement": statement,
        "proof": proof,
        "original_name": original_name.strip(),
        "source": source.strip(),
        "proved_at": proved_at or _now(),
        "key": key if key is not None else normalize_key(statement),
    }


def render_block(entry: dict[str, str]) -> str:
    """Render a single entry as a compilable ``-- @stored-theorem`` block."""
    proof = entry["proof"].strip()
    # Indent a multi-line proof body by two spaces so it reads as a proof block.
    proof_lines = proof.splitlines() or [""]
    if len(proof_lines) == 1:
        proof_render = f"  {proof_lines[0]}"
    else:
        proof_render = "\n".join("  " + ln if ln.strip() else ln for ln in proof_lines)
    return (
        f"{_BEGIN} {entry['name']}\n"
        f"-- Original: {entry.get('original_name', '')}\n"
        f"-- Source:   {entry.get('source', '')}\n"
        f"-- Proved:   {entry.get('proved_at', '')}\n"
        f"-- Key:      {entry.get('key', normalize_key(entry['statement']))}\n"
        f"theorem {entry['name']} {entry['statement']} :=\n"
        f"{proof_render}\n"
        f"{_END}"
    )


def render_file(entries: list[dict[str, str]], *, namespace: str | None = None) -> str:
    """Render the aggregate ``Stored.lean``-style file for a list of entries.

    When `namespace` is given, the blocks are wrapped in ``namespace``/``end`` so
    reuse hints read ``exact <Namespace>.<name>``.
    """
    header = (
        "-- Theoremata compilable lemma cache (auto-generated).\n"
        "-- Each block below is real Lean and doubles as reusable, importable proof.\n"
        "-- Do not edit by hand; use theoremata_tools.lemma_cache.\n"
    )
    blocks = "\n\n".join(render_block(e) for e in entries)
    if namespace:
        body = f"namespace {namespace}\n\n{blocks}\n\nend {namespace}\n"
    else:
        body = f"{blocks}\n" if blocks else ""
    return f"{header}\n{body}"


def parse_file(text: str) -> list[dict[str, str]]:
    """Parse a ``Stored.lean``-style file back into entries."""
    entries: list[dict[str, str]] = []
    for m in STORED_THEOREM_BLOCK_RE.finditer(text):
        g = m.groupdict()
        statement, proof = _split_statement_and_proof(g["type_and_body"])
        entries.append(
            {
                "name": g["name"].strip(),
                "statement": statement,
                "proof": proof,
                "original_name": (g["original"] or "").strip(),
                "source": (g["source"] or "").strip(),
                "proved_at": (g["proved"] or "").strip(),
                "key": (g["key"] or "").strip() or normalize_key(statement),
            }
        )
    return entries


def _split_statement_and_proof(type_and_body: str) -> tuple[str, str]:
    """Split ``<statement> := <proof>`` at the top-level ``:=`` (paren-aware)."""
    depth = 0
    i = 0
    n = len(type_and_body)
    while i < n:
        ch = type_and_body[i]
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth = max(depth - 1, 0)
        elif ch == ":" and depth == 0 and i + 1 < n and type_and_body[i + 1] == "=":
            statement = type_and_body[:i].strip()
            proof = type_and_body[i + 2 :].strip()
            return statement, proof
        i += 1
    return type_and_body.strip(), ""


class LemmaCache:
    """A file-backed compilable lemma cache.

    The store is a single ``Stored.lean``-style file. All mutating operations
    round-trip through `render_file`, so the on-disk artifact always compiles.
    Lookups are keyed on the normalized statement (`normalize_key`); appending an
    entry whose key already exists is idempotent unless ``overwrite=True``.
    """

    def __init__(self, path: str, *, namespace: str | None = None) -> None:
        self.path = path
        self.namespace = namespace

    # -- persistence -----------------------------------------------------
    def load(self) -> list[dict[str, str]]:
        if not os.path.exists(self.path):
            return []
        with open(self.path, encoding="utf-8") as fh:
            return parse_file(fh.read())

    def _save(self, entries: list[dict[str, str]]) -> None:
        parent = os.path.dirname(os.path.abspath(self.path))
        os.makedirs(parent, exist_ok=True)
        with open(self.path, "w", encoding="utf-8") as fh:
            fh.write(render_file(entries, namespace=self.namespace))

    # -- public API ------------------------------------------------------
    def append(self, entry: dict[str, str], *, overwrite: bool = False) -> bool:
        """Append a proved lemma. Returns True if the store changed.

        De-duplicates on the normalized key: a second lemma with the same key is
        skipped (idempotent) unless ``overwrite`` replaces the existing entry.
        """
        entry = dict(entry)
        entry.setdefault("key", normalize_key(entry["statement"]))
        entry.setdefault("proved_at", _now())
        entries = self.load()
        for idx, existing in enumerate(entries):
            same = existing["key"] == entry["key"] or existing["name"] == entry["name"]
            if same:
                if overwrite:
                    entries[idx] = entry
                    self._save(entries)
                    return True
                return False
        entries.append(entry)
        self._save(entries)
        return True

    def append_lemma(
        self,
        name: str,
        statement: str,
        proof: str,
        *,
        original_name: str = "",
        source: str = "",
        overwrite: bool = False,
    ) -> dict[str, str]:
        """Build and append an entry; returns the stored entry."""
        entry = make_entry(
            name,
            statement,
            proof,
            original_name=original_name,
            source=source,
        )
        self.append(entry, overwrite=overwrite)
        return entry

    def get(self, key: str) -> dict[str, str] | None:
        """Retrieve an entry by (raw or normalized) statement key or by name."""
        nkey = normalize_key(key)
        for entry in self.load():
            if entry["key"] == nkey or entry["key"] == key or entry["name"] == key:
                return entry
        return None

    def list(self) -> list[dict[str, str]]:
        """Return all cached entries."""
        return self.load()

    def render(self) -> str:
        """Render the aggregate compilable file (as persisted)."""
        return render_file(self.load(), namespace=self.namespace)

    def usage_hint(self, entry: dict[str, str]) -> str:
        """Ready-to-paste reuse hint for the formalizer."""
        prefix = f"{self.namespace}." if self.namespace else ""
        return f"exact {prefix}{entry['name']} <args>"


# --- worker / CLI entrypoint -------------------------------------------------
def run(request: dict[str, Any]) -> dict[str, Any]:
    """JSON dispatch used by the worker.

    Operations (``op``): ``append``, ``get``, ``list``, ``render``.
    """
    op = request.get("op", "list")
    path = request["path"]
    cache = LemmaCache(path, namespace=request.get("namespace"))

    if op == "append":
        entry = cache.append_lemma(
            request["name"],
            request["statement"],
            request["proof"],
            original_name=request.get("original_name", ""),
            source=request.get("source", ""),
            overwrite=bool(request.get("overwrite", False)),
        )
        return {"ok": True, "op": op, "entry": entry, "usage": cache.usage_hint(entry)}
    if op == "get":
        entry = cache.get(request["key"])
        return {
            "ok": True,
            "op": op,
            "found": entry is not None,
            "entry": entry,
            "usage": cache.usage_hint(entry) if entry else None,
        }
    if op == "list":
        entries = cache.list()
        return {"ok": True, "op": op, "count": len(entries), "entries": entries}
    if op == "render":
        return {"ok": True, "op": op, "text": cache.render()}
    raise ValueError(f"unknown lemma_cache op: {op}")


def main() -> None:
    request = json.load(sys.stdin)
    try:
        response = run(request)
    except Exception as exc:  # noqa: BLE001 - surface as JSON
        response = {"ok": False, "error": str(exc)}
    print(json.dumps(response))
    raise SystemExit(0 if response.get("ok") else 1)


if __name__ == "__main__":
    main()
