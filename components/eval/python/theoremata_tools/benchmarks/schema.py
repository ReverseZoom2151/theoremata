"""Common internal benchmark item schema (Tier 4).

Every corpus loader turns its native format into a uniform ``item`` dict so the
grader dispatch and eval harness never has to special-case a corpus. The schema
(deliberately the exact set named in the Tier-4 spec)::

    {
      "id":        str,          # stable, corpus-unique primary key
      "kind":      str,          # see KINDS below
      "informal":  str,          # NL statement: docstring / blueprint prose / problem
      "formal":    str | None,   # Lean statement/stub when one exists
      "expected":  Any,          # gold target (shape depends on the track)
      "provenance": dict,        # {corpus, path, ...} — where it came from
      "grading":   dict,         # {track, method, ...} — how to grade it
    }

The three ``kind`` values line up 1:1 with the three tracks/graders:

* ``formalization``  -> Lean compile + axiom-whitelist + statement-preservation
* ``nl_answer``      -> deterministic-rubric (sympy) answer grading
* ``falsification``  -> flaw / counterexample detection (or must-reject)
"""
from __future__ import annotations

from typing import Any

KINDS = (
    "formalization",
    "nl_answer",
    "falsification",
    "verified_programming",
    "statement_target",
    "external_artifact",
    "reformulation",
    # QuantumLean ships model outputs + a human 0-2 rubric, NOT a gold formal
    # proof — graded typecheck-only (no statement-preservation is possible).
    "scientific_formalization",
    # IMO-ProofBench: an NL proof + a gold human grade + a model grade — an
    # EVALUATOR-CALIBRATION item (grade the grader, not the proof).
    "proof_grading",
    # A structured Lean/Mathlib tactic reference entry (retrieval / KB corpus).
    "tactic_reference",
)

# The canonical Lean "consistency" axiom trio. Anything else (notably sorryAx)
# fails the axioms gate. Straight from FormalQualBench's comparator recipe.
AXIOMS_WHITELIST = ("propext", "Quot.sound", "Classical.choice")


def make_item(
    *,
    id: str,
    kind: str,
    informal: str,
    expected: Any,
    grading: dict[str, Any],
    formal: str | None = None,
    provenance: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Build (and lightly validate) one benchmark item dict."""
    if kind not in KINDS:
        raise ValueError(f"unknown item kind: {kind!r} (expected one of {KINDS})")
    if not id:
        raise ValueError("item id must be non-empty")
    return {
        "id": str(id),
        "kind": kind,
        "informal": informal or "",
        "formal": formal,
        "expected": expected,
        "provenance": dict(provenance or {}),
        "grading": dict(grading),
    }
