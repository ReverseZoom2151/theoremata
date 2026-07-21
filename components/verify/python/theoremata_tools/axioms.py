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

`check_axioms_bulk` is a second, faster path over the same question: it splices the
`audit_axioms.lean` meta-program into the generated file and audits every named
theorem in ONE compile with one shared memo, instead of one compile per theorem.
It is deliberately additive. `check_axioms` is untouched and remains the path the
gate routes through until the two are shown to agree on a given workload.
"""
from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from typing import Any, Iterable

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


# ---------------------------------------------------------------------------
# Bulk path: the `collectAxioms` meta-program, one compile for many theorems.
# ---------------------------------------------------------------------------

# Everything in `audit_axioms.lean` above this marker is safe to splice into a
# generated proof file; everything below is the standalone executable, which
# defines `main` and would collide with a generated file that defines its own.
SPLICE_END_MARKER = "-- THEOREMATA_SPLICE_END"

RESULT_MARKER = "THEOREMATA_AXIOM_AUDIT"
SUMMARY_MARKER = "THEOREMATA_AXIOM_AUDIT_SUMMARY"
ERROR_MARKER = "THEOREMATA_AXIOM_AUDIT_ERROR"

AUDIT_LEAN_PATH = os.path.normpath(
    os.path.join(os.path.dirname(__file__), os.pardir, os.pardir, "lean", "audit_axioms.lean")
)

_IMPORT_RE = re.compile(r"^\s*(?:public\s+|private\s+|meta\s+)*import\s+\S")
_IDENT_RE = re.compile(r"^[A-Za-z_À-￿][A-Za-z0-9_'À-￿]*"
                       r"(?:\.[A-Za-z_À-￿][A-Za-z0-9_'À-￿]*)*$")


def split_imports(source: str) -> tuple[list[str], str]:
    """Split Lean `source` into its leading import lines and everything else.

    Lean requires every `import` to precede all other commands, so splicing the
    meta-program in means hoisting both files' imports to the top. Leading
    comments are returned as part of the body: a comment is legal after the
    imports, and keeping the attribution header in the generated file is a
    feature. Block-comment nesting is tracked so an `import` mentioned inside a
    comment is never mistaken for a real one.
    """
    imports: list[str] = []
    body: list[str] = []
    depth = 0
    seen_non_import = False
    for line in source.splitlines():
        stripped = line.strip()
        if depth == 0 and not seen_non_import and _IMPORT_RE.match(line):
            imports.append(stripped)
            continue
        opens = line.count("/-")
        closes = line.count("-/")
        if depth > 0 or opens > 0:
            body.append(line)
            depth = max(0, depth + opens - closes)
            continue
        if stripped and not stripped.startswith("--"):
            seen_non_import = True
        body.append(line)
    return imports, "\n".join(body)


def load_audit_prelude(path: str | None = None) -> str:
    """Read the spliceable portion of the `audit_axioms.lean` meta-program."""
    with open(path or AUDIT_LEAN_PATH, encoding="utf-8") as fh:
        text = fh.read()
    head, sep, _tail = text.partition(SPLICE_END_MARKER)
    if not sep:
        # Fail closed: a prelude we cannot split is a prelude we will not splice.
        raise ValueError(f"audit prelude missing {SPLICE_END_MARKER!r}")
    return head


def build_bulk_source(source: str, theorems: Iterable[str], prelude: str) -> str:
    """Assemble the single file that audits every theorem in one compile."""
    names = list(theorems)
    for name in names:
        # A name that is not a plain Lean identifier could smuggle arbitrary
        # syntax into the generated file, so reject rather than sanitize.
        if not _IDENT_RE.match(name):
            raise ValueError(f"not a Lean identifier: {name!r}")
    pre_imports, pre_body = split_imports(prelude)
    src_imports, src_body = split_imports(source)
    merged: list[str] = []
    for imp in pre_imports + src_imports:
        if imp not in merged:
            merged.append(imp)
    parts = merged + [pre_body, src_body, "#audit_axioms " + " ".join(names), ""]
    return "\n".join(parts)


def parse_audit_output(text: str) -> dict[str, list[str]] | None:
    """Extract the per-declaration axiom closures from an audit run.

    Returns None whenever the output cannot be trusted: the zero-declaration
    guard fired, no summary line was emitted (so the audit did not run to
    completion), or the summary count disagrees with the number of result lines.
    None is never a pass.
    """
    if ERROR_MARKER in text:
        return None
    results: dict[str, list[str]] = {}
    audited: int | None = None
    for raw in text.splitlines():
        line = raw.strip()
        idx = line.find(RESULT_MARKER)
        if idx < 0:
            continue
        payload = line[idx + len(RESULT_MARKER):].strip()
        if line[idx:].startswith(SUMMARY_MARKER):
            payload = line[idx + len(SUMMARY_MARKER):].strip()
            try:
                audited = int(json.loads(payload)["audited"])
            except (ValueError, KeyError, TypeError):
                return None
            continue
        try:
            obj = json.loads(payload)
            results[str(obj["decl"])] = [str(a) for a in obj["axioms"]]
        except (ValueError, KeyError, TypeError):
            return None
    if audited is None or audited == 0 or audited != len(results):
        return None
    return results


def check_axioms_bulk(
    source: str,
    theorems: Iterable[str],
    root: str | None = None,
    allowed: list[str] | None = None,
    lean_bin: str | None = None,
    timeout: float = 300.0,
    prelude_path: str | None = None,
) -> dict[str, Any]:
    """Audit many theorems in one Lean compile via the `collectAxioms` meta-program.

    Returns `{"ok", "audited", "clean", "results": {name: <check_axioms-shaped>}}`.
    Every failure mode (no toolchain, compile error, unparseable output, zero
    declarations audited, a requested theorem missing from the report) yields
    `ok=False` and `clean=False` for every theorem, which is what distinguishes
    it from a clean audit.
    """
    names = list(theorems)
    allowed = list(allowed) if allowed is not None else list(DEFAULT_ALLOWED)
    allowed_set = set(allowed)

    def bulk_fail(message: str) -> dict[str, Any]:
        return {
            "ok": False,
            "audited": 0,
            "clean": False,
            "stderr": message[:4000],
            "results": {n: _fail(n, allowed, message) for n in names},
        }

    if not names:
        # The zero-target case is the Python-side twin of the Lean guard: an
        # audit of nothing must never look like an audit that came back clean.
        return bulk_fail("check_axioms_bulk: no theorems requested")

    try:
        payload = build_bulk_source(source, names, load_audit_prelude(prelude_path))
    except (OSError, ValueError) as exc:
        return bulk_fail(str(exc))

    lake = _resolve("lake", lean_bin)
    lean = _resolve("lean", lean_bin)
    if root:
        if not lake:
            return bulk_fail("lake not found")
        cmd, cwd = [lake, "env", "lean"], root
    elif lake:
        cmd, cwd = [lake, "env", "lean"], None
    elif lean:
        cmd, cwd = [lean], None
    else:
        return bulk_fail("no Lean toolchain found")

    tmp = tempfile.NamedTemporaryFile(
        mode="w", suffix=".lean", encoding="utf-8", delete=False
    )
    try:
        tmp.write(payload)
        tmp.close()
        try:
            proc = subprocess.Popen(
                cmd + [tmp.name],
                cwd=cwd,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                encoding="utf-8",
                errors="replace",
            )
        except FileNotFoundError as exc:
            return bulk_fail(str(exc))
        try:
            out, err = proc.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            proc.terminate()
            try:
                proc.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.communicate()
            return bulk_fail(f"lean timed out after {timeout}s")
    finally:
        try:
            os.unlink(tmp.name)
        except OSError:
            pass

    combined = f"{out}\n{err}"
    parsed = parse_audit_output(combined)
    if parsed is None:
        return bulk_fail(f"audit produced no trustworthy report\n{(err or '').strip()}")

    compiled = proc.returncode == 0
    results: dict[str, Any] = {}
    all_clean = compiled
    for name in names:
        if name not in parsed:
            results[name] = _fail(name, allowed, "theorem absent from audit report")
            all_clean = False
            continue
        axiom_list = parsed[name]
        disallowed = [a for a in axiom_list if a not in allowed_set]
        clean = compiled and not disallowed
        all_clean = all_clean and clean
        results[name] = {
            "ok": True,
            "theorem": name,
            "axioms": axiom_list,
            "allowed": allowed,
            "disallowed": disallowed,
            "clean": clean,
            "compiled": compiled,
            "stderr": (err or "").strip()[:4000],
        }
    return {
        "ok": True,
        "audited": len(parsed),
        "clean": all_clean,
        "stderr": (err or "").strip()[:4000],
        "results": results,
    }


def main() -> None:
    if len(sys.argv) >= 2 and os.path.exists(sys.argv[1]):
        with open(sys.argv[1], encoding="utf-8") as fh:
            req = json.load(fh)
    else:
        req = json.load(sys.stdin)
    if req.get("theorems"):
        # Opt-in only. A request that names a single `theorem` keeps the
        # long-standing `#print axioms` path; nothing is silently rerouted.
        bulk = check_axioms_bulk(
            source=req["source"],
            theorems=req["theorems"],
            root=req.get("root"),
            allowed=req.get("allowed"),
            lean_bin=req.get("lean_bin"),
            timeout=float(req.get("timeout", 300.0)),
        )
        print(json.dumps(bulk))
        raise SystemExit(0 if bulk.get("ok") and bulk.get("clean") else 1)
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
