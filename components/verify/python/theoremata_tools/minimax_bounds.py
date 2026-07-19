"""Candidate ``K`` generation for ``poly_minimax`` certificates.

This module turns *explicit* (no hidden constants) polynomial approximation error
bounds into a **candidate** bound ``K`` for a concrete Bernstein approximant, so
that :func:`theoremata_tools.cert_sturm.export_poly_minimax_cert` has a defensible
number to certify instead of a guess pulled out of the air.

Why this exists
---------------
``cert_sturm``'s ``poly_minimax`` kind requires the caller to SUPPLY ``K``.  Nothing
in the pipeline suggested a value.  The bounds transcribed here compute one from a
function class (a Lipschitz or Hoelder constant), a degree ``n`` and an interval.

Trust boundary (read this before using anything below)
------------------------------------------------------
Everything this module returns is a **CLAIM, not a proof**.  ``BoundCandidate.verified``
is hard-wired to ``False`` and there is no code path that sets it otherwise.  The sole
authority on whether ``|f - p| <= K`` holds is :func:`theoremata_tools.cert_sturm.check`,
which re-derives the bound from the raw rationals and knows nothing about these
formulas.  A candidate that ``check`` rejects is simply wrong, no matter what the
formula said.  We deliberately do not import ``cert_sturm`` here: this module is a
*generator*, and generators must not be able to influence the checker.

Two further honesty constraints are enforced structurally rather than by convention:

*   Every bound's hypotheses (the Lipschitz/Hoelder constant of ``f`` or ``f'``, the
    accuracy of the sampled node values) are things we CANNOT machine-check from the
    rationals we hold.  They are therefore recorded verbatim in
    ``BoundCandidate.assumptions`` and propagated into the candidate dict, so an
    assumption can never be made silently.
*   Every constant is carried as an exact ``Fraction``.  Where a formula contains an
    irrational (``sqrt(6)``, ``n**(-3/4)``), we compute a rational bound in the
    *conservative* direction, so the returned ``K`` is never smaller than the formula's
    true value.  Rounding a bound the wrong way would silently weaken it.

Sources
-------
All bounds are transcribed from Peter Occil's public-domain notes, vendored at
``resources/peteroupc.github.io-master/``:

*   ``bernapprox.md``, section "Approximations by Polynomials" ->
    "Approximations on the Closed Unit Interval", the table whose columns are
    "If f(lambda) / Then the following polynomial / Is close to f with the following
    error bound".  Rows used: Lorentz (1964), Schurer and Steutel (1975), Kac (1938),
    Sikkema (1961).
*   ``bernapprox.md``, same section, Note 1 (rounding Bernstein coefficients to
    multiples of delta moves the polynomial by at most delta).
*   ``bernapprox.md``, section "Approximations Beyond the Closed Unit Interval",
    for the change of interval, including its explicit warning that the ``H_r``-based
    bounds do NOT transfer off the closed unit interval.

Those notes are released to the public domain (site ``LICENSE`` is the Unlicense;
each note carries its own "License" section releasing it to the public domain, with
CC0 as a fallback), so the formulas are quoted here without restriction.  The notes
are third-party data: they were read as prose and transcribed by hand, so each
function names the exact row it came from and a transcription error is a real risk.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from fractions import Fraction
from typing import Any, Iterable, Sequence

# Denominator used for the rational bounds we derive from irrational quantities.
# Large enough that the conservative rounding it forces is negligible next to any
# realistic K, small enough that the resulting rationals do not bloat the exact
# Sturm arithmetic downstream.
_ROUND_DEN = 10 ** 24


# --------------------------------------------------------------------------- #
# Exact rational bounds for irrational powers.
# --------------------------------------------------------------------------- #

def _iroot(n: int, k: int) -> int:
    """Floor of the ``k``-th root of a non-negative integer, exactly.

    Integer Newton iteration rather than ``n ** (1.0 / k)``: a float root of a large
    integer is off by enough to flip the direction we are rounding, which is the one
    error this module must not make.
    """
    if n < 0:
        raise ValueError("negative radicand")
    if k < 1:
        raise ValueError("root order must be at least 1")
    if n == 0:
        return 0
    x = 1 << ((n.bit_length() + k - 1) // k)
    while True:
        y = ((k - 1) * x + n // x ** (k - 1)) // k
        if y >= x:
            return x
        x = y


def _pow_bound(x: Fraction, num: int, den: int, *, upper: bool) -> Fraction:
    """A rational bound on ``x ** (num / den)`` for ``x >= 0``, ``num, den >= 1``.

    ``upper=True`` returns a value at least the true one, ``upper=False`` a value at
    most the true one.  Both have denominator :data:`_ROUND_DEN`, so the result stays
    a well-behaved rational rather than an ever-growing bisection artefact.
    """
    if x < 0:
        raise ValueError("negative base")
    if num < 1 or den < 1:
        raise ValueError("exponent parts must be positive integers")
    y = x ** num
    if y == 0:
        return Fraction(0)
    # Want m/_ROUND_DEN with (m/_ROUND_DEN) ** den on the correct side of y, i.e.
    # m ** den * y.denominator vs y.numerator * _ROUND_DEN ** den.
    target = y.numerator * _ROUND_DEN ** den
    q = y.denominator
    m = _iroot(target // q, den)
    if upper:
        while m ** den * q < target:
            m += 1
    else:
        while m > 0 and m ** den * q > target:
            m -= 1
    return Fraction(m, _ROUND_DEN)


def _as_fraction(x: Any) -> Fraction:
    """Parse a number exactly, refusing floats-as-decimals surprises via ``str``."""
    if isinstance(x, Fraction):
        return x
    if isinstance(x, bool):
        raise TypeError("boolean where a rational was expected")
    if isinstance(x, int):
        return Fraction(x)
    return Fraction(str(x))


# --------------------------------------------------------------------------- #
# The candidate record.
# --------------------------------------------------------------------------- #

@dataclass(frozen=True)
class BoundCandidate:
    """An UNVERIFIED upper bound ``K`` on ``|f - p|`` over ``[a, b]``.

    ``verified`` exists only to be ``False``: it is a reminder at every use site that
    the number came from a formula, and that only ``cert_sturm.check`` can promote a
    claim to a checked fact.
    """

    K: Fraction
    bound_kind: str
    citation: str
    degree: int
    domain: tuple[Fraction, Fraction]
    assumptions: tuple[str, ...] = ()
    verified: bool = field(default=False, init=False)

    def with_extra(self, added: Fraction, why: str) -> "BoundCandidate":
        """Widen the bound by ``added``, recording ``why`` as a further assumption."""
        added = _as_fraction(added)
        if added < 0:
            raise ValueError("cannot widen a bound by a negative amount")
        return BoundCandidate(
            K=self.K + added,
            bound_kind=self.bound_kind,
            citation=self.citation,
            degree=self.degree,
            domain=self.domain,
            assumptions=self.assumptions + (why,),
        )

    def as_dict(self) -> dict:
        return {
            "K": str(self.K),
            "bound_kind": self.bound_kind,
            "citation": self.citation,
            "degree": self.degree,
            "domain": [str(self.domain[0]), str(self.domain[1])],
            "assumptions": list(self.assumptions),
            "verified": False,
            "status": "CANDIDATE: unverified until cert_sturm.check accepts a "
                      "poly_minimax log built from it",
        }


def _domain(domain: Sequence[Any]) -> tuple[Fraction, Fraction]:
    a, b = _as_fraction(domain[0]), _as_fraction(domain[1])
    if a >= b:
        raise ValueError("domain must satisfy a < b")
    return a, b


def _check_degree(n: int, minimum: int = 1) -> int:
    n = int(n)
    if n < minimum:
        raise ValueError(f"degree n must be at least {minimum}")
    return n


# --------------------------------------------------------------------------- #
# The bounds themselves.  Each applies to p = B_n(f), the degree-n Bernstein
# polynomial of f on the stated interval (Bernstein coefficients f(x_j) at the
# n+1 evenly spaced nodes).  They do NOT apply to an arbitrary polynomial.
# --------------------------------------------------------------------------- #

def bernstein_bound_lipschitz_derivative(L1: Any, n: int,
                                         domain: Sequence[Any] = (0, 1)
                                         ) -> BoundCandidate:
    """``|f - B_n(f)| <= L1 * (b - a)**2 / (8n)`` when ``f'`` is Lipschitz.

    Row "Has Lipschitz-continuous derivative / B_n(f) / epsilon = L_1/(8n)",
    attributed to Lorentz (1964), in ``bernapprox.md`` -> "Approximations on the
    Closed Unit Interval".  The ``(b - a)**2`` factor is the interval change of
    variables from "Approximations Beyond the Closed Unit Interval": for
    ``f(x) = g(a + (b-a)x)`` the rescaled Lipschitz constant of the ``r``-th
    derivative is ``L * (b-a)**(r+1)``, and here ``r = 1``.  That section's own
    worked example (``g`` on ``[1, 3]``, factor ``4L``) is this formula.

    Exact: no irrational appears, so the returned ``K`` is the formula's value.
    """
    a, b = _domain(domain)
    n = _check_degree(n)
    L1 = _as_fraction(L1)
    if L1 < 0:
        raise ValueError("a Lipschitz constant cannot be negative")
    K = L1 * (b - a) ** 2 / (8 * n)
    return BoundCandidate(
        K=K, bound_kind="bernstein_lipschitz_derivative",
        citation="peteroupc bernapprox.md, 'Approximations on the Closed Unit "
                 "Interval' table, row 'Has Lipschitz-continuous derivative' "
                 "(Lorentz 1964); interval change of variables from "
                 "'Approximations Beyond the Closed Unit Interval'",
        degree=n, domain=(a, b),
        assumptions=(
            f"UNCHECKED: f' is Lipschitz continuous on [{a}, {b}] with constant "
            f"at most {L1}",
            f"UNCHECKED: p is exactly the degree-{n} Bernstein polynomial of f on "
            f"[{a}, {b}] (Bernstein coefficients f(x_j) at the n+1 evenly spaced "
            f"nodes)",
        ),
    )


def bernstein_bound_holder_derivative(H1: Any, alpha: Any, n: int
                                      ) -> BoundCandidate:
    """``|f - B_n(f)| <= H1 / (4 * n**((1+alpha)/2))`` on ``[0, 1]``, ``f'`` Hoelder.

    Row "Has Hoelder-continuous derivative / B_n(f) /
    epsilon = H_1/(4 n**((1+alpha)/2))", attributed to Schurer and Steutel (1975),
    in ``bernapprox.md`` -> "Approximations on the Closed Unit Interval".

    Restricted to ``[0, 1]`` on purpose.  That same document states, in
    "Approximations Beyond the Closed Unit Interval", that the bounds relying on
    ``H_r`` do not carry over to a general interval; rescaling them anyway would be
    inventing a result the source declines to state.

    ``n**((1+alpha)/2)`` is irrational in general, so we take a rational LOWER bound
    of the denominator, which makes ``K`` an upper bound of the formula's value.
    """
    n = _check_degree(n)
    H1 = _as_fraction(H1)
    alpha = _as_fraction(alpha)
    if H1 < 0:
        raise ValueError("a Hoelder constant cannot be negative")
    if not (0 < alpha <= 1):
        raise ValueError("Hoelder exponent alpha must satisfy 0 < alpha <= 1")
    # (1 + alpha)/2 with alpha = p/q is (q + p) / (2q).
    p, q = alpha.numerator, alpha.denominator
    denom = _pow_bound(Fraction(n), q + p, 2 * q, upper=False)
    if denom <= 0:
        raise ArithmeticError("degenerate rational bound for n**((1+alpha)/2)")
    K = H1 / (4 * denom)
    return BoundCandidate(
        K=K, bound_kind="bernstein_holder_derivative",
        citation="peteroupc bernapprox.md, 'Approximations on the Closed Unit "
                 "Interval' table, row 'Has Hoelder-continuous derivative' "
                 "(Schurer and Steutel 1975)",
        degree=n, domain=(Fraction(0), Fraction(1)),
        assumptions=(
            f"UNCHECKED: f' is Hoelder continuous on [0, 1] with exponent {alpha} "
            f"and constant at most {H1}",
            f"UNCHECKED: p is exactly the degree-{n} Bernstein polynomial of f on "
            f"[0, 1]",
            "the source states H_r-based bounds do not transfer off [0, 1]; this "
            "function therefore refuses any other interval",
        ),
    )


def bernstein_bound_holder(H0: Any, alpha: Any, n: int) -> BoundCandidate:
    """``|f - B_n(f)| <= H0 * (1/(4n))**(alpha/2)`` on ``[0, 1]``, ``f`` Hoelder.

    Row "Is Hoelder continuous / B_n(f) / epsilon = H_0*(1/(4n))**(alpha/2)",
    attributed to Kac (1938), in ``bernapprox.md`` -> "Approximations on the Closed
    Unit Interval".  ``[0, 1]`` only, for the reason given in
    :func:`bernstein_bound_holder_derivative`.

    ``(1/(4n))**(alpha/2)`` is taken as a rational UPPER bound, so ``K`` is never
    below the formula's value.
    """
    n = _check_degree(n)
    H0 = _as_fraction(H0)
    alpha = _as_fraction(alpha)
    if H0 < 0:
        raise ValueError("a Hoelder constant cannot be negative")
    if not (0 < alpha <= 1):
        raise ValueError("Hoelder exponent alpha must satisfy 0 < alpha <= 1")
    p, q = alpha.numerator, alpha.denominator
    factor = _pow_bound(Fraction(1, 4 * n), p, 2 * q, upper=True)
    K = H0 * factor
    return BoundCandidate(
        K=K, bound_kind="bernstein_holder",
        citation="peteroupc bernapprox.md, 'Approximations on the Closed Unit "
                 "Interval' table, row 'Is Hoelder continuous' (Kac 1938)",
        degree=n, domain=(Fraction(0), Fraction(1)),
        assumptions=(
            f"UNCHECKED: f is Hoelder continuous on [0, 1] with exponent {alpha} "
            f"and constant at most {H0}",
            f"UNCHECKED: p is exactly the degree-{n} Bernstein polynomial of f on "
            f"[0, 1]",
            "the source states H_r-based bounds do not transfer off [0, 1]; this "
            "function therefore refuses any other interval",
        ),
    )


# Sikkema's constant, (4306 + 837*sqrt(6)) / 5832, as transcribed.  The source also
# quotes the decimal ceiling 1.08989 for it.
_SIKKEMA_NUM = 4306
_SIKKEMA_SQRT_COEFF = 837
_SIKKEMA_DEN = 5832


def sikkema_constant() -> Fraction:
    """Rational UPPER bound on ``(4306 + 837*sqrt(6)) / 5832`` (Sikkema 1961).

    Transcribed from ``bernapprox.md`` -> "Approximations on the Closed Unit
    Interval", row "Is Lipschitz continuous ... Sikkema (1961)", which also gives the
    decimal ``< 1.08989``; :func:`sikkema_constant` is asserted against that decimal
    in the tests as a transcription check.
    """
    sqrt6 = _pow_bound(Fraction(6), 1, 2, upper=True)
    return (Fraction(_SIKKEMA_NUM) + _SIKKEMA_SQRT_COEFF * sqrt6) / _SIKKEMA_DEN


def bernstein_bound_lipschitz(L0: Any, n: int, domain: Sequence[Any] = (0, 1)
                              ) -> BoundCandidate:
    """``|f - B_n(f)| <= C * L0 * (b - a) / sqrt(n)`` when ``f`` is Lipschitz.

    ``bernapprox.md`` -> "Approximations on the Closed Unit Interval" gives two rows
    for a Lipschitz ``f``:

    *   "Is Lipschitz continuous / epsilon = L_0*sqrt(1/(4n))", flagged there as the
        ``alpha = 1`` special case of the Kac (1938) row, i.e. ``C = 1/2``; and
    *   "Is Lipschitz continuous / epsilon = ((4306+837*sqrt(6))/5832) L_0/n**(1/2)",
        Sikkema (1961), i.e. ``C < 1.08989``.

    We compute both and return the smaller.  They are not redundant: the constants
    disagree by a factor of about 2.18 and we have not independently confirmed either
    from the primary literature, so taking the minimum is a choice worth seeing in the
    ``bound_kind`` rather than one row being quietly dropped.

    Unlike the Hoelder rows, a Lipschitz constant DOES transfer to ``[a, b]``: the
    interval section gives ``L_r -> L * (b-a)**(r+1)``, and here ``r = 0``.
    """
    a, b = _domain(domain)
    n = _check_degree(n)
    L0 = _as_fraction(L0)
    if L0 < 0:
        raise ValueError("a Lipschitz constant cannot be negative")
    scaled = L0 * (b - a)                     # L_0 for f(x) = g(a + (b-a)x)
    inv_sqrt_n = _pow_bound(Fraction(1, n), 1, 2, upper=True)
    kac = scaled * _pow_bound(Fraction(1, 4 * n), 1, 2, upper=True)
    sikkema = scaled * sikkema_constant() * inv_sqrt_n
    if kac <= sikkema:
        K, which = kac, "kac1938_alpha1"
    else:
        K, which = sikkema, "sikkema1961"
    return BoundCandidate(
        K=K, bound_kind=f"bernstein_lipschitz[{which}]",
        citation="peteroupc bernapprox.md, 'Approximations on the Closed Unit "
                 "Interval' table, rows 'Is Lipschitz continuous' (Kac 1938 "
                 "alpha=1, and Sikkema 1961); interval change of variables from "
                 "'Approximations Beyond the Closed Unit Interval'",
        degree=n, domain=(a, b),
        assumptions=(
            f"UNCHECKED: f is Lipschitz continuous on [{a}, {b}] with constant at "
            f"most {L0}",
            f"UNCHECKED: p is exactly the degree-{n} Bernstein polynomial of f on "
            f"[{a}, {b}]",
            "the two source rows give constants 1/2 and about 1.08989 for the same "
            "hypothesis; the smaller was taken",
        ),
    )


# --------------------------------------------------------------------------- #
# Building the concrete Bernstein approximant, exactly.
# --------------------------------------------------------------------------- #

def bernstein_nodes(n: int, domain: Sequence[Any] = (0, 1)) -> list[Fraction]:
    """The ``n + 1`` evenly spaced nodes ``a + (b - a) * j / n`` as exact rationals."""
    a, b = _domain(domain)
    n = _check_degree(n)
    return [a + (b - a) * Fraction(j, n) for j in range(n + 1)]


def _poly_mul(p: list[Fraction], q: list[Fraction]) -> list[Fraction]:
    out = [Fraction(0)] * (len(p) + len(q) - 1)
    for i, pi in enumerate(p):
        if pi == 0:
            continue
        for j, qj in enumerate(q):
            if qj:
                out[i + j] += pi * qj
    return out


def _binomial(n: int, k: int) -> int:
    c = 1
    for i in range(k):
        c = c * (n - i) // (i + 1)
    return c


def bernstein_monomial_coeffs(bern_coeffs: Sequence[Any],
                              domain: Sequence[Any] = (0, 1)) -> list[Fraction]:
    """Monomial coefficients (ascending) of a Bernstein-form polynomial on ``[a, b]``.

    ``bern_coeffs`` are the ``n + 1`` Bernstein coefficients for ``[a, b]``, i.e.

        p(x) = sum_j a_j * C(n, j) * (x - a)**j * (b - x)**(n - j) / (b - a)**n

    as written in ``bernapprox.md`` -> "Approximations Beyond the Closed Unit
    Interval".  The conversion is exact over ``Fraction``; ``cert_sturm`` wants powers
    of ``x``, and doing the change of basis in floating point here would put an
    unbounded, uncertified error between the polynomial we bounded and the polynomial
    we certify.
    """
    a, b = _domain(domain)
    coeffs = [_as_fraction(c) for c in bern_coeffs]
    if len(coeffs) < 2:
        raise ValueError("need at least two Bernstein coefficients (degree >= 1)")
    n = len(coeffs) - 1
    acc = [Fraction(0)] * (n + 1)
    for j, aj in enumerate(coeffs):
        if aj == 0:
            continue
        term = [Fraction(1)]
        for _ in range(j):                       # (x - a)**j
            term = _poly_mul(term, [-a, Fraction(1)])
        for _ in range(n - j):                   # (b - x)**(n - j)
            term = _poly_mul(term, [b, Fraction(-1)])
        scale = aj * _binomial(n, j)
        for i, t in enumerate(term):
            acc[i] += scale * t
    inv = Fraction(1) / (b - a) ** n
    out = [c * inv for c in acc]
    # Drop trailing zeros so the coefficient list states the true degree; a Bernstein
    # form of degree n can represent a lower-degree polynomial, and downstream Sturm
    # work is cheaper on the honest degree.  Keep at least the constant term.
    while len(out) > 1 and out[-1] == 0:
        out.pop()
    return out


# --------------------------------------------------------------------------- #
# The one entry point most callers want.
# --------------------------------------------------------------------------- #

def poly_minimax_candidate(
    bound: BoundCandidate,
    node_values: Sequence[Any],
    *,
    node_slack: Any = 0,
    strict_slack: Any = 0,
) -> dict:
    """Assemble ``p_coeffs`` and a candidate ``K`` for ``export_poly_minimax_cert``.

    ``node_values`` are exact rationals used as the Bernstein coefficients, one per
    node of :func:`bernstein_nodes`.  For a general ``f`` they will be *rounded*
    samples, so ``node_slack`` must be an upper bound on
    ``max_j |f(x_j) - node_values[j]|``.  The extra ``node_slack`` in ``K`` is exactly
    Note 1 of ``bernapprox.md`` -> "Approximations on the Closed Unit Interval":
    perturbing every Bernstein coefficient by at most ``delta`` moves the polynomial
    by at most ``delta``, because the Bernstein basis is non-negative on ``[a, b]``
    and sums to one there.

    ``strict_slack`` is a deliberate escape hatch, not a fudge factor.  The source
    bounds are non-strict (``|f - p| <= K``) and several are attained, whereas
    ``cert_sturm``'s no-crossing step needs ``|p - T|`` to stay STRICTLY below
    ``K - delta``.  A candidate equal to an attained bound is therefore correct and
    still rejected.  Adding a positive ``strict_slack`` is how a caller asks for a
    slightly weaker claim that the checker can actually close; leaving it at zero and
    watching the rejection is the honest default.

    Returns a plain dict.  It does not build, sign, or check a certificate: the
    returned ``K`` is unverified until ``cert_sturm.check`` says otherwise, and that
    is stated in the ``status`` field.
    """
    if not isinstance(bound, BoundCandidate):
        raise TypeError("bound must be a BoundCandidate")
    a, b = bound.domain
    values = [_as_fraction(v) for v in node_values]
    if len(values) != bound.degree + 1:
        raise ValueError(f"expected {bound.degree + 1} node values for degree "
                         f"{bound.degree}, got {len(values)}")
    slack = _as_fraction(node_slack)
    if slack < 0:
        raise ValueError("node_slack must be non-negative")
    strict = _as_fraction(strict_slack)
    if strict < 0:
        raise ValueError("strict_slack must be non-negative")

    total = bound
    if slack > 0:
        total = total.with_extra(
            slack,
            f"UNCHECKED: every supplied node value is within {slack} of the true "
            f"f(x_j) (bernapprox.md 'Approximations on the Closed Unit Interval', "
            f"Note 1: perturbing Bernstein coefficients by at most delta moves the "
            f"polynomial by at most delta)")
    if strict > 0:
        total = total.with_extra(
            strict,
            f"widened by {strict} so the claim is strict enough for cert_sturm's "
            f"no-crossing step; this weakens the claim, it does not strengthen it")

    p_coeffs = bernstein_monomial_coeffs(values, (a, b))
    out = total.as_dict()
    out["p_coeffs"] = [str(c) for c in p_coeffs]
    out["domain"] = [str(a), str(b)]
    out["node_values"] = [str(v) for v in values]
    return out


def describe_available_bounds() -> list[dict]:
    """Machine-readable catalogue of what this module can generate, and from what.

    Kept next to the bounds so a caller can see the required inputs (which are exactly
    the unverifiable hypotheses) before committing to a bound kind.
    """
    return [
        {"kind": "bernstein_lipschitz_derivative",
         "needs": ["L1: Lipschitz constant of f'"],
         "interval": "any [a, b]",
         "formula": "L1 * (b-a)**2 / (8n)",
         "source": "Lorentz 1964, via bernapprox.md"},
        {"kind": "bernstein_holder_derivative",
         "needs": ["H1: Hoelder constant of f'", "alpha in (0, 1]"],
         "interval": "[0, 1] only",
         "formula": "H1 / (4 * n**((1+alpha)/2))",
         "source": "Schurer and Steutel 1975, via bernapprox.md"},
        {"kind": "bernstein_holder",
         "needs": ["H0: Hoelder constant of f", "alpha in (0, 1]"],
         "interval": "[0, 1] only",
         "formula": "H0 * (1/(4n))**(alpha/2)",
         "source": "Kac 1938, via bernapprox.md"},
        {"kind": "bernstein_lipschitz",
         "needs": ["L0: Lipschitz constant of f"],
         "interval": "any [a, b]",
         "formula": "min(1/2, (4306+837*sqrt(6))/5832) * L0 * (b-a) / sqrt(n)",
         "source": "Kac 1938 and Sikkema 1961, via bernapprox.md"},
    ]


def run(request: dict) -> dict:
    """Worker entrypoint.  ``request["op"]`` selects a bound or the catalogue.

    Ops: ``catalogue``, ``lipschitz_derivative``, ``holder_derivative``, ``holder``,
    ``lipschitz``.  All but ``catalogue`` accept ``n`` and the relevant constants, plus
    optional ``domain``, ``node_values``, ``node_slack`` and ``strict_slack``; with
    ``node_values`` the result is a full :func:`poly_minimax_candidate` dict, otherwise
    just the bound.  Nothing here verifies anything.
    """
    op = request.get("op", "catalogue")
    if op == "catalogue":
        return {"bounds": describe_available_bounds()}
    n = request["n"]
    domain = request.get("domain", (0, 1))
    if op == "lipschitz_derivative":
        bound = bernstein_bound_lipschitz_derivative(request["L1"], n, domain)
    elif op == "holder_derivative":
        bound = bernstein_bound_holder_derivative(request["H1"], request["alpha"], n)
    elif op == "holder":
        bound = bernstein_bound_holder(request["H0"], request["alpha"], n)
    elif op == "lipschitz":
        bound = bernstein_bound_lipschitz(request["L0"], n, domain)
    else:
        raise ValueError(f"unknown op: {op!r}")
    if "node_values" in request:
        return poly_minimax_candidate(
            bound, request["node_values"],
            node_slack=request.get("node_slack", 0),
            strict_slack=request.get("strict_slack", 0))
    return bound.as_dict()
