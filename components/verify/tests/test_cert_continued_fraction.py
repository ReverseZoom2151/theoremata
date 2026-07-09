"""Tests for the continued-fraction / Diophantine hardness cert-log module.

Offline, deterministic, exact.  Exports certificates for known continued
fractions, confirms the self-contained reference checker (a) validates a
genuine cert and (b) REJECTS every tampered one or overstated hardness bound
(the soundness boundary).  Pure standard library; sympy is optional.
"""
import copy
import json
import sys
from fractions import Fraction
from pathlib import Path

import pytest

# The verify tools live under components/verify/python.
_ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(_ROOT / "components" / "verify" / "python"))

from theoremata_tools.cert_continued_fraction import (  # noqa: E402
    FORMAT,
    check,
    continued_fraction,
    convergents,
    export_continued_fraction_cert,
    run,
)


def _roundtrip(log):
    """JSON dump/load a log (proves it is plain, transport-neutral JSON)."""
    return json.loads(json.dumps(log))


# --------------------------------------------------------------------------- #
# Hand-verified continued-fraction facts.
# --------------------------------------------------------------------------- #

def test_continued_fraction_of_355_113():
    # 355/113 = [3; 7, 16] (the famous Milu / near-pi approximation).
    a = continued_fraction(Fraction(355, 113))
    assert a == [3, 7, 16]
    ps, qs = convergents(a)
    # Convergents: 3/1, 22/7, 355/113.
    assert list(zip(ps, qs)) == [(3, 1), (22, 7), (355, 113)]


def test_continued_fraction_of_plain_rational():
    # 43/19 = [2; 3, 1, 4]  (2 + 1/(3 + 1/(1 + 1/4))).
    a = continued_fraction(Fraction(43, 19))
    ps, qs = convergents(a)
    assert Fraction(ps[-1], qs[-1]) == Fraction(43, 19)


# --------------------------------------------------------------------------- #
# Export + validate.
# --------------------------------------------------------------------------- #

def test_export_and_check_355_113():
    log = export_continued_fraction_cert(Fraction(355, 113))
    assert log["format"] == FORMAT
    assert log["kind"] == "continued_fraction"
    res = check(log)
    assert res["valid"] is True, res
    assert res["checked_steps"] == len(log["steps"])


def test_export_accepts_string_target():
    log = export_continued_fraction_cert("355/113")
    assert check(log)["valid"] is True


def test_export_and_check_plain_rational():
    log = export_continued_fraction_cert(Fraction(43, 19))
    assert check(log)["valid"] is True, check(log)


def test_export_with_explicit_partial_quotients():
    log = export_continued_fraction_cert(Fraction(355, 113),
                                         partial_quotients=[3, 7, 16])
    assert check(log)["valid"] is True


def test_roundtrips_through_json():
    log = export_continued_fraction_cert(Fraction(355, 113))
    assert check(_roundtrip(log))["valid"] is True


def test_carried_hardness_bound_matches_classical_estimate():
    # For 355/113 the k=1 convergent is 22/7 (q1=7, q2=113): the certified
    # lower bound is 1/(q1*(q1+q2)) = 1/(7*120) = 1/840, and the true error
    # |355/113 - 22/7| = 1/791 which is indeed >= 1/840.
    log = export_continued_fraction_cert(Fraction(355, 113))
    bound_steps = [s for s in log["steps"] if s["op"] == "assert_hardness_bound"]
    k1 = [s for s in bound_steps if s["k"] == 1][0]
    assert Fraction(k1["bound"]) == Fraction(1, 840)
    assert abs(Fraction(355, 113) - Fraction(22, 7)) == Fraction(1, 791)
    assert Fraction(1, 791) >= Fraction(1, 840)


# --------------------------------------------------------------------------- #
# Tamper rejection (soundness boundary).
# --------------------------------------------------------------------------- #

def test_tampered_partial_quotient_rejected():
    log = export_continued_fraction_cert(Fraction(355, 113))
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "partial_quotients":
            step["a"][1] = "8"  # 7 -> 8: convergents no longer reproduce target
    res = check(bad)
    assert res["valid"] is False
    assert "convergent" in res["reason"] or "target" in res["reason"]


def test_tampered_convergent_rejected():
    log = export_continued_fraction_cert(Fraction(355, 113))
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "convergents":
            step["p"][1] = "23"  # 22 -> 23: mismatch vs recomputed convergents
    res = check(bad)
    assert res["valid"] is False
    assert "match" in res["reason"] or "convergent" in res["reason"]


def test_overstated_hardness_bound_rejected():
    log = export_continued_fraction_cert(Fraction(355, 113))
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "assert_hardness_bound" and step["k"] == 1:
            step["bound"] = "1/2"  # true error is 1/791; 1/2 is wildly overstated
    res = check(bad)
    assert res["valid"] is False
    assert "overstated" in res["reason"] or "hardness" in res["reason"]


def test_tampered_target_rejected():
    log = export_continued_fraction_cert(Fraction(355, 113))
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "cf_target":
            step["x"] = "3"  # last convergent 355/113 != 3
    res = check(bad)
    assert res["valid"] is False


def test_unknown_format_rejected():
    res = check({"format": "bogus.v9", "kind": "continued_fraction", "steps": []})
    assert res["valid"] is False


def test_no_conclusion_rejected():
    log = export_continued_fraction_cert(Fraction(355, 113))
    bad = copy.deepcopy(log)
    bad["steps"] = [s for s in bad["steps"]
                    if s["op"] not in ("assert_hardness_bound",
                                       "assert_best_approximation")]
    assert check(bad)["valid"] is False


# --------------------------------------------------------------------------- #
# Determinism + run() dispatch.
# --------------------------------------------------------------------------- #

def test_determinism_export_and_check_are_stable():
    log1 = export_continued_fraction_cert(Fraction(355, 113))
    log2 = export_continued_fraction_cert(Fraction(355, 113))
    assert json.dumps(log1, sort_keys=True) == json.dumps(log2, sort_keys=True)
    r1 = check(log1)
    r2 = check(_roundtrip(log1))
    assert r1["valid"] == r2["valid"] is True
    assert r1["checked_steps"] == r2["checked_steps"]


def test_run_export_then_check_roundtrip():
    exported = run({"op": "export", "target": "355/113"})
    assert "log" in exported
    checked = run({"op": "check", "log": exported["log"]})
    assert checked["valid"] is True


def test_run_check_rejects_tampered():
    log = export_continued_fraction_cert(Fraction(43, 19))
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "convergents":
            step["q"][0] = "99"
    assert run({"op": "check", "log": bad})["valid"] is False


def test_run_unknown_op_raises():
    with pytest.raises(ValueError):
        run({"op": "nope"})


# --------------------------------------------------------------------------- #
# Optional sympy cross-check (skipped if sympy absent).
# --------------------------------------------------------------------------- #

def test_matches_sympy_continued_fraction():
    sympy = pytest.importorskip("sympy")
    from sympy import Rational
    from sympy.ntheory.continued_fraction import continued_fraction as sym_cf
    target = Fraction(415, 93)
    assert continued_fraction(target) == list(sym_cf(Rational(415, 93)))
    assert check(export_continued_fraction_cert(target))["valid"] is True
