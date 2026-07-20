"""BRIDGE-style verified-programming runner: STRUCTURAL SCORING ONLY.

WHAT THIS IS NOT
================
This runner does not verify anything. It grades a submitted Lean artifact on
structural signals only (the required function signature appears verbatim, and
no ``sorry``/``admit``/non-whitelisted axiom is present). It never compiles the
artifact and never runs the BRIDGE oracle input/output pairs against it.

The distinction matters because the two are trivially separable: a submission of
``def minimumPushes (word : String) : Int := 0`` satisfies every structural
signal for the whole benchmark while being wrong on essentially every oracle
case. So a structural score is an upper bound on a verified score, and nothing
more. Reporting it under a name like ``accuracy`` or ``correct`` invites a
reader to treat it as a pass rate for verified programs, which it is not, and
that misreading is the reason this module names every metric ``structural_*``
and stamps ``verified: False`` / ``verification_status: "not_verified"`` on
every response and on every per-item row.

WHY OPT FOR HONEST LABELS OVER RUNNING THE ORACLE
=================================================
The vendored corpus (``resources/BRIDGE-main``) does ship real oracle data: all
178 rows carry ``tests.inputs`` (named-kwarg dicts) and ``tests.expected_outputs``.
What it does not ship is anything that can consume them here:

* there is no reference implementation. ``python.function_signature`` is a bare
  ``def f(...) -> T:\n    pass`` stub with no body, so the oracle can only be
  applied to a *submitted* artifact, never self-checked;
* the submitted artifact is Lean, so executing the oracle means compiling
  untrusted model-generated Lean against a Lake/Mathlib project and running it,
  per item, with a timeout and an isolation boundary.

Executing untrusted content is deliberately not done in this module.
``resources/`` is vendored third-party data and is treated as untrusted: some
vendored repos ship helper scripts that phone home with file contents, so no
script under ``resources/`` is ever invoked, and no candidate text is ever
``exec``/``eval``/``compile``d in-process. The project already has the correct
place for that boundary: ``theoremata_tools.eval_execution``, whose injected
``Runner`` seam is the single point where a sandboxed compiler or program runner
is allowed to touch candidate content. When a sandboxed Lean runner exists, the
oracle path belongs behind that seam, and this module's report should then gain
a genuinely verified metric alongside (not instead of) the structural one.

Until then: an honest structural score is useful, a structural score labelled as
verification is not.
"""
from __future__ import annotations

from typing import Any

from .graders import grade
from .loaders import LOADERS

# Stamped on every response and every per-item row. Downstream consumers and
# report generators should key off these rather than inferring intent from a
# metric name, which is what went wrong before.
NOT_VERIFIED = "not_verified"
GRADING_MODE = "structural_only"

DISCLAIMER = (
    "NOT A VERIFICATION RESULT. Scores are structural only: the required Lean "
    "signature is present and no sorry/admit/non-whitelisted axiom appears. "
    "Nothing was compiled and the BRIDGE oracle input/output tests were NOT "
    "executed, so a trivially wrong implementation with a correct signature "
    "counts as a structural pass. Treat structural_pass_rate as an upper bound "
    "on a verified pass rate, never as one."
)


def _oracle_available(item: dict[str, Any]) -> bool:
    """Does this item ship oracle I/O that a future sandboxed runner could use?

    Reported so the gap is visible as a number: N items carry executable oracle
    data that this run did not execute. A silent gap is how a structural score
    gets mistaken for a verified one.
    """
    oracle = (item.get("expected") or {}).get("oracle_tests") or {}
    return bool(oracle.get("inputs")) and bool(oracle.get("expected_outputs"))


def run_verified_programming(
    *,
    benchmark: str = "bridge178",
    responses: dict[str, Any] | None = None,
    limit: int | None = None,
) -> dict[str, Any]:
    """Structurally grade submitted BRIDGE-style artifacts. No execution.

    Parameters
    ----------
    benchmark:
        Registry name (default ``bridge178``).
    responses:
        ``item_id -> submitted Lean text``. Missing ids grade as empty. The text
        is treated as untrusted data and is only ever string-matched.
    limit:
        Cap the number of items processed (after load).

    Returns a report whose every metric is named ``structural_*`` and which
    carries ``verified: False``. It contains no ``accuracy``/``correct`` key by
    design: those names previously implied a verification verdict this track
    cannot produce.

    When the corpus is absent the loader yields no items and this returns an
    empty, clearly-marked report rather than failing.
    """
    if benchmark not in LOADERS:
        raise KeyError(f"unknown benchmark {benchmark!r}; known: {sorted(LOADERS)}")
    items = LOADERS[benchmark]()
    if isinstance(limit, int) and limit >= 0:
        items = items[:limit]
    responses = responses or {}

    results: list[dict[str, Any]] = []
    responded = 0
    structural_pass = 0
    oracle_available = 0
    for item in items:
        verdict = grade(item, responses.get(item["id"], ""))
        has_oracle = _oracle_available(item)
        if verdict.get("is_solved"):
            responded += 1
        if verdict.get("is_correct"):
            structural_pass += 1
        if has_oracle:
            oracle_available += 1
        # Per-item keys are renamed too. Passing the grader's `is_correct`
        # through unchanged would let a row be read as a verification verdict
        # even when the enclosing report is labelled correctly.
        results.append(
            {
                "id": item["id"],
                "responded": bool(verdict.get("is_solved")),
                "structural_pass": bool(verdict.get("is_correct")),
                "verified": False,
                "verification_status": NOT_VERIFIED,
                "oracle_available": has_oracle,
                "oracle_executed": False,
                "detail": verdict.get("detail"),
            }
        )

    n = len(items)
    return {
        "benchmark": benchmark,
        "n": n,
        "corpus_present": n > 0,
        # The claim-level fields come first so a truncated or shallow render of
        # this dict still shows what kind of result it is.
        "verified": False,
        "verification_status": NOT_VERIFIED,
        "grading_mode": GRADING_MODE,
        "is_verification_result": False,
        "disclaimer": DISCLAIMER,
        "responded": responded,
        "structural_pass": structural_pass,
        "structural_pass_rate": (structural_pass / n) if n else None,
        "structural_signals": ["lean_signature_present", "no_sorry_or_admit_axioms"],
        # The size of what was skipped, stated rather than implied.
        "oracle_available": oracle_available,
        "oracle_executed": 0,
        "oracle_note": (
            f"{oracle_available} of {n} items ship executable oracle I/O that "
            "this run did NOT execute; running it requires compiling untrusted "
            "Lean in a sandbox behind the eval_execution Runner seam."
        ),
        "results": results,
    }
