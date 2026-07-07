"""STaR / rejection-sampling SFT dataset export (Theoremata plan section 14).

Turns the agent's own *verified* Lean proofs into supervised fine-tuning data
(the STaR loop: sample -> keep only what the verifier accepts -> retrain). Two
sources of rows:

* ``star_dataset`` -- rejection sampling. Keep only proofs the Lean compiler
  verified *and* whose axiom closure is clean (no ``sorry`` / disallowed
  axioms), dedupe identical (goal, proof) pairs, and emit chat-format SFT rows.
* ``rationalize`` -- STaR rationalization. For a goal the agent could not prove
  on its own, condition on a provided human proof sketch to build a training
  row anyway, so hard goals are not lost from the dataset.

The row format is chat-SFT (a ``messages`` list) so it drops straight into
TRL's ``SFTTrainer`` with a chat template. No GPU or training here -- this is
pure data shaping.
"""
from __future__ import annotations

import json
import sys
from typing import Any, Callable, Iterable

# A single supervised example: user turn = goal, assistant turn = proof.
Row = dict[str, Any]


def sft_row(goal: str, proof: str) -> Row:
    """Build one chat-format SFT row from a goal and its proof."""
    return {
        "messages": [
            {"role": "user", "content": goal},
            {"role": "assistant", "content": proof},
        ]
    }


def _accepts(record: dict[str, Any]) -> bool:
    """A record is kept only when the proof verified and its axiom closure is
    clean. ``axioms_ok`` is optional and defaults to True (absent means the
    caller did not run the axiom audit, so we do not reject on it)."""
    if not record.get("verified"):
        return False
    return bool(record.get("axioms_ok", True))


def star_dataset(records: list[dict[str, Any]]) -> dict[str, Any]:
    """Rejection-sampling SFT export.

    ``records`` are attempt records ``{goal, proof, verified: bool,
    axioms_ok?: bool}``. Keep only verified + axioms-ok records, dedupe by the
    exact ``(goal, proof)`` pair, and emit chat-SFT rows.

    Returns ``{ok, kept, dropped, rows}`` where ``kept`` is the number of rows
    emitted and ``dropped`` is every record that was rejected or was a dupe.
    """
    rows: list[Row] = []
    seen: set[tuple[str, str]] = set()
    dropped = 0
    for record in records:
        if not _accepts(record):
            dropped += 1
            continue
        goal = str(record.get("goal", ""))
        proof = str(record.get("proof", ""))
        key = (goal, proof)
        if key in seen:
            dropped += 1
            continue
        seen.add(key)
        rows.append(sft_row(goal, proof))
    return {"ok": True, "kept": len(rows), "dropped": dropped, "rows": rows}


def rationalize(record: dict[str, Any], human_sketch: str) -> Row:
    """STaR rationalization: build a training row for a goal the agent could
    not prove by conditioning on a provided human proof sketch.

    The sketch becomes the assistant target so the model learns to reproduce a
    correct line of reasoning for hard goals it would otherwise miss. Prefer a
    verified ``proof`` on the record if one is present (a rationalized attempt
    that later verified); otherwise fall back to the human sketch itself.
    """
    goal = str(record.get("goal", ""))
    proof = str(record.get("proof") or human_sketch)
    row = sft_row(goal, proof)
    row["rationalized"] = True
    return row


def write_jsonl(rows: Iterable[Row], path: str) -> int:
    """Write ``rows`` as JSON Lines to ``path``; return the number written."""
    count = 0
    with open(path, "w", encoding="utf-8") as fh:
        for row in rows:
            fh.write(json.dumps(row, ensure_ascii=False))
            fh.write("\n")
            count += 1
    return count


def run(request: dict[str, Any]) -> dict[str, Any]:
    op = request.get("op", "star_dataset")
    if op == "star_dataset":
        result = star_dataset(request["records"])
        path = request.get("path")
        if path:
            result = dict(result)
            result["written"] = write_jsonl(result["rows"], path)
            result["path"] = path
        return result
    if op == "rationalize":
        return {"ok": True, "row": rationalize(request["record"], request["human_sketch"])}
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
