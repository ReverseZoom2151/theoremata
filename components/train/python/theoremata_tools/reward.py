"""Composable GRPO reward for verifiable proof training.

Adapts DeepMath's reward *craft* (``docs/resource-mining/DeepMath.md`` §2.6) to
Theoremata's formal-proof setting. Crucially, the reward is **verifier-driven**:
our Lean gate (compile + ``#print axioms`` closure) is the ground truth, not
answer-string matching. DeepMath rewards a numeric ``math_verify`` match; we
reward a clean Lean verdict.

The three DeepMath moves we keep:

* **Correctness reward from the verdict** -- ``1.0`` when the proof compiled and
  the axiom gate is clean, else ``0.0`` (``correctness_reward``).
* **Tool-use shaping, decoupled from correctness** -- a small ``0.1``-weighted
  bonus for *legitimately using* a tool / falsifier during the trace, whether or
  not the proof ultimately verified (``tool_use_reward``). DeepMath weights
  accuracy:tool-use at ``1.0 : 0.1``.
* **Skip, don't punish, on missing/unparseable gold** -- ``reward`` returns
  ``None`` (drop the sample from the batch) when there is no parseable gold
  obligation to verify against, rather than scoring it ``0`` and poisoning the
  gradient (DeepMath ``rewards.py:56-59``).

Plus the aggregation helpers DeepMath reports over ``k`` samples:
``pass_at_k`` and ``majority_at_k`` (``metrics.py``).
"""
from __future__ import annotations

from collections import Counter
from typing import Any, Callable, Optional, Sequence

# DeepMath weighting: accuracy 1.0, tool-use shaping 0.1.
TOOL_USE_WEIGHT = 0.1

# A verdict is either a structured dict {compiled, axioms_ok} or a bare bool.
Verdict = Any
GoldParser = Callable[[Any], Optional[Any]]


# ---------------------------------------------------------------------------
# Gold parsing (verifier-driven: "gold" is the obligation to check against)
# ---------------------------------------------------------------------------

def default_parse_gold(gold: Any) -> Optional[Any]:
    """Return the parsed gold obligation, or ``None`` if it is missing /
    unparseable. For us the "gold" is the formal statement / obligation the
    proof must discharge; an empty or whitespace-only string counts as
    unparseable. Non-string golds are passed through when truthy."""
    if gold is None:
        return None
    if isinstance(gold, str):
        stripped = gold.strip()
        return stripped or None
    return gold or None


# ---------------------------------------------------------------------------
# Reward components
# ---------------------------------------------------------------------------

def _verdict_pass(verdict: Verdict) -> Optional[bool]:
    """Interpret a verifier verdict as pass/fail, or ``None`` when there is no
    verdict at all (verifier did not run -> caller decides to skip)."""
    if verdict is None:
        return None
    if isinstance(verdict, dict):
        if not verdict:
            return None
        return bool(verdict.get("compiled")) and bool(verdict.get("axioms_ok", True))
    return bool(verdict)


def correctness_reward(verdict: Verdict) -> Optional[float]:
    """``1.0`` iff the proof compiled AND its axiom closure is clean, else
    ``0.0``. Returns ``None`` when no verdict is present (nothing was checked)."""
    passed = _verdict_pass(verdict)
    if passed is None:
        return None
    return 1.0 if passed else 0.0


def used_tool(sample: dict[str, Any]) -> bool:
    """Whether the trace *legitimately* exercised a tool / falsifier. Legitimacy
    means the tool actually ran (produced a result / no error), not merely that
    the model typed a tool call. Recognized signals, in order:

    * ``tool_calls`` / ``tools_used``: a non-empty list of successful calls. A
      dict entry counts only when it is not marked ``error``/``ok is False``.
    * ``used_tool`` / ``used_falsifier``: an explicit boolean flag.
    """
    for key in ("tool_calls", "tools_used"):
        calls = sample.get(key)
        if isinstance(calls, (list, tuple)) and calls:
            for call in calls:
                if isinstance(call, dict):
                    if call.get("error") or call.get("ok") is False:
                        continue
                    return True
                if call:
                    return True
    return bool(sample.get("used_tool") or sample.get("used_falsifier"))


def tool_use_reward(sample: dict[str, Any], weight: float = TOOL_USE_WEIGHT) -> float:
    """Shaping bonus for legitimate tool/falsifier use, INDEPENDENT of whether
    the proof verified. ``weight`` (default ``0.1``) matches DeepMath's
    accuracy:tool-use ratio of ``1.0 : 0.1``."""
    return weight if used_tool(sample) else 0.0


# ---------------------------------------------------------------------------
# Composite reward
# ---------------------------------------------------------------------------

def reward(
    sample: dict[str, Any],
    *,
    tool_weight: float = 0.0,
    parse_gold: GoldParser = default_parse_gold,
) -> Optional[float]:
    """Composite GRPO reward for one sample.

    ``sample`` carries at least:

    * ``verdict``: the verifier output ``{compiled, axioms_ok}`` (or a bool).
    * ``gold``: the obligation/statement to verify against.
    * optionally ``tool_calls`` / ``tools_used`` / ``used_tool`` for shaping.

    Returns:

    * ``None`` -- SKIP the sample: the gold is missing/unparseable (DeepMath's
      "don't punish, drop it"), so there is nothing to score against.
    * ``1.0`` -- proof compiled and axiom gate clean.
    * ``0.0`` -- proof failed the gate.

    plus ``tool_weight`` (default ``0.0``, i.e. correctness-only) added when the
    trace legitimately used a tool. Pass ``tool_weight=0.1`` to enable DeepMath's
    tool-use shaping. Correctness stays the dominant term.
    """
    if parse_gold(sample.get("gold")) is None:
        return None
    base = correctness_reward(sample.get("verdict"))
    if base is None:
        # gold present but nothing was verified -> also skip (no signal).
        return None
    return base + tool_use_reward(sample, tool_weight)


def make_reward_fn(*, tool_weight: float = TOOL_USE_WEIGHT) -> Callable[[dict[str, Any]], Optional[float]]:
    """Build a single-arg reward callable with tool-use shaping pre-bound --
    the shape a GRPO trainer's ``reward_funcs`` expects. Defaults to the
    DeepMath ``0.1`` shaping weight."""

    def _fn(sample: dict[str, Any]) -> Optional[float]:
        return reward(sample, tool_weight=tool_weight)

    return _fn


# ---------------------------------------------------------------------------
# Aggregation helpers (over k samples for one problem)
# ---------------------------------------------------------------------------

def pass_at_k(verdicts: Sequence[Verdict]) -> float:
    """``1.0`` if ANY of the ``k`` sampled proofs passed the gate, else ``0.0``.
    Verdicts that are ``None`` (unchecked) are ignored."""
    for v in verdicts:
        if _verdict_pass(v):
            return 1.0
    return 0.0


def majority_at_k(answers: Sequence[Any]) -> Optional[Any]:
    """The most common answer among ``k`` samples (majority vote), or ``None``
    for an empty input. Non-hashable answers are keyed by their string form
    (DeepMath ``metrics.majority`` hashability handling), and the original
    object of the winning key is returned. Ties break toward the
    earliest-seen answer."""
    if not answers:
        return None
    counts: Counter[str] = Counter()
    first_obj: dict[str, Any] = {}
    order: dict[str, int] = {}
    for i, a in enumerate(answers):
        key = a if isinstance(a, (str, int, float, bool)) else repr(a)
        key = str(key)
        counts[key] += 1
        if key not in first_obj:
            first_obj[key] = a
            order[key] = i
    # max by (count, earliest index) -> highest count, tie to earliest seen.
    best_key = max(counts, key=lambda k: (counts[k], -order[k]))
    return first_obj[best_key]


def majority_pass_at_k(samples: Sequence[dict[str, Any]]) -> float:
    """Verifier-native majority@k: ``1.0`` if the majority *verdict* over the
    ``k`` samples is a pass, else ``0.0``. Each sample contributes its
    ``verdict``; ``None`` verdicts vote "fail" (unchecked is not a pass)."""
    if not samples:
        return 0.0
    votes = [bool(_verdict_pass(s.get("verdict"))) for s in samples]
    return 1.0 if votes.count(True) > votes.count(False) else 0.0
