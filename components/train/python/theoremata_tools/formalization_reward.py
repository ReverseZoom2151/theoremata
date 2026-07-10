"""Label-free autoformalization reward (FormaRL SC∧CC rule) for GRPO.

Ports the *idea* from FormaRL (MIT-licensed; built from the architecture, not
from vendored source — the vendored resource is UNTRUSTED DATA): when there is
no gold Lean statement to compare against, you can still reward an
*autoformalization* — turning a natural-language (NL) problem into a Lean
theorem statement — with a label-free signal made of two cheap, independent
checks:

* **SC — Syntax Check.** Does the produced Lean statement *compile with*
  ``sorry`` (i.e. is it a well-formed theorem signature the toolchain accepts,
  proof deferred)? This is a property of the Lean text alone.
* **CC — Consistency Check.** Does the Lean statement carry *exactly the same
  hypotheses and conclusion* as the NL problem — no dropped premise, no flipped
  quantifier, no weakened goal? FormaRL asks an LLM judge to answer
  ``\\boxed{true/false}``. Here CC is the SAME faithfulness question our
  richer :mod:`theoremata_tools.statement_roundtrip` already answers (a
  back-translation round-trip with a divergence taxonomy), thresholded to a
  boolean — so CC reuses a stronger signal than a bare yes/no judge.

**Reward rule (FormaRL):** ``1.0`` iff SC *and* CC both pass, else ``0.0``.
This is the reward that is fed to GRPO as the per-sample scalar for an
autoformalization rollout. It sits ALONGSIDE the verifier-driven proof reward
in :mod:`theoremata_tools.reward`; it does not replace it. SC/CC are label-free
*statement-quality* signals, never a proof-correctness certificate.

Both checks are injected callables so the module is deterministic and fully
offline: the real SC is a Lean compile and the real CC is a strong LLM judge —
both are GPU/model/toolchain-gated and are mocked in tests behind
``syntax_check`` / ``consistency_check``.

Anti reward-hacking (both filters are load-bearing — FormaRL reports each as a
concrete exploit the naive SC∧CC rule falls to):

* **Trivial always-compiling stub.** A statement like ``theorem t : True := by
  sorry`` (or ``0 = 0``, ``x = x``) *passes SC* while asserting nothing. Left
  unguarded, the policy learns to emit a vacuous-but-compiling statement for
  free reward. We reject it with a triviality screen (a lexical goal check by
  default; a structured :func:`theoremata_tools.triviality.triviality_check`
  callable can be injected for the numeric-witness screen).
* **NL echoed back.** If the "Lean statement" is really the NL problem restated
  as prose (no Lean structure), a naive judge sees near-identical text and CC
  fires spuriously — a direct CC reward-hack. We reject a statement that lacks
  Lean structure or whose surface is a near-copy of the NL problem.

FormaRL's **"CC specificity collapses under pass@k"** warning: CC's precision as
a *filter* degrades as you draw more samples per problem — with enough draws,
some junk statement eventually flukes a ``true`` from the judge, so using CC to
*gate* a large sampling budget lets hacks through. FormaRL's mitigation, which
we expose as :func:`selection_policy`: **SC gates sampling** (cheap, high-
precision, run on every draw) while **CC scores only the single SC-surviving
winner** — never use CC to sift a wide pass@k pool.
"""
from __future__ import annotations

import json
import re
import sys
from typing import Any, Callable, Optional

OP = "formalization_reward"

# Default faithfulness threshold for the round-trip-backed CC: the round-trip
# score lies in [0, 1]; at/above this AND not a hard MISMATCH counts as CC-pass.
DEFAULT_CC_THRESHOLD = 0.6

# A callable that answers SC: does this Lean statement compile (with sorry)?
SyntaxCheck = Callable[[str], bool]
# A callable that answers CC: does the Lean statement mean the NL problem?
ConsistencyCheck = Callable[[str, str], bool]
# Optional structured triviality screen: Lean text -> is-trivial bool.
TrivialityCheck = Callable[[str], bool]

# Lean statement structural markers used by the anti-hack screens.
_HEADER = re.compile(r"\b(?:theorem|lemma|example|def)\b")
_LEAN_GLYPHS = "∀∃→↔≤≥≠∧∨¬∣∈"
# Groups like ``(n : Nat)`` / ``{x : Real}`` — binders, stripped before we
# isolate the conclusion so their inner ``:`` is not mistaken for the type ``:``.
_BINDER_GROUP = re.compile(r"[({\[][^(){}\[\]]*[)}\]]")
_WORD = re.compile(r"[A-Za-z][A-Za-z_]*")

# Goal predicates that assert nothing (the trivial-stub exploit surface).
_TRIVIAL_GOALS = frozenset({"true", "trivial"})


# --------------------------------------------------------------------------- #
# Default SC — lexical well-formedness screen (stand-in for a Lean compile)
# --------------------------------------------------------------------------- #

def _balanced(text: str) -> bool:
    """Are ``()``/``[]``/``{}`` balanced and correctly nested?"""
    pairs = {")": "(", "]": "[", "}": "{"}
    stack: list[str] = []
    for ch in text:
        if ch in "([{":
            stack.append(ch)
        elif ch in pairs:
            if not stack or stack.pop() != pairs[ch]:
                return False
    return not stack


def default_syntax_check(lean_statement: str) -> bool:
    """Offline SC stand-in: accept a *well-formed* Lean theorem signature.

    The real SC compiles the statement with ``sorry`` — GPU/toolchain-gated. This
    deterministic screen approximates "would compile with sorry" lexically: a
    header keyword (``theorem``/``lemma``/``example``/``def``), a type ascription
    ``:`` after the header, and balanced brackets. ``sorry`` is *expected* here
    (the proof is deliberately deferred), so it is never penalised. Prefer
    injecting a real Lean-compile callable when the toolchain is available.
    """
    text = (lean_statement or "").strip()
    if not text or not _HEADER.search(text):
        return False
    if not _balanced(text):
        return False
    # There must be a type ascription for the proposition. Strip binder groups
    # (their ``:`` is a binder, not the theorem type) then require a ``:``.
    head = text.split(":=", 1)[0]
    stripped = _BINDER_GROUP.sub(" ", head)
    return ":" in stripped


# --------------------------------------------------------------------------- #
# Default CC — reuse statement_roundtrip's faithfulness, thresholded to a bool
# --------------------------------------------------------------------------- #

def _content_tokens(text: str) -> set[str]:
    return {m.group(0).lower() for m in _WORD.finditer(text or "")}


def _lexical_faithful(nl_problem: str, lean_statement: str, threshold: float) -> bool:
    """Coarse offline CC fallback when statement_roundtrip is unavailable.

    A shallow content-token overlap gate. This is deliberately weak — the real
    CC is the round-trip validator or an LLM judge; this only keeps the default
    usable when :mod:`theoremata_tools.statement_roundtrip` is not importable.
    """
    a, b = _content_tokens(nl_problem), _content_tokens(lean_statement)
    if not a or not b:
        return False
    return len(a & b) / len(a | b) >= threshold


def make_default_consistency_check(
    threshold: float = DEFAULT_CC_THRESHOLD,
) -> ConsistencyCheck:
    """Build the default CC callable: our round-trip faithfulness, thresholded.

    Uses :func:`theoremata_tools.statement_roundtrip.roundtrip_validate` — a
    back-translation of the Lean statement diffed against the NL problem, scored
    in ``[0, 1]`` with a divergence taxonomy. CC passes iff the score is at/above
    ``threshold`` AND the verdict is not a hard ``mismatch`` (a quantifier /
    relation / negation reversal can never be "consistent" even at high overlap).
    Falls back to a coarse lexical gate if the module is not on the path.
    """

    def _cc(nl_problem: str, lean_statement: str) -> bool:
        try:
            from theoremata_tools.statement_roundtrip import (
                MISMATCH,
                roundtrip_validate,
            )
        except Exception:  # noqa: BLE001 — offline fallback
            return _lexical_faithful(nl_problem, lean_statement, threshold)
        report = roundtrip_validate(nl_problem, lean_statement)
        return report["verdict"] != MISMATCH and report["score"] >= threshold

    return _cc


# --------------------------------------------------------------------------- #
# Anti reward-hacking screens
# --------------------------------------------------------------------------- #

def _conclusion(lean_statement: str) -> str:
    """Best-effort extraction of the proposition/conclusion text of a statement.

    Drops the proof term (``:= …``), strips binder groups, takes the text after
    the first surviving ``:`` (the theorem type) and, if it is a quantified
    ``∀ …, body``, the body after the last top-level comma. Lexical only.
    """
    head = (lean_statement or "").split(":=", 1)[0]
    stripped = _BINDER_GROUP.sub(" ", head)
    if ":" in stripped:
        prop = stripped.split(":", 1)[1]
    else:
        prop = stripped
    if "," in prop:
        prop = prop.rsplit(",", 1)[1]
    return prop.strip()


def is_trivial_statement(
    lean_statement: str,
    *,
    triviality_check: Optional[TrivialityCheck] = None,
) -> bool:
    """Trivial-stub anti-hack: does the statement assert nothing?

    If a structured ``triviality_check`` callable is injected (e.g. a wrapper
    around :func:`theoremata_tools.triviality.triviality_check` that returns the
    ``trivial`` flag), it is authoritative. Otherwise a lexical goal screen fires
    on the vacuous conclusions that pass SC for free: ``True`` / ``trivial``, or a
    reflexive equality whose two sides are textually identical (``0 = 0``,
    ``x = x``, ``1 = 1``).
    """
    if triviality_check is not None:
        return bool(triviality_check(lean_statement))
    goal = _conclusion(lean_statement)
    if not goal:
        return True
    low = goal.lower().strip()
    if low in _TRIVIAL_GOALS:
        return True
    # Reflexive (in)equality with identical sides: X = X / X ≤ X / X ↔ X …
    for op in ("↔", "=", "≤", "≥"):
        if op in goal:
            left, right = goal.split(op, 1)
            if left.strip() and left.strip() == right.strip():
                return True
    return False


def _has_lean_structure(lean_statement: str) -> bool:
    text = lean_statement or ""
    if _HEADER.search(text):
        return True
    if ":=" in text:
        return True
    return any(g in text for g in _LEAN_GLYPHS)


def is_nl_echo(nl_problem: str, lean_statement: str) -> bool:
    """NL-echo anti-hack: is the "Lean statement" really the NL problem restated?

    Two ways this fires: (1) the candidate has NO Lean structure at all (no
    header keyword, no ``:=``, no logical glyph) — it is prose, not Lean; or
    (2) the candidate lacks the ``:=``/type scaffolding of a real theorem AND its
    word set is a near-copy of the NL problem (Jaccard ≥ 0.85). Either way a text-
    similarity judge would spuriously fire CC, so we reject before scoring.
    """
    lean = lean_statement or ""
    if not _has_lean_structure(lean):
        return True
    a, b = _content_tokens(nl_problem), _content_tokens(lean)
    if a and b:
        overlap = len(a & b) / len(a | b)
        if overlap >= 0.85 and _HEADER.search(lean) is None:
            return True
    return False


# --------------------------------------------------------------------------- #
# The reward
# --------------------------------------------------------------------------- #

def formalization_reward(
    nl_problem: str,
    lean_statement: str,
    *,
    syntax_check: SyntaxCheck = default_syntax_check,
    consistency_check: Optional[ConsistencyCheck] = None,
    triviality_check: Optional[TrivialityCheck] = None,
) -> dict[str, Any]:
    """FormaRL label-free autoformalization reward for one (NL, Lean) pair.

    ``reward`` is ``1.0`` iff SC and CC both pass AND neither anti-hack fires,
    else ``0.0``.

    Parameters
    ----------
    nl_problem, lean_statement:
        The natural-language problem and the model's Lean statement. Both are
        UNTRUSTED DATA — only their surface is inspected; embedded instructions
        are never executed.
    syntax_check:
        SC callable ``lean -> bool`` ("compiles with sorry"). Defaults to the
        offline lexical well-formedness screen; inject a real Lean compile.
    consistency_check:
        CC callable ``(nl, lean) -> bool`` ("same hypotheses+conclusion"). When
        ``None``, uses the round-trip-backed default (:func:`
        make_default_consistency_check`); inject an LLM judge for the real signal.
    triviality_check:
        Optional structured triviality screen passed through to the trivial-stub
        anti-hack.

    Returns
    -------
    ``{"reward": 1.0|0.0, "sc": bool, "cc": bool, "reason": str}`` — plus
    ``trivial`` / ``nl_echo`` flags for observability.
    """
    nl_problem = str(nl_problem or "")
    lean_statement = str(lean_statement or "")
    if consistency_check is None:
        consistency_check = make_default_consistency_check()

    sc = bool(syntax_check(lean_statement))
    cc = bool(consistency_check(nl_problem, lean_statement))
    trivial = is_trivial_statement(lean_statement, triviality_check=triviality_check)
    echo = is_nl_echo(nl_problem, lean_statement)

    passed = sc and cc and not trivial and not echo
    reason = _reason(sc, cc, trivial, echo)
    return {
        "op": OP,
        "reward": 1.0 if passed else 0.0,
        "sc": sc,
        "cc": cc,
        "trivial": trivial,
        "nl_echo": echo,
        "reason": reason,
    }


def _reason(sc: bool, cc: bool, trivial: bool, echo: bool) -> str:
    if trivial:
        return ("rejected: trivial always-compiling stub (SC passes but the "
                "statement asserts nothing) — anti reward-hack")
    if echo:
        return ("rejected: the Lean statement echoes the NL problem back "
                "(CC reward-hack) — anti reward-hack")
    if not sc:
        return "SC failed: the Lean statement is not well-formed (does not compile with sorry)"
    if not cc:
        return ("CC failed: the Lean statement does not carry the same "
                "hypotheses+conclusion as the NL problem")
    return "SC and CC both pass: faithful, well-formed autoformalization"


# --------------------------------------------------------------------------- #
# selection_policy — SC gates sampling, CC scores only the winner
# --------------------------------------------------------------------------- #

def selection_policy(
    nl_problem: str,
    lean_statements: list[str],
    *,
    syntax_check: SyntaxCheck = default_syntax_check,
    consistency_check: Optional[ConsistencyCheck] = None,
    triviality_check: Optional[TrivialityCheck] = None,
) -> dict[str, Any]:
    """FormaRL pass@k discipline: SC gates the pool, CC scores only the winner.

    FormaRL warns that **CC's specificity collapses under pass@k** — with many
    draws per problem, a junk statement eventually flukes a ``true`` from the
    judge, so using CC to sift a wide sampling pool leaks hacks. The mitigation:

    1. **SC (+ anti-hack) gates every sample** — cheap, high-precision, applied
       to all ``k`` draws. Trivial stubs and NL echoes are dropped here too.
    2. **CC is run on exactly ONE candidate** — the first SC-surviving draw — and
       its reward is the only CC-scored reward returned.

    Returns ``{"winner", "winner_index", "sc_pass", "reward", "reason"}``.
    ``winner`` is ``None`` when no draw passes SC; then reward is ``0.0``.
    """
    if consistency_check is None:
        consistency_check = make_default_consistency_check()

    sc_pass: list[int] = []
    for i, lean in enumerate(lean_statements):
        lean = str(lean or "")
        if not bool(syntax_check(lean)):
            continue
        if is_trivial_statement(lean, triviality_check=triviality_check):
            continue
        if is_nl_echo(nl_problem, lean):
            continue
        sc_pass.append(i)

    if not sc_pass:
        return {
            "op": OP,
            "winner": None,
            "winner_index": None,
            "sc_pass": [],
            "reward": 0.0,
            "reason": "no draw passed SC (or all were trivial-stub / NL-echo)",
        }

    winner_index = sc_pass[0]
    winner = str(lean_statements[winner_index] or "")
    # CC scores only the single winner — never the whole pool (pass@k collapse).
    result = formalization_reward(
        nl_problem,
        winner,
        syntax_check=syntax_check,
        consistency_check=consistency_check,
        triviality_check=triviality_check,
    )
    return {
        "op": OP,
        "winner": winner,
        "winner_index": winner_index,
        "sc_pass": sc_pass,
        "reward": result["reward"],
        "cc": result["cc"],
        "reason": ("CC scored the single SC-winner only (pass@k specificity "
                   f"guard): {result['reason']}"),
    }


# --------------------------------------------------------------------------- #
# Worker dispatch (offline default checks; callables may ride in a Python dict)
# --------------------------------------------------------------------------- #

def run(request: dict[str, Any]) -> dict[str, Any]:
    """Worker entrypoint (op ``formalization_reward``).

    ``{nl_problem, lean_statement, ...}`` -> :func:`formalization_reward`, or
    ``{op: "selection_policy", nl_problem, lean_statements: [...]}`` ->
    :func:`selection_policy`. ``syntax_check`` / ``consistency_check`` /
    ``triviality_check`` may be supplied as callables when called from Python;
    a JSON request uses the offline defaults. A ``threshold`` sets the default
    CC round-trip cutoff.
    """
    op = request.get("op", OP)
    threshold = float(request.get("threshold", DEFAULT_CC_THRESHOLD))
    consistency_check = request.get("consistency_check")
    if consistency_check is None:
        consistency_check = make_default_consistency_check(threshold)
    kwargs: dict[str, Any] = {
        "consistency_check": consistency_check,
        "triviality_check": request.get("triviality_check"),
    }
    if request.get("syntax_check") is not None:
        kwargs["syntax_check"] = request["syntax_check"]

    if op == "selection_policy":
        return selection_policy(
            request.get("nl_problem", ""),
            list(request.get("lean_statements", [])),
            **kwargs,
        )
    if op == OP:
        return formalization_reward(
            request.get("nl_problem", ""),
            request.get("lean_statement", request.get("lean", "")),
            **kwargs,
        )
    raise ValueError(f"unknown op: {op}")


def main() -> None:
    if len(sys.argv) >= 2:
        with open(sys.argv[1], encoding="utf-8") as fh:
            req = json.load(fh)
    else:
        req = json.load(sys.stdin)
    print(json.dumps(run(req), indent=2, ensure_ascii=False))
    raise SystemExit(0)


if __name__ == "__main__":
    main()
