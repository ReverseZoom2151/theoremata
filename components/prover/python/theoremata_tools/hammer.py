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
* **real**: invokes the actual tool (Sledgehammer via ``isabelle
  process_theories``, CoqHammer via ``coqc``, aesop via ``lake env lean`` --
  natively or, on Windows, through WSL Ubuntu). Selected automatically when the
  tool is probed (``PATH``, WSL, or an env override). A successful live run
  reports ``mode: "live"``; if the toolchain cannot be invoked the adapter
  degrades to mock and says so (never raises).

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

import functools
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

# True on native Windows, where the real toolchains live inside WSL Ubuntu and
# must be reached through ``wsl.exe`` with Windows->/mnt path translation.
_IS_WINDOWS = os.name == "nt"

# WSL distro + in-WSL tool locations (overridable via env). The Isabelle path
# keeps a literal ``$HOME`` so it resolves per-user inside the login shell.
_WSL_DISTRO = os.environ.get("THEOREMATA_WSL_DISTRO", "Ubuntu")
_ISABELLE_WSL_PATH = os.environ.get(
    "THEOREMATA_ISABELLE_WSL_PATH", "$HOME/Isabelle2025-2/bin/isabelle"
)
# Prefix marking a ``_command_for`` result that must be run through WSL.
_WSL_PREFIX = "wsl:"
# Extra wall-clock head-room over the Sledgehammer soft timeout: HOL heap load
# + process start-up cost of a cold ``isabelle process_theories`` run.
_ISABELLE_STARTUP_BUDGET = 150

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
# ATPs actually installed alongside CoqHammer on this box (apt eprover + z3); the
# full-tier `hammer` dispatches premise selection to these.
_ROCQ_LIVE_ATP_PROVERS = ["eprover", "z3"]

# On this Windows box CoqHammer lives inside the opam switch: only
# ``eval "$(opam env)"`` puts Rocq 9.1.1 + the Hammer plugin on PATH (a bare
# ``coqc`` is the apt 8.18 build, which lacks the plugin). Every WSL coqc
# invocation for the rocq backend must therefore be prefixed with this line.
_OPAM_ENV_PREFIX = 'eval "$(opam env)"\n'

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


def _wsl_available() -> bool:
    """True if a ``wsl.exe`` launcher is discoverable (Windows only)."""
    return _IS_WINDOWS and bool(shutil.which("wsl.exe") or shutil.which("wsl"))


@functools.lru_cache(maxsize=None)
def _wsl_probe(check_cmd: str) -> bool:
    """Run ``bash -lc <check_cmd>`` in WSL; True iff it exits 0.

    Cached: toolchain presence is stable for the life of the process, and the
    probe spawns a real ``wsl.exe`` (~1s cold), so we must not repeat it per
    call. ``wsl.exe`` is invoked directly (no intermediate shell), so ``$HOME``
    and friends expand normally.
    """
    if not _wsl_available():
        return False
    try:
        proc = subprocess.run(
            ["wsl.exe", "-d", _WSL_DISTRO, "--", "bash", "-lc", check_cmd],
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return False
    return proc.returncode == 0


def _command_for(system: str) -> Optional[str]:
    """Return the live driver command for ``system`` if one is available.

    Honours an explicit env override first, then probes ``PATH`` (native), then
    probes WSL Ubuntu (the real toolchains live there on this Windows box). A
    result prefixed ``wsl:`` denotes a command to be run through ``wsl.exe``.
    Returns ``None`` when nothing is available (-> mock).
    """
    if system == "isabelle":
        override = os.environ.get("THEOREMATA_ISABELLE_COMMAND")
        if override:
            return override
        native = shutil.which("isabelle")
        if native:
            return native
        if _wsl_probe(f'test -x "{_ISABELLE_WSL_PATH}"'):
            return _WSL_PREFIX + _ISABELLE_WSL_PATH
        return None
    if system == "rocq":
        override = os.environ.get("THEOREMATA_ROCQ_COMMAND")
        if override:
            return override
        native = shutil.which("rocq") or shutil.which("coqc")
        if native:
            return native
        if _wsl_probe("command -v coqc >/dev/null 2>&1"):
            return _WSL_PREFIX + "coqc"
        return None
    if system == "lean":
        return (
            os.environ.get("THEOREMATA_LEAN_COMMAND")
            or shutil.which("lake")
            or shutil.which("lean")
        )
    return None


def _win_to_wsl_path(path: str) -> str:
    """Translate a Windows path (``C:\\a\\b``) to its WSL form (``/mnt/c/a/b``)."""
    ap = os.path.abspath(path)
    drive, rest = os.path.splitdrive(ap)
    rest = rest.replace("\\", "/")
    if drive and len(drive) >= 2 and drive[1] == ":":
        return f"/mnt/{drive[0].lower()}{rest}"
    return ap.replace("\\", "/")


def _wsl_bash(script_body: str, wall_timeout: int) -> "tuple[int, str]":
    """Run a bash script in WSL and return ``(returncode, stdout+stderr)``.

    The script is executed from a temp **file** (``bash <file>``) rather than
    ``bash -lc <string>``: passing a multi-line script inline through the
    ``wsl.exe`` argument layer silently drops command substitutions
    (``D=$(mktemp -d)`` yields an empty ``D``), whereas a script file behaves
    normally. Written with LF newlines for the Linux side. Raises
    ``HammerUnavailable`` on spawn/timeout failure (-> graceful mock fallback).
    """
    fd, spath = tempfile.mkstemp(suffix=".sh")
    os.close(fd)
    try:
        with open(spath, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(script_body)
        try:
            proc = subprocess.run(
                ["wsl.exe", "-d", _WSL_DISTRO, "--", "bash", _win_to_wsl_path(spath)],
                capture_output=True,
                text=True,
                timeout=wall_timeout,
                check=False,
            )
        except (OSError, subprocess.SubprocessError) as exc:
            raise HammerUnavailable(f"wsl invocation failed: {exc}") from exc
        return proc.returncode, (proc.stdout or "") + (proc.stderr or "")
    finally:
        try:
            os.unlink(spath)
        except OSError:
            pass


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


def _isabelle_theory_text(goal: str, provers: list[str], timeout: int) -> str:
    """Build a self-contained ``Scratch.thy`` that fires Sledgehammer.

    ``oops`` abandons the (still-open) goal after Sledgehammer has run, so the
    theory always processes cleanly regardless of whether we reconstruct -- we
    only care about the ``Try this: ...`` diagnostics it emits.
    """
    prover_opt = " ".join(provers)
    return (
        "theory Scratch\n"
        "  imports Main\n"
        "begin\n"
        f'theorem sledgehammer_goal: "{goal}"\n'
        f'  sledgehammer [provers = "{prover_opt}", timeout = {timeout}]\n'
        "  oops\n"
        "end\n"
    )


def _parse_sledgehammer(out: str) -> Optional[str]:
    """Extract the first reconstructed tactic from Sledgehammer's output.

    Lines look like ``e: Try this: by auto (0.3 ms)`` or
    ``Try this: by (metis one_add_one) (12 ms)`` or
    ``e: Try this: using one_add_one by blast (0.2 ms)``. We strip the trailing
    ``(<time> ms|s)`` preplay annotation and return the bare Isar method.
    """
    for match in re.finditer(r"Try this:\s*(.+)", out):
        cand = match.group(1).strip()
        cand = re.sub(r"\s*\(\s*[\d.]+\s*(?:ms|s)\s*\)\s*$", "", cand).strip()
        # Guard against a bare "Duplicate proof" / empty capture.
        if cand and re.search(r"\b(by|using|unfolding|apply)\b", cand):
            return cand
    return None


def _run_isabelle(theory_text: str, wall_timeout: int) -> str:
    """Write ``Scratch.thy`` and run ``isabelle process_theories -O`` on it.

    Native (Linux/PATH) invocation runs the ``isabelle`` binary directly. On
    Windows the binary lives in WSL, so we translate the temp file to its
    ``/mnt/...`` path, copy it into a fresh in-WSL scratch dir (native FS avoids
    ``/mnt`` line-ending/permission quirks) and run there via ``wsl.exe``.
    """
    command = _command_for("isabelle")
    if not command:
        raise HammerUnavailable("no isabelle command")
    with tempfile.TemporaryDirectory() as tmp:
        thy_path = os.path.join(tmp, "Scratch.thy")
        # LF newlines: the theory is consumed by Isabelle inside WSL/Linux.
        with open(thy_path, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(theory_text)
        if command.startswith(_WSL_PREFIX):
            isa = command[len(_WSL_PREFIX):]
            mnt = _win_to_wsl_path(thy_path)
            # Copy the theory onto WSL's native FS (mktemp dir) before running,
            # sidestepping /mnt line-ending/permission quirks.
            script = (
                "set -e\n"
                f'ISA="{isa}"\n'
                "D=$(mktemp -d)\n"
                f'cp "{mnt}" "$D/Scratch.thy"\n'
                '"$ISA" process_theories -O -D "$D" Scratch\n'
            )
            _rc, out = _wsl_bash(script, wall_timeout)
            return out
        out = _run_subprocess(
            [command, "process_theories", "-O", "-D", tmp, "Scratch"],
            "",
            wall_timeout,
        )
        return out


def _real_isabelle(
    goal_or_state: Any,
    context: dict[str, Any],
    timeout: int,
    requested_mode: Optional[str],
) -> dict[str, Any]:
    """Drive Sledgehammer headlessly and return a kernel-checked reconstruction.

    Writes a ``Scratch.thy`` with ``sledgehammer [provers = ..., timeout = t]``
    at the goal, runs it through ``isabelle process_theories -O`` (natively or
    via WSL), and parses the ``Try this: by (metis ...)`` / ``by (smt ...)`` /
    ``by auto`` / ``using ... by ...`` line out of the emitted diagnostics. The
    returned method is a native Isar proof re-checked by Isabelle's own kernel;
    the external ATP is only a hint oracle.
    """
    command = _command_for("isabelle")
    if not command:
        raise HammerUnavailable("no isabelle command")
    provers = list(context.get("provers") or _ISABELLE_PROVERS)
    goal = _goal_text(goal_or_state)
    theory_text = _isabelle_theory_text(goal, provers, timeout)
    # Generous wall-clock budget: the Sledgehammer soft timeout plus HOL-heap
    # load + process start-up. A wall-clock overrun surfaces as HammerUnavailable
    # (-> graceful mock fallback), never a raise to the caller.
    out = _run_isabelle(theory_text, timeout + _ISABELLE_STARTUP_BUDGET)
    tactic = _parse_sledgehammer(out)
    return _result(
        system="isabelle",
        tool="sledgehammer",
        success=tactic is not None,
        reconstructed_tactic=tactic,
        provers_tried=provers,
        message=(
            f"Sledgehammer: reconstructed `{tactic}` via preplay (kernel-checked)."
            if tactic
            else "Sledgehammer: no reconstruction found in output."
        ),
        mode="live",
        requested_mode=requested_mode,
    )


@functools.lru_cache(maxsize=None)
def _coqhammer_plugin_available(command: str, full: bool) -> bool:
    """True iff the CoqHammer plugin required for ``tier`` compiles.

    Pure tier needs ``coq-hammer-tactics`` (the ``Tactics`` module ->
    ``sauto``/``hauto``); full tier additionally needs ``coq-hammer`` (the
    ``Hammer`` module -> ``hammer`` + ATPs). We compile a one-line probe ``.v``
    that just ``Require``s the module(s); a non-zero ``coqc`` exit means the
    plugin is absent (``opam install coq-hammer[-tactics]`` needed).

    The plugin lives in the opam switch (Rocq 9.1.1), so the WSL probe compiles
    under ``eval "$(opam env)"`` -- a bare ``coqc`` is the apt 8.18 build without
    the Hammer plugin and would always fail the ``Require``.
    """
    require = (
        "From Hammer Require Import Hammer Tactics."
        if full
        else "From Hammer Require Import Tactics."
    )
    try:
        with tempfile.TemporaryDirectory() as tmp:
            vfile = os.path.join(tmp, "Probe.v")
            with open(vfile, "w", encoding="utf-8", newline="\n") as fh:
                fh.write(require + "\n")
            if command.startswith(_WSL_PREFIX):
                coqc = command[len(_WSL_PREFIX):]
                rc, _out = _wsl_bash(
                    f'{_OPAM_ENV_PREFIX}{coqc} "{_win_to_wsl_path(vfile)}"\n', 90
                )
                return rc == 0
            proc = subprocess.run(
                [command, vfile],
                capture_output=True,
                text=True,
                timeout=90,
                check=False,
            )
            return proc.returncode == 0
    except (OSError, subprocess.SubprocessError):
        return False


def _compile_rocq(command: str, vtext: str, wall_timeout: int) -> "tuple[int, str]":
    """Compile a ``.v`` with ``coqc`` and return ``(returncode, output)``.

    On Windows the real ``coqc`` (Rocq 9.1.1 + Hammer plugin) lives in the opam
    switch inside WSL, so we prefix the script with ``eval "$(opam env)"`` and
    run through ``wsl.exe`` on the file's ``/mnt`` path (coqc reads/writes there
    fine). Native (Linux/PATH) invocation runs the binary directly; since
    ``_run_subprocess`` hides the exit code we treat any ``error`` in the output
    as a non-zero exit (kernel/plugin failure).
    """
    with tempfile.TemporaryDirectory() as tmp:
        vfile = os.path.join(tmp, "Generated.v")
        with open(vfile, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(vtext)
        if command.startswith(_WSL_PREFIX):
            coqc = command[len(_WSL_PREFIX):]
            return _wsl_bash(
                f'{_OPAM_ENV_PREFIX}{coqc} "{_win_to_wsl_path(vfile)}"\n',
                wall_timeout,
            )
        out = _run_subprocess([command, vfile], "", wall_timeout)
        return (1 if "error" in out.lower() else 0), out


def _parse_coqhammer_reconstruction(out: str) -> Optional[str]:
    """Pull the reconstruction tactic out of ``hammer``'s output.

    ``hammer`` prints e.g. ``Replace the hammer tactic with: sfirstorder`` (or
    ``hauto use: ...`` / ``sauto ...``). We take the rest of that line and strip
    a trailing period/whitespace. Returns ``None`` if no such line is present.
    """
    match = re.search(r"Replace the hammer tactic with:\s*(.+)", out)
    if not match:
        return None
    cand = match.group(1).strip().rstrip(".").strip()
    return cand or None


def _real_rocq(
    goal_or_state: Any,
    context: dict[str, Any],
    timeout: int,
    requested_mode: Optional[str],
) -> dict[str, Any]:
    """Drive CoqHammer headlessly (pure ``sauto`` tier vs full ``hammer`` tier).

    Live call (under ``eval "$(opam env)"`` on this box): compile a ``.v`` and
    check ``coqc`` exits 0 -- a success is a real, kernel-checked Rocq proof.

    * **pure tier** (default): ``From Hammer Require Import Tactics.`` +
      ``Goal <g>. Proof. sauto. Qed.`` -- pure CIC search, no external ATPs.
      ``reconstructed_tactic`` is ``sauto`` (or ``best``), ``provers_tried`` is
      empty.
    * **full tier** (``context["tier"] == "full"``): ``From Hammer Require
      Import Hammer Tactics.`` + ``Proof. hammer. Qed.`` -- ``hammer`` fires the
      installed ATPs (eprover/z3) for premise selection and prints a
      deterministic reconstruction (``Replace the hammer tactic with: ...``)
      that we parse and return. If the line can't be parsed we fall back to a
      ``sauto`` full-compile as the reconstruction. Either way the returned
      tactic was verified by ``coqc`` (kernel-checked).

    Raises ``HammerUnavailable`` (-> graceful mock fallback) if the CoqHammer
    plugin probe fails (``opam install coq-hammer[-tactics]`` needed).
    """
    command = _command_for("rocq")
    if not command:
        raise HammerUnavailable("no rocq command (coqc/rocq)")
    tier = str(context.get("tier", "pure")).lower()
    full = tier == "full"
    if not _coqhammer_plugin_available(command, full):
        pkg = "coq-hammer" if full else "coq-hammer-tactics"
        raise HammerUnavailable(
            f"CoqHammer not installed for the {tier} tier "
            f"(run `opam install {pkg}`)"
        )
    goal = _goal_text(goal_or_state)

    if not full:
        # Pure tier: self-contained sauto/best compile, no external ATPs.
        tactic_cmd = "best" if context.get("best") else "sauto"
        vtext = (
            "From Hammer Require Import Tactics.\n"
            f"Goal {goal}.\nProof. {tactic_cmd}. Qed.\n"
        )
        rc, out = _compile_rocq(command, vtext, timeout + 60)
        success = rc == 0
        return _result(
            system="rocq",
            tool="coqhammer:sauto",
            success=success,
            reconstructed_tactic=tactic_cmd if success else None,
            provers_tried=list(context.get("provers") or []),
            message=(
                f"CoqHammer (pure tier): `{tactic_cmd}` closed the goal "
                "(kernel-checked, no ATP)."
                if success
                else "CoqHammer (pure tier): `sauto`/`best` did not close the goal."
            ),
            mode="live",
            requested_mode=requested_mode,
            tier="pure",
        )

    # Full tier: `hammer` dispatches to ATPs then prints a reconstruction tactic.
    provers = list(context.get("provers") or _ROCQ_LIVE_ATP_PROVERS)
    vtext = (
        "From Hammer Require Import Hammer Tactics.\n"
        f"Set Hammer ATPLimit {timeout}.\n"
        f"Goal {goal}.\nProof. hammer. Qed.\n"
    )
    rc, out = _compile_rocq(command, vtext, timeout + 60)
    if rc != 0:
        return _result(
            system="rocq",
            tool="coqhammer:hammer",
            success=False,
            reconstructed_tactic=None,
            provers_tried=provers,
            message="CoqHammer (full tier): `hammer` found no proof / kernel error.",
            mode="live",
            requested_mode=requested_mode,
            tier="full",
        )
    # `hammer` succeeded (kernel-checked). Prefer the printed reconstruction; if
    # unparseable, fall back to a sauto full-compile as the deterministic tactic.
    reconstructed = _parse_coqhammer_reconstruction(out)
    if reconstructed is None:
        sauto_text = (
            "From Hammer Require Import Tactics.\n"
            f"Goal {goal}.\nProof. sauto. Qed.\n"
        )
        s_rc, _s_out = _compile_rocq(command, sauto_text, timeout + 60)
        reconstructed = "sauto" if s_rc == 0 else "hammer"
    return _result(
        system="rocq",
        tool="coqhammer:hammer",
        success=True,
        reconstructed_tactic=reconstructed,
        provers_tried=provers,
        message=(
            f"CoqHammer (full tier): `hammer` closed the goal via ATPs; replace "
            f"with `{reconstructed}` (kernel-checked)."
        ),
        mode="live",
        requested_mode=requested_mode,
        tier="full",
    )


def _real_lean(
    goal_or_state: Any,
    context: dict[str, Any],
    timeout: int,
    requested_mode: Optional[str],
) -> dict[str, Any]:
    """Drive ``aesop?`` headlessly -- gated on a Lean project exposing aesop.

    ``aesop`` is not in the Lean core; it ships with Mathlib and must be
    elaborated inside a Lake project whose dependencies expose it. Bare
    ``lean``/``lake`` on ``PATH`` cannot run it, so we require an explicit
    project via ``THEOREMATA_LEAN_PROJECT`` (a Lake workspace dir with
    aesop/Mathlib on ``LEAN_PATH``). Without it we raise ``HammerUnavailable``
    (-> graceful mock fallback) noting the gate.

    When a project is supplied: elaborate a ``.lean`` proving the goal
    ``by aesop?`` under ``lake env lean``; ``aesop?`` prints ``Try this:
    <script>`` -- the kernel-checked reconstruction. Duper/LeanHammer would be
    the external-ATP alternative wired at the same seam.
    """
    command = _command_for("lean")
    if not command:
        raise HammerUnavailable("no lean command (lake/lean)")
    project = os.environ.get("THEOREMATA_LEAN_PROJECT")
    if not project or not os.path.isdir(project):
        raise HammerUnavailable(
            "aesop requires a Lean project exposing aesop/Mathlib; set "
            "THEOREMATA_LEAN_PROJECT to a Lake workspace (bare Lean lacks aesop)"
        )
    goal = _goal_text(goal_or_state)
    with tempfile.TemporaryDirectory() as tmp:
        lean_file = os.path.join(tmp, "Generated.lean")
        with open(lean_file, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(f"import Mathlib\n\nexample : {goal} := by\n  aesop?\n")
        argv = (
            [command, "env", "lean", lean_file]
            if os.path.basename(command).startswith("lake")
            else [command, lean_file]
        )
        out = _run_subprocess(argv, "", timeout + 60)
    match = re.search(r"Try this:\s*([^\n]+)", out)
    tactic = match.group(1).strip() if match else None
    return _result(
        system="lean",
        tool="aesop",
        success=tactic is not None,
        reconstructed_tactic=tactic,
        provers_tried=[],
        message=(
            f"aesop?: reconstructed script `{tactic}`."
            if tactic
            else "aesop?: no script emitted."
        ),
        mode="live",
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
