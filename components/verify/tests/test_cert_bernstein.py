"""Tests for the Bernstein-basis nonnegativity certificate (kind ``bernstein``).

Pure stdlib (exact ``fractions``); no sympy needed. Path to the split
``theoremata_tools`` namespace is set up by ``conftest.py``.
"""
import json

from theoremata_tools.cert_bernstein import (
    bernstein_coeffs,
    check,
    export_bernstein_cert,
    run,
)


def test_bernstein_coeffs_are_exact():
    # p(x) = x on [0,1]: degree-1 Bernstein coeffs are [0, 1].
    assert [str(v) for v in bernstein_coeffs(["0", "1"], 0, 1)] == ["0", "1"]
    # p(x) = x^2 on [0,1]: [0, 0, 1].
    assert [str(v) for v in bernstein_coeffs(["0", "0", "1"], 0, 1)] == ["0", "0", "1"]


def test_valid_nonneg_cert_on_unit_interval_checks():
    # p(x) = x^2 + 1 >= 0 on [0,1]; Bernstein coeffs [1, 1, 2] all nonneg.
    log = export_bernstein_cert(["1", "0", "1"], ["0", "1"])
    assert log["kind"] == "bernstein"
    res = check(log)
    assert res["valid"] is True, res
    assert res["checked_steps"] == 2


def test_valid_cert_on_nonunit_interval_checks():
    # p(x) = x >= 0 on [1,3]; Bernstein coeffs [1, 3].
    log = export_bernstein_cert(["0", "1"], ["1", "3"])
    assert check(log)["valid"] is True


def test_export_refuses_positive_but_not_certifiable_at_this_degree():
    # (x-1)^2 = 1 - 2x + x^2 is >= 0 everywhere, but on [0,2] its degree-2
    # Bernstein coefficient b_1 = -1 < 0 -> not certifiable without degree
    # elevation. The exporter must refuse rather than emit a bogus cert.
    try:
        export_bernstein_cert(["1", "-2", "1"], ["0", "2"])
        assert False, "expected refusal for a non-certifiable-at-this-degree polynomial"
    except ValueError as exc:
        assert "Bernstein coefficient is negative" in str(exc)


def test_genuinely_negative_polynomial_is_rejected():
    # p(x) = x - 1 is negative near 0 on [0,1] (p(0) = -1). A forged cert that
    # claims nonneg coeffs must be rejected: recomputation yields b_0 = -1.
    forged = {
        "format": "theoremata.cert-log.v1",
        "kind": "bernstein",
        "claim": "p(x) >= 0 on [0, 1]",
        "steps": [
            {"op": "bernstein_problem", "var": "x", "coeffs": ["-1", "1"], "domain": ["0", "1"]},
            {"op": "bernstein_coeffs", "degree": 1, "b": ["0", "1"]},  # lie: real b_0 = -1
            {"op": "assert_coeffs_match"},
            {"op": "assert_nonneg"},
        ],
        "meta": {},
    }
    res = check(forged)
    assert res["valid"] is False
    assert "do not match" in res["reason"]


def test_tampered_coefficient_is_rejected():
    log = export_bernstein_cert(["1", "0", "1"], ["0", "1"])
    # Corrupt a carried Bernstein coefficient; recomputation catches it.
    for s in log["steps"]:
        if s["op"] == "bernstein_coeffs":
            s["b"][0] = str(int(s["b"][0]) + 5)
    res = check(log)
    assert res["valid"] is False
    assert "do not match" in res["reason"]


def test_tampered_monomial_coeff_flips_the_recomputation():
    log = export_bernstein_cert(["1", "0", "1"], ["0", "1"])
    for s in log["steps"]:
        if s["op"] == "bernstein_problem":
            s["coeffs"][2] = "-3"  # p becomes -3x^2 + 1: dips negative on [0,1]
    res = check(log)
    assert res["valid"] is False


def test_unknown_format_and_kind_rejected():
    assert check({"format": "nope"})["valid"] is False
    assert check({"format": "theoremata.cert-log.v1", "kind": "sos"})["valid"] is False
    assert check("not a dict")["valid"] is False


def test_missing_assertion_step_rejected():
    log = export_bernstein_cert(["0", "1"], ["0", "1"])
    log["steps"] = [s for s in log["steps"] if s["op"] != "assert_nonneg"]
    assert check(log)["valid"] is False


def test_json_round_trip_and_determinism():
    log = export_bernstein_cert(["1", "0", "1"], ["0", "1"])
    again = export_bernstein_cert(["1", "0", "1"], ["0", "1"])
    assert log == again  # deterministic
    reloaded = json.loads(json.dumps(log))
    assert check(reloaded)["valid"] is True


def test_run_export_then_check_round_trip():
    out = run({"op": "export", "coeffs": ["1", "0", "1"], "interval": ["0", "1"]})
    assert out["ok"] is True
    assert run({"op": "check", "log": out["log"]})["valid"] is True
