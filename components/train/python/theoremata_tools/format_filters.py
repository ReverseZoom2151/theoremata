"""Format-collapse filters for training-data quality (Kimina-Prover).

Kimina-Prover's data pipeline curates the *rollouts* it trains on, not just the
*problems* it samples. Two distinct filters are easy to conflate:

* :func:`curriculum_synth.beam_self_filter` drops easy **problems** (the policy
  already solves them consistently) so the corpus trends harder. It is a
  problem-level self-filter.
* THIS module drops degenerate **rollouts / samples** for the problems that
  survive: proofs that collapsed into a bad *format* (no real tactic content, a
  final proof the model never actually reasoned toward) and rollouts that would
  push the gradient the wrong way (below the group's reward quantile, or an
  all-fail group with no positive to contrast against).

Kimina reports two format-collapse signals that gate SFT/RL data:

* **Tactic-block presence** -- a "proof" that is just a term with no ``by`` /
  tactic section is a format collapse; keep only samples with >= 1 real tactic
  block (:func:`has_tactic_block`).
* **Snippet coverage of the final proof** -- the fraction of the final proof's
  tactic lines that actually appear in the model's reasoning / chain-of-thought.
  A high-reward proof whose tactics never show up in the CoT is a lucky
  format artifact, not learned reasoning; gate on coverage
  (:func:`snippet_coverage` / :func:`passes_coverage`).

Plus a group-level learning-signal filter:

* **Negative-gradient drop** -- within a group of rollouts for ONE problem, drop
  the samples that contribute a negative or zero learning signal: below the
  ``omega`` quantile of the group reward, or the whole group when it is all-fail
  / has no reward variance (every advantage is zero)
  (:func:`drop_negative_gradient`).

:func:`format_filter` composes all three and returns the survivors plus a
per-sample drop reason. Everything here is deterministic and offline (no model,
no GPU, no wall-clock, no randomness): it is a *curation* layer that shapes what
the GPU-gated SFT/RL later trains on.

Sample-row contract
-------------------
A *sample* / *rollout* is a dict carrying, best-effort:

* the final proof under ``final_proof`` (or ``proof``);
* the model reasoning / CoT under ``reasoning`` (or ``cot`` / ``thought`` /
  ``chain_of_thought``);
* a scalar reward under ``reward`` (or ``score``), else derived from a
  ``verdict`` ``{compiled, axioms_ok}`` / a ``verified`` flag;
* the problem it belongs to under ``problem`` (or ``goal`` / ``statement``),
  used only to group rollouts for the negative-gradient filter.
"""
from __future__ import annotations

import json
import math
import re
import sys
from typing import Any, Optional, Sequence

__all__ = [
    "has_tactic_block",
    "snippet_coverage",
    "passes_coverage",
    "drop_negative_gradient",
    "format_filter",
    "run",
]

# Clearly-tactic keywords: a line whose first token is one of these is tactic
# content even without an explicit ``by`` (e.g. inside a ``begin ... end`` block
# or a continued tactic sequence). Deliberately excludes ambiguous term/tactic
# dual-use words (``have``/``show``/``let``/``calc``) so a pure term proof is not
# misread as tactic content.
_TACTIC_KEYWORDS = frozenset(
    {
        "intro", "intros", "exact", "exact?", "apply", "apply?", "simp", "simpa",
        "rw", "rewrite", "erw", "linarith", "nlinarith", "polyrith", "ring",
        "ring_nf", "omega", "norm_num", "norm_cast", "push_cast", "field_simp",
        "positivity", "gcongr", "constructor", "cases", "rcases", "obtain",
        "induction", "refine", "use", "assumption", "trivial", "decide",
        "contradiction", "by_contra", "by_cases", "tauto", "aesop", "exfalso",
        "left", "right", "split", "subst", "unfold", "convert", "specialize",
        "rfl", "simp_all", "fin_cases", "interval_cases", "bound", "nlinari",
    }
)

# A `by`-tactic block, a Lean 3 `begin ... end` block.
_BY_RE = re.compile(r"\bby\b")
_BEGIN_RE = re.compile(r"\bbegin\b")
_FIRST_WORD_RE = re.compile(r"^([A-Za-z_][A-Za-z0-9_']*)")

# Structural line tokens that carry no tactic snippet on their own.
_STRUCTURAL = frozenset({"by", "begin", "end", "{", "}", "·", "|"})


# ---------------------------------------------------------------------------
# Field extraction (tolerant row contract)
# ---------------------------------------------------------------------------

def _proof_text(sample: Any) -> str:
    if isinstance(sample, str):
        return sample
    if isinstance(sample, dict):
        return str(sample.get("final_proof") or sample.get("proof") or "")
    return ""


def _reasoning_text(sample: Any) -> str:
    if isinstance(sample, dict):
        for key in ("reasoning", "cot", "thought", "think", "chain_of_thought"):
            v = sample.get(key)
            if v:
                return str(v)
    return ""


def _sample_reward(sample: dict[str, Any]) -> float:
    """Extract a scalar reward for one rollout, deterministically.

    Prefers an explicit ``reward`` / ``score``; else derives a binary reward from
    a ``verdict`` ``{compiled, axioms_ok}`` dict / bare bool, or a
    ``verified`` / ``passed`` / ``solved`` flag. Missing -> ``0.0``."""
    for key in ("reward", "score"):
        v = sample.get(key)
        if isinstance(v, bool):
            return 1.0 if v else 0.0
        if isinstance(v, (int, float)):
            return float(v)
    verdict = sample.get("verdict")
    if isinstance(verdict, dict):
        return 1.0 if (verdict.get("compiled") and verdict.get("axioms_ok", True)) else 0.0
    if verdict is not None:
        return 1.0 if verdict else 0.0
    for key in ("verified", "passed", "solved"):
        if key in sample:
            return 1.0 if sample[key] else 0.0
    return 0.0


def _problem_key(sample: dict[str, Any]) -> str:
    return str(sample.get("problem") or sample.get("goal") or sample.get("statement") or "")


# ---------------------------------------------------------------------------
# Tactic-line extraction
# ---------------------------------------------------------------------------

def _normalize_tactic_line(line: str) -> str:
    """Strip a proof line down to its tactic snippet: drop a leading ``:=`` /
    ``by`` scaffold and surrounding whitespace. Returns ``""`` for a purely
    structural line (``by`` / ``begin`` / ``end`` / a lone brace)."""
    s = line.strip()
    if not s:
        return ""
    # Drop a leading term-mode `:=` assignment arrow.
    if s.startswith(":="):
        s = s[2:].strip()
    # Peel a leading `by` (with or without a following tactic on the same line).
    m = re.match(r"by\b(.*)", s)
    if m:
        s = m.group(1).strip()
    if not s or s in _STRUCTURAL:
        return ""
    return s


def _tactic_lines(proof: str) -> list[str]:
    """The list of non-structural tactic snippets in a proof body (order
    preserved, structural-only lines removed)."""
    out: list[str] = []
    for raw in proof.splitlines():
        snip = _normalize_tactic_line(raw)
        if snip:
            out.append(snip)
    return out


# ---------------------------------------------------------------------------
# Filter 1: tactic-block presence
# ---------------------------------------------------------------------------

def has_tactic_block(sample: Any) -> bool:
    """``True`` iff the sample's proof contains >= 1 real tactic block.

    A tactic block is a ``by`` block, a Lean 3 ``begin ... end`` block, or a line
    that opens with a recognized tactic keyword. A "proof" that is just a term
    with no tactic content (e.g. ``lt_trans h1 h2``) has none and is rejected --
    the Kimina format-collapse signal. Accepts a sample dict or a raw proof
    string."""
    proof = _proof_text(sample)
    if not proof.strip():
        return False
    if _BY_RE.search(proof) or _BEGIN_RE.search(proof):
        return True
    for raw in proof.splitlines():
        m = _FIRST_WORD_RE.match(raw.strip())
        if m and m.group(1) in _TACTIC_KEYWORDS:
            return True
    return False


# ---------------------------------------------------------------------------
# Filter 2: snippet coverage of the final proof
# ---------------------------------------------------------------------------

def snippet_coverage(reasoning: str, final_proof: str) -> float:
    """Fraction of the final proof's tactic lines that actually appear in the
    model's reasoning / CoT (case-insensitive substring match).

    ``1.0`` means every tactic line of the proof was reasoned toward; a low value
    flags a proof whose tactics were never mentioned in the CoT (a lucky format
    artifact, not learned reasoning). A proof with no tactic lines to cover is
    vacuously ``1.0`` (the tactic-block filter handles empties separately)."""
    lines = _tactic_lines(final_proof or "")
    if not lines:
        return 1.0
    hay = (reasoning or "").lower()
    matched = sum(1 for ln in lines if ln.lower() in hay)
    return matched / len(lines)


def passes_coverage(sample: dict[str, Any], threshold: float = 0.6) -> bool:
    """Gate a sample on :func:`snippet_coverage` ``>= threshold`` (Kimina's
    snippet-coverage-of-final-proof filter; default ``0.6``)."""
    cov = snippet_coverage(_reasoning_text(sample), _proof_text(sample))
    return cov >= threshold


# ---------------------------------------------------------------------------
# Filter 3: negative-gradient drop (group of rollouts for one problem)
# ---------------------------------------------------------------------------

def _quantile(sorted_vals: Sequence[float], q: float) -> float:
    """Deterministic linear-interpolation quantile of an already-sorted list."""
    n = len(sorted_vals)
    if n == 0:
        return 0.0
    if n == 1:
        return sorted_vals[0]
    pos = max(0.0, min(1.0, q)) * (n - 1)
    lo = int(math.floor(pos))
    hi = int(math.ceil(pos))
    if lo == hi:
        return sorted_vals[lo]
    frac = pos - lo
    return sorted_vals[lo] * (1.0 - frac) + sorted_vals[hi] * frac


def drop_negative_gradient(
    samples: Sequence[dict[str, Any]], *, omega: float = 0.5
) -> list[dict[str, Any]]:
    """Within a group of rollouts for ONE problem, drop the samples that would
    contribute a negative or zero learning signal.

    Deterministic. A sample is dropped when:

    * the group is **all-fail** (max reward ``<= 0``) -- no positive to learn
      toward, so the whole group is dropped; or
    * the group has **no reward variance** (all rewards equal) -- every GRPO
      advantage is zero, so the whole group is dropped; or
    * the sample's reward is **below the ``omega`` quantile** of the group reward
      (``omega=0.5`` -> below the median): the low tail whose advantage is
      negative.

    Rewards are read via the tolerant contract (:func:`_sample_reward`). Input
    order is preserved among survivors."""
    items = list(samples)
    if not items:
        return []
    rewards = [_sample_reward(s) for s in items]
    if max(rewards) <= 0.0:
        return []  # all-fail group: no positive learning signal
    if max(rewards) == min(rewards):
        return []  # zero variance: every advantage is 0
    threshold = _quantile(sorted(rewards), omega)
    return [s for s, r in zip(items, rewards) if r >= threshold]


# ---------------------------------------------------------------------------
# Composite
# ---------------------------------------------------------------------------

def format_filter(
    samples: Sequence[dict[str, Any]],
    *,
    coverage_threshold: float = 0.6,
    omega: float = 0.5,
) -> dict[str, Any]:
    """Apply all three format-collapse filters and return the survivors plus a
    per-sample drop reason.

    Order: per-sample gates first (:func:`has_tactic_block`, then
    :func:`passes_coverage`), then the group-level :func:`drop_negative_gradient`
    over the survivors grouped by problem. Returns
    ``{ok, n_in, n_kept, n_dropped, kept, dropped}`` where ``kept`` is the list
    of surviving samples (original order) and ``dropped`` is a list of
    ``{index, reason}`` with ``reason`` in
    ``{no_tactic_block, low_coverage, negative_gradient}``."""
    dropped: list[dict[str, Any]] = []
    survivors: list[tuple[int, dict[str, Any]]] = []
    for i, s in enumerate(samples):
        if not has_tactic_block(s):
            dropped.append({"index": i, "reason": "no_tactic_block"})
            continue
        if not passes_coverage(s, coverage_threshold):
            dropped.append({"index": i, "reason": "low_coverage"})
            continue
        survivors.append((i, s))

    # Group survivors by problem (first-seen order), run the negative-gradient
    # filter per group, keep survivors by object identity.
    groups: dict[str, list[tuple[int, dict[str, Any]]]] = {}
    for i, s in survivors:
        groups.setdefault(_problem_key(s), []).append((i, s))

    kept: list[tuple[int, dict[str, Any]]] = []
    for items in groups.values():
        keep = drop_negative_gradient([s for _, s in items], omega=omega)
        keep_ids = {id(x) for x in keep}
        for i, s in items:
            if id(s) in keep_ids:
                kept.append((i, s))
            else:
                dropped.append({"index": i, "reason": "negative_gradient"})

    kept.sort(key=lambda t: t[0])
    dropped.sort(key=lambda d: d["index"])
    return {
        "ok": True,
        "n_in": len(samples),
        "n_kept": len(kept),
        "n_dropped": len(dropped),
        "kept": [s for _, s in kept],
        "dropped": dropped,
    }


# ---------------------------------------------------------------------------
# Worker dispatch
# ---------------------------------------------------------------------------

def run(request: dict[str, Any]) -> dict[str, Any]:
    """Dispatch the format filter over a JSON request.

    Op ``format_filter`` -- ``{samples:[...], coverage_threshold?, omega?}`` ->
    the :func:`format_filter` result ``{ok, n_in, n_kept, n_dropped, kept,
    dropped}``."""
    op = request.get("op", "format_filter")
    if op == "format_filter":
        return format_filter(
            request.get("samples", []),
            coverage_threshold=float(request.get("coverage_threshold", 0.6)),
            omega=float(request.get("omega", 0.5)),
        )
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
