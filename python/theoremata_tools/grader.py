"""Deterministic-first evaluation grader (Theoremata plan section 11).

Two-pipeline, answer-style grading routed by ``grade_kind`` plus a six-axis
fold that keeps evaluation dimensions separate (never one scalar). Symbolic
equivalence uses SymPy with the IneqMath rule: a decimal is equal to an exact
form only when it is *exactly* that value (0.5 == 1/2, but 6.28 != 2*pi).
"""
from __future__ import annotations

import json
import math
import re
import statistics
import sys
from collections import Counter
from typing import Any

_MARKERS = ("final answer is", "answer is", "final answer")


def extract_answer(text: str) -> str | None:
    """Return the answer following the last answer-marker, else the last
    balanced ``\\boxed{...}``, else None."""
    if not text:
        return None
    lowered = text.lower()
    best = -1
    marker_len = 0
    for marker in _MARKERS:
        idx = lowered.rfind(marker)
        if idx > best:
            best = idx
            marker_len = len(marker)
    if best >= 0:
        tail = text[best + marker_len :].strip()
        tail = tail.lstrip(":= ").strip()
        # stop at a sentence break / newline
        tail = re.split(r"[\n]", tail, maxsplit=1)[0].strip()
        return tail.rstrip(".").strip() or None

    boxed = _last_boxed(text)
    return boxed


def _last_boxed(text: str) -> str | None:
    marker = r"\boxed"
    start = text.rfind(marker)
    if start < 0:
        return None
    brace = text.find("{", start)
    if brace < 0:
        return None
    depth = 0
    for i in range(brace, len(text)):
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                return text[brace + 1 : i].strip()
    return None


def _clean(s: str) -> str:
    s = s.strip()
    for junk in ("$", "\\left", "\\right", "\\,", " "):
        s = s.replace(junk, "")
    return s.strip()


def _as_int(s: str) -> int | None:
    t = _clean(s).replace(",", "")
    try:
        return int(t)
    except ValueError:
        pass
    try:
        f = float(t)
    except ValueError:
        return None
    return int(f) if f.is_integer() else None


_REL_MAP = [
    (("\\geq", "≥", ">="), ">="),
    (("\\leq", "≤", "<="), "<="),
    (("\\neq", "≠", "!="), "none"),
    (("=",), "="),
    ((">",), ">"),
    (("<",), "<"),
]


def _canon_relation(s: str) -> str:
    t = _clean(s).lower()
    if "noneoftheabove" in t or t in {"none", "f", "(f)"}:
        return "none"
    for tokens, canon in _REL_MAP:
        if any(tok in t for tok in tokens):
            return canon
    return "none"


def _symbolic_equal(gold: str, pred: str) -> tuple[bool, str]:
    try:
        from sympy import simplify, sympify
    except Exception as exc:  # pragma: no cover - sympy is a declared dep
        return False, f"sympy_unavailable:{exc}"

    def prep(s: str) -> str:
        s = s.strip()
        for junk in ("$", "\\left", "\\right", "\\,"):
            s = s.replace(junk, "")
        return s.replace("^", "**")

    try:
        # rational=True turns 0.5 into Rational(1, 2) and 6.28 into 157/25, so
        # equality is exact: 0.5 == 1/2, but 6.28 != 2*pi.
        a = sympify(prep(gold), rational=True)
        b = sympify(prep(pred), rational=True)
    except Exception as exc:
        return False, f"parse_error:{exc}"
    try:
        return bool(simplify(a - b) == 0), "symbolic_exact"
    except Exception as exc:
        return bool(a == b), f"structural:{exc}"


def grade_answer(gold: str, pred: str, kind: str) -> dict[str, Any]:
    gold = "" if gold is None else str(gold)
    pred = "" if pred is None else str(pred)
    if kind == "integer":
        g, p = _as_int(gold), _as_int(pred)
        correct = g is not None and p is not None and g == p
        method = "integer_eq"
    elif kind == "relation":
        correct = _canon_relation(gold) == _canon_relation(pred)
        method = "relation_canon"
    elif kind == "symbolic":
        correct, method = _symbolic_equal(gold, pred)
    else:
        raise ValueError(f"unknown grade kind: {kind}")
    return {
        "correct": bool(correct),
        "kind": kind,
        "gold": gold,
        "pred": pred,
        "method": method,
    }


def _stderr(values: list[float]) -> float:
    if len(values) < 2:
        return 0.0
    return statistics.stdev(values) / math.sqrt(len(values))


def grade_samples(gold: str, preds: list[str], kind: str) -> dict[str, Any]:
    per_sample = [grade_answer(gold, p, kind) for p in preds]
    flags = [1.0 if r["correct"] else 0.0 for r in per_sample]
    k = len(preds)

    # majority vote over the raw (cleaned) predictions, then grade the winner.
    majority_correct = False
    majority_answer = None
    if preds:
        counts = Counter(_clean(str(p)) for p in preds)
        majority_answer = counts.most_common(1)[0][0]
        majority_correct = grade_answer(gold, majority_answer, kind)["correct"]

    return {
        "k": k,
        "pass_at_k": any(flags),
        "majority_at_k": bool(majority_correct),
        "majority_answer": majority_answer,
        "averaged_at_k": (sum(flags) / k) if k else 0.0,
        "stderr": _stderr(flags),
        "per_sample": per_sample,
    }


def _get(d: dict[str, Any], *keys: str) -> bool:
    """True only if every key is present and truthy; used for AND-folds."""
    return all(bool(d.get(key)) for key in keys)


def six_axis(attempt: dict[str, Any]) -> dict[str, Any]:
    """Fold a raw attempt record into the six independent axes. An axis is
    ``None`` when none of its inputs are present; axes are never averaged into a
    single scalar."""

    def axis(present: bool, value: Any) -> Any:
        return value if present else None

    a = attempt

    discovery_present = any(k in a for k in ("solved", "pass_k", "majority_k"))
    informal_present = any(
        k in a for k in ("answer_correct", "ntc", "nlg", "nae", "nce")
    )
    formal_present = any(k in a for k in ("compiles", "no_sorry"))
    soundness_present = any(
        k in a for k in ("stmt_matches", "axioms_ok", "axiom_closure")
    )
    efficiency_present = any(k in a for k in ("tokens", "lean_lines", "tool_calls"))
    novelty_present = any(k in a for k in ("alias_score", "contamination_flag"))

    step_keys = [k for k in ("ntc", "nlg", "nae", "nce") if k in a]
    informal = {
        "answer_correct": a.get("answer_correct"),
        "ntc": a.get("ntc"),
        "nlg": a.get("nlg"),
        "nae": a.get("nae"),
        "nce": a.get("nce"),
        # overall is the AND of the answer and every provided step judge
        "overall": _get(a, "answer_correct", *step_keys)
        if "answer_correct" in a
        else None,
    }

    return {
        "discovery": axis(
            discovery_present,
            {
                "solved": a.get("solved"),
                "pass_k": a.get("pass_k"),
                "majority_k": a.get("majority_k"),
            },
        ),
        "informal": axis(informal_present, informal),
        "formal": axis(
            formal_present,
            {"compiles": a.get("compiles"), "no_sorry": a.get("no_sorry")},
        ),
        "soundness": axis(
            soundness_present,
            {
                "stmt_matches": a.get("stmt_matches"),
                "axioms_ok": a.get("axioms_ok"),
                "axiom_closure": a.get("axiom_closure"),
            },
        ),
        "efficiency": axis(
            efficiency_present,
            {
                "tokens": a.get("tokens"),
                "lean_lines": a.get("lean_lines"),
                "tool_calls": a.get("tool_calls"),
            },
        ),
        "novelty": axis(
            novelty_present,
            {
                "alias_score": a.get("alias_score"),
                "contamination_flag": a.get("contamination_flag"),
            },
        ),
    }


def run(request: dict[str, Any]) -> dict[str, Any]:
    op = request.get("op", "grade_answer")
    if op == "grade_answer":
        return grade_answer(request["gold"], request["pred"], request["kind"])
    if op == "grade_samples":
        return grade_samples(request["gold"], request["preds"], request["kind"])
    if op == "six_axis":
        return six_axis(request["attempt"])
    if op == "extract_answer":
        return {"answer": extract_answer(request["text"])}
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
