"""RLVR / GRPO training harness (Theoremata plan section 14).

Reinforcement Learning from Verifiable Rewards: the reward is *not* a learned
reward model, it is the Lean compiler -- a proof either verifies (reward 1.0)
or it does not (reward 0.0). This module builds a TRL ``GRPOConfig``-shaped
training config, maps a verifier over sampled completions to produce the
binary rewards, applies the Goldilocks filter (drop groups that are all-pass or
all-fail, which give zero policy gradient), and provides a ``train`` entrypoint
that dry-runs by default and never touches a GPU / TRL unless explicitly asked.

The config carries the DAPO-style knobs from the plan as documented keys:
clip-higher (asymmetric PPO clip), token-level loss aggregation, and overlong
(too-long completion) filtering.
"""
from __future__ import annotations

import json
import sys
from typing import Any, Callable, Optional

# A verifier is the Lean compiler in real use: proof text -> did it verify?
Verifier = Callable[[str], bool]

# Linear temperature annealing bounds (DeepMath training_utils: hot -> cold).
TEMPERATURE_START = 1.2
TEMPERATURE_END = 0.7


def grpo_config(model: str, dataset_path: str, **overrides: Any) -> dict[str, Any]:
    """Build a GRPO training-config dict matching TRL ``GRPOConfig`` fields.

    Sensible defaults for verifiable-reward proof training plus the DAPO knobs
    as documented keys. Any keyword in ``overrides`` replaces the default of
    the same name (unknown keys are added too, so caller-specific TRL fields
    pass through).
    """
    config: dict[str, Any] = {
        # --- model / data ------------------------------------------------
        "model": model,
        "dataset_path": dataset_path,
        "output_dir": "outputs/grpo",
        # --- GRPO group sampling ----------------------------------------
        # num_generations == group size G: completions sampled per prompt,
        # scored together and advantage-normalized within the group.
        "num_generations": 8,
        "max_prompt_length": 1024,
        "max_completion_length": 1024,
        "temperature": TEMPERATURE_START,
        "top_p": 1.0,
        # linear temperature annealing (DeepMath): start hot to explore diverse
        # proof strategies, cool to stabilize. Applied per-step by
        # ``linear_temperature`` in the real trainer / reported in the dry run.
        "temperature_start": TEMPERATURE_START,
        "temperature_end": TEMPERATURE_END,
        # --- optimization ------------------------------------------------
        "learning_rate": 1e-6,
        "beta": 0.0,  # KL penalty coefficient; DAPO drops the KL term (beta=0)
        "epsilon": 0.2,  # lower PPO clip bound
        "num_train_epochs": 1,
        "max_steps": 500,
        "per_device_train_batch_size": 1,
        "gradient_accumulation_steps": 8,
        "logging_steps": 1,
        "save_steps": 100,
        "seed": 0,
        # --- DAPO-style knobs (plan section 14) -------------------------
        # clip-higher: decouple the upper PPO clip so low-prob tokens can grow,
        # preserving exploration / entropy.
        "epsilon_high": 0.28,
        # token-level loss: average the loss over tokens (not per-sequence),
        # so long correct proofs are not down-weighted.
        "loss_type": "dapo",
        "scale_rewards": False,
        # overlong filtering: mask (do not penalize) completions truncated at
        # max_completion_length rather than treating them as failures.
        "mask_truncated_completions": True,
        "overlong_filter": True,
    }
    config.update(overrides)
    return config


def reward_from_verifier(completions: list[str], verifier: Verifier) -> list[float]:
    """RLVR reward: 1.0 if the verifier accepts the completion, else 0.0.

    In production ``verifier`` compiles the proof with Lean and checks the
    axiom closure; here it is any ``str -> bool`` callable.
    """
    return [1.0 if verifier(c) else 0.0 for c in completions]


def goldilocks_keep(group_rewards: list[float]) -> bool:
    """Goldilocks filter: keep a group only if its pass-rate is strictly
    between 0 and 1. All-pass and all-fail groups have identical rewards, so
    their within-group advantages are all zero -> zero gradient -> wasted
    compute. An empty group is dropped.
    """
    if not group_rewards:
        return False
    total = sum(group_rewards)
    return 0.0 < total < len(group_rewards)


def linear_temperature(step: int, total_steps: int, config: dict[str, Any]) -> float:
    """Linearly annealed sampling temperature at ``step`` of ``total_steps``.

    Interpolates ``temperature_start`` -> ``temperature_end`` (defaults 1.2 ->
    0.7). ``step`` is clamped into ``[0, total_steps]``; a non-positive
    ``total_steps`` returns the start temperature. Portable to any best-of-N /
    MCTS sampler (DeepMath ``training_utils.GRPOTrainerTemperature``)."""
    hot = float(config.get("temperature_start", TEMPERATURE_START))
    cold = float(config.get("temperature_end", TEMPERATURE_END))
    if total_steps <= 0:
        return hot
    frac = min(max(step, 0), total_steps) / total_steps
    return hot + (cold - hot) * frac


def _load_dataset(dataset: Any) -> list[dict[str, Any]]:
    """Coerce a dataset argument into a list of sample dicts. Accepts an
    in-memory list, a harvester result dict (``{"rows": [...]}``), or a path to
    a JSONL file (one sample per line)."""
    if isinstance(dataset, str):
        rows: list[dict[str, Any]] = []
        with open(dataset, encoding="utf-8") as fh:
            for line in fh:
                line = line.strip()
                if line:
                    rows.append(json.loads(line))
        return rows
    if isinstance(dataset, dict) and "rows" in dataset:
        return list(dataset["rows"])
    if isinstance(dataset, list):
        return list(dataset)
    raise ValueError("unrecognized dataset; expected list, {'rows': [...]}, or a JSONL path")


def dry_run_grpo(
    dataset: Any,
    reward_fn: Callable[[dict[str, Any]], Optional[float]],
    config: dict[str, Any],
) -> dict[str, Any]:
    """Validate a dataset + reward end-to-end WITHOUT a GPU / trainer.

    Runs ``reward_fn`` over every sample, tallies scored vs. skipped
    (``reward_fn`` returned ``None``), groups the scored rewards by
    ``prompt``/``group``/``problem_id`` into GRPO groups and reports how many
    survive the Goldilocks filter, and computes the temperature schedule the
    real run would follow. This is exactly the plumbing a real GRPO step would
    exercise, minus the model.

    Returns a report describing what a real run *would* do.
    """
    samples = _load_dataset(dataset)
    num_generations = int(config.get("num_generations", 8))

    scored = 0
    skipped = 0
    reward_sum = 0.0
    groups: dict[str, list[float]] = {}
    for i, sample in enumerate(samples):
        r = reward_fn(sample)
        if r is None:
            skipped += 1
            continue
        scored += 1
        reward_sum += r
        key = str(
            sample.get("group")
            or sample.get("problem_id")
            or sample.get("prompt")
            or i // max(num_generations, 1)
        )
        groups.setdefault(key, []).append(r)

    kept_groups = {k: v for k, v in groups.items() if goldilocks_keep(v)}
    total_steps = int(config.get("max_steps", 0))
    temp_schedule = {
        "start": linear_temperature(0, total_steps, config),
        "mid": linear_temperature(total_steps // 2, total_steps, config),
        "end": linear_temperature(total_steps, total_steps, config),
    }

    return {
        "ok": True,
        "dry_run": True,
        "reason": "no_trainer_backend",
        "dataset_size": len(samples),
        "scored": scored,
        "skipped": skipped,
        "mean_reward": (reward_sum / scored) if scored else 0.0,
        "num_groups": len(groups),
        "groups_kept": len(kept_groups),
        "groups_dropped": len(groups) - len(kept_groups),
        "temperature_schedule": temp_schedule,
        "would_run": {
            "model": config.get("model"),
            "num_generations": num_generations,
            "max_steps": total_steps,
        },
    }


def train(config: dict[str, Any], *, dry_run: bool = True) -> dict[str, Any]:
    """GRPO training entrypoint.

    When ``dry_run`` (the default) or TRL is not importable, return the config
    that *would* run without importing TRL or touching a GPU. Only when
    ``dry_run=False`` and ``import trl`` succeeds is a real trainer attempted.
    """
    if dry_run:
        return {"ok": True, "dry_run": True, "would_run": config}

    try:
        import trl  # noqa: F401  (lazy: never imported in dry-run / no-GPU path)
    except Exception as exc:
        return {
            "ok": True,
            "dry_run": True,
            "reason": f"trl_unavailable:{exc}",
            "would_run": config,
        }

    # Real training path -- requires a GPU + TRL, exercised only outside CI.
    from trl import GRPOConfig, GRPOTrainer  # pragma: no cover

    known = set(getattr(GRPOConfig, "__dataclass_fields__", {}))  # pragma: no cover
    trl_kwargs = {k: v for k, v in config.items() if k in known}  # pragma: no cover
    grpo_config_obj = GRPOConfig(**trl_kwargs)  # pragma: no cover
    return {  # pragma: no cover
        "ok": True,
        "dry_run": False,
        "config": grpo_config_obj,
        "trainer_cls": GRPOTrainer.__name__,
    }


def run(request: dict[str, Any]) -> dict[str, Any]:
    op = request.get("op", "grpo_config")
    if op == "grpo_config":
        return grpo_config(
            request["model"],
            request["dataset_path"],
            **request.get("overrides", {}),
        )
    if op == "goldilocks_keep":
        return {"keep": goldilocks_keep(request["group_rewards"])}
    if op == "train":
        return train(request["config"], dry_run=request.get("dry_run", True))
    if op == "dry_run":
        from .reward import make_reward_fn

        config = request.get("config") or grpo_config(
            request.get("model", "model"),
            request.get("dataset_path", request.get("dataset", "dataset")),
            **request.get("overrides", {}),
        )
        reward_fn = make_reward_fn(tool_weight=request.get("tool_weight", 0.1))
        return dry_run_grpo(request["dataset"], reward_fn, config)
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
