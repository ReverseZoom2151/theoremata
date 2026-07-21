import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

import pytest  # noqa: E402

from theoremata_tools.eval_integrity import (  # noqa: E402
    EnvelopeViolation,
    EvalRecord,
    ProvenanceError,
    RunEnvelope,
    ScoreVerdict,
    aggregate_ranking_quality,
    environment_fingerprint,
    export_preference_corpus,
    outcome_rates,
    pass_at_1_rate,
    ranking_quality,
    run,
    same_environment,
    solved_rate,
    stratified_metrics,
)


def _rec(rid, origin, **kw):
    kw.setdefault("chosen", f"good-{rid}")
    kw.setdefault("rejected", f"bad-{rid}")
    return EvalRecord(record_id=rid, origin=origin, **kw)


# --- provenance model -----------------------------------------------------

def test_training_eligible_is_derived_from_origin():
    assert _rec("a", "organic").training_eligible is True
    assert _rec("b", "benchmark_aligned").training_eligible is True
    assert _rec("c", "dev_attested").training_eligible is False
    assert _rec("d", "synthetic_mutation").training_eligible is False


def test_training_eligible_cannot_be_asserted_by_caller():
    # the flag is init=False, so it is not even a constructor argument
    with pytest.raises(TypeError):
        EvalRecord(record_id="x", origin="dev_attested", training_eligible=True)
    # and a mapping claiming eligibility is ignored, not honored
    rec = EvalRecord.from_mapping(
        {"record_id": "x", "origin": "dev_attested", "training_eligible": True}
    )
    assert rec.training_eligible is False


def test_unknown_or_missing_origin_raises():
    with pytest.raises(ProvenanceError):
        EvalRecord(record_id="x", origin="vibes")
    with pytest.raises(ProvenanceError):
        EvalRecord.from_mapping({"record_id": "x"})


def test_record_is_frozen():
    rec = _rec("a", "dev_attested")
    with pytest.raises(Exception):
        rec.training_eligible = True  # type: ignore[misc]


# --- preference export firewall -------------------------------------------

def test_export_drops_dev_attested_and_synthetic_with_exact_count():
    records = [
        _rec("o1", "organic"),
        _rec("o2", "organic"),
        _rec("b1", "benchmark_aligned"),
        _rec("d1", "dev_attested"),
        _rec("d2", "dev_attested"),
        _rec("s1", "synthetic_mutation"),
    ]
    res = export_preference_corpus(records)
    assert res["n_input"] == 6
    assert res["kept"] == 3
    assert res["dropped"] == 3
    assert res["dropped_by_origin"] == {"dev_attested": 2, "synthetic_mutation": 1}
    ids = {p["record_id"] for p in res["pairs"]}
    assert ids == {"o1", "o2", "b1"}
    assert all(p["origin"] not in {"dev_attested", "synthetic_mutation"} for p in res["pairs"])


def test_export_drops_pairs_without_both_sides():
    records = [
        _rec("o1", "organic"),
        EvalRecord(record_id="o2", origin="organic", chosen="c"),  # no rejected
    ]
    res = export_preference_corpus(records)
    assert res["kept"] == 1
    assert res["dropped"] == 1
    assert res["dropped_incomplete"] == 1


def test_export_accepts_mappings_and_still_filters():
    res = export_preference_corpus(
        [
            {"record_id": "o1", "origin": "organic", "chosen": "c", "rejected": "r"},
            {"record_id": "d1", "origin": "dev_attested", "chosen": "c", "rejected": "r"},
        ]
    )
    assert res["kept"] == 1
    assert res["dropped"] == 1


def test_export_of_only_ineligible_records_is_empty_not_silent():
    res = export_preference_corpus([_rec("d1", "dev_attested"), _rec("s1", "synthetic_mutation")])
    assert res["pairs"] == []
    assert res["kept"] == 0
    assert res["dropped"] == 2


# --- dual rates -----------------------------------------------------------

def test_solved_rate_and_pass_at_1_differ():
    records = [
        _rec("a", "organic", solved=True, first_attempt_solved=True, attempts=1),
        _rec("b", "organic", solved=True, first_attempt_solved=False, attempts=7),
        _rec("c", "organic", solved=True, first_attempt_solved=False, attempts=3),
        _rec("d", "organic", solved=False, attempts=8),
    ]
    assert solved_rate(records) == pytest.approx(0.75)
    assert pass_at_1_rate(records) == pytest.approx(0.25)
    res = outcome_rates(records)
    assert res["solved_rate"] != res["pass_at_1_rate"]
    assert res["n_solved"] == 3
    assert res["n_pass_at_1"] == 1
    assert res["retry_lift"] == pytest.approx(0.5)


def test_pass_at_1_never_exceeds_solved_rate():
    # a record claiming a first-attempt solve while unsolved is corrected
    rec = _rec("a", "organic", solved=False, first_attempt_solved=True)
    assert rec.first_attempt_solved is False
    assert pass_at_1_rate([rec]) <= solved_rate([rec])


def test_rates_empty_are_zero_and_distinct_fields_present():
    res = outcome_rates([])
    assert res["solved_rate"] == 0.0
    assert res["pass_at_1_rate"] == 0.0
    assert res["n"] == 0
    assert "solved_rate" in res and "pass_at_1_rate" in res


# --- run envelopes --------------------------------------------------------

@pytest.mark.parametrize("kind", ["benchmark", "evaluation", "public_report"])
def test_dev_attested_rejected_from_sealed_envelopes(kind):
    env = RunEnvelope(kind=kind)
    with pytest.raises(EnvelopeViolation):
        env.admit(_rec("d1", "dev_attested"))
    with pytest.raises(EnvelopeViolation):
        env.seal([_rec("o1", "organic"), _rec("d1", "dev_attested")])


@pytest.mark.parametrize("kind", ["benchmark", "evaluation", "public_report"])
def test_sealed_envelopes_have_no_override_flag(kind):
    with pytest.raises(EnvelopeViolation):
        RunEnvelope(kind=kind, allow_dev_attested=True)


def test_private_audit_requires_explicit_flag():
    strict = RunEnvelope(kind="private_audit")
    with pytest.raises(EnvelopeViolation):
        strict.admit(_rec("d1", "dev_attested"))

    permissive = RunEnvelope(kind="private_audit", allow_dev_attested=True)
    admitted = permissive.admit(_rec("d1", "dev_attested"))
    assert admitted.record_id == "d1"
    assert admitted.training_eligible is False  # admitted, still never trainable


def test_non_dev_attested_records_pass_every_envelope():
    records = [
        _rec("o1", "organic", solved=True, first_attempt_solved=True),
        _rec("s1", "synthetic_mutation", solved=False),
        _rec("b1", "benchmark_aligned", solved=True),
    ]
    for kind in ("benchmark", "evaluation", "public_report", "private_audit"):
        assert len(RunEnvelope(kind=kind).seal(records)) == 3


def test_envelope_report_carries_both_rates():
    env = RunEnvelope(kind="public_report")
    res = env.report([
        _rec("a", "organic", solved=True, first_attempt_solved=True),
        _rec("b", "organic", solved=True, first_attempt_solved=False),
    ])
    assert res["envelope"] == "public_report"
    assert res["solved_rate"] == pytest.approx(1.0)
    assert res["pass_at_1_rate"] == pytest.approx(0.5)


def test_unknown_envelope_kind_raises():
    with pytest.raises(ValueError):
        RunEnvelope(kind="marketing")


# --- stratified metrics ---------------------------------------------------

def _bucketed(domain, n, n_solved):
    return [
        _rec(
            f"{domain}-{i}",
            "organic",
            solved=i < n_solved,
            first_attempt_solved=i < n_solved,
            strata={"domain": domain},
        )
        for i in range(n)
    ]


def test_aggregate_conceals_buckets_1655_case():
    # the reported regime: 16.55% overall while one bucket sits at 100%
    records = _bucketed("easy", 31, 31) + _bucketed("hard", 1969, 300)
    res = stratified_metrics(records, keys=("domain",))
    assert res["aggregate"]["n"] == 2000
    assert res["aggregate"]["solved_rate"] == pytest.approx(0.1655)
    buckets = res["buckets"]["domain"]
    assert buckets["easy"]["solved_rate"] == pytest.approx(1.0)
    assert buckets["hard"]["solved_rate"] == pytest.approx(300 / 1969)
    # the aggregate alone would have hidden a full-range spread
    assert res["spread"]["domain"] > 0.8


def test_stratified_reports_all_axes_and_keeps_rates_distinct():
    records = [
        _rec("a", "organic", solved=True, first_attempt_solved=True,
             strata={"goal_shape": "eq", "depth": 1, "domain": "algebra"}),
        _rec("b", "organic", solved=True, first_attempt_solved=False,
             strata={"goal_shape": "eq", "depth": 3, "domain": "algebra"}),
        _rec("c", "organic", solved=False,
             strata={"goal_shape": "forall", "depth": 3, "domain": "number_theory"}),
    ]
    res = stratified_metrics(records)
    assert set(res["buckets"]) == {"goal_shape", "depth", "domain"}
    eq = res["buckets"]["goal_shape"]["eq"]
    assert eq["solved_rate"] == pytest.approx(1.0)
    assert eq["pass_at_1_rate"] == pytest.approx(0.5)
    assert res["buckets"]["depth"]["3"]["solved_rate"] == pytest.approx(0.5)
    assert res["buckets"]["goal_shape"]["forall"]["solved_rate"] == 0.0


def test_stratified_missing_key_goes_to_sentinel_bucket():
    res = stratified_metrics([_rec("a", "organic", solved=True)], keys=("domain",))
    assert list(res["buckets"]["domain"]) == ["__unspecified__"]


def test_stratified_empty_is_safe():
    res = stratified_metrics([], keys=("domain",))
    assert res["aggregate"]["n"] == 0
    assert res["buckets"]["domain"] == {}
    assert res["spread"]["domain"] == 0.0


# --- score / verdict telemetry --------------------------------------------

def test_ranking_quality_perfect_ordering():
    cands = [
        ScoreVerdict("v1", 0.9, True),
        ScoreVerdict("v2", 0.8, True),
        ScoreVerdict("u1", 0.4, False),
        ScoreVerdict("u2", 0.1, False),
    ]
    res = ranking_quality(cands)
    assert res["ranking_quality"] == pytest.approx(1.0)
    assert res["n_pairs"] == 4
    assert res["n_discordant"] == 0
    assert res["top1_verified"] is True


def test_ranking_quality_inverted_ordering():
    cands = [
        ScoreVerdict("u1", 0.9, False),
        ScoreVerdict("v1", 0.1, True),
    ]
    res = ranking_quality(cands)
    assert res["ranking_quality"] == pytest.approx(0.0)
    assert res["top1_verified"] is False


def test_ranking_quality_hand_built_mixed_case():
    # verified {0.9, 0.3} vs unverified {0.7, 0.2}:
    # (0.9>0.7) ok, (0.9>0.2) ok, (0.3<0.7) bad, (0.3>0.2) ok  -> 3/4
    cands = [
        {"candidate_id": "v1", "score": 0.9, "verified": True},
        {"candidate_id": "u1", "score": 0.7, "verified": False},
        {"candidate_id": "v2", "score": 0.3, "verified": True},
        {"candidate_id": "u2", "score": 0.2, "verified": False},
    ]
    res = ranking_quality(cands)
    assert res["ranking_quality"] == pytest.approx(0.75)
    assert (res["n_concordant"], res["n_discordant"], res["n_tied"]) == (3, 1, 0)


def test_ranking_quality_ties_count_as_half():
    cands = [ScoreVerdict("v", 0.5, True), ScoreVerdict("u", 0.5, False)]
    res = ranking_quality(cands)
    assert res["ranking_quality"] == pytest.approx(0.5)
    assert res["n_tied"] == 1
    # a tie at the top is not credited as a top-1 hit
    assert res["top1_verified"] is False


def test_ranking_quality_degenerate_groups_are_none_not_one():
    assert ranking_quality([ScoreVerdict("v", 1.0, True)])["ranking_quality"] is None
    assert ranking_quality([ScoreVerdict("u", 1.0, False)])["ranking_quality"] is None
    assert ranking_quality([])["ranking_quality"] is None


def test_score_verdict_unknown_verdict_is_unverified():
    assert ScoreVerdict.from_mapping({"score": 1.0}).verified is False
    assert ScoreVerdict.from_mapping({"score": 1.0, "verdict": "ok"}).verified is True
    assert ScoreVerdict.from_mapping({"score": 1.0, "verdict": "error"}).verified is False


def test_aggregate_ranking_quality_pools_pairs():
    good = [ScoreVerdict("v", 0.9, True), ScoreVerdict("u", 0.1, False)]
    bad = [ScoreVerdict("u", 0.9, False), ScoreVerdict("v", 0.1, True)]
    res = aggregate_ranking_quality([good, bad])
    assert res["n_pairs"] == 2
    assert res["ranking_quality"] == pytest.approx(0.5)
    assert res["n_groups"] == 2
    assert res["top1_rate"] == pytest.approx(0.5)


# --- environment fingerprint ----------------------------------------------

_TOOLCHAIN = {"lean": "4.9.0", "rustc": "1.79.0"}
_PIN = "mathlib@abc123"
_MANIFEST = ["Mathlib.Algebra.Order", "Mathlib.Data.Nat.Basic"]


def test_fingerprint_is_stable_and_order_insensitive():
    a = environment_fingerprint(_TOOLCHAIN, _PIN, _MANIFEST)
    b = environment_fingerprint(
        {"rustc": "1.79.0", "lean": "4.9.0"},
        _PIN,
        list(reversed(_MANIFEST)),
    )
    assert a == b == environment_fingerprint(_TOOLCHAIN, _PIN, _MANIFEST)
    assert len(a) == 64
    assert same_environment(a, b)


def test_fingerprint_changes_when_any_component_changes():
    base = environment_fingerprint(_TOOLCHAIN, _PIN, _MANIFEST)
    assert environment_fingerprint({**_TOOLCHAIN, "lean": "4.10.0"}, _PIN, _MANIFEST) != base
    assert environment_fingerprint(_TOOLCHAIN, "mathlib@def456", _MANIFEST) != base
    assert environment_fingerprint(_TOOLCHAIN, _PIN, _MANIFEST + ["Mathlib.Tactic"]) != base
    assert not same_environment(
        base, environment_fingerprint(_TOOLCHAIN, "mathlib@def456", _MANIFEST)
    )


# --- run() dispatch -------------------------------------------------------

def test_run_default_op_is_outcome_rates():
    res = run({"records": [{"record_id": "a", "origin": "organic", "solved": True}]})
    assert res["op"] == "outcome_rates"
    assert res["solved_rate"] == 1.0
    assert res["pass_at_1_rate"] == 0.0


def test_run_export_reports_dropped():
    res = run(
        {
            "op": "export_preference_corpus",
            "records": [
                {"record_id": "o", "origin": "organic", "chosen": "c", "rejected": "r"},
                {"record_id": "d", "origin": "dev_attested", "chosen": "c", "rejected": "r"},
            ],
        }
    )
    assert res["kept"] == 1 and res["dropped"] == 1


def test_run_envelope_report_raises_on_dev_attested():
    with pytest.raises(EnvelopeViolation):
        run(
            {
                "op": "envelope_report",
                "envelope": "benchmark",
                "records": [{"record_id": "d", "origin": "dev_attested"}],
            }
        )


def test_run_stratified_and_ranking_and_fingerprint():
    strat = run(
        {
            "op": "stratified_metrics",
            "keys": ["domain"],
            "records": [{"record_id": "a", "origin": "organic", "solved": True,
                         "strata": {"domain": "algebra"}}],
        }
    )
    assert strat["buckets"]["domain"]["algebra"]["solved_rate"] == 1.0

    rank = run(
        {
            "op": "ranking_quality",
            "candidates": [
                {"candidate_id": "v", "score": 1.0, "verified": True},
                {"candidate_id": "u", "score": 0.0, "verified": False},
            ],
        }
    )
    assert rank["ranking_quality"] == pytest.approx(1.0)

    fp = run(
        {
            "op": "environment_fingerprint",
            "toolchain": _TOOLCHAIN,
            "corpus_pin": _PIN,
            "import_manifest": _MANIFEST,
        }
    )
    assert fp["fingerprint"] == environment_fingerprint(_TOOLCHAIN, _PIN, _MANIFEST)


def test_run_unknown_op_raises():
    with pytest.raises(ValueError):
        run({"op": "nope"})


# =========================================================================== #
# Deterministic audit sampling
# =========================================================================== #

from theoremata_tools.eval_integrity import (  # noqa: E402
    DEFAULT_AUDIT_FRACTION,
    PIN_SEMANTICS,
    EvidenceError,
    EvidencePin,
    Finding,
    JudgedItem,
    SampledRate,
    SamplingError,
    audit_agreement,
    audit_key,
    audit_key_position,
    check_findings,
    content_digest,
    pin_evidence,
    select_audit_sample,
    verify_pins,
)

_SALT = "run-2026-07-21-a"
_OTHER_SALT = "run-2026-07-21-b"


def _confident_ids(n=1000, prefix="item-"):
    return [f"{prefix}{i:04d}" for i in range(n)]


def test_audit_key_is_a_pure_function_of_id_and_salt():
    assert audit_key("item-1", _SALT) == audit_key("item-1", _SALT)
    assert audit_key("item-1", _SALT) != audit_key("item-1", _OTHER_SALT)
    assert audit_key("item-1", _SALT) != audit_key("item-2", _SALT)
    # The NUL separator stops ("ab","c") colliding with ("a","bc").
    assert audit_key("c", "ab") != audit_key("bc", "a")
    assert 0.0 <= audit_key_position("item-1", _SALT) < 1.0


def test_same_inputs_give_the_same_sample():
    ids = _confident_ids()
    a = select_audit_sample(ids, _SALT)
    b = select_audit_sample(ids, _SALT)
    assert a.item_ids == b.item_ids
    assert a.item_ids != ()


def test_sample_is_independent_of_input_order_and_duplication():
    """No dependence on iteration order, dict ordering, or the clock."""
    ids = _confident_ids()
    expected = select_audit_sample(ids, _SALT).item_ids
    shuffled = list(reversed(ids))
    duplicated = ids + ids[:100]
    assert select_audit_sample(shuffled, _SALT).item_ids == expected
    assert select_audit_sample(duplicated, _SALT).item_ids == expected
    # Mappings in a different key order describe the same items.
    as_mappings = [{"id": i, "confident": True, "label": "ok"} for i in shuffled]
    assert select_audit_sample(as_mappings, _SALT).item_ids == expected


def test_a_different_salt_gives_a_different_sample():
    ids = _confident_ids()
    a = set(select_audit_sample(ids, _SALT).item_ids)
    b = set(select_audit_sample(ids, _OTHER_SALT).item_ids)
    assert a != b
    # Not merely different: substantially so, as independent draws should be.
    assert len(a & b) < 0.5 * min(len(a), len(b))


def test_sampled_fraction_is_about_the_requested_fraction():
    ids = _confident_ids(2000)
    sample = select_audit_sample(ids, _SALT, DEFAULT_AUDIT_FRACTION)
    assert sample.denominator == 2000
    assert 150 <= sample.size <= 250, sample.size
    assert sample.coverage == pytest.approx(sample.size / 2000)


def test_growing_the_set_never_evicts_an_already_sampled_item():
    """A sample taken mid-run stays valid as the run grows."""
    first = select_audit_sample(_confident_ids(500), _SALT)
    later = select_audit_sample(_confident_ids(1000), _SALT)
    assert set(first.item_ids) <= set(later.item_ids)


def test_only_confident_items_are_sampled_by_default():
    items = [
        JudgedItem(item_id=i, label="ok", confident=(int(i.split("-")[1]) % 2 == 0))
        for i in _confident_ids(400)
    ]
    sample = select_audit_sample(items, _SALT)
    assert sample.population == 400
    assert sample.denominator == 200
    assert sample.n_unconfident == 200
    assert all(int(i.split("-")[1]) % 2 == 0 for i in sample.item_ids)
    # The bias this prevents: sampling the unconfident cases instead would be a
    # different, non-comparable population.
    other = select_audit_sample(items, _SALT, confident_only=False)
    assert other.denominator == 400
    assert set(sample.item_ids) < set(other.item_ids)


def test_sampling_requires_a_salt_and_a_legal_fraction():
    with pytest.raises(SamplingError):
        select_audit_sample(["a"], "")
    with pytest.raises(SamplingError):
        select_audit_sample(["a"], _SALT, 1.5)
    with pytest.raises(SamplingError):
        JudgedItem(item_id="")


def test_fraction_zero_and_one_are_the_honest_extremes():
    ids = _confident_ids(200)
    assert select_audit_sample(ids, _SALT, 0.0).item_ids == ()
    assert set(select_audit_sample(ids, _SALT, 1.0).item_ids) == set(ids)


# =========================================================================== #
# Disclosure of a sampled rate
# =========================================================================== #

def test_a_sampled_rate_always_discloses_fraction_salt_and_denominator():
    rate = SampledRate(
        name="judge_review_agreement",
        numerator=15,
        reviewed=35,
        audit_fraction=DEFAULT_AUDIT_FRACTION,
        audit_salt=_SALT,
        denominator=350,
        population=400,
    )
    payload = rate.to_dict()
    assert payload["sampled"] is True
    assert payload["audit_fraction"] == DEFAULT_AUDIT_FRACTION
    assert payload["audit_salt"] == _SALT
    assert payload["denominator"] == 350
    assert payload["rate"] == pytest.approx(15 / 35)
    for needle in ("35 of 350", _SALT, "sample estimate", "0.1"):
        assert needle in payload["disclosure"]


def test_a_rate_over_zero_reviews_is_absent_not_zero():
    rate = SampledRate("x", 0, 0, 0.1, _SALT, 100, 100)
    assert rate.rate is None
    assert rate.to_dict()["rate"] is None


def test_audit_agreement_reports_a_disclosed_rate():
    items = [JudgedItem(item_id=i, label="correct") for i in _confident_ids(500)]
    sample = select_audit_sample(items, _SALT)
    # Reviewer disagrees on every third sampled item.
    truth = {
        item_id: ("correct" if idx % 3 else "incorrect")
        for idx, item_id in enumerate(sample.item_ids)
    }
    report = audit_agreement(items, _SALT, truth)

    assert report["sample"]["item_ids"] == list(sample.item_ids)
    agreement = report["agreement"]
    assert agreement["sampled"] is True
    assert agreement["audit_salt"] == _SALT
    assert agreement["audit_fraction"] == DEFAULT_AUDIT_FRACTION
    assert agreement["denominator"] == 500
    assert agreement["reviewed"] == len(sample.item_ids)
    expected_agree = sum(1 for i, _ in enumerate(sample.item_ids) if i % 3)
    assert agreement["numerator"] == expected_agree
    assert report["n_unreviewed"] == 0
    assert len(report["disagreements"]) == agreement["reviewed"] - expected_agree


def test_unreviewed_sampled_items_are_counted_not_scored_as_agreement():
    items = [JudgedItem(item_id=i, label="correct") for i in _confident_ids(300)]
    sample = select_audit_sample(items, _SALT)
    partial = {item_id: "correct" for item_id in sample.item_ids[:3]}
    report = audit_agreement(items, _SALT, partial)
    assert report["agreement"]["reviewed"] == 3
    assert report["agreement"]["numerator"] == 3
    assert report["n_unreviewed"] == len(sample.item_ids) - 3
    # The denominator still discloses the whole confident population.
    assert report["agreement"]["denominator"] == 300


# =========================================================================== #
# Evidence pinning by content hash
# =========================================================================== #

def test_content_digest_pins_bytes_and_is_stable():
    assert content_digest("abc") == content_digest(b"abc")
    assert content_digest("abc").startswith("sha256:")
    assert content_digest("abc") != content_digest("abd")
    with pytest.raises(EvidenceError):
        content_digest({"not": "bytes"})


def test_pin_detects_a_later_mutation():
    pin = EvidencePin.of("src/a.lean", "theorem t : True := by trivial\n")
    assert pin.matches("theorem t : True := by trivial\n")
    assert not pin.matches("theorem t : True := by sorry\n")
    assert pin.size_bytes == len("theorem t : True := by trivial\n")


def test_every_finding_names_a_pinned_artifact():
    """The 338-of-338, zero-dangling property."""
    artifacts = {f"src/f{i}.lean": f"content {i}" for i in range(20)}
    pins = pin_evidence(artifacts)
    findings = [
        Finding(finding_id=f"F{i}", path=f"src/f{i % 20}.lean") for i in range(338)
    ]
    report = check_findings(findings, pins)
    assert report["n_findings"] == 338
    assert report["n_pinned"] == 338
    assert report["n_dangling"] == 0
    assert report["all_pinned"] is True
    assert report["dangling"] == []


def test_a_finding_outside_the_pinned_set_is_dangling_not_dropped():
    pins = pin_evidence({"src/a.lean": "a"})
    report = check_findings(
        [
            {"id": "F1", "file": "src/a.lean"},
            {"id": "F2", "file": "src/ghost.lean"},
        ],
        pins,
    )
    assert report["n_findings"] == 2
    assert report["n_dangling"] == 1
    assert report["dangling"] == ["src/ghost.lean"]
    assert report["all_pinned"] is False
    with pytest.raises(EvidenceError):
        check_findings([{"id": "F3"}], pins)


def test_verify_pins_flags_mutation_and_absence():
    pins = pin_evidence({"a": "one", "b": "two", "c": "three"})
    report = verify_pins(pins, {"a": "one", "b": "TWO"})
    assert report["n_pins"] == 3
    assert report["n_intact"] == 1
    assert report["n_mutated"] == 1
    assert report["mutated"][0]["path"] == "b"
    assert report["mutated"][0]["pinned_digest"] != report["mutated"][0]["current_digest"]
    assert report["missing"] == ["c"]
    assert report["all_intact"] is False

    clean = verify_pins(pins, {"a": "one", "b": "two", "c": "three"})
    assert clean["all_intact"] is True


def test_every_pin_report_says_a_hash_pins_bytes_only():
    """A pin must never be quietly upgraded into a correctness claim."""
    pins = pin_evidence({"a": "one"})
    assert "does not make the judgement correct" in PIN_SEMANTICS
    assert pins["a"].to_dict()["pin_semantics"] == PIN_SEMANTICS
    assert check_findings([Finding("F1", "a")], pins)["pin_semantics"] == PIN_SEMANTICS
    assert verify_pins(pins, {"a": "one"})["pin_semantics"] == PIN_SEMANTICS


# =========================================================================== #
# Worker dispatch for the new ops
# =========================================================================== #

def test_run_dispatches_the_new_ops():
    ids = _confident_ids(300)
    sample = run({"op": "select_audit_sample", "salt": _SALT, "items": ids})
    assert sample["salt"] == _SALT
    assert sample["item_ids"] == list(select_audit_sample(ids, _SALT).item_ids)

    truth = {i: "correct" for i in sample["item_ids"]}
    agreement = run(
        {
            "op": "audit_agreement",
            "salt": _SALT,
            "items": [{"id": i, "label": "correct"} for i in ids],
            "ground_truth": truth,
        }
    )
    assert agreement["agreement"]["rate"] == pytest.approx(1.0)
    assert agreement["agreement"]["sampled"] is True
    assert agreement["agreement"]["audit_salt"] == _SALT

    pinned = run({"op": "pin_evidence", "artifacts": {"src/a.lean": "a"}})
    assert pinned["n_pins"] == 1
    assert pinned["pins"]["src/a.lean"]["digest"] == content_digest("a")

    checked = run(
        {
            "op": "check_findings",
            "findings": [{"id": "F1", "path": "src/a.lean"}],
            "pins": pinned["pins"],
        }
    )
    assert checked["all_pinned"] is True

    verified = run(
        {"op": "verify_pins", "pins": pinned["pins"], "current": {"src/a.lean": "b"}}
    )
    assert verified["n_mutated"] == 1
