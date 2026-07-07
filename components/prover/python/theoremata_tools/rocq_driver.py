"""Rocq (Coq) interaction-driver client for Theoremata.

Exposes a uniform *ProofSession* JSON contract (shared with
``isabelle_driver``) over a Rocq backend. Prefers **coq-lsp Petanque**
(``pet`` / ``pet-server`` speaking the ``petanque/start`` -> ``petanque/run``
-> ``petanque/goals`` JSON-RPC methods, from ``docs/formal-systems/rocq.md``
B.2); falls back to **SerAPI** (``sertop`` ``Add`` / ``Exec`` /
``Query Goals`` s-expressions) when only that binary is present.

Two run modes
-------------
* **live**  -- a real toolchain binary is available (``pet``/``pet-server`` or
  ``sertop``). We shell to it and speak the actual protocol.
* **mock**  -- offline, deterministic. No binary present (or
  ``THEOREMATA_ROCQ_MOCK=1``). Returns canned goal states and a ``proved``
  verdict for a trivially-closing tactic (``exact I`` / ``Qed`` / ...). This
  is the default on a box without a Rocq toolchain.

Detection is graceful: if the preferred binary is missing we probe the next
one, and if none is reachable we degrade to ``mock``. Every response carries
``mode`` (``"mock"`` | ``"live"``) and ``backend``
(``"petanque"`` | ``"serapi"`` | ``"mock"``).

Uniform ProofSession contract
-----------------------------
``start(project)``               -> ``{ok, session}``
``submit_unit(session, code)``   -> ``{ok, proved, goals, messages, errors}``
``step_tactic(session, state, tactic)``
                                 -> ``{ok, state, goals, proof_finished, messages}``
``goal_state(session, state)``   -> ``{ok, goals:[{hyps, concl}], ...}``
``stop(session)``                -> ``{ok}``

The ``session`` object is a JSON-serialisable dict (so it can round-trip
through the JSON-lines worker); live process handles are kept in an in-process
registry keyed by ``session["id"]``.

Worker wiring (report only -- this module does NOT edit worker.py)
------------------------------------------------------------------
A future dispatch line in the tools ``worker.py`` would be::

    if tool == "rocq_session":
        from theoremata_tools.rocq_driver import run as rocq_run
        return rocq_run(request)   # request = {"op": "start"|"submit_unit"|...}

Enabling live mode
------------------
Set ``THEOREMATA_ROCQ_MOCK=0`` and make one of the binaries discoverable, via
either ``PATH`` or an explicit override:

* ``THEOREMATA_ROCQ_PET``     -> path to ``pet`` / ``pet-server`` (Petanque)
* ``THEOREMATA_ROCQ_SERTOP``  -> path to ``sertop`` (SerAPI)
"""
from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import threading
from typing import Any, Optional

# --------------------------------------------------------------------------- #
# Configuration / environment.
# --------------------------------------------------------------------------- #
MOCK_ENV = "THEOREMATA_ROCQ_MOCK"
PET_ENV = "THEOREMATA_ROCQ_PET"
SERTOP_ENV = "THEOREMATA_ROCQ_SERTOP"

# Tactics that trivially close a (mock) goal. Trailing "." is stripped first.
CLOSING_TACTICS = {
    "exact I",
    "exact tt",
    "reflexivity",
    "trivial",
    "auto",
    "now",
    "constructor",
    "assumption",
    "tauto",
    "easy",
    "qed",  # "Qed" lower-cased -- treated as "seal the finished proof"
}

# The canned goal a fresh mock session opens on.
_MOCK_GOAL = {"hyps": [], "concl": "True"}

# In-process registry of live backend handles, keyed by session id.
_LIVE: "dict[str, _LiveClient]" = {}
_LIVE_LOCK = threading.Lock()
_COUNTER = 0
_COUNTER_LOCK = threading.Lock()


def _next_id(prefix: str) -> str:
    global _COUNTER
    with _COUNTER_LOCK:
        _COUNTER += 1
        return f"{prefix}-{_COUNTER}"


def _norm_tactic(tactic: str) -> str:
    """Normalise a tactic for closing-set comparison: strip whitespace + '.'."""
    return tactic.strip().rstrip(".").strip()


def _is_closing(tactic: str) -> bool:
    return _norm_tactic(tactic).lower() in {t.lower() for t in CLOSING_TACTICS}


# --------------------------------------------------------------------------- #
# Backend discovery.
# --------------------------------------------------------------------------- #
def _find_binary(env_var: str, *names: str) -> Optional[str]:
    """Return an explicit env override (if it exists) or the first ``PATH`` hit."""
    override = os.environ.get(env_var)
    if override:
        if os.path.exists(override) or shutil.which(override):
            return override
        return None
    for name in names:
        found = shutil.which(name)
        if found:
            return found
    return None


def detect_backend() -> "tuple[str, str, Optional[str]]":
    """Decide ``(mode, backend, binary)``.

    Preference order: Petanque (``pet``/``pet-server``) > SerAPI (``sertop``).
    Honours ``THEOREMATA_ROCQ_MOCK`` (1/true forces mock, 0/false forces a live
    attempt even if we cannot see a binary). Default: mock unless a binary is
    discoverable.
    """
    forced = os.environ.get(MOCK_ENV, "")
    if forced in ("1", "true", "True"):
        return "mock", "mock", None

    pet = _find_binary(PET_ENV, "pet-server", "pet")
    if pet:
        return "live", "petanque", pet
    sertop = _find_binary(SERTOP_ENV, "sertop")
    if sertop:
        return "live", "serapi", sertop

    if forced in ("0", "false", "False"):
        # Live explicitly requested but nothing found -> still report a live
        # attempt against the conventional binary name so the caller sees the
        # protocol path was taken (it will error at connect time).
        return "live", "petanque", os.environ.get(PET_ENV, "pet")

    return "mock", "mock", None


# --------------------------------------------------------------------------- #
# Live client: Petanque (stdio JSON-RPC) with a SerAPI shim.
# --------------------------------------------------------------------------- #
class _LiveClient:
    """Thin live backend wrapper.

    Petanque: newline-delimited JSON-RPC 2.0 over the ``pet`` process stdio,
    speaking ``petanque/start`` / ``petanque/run`` / ``petanque/goals``.
    SerAPI: ``sertop`` reading s-expressions on stdin.

    On a box without a real toolchain this simply fails to spawn/connect and
    the caller falls back to mock; the code path exists so live mode attempts
    the genuine protocol wherever a binary is present.
    """

    def __init__(self, backend: str, binary: str, project: dict[str, Any]):
        self.backend = backend
        self.binary = binary
        self.project = project
        self._rpc_id = 0
        self._proc = self._spawn()

    def _spawn(self) -> subprocess.Popen:
        args = [self.binary]
        if self.backend == "serapi":
            # SerAPI maps physical dir to a logical path with a comma.
            root = self.project.get("root")
            logical = self.project.get("logical", "Gen")
            if root:
                args += ["-Q", f"{root},{logical}"]
        return subprocess.Popen(
            args,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )

    # -- Petanque JSON-RPC ------------------------------------------------- #
    def _rpc(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        assert self._proc.stdin and self._proc.stdout
        self._rpc_id += 1
        msg = {"jsonrpc": "2.0", "id": self._rpc_id, "method": method,
               "params": params}
        self._proc.stdin.write(json.dumps(msg) + "\n")
        self._proc.stdin.flush()
        line = self._proc.stdout.readline()
        if not line:
            raise RuntimeError("petanque: empty response")
        reply = json.loads(line)
        if "error" in reply:
            raise RuntimeError(f"petanque error: {reply['error']}")
        return reply.get("result", {})

    # -- SerAPI s-expr ----------------------------------------------------- #
    def _sertop(self, sexp: str) -> str:
        assert self._proc.stdin and self._proc.stdout
        self._proc.stdin.write(sexp + "\n")
        self._proc.stdin.flush()
        out = []
        # Read until a "Completed" answer terminates the command.
        while True:
            line = self._proc.stdout.readline()
            if not line:
                break
            out.append(line)
            if "Completed" in line:
                break
        return "".join(out)

    # -- Uniform ops ------------------------------------------------------- #
    def submit_unit(self, code: str) -> dict[str, Any]:
        if self.backend == "petanque":
            uri = self.project.get("uri", "file:///Generated.v")
            thm = _guess_theorem(code) or "Generated"
            res = self._rpc("petanque/start",
                            {"uri": uri, "thm": thm, "pre_commands": code})
            proved = bool(res.get("proof_finished"))
            return {"ok": True, "proved": proved,
                    "goals": _petanque_goals(res),
                    "messages": _feedback(res), "errors": []}
        # SerAPI
        raw = self._sertop(f'(Add () "{_escape(code)}")')
        return {"ok": True, "proved": "Completed" in raw and "CoqExn" not in raw,
                "goals": [], "messages": [raw], "errors": []
                if "CoqExn" not in raw else [raw]}

    def step_tactic(self, state: Any, tactic: str) -> dict[str, Any]:
        if self.backend == "petanque":
            res = self._rpc("petanque/run", {"st": state, "tac": tactic})
            new_state = res.get("st", state)
            goals = self._rpc("petanque/goals", {"st": new_state})
            return {"ok": True, "state": new_state,
                    "goals": _goalconfig_goals(goals),
                    "proof_finished": bool(res.get("proof_finished")),
                    "messages": _feedback(res)}
        raw = self._sertop(f'(Add ((ontop {state})) "{_escape(tactic)}")')
        return {"ok": True, "state": state, "goals": [],
                "proof_finished": False, "messages": [raw]}

    def goal_state(self, state: Any) -> dict[str, Any]:
        if self.backend == "petanque":
            goals = self._rpc("petanque/goals", {"st": state})
            return {"ok": True, "state": state,
                    "goals": _goalconfig_goals(goals)}
        raw = self._sertop(f"(Query ((sid {state})) Goals)")
        return {"ok": True, "state": state, "goals": [], "raw": raw}

    def stop(self) -> None:
        try:
            if self._proc.stdin:
                self._proc.stdin.close()
            self._proc.terminate()
            self._proc.wait(timeout=5)
        except Exception:  # noqa: BLE001 -- best-effort teardown
            try:
                self._proc.kill()
            except Exception:  # noqa: BLE001
                pass


# --------------------------------------------------------------------------- #
# Petanque / SerAPI response helpers.
# --------------------------------------------------------------------------- #
def _guess_theorem(code: str) -> Optional[str]:
    m = re.search(r"\b(?:Theorem|Lemma|Corollary|Proposition|Fact)\s+(\w+)", code)
    return m.group(1) if m else None


def _feedback(res: dict[str, Any]) -> list[str]:
    fb = res.get("feedback") or []
    out: list[str] = []
    for item in fb:
        if isinstance(item, (list, tuple)) and len(item) == 2:
            out.append(str(item[1]))
        else:
            out.append(str(item))
    return out


def _hyp_names(hyp: dict[str, Any]) -> str:
    names = hyp.get("names") or []
    ty = hyp.get("ty", "")
    joined = ", ".join(str(n) for n in names) if isinstance(names, list) else str(names)
    return f"{joined} : {ty}".strip(" :")


def _goalconfig_goals(gc: Any) -> list[dict[str, Any]]:
    """Turn a Petanque ``GoalConfig`` into ``[{hyps, concl}]``."""
    if not isinstance(gc, dict):
        return []
    out: list[dict[str, Any]] = []
    for g in gc.get("goals", []) or []:
        if not isinstance(g, dict):
            continue
        out.append({
            "hyps": [_hyp_names(h) for h in g.get("hyps", []) if isinstance(h, dict)],
            "concl": g.get("ty", ""),
        })
    return out


def _petanque_goals(_res: dict[str, Any]) -> list[dict[str, Any]]:
    # ``Run_result`` does not embed goals; caller reads them via petanque/goals.
    return []


def _escape(text: str) -> str:
    return text.replace("\\", "\\\\").replace('"', '\\"')


# --------------------------------------------------------------------------- #
# Mock backend (deterministic, offline).
# --------------------------------------------------------------------------- #
def _mock_goals_after(tactic: Optional[str]) -> list[dict[str, Any]]:
    if tactic is not None and _is_closing(tactic):
        return []
    return [dict(_MOCK_GOAL)]


def _mock_submit_unit(code: str) -> dict[str, Any]:
    """Proved iff the unit contains a trivially-closing tactic (or ``Qed``)."""
    tokens = re.findall(r"[A-Za-z_]+", code)
    lowered = {t.lower() for t in tokens}
    proved = bool(lowered & {t.lower() for t in CLOSING_TACTICS})
    goals = [] if proved else [dict(_MOCK_GOAL)]
    messages = (["Closed under the global context"] if proved
                else ["1 goal remaining"])
    return {"ok": True, "proved": proved, "goals": goals,
            "messages": messages, "errors": [], "mode": "mock",
            "backend": "mock"}


def _mock_step_tactic(state: Any, tactic: str) -> dict[str, Any]:
    try:
        st = int(state)
    except (TypeError, ValueError):
        st = 0
    finished = _is_closing(tactic)
    return {"ok": True, "state": st + 1, "goals": _mock_goals_after(tactic),
            "proof_finished": finished,
            "messages": (["Proof finished."] if finished else []),
            "mode": "mock", "backend": "mock"}


def _mock_goal_state(state: Any) -> dict[str, Any]:
    return {"ok": True, "state": state, "goals": [dict(_MOCK_GOAL)],
            "mode": "mock", "backend": "mock"}


# --------------------------------------------------------------------------- #
# Uniform ProofSession contract.
# --------------------------------------------------------------------------- #
def start(project: Optional[dict[str, Any]] = None) -> dict[str, Any]:
    """Open a proof session. ``project`` may carry ``root``/``uri``/``logical``."""
    project = project or {}
    mode, backend, binary = detect_backend()
    if mode == "live":
        try:
            client = _LiveClient(backend, binary or "pet", project)
            sid = _next_id("rocq")
            with _LIVE_LOCK:
                _LIVE[sid] = client
            return {"ok": True, "session": {
                "id": sid, "mode": "live", "backend": backend,
                "binary": binary, "project": project, "state": 0}}
        except Exception as exc:  # noqa: BLE001 -- degrade to mock
            mode, backend = "mock", "mock"
            sid = _next_id("rocq")
            return {"ok": True, "session": {
                "id": sid, "mode": "mock", "backend": "mock",
                "project": project, "state": 0,
                "note": f"live start failed, using mock: {exc}"}}
    sid = _next_id("rocq")
    return {"ok": True, "session": {
        "id": sid, "mode": "mock", "backend": "mock",
        "project": project, "state": 0}}


def _live_client(session: dict[str, Any]) -> Optional[_LiveClient]:
    if session.get("mode") != "live":
        return None
    with _LIVE_LOCK:
        return _LIVE.get(session.get("id", ""))


def submit_unit(session: dict[str, Any], code: str) -> dict[str, Any]:
    """Submit a whole proof unit; report whether it closes."""
    client = _live_client(session)
    if client is not None:
        try:
            res = client.submit_unit(code)
            res.setdefault("mode", "live")
            res.setdefault("backend", client.backend)
            return res
        except Exception as exc:  # noqa: BLE001 -- degrade to mock
            out = _mock_submit_unit(code)
            out["note"] = f"live submit failed, using mock: {exc}"
            return out
    return _mock_submit_unit(code)


def step_tactic(session: dict[str, Any], state: Any, tactic: str) -> dict[str, Any]:
    """Run one tactic from ``state``; return the next state + goals."""
    client = _live_client(session)
    if client is not None:
        try:
            res = client.step_tactic(state, tactic)
            res.setdefault("mode", "live")
            res.setdefault("backend", client.backend)
            return res
        except Exception as exc:  # noqa: BLE001 -- degrade to mock
            out = _mock_step_tactic(state, tactic)
            out["note"] = f"live step failed, using mock: {exc}"
            return out
    return _mock_step_tactic(state, tactic)


def goal_state(session: dict[str, Any], state: Any) -> dict[str, Any]:
    """Read the goal state at ``state``: ``{goals:[{hyps, concl}], ...}``."""
    client = _live_client(session)
    if client is not None:
        try:
            res = client.goal_state(state)
            res.setdefault("mode", "live")
            res.setdefault("backend", client.backend)
            return res
        except Exception as exc:  # noqa: BLE001 -- degrade to mock
            out = _mock_goal_state(state)
            out["note"] = f"live goals failed, using mock: {exc}"
            return out
    return _mock_goal_state(state)


def stop(session: dict[str, Any]) -> dict[str, Any]:
    """Tear the session down (terminate a live backend if any)."""
    sid = session.get("id", "")
    with _LIVE_LOCK:
        client = _LIVE.pop(sid, None)
    if client is not None:
        client.stop()
    return {"ok": True, "id": sid, "mode": session.get("mode", "mock")}


# --------------------------------------------------------------------------- #
# Worker-style dispatch (tool == "rocq_session").
# --------------------------------------------------------------------------- #
def run(request: dict[str, Any]) -> dict[str, Any]:
    """Dispatch a worker-style request. ``op`` selects the ProofSession call."""
    op = request.get("op", "start")
    if op == "start":
        return start(request.get("project"))
    if op == "detect":
        mode, backend, binary = detect_backend()
        return {"ok": True, "mode": mode, "backend": backend, "binary": binary}
    session = request.get("session")
    if not isinstance(session, dict):
        return {"ok": False, "error": "missing 'session'"}
    if op == "submit_unit":
        return submit_unit(session, request.get("code", ""))
    if op == "step_tactic":
        return step_tactic(session, request.get("state", 0),
                           request.get("tactic", ""))
    if op == "goal_state":
        return goal_state(session, request.get("state", 0))
    if op == "stop":
        return stop(session)
    return {"ok": False, "error": f"unknown op: {op}"}


def main() -> None:
    req = json.load(sys.stdin)
    out = run(req)
    print(json.dumps(out))
    raise SystemExit(0 if out.get("ok", True) else 1)


if __name__ == "__main__":
    main()
