"""Detector for `statement_rests_on_opaque_constant`.

WHY THIS IS NOT THE AXIOM AUDIT
-------------------------------
Our layer-2 audit (`theoremata_tools.axioms.check_axioms`) runs `#print axioms`
on a theorem and asks whether its dependency closure is inside an allowlist.
That closure is flat: it merges the theorem's TYPE and its VALUE into one set of
axiom names with no attribution. So the audit answers exactly one question,
"does this theorem depend on something forbidden", and it returns the identical
verdict `[sorryAx]` for two situations that are morally opposite:

  (a) an honest incomplete proof of a real statement, and
  (b) a complete, sorry-free proof of a statement that is made of admitted
      placeholders and therefore asserts nothing.

Case (a) is a promise to finish. Case (b) is a statement with no mathematical
content, wearing the name of a published theorem. A human reading `[sorryAx]`
greps the proof, finds no `sorry`, and concludes the tool is confused. A
mutation-style triviality check cannot separate them either: it perturbs the
statement and asks whether it stays provable, but replacing one opaque constant
with another opaque constant changes nothing, so the statement is not trivially
TRUE, it is CONTENTLESS.

This module answers the different question: are the constants appearing in the
theorem's STATEMENT (its type, never its proof) backed by real definitions? It
attributes `sorryAx` to the individual constants of the type and reports which
ones and where they are declared. On the honest-sorry file it reports nothing,
because nothing is wrong with the statement. That separation is the whole point.

WHAT THIS CHECK CAN AND CANNOT SAY
----------------------------------
It can only ACCUSE. "No opaque constant found" is not a blessing: a statement
built entirely from real definitions can still be mis-stated, vacuous, weaker
than its name claims, or plainly false. No verdict in this module reads as an
endorsement, and there is no boolean here named `sound`, `valid` or `clean`.

It fails closed into SILENCE, never into an accusation. Anything ambiguous, any
missing import, any file that does not elaborate, any unparseable probe output
yields `UNKNOWN` with a withholding reason. A false accusation against honest
third-party mathematics is worse than a miss.

THE ADMITTED-VERSUS-ABSTRACT BOUNDARY
-------------------------------------
A constant is reported opaque if and only if `sorryAx` occurs in its OWN axiom
closure. Lean emits `sorryAx` for exactly one reason: the author wrote `sorry`
or `admit`. It is not a keyword anyone declares on purpose. So this test tracks
authorial intent rather than declaration syntax:

  - A section `variable`, a type parameter or a typeclass instance argument is a
    BINDER in the theorem's type, not a constant. It never enters the candidate
    set at all, so a normal abstract algebra lemma over a group cannot be
    flagged. This is structural, not a heuristic.
  - An `axiom` the author declared deliberately has itself, not `sorryAx`, in
    its closure. Not flagged. It is a visible, intentional assumption and the
    layer-2 audit already owns it.
  - An `opaque` declaration with a real value has no `sorryAx`. Not flagged;
    sealing an implementation is legitimate.
  - `opaque foo : T := sorry` DOES carry `sorryAx` and IS flagged. The `opaque`
    keyword seals the definitional unfolding, not the admission; the author
    still wrote `sorry`, and this is precisely the laundering the check exists
    to catch.
  - A `def` or `theorem` with a real body has a clean closure. Not flagged.

The candidate set comes from Lean itself, via `Expr.getUsedConstants` on the
elaborated type, so we never guess which identifier is a constant by parsing.
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

# The verdicts. `NO_FINDING` is deliberately not called "clean" or "sound".
VERDICT_OPAQUE = "opaque_constant_found"
VERDICT_NO_FINDING = "no_opaque_constant_found"
VERDICT_UNKNOWN = "unknown"

DISCLAIMER = (
    "This check can only accuse. 'no_opaque_constant_found' means no constant in "
    "the statement carries sorryAx; it does not mean the statement is meaningful, "
    "correct, or worthy of its name."
)

_MARK_BEGIN = "THEOREMATA_OPAQUE_BEGIN"
_MARK_END = "THEOREMATA_OPAQUE_END"
_MARK_CONST = "THEOREMATA_OPAQUE_CONST"
_MARK_MISSING = "THEOREMATA_OPAQUE_MISSING"

_ADMITTED_AXIOM = "sorryAx"

# Lean identifiers admit a wide unicode range; rather than model it, we accept a
# conservative set and withhold on anything else. A name we cannot vouch for is
# interpolated into Lean source, so being strict here is a safety property, not
# just tidiness.
_NAME_RE = re.compile(r"^[A-Za-z_À-ɏͰ-Ͽ][A-Za-z0-9_'!?À-ɏͰ-Ͽ]*"
                      r"(\.[A-Za-z0-9_'!?À-ɏͰ-Ͽ]+)*$")

_DECL_KEYWORDS = (
    "def",
    "theorem",
    "lemma",
    "abbrev",
    "opaque",
    "axiom",
    "instance",
    "structure",
    "inductive",
    "class",
)

# An `error:` line means the file did not elaborate, so no report from it can be
# trusted. `sorry` only produces a warning, which is expected here.
_ERROR_RE = re.compile(r"^.*?:\d+:\d+: error:", re.MULTILINE)


def _resolve(name: str, override: str | None = None) -> str | None:
    """Locate a Lean toolchain binary: explicit override, process PATH, then the
    default elan install dir, which is off a non-login PATH on Windows."""
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


def render_probe(theorem: str) -> str:
    """Lean source that prints one line per constant of `theorem`'s TYPE.

    `getUsedConstants` is taken from `ci.type`, never `ci.value`, which is what
    restricts the whole check to the statement. Output is assembled with plain
    string concatenation rather than `m!` interpolation because the message
    pretty printer may reflow formatted data across lines and break parsing.
    """
    return (
        "open Lean in\n"
        "run_cmd do\n"
        "  let env ← Lean.getEnv\n"
        f"  let target : Lean.Name := `{theorem}\n"
        "  match env.find? target with\n"
        f'  | none => Lean.logInfo "{_MARK_MISSING}"\n'
        "  | some ci =>\n"
        f'    Lean.logInfo "{_MARK_BEGIN}"\n'
        "    for c in ci.type.getUsedConstants do\n"
        "      match env.find? c with\n"
        "      | none => pure ()\n"
        "      | some d =>\n"
        "        let axs ← Lean.collectAxioms c\n"
        "        let kind := match d with\n"
        '          | .axiomInfo _ => "axiom"\n'
        '          | .opaqueInfo _ => "opaque"\n'
        '          | .defnInfo _ => "def"\n'
        '          | .thmInfo _ => "theorem"\n'
        '          | .inductInfo _ => "inductive"\n'
        '          | .ctorInfo _ => "constructor"\n'
        '          | .recInfo _ => "recursor"\n'
        '          | .quotInfo _ => "quot"\n'
        f'        Lean.logInfo ("{_MARK_CONST}|" ++ toString c ++ "|" ++ kind ++ "|"\n'
        '          ++ String.intercalate "," (axs.toList.map toString))\n'
        f'    Lean.logInfo "{_MARK_END}"\n'
    )


def parse_probe_output(text: str) -> list[dict[str, Any]] | None:
    """Turn probe output into constant records.

    Returns None whenever the report is not demonstrably complete: no begin
    marker, no end marker, or the target was not found. A truncated report could
    omit the very constant that is opaque, or omit the context that makes a
    flagged one legitimate, so a partial parse must never become a verdict.
    """
    if _MARK_MISSING in text:
        return None
    begin = text.find(_MARK_BEGIN)
    end = text.find(_MARK_END)
    if begin < 0 or end < 0 or end < begin:
        return None
    records: list[dict[str, Any]] = []
    for line in text[begin:end].splitlines():
        idx = line.find(_MARK_CONST + "|")
        if idx < 0:
            continue
        parts = line[idx + len(_MARK_CONST) + 1 :].split("|")
        if len(parts) < 3:
            return None
        name, kind, axioms_raw = parts[0].strip(), parts[1].strip(), parts[2].strip()
        if not name:
            return None
        axioms = [a.strip() for a in axioms_raw.split(",") if a.strip()]
        records.append({"name": name, "kind": kind, "axioms": axioms})
    return records


def _mask_comments(source: str) -> str:
    """Blank out line and nested block comments, preserving length and newlines,
    so a declaration keyword quoted in prose cannot fake a definition site."""
    out = list(source)
    i, n = 0, len(source)
    depth = 0
    while i < n:
        two = source[i : i + 2]
        if depth == 0 and two == "--":
            while i < n and source[i] != "\n":
                out[i] = " "
                i += 1
            continue
        if two == "/-":
            depth += 1
            out[i] = out[i + 1] = " "
            i += 2
            continue
        if depth > 0 and two == "-/":
            depth -= 1
            out[i] = out[i + 1] = " "
            i += 2
            continue
        if depth > 0 and source[i] != "\n":
            out[i] = " "
        i += 1
    return "".join(out)


def locate_declaration(source: str, name: str) -> dict[str, Any] | None:
    """Best-effort source location for a constant, so a human can go look.

    Purely lexical and purely informational: it never affects the verdict. A
    constant from another file or a namespace we cannot reconstruct simply has
    no location, which is reported as null rather than guessed.
    """
    masked = _mask_comments(source)
    base = name.split(".")[-1]
    if not base:
        return None
    keywords = "|".join(_DECL_KEYWORDS)
    pattern = re.compile(
        rf"^[ \t]*(?:@\[[^\]]*\][ \t]*)?(?:private[ \t]+|protected[ \t]+|noncomputable[ \t]+|partial[ \t]+|unsafe[ \t]+)*"
        rf"(?:{keywords})[ \t]+(?:[^\s:]*\.)?{re.escape(base)}(?=[\s:({{\[]|$)",
        re.MULTILINE,
    )
    match = pattern.search(masked)
    if not match:
        return None
    line_no = masked.count("\n", 0, match.start()) + 1
    line_text = source.splitlines()[line_no - 1].strip()
    return {"line": line_no, "text": line_text[:300]}


def classify(records: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Select the admitted constants. The boundary is `sorryAx` in the
    constant's own closure; see the module docstring for why that, and not the
    declaration keyword, is the right discriminator."""
    return [r for r in records if _ADMITTED_AXIOM in r["axioms"]]


def _withhold(theorem: str, reason: str, stderr: str = "") -> dict[str, Any]:
    return {
        "ok": True,
        "theorem": theorem,
        "verdict": VERDICT_UNKNOWN,
        "withheld_reason": reason,
        "opaque_constants": [],
        "statement_constants": [],
        "statement_constant_count": 0,
        "compiled": False,
        "stderr": stderr.strip()[:4000],
        "disclaimer": DISCLAIMER,
    }


def check_statement_constants(
    source: str,
    theorem: str,
    root: str | None = None,
    lean_bin: str | None = None,
    timeout: float = 300.0,
) -> dict[str, Any]:
    """Report which constants of `theorem`'s statement are admitted placeholders.

    Every failure path below withholds rather than accuses, which is what makes
    a false accusation structurally impossible: the only way to reach
    VERDICT_OPAQUE is a zero-exit Lean run that emitted a complete, error-free
    probe report naming a constant whose own closure contains `sorryAx`.
    """
    if not _NAME_RE.match(theorem or ""):
        return _withhold(theorem, "theorem name is not a plain Lean identifier")

    payload = source if source.endswith("\n") else source + "\n"
    # `import Lean` must precede every other command, and the caller's own
    # imports are already at the top, so it goes ahead of them.
    payload = "import Lean\n" + payload + render_probe(theorem)

    tmp = tempfile.NamedTemporaryFile(
        mode="w", suffix=".lean", encoding="utf-8", delete=False
    )
    try:
        tmp.write(payload)
        tmp.close()

        lake = _resolve("lake", lean_bin if lean_bin and "lake" in lean_bin else None)
        lean = _resolve("lean", lean_bin if lean_bin and "lake" not in lean_bin else None)
        if root:
            if not lake:
                return _withhold(theorem, "lake not found but a project root was given")
            cmd, cwd = [lake, "env", "lean", tmp.name], root
        elif lake:
            cmd, cwd = [lake, "env", "lean", tmp.name], None
        elif lean:
            cmd, cwd = [lean, tmp.name], None
        else:
            return _withhold(theorem, "no Lean toolchain found")

        try:
            proc = subprocess.Popen(
                cmd,
                cwd=cwd,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                encoding="utf-8",
                errors="replace",
            )
        except OSError as exc:
            return _withhold(theorem, f"could not launch Lean: {exc}")

        try:
            out, err = proc.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            proc.terminate()
            try:
                proc.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.communicate()
            return _withhold(theorem, f"lean timed out after {timeout}s")
    finally:
        try:
            os.unlink(tmp.name)
        except OSError:
            pass

    combined = f"{out}\n{err}"
    if proc.returncode != 0:
        return _withhold(theorem, "file did not elaborate (lean exited nonzero)", combined)
    if _ERROR_RE.search(combined):
        return _withhold(theorem, "file produced elaboration errors", combined)

    records = parse_probe_output(combined)
    if records is None:
        return _withhold(
            theorem, "probe report absent or incomplete (target may not exist)", combined
        )

    opaque = classify(records)
    for rec in opaque:
        rec["defined_at"] = locate_declaration(source, rec["name"])

    return {
        "ok": True,
        "theorem": theorem,
        "verdict": VERDICT_OPAQUE if opaque else VERDICT_NO_FINDING,
        "withheld_reason": None,
        "opaque_constants": opaque,
        "statement_constants": [r["name"] for r in records],
        "statement_constant_count": len(records),
        "compiled": True,
        "stderr": (err or "").strip()[:4000],
        "disclaimer": DISCLAIMER,
    }


def run(request: dict) -> dict:
    """Worker entrypoint (key ``opaque_statement``).

    ``{source | source_path, theorem_name, lake_workspace?, lean_bin?, timeout?}``
    -> a verdict dict. Callers gating on this MUST branch only on
    ``verdict == "opaque_constant_found"``. Both ``no_opaque_constant_found``
    and ``unknown`` mean no signal, never approval.
    """
    source = request.get("source")
    if source is None:
        with open(request["source_path"], encoding="utf-8") as fh:
            source = fh.read()
    return check_statement_constants(
        source=source,
        theorem=request.get("theorem_name") or request["theorem"],
        root=request.get("lake_workspace") or request.get("root"),
        lean_bin=request.get("lean_bin"),
        timeout=float(request.get("timeout", 300.0)),
    )


def main() -> None:
    if len(sys.argv) >= 2 and os.path.exists(sys.argv[1]):
        with open(sys.argv[1], encoding="utf-8") as fh:
            req = json.load(fh)
    else:
        req = json.load(sys.stdin)
    result = check_statement_constants(
        source=req["source"],
        theorem=req["theorem"],
        root=req.get("root"),
        lean_bin=req.get("lean_bin"),
        timeout=float(req.get("timeout", 300.0)),
    )
    print(json.dumps(result, indent=2))
    # Only a positive finding is a failure. Withholding exits 0: silence is not
    # an accusation, and a missing toolchain must not fail a build.
    raise SystemExit(1 if result["verdict"] == VERDICT_OPAQUE else 0)


if __name__ == "__main__":
    main()
