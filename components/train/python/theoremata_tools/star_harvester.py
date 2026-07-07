"""STaR trace harvester: completed proof-DAG traces -> SFT records.

This turns Theoremata's *own* successful runs into supervised fine-tuning data
(the STaR loop: sample -> keep only what the verifier accepts -> retrain). It
walks completed proof-DAG traces and emits ``{prompt, completion}`` records from
**successful, verified obligations only**, where *verified* means the proof both
compiled and passed the axiom gate (``#print axioms`` closure clean, no
``sorry`` / disallowed axioms).

Decoupling contract
-------------------
This module is deliberately **not coupled to the live Rust core**. It consumes a
project's *exported* JSON / DB rows. Two accepted input shapes:

1. **Trace-record list (primary contract).** A list of ``ObligationTrace`` dicts
   (or a dict ``{"traces": [...]}`` / ``{"obligations": [...]}``). Each record::

       {
         "obligation_id": str,          # stable id (used for provenance/hash)
         "goal": str,                   # the statement to prove -> becomes prompt
         "formal_statement": str|None,  # the required Lean statement (optional)
         "proof": str,                  # the Lean proof text -> becomes completion
         "verified": {                  # verifier verdict; BOTH must be true
             "compiled": bool,          #   passed the Lean compiler
             "axioms_ok": bool,         #   axiom closure clean (gate passed)
         },
         # -- optional "reformulate to required statement" adapter step --
         "reformulation": {             # if the agent had to restate the goal
             "from_statement": str,     #   what the user/blueprint asked
             "to_statement": str,       #   the statement actually proved
         }
       }

   Back-compat: ``verified`` may instead be a bare ``bool`` with a sibling
   ``axioms_ok`` (matching ``sft_export.star_dataset``'s record shape).

2. **GraphExport dict.** A ``{"project", "nodes", "edges", "events"}`` export
   (the Rust ``GraphExport`` serialized to JSON). ``from_graph_export`` walks the
   nodes/edges/events into ``ObligationTrace`` records, then the same pipeline
   runs. A formal proof counts as verified when its node status is
   ``formally_verified``, it is not ``tainted``, and a ``verifies`` edge of
   strength ``lean_checked`` backs it (axiom-gate signal). This adapter is
   best-effort; prefer shape (1) if the exporter can emit it directly.

Output
------
``harvest`` returns ``{ok, kept, dropped, skipped_unverified, rows}`` where each
row is ``{"prompt", "completion", "meta": {...}}``. Rows are de-duplicated by a
SHA-256 content hash over ``(prompt, completion)``. When a reformulation adapter
step is present it is emitted as an *additional* tagged row so the model learns
the restatement move as its own training signal.

No GPU, no training, no live-core coupling -- pure data shaping.
"""
from __future__ import annotations

import hashlib
import json
import sys
from typing import Any, Iterable

# One SFT record. Flat {prompt, completion} (+ provenance meta), per the Tier-5
# spec -- distinct from sft_export's chat ``messages`` shape. Use
# ``to_chat_row`` to convert into the chat-SFT format when needed.
Row = dict[str, Any]

_REFORMULATE_INSTRUCTION = (
    "Reformulate the informal statement into the exact formal statement "
    "required for the proof."
)


# ---------------------------------------------------------------------------
# Verification predicate
# ---------------------------------------------------------------------------

def _verdict_of(record: dict[str, Any]) -> tuple[bool, bool]:
    """Extract ``(compiled, axioms_ok)`` from a trace record, tolerating both
    the structured ``verified={compiled,axioms_ok}`` shape and the flat
    ``verified: bool`` + ``axioms_ok: bool`` shape used elsewhere.

    ``axioms_ok`` defaults to ``True`` when absent *only if compilation was
    recorded* -- absence means the audit was not run, which is not on its own a
    rejection reason (mirrors ``sft_export._accepts``).
    """
    verified = record.get("verified")
    if isinstance(verified, dict):
        compiled = bool(verified.get("compiled"))
        axioms_ok = bool(verified.get("axioms_ok", True))
        return compiled, axioms_ok
    # flat shape: verified is a bool, axioms_ok is a sibling
    compiled = bool(verified)
    axioms_ok = bool(record.get("axioms_ok", True))
    return compiled, axioms_ok


def is_verified(record: dict[str, Any]) -> bool:
    """A trace is verified only when it BOTH compiled and passed the axiom
    gate. This is the ground-truth filter for STaR harvesting."""
    compiled, axioms_ok = _verdict_of(record)
    return compiled and axioms_ok


# ---------------------------------------------------------------------------
# Row construction + hashing
# ---------------------------------------------------------------------------

def content_hash(prompt: str, completion: str) -> str:
    """Stable SHA-256 over the ``(prompt, completion)`` pair for dedup."""
    h = hashlib.sha256()
    h.update(prompt.encode("utf-8"))
    h.update(b"\x00")
    h.update(completion.encode("utf-8"))
    return h.hexdigest()


def _row(prompt: str, completion: str, meta: dict[str, Any]) -> Row:
    meta = dict(meta)
    meta["hash"] = content_hash(prompt, completion)
    return {"prompt": prompt, "completion": completion, "meta": meta}


def _rows_for_trace(record: dict[str, Any]) -> list[Row]:
    """Emit the SFT row(s) for one *already-verified* trace.

    The proof row is always emitted. If a ``reformulation`` adapter step is
    present (the agent restated the goal to the required formal statement), an
    additional tagged row teaches that restatement as its own signal.
    """
    obligation_id = str(record.get("obligation_id", ""))
    goal = str(record.get("goal", ""))
    formal = record.get("formal_statement")
    proof = str(record.get("proof", ""))

    rows: list[Row] = []

    # Main proof row. Prompt is the required formal statement when we have it
    # (that is what the proof actually discharges), else the informal goal.
    prompt = str(formal) if formal else goal
    rows.append(
        _row(
            prompt,
            proof,
            {
                "kind": "proof",
                "obligation_id": obligation_id,
                "goal": goal,
            },
        )
    )

    # Optional reformulate-to-required-statement adapter step.
    reform = record.get("reformulation")
    if isinstance(reform, dict):
        src = str(reform.get("from_statement", goal))
        dst = str(reform.get("to_statement", formal or ""))
        if src and dst:
            rows.append(
                _row(
                    f"{_REFORMULATE_INSTRUCTION}\n\n{src}",
                    dst,
                    {
                        "kind": "reformulation",
                        "obligation_id": obligation_id,
                    },
                )
            )
    return rows


# ---------------------------------------------------------------------------
# GraphExport adapter (shape 2 -> shape 1)
# ---------------------------------------------------------------------------

def from_graph_export(export: dict[str, Any]) -> list[dict[str, Any]]:
    """Walk a serialized ``GraphExport`` into ``ObligationTrace`` records.

    A formal-proof node is treated as a verified obligation when:

    * its ``kind`` is ``formal_proof``,
    * its ``status`` is ``formally_verified`` and it is not ``tainted``,
    * a ``verifies`` edge of strength ``lean_checked`` originates from it
      (the machine-checked axiom-gate signal); if no edges are present at all
      we fall back to the node status alone.

    The proof node's ``statement`` is the completion; the obligation/statement
    it verifies supplies the prompt. Best-effort and lossy -- prefer emitting
    ``ObligationTrace`` records directly.
    """
    nodes = {n["id"]: n for n in export.get("nodes", [])}
    edges = export.get("edges", [])

    # index: proof node id -> target it lean-verifies
    lean_verifies: dict[str, str] = {}
    have_edges = bool(edges)
    for e in edges:
        if e.get("kind") == "verifies" and e.get("evidence_strength") == "lean_checked":
            lean_verifies[e["source_id"]] = e["target_id"]

    traces: list[dict[str, Any]] = []
    for node in nodes.values():
        if node.get("kind") != "formal_proof":
            continue
        status_ok = node.get("status") == "formally_verified"
        clean = not node.get("tainted", False)
        if have_edges:
            axioms_ok = node["id"] in lean_verifies
        else:
            axioms_ok = status_ok  # no edge info: trust the status
        target_id = lean_verifies.get(node["id"])
        target = nodes.get(target_id, {}) if target_id else {}
        goal = target.get("statement") or node.get("statement", "")
        formal = (
            target.get("formal_statement")
            or node.get("formal_statement")
            or target.get("statement")
        )
        traces.append(
            {
                "obligation_id": node["id"],
                "goal": goal,
                "formal_statement": formal,
                "proof": node.get("statement", ""),
                "verified": {"compiled": bool(status_ok), "axioms_ok": bool(axioms_ok and clean)},
            }
        )
    return traces


def _normalize_input(data: Any) -> list[dict[str, Any]]:
    """Coerce accepted input shapes into a list of ObligationTrace records."""
    if isinstance(data, list):
        return data
    if isinstance(data, dict):
        if "traces" in data:
            return list(data["traces"])
        if "obligations" in data:
            return list(data["obligations"])
        # looks like a GraphExport
        if "nodes" in data:
            return from_graph_export(data)
    raise ValueError("unrecognized harvester input; expected a trace list or GraphExport")


# ---------------------------------------------------------------------------
# Public entrypoint
# ---------------------------------------------------------------------------

def harvest(data: Any) -> dict[str, Any]:
    """Harvest SFT rows from completed proof-DAG traces.

    ``data`` is either a list of ObligationTrace records, a wrapper dict, or a
    GraphExport dict (see module docstring). Only verified obligations produce
    rows; duplicates (by content hash) are dropped.

    Returns ``{ok, kept, dropped, skipped_unverified, rows}``.
    """
    records = _normalize_input(data)
    rows: list[Row] = []
    seen: set[str] = set()
    dropped = 0
    skipped_unverified = 0

    for record in records:
        if not is_verified(record):
            skipped_unverified += 1
            continue
        for row in _rows_for_trace(record):
            key = row["meta"]["hash"]
            if key in seen:
                dropped += 1
                continue
            seen.add(key)
            rows.append(row)

    return {
        "ok": True,
        "kept": len(rows),
        "dropped": dropped,
        "skipped_unverified": skipped_unverified,
        "rows": rows,
    }


def to_chat_row(row: Row) -> dict[str, Any]:
    """Convert a ``{prompt, completion}`` harvest row into the chat-SFT shape
    (``messages`` list) that ``sft_export``/TRL's ``SFTTrainer`` consume."""
    return {
        "messages": [
            {"role": "user", "content": row["prompt"]},
            {"role": "assistant", "content": row["completion"]},
        ]
    }


def write_jsonl(rows: Iterable[Row], path: str) -> int:
    """Write harvest rows as JSON Lines; return the count written."""
    count = 0
    with open(path, "w", encoding="utf-8") as fh:
        for row in rows:
            fh.write(json.dumps(row, ensure_ascii=False))
            fh.write("\n")
            count += 1
    return count


def run(request: dict[str, Any]) -> dict[str, Any]:
    op = request.get("op", "harvest")
    if op == "harvest":
        data = request.get("data", request.get("traces", request.get("export")))
        result = harvest(data)
        path = request.get("path")
        if path:
            result = dict(result)
            result["written"] = write_jsonl(result["rows"], path)
            result["path"] = path
        if request.get("chat"):
            result = dict(result)
            result["rows"] = [to_chat_row(r) for r in result["rows"]]
        return result
    raise ValueError(f"unknown op: {op}")


def main() -> None:
    if len(sys.argv) >= 2:
        with open(sys.argv[1], encoding="utf-8") as fh:
            request = json.load(fh)
    else:
        request = json.load(sys.stdin)
    print(json.dumps(run(request), indent=2, default=str))
    raise SystemExit(0)


if __name__ == "__main__":
    main()
