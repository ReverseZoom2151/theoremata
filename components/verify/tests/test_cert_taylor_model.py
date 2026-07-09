"""Tests for the Taylor-model approximation-bound certificate.

Offline, deterministic, exact-precision.  Exports Taylor-model certs to the
``theoremata.cert-log.v1`` format (kind ``taylor_model``) and confirms the
self-contained reference checker (a) validates genuine certs and (b) REJECTS
tampered ones (too-tight remainder, wrong epsilon) — the soundness boundary.

``mpmath`` ships with sympy; guard anyway so the suite skips cleanly if absent.
"""
import copy
import sys
from pathlib import Path

import pytest

pytest.importorskip("mpmath")

# Put the verify component's python/ dir on the path (namespace package).
_ROOT = Path(__file__).resolve().parents[3]
for rel in ("components/verify/python",):
    p = str(_ROOT / rel)
    if p not in sys.path:
        sys.path.insert(0, p)

from theoremata_tools.cert_taylor_model import (  # noqa: E402
    FORMAT,
    KINDS,
    check,
    export_taylor_model_cert,
    residual_enclosure,
    run,
)


# --------------------------------------------------------------------------- #
# Helpers: compute a valid remainder Delta for a given model via the checker's
# OWN residual enclosure, so fixtures are honest (Delta really encloses what the
# checker recomputes) yet TIGHT (mincing beats the dependency problem).
# --------------------------------------------------------------------------- #

def _residual_bound(func, coeffs, a, b, z0=0):
    """A safe, tight scalar bound on |f - T| over [a,b] (widened a hair)."""
    lo, hi = residual_enclosure(func, coeffs, [str(a), str(b)],
                                expansion_point=z0)
    m = max(abs(float(lo)), abs(float(hi)))
    return m * 1.05 + 1e-12


# --------------------------------------------------------------------------- #
# Valid certificates export + check True.
# --------------------------------------------------------------------------- #

def test_polynomial_exact_zero_remainder():
    """A polynomial equal to its own Taylor model: residual is exactly 0."""
    # f(x) = 2 + 3x - x^2 ; T identical ; Delta = 0 exactly ; epsilon = 0.
    coeffs = ["2", "3", "-1"]
    log = export_taylor_model_cert(
        func="poly", coeffs=coeffs, func_coeffs=coeffs,
        remainder=["0", "0"], domain=["-2", "2"], epsilon="0",
    )
    assert log["format"] == FORMAT and log["kind"] == "taylor_model"
    assert log["kind"] in KINDS
    res = check(log)
    assert res["valid"] is True, res["reason"]


def test_polynomial_exact_with_shift():
    """Same idea about a non-zero expansion point z0=1 (exact residual = 0)."""
    fc = ["1", "0", "0", "1"]           # f(x) = 1 + x^3, in powers of x
    # T re-expressed about z0 = 1: 1 + x^3 = 2 + 3u + 3u^2 + u^3, u = x-1
    Tc = ["2", "3", "3", "1"]
    log = export_taylor_model_cert(
        func="poly", coeffs=Tc, func_coeffs=fc,
        remainder=["0", "0"], domain=["0", "2"], epsilon="0",
        expansion_point="1",
    )
    assert check(log)["valid"] is True


def test_exp_taylor_model_valid():
    """exp on [-1/2, 1/2], degree-6 Taylor about 0, honest Delta and epsilon."""
    coeffs = ["1", "1", "1/2", "1/6", "1/24", "1/120", "1/720"]
    bound = _residual_bound("exp", [eval_frac(c) for c in coeffs], -0.5, 0.5)
    log = export_taylor_model_cert(
        func="exp", coeffs=coeffs,
        remainder=[str(-bound), str(bound)],
        domain=["-1/2", "1/2"], epsilon=str(bound * 1.1),
    )
    res = check(log)
    assert res["valid"] is True, res["reason"]


def test_sin_taylor_model_valid():
    """sin on [-1/4, 1/4], degree-5 Taylor about 0."""
    coeffs = ["0", "1", "0", "-1/6", "0", "1/120"]
    bound = _residual_bound("sin", [eval_frac(c) for c in coeffs], -0.25, 0.25)
    log = export_taylor_model_cert(
        func="sin", coeffs=coeffs,
        remainder=[str(-bound), str(bound)],
        domain=["-1/4", "1/4"], epsilon=str(bound * 1.1),
    )
    assert check(log)["valid"] is True


def test_modified_taylor_model_valid():
    """Modified (relative-remainder) variant: residual = (x-z0)^{n+1} * g.

    Uses exp about 0 with order n so the certified enclosure is
    (x)^{n+1} * Delta.  A Delta bounding g = residual/x^{n+1} is honest here and
    the checker verifies the factored enclosure.
    """
    from mpmath import iv, mpf
    iv.dps = 50
    coeffs = ["1", "1", "1/2"]          # order n = 2
    n = 2
    X = iv.mpf(["-1/4", "1/4"])
    T = iv.mpf(0)
    for c in reversed(coeffs):
        T = T * X + iv.mpf(str(c))
    R = iv.exp(X) - T
    Xp = X ** (n + 1)
    # g = R / x^{n+1}; bound |g| by |R|_max / min|x^{n+1}| is unstable near 0,
    # so bound g directly via the analytic tail: g in [1/(n+1)! - .., ..].
    # Simplest honest Delta: enclose g by dividing the interval residual by the
    # interval x^{n+1} using a domain excluding 0 is unnecessary — instead pick
    # Delta wide enough that (x)^{n+1} * Delta still encloses R over all x.
    gbound = float(max(abs(mpf(R.a)), abs(mpf(R.b)))) / float(
        min(abs(mpf((iv.mpf("1/4") ** (n + 1)).a)),
            abs(mpf((iv.mpf("1/4") ** (n + 1)).b)))) * 1.2 + 1e-6
    # epsilon must cover (x)^{n+1} * Delta over the whole domain:
    eps = gbound * float(mpf((iv.mpf("1/4") ** (n + 1)).b)) * 1.05 + 1e-9
    log = export_taylor_model_cert(
        func="exp", coeffs=coeffs, order=n, modified=True,
        remainder=[str(-gbound), str(gbound)],
        domain=["-1/4", "1/4"], epsilon=str(eps),
    )
    res = check(log)
    assert res["valid"] is True, res["reason"]
    assert log["meta"]["variant"] == "modified"


# --------------------------------------------------------------------------- #
# Tampered certificates are REJECTED (the soundness boundary).
# --------------------------------------------------------------------------- #

def _valid_exp_log():
    coeffs = ["1", "1", "1/2", "1/6", "1/24", "1/120", "1/720"]
    bound = _residual_bound("exp", [eval_frac(c) for c in coeffs], -0.5, 0.5)
    return export_taylor_model_cert(
        func="exp", coeffs=coeffs,
        remainder=[str(-bound), str(bound)],
        domain=["-1/2", "1/2"], epsilon=str(bound * 1.1),
    )


def test_reject_too_tight_remainder():
    """Shrinking Delta below the true residual is rejected."""
    log = _valid_exp_log()
    assert check(log)["valid"] is True
    tampered = copy.deepcopy(log)
    tampered["steps"][0]["remainder"] = ["-1/1000000000000", "1/1000000000000"]
    res = check(tampered)
    assert res["valid"] is False
    assert "too tight" in res["reason"] or "epsilon" in res["reason"]


def test_reject_wrong_epsilon():
    """Claiming an epsilon smaller than the (honest) remainder is rejected."""
    log = _valid_exp_log()
    tampered = copy.deepcopy(log)
    tampered["steps"][0]["epsilon"] = "1/1000000000000"
    res = check(tampered)
    assert res["valid"] is False
    assert "epsilon too small" in res["reason"]


def test_reject_truncated_polynomial():
    """Dropping a Taylor coefficient (worse approx) breaks the tight epsilon."""
    coeffs = ["1", "1", "1/2", "1/6", "1/24", "1/120", "1/720"]
    bound = _residual_bound("exp", [eval_frac(c) for c in coeffs], -0.5, 0.5)
    log = export_taylor_model_cert(
        func="exp", coeffs=coeffs,
        remainder=[str(-bound), str(bound)],
        domain=["-1/2", "1/2"], epsilon=str(bound * 1.1),
    )
    tampered = copy.deepcopy(log)
    # Zero out the linear term: T no longer approximates exp to epsilon.
    tampered["steps"][0]["coeffs"][1] = "0"
    assert check(tampered)["valid"] is False


def test_reject_bad_format_and_kind():
    log = _valid_exp_log()
    bad = copy.deepcopy(log); bad["format"] = "nope"
    assert check(bad)["valid"] is False
    bad2 = copy.deepcopy(log); bad2["kind"] = "lp_primal_dual"
    assert check(bad2)["valid"] is False


def test_reject_malformed_missing_conclusion():
    """A log with no assertion steps reaches no verified conclusion."""
    log = _valid_exp_log()
    stripped = copy.deepcopy(log)
    stripped["steps"] = [stripped["steps"][0]]   # drop the assert steps
    res = check(stripped)
    assert res["valid"] is False
    assert "conclusion" in res["reason"]


# --------------------------------------------------------------------------- #
# Determinism + worker dispatch.
# --------------------------------------------------------------------------- #

def test_determinism():
    """Re-checking the same log yields identical results (fixed precision)."""
    log = _valid_exp_log()
    r1 = check(log)
    r2 = check(log)
    assert r1 == r2 and r1["valid"] is True


def test_worker_run_export_then_check():
    coeffs = ["1", "1", "1/2", "1/6"]
    bound = _residual_bound("exp", [eval_frac(c) for c in coeffs], -0.2, 0.2)
    out = run({
        "op": "export", "func": "exp", "coeffs": coeffs,
        "remainder": [str(-bound), str(bound)],
        "domain": ["-1/5", "1/5"], "epsilon": str(bound * 1.1),
    })
    assert "log" in out
    res = run({"op": "check", "log": out["log"]})
    assert res["valid"] is True, res["reason"]


# --------------------------------------------------------------------------- #

def eval_frac(s):
    from fractions import Fraction
    return Fraction(s)
