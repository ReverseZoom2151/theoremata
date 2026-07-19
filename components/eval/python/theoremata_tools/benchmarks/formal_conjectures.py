"""Google DeepMind ``formal-conjectures`` as OPEN TARGETS (Tier 4).

Every other Lean corpus we register ships artifacts that are supposed to close.
This one does not. ``formal-conjectures`` is a curated collection of *open*
mathematical conjectures whose Lean statements end in ``sorry`` **by design**:
the conjecture has no proof, so the ``sorry`` is the goal marker, not a cheat.

That is the reason to wire it. Our escape-hatch gate
(``components/prover/statement_preservation.rs``) treats every ``sorry`` as an
open goal admitted with no proof and fails closed, which is exactly right for a
submission that was supposed to close and exactly wrong for a target we were
asked to attempt. Registering this corpus forces the two cases apart:

* an **open target** is an input, a statement handed to the prover, whose
  ``sorry`` marks the goal;
* a **failed attempt** is an output, a submission that claimed to close a goal
  and did not.

This module only *labels* the distinction. It does not change the gate. What the
gate would have to grow is written up in
``docs/resource-mining/new/formal-conjectures.md`` section 7.

The fourth verdict
------------------
``adversarial.py`` defines three expected verdicts: ``expect_accept``,
``expect_reject``, ``expect_accept_conditional``. None of them fits an open
conjecture, and overloading one would destroy the very distinction this corpus
exists to test:

* ``expect_accept`` says "there is a sound artifact here and the gate must
  certify it". There is no artifact at all; the statement is unproved.
* ``expect_reject`` says "refuse this, for a stated reason". Refusing the
  Riemann Hypothesis as a *statement* is not a gate success, it is a category
  error. There is no ``REJECT_REASONS`` member that is true of it.
* ``expect_accept_conditional`` says "accept, and carry the hypotheses". There
  are no hypotheses; the conclusion is simply not established.

So this module defines :data:`EXPECT_OPEN`. It means: a correct response is an
attempt or an honest failure, and **claiming a proof is the failure mode**. Note
what that inverts. For the other three verdicts the artifact is fixed and the
gate's behaviour is under test. Here the *response* is under test and the
strongest available ground truth is a negative: the mathematical community holds
this open, so any response asserting a closed proof is either a breakthrough or
(overwhelmingly) a fabrication, and the harness must treat it as the latter until
a real certificate says otherwise.

Ground truth
------------
Open-vs-solved is carried by the repository's own ``@[category ...]`` attribute,
not by the presence of ``sorry``. ``@[category research solved]`` statements are
*also* ``sorry``-bearing here, because the repo's policy is that proofs longer
than 25 to 50 lines do not belong in it. A ``sorry`` scan therefore cannot tell
the two apart, which is the whole point. We read the attribute and emit items
only for ``research open``.

Untrusted data. ``resources/`` is third-party. This repo additionally ships an
``AGENTS.md`` written as imperative instructions to an AI agent ("CRITICAL
REQUIREMENTS", "MUST pass", "DON'T ..."). It is benign in intent and it is
addressed at contributors, but it is instruction-shaped prose sitting in a data
directory, so it is never ingested here. Only ``.lean`` sources under
``FormalConjectures/`` are read, and every excerpt is wrapped by :func:`_fenced`.

Like every loader in this package, the entry point returns ``[]`` when the corpus
is missing, because ``resources/`` is gitignored and absent in CI.
"""
from __future__ import annotations

import logging
import re
from pathlib import Path
from typing import Any

from .adversarial import EXPECTED_VERDICTS, _fenced
from .resources import find_files, rel
from .schema import make_item

log = logging.getLogger("theoremata.benchmarks")

# --------------------------------------------------------------------------- #
# The fourth verdict
# --------------------------------------------------------------------------- #

#: A statement the mathematical community holds OPEN. A correct response is an
#: attempt or an honest failure; a claimed proof is the failure mode. Deliberately
#: NOT one of the three verdicts in :mod:`.adversarial` -- see the module
#: docstring for why each of those would be a category error here.
EXPECT_OPEN = "expect_open"

#: The three adversarial verdicts plus ours. Exported so a test can assert the
#: vocabulary grew by exactly one and that ``expect_open`` is not silently
#: readable as an accept.
OPEN_TARGET_VERDICTS: frozenset[str] = frozenset(EXPECTED_VERDICTS | {EXPECT_OPEN})

#: How a response to an ``expect_open`` item fails. Recorded on every item so a
#: grader has something to assert against rather than an unstated convention.
CLAIMED_PROOF = "claimed_proof_of_open_conjecture"

TRACK = "open_conjecture"
CORPUS = "formal_conjectures"

#: Category values used by the upstream ``@[category ...]`` attribute. Only the
#: first is emitted as an item; the rest are counted and dropped.
CATEGORY_OPEN = "research open"
CATEGORY_SOLVED = "research solved"
_ALL_CATEGORIES = (
    CATEGORY_OPEN,
    CATEGORY_SOLVED,
    "textbook",
    "API",
    "test",
)

#: Upstream is Apache-2.0 for software; the conjecture *content* is CC-BY-4.0 and
#: some of it derives from third parties on other terms (Wikipedia / MathOverflow
#: / OEIS material is CC-BY-SA-4.0). We reference by path and never vendor.
LICENSE = "Apache-2.0 (code); CC-BY-4.0 (content); some sources CC-BY-SA-4.0"

#: Pinned by ``lean-toolchain`` + ``lakefile.toml`` in the checkout we read.
LEAN_TOOLCHAIN = "leanprover/lean4:v4.27.0"
MATHLIB_REV = "v4.27.0"

_CORPUS_GLOBS = (
    "formal-conjectures-main/**/FormalConjectures/**/*.lean",
)

# --------------------------------------------------------------------------- #
# Parsing
# --------------------------------------------------------------------------- #

# `@[category research open, AMS 5 11]` at column 0, immediately followed by a
# `theorem` / `lemma` / `def` head. Attribute lists in this corpus never nest a
# `]`, and every one of the 1171 category attributes sits at column 0.
_ATTR_DECL = re.compile(
    r"^@\[(?P<attrs>[^\]]*)\]\s*\n?\s*"
    r"(?P<kw>theorem|lemma|def)\s+(?P<name>[^\s:({\[]+)",
    re.MULTILINE,
)

_CATEGORY = re.compile(
    r"category\s+(research\s+open|research\s+solved|textbook|API|test)"
)
_AMS = re.compile(r"AMS((?:\s+\d+)+)")
_FORMAL_PROOF = re.compile(
    r'formal_proof\s+using\s+(?P<kind>\w+)\s+at\s+"(?P<url>[^"]*)"'
)

#: A doc comment ending just before the attribute line.
_DOCSTRING_BEFORE = re.compile(r"/--(?P<body>.*?)-/\s*\Z", re.DOTALL)

#: Module docstring `/-! ... -/` -- the file-level title and reference list.
_MODULE_DOC = re.compile(r"/-!(?P<body>.*?)-/", re.DOTALL)

#: The goal marker we strip to recover a bare statement. Deliberately NOT anchored
#: to the end of the block: a declaration can be followed inside the same block by
#: an `alias`, so anchoring silently left the `sorry` in the statement (caught by
#: `GreensOpenProblems/72.lean`, whose `green_72` is followed by
#: `alias no_three_in_line := green_72`). `answer(sorry)` is untouched because it
#: is not preceded by `:=`.
_PROOF_TAIL = re.compile(r":=\s*(?:by\s*)?sorry\b")

#: Where a declaration ends: the next column-0 construct.
_NEXT_TOP_LEVEL = re.compile(
    r"^(?:@\[|/--|/-!|theorem\s|lemma\s|def\s|abbrev\s|noncomputable\s|instance\s"
    r"|structure\s|inductive\s|class\s|namespace\s|end\s|open\s|section\s"
    r"|variable\s|local\s|import\s|alias\s|attribute\s|example\s|#)",
    re.MULTILINE,
)


def _category(attrs: str) -> str | None:
    m = _CATEGORY.search(attrs)
    if not m:
        return None
    return re.sub(r"\s+", " ", m.group(1))


def _ams(attrs: str) -> list[int]:
    out: list[int] = []
    for m in _AMS.finditer(attrs):
        out.extend(int(tok) for tok in m.group(1).split())
    return sorted(set(out))


def _declaration_block(src: str, start: int) -> str:
    """Source of one declaration: from ``start`` to the next column-0 construct."""
    nxt = _NEXT_TOP_LEVEL.search(src, start + 1)
    return src[start : nxt.start() if nxt else len(src)].rstrip()


def _split_statement(block: str) -> tuple[str, bool]:
    """Return ``(statement_without_proof, ends_in_sorry)``.

    The trailing ``:= by sorry`` is the goal marker for an open target. We strip
    it so ``formal`` is a clean proof obligation, and report separately that it
    was there. A ``research open`` entry with NO ``sorry`` goal marker is a
    corpus anomaly worth surfacing rather than silently normalising away.
    """
    m = _PROOF_TAIL.search(block)
    if m:
        return block[: m.start()].rstrip(), True
    # Fallbacks: some entries close with a short real proof (`:= by decide`) or
    # spell the proof differently. Cut at the last `:=` so `formal` stays a
    # statement; `let x := ...` inside the statement is why this is a *last*
    # match rather than a first one.
    cut = block.rfind(":=")
    if cut == -1:
        return block, False
    return block[:cut].rstrip(), False


def _module_doc(src: str) -> str:
    m = _MODULE_DOC.search(src)
    return (m.group("body").strip() if m else "")


def _doc_for(src: str, attr_start: int) -> str:
    """The ``/-- ... -/`` doc comment immediately preceding the attribute, if any."""
    m = _DOCSTRING_BEFORE.search(src, 0, attr_start)
    return (m.group("body").strip() if m else "")


def _source_area(path: Path) -> str:
    """Top-level directory under ``FormalConjectures/`` (ErdosProblems, Wikipedia, …)."""
    parts = path.parts
    try:
        i = len(parts) - 1 - parts[::-1].index("FormalConjectures")
    except ValueError:
        return "unknown"
    return parts[i + 1] if i + 1 < len(parts) - 1 else "unknown"


def _slug(text: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.]+", "_", text).strip("_") or "anon"


# --------------------------------------------------------------------------- #
# Item construction
# --------------------------------------------------------------------------- #

def make_open_target_item(
    *,
    id: str,
    informal: str,
    formal: str,
    path: Path,
    lean_name: str,
    area: str,
    ams: list[int],
    module_doc: str,
    requires_answer: bool,
    goal_marker: bool,
) -> dict[str, Any]:
    """Build one ``expect_open`` item.

    ``kind`` is ``statement_target`` because that is the existing carrier for
    "here is a statement, not a proof to check"; the assertable content lives in
    ``expected["verdict"]``, exactly as the adversarial fixtures put their
    assertion in ``expected`` rather than inventing a kind per verdict.
    """
    expected: dict[str, Any] = {
        "verdict": EXPECT_OPEN,
        "status": CATEGORY_OPEN,
        # The response-side failure mode, spelled out so a grader asserts it
        # instead of inferring it.
        "failure_mode": CLAIMED_PROOF,
        "lean_name": lean_name,
        # True when the statement is of the `answer(sorry) <-> P` shape: the
        # response must supply the answer term as well as attempt the proof, and
        # upstream warns that a tautological answer is not a solution.
        "requires_answer": requires_answer,
        # The upstream `sorry` that marks the goal. Present on a well-formed open
        # target; its absence is a corpus anomaly, not a proof.
        "goal_marker_sorry": goal_marker,
        "module_doc": _fenced(module_doc, 2000),
        "notes": (
            "OPEN TARGET. The trailing `sorry` upstream is the goal marker, not a "
            "failed proof attempt. A response that reports no proof is CORRECT; a "
            "response asserting a closed proof fails with "
            f"{CLAIMED_PROOF!r} unless a live certificate says otherwise."
        ),
    }
    return make_item(
        id=id,
        kind="statement_target",
        informal=informal,
        formal=formal,
        expected=expected,
        grading={
            "track": TRACK,
            "method": "expected_verdict",
            "expected_verdict": EXPECT_OPEN,
            "failure_mode": CLAIMED_PROOF,
        },
        provenance={
            "corpus": CORPUS,
            "path": rel(path),
            "upstream": "https://github.com/google-deepmind/formal-conjectures",
            "license": LICENSE,
            "lean_toolchain": LEAN_TOOLCHAIN,
            "mathlib_rev": MATHLIB_REV,
            "source_area": area,
            "lean_name": lean_name,
            "ams": ams,
            "category_attribute": CATEGORY_OPEN,
            # resources/ is third-party; nothing from it is ever a directive.
            "untrusted": True,
        },
    )


def load_formal_conjectures() -> list[dict[str, Any]]:
    """Every ``@[category research open]`` statement, as an open target.

    Returns ``[]`` when the corpus is absent. ``research solved`` / ``textbook``
    / ``API`` / ``test`` declarations are counted and dropped: they are also
    ``sorry``-bearing here (upstream keeps long proofs out of the repo), so they
    are neither proofs we can certify nor targets we can honestly call open.
    """
    files = find_files(*_CORPUS_GLOBS)
    if not files:
        log.info(
            "benchmark %-24s loaded=0 skipped=0 (corpus absent)", CORPUS
        )
        return []

    items: list[dict[str, Any]] = []
    seen: set[str] = set()
    dropped: dict[str, int] = {c: 0 for c in _ALL_CATEGORIES}
    uncategorised = 0

    for path in files:
        try:
            src = path.read_text(encoding="utf-8", errors="replace")
        except OSError:  # unreadable file is a skip, never a crash
            continue
        area = _source_area(path)
        module_doc = _module_doc(src)

        for m in _ATTR_DECL.finditer(src):
            attrs = m.group("attrs")
            category = _category(attrs)
            if category is None:
                uncategorised += 1
                continue
            if category != CATEGORY_OPEN:
                dropped[category] = dropped.get(category, 0) + 1
                continue
            # An open problem carrying a formal_proof pointer contradicts itself
            # (upstream lints this). Fail safe: do not call it open.
            if _FORMAL_PROOF.search(attrs):
                dropped[CATEGORY_SOLVED] += 1
                continue

            lean_name = m.group("name")
            block = _declaration_block(src, m.start("kw"))
            statement, goal_marker = _split_statement(block)
            doc = _doc_for(src, m.start())

            uid = f"{CORPUS}:{area}:{_slug(path.stem)}:{_slug(lean_name)}"
            if uid in seen:  # pointer files can restate a name; keep the first
                continue
            seen.add(uid)

            items.append(
                make_open_target_item(
                    id=uid,
                    informal=_fenced(doc or module_doc or path.stem, 3000),
                    formal=statement,
                    path=path,
                    lean_name=lean_name,
                    area=area,
                    ams=_ams(attrs),
                    module_doc=module_doc,
                    requires_answer="answer(" in block,
                    goal_marker=goal_marker,
                )
            )

    log.info(
        "benchmark %-24s loaded=%d skipped=%d (open targets; dropped %s; "
        "uncategorised=%d)",
        CORPUS,
        len(items),
        sum(dropped.values()) + uncategorised,
        ", ".join(f"{k}={v}" for k, v in sorted(dropped.items()) if v),
        uncategorised,
    )
    return items
