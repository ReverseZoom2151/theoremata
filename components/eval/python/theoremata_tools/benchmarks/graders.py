"""Per-track graders for the unified benchmark harness (Tier 4).

Each grader returns the uniform verdict ``{is_solved, is_correct, detail}``:

* ``is_solved``  — the response engaged the task in a gradable way (an answer was
  extracted / a statement was produced / a verdict was rendered);
* ``is_correct`` — it is *right* by the track's rubric;
* ``detail``     — a dict explaining the decision (method, extracted values, …).

Tracks:

* :func:`grade_formalization` — Lean compile + axiom-whitelist + statement
  preservation. Shells out to ``leanprover/comparator`` when it is available
  (``$THEOREMATA_COMPARATOR``); otherwise degrades to a deterministic
  statement-string comparison + a ``sorry``/axiom-whitelist check.
* :func:`grade_nl_answer` — deterministic symbolic/integer/relation grading via
  the existing :mod:`theoremata_tools.grader`, with an LLM-judge fallback (the
  mock-capable provider) only when symbolic parsing is inconclusive.
* :func:`grade_falsification` — flaw / counterexample detection, or must-reject
  for the negative fixture.
"""
from __future__ import annotations

import os
import re
import shutil
from typing import Any, Callable

from theoremata_tools import grader as base_grader

# --------------------------------------------------------------------------- #
# Formalization track
# --------------------------------------------------------------------------- #

_SORRY_TOKENS = ("sorry", "sorryax", "admit")
_NON_WHITELIST_AXIOM_HINT = re.compile(
    r"axioms?\s*:?\s*\[?[^\]]*\b(sorryAx|[A-Z]\w*\.\w+)\b", re.IGNORECASE
)


def _normalize_lean(s: str) -> str:
    """Whitespace-insensitive normalization for statement comparison."""
    return re.sub(r"\s+", " ", (s or "")).strip()


def _statement_preserved(expected_formal: str, response: str) -> bool:
    """The expected statement (or its bound Lean name) must appear intact in the
    response — the anti-cheat "didn't weaken the theorem" check."""
    exp = _normalize_lean(expected_formal)
    resp = _normalize_lean(response)
    if not exp:
        return False
    if exp in resp:
        return True
    # Fall back to the signature up to the proof separator (ignore the `:= by`).
    exp_sig = _normalize_lean(re.split(r":=", expected_formal, maxsplit=1)[0])
    return bool(exp_sig) and exp_sig in resp


def _axioms_ok(response: str, whitelist: list[str]) -> tuple[bool, str]:
    """Reject any residual ``sorry`` (leaves sorryAx) or a non-whitelisted axiom
    named in a ``#print axioms`` block the caller pasted in."""
    low = response.lower()
    if any(tok in low for tok in _SORRY_TOKENS):
        return False, "residual_sorry_or_admit"
    wl = {w.lower() for w in whitelist}
    for m in _NON_WHITELIST_AXIOM_HINT.finditer(response):
        axiom = m.group(1)
        if axiom.lower() == "sorryax" or axiom.lower() not in wl:
            return False, f"non_whitelisted_axiom:{axiom}"
    return True, "axioms_ok"


def _comparator_path() -> str | None:
    env = os.environ.get("THEOREMATA_COMPARATOR")
    if env and os.path.exists(env):
        return env
    return shutil.which("comparator")


def grade_formalization(item: dict[str, Any], response: str) -> dict[str, Any]:
    expected = item.get("expected") or {}
    expected_formal = expected.get("formal_statement") or item.get("formal") or ""
    whitelist = expected.get("axioms_whitelist") or []
    response = response or ""

    comparator = _comparator_path()
    if comparator:
        # Comparator/landrun is Linux-only and needs a built Mathlib; we surface
        # its availability but still fall through to the deterministic gate if we
        # can't actually invoke it here. (Kept as a hook, per the spec recipe.)
        detail_tool = {"comparator": comparator, "invoked": False}
    else:
        detail_tool = {"comparator": None, "invoked": False}

    preserved = _statement_preserved(expected_formal, response)
    axioms_ok, axiom_reason = _axioms_ok(response, whitelist)
    is_correct = preserved and axioms_ok
    return {
        "is_solved": preserved,
        "is_correct": is_correct,
        "detail": {
            "track": "formalization",
            "method": "comparator" if comparator else "statement+axioms",
            "statement_preserved": preserved,
            "axioms_ok": axioms_ok,
            "axiom_reason": axiom_reason,
            "expected_lean_name": expected.get("lean_name"),
            **detail_tool,
        },
    }


# --------------------------------------------------------------------------- #
# NL / answer track
# --------------------------------------------------------------------------- #

# map the corpus answer_kind onto the base grader's routing keys
_KIND_ROUTE = {"integer": "integer", "relation": "relation", "bound": "symbolic",
               "symbolic": "symbolic"}

JudgeFn = Callable[[str, str], dict[str, Any]]

_BOUND_JUNK = ("$", "\\left", "\\right", "\\,", " ")


def _bound_value(s: str) -> str:
    """Normalize an IneqMath bound answer to its bare value: drop ``$``/spaces,
    a ``\\boxed{…}`` wrapper, and a leading ``C=`` so ``$$C = 3$$`` -> ``3``."""
    s = (s or "").strip()
    for junk in _BOUND_JUNK:
        s = s.replace(junk, "")
    m = re.match(r"\\boxed\{(.*)\}$", s)
    if m:
        s = m.group(1)
    s = re.sub(r"^[A-Za-z]+=", "", s)  # strip a leading "C=" style label
    return s.strip()


def _default_llm_judge(gold: str, pred: str) -> dict[str, Any]:
    """LLM-judge fallback for answer equivalence via the mock-capable provider.

    Uses the IneqMath rubric (exact forms only; decimals never equal exact).
    Deterministic in mock mode so tests never hit the network.
    """
    try:
        from theoremata_tools.model_provider import generate
    except Exception as exc:  # provider component not on path
        return {"equivalent": False, "reason": f"judge_unavailable:{exc}"}
    request = {
        "role": "answer_equivalence_judge",
        "task": (
            "Decide if the predicted answer is mathematically EQUIVALENT to the "
            "gold answer. Exact forms only: 1/2 == 0.5 is True, but a decimal "
            "approximation of an exact expression (2*pi vs 6.28) is False."
        ),
        "context": {"gold": gold, "pred": pred},
        "output_schema": {
            "type": "object",
            "required": ["equivalent"],
            "properties": {"equivalent": {"type": "boolean"},
                           "analysis": {"type": "string"}},
        },
    }
    try:
        content, model = generate(request)
        return {
            "equivalent": bool(content.get("equivalent")),
            "reason": f"llm_judge:{model}",
            "analysis": content.get("analysis"),
        }
    except Exception as exc:  # noqa: BLE001
        return {"equivalent": False, "reason": f"judge_error:{exc}"}


def grade_nl_answer(
    item: dict[str, Any],
    response: str,
    judge: JudgeFn | None = None,
) -> dict[str, Any]:
    expected = item.get("expected") or {}
    gold = str(expected.get("answer", ""))
    answer_kind = expected.get("answer_kind") or item.get("grading", {}).get(
        "answer_kind", "symbolic"
    )
    route = _KIND_ROUTE.get(answer_kind, "symbolic")

    extracted = base_grader.extract_answer(response)
    pred = extracted if extracted is not None else (response or "").strip()
    is_solved = bool(pred)

    # bound answers ("C = <value>") compare on the bare value, exact-string first
    if answer_kind == "bound":
        gold_v, pred_v = _bound_value(gold), _bound_value(pred)
        if gold_v and gold_v == pred_v:
            verdict = {"correct": True, "method": "exact_string"}
        else:
            verdict = base_grader.grade_answer(gold_v, pred_v, "symbolic")
    else:
        verdict = base_grader.grade_answer(gold, pred, route)
    is_correct = bool(verdict["correct"])
    method = verdict["method"]

    # LLM-judge fallback ONLY when the deterministic symbolic path was
    # inconclusive (parse error) and we're on a symbolic/bound answer.
    if (
        not is_correct
        and route == "symbolic"
        and str(method).startswith(("parse_error", "sympy_unavailable", "structural"))
    ):
        jfn = judge or _default_llm_judge
        j = jfn(gold, pred)
        if j.get("equivalent"):
            is_correct = True
        method = f"{method}->{j.get('reason', 'llm_judge')}"

    return {
        "is_solved": is_solved,
        "is_correct": is_correct,
        "detail": {
            "track": "nl_answer",
            "method": method,
            "answer_kind": answer_kind,
            "gold": gold,
            "extracted": pred,
        },
    }


# --------------------------------------------------------------------------- #
# Falsification / critic track
# --------------------------------------------------------------------------- #

_DETECT_KEYWORDS = (
    "counterexample", "counter-example", "is false", "is incorrect", "not true",
    "does not hold", "cannot be proven", "flaw", "refute", "refuted", "disprove",
    "disproved", "false statement", "the claim is false", "reject", "rejected",
    "no such", "contradiction", "not valid", "invalid",
)
_ACCEPT_KEYWORDS = (
    "qed", "proof complete", "we have proven", "hence proved", "therefore proven",
    "the proof is valid", "compiles", "verified", "is proven",
)


def _has_any(text: str, needles: tuple[str, ...]) -> list[str]:
    low = text.lower()
    return [n for n in needles if n in low]


def _structured_verdict(response: Any) -> str | None:
    """Accept a structured ``{"verdict": ...}`` response in addition to prose."""
    if isinstance(response, dict):
        v = str(response.get("verdict", "")).lower()
        if v:
            return v
    return None


def grade_falsification(item: dict[str, Any], response: Any) -> dict[str, Any]:
    expected = item.get("expected") or {}
    mode = expected.get("mode", "detect_flaw")
    text = response if isinstance(response, str) else str(
        (response or {}).get("text", response) if isinstance(response, dict) else response
    )
    verdict = _structured_verdict(response)
    detected = _has_any(text, _DETECT_KEYWORDS)
    accepted = _has_any(text, _ACCEPT_KEYWORDS)

    if mode == "reject":
        # Negative fixture: correct iff the response rejects / refuses.
        is_reject = verdict in {"reject", "rejected", "invalid", "false"} or bool(detected)
        # An explicit acceptance without rejection is the failure mode.
        if accepted and not is_reject:
            is_reject = False
        return {
            "is_solved": bool(text.strip()) or verdict is not None,
            "is_correct": bool(is_reject),
            "detail": {
                "track": "falsification",
                "mode": "reject",
                "detected_markers": detected,
                "accept_markers": accepted,
                "verdict": verdict,
            },
        }

    # detect_flaw (brokenmath): correct iff the flaw is detected and it is NOT
    # falsely "proven".
    flagged = verdict in {"flawed", "false", "detected", "reject", "counterexample"} or bool(
        detected
    )
    falsely_proven = bool(accepted) and not flagged
    is_correct = flagged and not falsely_proven
    return {
        "is_solved": bool(text.strip()) or verdict is not None,
        "is_correct": bool(is_correct),
        "detail": {
            "track": "falsification",
            "mode": "detect_flaw",
            "detected_markers": detected,
            "accept_markers": accepted,
            "verdict": verdict,
            "falsely_proven": falsely_proven,
        },
    }


# --------------------------------------------------------------------------- #
# Dispatch by track
# --------------------------------------------------------------------------- #

def grade(item: dict[str, Any], response: Any, **kw: Any) -> dict[str, Any]:
    """Grade a response against an item, routing by ``item['kind']``."""
    kind = item.get("kind")
    if kind == "formalization":
        return grade_formalization(item, response if isinstance(response, str) else str(response))
    if kind == "nl_answer":
        return grade_nl_answer(
            item, response if isinstance(response, str) else str(response),
            judge=kw.get("judge"),
        )
    if kind == "falsification":
        return grade_falsification(item, response)
    raise ValueError(f"cannot grade item of kind {kind!r}")
