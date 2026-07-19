"""Tests for the Wolfram-as-untrusted-generator module.

These run with NO Wolfram Engine present (the CI condition).  The engine bridge
is monkeypatched, so every test is offline and deterministic.  The load-bearing
assertions are:

* absent engine  -> clean ``unavailable``, never an exception, never a cert;
* canned VALID   -> the generated document is accepted by the REAL existing
                    checker (proving the generator's output really feeds it);
* canned TAMPERED-> no certificate is returned and the checker's rejection is
                    reported;
* canned FLOAT   -> rejected before it can reach a checker at all.
"""
import sys
from pathlib import Path

import pytest

pytest.importorskip("sympy")

_ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(_ROOT / "components/verify/python"))

from theoremata_tools import cert_nullstellensatz as ns_mod  # noqa: E402
from theoremata_tools import cert_sos as sos_mod  # noqa: E402
from theoremata_tools import cert_sturm as sturm_mod  # noqa: E402
from theoremata_tools import wolfram_cert as wc  # noqa: E402


# --------------------------------------------------------------------------- #
# Helpers: fake engine bridges.
# --------------------------------------------------------------------------- #

def _engine(monkeypatch, result, *, ok=True):
    """Pretend an engine exists and always replies with ``result``."""
    monkeypatch.setattr(wc, "available", lambda: True)

    def _fake_evaluate(code, *, timeout=30.0):
        return {"ok": ok, "result": result, "error": None if ok else "boom",
                "unavailable": False}

    monkeypatch.setattr(wc, "evaluate", _fake_evaluate)


def _no_engine(monkeypatch):
    monkeypatch.setattr(wc, "available", lambda: False)

    def _boom(code, *, timeout=30.0):  # must never be reached
        raise AssertionError("evaluate() called while the engine is unavailable")

    monkeypatch.setattr(wc, "evaluate", _boom)


# Canned requests reused across tests.
_SOS_REQ = {"op": "sos", "p": "x**2 + 2*x + 1", "x": "x"}
_NS_REQ = {"op": "nullstellensatz", "polys": ["x - 1", "y - 1"],
           "gens": ["x", "y"], "target": "x*y - 1"}
_STURM_REQ = {"op": "sturm", "coeffs": ["-1", "0", "1"], "interval": ["-2", "2"]}


# --------------------------------------------------------------------------- #
# 1. No engine -> clean unavailable for every op.
# --------------------------------------------------------------------------- #

@pytest.mark.parametrize("request_", [_SOS_REQ, _NS_REQ, _STURM_REQ])
def test_unavailable_is_clean_for_every_op(monkeypatch, request_):
    _no_engine(monkeypatch)
    res = wc.run(request_)
    assert res["unavailable"] is True
    assert res["ok"] is False
    assert res["cert"] is None
    assert res["checked"] is False
    assert "reason" in res


def test_every_declared_op_is_dispatchable(monkeypatch):
    _no_engine(monkeypatch)
    for op in wc.OPS:
        assert wc.run({"op": op, "p": "x**2", "x": "x", "polys": [], "gens": ["x"],
                       "coeffs": ["1"], "interval": ["0", "1"]})["unavailable"]


def test_unknown_op_raises():
    with pytest.raises(ValueError):
        wc.run({"op": "not_an_op"})


# --------------------------------------------------------------------------- #
# 2. Canned VALID replies -> our REAL checker accepts the generated document.
# --------------------------------------------------------------------------- #

def test_sos_valid_reply_is_accepted_by_the_real_checker(monkeypatch):
    # (x + 1)^2 == x^2 + 2x + 1, so the SOS identity holds exactly.
    _engine(monkeypatch, "{{1 + x}, {}}")
    res = wc.run(_SOS_REQ)
    assert res["ok"] is True
    assert res["cert"] is not None
    assert res["check"]["valid"] is True
    # The document must be a real cert-log the EXISTING checker accepts on its own.
    assert res["cert"]["format"] == sos_mod.FORMAT
    assert res["cert"]["kind"] == "sos"
    assert sos_mod.check(res["cert"])["valid"] is True


def test_sos_interval_valid_reply_is_accepted(monkeypatch):
    # p = (x-1)(2-x) = -x^2 + 3x - 2 on [1, 2]: multiplier form with t = 1.
    _engine(monkeypatch, "{{}, {1}}")
    res = wc.run({"op": "sos", "p": "-x**2 + 3*x - 2", "x": "x",
                  "interval": ["1", "2"]})
    assert res["ok"] is True
    assert sos_mod.check(res["cert"])["valid"] is True


def test_nullstellensatz_valid_reply_is_accepted_by_the_real_checker(monkeypatch):
    # y*(x - 1) + 1*(y - 1) == x*y - 1.
    _engine(monkeypatch, "{y, 1}")
    res = wc.run(_NS_REQ)
    assert res["ok"] is True
    assert res["cert"]["kind"] == ns_mod.KIND
    assert ns_mod.check(res["cert"])["valid"] is True


def test_sturm_valid_reply_is_accepted_by_the_real_checker(monkeypatch):
    # x^2 - 1 has the distinct roots -1 and 1 in (-2, 2].
    _engine(monkeypatch, "2")
    res = wc.run(_STURM_REQ)
    assert res["ok"] is True
    assert res["cert"]["steps"][0]["root_count"] == 2
    assert sturm_mod.check(res["cert"])["valid"] is True


# --------------------------------------------------------------------------- #
# 3. Canned TAMPERED replies -> no certificate, checker's rejection reported.
# --------------------------------------------------------------------------- #

def test_sos_wrong_square_is_refused(monkeypatch):
    # (1 + 2x)^2 != x^2 + 2x + 1.
    _engine(monkeypatch, "{{1 + 2*x}, {}}")
    res = wc.run(_SOS_REQ)
    assert res["ok"] is False
    assert res["cert"] is None
    assert res["checked"] is True
    assert res["check"]["valid"] is False
    assert "REJECTED" in res["reason"]
    assert "SOS identity" in res["check"]["reason"]


def test_nullstellensatz_wrong_cofactor_is_refused(monkeypatch):
    # y*(x-1) + 2*(y-1) != x*y - 1.
    _engine(monkeypatch, "{y, 2}")
    res = wc.run(_NS_REQ)
    assert res["ok"] is False
    assert res["cert"] is None
    assert res["check"]["valid"] is False
    assert "cofactor identity FAILS" in res["check"]["reason"]


def test_sturm_wrong_count_is_refused(monkeypatch):
    # Wolfram claims 1 root; the checker re-derives 2 from the Sturm chain.
    _engine(monkeypatch, "1")
    res = wc.run(_STURM_REQ)
    assert res["ok"] is False
    assert res["cert"] is None
    assert res["check"]["valid"] is False
    assert "root count mismatch" in res["check"]["reason"]


# --------------------------------------------------------------------------- #
# 4. Inexact floats are rejected before reaching any checker.
# --------------------------------------------------------------------------- #

@pytest.mark.parametrize("op_request,reply", [
    (_SOS_REQ, "{{1.5 + x}, {}}"),
    (_NS_REQ, "{1.0*y, 1}"),
])
def test_float_coefficients_are_rejected(monkeypatch, op_request, reply):
    _engine(monkeypatch, reply)
    res = wc.run(op_request)
    assert res["ok"] is False
    assert res["cert"] is None
    assert res["checked"] is False       # refused before any checker ran
    assert "inexact float" in res["reason"]


def test_float_root_count_is_rejected(monkeypatch):
    _engine(monkeypatch, "2.0")
    res = wc.run(_STURM_REQ)
    assert res["ok"] is False
    assert res["cert"] is None
    assert "inexact" in res["reason"]


# --------------------------------------------------------------------------- #
# 5. Engine-level and shape failures never leak a certificate.
# --------------------------------------------------------------------------- #

def test_engine_error_yields_no_cert(monkeypatch):
    _engine(monkeypatch, None, ok=False)
    res = wc.run(_SOS_REQ)
    assert res["ok"] is False and res["cert"] is None
    assert "wolfram evaluation failed" in res["reason"]


def test_wolfram_failed_token_yields_no_cert(monkeypatch):
    _engine(monkeypatch, "$Failed")
    res = wc.run(_NS_REQ)
    assert res["ok"] is False and res["cert"] is None


def test_mid_request_unavailability_yields_no_cert(monkeypatch):
    monkeypatch.setattr(wc, "available", lambda: True)
    monkeypatch.setattr(wc, "evaluate", lambda code, timeout=30.0: {
        "ok": False, "result": None, "error": None, "unavailable": True})
    res = wc.run(_STURM_REQ)
    assert res["ok"] is False and res["cert"] is None
    assert "unavailable" in res["reason"]


def test_cofactor_arity_mismatch_is_refused(monkeypatch):
    _engine(monkeypatch, "{y}")          # one cofactor for two generators
    res = wc.run(_NS_REQ)
    assert res["ok"] is False and res["cert"] is None
    assert "cofactor" in res["reason"]


def test_malformed_wolfram_list_is_refused(monkeypatch):
    _engine(monkeypatch, "not a list at all")
    res = wc.run(_SOS_REQ)
    assert res["ok"] is False and res["cert"] is None


# --------------------------------------------------------------------------- #
# 6. Structural guarantee: a cert is returned only alongside a passing verdict.
# --------------------------------------------------------------------------- #

@pytest.mark.parametrize("request_,reply", [
    (_SOS_REQ, "{{1 + x}, {}}"),
    (_SOS_REQ, "{{1 + 2*x}, {}}"),
    (_NS_REQ, "{y, 1}"),
    (_NS_REQ, "{y, 2}"),
    (_STURM_REQ, "2"),
    (_STURM_REQ, "1"),
])
def test_cert_present_iff_checker_accepted(monkeypatch, request_, reply):
    _engine(monkeypatch, reply)
    res = wc.run(request_)
    if res["cert"] is None:
        assert res["ok"] is False
    else:
        assert res["ok"] is True
        assert res["check"]["valid"] is True
        assert res["trusted_without_check"] is False


def test_no_wl_split_smuggles_scalars():
    with pytest.raises(Exception):
        wc._wl_split("3")
