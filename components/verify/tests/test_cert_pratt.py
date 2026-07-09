"""Tests for the Pratt primality certificate exporter + reference checker.

Offline, deterministic, exact.  The GENERATOR side (build_pratt / export) uses
``sympy.factorint`` and is gated with ``importorskip``.  The CHECKER side is
pure standard library and is exercised on hand-crafted cert dicts so it runs
without sympy, including the tamper-rejection tests (the soundness boundary).
"""
import copy
import sys
from pathlib import Path

import pytest

_ROOT = Path(__file__).resolve().parents[3]
for rel in ("components/verify/python", "components/tools/python",
            "components/prover/python"):
    sys.path.insert(0, str(_ROOT / rel))

from theoremata_tools.cert_pratt import (  # noqa: E402
    FORMAT,
    KINDS,
    check,
    run,
)


# --------------------------------------------------------------------------- #
# Hand-crafted certs (no sympy) so the CHECKER is exercised pure-stdlib.
# --------------------------------------------------------------------------- #

# 7 is prime: 7-1 = 6 = 2 * 3; witness a = 3 has order 6 mod 7.
#   3^6 = 729 ≡ 1 (mod 7); 3^3 = 27 ≡ 6 ≢ 1; 3^2 = 9 ≡ 2 ≢ 1.
_NODE_7 = {
    "n": 7, "a": 3, "factors": [[2, 1], [3, 1]],
    "children": [{"n": 3, "a": 2, "factors": [[2, 1]], "children": []}],
}


def _log(node):
    return {
        "format": FORMAT,
        "kind": "pratt_primality",
        "claim": f"{node['n']} is prime (Pratt certificate)",
        "steps": [{"op": "pratt_witness", "root": node}, {"op": "assert_prime"}],
        "meta": {"producer": "test"},
    }


def test_handcrafted_valid_cert_pure_stdlib():
    """A valid Pratt cert for 7 (with recursive child for 3) checks True."""
    res = check(_log(_NODE_7))
    assert res["valid"] is True, res["reason"]
    assert res["n"] == 7
    assert res["kind"] == "pratt_primality"
    assert res["checked_nodes"] == 2  # 7 and its child 3


def test_base_case_two():
    res = check(_log({"n": 2}))
    assert res["valid"] is True
    assert res["n"] == 2


# --------------------------------------------------------------------------- #
# Tamper rejection (soundness boundary) — all pure-stdlib.
# --------------------------------------------------------------------------- #

def test_reject_wrong_witness():
    bad = copy.deepcopy(_NODE_7)
    bad["a"] = 2  # 2 has order 3 mod 7 (2^3 = 8 ≡ 1), NOT a primitive root
    res = check(_log(bad))
    assert res["valid"] is False
    assert "order check" in res["reason"] or "Fermat" in res["reason"]


def test_reject_incomplete_factorization():
    bad = copy.deepcopy(_NODE_7)
    bad["factors"] = [[3, 1]]        # drop the factor 2: 3 != 6
    bad["children"] = [{"n": 3, "a": 2, "factors": [[2, 1]], "children": []}]
    res = check(_log(bad))
    assert res["valid"] is False
    assert "incomplete" in res["reason"]


def test_reject_dropped_prime_factor_with_matching_product():
    # n-1 = 6; claim it is 6^1 with "prime" 6 -> product matches but 6 isn't
    # prime, so the child cert requirement fails.
    bad = {"n": 7, "a": 3, "factors": [[6, 1]], "children": []}
    res = check(_log(bad))
    assert res["valid"] is False
    assert "child certificate" in res["reason"]


def test_reject_missing_child_for_prime_factor():
    bad = copy.deepcopy(_NODE_7)
    bad["children"] = []  # 3 is a prime factor > 2 but has no child cert
    res = check(_log(bad))
    assert res["valid"] is False
    assert "child certificate" in res["reason"]


def test_reject_bad_child():
    bad = copy.deepcopy(_NODE_7)
    bad["children"] = [{"n": 3, "a": 2, "factors": [[2, 2]], "children": []}]  # 2^2=4 != 2
    res = check(_log(bad))
    assert res["valid"] is False


def test_reject_composite_n():
    # 9 is composite; no primitive root exists. Any witness fails order check.
    bad = {"n": 9, "a": 2, "factors": [[2, 3]], "children": []}  # 2^3 = 8 != 9-1
    res = check(_log(bad))
    assert res["valid"] is False


def test_reject_unknown_format_and_kind():
    log = _log(_NODE_7)
    log["format"] = "bogus"
    assert check(log)["valid"] is False
    log2 = _log(_NODE_7)
    log2["kind"] = "not_pratt"
    assert check(log2)["valid"] is False


# --------------------------------------------------------------------------- #
# Generator side (needs sympy).
# --------------------------------------------------------------------------- #

@pytest.mark.parametrize("p", [3, 5, 7, 13, 97, 1009])
def test_generator_export_and_check(p):
    pytest.importorskip("sympy")
    from theoremata_tools.cert_pratt import export_pratt_cert

    log = export_pratt_cert(p)
    assert log["format"] == FORMAT and log["kind"] == "pratt_primality"
    res = check(log)
    assert res["valid"] is True, res["reason"]
    assert res["n"] == p


@pytest.mark.parametrize("composite", [9, 15, 21, 100, 1001])
def test_generator_rejects_composite(composite):
    pytest.importorskip("sympy")
    from theoremata_tools.cert_pratt import build_pratt

    with pytest.raises(ValueError):
        build_pratt(composite)


def test_generator_determinism():
    pytest.importorskip("sympy")
    from theoremata_tools.cert_pratt import export_pratt_cert

    assert export_pratt_cert(1009) == export_pratt_cert(1009)


def test_run_export_then_check_roundtrip():
    pytest.importorskip("sympy")
    out = run({"op": "export", "n": 97})
    assert "log" in out
    res = run({"op": "check", "log": out["log"]})
    assert res["valid"] is True and res["n"] == 97


def test_run_check_rejects_tampered():
    """run(check) on a tampered hand-crafted cert (no sympy needed)."""
    bad = copy.deepcopy(_NODE_7)
    bad["a"] = 2
    res = run({"op": "check", "log": _log(bad)})
    assert res["valid"] is False
