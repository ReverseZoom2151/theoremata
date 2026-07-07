"""Authoritative axiom soundness gate via Lean's `#print axioms`.

Unlike the lexical `lean_soundness` pre-gate (which only scans source text),
this actually elaborates the proof and asks the Lean kernel which axioms the
theorem's full transitive closure depends on, then checks that set against an
allowlist. This is the gate that may flip a node to `formally_verified`:
`sorryAx`, `Lean.ofReduceBool`/native-decide trust, and any custom axiom all
fall outside the default allowlist and mark the result unsound.

`#print axioms <thm>` prints one of:
    'thm' depends on axioms: [propext, Classical.choice, Quot.sound]
    'thm' does not depend on any axioms
"""
from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from typing import Any

# The three standard Mathlib-classical axioms and nothing else.
DEFAULT_ALLOWED = ["propext", "Classical.choice", "Quot.sound"]

_DEPENDS_RE = re.compile(r"depends on axioms:\s*\[([^\]]*)\]")
_NO_AXIOMS_RE = re.compile(r"does not depend on any axioms")


def _resolve(name: str, override: str | None = None) -> str | None:
    """Locate a Lean toolchain binary: explicit override, process PATH, then the
    default elan install dir (which may be off a non-login PATH on Windows)."""
    if override:
        return override
    found = shutil.which(name)
    if found:
        return found
    for ext in ("", ".exe"):
        candidate = os.path.expanduser(os.path.join("~", ".elan", "bin", name + ext))
        if os.path.exists(candidate):
            return candidate
    return None


def _parse_axioms(text: str) -> list[str] | None:
    """Extract the axiom list from `#print axioms` output. Returns [] for
    'does not depend on any axioms', a list of names for the bracket form, or
    None if no axiom report was found at all."""
    if _NO_AXIOMS_RE.search(text):
        # A bracketed report, if also present, is authoritative; but the
        # no-axioms line alone means an empty closure.
        m = _DEPENDS_RE.search(text)
        if not m:
            return []
    m = _DEPENDS_RE.search(text)
    if not m:
        return [] if _NO_AXIOMS_RE.search(text) else None
    inner = m.group(1).strip()
    if not inner:
        return []
    return [a.strip() for a in inner.split(",") if a.strip()]


def check_axioms(
    source: str,
    theorem: str,
    root: str | None = None,
    allowed: list[str] | None = None,
    lean_bin: str | None = None,
    timeout: float = 300.0,
) -> dict[str, Any]:
    """Elaborate `source`, print the axiom closure of `theorem`, and check it
    against `allowed`. When `root` is a Lake project (e.g. the Mathlib
    checkout), run inside it so `import Mathlib` resolves against its build."""
    allowed = list(allowed) if allowed is not None else list(DEFAULT_ALLOWED)
    payload = source if source.endswith("\n") else source + "\n"
    payload += f"#print axioms {theorem}\n"

    tmp = tempfile.NamedTemporaryFile(
        mode="w", suffix=".lean", encoding="utf-8", delete=False
    )
    try:
        tmp.write(payload)
        tmp.close()

        if root:
            lake = _resolve("lake", lean_bin)
            if not lake:
                return _fail(theorem, allowed, "lake not found")
            cmd = [lake, "env", "lean", tmp.name]
            cwd: str | None = root
        else:
            lake = _resolve("lake", lean_bin)
            lean = _resolve("lean", lean_bin)
            if lake:
                cmd = [lake, "env", "lean", tmp.name]
            elif lean:
                cmd = [lean, tmp.name]
            else:
                return _fail(theorem, allowed, "no Lean toolchain found")
            cwd = None

        try:
            proc = subprocess.Popen(
                cmd,
                cwd=cwd,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                encoding="utf-8",
                errors="replace",
            )
        except FileNotFoundError as exc:
            return _fail(theorem, allowed, str(exc))

        try:
            out, err = proc.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            proc.terminate()
            try:
                proc.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.communicate()
            return _fail(theorem, allowed, f"lean timed out after {timeout}s")
    finally:
        try:
            os.unlink(tmp.name)
        except OSError:
            pass

    combined = f"{out}\n{err}"
    # A real compile error (not just a `sorry` warning) means we cannot trust
    # the axiom report. `#print axioms` on a failed decl may be absent; treat a
    # non-zero exit as not-compiled.
    compiled = proc.returncode == 0
    axiom_list = _parse_axioms(combined)
    if axiom_list is None:
        # No axiom report parsed — most likely the theorem name was wrong or the
        # file failed before the command. Report unclean with the diagnostics.
        return {
            "ok": True,
            "theorem": theorem,
            "axioms": [],
            "allowed": allowed,
            "disallowed": [],
            "clean": False,
            "compiled": compiled,
            "stderr": (err or "").strip()[:4000],
        }

    allowed_set = set(allowed)
    disallowed = [a for a in axiom_list if a not in allowed_set]
    clean = compiled and not disallowed
    return {
        "ok": True,
        "theorem": theorem,
        "axioms": axiom_list,
        "allowed": allowed,
        "disallowed": disallowed,
        "clean": clean,
        "compiled": compiled,
        "stderr": (err or "").strip()[:4000],
    }


def _fail(theorem: str, allowed: list[str], message: str) -> dict[str, Any]:
    return {
        "ok": False,
        "theorem": theorem,
        "axioms": [],
        "allowed": allowed,
        "disallowed": [],
        "clean": False,
        "compiled": False,
        "stderr": message,
    }


def main() -> None:
    if len(sys.argv) >= 2 and os.path.exists(sys.argv[1]):
        with open(sys.argv[1], encoding="utf-8") as fh:
            req = json.load(fh)
    else:
        req = json.load(sys.stdin)
    result = check_axioms(
        source=req["source"],
        theorem=req["theorem"],
        root=req.get("root"),
        allowed=req.get("allowed"),
        lean_bin=req.get("lean_bin"),
        timeout=float(req.get("timeout", 300.0)),
    )
    print(json.dumps(result))
    raise SystemExit(0 if result.get("clean") else 1)


if __name__ == "__main__":
    main()
