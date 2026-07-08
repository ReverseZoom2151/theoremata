"""Reference client for the Aristotle MCP tool surface (Harmonic's prover).

This module is a **self-contained, offline-capable reference implementation** of
the complete protocol exposed by the ``lean-aristotle-mcp`` server, so that
Theoremata's Rust ``aristotle.rs`` backend has a single ground-truth Python
mirror to target. It is deliberately faithful to the *upstream implementation*
(``resources/lean-aristotle-mcp-main/**/src/aristotle_mcp/tools.py`` +
``mock.py`` + ``stubs/aristotlelib/__init__.pyi``), not to the aspirational
design doc (which advertises fields the real server never returns).

What it models
--------------
* **The 6 MCP tools** -- ``prove`` / ``check_proof`` / ``prove_file`` /
  ``check_prove_file`` / ``formalize`` / ``check_formalize`` -- in both sync
  (``wait=True``) and async (``wait=False`` -> ``project_id`` -> ``check_*``)
  modes.
* **The raw ``aristotlelib`` status machine** -- ``ProjectStatus`` =
  ``NOT_STARTED | QUEUED | IN_PROGRESS | COMPLETE | FAILED | PENDING_RETRY`` --
  and its normalization to our vocabulary via :func:`map_api_status`
  (``PENDING_RETRY -> in_progress``, matching upstream ``_map_api_status``).
  Every result carries BOTH the normalized ``status`` and the ``raw_status``
  (the ``ProjectStatus`` name), because a Rust backend calling ``aristotlelib``
  directly must consume the raw set.
* **``ProjectInputType``** (``FORMAL_LEAN = 2`` for ``prove``/``prove_file``,
  ``INFORMAL = 3`` for ``formalize``) -- the one switch that lets a single
  backend serve both "prove sorries" and "formalize NL -> Lean".
* **Input-size guards** -- code <= 1 MB, description <= 100 KB, file <= 10 MB.
* **Polling defaults** -- ``polling_interval_seconds=30``,
  ``max_polling_failures=3`` (the SDK floor; surfaced so our scheduler reuses
  them rather than tight-looping).
* **The ``aristotle://status`` resource** -- :meth:`AristotleMCPClient.status`.

Two execution modes
--------------------
* **mock** (default when no API key / no ``aristotlelib`` is importable):
  deterministic, offline, keyword-triggered exactly like the upstream server --
  code containing ``false_theorem``/``bad_lemma`` -> counterexample;
  ``timeout``/``hard`` -> failed; a filename containing ``partial``/``fail`` ->
  partial/failed; ``formalize`` keys off ``even``/``prime``/``commut``. Async
  polls walk the **raw enum** ``NOT_STARTED -> QUEUED -> IN_PROGRESS ->
  COMPLETE`` (or ``FAILED``) so callers can exercise the full state machine
  without a network.
* **live**: mirrors ``tools.py`` against the real ``aristotlelib`` SDK
  (``Project.create``/``solve``/``wait_for_completion``/``get_solution`` and
  ``Project.prove_from_file`` with ``project_input_type`` +
  ``formal_input_context``; async recovers the missing ``project_id`` via
  ``list_projects`` -- a documented upstream race). If ``aristotlelib`` cannot
  be imported the client degrades **gracefully** to an ``error`` result
  (``ok=False``, ``mode="live-unavailable"``) and NEVER raises or hits the
  network -- which is why the test-suite is fully offline.

Worker wiring (report only -- this module does NOT edit worker.py)
------------------------------------------------------------------
Dispatch key ``"aristotle_mcp"``::

    if tool == "aristotle_mcp":
        from theoremata_tools.aristotle_mcp_client import run as aristotle_run
        return aristotle_run(request)

with e.g. ``request = {"tool": "aristotle_mcp", "op": "prove",
"code": "theorem t : ... := by sorry", "wait": false, "mock": true}``. See
:func:`run`.

Uses the Python standard library only. ``aristotlelib`` is an *optional* live
dependency imported lazily inside the guarded live path.
"""
from __future__ import annotations

import os
import threading
import uuid
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Optional

__all__ = [
    "ProjectStatus",
    "ProjectInputType",
    "AristotleResult",
    "AristotleMCPClient",
    "map_api_status",
    "run",
    "MAX_CODE_SIZE",
    "MAX_DESCRIPTION_SIZE",
    "MAX_FILE_SIZE",
    "DEFAULT_POLLING_INTERVAL_SECONDS",
    "DEFAULT_MAX_POLLING_FAILURES",
]


# --------------------------------------------------------------------------- #
# Raw SDK enums (mirrors aristotlelib 0.6.x -- stubs/aristotlelib/__init__.pyi).
# --------------------------------------------------------------------------- #
class ProjectStatus(Enum):
    """Raw ``aristotlelib`` project status (the values the API actually returns).

    A Rust backend calling the SDK directly consumes THIS set; the MCP wrapper's
    friendlier vocabulary ("proved"/"queued"/...) is a normalization of it (see
    :func:`map_api_status`).
    """

    NOT_STARTED = "NOT_STARTED"
    QUEUED = "QUEUED"
    IN_PROGRESS = "IN_PROGRESS"
    COMPLETE = "COMPLETE"
    FAILED = "FAILED"
    PENDING_RETRY = "PENDING_RETRY"


class ProjectInputType(Enum):
    """Input kind dispatched to the API. ``FORMAL_LEAN`` for prove/prove_file,
    ``INFORMAL`` for formalize (NL -> Lean). Integer values match the SDK."""

    FORMAL_LEAN = 2
    INFORMAL = 3


# --------------------------------------------------------------------------- #
# Constants (defense-in-depth guards + polling floor -- from tools.py / the SDK).
# --------------------------------------------------------------------------- #
MAX_CODE_SIZE = 1_000_000  # 1 MB for `prove` code input
MAX_DESCRIPTION_SIZE = 100_000  # 100 KB for `formalize` NL descriptions
MAX_FILE_SIZE = 10_000_000  # 10 MB for `prove_file` file inputs

DEFAULT_POLLING_INTERVAL_SECONDS = 30  # SDK default; a good floor for a scheduler
DEFAULT_MAX_POLLING_FAILURES = 3  # SDK default before giving up on a poll

# Mock trigger keywords -- the deterministic-fixture convention shared with the
# upstream mock server (mock.py). Reused verbatim so fixtures port both ways.
_COUNTEREXAMPLE_MARKERS = ("false_theorem", "bad_lemma")
_FAILURE_MARKERS = ("timeout", "hard")

# The raw-status progression a mock async job walks on successive `check_*`
# polls (index 0 == "already submitted, not yet polled"). The final state is
# substituted per-job (COMPLETE vs FAILED) from the job's decided outcome.
_MOCK_POLL_PROGRESSION = (
    ProjectStatus.NOT_STARTED,
    ProjectStatus.QUEUED,
    ProjectStatus.IN_PROGRESS,
)


# --------------------------------------------------------------------------- #
# Status normalization (mirrors tools.py::_map_api_status).
# --------------------------------------------------------------------------- #
def map_api_status(status: "ProjectStatus | str", percent_complete: Optional[int]) -> tuple[str, str]:
    """Map a raw :class:`ProjectStatus` to ``(normalized_status, message)``.

    Faithful to upstream ``_map_api_status``: ``PENDING_RETRY`` normalizes to
    ``in_progress``; ``QUEUED``/``NOT_STARTED`` collapse to ``queued``. Accepts
    either a :class:`ProjectStatus` or its raw name string.
    """
    name = status.name if isinstance(status, ProjectStatus) else str(status).upper()
    pct = percent_complete or 0
    if name == "COMPLETE":
        return "complete", "Proof completed"
    if name in ("QUEUED", "NOT_STARTED"):
        return "queued", "Proof is queued, waiting to start"
    if name == "IN_PROGRESS":
        return "in_progress", f"Proof is being computed ({pct}% complete)"
    if name == "PENDING_RETRY":
        return "in_progress", "Proof is pending retry"
    if name == "FAILED":
        return "failed", "Proof failed"
    return "in_progress", f"Status: {name}"


# --------------------------------------------------------------------------- #
# Unified result type.
# --------------------------------------------------------------------------- #
@dataclass
class AristotleResult:
    """Uniform result across all six tools.

    ``status`` is the normalized vocabulary
    (``submitted | queued | in_progress | proved | formalized | partial |
    counterexample | failed | error``); ``raw_status`` is the underlying
    :class:`ProjectStatus` name when one is known (``None`` for pre-submission
    or pure client-side errors). ``input_type`` records the
    :class:`ProjectInputType` that dispatched the call.
    """

    tool: str
    status: str
    raw_status: Optional[str] = None
    project_id: Optional[str] = None
    percent_complete: Optional[int] = None
    code: Optional[str] = None
    lean_code: Optional[str] = None
    counterexample: Optional[str] = None
    output_path: Optional[str] = None
    input_type: Optional[str] = None
    message: str = ""
    mode: str = "mock"
    ok: bool = True

    def to_dict(self) -> dict[str, Any]:
        """JSON-serializable dict; omits ``None`` optional fields (matching the
        upstream ``to_dict`` shape) but always keeps status/message/tool/ok."""
        out: dict[str, Any] = {
            "ok": self.ok,
            "tool": self.tool,
            "status": self.status,
            "message": self.message,
            "mode": self.mode,
        }
        for key in (
            "raw_status",
            "project_id",
            "percent_complete",
            "code",
            "lean_code",
            "counterexample",
            "output_path",
            "input_type",
        ):
            val = getattr(self, key)
            if val is not None:
                out[key] = val
        return out


# --------------------------------------------------------------------------- #
# Mock job store.
# --------------------------------------------------------------------------- #
@dataclass
class _MockJob:
    tool: str
    final_status: ProjectStatus  # COMPLETE or FAILED
    outcome: str  # normalized terminal status: proved | counterexample | formalized | partial | failed
    poll_count: int = 0
    code: Optional[str] = None
    lean_code: Optional[str] = None
    counterexample: Optional[str] = None
    output_path: Optional[str] = None
    input_type: Optional[str] = None
    message: str = ""


# --------------------------------------------------------------------------- #
# Client.
# --------------------------------------------------------------------------- #
class AristotleMCPClient:
    """Offline-capable reference client for the Aristotle MCP tool surface.

    Parameters
    ----------
    mock:
        ``True`` forces the deterministic offline mock; ``False`` forces the
        live ``aristotlelib`` path (which degrades gracefully if the SDK is
        absent). ``None`` (default) auto-selects: mock unless a live path is
        actually usable (``ARISTOTLE_API_KEY`` set AND ``aristotlelib``
        importable). ``ARISTOTLE_MOCK=true`` in the environment also forces mock.
    api_key:
        Overrides ``ARISTOTLE_API_KEY`` for the live path.
    polling_interval_seconds / max_polling_failures:
        Passed through to the live SDK; exposed so a scheduler reuses the SDK
        floor instead of tight-looping.
    """

    def __init__(
        self,
        mock: Optional[bool] = None,
        *,
        api_key: Optional[str] = None,
        polling_interval_seconds: int = DEFAULT_POLLING_INTERVAL_SECONDS,
        max_polling_failures: int = DEFAULT_MAX_POLLING_FAILURES,
    ) -> None:
        self._api_key = api_key if api_key is not None else os.environ.get("ARISTOTLE_API_KEY")
        self.polling_interval_seconds = polling_interval_seconds
        self.max_polling_failures = max_polling_failures
        self._mock = self._resolve_mock(mock)
        self._lock = threading.Lock()
        self._jobs: dict[str, _MockJob] = {}

    # -- mode / status resolution ------------------------------------------- #
    def _resolve_mock(self, requested: Optional[bool]) -> bool:
        if requested is not None:
            return requested
        if os.environ.get("ARISTOTLE_MOCK", "").lower() in ("true", "1", "yes"):
            return True
        # Auto: live only if a key is present AND the SDK is importable.
        return not (bool(self._api_key) and _aristotlelib_available())

    @property
    def mock_mode(self) -> bool:
        return self._mock

    def has_api_key(self) -> bool:
        return bool(self._api_key)

    def status(self) -> dict[str, Any]:
        """Mirror of the ``aristotle://status`` MCP resource."""
        ready = self._mock or self.has_api_key()
        if not ready:
            message = (
                "Not configured. Set ARISTOTLE_API_KEY (https://aristotle.harmonic.fun/) "
                "or pass mock=True for offline use."
            )
        elif self._mock:
            message = "Running in mock mode (no API calls)"
        else:
            message = "Ready to call Aristotle API"
        return {
            "mock_mode": self._mock,
            "api_key_configured": self.has_api_key(),
            "ready": ready,
            "message": message,
        }

    # -- tool: prove -------------------------------------------------------- #
    def prove(
        self,
        code: str,
        *,
        context_files: Optional[list[str]] = None,
        hint: Optional[str] = None,
        wait: bool = True,
    ) -> AristotleResult:
        """Fill ``sorry`` statements in Lean 4 ``code`` (input type FORMAL_LEAN)."""
        if len(code) > MAX_CODE_SIZE:
            return self._guard_error(
                "prove", f"Code exceeds maximum size of {MAX_CODE_SIZE} bytes."
            )
        if self._mock:
            return self._mock_prove(code, wait=wait)
        return self._live_prove(code, context_files, hint, wait)

    # -- tool: check_proof -------------------------------------------------- #
    def check_proof(self, project_id: str) -> AristotleResult:
        """Poll an async ``prove`` submission."""
        if self._mock:
            return self._mock_poll("prove", "check_proof", project_id)
        return self._live_check("check_proof", project_id)

    # -- tool: prove_file --------------------------------------------------- #
    def prove_file(
        self,
        file_path: str,
        *,
        output_path: Optional[str] = None,
        wait: bool = True,
    ) -> AristotleResult:
        """Prove every ``sorry`` in a Lean file (auto import resolution, FORMAL_LEAN)."""
        if not os.path.exists(file_path):
            return self._guard_error("prove_file", f"File not found: {file_path}")
        try:
            file_size = os.path.getsize(file_path)
        except OSError as exc:
            return self._guard_error("prove_file", f"Could not stat file: {exc}")
        if file_size > MAX_FILE_SIZE:
            return self._guard_error(
                "prove_file", f"File exceeds maximum size of {MAX_FILE_SIZE} bytes."
            )
        actual_output = output_path
        if actual_output is None:
            base, ext = os.path.splitext(file_path)
            actual_output = f"{base}_aristotle{ext}"
        if self._mock:
            return self._mock_prove_file(file_path, actual_output, wait=wait)
        return self._live_prove_file(file_path, actual_output, wait)

    # -- tool: check_prove_file --------------------------------------------- #
    def check_prove_file(
        self,
        project_id: str,
        *,
        output_path: Optional[str] = None,
        save: bool = False,
    ) -> AristotleResult:
        """Poll an async ``prove_file`` submission (``save=True`` to write output)."""
        if self._mock:
            return self._mock_poll(
                "prove_file", "check_prove_file", project_id, output_path=output_path, save=save
            )
        return self._live_check("check_prove_file", project_id)

    # -- tool: formalize ---------------------------------------------------- #
    def formalize(
        self,
        description: str,
        *,
        prove: bool = False,
        context_file: Optional[str] = None,
        wait: bool = True,
    ) -> AristotleResult:
        """Convert NL math ``description`` to Lean 4 (input type INFORMAL)."""
        if len(description) > MAX_DESCRIPTION_SIZE:
            return self._guard_error(
                "formalize",
                f"Description exceeds maximum size of {MAX_DESCRIPTION_SIZE} bytes.",
            )
        if self._mock:
            return self._mock_formalize(description, prove, context_file, wait=wait)
        return self._live_formalize(description, prove, context_file, wait)

    # -- tool: check_formalize ---------------------------------------------- #
    def check_formalize(self, project_id: str) -> AristotleResult:
        """Poll an async ``formalize`` submission."""
        if self._mock:
            return self._mock_poll("formalize", "check_formalize", project_id)
        return self._live_check("check_formalize", project_id)

    # ------------------------------------------------------------------ #
    # Helpers.
    # ------------------------------------------------------------------ #
    def _guard_error(self, tool: str, message: str) -> AristotleResult:
        return AristotleResult(
            tool=tool, status="error", message=message, mode="mock" if self._mock else "live", ok=False
        )

    # ------------------------------------------------------------------ #
    # Mock backends (deterministic, offline).
    # ------------------------------------------------------------------ #
    def _decide_prove(self, code: str) -> tuple[ProjectStatus, str, dict[str, Any]]:
        low = code.lower()
        if any(m in low for m in _COUNTEREXAMPLE_MARKERS):
            return (
                ProjectStatus.FAILED,
                "counterexample",
                {
                    "counterexample": (
                        "n = 0 provides a counterexample: the left-hand side "
                        "evaluates to 0, but the right-hand side evaluates to 1"
                    ),
                    "message": "Statement is false; counterexample found",
                },
            )
        if any(m in low for m in _FAILURE_MARKERS):
            return (
                ProjectStatus.FAILED,
                "failed",
                {
                    "message": (
                        "Could not find a proof within the time limit. "
                        "This does not mean the statement is false."
                    )
                },
            )
        return (
            ProjectStatus.COMPLETE,
            "proved",
            {
                "code": code + "\n-- Proof filled by Aristotle (mock)",
                "message": "Successfully proved",
            },
        )

    def _mock_prove(self, code: str, *, wait: bool) -> AristotleResult:
        final_status, outcome, extras = self._decide_prove(code)
        project_id = f"mock-{uuid.uuid4()}"
        job = _MockJob(
            tool="prove",
            final_status=final_status,
            outcome=outcome,
            code=extras.get("code"),
            counterexample=extras.get("counterexample"),
            input_type=ProjectInputType.FORMAL_LEAN.name,
            message=extras.get("message", ""),
        )
        if not wait:
            with self._lock:
                self._jobs[project_id] = job
            return AristotleResult(
                tool="prove",
                status="submitted",
                raw_status=ProjectStatus.NOT_STARTED.name,
                project_id=project_id,
                input_type=job.input_type,
                message="Proof submitted. Use check_proof to poll for results.",
            )
        return AristotleResult(
            tool="prove",
            status=outcome,
            raw_status=final_status.name,
            project_id=project_id,
            percent_complete=100,
            code=job.code,
            counterexample=job.counterexample,
            input_type=job.input_type,
            message=job.message,
        )

    def _mock_prove_file(self, file_path: str, output_path: str, *, wait: bool) -> AristotleResult:
        low = file_path.lower()
        if "partial" in low:
            final_status, outcome, message = ProjectStatus.COMPLETE, "partial", "Some proofs could not be completed"
        elif "fail" in low:
            final_status, outcome, message = ProjectStatus.FAILED, "failed", "Could not find proofs within the time limit"
        else:
            final_status, outcome, message = ProjectStatus.COMPLETE, "proved", "Successfully proved"
        project_id = f"mock-file-{uuid.uuid4()}"
        job = _MockJob(
            tool="prove_file",
            final_status=final_status,
            outcome=outcome,
            output_path=output_path,
            input_type=ProjectInputType.FORMAL_LEAN.name,
            message=message,
        )
        if not wait:
            with self._lock:
                self._jobs[project_id] = job
            return AristotleResult(
                tool="prove_file",
                status="submitted",
                raw_status=ProjectStatus.NOT_STARTED.name,
                project_id=project_id,
                output_path=output_path,
                input_type=job.input_type,
                message="Proof submitted. Use check_prove_file to poll for results.",
            )
        return AristotleResult(
            tool="prove_file",
            status=outcome,
            raw_status=final_status.name,
            project_id=project_id,
            percent_complete=100,
            output_path=output_path if outcome != "failed" else None,
            input_type=job.input_type,
            message=message,
        )

    def _mock_formalize(
        self, description: str, prove: bool, context_file: Optional[str], *, wait: bool
    ) -> AristotleResult:
        lean_code, outcome, message = _generate_mock_lean_code(description, prove, context_file)
        project_id = f"mock-formalize-{uuid.uuid4()}"
        job = _MockJob(
            tool="formalize",
            final_status=ProjectStatus.COMPLETE,
            outcome=outcome,
            lean_code=lean_code,
            input_type=ProjectInputType.INFORMAL.name,
            message=message,
        )
        if not wait:
            with self._lock:
                self._jobs[project_id] = job
            return AristotleResult(
                tool="formalize",
                status="submitted",
                raw_status=ProjectStatus.NOT_STARTED.name,
                project_id=project_id,
                input_type=job.input_type,
                message="Formalization submitted. Use check_formalize to poll for results.",
            )
        return AristotleResult(
            tool="formalize",
            status=outcome,
            raw_status=ProjectStatus.COMPLETE.name,
            project_id=project_id,
            percent_complete=100,
            lean_code=lean_code,
            input_type=job.input_type,
            message=message,
        )

    def _mock_poll(
        self,
        expected_tool: str,
        check_tool: str,
        project_id: str,
        *,
        output_path: Optional[str] = None,
        save: bool = False,
    ) -> AristotleResult:
        with self._lock:
            job = self._jobs.get(project_id)
            if job is None or job.tool != expected_tool:
                return AristotleResult(
                    tool=check_tool,
                    status="error",
                    project_id=project_id,
                    message=f"Unknown project ID: {project_id}",
                    ok=False,
                )
            job.poll_count += 1
            poll = job.poll_count

        # Walk the raw enum: QUEUED (poll 1) -> IN_PROGRESS (poll 2) -> final.
        if poll < len(_MOCK_POLL_PROGRESSION):
            raw = _MOCK_POLL_PROGRESSION[poll]
            norm, message = map_api_status(raw, 0 if raw != ProjectStatus.IN_PROGRESS else 50)
            return AristotleResult(
                tool=check_tool,
                status=norm,
                raw_status=raw.name,
                project_id=project_id,
                percent_complete=0 if raw != ProjectStatus.IN_PROGRESS else 50,
                input_type=job.input_type,
                message=message,
            )

        # Terminal: substitute the job's decided outcome for the raw COMPLETE/FAILED.
        result = AristotleResult(
            tool=check_tool,
            status=job.outcome,
            raw_status=job.final_status.name,
            project_id=project_id,
            percent_complete=100,
            input_type=job.input_type,
            message=job.message,
        )
        if job.tool == "prove":
            result.code = job.code
            result.counterexample = job.counterexample
        elif job.tool == "formalize":
            result.lean_code = job.lean_code
        elif job.tool == "prove_file":
            if save and job.outcome != "failed":
                result.output_path = output_path or job.output_path
            elif job.outcome == "proved":
                result.message = "Proof complete. Call again with save=True to write the solution."
        return result

    # ------------------------------------------------------------------ #
    # Live backends (mirror tools.py; guarded so the SDK is optional).
    # ------------------------------------------------------------------ #
    def _live_unavailable(self, tool: str, detail: str) -> AristotleResult:
        return AristotleResult(
            tool=tool,
            status="error",
            mode="live-unavailable",
            ok=False,
            message=(
                f"Live Aristotle path unavailable ({detail}). Install `aristotlelib` and set "
                "ARISTOTLE_API_KEY, or use mock mode. No network call was made."
            ),
        )

    def _live_prove(
        self, code: str, context_files: Optional[list[str]], hint: Optional[str], wait: bool
    ) -> AristotleResult:
        return self._live_dispatch(
            "prove",
            lambda mod: self._async_prove(mod, code, context_files, hint, wait),
        )

    def _live_prove_file(self, file_path: str, output_path: str, wait: bool) -> AristotleResult:
        return self._live_dispatch(
            "prove_file",
            lambda mod: self._async_prove_file(mod, file_path, output_path, wait),
        )

    def _live_formalize(
        self, description: str, prove: bool, context_file: Optional[str], wait: bool
    ) -> AristotleResult:
        return self._live_dispatch(
            "formalize",
            lambda mod: self._async_formalize(mod, description, prove, context_file, wait),
        )

    def _live_check(self, tool: str, project_id: str) -> AristotleResult:
        return self._live_dispatch(tool, lambda mod: self._async_check(mod, tool, project_id))

    def _live_dispatch(self, tool: str, coro_factory: Any) -> AristotleResult:
        """Run a live coroutine, degrading to a graceful error on any failure.

        Never raises and never reaches the network unless ``aristotlelib`` is
        importable (it is not in the offline test environment), so tests forcing
        ``mock=False`` observe a clean ``live-unavailable`` error.
        """
        if not self.has_api_key():
            return self._live_unavailable(tool, "no ARISTOTLE_API_KEY")
        mod = _import_aristotlelib()
        if mod is None:
            return self._live_unavailable(tool, "aristotlelib not importable")
        import asyncio

        try:
            return asyncio.run(coro_factory(mod))
        except Exception as exc:  # pragma: no cover - requires the live SDK
            return AristotleResult(
                tool=tool,
                status="error",
                mode="live",
                ok=False,
                message=_sanitize_error(exc),
            )

    async def _async_prove(
        self, mod: Any, code: str, context_files: Optional[list[str]], hint: Optional[str], wait: bool
    ) -> AristotleResult:  # pragma: no cover - requires the live SDK
        Project = mod.Project
        project = await Project.create()
        if context_files:
            await project.add_context([os.path.realpath(p) for p in context_files])
        payload = f"-- Hint: {hint}\n{code}" if hint else code
        await project.solve(input_content=payload)
        project_id = str(project.project_id)
        if not wait:
            return AristotleResult(
                tool="prove",
                status="submitted",
                raw_status=ProjectStatus.NOT_STARTED.name,
                project_id=project_id,
                input_type=ProjectInputType.FORMAL_LEAN.name,
                mode="live",
                message="Proof submitted. Use check_proof to poll for results.",
            )
        return await self._await_and_read(mod, "prove", project, project_id)

    async def _async_prove_file(
        self, mod: Any, file_path: str, output_path: str, wait: bool
    ) -> AristotleResult:  # pragma: no cover - requires the live SDK
        Project = mod.Project
        ProjectStatusLive = mod.ProjectStatus
        result_path = await Project.prove_from_file(
            input_file_path=os.path.realpath(file_path),
            output_file_path=os.path.realpath(output_path),
            auto_add_imports=True,
            wait_for_completion=wait,
            polling_interval_seconds=self.polling_interval_seconds,
            max_polling_failures=self.max_polling_failures,
        )
        if not wait:
            # prove_from_file returns no project_id -> recover via list_projects
            # (documented upstream race condition).
            projects, _ = await Project.list_projects(
                limit=5,
                status=[
                    ProjectStatusLive.QUEUED,
                    ProjectStatusLive.IN_PROGRESS,
                    ProjectStatusLive.NOT_STARTED,
                ],
            )
            if not projects:
                return AristotleResult(
                    tool="prove_file", status="error", mode="live", ok=False,
                    message="Could not find submitted project",
                )
            return AristotleResult(
                tool="prove_file",
                status="submitted",
                raw_status=ProjectStatus.NOT_STARTED.name,
                project_id=str(projects[0].project_id),
                output_path=os.path.realpath(output_path),
                input_type=ProjectInputType.FORMAL_LEAN.name,
                mode="live",
                message="Proof submitted. Use check_prove_file to poll for results.",
            )
        proved = bool(result_path and os.path.exists(result_path))
        return AristotleResult(
            tool="prove_file",
            status="proved" if proved else "failed",
            raw_status=ProjectStatus.COMPLETE.name if proved else ProjectStatus.FAILED.name,
            output_path=os.path.abspath(result_path) if proved else None,
            percent_complete=100,
            input_type=ProjectInputType.FORMAL_LEAN.name,
            mode="live",
            message="Proof completed successfully" if proved else "Completed but solution file not found",
        )

    async def _async_formalize(
        self, mod: Any, description: str, prove: bool, context_file: Optional[str], wait: bool
    ) -> AristotleResult:  # pragma: no cover - requires the live SDK
        import tempfile

        Project = mod.Project
        ProjectInputTypeLive = mod.ProjectInputType
        with tempfile.NamedTemporaryFile(mode="w", suffix=".txt", delete=False) as fh:
            fh.write(description)
            temp_path = fh.name
        try:
            result_path = await Project.prove_from_file(
                input_file_path=temp_path,
                project_input_type=ProjectInputTypeLive.INFORMAL,
                formal_input_context=os.path.realpath(context_file) if context_file else None,
                wait_for_completion=wait,
            )
        finally:
            if wait and os.path.exists(temp_path):
                os.unlink(temp_path)
        if result_path and os.path.exists(result_path):
            with open(result_path, encoding="utf-8") as fh:
                lean_code = fh.read()
            outcome = "proved" if prove else "formalized"
            return AristotleResult(
                tool="formalize",
                status=outcome,
                raw_status=ProjectStatus.COMPLETE.name,
                lean_code=lean_code,
                percent_complete=100,
                input_type=ProjectInputType.INFORMAL.name,
                mode="live",
                message="Successfully formalized and proved" if prove else "Successfully formalized",
            )
        return AristotleResult(
            tool="formalize", status="failed", mode="live",
            input_type=ProjectInputType.INFORMAL.name,
            message="Could not formalize the statement",
        )

    async def _async_check(self, mod: Any, tool: str, project_id: str) -> AristotleResult:  # pragma: no cover
        Project = mod.Project
        project = await Project.from_id(project_id)
        await project.refresh()
        raw = project.status
        raw_name = raw.name if hasattr(raw, "name") else str(raw).upper()
        norm, message = map_api_status(raw_name, project.percent_complete)
        result_tool = tool
        if norm == "complete":
            return await self._await_and_read(mod, tool, project, project_id, already_complete=True)
        return AristotleResult(
            tool=result_tool,
            status=norm,
            raw_status=raw_name,
            project_id=project_id,
            percent_complete=project.percent_complete if norm != "queued" else 0,
            mode="live",
            message=message,
        )

    async def _await_and_read(
        self, mod: Any, tool: str, project: Any, project_id: str, *, already_complete: bool = False
    ) -> AristotleResult:  # pragma: no cover - requires the live SDK
        import tempfile

        with tempfile.NamedTemporaryFile(mode="w", suffix=".lean", delete=False) as fh:
            out_path = fh.name
        try:
            if already_complete:
                sol = await project.get_solution(output_path=out_path)
            else:
                sol = await project.wait_for_completion(output_file_path=out_path)
            if sol and os.path.exists(str(sol)):
                with open(str(sol), encoding="utf-8") as fh:
                    text = fh.read()
                is_formalize = tool in ("formalize", "check_formalize")
                return AristotleResult(
                    tool=tool,
                    status="formalized" if is_formalize else "proved",
                    raw_status=ProjectStatus.COMPLETE.name,
                    project_id=project_id,
                    percent_complete=100,
                    lean_code=text if is_formalize else None,
                    code=None if is_formalize else text,
                    mode="live",
                    message="Completed successfully",
                )
        finally:
            if os.path.exists(out_path):
                os.unlink(out_path)
        return AristotleResult(
            tool=tool, status="failed", project_id=project_id, mode="live",
            message="Completed but no solution available",
        )


# --------------------------------------------------------------------------- #
# Mock Lean codegen (mirrors mock.py::_generate_mock_lean_code).
# --------------------------------------------------------------------------- #
def _generate_mock_lean_code(
    description: str, prove: bool, context_file: Optional[str]
) -> tuple[str, str, str]:
    header = ""
    if context_file:
        name = os.path.splitext(os.path.basename(context_file))[0]
        header = f"-- Using context from: {context_file}\nimport {name}\n\n"
    low = description.lower()
    if "even" in low and "sum" in low:
        body = (
            "def even (n : Nat) : Prop := ∃ k, n = 2 * k\n\n"
            "theorem sum_of_evens (a b : Nat) (ha : even a) (hb : even b) : even (a + b) := by\n"
            + ("  obtain ⟨ka, hka⟩ := ha; obtain ⟨kb, hkb⟩ := hb; exact ⟨ka + kb, by omega⟩" if prove else "  sorry")
        )
    elif "prime" in low:
        body = (
            "def prime (n : Nat) : Prop := n > 1 ∧ ∀ m, m ∣ n → m = 1 ∨ m = n\n\n"
            "theorem prime_example : prime 7 := by\n" + ("  decide" if prove else "  sorry")
        )
    elif "commut" in low:
        body = "theorem add_comm_example (a b : Nat) : a + b = b + a := by\n" + ("  ring" if prove else "  sorry")
    else:
        body = f"-- Formalization of: {description}\ntheorem statement : True := by\n" + ("  trivial" if prove else "  sorry")
    lean_code = header + body
    status = "proved" if prove else "formalized"
    message = "Formalized and proved" if prove else "Successfully formalized to Lean 4"
    if context_file:
        message += f" (using context from {context_file})"
    return lean_code, status, message


# --------------------------------------------------------------------------- #
# Live SDK import guard + error sanitization.
# --------------------------------------------------------------------------- #
def _import_aristotlelib() -> Any:
    try:
        import aristotlelib  # type: ignore

        return aristotlelib
    except Exception:
        return None


def _aristotlelib_available() -> bool:
    return _import_aristotlelib() is not None


def _sanitize_error(error: Exception) -> str:  # pragma: no cover - live path
    text = str(error).lower()
    if "counterexample" in text:
        return str(error)
    if "timeout" in text or "timed out" in text:
        return "Request timed out. Please try again."
    if "unauthorized" in text or "authentication" in text:
        return "Authentication failed. Please check your API key."
    if "rate limit" in text or "too many" in text:
        return "Rate limit exceeded. Please wait before retrying."
    if "connection" in text or "network" in text:
        return "Connection error. Please check your network and try again."
    return "An error occurred while processing your request."


# --------------------------------------------------------------------------- #
# Worker-style dispatch (report-only wiring: tool == "aristotle_mcp").
# --------------------------------------------------------------------------- #
def run(request: dict[str, Any]) -> dict[str, Any]:
    """Dispatch a worker-style ``{"tool": "aristotle_mcp", "op": ..., ...}`` request.

    ``op`` selects the tool: ``prove | check_proof | prove_file | check_prove_file
    | formalize | check_formalize | status``. ``mock`` (bool) forces the mode;
    omit it to auto-select. Returns the result's ``to_dict()`` (``status`` for
    the ``status`` op returns the resource dict).
    """
    op = request.get("op", request.get("operation", "prove"))
    client = AristotleMCPClient(
        mock=request.get("mock"),
        api_key=request.get("api_key"),
        polling_interval_seconds=int(
            request.get("polling_interval_seconds", DEFAULT_POLLING_INTERVAL_SECONDS)
        ),
        max_polling_failures=int(request.get("max_polling_failures", DEFAULT_MAX_POLLING_FAILURES)),
    )

    if op == "status":
        return {"ok": True, "tool": "status", **client.status()}
    if op == "prove":
        res = client.prove(
            request["code"],
            context_files=request.get("context_files"),
            hint=request.get("hint"),
            wait=request.get("wait", True),
        )
    elif op == "check_proof":
        res = client.check_proof(request["project_id"])
    elif op == "prove_file":
        res = client.prove_file(
            request["file_path"],
            output_path=request.get("output_path"),
            wait=request.get("wait", True),
        )
    elif op == "check_prove_file":
        res = client.check_prove_file(
            request["project_id"],
            output_path=request.get("output_path"),
            save=request.get("save", False),
        )
    elif op == "formalize":
        res = client.formalize(
            request["description"],
            prove=request.get("prove", False),
            context_file=request.get("context_file"),
            wait=request.get("wait", True),
        )
    elif op == "check_formalize":
        res = client.check_formalize(request["project_id"])
    else:
        raise ValueError(f"unknown aristotle_mcp op: {op!r}")
    return res.to_dict()


def main() -> None:
    import json
    import sys

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
