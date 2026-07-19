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

Two transports, same contract. A LOCAL ``wolframscript`` binary is preferred when
present. Otherwise the CAG ``WolframLanguageCompute`` endpoint evaluates the same
source over HTTP, which removes the local install and its licence entirely at the
cost of sending the expression to a third party. Callers do not choose; they call
:func:`evaluate` and get whichever is configured, with the transport reported so a
result is always attributable.
"""
from __future__ import annotations

import json
import os
import shutil
import urllib.error
import urllib.request
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

#: API key for the CAG ``WolframLanguageCompute`` endpoint, which evaluates
#: Wolfram Language over HTTP with no local install. Access is via
#: partner-program@wolfram.com, so this is not the self-serve Alpha AppID.
CLOUD_KEY_ENV = "THEOREMATA_WOLFRAM_CLOUD_KEY"

CLOUD_COMPUTE_ENDPOINT = "https://services.wolfram.com/api/cag/v1/WolframLanguageCompute"

#: The endpoint's own default output cap. Oversized results come back elided with
#: `<<` and `>>` markers, which would silently truncate a certificate, so the
#: elision is detected rather than parsed.
CLOUD_MAX_CHARS = 10000


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


def _cloud_key() -> str | None:
    value = os.environ.get(CLOUD_KEY_ENV, "").strip()
    return value or None


def transport() -> str | None:
    """Which transport an evaluation would use: ``local``, ``cloud``, or None.

    Local is preferred when both are configured. A local kernel keeps the
    expression on the machine, and an untrusted oracle we are already not
    trusting is still better run without shipping the goal to a third party.
    """
    if not _enabled():
        return None
    if _binary() is not None:
        return "local"
    if _cloud_key() is not None:
        return "cloud"
    return None


def available() -> bool:
    """True only if the operator opted in AND some transport is configured.

    Deliberately conservative: a machine with a kernel installed but no opt-in
    reports unavailable, because consulting a licence-gated engine should be a
    decision rather than a side effect of it being on PATH.
    """
    return transport() is not None


def _describe_unavailable() -> str:
    """Say WHICH precondition failed, so an operator can act on it."""
    if not _enabled():
        return f"Wolfram support is off; set {WOLFRAM_ENABLED_ENV}=1 to enable it"
    return (
        "no Wolfram transport configured: no binary on PATH "
        f"(looked for {', '.join(_BINARIES)}; set {WOLFRAM_BINARY_ENV}) "
        f"and no cloud key ({CLOUD_KEY_ENV})"
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
    chosen = transport()
    if chosen is None:
        return unavailable_response()
    if chosen == "cloud":
        return _evaluate_cloud(code, timeout=timeout)
    return _evaluate_local(code, timeout=timeout)


def _evaluate_cloud(code: str, *, timeout: float) -> dict:
    """Evaluate through the CAG WolframLanguageCompute endpoint.

    Same contract as the local path. The expression leaves the machine, which is
    why the transport is reported back: a caller auditing where a certificate came
    from needs to know it was computed off-box.
    """
    key = _cloud_key()
    if key is None:
        return unavailable_response()

    payload = json.dumps(
        {
            "code": code,
            "maxChars": CLOUD_MAX_CHARS,
            # The endpoint enforces its own deadline too; passing ours keeps the
            # two from disagreeing about how long is too long.
            "timeConstraint": timeout,
        }
    ).encode("utf-8")
    request = urllib.request.Request(
        CLOUD_COMPUTE_ENDPOINT,
        data=payload,
        headers={
            "Authorization": f"Bearer {key}",
            "Content-Type": "application/json",
            "User-Agent": "theoremata",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout + 5.0) as response:
            body = response.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as exc:
        detail = ""
        try:
            detail = exc.read().decode("utf-8", "replace")[:300]
        except Exception:
            pass
        return {
            "ok": False,
            "result": None,
            "error": f"WolframLanguageCompute returned HTTP {exc.code}: {detail}",
            "unavailable": False,
            "transport": "cloud",
        }
    except (urllib.error.URLError, TimeoutError, OSError) as exc:
        return {
            "ok": False,
            "result": None,
            "error": f"network failure reaching WolframLanguageCompute: {exc}",
            "unavailable": False,
            "transport": "cloud",
        }

    try:
        parsed = json.loads(body)
    except json.JSONDecodeError:
        return {
            "ok": False,
            "result": None,
            "error": "WolframLanguageCompute returned unparseable JSON",
            "unavailable": False,
            "transport": "cloud",
        }

    result = parsed.get("result")
    if not parsed.get("success") or not isinstance(result, str):
        return {
            "ok": False,
            "result": result if isinstance(result, str) else None,
            "error": f"WolframLanguageCompute reported failure (code {parsed.get('code')})",
            "unavailable": False,
            "transport": "cloud",
        }

    # Elision is a correctness hazard, not a display detail. The endpoint truncates
    # oversized output with `<<` and `>>`, and a truncated polynomial or rule list
    # parses as a SHORTER valid expression rather than as an error. Refuse it.
    if "<<" in result and ">>" in result:
        return {
            "ok": False,
            "result": result,
            "error": (
                "WolframLanguageCompute elided its output (<< >>); a truncated "
                "expression can parse as a valid shorter one, so it is refused"
            ),
            "unavailable": False,
            "transport": "cloud",
        }

    return _classify_output(result, stderr=None, transport="cloud")


def _evaluate_local(code: str, *, timeout: float) -> dict:
    """Evaluate by shelling out to a locally installed kernel."""
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
            "transport": "local",
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
            "transport": "local",
        }

    return _classify_output(stdout, stderr=stderr, transport="local")


#: In-band failure markers. Both transports can report an evaluation failure while
#: signalling success at the protocol level (exit 0 locally, success:true in the
#: cloud), so the payload is inspected on BOTH paths. Same discipline the formal
#: backends apply in `SuccessSignal`: a protocol-level success is not proof.
_FAILURE_MARKERS = ("$failed", "syntax::", "::argx", "::argrx", "::nonopt")


def _classify_output(stdout: str, *, stderr: str | None, transport: str) -> dict:
    """Decide whether output that arrived "successfully" is actually a result."""
    lowered = stdout.lower()
    for marker in _FAILURE_MARKERS:
        if marker in lowered:
            return {
                "ok": False,
                "result": stdout or None,
                "error": f"wolfram reported a failure marker ({marker}) despite reporting success",
                "unavailable": False,
                "transport": transport,
            }
    return {
        "ok": True,
        "result": stdout,
        "error": stderr or None,
        "unavailable": False,
        "transport": transport,
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
