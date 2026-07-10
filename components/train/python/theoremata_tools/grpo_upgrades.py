"""GRPO config/aggregation upgrades (agentic-patterns-mining H2).

Two buildable-now, GPU-free refinements the H2 mining flags as high-leverage:

* **2-GRPO preset (G=2)** -- the book's §7.5.5 result that a *paired* group of
  size ``G=2`` gives a DPO-like contrastive signal that matches ``G=16`` on
  GSM8K/MATH/code, at ~4-6x lower rollout cost. Since proof rollouts are our
  dominant expense, exposing a G=2 recipe is the cheapest large win available.
  :func:`two_grpo_config` returns a *copy* of any :func:`grpo.grpo_config` dict
  with the group size pinned to 2, everything else preserved.

* **GDPO normalize-then-sum (§9.6)** -- ``reward.py`` blends correctness + tool
  + format by *weighted sum*, which is scale-sensitive: if one channel's reward
  variance dwarfs the others it dominates the within-group advantage and the
  other channels' signal collapses. The book's fix is to **z-normalize each
  component within the group first, then sum** the normalized channels, so every
  channel contributes on an equal (unit-variance) footing.
  :func:`normalize_then_sum` / :func:`gdpo_advantages` implement this exactly
  with the stdlib :mod:`statistics` (deterministic, no numpy).

This module is *additive*: it does not import or mutate ``grpo.py`` /
``reward.py``. Wire it in by (a) calling ``two_grpo_config`` on the config a
trainer would consume, and (b) feeding per-component reward vectors through
``gdpo_advantages`` in place of the scalar additive blend. Both are pure
config/aggregation; the gradient step itself stays GPU-gated.
"""
from __future__ import annotations

import statistics
from typing import Any

# The paired 2-GRPO group size (book §7.5.5).
TWO_GRPO_GROUP_SIZE = 2

# Config keys that carry the GRPO group size, in the vocabularies we support:
# ``num_generations`` is TRL's field name (used by grpo.py); ``group_size`` /
# ``G`` are the aliases the mining note and DPO-style recipes use.
_GROUP_SIZE_KEYS = ("num_generations", "group_size", "G")


def two_grpo_config(base_config: dict[str, Any]) -> dict[str, Any]:
    """Return a **copy** of ``base_config`` switched to the 2-GRPO preset.

    Pins the group size to ``2`` under every group-size key already present
    (``num_generations`` -- TRL's name used by ``grpo.grpo_config`` -- plus the
    ``group_size`` / ``G`` aliases) and *also* records ``group_size`` and ``G``
    so downstream code that reads either alias sees the paired size. All other
    keys are preserved untouched.

    Rollout economics: standard GRPO samples ``G`` completions per prompt
    (``grpo.py`` defaults ``num_generations=8``; the book's strong baseline is
    ``G=16``). Cost scales ~linearly in ``G``, so ``G=2`` cuts per-prompt
    rollouts by ``8/2 = 4x`` vs our default and ``16/2 = 8x`` vs the G=16
    baseline -- the ~4-6x end-to-end speedup §7.5.5 reports, with no accuracy
    loss on GSM8K/MATH/code because the paired samples still give a valid
    contrastive (DPO-like) advantage.
    """
    config = dict(base_config)  # shallow copy: never mutate the caller's dict
    prev = config.get("num_generations") or config.get("group_size") or config.get("G")
    for key in _GROUP_SIZE_KEYS:
        if key in config:
            config[key] = TWO_GRPO_GROUP_SIZE
    # Ensure the canonical + alias keys are set even if absent originally, so a
    # trainer reading any of them gets the paired preset.
    config["num_generations"] = TWO_GRPO_GROUP_SIZE
    config["group_size"] = TWO_GRPO_GROUP_SIZE
    config["G"] = TWO_GRPO_GROUP_SIZE
    # Provenance / documentation of the savings, for the dry-run report.
    config["grpo_preset"] = "2-GRPO"
    if prev:
        config["rollout_savings_vs_prev"] = float(prev) / TWO_GRPO_GROUP_SIZE
    return config


def _znorm(values: list[float]) -> list[float]:
    """Z-normalize (zero-mean, unit-std) a component's values within the group.

    Uses the population std (:func:`statistics.pstdev`) -- the group *is* the
    population for within-group GRPO normalization. A **zero-variance** component
    (all values identical, e.g. a channel that fired for every sample) has
    ``std == 0``; there is no direction to normalize along, so every entry maps
    to ``0.0`` (no div-by-zero, contributes nothing to the sum) which is the
    correct "this channel carries no signal in this group" behaviour.
    """
    if not values:
        return []
    mean = statistics.mean(values)
    std = statistics.pstdev(values)
    if std == 0.0:
        return [0.0 for _ in values]
    return [(v - mean) / std for v in values]


def normalize_then_sum(components: dict[str, list[float]]) -> list[float]:
    """GDPO multi-objective blend (book §9.6): z-normalize each reward component
    *within the group*, then sum the normalized channels elementwise.

    ``components`` maps a channel name (e.g. ``"correctness"``, ``"tool"``,
    ``"format"``) to that channel's per-sample reward vector; all vectors must
    have the same length (one entry per sample in the group). Returns the
    per-sample summed-normalized reward.

    Because every channel is rescaled to unit variance before summing, a
    large-scale channel can no longer dominate the blend the way it does under a
    raw weighted sum -- this is what avoids advantage collapse. A zero-variance
    channel contributes ``0`` (see :func:`_znorm`). An empty ``components`` (or
    zero-length vectors) yields an empty result.
    """
    if not components:
        return []
    lengths = {len(v) for v in components.values()}
    if len(lengths) != 1:
        raise ValueError(f"component vectors must be equal length, got {lengths}")
    n = lengths.pop()
    normalized = [_znorm(list(vec)) for vec in components.values()]
    return [sum(col[i] for col in normalized) for i in range(n)]


def gdpo_advantages(
    rewards_by_component: dict[str, list[float]],
    *,
    center: bool = True,
) -> list[float]:
    """GDPO per-sample advantage vector for one GRPO group.

    Computes :func:`normalize_then_sum` over the component reward vectors, then
    (when ``center``, the default) subtracts the group mean so the advantages are
    zero-mean -- the standard GRPO baseline-subtraction that leaves relative
    ordering intact while stabilizing the policy-gradient scale. Because the
    blend was normalized per channel *before* summing, no single large-scale
    component dominates the resulting advantage (advantage collapse avoided).

    Returns the per-sample advantage vector (same length as each input vector).
    """
    blended = normalize_then_sum(rewards_by_component)
    if not center or not blended:
        return blended
    mean = statistics.mean(blended)
    return [b - mean for b in blended]


def run(request: dict[str, Any]) -> dict[str, Any]:
    """Worker dispatch for the ``grpo_upgrades`` op family.

    ``op``:

    * ``two_grpo_config`` -- ``{"config": {...}}`` -> ``{"ok", "config"}``.
    * ``normalize_then_sum`` -- ``{"components": {...}}`` -> ``{"ok", "blended"}``.
    * ``gdpo_advantages`` -- ``{"components"|"rewards_by_component": {...},
      "center"?: bool}`` -> ``{"ok", "advantages"}``.

    An unknown op returns ``{"ok": False, ...}`` (never raises), matching the
    worker contract used across the tools.
    """
    op = request.get("op")
    if op == "two_grpo_config":
        return {"ok": True, "config": two_grpo_config(request["config"])}
    if op == "normalize_then_sum":
        return {"ok": True, "blended": normalize_then_sum(request["components"])}
    if op == "gdpo_advantages":
        comps = request.get("rewards_by_component") or request.get("components") or {}
        return {
            "ok": True,
            "advantages": gdpo_advantages(comps, center=request.get("center", True)),
        }
    return {"ok": False, "error": f"unknown op: {op}"}
