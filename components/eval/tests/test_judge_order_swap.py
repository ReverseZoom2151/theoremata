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
# The cost switch
# --------------------------------------------------------------------------- #
def test_order_swap_is_off_by_default(monkeypatch):
    monkeypatch.delenv(judge_binding.ENV_ORDER_SWAP, raising=False)
    assert judge_binding.order_swap_enabled() is False

    seen = _install_generate(
        monkeypatch, lambda request: _bound(request, {"equivalent": True})
    )
    out = graders._default_llm_judge("1/2", "0.5")
    assert len(seen) == 1, "the default must not double anyone's eval bill"
    assert out["equivalent"] is True
    assert out["order_swapped"] is False
    assert out["order_stable"] is None


def test_env_switch_and_explicit_argument_control_the_second_pass(monkeypatch):
    monkeypatch.setenv(judge_binding.ENV_ORDER_SWAP, "1")
    assert judge_binding.order_swap_enabled() is True
    seen = _install_generate(
        monkeypatch, lambda request: _bound(request, {"equivalent": True})
    )
    graders._default_llm_judge("1/2", "0.5")
    assert len(seen) == 2

    # An explicit argument overrides the environment, in both directions.
    seen.clear()
    graders._default_llm_judge("1/2", "0.5", order_swap=False)
    assert len(seen) == 1

    monkeypatch.delenv(judge_binding.ENV_ORDER_SWAP, raising=False)
    seen.clear()
    graders._default_llm_judge("1/2", "0.5", order_swap=True)
    assert len(seen) == 2


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
    """A two-arg JudgeFn that opts the harness into order-swapped judging."""
    return graders._default_llm_judge(gold, pred, order_swap=True)


def test_judge_stability_report_surfaces_our_own_number(monkeypatch):
    item = {"expected": {"answer": "2*pi", "answer_kind": "symbolic"}}
    verdicts = []

    _install_generate(
        monkeypatch, _by_order({"equivalent": True}, {"equivalent": True})
    )
    verdicts.append(
        graders.grade_nl_answer(item, r"\boxed{\int_{)}}", judge=_swapping_judge)
    )

    _install_generate(
        monkeypatch, _by_order({"equivalent": True}, {"equivalent": False})
    )
    verdicts.append(
        graders.grade_nl_answer(item, r"\boxed{\int_{)}}", judge=_swapping_judge)
    )

    # A single-pass item must not be counted as stable.
    _install_generate(
        monkeypatch, lambda request: _bound(request, {"equivalent": False})
    )
    verdicts.append(graders.grade_nl_answer(item, r"\boxed{\int_{)}}"))

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
