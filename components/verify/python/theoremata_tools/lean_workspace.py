"""Theoremata Lean-workspace scaffolder.

Generates a Lake project that depends on a *local* Mathlib checkout via a path
dependency, so it reuses the existing olean cache instead of re-downloading /
re-compiling Mathlib. Generated proofs are written as modules inside the
``Theoremata`` library so they can be compiled, audited, and later run through
LeanParanoia (which needs compiled, importable modules).

Public API:
    scaffold(target_dir, mathlib_root) -> dict
    place_proof(workspace_dir, module_name, source) -> dict
    resolve_lake() -> str
    build(workspace_dir, module_name=None, timeout=...) -> dict   # optional

CLI:
    python -m theoremata_tools.lean_workspace request.json
    echo '{...}' | python -m theoremata_tools.lean_workspace
"""

from __future__ import annotations

import json
import os
import re
import shutil
import signal
import subprocess
import sys
from pathlib import Path

# The Lean library name that holds generated proofs. A module placed as
# ``<workspace>/Theoremata/Foo.lean`` has the fully-qualified Lean module name
# ``Theoremata.Foo`` and is imported / audited under that name.
LIB_NAME = "Theoremata"

# ---------------------------------------------------------------------------
# Path / toolchain helpers
# ---------------------------------------------------------------------------


def _abs(p: str | os.PathLike) -> str:
    return os.path.abspath(os.fspath(p))


def _read_toolchain(mathlib_root: str) -> str:
    """Return the toolchain string from ``mathlib_root/lean-toolchain``.

    The toolchain MUST match Mathlib's so the prebuilt oleans are compatible.
    """
    tc = Path(mathlib_root) / "lean-toolchain"
    if not tc.is_file():
        raise FileNotFoundError(f"mathlib lean-toolchain not found: {tc}")
    return tc.read_text(encoding="utf-8").strip()


def _toml_path(p: str) -> str:
    """Format a filesystem path for embedding inside a TOML string.

    We use forward slashes even on Windows: Lake accepts them and it avoids
    TOML backslash-escape pitfalls.
    """
    return _abs(p).replace("\\", "/")


def _sanitize_module_name(name: str) -> str:
    """Validate/normalise a Lean module component name.

    Allows dotted names (e.g. ``Proofs.Foo``); each component must be a valid
    Lean identifier-ish token. Returns the cleaned name.
    """
    name = name.strip()
    if name.endswith(".lean"):
        name = name[: -len(".lean")]
    name = name.strip("/").replace("/", ".").replace("\\", ".")
    if not name:
        raise ValueError("module_name is empty")
    for part in name.split("."):
        if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_']*", part):
            raise ValueError(f"invalid module name component: {part!r}")
    return name


# ---------------------------------------------------------------------------
# File-content templates
# ---------------------------------------------------------------------------


def _lakefile_toml(mathlib_root: str) -> str:
    mathlib_path = _toml_path(mathlib_root)
    return f"""\
name = "theoremata"
version = "0.1.0"
defaultTargets = ["{LIB_NAME}"]

[leanOptions]
pp.unicode.fun = true
autoImplicit = false

# Local Mathlib path dependency. Points at an existing checkout so its prebuilt
# olean cache is reused (no download, no recompilation of Mathlib itself).
[[require]]
name = "mathlib"
path = "{mathlib_path}"

# Workspace library that holds the root module plus every generated proof
# placed under the `{LIB_NAME}/` directory (module names `{LIB_NAME}.<Name>`).
[[lean_lib]]
name = "{LIB_NAME}"
"""


def _root_module() -> str:
    return f"""\
/-!
Root module for the Theoremata Lean workspace.

Generated proofs live in sibling modules `{LIB_NAME}.<Name>`. This root is kept
deliberately minimal and pulls in no heavy dependencies, so `lake build` of the
root alone stays cheap; individual proof modules pull in Mathlib as needed.
-/

namespace {LIB_NAME}

/-- Marker that the workspace scaffolding is present. -/
def workspaceReady : Prop := True

end {LIB_NAME}
"""


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def scaffold(target_dir: str, mathlib_root: str) -> dict:
    """Create a Lake project at ``target_dir`` requiring local Mathlib.

    Writes ``lean-toolchain`` (copied from mathlib), ``lakefile.toml`` (with a
    ``require mathlib`` path dependency), and the root ``Theoremata.lean``.
    """
    target = Path(_abs(target_dir))
    mathlib_root_abs = _abs(mathlib_root)
    if not Path(mathlib_root_abs).is_dir():
        raise FileNotFoundError(f"mathlib_root does not exist: {mathlib_root_abs}")

    toolchain = _read_toolchain(mathlib_root_abs)

    target.mkdir(parents=True, exist_ok=True)
    (target / LIB_NAME).mkdir(parents=True, exist_ok=True)

    files: list[str] = []

    tc_file = target / "lean-toolchain"
    # Preserve exact toolchain string (with trailing newline) so it matches.
    tc_file.write_text(toolchain + "\n", encoding="utf-8")
    files.append(str(tc_file))

    lakefile = target / "lakefile.toml"
    lakefile.write_text(_lakefile_toml(mathlib_root_abs), encoding="utf-8")
    files.append(str(lakefile))

    root = target / f"{LIB_NAME}.lean"
    root.write_text(_root_module(), encoding="utf-8")
    files.append(str(root))

    return {
        "ok": True,
        "target_dir": str(target),
        "mathlib_root": mathlib_root_abs,
        "files": files,
        "toolchain": toolchain,
        "lib_name": LIB_NAME,
    }


def place_proof(workspace_dir: str, module_name: str, source: str) -> dict:
    """Write ``source`` to ``<workspace>/Theoremata/<module_name>.lean``.

    Returns the module path and the fully-qualified Lean module name
    ``Theoremata.<module_name>`` used for ``import`` / ``lake exe paranoia``.
    """
    workspace = Path(_abs(workspace_dir))
    clean = _sanitize_module_name(module_name)

    # Dotted components become nested directories under the lib root.
    parts = clean.split(".")
    module_file = workspace / LIB_NAME
    for p in parts[:-1]:
        module_file = module_file / p
    module_file.mkdir(parents=True, exist_ok=True)
    module_file = module_file / f"{parts[-1]}.lean"

    module_file.write_text(source, encoding="utf-8")

    qualified = f"{LIB_NAME}.{clean}"
    return {
        "ok": True,
        "workspace_dir": str(workspace),
        "module_file": str(module_file),
        "module_name": clean,
        "qualified_name": qualified,
    }


# ---------------------------------------------------------------------------
# Lake / build helpers
# ---------------------------------------------------------------------------


def resolve_lake() -> str:
    """Locate the ``lake`` executable, prepending ``$HOME/.elan/bin`` to PATH.

    Returns the resolved lake path (or bare ``"lake"`` if not found on PATH,
    so callers still get a sensible command to try).
    """
    elan_bin = Path(os.path.expanduser("~")) / ".elan" / "bin"
    if elan_bin.is_dir():
        cur = os.environ.get("PATH", "")
        entries = cur.split(os.pathsep)
        if str(elan_bin) not in entries:
            os.environ["PATH"] = str(elan_bin) + os.pathsep + cur
    found = shutil.which("lake")
    return found or "lake"


def build(workspace_dir: str, module_name: str | None = None, timeout: float = 900.0) -> dict:
    """Optionally compile a placed module with ``lake env lean``.

    Kept optional and used sparingly: compiling against Mathlib is slow. If
    ``module_name`` is given, its ``.lean`` file is type-checked; otherwise the
    root module is checked. Timeout-guarded with SIGTERM -> SIGKILL escalation.
    """
    workspace = Path(_abs(workspace_dir))
    lake = resolve_lake()

    if module_name:
        clean = _sanitize_module_name(module_name)
        rel = Path(LIB_NAME)
        for p in clean.split("."):
            rel = rel / p
        target_file = str(rel) + ".lean"
    else:
        target_file = f"{LIB_NAME}.lean"

    cmd = [lake, "env", "lean", target_file]
    proc = subprocess.Popen(
        cmd,
        cwd=str(workspace),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    try:
        out, _ = proc.communicate(timeout=timeout)
        rc = proc.returncode
        timed_out = False
    except subprocess.TimeoutExpired:
        timed_out = True
        try:
            proc.terminate()  # SIGTERM
            try:
                out, _ = proc.communicate(timeout=15)
            except subprocess.TimeoutExpired:
                proc.kill()  # SIGKILL
                out, _ = proc.communicate()
        except Exception:
            out = ""
        rc = proc.returncode if proc.returncode is not None else -signal.SIGKILL

    return {
        "ok": (not timed_out) and rc == 0,
        "returncode": rc,
        "timed_out": timed_out,
        "command": cmd,
        "target_file": target_file,
        "output": out or "",
    }


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def _handle(req: dict) -> dict:
    op = req.get("op")
    if op == "scaffold":
        return scaffold(req["target_dir"], req["mathlib_root"])
    if op == "place_proof":
        return place_proof(req["workspace_dir"], req["module_name"], req["source"])
    if op == "build":
        return build(
            req["workspace_dir"],
            req.get("module_name"),
            float(req.get("timeout", 900.0)),
        )
    raise ValueError(f"unknown op: {op!r}")


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] not in ("-", ""):
        raw = Path(argv[0]).read_text(encoding="utf-8")
    else:
        raw = sys.stdin.read()

    try:
        req = json.loads(raw)
        result = _handle(req)
    except Exception as exc:  # emit structured error, still exit 0
        print(json.dumps({"ok": False, "error": str(exc), "error_type": type(exc).__name__}))
        return 0

    print(json.dumps(result))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
