"""Anti-tamper environment replay: re-check an `.olean` against its own imports.

Drives `components/verify/lean/replay_environment.lean`, a modified MIT port of
SafeVerify (see that file's header for the attribution chain and the licence text).

THE PROBLEM THIS CLOSES
-----------------------
Layer 3 of the gate is `leanchecker`, and the bulk axiom auditor
(`audit_axioms.lean`) walks an environment. Both TRUST the environment they are
handed. A producer that controls the elaborator can reach the kernel environment
directly (core Lean ships
`Lean.Kernel.Environment.addDeclWithoutChecking` for exactly that), write an
`.olean` asserting a theorem the kernel never accepted, and hand it over. Such a
declaration has an EMPTY axiom closure, so an axiom audit calls it clean. A
replay reconstructs the environment from the module's declared imports and sends
every stored declaration back through the kernel, so what was validated is
provably what the imports describe rather than whatever was handed over.

WHAT A SUCCESSFUL REPLAY ESTABLISHES
------------------------------------
Exactly this: every non-`unsafe`, non-`partial` constant stored in the given
`.olean` was re-accepted by the Lean kernel in an environment built from that
module's own declared imports as they exist on disk right now, and its stored
constructors and recursors match the ones the kernel regenerates from the
replayed inductives.

WHAT IT DOES NOT ESTABLISH
--------------------------
  * NOT that the proof is sound or relevant. `theorem t : True := trivial`
    replays perfectly. Replay says nothing about WHICH theorem was proved.
  * NOT that the axiom closure is clean. `sorryAx` is a real axiom and the
    kernel accepts declarations that use it, so a `sorry`-ridden module replays
    green. That question belongs to `check_axioms` / `audit_axioms.lean`, and
    nothing here changes, weakens, or reroutes them.
  * NOT that the imports are trustworthy. Replay rebuilds FROM the imports; it
    does not audit them. A doctored `.olean` inside the import closure is loaded
    by `importModules`, not re-checked. Covering a closure means replaying every
    module in it.
  * NOT that `unsafe` / `partial` constants are valid. `Environment.replay`
    skips them; they are reported here in `skipped`, and by default a non-empty
    `skipped` makes the result not clean.
  * NOT anything about environment extensions (instances, simp sets,
    attributes). Only kernel-level constants are replayed.

FAIL-CLOSED CONTRACT
--------------------
`clean` is True only when the Lean run completed, exited zero, emitted a summary
line whose count agrees with the result line, replayed at least one declaration,
and skipped nothing (unless `allow_skipped`). Missing toolchain, timeout,
crash, unparseable output, zero declarations, and kernel rejection are all
`clean=False`, and `ok` separates "the replay ran and answered" from "the replay
could not run". Neither is ever a pass.

One case deserves naming because it is load-bearing rather than incidental: a
TRUNCATED `.olean` segfaults Lean inside `readModuleData`, in native code,
before any catchable exception exists. The Lean side cannot report it. This
wrapper is what keeps it closed, by refusing any exit code outside 0 and 1 and
by requiring a summary line that a dead process never printed.

NOTHING IN THE GATE DEPENDS ON THIS YET. It is additive alongside `check_axioms`
and the bulk auditor, by design, until it has been proven on real workloads.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
from typing import Any, Sequence

from theoremata_tools.axioms import _resolve

RESULT_MARKER = "THEOREMATA_ENV_REPLAY"
SUMMARY_MARKER = "THEOREMATA_ENV_REPLAY_SUMMARY"
ERROR_MARKER = "THEOREMATA_ENV_REPLAY_ERROR"

REPLAY_LEAN_PATH = os.path.normpath(
    os.path.join(
        os.path.dirname(__file__),
        os.pardir,
        os.pardir,
        "lean",
        "replay_environment.lean",
    )
)


def parse_replay_output(text: str) -> dict[str, Any] | None:
    """Extract the replay report from a run's combined stdout and stderr.

    Returns None whenever the output cannot be trusted, and None is never a
    pass. Specifically: the error marker appeared anywhere (which covers both
    the zero-declaration guard and a kernel rejection), no result line was
    found, no summary line was found (so the run did not reach the end), the
    two disagree on the count, or either payload failed to parse.

    This is pure text handling with no Lean involved, so it is the part CI can
    actually exercise.
    """
    if ERROR_MARKER in text:
        return None

    result: dict[str, Any] | None = None
    summary: int | None = None

    for raw in text.splitlines():
        line = raw.strip()
        idx = line.find(RESULT_MARKER)
        if idx < 0:
            continue
        rest = line[idx:]
        # The summary marker starts with the result marker as a prefix, so the
        # longer one has to be tested first or every summary line is misread as
        # a result line.
        if rest.startswith(SUMMARY_MARKER):
            payload = rest[len(SUMMARY_MARKER):].strip()
            try:
                summary = int(json.loads(payload)["replayed"])
            except (ValueError, KeyError, TypeError):
                return None
            continue
        payload = rest[len(RESULT_MARKER):].strip()
        try:
            obj = json.loads(payload)
            parsed = {
                "olean": str(obj["olean"]),
                "imports": [str(m) for m in obj["imports"]],
                "total_constants": int(obj["total_constants"]),
                "replayed": int(obj["replayed"]),
                "skipped": [str(n) for n in obj["skipped"]],
            }
        except (ValueError, KeyError, TypeError):
            return None
        if result is not None:
            # Two result lines mean we cannot say which module the summary
            # describes. Refuse rather than guess.
            return None
        result = parsed

    if result is None or summary is None:
        return None
    if summary != result["replayed"]:
        return None
    if result["replayed"] == 0:
        # The Python-side twin of the Lean zero-declaration guard. A replay that
        # checked nothing must never look like a replay that came back clean,
        # even if some future change to the Lean side stops emitting the marker.
        return None
    return result


def _fail(olean: str, message: str) -> dict[str, Any]:
    return {
        "ok": False,
        "olean": olean,
        "imports": [],
        "total_constants": 0,
        "replayed": 0,
        "skipped": [],
        "clean": False,
        "error": message[:4000],
    }


def replay_olean(
    olean: str,
    search_paths: Sequence[str] | None = None,
    lean_bin: str | None = None,
    timeout: float = 600.0,
    allow_skipped: bool = False,
    replay_lean_path: str | None = None,
) -> dict[str, Any]:
    """Replay one compiled Lean module through the kernel against its own imports.

    `search_paths` are Lean search ROOTS (directories containing `.olean`s), not
    modules; they are what lets the module's imports resolve.

    `allow_skipped` relaxes the default refusal of modules containing `unsafe`
    or `partial` constants. Those are never sent to the kernel by
    `Environment.replay`, so a module containing them has been only partly
    replayed. The default is to refuse; the flag exists so a caller that has
    another reason to accept them can say so explicitly rather than have the
    hole go unnoticed.
    """
    script = replay_lean_path or REPLAY_LEAN_PATH
    if not os.path.exists(script):
        return _fail(olean, f"replay meta-program not found at {script}")
    if not os.path.exists(olean):
        return _fail(olean, f"object file '{olean}' does not exist")

    lean = _resolve("lean", lean_bin)
    if not lean:
        # No toolchain is not a pass. CI has none, and this is the branch that
        # keeps it from silently looking like one.
        return _fail(olean, "no Lean toolchain found")

    cmd = [lean, "--run", script, olean, *(search_paths or [])]
    try:
        proc = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            encoding="utf-8",
            errors="replace",
        )
    except OSError as exc:
        return _fail(olean, str(exc))
    try:
        out, err = proc.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        proc.terminate()
        try:
            proc.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.communicate()
        return _fail(olean, f"lean timed out after {timeout}s")

    combined = f"{out}\n{err}"
    parsed = parse_replay_output(combined)
    if parsed is None:
        # `ok=True` here: the replay ran and gave an answer, and the answer is
        # "no". That is different from `ok=False`, which means the replay could
        # not run at all. Both are `clean=False`.
        ran = proc.returncode in (0, 1)
        return {
            "ok": ran,
            "olean": olean,
            "imports": [],
            "total_constants": 0,
            "replayed": 0,
            "skipped": [],
            "clean": False,
            "error": (err or out or "").strip()[:4000],
        }

    clean = proc.returncode == 0
    error = ""
    if not clean:
        error = f"lean exited {proc.returncode} despite a well-formed report"
    if parsed["skipped"] and not allow_skipped:
        clean = False
        error = (
            "module contains unsafe/partial constants that Environment.replay "
            f"never sends to the kernel: {', '.join(parsed['skipped'])}"
        )
    return {
        "ok": True,
        "olean": parsed["olean"],
        "imports": parsed["imports"],
        "total_constants": parsed["total_constants"],
        "replayed": parsed["replayed"],
        "skipped": parsed["skipped"],
        "clean": clean,
        "error": error,
    }


def main() -> None:
    if len(sys.argv) >= 2 and os.path.exists(sys.argv[1]):
        with open(sys.argv[1], encoding="utf-8") as fh:
            req = json.load(fh)
    else:
        req = json.load(sys.stdin)
    result = replay_olean(
        olean=req["olean"],
        search_paths=req.get("search_paths"),
        lean_bin=req.get("lean_bin"),
        timeout=float(req.get("timeout", 600.0)),
        allow_skipped=bool(req.get("allow_skipped", False)),
    )
    print(json.dumps(result))
    raise SystemExit(0 if result.get("clean") else 1)


if __name__ == "__main__":
    main()
