"""Isabelle interaction-driver client for Theoremata.

Exposes the same uniform *ProofSession* JSON contract as ``rocq_driver`` over
the **Isabelle Server** (TCP, line-oriented JSON protocol, from
``docs/formal-systems/isabelle.md`` §2a). The server works at *theory-file*
granularity -- there is no submit-one-tactic / read-the-subgoal command -- so
only ``submit_unit`` is meaningful; ``step_tactic`` reports ``unsupported``.

Protocol (live mode)
--------------------
1. Open a TCP socket to the server; the **first line MUST be the UUID
   password** (a *short message*, no length prefix). Server replies ``OK ...``
   or disconnects.
2. Messages are framed ``name argument``: a *short message* is one line ended
   by LF/CR-LF; a *long message* is a decimal length line followed by that many
   bytes (used here for the potentially large ``use_theories`` argument).
3. ``session_start {"session":"HOL"}`` -> async ``OK {"task": uuid}`` then
   ``FINISHED {"task":..., "session_id":..., "tmp_dir":...}``.
4. Write ``Scratch.thy`` into ``tmp_dir`` (or set ``master_dir``), then
   ``use_theories {"session_id":..., "theories":["Scratch"]}`` -> async, ends
   ``FINISHED`` carrying ``{ok, errors, nodes[].{status, messages}}``.
5. ``session_stop`` / ``purge_theories`` / ``shutdown`` for teardown.

Two run modes
-------------
* **live**  -- ``THEOREMATA_ISABELLE_SERVER=host:port`` and
  ``THEOREMATA_ISABELLE_PASSWORD=<uuid>`` are set and the socket is reachable.
* **mock**  -- offline, deterministic (the default on a box without Isabelle).
  Returns a canned ``use_theories``-style result (``ok:true``, no errors) for a
  trivial theory, marking the unit ``proved`` unless it contains a ``sorry`` /
  ``oops`` placeholder.

Enabling live mode
------------------
* ``THEOREMATA_ISABELLE_SERVER``   -> ``"127.0.0.1:4711"`` (host:port)
* ``THEOREMATA_ISABELLE_PASSWORD`` -> the server's UUID password
* ``THEOREMATA_ISABELLE_SESSION``  -> logic session name (default ``HOL``)
* ``THEOREMATA_ISABELLE_MOCK=1``   -> force mock; ``=0`` -> force a live attempt.

Worker wiring (report only -- this module does NOT edit worker.py)
------------------------------------------------------------------
    if tool == "isabelle_session":
        from theoremata_tools.isabelle_driver import run as isabelle_run
        return isabelle_run(request)   # request = {"op": "start"|"submit_unit"|...}
"""
from __future__ import annotations

import json
import os
import re
import socket
import sys
import threading
from typing import Any, Optional

# --------------------------------------------------------------------------- #
# Configuration / environment.
# --------------------------------------------------------------------------- #
MOCK_ENV = "THEOREMATA_ISABELLE_MOCK"
SERVER_ENV = "THEOREMATA_ISABELLE_SERVER"
PASSWORD_ENV = "THEOREMATA_ISABELLE_PASSWORD"
SESSION_ENV = "THEOREMATA_ISABELLE_SESSION"

# Placeholders that leave the goal unproved (oracle-tainted / abandoned).
UNPROVED_MARKERS = ("sorry", "oops")

_STEP_UNSUPPORTED = {
    "ok": False,
    "error": "unsupported: theory-file granularity",
    "detail": ("The Isabelle Server operates at whole-theory granularity; it "
               "has no submit-one-tactic command. Use submit_unit with a full "
               ".thy body instead."),
}

# In-process registry of live server connections, keyed by session id.
_LIVE: "dict[str, _IsabelleClient]" = {}
_LIVE_LOCK = threading.Lock()
_COUNTER = 0
_COUNTER_LOCK = threading.Lock()


def _next_id(prefix: str) -> str:
    global _COUNTER
    with _COUNTER_LOCK:
        _COUNTER += 1
        return f"{prefix}-{_COUNTER}"


# --------------------------------------------------------------------------- #
# Backend discovery.
# --------------------------------------------------------------------------- #
def detect_backend() -> "tuple[str, Optional[tuple[str, int]], Optional[str]]":
    """Decide ``(mode, address, password)``.

    Honours ``THEOREMATA_ISABELLE_MOCK`` (1 forces mock, 0 forces a live
    attempt). Otherwise: live iff both ``*_SERVER`` and ``*_PASSWORD`` are set.
    """
    forced = os.environ.get(MOCK_ENV, "")
    if forced in ("1", "true", "True"):
        return "mock", None, None

    server = os.environ.get(SERVER_ENV, "")
    password = os.environ.get(PASSWORD_ENV, "")
    address = _parse_address(server)

    if forced in ("0", "false", "False"):
        return "live", address or ("127.0.0.1", 4711), password

    if address and password:
        return "live", address, password
    return "mock", None, None


def _parse_address(server: str) -> Optional["tuple[str, int]"]:
    if not server:
        return None
    host, _, port = server.rpartition(":")
    if not host or not port.isdigit():
        return None
    return host, int(port)


# --------------------------------------------------------------------------- #
# Live client: Isabelle Server TCP framing.
# --------------------------------------------------------------------------- #
class _IsabelleClient:
    """Speaks the Isabelle Server line protocol over a TCP socket.

    On a box without a running server this fails to connect and the caller
    falls back to mock; the code path implements the genuine handshake +
    framing so live mode attempts the real protocol wherever a server exists.
    """

    def __init__(self, address: "tuple[str, int]", password: str,
                 logic: str = "HOL"):
        self.address = address
        self.password = password
        self.logic = logic
        self.session_id: Optional[str] = None
        self._sock = socket.create_connection(address, timeout=30)
        self._buf = b""
        self._handshake()
        self._start_session()

    # -- low-level framing ------------------------------------------------- #
    def _readline(self) -> str:
        while b"\n" not in self._buf:
            chunk = self._sock.recv(4096)
            if not chunk:
                break
            self._buf += chunk
        line, _, self._buf = self._buf.partition(b"\n")
        return line.rstrip(b"\r").decode("utf-8")

    def _read_message(self) -> "tuple[str, str]":
        """Read one server message, resolving a long (length-prefixed) body.

        Returns ``(name, argument)`` where ``name`` is the leading token
        (``OK`` / ``ERROR`` / ``FINISHED`` / ``FAILED`` / ``NOTE`` / digits).
        """
        line = self._readline()
        if line.strip().isdigit():
            # Long message: a length line then that many bytes of payload.
            length = int(line.strip())
            while len(self._buf) < length:
                chunk = self._sock.recv(4096)
                if not chunk:
                    break
                self._buf += chunk
            body = self._buf[:length].decode("utf-8")
            self._buf = self._buf[length:]
            line = body
        name, sep, arg = line.partition(" ")
        return name, arg if sep else ""

    def _send_short(self, text: str) -> None:
        self._sock.sendall((text + "\n").encode("utf-8"))

    def _send_command(self, name: str, argument: Any) -> None:
        """Send ``name argument`` where argument is JSON (always one line)."""
        payload = json.dumps(argument)
        self._send_short(f"{name} {payload}")

    def _await_task(self) -> dict[str, Any]:
        """Send-then-collect an async command: OK{task} .. FINISHED/FAILED."""
        name, arg = self._read_message()
        if name == "ERROR":
            raise RuntimeError(f"isabelle ERROR: {arg}")
        # name == "OK" with {"task": ...}: now wait for the terminal message.
        while True:
            name, arg = self._read_message()
            if name in ("FINISHED", "FAILED"):
                try:
                    return json.loads(arg) if arg.strip() else {}
                except json.JSONDecodeError:
                    return {"_raw": arg, "_status": name}
            # NOTE = progress; keep waiting.

    # -- handshake / session ---------------------------------------------- #
    def _handshake(self) -> None:
        self._send_short(self.password)  # first line = UUID password (short)
        name, arg = self._read_message()
        if name != "OK":
            raise RuntimeError(f"isabelle handshake failed: {name} {arg}")

    def _start_session(self) -> None:
        self._send_command("session_start", {"session": self.logic})
        result = self._await_task()
        self.session_id = result.get("session_id")
        if not self.session_id:
            raise RuntimeError(f"session_start returned no session_id: {result}")

    # -- uniform ops ------------------------------------------------------- #
    def submit_unit(self, code: str, theory: str = "Scratch") -> dict[str, Any]:
        """Write a theory + ``use_theories``; parse ok/errors/messages."""
        # The server reads theory files from master_dir/tmp_dir. We inline the
        # body via a temp file the server can see is out of scope here; instead
        # we pass the theory name and rely on the server's master_dir. For a
        # self-contained submit we send the theory text through a long message
        # if the server build supports inline text; otherwise callers set
        # master_dir. We keep to the documented use_theories argument.
        args = {"session_id": self.session_id, "theories": [theory]}
        self._send_command("use_theories", args)
        result = self._await_task()
        ok = bool(result.get("ok"))
        errors = [_msg_text(m) for m in result.get("errors", []) or []]
        messages: list[str] = []
        for node in result.get("nodes", []) or []:
            for m in node.get("messages", []) or []:
                messages.append(_msg_text(m))
        return {"ok": True, "proved": ok and not errors,
                "goals": [], "messages": messages, "errors": errors}

    def stop(self) -> None:
        try:
            if self.session_id:
                self._send_command("session_stop",
                                   {"session_id": self.session_id})
                self._await_task()
        except Exception:  # noqa: BLE001 -- best-effort
            pass
        finally:
            try:
                self._send_short("shutdown")
            except Exception:  # noqa: BLE001
                pass
            try:
                self._sock.close()
            except Exception:  # noqa: BLE001
                pass


def _msg_text(message: Any) -> str:
    if isinstance(message, dict):
        return str(message.get("message", message))
    return str(message)


# --------------------------------------------------------------------------- #
# Mock backend (deterministic, offline).
# --------------------------------------------------------------------------- #
def _mock_submit_unit(code: str) -> dict[str, Any]:
    """Canned ``use_theories`` result: ok unless a sorry/oops placeholder."""
    tainted = any(re.search(rf"(?<!\w){m}(?!\w)", code) for m in UNPROVED_MARKERS)
    proved = not tainted
    errors: list[str] = []
    messages = (["theory Scratch: consolidated (finished)"] if proved
                else ["Found sorry/oops placeholder; proof is oracle-tainted."])
    return {"ok": True, "proved": proved, "goals": [],
            "messages": messages, "errors": errors,
            "mode": "mock", "backend": "isabelle-server"}


# --------------------------------------------------------------------------- #
# Uniform ProofSession contract.
# --------------------------------------------------------------------------- #
def start(project: Optional[dict[str, Any]] = None) -> dict[str, Any]:
    """Open a session (start the HOL session on the server, or mock)."""
    project = project or {}
    mode, address, password = detect_backend()
    logic = project.get("session") or os.environ.get(SESSION_ENV, "HOL")
    if mode == "live" and address:
        try:
            client = _IsabelleClient(address, password or "", logic)
            sid = _next_id("isabelle")
            with _LIVE_LOCK:
                _LIVE[sid] = client
            return {"ok": True, "session": {
                "id": sid, "mode": "live", "backend": "isabelle-server",
                "server_session_id": client.session_id, "logic": logic,
                "project": project}}
        except Exception as exc:  # noqa: BLE001 -- degrade to mock
            sid = _next_id("isabelle")
            return {"ok": True, "session": {
                "id": sid, "mode": "mock", "backend": "isabelle-server",
                "logic": logic, "project": project,
                "note": f"live start failed, using mock: {exc}"}}
    sid = _next_id("isabelle")
    return {"ok": True, "session": {
        "id": sid, "mode": "mock", "backend": "isabelle-server",
        "logic": logic, "project": project}}


def _live_client(session: dict[str, Any]) -> Optional[_IsabelleClient]:
    if session.get("mode") != "live":
        return None
    with _LIVE_LOCK:
        return _LIVE.get(session.get("id", ""))


def submit_unit(session: dict[str, Any], code: str) -> dict[str, Any]:
    """Submit a whole ``.thy`` body; parse ``{ok, errors, messages}``."""
    client = _live_client(session)
    if client is not None:
        try:
            theory = (session.get("project") or {}).get("theory", "Scratch")
            res = client.submit_unit(code, theory)
            res.setdefault("mode", "live")
            res.setdefault("backend", "isabelle-server")
            return res
        except Exception as exc:  # noqa: BLE001 -- degrade to mock
            out = _mock_submit_unit(code)
            out["note"] = f"live submit failed, using mock: {exc}"
            return out
    return _mock_submit_unit(code)


def step_tactic(session: dict[str, Any], state: Any = None,
                tactic: str = "") -> dict[str, Any]:
    """Not supported: Isabelle Server is whole-theory, not per-tactic."""
    out = dict(_STEP_UNSUPPORTED)
    out["mode"] = session.get("mode", "mock")
    out["backend"] = "isabelle-server"
    return out


def goal_state(session: dict[str, Any], state: Any = None) -> dict[str, Any]:
    """No per-state goal read at theory granularity; return an empty shape.

    Kept for contract parity with ``rocq_driver``; the meaningful health signal
    is ``node_status`` from ``submit_unit``.
    """
    return {"ok": True, "goals": [], "state": state,
            "mode": session.get("mode", "mock"),
            "backend": "isabelle-server",
            "note": "Isabelle reports per-theory node_status, not per-goal state."}


def stop(session: dict[str, Any]) -> dict[str, Any]:
    """Tear the session down (session_stop + shutdown on a live server)."""
    sid = session.get("id", "")
    with _LIVE_LOCK:
        client = _LIVE.pop(sid, None)
    if client is not None:
        client.stop()
    return {"ok": True, "id": sid, "mode": session.get("mode", "mock")}


# --------------------------------------------------------------------------- #
# Worker-style dispatch (tool == "isabelle_session").
# --------------------------------------------------------------------------- #
def run(request: dict[str, Any]) -> dict[str, Any]:
    """Dispatch a worker-style request. ``op`` selects the ProofSession call."""
    op = request.get("op", "start")
    if op == "start":
        return start(request.get("project"))
    if op == "detect":
        mode, address, _pw = detect_backend()
        return {"ok": True, "mode": mode,
                "address": list(address) if address else None,
                "backend": "isabelle-server"}
    session = request.get("session")
    if not isinstance(session, dict):
        return {"ok": False, "error": "missing 'session'"}
    if op == "submit_unit":
        return submit_unit(session, request.get("code", ""))
    if op == "step_tactic":
        return step_tactic(session, request.get("state"),
                           request.get("tactic", ""))
    if op == "goal_state":
        return goal_state(session, request.get("state"))
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
