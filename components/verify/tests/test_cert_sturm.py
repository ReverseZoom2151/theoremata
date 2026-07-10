"""Tests for the Sturm real-root-count + polynomial-minimax certificates.

Offline, deterministic, exact rational arithmetic.  Exercises both cert-log
kinds emitted by :mod:`theoremata_tools.cert_sturm`:

* ``sturm`` — distinct real-root count of a squarefree polynomial on ``(a, b]``
  via ``V(a) - V(b)``; the checker RE-DERIVES the Sturm chain from ``p`` alone
  and rejects a tampered chain or a wrong count (the soundness boundary).
* ``poly_minimax`` — composite ``|f - p| <= K`` on ``[a, b]`` (Taylor bound +
  Sturm no-crossing); a too-small ``K`` is rejected.

``mpmath`` (needed only by the ``poly_minimax`` Taylor sub-check) ships with
sympy; guard anyway so the suite skips cleanly if absent.
"""
import copy
import sys
from pathlib import Path

import pytest

pytest.importorskip("sympy")   # cert stack assumes the sympy/mpmath dep is present
pytest.importorskip("mpmath")

# Put the verify component's python/ dir on the path (namespace package).
_ROOT = Path(__file__).resolve().parents[3]
for rel in ("components/verify/python",):
    p = str(_ROOT / rel)
    if p not in sys.path:
        sys.path.insert(0, p)

from theoremata_tools.cert_sturm import (  # noqa: E402
    FORMAT,
    KINDS,
    check,
    export_poly_minimax_cert,
    export_sturm_cert,
    run,
)


# --------------------------------------------------------------------------- #
# Kind ``sturm``: valid root counts.
# --------------------------------------------------------------------------- #

def test_x2_minus_2_one_root_on_0_2():
    """x^2 - 2 has exactly one root (sqrt 2) in (0, 2]."""
    log = export_sturm_cert(["-2", "0", "1"], ["0", "2"])
    assert log["format"] == FORMAT and log["kind"] == "sturm"
    assert log["kind"] in KINDS
    assert log["steps"][0]["root_count"] == 1
    res = check(log)
    assert res["valid"] is True, res["reason"]


def test_x2_minus_2_zero_roots_on_neg1_0():
    """x^2 - 2 has no root in (-1, 0] (its roots are +/- sqrt 2)."""
    log = export_sturm_cert(["-2", "0", "1"], ["-1", "0"])
    assert log["steps"][0]["root_count"] == 0
    assert check(log)["valid"] is True


def test_x3_minus_x_three_roots():
    """x^3 - x = x(x-1)(x+1) has three distinct roots in (-2, 2]."""
    log = export_sturm_cert(["0", "-1", "0", "1"], ["-2", "2"])
    assert log["steps"][0]["root_count"] == 3
    assert check(log)["valid"] is True


def test_x3_minus_x_one_root_on_half_2():
    """Only x = 1 lies in (1/2, 2] for x^3 - x."""
    log = export_sturm_cert(["0", "-1", "0", "1"], ["1/2", "2"])
    assert log["steps"][0]["root_count"] == 1
    assert check(log)["valid"] is True


def test_endpoint_root_rejected_at_export():
    """An endpoint that is a root makes the count on (a, b] ill-defined."""
    # p(0) = 0 for x^3 - x, so a = 0 is a root -> exporter refuses.
    with pytest.raises(ValueError):
        export_sturm_cert(["0", "-1", "0", "1"], ["0", "2"])


def test_non_squarefree_rejected_at_export():
    """(x-1)^2 = x^2 - 2x + 1 is not squarefree -> exporter refuses."""
    with pytest.raises(ValueError):
        export_sturm_cert(["1", "-2", "1"], ["-3", "3"])


# --------------------------------------------------------------------------- #
# Kind ``sturm``: tampering is REJECTED (the soundness boundary).
# --------------------------------------------------------------------------- #

def _valid_sturm_log():
    return export_sturm_cert(["0", "-1", "0", "1"], ["-2", "2"])   # 3 roots


def test_reject_tampered_chain():
    """Corrupting a Sturm-chain entry no longer matches the chain from p."""
    log = _valid_sturm_log()
    assert check(log)["valid"] is True
    tampered = copy.deepcopy(log)
    # steps[1] is the sturm_chain step; perturb the derivative entry (chain[1]).
    tampered["steps"][1]["chain"][1] = ["0", "1"]   # bogus p' = x
    res = check(tampered)
    assert res["valid"] is False
    assert "chain" in res["reason"]


def test_reject_wrong_root_count():
    """A doctored root_count that disagrees with V(a) - V(b) is rejected."""
    log = _valid_sturm_log()
    tampered = copy.deepcopy(log)
    tampered["steps"][0]["root_count"] = 2      # true count is 3
    res = check(tampered)
    assert res["valid"] is False
    assert "root count" in res["reason"]


def test_reject_wrong_sign_variations():
    """A doctored sign-variation count is caught before the root-count step."""
    log = _valid_sturm_log()
    tampered = copy.deepcopy(log)
    tampered["steps"][2]["at_a"] = tampered["steps"][2]["at_a"] + 1
    res = check(tampered)
    assert res["valid"] is False
    assert "sign variations" in res["reason"]


def test_reject_bad_format_and_kind():
    log = _valid_sturm_log()
    bad = copy.deepcopy(log); bad["format"] = "nope"
    assert check(bad)["valid"] is False
    bad2 = copy.deepcopy(log); bad2["kind"] = "lp_primal_dual"
    assert check(bad2)["valid"] is False


def test_reject_missing_conclusion():
    """Dropping the assert steps reaches no verified conclusion."""
    log = _valid_sturm_log()
    stripped = copy.deepcopy(log)
    stripped["steps"] = stripped["steps"][:3]   # drop assert_chain/assert_root_count
    res = check(stripped)
    assert res["valid"] is False
    assert "conclusion" in res["reason"]


# --------------------------------------------------------------------------- #
# Kind ``poly_minimax``: composite |f - p| <= K.
# --------------------------------------------------------------------------- #

def _poly_minimax_log(K="1/2"):
    """f(x) = x (exact, delta = 0), T = x, p = x + 1/10 on [0, 1]."""
    return export_poly_minimax_cert(
        func="poly", t_coeffs=["0", "1"], p_coeffs=["1/10", "1"],
        domain=["0", "1"], K=K, delta="0", func_coeffs=["0", "1"],
    )


def test_poly_minimax_valid():
    """|x - (x + 1/10)| = 1/10 <= 1/2, so the composite bound holds."""
    log = _poly_minimax_log(K="1/2")
    assert log["format"] == FORMAT and log["kind"] == "poly_minimax"
    res = check(log)
    assert res["valid"] is True, res["reason"]


def test_poly_minimax_valid_exact_equal():
    """p == f exactly: residual is 0, any positive K holds."""
    log = export_poly_minimax_cert(
        func="poly", t_coeffs=["0", "1"], p_coeffs=["0", "1"],
        domain=["0", "1"], K="1/1000", delta="0", func_coeffs=["0", "1"],
    )
    assert check(log)["valid"] is True


def test_poly_minimax_too_small_K_rejected():
    """K = 1/20 < |f - p| = 1/10: the no-crossing check must reject."""
    log = _poly_minimax_log(K="1/20")
    res = check(log)
    assert res["valid"] is False
    assert "K too small" in res["reason"] or "no-crossing" in res["reason"]


def test_poly_minimax_tampered_p_rejected():
    """Widening p beyond K breaks the no-crossing argument."""
    log = _poly_minimax_log(K="1/2")
    tampered = copy.deepcopy(log)
    tampered["steps"][0]["p_coeffs"] = ["1", "1"]   # p = x + 1, residual 1 > 1/2
    res = check(tampered)
    assert res["valid"] is False


def test_poly_minimax_tampered_taylor_rejected():
    """Corrupting the embedded Taylor sub-cert is caught by re-checking it."""
    log = _poly_minimax_log(K="1/2")
    tampered = copy.deepcopy(log)
    # Make T disagree with f so the Taylor residual no longer fits epsilon = 0.
    tampered["steps"][1]["taylor"]["steps"][0]["coeffs"] = ["5", "1"]
    res = check(tampered)
    assert res["valid"] is False


# --------------------------------------------------------------------------- #
# Determinism, round-trip, worker dispatch.
# --------------------------------------------------------------------------- #

def test_determinism_sturm():
    log = _valid_sturm_log()
    assert check(log) == check(log)


def test_determinism_export():
    """Exporting the same inputs twice yields identical documents."""
    a = export_sturm_cert(["-2", "0", "1"], ["0", "2"])
    b = export_sturm_cert(["-2", "0", "1"], ["0", "2"])
    assert a == b


def test_round_trip_sturm_via_worker():
    out = run({"op": "export", "kind": "sturm",
               "coeffs": ["-2", "0", "1"], "interval": ["0", "2"]})
    assert "log" in out
    res = run({"op": "check", "log": out["log"]})
    assert res["valid"] is True, res["reason"]


def test_round_trip_poly_minimax_via_worker():
    out = run({"op": "export", "kind": "poly_minimax", "func": "poly",
               "t_coeffs": ["0", "1"], "p_coeffs": ["1/10", "1"],
               "func_coeffs": ["0", "1"], "domain": ["0", "1"],
               "K": "1/2", "delta": "0"})
    assert "log" in out
    res = run({"op": "check", "log": out["log"]})
    assert res["valid"] is True, res["reason"]
