"""Power planning for comparative evals: how many items BEFORE spending budget.

This module answers a question the rest of the eval layer does not: given that we
intend to claim "system A beats system B", how many benchmark items must the
comparison run over for that claim to be supportable at all. It is the
*prospective* counterpart to ``proof_calibration.bootstrap_ci``, which is
*retrospective* (how uncertain is a number we already have).

PLANNING ONLY. Everything here is an a-priori design estimate computed from
ASSUMED inputs (an assumed win rate, an assumed tie rate, an assumed
intra-cluster correlation). Nothing in this module inspects an actual eval run,
and no output of this module is evidence that any finished result is
significant, real, or reproducible. A plan that says "500 items" and a run that
collected 500 items still has to be analysed on its own merits. Every returned
structure carries ``planning_only: True`` and a ``disclaimer`` string so the
number cannot travel without that caveat attached.

The design
----------
The comparison is modelled as a **paired sign test against p = 0.5**. On each
item both systems are scored, the pair is called for A, called for B, or is
non-decisive (a tie, a grader abstention, or a harness error). Under the null
"the systems are indistinguishable", A wins half of the *decisive* pairs. This
is the weakest defensible model: it assumes only that the pairing is valid and
the items are exchangeable, and it needs no score scale, no normality, and no
variance estimate. That weakness is the point, since a comparative eval over
heterogeneous benchmark items rarely earns anything stronger.

Four multipliers turn the raw sign-test count into an item budget:

1. ``required_decisive_pairs``  exact binomial sign-test N (see below).
2. **PPI** (optional): a cheap AI pre-pass that correlates with the expensive
   ground truth reduces the labelling requirement by ``(1 - rho ** 2)``.
3. **Design effect** (Kish): items from one problem family are not independent,
   so multiply by ``1 + (m - 1) * icc``.
4. **Tie inflation**: ties carry no sign-test signal, so ``N`` decisive pairs
   require ``N / (1 - tie_rate)`` items.

Order matters and is fixed: PPI and clustering both act on the *statistical*
requirement (decisive pairs), while tie inflation is the last step that converts
decisive pairs into items to actually run.

Formulas, and which are exact
-----------------------------
``required_decisive_pairs`` is **exact** whenever the answer is at or below
``exact_max_n`` (default 2000). For a candidate N the critical value is

    c = min { c : P(S >= c | N, p = 0.5) <= alpha_tail }

with ``alpha_tail = alpha / 2`` two-sided and ``alpha`` one-sided, and the power
at the alternative ``p1`` is ``P(S >= c | N, p1)``, plus the mirrored lower tail
``P(S <= N - c | N, p1)`` when two-sided. Both tail probabilities are summed
term by term from the exact binomial pmf computed in log space via
``math.lgamma``, so there is no normal approximation anywhere in that path. The
reported N is the SMALLEST N whose exact power reaches the target.

That smallest-N convention needs a warning. Exact binomial power is **not
monotone in N**: it saws, because the critical value moves in integer steps
while N moves continuously. For p1 = 0.75, alpha = 0.05 two-sided, power at
N = 30 is 0.803 but at N = 31 it drops to 0.771. The reported N is therefore
the first N that attains the target, not an N beyond which power stays attained.
Every plan reports ``achieved_power_at_n`` and ``exact_power_is_sawtoothed:
True`` so a caller who wants a floor can round up rather than discover this in
production.

Above ``exact_max_n`` the module falls back to the standard normal
approximation

    N = ( (z_{1 - alpha_tail} * 0.5 + z_{power} * sqrt(p1 * (1 - p1)))
          / |p1 - 0.5| ) ** 2

which is **approximate** and is labelled as such in ``method``. It degrades
exactly where the normal approximation to the binomial always degrades: small N
(it is only reached for N above 2000, so this is not a practical concern here)
and extreme p1 near 0 or 1, where the binomial is skewed and the continuous
symmetric approximation understates the required N. Empirically it understates
by a few percent even in the comfortable middle: for p1 = 0.8, alpha = 0.05
two-sided, power 0.90 it says 25 where the exact answer is 28. Treat it as a
lower bound.

The design effect is Kish's, ``deff = 1 + (m - 1) * icc``, exact under its own
assumption of equal cluster sizes; unequal clusters make it optimistic, so
``mean_cluster_size`` should be the larger of the mean and a size-weighted mean
if those differ much.

The PPI adjustment is a **variance-ratio heuristic**, not an exact result. It
assumes the rectifier is unbiased, the cheap pre-pass and the expensive label
are jointly close to bivariate normal on the paired-difference statistic, and
the unlabelled pool is effectively unlimited. Under those assumptions the
variance of the rectified estimator scales by ``1 - rho ** 2``, so the labelling
requirement scales the same way. When any assumption is shaky, drop it: the
plan without PPI is the conservative one.

Refusals
--------
Impossible inputs raise ``PowerPlanError`` rather than returning a number.
Silently clamping a planning input produces a number that answers a question
nobody asked, which is strictly worse than an error. See ``PowerPlanError``.

Pure Python and the standard library only. ``scipy`` is deliberately not
imported: it is installed in some environments here but is NOT a declared
dependency in ``pyproject.toml``, and a planning module that runs in CI must not
depend on an undeclared package.

Public API:
    ``plan(...) -> dict``                  end-to-end item budget with assumptions
    ``required_decisive_pairs(...) -> int``  exact/approx sign-test N
    ``sign_test_power(n, ...) -> float``     exact power of a sign test at size n
    ``design_effect(icc, m) -> float``       Kish design effect
    ``tie_inflation_factor(tie_rate) -> float``
    ``ppi_variance_factor(rho) -> float``
    ``run(request)``                       JSON worker entry (op in {plan, power})
"""
from __future__ import annotations

import json
import math
import sys
from statistics import NormalDist
from typing import Any

__all__ = [
    "PowerPlanError",
    "plan",
    "required_decisive_pairs",
    "sign_test_power",
    "design_effect",
    "tie_inflation_factor",
    "ppi_variance_factor",
    "run",
]

DISCLAIMER = (
    "PLANNING ESTIMATE ONLY. Computed from assumed inputs before any data is "
    "collected. This is not evidence that any finished comparison is "
    "significant, and it does not validate any result."
)

# Above this the exact binomial search costs O(n^2) term evaluations, so the
# module switches to the normal approximation and says so in ``method``.
DEFAULT_EXACT_MAX_N = 2000


class PowerPlanError(ValueError):
    """An input that has no meaningful planning answer.

    Raised instead of clamping. A clamped planning number looks like an answer
    to the question that was asked but is an answer to a different one, and it
    carries no signal that the substitution happened.
    """


# --------------------------------------------------------------------------- #
# Input validation
# --------------------------------------------------------------------------- #

def _check_unit_open(name: str, value: Any) -> float:
    """Require a value strictly inside (0, 1)."""
    try:
        v = float(value)
    except (TypeError, ValueError):
        raise PowerPlanError(f"{name} must be a number, got {value!r}") from None
    if math.isnan(v) or not (0.0 < v < 1.0):
        raise PowerPlanError(f"{name} must lie strictly in (0, 1), got {v!r}")
    return v


def _check_win_rate(win_rate: Any) -> float:
    v = _check_unit_open("win_rate", win_rate)
    if v == 0.5:
        raise PowerPlanError(
            "win_rate == 0.5 is a zero effect size: the alternative equals the "
            "null, so no finite sample size attains power above alpha. Choose "
            "the smallest win rate the comparison must be able to detect."
        )
    return v


def _check_tie_rate(tie_rate: Any) -> float:
    try:
        v = float(tie_rate)
    except (TypeError, ValueError):
        raise PowerPlanError(f"tie_rate must be a number, got {tie_rate!r}") from None
    if math.isnan(v) or not (0.0 <= v < 1.0):
        raise PowerPlanError(
            f"tie_rate must lie in [0, 1), got {v!r}. A tie rate of 1.0 means no "
            "comparison is ever decisive, which needs infinitely many items."
        )
    return v


def _check_icc(icc: Any) -> float:
    try:
        v = float(icc)
    except (TypeError, ValueError):
        raise PowerPlanError(f"icc must be a number, got {icc!r}") from None
    if math.isnan(v) or not (0.0 <= v <= 1.0):
        raise PowerPlanError(f"icc must lie in [0, 1], got {v!r}")
    return v


def _check_cluster_size(m: Any) -> float:
    try:
        v = float(m)
    except (TypeError, ValueError):
        raise PowerPlanError(f"mean_cluster_size must be a number, got {m!r}") from None
    if math.isnan(v) or v <= 0.0:
        raise PowerPlanError(f"mean_cluster_size must be positive, got {v!r}")
    return v


def _check_sides(sides: Any) -> int:
    if sides not in (1, 2):
        raise PowerPlanError(f"sides must be 1 or 2, got {sides!r}")
    return int(sides)


# --------------------------------------------------------------------------- #
# Exact binomial machinery
# --------------------------------------------------------------------------- #

def _log_binom_pmf(n: int, p: float) -> list[float]:
    """Exact binomial log-pmf for k = 0..n.

    Log space rather than a multiplicative recurrence because the recurrence
    seeds on (1 - p) ** n, which underflows to exactly 0.0 around n = 1080 for
    p = 0.5 and then poisons every subsequent term.
    """
    lgn = math.lgamma(n + 1)
    lp = math.log(p)
    lq = math.log1p(-p)
    return [
        lgn - math.lgamma(k + 1) - math.lgamma(n - k + 1) + k * lp + (n - k) * lq
        for k in range(n + 1)
    ]


def _upper_tail(logpmf: list[float], c: int) -> float:
    """P(S >= c) summed from the far tail inward for accuracy."""
    n = len(logpmf) - 1
    if c > n:
        return 0.0
    if c <= 0:
        return 1.0
    return math.fsum(math.exp(logpmf[k]) for k in range(n, c - 1, -1))


def _lower_tail(logpmf: list[float], c: int) -> float:
    """P(S <= c)."""
    if c < 0:
        return 0.0
    n = len(logpmf) - 1
    if c >= n:
        return 1.0
    return math.fsum(math.exp(logpmf[k]) for k in range(0, c + 1))


def _critical_value(logpmf_null: list[float], alpha_tail: float) -> int:
    """Smallest c with P(S >= c | p = 0.5) <= alpha_tail.

    Walking down from n + 1 rather than up from 0 keeps the test conservative:
    the first c whose tail exceeds alpha_tail is rejected and its successor
    returned, so the realised size never exceeds the nominal one.
    """
    n = len(logpmf_null) - 1
    for c in range(n + 1, -1, -1):
        if _upper_tail(logpmf_null, c) > alpha_tail:
            return c + 1
    return 0


def sign_test_power(
    n: int,
    win_rate: float,
    alpha: float = 0.05,
    sides: int = 2,
) -> float:
    """Exact power of a sign test on ``n`` decisive pairs at alternative ``win_rate``.

    No normal approximation. Note that this is NOT monotone in ``n``; see the
    module docstring.
    """
    if not isinstance(n, int) or isinstance(n, bool) or n < 1:
        raise PowerPlanError(f"n must be a positive int, got {n!r}")
    p1 = _check_win_rate(win_rate)
    a = _check_unit_open("alpha", alpha)
    s = _check_sides(sides)

    alpha_tail = a / 2.0 if s == 2 else a
    logpmf_null = _log_binom_pmf(n, 0.5)
    logpmf_alt = _log_binom_pmf(n, p1)
    c_hi = _critical_value(logpmf_null, alpha_tail)
    power = _upper_tail(logpmf_alt, c_hi)
    if s == 2:
        # The null is symmetric, so the lower rejection region mirrors the upper.
        power += _lower_tail(logpmf_alt, n - c_hi)
    return min(1.0, power)


def _normal_approx_n(p1: float, alpha: float, power: float, sides: int) -> int:
    z_alpha = NormalDist().inv_cdf(1.0 - (alpha / sides))
    z_beta = NormalDist().inv_cdf(power)
    numer = z_alpha * 0.5 + z_beta * math.sqrt(p1 * (1.0 - p1))
    return max(1, math.ceil((numer / abs(p1 - 0.5)) ** 2))


def required_decisive_pairs(
    win_rate: float,
    alpha: float = 0.05,
    power: float = 0.80,
    sides: int = 2,
    exact_max_n: int = DEFAULT_EXACT_MAX_N,
    _detail: bool = False,
) -> Any:
    """Decisive pairs needed for a sign test to reach ``power`` at ``alpha``.

    Returns an int, or with ``_detail`` the tuple
    ``(n, method, achieved_power_or_None)``.
    """
    p1 = _check_win_rate(win_rate)
    a = _check_unit_open("alpha", alpha)
    pw = _check_unit_open("power", power)
    s = _check_sides(sides)
    if not isinstance(exact_max_n, int) or isinstance(exact_max_n, bool) or exact_max_n < 1:
        raise PowerPlanError(f"exact_max_n must be a positive int, got {exact_max_n!r}")

    approx = _normal_approx_n(p1, a, pw, s)
    # The approximation understates, so it is a safe place to bound the exact
    # search; the search itself still starts at 1 for correctness.
    if approx > exact_max_n:
        return (approx, "normal_approximation", None) if _detail else approx

    alpha_tail = a / 2.0 if s == 2 else a
    ceiling = min(exact_max_n, max(4 * approx, approx + 64))
    for n in range(1, ceiling + 1):
        logpmf_null = _log_binom_pmf(n, 0.5)
        c_hi = _critical_value(logpmf_null, alpha_tail)
        if c_hi > n:
            # No rejection region exists at this alpha and n, so power is 0.
            continue
        logpmf_alt = _log_binom_pmf(n, p1)
        achieved = _upper_tail(logpmf_alt, c_hi)
        if s == 2:
            achieved += _lower_tail(logpmf_alt, n - c_hi)
        if achieved >= pw:
            return (n, "exact_binomial", min(1.0, achieved)) if _detail else n

    # The exact search ran out of room; report the approximation and label it.
    return (approx, "normal_approximation", None) if _detail else approx


# --------------------------------------------------------------------------- #
# Multipliers
# --------------------------------------------------------------------------- #

def design_effect(icc: float, mean_cluster_size: float) -> float:
    """Kish design effect ``1 + (m - 1) * icc``.

    Exact when clusters are equal-sized; optimistic when they are not.
    """
    rho = _check_icc(icc)
    m = _check_cluster_size(mean_cluster_size)
    return 1.0 + (m - 1.0) * rho


def tie_inflation_factor(tie_rate: float) -> float:
    """``1 / (1 - tie_rate)``: items needed per decisive pair."""
    t = _check_tie_rate(tie_rate)
    return 1.0 / (1.0 - t)


def ppi_variance_factor(ppi_correlation: float) -> float:
    """``1 - rho ** 2``: the labelling requirement's shrink factor under PPI.

    Heuristic, not exact; see the module docstring for the assumptions it rides
    on. ``rho`` of 1.0 is refused because a cheap label perfectly correlated
    with the expensive one would imply zero expensive labels are ever needed,
    which is never true in practice and would silently zero out the plan.
    """
    try:
        rho = float(ppi_correlation)
    except (TypeError, ValueError):
        raise PowerPlanError(
            f"ppi_correlation must be a number, got {ppi_correlation!r}"
        ) from None
    if math.isnan(rho) or not (0.0 <= rho < 1.0):
        raise PowerPlanError(
            f"ppi_correlation must lie in [0, 1), got {rho!r}. A correlation of "
            "1.0 would imply the expensive ground truth is never needed."
        )
    return 1.0 - rho * rho


# --------------------------------------------------------------------------- #
# End-to-end plan
# --------------------------------------------------------------------------- #

def plan(
    win_rate: float,
    alpha: float = 0.05,
    power: float = 0.80,
    sides: int = 2,
    tie_rate: float = 0.0,
    icc: float = 0.0,
    mean_cluster_size: float = 1.0,
    ppi_correlation: float | None = None,
    exact_max_n: int = DEFAULT_EXACT_MAX_N,
    label: str | None = None,
) -> dict[str, Any]:
    """Plan the item budget for a paired A-versus-B comparative eval.

    PLANNING ESTIMATE. Not a significance claim about any finished run.

    Parameters mirror the assumptions they encode. ``win_rate`` is the smallest
    per-decisive-pair win probability for A that the comparison must be able to
    detect. ``tie_rate`` is the fraction of items expected to yield no decisive
    verdict, and it should include grader abstentions and harness errors, not
    only literal ties: in the eval corpus that motivated this module, 694 ties
    plus 433 errors out of 2102 items left only 975 decisive comparisons, a
    non-decisive rate of about 0.536.

    Every assumption used is echoed in the returned ``assumptions`` block, so
    the number cannot be quoted without them.
    """
    n_stat, method, achieved = required_decisive_pairs(
        win_rate, alpha=alpha, power=power, sides=sides,
        exact_max_n=exact_max_n, _detail=True,
    )
    tie = tie_inflation_factor(tie_rate)
    deff = design_effect(icc, mean_cluster_size)

    ppi_used = ppi_correlation is not None
    ppi_factor = ppi_variance_factor(ppi_correlation) if ppi_used else 1.0

    # PPI and clustering both scale the statistical requirement; tie inflation
    # is applied last because it converts decisive pairs into items to run.
    after_ppi = n_stat * ppi_factor
    after_deff = after_ppi * deff
    decisive_needed = max(1, math.ceil(after_deff))
    items_needed = max(decisive_needed, math.ceil(decisive_needed * tie))

    return {
        "planning_only": True,
        "disclaimer": DISCLAIMER,
        "label": label,
        "required_items": items_needed,
        "required_decisive_pairs": decisive_needed,
        "assumptions": {
            "test": "paired sign test against p = 0.5",
            "null_hypothesis": "A wins half of the decisive comparisons",
            "alternative_win_rate": float(win_rate),
            "effect_size_abs": abs(float(win_rate) - 0.5),
            "alpha": float(alpha),
            "sides": int(sides),
            "target_power": float(power),
            "tie_rate": float(tie_rate),
            "icc": float(icc),
            "mean_cluster_size": float(mean_cluster_size),
            "ppi_used": ppi_used,
            "ppi_correlation": (
                float(ppi_correlation) if ppi_used else None
            ),
        },
        "stages": {
            "sign_test_pairs": int(n_stat),
            "ppi_factor": ppi_factor,
            "after_ppi": after_ppi,
            "design_effect": deff,
            "after_design_effect": after_deff,
            "tie_inflation_factor": tie,
        },
        "method": method,
        "method_is_exact": method == "exact_binomial",
        "achieved_power_at_n": achieved,
        "exact_power_is_sawtoothed": True,
        "notes": [
            "required_items is a prospective budget, not a result.",
            "Reported N is the smallest N attaining the target power; exact "
            "binomial power is not monotone in N, so a slightly larger N may "
            "have slightly lower power.",
            (
                "PPI adjustment applied as a (1 - rho ** 2) variance heuristic; "
                "it assumes an unbiased rectifier and an effectively unlimited "
                "unlabelled pool."
                if ppi_used
                else "No PPI adjustment applied."
            ),
            "Design effect is Kish's 1 + (m - 1) * icc, exact only for "
            "equal-sized clusters.",
        ],
    }


# --------------------------------------------------------------------------- #
# JSON worker entry
# --------------------------------------------------------------------------- #

def run(request: dict[str, Any]) -> dict[str, Any]:
    """JSON dispatch for the worker (tool key ``eval_power``)."""
    op = request.get("op", "plan")
    if op == "plan":
        raw_ppi = request.get("ppi_correlation")
        return plan(
            request.get("win_rate", 0.6),
            alpha=request.get("alpha", 0.05),
            power=request.get("power", 0.80),
            sides=request.get("sides", 2),
            tie_rate=request.get("tie_rate", 0.0),
            icc=request.get("icc", 0.0),
            mean_cluster_size=request.get("mean_cluster_size", 1.0),
            ppi_correlation=raw_ppi,
            exact_max_n=int(request.get("exact_max_n", DEFAULT_EXACT_MAX_N)),
            label=request.get("label"),
        )
    if op == "power":
        n = request.get("n")
        if not isinstance(n, int) or isinstance(n, bool):
            raise PowerPlanError(f"op=power requires an int n, got {n!r}")
        return {
            "planning_only": True,
            "disclaimer": DISCLAIMER,
            "n": n,
            "power": sign_test_power(
                n,
                request.get("win_rate", 0.6),
                alpha=request.get("alpha", 0.05),
                sides=request.get("sides", 2),
            ),
            "assumptions": {
                "test": "paired sign test against p = 0.5",
                "alternative_win_rate": float(request.get("win_rate", 0.6)),
                "alpha": float(request.get("alpha", 0.05)),
                "sides": int(request.get("sides", 2)),
            },
            "method": "exact_binomial",
        }
    raise PowerPlanError(f"unknown op: {op}")


def main() -> None:
    if len(sys.argv) >= 2:
        with open(sys.argv[1], encoding="utf-8") as fh:
            request = json.load(fh)
    else:
        request = json.load(sys.stdin)
    json.dump(run(request), sys.stdout)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
