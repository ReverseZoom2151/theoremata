"""Recall / specificity AUDIT of the statement round-trip consistency validator.

Ports the measurement discipline behind FormaRL's warning that **"CC specificity
collapses under pass@k"** — the Consistency Check (does the Lean statement mean
the same thing as the NL problem?) is a *filter*, and a filter is only as good
as its precision. A CC judge that waves through perturbed statements is a reward
hack waiting to happen: with enough draws per problem some junk statement flukes
a ``true``. Before spending any RL budget you want to know, offline, how leaky
your CC filter actually is.

This module measures that leak with a **perturb-and-detect** protocol built on
top of :mod:`theoremata_tools.statement_roundtrip` (the CC signal it audits):

1. take ``(nl, faithful_lean)`` pairs where the Lean statement is a *faithful*
   encoding of the NL problem;
2. apply deterministic, taxonomy-keyed **perturbations** to each faithful Lean
   statement — drop a hypothesis, flip a quantifier, weaken a bound, swap a
   relation direction, rename a constant. Each perturbation makes the statement
   mean something DIFFERENT, so a faithful validator SHOULD reject it;
3. measure two rates:

   * **recall** — fraction of the *faithful* pairs the validator passes (a good
     validator keeps the true statements; recall < 1 means it is over-eager to
     reject);
   * **specificity** — fraction of the *perturbed* variants the validator
     correctly rejects, broken down per perturbation kind (specificity < 1 means
     the filter leaks — the reward-hack surface).

It also reports the **pass@k specificity-collapse** metric: from an attacker's
view, if ANY of the best-of-k perturbed variants slips past the validator the
attack succeeds, so ``specificity@k`` = fraction of pairs whose *every* one of
the first ``k`` perturbed variants was caught. It is monotonically
non-increasing in ``k`` — the more shots the adversary takes, the more the
filter's effective specificity collapses. This reproduces FormaRL's qualitative
warning as a concrete, offline number.

Design notes:

* The validator is INJECTED as a callable ``validate(nl, lean) -> bool | report``
  (default = our :func:`theoremata_tools.statement_roundtrip.roundtrip_validate`,
  normalised through the same CC rule the reward uses: ``verdict != mismatch and
  score >= threshold``). Injecting it keeps the audit deterministic and lets a
  test drive it with mock validators.
* Everything here is **offline and validator-agnostic**. It measures whatever
  validator you hand it; it does not itself judge faithfulness. A live audit
  against the real LLM judge is model-gated — this reproduces the *shape* of the
  collapse deterministically so it can run in CI before any RL spend.
* All inputs are UNTRUSTED DATA: only the surface of each statement is edited /
  inspected; embedded instructions are never executed.
"""
from __future__ import annotations

import json
import re
import sys
from typing import Any, Callable, Optional, Union

# Taxonomy constants come from the validator being audited so the per-kind
# report speaks the same language as the divergence taxonomy. Defensive fallback
# keeps this module importable even if the validator moves.
try:  # pragma: no cover - trivial import shim
    from theoremata_tools.statement_roundtrip import (
        ADDED_CONSTRAINT,
        DROPPED_HYPOTHESIS,
        LEXICAL_DRIFT,
        MISMATCH,
        NEGATION_MISMATCH,
        QUANTIFIER_FLIP,
        RELATION_FLIP,
    )
except Exception:  # pragma: no cover
    MISMATCH = "mismatch"
    QUANTIFIER_FLIP = "quantifier-flip"
    RELATION_FLIP = "relation-flip"
    DROPPED_HYPOTHESIS = "dropped-hypothesis"
    ADDED_CONSTRAINT = "added-constraint"
    NEGATION_MISMATCH = "negation-mismatch"
    LEXICAL_DRIFT = "lexical-drift"

OP = "roundtrip_audit"

# Mirrors formalization_reward.DEFAULT_CC_THRESHOLD — the CC pass cutoff we audit.
DEFAULT_THRESHOLD = 0.6

# A validator maps (nl, lean) -> either a bool "is faithful/consistent" or a
# round-trip report dict ({verdict, score, ...}) which we normalise.
Validator = Callable[[str, str], Union[bool, dict]]

_KEYWORDS = frozenset({
    "theorem", "lemma", "example", "def", "by", "sorry", "fun", "let", "in",
    "nat", "int", "real", "rat", "prop", "type", "sort", "forall", "exists",
})

_IDENT = r"[A-Za-z_][A-Za-z0-9_']*"


# --------------------------------------------------------------------------- #
# Deterministic perturbations — each keyed to a divergence-taxonomy kind
# --------------------------------------------------------------------------- #

def _first_glyph(text: str, glyphs: tuple[str, ...]) -> Optional[tuple[int, str]]:
    """Index + glyph of the earliest-occurring member of ``glyphs`` in ``text``."""
    best: Optional[tuple[int, str]] = None
    for g in glyphs:
        i = text.find(g)
        if i != -1 and (best is None or i < best[0]):
            best = (i, g)
    return best


def _drop_hypothesis(s: str) -> str:
    """Remove the first antecedent ``H →`` (a dropped premise).

    Locates the first implication arrow and deletes the clause feeding it, back
    to the nearest preceding ``,`` or ``:`` (so a leading binder/quantifier and
    the theorem-type colon are preserved). ``∀ x, H → C`` becomes ``∀ x, C``.
    """
    for arrow in ("→", "->"):
        i = s.find(arrow)
        if i == -1:
            continue
        cut = max(s.rfind(":", 0, i), s.rfind(",", 0, i))
        start = 0 if cut == -1 else cut + 1
        return s[:start] + " " + s[i + len(arrow):]
    return s  # no hypothesis to drop


def _flip_quantifier(s: str) -> str:
    """Swap the first ∀ ↔ ∃ (a reversed quantifier).

    Falls back to injecting an existential after the theorem-type colon when the
    statement carries no explicit quantifier glyph (its universals are implicit
    binders), which the validator sees as an added quantifier.
    """
    hit = _first_glyph(s, ("∀", "∃"))
    if hit is not None:
        i, ch = hit
        repl = "∃" if ch == "∀" else "∀"
        return s[:i] + repl + s[i + 1:]
    if ":" in s:
        return s.replace(":", ": ∃ _q,", 1)
    return s


# Strictness change (weaken/tighten a bound): ≤ ↔ < , ≥ ↔ >.
_BOUND_WEAKEN = {"≤": "<", "<": "≤", "≥": ">", ">": "≥"}
# Direction change (swap a relation): ≤ ↔ ≥ , < ↔ > , = ↔ ≠.
_REL_SWAP = {"≤": "≥", "≥": "≤", "<": ">", ">": "<", "=": "≠", "≠": "="}

_NUM = re.compile(r"\d+")


def _weaken_bound(s: str) -> str:
    """Flip the strictness of the first bound (``≤``↔``<``, ``≥``↔``>``).

    Falls back to bumping the first numeric literal by one when no bound glyph is
    present (a changed threshold value).
    """
    hit = _first_glyph(s, tuple(_BOUND_WEAKEN))
    if hit is not None:
        i, g = hit
        return s[:i] + _BOUND_WEAKEN[g] + s[i + 1:]
    m = _NUM.search(s)
    if m:
        bumped = str(int(m.group(0)) + 1)
        return s[:m.start()] + bumped + s[m.end():]
    return s


def _swap_relation(s: str) -> str:
    """Reverse the direction of the first relation (``≤``↔``≥``, ``<``↔``>``, ``=``↔``≠``)."""
    hit = _first_glyph(s, tuple(_REL_SWAP))
    if hit is not None:
        i, g = hit
        return s[:i] + _REL_SWAP[g] + s[i + 1:]
    return s


def _rename_constant(s: str) -> str:
    """Rename the first binder variable (or first non-keyword identifier).

    All whole-word occurrences of the chosen name are renamed consistently to a
    fixed, distinct new name, so the statement now constrains a different object.
    """
    m = re.search(r"[({\[]\s*(" + _IDENT + r")\s*:", s)
    name = m.group(1) if m else None
    if name is None:
        for w in re.findall(_IDENT, s):
            if w.lower() not in _KEYWORDS:
                name = w
                break
    if not name:
        return s
    return re.sub(r"\b" + re.escape(name) + r"\b", "z" + name, s)


# Ordered so the strong, reliably-caught perturbations come first; the weaker,
# lexical ones (which a shallow validator leaks) come last — this is the order
# the pass@k collapse walks, surfacing the drop as the weak kinds enter.
_PERTURBATIONS: dict[str, tuple[Callable[[str], str], str]] = {
    "drop_hypothesis": (_drop_hypothesis, DROPPED_HYPOTHESIS),
    "flip_quantifier": (_flip_quantifier, QUANTIFIER_FLIP),
    "swap_relation": (_swap_relation, RELATION_FLIP),
    "weaken_bound": (_weaken_bound, RELATION_FLIP),
    "rename_constant": (_rename_constant, LEXICAL_DRIFT),
}

PERTURBATION_KINDS: tuple[str, ...] = tuple(_PERTURBATIONS)


def taxonomy_of(kind: str) -> str:
    """The divergence-taxonomy kind a perturbation is designed to induce."""
    try:
        return _PERTURBATIONS[kind][1]
    except KeyError:
        raise ValueError(f"unknown perturbation kind: {kind}") from None


def perturb(lean_statement: str, kind: str) -> str:
    """Apply a deterministic, taxonomy-keyed perturbation to a Lean statement.

    Parameters
    ----------
    lean_statement:
        The (faithful) Lean 4 theorem statement to corrupt. UNTRUSTED DATA — only
        its surface syntax is edited.
    kind:
        One of :data:`PERTURBATION_KINDS`: ``drop_hypothesis``,
        ``flip_quantifier``, ``swap_relation``, ``weaken_bound``,
        ``rename_constant``.

    Returns
    -------
    The perturbed statement. If the perturbation target is absent (e.g. no
    hypothesis to drop), the original string is returned unchanged — callers
    treat an unchanged result as "not generated / inapplicable".
    """
    try:
        fn, _ = _PERTURBATIONS[kind]
    except KeyError:
        raise ValueError(f"unknown perturbation kind: {kind}") from None
    return fn(str(lean_statement or ""))


# --------------------------------------------------------------------------- #
# Validator normalisation + default
# --------------------------------------------------------------------------- #

def _default_validate(nl: str, lean: str) -> dict:
    """Default validator = our round-trip consistency validator (a report dict)."""
    from theoremata_tools.statement_roundtrip import roundtrip_validate

    return roundtrip_validate(nl, lean)


def _is_pass(result: Union[bool, dict, Any], threshold: float) -> bool:
    """Normalise a validator result to a boolean "passed as faithful".

    A bare bool is taken as-is. A round-trip report dict is thresholded with the
    SAME rule the FormaRL CC reward uses: not a hard ``mismatch`` AND
    ``score >= threshold``. Anything else is coerced with ``bool``.
    """
    if isinstance(result, bool):
        return result
    if isinstance(result, dict):
        if "verdict" in result:
            score = result.get("score")
            score_ok = score is None or float(score) >= threshold
            return result["verdict"] != MISMATCH and score_ok
        if "passed" in result:
            return bool(result["passed"])
    return bool(result)


# --------------------------------------------------------------------------- #
# The audit
# --------------------------------------------------------------------------- #

def _coerce_pairs(pairs: Any) -> list[tuple[str, str]]:
    out: list[tuple[str, str]] = []
    for p in pairs or []:
        if isinstance(p, dict):
            nl = p.get("nl", p.get("informal", p.get("nl_problem", "")))
            lean = p.get("lean", p.get("lean_statement", p.get("faithful_lean", "")))
        else:
            nl, lean = p[0], p[1]
        out.append((str(nl or ""), str(lean or "")))
    return out


def audit(
    pairs: Any,
    *,
    validate: Validator = _default_validate,
    kinds: tuple[str, ...] = PERTURBATION_KINDS,
    threshold: float = DEFAULT_THRESHOLD,
) -> dict[str, Any]:
    """Audit a consistency validator's recall and per-kind specificity.

    Parameters
    ----------
    pairs:
        Iterable of ``(nl, faithful_lean)`` — 2-tuples or dicts (``{nl, lean}``).
        Each ``faithful_lean`` is assumed to be a correct encoding of ``nl``.
    validate:
        The validator under audit, ``(nl, lean) -> bool | report``. Defaults to
        :func:`theoremata_tools.statement_roundtrip.roundtrip_validate`; a report
        dict is thresholded via the CC rule. Inject a mock (or an LLM judge) here.
    kinds:
        Which perturbation kinds to exercise (default: all).
    threshold:
        CC pass cutoff applied when the validator returns a round-trip report.

    Returns
    -------
    ``{op, recall, specificity, per_kind, confusion, pass_at_k_specificity,
    n_pairs, n_perturbations, kinds, threshold, note}``.

    * ``recall`` — fraction of faithful pairs the validator passed.
    * ``specificity`` — fraction of perturbed variants correctly rejected.
    * ``per_kind[kind]`` — ``{taxonomy, generated, caught, specificity}``.
    * ``confusion`` — ``{tp, fn, tn, fp}`` (positives = faithful, negatives =
      perturbed).
    * ``pass_at_k_specificity[k]`` — specificity when the attacker takes the
      best of the first ``k`` perturbed variants (monotone non-increasing in k:
      the collapse metric).
    """
    data = _coerce_pairs(pairs)
    kinds = tuple(kinds)

    # Recall over faithful pairs.
    tp = 0
    for nl, lean in data:
        if _is_pass(validate(nl, lean), threshold):
            tp += 1
    n_pairs = len(data)
    fn = n_pairs - tp

    per_kind: dict[str, dict[str, Any]] = {
        k: {"taxonomy": taxonomy_of(k), "generated": 0, "caught": 0,
            "specificity": None}
        for k in kinds
    }

    # For pass@k: per pair, the ordered caught-flags of its applicable variants.
    per_pair_caught: list[list[bool]] = []
    total_perturbed = 0
    total_caught = 0

    for nl, lean in data:
        flags: list[bool] = []
        for k in kinds:
            variant = perturb(lean, k)
            if variant == lean:
                continue  # perturbation inapplicable -> not generated
            per_kind[k]["generated"] += 1
            total_perturbed += 1
            caught = not _is_pass(validate(nl, variant), threshold)
            flags.append(caught)
            if caught:
                per_kind[k]["caught"] += 1
                total_caught += 1
        per_pair_caught.append(flags)

    for k in kinds:
        g = per_kind[k]["generated"]
        if g:
            per_kind[k]["specificity"] = round(per_kind[k]["caught"] / g, 6)

    fp = total_perturbed - total_caught
    recall = round(tp / n_pairs, 6) if n_pairs else None
    specificity = round(total_caught / total_perturbed, 6) if total_perturbed else None

    # pass@k specificity collapse: attacker wins if ANY of the first k variants
    # slips past, so specificity@k = fraction of pairs whose first k variants
    # were ALL caught. Non-increasing in k.
    pass_at_k: dict[int, float] = {}
    max_k = len(kinds)
    for kk in range(1, max_k + 1):
        ok = 0
        considered = 0
        for flags in per_pair_caught:
            prefix = flags[:kk]
            if not prefix:
                continue
            considered += 1
            if all(prefix):
                ok += 1
        pass_at_k[kk] = round(ok / considered, 6) if considered else None

    return {
        "op": OP,
        "recall": recall,
        "specificity": specificity,
        "per_kind": per_kind,
        "confusion": {"tp": tp, "fn": fn, "tn": total_caught, "fp": fp},
        "pass_at_k_specificity": pass_at_k,
        "n_pairs": n_pairs,
        "n_perturbations": total_perturbed,
        "kinds": list(kinds),
        "threshold": threshold,
        "note": (
            "OFFLINE, validator-agnostic audit. It measures whatever validator "
            "is injected (default = the deterministic lexical round-trip); a live "
            "audit against the real LLM judge is model-gated. specificity < 1 or a "
            "declining pass_at_k marks the CC reward-hack surface FormaRL warns of."
        ),
    }


# --------------------------------------------------------------------------- #
# Worker dispatch
# --------------------------------------------------------------------------- #

def run(request: dict[str, Any]) -> dict[str, Any]:
    """Worker entrypoint (op ``roundtrip_audit``).

    ``{pairs: [[nl, lean], ...], threshold?, kinds?}`` -> :func:`audit`. A JSON
    request uses the default round-trip validator (callables are a Python-API
    affordance and cannot ride in JSON). An unknown op raises.
    """
    op = request.get("op", OP)
    if op != OP:
        raise ValueError(f"unknown op: {op}")
    kwargs: dict[str, Any] = {}
    if request.get("threshold") is not None:
        kwargs["threshold"] = float(request["threshold"])
    if request.get("kinds"):
        kwargs["kinds"] = tuple(request["kinds"])
    if callable(request.get("validate")):
        kwargs["validate"] = request["validate"]
    return audit(request.get("pairs", []), **kwargs)


def main() -> None:
    if len(sys.argv) >= 2:
        with open(sys.argv[1], encoding="utf-8") as fh:
            req = json.load(fh)
    else:
        req = json.load(sys.stdin)
    print(json.dumps(run(req), indent=2, ensure_ascii=False, default=str))
    raise SystemExit(0)


if __name__ == "__main__":
    main()
