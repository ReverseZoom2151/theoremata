"""Rocq (Coq) premise retrieval for Theoremata (parity with Lean retrieval).

Coq's premise model is *interactive*: instead of a giant pre-built declaration
index (the Lean/Mathlib approach in :mod:`retrieval`), you ask a live toplevel
``Search`` / ``SearchPattern`` / ``SearchRewrite`` question and read back the
matching ``name: type`` lines. This module mirrors that model as an on-demand
search that still emits the SAME result contract as the Lean retriever so its
output can feed the shared reranker/cascade:

    {ok, backend, mode, results: [{name, module, kind, score}, ...]}

How a query runs
----------------
``search`` writes a throwaway ``.v`` that ``Require Import``s a lightweight set
of libraries (default :data:`DEFAULT_IMPORTS`; MathComp when requested) then
issues one ``Search`` vernacular, compiles it with ``coqc`` and parses the
printed premises. On this Windows box the real toolchain lives in **WSL Ubuntu**
(``/usr/bin/coqc``), reached through ``wsl.exe`` with Windows->``/mnt`` path
translation (same mechanism as ``prover``'s ``rocq_driver``/``hammer``); a
native ``coqc`` on ``PATH`` is used directly when present (e.g. real Linux).

Two run modes (graceful degradation)
------------------------------------
* **live**  -- a ``coqc`` binary is reachable (native or via WSL). We compile a
  real query and parse real premises.
* **mock**  -- offline / no toolchain (or ``THEOREMATA_ROCQ_RETRIEVAL_MOCK=1``).
  Returns deterministic canned premises for a handful of common stdlib queries
  so tests and the agent loop keep working with no Coq installed.

Every response carries ``backend`` (``"coqc"`` | ``"coqc-wsl"`` | ``"mock"``)
and ``mode`` (``"live"`` | ``"mock"``).

Scoring
-------
``Search`` returns premises in Coq's own order; we keep that as a stable
tiebreak and layer a light lexical score on top (shared tokeniser with
:mod:`retrieval`) so a name-shaped query such as ``add_comm`` ranks its exact
hit first while a pure pattern query (``(?n + ?m = ?m + ?n)``) preserves Coq's
ordering. ``module`` is derived from the qualified name (``Nat.add_comm`` ->
``Nat``); ``kind`` is a coarse default (``"lemma"``) for interactive Search --
exact per-constant kinds would need a follow-up ``About`` per hit, which the
on-demand model deliberately avoids. ``dump_decls`` (for a *local* ``.v``) does
report exact kinds, read from the ``.glob`` cross-reference file.

Worker wiring (report only -- this module does NOT edit worker.py)
------------------------------------------------------------------
    if tool == "rocq_retrieve":
        from theoremata_tools.rocq_retrieval import run as rocq_retrieve_run
        return rocq_retrieve_run(request)   # {"op": "search"|"dump", ...}
"""
from __future__ import annotations

import functools
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, Optional

from . import retrieval  # reuse the tokenizer / normaliser for lexical scoring

# --------------------------------------------------------------------------- #
# Configuration / environment.
# --------------------------------------------------------------------------- #
MOCK_ENV = "THEOREMATA_ROCQ_RETRIEVAL_MOCK"
COQC_ENV = "THEOREMATA_ROCQ_COQC"  # explicit native coqc path override
WSL_DISTRO = os.environ.get("THEOREMATA_WSL_DISTRO", "Ubuntu")
_IS_WINDOWS = os.name == "nt"

# CoqHammer / MathComp on this box live in the opam switch; ``eval "$(opam env)"``
# puts them on PATH. Prepended only when a query asks for MathComp libraries.
_OPAM_ENV_PREFIX = 'eval "$(opam env)" 2>/dev/null\n'

# A lightweight, always-available default corpus. ``Coq.Init.Prelude`` is loaded
# implicitly; Arith + List cover the common arithmetic / list premises.
DEFAULT_IMPORTS = ["Coq.Arith.Arith", "Coq.Lists.List"]

# Query flavours -> vernacular command.
_MODE_VERNAC = {
    "search": "Search",
    "pattern": "SearchPattern",
    "rewrite": "SearchRewrite",
}

# .glob kind token -> our declaration ``kind`` (for dump_decls).
_GLOB_KIND = {
    "def": "def",
    "prf": "theorem",
    "thm": "theorem",
    "ind": "inductive",
    "constr": "constructor",
    "var": "variable",
    "ax": "axiom",
    "inst": "instance",
    "not": "notation",
    "coe": "coercion",
    "prfax": "axiom",
    "scheme": "scheme",
    "sec": "section",
    "mod": "module",
    "modtype": "moduletype",
    "lib": "library",
}


# --------------------------------------------------------------------------- #
# WSL / native coqc plumbing.
# --------------------------------------------------------------------------- #
def _win_to_wsl_path(path: str) -> str:
    """Translate a Windows path (``C:\\a\\b``) to its WSL form (``/mnt/c/a/b``)."""
    ap = os.path.abspath(path)
    drive, rest = os.path.splitdrive(ap)
    rest = rest.replace("\\", "/")
    if drive and len(drive) >= 2 and drive[1] == ":":
        return f"/mnt/{drive[0].lower()}{rest}"
    return ap.replace("\\", "/")


def _wsl_available() -> bool:
    """True if a ``wsl.exe`` launcher is discoverable (Windows only)."""
    return _IS_WINDOWS and bool(shutil.which("wsl.exe") or shutil.which("wsl"))


@functools.lru_cache(maxsize=None)
def _wsl_probe(check_cmd: str) -> bool:
    """Run ``bash -lc <check_cmd>`` in WSL; True iff it exits 0. Cached (a WSL
    spawn is ~1s cold and toolchain presence is stable for the process)."""
    if not _wsl_available():
        return False
    try:
        proc = subprocess.run(
            ["wsl.exe", "-d", WSL_DISTRO, "--", "bash", "-lc", check_cmd],
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return False
    return proc.returncode == 0


def _native_coqc() -> Optional[str]:
    """An explicit override or a ``coqc``/``rocq`` on the process PATH."""
    override = os.environ.get(COQC_ENV)
    if override and (os.path.exists(override) or shutil.which(override)):
        return override
    return shutil.which("coqc") or shutil.which("rocq")


def detect_backend() -> "tuple[str, str, Optional[str]]":
    """Decide ``(mode, backend, binary)``.

    Preference: forced-mock env > native ``coqc`` > WSL ``coqc`` > mock. With
    ``THEOREMATA_ROCQ_RETRIEVAL_MOCK=0`` a live WSL attempt is reported even when
    the probe cannot see a binary (so the caller sees the live path was taken).
    """
    forced = os.environ.get(MOCK_ENV, "")
    if forced in ("1", "true", "True"):
        return "mock", "mock", None

    native = _native_coqc()
    if native:
        return "live", "coqc", native
    if _wsl_probe("command -v coqc >/dev/null 2>&1"):
        return "live", "coqc-wsl", "coqc"
    if forced in ("0", "false", "False"):
        return "live", "coqc-wsl", "coqc"
    return "mock", "mock", None


def _wsl_bash(script_body: str, timeout: float) -> "tuple[int, str]":
    """Run a bash script in WSL, return ``(returncode, stdout+stderr)``.

    Executed from a temp **file** (``bash <file>``) not ``bash -lc <string>``:
    passing a multi-line script (or one with ``$(...)`` substitutions) through
    the ``wsl.exe`` argument layer silently mangles it, whereas a script file
    behaves normally. Written with LF newlines for the Linux side.
    """
    fd, spath = tempfile.mkstemp(suffix=".sh")
    os.close(fd)
    try:
        with open(spath, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(script_body)
        try:
            proc = subprocess.run(
                ["wsl.exe", "-d", WSL_DISTRO, "--", "bash", "-l",
                 _win_to_wsl_path(spath)],
                capture_output=True,
                text=True,
                timeout=timeout,
                check=False,
            )
        except (OSError, subprocess.SubprocessError) as exc:
            return 1, f"wsl invocation failed: {exc}"
        return proc.returncode, (proc.stdout or "") + (proc.stderr or "")
    finally:
        try:
            os.unlink(spath)
        except OSError:
            pass


def _run_coqc(vfile: str, backend: str, binary: str, *, use_opam: bool,
              timeout: float) -> "tuple[int, str]":
    """Compile ``vfile`` (a Windows/native path) and return ``(rc, output)``.

    ``coqc`` writes its ``.vo``/``.glob`` next to the source; the printed
    ``Search`` premises come back on stdout/stderr, which we merge.
    """
    if backend == "coqc-wsl":
        mnt = _win_to_wsl_path(vfile)
        prefix = _OPAM_ENV_PREFIX if use_opam else ""
        script = f'{prefix}{binary} "{mnt}" 2>&1\n'
        return _wsl_bash(script, timeout)
    # Native.
    try:
        proc = subprocess.run(
            [binary, vfile],
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        return 1, f"coqc invocation failed: {exc}"
    return proc.returncode, (proc.stdout or "") + (proc.stderr or "")


# --------------------------------------------------------------------------- #
# Query construction + output parsing.
# --------------------------------------------------------------------------- #
def _require_lines(imports: list[str]) -> "tuple[list[str], bool]":
    """Build ``Require Import`` lines; flag whether opam (MathComp) is needed.

    A dotted stdlib path (``Coq.Arith.Arith``) becomes ``Require Import <path>.``
    A ``mathcomp``-rooted path (``mathcomp.ssreflect`` / ``mathcomp.ssrnat``)
    becomes ``From mathcomp Require Import <leaf>.`` and turns on the opam env.
    """
    lines: list[str] = []
    use_opam = False
    for imp in imports:
        imp = imp.strip()
        if not imp:
            continue
        if imp.startswith("mathcomp."):
            use_opam = True
            leaf = imp.split(".", 1)[1]
            lines.append(f"From mathcomp Require Import {leaf}.")
        elif imp.lower() == "mathcomp":
            use_opam = True
            lines.append("From mathcomp Require Import all_ssreflect.")
        else:
            lines.append(f"Require Import {imp}.")
    return lines, use_opam


# A "name-shaped" query is a bare identifier fragment (letters/digits/_/./').
# Coq's bare ``Search foo`` looks for premises *mentioning* the constant ``foo``
# (and can trip deprecated-notation warnings); a name lookup wants the substring
# form ``Search "foo"`` which matches by declaration name. Anything with spaces,
# parens, ``?`` holes or operators is a genuine pattern -> passed through as-is.
_NAME_RE = re.compile(r"^[\w.']+$")


# The query ``.v`` is a committed template (parity with the Lean retriever, whose
# ``dump_decls.lean`` lives in ``retrieval/lean/``). ``search_template.v`` sits in
# ``retrieval/coq/`` -- ``parents[2]`` is ``components/retrieval`` -- and carries a
# leading ``(* ... *)`` header we strip so the compiled source is byte-identical to
# the old inline string (see the template file for the placeholder contract).
_TEMPLATE_PATH = Path(__file__).resolve().parents[2] / "coq" / "search_template.v"


def _strip_header(text: str) -> str:
    """Drop a leading ``(* ... *)`` documentation header, if present."""
    if text.startswith("(*"):
        end = text.find("*)")
        if end != -1:
            text = text[end + 2:].lstrip("\n")
    return text


@functools.lru_cache(maxsize=1)
def _search_template() -> str:
    """Load the committed ``search_template.v`` scaffold (header stripped)."""
    return _strip_header(_TEMPLATE_PATH.read_text(encoding="utf-8"))


def _build_vfile(query: str, imports: list[str], mode: str) -> "tuple[str, bool]":
    """Return ``(source, use_opam)`` for a one-shot search ``.v``.

    The source is the committed ``search_template.v`` with ``__IMPORTS__`` and
    ``__SEARCH_COMMAND__`` substituted -- byte-identical to the former inline
    ``"\\n".join(req + [cmd]) + "\\n"`` construction.
    """
    req, use_opam = _require_lines(imports)
    vernac = _MODE_VERNAC.get(mode or "search", "Search")
    q = query.strip().rstrip(".")
    if vernac == "Search":
        if _NAME_RE.match(q) and not q.startswith('"'):
            # Name-shaped -> substring search by declaration name.
            cmd = f'Search "{q}".'
        else:
            cmd = f"Search {q}."
    else:
        # SearchPattern / SearchRewrite take a single parenthesised pattern.
        pat = q if q.startswith("(") else f"({q})"
        cmd = f"{vernac} {pat}."
    body = (
        _search_template()
        .replace("__IMPORTS__", "\n".join(req))
        .replace("__SEARCH_COMMAND__", cmd)
    )
    return body, use_opam


# A premise entry begins at column 0 with ``name:`` (Coq identifiers/notation
# names carry no spaces); its type may wrap onto following indented lines.
_ENTRY_RE = re.compile(r"^(\S+):\s?(.*)$")
_NOISE_PREFIXES = ("File ", "Error", "Warning", "Toplevel", "Syntax error")


def _parse_search_output(text: str) -> "tuple[list[dict[str, Any]], Optional[str]]":
    """Parse ``coqc`` Search output into ``[{name, type}]`` + any error string."""
    error: Optional[str] = None
    entries: list[dict[str, Any]] = []
    cur: Optional[dict[str, Any]] = None
    for raw in text.splitlines():
        line = raw.rstrip()
        if not line:
            continue
        if line.startswith(_NOISE_PREFIXES):
            if line.startswith("Error") and error is None:
                error = line.strip()
            cur = None
            continue
        if line[0].isspace():
            # Continuation of the current entry's type.
            if cur is not None:
                cur["type"] = (cur["type"] + " " + line.strip()).strip()
            continue
        m = _ENTRY_RE.match(line)
        if not m:
            cur = None
            continue
        name, rest = m.group(1), m.group(2).strip()
        # Skip stray labels ("Search ... :" echoes never appear, but be safe).
        cur = {"name": name, "type": rest}
        entries.append(cur)
    return entries, error


def _module_of(name: str) -> str:
    """Module = the qualifier of a dotted name (``Nat.add_comm`` -> ``Nat``)."""
    return name.rsplit(".", 1)[0] if "." in name else ""


def _score(query: str, name: str, module: str, rank: int) -> float:
    """Light lexical score with Coq's own ordering kept as a stable tiebreak."""
    q_norm = retrieval._normalize_ident(query)
    q_terms = set(retrieval.tokenize(query))
    name_terms = retrieval.tokenize(name)
    mod_terms = retrieval.tokenize(module)
    overlap = sum(1 for t in name_terms if t in q_terms)
    overlap += sum(0.5 for t in mod_terms if t in q_terms)
    exact = 0.0
    if q_norm and retrieval._normalize_ident(name) == q_norm:
        exact = 10.0
    elif q_norm and retrieval._normalize_ident(retrieval._last_segment(name)) == q_norm:
        exact = 5.0
    # Rank decay (<1) preserves Coq's premise order when lexical signal is flat.
    rank_bonus = 1.0 / (rank + 2.0)
    return round(2.0 * overlap + exact + rank_bonus, 6)


def _rank_entries(query: str, entries: list[dict[str, Any]], limit: int) -> list[dict[str, Any]]:
    scored: list[tuple[float, int, dict[str, Any]]] = []
    for rank, e in enumerate(entries):
        name = e.get("name") or ""
        if not name:
            continue
        module = _module_of(name)
        rec = {
            "name": name,
            "module": module,
            "kind": "lemma",  # coarse default for interactive Search (see docstring)
            "score": _score(query, name, module, rank),
        }
        scored.append((rec["score"], rank, rec))
    scored.sort(key=lambda t: (-t[0], t[1]))
    return [rec for _s, _r, rec in scored[: max(0, limit)]]


# --------------------------------------------------------------------------- #
# Mock backend (offline, deterministic).
# --------------------------------------------------------------------------- #
# A tiny canned stdlib corpus keyed by query substring, so an offline call for a
# common lemma shape still returns plausible premises in the shared contract.
_MOCK_CORPUS: list[dict[str, str]] = [
    {"name": "Nat.add_comm", "type": "forall n m : nat, n + m = m + n"},
    {"name": "Nat.add_assoc", "type": "forall n m p : nat, n + (m + p) = n + m + p"},
    {"name": "Nat.add_0_r", "type": "forall n : nat, n + 0 = n"},
    {"name": "Nat.add_0_l", "type": "forall n : nat, 0 + n = n"},
    {"name": "Nat.mul_comm", "type": "forall n m : nat, n * m = m * n"},
    {"name": "Nat.mul_assoc", "type": "forall n m p : nat, n * (m * p) = n * m * p"},
    {"name": "Nat.le_refl", "type": "forall n : nat, n <= n"},
    {"name": "Nat.le_trans", "type": "forall n m p : nat, n <= m -> m <= p -> n <= p"},
    {"name": "le_n", "type": "forall n : nat, n <= n"},
    {"name": "app_length",
     "type": "forall (A : Type) (l l' : list A), length (l ++ l') = length l + length l'"},
    {"name": "app_assoc",
     "type": "forall (A : Type) (l m n : list A), l ++ m ++ n = (l ++ m) ++ n"},
    {"name": "rev_involutive",
     "type": "forall (A : Type) (l : list A), rev (rev l) = l"},
]


def _mock_search(query: str, limit: int) -> list[dict[str, Any]]:
    """Deterministic offline premises: rank the canned corpus by lexical overlap
    with ``query`` (falling back to the whole corpus when nothing matches)."""
    q_terms = set(retrieval.tokenize(query))
    matched = [
        e for e in _MOCK_CORPUS
        if q_terms & (set(retrieval.tokenize(e["name"])) | set(retrieval.tokenize(e["type"])))
    ]
    entries = matched or list(_MOCK_CORPUS)
    return _rank_entries(query, entries, limit)


# --------------------------------------------------------------------------- #
# Public API.
# --------------------------------------------------------------------------- #
def search(
    query: str,
    *,
    imports: Optional[list[str]] = None,
    limit: int = 20,
    mode: Optional[str] = None,
    timeout: float = 120.0,
) -> dict[str, Any]:
    """Run a Coq premise search for ``query`` and return the shared contract.

    ``mode`` selects the vernacular: ``"search"`` (default, ``Search <query>``),
    ``"pattern"`` (``SearchPattern (<query>)``), or ``"rewrite"``
    (``SearchRewrite (<query>)``). ``imports`` defaults to
    :data:`DEFAULT_IMPORTS`; pass ``mathcomp``-rooted paths to pull MathComp.
    Degrades to deterministic mock results when no ``coqc`` is reachable.
    """
    imports = imports if imports is not None else DEFAULT_IMPORTS
    m, backend, binary = detect_backend()

    if m == "mock":
        return _mock_result(query, imports, limit)

    body, use_opam = _build_vfile(query, imports, mode or "search")
    tmpdir = tempfile.mkdtemp(prefix="theoremata_rocq_")
    vfile = os.path.join(tmpdir, "TheoremataQuery.v")
    try:
        with open(vfile, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(body)
        rc, out = _run_coqc(vfile, backend, binary or "coqc",
                            use_opam=use_opam, timeout=timeout)
        entries, error = _parse_search_output(out)
        if not entries and error:
            # Live compile failed outright (bad import, etc.) -> mock fallback.
            fallback = _mock_result(query, imports, limit)
            fallback.update({
                "ok": True, "backend": backend, "mode": "live",
                "query": query, "imports": list(imports),
                "note": f"live search error, using mock: {error}",
                "results_backend": "mock",
            })
            return fallback
        results = _rank_entries(query, entries, limit)
        return {
            "ok": True,
            "backend": backend,
            "mode": "live",
            "query": query,
            "imports": list(imports),
            "count": len(results),
            "results": results,
        }
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)


def _mock_result(query: str, imports: list[str], limit: int) -> dict[str, Any]:
    results = _mock_search(query, limit)
    return {
        "ok": True,
        "backend": "mock",
        "mode": "mock",
        "query": query,
        "imports": list(imports),
        "count": len(results),
        "results": results,
    }


# --------------------------------------------------------------------------- #
# dump_decls: enumerate a local .v's declarations via its .glob.
# --------------------------------------------------------------------------- #
def _parse_glob(text: str) -> list[dict[str, Any]]:
    """Parse a coqc ``.glob`` file into ``[{name, module, kind}]`` declarations.

    Definition lines look like ``def 11:15 <> myDef`` / ``prf 37:41 <> myThm``;
    an ``F<Module>`` line names the enclosing module. ``R...`` reference lines
    and digests are skipped.
    """
    module = ""
    decls: list[dict[str, Any]] = []
    line_re = re.compile(r"^([a-z]+)\s+\d+:\d+\s+(\S+)\s+(\S+)$")
    for raw in text.splitlines():
        line = raw.rstrip()
        if not line:
            continue
        if line.startswith("F"):
            module = line[1:].strip()
            continue
        if line[0] not in "abcdefghijklmnopqrstuvwxyz":
            continue  # DIGEST, R<...> references, etc.
        m = line_re.match(line)
        if not m:
            continue
        tok, _secpath, name = m.group(1), m.group(2), m.group(3)
        kind = _GLOB_KIND.get(tok)
        if kind is None:
            continue
        full = f"{module}.{name}" if module else name
        decls.append({
            "name": full,
            "module": module,
            "kind": kind,
            "is_axiom": kind == "axiom",
        })
    return decls


def dump_decls(vfile: str, *, timeout: float = 300.0) -> dict[str, Any]:
    """Compile a local ``.v`` and enumerate its declarations from the ``.glob``.

    Returns ``{ok, backend, mode, decls:[{name, module, kind, is_axiom}]}``.
    Live only (needs ``coqc``); returns ``ok:false`` in mock/no-toolchain mode.
    """
    m, backend, binary = detect_backend()
    if m == "mock":
        return {"ok": False, "backend": "mock", "mode": "mock", "decls": [],
                "stderr": "coqc unavailable (dump requires a live toolchain)"}
    if not os.path.exists(vfile):
        return {"ok": False, "backend": backend, "mode": "live", "decls": [],
                "stderr": f"file not found: {vfile}"}
    rc, out = _run_coqc(vfile, backend, binary or "coqc",
                        use_opam=False, timeout=timeout)
    glob = os.path.splitext(vfile)[0] + ".glob"
    if not os.path.exists(glob):
        return {"ok": False, "backend": backend, "mode": "live", "decls": [],
                "stderr": (out or "no .glob produced").strip()[:2000]}
    try:
        with open(glob, encoding="utf-8", errors="replace") as fh:
            decls = _parse_glob(fh.read())
    except OSError as exc:
        return {"ok": False, "backend": backend, "mode": "live", "decls": [],
                "stderr": str(exc)}
    return {"ok": len(decls) > 0, "backend": backend, "mode": "live",
            "count": len(decls), "decls": decls}


# --------------------------------------------------------------------------- #
# Worker-style dispatch (tool == "rocq_retrieve").
# --------------------------------------------------------------------------- #
def run(request: dict[str, Any]) -> dict[str, Any]:
    """Dispatch a worker-style request. ``op`` selects search vs dump.

    ``{op:"search", query, imports?, limit?, mode?}`` -> premise search.
    ``{op:"dump", vfile}`` -> local-file declaration enumeration.
    ``{op:"detect"}`` -> report the resolved backend/mode.
    """
    op = request.get("op", "search")
    if op == "detect":
        mode, backend, binary = detect_backend()
        return {"ok": True, "mode": mode, "backend": backend, "binary": binary}
    if op == "search":
        return search(
            request.get("query", ""),
            imports=request.get("imports"),
            limit=int(request.get("limit", 20)),
            mode=request.get("mode"),
            timeout=float(request.get("timeout", 120.0)),
        )
    if op == "dump":
        return dump_decls(
            request.get("vfile", ""),
            timeout=float(request.get("timeout", 300.0)),
        )
    return {"ok": False, "stderr": f"unknown op: {op}"}


def main() -> None:
    if len(sys.argv) >= 2 and os.path.exists(sys.argv[1]):
        with open(sys.argv[1], encoding="utf-8") as fh:
            req = json.load(fh)
    else:
        req = json.load(sys.stdin)
    out = run(req)
    print(json.dumps(out))
    raise SystemExit(0 if out.get("ok", True) else 1)


if __name__ == "__main__":
    main()
