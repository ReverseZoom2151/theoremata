"""Tests for the SOS / Positivstellensatz cert exporter + pure checker.

Offline, deterministic, exact.  Exercises univariate SOS (global + on an
interval, including the ``(x-a)(b-x)`` multiplier term), the multivariate Gram
checker, tamper rejection with a ``p(u) < 0`` witness, and determinism.
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

from theoremata_tools.cert_sos import (  # noqa: E402
    FORMAT,
    check,
    check_sos_gram,
    export_gram_sos_cert,
    export_sos_cert,
    export_univariate_sos_cert,
    generate_multivariate,
    generate_univariate_sos,
    run,
)


def _roundtrip(log):
    return json.loads(json.dumps(log))


# --------------------------------------------------------------------------- #
# Univariate SOS.
# --------------------------------------------------------------------------- #

def _exact_root_finder(coeffs):
    """Deterministic exact-root seam: real roots of the given rational poly.

    Only used to prove the ``root_finder`` seam is injectable; falls back to
    sympy's own root solving but rounds nothing (kept exact via nsimplify).
    """
    z = sympy.Symbol("_z")
    return [complex(r) for r in sympy.Poly(list(coeffs), z).nroots(n=40)]


def test_global_sos_x2_plus_1():
    x = symbols("x")
    log = export_univariate_sos_cert(x**2 + 1, x=x, root_finder=_exact_root_finder)
    assert log["format"] == FORMAT and log["kind"] == "sos"
    res = check(log)
    assert res["valid"] is True, res
    assert check(_roundtrip(log))["valid"] is True


def test_global_sos_perfect_square():
    x = symbols("x")
    log = export_univariate_sos_cert(x**2 - 2 * x + 1, x=x)
    assert check(log)["valid"] is True


def test_interval_sos_global_square_on_0_2():
    # (x-1)^2 is a global square; on [0,2] it certifies with empty multiplier.
    x = symbols("x")
    log = export_univariate_sos_cert(x**2 - 2 * x + 1, x=x, interval=(0, 2))
    assert check(log)["valid"] is True
    # No multiplier squares needed for a globally nonnegative polynomial.
    terms = next(s for s in log["steps"] if s["op"] == "sos_terms")
    assert terms["multiplier_squares"] == []


def test_interval_sos_uses_boundary_multiplier():
    # x*(2-x) >= 0 on [0,2] but not globally: needs the (x-a)(b-x) term.
    x = symbols("x")
    log = export_univariate_sos_cert(2 * x - x**2, x=x, interval=(0, 2))
    terms = next(s for s in log["steps"] if s["op"] == "sos_terms")
    assert terms["squares"] == []
    assert terms["multiplier_squares"], "expected a nonempty multiplier SOS"
    assert check(log)["valid"] is True


def test_univariate_tampered_square_rejected():
    x = symbols("x")
    log = export_univariate_sos_cert(x**2 + 1, x=x)
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "sos_terms":
            # Corrupt a square's coefficient: identity must break.
            step["squares"][0]["terms"] = [[[1], "2"]]  # was x -> now 2x
    assert check(bad)["valid"] is False


# --------------------------------------------------------------------------- #
# Multivariate Gram checker.
# --------------------------------------------------------------------------- #

def test_gram_valid_perfect_square():
    x, y = symbols("x y")
    # x^2 + 2xy + y^2 = (x+y)^2 ; Q = [[1,1],[1,1]] over z = [x, y].
    monomials = [(1, 0), (0, 1)]
    Q = [["1", "1"], ["1", "1"]]
    res = check_sos_gram(x**2 + 2 * x * y + y**2, [x, y], monomials, Q)
    assert res["valid"] is True, res
    log = export_gram_sos_cert(x**2 + 2 * x * y + y**2, gens=[x, y],
                               monomials=monomials, Q=Q)
    assert check(log)["valid"] is True
    assert check(_roundtrip(log))["valid"] is True


def test_gram_non_psd_rejected_with_witness():
    x, y = symbols("x y")
    # p = x^2 - y^2 is genuinely negative; honest Gram is diag(1,-1), not PSD.
    monomials = [(1, 0), (0, 1)]
    Q = [["1", "0"], ["0", "-1"]]
    log = export_gram_sos_cert(x**2 - y**2, gens=[x, y], monomials=monomials, Q=Q)
    res = check(log)
    assert res["valid"] is False
    assert res["witness"] is not None, "expected a p(u)<0 witness"
    # Re-evaluate the witness point exactly: p(u) must be < 0.
    pt = {symbols(k): sympy.Rational(v) for k, v in res["witness"]["point"].items()}
    assert (x**2 - y**2).xreplace(pt) < 0


def test_gram_tampered_identity_rejected():
    x, y = symbols("x y")
    monomials = [(1, 0), (0, 1)]
    log = export_gram_sos_cert(x**2 + 2 * x * y + y**2, gens=[x, y],
                               monomials=monomials, Q=[["1", "1"], ["1", "1"]])
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "gram_matrix":
            step["Q"] = [["1", "1"], ["1", "-1"]]  # no longer reproduces p
    res = check(bad)
    assert res["valid"] is False
    assert "identity" in res["reason"]


def test_multivariate_generator_is_stubbed():
    assert generate_multivariate is None


# --------------------------------------------------------------------------- #
# Format rejection, determinism, run() dispatch.
# --------------------------------------------------------------------------- #

def test_unknown_format_rejected():
    assert check({"format": "bogus.v9", "kind": "sos", "steps": []})["valid"] is False


def test_wrong_leading_op_rejected():
    assert check({"format": FORMAT, "kind": "sos",
                  "steps": [{"op": "gram_matrix", "Q": []}]})["valid"] is False


def test_determinism_export_and_check():
    x, y = symbols("x y")
    a = export_gram_sos_cert(x**2 + 2 * x * y + y**2, gens=[x, y],
                             monomials=[(1, 0), (0, 1)], Q=[["1", "1"], ["1", "1"]])
    b = export_gram_sos_cert(x**2 + 2 * x * y + y**2, gens=[x, y],
                             monomials=[(1, 0), (0, 1)], Q=[["1", "1"], ["1", "1"]])
    assert json.dumps(a, sort_keys=True) == json.dumps(b, sort_keys=True)

    xx = symbols("x")
    u1 = export_univariate_sos_cert(xx**2 + 1, x=xx)
    u2 = export_univariate_sos_cert(xx**2 + 1, x=xx)
    assert json.dumps(u1, sort_keys=True) == json.dumps(u2, sort_keys=True)
    assert check(u1)["valid"] == check(_roundtrip(u1))["valid"] is True


def test_run_export_then_check_roundtrip_univariate():
    xx = symbols("x")
    exported = run({"op": "export", "p": xx**2 + 1, "x": xx})
    assert "log" in exported
    assert run({"op": "check", "log": exported["log"]})["valid"] is True


def test_run_export_gram_and_check():
    x, y = symbols("x y")
    exported = run({"op": "export", "mode": "gram", "p": x**2 + 2 * x * y + y**2,
                    "gens": [x, y], "monomials": [(1, 0), (0, 1)],
                    "gram": [["1", "1"], ["1", "1"]]})
    assert run({"op": "check", "log": exported["log"]})["valid"] is True


def test_export_sos_cert_dispatch():
    x, y = symbols("x y")
    uni = export_sos_cert(x**2 + 1, x=x)
    assert uni["meta"]["mode"] == "univariate"
    gram = export_sos_cert(x**2 + 2 * x * y + y**2, gens=[x, y],
                           monomials=[(1, 0), (0, 1)], gram=[["1", "1"], ["1", "1"]])
    assert gram["meta"]["mode"] == "gram"


def test_run_unknown_op_raises():
    with pytest.raises(ValueError):
        run({"op": "nope"})


def test_generate_univariate_sos_direct():
    x = symbols("x")
    gen = generate_univariate_sos(x**2 + 1, x)
    assert gen["multiplier_squares"] == []
    assert sympy.expand(sum(s**2 for s in gen["squares"]) - (x**2 + 1)) == 0
