"""Per-formal-system SOURCE-SCAN soundness pre-gate (layer 2c).

A cheap, deterministic, standard-library-only lexical scanner that flags the
*escape hatches* a proof can use to defeat the earlier layers of the universal
proof gate (compile -> axiom/oracle audit -> kernel re-check). Those layers can
be silently bypassed:

  * Rocq: ``-type-in-type`` / ``Unset Universe Checking`` /
    ``Unset Guard Checking`` / ``Unset Positivity Checking`` /
    ``#[bypass_check(...)]`` disable kernel checks *without* introducing an axiom,
    so they never surface in ``Print Assumptions`` and ``rocqchk`` honours the same
    relaxed flags the ``.vo`` was built with. ``Admitted`` / ``admit`` and the
    assumption commands introduce postulates.
  * Isabelle: ``sorry`` (= the ``Pure.skip_proof`` oracle) only works when
    ``quick_and_dirty`` is on; ``oops`` abandons a proof; an added ``oracle`` /
    ``Thm.add_oracle`` or ``axiomatization`` injects trusted-by-fiat facts.
  * Lean: ``sorry`` / ``sorryAx`` / ``admit`` leave holes; ``native_decide`` and
    ``@[implemented_by]`` / ``@[extern]`` / ``@[csimp]`` trust the compiler;
    ``Lean.ofReduceBool`` / ``Lean.trustCompiler`` and custom ``axiom`` widen the
    trusted base.

This is a *pre-screen*, never a soundness proof: it is purely lexical and cannot
see cheats pulled in transitively through imports. It must run alongside (not
instead of) the actual compile + axiom/oracle audit + kernel re-check. It
complements the deeper Lean ``axioms``/``LeanParanoia`` gate with a fast check
that also covers Rocq and Isabelle.

Public API:
    scan(system, source) -> {clean, system, flags: [{pattern, severity, line, snippet}]}
    run(request)         -> scan(request["system"], request["source"])  (worker dispatch)

Worker dispatch key: ``source_scan``.
"""
from __future__ import annotations

import bisect
import json
import re
import sys
from typing import Any

CRITICAL = "critical"
WARNING = "warning"

SYSTEMS = ("lean", "rocq", "isabelle")

_SNIPPET_MAX = 160


# ---------------------------------------------------------------------------
# Comment / string / cartouche maskers
#
# Each masker returns a same-length string in which every comment, string, and
# (for Isabelle) cartouche byte is replaced by a space (newlines are always
# preserved). Downstream regexes therefore never match inside those regions,
# while 1-based line numbers stay exact.
# ---------------------------------------------------------------------------


def _blank_run(chars: list[str], start: int, length: int) -> None:
    for j in range(start, start + length):
        if chars[j] != "\n":
            chars[j] = " "


def _mask(
    text: str,
    *,
    line_comment: str | None = None,
    blocks: tuple[tuple[str, str], ...] = (),
    string_quote: str | None = '"',
    string_escape: str = "backslash",  # "backslash" | "double"
) -> str:
    """Generic length-preserving masker.

    ``blocks`` is a tuple of ``(open, close)`` delimiter pairs, each of which
    nests within itself (Rocq/Isabelle ``(* *)``, Lean ``/- -/``, Isabelle
    ``‹ ›`` cartouches). While inside a block only that block's own delimiters
    are recognised, so its content is treated opaquely.
    """
    chars = list(text)
    n = len(chars)
    i = 0
    stack: list[tuple[str, str]] = []
    in_string = False

    while i < n:
        if in_string:
            ch = chars[i]
            if string_escape == "backslash" and ch == "\\" and i + 1 < n:
                _blank_run(chars, i, 2)
                i += 2
                continue
            if ch == string_quote:
                if (
                    string_escape == "double"
                    and i + 1 < n
                    and chars[i + 1] == string_quote
                ):
                    # A doubled quote ("") is an escaped quote, not a terminator.
                    _blank_run(chars, i, 2)
                    i += 2
                    continue
                chars[i] = " "
                in_string = False
                i += 1
                continue
            if ch != "\n":
                chars[i] = " "
            i += 1
            continue

        if stack:
            top_open, top_close = stack[-1]
            if text[i : i + len(top_open)] == top_open:
                _blank_run(chars, i, len(top_open))
                stack.append((top_open, top_close))
                i += len(top_open)
                continue
            if text[i : i + len(top_close)] == top_close:
                _blank_run(chars, i, len(top_close))
                stack.pop()
                i += len(top_close)
                continue
            if chars[i] != "\n":
                chars[i] = " "
            i += 1
            continue

        # Top level.
        if line_comment and text[i : i + len(line_comment)] == line_comment:
            while i < n and chars[i] != "\n":
                chars[i] = " "
                i += 1
            continue

        matched = False
        for open_tok, close_tok in blocks:
            if text[i : i + len(open_tok)] == open_tok:
                _blank_run(chars, i, len(open_tok))
                stack.append((open_tok, close_tok))
                i += len(open_tok)
                matched = True
                break
        if matched:
            continue

        if string_quote is not None and chars[i] == string_quote:
            chars[i] = " "
            in_string = True
            i += 1
            continue

        i += 1

    return "".join(chars)


def mask_lean(text: str) -> str:
    """Mask Lean ``--`` line comments, nestable ``/- -/`` blocks, and strings."""
    return _mask(
        text,
        line_comment="--",
        blocks=(("/-", "-/"),),
        string_quote='"',
        string_escape="backslash",
    )


def mask_rocq(text: str) -> str:
    """Mask Rocq/Coq nestable ``(* *)`` comments and strings (``""`` escapes)."""
    return _mask(
        text,
        line_comment=None,
        blocks=(("(*", "*)"),),
        string_quote='"',
        string_escape="double",
    )


def mask_isabelle(text: str) -> str:
    """Mask Isabelle nestable ``(* *)`` comments, ``‹ ›`` cartouches (text
    blocks / inner-syntax), and ``"..."`` strings."""
    return _mask(
        text,
        line_comment=None,
        blocks=(("(*", "*)"), ("‹", "›")),
        string_quote='"',
        string_escape="backslash",
    )


# ---------------------------------------------------------------------------
# Per-system escape-hatch pattern tables: (pattern_name, regex, severity)
# ---------------------------------------------------------------------------

# Lean identifiers admit the prime `'`, so `sorry'`/`my_sorry` must NOT match.
def _lean_word(tok: str) -> re.Pattern[str]:
    return re.compile(rf"(?<![\w']){tok}(?![\w'])")


def _lean_attr(name: str) -> re.Pattern[str]:
    # Match `@[ ... name ... ]` so `@[simp, implemented_by f]` is caught.
    return re.compile(rf"@\[[^\]]*\b{name}\b[^\]]*\]")


_LEAN_RULES: tuple[tuple[str, re.Pattern[str], str], ...] = (
    ("sorry", _lean_word("sorry"), CRITICAL),
    ("sorryAx", _lean_word("sorryAx"), CRITICAL),
    ("admit", _lean_word("admit"), CRITICAL),
    ("native_decide", _lean_word("native_decide"), CRITICAL),
    ("axiom", _lean_word("axiom"), CRITICAL),
    ("ofReduceBool", re.compile(r"\bLean\.ofReduceBool\b"), CRITICAL),
    ("ofReduceNat", re.compile(r"\bLean\.ofReduceNat\b"), CRITICAL),
    ("trustCompiler", re.compile(r"\bLean\.trustCompiler\b"), CRITICAL),
    ("implemented_by", _lean_attr("implemented_by"), WARNING),
    ("extern", _lean_attr("extern"), WARNING),
    ("csimp", _lean_attr("csimp"), WARNING),
)

_ROCQ_RULES: tuple[tuple[str, re.Pattern[str], str], ...] = (
    ("Admitted", re.compile(r"\bAdmitted\b"), CRITICAL),
    ("admit", re.compile(r"\badmit\b"), CRITICAL),
    (
        "global_assumption",
        re.compile(
            r"\b(?:Axiom|Axioms|Parameter|Parameters|Conjecture|Conjectures"
            r"|Hypothesis|Hypotheses|Variable|Variables)\b"
        ),
        CRITICAL,
    ),
    ("type_in_type", re.compile(r"-type-in-type"), CRITICAL),
    ("unset_universe_checking", re.compile(r"Unset\s+Universe\s+Checking"), CRITICAL),
    ("unset_guard_checking", re.compile(r"Unset\s+Guard\s+Checking"), CRITICAL),
    (
        "unset_positivity_checking",
        re.compile(r"Unset\s+Positivity\s+Checking"),
        CRITICAL,
    ),
    ("bypass_check", re.compile(r"\bbypass_check\b"), CRITICAL),
    ("admit_obligations", re.compile(r"Admit\s+Obligations"), CRITICAL),
)

_ISABELLE_RULES: tuple[tuple[str, re.Pattern[str], str], ...] = (
    ("sorry", re.compile(r"\bsorry\b"), CRITICAL),
    ("oops", re.compile(r"\boops\b"), CRITICAL),
    ("quick_and_dirty", re.compile(r"\bquick_and_dirty\b"), CRITICAL),
    ("add_oracle", re.compile(r"\bThm\.add_oracle\b"), CRITICAL),
    ("oracle", re.compile(r"\boracle\b"), CRITICAL),
    ("axiomatization", re.compile(r"\baxiomatization\b"), CRITICAL),
    ("axioms", re.compile(r"\baxioms\b"), CRITICAL),
    ("nitpick", re.compile(r"\bnitpick\b"), WARNING),
    ("quickcheck", re.compile(r"\bquickcheck\b"), WARNING),
)

_RULES = {
    "lean": _LEAN_RULES,
    "rocq": _ROCQ_RULES,
    "isabelle": _ISABELLE_RULES,
}

_MASKERS = {
    "lean": mask_lean,
    "rocq": mask_rocq,
    "isabelle": mask_isabelle,
}

# Aliases for the two systems that have common alternate names.
_ALIASES = {
    "coq": "rocq",
    "isabelle/hol": "isabelle",
    "hol": "isabelle",
    "lean4": "lean",
}


def _line_starts(text: str) -> list[int]:
    starts = [0]
    for i, ch in enumerate(text):
        if ch == "\n":
            starts.append(i + 1)
    return starts


def scan(system: str, source: str) -> dict[str, Any]:
    """Lexically scan ``source`` for ``system``'s soundness escape hatches.

    Returns ``{"clean": bool, "system": str, "flags": [...]}`` where each flag is
    ``{"pattern": str, "severity": "critical"|"warning", "line": int,
    "snippet": str}``. ``clean`` is True iff there are zero ``critical`` flags
    (``warning`` flags are informational and do not break cleanliness). Line
    numbers are 1-based. Raises ``ValueError`` for an unknown system.
    """
    key = system.strip().lower()
    key = _ALIASES.get(key, key)
    if key not in _RULES:
        raise ValueError(
            f"unknown formal system {system!r}; expected one of {SYSTEMS}"
        )

    masked = _MASKERS[key](source)
    starts = _line_starts(masked)
    src_lines = source.splitlines()

    def locate(pos: int) -> int:
        return bisect.bisect_right(starts, pos)

    flags: list[dict[str, Any]] = []
    for name, regex, severity in _RULES[key]:
        for m in regex.finditer(masked):
            line = locate(m.start())
            raw = src_lines[line - 1] if 0 <= line - 1 < len(src_lines) else ""
            snippet = raw.strip()[:_SNIPPET_MAX]
            flags.append(
                {
                    "pattern": name,
                    "severity": severity,
                    "line": line,
                    "snippet": snippet,
                }
            )

    flags.sort(key=lambda f: (f["line"], f["pattern"]))
    clean = not any(f["severity"] == CRITICAL for f in flags)
    return {"clean": clean, "system": key, "flags": flags}


def run(request: dict[str, Any]) -> dict[str, Any]:
    """Worker-dispatch entrypoint: ``{system, source}`` -> scan result."""
    return scan(request["system"], request["source"])


def main() -> None:
    if len(sys.argv) >= 2:
        with open(sys.argv[1], encoding="utf-8") as fh:
            req = json.load(fh)
    else:
        req = json.load(sys.stdin)
    result = run(req)
    print(json.dumps(result, indent=2, ensure_ascii=False))
    raise SystemExit(0 if result["clean"] else 1)


if __name__ == "__main__":
    main()
