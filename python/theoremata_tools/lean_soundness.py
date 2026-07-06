"""Lexical Lean soundness pre-gate.

A cheap, deterministic, standard-library-only static check that a Lean proof
does not secretly cheat: it flags residual `sorry`/`admit` placeholders and
`axiom`/`constant`/`postulate` declarations while ignoring anything that lives
inside a comment or a string literal.

This is a *pre-gate*, not a soundness guarantee: it is purely lexical and
cannot see axioms pulled in transitively from imports, `Classical.choice`, or
`sorry` synthesised by a macro. It must precede, never replace, actual Lean
compilation, `#print axioms`, and kernel replay.

Ported from mathcode's `_lean_masking.py` + `axiom_checker.py`.
"""
from __future__ import annotations

import bisect
import json
import re
import sys
from typing import Any


def mask_comments_and_strings(text: str, *, mask_strings: bool = True) -> str:
    """Blank Lean comments/strings to spaces, preserving length and newlines.

    Returns a same-length string in which every comment byte and string byte is
    replaced by a space (newlines are always preserved), so downstream regexes
    never match inside comments/strings while line/column numbers stay exact.
    Handles nestable block comments ``/- ... -/``, line comments ``-- ...``, and
    string literals ``"..."`` with backslash escapes (``\\"`` does not close).
    """
    chars = list(text)
    i = 0
    n = len(chars)
    block_depth = 0
    in_string = False

    while i < n:
        if in_string:
            if chars[i] == "\\" and i + 1 < n:
                # Blank the backslash and the escaped char as a pair so an
                # escaped quote can never terminate the string.
                if chars[i] != "\n":
                    chars[i] = " "
                if chars[i + 1] != "\n":
                    chars[i + 1] = " "
                i += 2
                continue
            if chars[i] == '"':
                chars[i] = " "
                in_string = False
            elif chars[i] != "\n":
                chars[i] = " "
            i += 1
            continue

        if block_depth > 0:
            if i + 1 < n and text[i : i + 2] == "/-":
                chars[i] = chars[i + 1] = " "
                block_depth += 1
                i += 2
                continue
            if i + 1 < n and text[i : i + 2] == "-/":
                chars[i] = chars[i + 1] = " "
                block_depth -= 1
                i += 2
                continue
            if chars[i] != "\n":
                chars[i] = " "
            i += 1
            continue

        if i + 1 < n and text[i : i + 2] == "--":
            chars[i] = chars[i + 1] = " "
            i += 2
            while i < n and chars[i] != "\n":
                chars[i] = " "
                i += 1
            continue

        if i + 1 < n and text[i : i + 2] == "/-":
            chars[i] = chars[i + 1] = " "
            block_depth = 1
            i += 2
            continue

        if mask_strings and chars[i] == '"':
            chars[i] = " "
            in_string = True
            i += 1
            continue

        i += 1

    return "".join(chars)


# A Lean-identifier-aware boundary: the prime `'` is a legal identifier char, so
# `sorry'` and `my_sorry` must NOT match.
_PLACEHOLDER_RE = re.compile(r"(?<![\w'])(sorry|admit)(?![\w'])")
_ATTR_FRAGMENT = r"(?:@\[(?:[^\]\[]|\[[^\]]*\])*\]\s*)*"
_DECL_NAME = r"([^\s:({\[]+)"
_FORBIDDEN_RE = re.compile(
    rf"^\s*{_ATTR_FRAGMENT}(?:(?:private|protected|noncomputable|local|unsafe|partial)\s+)*"
    rf"(?:axiom|constant|postulate)\s+{_DECL_NAME}",
    re.MULTILINE,
)
_NONCOMPUTABLE_RE = re.compile(
    rf"^\s*{_ATTR_FRAGMENT}(?:(?:private|protected|local|unsafe|partial)\s+)*"
    r"noncomputable\s+"
    rf"{_ATTR_FRAGMENT}(?:(?:private|protected|local|unsafe|partial)\s+)*"
    rf"(?:def|instance)\s+{_DECL_NAME}",
    re.MULTILINE,
)


def _line_starts(text: str) -> list[int]:
    starts = [0]
    for i, ch in enumerate(text):
        if ch == "\n":
            starts.append(i + 1)
    return starts


def check(text: str) -> dict[str, Any]:
    """Scan a Lean source string for soundness violations.

    Returns ``{"clean": bool, "issues": [...]}`` where each issue is
    ``{"severity": "critical"|"info", "kind": str, "line": int, "column": int,
    ...}`` (placeholders carry ``"token"``; declarations carry ``"name"``).
    ``clean`` is True iff there are zero ``critical`` issues. Line numbers are
    1-based; columns are 0-based offsets from the start of the line.
    """
    masked = mask_comments_and_strings(text)
    starts = _line_starts(masked)

    def locate(pos: int) -> tuple[int, int]:
        line = bisect.bisect_right(starts, pos)
        column = pos - starts[line - 1]
        return line, column

    issues: list[dict[str, Any]] = []

    for m in _FORBIDDEN_RE.finditer(masked):
        line, column = locate(m.start())
        issues.append({
            "severity": "critical",
            "kind": "forbidden_declaration",
            "line": line,
            "column": column,
            "name": m.group(1),
            "message": f"forbidden `{m.group(0).strip()}` — proof must not introduce axioms",
        })

    for m in _PLACEHOLDER_RE.finditer(masked):
        line, column = locate(m.start())
        issues.append({
            "severity": "critical",
            "kind": "placeholder",
            "line": line,
            "column": column,
            "token": m.group(1),
            "message": f"proof placeholder `{m.group(1)}` still present",
        })

    for m in _NONCOMPUTABLE_RE.finditer(masked):
        line, column = locate(m.start())
        issues.append({
            "severity": "info",
            "kind": "noncomputable",
            "line": line,
            "column": column,
            "name": m.group(1),
            "message": f"noncomputable declaration `{m.group(1)}` — check if intentional",
        })

    issues.sort(key=lambda i: (i["line"], i["column"]))
    clean = not any(i["severity"] == "critical" for i in issues)
    return {"clean": clean, "issues": issues}


def main() -> None:
    if len(sys.argv) >= 2:
        with open(sys.argv[1], encoding="utf-8") as fh:
            text = fh.read()
    else:
        text = sys.stdin.read()
    result = check(text)
    print(json.dumps(result, indent=2))
    raise SystemExit(0 if result["clean"] else 1)


if __name__ == "__main__":
    main()
