"""Tests for the Wilf--Zeilberger certificate exporter + reference checker.

Offline, deterministic, exact.  Uses sympy (Gosper) to FIND a WZ certificate for
a classic hypergeometric identity, serializes it to ``theoremata.cert-log.v1``
(kind ``wz``), and confirms the self-contained checker (a) validates a genuine
certificate and (b) REJECTS a tampered one -- the soundness boundary.
"""
import copy
import sys
from pathlib import Path

import pytest

pytest.importorskip("sympy")

# The verify tools live under this component root (mirror test_cert_log.py).
_ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(_ROOT / "components/verify/python"))

import sympy  # noqa: E402
from sympy import binomial, symbols  # noqa: E402

from theoremata_tools.cert_wz import (  # noqa: E402
    FORMAT,
    KIND,
    WZError,
    check,
    derive_wz_certificate,
    export_wz_cert,
    run,
)

n, k = symbols("n k", integer=True, nonnegative=True)


# --------------------------------------------------------------------------- #
# Classic identities.
# --------------------------------------------------------------------------- #

def _binomial_sum_cert():
    # Sum_k C(n,k) = 2^n
    return export_wz_cert(binomial(n, k), 2 ** n)


def test_export_shape_binomial():
    log = _binomial_sum_cert()
    assert log["format"] == FORMAT
    assert log["kind"] == KIND == "wz"
    ops = [s["op"] for s in log["steps"]]
    assert ops == ["wz_summand", "wz_certificate", "wz_identity",
                   "assert_wz_telescoping"]
    # Expressions carry both a round-trippable srepr and a human string.
    F = log["steps"][0]["F"]
    assert "srepr" in F and "str" in F


def test_check_valid_binomial():
    log = _binomial_sum_cert()
    res = check(log)
    assert res["valid"] is True, res["reason"]
    assert res["kind"] == "wz"
    assert res["checked_steps"] == 4


def test_check_valid_second_identity():
    # Sum_k k*C(n,k) = n*2^(n-1)  (also Gosper-summable)
    log = export_wz_cert(k * binomial(n, k), n * 2 ** (n - 1))
    res = check(log)
    assert res["valid"] is True, res["reason"]


def test_tampered_certificate_rejected():
    log = _binomial_sum_cert()
    tampered = copy.deepcopy(log)
    # Corrupt R(n,k): replace with a different rational function.
    bad = sympy.srepr((k + 1) / (n + 2))
    tampered["steps"][1]["R"] = {"srepr": bad, "str": str((k + 1) / (n + 2))}
    res = check(tampered)
    assert res["valid"] is False
    assert "telescoping" in res["reason"].lower()


def test_tampered_summand_rejected():
    log = _binomial_sum_cert()
    tampered = copy.deepcopy(log)
    # Change F to an unrelated hypergeometric term; R no longer telescopes it.
    bad = sympy.srepr(binomial(n, k) ** 2)
    tampered["steps"][0]["F"] = {"srepr": bad, "str": str(binomial(n, k) ** 2)}
    res = check(tampered)
    assert res["valid"] is False


def test_malformed_rejected_not_crash():
    assert check({"format": "nope"})["valid"] is False
    assert check({"format": FORMAT, "kind": "wz", "steps": []})["valid"] is False
    assert check({"format": FORMAT, "kind": "wz",
                  "steps": [{"op": "wz_summand", "F": {"srepr": "@@bad@@"}}],
                  "claim": ""})["valid"] is False
    # Missing conclusion step -> no verified conclusion.
    log = _binomial_sum_cert()
    log2 = {**log, "steps": log["steps"][:3]}
    assert check(log2)["valid"] is False


def test_determinism():
    a = _binomial_sum_cert()
    b = _binomial_sum_cert()
    assert a == b
    r1, r2 = check(a), check(b)
    assert r1 == r2 and r1["valid"] is True


def test_not_gosper_summable_graceful():
    # Sum_k 1/(k^2+n+1) has no hypergeometric closed form: no WZ certificate.
    with pytest.raises(WZError):
        export_wz_cert(1 / (k ** 2 + n + 1), 1)
    # The worker path returns a clean reason instead of raising.
    res = run({"op": "export", "summand": "1/(k**2+n+1)", "rhs": 1})
    assert res["ok"] is False
    assert "gosper" in res["reason"].lower() or "summable" in res["reason"].lower()


def test_run_export_then_check_roundtrip():
    exported = run({"op": "export", "summand": "binomial(n,k)", "rhs": "2**n"})
    assert exported["ok"] is True
    res = run({"op": "check", "log": exported["log"]})
    assert res["valid"] is True, res["reason"]


def test_derive_returns_certificate():
    parts = derive_wz_certificate(binomial(n, k), 2 ** n)
    # G(n,k) = R(n,k) F(n,k) and the telescoping residual is exactly 0.
    F, R, G = parts["F"], parts["R"], parts["G"]
    residual = sympy.simplify(
        (F.subs(n, n + 1) - F) - (G.subs(k, k + 1) - G))
    assert residual == 0
    assert sympy.simplify(G - R * F) == 0
