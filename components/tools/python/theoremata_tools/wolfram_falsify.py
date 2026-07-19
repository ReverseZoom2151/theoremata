"""Wolfram Engine as an UNTRUSTED counterexample oracle and relation finder.

This is a sibling of :mod:`.falsify` (our own bounded, self-contained search),
not a replacement. The difference is trust: :mod:`.falsify` evaluates the claim
itself, so its ``counterexample`` verdict is grounded in code we control. A
Wolfram Engine is a large closed-source system we did not verify, so nothing it
says is admitted as evidence on its own authority.

Two rules make that safe:

1. **Every witness is re-verified by us.** When Wolfram proposes a
   counterexample, we parse it into exact Python rationals and evaluate the
   *original* claim at that point ourselves, with exact arithmetic. Only a
   witness our own recheck confirms is reported as a refutation. A witness we
   cannot confirm (bogus, inexact, or unparseable) is DISCARDED, never
   reported. A confirmed counterexample is therefore self-certifying: the
   oracle only had to guess, we did the checking. That is why an untrusted
   oracle is admissible here at all.

2. **The asymmetry is enforced in the vocabulary.** Finding a counterexample
   refutes the statement. Finding none proves NOTHING: the search is bounded,
   heuristic, and over a domain we chose. So "nothing found" is reported as
   ``no_counterexample_found`` and carries ``refuted=False``, ``proves=None``
   and ``search_exhausted=False``. There is no verdict in this module that
   means "verified", "passed", or "holds", because this module can never
   establish one.

``integer_relation`` is conjecture generation only. PSLQ works to finite
precision, so any relation it returns is a numerical coincidence until someone
proves it. Its results are marked ``proved=False`` and
``status="unproved_conjecture"`` and must go through the normal proof gate like
any other model-generated guess.

Mistranslation of a claim into Wolfram Language is a recall problem, never a
soundness problem: a bad translation makes Wolfram search for the wrong thing,
and whatever it returns still has to survive our own recheck of the original
Python claim.
"""
from __future__ import annotations

import ast
import re
from fractions import Fraction
from typing import Any

from .safe_eval import ALLOWED_NAMES, compile_expression
from .wolfram_link import available, evaluate

#: Verdicts this module can emit. Deliberately contains no positive verdict.
VERDICT_COUNTEREXAMPLE = "counterexample"          # matches falsify.py
VERDICT_NONE_FOUND = "no_counterexample_found"
VERDICT_INCONCLUSIVE = "inconclusive"
VERDICT_UNAVAILABLE = "unavailable"
VERDICT_CANDIDATE_RELATION = "candidate_relation"
VERDICT_NO_RELATION = "no_relation_found"

DEFAULT_TIMEOUT_SECONDS = 30.0

#: Wolfram search primitives we are willing to drive.
SUPPORTED_METHODS = (
    "FindInstance",
    "Reduce",
    "NSolve",
    "SemialgebraicComponentInstances",
)

#: Domains that yield exact witnesses. ``Reals`` is allowed but flagged,
#: because it can hand back approximate numbers that fail rationalization.
SUPPORTED_DOMAINS = ("Integers", "Rationals", "Reals", "Algebraics", "Complexes")


# --- exactness guards -------------------------------------------------------


class InexactError(ValueError):
    """Raised when a value or a claim cannot be handled with exact arithmetic."""


def _assert_exactly_checkable(claim: str, names: set[str]) -> Any:
    """Compile ``claim`` and refuse it if an exact recheck is impossible.

    Float literals and ``math``/``statistics`` calls both drag the evaluation
    into binary floating point, where a comparison can come out the wrong way
    by a rounding error. Since the entire safety argument rests on OUR recheck
    being trustworthy, we would rather decline to recheck than recheck
    approximately and call the result a refutation.
    """
    code = compile_expression(claim, names)
    tree = ast.parse(claim, mode="eval")
    for node in ast.walk(tree):
        if isinstance(node, ast.Constant) and isinstance(node.value, (float, complex)):
            raise InexactError(
                "claim contains a floating-point literal; exact recheck refused"
            )
        if isinstance(node, ast.Attribute):
            raise InexactError(
                "claim uses math/statistics; exact recheck refused"
            )
    return code


_INT_RE = re.compile(r"^[+-]?\d+$")
_RATIO_RE = re.compile(r"^([+-]?\d+)\s*/\s*([+-]?\d+)$")
_DECIMAL_RE = re.compile(r"^[+-]?(?:\d+\.\d*|\.\d+)$")
_RATIONAL_HEAD_RE = re.compile(r"^Rational\[\s*([+-]?\d+)\s*,\s*([+-]?\d+)\s*\]$")


def parse_exact(token: str) -> Fraction:
    """Parse one Wolfram-printed number into an exact :class:`Fraction`.

    Accepts integers, ``p/q``, ``Rational[p, q]`` and exact decimal strings
    (``2.5`` is exactly 5/2 as written). Rejects everything else, including
    machine-precision reals carrying a backtick and symbolic values such as
    ``Sqrt[2]``: those cannot be rationalized without changing the number, and
    a witness we cannot represent exactly is a witness we cannot check.
    """
    text = token.strip()
    while text.startswith("(") and text.endswith(")"):
        text = text[1:-1].strip()
    negate = False
    if text.startswith("-(") and text.endswith(")"):
        negate, text = True, text[2:-1].strip()
    if "`" in text or "*^" in text:
        raise InexactError(f"inexact machine-precision value: {token!r}")

    value: Fraction
    m = _RATIONAL_HEAD_RE.match(text)
    if m:
        value = Fraction(int(m.group(1)), int(m.group(2)))
    elif _INT_RE.match(text):
        value = Fraction(int(text))
    elif (m := _RATIO_RE.match(text)) is not None:
        denominator = int(m.group(2))
        if denominator == 0:
            raise InexactError(f"zero denominator in {token!r}")
        value = Fraction(int(m.group(1)), denominator)
    elif _DECIMAL_RE.match(text):
        # Fraction(str) is exact on the decimal AS WRITTEN, unlike float().
        value = Fraction(text)
    else:
        raise InexactError(f"not an exact rational: {token!r}")
    return -value if negate else value


_RULE_RE = re.compile(r"([A-Za-z][A-Za-z0-9_]*)\s*->\s*([^,{}]+)")


def parse_witnesses(result: str, names: list[str]) -> list[dict[str, Fraction]]:
    """Extract candidate assignments from a Wolfram rule list.

    Best-effort by design. A rule set we cannot parse simply yields no
    candidate, which costs recall and never soundness.
    """
    witnesses: list[dict[str, Fraction]] = []
    wanted = set(names)
    for block in re.findall(r"\{([^{}]*->[^{}]*)\}", result or ""):
        assignment: dict[str, Fraction] = {}
        for name, raw in _RULE_RE.findall(block):
            if name not in wanted:
                continue
            try:
                assignment[name] = parse_exact(raw)
            except InexactError:
                assignment = {}
                break
        if assignment and set(assignment) == wanted:
            witnesses.append(assignment)
    return witnesses


# --- the load-bearing recheck ----------------------------------------------


def recheck(
    assignment: dict[str, Fraction],
    claim: str,
    assumptions: str = "True",
) -> dict[str, Any]:
    """Independently confirm that ``assignment`` really falsifies ``claim``.

    This is the whole safety story. We do not ask Wolfram whether its answer is
    right; we substitute the witness into the ORIGINAL Python claim and
    evaluate it ourselves with exact rationals, under the same restricted
    evaluator the rest of the toolchain uses. Every value is coerced to
    :class:`Fraction` (including integers) so that ``/`` and ``**`` stay exact
    instead of silently becoming float division.

    A witness is confirmed only when the assumptions evaluate to exactly
    ``True`` and the claim evaluates to exactly ``False``. Anything else --
    inadmissible point, claim still true, non-boolean result, evaluation error
    -- is a rejection.
    """
    names = set(assignment)
    try:
        claim_code = _assert_exactly_checkable(claim, names)
        assumption_code = _assert_exactly_checkable(assumptions, names)
    except (InexactError, ValueError) as exc:
        return {"confirmed": False, "reason": f"claim not exactly checkable: {exc}"}

    scope = {**ALLOWED_NAMES, **{k: Fraction(v) for k, v in assignment.items()}}
    try:
        admissible = eval(assumption_code, {"__builtins__": {}}, scope)
        holds = eval(claim_code, {"__builtins__": {}}, scope)
    except Exception as exc:  # noqa: BLE001 - any failure is a rejection
        return {"confirmed": False, "reason": f"recheck evaluation failed: {exc}"}

    if not isinstance(admissible, bool) or not isinstance(holds, bool):
        # A non-boolean means the expression is not a predicate; we refuse to
        # guess what truthiness was intended.
        return {"confirmed": False, "reason": "claim/assumptions are not boolean"}
    if not admissible:
        return {"confirmed": False, "reason": "witness violates the assumptions"}
    if holds:
        return {"confirmed": False, "reason": "claim is TRUE at the proposed witness"}
    return {"confirmed": True, "reason": "claim is FALSE at the witness (exact recheck)"}


# --- Python -> Wolfram Language (translation is recall-only) ----------------

_BINOPS = {
    ast.Add: "+", ast.Sub: "-", ast.Mult: "*", ast.Div: "/", ast.Pow: "^",
}
_CMPOPS = {
    ast.Eq: "==", ast.NotEq: "!=", ast.Lt: "<", ast.LtE: "<=",
    ast.Gt: ">", ast.GtE: ">=",
}


def to_wolfram(expression: str) -> str:
    """Translate a restricted Python boolean/arithmetic expression to WL.

    Only a small operator set is supported; anything else raises. A wrong or
    missing translation can only make the oracle search for the wrong thing --
    the recheck still runs against the original Python claim.
    """
    def walk(node: ast.AST) -> str:
        if isinstance(node, ast.Expression):
            return walk(node.body)
        if isinstance(node, ast.Constant):
            if node.value is True:
                return "True"
            if node.value is False:
                return "False"
            if isinstance(node.value, int):
                return str(node.value)
            raise ValueError(f"unsupported constant: {node.value!r}")
        if isinstance(node, ast.Name):
            return node.id
        if isinstance(node, ast.UnaryOp):
            if isinstance(node.op, ast.USub):
                return f"(-{walk(node.operand)})"
            if isinstance(node.op, ast.UAdd):
                return walk(node.operand)
            if isinstance(node.op, ast.Not):
                return f"(!{walk(node.operand)})"
        if isinstance(node, ast.BinOp) and type(node.op) in _BINOPS:
            return f"({walk(node.left)} {_BINOPS[type(node.op)]} {walk(node.right)})"
        if isinstance(node, ast.BoolOp):
            joiner = " && " if isinstance(node.op, ast.And) else " || "
            return "(" + joiner.join(walk(v) for v in node.values) + ")"
        if isinstance(node, ast.Compare):
            if not all(type(op) in _CMPOPS for op in node.ops):
                raise ValueError("unsupported comparison operator")
            parts = [walk(node.left)]
            for op, comparator in zip(node.ops, node.comparators):
                parts.append(_CMPOPS[type(op)])
                parts.append(walk(comparator))
            return "(" + " ".join(parts) + ")"
        raise ValueError(f"cannot translate to Wolfram Language: {type(node).__name__}")

    return walk(ast.parse(expression, mode="eval"))


# --- responses --------------------------------------------------------------


def _base(op: str) -> dict[str, Any]:
    return {
        "op": op,
        "oracle": "wolfram",
        "trusted": False,
        "refuted": False,
        "proved": False,
    }


def _unavailable(op: str) -> dict[str, Any]:
    out = _base(op)
    out.update({
        "verdict": VERDICT_UNAVAILABLE,
        "available": False,
        "reason": "no Wolfram Engine / wolframclient present",
    })
    return out


def _timeout_hit(error: str | None) -> bool:
    return bool(error) and "timeout" in (error or "").lower()


# --- ops --------------------------------------------------------------------


def falsify(request: dict[str, Any]) -> dict[str, Any]:
    """Ask Wolfram for a counterexample to a universally quantified ``claim``.

    Request keys: ``variables`` (list of names, or the ``falsify``-style dict),
    ``claim`` (Python expression), ``assumptions`` (Python expression, default
    ``"True"``), ``domain`` (default ``"Integers"``), ``method`` (default
    ``"FindInstance"``), ``max_instances``, ``timeout_seconds``, and optional
    ``wl_claim`` / ``wl_assumptions`` overrides for the Wolfram-side form.

    Never returns a positive verdict. See the module docstring.
    """
    out = _base("falsify")
    if not available():
        return _unavailable("falsify")

    claim = request.get("claim")
    if not claim:
        raise ValueError("falsify requires a 'claim'")
    assumptions = request.get("assumptions") or "True"
    variables = request.get("variables") or {}
    names = list(variables) if isinstance(variables, dict) else list(variables)
    if not names:
        raise ValueError("falsify requires at least one variable")

    domain = request.get("domain", "Integers")
    if domain not in SUPPORTED_DOMAINS:
        raise ValueError(f"unsupported domain: {domain}")
    method = request.get("method", "FindInstance")
    if method not in SUPPORTED_METHODS:
        raise ValueError(f"unsupported method: {method}")
    max_instances = int(request.get("max_instances", 4))
    timeout_seconds = float(request.get("timeout_seconds", DEFAULT_TIMEOUT_SECONDS))

    try:
        wl_claim = request.get("wl_claim") or to_wolfram(claim)
        wl_assumptions = request.get("wl_assumptions") or to_wolfram(assumptions)
    except ValueError as exc:
        # We cannot phrase the question for the oracle. That is a recall loss,
        # so it is inconclusive -- never a pass.
        out.update({
            "available": True,
            "verdict": VERDICT_INCONCLUSIVE,
            "reason": f"cannot translate claim to Wolfram Language: {exc}",
            "proves": None,
            "search_exhausted": False,
        })
        return out
    var_list = "{" + ", ".join(names) + "}"
    # Search the NEGATION under the assumptions: that is exactly the set of
    # counterexamples to the universally quantified claim.
    target = f"({wl_assumptions}) && !({wl_claim})"
    if method == "Reduce":
        code = f"ToString[InputForm[Reduce[{target}, {var_list}, {domain}]]]"
    elif method == "NSolve":
        code = f"ToString[InputForm[NSolve[{target}, {var_list}, {domain}]]]"
    elif method == "SemialgebraicComponentInstances":
        code = (
            "ToString[InputForm[SemialgebraicComponentInstances["
            f"{target}, {var_list}]]]"
        )
    else:
        code = (
            f"ToString[InputForm[FindInstance[{target}, {var_list}, {domain}, "
            f"{max_instances}]]]"
        )

    response = evaluate(code, timeout=timeout_seconds)
    out.update({
        "available": True,
        "method": method,
        "domain": domain,
        "claim": claim,
        "assumptions": assumptions,
        "wolfram_code": code,
        "bound": {"max_instances": max_instances, "timeout_seconds": timeout_seconds},
        # A heuristic, bounded search can never justify a universal claim.
        "search_exhausted": False,
        "timeout_hit": _timeout_hit(response.get("error")),
    })
    if response.get("unavailable"):
        return _unavailable("falsify")
    if not response.get("ok"):
        out.update({
            "verdict": VERDICT_INCONCLUSIVE,
            "reason": f"wolfram evaluation failed: {response.get('error')}",
            "proves": None,
        })
        return out

    result = response.get("result") or ""
    out["wolfram_result"] = result
    candidates = parse_witnesses(result, names)
    rejected: list[dict[str, Any]] = []
    for candidate in candidates:
        verdict = recheck(candidate, claim, assumptions)
        if verdict["confirmed"]:
            out.update({
                "verdict": VERDICT_COUNTEREXAMPLE,
                "refuted": True,
                "independently_verified": True,
                "assignment": {k: str(v) for k, v in candidate.items()},
                "assignment_numerator_denominator": {
                    k: [v.numerator, v.denominator] for k, v in candidate.items()
                },
                "recheck": verdict["reason"],
                "rejected_witnesses": rejected,
            })
            return out
        rejected.append({
            "assignment": {k: str(v) for k, v in candidate.items()},
            "reason": verdict["reason"],
        })

    out["rejected_witnesses"] = rejected
    out["independently_verified"] = False
    if rejected:
        # Wolfram proposed something and OUR check refuted it. We discard it.
        out.update({
            "verdict": VERDICT_INCONCLUSIVE,
            "reason": (
                "every proposed witness failed independent exact recheck "
                "and was discarded"
            ),
            "proves": None,
        })
        return out
    out.update({
        "verdict": VERDICT_NONE_FOUND,
        # Spelled out so no downstream reader can mistake this for a pass.
        "proves": None,
        "note": (
            "no counterexample found; this is NOT verification, NOT a pass, and "
            "NOT evidence the claim holds. The search was bounded and "
            "heuristic."
        ),
    })
    return out


def integer_relation(request: dict[str, Any]) -> dict[str, Any]:
    """Run PSLQ (``FindIntegerRelation``) over numeric ``constants``.

    Pure conjecture generation. PSLQ works at finite precision, so a relation
    it reports is a numerical coincidence -- possibly a true identity, possibly
    an artefact of the working precision -- until it is proved. The response is
    always ``proved=False`` / ``status="unproved_conjecture"``.
    """
    if not available():
        out = _unavailable("integer_relation")
        out["status"] = "unproved_conjecture"
        return out

    constants = request.get("constants")
    if not constants:
        raise ValueError("integer_relation requires 'constants'")
    precision = int(request.get("precision", 50))
    timeout_seconds = float(request.get("timeout_seconds", DEFAULT_TIMEOUT_SECONDS))
    terms = ", ".join(str(c) for c in constants)
    code = (
        f"ToString[InputForm[FindIntegerRelation[N[{{{terms}}}, {precision}]]]]"
    )

    out = _base("integer_relation")
    response = evaluate(code, timeout=timeout_seconds)
    out.update({
        "available": True,
        "constants": list(constants),
        "precision": precision,
        "wolfram_code": code,
        "status": "unproved_conjecture",
        "timeout_hit": _timeout_hit(response.get("error")),
        "note": (
            "PSLQ result at finite precision. This is a CANDIDATE relation only: "
            "a numerical coincidence until proved. It must pass the normal proof "
            "gate before it counts as anything."
        ),
    })
    if response.get("unavailable"):
        result = _unavailable("integer_relation")
        result["status"] = "unproved_conjecture"
        return result
    if not response.get("ok"):
        out.update({
            "verdict": VERDICT_INCONCLUSIVE,
            "reason": f"wolfram evaluation failed: {response.get('error')}",
        })
        return out

    result = (response.get("result") or "").strip()
    out["wolfram_result"] = result
    coefficients: list[int] | None = None
    match = re.search(r"\{\s*([+-]?\d+(?:\s*,\s*[+-]?\d+)*)\s*\}", result)
    if match and "Failed" not in result:
        coefficients = [int(part) for part in match.group(1).split(",")]
    if coefficients is None or len(coefficients) != len(constants):
        out.update({
            "verdict": VERDICT_NO_RELATION,
            "coefficients": None,
            "reason": "no integer relation returned at this precision",
        })
        return out
    out.update({
        "verdict": VERDICT_CANDIDATE_RELATION,
        "coefficients": coefficients,
        "relation": " + ".join(
            f"({c})*({t})" for c, t in zip(coefficients, constants)
        ) + " == 0  (CONJECTURED, unproved)",
    })
    return out


_OPS = {"falsify": falsify, "integer_relation": integer_relation}


def run(request: dict[str, Any]) -> dict[str, Any]:
    """Dispatch on ``request['op']``, matching the other tool adapters."""
    op = request.get("op")
    handler = _OPS.get(op)
    if handler is None:
        raise ValueError(f"unknown op: {op}")
    return handler({k: v for k, v in request.items() if k != "op"})
