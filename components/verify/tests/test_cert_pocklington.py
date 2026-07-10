"""Tests for the Pocklington primality certificate exporter + reference checker.

Offline, deterministic, exact.  The GENERATOR side (build_pocklington / export)
uses ``sympy.factorint`` and is gated with ``importorskip``.  The CHECKER side is
pure standard library and is exercised on hand-crafted cert dicts so it runs
without sympy, including the tamper-rejection tests (the soundness boundary).

The defining contrast with the Pratt cert: only the factored part ``F`` of
``n - 1`` (with ``F > sqrt(n)``) is factored/recursed on; the cofactor ``R`` is
left unfactored.
"""
import copy
import sys
from pathlib import Path

import pytest

_ROOT = Path(__file__).resolve().parents[3]
for rel in ("components/verify/python", "components/tools/python",
            "components/prover/python"):
    sys.path.insert(0, str(_ROOT / rel))

from theoremata_tools.cert_pocklington import (  # noqa: E402
    FORMAT,
    KINDS,
    check,
    run,
)


# --------------------------------------------------------------------------- #
# Hand-crafted certs (no sympy) so the CHECKER is exercised pure-stdlib.
# --------------------------------------------------------------------------- #

# 7 is prime: 7-1 = 6 = F*R with F = 3 (> sqrt(7) ~ 2.65), R = 2 UNFACTORED.
#   witness a = 3: 3^6 = 729 ≡ 1 (mod 7); q=3: gcd(3^(6/3) - 1, 7) = gcd(9-1,7)
#   = gcd(8,7) = 1  (checker computes 3^2 mod 7 = 2, gcd(2-1,7)=1).
# The single prime factor of F is 3, certified by a recursive child node.
_NODE_3 = {"n": 3, "a": 2, "F": 2, "R": 1, "factors": [[2, 1]], "children": []}
_NODE_7 = {
    "n": 7, "a": 3, "F": 3, "R": 2, "factors": [[3, 1]],
    "children": [_NODE_3],
}


def _log(node):
    return {
        "format": FORMAT,
        "kind": "pocklington_primality",
        "claim": f"{node['n']} is prime (Pocklington certificate)",
        "steps": [{"op": "pocklington_witness", "root": node},
                  {"op": "assert_prime"}],
        "meta": {"producer": "test"},
    }


def test_handcrafted_valid_cert_pure_stdlib():
    """A valid Pocklington cert for 7 (child for prime factor 3 of F) checks True."""
    res = check(_log(_NODE_7))
    assert res["valid"] is True, res["reason"]
    assert res["n"] == 7
    assert res["kind"] == "pocklington_primality"
    assert res["checked_nodes"] == 2  # 7 and its child 3


def test_base_case_two():
    res = check(_log({"n": 2}))
    assert res["valid"] is True
    assert res["n"] == 2


def test_cofactor_R_is_left_unfactored():
    """The distinguishing property vs Pratt: R carries no factorization at all."""
    # 7-1 = 6, F = 3 factored, R = 2 present only as an integer (no 'factors').
    assert "factors" not in {k: v for k, v in _NODE_7.items() if k == "R"}
    assert _NODE_7["R"] == 2
    assert check(_log(_NODE_7))["valid"] is True


# --------------------------------------------------------------------------- #
# Tamper rejection (soundness boundary) — all pure-stdlib.
# --------------------------------------------------------------------------- #

def test_reject_wrong_witness():
    # a = 6 ≡ -1 (mod 7) passes Fermat but gcd(6^2 - 1, 7) = gcd(0,7) = 7 != 1.
    bad = copy.deepcopy(_NODE_7)
    bad["a"] = 6
    res = check(_log(bad))
    assert res["valid"] is False
    assert "gcd condition" in res["reason"]


def test_reject_bad_gcd_condition():
    # Explicit bad-gcd path with a different witness that also fails only the
    # gcd condition (order too small for the prime q | F).
    bad = copy.deepcopy(_NODE_7)
    bad["a"] = 6
    res = check(_log(bad))
    assert res["valid"] is False
    assert "gcd" in res["reason"]


def test_reject_F_too_small():
    # Choose F = 2 (< sqrt(7) ~ 2.65): F*R = 6 = n-1 and gcd(2,3)=1, but F^2 = 4
    # <= 7, so the factored part does not dominate -> rejected.
    bad = {"n": 7, "a": 3, "F": 2, "R": 3, "factors": [[2, 1]], "children": []}
    res = check(_log(bad))
    assert res["valid"] is False
    assert "too small" in res["reason"]


def test_reject_incomplete_factorization_of_F():
    # F = 3 but factors only list [2,1]: prod = 2 != F = 3 -> incomplete.
    bad = copy.deepcopy(_NODE_7)
    bad["factors"] = [[2, 1]]
    res = check(_log(bad))
    assert res["valid"] is False
    assert "incomplete" in res["reason"]


def test_reject_F_R_product_wrong():
    # F*R must equal n-1. Here 3*3 = 9 != 6.
    bad = copy.deepcopy(_NODE_7)
    bad["R"] = 3
    res = check(_log(bad))
    assert res["valid"] is False
    assert "!= n-1" in res["reason"]


def test_reject_F_R_not_coprime():
    # 13-1 = 12; F = 6, R = 2 -> F*R = 12 but gcd(6,2) = 2 != 1.
    bad = {"n": 13, "a": 2, "F": 6, "R": 2, "factors": [[2, 1], [3, 1]],
           "children": [{"n": 3, "a": 2, "F": 2, "R": 1,
                         "factors": [[2, 1]], "children": []}]}
    res = check(_log(bad))
    assert res["valid"] is False
    assert "not coprime" in res["reason"]


def test_reject_unfactored_prime_of_F_lacks_child():
    # F = 3 requires a child certifying that 3 is prime; drop it.
    bad = copy.deepcopy(_NODE_7)
    bad["children"] = []
    res = check(_log(bad))
    assert res["valid"] is False
    assert "child certificate" in res["reason"]


def test_reject_composite_fermat_fails():
    # 9 is composite: F = 8 = 2^3 (> sqrt(9)), R = 1, but a = 2 fails Fermat:
    # 2^8 = 256 ≡ 4 (mod 9) != 1 -> rejected.
    bad = {"n": 9, "a": 2, "F": 8, "R": 1, "factors": [[2, 3]], "children": []}
    res = check(_log(bad))
    assert res["valid"] is False
    assert "Fermat" in res["reason"]


def test_reject_unknown_format_and_kind():
    log = _log(_NODE_7)
    log["format"] = "bogus"
    assert check(log)["valid"] is False
    log2 = _log(_NODE_7)
    log2["kind"] = "not_pocklington"
    assert check(log2)["valid"] is False


def test_determinism_of_checker():
    a = check(_log(_NODE_7))
    b = check(_log(_NODE_7))
    assert a == b


# --------------------------------------------------------------------------- #
# Generator side (needs sympy).
# --------------------------------------------------------------------------- #

@pytest.mark.parametrize("p", [3, 5, 7, 13, 97, 1009, 7919])
def test_generator_export_and_check(p):
    pytest.importorskip("sympy")
    from theoremata_tools.cert_pocklington import export_pocklington_cert

    log = export_pocklington_cert(p)
    assert log["format"] == FORMAT and log["kind"] == "pocklington_primality"
    res = check(log)
    assert res["valid"] is True, res["reason"]
    assert res["n"] == p


def test_generator_uses_partial_factorization():
    """The exported F is a PROPER divisor of n-1 (partial factorization) when
    possible — the whole point vs Pratt."""
    pytest.importorskip("sympy")
    from theoremata_tools.cert_pocklington import build_pocklington

    # 1009-1 = 1008 = 2^4 * 3^2 * 7; F need only exceed sqrt(1009) ~ 31.8, so R
    # stays > 1 (a genuine unfactored cofactor).
    node = build_pocklington(1009)
    assert node["F"] * node["R"] == 1008
    assert node["R"] > 1  # not the full n-1: some of n-1 left unfactored
    assert node["F"] * node["F"] > 1009


@pytest.mark.parametrize("composite", [9, 15, 21, 100, 1001])
def test_generator_rejects_composite(composite):
    pytest.importorskip("sympy")
    from theoremata_tools.cert_pocklington import build_pocklington

    with pytest.raises(ValueError):
        build_pocklington(composite)


def test_generator_determinism():
    pytest.importorskip("sympy")
    from theoremata_tools.cert_pocklington import export_pocklington_cert

    assert export_pocklington_cert(1009) == export_pocklington_cert(1009)


def test_run_export_then_check_roundtrip():
    pytest.importorskip("sympy")
    out = run({"op": "export", "n": 97})
    assert "log" in out
    res = run({"op": "check", "log": out["log"]})
    assert res["valid"] is True and res["n"] == 97


def test_run_check_rejects_tampered():
    """run(check) on a tampered hand-crafted cert (no sympy needed)."""
    bad = copy.deepcopy(_NODE_7)
    bad["a"] = 6
    res = run({"op": "check", "log": _log(bad)})
    assert res["valid"] is False
