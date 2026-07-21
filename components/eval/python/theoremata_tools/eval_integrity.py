"""Anti-self-deception plumbing for the eval harness (Tier 4, items 25-30).

The outcome harness (:mod:`theoremata_tools.eval_harness`) and the path harness
(:mod:`theoremata_tools.trajectory_eval`) both answer "how well did we do?".
Neither answers "is that number allowed to mean what we want it to mean?". This
module is that second question, and every primitive here exists because a
specific way of fooling ourselves is otherwise available:

* **Provenance firewall** — every record carries an ``origin`` tag
  (:data:`ORIGINS`). Records tagged ``dev_attested`` (a human said it was fine)
  or ``synthetic_mutation`` (we generated the "ground truth" ourselves) are
  *structurally* barred from preference/DPO export: :class:`EvalRecord`
  recomputes ``training_eligible`` in ``__post_init__`` and ignores any value
  the caller passes, and :func:`export_preference_corpus` filters on that
  derived flag rather than on caller discipline. It returns the dropped count,
  so a corpus can never shrink silently.
* **Dual rates** — :func:`outcome_rates` reports ``solved_rate`` (solved at
  all, any attempt) and ``pass_at_1_rate`` (solved on a genuine first attempt)
  as two distinct fields. Collapsing them is how a k=32 budget gets reported as
  single-shot capability.
* **Run envelopes** — :class:`RunEnvelope` refuses a ``dev_attested`` record in
  a ``benchmark`` / ``evaluation`` / ``public_report`` envelope with no
  override available at all; only ``private_audit`` may admit one, and only
  with an explicit ``allow_dev_attested=True``.
* **Stratified metrics** — :func:`stratified_metrics` reports per-bucket rates
  beside the aggregate, because an aggregate hides regime differences (the
  reported case: 16.55% overall while one bucket sat at 100%).
* **Score/verdict telemetry** — :class:`ScoreVerdict` pairs the model's own
  score with the verifier's verdict, and :func:`ranking_quality` measures how
  often a higher-scored candidate was the verified one. This is the only signal
  that says whether the best-first priority function ranks truth well.
* **Deterministic audit sampling**: :func:`select_audit_sample` picks a fixed
  fraction of the *confident* judge labels for expensive review, keyed on
  ``sha256(salt, item_id)``. Auditing only the hard or disagreeing cases measures
  agreement on a biased slice; the resulting number is uninterpretable. Rates
  derived from a sample come back as a :class:`SampledRate`, which cannot be
  serialized without its fraction, salt and denominator.
* **Evidence pinning**: :func:`pin_evidence` hashes the artifact a judgement was
  made about, :func:`check_findings` proves every finding names something in the
  pinned set, and :func:`verify_pins` detects a later mutation. A content hash
  pins bytes only; it never makes a judgement correct.
* **Environment fingerprint** — :func:`environment_fingerprint` hashes
  (toolchain, corpus/mathlib pin, import manifest). A cached or reported pass
  is comparable only inside one fingerprint.

Pure stdlib, no wall clock, no RNG: the same inputs always produce the same
outputs, including the fingerprint digest. This module imports no other
Theoremata code, so it is safe to wire into the worker without touching the
shared eval files.
"""
from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass, field
from typing import Any, Iterable, Mapping, Sequence

# --------------------------------------------------------------------------- #
# Provenance
# --------------------------------------------------------------------------- #

ORIGIN_ORGANIC = "organic"
ORIGIN_SYNTHETIC_MUTATION = "synthetic_mutation"
ORIGIN_DEV_ATTESTED = "dev_attested"
ORIGIN_BENCHMARK_ALIGNED = "benchmark_aligned"

#: Every legal provenance tag. An unknown tag is an error, never a default.
ORIGINS: frozenset[str] = frozenset(
    {
        ORIGIN_ORGANIC,
        ORIGIN_SYNTHETIC_MUTATION,
        ORIGIN_DEV_ATTESTED,
        ORIGIN_BENCHMARK_ALIGNED,
    }
)

#: Origins that may never reach a preference/DPO export. ``dev_attested`` is a
#: human vouching for an outcome (training on it trains on our own opinion);
#: ``synthetic_mutation`` is a label we manufactured (training on it trains on
#: our own generator). Both are self-reinforcing, so both are barred.
TRAINING_INELIGIBLE_ORIGINS: frozenset[str] = frozenset(
    {ORIGIN_DEV_ATTESTED, ORIGIN_SYNTHETIC_MUTATION}
)

#: Envelope kinds. Only ``private_audit`` can ever see a ``dev_attested`` record.
ENVELOPE_BENCHMARK = "benchmark"
ENVELOPE_EVALUATION = "evaluation"
ENVELOPE_PUBLIC_REPORT = "public_report"
ENVELOPE_PRIVATE_AUDIT = "private_audit"

ENVELOPE_KINDS: frozenset[str] = frozenset(
    {
        ENVELOPE_BENCHMARK,
        ENVELOPE_EVALUATION,
        ENVELOPE_PUBLIC_REPORT,
        ENVELOPE_PRIVATE_AUDIT,
    }
)

#: Envelope kinds from which a ``dev_attested`` record is unconditionally
#: excluded — there is deliberately no flag that relaxes this set.
SEALED_ENVELOPE_KINDS: frozenset[str] = frozenset(
    {ENVELOPE_BENCHMARK, ENVELOPE_EVALUATION, ENVELOPE_PUBLIC_REPORT}
)


class ProvenanceError(ValueError):
    """An origin tag is missing, unknown, or used where it is not permitted."""


class EnvelopeViolation(ValueError):
    """A record was admitted to a run envelope that forbids its provenance."""


@dataclass(frozen=True)
class EvalRecord:
    """One eval outcome plus the provenance needed to decide what it may fund.

    Fields:

    * ``record_id`` — stable identifier (used for ordering / dedup reporting).
    * ``origin`` — one of :data:`ORIGINS`; anything else raises
      :class:`ProvenanceError`.
    * ``solved`` — the problem was solved on *some* attempt.
    * ``first_attempt_solved`` — solved on a genuine first attempt. Forced
      ``False`` when ``solved`` is ``False``, since a run that never solved
      cannot have solved first.
    * ``attempts`` — number of attempts made (``>= 1``).
    * ``strata`` — bucketing keys (goal shape, depth, domain, ...) for
      :func:`stratified_metrics`.
    * ``chosen`` / ``rejected`` — the preference pair carried for DPO export.
    * ``payload`` — anything else the caller wants to carry through.

    ``training_eligible`` is **derived, not accepted**: it is recomputed from
    ``origin`` in ``__post_init__``, so constructing a record cannot assert
    eligibility that the origin does not grant. This is the firewall; the export
    filter merely reads it.
    """

    record_id: str
    origin: str
    solved: bool = False
    first_attempt_solved: bool = False
    attempts: int = 1
    strata: dict[str, Any] = field(default_factory=dict)
    chosen: Any = None
    rejected: Any = None
    payload: dict[str, Any] = field(default_factory=dict)
    training_eligible: bool = field(init=False, default=False)

    def __post_init__(self) -> None:
        if self.origin not in ORIGINS:
            raise ProvenanceError(
                f"unknown origin {self.origin!r}; expected one of {sorted(ORIGINS)}"
            )
        if int(self.attempts) < 1:
            raise ValueError(f"attempts must be >= 1, got {self.attempts!r}")
        object.__setattr__(self, "solved", bool(self.solved))
        object.__setattr__(self, "attempts", int(self.attempts))
        # A run that never solved cannot have solved on its first attempt.
        object.__setattr__(
            self,
            "first_attempt_solved",
            bool(self.first_attempt_solved) and self.solved,
        )
        # Derived by construction — a caller-supplied value is never honored.
        object.__setattr__(
            self, "training_eligible", self.origin not in TRAINING_INELIGIBLE_ORIGINS
        )

    @classmethod
    def from_mapping(cls, data: Mapping[str, Any]) -> "EvalRecord":
        """Build a record from a JSON-ish mapping, ignoring unknown keys.

        Any ``training_eligible`` key in ``data`` is dropped rather than
        honored: eligibility is a function of ``origin`` alone.
        """
        if "origin" not in data:
            raise ProvenanceError("record is missing the required 'origin' tag")
        return cls(
            record_id=str(data.get("record_id", data.get("id", ""))),
            origin=str(data["origin"]),
            solved=bool(data.get("solved", False)),
            first_attempt_solved=bool(data.get("first_attempt_solved", False)),
            attempts=int(data.get("attempts", 1)),
            strata=dict(data.get("strata", {}) or {}),
            chosen=data.get("chosen"),
            rejected=data.get("rejected"),
            payload=dict(data.get("payload", {}) or {}),
        )

    def to_dict(self) -> dict[str, Any]:
        """Plain-dict view, including the derived ``training_eligible`` flag."""
        return {
            "record_id": self.record_id,
            "origin": self.origin,
            "solved": self.solved,
            "first_attempt_solved": self.first_attempt_solved,
            "attempts": self.attempts,
            "strata": dict(self.strata),
            "chosen": self.chosen,
            "rejected": self.rejected,
            "payload": dict(self.payload),
            "training_eligible": self.training_eligible,
        }


def coerce_records(records: Iterable[Any]) -> list[EvalRecord]:
    """Normalize an iterable of records/mappings into :class:`EvalRecord` list.

    Raises :class:`ProvenanceError` for an untagged mapping, so an untagged
    record can never slip through as an implicitly-organic one.
    """
    out: list[EvalRecord] = []
    for rec in records:
        if isinstance(rec, EvalRecord):
            out.append(rec)
        elif isinstance(rec, Mapping):
            out.append(EvalRecord.from_mapping(rec))
        else:
            raise TypeError(f"expected EvalRecord or mapping, got {type(rec).__name__}")
    return out


# --------------------------------------------------------------------------- #
# Preference / DPO export firewall
# --------------------------------------------------------------------------- #

def export_preference_corpus(records: Iterable[Any]) -> dict[str, Any]:
    """Export the preference pairs that are *allowed* to become training data.

    Filtering is applied by construction: only records whose derived
    ``training_eligible`` flag is true survive, so a ``dev_attested`` or
    ``synthetic_mutation`` record cannot enter the corpus regardless of how the
    caller assembled it. Records lacking both ``chosen`` and ``rejected`` are
    also dropped (there is no pair to learn from).

    Returns ``{"pairs", "kept", "dropped", "dropped_by_origin",
    "dropped_incomplete", "n_input"}``. The dropped count is returned rather
    than logged, so a truncated export is impossible to miss.
    """
    recs = coerce_records(records)
    pairs: list[dict[str, Any]] = []
    dropped_by_origin: dict[str, int] = {}
    dropped_incomplete = 0

    for rec in recs:
        if not rec.training_eligible:
            dropped_by_origin[rec.origin] = dropped_by_origin.get(rec.origin, 0) + 1
            continue
        if rec.chosen is None or rec.rejected is None:
            dropped_incomplete += 1
            continue
        pairs.append(
            {
                "record_id": rec.record_id,
                "origin": rec.origin,
                "chosen": rec.chosen,
                "rejected": rec.rejected,
            }
        )

    kept = len(pairs)
    return {
        "pairs": pairs,
        "kept": kept,
        "dropped": len(recs) - kept,
        "dropped_by_origin": dropped_by_origin,
        "dropped_incomplete": dropped_incomplete,
        "n_input": len(recs),
    }


# --------------------------------------------------------------------------- #
# Dual rates
# --------------------------------------------------------------------------- #

def solved_rate(records: Iterable[Any]) -> float:
    """Fraction of records solved on *any* attempt. ``0.0`` for an empty set."""
    recs = coerce_records(records)
    if not recs:
        return 0.0
    return sum(1 for r in recs if r.solved) / len(recs)


def pass_at_1_rate(records: Iterable[Any]) -> float:
    """Fraction solved on a *genuine first attempt*. ``0.0`` for an empty set.

    This is always ``<= solved_rate`` by construction: ``first_attempt_solved``
    is forced false when ``solved`` is false.
    """
    recs = coerce_records(records)
    if not recs:
        return 0.0
    return sum(1 for r in recs if r.first_attempt_solved) / len(recs)


def outcome_rates(records: Iterable[Any]) -> dict[str, Any]:
    """Both rates side by side, never blended into one headline number.

    Returns ``{"n", "n_solved", "n_pass_at_1", "solved_rate", "pass_at_1_rate",
    "retry_lift"}`` where ``retry_lift`` is ``solved_rate - pass_at_1_rate``:
    the share of the headline that only exists because of extra attempts.
    """
    recs = coerce_records(records)
    n = len(recs)
    n_solved = sum(1 for r in recs if r.solved)
    n_first = sum(1 for r in recs if r.first_attempt_solved)
    solved = (n_solved / n) if n else 0.0
    first = (n_first / n) if n else 0.0
    return {
        "n": n,
        "n_solved": n_solved,
        "n_pass_at_1": n_first,
        "solved_rate": solved,
        "pass_at_1_rate": first,
        "retry_lift": solved - first,
    }


# --------------------------------------------------------------------------- #
# Run envelopes
# --------------------------------------------------------------------------- #

@dataclass(frozen=True)
class RunEnvelope:
    """A declared purpose for a set of results, with provenance enforcement.

    ``kind`` is one of :data:`ENVELOPE_KINDS`. For the sealed kinds
    (:data:`SEALED_ENVELOPE_KINDS`) a ``dev_attested`` record is rejected
    unconditionally — passing ``allow_dev_attested=True`` for one of those kinds
    is itself an error, so there is no override path to reach for under
    deadline. Only ``private_audit`` may admit dev-attested records, and only
    when the caller says so explicitly.
    """

    kind: str
    allow_dev_attested: bool = False

    def __post_init__(self) -> None:
        if self.kind not in ENVELOPE_KINDS:
            raise ValueError(
                f"unknown envelope kind {self.kind!r}; "
                f"expected one of {sorted(ENVELOPE_KINDS)}"
            )
        if self.allow_dev_attested and self.kind in SEALED_ENVELOPE_KINDS:
            raise EnvelopeViolation(
                f"allow_dev_attested is not available for envelope kind "
                f"{self.kind!r}; dev-attested records are excluded unconditionally"
            )

    def admits(self, record: Any) -> bool:
        """Whether ``record`` may enter this envelope (no exception raised)."""
        rec = coerce_records([record])[0]
        if rec.origin != ORIGIN_DEV_ATTESTED:
            return True
        return self.kind == ENVELOPE_PRIVATE_AUDIT and self.allow_dev_attested

    def admit(self, record: Any) -> EvalRecord:
        """Return ``record`` normalized, or raise :class:`EnvelopeViolation`."""
        rec = coerce_records([record])[0]
        if self.admits(rec):
            return rec
        if self.kind == ENVELOPE_PRIVATE_AUDIT:
            raise EnvelopeViolation(
                f"record {rec.record_id!r} is dev_attested; a private_audit "
                f"envelope requires an explicit allow_dev_attested=True"
            )
        raise EnvelopeViolation(
            f"record {rec.record_id!r} is dev_attested and can never enter a "
            f"{self.kind!r} envelope"
        )

    def seal(self, records: Iterable[Any]) -> list[EvalRecord]:
        """Admit every record, raising on the first violation.

        Fails closed and eagerly: a run that contains one inadmissible record
        does not get partially reported.
        """
        return [self.admit(rec) for rec in coerce_records(records)]

    def report(self, records: Iterable[Any]) -> dict[str, Any]:
        """Seal ``records`` and return the dual-rate report tagged with the kind."""
        sealed = self.seal(records)
        return {
            "envelope": self.kind,
            "allow_dev_attested": self.allow_dev_attested,
            **outcome_rates(sealed),
        }


# --------------------------------------------------------------------------- #
# Stratified metrics
# --------------------------------------------------------------------------- #

_UNSPECIFIED_BUCKET = "__unspecified__"


def _bucket_value(record: EvalRecord, key: str) -> str:
    """Stringified stratum value for ``key``, or a sentinel when absent."""
    if key in record.strata:
        return str(record.strata[key])
    if key in record.payload:
        return str(record.payload[key])
    return _UNSPECIFIED_BUCKET


def stratified_metrics(
    records: Iterable[Any],
    keys: Sequence[str] = ("goal_shape", "depth", "domain"),
) -> dict[str, Any]:
    """Per-bucket rates reported *alongside* the aggregate, never instead of it.

    ``keys`` names the stratification axes to read off each record's ``strata``
    (falling back to ``payload``). For each axis the result maps bucket value ->
    the full :func:`outcome_rates` block, so ``solved_rate`` and
    ``pass_at_1_rate`` stay distinct inside every bucket too.

    The result also carries ``spread`` per axis — ``max - min`` of the bucket
    ``solved_rate`` values — which is the number that makes an
    aggregate-conceals-buckets situation visible at a glance.
    """
    recs = coerce_records(records)
    aggregate = outcome_rates(recs)

    buckets: dict[str, dict[str, Any]] = {}
    spread: dict[str, float] = {}
    for key in keys:
        grouped: dict[str, list[EvalRecord]] = {}
        for rec in recs:
            grouped.setdefault(_bucket_value(rec, key), []).append(rec)
        per_bucket = {name: outcome_rates(group) for name, group in sorted(grouped.items())}
        buckets[key] = per_bucket
        rates = [b["solved_rate"] for b in per_bucket.values()]
        spread[key] = (max(rates) - min(rates)) if rates else 0.0

    return {"aggregate": aggregate, "buckets": buckets, "spread": spread}


# --------------------------------------------------------------------------- #
# Score / verdict telemetry
# --------------------------------------------------------------------------- #

@dataclass(frozen=True)
class ScoreVerdict:
    """One candidate's self-reported score paired with the verifier's verdict.

    ``score`` is whatever the priority function produced (model confidence,
    heuristic value, log-prob); ``verified`` is the verifier's binary verdict.
    Keeping both on one record is what makes it possible to ask whether the
    score ordering and the truth ordering agree.
    """

    candidate_id: str
    score: float
    verified: bool

    def __post_init__(self) -> None:
        object.__setattr__(self, "score", float(self.score))
        object.__setattr__(self, "verified", bool(self.verified))

    @classmethod
    def from_mapping(cls, data: Mapping[str, Any]) -> "ScoreVerdict":
        """Build from a mapping with ``score`` and ``verified``/``verdict``."""
        if "verified" in data:
            verified = bool(data["verified"])
        elif "verdict" in data:
            verdict = data["verdict"]
            if isinstance(verdict, str):
                verified = verdict.strip().lower() in {"ok", "verified", "pass", "true"}
            else:
                verified = bool(verdict)
        else:
            # Unknown verdict is treated as unverified: telemetry must never be
            # inflated by records the verifier did not actually bless.
            verified = False
        return cls(
            candidate_id=str(data.get("candidate_id", data.get("id", ""))),
            score=float(data.get("score", 0.0)),
            verified=verified,
        )


def _coerce_candidates(candidates: Iterable[Any]) -> list[ScoreVerdict]:
    out: list[ScoreVerdict] = []
    for cand in candidates:
        if isinstance(cand, ScoreVerdict):
            out.append(cand)
        elif isinstance(cand, Mapping):
            out.append(ScoreVerdict.from_mapping(cand))
        else:
            raise TypeError(
                f"expected ScoreVerdict or mapping, got {type(cand).__name__}"
            )
    return out


def ranking_quality(candidates: Iterable[Any]) -> dict[str, Any]:
    """How well the score ordering agrees with the verifier, for one problem.

    Over every (verified, unverified) candidate pair, a pair is *concordant*
    when the verified candidate scored strictly higher, *tied* when the scores
    are equal, and discordant otherwise. ``ranking_quality`` is
    ``(concordant + 0.5 * tied) / n_pairs`` — the rank-order agreement (an AUC),
    where ``1.0`` means the priority function put every verified candidate above
    every unverified one and ``0.5`` means it ranked no better than a coin.

    Returns ``{"ranking_quality", "n_pairs", "n_concordant", "n_discordant",
    "n_tied", "top1_verified", "n_verified", "n"}``. ``ranking_quality`` is
    ``None`` when there is no pair to compare (all-verified or none-verified),
    because a degenerate case must not be reported as a perfect score.
    ``top1_verified`` is whether the highest-scored candidate was verified,
    breaking ties pessimistically (a tie at the top does not count as a hit
    unless every tied candidate is verified).
    """
    cands = _coerce_candidates(candidates)
    verified = [c for c in cands if c.verified]
    unverified = [c for c in cands if not c.verified]

    concordant = 0
    discordant = 0
    tied = 0
    for v in verified:
        for u in unverified:
            if v.score > u.score:
                concordant += 1
            elif v.score < u.score:
                discordant += 1
            else:
                tied += 1

    n_pairs = concordant + discordant + tied
    quality = ((concordant + 0.5 * tied) / n_pairs) if n_pairs else None

    if cands:
        top_score = max(c.score for c in cands)
        top1 = all(c.verified for c in cands if c.score == top_score)
    else:
        top1 = False

    return {
        "ranking_quality": quality,
        "n_pairs": n_pairs,
        "n_concordant": concordant,
        "n_discordant": discordant,
        "n_tied": tied,
        "top1_verified": top1,
        "n_verified": len(verified),
        "n": len(cands),
    }


def aggregate_ranking_quality(groups: Iterable[Iterable[Any]]) -> dict[str, Any]:
    """Pool :func:`ranking_quality` over many problems (one group per problem).

    Pairs are pooled across groups (a micro-average) so a problem with many
    candidates weighs proportionally, and degenerate groups contribute no pairs
    rather than an imaginary ``1.0``. ``top1_rate`` is the fraction of
    non-empty groups whose top-scored candidate was verified.
    """
    concordant = discordant = tied = 0
    n_groups = 0
    top1_hits = 0
    for group in groups:
        res = ranking_quality(group)
        if res["n"] == 0:
            continue
        n_groups += 1
        concordant += res["n_concordant"]
        discordant += res["n_discordant"]
        tied += res["n_tied"]
        top1_hits += 1 if res["top1_verified"] else 0

    n_pairs = concordant + discordant + tied
    return {
        "ranking_quality": ((concordant + 0.5 * tied) / n_pairs) if n_pairs else None,
        "n_pairs": n_pairs,
        "n_concordant": concordant,
        "n_discordant": discordant,
        "n_tied": tied,
        "n_groups": n_groups,
        "top1_rate": (top1_hits / n_groups) if n_groups else 0.0,
    }


# --------------------------------------------------------------------------- #
# Environment fingerprint
# --------------------------------------------------------------------------- #

def _canonical(value: Any) -> Any:
    """Canonical, order-insensitive form of a nested structure for hashing.

    Mappings become sorted key/value pair lists, and sets and sequences become
    lists sorted by canonical ``repr``. Ordering is therefore irrelevant: two
    environments that differ only in how their manifest was assembled hash the
    same, while any change of *content* changes the digest.
    """
    if isinstance(value, Mapping):
        return [[str(k), _canonical(value[k])] for k in sorted(value, key=str)]
    if isinstance(value, (set, frozenset)):
        return sorted((repr(_canonical(v)) for v in value))
    if isinstance(value, (list, tuple)):
        return sorted((_canonical(v) for v in value), key=repr)
    if isinstance(value, bool) or value is None:
        return value
    if isinstance(value, (int, float, str)):
        return value
    return repr(value)


def environment_fingerprint(
    toolchain: Any,
    corpus_pin: Any,
    import_manifest: Any,
) -> str:
    """Stable SHA-256 hex digest of the environment a result was produced in.

    ``toolchain`` (compiler/prover versions), ``corpus_pin`` (mathlib or corpus
    revision) and ``import_manifest`` (the set of imports in scope) are hashed
    together after canonicalization, so the digest is insensitive to dict
    ordering but changes if *any* component changes. A cached or reported pass
    is only comparable to another result with the same fingerprint.

    Deterministic across processes: no salt, no clock, no ``hash()``.
    """
    blob = json.dumps(
        {
            "toolchain": _canonical(toolchain),
            "corpus_pin": _canonical(corpus_pin),
            "import_manifest": _canonical(import_manifest),
        },
        sort_keys=True,
        separators=(",", ":"),
        default=str,
    )
    return hashlib.sha256(blob.encode("utf-8")).hexdigest()


def same_environment(fingerprint_a: str, fingerprint_b: str) -> bool:
    """Whether two results are comparable at all (identical fingerprints)."""
    return fingerprint_a == fingerprint_b


# --------------------------------------------------------------------------- #
# Deterministic audit sampling
# --------------------------------------------------------------------------- #
# A cheap automated judge labels a large set. Some of those labels get expensive
# human or ground-truth review. Which ones is the whole question.
#
# The tempting queue is "review the hard cases and the ones the judges disagreed
# about". That queue is stratified toward exactly the items where the judge is
# weakest, so an agreement rate measured on it is not an estimate of agreement on
# the population: it is an estimate of agreement on the worst slice, and it can
# understate or overstate the real number by an unbounded amount. The mined
# corpus reported 15 of 35 human-versus-AI agreement off such a queue, which is
# why that figure cannot be interpreted at all.
#
# The fix is to audit a fixed fraction of the CONFIDENT cases, chosen by hashing
# (item id, run salt). Deterministic means: reproducible from the two inputs
# alone, independent of iteration order, dict ordering, the clock, and any RNG,
# and therefore impossible to re-roll until the audit looks good. Publish the
# salt with the run and anyone can recompute the identical sample.

#: Default share of confident items sent for expensive review.
DEFAULT_AUDIT_FRACTION = 0.10

#: Bits of the digest consumed to place an item in [0, 1). 64 bits is far more
#: resolution than any realistic fraction needs.
_KEY_BITS = 64
_KEY_HEX = _KEY_BITS // 4
_KEY_SPACE = float(1 << _KEY_BITS)


class SamplingError(ValueError):
    """An audit sample was requested with parameters that cannot be honored."""


def audit_key(item_id: str, salt: str) -> str:
    """Stable selection key for one item under one run salt.

    ``sha256(salt || 0x00 || item_id)``. The NUL separator is what stops
    ``("ab", "c")`` and ``("a", "bc")`` from colliding, which would otherwise let
    a crafted id impersonate another item's key. The salt goes first so that a
    run salt cannot be forged by appending to an id.
    """
    blob = f"{salt}\x00{item_id}".encode("utf-8")
    return hashlib.sha256(blob).hexdigest()


def audit_key_position(item_id: str, salt: str) -> float:
    """The item's position in ``[0, 1)`` under this salt.

    Selection is ``position < fraction``. This is a function of the pair alone,
    so it does not matter in what order the items arrive or how many there are.
    """
    return int(audit_key(item_id, salt)[:_KEY_HEX], 16) / _KEY_SPACE


@dataclass(frozen=True)
class JudgedItem:
    """One item a cheap automated judge has labelled.

    ``confident`` is the judge's own claim that it is sure. It is the population
    the audit samples from, because a confident label is precisely the one nobody
    would otherwise check.
    """

    item_id: str
    label: Any = None
    confident: bool = True
    payload: dict[str, Any] = field(default_factory=dict)

    def __post_init__(self) -> None:
        if not self.item_id:
            raise SamplingError("every judged item needs a non-empty item_id")
        object.__setattr__(self, "item_id", str(self.item_id))
        object.__setattr__(self, "confident", bool(self.confident))

    @classmethod
    def from_mapping(cls, data: Mapping[str, Any]) -> "JudgedItem":
        return cls(
            item_id=str(data.get("item_id", data.get("id", ""))),
            label=data.get("label"),
            confident=bool(data.get("confident", True)),
            payload=dict(data.get("payload", {}) or {}),
        )


def _coerce_items(items: Iterable[Any]) -> list[JudgedItem]:
    out: list[JudgedItem] = []
    for item in items:
        if isinstance(item, JudgedItem):
            out.append(item)
        elif isinstance(item, Mapping):
            out.append(JudgedItem.from_mapping(item))
        elif isinstance(item, str):
            out.append(JudgedItem(item_id=item))
        else:
            raise TypeError(
                f"expected JudgedItem, mapping or str id, got {type(item).__name__}"
            )
    return out


@dataclass(frozen=True)
class AuditSample:
    """A reproducible audit sample plus everything needed to re-derive it.

    ``salt`` and ``fraction`` are carried on the sample rather than left in the
    caller's head, because a rate computed from this sample must disclose both
    (see :class:`SampledRate`). ``denominator`` is the confident population the
    fraction was applied to, which is the number a reader needs to judge how
    much the sampled rate is actually worth.
    """

    salt: str
    fraction: float
    item_ids: tuple[str, ...]
    denominator: int
    population: int
    n_unconfident: int

    @property
    def size(self) -> int:
        return len(self.item_ids)

    @property
    def coverage(self) -> float:
        """Realized share of the confident population, not the requested one."""
        return (self.size / self.denominator) if self.denominator else 0.0

    def contains(self, item_id: str) -> bool:
        return item_id in set(self.item_ids)

    def to_dict(self) -> dict[str, Any]:
        return {
            "salt": self.salt,
            "fraction": self.fraction,
            "item_ids": list(self.item_ids),
            "size": self.size,
            "denominator": self.denominator,
            "population": self.population,
            "n_unconfident": self.n_unconfident,
            "coverage": self.coverage,
        }


def select_audit_sample(
    items: Iterable[Any],
    salt: str,
    fraction: float = DEFAULT_AUDIT_FRACTION,
    *,
    confident_only: bool = True,
) -> AuditSample:
    """Pick the audit sample deterministically from ``(item_id, salt)``.

    An item is selected exactly when ``audit_key_position(item_id, salt) <
    fraction``. Consequences that matter:

    * The result does not depend on input order, on how many items were passed,
      or on anything mutable. Feeding the same ids shuffled gives the same
      sample.
    * There is no RNG and no clock, so the sample cannot be re-rolled until it
      looks favourable. Publishing the salt makes the choice checkable by anyone.
    * Adding new items never removes an existing one from the sample, so a sample
      taken mid-run stays valid as the run grows.

    ``confident_only`` (the default) restricts the population to items the judge
    was confident about. That is the point of the mechanism: the confident cases
    are the ones nothing else will ever check.

    Duplicate ids are collapsed, and the id list is returned sorted, so the
    output is canonical.
    """
    if not isinstance(salt, str) or not salt:
        raise SamplingError("audit sampling requires a non-empty run salt")
    fraction = float(fraction)
    if not 0.0 <= fraction <= 1.0:
        raise SamplingError(f"fraction must be in [0, 1], got {fraction!r}")

    coerced = _coerce_items(items)
    population = len({i.item_id for i in coerced})
    pool_ids = sorted(
        {i.item_id for i in coerced if i.confident or not confident_only}
    )
    n_unconfident = population - len(pool_ids)

    selected = tuple(
        item_id
        for item_id in pool_ids
        if audit_key_position(item_id, salt) < fraction
    )
    return AuditSample(
        salt=salt,
        fraction=fraction,
        item_ids=selected,
        denominator=len(pool_ids),
        population=population,
        n_unconfident=n_unconfident,
    )


@dataclass(frozen=True)
class SampledRate:
    """A rate measured on an audit sample, which cannot be reported bare.

    Every serialization carries ``sampled: true`` alongside the fraction, the
    salt and the denominator. That is the entire reason this type exists: a rate
    computed on 10 percent of the confident cases and printed as though it
    covered the run is the specific failure the audit mechanism was built to
    prevent, and a plain float has nowhere to keep the caveat.

    ``rate`` is ``None`` when nothing was reviewed. A rate over zero reviews is
    not 0.0 and not 1.0; it is absent.
    """

    name: str
    numerator: int
    reviewed: int
    audit_fraction: float
    audit_salt: str
    denominator: int
    population: int

    @property
    def rate(self) -> Any:
        return (self.numerator / self.reviewed) if self.reviewed else None

    @property
    def disclosure(self) -> str:
        """One sentence that must travel with the number wherever it goes."""
        return (
            f"{self.name} measured on a deterministic audit sample: "
            f"{self.reviewed} of {self.denominator} confident items "
            f"(population {self.population}), fraction "
            f"{self.audit_fraction:.4g}, salt {self.audit_salt!r}. "
            "Reproduce with select_audit_sample(items, salt, fraction). "
            "This is a sample estimate, not a rate over the full run."
        )

    def to_dict(self) -> dict[str, Any]:
        return {
            "name": self.name,
            "rate": self.rate,
            "numerator": self.numerator,
            "reviewed": self.reviewed,
            "sampled": True,
            "audit_fraction": self.audit_fraction,
            "audit_salt": self.audit_salt,
            "denominator": self.denominator,
            "population": self.population,
            "disclosure": self.disclosure,
        }

    def __str__(self) -> str:  # pragma: no cover - convenience only
        shown = "n/a" if self.rate is None else f"{self.rate:.4g}"
        return f"{self.name}={shown} [{self.disclosure}]"


def audit_agreement(
    items: Iterable[Any],
    salt: str,
    ground_truth: Mapping[str, Any],
    fraction: float = DEFAULT_AUDIT_FRACTION,
    *,
    confident_only: bool = True,
) -> dict[str, Any]:
    """Judge-versus-review agreement over a deterministic audit sample.

    ``ground_truth`` maps item id to the reviewed label; ids absent from it were
    not reviewed and are counted as ``n_unreviewed`` rather than quietly treated
    as agreements. The returned ``agreement`` is a :class:`SampledRate` dict, so
    the fraction, the salt and the denominator travel with the number.

    Nothing here says the judge is right. It says how often the judge and the
    reviewer said the same thing on an unbiased slice of the confident cases.
    """
    coerced = _coerce_items(items)
    sample = select_audit_sample(
        coerced, salt, fraction, confident_only=confident_only
    )
    by_id = {i.item_id: i for i in coerced}

    agree = 0
    reviewed = 0
    unreviewed: list[str] = []
    disagreements: list[dict[str, Any]] = []
    for item_id in sample.item_ids:
        if item_id not in ground_truth:
            unreviewed.append(item_id)
            continue
        reviewed += 1
        judged = by_id[item_id].label
        truth = ground_truth[item_id]
        if judged == truth:
            agree += 1
        else:
            disagreements.append(
                {"item_id": item_id, "judge_label": judged, "review_label": truth}
            )

    rate = SampledRate(
        name="judge_review_agreement",
        numerator=agree,
        reviewed=reviewed,
        audit_fraction=sample.fraction,
        audit_salt=sample.salt,
        denominator=sample.denominator,
        population=sample.population,
    )
    return {
        "agreement": rate.to_dict(),
        "sample": sample.to_dict(),
        "n_unreviewed": len(unreviewed),
        "unreviewed": unreviewed,
        "disagreements": disagreements,
    }


# --------------------------------------------------------------------------- #
# Evidence pinning by content hash
# --------------------------------------------------------------------------- #
# A judgement is about an artifact. Naming the artifact by path is not enough:
# the file changes, and then nobody can tell whether the finding was about what
# is on disk now. Pinning the content hash makes the pairing checkable, and makes
# a later mutation detectable rather than invisible.
#
# In the mined corpus this produced a concrete, checkable result: 338 of 338
# findings named a file that was present in the pinned diff, zero dangling
# references. That is exactly the property :func:`check_findings` measures.
#
# What a content hash does NOT do: it does not make the judgement correct, and it
# does not make the artifact good. It pins bytes. Every report below repeats that
# in ``pin_semantics`` so the guarantee is never quietly upgraded.

#: The one sentence that must accompany any pin-based claim.
PIN_SEMANTICS = (
    "A content hash pins bytes and nothing more: it proves which exact artifact "
    "was judged and makes a later mutation detectable. It does not make the "
    "judgement correct, complete, or well-founded."
)


class EvidenceError(ValueError):
    """A pin is malformed, or a finding refers to evidence that was not pinned."""


def content_digest(content: Any) -> str:
    """``sha256:<hex>`` over the artifact's bytes.

    ``str`` is encoded as UTF-8; ``bytes`` is hashed as given. Anything else is
    refused rather than coerced, because ``repr``-hashing an arbitrary object
    would pin a Python rendering instead of the artifact.
    """
    if isinstance(content, bytes):
        raw = content
    elif isinstance(content, str):
        raw = content.encode("utf-8")
    else:
        raise EvidenceError(
            f"can only pin bytes or str, got {type(content).__name__}"
        )
    return "sha256:" + hashlib.sha256(raw).hexdigest()


@dataclass(frozen=True)
class EvidencePin:
    """One artifact pinned by content at the moment it was judged."""

    path: str
    digest: str
    size_bytes: int

    @classmethod
    def of(cls, path: str, content: Any) -> "EvidencePin":
        raw = content.encode("utf-8") if isinstance(content, str) else content
        if not isinstance(raw, bytes):
            raise EvidenceError(
                f"can only pin bytes or str, got {type(content).__name__}"
            )
        return cls(path=str(path), digest=content_digest(raw), size_bytes=len(raw))

    def matches(self, content: Any) -> bool:
        """Whether ``content`` is byte-identical to what was judged."""
        return content_digest(content) == self.digest

    def to_dict(self) -> dict[str, Any]:
        return {
            "path": self.path,
            "digest": self.digest,
            "size_bytes": self.size_bytes,
            "pin_semantics": PIN_SEMANTICS,
        }


def pin_evidence(artifacts: Mapping[str, Any]) -> dict[str, EvidencePin]:
    """Pin every ``path -> content`` pair. The pinned set is the judged set."""
    return {str(path): EvidencePin.of(path, content) for path, content in artifacts.items()}


@dataclass(frozen=True)
class Finding:
    """One judgement, naming the artifact it was made about."""

    finding_id: str
    path: str
    detail: Any = None

    @classmethod
    def from_mapping(cls, data: Mapping[str, Any]) -> "Finding":
        path = data.get("path", data.get("file"))
        if not path:
            raise EvidenceError(
                f"finding {data.get('finding_id', data.get('id'))!r} names no artifact"
            )
        return cls(
            finding_id=str(data.get("finding_id", data.get("id", ""))),
            path=str(path),
            detail=data.get("detail"),
        )


def _coerce_findings(findings: Iterable[Any]) -> list[Finding]:
    out: list[Finding] = []
    for f in findings:
        if isinstance(f, Finding):
            out.append(f)
        elif isinstance(f, Mapping):
            out.append(Finding.from_mapping(f))
        else:
            raise TypeError(f"expected Finding or mapping, got {type(f).__name__}")
    return out


def check_findings(
    findings: Iterable[Any], pins: Mapping[str, Any]
) -> dict[str, Any]:
    """Check that every finding names an artifact that was actually pinned.

    A finding whose path is absent from ``pins`` is *dangling*: the judgement is
    about something outside the evidence set, so it cannot be checked against
    what was judged. Those are listed, not dropped.

    Returns ``{"n_findings", "n_pinned", "n_dangling", "dangling", "all_pinned",
    "pin_semantics"}``. ``all_pinned`` being true is the "338 of 338, zero
    dangling" property; it says the findings and the evidence line up, and says
    nothing at all about whether the findings are right.
    """
    items = _coerce_findings(findings)
    known = set(pins.keys())
    dangling = sorted({f.path for f in items if f.path not in known})
    n_dangling = sum(1 for f in items if f.path not in known)
    return {
        "n_findings": len(items),
        "n_pinned": len(items) - n_dangling,
        "n_dangling": n_dangling,
        "dangling": dangling,
        "all_pinned": n_dangling == 0,
        "pin_semantics": PIN_SEMANTICS,
    }


def verify_pins(
    pins: Mapping[str, Any], current: Mapping[str, Any]
) -> dict[str, Any]:
    """Re-check pinned artifacts against their current content.

    ``pins`` maps path to an :class:`EvidencePin` or its dict form; ``current``
    maps path to the bytes/str on disk now. A path whose bytes changed is
    ``mutated``; a path absent from ``current`` is ``missing``. Both make every
    finding about that artifact unsafe to re-use, which is the whole point of
    pinning rather than merely naming.
    """
    mutated: list[dict[str, Any]] = []
    missing: list[str] = []
    intact = 0
    for path in sorted(pins.keys()):
        pin = pins[path]
        if isinstance(pin, Mapping):
            pin = EvidencePin(
                path=str(pin.get("path", path)),
                digest=str(pin["digest"]),
                size_bytes=int(pin.get("size_bytes", 0)),
            )
        if not isinstance(pin, EvidencePin):
            raise EvidenceError(f"pin for {path!r} is not an EvidencePin")
        if path not in current:
            missing.append(path)
            continue
        if pin.matches(current[path]):
            intact += 1
        else:
            mutated.append(
                {
                    "path": path,
                    "pinned_digest": pin.digest,
                    "current_digest": content_digest(current[path]),
                }
            )
    return {
        "n_pins": len(pins),
        "n_intact": intact,
        "n_mutated": len(mutated),
        "n_missing": len(missing),
        "mutated": mutated,
        "missing": missing,
        "all_intact": not mutated and not missing,
        "pin_semantics": PIN_SEMANTICS,
    }


# --------------------------------------------------------------------------- #
# JSON dispatch (worker.py hook) + CLI
# --------------------------------------------------------------------------- #

def run(request: dict[str, Any]) -> dict[str, Any]:
    """JSON dispatch for the ``eval_integrity`` worker op.

    Sub-ops (``op`` field, default ``outcome_rates``):

    * ``export_preference_corpus`` -> the filtered corpus + dropped counts
    * ``outcome_rates`` (default) -> the dual-rate block
    * ``envelope_report`` -> ``{"envelope", ...rates}``; raises
      :class:`EnvelopeViolation` on a provenance violation
    * ``stratified_metrics`` -> ``{"aggregate", "buckets", "spread"}``
    * ``ranking_quality`` -> single-group telemetry
    * ``aggregate_ranking_quality`` -> pooled telemetry over ``groups``
    * ``environment_fingerprint`` -> ``{"fingerprint": hex}``
    * ``select_audit_sample`` -> the deterministic sample + its disclosure fields
    * ``audit_agreement`` -> judge-versus-review agreement as a disclosed rate
    * ``pin_evidence`` -> ``path -> {digest, size_bytes}``
    * ``check_findings`` -> dangling-reference audit over the pinned set
    * ``verify_pins`` -> re-check pinned artifacts for later mutation
    """
    op = request.get("op", "outcome_rates")
    if op == "export_preference_corpus":
        return {"op": op, **export_preference_corpus(request.get("records", []))}
    if op == "outcome_rates":
        return {"op": op, **outcome_rates(request.get("records", []))}
    if op == "envelope_report":
        envelope = RunEnvelope(
            kind=str(request["envelope"]),
            allow_dev_attested=bool(request.get("allow_dev_attested", False)),
        )
        return {"op": op, **envelope.report(request.get("records", []))}
    if op == "stratified_metrics":
        keys = tuple(request.get("keys") or ("goal_shape", "depth", "domain"))
        return {"op": op, **stratified_metrics(request.get("records", []), keys)}
    if op == "ranking_quality":
        return {"op": op, **ranking_quality(request.get("candidates", []))}
    if op == "aggregate_ranking_quality":
        return {"op": op, **aggregate_ranking_quality(request.get("groups", []))}
    if op == "environment_fingerprint":
        return {
            "op": op,
            "fingerprint": environment_fingerprint(
                request.get("toolchain"),
                request.get("corpus_pin"),
                request.get("import_manifest"),
            ),
        }
    if op == "select_audit_sample":
        sample = select_audit_sample(
            request.get("items", []),
            str(request["salt"]),
            float(request.get("fraction", DEFAULT_AUDIT_FRACTION)),
            confident_only=bool(request.get("confident_only", True)),
        )
        return {"op": op, **sample.to_dict()}
    if op == "audit_agreement":
        return {
            "op": op,
            **audit_agreement(
                request.get("items", []),
                str(request["salt"]),
                dict(request.get("ground_truth", {})),
                float(request.get("fraction", DEFAULT_AUDIT_FRACTION)),
                confident_only=bool(request.get("confident_only", True)),
            ),
        }
    if op == "pin_evidence":
        pins = pin_evidence(request.get("artifacts", {}))
        return {
            "op": op,
            "pins": {path: pin.to_dict() for path, pin in pins.items()},
            "n_pins": len(pins),
            "pin_semantics": PIN_SEMANTICS,
        }
    if op == "check_findings":
        return {
            "op": op,
            **check_findings(request.get("findings", []), request.get("pins", {})),
        }
    if op == "verify_pins":
        return {
            "op": op,
            **verify_pins(request.get("pins", {}), request.get("current", {})),
        }
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
