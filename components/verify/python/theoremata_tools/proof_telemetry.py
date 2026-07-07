"""Proof telemetry — a tactic histogram + proven/unproven verdict for Lean source.

Adapted from mathcode's `proof_stats.py`, distilled to the signal Theoremata
needs as a *ranking / logging* input for DAG nodes:

  * a **tactic histogram** over a curated tactic vocabulary (how many ``simp``,
    ``rw``, ``exact``, ``omega``, ``linarith``, ``decide``, ``induction`` ...),
  * a **proven/unproven verdict** (does the source still contain ``sorry`` or
    ``admit``?),
  * a small derived **ranking score** (proven proofs rank above unproven; among
    proven, shorter/lower-automation proofs rank higher — a proxy for a cleaner,
    more trustworthy proof).

Crucially, tactic-like tokens that appear inside comments or string literals must
not be counted (a comment ``-- just use simp here`` is not a ``simp`` call). We
reuse the nested-comment/string masking idea from mathcode's ``_lean_masking.py``
via a small, robust re-implementation (`mask_lean`) that blanks comments/strings
to spaces while preserving length and newline positions, so counts and any
line/column math stay exact.

Standard-library only; imports no other Theoremata code.
"""
from __future__ import annotations

import json
import re
import sys
from typing import Any

# Curated tactic vocabulary (mathcode's list, kept as a sensible default). Order
# is irrelevant; matching is by word boundary against the masked source.
TACTIC_VOCAB = (
    "simp", "simpa", "rfl", "ring", "ring_nf", "omega", "linarith", "nlinarith",
    "norm_num", "norm_cast", "push_cast", "aesop", "decide", "native_decide",
    "exact", "exact?", "apply", "rw", "rwa", "intro", "intros", "constructor",
    "cases", "rcases", "obtain", "have", "let", "show", "suffices", "calc",
    "induction", "ext", "funext", "congr", "convert", "refine", "use", "trivial",
    "tauto", "contradiction", "exfalso", "push_neg", "by_contra", "field_simp",
    "gcongr", "positivity", "assumption", "unfold", "change", "subst", "split",
)

# Build one alternation regex, longest-first so `exact?` beats `exact`, etc. The
# guards forbid identifier characters (and Lean's guillemet-quoted names and a
# leading `.`) on either side so `simple`/`.simp`/`«simp»` are not matched.
_ESCAPED = sorted((re.escape(t) for t in TACTIC_VOCAB), key=len, reverse=True)
_TACTIC_RE = re.compile(
    r"(?<![\w'.«])(" + "|".join(_ESCAPED) + r")(?![\w'»])"
)

_SORRY_RE = re.compile(r"(?<![\w'])sorry(?![\w'])")
_ADMIT_RE = re.compile(r"(?<![\w'])admit(?![\w'])")


def mask_lean(text: str, *, mask_strings: bool = True) -> str:
    """Blank Lean comments/strings to spaces, preserving length and newlines.

    Handles nestable block comments ``/- ... /- ... -/ ... -/``, line comments
    ``-- ...``, and double-quoted string literals with backslash escapes. Every
    masked byte becomes a space except ``\\n``, which is always preserved, so the
    returned string is the same length as the input and downstream regexes never
    match inside a comment or a string while line/column math stays exact.
    """
    chars = list(text)
    i = 0
    n = len(chars)
    block_depth = 0
    in_string = False

    while i < n:
        if in_string:
            if chars[i] == "\\" and i + 1 < n:
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


def tactic_histogram(source: str, *, masked: bool = False) -> dict[str, int]:
    """Return a ``{tactic: count}`` histogram over the curated vocabulary.

    Tokens inside comments/strings are ignored (the source is masked first unless
    `masked` says it already is). Entries with zero count are omitted; the result
    is ordered by descending frequency then tactic name for stable output.
    """
    scan = source if masked else mask_lean(source)
    counts: dict[str, int] = {}
    for m in _TACTIC_RE.finditer(scan):
        tac = m.group(1)
        counts[tac] = counts.get(tac, 0) + 1
    return dict(sorted(counts.items(), key=lambda kv: (-kv[1], kv[0])))


def has_sorry(source: str, *, masked: bool = False) -> bool:
    scan = source if masked else mask_lean(source)
    return bool(_SORRY_RE.search(scan))


def has_admit(source: str, *, masked: bool = False) -> bool:
    scan = source if masked else mask_lean(source)
    return bool(_ADMIT_RE.search(scan))


def verdict(source: str, *, masked: bool = False) -> str:
    """``"unproven"`` if a live ``sorry``/``admit`` remains, else ``"proven"``."""
    scan = source if masked else mask_lean(source)
    return "unproven" if (has_sorry(scan, masked=True) or has_admit(scan, masked=True)) else "proven"


def _ranking_score(proven: bool, total_calls: int, unique: int, uses_native_decide: bool) -> float:
    """Derive a ranking signal in roughly [0, 1].

    Unproven proofs score 0. A proven proof starts at 1.0 and is gently penalized
    for length (total tactic calls) and for leaning on ``native_decide`` (an
    untrusted, kernel-bypassing tactic Theoremata's soundness gate rejects), so
    shorter, kernel-honest proofs rank higher when best-of-N candidates tie.
    """
    if not proven:
        return 0.0
    score = 1.0 / (1.0 + 0.03 * total_calls)
    if uses_native_decide:
        score *= 0.5
    return round(score, 6)


def analyze(source: str) -> dict[str, Any]:
    """Full telemetry record for a Lean proof source (JSON-able).

    Fields: ``tactic_frequency`` (histogram), ``unique_tactics``,
    ``total_tactic_calls``, ``has_sorry``, ``has_admit``, ``status``
    (``proven``/``unproven``), ``uses_native_decide``, and a derived
    ``ranking_score`` in [0, 1] usable as a DAG-node ranking signal.
    """
    scan = mask_lean(source)
    histogram = tactic_histogram(scan, masked=True)
    total = sum(histogram.values())
    sorry = has_sorry(scan, masked=True)
    admit = has_admit(scan, masked=True)
    proven = not (sorry or admit)
    uses_native = histogram.get("native_decide", 0) > 0
    return {
        "tactic_frequency": histogram,
        "unique_tactics": len(histogram),
        "total_tactic_calls": total,
        "has_sorry": sorry,
        "has_admit": admit,
        "status": "proven" if proven else "unproven",
        "uses_native_decide": uses_native,
        "ranking_score": _ranking_score(proven, total, len(histogram), uses_native),
    }


# --- worker / CLI entrypoint -------------------------------------------------
def run(request: dict[str, Any]) -> dict[str, Any]:
    """JSON dispatch used by the worker: analyze ``source`` (or read ``path``)."""
    source = request.get("source")
    if source is None:
        path = request["path"]
        with open(path, encoding="utf-8") as fh:
            source = fh.read()
    result = analyze(source)
    result["ok"] = True
    return result


def main() -> None:
    request = json.load(sys.stdin)
    try:
        response = run(request)
    except Exception as exc:  # noqa: BLE001 - surface as JSON
        response = {"ok": False, "error": str(exc)}
    print(json.dumps(response))
    raise SystemExit(0 if response.get("ok") else 1)


if __name__ == "__main__":
    main()
