"""Isabelle premise retrieval for Theoremata (parity with Lean retrieval).

Where the Lean layer dumps a whole declaration index up front and ranks it
in-memory (see ``retrieval.py`` / ``decl_index.py``), Isabelle already ships a
first-class, goal-aware premise search -- the ``find_theorems`` command -- so we
mirror *that* on demand rather than materialising a giant index. A query is run
headlessly against a warm HOL session and the printed ``name: statement``
matches are parsed into the **same result contract** the Lean retriever emits::

    {ok, backend, results: [{name, module, kind, score}]}

so Isabelle hits can feed the identical reranker / cascade. ``module`` is the
Isabelle theory (first dotted segment of the fully-qualified fact name),
``kind`` is ``"theorem"`` (``find_theorems`` returns facts/lemmas), and
``score`` is a lexical rank score (``find_theorems``' own relevance order,
refined by query/name token overlap).

Two execution modes (mirroring ``hammer.py`` / ``isabelle_driver.py``)
----------------------------------------------------------------------
* **live** -- an ``isabelle`` binary is probeable (native ``PATH`` or, on this
  Windows box, inside WSL Ubuntu via ``wsl.exe`` + ``$HOME``-``.profile`` login
  shell). A temp theory that invokes ``find_theorems (limit) <criteria>`` is run
  through ``isabelle process_theories -O`` and its output parsed.
* **mock** -- offline, deterministic. When no toolchain is present (or
  ``THEOREMATA_ISABELLE_RETRIEVAL_MOCK=1``) we synthesise contract-shaped
  results from the query tokens so the pipeline (and tests) work with no
  toolchain. ``backend`` / ``mode`` always report which path ran.

Standard library only -- no third-party deps.

Worker wiring (report only -- this module does NOT edit worker.py)
------------------------------------------------------------------
Dispatch key ``"isabelle_retrieve"``::

    if tool == "isabelle_retrieve":
        from theoremata_tools.isabelle_retrieval import run as isabelle_retrieve_run
        return isabelle_retrieve_run(request)   # {"op": "search", "query": ...}
"""
from __future__ import annotations

import functools
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, Optional

# --------------------------------------------------------------------------- #
# Configuration / environment.
# --------------------------------------------------------------------------- #
_IS_WINDOWS = os.name == "nt"
_WSL_DISTRO = os.environ.get("THEOREMATA_WSL_DISTRO", "Ubuntu")
_ISABELLE_WSL_PATH = os.environ.get("THEOREMATA_ISABELLE_WSL_PATH", "isabelle")
_WSL_PREFIX = "wsl:"

# Force mock (1) or force a live attempt (0); unset -> auto-probe.
_MOCK_ENV = "THEOREMATA_ISABELLE_RETRIEVAL_MOCK"
# Explicit command override (native path or ``wsl:isabelle``).
_COMMAND_ENV = "THEOREMATA_ISABELLE_COMMAND"

# Generous wall-clock budget: a warm HOL heap load + ``process_theories``
# start-up cost dwarfs the actual ``find_theorems`` evaluation.
DEFAULT_TIMEOUT = 180

DEFAULT_LIMIT = 20
DEFAULT_SESSION = "HOL"


class IsabelleUnavailable(RuntimeError):
    """Raised when the live Isabelle toolchain cannot be invoked (-> mock)."""


# --------------------------------------------------------------------------- #
# Toolchain discovery + WSL plumbing (mirrors hammer.py, inlined / std-lib).
# --------------------------------------------------------------------------- #
def _wsl_available() -> bool:
    return _IS_WINDOWS and bool(shutil.which("wsl.exe") or shutil.which("wsl"))


@functools.lru_cache(maxsize=None)
def _wsl_probe(check_cmd: str) -> bool:
    """``bash -lc <check_cmd>`` in WSL; True iff exit 0 (cached: ~1s cold)."""
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


def isabelle_command() -> Optional[str]:
    """Live ``isabelle`` driver command, or ``None`` (-> mock).

    Order: explicit env override, native ``PATH``, then WSL Ubuntu (where the
    real Isabelle2025-2 lives on this box). A ``wsl:`` prefix marks a command to
    be run through ``wsl.exe`` with a login shell (so ``~/.profile`` PATH wins).
    """
    override = os.environ.get(_COMMAND_ENV)
    if override:
        return override
    native = shutil.which("isabelle")
    if native:
        return native
    # `command -v` uses the login shell's PATH (Isabelle added via ~/.profile).
    if _wsl_probe(f'command -v "{_ISABELLE_WSL_PATH}" >/dev/null 2>&1'):
        return _WSL_PREFIX + _ISABELLE_WSL_PATH
    return None


def tool_available() -> bool:
    return isabelle_command() is not None


def _resolve_mode(requested: Optional[str]) -> str:
    """Effective mode: explicit arg > env > auto(probe)."""
    req = requested or os.environ.get(_MOCK_ENV)
    if req in ("mock", "1", "true", "True"):
        return "mock"
    if req in ("live", "real", "0", "false", "False"):
        return "live"
    return "live" if tool_available() else "mock"


def _win_to_wsl_path(path: str) -> str:
    """``C:\\a\\b`` -> ``/mnt/c/a/b``."""
    ap = os.path.abspath(path)
    drive, rest = os.path.splitdrive(ap)
    rest = rest.replace("\\", "/")
    if drive and len(drive) >= 2 and drive[1] == ":":
        return f"/mnt/{drive[0].lower()}{rest}"
    return ap.replace("\\", "/")


def _wsl_bash(script_body: str, wall_timeout: int) -> "tuple[int, str]":
    """Run a bash script *file* in WSL; return ``(rc, stdout+stderr)``.

    A temp file is used rather than ``bash -lc <string>`` because passing a
    multi-line script inline through ``wsl.exe`` silently drops command
    substitutions (``D=$(mktemp -d)`` yields empty) -- see hammer.py's note.
    """
    fd, spath = tempfile.mkstemp(suffix=".sh")
    os.close(fd)
    try:
        with open(spath, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(script_body)
        try:
            proc = subprocess.run(
                ["wsl.exe", "-d", _WSL_DISTRO, "--", "bash", "-l",
                 _win_to_wsl_path(spath)],
                capture_output=True,
                text=True,
                timeout=wall_timeout,
                check=False,
            )
        except (OSError, subprocess.SubprocessError) as exc:
            raise IsabelleUnavailable(f"wsl invocation failed: {exc}") from exc
        return proc.returncode, (proc.stdout or "") + (proc.stderr or "")
    finally:
        try:
            os.unlink(spath)
        except OSError:
            pass


# --------------------------------------------------------------------------- #
# Query construction.
# --------------------------------------------------------------------------- #
# ``find_theorems`` modes that take a quoted argument (``mode: "pat"``). Bare
# term patterns (mode=None) are just the quoted pattern. Goal-relative modes
# (intro/elim/dest/solves) are only meaningful inside a proof and are passed
# through verbatim if requested.
_ARG_MODES = {"name", "simp"}


def _sanitize(query: str) -> str:
    """Strip characters that would break the inner-syntax string literal."""
    # Drop embedded double quotes (they would terminate the "..." literal) and
    # collapse newlines -- a find_theorems criterion is a single token/pattern.
    return query.replace('"', " ").replace("\n", " ").replace("\r", " ").strip()


def _criteria(query: str, mode: Optional[str]) -> str:
    q = _sanitize(query)
    if mode and mode in _ARG_MODES:
        return f'{mode}: "{q}"'
    if mode and mode not in _ARG_MODES:
        # Goal-relative / bare criterion (e.g. intro/elim/dest/solves): the
        # mode word carries the meaning; append a quoted pattern only if given.
        return f'{mode}' if not q else f'{mode} "{q}"'
    return f'"{q}"'


# The query ``.thy`` is a committed template (parity with the Lean retriever,
# whose ``dump_decls.lean`` lives in ``retrieval/lean/``).
# ``find_theorems_template.thy`` sits in ``retrieval/isabelle/`` -- ``parents[2]``
# is ``components/retrieval`` -- and carries a leading ``(* ... *)`` header we
# strip so the generated theory is byte-identical to the old inline string (see
# the template file for the placeholder contract).
_TEMPLATE_PATH = (
    Path(__file__).resolve().parents[2] / "isabelle" / "find_theorems_template.thy"
)


def _strip_header(text: str) -> str:
    """Drop a leading ``(* ... *)`` documentation header, if present."""
    if text.startswith("(*"):
        end = text.find("*)")
        if end != -1:
            text = text[end + 2:].lstrip("\n")
    return text


@functools.lru_cache(maxsize=1)
def _theory_template() -> str:
    """Load the committed ``find_theorems_template.thy`` scaffold (header stripped)."""
    return _strip_header(_TEMPLATE_PATH.read_text(encoding="utf-8"))


def _theory_text(query: str, limit: int, mode: Optional[str]) -> str:
    """A self-contained ``Scratch.thy`` running one ``find_theorems`` query.

    Rendered from the committed ``find_theorems_template.thy`` by substituting
    ``__LIMIT__`` / ``__CRITERIA__`` -- byte-identical to the former inline
    scaffold.
    """
    n = max(1, int(limit))
    return (
        _theory_template()
        .replace("__LIMIT__", str(n))
        .replace("__CRITERIA__", _criteria(query, mode))
    )


# --------------------------------------------------------------------------- #
# Live invocation.
# --------------------------------------------------------------------------- #
def _run_find_theorems(theory_text: str, wall_timeout: int) -> str:
    """Run the generated theory through ``isabelle process_theories -O``.

    Native (Linux/PATH) invocation runs the binary directly. On Windows the
    binary is in WSL, so we copy the theory onto WSL's native FS (a ``mktemp``
    dir avoids ``/mnt`` line-ending/permission quirks) and run there.
    """
    command = isabelle_command()
    if not command:
        raise IsabelleUnavailable("no isabelle command")
    if command.startswith(_WSL_PREFIX):
        isa = command[len(_WSL_PREFIX):]
        with tempfile.TemporaryDirectory() as tmp:
            thy_path = os.path.join(tmp, "Scratch.thy")
            with open(thy_path, "w", encoding="utf-8", newline="\n") as fh:
                fh.write(theory_text)
            mnt = _win_to_wsl_path(thy_path)
            script = (
                "set -e\n"
                f'ISA="{isa}"\n'
                "D=$(mktemp -d)\n"
                f'cp "{mnt}" "$D/Scratch.thy"\n'
                '"$ISA" process_theories -O -D "$D" Scratch 2>&1\n'
            )
            _rc, out = _wsl_bash(script, wall_timeout)
            return out
    # Native invocation.
    with tempfile.TemporaryDirectory() as tmp:
        thy_path = os.path.join(tmp, "Scratch.thy")
        with open(thy_path, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(theory_text)
        try:
            proc = subprocess.run(
                [command, "process_theories", "-O", "-D", tmp, "Scratch"],
                capture_output=True,
                text=True,
                timeout=wall_timeout,
                check=False,
            )
        except (OSError, subprocess.SubprocessError) as exc:
            raise IsabelleUnavailable(f"invocation failed: {exc}") from exc
        return (proc.stdout or "") + (proc.stderr or "")


# --------------------------------------------------------------------------- #
# Output parsing.
# --------------------------------------------------------------------------- #
# A match name line: exactly two leading spaces, then a whitespace/colon-free
# fact name (may carry a ``(<idx>)`` disambiguator), a colon, then the (possibly
# empty, wrapped-to-next-line) statement.
_MATCH_RE = re.compile(r"^  (?P<name>[^\s:]+):(?P<rest>.*)$")
_FOUND_RE = re.compile(r"found\s+(\d+)\s+theorem")
_FOUND_NOTHING_RE = re.compile(r"found nothing")


def _parse_matches(out: str) -> "list[tuple[str, str]]":
    """Parse ``find_theorems`` output into ``[(name, statement), ...]``.

    Handles the wrapped-statement layout (a long name line ends at the colon and
    the statement continues on the next 4-space-indented line(s)).
    """
    lines = out.splitlines()
    start = None
    for i, line in enumerate(lines):
        if _FOUND_NOTHING_RE.search(line):
            return []
        if _FOUND_RE.search(line):
            start = i + 1
            break
    if start is None:
        return []

    matches: "list[list[str]]" = []
    for line in lines[start:]:
        if not line.strip():
            break  # blank line terminates the match block
        m = _MATCH_RE.match(line)
        if m:
            matches.append([m.group("name"), m.group("rest").strip()])
        elif line.startswith("    ") and matches:
            # continuation of the previous statement (wrapped line)
            cont = line.strip()
            matches[-1][1] = (matches[-1][1] + " " + cont).strip()
        else:
            break  # an unindented line (e.g. "Finished Draft ...")
    return [(n, s) for n, s in matches]


# --------------------------------------------------------------------------- #
# Scoring + contract assembly.
# --------------------------------------------------------------------------- #
_TOKEN_RE = re.compile(r"[A-Za-z0-9]+")


def _tokenize(text: str) -> "list[str]":
    return [t.lower() for t in _TOKEN_RE.findall(text or "")]


def _module_of(name: str) -> str:
    """Isabelle theory = first dotted segment of the qualified fact name."""
    return name.split(".", 1)[0] if name else ""


def _to_results(
    matches: "list[tuple[str, str]]", query: str, limit: int
) -> "list[dict[str, Any]]":
    """Assemble the ``{name, module, kind, score}`` contract records.

    ``find_theorems`` already returns matches in relevance order, so that rank
    is the dominant score term; query/name token overlap refines ties. Scores
    are strictly descending, so downstream consumers can treat the list as
    ranked without re-sorting.
    """
    q_tokens = set(_tokenize(query))
    n = len(matches)
    results: "list[dict[str, Any]]" = []
    for i, (name, statement) in enumerate(matches[: max(0, limit)]):
        name_tokens = set(_tokenize(name))
        stmt_tokens = set(_tokenize(statement))
        overlap = len(q_tokens & name_tokens) + 0.5 * len(q_tokens & stmt_tokens)
        # Rank component dominates (spread over n); overlap only breaks ties.
        score = float(n - i) + 0.01 * overlap
        results.append(
            {
                "name": name,
                "module": _module_of(name),
                "kind": "theorem",
                "score": round(score, 6),
            }
        )
    return results


# --------------------------------------------------------------------------- #
# Mock backend (deterministic, offline).
# --------------------------------------------------------------------------- #
def _mock_search(query: str, limit: int, mode: Optional[str]) -> "dict[str, Any]":
    """Deterministic contract-shaped results synthesised from the query.

    Mirrors what a live ``find_theorems`` would return closely enough for the
    pipeline/tests: fabricated facts whose names embed the query tokens, in a
    fixed theory, ranked descending. An empty/whitespace query yields no hits.
    """
    tokens = _tokenize(query)
    matches: "list[tuple[str, str]]" = []
    if tokens:
        base = "_".join(tokens[:3]) or "fact"
        # A small, stable family of plausible-looking facts.
        suffixes = ["def", "simps", "intro", "eq", "comm"]
        for suf in suffixes[: max(1, min(limit, len(suffixes)))]:
            fact = f"Theoremata_Mock.{base}_{suf}"
            stmt = " = ".join(tokens[:2]) if len(tokens) >= 2 else tokens[0]
            matches.append((fact, stmt))
    results = _to_results(matches, query, limit)
    return {
        "ok": True,
        "backend": "mock",
        "mode": "mock",
        "session": DEFAULT_SESSION,
        "query": query,
        "count": len(results),
        "results": results,
    }


# --------------------------------------------------------------------------- #
# Public API.
# --------------------------------------------------------------------------- #
def search(
    query: str,
    *,
    session: str = DEFAULT_SESSION,
    limit: int = DEFAULT_LIMIT,
    mode: Optional[str] = None,
    force_mode: Optional[str] = None,
    timeout: int = DEFAULT_TIMEOUT,
) -> "dict[str, Any]":
    """Run an Isabelle ``find_theorems`` premise search; return the contract.

    Parameters
    ----------
    query:
        A ``find_theorems`` criterion. With ``mode=None`` it is a term pattern
        (e.g. ``"_ + 0 = _"``); with ``mode="name"`` a name wildcard; with
        ``mode="simp"`` a simp-LHS pattern. Goal-relative modes
        (``intro``/``elim``/``dest``/``solves``) are passed through.
    session:
        Object-logic session (informational; the theory imports ``Main`` = HOL).
    limit:
        Max results (``find_theorems (limit) ...``).
    mode:
        ``find_theorems`` criterion mode (see above).
    force_mode:
        ``"live"`` / ``"mock"`` to override auto-probing (mainly for tests).
    timeout:
        Wall-clock budget for the live run (seconds).

    Returns ``{ok, backend, mode, session, query, count,
    results: [{name, module, kind, score}]}``. Never raises: a live failure
    degrades to mock with a ``note``.
    """
    effective = _resolve_mode(force_mode)
    if effective == "live":
        try:
            theory_text = _theory_text(query, limit, mode)
            out = _run_find_theorems(theory_text, timeout)
            matches = _parse_matches(out)
            results = _to_results(matches, query, limit)
            return {
                "ok": True,
                "backend": "isabelle",
                "mode": "live",
                "session": session,
                "query": query,
                "count": len(results),
                "results": results,
            }
        except IsabelleUnavailable as exc:
            out = _mock_search(query, limit, mode)
            out["session"] = session
            out["note"] = f"live mode unavailable ({exc}); fell back to mock"
            return out
    out = _mock_search(query, limit, mode)
    out["session"] = session
    return out


# --------------------------------------------------------------------------- #
# Worker-style dispatch (tool == "isabelle_retrieve").
# --------------------------------------------------------------------------- #
def run(request: "dict[str, Any]") -> "dict[str, Any]":
    """Dispatch a worker-style ``{"op": "search", ...}`` request."""
    op = request.get("op", "search")
    if op == "detect":
        return {
            "ok": True,
            "backend": "isabelle" if tool_available() else "mock",
            "mode": _resolve_mode(request.get("force_mode")),
            "command": isabelle_command(),
        }
    if op != "search":
        return {"ok": False, "error": f"unknown op: {op}"}
    return search(
        request.get("query", ""),
        session=request.get("session", DEFAULT_SESSION),
        limit=int(request.get("limit", DEFAULT_LIMIT)),
        mode=request.get("mode"),
        force_mode=request.get("force_mode"),
        timeout=int(request.get("timeout", DEFAULT_TIMEOUT)),
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
