"""Statement-level mutation testing for Lean 4 theorems.

THE DEFECT THIS HUNTS
=====================
`name_claims_more_than_statement`: a theorem whose identifier advertises a
substantive property while its proposition constrains nothing. The canonical
shape, seen independently in more than one third-party corpus, is

    theorem xHyperbolicity (C : Coordinates) (P : Parameters) (U : State) :
        (exists r1 : Real, r1 = (xFluxJacobianEigenExprs C P U).lambda1) ...

Every conjunct holds for ANY right-hand side of the right type. Such files carry
no `sorry`, no custom axiom, and they pass statement preservation, because the
statement genuinely was preserved. It is merely empty.

THE CHECK
=========
Mutation testing applied to statements rather than to code. Take the
domain-specific definitions that the statement mentions, replace their BODIES
with unrelated constants while keeping their SIGNATURES byte-for-byte, and
re-run the unchanged proof. If the same proof still closes the goal against a
definition that is provably not the original, the statement did not constrain
that definition.

The mutation is run twice with two different sentinel constants. A statement
that is satisfied, by the same proof, by two mutually distinct constant
definitions cannot be pinning those definitions down at all.

THE CLASS OF STATEMENTS COVERED
===============================
This check is deliberately narrow. It applies ONLY when every one of the
following holds, and withholds otherwise:

  1. The target is a top-level `theorem` or `lemma`, found exactly once in the
     file, whose declaration contains a `:=` at bracket depth zero separating
     the header from the proof.
  2. The proof mentions no `sorry`, `admit`, or `stop`.
  3. The header names at least one definition declared, earlier in the same
     file, by `def` or `noncomputable def`.
  4. EVERY such same-file definition named by the header is mutable, meaning it
     matches the restricted shape

         [noncomputable] def NAME (b1 : T1) (b2 : T2) ... : RET :=  body

     with binder groups that contain no nested parentheses and a return type
     RET that is a single identifier token.
  5. RET names a `structure` declared in the same file with the plain form
     `structure RET where`, no parameters, no `extends`, at least one field,
     and every field typed by a single identifier drawn from a small numeric
     whitelist for which an integer literal is a legal inhabitant.

Anything else, including any statement whose definitions cannot all be
replaced, is WITHHELD. Partial mutation is never attempted: mutating only some
of the definitions a statement mentions could indict a theorem that genuinely
constrains the rest.

VERDICTS, AND WHY NONE OF THEM MEANS "GOOD"
===========================================
  `trivial`            An accusation. The mutants elaborated and the unchanged
                       proof closed them.
  `not_shown_trivial`  The check ran and did not produce evidence. This is NOT
                       a certificate of meaning. It is the same posture as
                       `formally_verified: false` in the search stages.
  `withheld`           The check could not be applied, or something was
                       ambiguous. Silence, never suspicion.

There is deliberately no verdict meaning "this statement is good". Surviving
mutation is not evidence of content; it is the absence of evidence of emptiness.

FAILING CLOSED
==============
A mutant that fails to elaborate proves NOTHING about triviality, so the run is
staged to keep those two situations apart:

  baseline  The truncated, UNMUTATED prefix must compile. If it does not, the
            toolchain or the file is at fault, not the statement: withheld.
  stage A   The mutant with the proof replaced by `sorry`. This asks only
            whether the mutated statement is still well-typed. A failure here
            means the mutation itself was inapplicable: withheld.
  stage B   The mutant with the original proof. Only a stage-B success, on
            every sentinel, is evidence, and only then is `trivial` returned.

So a stage-B failure and an unrelated type error can never be confused: the
former is reachable only after stage A has already certified the mutated
statement elaborates.

SAFETY
======
Corpus content is untrusted data. This module reads Lean sources and writes
mutants into a caller-supplied scratch directory. It never executes anything
from the source tree; the only process it launches is the Lean elaborator, on a
file this module wrote, under a hard timeout.

Public API:
    plan_mutation(source, theorem_name) -> dict
    render_mutant(source, plan, sentinel, with_proof=True) -> str
    check_statement_triviality(...) -> dict

CLI:
    python -m theoremata_tools.statement_triviality request.json
    echo '{...}' | python -m theoremata_tools.statement_triviality
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

VERDICT_TRIVIAL = "trivial"
VERDICT_NOT_SHOWN_TRIVIAL = "not_shown_trivial"
VERDICT_WITHHELD = "withheld"

# Field types for which a bare nonnegative integer literal is a legal
# inhabitant. Kept short on purpose: a wrong guess here shows up as a stage-A
# failure (withheld), but every entry we cannot justify is a wasted Lean run.
LITERAL_FIELD_TYPES = frozenset(
    {"Int", "Nat", "Real", "Float", "Rat", "ℤ", "ℕ", "ℝ", "ℚ"}
)

# Two constants, mutually distinct, so that a `trivial` verdict rests on the
# statement admitting two different definitions rather than on one lucky value.
SENTINELS = (424242, 909091)

# Proof-hole tokens. Their presence makes any triviality claim meaningless,
# because the goal was never closed in the first place.
PROOF_HOLE_RE = re.compile(r"(?<![A-Za-z0-9_'])(sorry|admit|stop)(?![A-Za-z0-9_'])")

_DECL_KEYWORDS = (
    "def",
    "abbrev",
    "theorem",
    "lemma",
    "structure",
    "inductive",
    "instance",
    "example",
    "axiom",
    "opaque",
    "class",
)

_MODIFIERS = ("noncomputable", "private", "protected", "partial", "unsafe", "scoped")

_DECL_LINE_RE = re.compile(
    r"^(?P<mods>(?:(?:" + "|".join(_MODIFIERS) + r")\s+)*)"
    r"(?P<kw>" + "|".join(_DECL_KEYWORDS) + r")"
    r"(?![A-Za-z0-9_'])"
    r"(?:\s+(?P<name>[A-Za-z_][A-Za-z0-9_.'!?]*))?"
)

# Lines that end a declaration without starting one we care about.
_BOUNDARY_LINE_RE = re.compile(
    r"^(?:namespace|end|section|open|import|set_option|attribute|variable|universe|@\[|/-|--|#)"
)

_FIELD_RE = re.compile(r"^\s+(?P<name>[A-Za-z_][A-Za-z0-9_']*)\s*:\s*(?P<type>[^\s]+)\s*$")

# The restricted mutable-def shape. Binder groups may not nest parentheses and
# the return type must be one identifier token, so the colon this matches is
# unambiguously the return-type colon and not a binder's.
def _def_header_re(name: str) -> re.Pattern[str]:
    return re.compile(
        r"^(?P<mods>(?:(?:" + "|".join(_MODIFIERS) + r")\s+)*)"
        r"def\s+" + re.escape(name) + r"\s*"
        r"(?P<binders>(?:\([^()]*\)\s*)*)"
        r":\s*(?P<ret>[A-Za-z_][A-Za-z0-9_.']*)\s*:=",
        re.S,
    )


# ---------------------------------------------------------------------------
# Comment-aware scanning
# ---------------------------------------------------------------------------


def _comment_mask(text: str) -> list[bool]:
    """Return a per-character mask that is True inside a Lean comment.

    Needed because a prose block comment can contain lines that look exactly
    like declarations, and both fixtures in this repo have such prose. Treating
    a sentence in a docstring as a `theorem` would silently mis-slice the file.
    """
    mask = [False] * len(text)
    i = 0
    n = len(text)
    depth = 0
    while i < n:
        if depth == 0 and text.startswith("--", i):
            j = text.find("\n", i)
            j = n if j < 0 else j
            for k in range(i, j):
                mask[k] = True
            i = j
            continue
        if text.startswith("/-", i):
            depth += 1
            mask[i] = mask[min(i + 1, n - 1)] = True
            i += 2
            continue
        if depth > 0 and text.startswith("-/", i):
            depth -= 1
            mask[i] = mask[min(i + 1, n - 1)] = True
            i += 2
            continue
        if depth > 0:
            mask[i] = True
        i += 1
    return mask


def _line_starts(text: str) -> list[int]:
    starts = [0]
    for m in re.finditer(r"\n", text):
        starts.append(m.end())
    return starts


def _depth_zero_token(text: str, token: str, start: int, end: int, mask: list[bool]) -> int:
    """Offset of the first `token` at bracket depth zero in [start, end), or -1."""
    openers = "([{"
    closers = ")]}"
    depth = 0
    i = start
    while i < end:
        if mask[i]:
            i += 1
            continue
        c = text[i]
        if c in openers or c == "⟨":
            depth += 1
        elif c in closers or c == "⟩":
            depth -= 1
        elif depth == 0 and text.startswith(token, i):
            return i
        i += 1
    return -1


# ---------------------------------------------------------------------------
# Declaration slicing
# ---------------------------------------------------------------------------


def _slice_declarations(text: str) -> list[dict]:
    """Split a Lean source into top-level declaration spans.

    A declaration runs from its keyword line at column zero to the next line at
    column zero that starts another declaration or a boundary construct.
    """
    mask = _comment_mask(text)
    starts = _line_starts(text)

    marks: list[tuple[int, dict | None]] = []
    for off in starts:
        # A comment opening at column zero ends the preceding declaration, so it
        # must register as a boundary even though every byte of it is masked.
        opens_comment = text.startswith("/-", off) or text.startswith("--", off)
        if off < len(text) and mask[off] and not opens_comment:
            continue
        if opens_comment:
            marks.append((off, None))
            continue
        line_end = text.find("\n", off)
        line_end = len(text) if line_end < 0 else line_end
        line = text[off:line_end]
        m = _DECL_LINE_RE.match(line)
        if m and m.group("name"):
            marks.append((off, {"kw": m.group("kw"), "name": m.group("name"), "start": off}))
        elif _BOUNDARY_LINE_RE.match(line):
            marks.append((off, None))

    decls: list[dict] = []
    for idx, (off, info) in enumerate(marks):
        if info is None:
            continue
        end = marks[idx + 1][0] if idx + 1 < len(marks) else len(text)
        info = dict(info)
        info["end"] = end
        decls.append(info)
    return decls


def _parse_structures(text: str, decls: list[dict]) -> dict[str, dict]:
    """Collect same-file structures that admit an all-literal instance."""
    out: dict[str, dict] = {}
    for d in decls:
        if d["kw"] != "structure":
            continue
        body = text[d["start"] : d["end"]]
        first_nl = body.find("\n")
        head = body[: first_nl if first_nl >= 0 else len(body)]
        # Only the plain parameterless form. Parameters or `extends` change what
        # a constant instance would even mean, so they are out of scope.
        if not re.fullmatch(r"structure\s+" + re.escape(d["name"]) + r"\s+where\s*", head):
            out[d["name"]] = {"supported": False, "reason": "structure head not `structure N where`"}
            continue
        fields: list[tuple[str, str]] = []
        supported = True
        reason = ""
        for raw in body[first_nl + 1 :].splitlines() if first_nl >= 0 else []:
            if not raw.strip() or raw.strip().startswith("--"):
                continue
            fm = _FIELD_RE.match(raw)
            if not fm:
                supported = False
                reason = f"unparsed field line: {raw.strip()[:60]!r}"
                break
            if fm.group("type") not in LITERAL_FIELD_TYPES:
                supported = False
                reason = f"field {fm.group('name')} has non-literal type {fm.group('type')}"
                break
            fields.append((fm.group("name"), fm.group("type")))
        if supported and not fields:
            supported = False
            reason = "structure has no fields, so both sentinels would coincide"
        out[d["name"]] = {
            "supported": supported,
            "reason": reason,
            "fields": fields,
        }
    return out


def _withheld(reason: str, **extra) -> dict:
    return {"ok": True, "verdict": VERDICT_WITHHELD, "reason": reason, **extra}


# ---------------------------------------------------------------------------
# Planning
# ---------------------------------------------------------------------------


def plan_mutation(source: str, theorem_name: str) -> dict:
    """Decide whether the covered class applies, and if so how to mutate.

    Pure: touches no filesystem and runs no Lean. Returns either a withheld
    result or a plan carrying the exact byte spans to rewrite.
    """
    text = source
    mask = _comment_mask(text)
    decls = _slice_declarations(text)

    targets = [d for d in decls if d["kw"] in ("theorem", "lemma") and d["name"] == theorem_name]
    if len(targets) != 1:
        return _withheld(
            f"expected exactly one top-level theorem named {theorem_name!r}, found {len(targets)}"
        )
    thm = targets[0]

    sep = _depth_zero_token(text, ":=", thm["start"], thm["end"], mask)
    if sep < 0:
        return _withheld(
            "target theorem has no `:=` at bracket depth zero, so header and proof "
            "cannot be separated (tactic blocks introduced some other way are out of scope)"
        )

    name_end = text.index(theorem_name, thm["start"]) + len(theorem_name)
    header = text[name_end:sep]
    proof = text[sep + 2 : thm["end"]]

    if PROOF_HOLE_RE.search(proof):
        return _withheld("proof contains a hole (sorry/admit/stop); triviality is not the defect here")

    structures = _parse_structures(text, decls)
    defs_by_name = {d["name"]: d for d in decls if d["kw"] == "def" and d["start"] < thm["start"]}

    # Identifiers used in the header, excluding field projections, which are
    # preceded by a dot and are never top-level definition names.
    used = {m.group(1) for m in re.finditer(r"(?<![.\w'])([A-Za-z_][A-Za-z0-9_']*)", header)}
    referenced = sorted(n for n in used if n in defs_by_name)

    if not referenced:
        return _withheld(
            "statement names no definition declared earlier in this file, so there is "
            "nothing file-local to mutate"
        )

    plan_defs = []
    for name in referenced:
        d = defs_by_name[name]
        decl_text = text[d["start"] : d["end"]]
        hm = _def_header_re(name).match(decl_text)
        if not hm:
            return _withheld(
                f"definition {name!r} referenced by the statement does not match the "
                "covered def shape (simple parenthesised binders, single-identifier "
                "return type); refusing to mutate only part of the statement"
            )
        ret = hm.group("ret")
        st = structures.get(ret)
        if st is None:
            return _withheld(
                f"definition {name!r} returns {ret!r}, which is not a structure declared "
                "in this file"
            )
        if not st["supported"]:
            return _withheld(f"return structure {ret!r} is out of scope: {st['reason']}")
        plan_defs.append(
            {
                "name": name,
                "return_type": ret,
                "fields": st["fields"],
                "body_start": d["start"] + hm.end(),
                "decl_end": d["end"],
            }
        )

    return {
        "ok": True,
        "verdict": None,
        "theorem": theorem_name,
        "theorem_start": thm["start"],
        "theorem_end": thm["end"],
        "header_end": sep,
        "proof_start": sep + 2,
        "mutated_defs": plan_defs,
        "trailing_scopes": _open_scopes(text[: thm["end"]], mask),
    }


def _open_scopes(prefix: str, mask: list[bool]) -> list[str]:
    """Namespaces and sections still open at the end of `prefix`.

    Truncating the file after the target theorem drops the file's own `end`
    lines, and Lean rejects a source with an unclosed scope. The mutant has to
    re-close them itself.
    """
    stack: list[str] = []
    for off in _line_starts(prefix):
        if off < len(mask) and mask[off]:
            continue
        line_end = prefix.find("\n", off)
        line_end = len(prefix) if line_end < 0 else line_end
        line = prefix[off:line_end].rstrip()
        m = re.match(r"^namespace\s+([A-Za-z_][A-Za-z0-9_.']*)", line)
        if m:
            stack.append(m.group(1))
            continue
        m = re.match(r"^section(?:\s+([A-Za-z_][A-Za-z0-9_']*))?\s*$", line)
        if m:
            stack.append(m.group(1) or "")
            continue
        if re.match(r"^end(?![A-Za-z0-9_'])", line) and stack:
            stack.pop()
    return stack


# ---------------------------------------------------------------------------
# Rendering
# ---------------------------------------------------------------------------


def _constant_body(fields: list[tuple[str, str]], sentinel: int, slot: int) -> str:
    """One mutated definition body, with a DISTINCT constant per field slot.

    Every slot must differ, across fields and across definitions alike. Giving
    them all one value is a false-positive generator: a relational statement
    such as `|mu| >= |lambda|` over two definitions collapses both sides to the
    same constant, the relation then holds reflexively, and a substantive
    theorem is reported trivial. Two mutually distinct sentinel RUNS defend
    against a single lucky value but not against that co-mutation, because both
    sides move together in each run.

    Distinctness cannot cause a missed detection in the other direction: a
    statement that is genuinely trivial is true for ANY values, so it stays
    trivial when the values differ. The change is therefore strictly
    discriminating.
    """
    parts = []
    for i, (fname, ftype) in enumerate(fields):
        parts.append(f"{fname} := ({sentinel + slot + i} : {ftype})")
    return " { " + ", ".join(parts) + " }"


def render_mutant(source: str, plan: dict, sentinel: int, with_proof: bool = True) -> str:
    """Build one mutant source.

    The file is truncated just after the target theorem, because declarations
    that follow it are irrelevant and a later theorem broken by the mutation
    would contaminate the compile result. Every mutated definition keeps its
    signature verbatim; only the body is replaced, which is what makes the
    mutant well-typed by construction whenever the original was.
    """
    text = source
    end = plan["theorem_end"]

    if with_proof:
        pieces = [text[:end]]
    else:
        # Stage A asks only whether the mutated STATEMENT elaborates.
        pieces = [text[: plan["proof_start"]] + "\n  sorry\n"]
    body = pieces[0]

    # Slot bases are assigned in the plan's own declaration order, NOT in the
    # reversed edit order below, so a definition always receives the same
    # constants no matter how the edits happen to be sequenced. Spacing by the
    # field count keeps every slot in the file distinct.
    slot_base: dict[int, int] = {}
    next_slot = 0
    for d in plan["mutated_defs"]:
        slot_base[id(d)] = next_slot
        next_slot += max(1, len(d["fields"]))

    edits = sorted(plan["mutated_defs"], key=lambda d: d["body_start"], reverse=True)
    for d in edits:
        stop = min(d["decl_end"], len(body))
        if d["body_start"] >= len(body):
            continue
        body = (
            body[: d["body_start"]]
            + _constant_body(d["fields"], sentinel, slot_base[id(d)])
            + "\n\n"
            + body[stop:]
        )

    tail = ""
    for scope in reversed(plan.get("trailing_scopes") or []):
        tail += f"\nend {scope}" if scope else "\nend"
    return body.rstrip() + "\n" + tail + "\n"


# ---------------------------------------------------------------------------
# Lean driving
# ---------------------------------------------------------------------------


def _prepend_elan() -> None:
    """Put elan's shims on PATH. They are how a Windows box reaches lean/lake."""
    elan_bin = Path(os.path.expanduser("~")) / ".elan" / "bin"
    if elan_bin.is_dir():
        cur = os.environ.get("PATH", "")
        if str(elan_bin) not in cur.split(os.pathsep):
            os.environ["PATH"] = str(elan_bin) + os.pathsep + cur


def _resolve(exe: str) -> str | None:
    _prepend_elan()
    return shutil.which(exe)


def lean_available() -> bool:
    """Capability probe, so tests can skip rather than fail where Lean is absent."""
    exe = _resolve("lean")
    if not exe:
        return False
    try:
        proc = subprocess.run([exe, "--version"], capture_output=True, text=True, timeout=120)
    except Exception:
        return False
    return proc.returncode == 0


def _elaborate(path: str, lake_workspace: str | None, timeout: float) -> dict:
    """Run the elaborator on one file. Success requires rc 0 and no `error:`.

    Warnings, including `declaration uses 'sorry'`, are tolerated; only errors
    and nonzero exit disqualify a compile.
    """
    if lake_workspace:
        # `lake env` from a workspace supplies LEAN_PATH for its dependencies,
        # which is the only way a Mathlib-importing source elaborates.
        cmd = [_resolve("lake") or "lake", "env", "lean", os.path.abspath(path)]
        cwd = lake_workspace
    else:
        cmd = [_resolve("lean") or "lean", os.path.abspath(path)]
        cwd = os.path.dirname(os.path.abspath(path)) or None

    try:
        proc = subprocess.run(
            cmd, cwd=cwd, capture_output=True, text=True, timeout=timeout
        )
    except subprocess.TimeoutExpired:
        return {"ok": False, "timed_out": True, "returncode": None, "output": "", "command": cmd}
    except FileNotFoundError as exc:
        return {
            "ok": False,
            "timed_out": False,
            "returncode": None,
            "output": str(exc),
            "command": cmd,
        }

    out = (proc.stdout or "") + (proc.stderr or "")
    return {
        "ok": proc.returncode == 0 and "error:" not in out,
        "timed_out": False,
        "returncode": proc.returncode,
        "output": out,
        "command": cmd,
    }


# ---------------------------------------------------------------------------
# The check
# ---------------------------------------------------------------------------


def check_statement_triviality(
    source_path: str,
    theorem_name: str,
    work_dir: str | None = None,
    lake_workspace: str | None = None,
    timeout: float = 300.0,
    keep_artifacts: bool = False,
) -> dict:
    """Run the full staged check. Never raises on Lean failure; withholds."""
    src_file = Path(source_path)
    if not src_file.is_file():
        return _withheld(f"source file not found: {source_path}")
    source = src_file.read_text(encoding="utf-8")

    plan = plan_mutation(source, theorem_name)
    if plan.get("verdict") == VERDICT_WITHHELD:
        return plan

    tmp_owner = None
    if work_dir is None:
        tmp_owner = tempfile.mkdtemp(prefix="stmt_triviality_")
        work_dir = tmp_owner
    work = Path(work_dir)
    work.mkdir(parents=True, exist_ok=True)

    stages: list[dict] = []
    try:
        # Baseline. If the unmutated prefix does not compile here, nothing that
        # follows can be attributed to the mutation.
        baseline_plan = dict(plan)
        baseline_plan["mutated_defs"] = []
        baseline_src = render_mutant(source, baseline_plan, SENTINELS[0], with_proof=True)
        baseline_file = work / "baseline.lean"
        baseline_file.write_text(baseline_src, encoding="utf-8")
        baseline = _elaborate(str(baseline_file), lake_workspace, timeout)
        stages.append({"stage": "baseline", **baseline})
        if not baseline["ok"]:
            return _withheld(
                "baseline (unmutated, truncated) source does not elaborate here, so no "
                "conclusion about the statement can be drawn",
                stages=stages,
                theorem=theorem_name,
            )

        for sentinel in SENTINELS:
            stmt_src = render_mutant(source, plan, sentinel, with_proof=False)
            stmt_file = work / f"mutant_{sentinel}_stmt.lean"
            stmt_file.write_text(stmt_src, encoding="utf-8")
            a = _elaborate(str(stmt_file), lake_workspace, timeout)
            stages.append({"stage": "A", "sentinel": sentinel, "file": str(stmt_file), **a})
            if not a["ok"]:
                return _withheld(
                    f"mutant statement for sentinel {sentinel} did not elaborate; the "
                    "mutation is inapplicable and proves nothing about triviality",
                    stages=stages,
                    theorem=theorem_name,
                    mutated_defs=[d["name"] for d in plan["mutated_defs"]],
                )

            full_src = render_mutant(source, plan, sentinel, with_proof=True)
            full_file = work / f"mutant_{sentinel}_full.lean"
            full_file.write_text(full_src, encoding="utf-8")
            b = _elaborate(str(full_file), lake_workspace, timeout)
            stages.append({"stage": "B", "sentinel": sentinel, "file": str(full_file), **b})
            if not b["ok"]:
                return {
                    "ok": True,
                    "verdict": VERDICT_NOT_SHOWN_TRIVIAL,
                    "reason": (
                        f"the unchanged proof did not close the mutated goal for sentinel "
                        f"{sentinel}; this is not a certificate that the statement has "
                        "content, only an absence of evidence that it does not"
                    ),
                    "theorem": theorem_name,
                    "mutated_defs": [d["name"] for d in plan["mutated_defs"]],
                    "stages": stages,
                }

        names = [d["name"] for d in plan["mutated_defs"]]
        return {
            "ok": True,
            "verdict": VERDICT_TRIVIAL,
            "reason": (
                f"the statement of {theorem_name!r} is closed by its own unchanged proof "
                f"after replacing {', '.join(names)} with each of two mutually distinct "
                "constants, so the statement does not constrain those definitions"
            ),
            "theorem": theorem_name,
            "mutated_defs": names,
            "sentinels": list(SENTINELS),
            "stages": stages,
        }
    finally:
        if tmp_owner and not keep_artifacts:
            shutil.rmtree(tmp_owner, ignore_errors=True)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def run(request: dict) -> dict:
    """Worker entrypoint (key ``statement_triviality``).

    ``{op?, source_path, theorem_name, work_dir?, lake_workspace?, timeout?}``
    -> a verdict dict. Callers gating on this MUST branch on
    ``verdict == "trivial"`` and treat everything else as no signal; neither
    ``not_shown_trivial`` nor ``withheld`` may be read as approval.
    """
    return _handle(request)


def _handle(req: dict) -> dict:
    op = req.get("op", "check")
    if op == "plan":
        src = req.get("source")
        if src is None:
            src = Path(req["source_path"]).read_text(encoding="utf-8")
        return plan_mutation(src, req["theorem_name"])
    if op == "check":
        return check_statement_triviality(
            req["source_path"],
            req["theorem_name"],
            work_dir=req.get("work_dir"),
            lake_workspace=req.get("lake_workspace"),
            timeout=float(req.get("timeout", 300.0)),
            keep_artifacts=bool(req.get("keep_artifacts", False)),
        )
    raise ValueError(f"unknown op: {op!r}")


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    raw = Path(argv[0]).read_text(encoding="utf-8") if argv and argv[0] not in ("-", "") else sys.stdin.read()
    try:
        result = _handle(json.loads(raw))
    except Exception as exc:
        print(json.dumps({"ok": False, "error": str(exc), "error_type": type(exc).__name__}))
        return 0
    print(json.dumps(result, default=str))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
