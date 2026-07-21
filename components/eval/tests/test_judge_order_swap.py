"""Order-swapped two-pass judging for the comparative (pairwise) judge path.

The mined evidence these tests instrument: a large pairwise LLM-judge corpus had
an aggregate position bias of EXACTLY zero,

    order ab: a=212, b=280, tie=343
    order ba: a=201, b=282, tie=351

while only 831 of 1051 individual judgements, 79.1 percent, survived an order
swap. Balanced totals hid roughly one arbitrary decision in five. These tests
pin the instrument that would catch the same thing here: two passes with the
candidates swapped, a first-class UNSTABLE outcome that is neither a tie nor a
coin flip nor a fallback to pass one, and a measurable rate.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

_ROOT = Path(__file__).resolve().parents[3]
for _p in (
    _ROOT / "components" / "eval" / "python",
    _ROOT / "components" / "provider" / "python",
):
    if str(_p) not in sys.path:
        sys.path.insert(0, str(_p))

from theoremata_tools import judge_binding  # noqa: E402
from theoremata_tools.benchmarks import graders  # noqa: E402

BF = judge_binding.BINDING_FIELD


@pytest.fixture(autouse=True)
def _clean_judge_state():
    """A fresh cache and budget per test.

    The cache is process-wide by design (a rerun over the same corpus should be
    free), which means one test's decision would otherwise answer another test's
    question. Resetting is a test-isolation concern, not a production one.
    """
    judge_binding.CACHE.reset()
    judge_binding.BUDGET.reset()
    yield
    judge_binding.CACHE.reset()
    judge_binding.BUDGET.reset()


# --------------------------------------------------------------------------- #
# Fakes
# --------------------------------------------------------------------------- #
def _marker_of(request) -> str:
    task = request["task"]
    start = task.index(judge_binding.MARKER_PREFIX)
    return task[start:].split()[0].strip()


def _install_generate(monkeypatch, reply_for):
    """Patch the provider with a fake that sees each bound request."""
    seen: list[dict] = []
    from theoremata_tools import model_provider

    def fake_generate(request, *args, **kwargs):
        seen.append(request)
        return reply_for(request), "fake-model"

    monkeypatch.setattr(model_provider, "generate", fake_generate)
    return seen


def _bound(request, obj) -> dict:
    """An honest reply: the request's own marker followed by the verdict."""
    return {BF: _marker_of(request) + " " + json.dumps(obj)}


def _by_order(ab_verdict, ba_verdict, gold="2*pi"):
    """Reply per presentation order, so a position-sensitive judge is simulable."""

    def reply(request):
        ctx = request.get("context") or {}
        is_ab = ctx.get("answer_a") == gold
        return _bound(request, ab_verdict if is_ab else ba_verdict)

    return reply


# --------------------------------------------------------------------------- #
# The generic two-pass helper
# --------------------------------------------------------------------------- #
def test_agreeing_passes_return_the_agreed_verdict_marked_stable():
    report = judge_binding.two_pass_swapped(lambda order, marker: {"outcome": "a"})
    assert report["status"] == judge_binding.STABLE
    assert report["order_stable"] is True
    assert report["decided"] is True
    assert report["outcome"] == "a"
    assert report["unstable_reason"] is None


def test_disagreeing_passes_are_unstable_and_return_neither_verdict():
    seq = {"ab": "a", "ba": "b"}
    report = judge_binding.two_pass_swapped(
        lambda order, marker: {"outcome": seq[order]}
    )
    assert report["status"] == judge_binding.UNSTABLE
    assert report["order_stable"] is False
    assert report["decided"] is False
    assert report["unstable_reason"] == judge_binding.UNSTABLE_DISAGREED
    # NOT either pass's verdict, and NOT a tie: the outcome slot is empty.
    assert report["outcome"] is None
    assert report["outcome"] not in ("a", "b", "tie")


def test_tie_is_a_verdict_and_survives_only_when_both_passes_tie():
    both_tie = judge_binding.two_pass_swapped(lambda order, marker: {"outcome": "tie"})
    assert both_tie["status"] == judge_binding.STABLE
    assert both_tie["outcome"] == "tie"

    seq = {"ab": "tie", "ba": "a"}
    mixed = judge_binding.two_pass_swapped(
        lambda order, marker: {"outcome": seq[order]}
    )
    assert mixed["status"] == judge_binding.UNSTABLE
    assert mixed["outcome"] is None, "a tie must not absorb a disagreement"


@pytest.mark.parametrize(
    "second",
    [
        {"outcome": None, "error": "judge_unbound:marker_absent"},
        None,
        "not-a-dict",
    ],
)
def test_a_failed_or_unbound_second_pass_never_yields_stability(second):
    calls = {"n": 0}

    def pass_fn(order, marker):
        calls["n"] += 1
        return {"outcome": True} if calls["n"] == 1 else second

    report = judge_binding.two_pass_swapped(pass_fn)
    assert report["status"] == judge_binding.UNSTABLE
    assert report["order_stable"] is False
    assert report["unstable_reason"] == judge_binding.UNSTABLE_UNAVAILABLE
    # The surviving pass's True must not leak out as an agreed verdict.
    assert report["outcome"] is None
    assert report["decided"] is False


def test_a_raising_pass_is_unavailable_not_an_exception():
    def pass_fn(order, marker):
        if order == judge_binding.ORDER_BA:
            raise RuntimeError("provider exploded")
        return {"outcome": True}

    report = judge_binding.two_pass_swapped(pass_fn)
    assert report["status"] == judge_binding.UNSTABLE
    assert report["unstable_reason"] == judge_binding.UNSTABLE_UNAVAILABLE
    assert report["outcome"] is None


def test_each_pass_mints_its_own_marker():
    report = judge_binding.two_pass_swapped(lambda order, marker: {"outcome": marker})
    markers = report["markers"]
    assert len(markers) == 2
    assert markers[0] != markers[1]
    assert report["distinct_markers"] is True
    # Each pass saw exactly the marker minted for it, so neither can satisfy the
    # other's binding.
    assert [p["outcome"] for p in report["passes"]] == markers
    # Two distinct markers also means two distinct outcomes, hence unstable:
    # sharing one marker is the only way this shape could look "stable".
    assert report["status"] == judge_binding.UNSTABLE


# --------------------------------------------------------------------------- #
# The instability measurable
# --------------------------------------------------------------------------- #
def test_instability_rate_over_a_mixed_batch():
    stable = {"order_swapped": True, "status": judge_binding.STABLE}
    disagreed = {
        "order_swapped": True,
        "status": judge_binding.UNSTABLE,
        "unstable_reason": judge_binding.UNSTABLE_DISAGREED,
    }
    unavailable = {
        "order_swapped": True,
        "status": judge_binding.UNSTABLE,
        "unstable_reason": judge_binding.UNSTABLE_UNAVAILABLE,
    }
    single = {"order_swapped": False, "status": judge_binding.UNSTABLE}

    stats = judge_binding.instability_rate(
        [stable, stable, stable, disagreed, unavailable, single, single]
    )
    assert stats["measured"] is True
    assert stats["n_order_swapped"] == 5
    assert stats["n_single_pass_excluded"] == 2
    assert stats["n_stable"] == 3
    assert stats["n_unstable"] == 2
    assert stats["n_disagreed"] == 1
    assert stats["n_unavailable"] == 1
    assert stats["instability_rate"] == pytest.approx(0.4)
    assert stats["stability_rate"] == pytest.approx(0.6)


def test_instability_rate_reports_nothing_measured_rather_than_zero():
    stats = judge_binding.instability_rate([{"order_swapped": False}])
    assert stats["measured"] is False
    assert stats["instability_rate"] is None
    assert stats["stability_rate"] is None
    assert stats["n_single_pass_excluded"] == 1


# --------------------------------------------------------------------------- #
# What replaced the cost switch: a default-ON instrument, a content cache and a
# bounded escape
# --------------------------------------------------------------------------- #
def test_order_swap_is_on_by_default(monkeypatch):
    """The measurement runs unless someone deliberately opts out of it."""
    swap, reason = judge_binding.decide_order_swap()
    assert swap is True
    assert reason is None

    seen = _install_generate(
        monkeypatch, lambda request: _bound(request, {"equivalent": True})
    )
    out = graders._default_llm_judge("1/2", "0.5")
    assert len(seen) == 2, "an instrument that never runs measures nothing"
    assert out["equivalent"] is True
    assert out["order_stable"] is True


def test_the_boolean_env_switch_is_gone():
    """The old opt-in switch must not exist to be set back to off."""
    assert not hasattr(judge_binding, "ENV_ORDER_SWAP")
    assert not hasattr(judge_binding, "order_swap_enabled")


def test_an_explicit_opt_out_keeps_its_old_single_pass_meaning(monkeypatch):
    seen = _install_generate(
        monkeypatch, lambda request: _bound(request, {"equivalent": True})
    )
    out = graders._default_llm_judge("1/2", "0.5", order_swap=False)
    assert len(seen) == 1
    assert out["equivalent"] is True
    assert out["reason"] == "llm_judge:fake-model"
    # ... and it claims no stability, so it stays out of the denominator.
    assert out["order_swapped"] is False
    assert out["order_stable"] is None
    assert out["stability"]["sampling_reason"] == judge_binding.SAMPLED_CALLER_OPTED_OUT
    assert judge_binding.instability_rate([out["stability"]])["measured"] is False


def test_the_budget_bounds_a_huge_batch_and_says_so(monkeypatch):
    judge_binding.BUDGET.reset(limit=2)
    seen = _install_generate(
        monkeypatch, lambda request: _bound(request, {"equivalent": True})
    )
    stabilities = []
    for i in range(4):
        out = graders._default_llm_judge(f"gold{i}", f"pred{i}")
        stabilities.append(out["stability"])

    swapped = [s for s in stabilities if s.get("order_swapped")]
    sampled = [s for s in stabilities if not s.get("order_swapped")]
    assert len(swapped) == 2 and len(sampled) == 2
    assert len(seen) == 2 * 2 + 2, "past the cap an item costs one pass, not two"
    for s in sampled:
        assert s["sampled"] is True
        assert s["sampling_reason"] == judge_binding.SAMPLED_BUDGET_EXHAUSTED

    stats = judge_binding.instability_rate(stabilities)
    # The rate is real, but it is over 2 of 4 judged items and must say so.
    assert stats["sampled"] is True
    assert stats["n_sampled_out"] == 2
    assert stats["denominator"] == 2
    assert stats["n_judged_items"] == 4
    assert "SAMPLED" in stats["note"]
    assert "denominator of 2" in stats["note"]


def test_an_unsampled_batch_does_not_cry_sampling():
    stats = judge_binding.instability_rate(
        [{"order_swapped": True, "status": judge_binding.STABLE}]
    )
    assert stats["sampled"] is False
    assert stats["n_sampled_out"] == 0
    assert "SAMPLED" not in stats["note"]


def test_an_unreadable_budget_does_not_disable_the_measurement(monkeypatch):
    monkeypatch.setenv(judge_binding.ENV_SWAP_BUDGET, "not-a-number")
    judge_binding.BUDGET.reset()
    assert judge_binding.BUDGET.limit() == judge_binding.DEFAULT_SWAP_BUDGET
    assert judge_binding.decide_order_swap()[0] is True


def test_zero_budget_means_unlimited_not_off(monkeypatch):
    monkeypatch.setenv(judge_binding.ENV_SWAP_BUDGET, "0")
    judge_binding.BUDGET.reset()
    assert judge_binding.BUDGET.snapshot()["unlimited"] is True
    assert all(judge_binding.decide_order_swap()[0] for _ in range(50))


# --------------------------------------------------------------------------- #
# The content-keyed cache
# --------------------------------------------------------------------------- #
def test_cache_key_is_content_never_the_marker():
    a = graders._equivalence_cache_key("2*pi", "6.283")
    b = graders._equivalence_cache_key("2*pi", "6.283")
    assert a == b, "the same content must key the same way on every call"
    # The judged relation is symmetric, so the swapped pair is the same question.
    assert a == graders._equivalence_cache_key("6.283", "2*pi")
    assert a != graders._equivalence_cache_key("2*pi", "6.284")
    # No marker, and nothing marker-shaped, can be inside the key.
    for _ in range(4):
        assert judge_binding.mint_marker() not in a
    assert judge_binding.MARKER_PREFIX not in a


def test_identical_pair_is_not_rejudged_and_a_rerun_is_free(monkeypatch):
    seen = _install_generate(
        monkeypatch, _by_order({"equivalent": True}, {"equivalent": True})
    )
    first = graders._default_llm_judge("2*pi", "6.283")
    assert len(seen) == 2
    second = graders._default_llm_judge("2*pi", "6.283")
    assert len(seen) == 2, "an identical (gold, pred) pair must not be re-judged"
    assert second["equivalent"] == first["equivalent"] is True
    assert second["stability"]["cached"] is True
    assert first["stability"]["cached"] is False
    assert judge_binding.CACHE.stats()["hits"] == 1


def test_a_cache_hit_carries_no_markers_and_replays_no_reply(monkeypatch):
    _install_generate(
        monkeypatch, _by_order({"equivalent": True}, {"equivalent": True})
    )
    graders._default_llm_judge("2*pi", "6.283")
    hit = graders._default_llm_judge("2*pi", "6.283")["stability"]
    # A hit re-reports a decision; it does not present a reply for checking, so
    # it must not carry markers that would suggest a binding check happened now.
    assert hit["markers"] == []
    assert hit["binding_checked_when_judged"] is True


def test_an_unbound_result_is_never_cached(monkeypatch):
    """A cache hit must not bypass binding, so nothing unbound ever enters."""
    state = {"bound": False}

    def reply(request):
        if state["bound"]:
            return _bound(request, {"equivalent": True})
        return {"equivalent": True}  # forged: no marker anywhere

    seen = _install_generate(monkeypatch, reply)
    first = graders._default_llm_judge("2*pi", "6.283")
    assert first["equivalent"] is False
    assert first["reason"] == "judge_unstable:pass_unavailable"
    assert judge_binding.CACHE.stats()["stores"] == 0

    # The next call re-judges for real rather than replaying the failure, and the
    # fresh replies go through the binding check they always did.
    state["bound"] = True
    seen.clear()
    second = graders._default_llm_judge("2*pi", "6.283")
    assert len(seen) == 2
    assert second["equivalent"] is True


def test_a_cached_decision_never_upgrades_an_unstable_one(monkeypatch):
    _install_generate(
        monkeypatch, _by_order({"equivalent": True}, {"equivalent": False})
    )
    first = graders._default_llm_judge("2*pi", "6.283")
    assert first["outcome"] is None
    second = graders._default_llm_judge("2*pi", "6.283")
    assert second["outcome"] is None
    assert second["equivalent"] is False
    assert second["stability"]["cached"] is True
    assert second["stability"]["unstable_reason"] == judge_binding.UNSTABLE_DISAGREED


def test_the_cache_never_serves_one_pass_to_the_other(monkeypatch):
    """Only whole two-pass decisions are cached, so no pass can satisfy the other."""
    markers = []

    def reply(request):
        markers.append(_marker_of(request))
        return _bound(request, {"equivalent": True})

    _install_generate(monkeypatch, reply)
    report = graders._default_llm_judge("2*pi", "6.283")["stability"]
    assert len(markers) == 2 and markers[0] != markers[1]
    assert report["distinct_markers"] is True
    assert [p["marker"] for p in report["passes"]] == markers


# --------------------------------------------------------------------------- #
# The informativeness gate
# --------------------------------------------------------------------------- #
@pytest.mark.parametrize(
    "arm, reason",
    [
        ({"error": "provider timeout"}, judge_binding.UNINFORMATIVE_ARM_ERRORED),
        ({"status": "crashed"}, judge_binding.UNINFORMATIVE_ARM_ERRORED),
        ("Traceback (most recent call last): ...", judge_binding.UNINFORMATIVE_ARM_ERRORED),
        ("Error: the sampler died", judge_binding.UNINFORMATIVE_ARM_ERRORED),
        ("", judge_binding.UNINFORMATIVE_ARM_MISSING),
        (None, judge_binding.UNINFORMATIVE_ARM_MISSING),
    ],
)
def test_an_errored_or_missing_arm_is_refused_not_judged(arm, reason):
    screen = judge_binding.screen_comparison("2*pi", arm)
    assert screen["informative"] is False
    assert screen["ungraded"] is True
    assert screen["ungraded_reason"] == reason


def test_a_forced_tie_is_not_a_tie_the_judge_reached():
    screen = judge_binding.screen_comparison("a", "b", forced_tie=True)
    assert screen["informative"] is False
    assert screen["ungraded_reason"] == judge_binding.UNINFORMATIVE_FORCED_TIE


def test_the_gate_stops_the_judge_from_ever_being_called(monkeypatch):
    seen = _install_generate(
        monkeypatch, lambda request: _bound(request, {"equivalent": True})
    )
    item = {"expected": {"answer": "2*pi", "answer_kind": "symbolic"}}
    verdict = graders.grade_nl_answer(item, "Error: the sampler died")
    assert seen == [], "an arm that crashed must not be sent to a judge"
    assert verdict["is_correct"] is False
    block = verdict["detail"]["judge_informativeness"]
    assert block["informative"] is False
    assert block["ungraded_reason"] == judge_binding.UNINFORMATIVE_ARM_ERRORED
    # ... and it left no stability observation to pollute the instability rate.
    assert "judge_order_swap" not in verdict["detail"]


def test_uninformative_comparisons_are_excluded_with_their_own_count():
    stats = judge_binding.comparison_rates(
        [
            {"informative": True, "outcome": True},
            {"informative": True, "outcome": True},
            {"informative": True, "outcome": False},
            {"informative": False, "ungraded_reason": judge_binding.UNINFORMATIVE_ARM_ERRORED},
            {"informative": False, "ungraded_reason": judge_binding.UNINFORMATIVE_FORCED_TIE},
        ]
    )
    assert stats["measured"] is True
    assert stats["n_comparisons"] == 5
    assert stats["n_informative"] == 3
    assert stats["n_uninformative_excluded"] == 2
    assert stats["denominator"] == 3
    assert stats["uninformative_reasons"] == {
        judge_binding.UNINFORMATIVE_ARM_ERRORED: 1,
        judge_binding.UNINFORMATIVE_FORCED_TIE: 1,
    }
    # The rate is over the 3 informative comparisons, never over all 5.
    assert stats["rates"]["true"] == pytest.approx(2 / 3)


def test_an_all_uninformative_batch_reports_nothing_measured():
    stats = judge_binding.comparison_rates(
        [{"informative": False, "ungraded_reason": judge_binding.UNINFORMATIVE_ARM_ERRORED}]
    )
    assert stats["measured"] is False
    assert stats["rates"] is None
    assert stats["denominator"] is None


def test_an_unparseable_judge_reply_is_a_parse_failure_not_an_opinion(monkeypatch):
    _install_generate(monkeypatch, lambda request: {"equivalent": True})
    item = {"expected": {"answer": "2*pi", "answer_kind": "symbolic"}}
    verdict = graders.grade_nl_answer(item, r"\boxed{\int_{)}}")
    block = verdict["detail"]["judge_informativeness"]
    assert block["informative"] is False
    assert block["ungraded_reason"] == judge_binding.UNINFORMATIVE_JUDGE_PARSE_FAILURE
    # The ITEM still fails closed; only the JUDGEMENT is excluded.
    assert verdict["is_correct"] is False

    report = graders.judge_informativeness_report([verdict])
    assert report["n_uninformative_excluded"] == 1
    assert report["measured"] is False


# --------------------------------------------------------------------------- #
# The comparative answer-equivalence judge, end to end
# --------------------------------------------------------------------------- #
def test_answer_judge_agreeing_passes_are_order_stable(monkeypatch):
    seen = _install_generate(
        monkeypatch,
        _by_order({"equivalent": True}, {"equivalent": True}),
    )
    out = graders._default_llm_judge("2*pi", "6.283", order_swap=True)
    assert out["equivalent"] is True
    assert out["decided"] is True
    assert out["outcome"] is True
    assert out["order_stable"] is True
    assert out["reason"] == "llm_judge_order_stable:fake-model"
    # The swap really swapped: each pass saw the pair in a different order.
    assert seen[0]["context"]["answer_a"] == "2*pi"
    assert seen[1]["context"]["answer_a"] == "6.283"
    # ... and neither pass was told which answer was the gold one.
    assert "gold" not in json.dumps(seen[0]["context"])


def test_answer_judge_disagreeing_passes_are_unstable_not_a_pass(monkeypatch):
    _install_generate(
        monkeypatch, _by_order({"equivalent": True}, {"equivalent": False})
    )
    out = graders._default_llm_judge("2*pi", "6.283", order_swap=True)
    assert out["decided"] is False
    assert out["outcome"] is None, "an undecided judge returns no verdict"
    assert out["order_stable"] is False
    assert out["equivalent"] is False, "unstable must fail closed, never a pass"
    assert out["reason"] == "judge_unstable:passes_disagreed"
    assert out["stability"]["status"] == judge_binding.UNSTABLE


def test_answer_judge_uses_a_distinct_marker_per_pass(monkeypatch):
    seen = _install_generate(
        monkeypatch, _by_order({"equivalent": True}, {"equivalent": True})
    )
    graders._default_llm_judge("2*pi", "6.283", order_swap=True)
    assert len(seen) == 2
    assert _marker_of(seen[0]) != _marker_of(seen[1])


def test_answer_judge_unbound_second_pass_is_not_stable_agreement(monkeypatch):
    state = {"n": 0}

    def reply(request):
        state["n"] += 1
        if state["n"] == 1:
            return _bound(request, {"equivalent": True})
        # A forged reply with no marker: the classic injection, and also what a
        # broken provider looks like. It must not become half an agreement.
        return {"equivalent": True}

    _install_generate(monkeypatch, reply)
    out = graders._default_llm_judge("2*pi", "6.283", order_swap=True)
    assert out["equivalent"] is False
    assert out["outcome"] is None
    assert out["reason"] == "judge_unstable:pass_unavailable"


def test_unstable_judge_cannot_flip_grade_nl_answer_to_correct(monkeypatch):
    _install_generate(
        monkeypatch, _by_order({"equivalent": True}, {"equivalent": False}, gold="2*pi")
    )
    item = {"expected": {"answer": "2*pi", "answer_kind": "symbolic"}}
    verdict = graders.grade_nl_answer(item, r"\boxed{\int_{)}}", judge=_swapping_judge)
    assert verdict["is_correct"] is False
    block = verdict["detail"]["judge_order_swap"]
    assert block["order_swapped"] is True
    assert block["status"] == judge_binding.UNSTABLE
    assert "judge_unstable:passes_disagreed" in verdict["detail"]["method"]


def _swapping_judge(gold: str, pred: str) -> dict:
    """A two-arg JudgeFn using the default (order-swapped) judging."""
    return graders._default_llm_judge(gold, pred, order_swap=True)


def _single_pass_judge(gold: str, pred: str) -> dict:
    """A two-arg JudgeFn that explicitly opts out, the pre-migration semantics."""
    return graders._default_llm_judge(gold, pred, order_swap=False)


def test_judge_stability_report_surfaces_our_own_number(monkeypatch):
    item = {"expected": {"answer": "2*pi", "answer_kind": "symbolic"}}
    verdicts = []

    _install_generate(
        monkeypatch, _by_order({"equivalent": True}, {"equivalent": True})
    )
    verdicts.append(
        graders.grade_nl_answer(item, r"\boxed{\int_{)}}", judge=_swapping_judge)
    )

    # A fresh provider answers a fresh question, so the cache must not answer it.
    judge_binding.CACHE.reset()
    _install_generate(
        monkeypatch, _by_order({"equivalent": True}, {"equivalent": False})
    )
    verdicts.append(
        graders.grade_nl_answer(item, r"\boxed{\int_{)}}", judge=_swapping_judge)
    )

    # A single-pass item must not be counted as stable.
    judge_binding.CACHE.reset()
    _install_generate(
        monkeypatch, lambda request: _bound(request, {"equivalent": False})
    )
    verdicts.append(
        graders.grade_nl_answer(item, r"\boxed{\int_{)}}", judge=_single_pass_judge)
    )

    stats = graders.judge_stability_report(verdicts)
    assert stats["n_order_swapped"] == 2
    assert stats["n_single_pass_excluded"] == 1
    assert stats["n_stable"] == 1
    assert stats["n_disagreed"] == 1
    assert stats["instability_rate"] == pytest.approx(0.5)


# --------------------------------------------------------------------------- #
# Mock-mode determinism (no network, ever)
# --------------------------------------------------------------------------- #
def test_mock_mode_order_swap_is_deterministic_and_never_passes(monkeypatch):
    monkeypatch.setenv("THEOREMATA_MODEL_MOCK", "1")
    first = graders._default_llm_judge("1/2", "0.5", order_swap=True)
    second = graders._default_llm_judge("1/2", "0.5", order_swap=True)
    # The canned mock reply carries no marker, so both passes fail closed.
    assert first["equivalent"] is False and second["equivalent"] is False
    assert first["reason"] == second["reason"] == "judge_unstable:pass_unavailable"
    assert first["outcome"] is None and second["outcome"] is None
