"""LEAN-GitHub-style corpus extraction: mine (theorem, tactic-sequence) training
records from a directory of Lean 4 source files and emit training JSONL for the
SFT flywheel.

Ports the *data-ingestion* half of the LEAN-GitHub pipeline
(``docs/paper-mining/lean-github.md``): enumerate ``*.lean`` files, extract
``theorem``/``lemma``/``example`` declarations with their statement and tactic
block, build a lightweight global **import graph** from ``import`` lines, and
**state-dedup** alpha-equivalent statements by kernel-order-ish hypothesis
renaming (LEAN-GitHub reports >50% of intermediate states are alpha-rename
duplicates). The verified records are emitted in the flywheel's shared
:data:`SFT_SCHEMA` chat shape so they drop straight into ``progress_sft``.

HONEST SCOPE — PARSE-BASED, NOT KERNEL-VERIFIED
-----------------------------------------------
LEAN-GitHub bypasses Lake and calls ``leanc`` directly to get real proof STATES
via an AST pass. That step is live-gated in Theoremata (no ``leanc``/``lake`` on
this box), so this module does a **purely syntactic parse**: regex/line-based
extraction robust to comments and strings. It therefore captures the
*syntactic tactic sequence* a human wrote — NOT kernel-verified intermediate
proof states. The (DECL, GOAL, PROOFSTEP) triple degrades to
(statement, statement, first-tactic): we have the declaration and the tactic
text but not the elaborated goal between steps. All Lean/repo content is treated
as UNTRUSTED DATA: it is parsed, never executed or imported.

Offline / deterministic / pure stdlib: no network, no build, no model.
"""
from __future__ import annotations

import json
import os
import re
from dataclasses import dataclass, field
from typing import Any, Optional

# Reuse the flywheel's chat-SFT contract when importable; fall back to the
# literal constant so this module also works standalone.
try:  # pragma: no cover - trivial import shim
    from theoremata_tools.flywheel import SFT_SCHEMA
except Exception:  # pragma: no cover
    SFT_SCHEMA = "theoremata.chat-sft.v1"

CORPUS_SOURCE = "lean_corpus"

DECL_KEYWORDS = ("theorem", "lemma", "example")


# ---------------------------------------------------------------------------
# Comment / string scrubbing (robust parsing prep)
# ---------------------------------------------------------------------------

def strip_comments_and_strings(text: str) -> str:
    """Blank out Lean line comments (``--``), nested block comments (``/- -/``),
    and string literals so downstream regexes never trip on ``theorem`` inside a
    comment or ``:= by`` inside a string. Newlines are preserved (comments/strings
    are replaced with spaces of equal length is unnecessary; we keep newlines so
    line structure survives). Returns a same-shape scrubbed copy."""
    out: list[str] = []
    i = 0
    n = len(text)
    depth = 0  # nested block-comment depth
    in_string = False
    while i < n:
        c = text[i]
        two = text[i : i + 2]
        if depth > 0:
            # inside a (possibly nested) block comment
            if two == "/-":
                depth += 1
                i += 2
                continue
            if two == "-/":
                depth -= 1
                i += 2
                continue
            out.append("\n" if c == "\n" else " ")
            i += 1
            continue
        if in_string:
            if c == "\\" and i + 1 < n:
                out.append("  ")
                i += 2
                continue
            if c == '"':
                in_string = False
                out.append(" ")
                i += 1
                continue
            out.append("\n" if c == "\n" else " ")
            i += 1
            continue
        # not in comment or string
        if two == "/-":
            depth += 1
            i += 2
            continue
        if two == "--":
            # line comment: consume to end of line
            while i < n and text[i] != "\n":
                i += 1
            continue
        if c == '"':
            in_string = True
            out.append(" ")
            i += 1
            continue
        out.append(c)
        i += 1
    return "".join(out)


# ---------------------------------------------------------------------------
# Declaration + tactic extraction
# ---------------------------------------------------------------------------

@dataclass
class Decl:
    """One parsed declaration."""

    kind: str
    name: str
    statement: str
    body: str
    is_tactic: bool
    tactics: list[str] = field(default_factory=list)
    module: str = ""
    file: str = ""

    @property
    def first_tactic(self) -> str:
        return self.tactics[0] if self.tactics else ""


_DECL_RE = re.compile(r"\b(theorem|lemma|example)\b")
_NAME_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_.'!?]*")

# open-bracket -> matching close, for depth tracking
_OPEN = {"(": ")", "[": "]", "{": "}", "⟨": "⟩", "⦃": "⦄"}
_CLOSE = set(_OPEN.values())


def _find_top_level_assign(s: str) -> int:
    """Index of the first ``:=`` at bracket depth 0 in ``s``, or -1."""
    depth = 0
    i = 0
    n = len(s)
    while i < n:
        c = s[i]
        if c in _OPEN:
            depth += 1
        elif c in _CLOSE:
            depth = max(0, depth - 1)
        elif depth == 0 and s[i : i + 2] == ":=":
            return i
        i += 1
    return -1


def split_tactics(body: str) -> list[str]:
    """Split a ``by`` tactic block into individual tactic steps.

    Splits on newlines and top-level ``;`` / ``<;>`` combinators (bracket-depth
    aware so ``simp [a, b]`` stays one step). Purely syntactic — steps are the
    tactic *text*, not elaborated goal states."""
    b = body.strip()
    if b.startswith("by"):
        b = b[2:]
    steps: list[str] = []
    depth = 0
    cur: list[str] = []

    def flush() -> None:
        t = "".join(cur).strip()
        if t:
            steps.append(t)
        cur.clear()

    i = 0
    n = len(b)
    while i < n:
        c = b[i]
        if c in _OPEN:
            depth += 1
            cur.append(c)
            i += 1
            continue
        if c in _CLOSE:
            depth = max(0, depth - 1)
            cur.append(c)
            i += 1
            continue
        if depth == 0:
            if c == "\n":
                flush()
                i += 1
                continue
            if b[i : i + 3] == "<;>":
                flush()
                i += 3
                continue
            if c == ";":
                flush()
                i += 1
                continue
        cur.append(c)
        i += 1
    flush()
    return steps


def parse_decls(text: str) -> list[Decl]:
    """Parse all ``theorem``/``lemma``/``example`` declarations from Lean source
    ``text``. Comments and strings are scrubbed first, so declarations hidden in
    comments are ignored."""
    scrubbed = strip_comments_and_strings(text)
    decls: list[Decl] = []
    starts = [m.start() for m in _DECL_RE.finditer(scrubbed)]
    for idx, start in enumerate(starts):
        end = starts[idx + 1] if idx + 1 < len(starts) else len(scrubbed)
        chunk = scrubbed[start:end]
        kw_match = _DECL_RE.match(chunk)
        if not kw_match:
            continue
        kind = kw_match.group(1)
        rest = chunk[kw_match.end():]
        # name (optional for `example`)
        name = ""
        sig_rest = rest
        nm = _NAME_RE.match(rest.lstrip())
        if kind != "example" and nm:
            name = nm.group(0)
            sig_rest = rest.lstrip()[nm.end():]
        assign = _find_top_level_assign(sig_rest)
        if assign < 0:
            statement = _norm_ws(sig_rest)
            body = ""
        else:
            statement = _norm_ws(sig_rest[:assign])
            body = sig_rest[assign + 2:].strip()
        statement = statement.lstrip(": ").strip() if statement.startswith(":") else statement.strip()
        is_tactic = body.strip().startswith("by")
        tactics = split_tactics(body) if is_tactic else []
        if not statement:
            continue
        decls.append(
            Decl(
                kind=kind,
                name=name,
                statement=statement,
                body=body,
                is_tactic=is_tactic,
                tactics=tactics,
            )
        )
    return decls


def _norm_ws(s: str) -> str:
    return re.sub(r"\s+", " ", s).strip()


# ---------------------------------------------------------------------------
# Import graph
# ---------------------------------------------------------------------------

_IMPORT_RE = re.compile(r"^\s*import\s+([A-Za-z0-9_.]+)", re.MULTILINE)


def parse_imports(text: str) -> list[str]:
    """All imported module names from ``import Foo.Bar`` lines (comment-safe)."""
    scrubbed = strip_comments_and_strings(text)
    return _IMPORT_RE.findall(scrubbed)


def module_name(root: str, path: str) -> str:
    """Module name for a Lean file: path relative to ``root`` with separators
    turned into dots and the ``.lean`` suffix removed (LEAN-GitHub-style)."""
    rel = os.path.relpath(path, root)
    rel = rel[:-5] if rel.lower().endswith(".lean") else rel
    return rel.replace(os.sep, ".").replace("/", ".")


# ---------------------------------------------------------------------------
# State de-duplication: kernel-order-ish hypothesis renaming
# ---------------------------------------------------------------------------

# binder groups: (a b : T)  {a : T}  [inst : T]  ⦃a : T⦄
_BINDER_GROUP_RE = re.compile(r"[\(\{\[⦃]\s*([^:()\[\]{}⦃⦄]+?)\s*:")
# quantifier / lambda binders: ∀ a b,  ∃ x,  fun a =>  λ a,
_QUANT_RE = re.compile(r"(?:∀|∃|Σ|Π|λ|\bfun\b)\s+([^,:=⇒]+?)\s*(?:,|:|=>|⇒)")

_IDENT_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_']*")


def _binder_names(statement: str) -> list[str]:
    """Collect local/bound variable names introduced by binders and quantifiers,
    in order of first appearance. These are the names that are alpha-renameable."""
    names: list[str] = []
    seen: set[str] = set()
    for m in list(_BINDER_GROUP_RE.finditer(statement)) + list(_QUANT_RE.finditer(statement)):
        for tok in _IDENT_RE.findall(m.group(1)):
            if tok not in seen:
                seen.add(tok)
                names.append(tok)
    return names


def canonicalize_statement(statement: str) -> str:
    """Return a canonical form of ``statement`` with binder/local names
    alpha-renamed to canonical tokens ``v0, v1, ...`` in kernel-order-ish
    (first-appearance) order, and whitespace normalized. Two statements that
    differ only by the names of their bound variables canonicalize to the same
    string. This is the parse-based analogue of LEAN-GitHub's kernel-order
    hypothesis renaming — it identifies alpha-equivalent statements for dedup."""
    stmt = _norm_ws(statement)
    binders = _binder_names(stmt)
    if not binders:
        return stmt
    # Assign canonical ids by order of first textual appearance in the statement.
    order: dict[str, int] = {}

    def _first_pos(name: str) -> int:
        m = re.search(r"\b" + re.escape(name) + r"\b", stmt)
        return m.start() if m else 1 << 30

    for name in sorted(set(binders), key=_first_pos):
        order[name] = len(order)
    # Whole-word replace each binder name with its canonical token. Placeholder
    # collisions are avoided with a sentinel prefix removed at the end.
    def _sub(match: re.Match) -> str:
        tok = match.group(0)
        if tok in order:
            return f"\x00v{order[tok]}\x00"
        return tok

    out = _IDENT_RE.sub(_sub, stmt)
    return out.replace("\x00", "")


# ---------------------------------------------------------------------------
# Record emission (flywheel chat-sft.v1)
# ---------------------------------------------------------------------------

def decl_to_record(decl: Decl, *, variant: str = "proof") -> dict[str, Any]:
    """Turn one :class:`Decl` into a flywheel :data:`SFT_SCHEMA` chat row.

    CANONICAL SCHEMA (``variant="proof"``): ``user=statement`` ->
    ``assistant=full proof body``. The tactic-STATE variant
    (``variant="first_tactic"``) instead targets ``assistant=first tactic`` — the
    parse-based stand-in for LEAN-GitHub's (GOAL -> PROOFSTEP) step objective
    (we have the statement and the tactic text, but no elaborated intermediate
    goal state without a live kernel). Both share the identical chat envelope;
    ``meta`` records which variant produced the row plus full provenance."""
    if variant == "first_tactic":
        target = decl.first_tactic or decl.body
    else:
        target = decl.body or decl.first_tactic
    return {
        "messages": [
            {"role": "user", "content": decl.statement},
            {"role": "assistant", "content": target},
        ],
        "meta": {
            "schema": SFT_SCHEMA,
            "source": CORPUS_SOURCE,
            "variant": variant,
            "kind": decl.kind,
            "decl": decl.name,
            "module": decl.module,
            "file": decl.file,
            "is_tactic": decl.is_tactic,
            "first_tactic": decl.first_tactic,
            "n_tactics": len(decl.tactics),
            "tactics": decl.tactics,
            "note": "parse-based: syntactic tactic sequence, not kernel-verified",
        },
    }


# ---------------------------------------------------------------------------
# Corpus extraction over a directory
# ---------------------------------------------------------------------------

def iter_lean_files(root: str) -> list[str]:
    """All ``*.lean`` files under ``root``, sorted for deterministic ordering."""
    hits: list[str] = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        for fn in sorted(filenames):
            if fn.lower().endswith(".lean"):
                hits.append(os.path.join(dirpath, fn))
    return sorted(hits)


def extract_corpus(
    root: str,
    *,
    variant: str = "proof",
    tactic_only: bool = True,
    dedup: bool = True,
) -> dict[str, Any]:
    """Walk ``root``, parse every Lean file, and produce deduped SFT records +
    a global import graph + stats.

    * ``tactic_only`` — keep only declarations with a ``by`` tactic block (the
      trainable ones); non-tactic (term-mode) proofs are counted but skipped.
    * ``dedup`` — drop declarations whose statement is alpha-equivalent to one
      already seen (kernel-order-ish canonicalization).

    Returns ``{ok, schema, records, import_graph, stats}`` where ``stats`` is
    ``{n_files, n_decls, n_tactic_decls, n_deduped, n_records, import_edges,
    n_modules}``.
    """
    files = iter_lean_files(root)
    import_graph: dict[str, list[str]] = {}
    all_decls: list[Decl] = []
    n_decls = 0
    n_tactic_decls = 0

    for path in files:
        try:
            with open(path, "r", encoding="utf-8", errors="replace") as fh:
                text = fh.read()
        except OSError:
            continue
        mod = module_name(root, path)
        rel = os.path.relpath(path, root).replace(os.sep, "/")
        imports = parse_imports(text)
        if imports or mod not in import_graph:
            import_graph.setdefault(mod, [])
            for dep in imports:
                if dep not in import_graph[mod]:
                    import_graph[mod].append(dep)
        for d in parse_decls(text):
            n_decls += 1
            d.module = mod
            d.file = rel
            if d.is_tactic:
                n_tactic_decls += 1
            all_decls.append(d)

    # Filter + dedup (stable, first-occurrence wins).
    seen_canon: set[str] = set()
    n_deduped = 0
    records: list[dict[str, Any]] = []
    for d in all_decls:
        if tactic_only and not d.is_tactic:
            continue
        if dedup:
            canon = canonicalize_statement(d.statement)
            if canon in seen_canon:
                n_deduped += 1
                continue
            seen_canon.add(canon)
        records.append(decl_to_record(d, variant=variant))

    import_edges = sum(len(v) for v in import_graph.values())
    return {
        "ok": True,
        "schema": SFT_SCHEMA,
        "records": records,
        "import_graph": import_graph,
        "stats": {
            "n_files": len(files),
            "n_decls": n_decls,
            "n_tactic_decls": n_tactic_decls,
            "n_deduped": n_deduped,
            "n_records": len(records),
            "import_edges": import_edges,
            "n_modules": len(import_graph),
        },
    }


def write_jsonl(rows, path: str) -> int:
    """Write ``rows`` as one JSON object per line; returns the count written."""
    count = 0
    with open(path, "w", encoding="utf-8") as fh:
        for row in rows:
            fh.write(json.dumps(row, ensure_ascii=False))
            fh.write("\n")
            count += 1
    return count


# ---------------------------------------------------------------------------
# Worker dispatch
# ---------------------------------------------------------------------------

def run(request: dict[str, Any]) -> dict[str, Any]:
    """JSON op dispatch.

    * ``extract`` — ``{root}`` -> ``{records, import_graph, stats}`` (stats has
      ``n_files, n_decls, n_deduped, import_edges``). Optional ``variant``,
      ``tactic_only``, ``dedup``, and ``with_records`` (default True; set False
      to return only stats + graph).
    * ``emit_jsonl`` — ``{records, path}`` (or ``{root, path}`` to extract then
      emit) -> ``{ok, written, path}``.
    """
    op = request.get("op", "extract")
    if op == "extract":
        res = extract_corpus(
            request["root"],
            variant=request.get("variant", "proof"),
            tactic_only=bool(request.get("tactic_only", True)),
            dedup=bool(request.get("dedup", True)),
        )
        if not request.get("with_records", True):
            res = {kk: vv for kk, vv in res.items() if kk != "records"}
        return res
    if op == "emit_jsonl":
        path = request["path"]
        records = request.get("records")
        if records is None:
            records = extract_corpus(
                request["root"],
                variant=request.get("variant", "proof"),
                tactic_only=bool(request.get("tactic_only", True)),
                dedup=bool(request.get("dedup", True)),
            )["records"]
        written = write_jsonl(records, path)
        return {"ok": True, "written": written, "path": path, "schema": SFT_SCHEMA}
    raise ValueError(f"unknown op: {op}")
