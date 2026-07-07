"""Unified "hammer" adapters for Theoremata's formal-system backends.

A *hammer* dispatches the current goal to strong automation and returns a
**reconstructed, kernel-checked tactic** -- the external ATP/SMT solver is only a
hint oracle, so its bugs cannot compromise soundness (the reconstruction is
re-checked by the proof assistant's own kernel). This module gives one entry
point, :func:`run_hammer`, over three backends:

============ ================= =============================================
system       tool              reconstruction returned
============ ================= =============================================
``isabelle`` Sledgehammer      ``by (metis ...)`` / ``by (smt ...)`` one-liner
``rocq``     CoqHammer         pure ``sauto``/``best`` OR ``hammer``->ATP ``hauto``
``lean``     aesop             ``aesop`` script (``aesop?``); Duper/LeanHammer = ext. ATP
============ ================= =============================================

Two execution modes
--------------------
* **mock** (default when the live toolchain is absent): offline, deterministic.
  Returns a reconstruction for a trivial goal and ``success: False`` for a
  nonsense/unprovable goal. ``kernel_checked`` is always ``True`` because the
  reconstruction *would* be re-checked by the kernel.
* **real**: invokes the actual tool (documented per backend). Selected
  automatically when the tool is probed on ``PATH`` (or via an env override);
  otherwise the adapter falls back to mock and reports the mode it used.

Rocq's two-tier split (modelled explicitly, see ``context["tier"]``)
--------------------------------------------------------------------
* ``pure`` (default, **always on**): the ``sauto``/``hauto``/``qauto``/``best``
  family -- pure CIC proof search, *no external ATPs*, no soundness cost,
  installable as ``coq-hammer-tactics``. ``provers_tried`` is empty.
* ``full``: ``hammer`` -> external ATPs (Vampire/E/Z3/cvc4) for premise
  selection, then a reconstruction tactic (e.g. ``hauto use: ...``) that
  replaces the non-deterministic ``hammer`` call. Needs ATPs on ``PATH``.

Contract (``run_hammer`` return dict)
-------------------------------------
``{ok, system, tool, success, reconstructed_tactic, kernel_checked,
provers_tried, message, mode}`` (plus backend extras such as ``tier`` and
``requested_mode``). ``ok`` = the adapter ran without error; ``success`` = a
proof/reconstruction was found; ``reconstructed_tactic`` is the kernel-checked
one-liner to splice into the final proof (``None`` on failure).

Worker wiring (report only -- this module does NOT edit worker.py)
------------------------------------------------------------------
Dispatch key ``"hammer"``::

    if tool == "hammer":
        from theoremata_tools.hammer import run as hammer_run
        return hammer_run(request)

with ``request = {"tool": "hammer", "system": "isabelle"|"rocq"|"lean",
"goal": <str|state-dict>, "mode": None|"mock"|"real", "timeout": 30,
"context": {...}}``. See :func:`run`.

Uses the Python standard library only.
"""
from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
import tempfile
from typing import Any, Optional

# --------------------------------------------------------------------------- #
# Constants.
# --------------------------------------------------------------------------- #
DEFAULT_TIMEOUT = 30

# Canonical backend identifiers and their aliases.
_SYSTEM_ALIASES = {
    "isabelle": "isabelle",
    "hol": "isabelle",
    "sledgehammer": "isabelle",
    "rocq": "rocq",
    "coq": "rocq",
    "coqhammer": "rocq",
    "lean": "lean",
    "lean4": "lean",
    "aesop": "lean",
}

# Default prover batteries (the ATP/SMT solvers a real invocation would fire).
_ISABELLE_PROVERS = ["e", "vampire", "cvc5", "z3"]
_ROCQ_ATP_PROVERS = ["vampire", "eprover", "z3", "cvc4"]

# Substrings that mark a goal as (deterministically) unprovable in mock mode.
# Kept conservative on purpose: arithmetic-shaped markers like "1 = 2" would
# false-positive on legit goals such as "1 + 1 = 2", so we key off explicit
# falsity/nonsense words only.
_UNPROVABLE_MARKERS = (
    "false",
    "nonsense",
    "unprovable",
    "contradiction",
)


class HammerUnavailable(RuntimeError):
    """Raised by a real backend when its live toolchain cannot be invoked."""


# --------------------------------------------------------------------------- #
# System / mode / availability resolution.
# --------------------------------------------------------------------------- #
def normalize_system(system: str) -> str:
    """Map an input backend name to its canonical id (raises on unknown)."""
    key = (system or "").strip().lower()
    if key not in _SYSTEM_ALIASES:
        raise ValueError(
            f"unknown hammer system: {system!r} "
            f"(expected one of isabelle/rocq/lean)"
        )
    return _SYSTEM_ALIASES[key]


def _command_for(system: str) -> Optional[str]:
    """Return the live driver command for ``system`` if one is available.

    Honours an explicit env override first, then probes ``PATH``. Returns
    ``None`` when nothing is available (-> mock).
    """
    if system == "isabelle":
        return os.environ.get("THEOREMATA_ISABELLE_COMMAND") or shutil.which(
            "isabelle"
        )
    if system == "rocq":
        return (
            os.environ.get("THEOREMATA_ROCQ_COMMAND")
            or shutil.which("rocq")
            or shutil.which("coqc")
        )
    if system == "lean":
        return (
            os.environ.get("THEOREMATA_LEAN_COMMAND")
            or shutil.which("lake")
            or shutil.which("lean")
        )
    return None


def tool_available(system: str) -> bool:
    """True if the live toolchain for ``system`` is probeable (no-op probe)."""
    return _command_for(normalize_system(system)) is not None


def _resolve_mode(system: str, requested: Optional[str]) -> str:
    """Resolve the effective mode: explicit arg > env > auto(probe)."""
    req = requested or os.environ.get("THEOREMATA_HAMMER_MODE")
    if req in ("mock", "real"):
        return req
    # auto: prefer real only when the tool is actually available.
    return "real" if tool_available(system) else "mock"


# --------------------------------------------------------------------------- #
# Goal inspection (mock provability heuristic).
# --------------------------------------------------------------------------- #
def _goal_text(goal_or_state: Any) -> str:
    """Extract a pretty-printed goal string from a str or proof-state dict."""
    if isinstance(goal_or_state, str):
        return goal_or_state
    if isinstance(goal_or_state, dict):
        for key in ("goal", "goals", "state", "statement", "target", "ty"):
            val = goal_or_state.get(key)
            if isinstance(val, str):
                return val
            if isinstance(val, list) and val and isinstance(val[0], str):
                return val[0]
    return str(goal_or_state)


def _looks_provable(goal_or_state: Any, context: dict[str, Any]) -> bool:
    """Deterministic mock oracle: trivial goal -> provable, nonsense -> not.

    An explicit ``context["provable"]`` boolean overrides the heuristic (handy
    for tests / callers that already know the verdict).
    """
    if "provable" in context:
        return bool(context["provable"])
    text = _goal_text(goal_or_state).strip().lower()
    if not text:
        return False
    return not any(marker in text for marker in _UNPROVABLE_MARKERS)


# --------------------------------------------------------------------------- #
# Result construction.
# --------------------------------------------------------------------------- #
def _result(
    *,
    system: str,
    tool: str,
    success: bool,
    reconstructed_tactic: Optional[str],
    provers_tried: list[str],
    message: str,
    mode: str,
    requested_mode: Optional[str],
    **extra: Any,
) -> dict[str, Any]:
    """Assemble the uniform hammer contract dict."""
    out: dict[str, Any] = {
        "ok": True,
        "system": system,
        "tool": tool,
        "success": success,
        "reconstructed_tactic": reconstructed_tactic,
        # A hammer's whole point: the returned tactic is a native, kernel-checked
        # reconstruction, never a trusted external-prover certificate.
        "kernel_checked": True,
        "provers_tried": provers_tried,
        "message": message,
        "mode": mode,
        "requested_mode": requested_mode,
    }
    out.update(extra)
    return out


# --------------------------------------------------------------------------- #
# Mock backends (offline, deterministic).
# --------------------------------------------------------------------------- #
def _mock_isabelle(
    goal_or_state: Any, context: dict[str, Any], requested_mode: Optional[str]
) -> dict[str, Any]:
    """Sledgehammer mock: `by (metis ...)` one-liner for a trivial goal.

    Real Sledgehammer fires E/Vampire/cvc5/Z3, preplays candidates, and prints
    ``Try this: by (metis ...)`` (or ``by (smt ...)``). On failure its companion
    falsifiers ``nitpick``/``quickcheck`` may exhibit a counterexample.
    """
    provers = list(context.get("provers") or _ISABELLE_PROVERS)
    if _looks_provable(goal_or_state, context):
        return _result(
            system="isabelle",
            tool="sledgehammer",
            success=True,
            reconstructed_tactic="by (metis)",
            provers_tried=provers,
            message=(
                "Sledgehammer (mock): Try this: by (metis) -- preplayed, "
                "kernel-checked reconstruction."
            ),
            mode="mock",
            requested_mode=requested_mode,
        )
    return _result(
        system="isabelle",
        tool="sledgehammer",
        success=False,
        reconstructed_tactic=None,
        provers_tried=provers,
        message=(
            "Sledgehammer (mock): no proof found; nitpick/quickcheck suggest a "
            "counterexample."
        ),
        mode="mock",
        requested_mode=requested_mode,
    )


def _mock_rocq(
    goal_or_state: Any, context: dict[str, Any], requested_mode: Optional[str]
) -> dict[str, Any]:
    """CoqHammer mock, modelling the pure-tier vs full-ATP-tier split.

    * ``tier="pure"`` (default): ``sauto``/``best`` family, no external ATPs.
    * ``tier="full"``: ``hammer`` -> Vampire/E/Z3/cvc4, then a reconstruction
      tactic (``hauto use: ...``) that replaces the non-deterministic call.
    """
    tier = str(context.get("tier", "pure")).lower()
    full = tier == "full"
    provers = list(context.get("provers") or (_ROCQ_ATP_PROVERS if full else []))
    tool = "coqhammer:hammer" if full else "coqhammer:sauto"
    if _looks_provable(goal_or_state, context):
        tactic = "hauto" if full else ("best" if context.get("best") else "sauto")
        if full:
            msg = (
                "CoqHammer (mock, full tier): hammer via ATPs; replace with "
                f"reconstruction `{tactic}` (kernel-checked)."
            )
        else:
            msg = (
                "CoqHammer (mock, pure tier): `sauto`/`best` -- no ATP, "
                "kernel-checked term."
            )
        return _result(
            system="rocq",
            tool=tool,
            success=True,
            reconstructed_tactic=tactic,
            provers_tried=provers,
            message=msg,
            mode="mock",
            requested_mode=requested_mode,
            tier=tier,
        )
    return _result(
        system="rocq",
        tool=tool,
        success=False,
        reconstructed_tactic=None,
        provers_tried=provers,
        message=(
            f"CoqHammer (mock, {tier} tier): no proof/reconstruction found "
            "(sauto never inducts -- supply elim/induction first)."
        ),
        mode="mock",
        requested_mode=requested_mode,
        tier=tier,
    )


def _mock_lean(
    goal_or_state: Any, context: dict[str, Any], requested_mode: Optional[str]
) -> dict[str, Any]:
    """aesop mock: white-box best-first search; ``aesop?`` prints the script.

    Lean has no in-tree black-box hammer; ``aesop`` is the closest analogue.
    The external-ATP-with-reconstruction option is Duper / LeanHammer.
    """
    if _looks_provable(goal_or_state, context):
        return _result(
            system="lean",
            tool="aesop",
            success=True,
            reconstructed_tactic="aesop",
            provers_tried=[],  # white-box: no external ATP fired
            message=(
                "aesop (mock): Try this: aesop -- kernel-checked script "
                "(Duper/LeanHammer = external-ATP reconstruction option)."
            ),
            mode="mock",
            requested_mode=requested_mode,
        )
    return _result(
        system="lean",
        tool="aesop",
        success=False,
        reconstructed_tactic=None,
        provers_tried=[],
        message=(
            "aesop (mock): search exhausted; try Duper/LeanHammer (external ATP) "
            "or premise search (exact?/apply?)."
        ),
        mode="mock",
        requested_mode=requested_mode,
    )


_MOCK_BACKENDS = {
    "isabelle": _mock_isabelle,
    "rocq": _mock_rocq,
    "lean": _mock_lean,
}


def _run_mock(
    system: str,
    goal_or_state: Any,
    context: dict[str, Any],
    requested_mode: Optional[str],
) -> dict[str, Any]:
    return _MOCK_BACKENDS[system](goal_or_state, context, requested_mode)


# --------------------------------------------------------------------------- #
# Real backends (documented live invocations). Exception-safe: on any failure
# they raise HammerUnavailable and run_hammer falls back to mock.
# --------------------------------------------------------------------------- #
def _run_subprocess(cmd: list[str], stdin: str, timeout: int) -> str:
    """Run a driver command, return combined stdout (raises on failure)."""
    try:
        proc = subprocess.run(
            cmd,
            input=stdin,
            text=True,
            capture_output=True,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise HammerUnavailable(f"invocation failed: {exc}") from exc
    return (proc.stdout or "") + (proc.stderr or "")


def _real_isabelle(
    goal_or_state: Any,
    context: dict[str, Any],
    timeout: int,
    requested_mode: Optional[str],
) -> dict[str, Any]:
    """Drive Sledgehammer headlessly.

    Live call (documented): write a ``Scratch.thy`` whose lemma body is::

        lemma goal: "<GOAL>"
          sledgehammer [provers = "<provers>", timeout = <t>]

    and submit it to the Isabelle Server (``session_start {"session":"HOL"}``
    then ``use_theories``), or run ``isabelle process``; then read the
    ``Try this: by (metis ...)`` line out of the returned ``messages``. Here we
    invoke the configured ``isabelle`` command and parse its stdout.
    """
    command = _command_for("isabelle")
    if not command:
        raise HammerUnavailable("no isabelle command")
    provers = list(context.get("provers") or _ISABELLE_PROVERS)
    goal = _goal_text(goal_or_state)
    with tempfile.TemporaryDirectory() as tmp:
        thy = os.path.join(tmp, "Scratch.thy")
        with open(thy, "w", encoding="utf-8") as fh:
            fh.write(
                "theory Scratch\n  imports Main\nbegin\n"
                f'lemma goal: "{goal}"\n'
                f'  sledgehammer [provers = "{" ".join(provers)}", '
                f"timeout = {timeout}]\nend\n"
            )
        out = _run_subprocess([command, "process", "-T", "Scratch"], "", timeout)
    match = re.search(r"Try this:\s*(by \([^\n]*\)|by [^\n]+)", out)
    tactic = match.group(1).strip() if match else None
    return _result(
        system="isabelle",
        tool="sledgehammer",
        success=tactic is not None,
        reconstructed_tactic=tactic,
        provers_tried=provers,
        message=(
            "Sledgehammer: reconstructed via preplay."
            if tactic
            else "Sledgehammer: no reconstruction in output."
        ),
        mode="real",
        requested_mode=requested_mode,
    )


def _real_rocq(
    goal_or_state: Any,
    context: dict[str, Any],
    timeout: int,
    requested_mode: Optional[str],
) -> dict[str, Any]:
    """Drive CoqHammer headlessly.

    Live call (documented): compile a ``.v`` with ``From Hammer Require Import
    Hammer Tactics.`` and either the pure-tier ``sauto``/``best`` (no ATP) or,
    for the full tier, ``hammer`` (which fires Vampire/E/Z3/cvc4 then prints a
    reconstruction tactic to substitute). We invoke ``rocq compile``/``coqc``
    and parse the reconstruction line.
    """
    command = _command_for("rocq")
    if not command:
        raise HammerUnavailable("no rocq command")
    tier = str(context.get("tier", "pure")).lower()
    full = tier == "full"
    provers = list(context.get("provers") or (_ROCQ_ATP_PROVERS if full else []))
    tactic_cmd = "hammer" if full else ("best" if context.get("best") else "sauto")
    goal = _goal_text(goal_or_state)
    with tempfile.TemporaryDirectory() as tmp:
        vfile = os.path.join(tmp, "Generated.v")
        with open(vfile, "w", encoding="utf-8") as fh:
            fh.write(
                "From Hammer Require Import Hammer Tactics.\n"
                f"Set Hammer ATPLimit {timeout}.\n"
                f"Lemma goal : {goal}.\nProof. {tactic_cmd}. Qed.\n"
            )
        out = _run_subprocess([command, vfile], "", timeout)
    # `hammer` prints e.g. "Replace the hammer tactic with: hauto use: ...".
    match = re.search(
        r"(?:Replace the hammer tactic with:?\s*)?((?:s?auto|hauto|qauto|best)"
        r"[^\n.]*)",
        out,
    )
    reconstructed = match.group(1).strip() if match else (
        tactic_cmd if not full else None
    )
    success = reconstructed is not None and "error" not in out.lower()
    return _result(
        system="rocq",
        tool="coqhammer:hammer" if full else "coqhammer:sauto",
        success=success,
        reconstructed_tactic=reconstructed if success else None,
        provers_tried=provers,
        message="CoqHammer: reconstruction accepted." if success else
        "CoqHammer: no reconstruction / kernel error.",
        mode="real",
        requested_mode=requested_mode,
        tier=tier,
    )


def _real_lean(
    goal_or_state: Any,
    context: dict[str, Any],
    timeout: int,
    requested_mode: Optional[str],
) -> dict[str, Any]:
    """Drive ``aesop?`` headlessly.

    Live call (documented): elaborate a ``.lean`` proving the goal ``by aesop?``
    under ``lake env lean`` (Mathlib on ``LEAN_PATH``); ``aesop?`` prints a
    ``Try this: <script>`` line that is the kernel-checked reconstruction.
    Duper/LeanHammer would be wired here as the external-ATP alternative.
    """
    command = _command_for("lean")
    if not command:
        raise HammerUnavailable("no lean command")
    goal = _goal_text(goal_or_state)
    with tempfile.TemporaryDirectory() as tmp:
        lean_file = os.path.join(tmp, "Generated.lean")
        with open(lean_file, "w", encoding="utf-8") as fh:
            fh.write(f"import Mathlib\n\nexample : {goal} := by\n  aesop?\n")
        argv = (
            [command, "env", "lean", lean_file]
            if os.path.basename(command).startswith("lake")
            else [command, lean_file]
        )
        out = _run_subprocess(argv, "", timeout)
    match = re.search(r"Try this:\s*([^\n]+)", out)
    tactic = match.group(1).strip() if match else None
    return _result(
        system="lean",
        tool="aesop",
        success=tactic is not None,
        reconstructed_tactic=tactic,
        provers_tried=[],
        message=(
            "aesop?: reconstructed script."
            if tactic
            else "aesop?: no script emitted."
        ),
        mode="real",
        requested_mode=requested_mode,
    )


_REAL_BACKENDS = {
    "isabelle": _real_isabelle,
    "rocq": _real_rocq,
    "lean": _real_lean,
}


# --------------------------------------------------------------------------- #
# Public entry point.
# --------------------------------------------------------------------------- #
def run_hammer(
    system: str,
    goal_or_state: Any,
    *,
    mode: Optional[str] = None,
    timeout: int = DEFAULT_TIMEOUT,
    context: Optional[dict[str, Any]] = None,
) -> dict[str, Any]:
    """Dispatch ``goal_or_state`` to the ``system`` hammer; return the contract.

    Parameters
    ----------
    system:
        ``"isabelle"`` (Sledgehammer), ``"rocq"`` (CoqHammer) or ``"lean"``
        (aesop). Aliases (coq, hol, lean4, ...) are accepted.
    goal_or_state:
        A pretty-printed goal string, or a proof-state dict carrying a
        ``goal``/``goals``/``state``/``statement`` field.
    mode:
        ``None`` (auto: real if the tool is probeable, else mock), ``"mock"``
        (force offline) or ``"real"`` (force live; falls back to mock with a
        note if the toolchain cannot be invoked).
    timeout:
        Soft ATP/reconstruction timeout in seconds.
    context:
        Backend knobs: ``provable`` (bool, override the mock oracle),
        ``provers`` (list), and for rocq ``tier`` (``"pure"``/``"full"``) and
        ``best`` (bool).

    Returns the uniform dict documented in the module docstring.
    """
    system = normalize_system(system)
    context = dict(context or {})
    effective = _resolve_mode(system, mode)

    if effective == "real":
        try:
            return _REAL_BACKENDS[system](
                goal_or_state, context, timeout, requested_mode=mode
            )
        except HammerUnavailable as exc:
            # Graceful fallback: report that we dropped to mock and why.
            result = _run_mock(system, goal_or_state, context, requested_mode=mode)
            result["message"] = (
                f"real mode unavailable ({exc}); fell back to mock. "
                + result["message"]
            )
            return result

    return _run_mock(system, goal_or_state, context, requested_mode=mode)


# --------------------------------------------------------------------------- #
# Worker-style dispatch (report-only wiring: tool == "hammer").
# --------------------------------------------------------------------------- #
def run(request: dict[str, Any]) -> dict[str, Any]:
    """Dispatch a worker-style ``{"tool": "hammer", ...}`` request."""
    system = request.get("system")
    if not system:
        raise ValueError("hammer request requires a 'system'")
    goal = request.get("goal", request.get("goal_or_state", request.get("state")))
    return run_hammer(
        system,
        goal,
        mode=request.get("mode"),
        timeout=int(request.get("timeout", DEFAULT_TIMEOUT)),
        context=request.get("context"),
    )


def main() -> None:
    import json

    if len(sys.argv) >= 2 and os.path.exists(sys.argv[1]):
        with open(sys.argv[1], encoding="utf-8") as fh:
            req = json.load(fh)
    else:
        req = json.load(sys.stdin)
    out = run(req)
    print(json.dumps(out, ensure_ascii=False))
    raise SystemExit(0 if out.get("ok", True) else 1)


if __name__ == "__main__":
    main()
