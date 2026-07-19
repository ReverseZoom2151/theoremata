"""Runtime bridge to a Wolfram Engine kernel, treated strictly as an UNTRUSTED oracle.

Wolfram is a search engine here, never a checker. It cannot enter the verification
gate for four independent reasons, recorded so nobody re-derives them:

1. The kernel is closed and proprietary. It cannot be audited and its result
   cannot be kernel-rechecked, so gate layers 2 and 3 are structurally impossible.
2. It is not proof-producing. ``Simplify`` / ``Integrate`` / ``Reduce`` return an
   answer, not a derivation, so there is nothing to re-verify.
3. It makes silent generic assumptions (branch cuts, generic position, implicit
   non-degeneracy). That is exactly the unaccounted-hypothesis failure mode
   ``components/prover/hypothesis_audit.rs`` exists to catch.
4. The free engine licence is development-only, so it can never be a required
   dependency of a deployed product.

What it IS good for is the slot the architecture already has: an untrusted engine
that FINDS an object, which one of our own pure checkers then verifies. That is
how SymPy is already used, and it is the cert-log design (``cert_sos.py`` and
friends): the checker is the sole trust boundary. Every caller of this module
owes its result an independent check before anything is concluded from it.

Absence is the normal case. Nothing here raises because an engine is missing; the
probe returns False and every evaluation returns an ``unavailable`` response, so
CI and any machine without a licence exercise the same degrade path.
"""
from __future__ import annotations

import os
import shutil
from typing import Any

#: Environment override pointing at a ``wolframscript`` (or ``WolframKernel``)
#: binary, for installs that are not on PATH.
WOLFRAM_BINARY_ENV = "THEOREMATA_WOLFRAM"

#: Opt-in switch. The engine is heavyweight and licence-gated, so it is consulted
#: only when an operator explicitly turns it on, even if a kernel is installed.
WOLFRAM_ENABLED_ENV = "THEOREMATA_WOLFRAM_ENABLED"

#: Default per-evaluation deadline. A symbolic query can run unboundedly (a
#: cylindrical decomposition on a bad input does not terminate in practice), so
#: every call is deadlined rather than trusted to return.
DEFAULT_TIMEOUT_SECONDS = 30.0

#: Candidate binary names, in preference order. ``wolframscript`` is the
#: supported command-line entry point for the free Developer engine.
_BINARIES = ("wolframscript", "wolfram", "WolframKernel", "math")


def _enabled() -> bool:
    """Whether the operator opted in. Default off."""
    raw = os.environ.get(WOLFRAM_ENABLED_ENV, "")
    return raw.strip().lower() in {"1", "true", "yes", "on"}


def _binary() -> str | None:
    """Locate a Wolfram command-line binary, honouring the explicit override."""
    override = os.environ.get(WOLFRAM_BINARY_ENV, "").strip()
    if override:
        # An explicit path is trusted to be what the operator meant, but it still
        # has to exist: a stale override must degrade, not blow up mid-run.
        return override if (os.path.isfile(override) or shutil.which(override)) else None
    for name in _BINARIES:
        found = shutil.which(name)
        if found:
            return found
    return None


def available() -> bool:
    """True only if the operator opted in AND a kernel binary was located.

    Deliberately conservative: a machine with a kernel installed but no opt-in
    reports unavailable, because consulting a licence-gated engine should be a
    decision rather than a side effect of it being on PATH.
    """
    return _enabled() and _binary() is not None


def _describe_unavailable() -> str:
    """Say WHICH precondition failed, so an operator can act on it."""
    if not _enabled():
        return f"Wolfram support is off; set {WOLFRAM_ENABLED_ENV}=1 to enable it"
    return (
        "no Wolfram binary found on PATH "
        f"(looked for {', '.join(_BINARIES)}; set {WOLFRAM_BINARY_ENV} to override)"
    )


def unavailable_response(**extra: Any) -> dict:
    """The canonical no-engine response every caller returns.

    Callers use this instead of inventing their own shape so that "we could not
    look" is always distinguishable from "we looked and found nothing". Conflating
    those two is the failure this whole module is written to avoid.
    """
    response = {
        "ok": False,
        "result": None,
        "error": None,
        "unavailable": True,
        "reason": _describe_unavailable(),
    }
    response.update(extra)
    return response


def evaluate(code: str, *, timeout: float = DEFAULT_TIMEOUT_SECONDS) -> dict:
    """Evaluate Wolfram Language source and return its output as text.

    Returns ``{"ok", "result", "error", "unavailable"}``. ``unavailable`` is True
    when there is no engine to ask, and ``ok`` is then False. A timeout, a
    non-zero exit, or a kernel error is ``ok=False`` with ``error`` set.

    The result is a STRING, deliberately. Parsing it into numbers is the caller's
    job, because only the caller knows what exactness it needs, and a float
    silently entering a certificate would break the exact arithmetic our checkers
    depend on. Nothing here interprets the mathematics.
    """
    if not available():
        return unavailable_response()

    binary = _binary()
    if binary is None:
        return unavailable_response()

    # Imported lazily: subprocess is only needed on the live path, and keeping it
    # out of module import means the absent-engine path stays trivial.
    import subprocess

    try:
        completed = subprocess.run(
            [binary, "-code", code],
            capture_output=True,
            text=True,
            timeout=timeout,
            # Never inherit a shell; the code string is Wolfram source, not a
            # command line, and must not be re-interpreted by a shell.
            shell=False,
        )
    except subprocess.TimeoutExpired:
        return {
            "ok": False,
            "result": None,
            "error": f"wolfram evaluation timed out after {timeout}s",
            "unavailable": False,
        }
    except OSError as exc:
        # The binary vanished or is not executable between probe and run. That is
        # an absent engine, not a mathematical result.
        return unavailable_response(error=str(exc))

    stdout = (completed.stdout or "").strip()
    stderr = (completed.stderr or "").strip()

    if completed.returncode != 0:
        return {
            "ok": False,
            "result": stdout or None,
            "error": stderr or f"wolfram exited {completed.returncode}",
            "unavailable": False,
        }

    # A zero exit is NOT proof of success. wolframscript reports evaluation
    # failures in-band while still exiting 0, so the sentinels are checked
    # explicitly. This is the same discipline the formal backends apply in
    # `SuccessSignal`: never trust an exit code on its own.
    lowered = stdout.lower()
    for marker in ("$failed", "syntax::", "::argx", "::argrx", "::nonopt"):
        if marker in lowered:
            return {
                "ok": False,
                "result": stdout or None,
                "error": f"wolfram reported a failure marker ({marker}) despite exit 0",
                "unavailable": False,
            }

    return {
        "ok": True,
        "result": stdout,
        "error": stderr or None,
        "unavailable": False,
    }


def run(request: dict) -> dict:
    """Worker entry point: probe, or evaluate a supplied expression.

    ``{"op": "available"}`` reports the probe. ``{"op": "evaluate", "code": ...}``
    evaluates. The evaluate op exists for diagnostics and for the generator
    modules that layer their own checking on top; it is not a certification path
    and returns nothing that may be treated as verified.
    """
    op = request.get("op", "available")
    if op == "available":
        return {
            "ok": True,
            "available": available(),
            "reason": None if available() else _describe_unavailable(),
        }
    if op == "evaluate":
        code = request.get("code")
        if not isinstance(code, str) or not code.strip():
            return {"ok": False, "error": "evaluate requires a non-empty `code` string"}
        timeout = request.get("timeout", DEFAULT_TIMEOUT_SECONDS)
        try:
            timeout = float(timeout)
        except (TypeError, ValueError):
            timeout = DEFAULT_TIMEOUT_SECONDS
        result = evaluate(code, timeout=timeout)
        # Restated at the boundary so no downstream consumer can read a raw
        # Wolfram evaluation as a checked result.
        result["trusted"] = False
        return result
    return {"ok": False, "error": f"unknown wolfram_link op: {op}"}
