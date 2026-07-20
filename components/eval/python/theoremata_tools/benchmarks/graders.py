"""Per-track graders for the unified benchmark harness (Tier 4).

Each grader returns the uniform verdict ``{is_solved, is_correct, detail}``:

* ``is_solved``  — the response engaged the task in a gradable way (an answer was
  extracted / a statement was produced / a verdict was rendered);
* ``is_correct`` — it is *right* by the track's rubric;
* ``detail``     — a dict explaining the decision (method, extracted values, …).

Tracks:

* :func:`grade_formalization` — Lean compile + axiom-whitelist + statement
  preservation. Shells out to ``leanprover/comparator`` when it is available
  (``$THEOREMATA_COMPARATOR``). When it is NOT available, statement
  preservation is undetermined and the item is reported UNGRADED
  (``verdict["ungraded"] is True``) rather than scored by a weaker proxy; the
  ``sorry``/axiom-whitelist gate can still produce a definitive fail.

Verdicts additionally carry ``ungraded`` (default ``False``). An ungraded item
has ``is_solved`` and ``is_correct`` both ``False`` and MUST be excluded from a
pass-rate denominator or reported separately; counting it either way is a
misreport.
* :func:`grade_nl_answer` — deterministic symbolic/integer/relation grading via
  the existing :mod:`theoremata_tools.grader`, with an LLM-judge fallback (the
  mock-capable provider) only when symbolic parsing is inconclusive.
* :func:`grade_falsification` — flaw / counterexample detection, or must-reject
  for the negative fixture.
"""
from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, Callable

from theoremata_tools import grader as base_grader

from .schema import AXIOMS_WHITELIST

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


_METAMATH_COMMENT = re.compile(r"\$\(.*?\$\)", re.DOTALL)


def _strip_noncode(text: str, system: str = "lean") -> str:
    """Remove comments, doc comments and string literals from a source text.

    WHY: every containment check in this module is plantable. A model can make
    the canonical statement "appear" in its submission just by pasting the goal
    into a comment, a docstring or a string literal while proving something
    else entirely. The Rust side already settled this question the same way
    (``ESCAPE_HATCH_COMMENT_POLICY`` is ``CodeOnly``): a commented mention must
    never count as the thing being mentioned. We apply this to BOTH sides of a
    comparison, so stripping can only ever remove a spurious match, never a
    genuine one.
    """
    text = text or ""
    if system == "metamath":
        # Metamath comments are ``$( ... $)``; there are no string literals.
        return _METAMATH_COMMENT.sub(" ", text)

    # Lean and Agda share the shape: nestable block comments plus a line
    # comment to end-of-line. Lean uses ``/- -/`` and ``--``, Agda ``{- -}``
    # and ``--``. Lean doc comments (``/-- ... -/``) are just block comments.
    block_open, block_close = ("{-", "-}") if system == "agda" else ("/-", "-/")
    out: list[str] = []
    i, n, depth = 0, len(text), 0
    while i < n:
        two = text[i : i + 2]
        if depth:
            if two == block_open:
                depth += 1
                i += 2
                continue
            if two == block_close:
                depth -= 1
                i += 2
                continue
            i += 1
            continue
        if two == block_open:
            depth += 1
            i += 2
            out.append(" ")
            continue
        if two == "--":
            nl = text.find("\n", i)
            i = n if nl < 0 else nl
            out.append(" ")
            continue
        if text[i] == '"':
            # String literals are data, not the proved statement; a planted
            # goal inside one must not satisfy a containment check either.
            j = i + 1
            while j < n:
                if text[j] == "\\":
                    j += 2
                    continue
                if text[j] == '"':
                    break
                j += 1
            i = min(j + 1, n)
            out.append(" ")
            continue
        out.append(text[i])
        i += 1
    return "".join(out)


def _statement_preserved(
    expected_formal: str, response: str, system: str = "lean"
) -> bool:
    """Code-only containment of the expected statement in the response.

    This is a *proxy* for the anti-cheat "didn't weaken the theorem" check; the
    authoritative answer comes from the comparator. It is deliberately code-only
    (see :func:`_strip_noncode`) so that a mention inside a comment, a doc
    comment or a string literal cannot satisfy it.
    """
    exp = _normalize_lean(_strip_noncode(expected_formal, system))
    resp = _normalize_lean(_strip_noncode(response, system))
    if not exp:
        return False
    if exp in resp:
        return True
    # Fall back to the signature up to the proof separator (ignore the `:= by`).
    exp_sig = _normalize_lean(re.split(r":=", exp, maxsplit=1)[0])
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


def _comparator_timeout() -> float:
    try:
        return float(os.environ.get("THEOREMATA_COMPARATOR_TIMEOUT", "120"))
    except ValueError:
        return 120.0


def _comparator_argv(comparator: str, config: Path) -> list[str]:
    """Build a cross-platform argv for a comparator executable or script.

    Unix tests often ship extensionless scripts with shebangs; on Windows those
    cannot be spawned directly, so we resolve the interpreter from the shebang
    (or from the script body) before falling back to a bare path invocation.
    """
    path = Path(comparator)
    cfg = str(config)
    if not path.is_file():
        return [comparator, cfg]

    suffix = path.suffix.lower()
    if suffix in {".py", ".pyw"}:
        return [sys.executable, str(path), cfg]
    if suffix in {".bat", ".cmd"}:
        return ["cmd", "/c", str(path), cfg]
    if suffix == ".exe":
        return [str(path), cfg]

    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return [str(path), cfg]

    shebang = lines[0] if lines and lines[0].startswith("#!") else ""
    body = "\n".join(lines[1:] if shebang else lines)

    if shebang:
        interp = shebang[2:].strip().lower()
        if "python" in interp:
            return [sys.executable, str(path), cfg]
        if "sh" in interp or "bash" in interp:
            # Honour one-line ``exit N`` stubs directly — Git-for-Windows sh can
            # mis-run extensionless scripts, so tests stay portable.
            m = re.match(r"^\s*exit\s+(\d+)\s*$", body.strip())
            if m:
                return [sys.executable, "-c", f"import sys; sys.exit({m.group(1)})"]
            for shell in ("bash", "sh"):
                found = shutil.which(shell)
                if found:
                    return [found, str(path), cfg]

    if "sys.argv" in body or re.search(r"\bimport\s+json\b", body):
        return [sys.executable, str(path), cfg]

    return [str(path), cfg]


def _run_comparator(
    comparator: str,
    *,
    expected_formal: str,
    response: str,
    theorem_name: str,
    whitelist: list[str],
) -> dict[str, Any]:
    """Run leanprover/comparator against a generated solution.

    This follows FormalQualBench's contract: a trusted challenge module contains
    the original statement/stub, a solution module contains the submitted proof,
    and comparator checks statement preservation plus the axiom allowlist.

    The comparator executable itself is external and may require LEAN_PATH /
    lake-env setup; this function still performs a real subprocess invocation
    and returns enough detail to diagnose environment failures.
    """
    with tempfile.TemporaryDirectory(prefix="theoremata-comparator-") as td:
        root = Path(td)
        challenge = root / "Challenge.lean"
        solution = root / "Solution.lean"
        config = root / "config.json"
        challenge.write_text(expected_formal.rstrip() + "\n", encoding="utf-8")
        solution.write_text(response.rstrip() + "\n", encoding="utf-8")
        config.write_text(
            json.dumps(
                {
                    "challenge_module": "Challenge",
                    "solution_module": "Solution",
                    "theorem_names": [theorem_name],
                    "permitted_axioms": whitelist,
                    "enable_nanoda": False,
                },
                indent=2,
            ),
            encoding="utf-8",
        )

        cmd = _comparator_argv(comparator, config)
        if os.environ.get("THEOREMATA_COMPARATOR_LAKE_ENV") == "1":
            lake = shutil.which("lake") or "lake"
            cmd = [lake, "env", *cmd]
        try:
            proc = subprocess.run(
                cmd,
                cwd=root,
                text=True,
                capture_output=True,
                timeout=_comparator_timeout(),
                check=False,
            )
            return {
                "invoked": True,
                "returncode": proc.returncode,
                "ok": proc.returncode == 0,
                "stdout": proc.stdout[-4000:],
                "stderr": proc.stderr[-4000:],
                "config": json.loads(config.read_text(encoding="utf-8")),
            }
        except Exception as exc:  # noqa: BLE001
            return {
                "invoked": True,
                "returncode": None,
                "ok": False,
                "error": str(exc),
            }


def _formal_system(item: dict[str, Any]) -> str:
    """Identify the formal system of a formalization item so grading never
    applies a Lean-only comparator to Agda/Metamath syntax.

    We read the loader-set signals (``grading.method`` / ``expected.mode`` /
    ``provenance.corpus``) rather than assume every ``kind == "formalization"``
    item is Lean. Returns ``"agda"``, ``"metamath"``, or ``"lean"`` (default).
    """
    expected = item.get("expected") or {}
    grading = item.get("grading") or {}
    prov = item.get("provenance") or {}
    hints = " ".join(
        str(x).lower()
        for x in (
            grading.get("system"),
            expected.get("system"),
            grading.get("method"),
            expected.get("mode"),
            prov.get("corpus"),
        )
        if x
    )
    if "agda" in hints or "1lab" in hints:
        return "agda"
    if "metamath" in hints:
        return "metamath"
    return "lean"


def _grade_nonlean_formalization(
    item: dict[str, Any], response: str, system: str
) -> dict[str, Any]:
    """Language-appropriate grading for a non-Lean formalization item.

    Never runs the Lean comparator, the Lean ``:=`` statement split, or the Lean
    ``sorry``/``#print axioms`` gate. When the item has no gold formal statement
    (to-be-formalized corpora such as 1Lab), it is marked *not auto-gradable*
    instead of being spuriously scored. When a gold formal statement exists
    (e.g. a Metamath ``$p`` assertion) there is still no verifier in this
    process, so the language-agnostic code-only comparison is reported as a
    labelled PROXY and the item stays UNGRADED. WHY: scoring an unverified
    string match into the same column as a verified pass inflates the pass rate
    exactly as a silent benchmark exclusion deflates the denominator.
    """
    expected = item.get("expected") or {}
    grading = item.get("grading") or {}
    expected_formal = expected.get("formal_statement") or item.get("formal") or ""
    response = response or ""
    is_solved = bool(response.strip())
    gold_present = bool(expected.get("gold_present", bool(expected_formal)))
    auto_gradable = bool(grading.get("auto_gradable", gold_present))

    if not expected_formal or not gold_present or not auto_gradable:
        return {
            "is_solved": is_solved,
            "is_correct": False,
            "ungraded": True,
            "detail": {
                "track": "formalization",
                "system": system,
                "method": f"{system}_not_auto_gradable",
                "graded": False,
                "ungraded": True,
                "ungraded_reason": "no_gold_statement",
                "auto_gradable": False,
                "gold_present": gold_present,
                "statement_preserved": None,
                "comparator": None,
                "invoked": False,
                "note": (
                    f"{system} formalization item has no gold formal statement to "
                    f"compare against; a real pass requires a live {system} "
                    "typecheck/verifier. Not auto-gradable, and the Lean grader "
                    "is deliberately NOT applied."
                ),
            },
        }

    # A gold statement exists but no verifier does. Report the code-only match
    # as a labelled proxy and leave the item ungraded.
    preserved_proxy = _statement_preserved(expected_formal, response, system)
    return {
        "is_solved": is_solved,
        "is_correct": False,
        "ungraded": True,
        "detail": {
            "track": "formalization",
            "system": system,
            "method": f"{system}_ungraded_no_verifier",
            "graded": False,
            "ungraded": True,
            "ungraded_reason": "verifier_unavailable",
            "statement_preserved": None,
            "auto_gradable": True,
            "gold_present": True,
            "comparator": None,
            "invoked": False,
            "proxy": {
                "is_proxy": True,
                "counts_toward_pass_rate": False,
                "method": f"{system}_code_only_statement_match",
                "statement_preserved_proxy": preserved_proxy,
            },
            "note": (
                f"{system} statement containment is a PROXY only (code-only "
                "normalized comparison, comments and string literals stripped). "
                f"A verified pass requires the {system} backend, so this item is "
                "reported UNGRADED and must be excluded from any pass rate."
            ),
        },
    }


def grade_formalization(item: dict[str, Any], response: str) -> dict[str, Any]:
    system = _formal_system(item)
    if system != "lean":
        return _grade_nonlean_formalization(item, response, system)

    expected = item.get("expected") or {}
    expected_formal = expected.get("formal_statement") or item.get("formal") or ""
    whitelist = expected.get("axioms_whitelist") or []
    response = response or ""

    comparator = _comparator_path()
    if comparator:
        theorem_name = expected.get("lean_name") or "MainTheorem"
        cmp = _run_comparator(
            comparator,
            expected_formal=expected_formal,
            response=response,
            theorem_name=theorem_name,
            whitelist=whitelist,
        )
        return {
            "is_solved": bool(response.strip()),
            "is_correct": bool(cmp.get("ok")),
            "ungraded": False,
            "detail": {
                "track": "formalization",
                "system": "lean",
                "method": "comparator",
                "graded": True,
                "ungraded": False,
                "statement_preserved": bool(cmp.get("ok")),
                "axioms_ok": bool(cmp.get("ok")),
                "axiom_reason": "comparator_exit_0" if cmp.get("ok") else "comparator_failed",
                "expected_lean_name": theorem_name,
                "comparator": comparator,
                **cmp,
            },
        }

    # No comparator: statement preservation CANNOT be decided here.
    #
    # WHY not fall back to a string comparison: statement preservation is one of
    # the four gate layers, and it is precisely the check that the proof proves
    # the theorem that was asked. A containment test accepts a proof of a
    # different theorem whenever the canonical statement happens to appear in
    # the submission, which a model can arrange for free. Scoring such an item
    # as a pass inflates the pass rate; the honest report is UNGRADED. Any
    # containment signal we still compute is labelled a proxy, is code-only
    # (comments, doc comments and string literals stripped, matching the Rust
    # side's CodeOnly escape-hatch policy) and never sets ``is_correct``.
    axioms_ok, axiom_reason = _axioms_ok(response, whitelist)
    preserved_proxy = _statement_preserved(expected_formal, response)

    if not axioms_ok:
        # A residual sorry/admit or a non-whitelisted axiom is a definitive
        # failure that does not need the comparator, so this stays GRADED. It
        # can only ever produce a fail, never a pass.
        return {
            "is_solved": bool(response.strip()),
            "is_correct": False,
            "ungraded": False,
            "detail": {
                "track": "formalization",
                "system": "lean",
                "method": "axiom_gate_failed",
                "graded": True,
                "ungraded": False,
                "statement_preserved": None,
                "axioms_ok": False,
                "axiom_reason": axiom_reason,
                "expected_lean_name": expected.get("lean_name"),
                "comparator": None,
                "invoked": False,
                "note": (
                    "The axiom/sorry gate is a necessary condition and it failed, "
                    "so the item is a definitive fail regardless of the "
                    "comparator being unavailable."
                ),
            },
        }

    return {
        # Ungraded items are not "solved" either: no gradable verdict was
        # produced, so neither counter may absorb them.
        "is_solved": False,
        "is_correct": False,
        "ungraded": True,
        "detail": {
            "track": "formalization",
            "system": "lean",
            "method": "ungraded_comparator_unavailable",
            "graded": False,
            "ungraded": True,
            "ungraded_reason": "comparator_unavailable",
            "statement_preserved": None,
            "axioms_ok": axioms_ok,
            "axiom_reason": axiom_reason,
            "expected_lean_name": expected.get("lean_name"),
            "comparator": None,
            "invoked": False,
            "response_present": bool(response.strip()),
            "proxy": {
                "is_proxy": True,
                "counts_toward_pass_rate": False,
                "method": "code_only_statement_containment",
                "statement_preserved_proxy": preserved_proxy,
            },
            "note": (
                "No statement comparator was available "
                "($THEOREMATA_COMPARATOR / `comparator` on PATH), so statement "
                "preservation is UNDETERMINED. This item is UNGRADED and must be "
                "excluded from any verification pass-rate denominator or reported "
                "separately. The proxy block is diagnostic only."
            ),
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
# Verified programming (BRIDGE)
# --------------------------------------------------------------------------- #

_FENCE_LEAN = re.compile(r"```(?:lean)?\s*([\s\S]*?)```", re.IGNORECASE)


def _extract_lean_text(response: Any) -> str:
    if isinstance(response, dict):
        for key in ("lean", "code", "text", "solution"):
            if key in response and response[key]:
                response = response[key]
                break
    text = response if isinstance(response, str) else str(response or "")
    m = _FENCE_LEAN.search(text)
    return (m.group(1) if m else text).strip()


def grade_verified_programming(item: dict[str, Any], response: Any) -> dict[str, Any]:
    """Structural BRIDGE grading: signatures present, no sorry; oracle deferred."""
    expected = item.get("expected") or {}
    lean = _extract_lean_text(response)
    signatures = expected.get("lean_signatures") or []
    # Code-only: a signature pasted into a comment or a string literal is not a
    # signature the submission actually defines.
    lean_code = _normalize_lean(_strip_noncode(lean))
    sig_hits = [
        s for s in signatures
        if s and _normalize_lean(_strip_noncode(str(s))) in lean_code
    ]
    signatures_ok = bool(signatures) and len(sig_hits) == len(signatures)
    axioms_ok, axiom_detail = _axioms_ok(lean, list(AXIOMS_WHITELIST))
    oracle = expected.get("oracle_tests") or {}
    has_oracle = bool(oracle.get("inputs")) and bool(oracle.get("expected_outputs"))
    is_solved = bool(lean.strip())
    is_correct = is_solved and signatures_ok and axioms_ok
    return {
        "is_solved": is_solved,
        "is_correct": is_correct,
        "detail": {
            "track": "verified_programming",
            "method": "signature_and_oracle",
            "signatures_ok": signatures_ok,
            "signatures_matched": sig_hits,
            "axioms_ok": axioms_ok,
            "axiom_detail": axiom_detail,
            "oracle_present": has_oracle,
            "oracle_executed": False,
            "oracle_note": "oracle execution requires live Lean; structural gate only",
        },
    }


# --------------------------------------------------------------------------- #
# Scientific formalization (QuantumLean) — typecheck-only, NO gold
# --------------------------------------------------------------------------- #

def grade_scientific_formalization(item: dict[str, Any], response: Any) -> dict[str, Any]:
    """QuantumLean has NO gold formal proof — only model outputs plus a human
    0-2 ``manual_eval`` rubric. Statement-preservation is therefore impossible
    and would be a fabricated pass. We grade honestly: engagement (a Lean snippet
    was produced) + a structural axiom/``sorry`` gate as a *necessary* condition,
    and surface the human rubric. Correctness is NOT auto-determinable without a
    live Lean typecheck against a gold reference that does not exist, so
    ``is_correct`` stays ``False`` (undetermined) with an explicit note.
    """
    expected = item.get("expected") or {}
    lean = _extract_lean_text(response)
    axioms_ok, axiom_detail = _axioms_ok(
        lean, list(expected.get("axioms_whitelist") or AXIOMS_WHITELIST)
    )
    is_solved = bool(lean.strip())
    return {
        "is_solved": is_solved,
        # No gold + no live typecheck => not auto-verifiable. Honest: never claim
        # correctness from statement-preservation (there is no statement to preserve).
        "is_correct": False,
        "detail": {
            "track": "formalization",
            "method": "typecheck_only",
            "auto_gradable": False,
            "gold_present": bool(expected.get("gold_present", False)),
            "axioms_ok": axioms_ok,
            "axiom_detail": axiom_detail,
            "structural_gate_only": True,
            "manual_eval": expected.get("manual_eval"),
            "response_model_keys": expected.get("response_model_keys"),
            "note": (
                "QuantumLean ships no gold formal proof; correctness requires a "
                "live Lean typecheck and/or the human 0-2 manual_eval rubric. "
                "No statement-preservation grade is possible."
            ),
        },
    }


# --------------------------------------------------------------------------- #
# Statement target (Millennium)
# --------------------------------------------------------------------------- #

def grade_statement_target(item: dict[str, Any], response: Any) -> dict[str, Any]:
    expected = item.get("expected") or {}
    lean = _extract_lean_text(response)
    formal = item.get("formal") or expected.get("lean_name") or ""
    lean_name = str(expected.get("lean_name") or "")
    # Code-only on both alternatives: a commented-out mention of the target
    # statement or of its Lean name must not count as having stated it.
    preserved = bool(
        _statement_preserved(str(formal), lean)
        or (lean_name and lean_name in _strip_noncode(lean))
    )
    axioms_ok, axiom_detail = _axioms_ok(lean, list(expected.get("axioms_whitelist") or AXIOMS_WHITELIST))
    is_solved = bool(lean.strip())
    is_correct = is_solved and preserved and axioms_ok
    return {
        "is_solved": is_solved,
        "is_correct": is_correct,
        "detail": {
            "track": "statement_target",
            "method": "statement_preservation",
            "statement_preserved": preserved,
            "axioms_ok": axioms_ok,
            "axiom_detail": axiom_detail,
            "reference_pdf": expected.get("reference_pdf"),
        },
    }


# --------------------------------------------------------------------------- #
# External artifact (Putnam / Aristotle outputs)
# --------------------------------------------------------------------------- #

def grade_external_artifact(item: dict[str, Any], response: Any) -> dict[str, Any]:
    expected = item.get("expected") or {}
    lean = _extract_lean_text(response) or str(item.get("formal") or "")
    headers = expected.get("headers") or []
    lean_name = str(expected.get("lean_name") or "")
    # Code-only: a header name mentioned in a comment is not a declared header.
    lean_code = _strip_noncode(lean)
    header_ok = not headers or any(h.get("name") in lean_code for h in headers)
    axioms_ok, axiom_detail = _axioms_ok(lean, list(expected.get("axioms_whitelist") or AXIOMS_WHITELIST))
    is_solved = bool(lean.strip())
    is_correct = is_solved and header_ok and axioms_ok
    return {
        "is_solved": is_solved,
        "is_correct": is_correct,
        "detail": {
            "track": "external_artifact",
            "method": "structural_and_axiom_gate",
            "headers_ok": header_ok,
            "axioms_ok": axioms_ok,
            "axiom_detail": axiom_detail,
            "provenance": expected.get("provenance") or {},
            "compile_checked": False,
            "note": "full compile/hardening requires live Lean suite",
        },
    }


# --------------------------------------------------------------------------- #
# Reformulation (FormulationBench / FLARE)
# --------------------------------------------------------------------------- #

def grade_reformulation(item: dict[str, Any], response: Any) -> dict[str, Any]:
    expected = item.get("expected") or {}
    text = response if isinstance(response, str) else str(response or "")
    if isinstance(response, dict):
        text = str(response.get("is_reformulation", response.get("verdict", text)))
    positive = bool(expected.get("is_reformulation", True))
    claims_yes = _has_any(text, ("equivalent", "reformulation", "is_reformulation", "true", "yes"))
    claims_no = _has_any(text, ("not equivalent", "not a reformulation", "false", "no"))
    if positive:
        is_correct = claims_yes and not claims_no
    else:
        is_correct = claims_no and not claims_yes
    return {
        "is_solved": bool(text.strip()),
        "is_correct": bool(is_correct),
        "detail": {
            "track": "reformulation",
            "method": "equivalence_claim",
            "expected_positive": positive,
            "claims_yes": claims_yes,
            "claims_no": claims_no,
            "lean_proof_checked": False,
        },
    }


# --------------------------------------------------------------------------- #
# Proof grading / evaluator calibration (IMO-ProofBench)
# --------------------------------------------------------------------------- #

_BOXED_NUM = re.compile(r"\\boxed\{\s*([0-9]*\.?[0-9]+)\s*\}")
_ANY_NUM = re.compile(r"-?[0-9]*\.?[0-9]+")


def _extract_score(response: Any) -> float | None:
    """Pull a numeric grade from a grader response (boxed first, else last num)."""
    if isinstance(response, (int, float)):
        return float(response)
    if isinstance(response, dict):
        for k in ("score", "grade", "rating"):
            if response.get(k) is not None:
                try:
                    return float(response[k])
                except (TypeError, ValueError):
                    pass
        response = response.get("text", "")
    text = response if isinstance(response, str) else str(response or "")
    m = _BOXED_NUM.search(text)
    if m:
        return float(m.group(1))
    nums = _ANY_NUM.findall(text)
    return float(nums[-1]) if nums else None


def grade_proof_grading(item: dict[str, Any], response: Any) -> dict[str, Any]:
    """Evaluator-calibration grader: how close is a proposed grade to the GOLD
    HUMAN grade? ``response`` is a grader's score (boxed number / dict / float),
    on either the 0-7 IMO scale or a normalized 0-1 scale. Correct iff the
    normalized grade lands within tolerance of the normalized human rating.
    """
    expected = item.get("expected") or {}
    gold_human = expected.get("gold_human_rating")
    tol = 0.15
    pred_raw = _extract_score(response)

    def _norm(v: float | None) -> float | None:
        if v is None:
            return None
        return v / 7.0 if v > 1.0 else float(v)

    gold_n = _norm(None if gold_human is None else float(gold_human))
    pred_n = _norm(pred_raw)
    is_solved = pred_raw is not None
    is_correct = (
        is_solved
        and gold_n is not None
        and pred_n is not None
        and abs(pred_n - gold_n) <= tol
    )
    return {
        "is_solved": is_solved,
        "is_correct": bool(is_correct),
        "detail": {
            "track": "proof_grading",
            "method": "grade_calibration",
            "gold_human_rating": gold_human,
            "gold_normalized": gold_n,
            "predicted_raw": pred_raw,
            "predicted_normalized": pred_n,
            "model_auto_rating": expected.get("model_auto_rating"),
            "abs_error": None if (gold_n is None or pred_n is None) else abs(pred_n - gold_n),
            "tolerance": tol,
        },
    }


# --------------------------------------------------------------------------- #
# IMO-AnswerBench — robust verifiable-answer matching
# --------------------------------------------------------------------------- #

# corpus answer_kind -> matcher route
_ANSWER_MATCH_ROUTE = {
    "integer": "integer",
    "relation": "relation",
    "symbolic": "symbolic",
    "bound": "symbolic",
    "set": "set",
    "list": "list",
    "string": "string",
}


def _unbox_answer(s: str) -> str:
    """Strip ``$`` and unwrap a single ``\\boxed{…}``/``\\text{…}`` wrapper while
    preserving case and operators, so the value stays sympy-parseable."""
    s = (s or "").strip()
    for junk in ("$", "\\left", "\\right", "\\,", "\\!"):
        s = s.replace(junk, "")
    s = s.strip()
    for wrapper in (r"\\boxed\{(.*)\}$", r"\\text\{(.*)\}$", r"\\mathrm\{(.*)\}$"):
        m = re.match(wrapper, s)
        if m:
            s = m.group(1).strip()
    return s.rstrip(".").strip()


def _canon_answer_string(s: str) -> str:
    """Format-resistant canonical form of a scalar answer: drop ``$``/spaces,
    unwrap a single ``\\boxed{…}``/``\\text{…}`` and outer braces, lowercase."""
    s = (s or "").strip()
    for junk in ("$", "\\left", "\\right", "\\,", "\\!", "\\ ", " "):
        s = s.replace(junk, "")
    for wrapper in (r"\\boxed\{(.*)\}$", r"\\text\{(.*)\}$", r"\\mathrm\{(.*)\}$"):
        m = re.match(wrapper, s)
        if m:
            s = m.group(1)
    return s.strip().strip("{}").rstrip(".").lower()


def _canon_answer_set(s: str, ordered: bool) -> list[str]:
    """Canonical form of a set/list answer: strip brackets, split on ``,``/``;``,
    canonicalize each element; sort unless the answer is order-sensitive."""
    inner = _canon_answer_string(s)
    inner = inner.strip("{}[]()")
    parts = [_canon_answer_string(p) for p in re.split(r"[,;]", inner) if p.strip()]
    return parts if ordered else sorted(parts)


def grade_answer_match(item: dict[str, Any], response: Any) -> dict[str, Any]:
    """Robust answer-matching grader for the verifiable-answer track.

    Extracts the final answer from ``response`` and judges semantic/format
    equivalence to the gold answer — numeric/symbolic (0.5 == 1/2 but 6.28 !=
    2*pi), canonical-string, or set/list equivalence — resistant to ``$``,
    ``\\boxed{…}``, spacing and (for sets) ordering. No partial credit.
    """
    expected = item.get("expected") or {}
    gold = str(expected.get("answer", ""))
    answer_kind = expected.get("answer_kind") or item.get("grading", {}).get(
        "answer_kind", "symbolic"
    )
    route = _ANSWER_MATCH_ROUTE.get(answer_kind, "symbolic")

    text = response if isinstance(response, str) else str(response or "")
    extracted = base_grader.extract_answer(text)
    pred = extracted if extracted is not None else text.strip()
    is_solved = bool(pred.strip())

    if route in ("set", "list"):
        ordered = route == "list"
        gold_c = _canon_answer_set(gold, ordered)
        pred_c = _canon_answer_set(pred, ordered)
        is_correct = bool(gold_c) and gold_c == pred_c
        method = f"canonical_{route}"
    elif route == "string":
        gold_c, pred_c = _canon_answer_string(gold), _canon_answer_string(pred)
        is_correct = bool(gold_c) and gold_c == pred_c
        method = "canonical_string"
    else:  # integer / relation / symbolic
        gold_c, pred_c = _canon_answer_string(gold), _canon_answer_string(pred)
        if gold_c and gold_c == pred_c:
            is_correct, method = True, "canonical_string"
        else:
            verdict = base_grader.grade_answer(
                _unbox_answer(gold), _unbox_answer(pred), route
            )
            is_correct, method = bool(verdict["correct"]), verdict["method"]

    return {
        "is_solved": is_solved,
        "is_correct": bool(is_correct and is_solved),
        "detail": {
            "track": "answer_match",
            "method": method,
            "answer_kind": answer_kind,
            "gold": gold,
            "extracted": pred,
            "perturbation": expected.get("perturbation"),
        },
    }


# --------------------------------------------------------------------------- #
# IMO-GradingBench — autograder-vs-human agreement
# --------------------------------------------------------------------------- #

# Paper 4-way rubric: Correct=7, Almost=6, Partial=1, Incorrect=0.
_GRADE_LABEL_POINTS = {"incorrect": 0, "partial": 1, "almost": 6, "correct": 7}
_GRADE_CANON_POINTS: tuple[tuple[int, str], ...] = (
    (0, "incorrect"),
    (1, "partial"),
    (6, "almost"),
    (7, "correct"),
)
_N_OUT_OF_7 = re.compile(r"(\d+(?:\.\d+)?)\s*(?:/|out\s+of)\s*7", re.IGNORECASE)


def _grade_bucket(value: float | None) -> str | None:
    if value is None:
        return None
    return min(_GRADE_CANON_POINTS, key=lambda p: abs(p[0] - value))[1]


def _extract_grade(response: Any) -> tuple[str | None, float | None]:
    """Pull a (label, 0-7 numeric) grade from an autograder response.

    Handles a bare number, a ``{score|grade|rating|points|label|verdict}`` dict,
    a 4-way rubric label word, ``N out of 7``, a ``\\boxed{N}`` and a trailing
    number — mirroring IMO-GradingBench's vanilla-prompt output formats.
    """
    label: str | None = None
    val: float | None = None
    if isinstance(response, (int, float)):
        val = float(response)
    elif isinstance(response, dict):
        for k in ("grade", "score", "rating", "points"):
            if response.get(k) is not None:
                try:
                    val = float(response[k])
                    break
                except (TypeError, ValueError):
                    pass
        lab = response.get("label") or response.get("verdict")
        if lab:
            label = str(lab).strip().lower()
        response = response.get("text", "") if (val is None and not label) else ""

    text = response if isinstance(response, str) else ""
    low = text.lower()
    if label is None and low:
        hits = [(low.rfind(l), l) for l in _GRADE_LABEL_POINTS if l in low]
        hits = [(i, l) for i, l in hits if i >= 0]
        if hits:
            label = max(hits)[1]
    if val is None and low:
        m = _N_OUT_OF_7.search(low)
        if m:
            val = float(m.group(1))
        else:
            mb = _BOXED_NUM.search(text)
            if mb:
                val = float(mb.group(1))
            else:
                nums = _ANY_NUM.findall(text)
                if nums:
                    val = float(nums[-1])
    if val is None and label in _GRADE_LABEL_POINTS:
        val = float(_GRADE_LABEL_POINTS[label])
    return label, val


def grade_grading_correlation(item: dict[str, Any], response: Any) -> dict[str, Any]:
    """Autograder-calibration grader: does a proposed grade AGREE with the GOLD
    HUMAN grade? Agreement = same 4-way rubric bucket (Correct/Almost/Partial/
    Incorrect); also surfaces the 0-7 absolute error (the MAE signal). This is
    the per-item unit the aggregate 4-way-accuracy / MAE metrics are built from.
    """
    expected = item.get("expected") or {}
    gold_human = expected.get("gold_human_rating")
    gold_v = None if gold_human is None else float(gold_human)
    gold_bucket = expected.get("gold_bucket") or _grade_bucket(gold_v)

    pred_label, pred_v = _extract_grade(response)
    pred_bucket = pred_label if pred_label in _GRADE_LABEL_POINTS else _grade_bucket(
        pred_v
    )
    is_solved = pred_label is not None or pred_v is not None
    agree = (
        is_solved
        and gold_bucket is not None
        and pred_bucket is not None
        and pred_bucket == gold_bucket
    )
    abs_error = None if (gold_v is None or pred_v is None) else abs(pred_v - gold_v)
    return {
        "is_solved": is_solved,
        "is_correct": bool(agree),
        "detail": {
            "track": "proof_grading",
            "method": "grading_correlation",
            "gold_human_rating": gold_human,
            "gold_bucket": gold_bucket,
            "predicted_raw": pred_v,
            "predicted_label": pred_label,
            "predicted_bucket": pred_bucket,
            "bucket_agreement": bool(agree),
            "abs_error": abs_error,
            "scale": "0-7",
        },
    }


# --------------------------------------------------------------------------- #
# Tactic reference (retrieval KB)
# --------------------------------------------------------------------------- #

def grade_tactic_reference(item: dict[str, Any], response: Any) -> dict[str, Any]:
    """A tactic-KB entry is a reference, not a task. Grade a response as a
    lightweight retrieval check: does it name the reference tactic?"""
    expected = item.get("expected") or {}
    tactic = str(expected.get("tactic") or "")
    text = response if isinstance(response, str) else str(response or "")
    hit = bool(tactic) and tactic.lower() in text.lower()
    return {
        "is_solved": bool(text.strip()),
        "is_correct": hit,
        "detail": {
            "track": "tactic_reference",
            "method": "retrieval_reference",
            "tactic": tactic,
            "matched": hit,
        },
    }


# --------------------------------------------------------------------------- #
# Dispatch by track
# --------------------------------------------------------------------------- #

def grade(item: dict[str, Any], response: Any, **kw: Any) -> dict[str, Any]:
    """Grade a response against an item, routing by ``item['kind']`` (with a
    grading-``method`` override for the IMO-Bench answer/grading tracks that
    share the ``nl_answer``/``proof_grading`` kinds)."""
    method = (item.get("grading") or {}).get("method")
    if method == "answer_match":
        return grade_answer_match(item, response)
    if method == "grading_correlation":
        return grade_grading_correlation(item, response)
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
    if kind == "verified_programming":
        return grade_verified_programming(item, response)
    if kind == "scientific_formalization":
        return grade_scientific_formalization(item, response)
    if kind == "statement_target":
        return grade_statement_target(item, response)
    if kind == "external_artifact":
        return grade_external_artifact(item, response)
    if kind == "reformulation":
        return grade_reformulation(item, response)
    if kind == "proof_grading":
        return grade_proof_grading(item, response)
    if kind == "tactic_reference":
        return grade_tactic_reference(item, response)
    raise ValueError(f"cannot grade item of kind {kind!r}")
