"""Tests for the Positivstellensatz infeasibility cert exporter + pure checker.

Offline, deterministic, exact.  Exercises: a linear Farkas infeasible system
expressed as a Positivstellensatz refutation, an ideal (equality) refutation
with a nontrivial cofactor, a genuinely nonlinear refutation
({-1-x^2 >= 0} is infeasible), tamper rejection (cofactor / non-SOS Gram
multiplier / non-zero sum / non-positive strict const), determinism and
round-trip.
"""
import copy
import json
import sys
from pathlib import Path

import pytest

pytest.importorskip("sympy")
import sympy  # noqa: E402
from sympy import symbols  # noqa: E402

_ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(_ROOT / "components/verify/python"))

from theoremata_tools.cert_positivstellensatz import (  # noqa: E402
    FORMAT,
    KIND,
    check,
    export_positivstellensatz_cert,
    generate,
    run,
)


def _roundtrip(log):
    return json.loads(json.dumps(log))


# --------------------------------------------------------------------------- #
# Valid refutations.
# --------------------------------------------------------------------------- #

def test_linear_farkas_as_positivstellensatz():
    # { x - 1 >= 0, -x >= 0 } is infeasible (x >= 1 and x <= 0).
    # Refutation: 1*(x-1) + 1*(-x) + 1 == 0, with sigma = 1 (SOS) on each q_j.
    x = symbols("x")
    log = export_positivstellensatz_cert(
        gens=[x],
        nonstrict=[x - 1, -x],
        cone=[{"cone_indices": [0], "sos": {"squares": [1]}},
              {"cone_indices": [1], "sos": {"squares": [1]}}],
        strict_part={"const": 1})
    assert log["format"] == FORMAT and log["kind"] == KIND
    res = check(log)
    assert res["valid"] is True, res
    assert check(_roundtrip(log))["valid"] is True


def test_ideal_equality_refutation_with_cofactor():
    # { x - 1 = 0, -x >= 0 } infeasible: x = 1 but -x >= 0 => -1 >= 0.
    # P = 1*(x-1), Q = 1*(-x), R = 1 : (x-1) + (-x) + 1 == 0.
    x = symbols("x")
    log = export_positivstellensatz_cert(
        gens=[x],
        equalities=[x - 1],
        nonstrict=[-x],
        ideal=[{"equality_index": 0, "cofactor": 1}],
        cone=[{"cone_indices": [0], "sos": {"squares": [1]}}],
        strict_part={"const": 1})
    assert check(log)["valid"] is True


def test_nonlinear_refutation_negative_square():
    # { -1 - x^2 >= 0 } is infeasible (would need x^2 <= -1).
    # 1*(-1 - x^2) + (x)^2 * 1 + 1 == 0 : cone term with q_0 and a bare SOS term.
    x = symbols("x")
    log = export_positivstellensatz_cert(
        gens=[x],
        nonstrict=[-1 - x**2],
        cone=[{"cone_indices": [0], "sos": {"squares": [1]}},
              {"cone_indices": [], "sos": {"squares": [x]}}],
        strict_part={"const": 1})
    res = check(log)
    assert res["valid"] is True, res


def test_valid_gram_sos_multiplier():
    # Same nonlinear system, but the bare SOS x^2 is supplied as a PSD Gram
    # (z = [x], Q = [[1]]) to exercise the reused cert_sos PSD test on accept.
    x = symbols("x")
    log = export_positivstellensatz_cert(
        gens=[x],
        nonstrict=[-1 - x**2],
        cone=[{"cone_indices": [0], "sos": {"squares": [1]}},
              {"cone_indices": [], "sos": {"monomials": [[1]], "Q": [["1"]]}}],
        strict_part={"const": 1})
    assert check(log)["valid"] is True


def test_strict_generator_in_strict_part():
    # { -x^2 >= 0, x > 0 } infeasible: forces x = 0 yet x > 0.
    # 1*(-x^2) + 0 ... use R = 1*r_0 (=x) times const? Need P+Q+R==0.
    # Take Q = 1*(-x^2) via cone on q_0, plus bare SOS (x)^2, R = 0*... no:
    # (-x^2) + x^2 + <strict>. We need the sum zero, so strict part must be 0,
    # which is not allowed. Instead certify { -x^2 >= 0, x - 1 > 0 }:
    # x <= 0 and x > 1, infeasible. Use R = 1*(x-1) (strict) + cone.
    # 1*(-x^2) + (x)^2*1 + ... = 0 already ignores strict; add nothing.
    # So certify purely-strict-driven: { 1 - x >= 0, x - 2 > 0 }? Simpler below.
    # { x - 2 > 0, 2 - x >= 0 }: x > 2 and x <= 2, infeasible.
    # 1*(2 - x) [cone] + 1*(x - 2) [strict R] == 0.
    x = symbols("x")
    log = export_positivstellensatz_cert(
        gens=[x],
        nonstrict=[2 - x],
        strict=[x - 2],
        cone=[{"cone_indices": [0], "sos": {"squares": [1]}}],
        strict_part={"const": 1, "strict_indices": [0]})
    res = check(log)
    assert res["valid"] is True, res


# --------------------------------------------------------------------------- #
# Tamper rejection.
# --------------------------------------------------------------------------- #

def test_tampered_cofactor_rejected():
    x = symbols("x")
    log = export_positivstellensatz_cert(
        gens=[x],
        equalities=[x - 1],
        nonstrict=[-x],
        ideal=[{"equality_index": 0, "cofactor": 1}],
        cone=[{"cone_indices": [0], "sos": {"squares": [1]}}],
        strict_part={"const": 1})
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "psatz_refutation":
            step["ideal"][0]["cofactor"] = {"vars": ["x"], "terms": [[[0], "2"]]}  # 1 -> 2
    res = check(bad)
    assert res["valid"] is False
    assert "identity" in res["reason"]


def test_tampered_nonzero_sum_rejected():
    x = symbols("x")
    log = export_positivstellensatz_cert(
        gens=[x], nonstrict=[x - 1, -x],
        cone=[{"cone_indices": [0], "sos": {"squares": [1]}},
              {"cone_indices": [1], "sos": {"squares": [1]}}],
        strict_part={"const": 1})
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "psatz_refutation":
            step["strict_part"]["const"] = "2"  # sum becomes +1, not 0
    assert check(bad)["valid"] is False


def test_non_sos_gram_multiplier_rejected_with_witness():
    # Supply sigma = 1 via a duplicate-monomial Gram so a non-PSD Q still
    # reproduces the SAME polynomial (identity holds) but fails the PSD test:
    # z = [1, 1], Q = [[2,0],[0,-1]] -> z^T Q z = 2 - 1 = 1, yet Q is indefinite.
    x = symbols("x")
    valid = export_positivstellensatz_cert(
        gens=[x], nonstrict=[x - 1, -x],
        cone=[{"cone_indices": [0],
               "sos": {"monomials": [[0], [0]], "Q": [["1", "0"], ["0", "0"]]}},
              {"cone_indices": [1], "sos": {"squares": [1]}}],
        strict_part={"const": 1})
    assert check(valid)["valid"] is True  # PSD Gram for sigma = 1 accepted.

    bad = copy.deepcopy(valid)
    for step in bad["steps"]:
        if step["op"] == "psatz_refutation":
            step["cone"][0]["sos"]["Q"] = [["2", "0"], ["0", "-1"]]  # non-PSD, sigma still 1
    res = check(bad)
    assert res["valid"] is False
    assert "SOS" in res["reason"]
    assert res["witness"] is not None, "expected a sigma(u) < 0 witness"


def test_non_positive_strict_const_rejected():
    x = symbols("x")
    log = export_positivstellensatz_cert(
        gens=[x], nonstrict=[x - 1, -x],
        cone=[{"cone_indices": [0], "sos": {"squares": [1]}},
              {"cone_indices": [1], "sos": {"squares": [1]}}],
        strict_part={"const": 1})
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "psatz_refutation":
            step["strict_part"]["const"] = "0"
    res = check(bad)
    assert res["valid"] is False
    assert "> 0" in res["reason"]


def test_out_of_range_index_rejected():
    x = symbols("x")
    log = export_positivstellensatz_cert(
        gens=[x], nonstrict=[x - 1, -x],
        cone=[{"cone_indices": [0], "sos": {"squares": [1]}},
              {"cone_indices": [1], "sos": {"squares": [1]}}],
        strict_part={"const": 1})
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "psatz_refutation":
            step["cone"][0]["cone_indices"] = [5]  # no such nonstrict generator
    assert check(bad)["valid"] is False


# --------------------------------------------------------------------------- #
# Format rejection, determinism, run() dispatch, generator stub.
# --------------------------------------------------------------------------- #

def test_unknown_format_rejected():
    assert check({"format": "bogus.v9", "kind": KIND, "steps": []})["valid"] is False


def test_wrong_leading_op_rejected():
    assert check({"format": FORMAT, "kind": KIND,
                  "steps": [{"op": "psatz_refutation"}]})["valid"] is False


def test_generator_is_stubbed():
    assert generate is None


def test_determinism_export_and_check():
    x = symbols("x")
    a = export_positivstellensatz_cert(
        gens=[x], nonstrict=[x - 1, -x],
        cone=[{"cone_indices": [0], "sos": {"squares": [1]}},
              {"cone_indices": [1], "sos": {"squares": [1]}}],
        strict_part={"const": 1})
    b = export_positivstellensatz_cert(
        gens=[x], nonstrict=[x - 1, -x],
        cone=[{"cone_indices": [0], "sos": {"squares": [1]}},
              {"cone_indices": [1], "sos": {"squares": [1]}}],
        strict_part={"const": 1})
    assert json.dumps(a, sort_keys=True) == json.dumps(b, sort_keys=True)
    assert check(a)["valid"] == check(_roundtrip(a))["valid"] is True


def test_run_export_then_check_roundtrip():
    x = symbols("x")
    exported = run({"op": "export", "gens": [x], "nonstrict": [x - 1, -x],
                    "cone": [{"cone_indices": [0], "sos": {"squares": [1]}},
                             {"cone_indices": [1], "sos": {"squares": [1]}}],
                    "strict_part": {"const": 1}})
    assert "log" in exported
    assert run({"op": "check", "log": exported["log"]})["valid"] is True


def test_run_unknown_op_raises():
    with pytest.raises(ValueError):
        run({"op": "nope"})
