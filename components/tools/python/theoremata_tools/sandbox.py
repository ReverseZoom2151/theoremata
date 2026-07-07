"""Hardened execution sandbox for untrusted, model-emitted math snippets.

Ports three robustness patterns from DeepMath's ``agent.py`` (studied in
``docs/resource-mining/DeepMath.md``) into a single, reusable module:

1. **Subprocess hard-kill timeout** with a pickle -> thread fallback
   (:func:`run_in_subprocess`). Untrusted work runs in a child process that is
   ``terminate()``/``kill()``-ed *and* tree-killed on timeout, so a runaway
   (e.g. an accidental ``sum(1 for _ in range(10**12))``) is stopped instead of
   hanging the worker. When the callable/args are not picklable we fall back to
   a daemon thread that can time out but cannot force-kill (DeepMath's fallback).

2. **Import allow-list** with a self-correcting hint (:func:`guard_imports`,
   :data:`ALLOWED_IMPORTS`). A disallowed import raises
   :class:`ImportNotAllowedError` carrying an ``"Import not allowed: X"`` message
   so the model can fix its own spec.

3. **Global step/token budget governor** (:class:`StepBudget`). Decouples the
   number of turns from a single total budget and forces graceful termination
   when the budget is exhausted, rather than running to an implicit per-turn cap.

This module is internal hardening: it does **not** change the JSON in/out
contract of :mod:`theoremata_tools.falsify` / :mod:`theoremata_tools.safe_eval`
that the Rust caller (``components/reason/falsification.rs``) depends on.
"""
from __future__ import annotations

import ast
import multiprocessing as mp
import os
import pickle
import signal
import subprocess
import sys
import threading
import traceback
from dataclasses import dataclass
from typing import Any, Callable, Iterable

# --- Import allow-list -----------------------------------------------------

#: Modules the falsifier / safe-eval sandbox permits. Kept to the pure-math libs
#: our checks actually need (see ``falsify.py`` / ``safe_eval.py``): no ``os``,
#: ``sys``, ``subprocess``, ``socket``, filesystem, or network modules.
ALLOWED_IMPORTS: frozenset[str] = frozenset(
    {
        "math",
        "cmath",
        "statistics",
        "itertools",
        "functools",
        "operator",
        "fractions",
        "decimal",
        "numbers",
        "random",
        "sympy",
    }
)


class ImportNotAllowedError(ValueError):
    """Raised when untrusted source imports a module outside the allow-list.

    Subclasses :class:`ValueError` so existing callers that catch ``ValueError``
    (and the ``test_blocks_unsafe_syntax`` contract) keep working, while the
    message carries a self-correcting hint for the model.
    """


def import_not_allowed_hint(module: str) -> str:
    """The exact ``"Import not allowed: X"`` hint (DeepMath ``agent.py``)."""
    allowed = ", ".join(sorted(ALLOWED_IMPORTS))
    return f"Import not allowed: {module}. Allowed modules: {allowed}"


def guard_imports(source: str) -> None:
    """Reject any import of a module outside :data:`ALLOWED_IMPORTS`.

    Detects ``import x`` / ``from x import ...`` statements and dynamic
    ``__import__("x")`` calls. Raises :class:`ImportNotAllowedError` (with a
    self-correcting hint) on the first disallowed module. A ``SyntaxError`` from
    ``ast.parse`` is left to the caller's own compile step to surface.
    """
    try:
        tree = ast.parse(source)
    except SyntaxError:
        # Not our job to report syntax errors; the real compile step will.
        return
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                root = alias.name.split(".", 1)[0]
                if root not in ALLOWED_IMPORTS:
                    raise ImportNotAllowedError(import_not_allowed_hint(root))
        elif isinstance(node, ast.ImportFrom):
            root = (node.module or "").split(".", 1)[0]
            if root and root not in ALLOWED_IMPORTS:
                raise ImportNotAllowedError(import_not_allowed_hint(root))
        elif isinstance(node, ast.Call):
            func = node.func
            if isinstance(func, ast.Name) and func.id == "__import__":
                if node.args and isinstance(node.args[0], ast.Constant) and isinstance(
                    node.args[0].value, str
                ):
                    root = node.args[0].value.split(".", 1)[0]
                    if root not in ALLOWED_IMPORTS:
                        raise ImportNotAllowedError(import_not_allowed_hint(root))
                # Bare/dynamic __import__ with a non-constant target: refuse.
                raise ImportNotAllowedError(import_not_allowed_hint("__import__(...)"))


# --- Step / token budget governor ------------------------------------------


@dataclass
class StepBudget:
    """A single total budget shared across all steps (DeepMath ``agent.py``).

    Decouples "how many turns/cases" from "how much total work". Spend one unit
    per step with :meth:`spend`; when the budget cannot cover the request the
    caller should terminate gracefully instead of continuing.
    """

    total: int
    used: int = 0

    def spend(self, amount: int = 1) -> bool:
        """Try to consume ``amount`` units. Returns ``False`` (without spending)
        when that would exceed the total, signalling graceful termination."""
        if self.used + amount > self.total:
            return False
        self.used += amount
        return True

    @property
    def remaining(self) -> int:
        return max(self.total - self.used, 0)

    @property
    def exhausted(self) -> bool:
        return self.used >= self.total


# --- Subprocess hard-kill runner -------------------------------------------

DEFAULT_TIMEOUT_SECONDS: float = 20.0

STATUS_OK = "ok"
STATUS_TIMEOUT = "timeout"
STATUS_ERROR = "error"
STATUS_IMPORT_ERROR = "import_error"


@dataclass
class SandboxResult:
    """Structured outcome of a sandboxed run."""

    status: str
    value: Any = None
    error: str | None = None
    hint: str | None = None

    @property
    def ok(self) -> bool:
        return self.status == STATUS_OK

    @property
    def timed_out(self) -> bool:
        return self.status == STATUS_TIMEOUT

    def unwrap(self) -> Any:
        """Return the value on success; otherwise raise the appropriate error.

        Preserves the pre-hardening behaviour where a bad spec propagates as an
        exception (which the JSON worker turns into ``{"ok": false, ...}``).
        """
        if self.status == STATUS_OK:
            return self.value
        if self.status == STATUS_IMPORT_ERROR:
            raise ImportNotAllowedError(self.error or "import not allowed")
        if self.status == STATUS_TIMEOUT:
            raise TimeoutError(self.error or "execution timed out")
        raise RuntimeError(self.error or "sandbox execution failed")


def _kill_process_tree(pid: int | None) -> None:
    """Best-effort kill of ``pid`` and any of its descendants.

    Windows: ``taskkill /F /T`` walks the tree. POSIX: kill the process group if
    one exists, else the pid. Never raises.
    """
    if not pid:
        return
    try:
        if sys.platform == "win32":
            subprocess.run(
                ["taskkill", "/F", "/T", "/PID", str(pid)],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            )
        else:
            try:
                os.killpg(os.getpgid(pid), signal.SIGKILL)
            except (ProcessLookupError, PermissionError, OSError):
                try:
                    os.kill(pid, signal.SIGKILL)
                except OSError:
                    pass
    except Exception:  # noqa: BLE001 - kill is strictly best-effort
        pass


def _process_wrapper(queue: "mp.Queue", func: Callable, args: tuple, kwargs: dict) -> None:
    """Child-process entry: run ``func`` and push a tagged result to ``queue``."""
    try:
        result = func(*args, **kwargs)
        queue.put(("OK", result))
    except ImportNotAllowedError as exc:
        queue.put(("IMPORT", str(exc)))
    except Exception:  # noqa: BLE001 - surface any failure as a string
        queue.put(("EXC", traceback.format_exc()))


def _run_in_thread(
    func: Callable, args: tuple, kwargs: dict, timeout: float
) -> SandboxResult:
    """Fallback for non-picklable callables: time out, but cannot force-kill."""
    box: dict[str, Any] = {}
    done = threading.Event()

    def _target() -> None:
        try:
            box["value"] = func(*args, **kwargs)
        except ImportNotAllowedError as exc:
            box["import_error"] = str(exc)
        except Exception:  # noqa: BLE001
            box["error"] = traceback.format_exc()
        finally:
            done.set()

    thread = threading.Thread(target=_target, daemon=True)
    thread.start()
    if not done.wait(timeout=timeout):
        return SandboxResult(
            status=STATUS_TIMEOUT,
            error=f"execution timed out after {timeout}s (thread fallback; not force-killed)",
        )
    if "import_error" in box:
        return SandboxResult(
            status=STATUS_IMPORT_ERROR, error=box["import_error"], hint=box["import_error"]
        )
    if "error" in box:
        return SandboxResult(status=STATUS_ERROR, error=box["error"])
    return SandboxResult(status=STATUS_OK, value=box.get("value"))


def run_in_subprocess(
    func: Callable,
    args: Iterable[Any] = (),
    kwargs: dict | None = None,
    timeout: float = DEFAULT_TIMEOUT_SECONDS,
) -> SandboxResult:
    """Run ``func(*args, **kwargs)`` under a hard timeout in a child process.

    Prefers a killable subprocess; on timeout the process is ``terminate()``-ed,
    then ``kill()``-ed, then tree-killed. Falls back to a daemon thread when the
    callable/arguments are not picklable (DeepMath's documented gotcha). Always
    returns a :class:`SandboxResult` -- never hangs, never leaks the child.
    """
    args = tuple(args)
    kwargs = kwargs or {}

    # Validate picklability early so we can choose the strong path deliberately.
    try:
        pickle.dumps((func, args, kwargs))
    except (pickle.PicklingError, AttributeError, TypeError):
        return _run_in_thread(func, args, kwargs, timeout)

    ctx = mp.get_context("spawn")
    queue: "mp.Queue" = ctx.Queue()
    proc = ctx.Process(
        target=_process_wrapper, args=(queue, func, args, kwargs), daemon=True
    )
    proc.start()
    proc.join(timeout)

    if proc.is_alive():
        proc.terminate()
        proc.join(1.0)
        if proc.is_alive():
            proc.kill()
            proc.join(1.0)
        _kill_process_tree(proc.pid)
        return SandboxResult(
            status=STATUS_TIMEOUT, error=f"execution timed out after {timeout}s"
        )

    try:
        tag, payload = queue.get(timeout=5.0)
    except Exception:  # noqa: BLE001 - queue empty / broken pipe on abnormal exit
        return SandboxResult(
            status=STATUS_ERROR,
            error="worker process exited without returning a result",
        )

    if tag == "OK":
        return SandboxResult(status=STATUS_OK, value=payload)
    if tag == "IMPORT":
        return SandboxResult(status=STATUS_IMPORT_ERROR, error=payload, hint=payload)
    return SandboxResult(status=STATUS_ERROR, error=payload)
