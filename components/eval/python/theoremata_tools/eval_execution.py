"""Live eval-execution harness (Tier 4).

The benchmark *loaders* (``theoremata_tools.benchmarks.loaders``) already turn
each corpus into uniform items, and the *graders* score a response
*structurally*. What was missing — the "Partial (loader only)" P2 tracks — is a
driver that actually **compiles/runs** a generated artifact against the oracle
and turns the outcome into a *score*. This module is that driver.

Design: the compile/run step lives behind an injected ``Runner`` seam so the
harness is fully offline-testable on fixtures and only needs live tooling to
*actually* execute:

    Runner = Callable[[artifact], ExecOutcome]

* a **proof** artifact -> the runner would shell out to Lean and compile it;
* a **program** artifact (BRIDGE) -> the runner would run the oracle I/O tests.

``ExecOutcome`` is a plain dict::

    {"status": "pass" | "fail" | "error",
     "detail": Any,
     "oracle_results"?: list}   # per-oracle rows for the program track

SECURITY: every candidate / example field is treated as **untrusted data**. The
harness only *packs* it into an ``artifact`` dict and hands that to the runner
seam — it never ``exec``/``eval``/``compile``s candidate content in-process. In
tests the runner is a deterministic mock; in production it is the (sandboxed)
Lean compiler or program runner. That seam is the *only* execution boundary.

Everything here is deterministic (stable example order, no randomness) and pure
stdlib.
"""
from __future__ import annotations

from typing import Any, Callable, Sequence

from theoremata_tools.benchmarks.schema import AXIOMS_WHITELIST

# --------------------------------------------------------------------------- #
# ExecOutcome vocabulary
# --------------------------------------------------------------------------- #

PASS = "pass"
FAIL = "fail"
ERROR = "error"
_VALID_STATUS = (PASS, FAIL, ERROR)

# Runner seam: run(artifact) -> ExecOutcome. Never executed in-process here.
Runner = Callable[[dict[str, Any]], dict[str, Any]]
# Repair seam: revise(artifact, outcome) -> revised candidate | None.
Reviser = Callable[[dict[str, Any], dict[str, Any]], Any]

# Which tracks map to which artifact kind.
_PROOF_TRACKS = {
    "proof",
    "proof_compile",
    "proof_completion",
    "formalization",
    "compile",
}
_PROGRAM_TRACKS = {
    "program",
    "program_oracle",
    "oracle",
    "verified_programming",
    "bridge",
}


def make_outcome(
    status: str,
    detail: Any = "",
    oracle_results: list[Any] | None = None,
) -> dict[str, Any]:
    """Build (and validate) an ``ExecOutcome`` dict."""
    if status not in _VALID_STATUS:
        raise ValueError(f"invalid status {status!r} (expected one of {_VALID_STATUS})")
    out: dict[str, Any] = {"status": status, "detail": detail}
    if oracle_results is not None:
        out["oracle_results"] = oracle_results
    return out


def _normalize_outcome(raw: Any) -> dict[str, Any]:
    """Coerce whatever the runner returned into a valid ExecOutcome.

    A malformed / non-dict return (a buggy runner) is surfaced as ``error``
    rather than crashing the harness."""
    if not isinstance(raw, dict):
        return make_outcome(ERROR, detail={"reason": "runner_return_not_dict", "value": repr(raw)})
    status = raw.get("status")
    if status not in _VALID_STATUS:
        return make_outcome(
            ERROR, detail={"reason": "runner_bad_status", "status": repr(status)}
        )
    out: dict[str, Any] = {"status": status, "detail": raw.get("detail", "")}
    if "oracle_results" in raw:
        out["oracle_results"] = raw["oracle_results"]
    return out


def _is_pass(outcome: dict[str, Any]) -> bool:
    return outcome.get("status") == PASS


# --------------------------------------------------------------------------- #
# Artifact assembly (untrusted candidate content packed as DATA, never run)
# --------------------------------------------------------------------------- #

def _resolve_kind(example: dict[str, Any], track: str) -> str:
    t = (track or "").strip().lower()
    if t in _PROGRAM_TRACKS:
        return "program"
    if t in _PROOF_TRACKS:
        return "proof"
    # Fall back to the loader item's own kind.
    if example.get("kind") == "verified_programming":
        return "program"
    return "proof"


def _build_proof_artifact(
    example: dict[str, Any], candidate: Any, track: str
) -> dict[str, Any]:
    """A proof-compile artifact. Consumes loader fields ``formal`` +
    ``expected.{formal_statement,lean_name,axioms_whitelist}``."""
    expected = example.get("expected") or {}
    return {
        "id": example.get("id"),
        "kind": "proof",
        "track": track,
        # UNTRUSTED: the submitted Lean proof text — carried as opaque data.
        "candidate": candidate,
        "informal": example.get("informal", ""),
        "formal": example.get("formal"),
        "expected_formal": expected.get("formal_statement") or example.get("formal"),
        "lean_name": expected.get("lean_name"),
        "axioms_whitelist": list(expected.get("axioms_whitelist") or AXIOMS_WHITELIST),
    }


def _build_program_artifact(
    example: dict[str, Any], candidate: Any, track: str
) -> dict[str, Any]:
    """A program-oracle artifact. Consumes BRIDGE loader fields
    ``expected.{lean_signatures,function_name,arguments,oracle_tests}`` — the
    oracle inputs are named-kwarg dicts, bound by ``arguments`` (never by
    position)."""
    expected = example.get("expected") or {}
    oracle = expected.get("oracle_tests") or {}
    arguments = oracle.get("arguments") or expected.get("arguments") or []
    return {
        "id": example.get("id"),
        "kind": "program",
        "track": track,
        # UNTRUSTED: the submitted program text — carried as opaque data.
        "candidate": candidate,
        "informal": example.get("informal", ""),
        "function_name": expected.get("function_name"),
        "arguments": list(arguments),
        "lean_signatures": list(expected.get("lean_signatures") or []),
        "oracle_tests": {
            "inputs": oracle.get("inputs"),
            "expected_outputs": oracle.get("expected_outputs"),
            "bind": oracle.get("bind", "kwargs"),
            "arguments": list(arguments),
        },
    }


def build_artifact(example: dict[str, Any], candidate: Any, track: str) -> dict[str, Any]:
    """Pack one (example, candidate) into a runner artifact. Dispatches on the
    track / example kind. Pure data-shuffling — candidate is never executed."""
    kind = _resolve_kind(example, track)
    if kind == "program":
        return _build_program_artifact(example, candidate, track)
    return _build_proof_artifact(example, candidate, track)


def _candidate_for(candidates: Any, example: dict[str, Any], idx: int) -> Any:
    """Look up the candidate for an example. ``candidates`` may be an
    ``id -> candidate`` mapping or a list aligned with ``examples``."""
    if isinstance(candidates, dict):
        return candidates.get(example.get("id"), "")
    if isinstance(candidates, (list, tuple)):
        return candidates[idx] if idx < len(candidates) else ""
    return ""


# --------------------------------------------------------------------------- #
# Safe seam invocation (the runner is the ONLY execution boundary)
# --------------------------------------------------------------------------- #

def _safe_run(runner: Runner, artifact: dict[str, Any]) -> dict[str, Any]:
    """Invoke the runner seam, catching *any* exception as status ``error`` so a
    misbehaving compiler/runner never crashes the harness."""
    try:
        raw = runner(artifact)
    except Exception as exc:  # noqa: BLE001 — a raising runner is an item error
        return make_outcome(
            ERROR, detail={"reason": "runner_raised", "exception": repr(exc)}
        )
    return _normalize_outcome(raw)


def _safe_revise(reviser: Reviser, artifact: dict[str, Any], outcome: dict[str, Any]) -> Any:
    """Invoke the repair seam, swallowing exceptions (a broken reviser simply
    yields no repair)."""
    try:
        return reviser(artifact, outcome)
    except Exception:  # noqa: BLE001
        return None


# --------------------------------------------------------------------------- #
# Core driver
# --------------------------------------------------------------------------- #

def execute_track(
    examples: Sequence[dict[str, Any]],
    candidates: Any,
    runner: Runner,
    *,
    track: str = "proof",
    reviser: Reviser | None = None,
) -> dict[str, Any]:
    """Compile/run every (example, candidate) through ``runner`` and score it.

    Parameters
    ----------
    examples:
        Loader items (the common benchmark schema). Iterated in order.
    candidates:
        ``id -> candidate`` mapping or a list aligned with ``examples``. Missing
        entries are graded as an empty candidate.
    runner:
        The injected execution seam ``run(artifact) -> ExecOutcome``. This is the
        *only* place candidate content is executed (a mock in tests; live Lean /
        a program runner in production).
    track:
        ``"proof"`` (compile-and-check) or ``"program"`` (run oracle tests).
        Dispatches artifact construction; also falls back to the item ``kind``.
    reviser:
        Optional repair seam. When a candidate does not pass, ``reviser`` may
        propose a fixed candidate that is re-run **once** (BRIDGE's repair loop).

    Returns
    -------
    A scored report::

        {track, n, n_pass, n_fail, n_error, pass_rate,
         items: [{id, status, detail, oracle_results?, attempts, repaired}]}
    """
    items: list[dict[str, Any]] = []
    n_pass = n_fail = n_error = 0

    for idx, example in enumerate(examples):
        candidate = _candidate_for(candidates, example, idx)
        artifact = build_artifact(example, candidate, track)
        artifact["attempt"] = 0
        outcome = _safe_run(runner, artifact)
        attempts = 1
        repaired = False

        # Repair loop: one re-run of a fixed candidate if it didn't pass.
        if not _is_pass(outcome) and reviser is not None:
            revised = _safe_revise(reviser, artifact, outcome)
            if revised is not None:
                artifact2 = build_artifact(example, revised, track)
                artifact2["attempt"] = 1
                outcome = _safe_run(runner, artifact2)
                attempts = 2
                repaired = True

        status = outcome.get("status")
        if status == PASS:
            n_pass += 1
        elif status == FAIL:
            n_fail += 1
        else:
            n_error += 1

        record: dict[str, Any] = {
            "id": example.get("id"),
            "status": status,
            "detail": outcome.get("detail"),
            "attempts": attempts,
            "repaired": repaired,
        }
        if "oracle_results" in outcome:
            record["oracle_results"] = outcome["oracle_results"]
        items.append(record)

    n = len(items)
    return {
        "track": track,
        "n": n,
        "n_pass": n_pass,
        "n_fail": n_fail,
        "n_error": n_error,
        "pass_rate": (n_pass / n) if n else None,
        "items": items,
    }


# --------------------------------------------------------------------------- #
# Offline runner/reviser factories (pure-JSON path for run())
# --------------------------------------------------------------------------- #

def outcome_table_runner(
    outcomes: dict[str, Any],
    repair_outcomes: dict[str, Any] | None = None,
) -> Runner:
    """A deterministic table-lookup runner for the offline JSON path.

    Maps ``artifact id -> ExecOutcome``. When an artifact is a repair re-run
    (``attempt >= 1``) and ``repair_outcomes`` has an entry, that entry wins —
    letting a JSON request model the "repair flips it to pass" flow without a
    live compiler. An id with no table entry yields status ``error``.
    """
    repair_outcomes = repair_outcomes or {}

    def _run(artifact: dict[str, Any]) -> dict[str, Any]:
        aid = artifact.get("id")
        if artifact.get("attempt", 0) >= 1 and aid in repair_outcomes:
            return repair_outcomes[aid]
        if aid in outcomes:
            return outcomes[aid]
        return make_outcome(ERROR, detail={"reason": "no_outcome_for_id", "id": aid})

    return _run


def table_reviser(repairs: dict[str, Any]) -> Reviser:
    """A table-lookup reviser: ``artifact id -> revised candidate`` (or no
    repair when the id is absent)."""

    def _revise(artifact: dict[str, Any], outcome: dict[str, Any]) -> Any:
        return repairs.get(artifact.get("id"))

    return _revise


# --------------------------------------------------------------------------- #
# JSON dispatch (worker.py hook) — op ``eval_execution``
# --------------------------------------------------------------------------- #

def run(
    request: dict[str, Any],
    *,
    runner: Runner | None = None,
    reviser: Reviser | None = None,
) -> dict[str, Any]:
    """Op ``eval_execution``.

    Request schema::

        {
          "op": "eval_execution",
          "track": "proof" | "program",
          "examples": [ <loader item>, ... ],
          "candidates": { id: candidate } | [ candidate, ... ],
          # offline runner seam (used when no `runner` kwarg is injected):
          "outcomes": { id: ExecOutcome },
          "repair_outcomes": { id: ExecOutcome },   # optional (repair re-run)
          "repairs": { id: revised_candidate }      # optional offline reviser
        }

    A live caller injects ``runner`` (and optionally ``reviser``) directly; the
    ``outcomes``/``repairs`` tables exist so the op stays offline-testable.
    """
    op = request.get("op", "eval_execution")
    if op != "eval_execution":
        raise ValueError(f"unknown op: {op!r} (expected 'eval_execution')")

    track = request.get("track") or "proof"
    examples = request.get("examples") or []
    candidates = request.get("candidates")
    if candidates is None:
        candidates = {}

    if runner is None:
        runner = outcome_table_runner(
            request.get("outcomes") or {}, request.get("repair_outcomes")
        )
    if reviser is None and request.get("repairs"):
        reviser = table_reviser(request["repairs"])

    report = execute_track(examples, candidates, runner, track=track, reviser=reviser)
    return {"op": "eval_execution", **report}
