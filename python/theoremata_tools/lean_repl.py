"""Theoremata warm persistent Lean checker (plan item B1).

Keeps ONE long-lived Lean process with its ``Environment`` warmed once (imports
loaded a single time) so repeated proof checks skip the ~1-minute cold Mathlib
import. Two execution modes, auto-detected, both behind the same interface:

* ``mode="repl"`` -- the community `leanprover-community/repl` executable
  (vendored/built under ``.theoremata/lean-repl``). It keeps an in-memory
  ``Environment`` and speaks JSON on stdin/stdout: ``{"cmd": "...", "env": <id>}``
  -> ``{"env": id, "messages": [...], "sorries": [...]}``. The warm imports are
  established once as env 0; every ``check`` reuses env 0, so a warm Mathlib
  session checks proofs in milliseconds instead of ~25-60s each.

* ``mode="lean"`` -- fallback when the REPL is unavailable. Each ``check`` runs
  ``lake env lean`` on a temp file. The process is not truly warm across calls,
  but a single ``LeanSession`` reuses one resolved toolchain / project setup and
  the OS/olean file cache stays hot, so repeated checks in one session are still
  cheaper than fully cold ones.

Measured (real hardware, Mathlib):
    import Mathlib (once): ~25.7 s
    subsequent checks    : 0.007 - 0.054 s each   (~500-1000x faster)

Public API:
    LeanSession(imports=..., root=..., ...).warm()/.check(src)/.close()
    run(request) -> dict         # {op:"check"|"warm", ...}
    main()                       # JSON argv-file/stdin -> JSON stdout

CLI:
    python -m theoremata_tools.lean_repl request.json
    echo '{"op":"warm","imports":["Init"]}' | python -m theoremata_tools.lean_repl
"""

from __future__ import annotations

import json
import os
import queue
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path
from typing import Any

# ---------------------------------------------------------------------------
# Locations
# ---------------------------------------------------------------------------

# repo_root/python/theoremata_tools/lean_repl.py  ->  parents[2] == repo root
_REPO_ROOT = Path(__file__).resolve().parents[2]
_REPL_PROJECT = _REPO_ROOT / ".theoremata" / "lean-repl"


def _default_repl_exe() -> Path | None:
    """Return the built REPL executable if present, else ``None``.

    Honours ``THEOREMATA_LEAN_REPL`` (a path to a prebuilt ``repl`` executable)
    first, for setups where the default ``.theoremata/lean-repl`` build tree is
    not available or has been relocated."""
    override = os.environ.get("THEOREMATA_LEAN_REPL")
    if override and Path(override).is_file():
        return Path(override)
    for name in ("repl.exe", "repl"):
        cand = _REPL_PROJECT / ".lake" / "build" / "bin" / name
        if cand.is_file():
            return cand
    return None


def _resolve(name: str, override: str | None = None) -> str | None:
    """Locate a toolchain binary: explicit override, then PATH, then elan bin.

    Prepends ``$HOME/.elan/bin`` to PATH first, matching the rest of the
    Theoremata tools (Windows non-login shells often miss it)."""
    if override:
        return override
    elan_bin = Path(os.path.expanduser("~")) / ".elan" / "bin"
    if elan_bin.is_dir():
        cur = os.environ.get("PATH", "")
        if str(elan_bin) not in cur.split(os.pathsep):
            os.environ["PATH"] = str(elan_bin) + os.pathsep + cur
    found = shutil.which(name)
    if found:
        return found
    for ext in ("", ".exe"):
        cand = elan_bin / (name + ext)
        if cand.exists():
            return str(cand)
    return None


# ---------------------------------------------------------------------------
# Session
# ---------------------------------------------------------------------------


class LeanSession:
    """A warm, persistent Lean checker.

    Parameters
    ----------
    imports:
        Modules to import once into the warm environment (default ``["Init"]``).
        Use ``["Mathlib"]`` for a full (slow-to-warm, fast-to-reuse) session.
    root:
        A Lake project directory whose build the imports resolve against. For
        Mathlib pass the mathlib checkout. When ``None`` the vendored REPL
        project is used (fine for core / ``Init`` imports).
    repl_exe / lake:
        Optional explicit binary paths. Auto-detected otherwise.
    warm_timeout / timeout:
        Seconds allowed for the initial import and for each individual check.
    prefer:
        ``"repl"`` (default, falls back to ``"lean"`` if the REPL is missing) or
        ``"lean"`` to force the fallback.
    """

    def __init__(
        self,
        imports: list[str] | None = None,
        root: str | None = None,
        repl_exe: str | None = None,
        lake: str | None = None,
        warm_timeout: float = 300.0,
        timeout: float = 120.0,
        prefer: str = "repl",
    ) -> None:
        self.imports = list(imports) if imports else ["Init"]
        self.root = os.path.abspath(root) if root else None
        self.warm_timeout = float(warm_timeout)
        self.timeout = float(timeout)

        self._lake = _resolve("lake", lake)
        exe = repl_exe or _default_repl_exe()
        self._repl_exe = str(exe) if exe else None

        if prefer == "repl" and self._repl_exe and self._lake:
            self.mode = "repl"
        else:
            self.mode = "lean"

        # repl-mode process state
        self._proc: subprocess.Popen | None = None
        self._reader: threading.Thread | None = None
        self._q: "queue.Queue[Any]" = queue.Queue()
        self._base_env: int | None = None
        self._warmed = False

    # -- process working directory -----------------------------------------
    def _cwd(self) -> str:
        if self.root:
            return self.root
        if _REPL_PROJECT.is_dir():
            return str(_REPL_PROJECT)
        return os.getcwd()

    # ------------------------------------------------------------------ repl
    def _reader_loop(self, stdout) -> None:
        """Accumulate blank-line-delimited JSON objects onto the queue.

        The REPL pretty-prints each response object over several lines and
        separates objects by a single empty line; empty lines never appear
        *inside* an object (nested content is indented)."""
        buf: list[str] = []
        try:
            for line in stdout:
                if line.strip() == "":
                    if buf:
                        self._q.put("".join(buf))
                        buf = []
                    continue
                buf.append(line)
        except Exception:
            pass
        finally:
            if buf:
                self._q.put("".join(buf))
            self._q.put(_EOF)

    def _start(self) -> None:
        cmd = [self._lake, "env", self._repl_exe]
        self._proc = subprocess.Popen(
            cmd,
            cwd=self._cwd(),
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            encoding="utf-8",
            errors="replace",  # Lean emits Unicode; cp1252 would crash on Windows
            bufsize=1,
        )
        # drain any stale items from a prior (dead) process
        self._q = queue.Queue()
        self._reader = threading.Thread(
            target=self._reader_loop, args=(self._proc.stdout,), daemon=True
        )
        self._reader.start()

    def _send(self, obj: dict) -> None:
        assert self._proc and self._proc.stdin
        self._proc.stdin.write(json.dumps(obj) + "\n\n")
        self._proc.stdin.flush()

    def _await(self, timeout: float) -> dict:
        """Wait for one response object, killing the process on timeout."""
        try:
            item = self._q.get(timeout=timeout)
        except queue.Empty:
            self._kill()
            raise TimeoutError(f"lean repl timed out after {timeout}s")
        if item is _EOF:
            raise BrokenPipeError("lean repl exited unexpectedly")
        return json.loads(item)

    def _kill(self) -> None:
        p = self._proc
        if not p:
            return
        try:
            p.terminate()  # SIGTERM / TerminateProcess
            try:
                p.wait(timeout=10)
            except subprocess.TimeoutExpired:
                p.kill()  # SIGKILL
                p.wait(timeout=10)
        except Exception:
            pass
        self._proc = None
        self._warmed = False
        self._base_env = None

    def _alive(self) -> bool:
        return self._proc is not None and self._proc.poll() is None

    # ------------------------------------------------------------------ warm
    def warm(self) -> dict:
        """Start the process (repl mode) and load the imports once.

        Idempotent: returns immediately if already warm. In fallback mode this
        primes the toolchain/olean cache with a no-op compile so the first real
        check is not penalised."""
        t0 = time.time()
        if self.mode == "repl":
            if self._warmed and self._alive():
                return {"ok": True, "mode": self.mode, "warmed": True, "env": self._base_env, "elapsed": 0.0}
            self._start()
            import_src = "\n".join(f"import {m}" for m in self.imports)
            try:
                self._send({"cmd": import_src})
                resp = self._await(self.warm_timeout)
            except (BrokenPipeError, TimeoutError) as exc:
                return {"ok": False, "mode": self.mode, "error": str(exc), "elapsed": time.time() - t0}
            self._base_env = resp.get("env", 0)
            self._warmed = True
            msgs = resp.get("messages", [])
            ok = not any(m.get("severity") == "error" for m in msgs)
            return {
                "ok": ok,
                "mode": self.mode,
                "warmed": True,
                "env": self._base_env,
                "imports": self.imports,
                "messages": msgs,
                "elapsed": time.time() - t0,
            }
        # fallback: prime the cache with a trivial compile
        res = self._lean_compile("")
        self._warmed = True
        return {
            "ok": res["ok"],
            "mode": self.mode,
            "warmed": True,
            "imports": self.imports,
            "elapsed": time.time() - t0,
        }

    # ----------------------------------------------------------------- check
    def check(self, source_or_theorem: str, print_axioms: str | None = None) -> dict:
        """Type-check ``source_or_theorem`` against the warm imports.

        Returns ``{ok, messages, sorries, [axioms], elapsed, mode}``. In repl
        mode reuses warm env 0 (fast). If the process died it is restarted and
        the imports re-warmed transparently. ``print_axioms`` (a declaration
        name) additionally runs ``#print axioms <name>`` and reports the result
        under ``axioms``."""
        t0 = time.time()
        if self.mode == "repl":
            if not (self._warmed and self._alive()):
                w = self.warm()
                if not w.get("ok"):
                    return {"ok": False, "mode": self.mode, "messages": [], "sorries": [],
                            "error": w.get("error", "warm failed"), "elapsed": time.time() - t0}
            try:
                self._send({"cmd": source_or_theorem, "env": self._base_env})
                resp = self._await(self.timeout)
            except (BrokenPipeError, TimeoutError) as exc:
                return {"ok": False, "mode": self.mode, "messages": [], "sorries": [],
                        "error": str(exc), "elapsed": time.time() - t0}
            msgs = resp.get("messages", [])
            sorries = resp.get("sorries", [])
            errors = [m for m in msgs if m.get("severity") == "error"]
            ok = not errors
            out: dict[str, Any] = {
                "ok": ok,
                "mode": self.mode,
                "messages": msgs,
                "sorries": sorries,
                "env": resp.get("env"),
                "elapsed": time.time() - t0,
            }
            if print_axioms:
                try:
                    self._send({"cmd": f"#print axioms {print_axioms}", "env": resp.get("env", self._base_env)})
                    ax = self._await(self.timeout)
                    out["axioms"] = [m.get("data", "") for m in ax.get("messages", [])]
                except (BrokenPipeError, TimeoutError):
                    out["axioms"] = None
            return out
        # fallback mode
        res = self._lean_compile(source_or_theorem)
        res["elapsed"] = time.time() - t0
        return res

    # ---------------------------------------------------- fallback lean mode
    def _lean_compile(self, source: str) -> dict:
        lake = self._lake or _resolve("lake")
        if not lake:
            return {"ok": False, "mode": "lean", "messages": [], "sorries": [], "error": "lake not found"}
        header = "\n".join(f"import {m}" for m in self.imports)
        text = (header + "\n" + source) if header else source
        tmp = tempfile.NamedTemporaryFile("w", suffix=".lean", delete=False, encoding="utf-8")
        try:
            tmp.write(text)
            tmp.close()
            cmd = [lake, "env", "lean", tmp.name]
            proc = subprocess.Popen(
                cmd, cwd=self._cwd(), stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                encoding="utf-8", errors="replace",
            )
            try:
                out, _ = proc.communicate(timeout=self.timeout)
                rc = proc.returncode
                timed_out = False
            except subprocess.TimeoutExpired:
                timed_out = True
                proc.terminate()
                try:
                    out, _ = proc.communicate(timeout=10)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    out, _ = proc.communicate()
                rc = proc.returncode
        finally:
            try:
                os.unlink(tmp.name)
            except OSError:
                pass
        out = out or ""
        messages, sorries = _parse_lean_output(out)
        ok = (not timed_out) and rc == 0 and not any(m["severity"] == "error" for m in messages)
        res = {"ok": ok, "mode": "lean", "messages": messages, "sorries": sorries, "raw": out}
        if timed_out:
            res["error"] = f"lean timed out after {self.timeout}s"
        return res

    # ----------------------------------------------------------------- close
    def close(self) -> dict:
        if self.mode == "repl" and self._proc is not None:
            try:
                if self._proc.stdin:
                    self._proc.stdin.close()
                self._proc.wait(timeout=5)
            except Exception:
                self._kill()
            self._proc = None
        self._warmed = False
        self._base_env = None
        return {"ok": True, "closed": True, "mode": self.mode}

    def __enter__(self) -> "LeanSession":
        self.warm()
        return self

    def __exit__(self, *exc) -> None:
        self.close()


_EOF = object()


def _parse_lean_output(out: str) -> tuple[list[dict], list[dict]]:
    """Parse ``lake env lean`` diagnostics into REPL-shaped message dicts."""
    messages: list[dict] = []
    sorries: list[dict] = []
    for line in out.splitlines():
        low = line.lower()
        sev = None
        if ": error:" in low or low.endswith(": error"):
            sev = "error"
        elif ": warning:" in low:
            sev = "warning"
        elif ": info:" in low:
            sev = "info"
        if sev:
            messages.append({"severity": sev, "data": line.strip()})
        if "sorry" in low:
            if not sev:
                messages.append({"severity": "warning", "data": line.strip()})
            sorries.append({"data": line.strip()})
    return messages, sorries


# ---------------------------------------------------------------------------
# Dispatch / CLI
# ---------------------------------------------------------------------------


def run(request: dict) -> dict:
    """Dispatch a single request. Each call is a self-contained session.

    ``{op:"warm", imports, root}``  -> warm result
    ``{op:"check", source, imports, root, print_axioms?}`` -> check result
    """
    op = request.get("op", "check")
    session = LeanSession(
        imports=request.get("imports"),
        root=request.get("root"),
        repl_exe=request.get("repl_exe"),
        lake=request.get("lake"),
        warm_timeout=float(request.get("warm_timeout", 300.0)),
        timeout=float(request.get("timeout", 120.0)),
        prefer=request.get("prefer", "repl"),
    )
    try:
        if op == "warm":
            return session.warm()
        if op == "check":
            w = session.warm()
            if not w.get("ok"):
                return {"ok": False, "mode": session.mode, "error": w.get("error", "warm failed"),
                        "warm": w}
            res = session.check(request["source"], print_axioms=request.get("print_axioms"))
            res["warm_elapsed"] = w.get("elapsed")
            return res
        raise ValueError(f"unknown op: {op!r}")
    finally:
        session.close()


# ---------------------------------------------------------------------------
# Resident server: one warm session reused across many requests
# ---------------------------------------------------------------------------

_ALLOWED_AXIOMS = {"propext", "Classical.choice", "Quot.sound"}


def _parse_axiom_names(axioms_msgs: list[str] | None) -> list[str]:
    """Extract axiom names from ``#print axioms`` message text.

    Handles both ``'t' does not depend on any axioms`` (-> ``[]``) and
    ``'t' depends on axioms: [propext, Classical.choice]``."""
    text = " ".join(a for a in (axioms_msgs or []) if a)
    if "does not depend on any axioms" in text:
        return []
    match = re.search(r"\[([^\]]*)\]", text)
    if not match:
        return []
    return [x.strip() for x in match.group(1).split(",") if x.strip()]


def serve(stdin: Any = None, stdout: Any = None) -> int:
    """Persistent newline-delimited JSON-RPC loop over ONE warm ``LeanSession``.

    Reads one request object per line and writes one response line each:
    request  ``{"op":"warm"|"check","source":?,"theorem":?,"imports":?,"root":?}``
    response ``{"ok":bool,"axioms_clean":bool,"messages":[...],"axioms":[...],"elapsed":...}``

    The session is created lazily and reused; it is recreated when the requested
    ``imports``/``root`` change or the underlying process dies. Malformed lines
    yield an error response but keep the loop alive; EOF exits cleanly."""
    stdin = sys.stdin if stdin is None else stdin
    stdout = sys.stdout if stdout is None else stdout
    state: dict[str, Any] = {"session": None, "imports": None, "root": None}

    def emit(obj: dict) -> None:
        stdout.write(json.dumps(obj) + "\n")
        stdout.flush()

    def ensure(imports: list[str], root: str | None):
        session = state["session"]
        stale = (
            session is None
            or state["imports"] != imports
            or state["root"] != root
            or (getattr(session, "mode", None) == "repl" and not session._alive())
        )
        if stale:
            if session is not None:
                try:
                    session.close()
                except Exception:
                    pass
            session = LeanSession(imports=imports, root=root)
            warm = session.warm()
            state.update(session=session, imports=imports, root=root)
            return session, warm
        return session, {"ok": True, "warmed": True, "elapsed": 0.0}

    while True:
        line = stdin.readline()
        if not line:  # EOF
            break
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except Exception as exc:  # noqa: BLE001
            emit({"ok": False, "error": f"malformed request: {exc}"})
            continue
        op = request.get("op", "check")
        imports = request.get("imports") or ["Init"]
        root = request.get("root")
        try:
            session, warm = ensure(imports, root)
            if not warm.get("ok", True):
                emit({"ok": False, "error": warm.get("error", "warm failed"), "warm": warm})
                continue
            if op == "warm":
                emit({"ok": True, "warmed": True, "mode": session.mode,
                      "elapsed": warm.get("elapsed", 0.0)})
                continue
            if op == "check":
                theorem = request.get("theorem")
                result = session.check(request.get("source", ""), print_axioms=theorem)
                axioms = _parse_axiom_names(result.get("axioms")) if theorem else []
                axioms_clean = bool(result.get("ok")) and (
                    not theorem or all(a in _ALLOWED_AXIOMS for a in axioms)
                )
                emit({
                    "ok": bool(result.get("ok")),
                    "axioms_clean": axioms_clean,
                    "messages": [m.get("data", "") for m in result.get("messages", [])],
                    "axioms": axioms,
                    "sorries": result.get("sorries", []),
                    "mode": session.mode,
                    "elapsed": result.get("elapsed"),
                })
                continue
            emit({"ok": False, "error": f"unknown op: {op!r}"})
        except Exception as exc:  # noqa: BLE001 -- reset session, keep serving
            try:
                if state["session"] is not None:
                    state["session"].close()
            except Exception:
                pass
            state["session"] = None
            emit({"ok": False, "error": str(exc), "error_type": type(exc).__name__})

    if state["session"] is not None:
        try:
            state["session"].close()
        except Exception:
            pass
    return 0


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] == "serve":
        return serve()
    if argv and argv[0] not in ("-", ""):
        raw = Path(argv[0]).read_text(encoding="utf-8")
    else:
        raw = sys.stdin.read()
    try:
        req = json.loads(raw)
        result = run(req)
    except Exception as exc:  # structured error, still exit 0
        print(json.dumps({"ok": False, "error": str(exc), "error_type": type(exc).__name__}))
        return 0
    print(json.dumps(result))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
