"""Tests for the Flyspeck modified-dual LP certificate exporter + checker.

Offline, deterministic, exact.  Confirms that a repaired-dual certificate
exports and checks ``valid=True`` (both a plain-infeasibility LP and a bound
LP), that every tampered variant is REJECTED (the soundness boundary), that the
document round-trips through JSON, and that export is deterministic.
"""
import copy
import json
import sys
from pathlib import Path

import pytest

# The verify tools live under one component root.
_ROOT = Path(__file__).resolve().parents[3]
for rel in ("components/verify/python",):
    sys.path.insert(0, str(_ROOT / rel))

from theoremata_tools.cert_flyspeck_lp import (  # noqa: E402
    FORMAT,
    KIND,
    check,
    export_flyspeck_lp_cert,
    repair_dual,
    run,
)


# --------------------------------------------------------------------------- #
# Fixtures: repaired-dual certificates (untrusted floating-point duals).
# --------------------------------------------------------------------------- #

def _infeasible_cert():
    """x >= 1 and x <= 0 is infeasible; true dual (1, 1); solver hands us floats."""
    rows = [
        {"a": {"x": -1}, "b": -1},   # -x <= -1   (i.e. x >= 1)
        {"a": {"x": 1}, "b": 0},     #  x <=  0
    ]
    dual = [1.0000000004, 0.9999999997]  # inexact -> repair snaps to (1, 1)
    return export_flyspeck_lp_cert(rows=rows, dual=dual)


def _bound_cert():
    """max x+y s.t. x<=3, y<=3, x+y<=4; dual (0,0,1) certifies bound 4."""
    rows = [
        {"a": {"x": 1}, "b": 3},
        {"a": {"y": 1}, "b": 3},
        {"a": {"x": 1, "y": 1}, "b": 4},
    ]
    dual = [0.0, 1e-12, 1.0000000002]  # -> repair snaps to (0, 0, 1)
    return export_flyspeck_lp_cert(rows=rows, dual=dual, objective={"x": 1, "y": 1})


# --------------------------------------------------------------------------- #
# Repair.
# --------------------------------------------------------------------------- #

def test_repair_recovers_exact_rationals_and_clamps_negatives():
    from fractions import Fraction
    y = repair_dual([0.3333333333333, 1.0000000001, -1e-9, "2/7"])
    assert y[0] == Fraction(1, 3)
    assert y[1] == Fraction(1)
    assert y[2] == Fraction(0)         # negative clamped to 0
    assert y[3] == Fraction(2, 7)


# --------------------------------------------------------------------------- #
# Happy path.
# --------------------------------------------------------------------------- #

def test_infeasible_cert_exports_and_checks():
    log = _infeasible_cert()
    assert log["format"] == FORMAT
    assert log["kind"] == KIND
    assert log["steps"][0]["mode"] == "infeasible"
    res = check(log)
    assert res["valid"] is True, res
    assert res["checked_steps"] == len(log["steps"])


def test_bound_cert_exports_and_checks():
    log = _bound_cert()
    assert log["steps"][0]["mode"] == "bound"
    # The certified bound is exactly 4.
    assert log["steps"][-1]["bound"] == "4"
    res = check(log)
    assert res["valid"] is True, res


def test_bound_weighted_sum_matches():
    # A different dual (1,1,0) also equals c on x/y and gives a looser bound 6.
    rows = [
        {"a": {"x": 1}, "b": 3},
        {"a": {"y": 1}, "b": 3},
        {"a": {"x": 1, "y": 1}, "b": 4},
    ]
    log = export_flyspeck_lp_cert(rows=rows, dual=[1.0, 1.0, 0.0],
                                  objective={"x": 1, "y": 1})
    assert log["steps"][-1]["bound"] == "6"
    assert check(log)["valid"] is True


# --------------------------------------------------------------------------- #
# Tampering -> REJECT (soundness boundary).
# --------------------------------------------------------------------------- #

def test_reject_negative_multiplier():
    log = _infeasible_cert()
    log["steps"][1]["y"][0] = "-1"
    res = check(log)
    assert res["valid"] is False
    assert "negative" in res["reason"].lower()


def test_reject_nonzero_structural_coeff():
    # Perturb one multiplier so Σ y.A no longer cancels the x column.
    log = _infeasible_cert()
    log["steps"][1]["y"][1] = "2"      # was 1 -> combination x-coeff becomes +1
    res = check(log)
    assert res["valid"] is False
    assert "combination is not exact" in res["reason"]


def test_reject_stray_coefficient_in_row():
    # Inject a stray structural coefficient directly into a row.
    log = _bound_cert()
    log["steps"][0]["rows"][2]["a"]["z"] = "5"
    res = check(log)
    assert res["valid"] is False
    assert "combination is not exact" in res["reason"]


def test_reject_wrong_bound():
    log = _infeasible_cert()
    log["steps"][-1]["bound"] = "-2"   # true weighted rhs is -1
    res = check(log)
    assert res["valid"] is False
    assert "bound mismatch" in res["reason"]


def test_reject_non_negative_infeasibility_bound():
    # A zero-combination system whose weighted rhs is >= 0 is NOT a contradiction.
    rows = [
        {"a": {"x": 1}, "b": 1},
        {"a": {"x": -1}, "b": 1},      # combination cancels, rhs sum = 2 >= 0
    ]
    log = export_flyspeck_lp_cert(rows=rows, dual=[1.0, 1.0])
    res = check(log)
    assert res["valid"] is False
    assert "contradiction" in res["reason"].lower() or "< 0" in res["reason"]


def test_reject_tampered_format_and_kind():
    log = _infeasible_cert()
    bad = copy.deepcopy(log)
    bad["format"] = "bogus.v0"
    assert check(bad)["valid"] is False
    bad2 = copy.deepcopy(log)
    bad2["kind"] = "lp_farkas"
    assert check(bad2)["valid"] is False


def test_reject_unknown_op():
    log = _infeasible_cert()
    log["steps"].insert(1, {"op": "evil_step"})
    res = check(log)
    assert res["valid"] is False
    assert "unknown op" in res["reason"]


# --------------------------------------------------------------------------- #
# JSON round-trip + determinism + worker.
# --------------------------------------------------------------------------- #

def test_json_roundtrip_preserves_validity():
    log = _bound_cert()
    reloaded = json.loads(json.dumps(log))
    assert reloaded == log
    assert check(reloaded)["valid"] is True


def test_export_is_deterministic():
    a = _bound_cert()
    b = _bound_cert()
    assert json.dumps(a, sort_keys=True) == json.dumps(b, sort_keys=True)


def test_worker_run_export_then_check():
    rows = [
        {"a": {"x": -1}, "b": -1},
        {"a": {"x": 1}, "b": 0},
    ]
    out = run({"op": "export", "rows": rows, "dual": [1.0000001, 0.9999999]})
    log = out["log"]
    assert log["kind"] == KIND
    res = run({"op": "check", "log": log})
    assert res["valid"] is True

    # Tamper post-export -> worker check rejects.
    log["steps"][-1]["bound"] = "0"
    assert run({"op": "check", "log": log})["valid"] is False
