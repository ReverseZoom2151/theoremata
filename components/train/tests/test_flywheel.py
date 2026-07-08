import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.flywheel import (  # noqa: E402
    auto_label,
    coerce_verification,
    dry_run,
    formal_oracle,
    label_dataset,
    majority_confirm,
    run,
    to_grpo_rows,
    to_sft_rows,
)


# --- majority_confirm ------------------------------------------------------

def test_majority_confirm():
    assert majority_confirm([True, True, False]) is True
    assert majority_confirm([True, False, False]) is False
    assert majority_confirm([]) is False  # empty is not confirmed
    assert majority_confirm([{"valid": True}, {"valid": True}, {"valid": False}]) is True


def test_coerce_verification_snaps_score():
    assert coerce_verification(0.9).score == 1.0
    assert coerce_verification(0.4).score == 0.5
    assert coerce_verification({"score": 0.1}).score == 0.0


# --- auto_label: NL path with mock oracles ---------------------------------

def test_auto_label_all_perfect_labels_one():
    # every verification scores 1.0 -> no issues -> label 1.0
    oracle = lambda p, pr: 1.0  # noqa: E731
    res = auto_label("prob", "proof", oracle, n=4)
    assert res["label"] == 1.0
    assert res["hard"] is False


def test_auto_label_confirmed_low_score_wins():
    # a verifier reports a fatal issue (0.0); meta majority confirms it
    verify = lambda p, pr: {"score": 0.0, "analysis": "gap", "reports_issue": True}  # noqa: E731
    meta = lambda p, pr, v: True  # noqa: E731
    res = auto_label("prob", "proof", verify, meta_oracle=meta, n=3, m=3, k=1)
    assert res["label"] == 0.0
    assert res["confirmed"] is True
    assert res["provenance"]["valid_analyses"] == 3


def test_auto_label_unconfirmed_issue_defaults_to_one():
    # verifier claims an issue but meta majority REJECTS the analysis -> label 1.0
    verify = lambda p, pr: {"score": 0.0, "reports_issue": True}  # noqa: E731
    meta = lambda p, pr, v: False  # noqa: E731
    res = auto_label("prob", "proof", verify, meta_oracle=meta, n=3, m=3, k=1)
    assert res["label"] == 1.0
    assert res["confirmed"] is False
    assert res["provenance"]["valid_analyses"] == 0


def test_auto_label_k_threshold_not_met():
    # only ONE valid analysis but k=2 required -> fall back to label 1.0
    calls = {"i": 0}

    def verify(p, pr):
        calls["i"] += 1
        # first call reports issue, rest are clean
        return {"score": 0.5, "reports_issue": True} if calls["i"] == 1 else 1.0

    res = auto_label("prob", "proof", verify, meta_oracle=lambda *a: True, n=3, m=1, k=2)
    assert res["label"] == 1.0
    assert res["confirmed"] is False


def test_auto_label_lowest_score_among_valid():
    scores = iter([0.5, 0.0, 1.0])

    def verify(p, pr):
        try:
            return {"score": next(scores), "reports_issue": True}
        except StopIteration:
            return 1.0

    res = auto_label("prob", "proof", verify, meta_oracle=lambda *a: True, n=3, m=1, k=1)
    assert res["label"] == 0.0  # lowest confirmed score


# --- formal oracle: HARD labels -------------------------------------------

def test_formal_oracle_hard_label_pass():
    verdict = lambda p, pr: {"compiled": True, "axioms_ok": True}  # noqa: E731
    oracle = formal_oracle(verdict)
    res = auto_label("prob", "proof", oracle, formal=True)
    assert res["label"] == 1.0
    assert res["hard"] is True
    assert res["provenance"]["oracle"] == "formal"


def test_formal_oracle_hard_label_fail():
    verdict = lambda p, pr: {"compiled": False, "axioms_ok": True}  # noqa: E731
    res = auto_label("prob", "proof", formal_oracle(verdict), formal=True)
    assert res["label"] == 0.0
    assert res["hard"] is True


# --- dataset labeling + conversions ---------------------------------------

def test_label_dataset_and_conversions():
    items = [
        {"problem": "p1", "proof": "good"},
        {"problem": "p2", "proof": "bad"},
    ]

    def verify(p, pr):
        return 1.0 if pr == "good" else {"score": 0.0, "reports_issue": True}

    out = label_dataset(items, verify, meta_oracle=lambda *a: True, n=2, m=1, k=1)
    assert out["labeled"] == 2
    labels = {r["problem"]: r["label"] for r in out["rows"]}
    assert labels == {"p1": 1.0, "p2": 0.0}

    sft = to_sft_rows(out["rows"])
    assert len(sft) == 1  # only the label-1.0 proof
    assert sft[0]["messages"][1]["content"] == "good"

    grpo = to_grpo_rows(out["rows"])
    assert len(grpo) == 2
    p2 = next(r for r in grpo if r["gold"] == "p2")
    assert p2["verdict"] == {"compiled": False, "axioms_ok": True}
    assert p2["gold_score"] == 0.0


def test_dry_run_offline():
    items = [{"problem": "p", "proof": "x"}]
    out = dry_run(items, lambda p, pr: 1.0, n=2)
    assert out["ok"] is True and out["dry_run"] is True
    assert out["labeled"] == 1
    assert out["sft_rows"] == 1


def test_run_replay_oracle():
    req = {
        "op": "label",
        "n": 3,
        "m": 1,
        "k": 1,
        "items": [
            {"problem": "p1", "proof": "a", "verifications": [1.0, 1.0, 1.0]},
            {"problem": "p2", "proof": "b", "verifications": [{"score": 0.0, "reports_issue": True}]},
        ],
    }
    out = run(req)
    labels = {r["problem"]: r["label"] for r in out["rows"]}
    assert labels["p1"] == 1.0
    assert labels["p2"] == 0.0  # no meta gate in replay -> trusted
