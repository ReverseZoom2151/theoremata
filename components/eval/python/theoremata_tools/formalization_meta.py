"""``formalization.yaml`` v0.3 — typed model + (de)serializer for the community
formalization-metadata schema (Tier 6, items 49-51).

WHY A COMMUNITY SCHEMA AND NOT OURS
-----------------------------------
Theoremata could trivially invent a bespoke "here is what our formalization run
produced" record. It should not. A formalization is only useful to a third party
if that party can read its provenance without reading our code, and the field is
converging on one nascent standard for exactly this: ``formalization.yaml``,
specified at ``github.com/mathlib-initiative/formalization.yaml``. We implement
**v0.3 of that external schema** rather than a private dialect, so our outputs
drop into other people's tooling and other people's outputs drop into ours. Where
the upstream spec is silent we stay permissive (unknown keys survive a
round-trip, see :attr:`FormalizationMeta.extra`) instead of inventing structure.

THE THREE LOAD-BEARING DESIGN POINTS
------------------------------------
1. ``status.sorry_in_definitions`` is a **separate field from**
   ``status.sorry_count``, not a subset roll-up of it. A ``sorry`` in a *proof*
   means "this theorem is not yet proved" — the statement still says what it
   says, and the gap is visible and local. A ``sorry`` in a *definition* is
   categorically worse: every downstream theorem that mentions that definition is
   proved *about a hole*, so those theorems can be vacuously true, mutually
   inconsistent, or simply about nothing, while the file still reports them as
   proved. One is missing work; the other silently invalidates work that looks
   finished. A single blended "sorry count" hides that distinction, so we refuse
   to blend them and :func:`validate` never derives one from the other.

2. ``status.main_results[]`` is a **per-declaration ledger**
   (``{declaration, file, sorry_count, axioms}``), not just a repo-level
   roll-up. A repo-level "0 sorries, axioms = [propext, Classical.choice,
   Quot.sound]" summary is compatible with the headline theorem being the one
   thing that is axiom-dirty. Per-declaration rows make the claim auditable
   declaration by declaration; the repo-level ``status`` fields remain as a
   summary, and :func:`validate` warns when the summary contradicts the ledger.

3. ``fidelity.divergences[]`` is a structured, **adversarial self-report** of
   every place the formal statement departs from the informal source. This is
   the genuinely novel practice we are adopting from the schema: rather than
   claiming "we formalized problem N", the author is required to enumerate how
   the formal statement is *not* problem N. A real corpus published under this
   schema used the mechanism to disclose that several of its competition
   statements **presupposed their own answers** — the numeric answer was baked
   into the statement, so the "theorem" verified an answer it had been handed
   rather than deriving it (:data:`DivergenceKind.ANSWER_BAKED_INTO_STATEMENT`).
   That class of defect is invisible to the kernel: the proof really does
   compile. Only a disclosed divergence catches it, which is why the kinds are a
   closed enum with a free-text ``detail``, not prose alone.

SERIALIZATION
-------------
``yaml`` is **not** a dependency of this package (see ``pyproject.toml``:
``sympy``, ``litellm``, and soft-gated extras only), and this module refuses to
add one for a data-shaping concern. The canonical in-memory exchange format is
therefore a plain ``dict`` (JSON-compatible: only str/int/bool/list/dict leaves),
via :meth:`FormalizationMeta.to_dict` / :meth:`from_dict`. :func:`to_yaml` and
:func:`from_yaml` are provided as the documented seam: they import ``PyYAML``
lazily and raise a clear :class:`RuntimeError` when it is absent, so installing
``pyyaml`` is the only step needed to turn on YAML I/O — no code changes here.

Serialization is **deterministic**: field order is the declaration order of the
dataclasses (never ``sorted()``, never dict-hash order), so two equal documents
always produce byte-identical output and diffs stay reviewable.

Offline / deterministic / pure stdlib: no network, no model, no I/O beyond the
file handles the caller passes in.
"""
from __future__ import annotations

import json
from dataclasses import dataclass, field
from typing import Any, Optional

SCHEMA_NAME = "formalization.yaml"
SCHEMA_VERSION = "0.3"
SCHEMA_URL = "https://github.com/mathlib-initiative/formalization.yaml"

# --------------------------------------------------------------------------- #
# Enums (plain string constants — matches sibling style, keeps JSON leaves str)
# --------------------------------------------------------------------------- #


class DivergenceKind:
    """Closed enum of ways a formal statement can depart from its informal source.

    Deliberately small and adversarial: each member names a *specific* failure a
    kernel cannot see. ``OTHER`` exists so an author is never forced to
    mis-classify, but a validator downstream may reasonably treat a corpus that
    is mostly ``OTHER`` as under-disclosed.
    """

    #: The answer is written into the statement, so the theorem verifies an
    #: answer it was handed instead of deriving it. The defect that a published
    #: corpus used this schema to disclose across several competition problems.
    ANSWER_BAKED_INTO_STATEMENT = "answer_baked_into_statement"
    #: A word-problem-to-mathematics modeling step was asserted, not proved.
    MODELING_STEP_UNPROVEN = "modeling_step_unproven"
    #: A known background fact was taken as a hypothesis/axiom rather than proved.
    BACKGROUND_FACT_ASSUMED = "background_fact_assumed"
    #: A hypothesis absent from the informal source was added.
    HYPOTHESIS_ADDED = "hypothesis_added"
    #: The formal statement proves strictly less than the informal source.
    STATEMENT_WEAKENED = "statement_weakened"
    #: The formal statement proves strictly more than the informal source.
    STATEMENT_STRENGTHENED = "statement_strengthened"
    #: Anything else; ``detail`` carries the explanation.
    OTHER = "other"


#: Every divergence kind, in canonical (declaration) order.
DIVERGENCE_KINDS: tuple[str, ...] = (
    DivergenceKind.ANSWER_BAKED_INTO_STATEMENT,
    DivergenceKind.MODELING_STEP_UNPROVEN,
    DivergenceKind.BACKGROUND_FACT_ASSUMED,
    DivergenceKind.HYPOTHESIS_ADDED,
    DivergenceKind.STATEMENT_WEAKENED,
    DivergenceKind.STATEMENT_STRENGTHENED,
    DivergenceKind.OTHER,
)


class ProblemStatus:
    """Lightweight per-problem tracking status, separate from ``status`` (which
    describes the *repository*). A problem is one of proved / disproved /
    unsolved; "unsolved" is the honest default and is never implied by a
    ``sorry_count`` of zero on some other declaration."""

    PROVED = "proved"
    DISPROVED = "disproved"
    UNSOLVED = "unsolved"


#: Every problem status, in canonical order.
PROBLEM_STATUSES: tuple[str, ...] = (
    ProblemStatus.PROVED,
    ProblemStatus.DISPROVED,
    ProblemStatus.UNSOLVED,
)


# --------------------------------------------------------------------------- #
# Helpers
# --------------------------------------------------------------------------- #

def _str_list(value: Any) -> list[str]:
    """Coerce a scalar-or-list field to ``list[str]``, preserving order.

    ``None`` and ``""`` become ``[]``; a bare string becomes a one-element list
    (upstream documents write single authors unwrapped). Order is never sorted —
    author order and axiom order are meaningful to a reader.
    """
    if value is None or value == "":
        return []
    if isinstance(value, str):
        return [value]
    if isinstance(value, (list, tuple)):
        return [str(v) for v in value]
    return [str(value)]


def _opt_str(value: Any) -> Optional[str]:
    """``None`` stays ``None``; everything else is stringified."""
    return None if value is None else str(value)


def _int(value: Any, default: int = 0) -> int:
    """Best-effort int coercion; unparseable values fall back to ``default``.

    Never raises: a malformed count is surfaced by :func:`validate` as a
    structured error rather than by blowing up the parse.
    """
    try:
        return int(value)
    except (TypeError, ValueError):
        return default


def _drop_empty(data: dict[str, Any]) -> dict[str, Any]:
    """Remove ``None`` values so serialized documents stay terse.

    Empty *lists* and zero counts are kept: "we checked and there are none" is a
    different claim from "we did not say", and for ``sorry_count`` /
    ``sorry_in_definitions`` that difference is the whole point.
    """
    return {k: v for k, v in data.items() if v is not None}


def _error(path: str, code: str, message: str) -> dict[str, str]:
    """One structured validation error."""
    return {"path": path, "code": code, "message": message}


# --------------------------------------------------------------------------- #
# Leaf records
# --------------------------------------------------------------------------- #

@dataclass
class Project:
    """``project``: who made this and under what terms."""

    name: Optional[str] = None
    authors: list[str] = field(default_factory=list)
    license: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "Project":
        data = data or {}
        return cls(
            name=_opt_str(data.get("name")),
            authors=_str_list(data.get("authors")),
            license=_opt_str(data.get("license")),
        )

    def to_dict(self) -> dict[str, Any]:
        return _drop_empty(
            {"name": self.name, "authors": list(self.authors), "license": self.license}
        )


@dataclass
class Source:
    """One entry of ``sources[]``: the informal artifact being formalized.

    ``type`` is free text upstream (e.g. ``competition``, ``paper``,
    ``conjecture``, ``textbook``); we do not close that enum because the schema
    does not.
    """

    title: Optional[str] = None
    authors: list[str] = field(default_factory=list)
    id: Optional[str] = None
    type: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "Source":
        data = data or {}
        return cls(
            title=_opt_str(data.get("title")),
            authors=_str_list(data.get("authors")),
            id=_opt_str(data.get("id")),
            type=_opt_str(data.get("type")),
        )

    def to_dict(self) -> dict[str, Any]:
        return _drop_empty(
            {
                "title": self.title,
                "authors": list(self.authors),
                "id": self.id,
                "type": self.type,
            }
        )


@dataclass
class MainResult:
    """One row of the **per-declaration ledger** ``status.main_results[]``.

    ``axioms`` is that declaration's own verified axiom set (what
    ``#print axioms`` reports), not the repository's union. A repo-level axiom
    list cannot tell a reader *which* theorem depends on ``Classical.choice``;
    this row can.
    """

    declaration: Optional[str] = None
    file: Optional[str] = None
    sorry_count: int = 0
    axioms: list[str] = field(default_factory=list)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "MainResult":
        data = data or {}
        return cls(
            declaration=_opt_str(data.get("declaration")),
            file=_opt_str(data.get("file")),
            sorry_count=_int(data.get("sorry_count", 0)),
            axioms=_str_list(data.get("axioms")),
        )

    def to_dict(self) -> dict[str, Any]:
        return _drop_empty(
            {
                "declaration": self.declaration,
                "file": self.file,
                "sorry_count": self.sorry_count,
                "axioms": list(self.axioms),
            }
        )


@dataclass
class Status:
    """``status``: the repository-level completeness claim.

    ``sorry_count`` counts ``sorry`` in *proofs*; ``sorry_in_definitions`` counts
    ``sorry`` in *definitions* and is an **independent** field — see the module
    docstring. Neither is derived from the other, and a document may legitimately
    report ``sorry_count=0`` with ``sorry_in_definitions=1``: nothing is left to
    prove, yet what was proved is about a hole.
    """

    scope: Optional[str] = None
    sorry_count: int = 0
    sorry_in_definitions: int = 0
    axioms: list[str] = field(default_factory=list)
    main_results: list[MainResult] = field(default_factory=list)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "Status":
        data = data or {}
        return cls(
            scope=_opt_str(data.get("scope")),
            sorry_count=_int(data.get("sorry_count", 0)),
            sorry_in_definitions=_int(data.get("sorry_in_definitions", 0)),
            axioms=_str_list(data.get("axioms")),
            main_results=[
                MainResult.from_dict(r) for r in (data.get("main_results") or [])
            ],
        )

    def to_dict(self) -> dict[str, Any]:
        return _drop_empty(
            {
                "scope": self.scope,
                "sorry_count": self.sorry_count,
                "sorry_in_definitions": self.sorry_in_definitions,
                "axioms": list(self.axioms),
                "main_results": [r.to_dict() for r in self.main_results],
            }
        )


@dataclass
class AutomationMethod:
    """One row of ``automation.methods[]``: provenance of *who or what* produced
    the proof (e.g. ``method="autonomous"``/``"human"``, ``framework="Theoremata"``).

    This is the field that keeps an agent-produced corpus honest about being
    agent-produced.
    """

    method: Optional[str] = None
    framework: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "AutomationMethod":
        data = data or {}
        return cls(
            method=_opt_str(data.get("method")),
            framework=_opt_str(data.get("framework")),
        )

    def to_dict(self) -> dict[str, Any]:
        return _drop_empty({"method": self.method, "framework": self.framework})


@dataclass
class Automation:
    """``automation``: the list of production methods behind this formalization."""

    methods: list[AutomationMethod] = field(default_factory=list)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "Automation":
        data = data or {}
        return cls(
            methods=[
                AutomationMethod.from_dict(m) for m in (data.get("methods") or [])
            ]
        )

    def to_dict(self) -> dict[str, Any]:
        return _drop_empty({"methods": [m.to_dict() for m in self.methods]})


@dataclass
class Review:
    """``review``: who checked this, and on what basis.

    ``status`` is free text precisely so it can record a **mixed** basis such as
    ``"human + agent"`` — collapsing that to a boolean "reviewed" would erase the
    distinction between a human reading every statement and a model asserting it
    did.
    """

    status: Optional[str] = None
    reviewers: list[str] = field(default_factory=list)
    notes: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "Review":
        data = data or {}
        return cls(
            status=_opt_str(data.get("status")),
            reviewers=_str_list(data.get("reviewers")),
            notes=_opt_str(data.get("notes")),
        )

    def to_dict(self) -> dict[str, Any]:
        return _drop_empty(
            {
                "status": self.status,
                "reviewers": list(self.reviewers),
                "notes": self.notes,
            }
        )


@dataclass
class Divergence:
    """One row of ``fidelity.divergences[]``: a disclosed departure of the formal
    statement from the informal source.

    ``kind`` should be one of :data:`DIVERGENCE_KINDS`; unknown kinds parse (so a
    future schema revision round-trips) but are flagged by :func:`validate`.
    ``detail`` is the free-text explanation and ``statement`` optionally points at
    the specific declaration affected.
    """

    kind: str = DivergenceKind.OTHER
    detail: Optional[str] = None
    statement: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "Divergence":
        data = data or {}
        return cls(
            kind=str(data.get("kind", DivergenceKind.OTHER)),
            detail=_opt_str(data.get("detail")),
            statement=_opt_str(data.get("statement")),
        )

    def to_dict(self) -> dict[str, Any]:
        return _drop_empty(
            {"kind": self.kind, "detail": self.detail, "statement": self.statement}
        )


@dataclass
class Fidelity:
    """``fidelity``: the adversarial self-report block.

    An empty ``divergences`` list is a positive claim ("we looked and found
    none"), which is why :meth:`to_dict` keeps it rather than dropping it.
    """

    divergences: list[Divergence] = field(default_factory=list)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "Fidelity":
        data = data or {}
        return cls(
            divergences=[
                Divergence.from_dict(d) for d in (data.get("divergences") or [])
            ]
        )

    def to_dict(self) -> dict[str, Any]:
        return _drop_empty({"divergences": [d.to_dict() for d in self.divergences]})


@dataclass
class AlignmentStatement:
    """One row of ``alignment.statements[]``: the informal-to-formal mapping.

    ``source`` names the informal statement, ``lean`` the formal declaration,
    ``module`` the file it lives in, ``status`` one of :data:`PROBLEM_STATUSES`,
    and ``note`` any caveat. One row per source statement, so a reader can check
    coverage without reading Lean.
    """

    source: Optional[str] = None
    lean: Optional[str] = None
    module: Optional[str] = None
    status: Optional[str] = None
    note: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "AlignmentStatement":
        data = data or {}
        return cls(
            source=_opt_str(data.get("source")),
            lean=_opt_str(data.get("lean")),
            module=_opt_str(data.get("module")),
            status=_opt_str(data.get("status")),
            note=_opt_str(data.get("note")),
        )

    def to_dict(self) -> dict[str, Any]:
        return _drop_empty(
            {
                "source": self.source,
                "lean": self.lean,
                "module": self.module,
                "status": self.status,
                "note": self.note,
            }
        )


@dataclass
class Alignment:
    """``alignment``: the per-statement informal-to-formal mapping table."""

    statements: list[AlignmentStatement] = field(default_factory=list)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "Alignment":
        data = data or {}
        return cls(
            statements=[
                AlignmentStatement.from_dict(s) for s in (data.get("statements") or [])
            ]
        )

    def to_dict(self) -> dict[str, Any]:
        return _drop_empty({"statements": [s.to_dict() for s in self.statements]})


# --------------------------------------------------------------------------- #
# Document root
# --------------------------------------------------------------------------- #

#: Top-level keys this module models, in canonical serialization order.
_KNOWN_TOP_LEVEL: tuple[str, ...] = (
    "version",
    "project",
    "sources",
    "status",
    "automation",
    "review",
    "fidelity",
    "alignment",
)


@dataclass
class FormalizationMeta:
    """A whole ``formalization.yaml`` v0.3 document.

    Unrecognized top-level keys are preserved verbatim in :attr:`extra` and
    re-emitted after the known keys, so a document written against a newer
    revision of the schema survives a Theoremata round-trip without data loss —
    dropping a key we do not understand would silently discard someone else's
    disclosure.
    """

    version: str = SCHEMA_VERSION
    project: Project = field(default_factory=Project)
    sources: list[Source] = field(default_factory=list)
    status: Status = field(default_factory=Status)
    automation: Automation = field(default_factory=Automation)
    review: Review = field(default_factory=Review)
    fidelity: Fidelity = field(default_factory=Fidelity)
    alignment: Alignment = field(default_factory=Alignment)
    extra: dict[str, Any] = field(default_factory=dict)

    # -- deserialization ---------------------------------------------------- #

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "FormalizationMeta":
        """Parse a plain dict into the typed model.

        Permissive by design: missing blocks become empty defaults and malformed
        scalars are coerced, so a partial document parses and its problems are
        reported by :meth:`validate` as structured errors rather than as an
        exception from the middle of a parse. Raises :class:`TypeError` only when
        the document itself is not a mapping.
        """
        if not isinstance(data, dict):
            raise TypeError(f"formalization document must be a mapping, got {type(data).__name__}")
        extra = {k: v for k, v in data.items() if k not in _KNOWN_TOP_LEVEL}
        return cls(
            version=str(data.get("version", SCHEMA_VERSION)),
            project=Project.from_dict(data.get("project") or {}),
            sources=[Source.from_dict(s) for s in (data.get("sources") or [])],
            status=Status.from_dict(data.get("status") or {}),
            automation=Automation.from_dict(data.get("automation") or {}),
            review=Review.from_dict(data.get("review") or {}),
            fidelity=Fidelity.from_dict(data.get("fidelity") or {}),
            alignment=Alignment.from_dict(data.get("alignment") or {}),
            extra=extra,
        )

    @classmethod
    def from_json(cls, text: str) -> "FormalizationMeta":
        """Parse a JSON document (the dependency-free interchange format)."""
        return cls.from_dict(json.loads(text))

    @classmethod
    def from_yaml(cls, text: str) -> "FormalizationMeta":
        """Parse a YAML document.

        ``PyYAML`` is **not** a declared dependency of ``theoremata-tools``; this
        is the documented seam. Raises :class:`RuntimeError` with an actionable
        message when it is unavailable — install ``pyyaml`` and this works with
        no change here. Uses ``yaml.safe_load`` only: the document is untrusted
        data and must never be able to construct Python objects.
        """
        yaml = _require_yaml()
        return cls.from_dict(yaml.safe_load(text) or {})

    # -- serialization ------------------------------------------------------ #

    def to_dict(self) -> dict[str, Any]:
        """Serialize to a JSON-compatible dict with **deterministic** key order.

        Order is the declaration order of :data:`_KNOWN_TOP_LEVEL` and of each
        dataclass's fields — never ``sorted()`` and never dict-hash order — so
        equal documents produce byte-identical output and diffs stay reviewable.
        Preserved unknown keys from :attr:`extra` follow, in insertion order.
        """
        out: dict[str, Any] = {
            "version": self.version,
            "project": self.project.to_dict(),
            "sources": [s.to_dict() for s in self.sources],
            "status": self.status.to_dict(),
            "automation": self.automation.to_dict(),
            "review": self.review.to_dict(),
            "fidelity": self.fidelity.to_dict(),
            "alignment": self.alignment.to_dict(),
        }
        for key, value in self.extra.items():
            out[key] = value
        return out

    def to_json(self, indent: int = 2) -> str:
        """Serialize to JSON, preserving insertion order (``sort_keys=False``)."""
        return json.dumps(self.to_dict(), indent=indent, sort_keys=False, default=str)

    def to_yaml(self) -> str:
        """Serialize to YAML.

        Same seam as :meth:`from_yaml`: raises :class:`RuntimeError` when
        ``PyYAML`` is absent. ``sort_keys=False`` keeps the deterministic
        declaration order established by :meth:`to_dict` instead of
        alphabetizing it.
        """
        yaml = _require_yaml()
        return yaml.safe_dump(self.to_dict(), sort_keys=False, allow_unicode=True)

    # -- validation --------------------------------------------------------- #

    def validate(self) -> list[dict[str, str]]:
        """Return structured errors ``{path, code, message}``; empty means valid.

        Returns a list rather than raising so a caller can report *every* problem
        in a document at once. Checks, in order:

        * required fields present (``project.name``, ``project.authors``,
          ``status.scope``, at least one ``sources`` entry);
        * counts non-negative (``sorry_count``, ``sorry_in_definitions``, and the
          same on each ledger row);
        * every ``fidelity.divergences[].kind`` is a known kind and carries a
          ``detail`` (an undocumented divergence discloses nothing);
        * every ``alignment.statements[].status`` is a known problem status;
        * per-declaration ledger rows name a declaration, and their axioms are a
          subset of the repo-level ``status.axioms`` when that summary is given
          (a ledger axiom missing from the summary means the summary understates
          the trust base).

        Note what is deliberately **not** checked: ``sorry_in_definitions`` is
        never required to relate to ``sorry_count`` in any way. They measure
        different things, and forcing a relationship would recreate exactly the
        conflation the schema exists to prevent.
        """
        errors: list[dict[str, str]] = []

        if not self.project.name:
            errors.append(
                _error("project.name", "missing_required", "project.name is required")
            )
        if not self.project.authors:
            errors.append(
                _error(
                    "project.authors",
                    "missing_required",
                    "project.authors must list at least one author",
                )
            )
        if not self.sources:
            errors.append(
                _error(
                    "sources",
                    "missing_required",
                    "at least one source must be declared",
                )
            )
        if not self.status.scope:
            errors.append(
                _error("status.scope", "missing_required", "status.scope is required")
            )

        for idx, source in enumerate(self.sources):
            if not source.title:
                errors.append(
                    _error(
                        f"sources[{idx}].title",
                        "missing_required",
                        "each source needs a title",
                    )
                )

        if self.status.sorry_count < 0:
            errors.append(
                _error(
                    "status.sorry_count",
                    "invalid_value",
                    "sorry_count must be non-negative",
                )
            )
        if self.status.sorry_in_definitions < 0:
            errors.append(
                _error(
                    "status.sorry_in_definitions",
                    "invalid_value",
                    "sorry_in_definitions must be non-negative",
                )
            )

        repo_axioms = set(self.status.axioms)
        for idx, result in enumerate(self.status.main_results):
            path = f"status.main_results[{idx}]"
            if not result.declaration:
                errors.append(
                    _error(
                        f"{path}.declaration",
                        "missing_required",
                        "each main result must name a declaration",
                    )
                )
            if result.sorry_count < 0:
                errors.append(
                    _error(
                        f"{path}.sorry_count",
                        "invalid_value",
                        "sorry_count must be non-negative",
                    )
                )
            if repo_axioms:
                for axiom in result.axioms:
                    if axiom not in repo_axioms:
                        errors.append(
                            _error(
                                f"{path}.axioms",
                                "axiom_not_in_repo_set",
                                f"declaration axiom {axiom!r} is absent from "
                                "status.axioms; the repo-level axiom summary "
                                "understates the trust base",
                            )
                        )

        for idx, divergence in enumerate(self.fidelity.divergences):
            path = f"fidelity.divergences[{idx}]"
            if divergence.kind not in DIVERGENCE_KINDS:
                errors.append(
                    _error(
                        f"{path}.kind",
                        "unknown_divergence_kind",
                        f"{divergence.kind!r} is not one of {list(DIVERGENCE_KINDS)}",
                    )
                )
            if not divergence.detail:
                errors.append(
                    _error(
                        f"{path}.detail",
                        "missing_required",
                        "a divergence without detail discloses nothing",
                    )
                )

        for idx, statement in enumerate(self.alignment.statements):
            path = f"alignment.statements[{idx}]"
            if statement.status is not None and statement.status not in PROBLEM_STATUSES:
                errors.append(
                    _error(
                        f"{path}.status",
                        "unknown_problem_status",
                        f"{statement.status!r} is not one of {list(PROBLEM_STATUSES)}",
                    )
                )

        return errors


# --------------------------------------------------------------------------- #
# YAML seam
# --------------------------------------------------------------------------- #

def yaml_available() -> bool:
    """Whether ``PyYAML`` can be imported (it is not a declared dependency)."""
    try:  # pragma: no cover - trivial import probe
        import yaml  # noqa: F401
    except Exception:  # pragma: no cover
        return False
    return True


def _require_yaml():
    """Import ``PyYAML`` or raise an actionable :class:`RuntimeError`."""
    try:
        import yaml
    except Exception as exc:  # pragma: no cover - depends on the environment
        raise RuntimeError(
            "YAML support requires PyYAML, which is not a dependency of "
            "theoremata-tools. Install it (`pip install pyyaml`) or use the "
            "dependency-free dict/JSON round-trip "
            "(FormalizationMeta.to_dict / from_dict / to_json / from_json)."
        ) from exc
    return yaml


# --------------------------------------------------------------------------- #
# Module-level convenience wrappers
# --------------------------------------------------------------------------- #

def from_dict(data: dict[str, Any]) -> FormalizationMeta:
    """Parse a plain dict into a :class:`FormalizationMeta`."""
    return FormalizationMeta.from_dict(data)


def to_dict(meta: FormalizationMeta) -> dict[str, Any]:
    """Serialize a :class:`FormalizationMeta` to a deterministic plain dict."""
    return meta.to_dict()


def from_yaml(text: str) -> FormalizationMeta:
    """Parse a YAML document (requires ``PyYAML``; see :meth:`FormalizationMeta.from_yaml`)."""
    return FormalizationMeta.from_yaml(text)


def to_yaml(meta: FormalizationMeta) -> str:
    """Serialize to YAML (requires ``PyYAML``; see :meth:`FormalizationMeta.to_yaml`)."""
    return meta.to_yaml()


def validate(data: Any) -> list[dict[str, str]]:
    """Validate a dict or :class:`FormalizationMeta`, returning structured errors."""
    meta = data if isinstance(data, FormalizationMeta) else FormalizationMeta.from_dict(data)
    return meta.validate()


# --------------------------------------------------------------------------- #
# JSON dispatch (worker.py hook) + CLI
# --------------------------------------------------------------------------- #

def run(request: dict[str, Any]) -> dict[str, Any]:
    """JSON dispatch for the ``formalization_meta`` worker op.

    Sub-ops (``op`` field, default ``validate``):

    * ``validate`` -> ``{"valid": bool, "errors": [...], "n_errors": int}``
    * ``normalize`` -> ``{"document": <deterministic dict>}`` (round-trip a
      document through the typed model to canonicalize key order)
    * ``schema`` -> the schema identity plus the divergence/status enums
    * ``to_yaml`` / ``from_yaml`` -> YAML I/O, gated on ``PyYAML``
    """
    op = request.get("op", "validate")
    if op == "schema":
        return {
            "op": op,
            "schema": SCHEMA_NAME,
            "version": SCHEMA_VERSION,
            "url": SCHEMA_URL,
            "divergence_kinds": list(DIVERGENCE_KINDS),
            "problem_statuses": list(PROBLEM_STATUSES),
            "yaml_available": yaml_available(),
        }
    if op == "validate":
        errors = validate(request.get("document") or {})
        return {"op": op, "valid": not errors, "errors": errors, "n_errors": len(errors)}
    if op == "normalize":
        meta = FormalizationMeta.from_dict(request.get("document") or {})
        return {"op": op, "document": meta.to_dict()}
    if op == "to_yaml":
        meta = FormalizationMeta.from_dict(request.get("document") or {})
        return {"op": op, "yaml": meta.to_yaml()}
    if op == "from_yaml":
        meta = FormalizationMeta.from_yaml(request.get("yaml", ""))
        return {"op": op, "document": meta.to_dict()}
    raise ValueError(f"unknown op: {op}")


def main() -> None:  # pragma: no cover - thin CLI shim
    import sys

    if len(sys.argv) >= 2:
        with open(sys.argv[1], encoding="utf-8") as fh:
            request = json.load(fh)
    else:
        request = json.load(sys.stdin)
    print(json.dumps(run(request), indent=2, default=str))
    raise SystemExit(0)


if __name__ == "__main__":
    main()
