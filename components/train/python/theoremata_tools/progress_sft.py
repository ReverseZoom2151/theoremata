"""LeanProgress-style progress SFT: predict remaining proof steps as text.

Reproduces LeanProgress's recipe
(``docs/resource-mining/reverify/LeanCopilot-LeanAgent-LeanProgress.md``) on OUR
proof-state traces. The predictor is a *generative* SFT LLM (not a scalar head):
given a proof state it completes an integer (steps-to-``no goals``) or a float
(``relative_progress = 1 - i/L``) as plain text.

Serialization (exact LeanProgress format, ``steps_adj_relative.py:61-65``)::

    prompt     = "---\nSTATE_AFTER: {goal}\n\n---\nSTEPS_TO_NO_GOALS:"
    completion = " {label}"

Labeling (``collect_steps_data.py:49-140``): reconstruct the path to ``no goals``;
a state ``i`` edges away from ``no goals`` is labeled ``steps_to_no_goals = i`` and
``relative_progress = 1 - i/L`` where ``L`` is the path length (total steps). The
``no goals`` terminal has ``i = 0`` (relative ``1.0``); the start state has
``i = L`` (relative ``0.0``).

Training config scaffold (documented, GPU-gated, dry-run by default): the real
recipe is deepseek-coder-1.3b via XTuner (``max_length=4096``, AdamW ``lr=1e-6``,
cosine w/ 3% warmup, ``max_epochs=5``, grad-clip 1.0, fp32). We validate the
dataset + config end-to-end without importing a trainer.
"""
from __future__ import annotations

import json
import sys
from typing import Any, Iterable

NO_GOALS = "no goals"

STEPS_TARGET = "steps"
RELATIVE_TARGET = "relative"


def progress_config(**overrides: Any) -> dict[str, Any]:
    """LeanProgress SFT config (deepseek-coder-1.3b recipe). Documented defaults;
    ``overrides`` replace any key. Never imports a trainer."""
    config: dict[str, Any] = {
        "model": "deepseek-ai/deepseek-coder-1.3b-base",
        "target": STEPS_TARGET,  # or "relative"
        "max_length": 4096,
        "pack_to_max_length": True,
        "optimizer": "AdamW",
        "learning_rate": 1e-6,
        "lr_scheduler": "cosine",
        "warmup_ratio": 0.03,
        "max_epochs": 5,
        "grad_clip": 1.0,
        "dtype": "fp32",  # fp32 for NaN stability (LeanProgress note)
        "seed": 0,
    }
    config.update(overrides)
    return config


def _path_from_trace(trace: dict[str, Any]) -> list[str]:
    """Extract an ordered state path ending at ``no goals`` from a trace.

    Accepts either ``{"states": [s0, s1, ..., "no goals"]}`` or
    ``{"edges": [{"state_before","state_after"}, ...]}`` (chained into a path).
    A trailing ``no goals`` is appended if the trace is a completed proof and
    does not already end in it.
    """
    if "states" in trace:
        path = [str(s) for s in trace["states"]]
    elif "edges" in trace:
        edges = trace["edges"]
        path = []
        for e in edges:
            if not path:
                path.append(str(e.get("state_before", "")))
            path.append(str(e.get("state_after", "")))
    else:
        raise ValueError("trace needs 'states' or 'edges'")
    if trace.get("proved", True) and (not path or path[-1] != NO_GOALS):
        path.append(NO_GOALS)
    return path


def label_path(path: list[str], target: str = STEPS_TARGET) -> list[dict[str, Any]]:
    """Label each *non-terminal* state on a path to ``no goals``.

    For a path of ``len(path)`` states, ``L = len(path) - 1`` (total steps). A
    state at position ``p`` is ``i = L - p`` edges from ``no goals``; it is
    labeled ``steps_to_no_goals = i`` and ``relative_progress = 1 - i/L``. The
    ``no goals`` terminal (``i = 0``) carries no goal to condition on and is
    skipped as a training example.
    """
    L = len(path) - 1
    if L <= 0:
        return []
    rows: list[dict[str, Any]] = []
    for p, goal in enumerate(path):
        i = L - p
        if i == 0:  # the 'no goals' terminal: nothing to predict from
            continue
        relative = 1.0 - i / L
        label = i if target == STEPS_TARGET else round(relative, 6)
        rows.append(
            {
                "goal": goal,
                "steps_to_no_goals": i,
                "relative_progress": round(relative, 6),
                "label": label,
            }
        )
    return rows


def sft_row(goal: str, label: Any) -> dict[str, Any]:
    """LeanProgress prompt/completion serialization."""
    prompt = f"---\nSTATE_AFTER: {goal}\n\n---\nSTEPS_TO_NO_GOALS:"
    return {"prompt": prompt, "completion": f" {label}"}


def build_progress_dataset(
    traces: Iterable[dict[str, Any]],
    target: str = STEPS_TARGET,
) -> dict[str, Any]:
    """Turn proof-state traces into LeanProgress SFT rows.

    Returns ``{ok, target, rows, skipped}`` where each row is
    ``{prompt, completion, meta:{steps_to_no_goals, relative_progress}}``.
    """
    if target not in (STEPS_TARGET, RELATIVE_TARGET):
        raise ValueError(f"target must be 'steps' or 'relative', got {target!r}")
    rows: list[dict[str, Any]] = []
    skipped = 0
    for trace in traces:
        path = _path_from_trace(trace)
        labels = label_path(path, target)
        if not labels:
            skipped += 1
            continue
        for lab in labels:
            row = sft_row(lab["goal"], lab["label"])
            row["meta"] = {
                "steps_to_no_goals": lab["steps_to_no_goals"],
                "relative_progress": lab["relative_progress"],
            }
            rows.append(row)
    return {"ok": True, "target": target, "rows": rows, "skipped": skipped}


def dry_run(traces: Iterable[dict[str, Any]], config: dict[str, Any] | None = None) -> dict[str, Any]:
    """Validate dataset + config end-to-end offline (no trainer, no GPU)."""
    config = config or progress_config()
    target = config.get("target", STEPS_TARGET)
    ds = build_progress_dataset(traces, target)
    # sanity: every completion parses back to a number of the right type.
    for row in ds["rows"]:
        val = row["completion"].strip()
        float(val)  # raises if malformed
    return {
        "ok": True,
        "dry_run": True,
        "reason": "no_trainer_backend",
        "target": target,
        "num_rows": len(ds["rows"]),
        "skipped": ds["skipped"],
        "would_run": {"model": config.get("model"), "max_epochs": config.get("max_epochs")},
    }


def write_jsonl(rows: Iterable[dict[str, Any]], path: str) -> int:
    count = 0
    with open(path, "w", encoding="utf-8") as fh:
        for row in rows:
            fh.write(json.dumps(row, ensure_ascii=False))
            fh.write("\n")
            count += 1
    return count


def run(request: dict[str, Any]) -> dict[str, Any]:
    op = request.get("op", "build")
    if op == "build":
        return build_progress_dataset(
            request.get("traces", []), request.get("target", STEPS_TARGET)
        )
    if op == "config":
        return progress_config(**request.get("overrides", {}))
    if op == "dry_run":
        return dry_run(request.get("traces", []), request.get("config"))
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
