"""Tests for the Bezout / GCD-cofactor certificate exporter + reference checker.

Offline, deterministic, exact.  The CHECKER side is pure standard library and is
exercised on hand-crafted cert dicts (integers and univariate polynomials over
Q) so it runs without sympy, including the tamper-rejection tests (the soundness
boundary).  The polynomial GENERATOR path uses sympy and is gated with
``importorskip``.
"""
import copy
import sys
from pathlib import Path

import pytest

_ROOT = Path(__file__).resolve().parents[3]
for rel in ("components/verify/python", "components/tools/python",
            "components/prover/python"):
    sys.path.insert(0, str(_ROOT / rel))

from theoremata_tools.cert_bezout import (  # noqa: E402
    FORMAT,
    KINDS,
    check,
    egcd,
    export_bezout_cert,
    export_divides_cert,
    run,
)


# --------------------------------------------------------------------------- #
# Hand-crafted certs (no sympy) so the CHECKER is exercised pure-stdlib.
# --------------------------------------------------------------------------- #

def _bezout_log(a, b, g, u, v, domain="int"):
    return {
        "format": FORMAT,
        "kind": "bezout",
        "claim": f"gcd({a}, {b}) = {g}",
        "steps": [
            {"op": "bezout_relation", "domain": domain,
             "a": a, "b": b, "g": g, "u": u, "v": v},
            {"op": "assert_gcd"},
        ],
        "meta": {"producer": "test"},
    }


def _divides_log(a, b, q, domain="int"):
    return {
        "format": FORMAT,
        "kind": "bezout",
        "claim": f"{a} | {b}",
        "steps": [
            {"op": "divides_relation", "domain": domain, "a": a, "b": b, "q": q},
            {"op": "assert_divides"},
        ],
        "meta": {"producer": "test"},
    }


# gcd(12, 18) = 6 = (-1)*12 + 1*18.
def test_valid_int_bezout():
    res = check(_bezout_log(12, 18, 6, -1, 1))
    assert res["valid"] is True, res["reason"]
    assert res["kind"] == "bezout"
    assert res["subkind"] == "bezout"
    assert res["domain"] == "int"
    assert res["checked_steps"] == 2


def test_valid_int_bezout_negative_and_coprime():
    # gcd(-4, 6) = 2 = 1*(-4) + 1*6.
    assert check(_bezout_log(-4, 6, 2, 1, 1))["valid"] is True
    # coprime: gcd(35, 64) = 1 = 11*35 + (-6)*64 = 385 - 384.
    assert check(_bezout_log(35, 64, 1, 11, -6))["valid"] is True


def test_egcd_matches_math_gcd():
    from math import gcd
    for a, b in [(12, 18), (35, 64), (-4, 6), (0, 5), (5, 0), (17, 17), (100, 40)]:
        g, u, v = egcd(a, b)
        assert g == gcd(a, b)
        assert u * a + v * b == g
        assert g >= 0


# --------------------------------------------------------------------------- #
# Tamper rejection (soundness boundary) — pure stdlib.
# --------------------------------------------------------------------------- #

def test_reject_tampered_cofactor():
    # Same g=6 but wrong u: 0*12 + 1*18 = 18 != 6.
    res = check(_bezout_log(12, 18, 6, 0, 1))
    assert res["valid"] is False
    assert "Bezout relation" in res["reason"]


def test_reject_non_divisor_g():
    # Cofactors satisfy the identity for g=6, but claim g=5 which is NOT the
    # combination and does not divide the operands. Use u,v so u*12+v*18 = 5?
    # Craft: claim g=4 with u=-1,v=1 gives -12+18 = 6 != 4 -> relation fails
    # first. To isolate the divisibility check, keep the identity TRUE for a g
    # that is a genuine common combination but not a divisor is impossible
    # (a combination that equals g with g|a,g|b is forced). So we test a g that
    # divides neither: 5 with u=4,v=-2: 48-36 = 12 != 5 (identity fails). The
    # divisibility guard is exercised by a g satisfying the identity yet not
    # dividing: g=12,u=1,v=0 -> 12 == 12 (identity ok), 12 | 12 ok but 12 | 18
    # is false.
    res = check(_bezout_log(12, 18, 12, 1, 0))
    assert res["valid"] is False
    assert "does not divide" in res["reason"]


def test_reject_non_normalized_negative_g():
    # -6 = (1)*12 + (-1)*18 satisfies the identity but g must be positive.
    res = check(_bezout_log(12, 18, -6, 1, -1))
    assert res["valid"] is False
    assert "normalized" in res["reason"] or "positive" in res["reason"]


def test_reject_wrong_format_and_kind():
    log = _bezout_log(12, 18, 6, -1, 1)
    log["format"] = "bogus"
    assert check(log)["valid"] is False
    log2 = _bezout_log(12, 18, 6, -1, 1)
    log2["kind"] = "not_bezout"
    assert check(log2)["valid"] is False


def test_reject_boolean_smuggled_as_int():
    bad = _bezout_log(12, 18, 6, -1, 1)
    bad["steps"][0]["g"] = True  # bool must be rejected, not treated as 1
    assert check(bad)["valid"] is False


# --------------------------------------------------------------------------- #
# divides sub-kind.
# --------------------------------------------------------------------------- #

def test_divides_valid():
    # 3 | 12 via quotient 4.
    res = check(_divides_log(3, 12, 4))
    assert res["valid"] is True, res["reason"]
    assert res["subkind"] == "divides"


def test_divides_reject_bad_quotient():
    # 3 * 5 = 15 != 12.
    res = check(_divides_log(3, 12, 5))
    assert res["valid"] is False
    assert "a | b fails" in res["reason"]


def test_divides_export_nondivisor_raises():
    # 5 does not divide 12: export must raise.
    with pytest.raises(ValueError):
        export_divides_cert(5, 12)


def test_divides_export_check_roundtrip():
    log = export_divides_cert(3, 12)
    res = check(log)
    assert res["valid"] is True and res["subkind"] == "divides"


# --------------------------------------------------------------------------- #
# Generator side (int path needs no sympy).
# --------------------------------------------------------------------------- #

@pytest.mark.parametrize("a,b", [(12, 18), (35, 64), (0, 7), (17, 17), (240, 46)])
def test_int_export_and_check(a, b):
    log = export_bezout_cert(a, b)
    assert log["format"] == FORMAT and log["kind"] == "bezout"
    assert check(log)["valid"] is True


def test_int_determinism():
    assert export_bezout_cert(240, 46) == export_bezout_cert(240, 46)
    assert export_divides_cert(3, 12) == export_divides_cert(3, 12)


def test_run_int_export_then_check_roundtrip():
    out = run({"op": "export", "subkind": "bezout", "a": 12, "b": 18})
    assert "log" in out
    res = run({"op": "check", "log": out["log"]})
    assert res["valid"] is True and res["subkind"] == "bezout"


def test_run_check_rejects_tampered():
    res = run({"op": "check", "log": _bezout_log(12, 18, 6, 0, 1)})
    assert res["valid"] is False


# --------------------------------------------------------------------------- #
# Polynomial domain (checker pure-stdlib; generator gated on sympy).
# --------------------------------------------------------------------------- #

# Hand-crafted poly cert, coeffs LOW-to-HIGH over Q:
#   a = x^2 - 1 = (x-1)(x+1)  -> [-1, 0, 1]
#   b = x^2 + 2x + 1 = (x+1)^2 -> [1, 2, 1]
#   g = gcd = x + 1 (monic)    -> [1, 1]
# Cofactors: u=-1/2, v=1/2:  (-1/2)(x^2-1) + (1/2)(x^2+2x+1)
#                          =  (1/2)(2x + 2) = x + 1 = g.  OK.
def test_poly_valid_handcrafted():
    log = _bezout_log(["-1", "0", "1"], ["1", "2", "1"], ["1", "1"],
                      ["-1/2"], ["1/2"], domain="poly")
    res = check(log)
    assert res["valid"] is True, res["reason"]
    assert res["domain"] == "poly"


def test_poly_reject_non_monic_g():
    # 2x + 2 divides both and is the combination, but is NOT monic.
    #   u=-1, v=1: -(x^2-1) + (x^2+2x+1) = 2x + 2.
    log = _bezout_log(["-1", "0", "1"], ["1", "2", "1"], ["2", "2"],
                      ["-1"], ["1"], domain="poly")
    res = check(log)
    assert res["valid"] is False
    assert "monic" in res["reason"]


def test_poly_reject_tampered_cofactor():
    log = _bezout_log(["-1", "0", "1"], ["1", "2", "1"], ["1", "1"],
                      ["0"], ["1/2"], domain="poly")  # u wrong
    assert check(log)["valid"] is False


def test_poly_divides_valid():
    # (x+1) | (x^2 - 1) with quotient (x - 1).
    log = _divides_log(["1", "1"], ["-1", "0", "1"], ["-1", "1"], domain="poly")
    assert check(log)["valid"] is True


def test_poly_divides_reject_bad_quotient():
    log = _divides_log(["1", "1"], ["-1", "0", "1"], ["1", "1"], domain="poly")
    assert check(log)["valid"] is False


def test_poly_generator_export_and_check():
    pytest.importorskip("sympy")
    # a = x^2 - 1, b = x^2 + 2x + 1 -> gcd x + 1.
    log = export_bezout_cert([-1, 0, 1], [1, 2, 1], domain="poly")
    assert log["kind"] == "bezout"
    res = check(log)
    assert res["valid"] is True, res["reason"]
    # g should be monic x + 1.
    assert log["steps"][0]["g"] == ["1", "1"]


def test_poly_generator_determinism():
    pytest.importorskip("sympy")
    assert (export_bezout_cert([-1, 0, 1], [1, 2, 1], domain="poly")
            == export_bezout_cert([-1, 0, 1], [1, 2, 1], domain="poly"))


def test_poly_generator_divides_roundtrip():
    pytest.importorskip("sympy")
    log = export_divides_cert([-1, 0, 1], [-1, -1, 1, 1], domain="poly")  # (x^2-1)|(x^3+x^2-x-1)?
    # x^3 + x^2 - x - 1 = (x^2 - 1)(x + 1); coeffs low->high: [-1,-1,1,1].
    assert check(log)["valid"] is True
