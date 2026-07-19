"""Tests for the explicit-bound candidate generator :mod:`theoremata_tools.minimax_bounds`.

Two things are under test, and they are different in kind:

1.  **Transcription fidelity.**  The bounds were copied out of prose notes, so each
    formula is pinned against an independently computed value: for ``f(x) = x**2``
    the Bernstein error is known in closed form, so the Lorentz bound can be compared
    with the truth rather than with itself.
2.  **The trust boundary.**  A candidate ``K`` is a claim.  The tests below feed
    candidates into ``cert_sturm``'s real exporter/checker and assert that the checker
    is the one deciding, including one case where a formula-correct ``K`` is REJECTED
    because the checker demands a strict inequality the (attained) bound does not
    give.  That rejection is the point, not a defect.
"""
import sys
from fractions import Fraction
from math import factorial
from pathlib import Path

import pytest

pytest.importorskip("sympy")
pytest.importorskip("mpmath")

# Put the verify component's python/ dir on the path (namespace package).
_ROOT = Path(__file__).resolve().parents[3]
for rel in ("components/verify/python",):
    _p = str(_ROOT / rel)
    if _p not in sys.path:
        sys.path.insert(0, _p)

from theoremata_tools.cert_sturm import (  # noqa: E402
    check,
    export_poly_minimax_cert,
)
from theoremata_tools.minimax_bounds import (  # noqa: E402
    BoundCandidate,
    bernstein_bound_holder,
    bernstein_bound_holder_derivative,
    bernstein_bound_lipschitz,
    bernstein_bound_lipschitz_derivative,
    bernstein_monomial_coeffs,
    bernstein_nodes,
    describe_available_bounds,
    poly_minimax_candidate,
    run,
    sikkema_constant,
    _pow_bound,
)


# --------------------------------------------------------------------------- #
# Rational bounds for irrational powers: direction is the whole point.
# --------------------------------------------------------------------------- #

def test_pow_bound_rounds_in_the_stated_direction():
    up = _pow_bound(Fraction(2), 1, 2, upper=True)
    lo = _pow_bound(Fraction(2), 1, 2, upper=False)
    assert up ** 2 >= 2
    assert lo ** 2 <= 2
    assert lo < up
    assert up - lo < Fraction(1, 10 ** 20)


def test_pow_bound_handles_fractional_exponents():
    # 8 ** (2/3) == 4 exactly; both directions must bracket it tightly.
    up = _pow_bound(Fraction(8), 2, 3, upper=True)
    lo = _pow_bound(Fraction(8), 2, 3, upper=False)
    assert lo <= 4 <= up


def test_sikkema_constant_matches_the_decimal_quoted_in_the_source():
    # bernapprox.md quotes (4306+837*sqrt(6))/5832 < 1.08989.  If the transcription
    # of either integer were wrong, this window would almost certainly miss.
    c = sikkema_constant()
    assert Fraction(108988, 100000) < c < Fraction(108989, 100000)


# --------------------------------------------------------------------------- #
# Transcription fidelity against a closed-form Bernstein error.
# --------------------------------------------------------------------------- #

def _bernstein_of_square(n: int, domain=(0, 1)) -> list[Fraction]:
    """Monomial coefficients of B_n(x**2) on ``domain``, built by the module."""
    nodes = bernstein_nodes(n, domain)
    return bernstein_monomial_coeffs([x * x for x in nodes], domain)


def test_bernstein_of_x_squared_is_the_known_closed_form():
    # B_n(x**2)(x) = x**2 + x(1-x)/n on [0, 1].  A basis-conversion bug would show up
    # here immediately, and a wrong p is a wrong certificate no matter how good K is.
    n = 4
    got = _bernstein_of_square(n)
    want = [Fraction(0), Fraction(1, n), Fraction(1) - Fraction(1, n)]
    assert got == want


def test_lorentz_bound_equals_the_true_sup_error_for_x_squared():
    # f' = 2x is Lipschitz with constant 2, so the row gives K = 2/(8n) = 1/(4n).
    # The true sup of |f - B_n(f)| is max x(1-x)/n = 1/(4n).  The bound is attained.
    n = 4
    bound = bernstein_bound_lipschitz_derivative(2, n)
    assert bound.K == Fraction(1, 4 * n)


def test_interval_rescaling_matches_the_true_error_on_a_wider_interval():
    # g(x) = x**2 on [1, 3]: g' Lipschitz with constant 2, (b-a)**2 = 4, so
    # K = 2*4/(8n) = 1/n.  Truly, B_n(g) - g = (x-1)(3-x)/n, whose sup is 1/n.
    n = 5
    bound = bernstein_bound_lipschitz_derivative(2, n, (1, 3))
    assert bound.K == Fraction(1, n)
    coeffs = _bernstein_of_square(n, (1, 3))
    # (x-1)(3-x)/n + x**2 = -3/n + (4/n)x + (1 - 1/n)x**2
    assert coeffs == [Fraction(-3, n), Fraction(4, n), Fraction(1) - Fraction(1, n)]


def test_holder_bounds_reduce_to_their_lipschitz_special_cases():
    n = 9
    # Kac at alpha = 1 is H0*sqrt(1/(4n)) = H0/(2*sqrt(n)); n = 9 makes it exact.
    kac = bernstein_bound_holder(6, 1, n)
    assert abs(kac.K - Fraction(1)) < Fraction(1, 10 ** 20)
    # Schurer-Steutel at alpha = 1 is H1/(4n); exact, no root involved.
    ss = bernstein_bound_holder_derivative(8, 1, n)
    assert abs(ss.K - Fraction(8, 4 * n)) < Fraction(1, 10 ** 20)


def test_lipschitz_bound_prefers_the_smaller_of_the_two_source_rows():
    b = bernstein_bound_lipschitz(1, 16)
    assert "kac1938_alpha1" in b.bound_kind        # 1/2 beats Sikkema's 1.08989
    assert abs(b.K - Fraction(1, 8)) < Fraction(1, 10 ** 20)


def test_lipschitz_bound_scales_linearly_with_interval_width():
    narrow = bernstein_bound_lipschitz(1, 16, (0, 1))
    wide = bernstein_bound_lipschitz(1, 16, (0, 3))
    assert wide.K == 3 * narrow.K


# --------------------------------------------------------------------------- #
# Hypotheses we cannot machine-check must be visible, and nothing is ever
# self-declared verified.
# --------------------------------------------------------------------------- #

def test_every_bound_records_its_unverifiable_hypotheses():
    for bound in (bernstein_bound_lipschitz_derivative(2, 4),
                  bernstein_bound_holder_derivative(1, 1, 4),
                  bernstein_bound_holder(1, Fraction(1, 2), 4),
                  bernstein_bound_lipschitz(1, 4)):
        assert bound.assumptions, f"{bound.bound_kind} declared no assumptions"
        assert any(a.startswith("UNCHECKED:") for a in bound.assumptions)
        assert bound.verified is False
        assert bound.as_dict()["verified"] is False


def test_candidate_cannot_be_marked_verified():
    bound = bernstein_bound_lipschitz_derivative(2, 4)
    with pytest.raises((AttributeError, TypeError)):
        bound.verified = True                     # frozen dataclass, non-init field


def test_node_slack_widens_K_and_names_why():
    n = 4
    bound = bernstein_bound_lipschitz_derivative(2, n)
    nodes = bernstein_nodes(n)
    cand = poly_minimax_candidate(bound, [x * x for x in nodes],
                                  node_slack=Fraction(1, 1000))
    assert Fraction(cand["K"]) == bound.K + Fraction(1, 1000)
    assert any("node value" in a for a in cand["assumptions"])
    assert cand["verified"] is False
    assert "CANDIDATE" in cand["status"]


def test_bad_inputs_are_refused_rather_than_silently_coerced():
    with pytest.raises(ValueError):
        bernstein_bound_lipschitz_derivative(-1, 4)          # negative constant
    with pytest.raises(ValueError):
        bernstein_bound_holder(1, 0, 4)                      # alpha out of range
    with pytest.raises(ValueError):
        bernstein_bound_holder(1, 2, 4)
    with pytest.raises(ValueError):
        bernstein_bound_lipschitz_derivative(1, 0)           # degree too small
    with pytest.raises(ValueError):
        bernstein_bound_lipschitz_derivative(1, 4, (2, 1))   # empty domain
    with pytest.raises(ValueError):
        poly_minimax_candidate(bernstein_bound_lipschitz_derivative(2, 4), [0, 1])
    with pytest.raises(TypeError):
        poly_minimax_candidate("not a bound", [0, 1])        # type: ignore[arg-type]


# --------------------------------------------------------------------------- #
# The trust boundary: cert_sturm.check decides, not the formula.
# --------------------------------------------------------------------------- #

def _square_cert(K, n=4, delta="0"):
    """A ``poly_minimax`` log for ``f = x**2`` vs its Bernstein approximant."""
    nodes = bernstein_nodes(n)
    p_coeffs = [str(c) for c in bernstein_monomial_coeffs([x * x for x in nodes])]
    f_coeffs = ["0", "0", "1"]
    return export_poly_minimax_cert(
        "poly", f_coeffs, p_coeffs, ["0", "1"], str(K),
        delta=delta, func_coeffs=f_coeffs)


def test_attained_candidate_K_is_rejected_by_the_checker():
    # The Lorentz bound is TIGHT for x**2: |f - p| equals K at x = 1/2.  cert_sturm's
    # no-crossing step needs a strict inequality, so a formula-correct candidate is
    # still rejected.  This is the behaviour we want: the formula does not get a vote.
    n = 4
    bound = bernstein_bound_lipschitz_derivative(2, n)
    res = check(_square_cert(bound.K, n))
    assert res["valid"] is False
    assert "no-crossing" in res["reason"]


def test_candidate_K_with_strict_slack_is_accepted_by_the_checker():
    n = 4
    bound = bernstein_bound_lipschitz_derivative(2, n)
    nodes = bernstein_nodes(n)
    cand = poly_minimax_candidate(bound, [x * x for x in nodes],
                                  strict_slack=Fraction(1, 1000))
    res = check(_square_cert(cand["K"], n))
    assert res["valid"] is True, res["reason"]
    # And the candidate's own polynomial is the one that got certified.
    assert cand["p_coeffs"] == [
        str(c) for c in bernstein_monomial_coeffs([x * x for x in nodes])]


def test_a_candidate_below_the_true_error_is_rejected():
    # Halving a correct K makes a false claim; the checker must catch it unaided.
    n = 4
    bound = bernstein_bound_lipschitz_derivative(2, n)
    res = check(_square_cert(bound.K / 2, n))
    assert res["valid"] is False


def test_exp_candidate_from_rounded_samples_round_trips_through_the_checker():
    # End to end on a non-polynomial f: rounded rational samples of exp, the Lorentz
    # bound plus the Note-1 rounding slack for K, a Taylor sub-certificate for the
    # exp side, and cert_sturm doing the actual verifying.
    import mpmath

    n = 4
    scale = 10 ** 6
    nodes = bernstein_nodes(n)
    with mpmath.workdps(40):
        values = []
        for x in nodes:
            v = mpmath.exp(mpmath.mpf(x.numerator) / x.denominator)
            values.append(Fraction(int(mpmath.floor(v * scale)), scale))
    # Flooring each sample loses at most 1/scale, which is exactly the Note-1 slack.
    node_slack = Fraction(1, scale)
    # exp'' is bounded by e on [0, 1], so exp' is Lipschitz with constant e; use a
    # rational strictly above e so the hypothesis is not the thing that fails.
    L1 = Fraction("2.7182818284590456")
    bound = bernstein_bound_lipschitz_derivative(L1, n)
    cand = poly_minimax_candidate(bound, values, node_slack=node_slack)

    # Degree-8 Taylor polynomial of exp at 0.  Its true remainder on [0, 1] is at
    # most e/9! < 3.1e-6, but the checker re-derives the remainder with interval
    # arithmetic whose spread is governed by the subdivision width, so delta has to
    # be loose enough for that recomputation, not merely true.  1/64 with 1024
    # subdivisions clears it, and still leaves K - delta well above the real error.
    # A dyadic delta also avoids the exporter pinning epsilon to the remainder and
    # then losing to outward rounding on a non-dyadic value such as 1/100.
    t_coeffs = [str(Fraction(1, factorial(k))) for k in range(9)]
    delta = Fraction(1, 64)
    log = export_poly_minimax_cert("exp", t_coeffs, cand["p_coeffs"], ["0", "1"],
                                   cand["K"], delta=str(delta), subdivisions=1024)
    res = check(log)
    assert res["valid"] is True, res["reason"]


# --------------------------------------------------------------------------- #
# Worker dispatch.
# --------------------------------------------------------------------------- #

def test_run_catalogue_lists_every_bound_with_its_required_inputs():
    cat = run({"op": "catalogue"})["bounds"]
    assert cat == describe_available_bounds()
    kinds = {b["kind"] for b in cat}
    assert kinds == {"bernstein_lipschitz_derivative", "bernstein_holder_derivative",
                     "bernstein_holder", "bernstein_lipschitz"}
    for entry in cat:
        assert entry["needs"] and entry["source"] and entry["formula"]


def test_run_returns_a_full_candidate_when_node_values_are_supplied():
    nodes = bernstein_nodes(4)
    out = run({"op": "lipschitz_derivative", "L1": 2, "n": 4,
               "node_values": [str(x * x) for x in nodes],
               "strict_slack": "1/1000"})
    assert out["verified"] is False
    assert Fraction(out["K"]) == Fraction(1, 16) + Fraction(1, 1000)
    assert out["p_coeffs"] == ["0", "1/4", "3/4"]


def test_run_rejects_an_unknown_op():
    with pytest.raises(ValueError):
        run({"op": "no_such_bound", "n": 4})


def test_bound_candidate_is_a_plain_value_object():
    b = bernstein_bound_lipschitz_derivative(2, 4)
    assert isinstance(b, BoundCandidate)
    assert b.degree == 4 and b.domain == (Fraction(0), Fraction(1))
    assert "bernapprox.md" in b.citation
