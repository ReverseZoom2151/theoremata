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
