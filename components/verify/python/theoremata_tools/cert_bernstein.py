"""Bernstein-basis nonnegativity certificate (kind ``bernstein``).

A univariate polynomial ``p(x)`` written in the Bernstein basis of degree ``n``
over ``[a, b]``,

    p(x) = sum_k b_k * B_{n,k}(x),   B_{n,k}(x) = C(n,k) t^k (1-t)^(n-k),  t=(x-a)/(b-a)

has the *convex-hull enclosure* property: ``min_k b_k <= p(x) <= max_k b_k`` on
``[a, b]``. In particular, **if every Bernstein coefficient ``b_k >= 0`` then
``p(x) >= 0`` for all x in [a, b]`` -- a checkable positivity certificate. This
is the cheap, offline, SDP-free complement to the Taylor-model (``taylor_model``,
for analytic functions) and sum-of-squares (``sos``, general but multivariate
generation is SDP-gated) certificates: for a *polynomial* on a box it needs only
exact rational arithmetic.

Design note (build-vs-reuse): the whole computation is the exact monomial ->
Bernstein change of basis plus a sign check, which ``fractions.Fraction`` +
``math.comb`` cover exactly; no CAS/solver is pulled in. The trust-critical
checker re-derives every Bernstein coefficient from the raw monomial data and
trusts nothing from the producer.

Soundness: nonnegative Bernstein coefficients are a *sufficient* (not necessary)
condition for nonnegativity on the box. A positive polynomial can still have a
negative low-degree Bernstein coefficient; raising the represented degree (degree
elevation) tightens the enclosure. The certificate therefore only ever *claims*
positivity when the coefficients it carries are all nonnegative, and the checker
independently confirms that -- so a bad certificate is rejected, never silently
accepted.

Format: the shared ``theoremata.cert-log.v1`` envelope, kind ``bernstein``.
Worker op: ``cert_bernstein`` (``run`` with ``op`` in {``export``, ``check``}).
A CakeML/HOL-Light-verified Bernstein checker validating the same log is the
toolchain-gated upgrade (see ``CAKEML_TARGET``).
"""
from __future__ import annotations

from fractions import Fraction
from math import comb
from typing import Any, Optional, Sequence

FORMAT = "theoremata.cert-log.v1"
KIND = "bernstein"
KINDS = (KIND,)
CAKEML_TARGET = "HOL Light / CakeML-verified Bernstein-coefficient checker"


def _fr(v: Any) -> Fraction:
    """Parse an exact rational from a string / int / [num, den] pair."""
    if isinstance(v, Fraction):
        return v
    if isinstance(v, (list, tuple)) and len(v) == 2:
        return Fraction(int(v[0]), int(v[1]))
    if isinstance(v, int):
        return Fraction(v)
    if isinstance(v, str):
        return Fraction(v)
    raise ValueError(f"not an exact rational: {v!r}")


def _remap_to_unit(coeffs: Sequence[Fraction], a: Fraction, b: Fraction) -> list[Fraction]:
    """Monomial coeffs of ``p(x)`` (ascending) -> monomial coeffs of ``p(a+(b-a)t)``
    in ``t`` (ascending), exactly. Uses ``(a+(b-a)t)^i = sum_j C(i,j) a^(i-j) w^j t^j``
    with ``w = b-a``."""
    if b == a:
        raise ValueError("degenerate interval: a == b")
    w = b - a
    n = len(coeffs) - 1
    d = [Fraction(0)] * (n + 1)
    for i, ci in enumerate(coeffs):
        if ci == 0:
            continue
        for j in range(i + 1):
            d[j] += ci * comb(i, j) * (a ** (i - j)) * (w ** j)
    return d


def bernstein_coeffs(coeffs: Sequence[Any], a: Any, b: Any) -> list[Fraction]:
    """Exact Bernstein coefficients (degree ``n = len(coeffs)-1``) of the polynomial
    with ascending monomial ``coeffs`` over ``[a, b]``.

    ``b_k = sum_{j<=k} ( C(k,j) / C(n,j) ) d_j`` where ``d`` are the monomial coeffs
    on the unit interval."""
    c = [_fr(v) for v in coeffs]
    if not c:
        raise ValueError("empty coefficient list")
    d = _remap_to_unit(c, _fr(a), _fr(b))
    n = len(c) - 1
    out: list[Fraction] = []
    for k in range(n + 1):
        s = Fraction(0)
        for j in range(k + 1):
            s += Fraction(comb(k, j), comb(n, j)) * d[j]
        out.append(s)
    return out


def export_bernstein_cert(
    coeffs: Sequence[Any],
    interval: Sequence[Any],
    *,
    var: str = "x",
    claim: Optional[str] = None,
) -> dict:
    """Serialize a Bernstein nonnegativity certificate for ``p`` (ascending monomial
    ``coeffs``) on ``interval = [a, b]``. Raises ``ValueError`` if some Bernstein
    coefficient is negative (the certificate only witnesses genuine nonnegativity)."""
    a, b = _fr(interval[0]), _fr(interval[1])
    bern = bernstein_coeffs(coeffs, a, b)
    if any(bk < 0 for bk in bern):
        raise ValueError(
            "polynomial is not certified nonnegative on the interval at this degree "
            "(a Bernstein coefficient is negative; try degree elevation)"
        )
    c = [_fr(v) for v in coeffs]
    n = len(c) - 1
    return {
        "format": FORMAT,
        "kind": KIND,
        "claim": claim or f"p({var}) >= 0 on [{a}, {b}]",
        "steps": [
            {
                "op": "bernstein_problem",
                "var": var,
                "coeffs": [str(v) for v in c],
                "domain": [str(a), str(b)],
            },
            {"op": "bernstein_coeffs", "degree": n, "b": [str(v) for v in bern]},
            {"op": "assert_coeffs_match"},
            {"op": "assert_nonneg"},
        ],
        "meta": {"producer": "cert_bernstein.export_bernstein_cert",
                 "cakeml_target": CAKEML_TARGET},
    }


def check(log: Any) -> dict:
    """Independently re-verify a ``bernstein`` certificate with exact rational
    arithmetic. Recomputes the Bernstein coefficients from the raw monomial data
    (trusting nothing from the producer), requires them to match the carried
    coefficients, and requires all of them to be nonnegative -- which certifies
    ``p >= 0`` on the interval. Any mismatch or negative coefficient is rejected."""
    if not isinstance(log, dict):
        return {"valid": False, "reason": "log is not an object"}
    if log.get("format") != FORMAT:
        return {"valid": False, "reason": f"unknown format: {log.get('format')!r}"}
    if log.get("kind") != KIND:
        return {"valid": False, "reason": f"not a {KIND} certificate: {log.get('kind')!r}"}
    steps = log.get("steps")
    if not isinstance(steps, list) or not steps:
        return {"valid": False, "reason": "missing steps"}

    problem = next((s for s in steps if s.get("op") == "bernstein_problem"), None)
    coeffs_step = next((s for s in steps if s.get("op") == "bernstein_coeffs"), None)
    if problem is None or coeffs_step is None:
        return {"valid": False, "reason": "missing bernstein_problem or bernstein_coeffs step"}
    ops = {s.get("op") for s in steps}
    if "assert_coeffs_match" not in ops or "assert_nonneg" not in ops:
        return {"valid": False, "reason": "missing required assertion step"}

    try:
        raw_coeffs = problem["coeffs"]
        domain = problem["domain"]
        a, b = _fr(domain[0]), _fr(domain[1])
        claimed = [_fr(v) for v in coeffs_step["b"]]
        recomputed = bernstein_coeffs(raw_coeffs, a, b)
    except (KeyError, ValueError, IndexError, TypeError, ZeroDivisionError) as exc:
        return {"valid": False, "reason": f"malformed certificate: {exc}"}

    checked = 0
    # assert_coeffs_match: the producer's Bernstein coefficients must be exactly
    # what the monomial data yields (independent recomputation).
    if len(claimed) != len(recomputed):
        return {"valid": False, "reason": "bernstein coefficient count mismatch"}
    if claimed != recomputed:
        return {"valid": False, "reason": "bernstein coefficients do not match the polynomial"}
    checked += 1
    # assert_nonneg: all coefficients >= 0 certifies p >= 0 on [a, b].
    neg = [i for i, bk in enumerate(recomputed) if bk < 0]
    if neg:
        return {"valid": False,
                "reason": f"negative Bernstein coefficient at index {neg[0]} "
                          "-> nonnegativity NOT certified"}
    checked += 1

    if "degree" in coeffs_step and coeffs_step["degree"] != len(raw_coeffs) - 1:
        return {"valid": False, "reason": "declared degree does not match the coefficient list"}

    return {"valid": True, "reason": "ok", "checked_steps": checked,
            "kind": KIND, "claim": log.get("claim")}


def run(request: dict) -> dict:
    """Worker entry point. ``op == "export"`` builds a certificate from
    ``coeffs`` + ``interval`` (+ optional ``var``/``claim``); ``op == "check"``
    validates a ``log``."""
    op = request.get("op", "check")
    if op == "export":
        log = export_bernstein_cert(
            request["coeffs"],
            request["interval"],
            var=request.get("var", "x"),
            claim=request.get("claim"),
        )
        return {"ok": True, "log": log}
    if op == "check":
        return check(request["log"])
    raise ValueError(f"unknown op: {op!r}")
