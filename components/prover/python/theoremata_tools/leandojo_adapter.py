"""LeanDojo-style proof-session adapter (initialize + run_tactic protocol).

Mock mode (default): deterministic tactic session that closes a trivial goal.
Live mode: ``THEOREMATA_LEANDOJO_COMMAND`` receives JSON on stdin and returns JSON.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
from typing import Any


def mock_enabled() -> bool:
    env = os.environ.get("THEOREMATA_LEANDOJO_MOCK", "")
    if env in ("1", "true", "True"):
        return True
    if env in ("0", "false", "False"):
        return False
    return not os.environ.get("THEOREMATA_LEANDOJO_COMMAND")


def _mock_initialize(request: dict[str, Any]) -> dict[str, Any]:
    theorem = request.get("theorem") or {}
    full_name = theorem.get("full_name") or "MainTheorem"
    return {
        "ok": True,
        "session_id": "mock-session",
        "state_id": 0,
        "theorem": full_name,
        "goals": [f"⊢ {request.get('statement', 'True')}"],
        "mock": True,
    }


def _mock_run_tactic(request: dict[str, Any]) -> dict[str, Any]:
    tactic = (request.get("tactic") or "trivial").strip()
    if tactic in {"trivial", "exact trivial", "simp"}:
        name = (request.get("theorem") or {}).get("full_name", "MainTheorem")
        short = name.rsplit(".", 1)[-1]
        lean = (
            "import Mathlib\n\n"
            f"/-- LeanDojo mock proof. -/\n"
            f"theorem {short} : True := by\n  trivial\n"
        )
        return {
            "ok": True,
            "status": "proved",
            "lean_code": lean,
            "tactic": tactic,
            "mock": True,
        }
    return {
        "ok": True,
        "status": "in_progress",
        "state_id": int(request.get("state_id", 0)) + 1,
        "goals": ["⊢ True"],
        "mock": True,
    }


def _live(command: str, request: dict[str, Any]) -> dict[str, Any]:
    proc = subprocess.run(
        command,
        input=json.dumps(request),
        text=True,
        capture_output=True,
        shell=True,
        check=False,
    )
    if proc.returncode != 0:
        return {"ok": False, "stderr": proc.stderr, "status": "error"}
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError:
        return {"ok": False, "stderr": proc.stdout, "status": "error"}


def run(request: dict[str, Any]) -> dict[str, Any]:
    op = request.get("op", "initialize")
    if mock_enabled():
        if op == "initialize":
            return _mock_initialize(request)
        if op == "run_tactic":
            return _mock_run_tactic(request)
        if op == "prove":
            init = _mock_initialize(request)
            tactic = _mock_run_tactic({**request, "state_id": init["state_id"]})
            return {**tactic, "session": init}
        return {"ok": False, "stderr": f"unknown op: {op}"}

    cmd = os.environ.get("THEOREMATA_LEANDOJO_COMMAND")
    if not cmd:
        return {"ok": False, "stderr": "THEOREMATA_LEANDOJO_COMMAND not set"}
    return _live(cmd, request)


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