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
import math
import sys
from typing import Any, Iterable

from .flywheel import SFT_SCHEMA  # single source of truth for the chat-SFT shape

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


# ---------------------------------------------------------------------------
# Chat-SFT: one honest, tiny fine-tune step over the flywheel's JSONL output.
#
# This consumes the shared :data:`flywheel.SFT_SCHEMA` chat rows (produced by
# ``flywheel.to_sft_rows`` / ``flywheel.revolution``) and runs ONE step of SFT.
# If torch + transformers import, a REAL single gradient step of a tiny,
# randomly-initialised, char-level GPT-2 runs entirely offline (no weights are
# downloaded). Otherwise a clearly-labelled dry-run reports dataset and
# loss-shaped metrics WITHOUT touching any weights -- honest about being
# not-a-real-train. This is distinct from the LeanProgress recipe above.
# ---------------------------------------------------------------------------


def sft_config(**overrides: Any) -> dict[str, Any]:
    """Tiny chat-SFT config for ONE honest step (distinct from the LeanProgress
    ``progress_config``). CPU-friendly; the torch path builds a random-init model
    so nothing is downloaded. ``overrides`` replace any key."""
    config: dict[str, Any] = {
        "model": "gpt2-tiny(random-init, char-level, offline)",
        "schema": SFT_SCHEMA,
        "max_length": 256,
        "learning_rate": 5e-5,
        "max_steps": 1,
        "batch_size": 1,
        "grad_clip": 1.0,
        "seed": 0,
    }
    config.update(overrides)
    return config


def read_sft_jsonl(path: str) -> list[dict[str, Any]]:
    """Read a chat-SFT JSONL file (:data:`flywheel.SFT_SCHEMA`) into rows."""
    rows: list[dict[str, Any]] = []
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if line:
                rows.append(json.loads(line))
    return rows


def _chat_example_text(row: dict[str, Any]) -> str:
    """Flatten one chat-SFT row into a single training string. Tolerates the flat
    ``{prompt, completion}`` shape too, so harvester rows also feed in."""
    msgs = row.get("messages")
    if isinstance(msgs, list):
        return "\n".join(
            f"{m.get('role', '')}: {m.get('content', '')}" for m in msgs
        )
    return f"{row.get('prompt', '')}{row.get('completion', '')}"


def _torch_transformers_available() -> bool:
    try:
        import torch  # noqa: F401
        import transformers  # noqa: F401
    except Exception:  # noqa: BLE001 - any import failure => offline dry-run
        return False
    return True


def _sft_dry_run(texts: list[str], config: dict[str, Any]) -> dict[str, Any]:
    """Deterministic offline dry-run: count tokens / build examples / report
    loss-shaped metrics. Clearly labelled NOT a real train -- no weights move."""
    n = len(texts)
    token_counts = [len(t.split()) for t in texts]
    total_tokens = sum(token_counts)
    vocab: set[str] = set()
    for t in texts:
        vocab.update(t.split())
    V = max(len(vocab), 1)
    # A loss-shaped REFERENCE, not a trained loss: the cross-entropy (nats) of a
    # uniform next-token guess over the observed vocabulary -- the analytic
    # baseline a real trainer would start near and then reduce.
    uniform_baseline_loss = round(math.log(V), 6)
    return {
        "ok": True,
        "trained": False,
        "backend": "dry_run",
        "reason": "torch+transformers unavailable",
        "note": "NOT a real train: metrics are analytic, no weights updated",
        "schema": SFT_SCHEMA,
        "num_examples": n,
        "num_tokens": total_tokens,
        "avg_tokens": round(total_tokens / n, 3) if n else 0.0,
        "vocab_size": V,
        "steps": 0,
        "uniform_baseline_loss": uniform_baseline_loss,
        "config": {"model": config.get("model"), "max_steps": config.get("max_steps")},
    }


def _sft_finetune_torch(texts: list[str], config: dict[str, Any]) -> dict[str, Any]:
    """One REAL SFT step, offline: a random-init char-level GPT-2 (built from a
    config, so nothing is downloaded) does a single forward/backward/optimizer
    step over the flattened examples. Returns the observed training loss."""
    import torch
    from transformers import GPT2Config, GPT2LMHeadModel

    if not texts:
        return {
            "ok": True,
            "trained": False,
            "backend": "torch",
            "reason": "empty dataset",
            "steps": 0,
        }

    torch.manual_seed(int(config.get("seed", 0)))
    # Char-level vocab built from the data => no pretrained download (offline).
    chars = sorted({c for t in texts for c in t})
    stoi = {c: i + 1 for i, c in enumerate(chars)}  # id 0 reserved for pad
    vocab_size = len(stoi) + 1
    max_len = int(config.get("max_length", 256))

    batch = [[stoi[c] for c in t][:max_len] or [0] for t in texts]
    width = max(len(ids) for ids in batch)
    input_ids = torch.zeros(len(batch), width, dtype=torch.long)
    for i, ids in enumerate(batch):
        input_ids[i, : len(ids)] = torch.tensor(ids, dtype=torch.long)

    model_cfg = GPT2Config(
        vocab_size=vocab_size,
        n_positions=max(width, 8),
        n_ctx=max(width, 8),
        n_embd=32,
        n_layer=2,
        n_head=2,
    )
    model = GPT2LMHeadModel(model_cfg)
    model.train()
    optim = torch.optim.AdamW(
        model.parameters(), lr=float(config.get("learning_rate", 5e-5))
    )

    labels = input_ids.clone()
    labels[labels == 0] = -100  # ignore pad positions in the LM loss

    out = model(input_ids=input_ids, labels=labels)
    loss0 = float(out.loss.detach())
    out.loss.backward()
    torch.nn.utils.clip_grad_norm_(
        model.parameters(), float(config.get("grad_clip", 1.0))
    )
    optim.step()

    return {
        "ok": True,
        "trained": True,
        "backend": "torch",
        "schema": SFT_SCHEMA,
        "num_examples": len(texts),
        "vocab_size": vocab_size,
        "steps": 1,
        "loss": round(loss0, 6),
        "config": {"model": "gpt2-tiny(random-init, char-level, offline)", "max_length": max_len},
    }


def sft_finetune(
    data: str | Iterable[dict[str, Any]],
    config: dict[str, Any] | None = None,
    *,
    backend: str = "auto",
) -> dict[str, Any]:
    """Run ONE honest, tiny SFT step over the flywheel's chat-SFT output.

    ``data`` is a path to a JSONL file (``flywheel.to_sft_rows`` /
    ``flywheel.revolution`` output) or an in-memory list of chat-SFT rows.
    ``backend`` is ``"auto"`` (real torch step if available, else dry-run),
    ``"torch"``, or ``"dry_run"``. Everything runs offline with no GPU.
    """
    config = config or sft_config()
    rows = read_sft_jsonl(data) if isinstance(data, str) else list(data)
    texts = [_chat_example_text(r) for r in rows]
    if backend == "auto":
        backend = "torch" if _torch_transformers_available() else "dry_run"
    if backend == "torch":
        return _sft_finetune_torch(texts, config)
    return _sft_dry_run(texts, config)


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
    if op == "sft":
        return sft_finetune(
            request.get("path", request.get("rows", [])),
            request.get("config"),
            backend=request.get("backend", "auto"),
        )
    if op == "sft_config":
        return sft_config(**request.get("overrides", {}))
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
