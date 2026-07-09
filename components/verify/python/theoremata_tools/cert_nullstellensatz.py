"""Nullstellensatz / Gröbner **cofactor** certificate exporter + independent checker.

This is the *algebraic-ideal* companion to the Wu pseudo-remainder certificate
in :mod:`theoremata_tools.cert_log` (kind ``wu_geometry``).  Where Wu's method
certifies "goal is an algebraic consequence of the hypotheses" by a successive
pseudo-remainder that vanishes, this module certifies the two dual facts a
**cofactor representation** gives directly:

* **Weak Nullstellensatz** — a family ``p_1, ..., p_r`` has *no common zero* iff
  ``1`` lies in the ideal ``<p_1, ..., p_r>``, witnessed by cofactors ``q_i``
  with ``Σ q_i p_i = 1`` (Hilbert's weak Nullstellensatz).
* **Ideal membership** — a goal polynomial ``g`` is in ``<p_1, ..., p_r>``,
  witnessed by ``g = Σ q_i p_i`` (the polynomial-arithmetic core behind an
  equational-geometry consequence).

The certificate is a ``theoremata.cert-log.v1`` document (same self-describing,
transport-neutral shape as ``cert_log.py``) of a NEW ``kind = "nullstellensatz"``.
Its steps carry the generators ``p_i``, the cofactors ``q_i`` and the target
(``1`` or ``g``) as **sympy-serializable polynomial dicts** (a monomial-exponent
list plus a rational-coefficient string), so the log is plain JSON.

Reuse-first (per project principle): sympy already implements Gröbner bases and
ideal arithmetic, so we DO NOT reimplement Buchberger.  The producer obtains the
quotients with

* :func:`sympy.polys.polytools.reduced` — the multivariate division algorithm,
  which yields cofactors *directly with respect to the given generators* when the
  target divides to a zero remainder (the ideal-membership fast path), and
* :meth:`sympy.polys.agca.ideals.Ideal.in_terms_of_generators` — sympy's
  syzygy/Gröbner-backed representation, which returns exact cofactors w.r.t. the
  ORIGINAL generators even for the weak-Nullstellensatz ``1 ∈ <p_i>`` case that
  plain division cannot reach.

:func:`sympy.groebner` decides membership independently (target reduces to ``0``
modulo the Gröbner basis) before any cofactors are trusted.

Soundness boundary
------------------
Exactly as in ``cert_log.py``, the **checker is the sound boundary**.
:func:`check` never trusts the producer: it re-reads the raw serialized
``p_i``/``q_i``/target, RE-EXPANDS ``Σ q_i p_i`` with its own exact-rational
polynomial arithmetic (sympy ``Poly`` over ``QQ``) and asserts it equals the
target *exactly*.  Any tampered cofactor makes the re-expansion differ from the
target and is REJECTED with a ``reason``.  All inputs are treated as UNTRUSTED
DATA and malformed input becomes ``valid=False`` rather than an exception.

Everything is exact (``sympy.Rational`` / ``QQ``), deterministic and offline.

Worker dispatch key: ``cert_nullstellensatz`` (see :func:`run`).
"""
from __future__ import annotations

from typing import Any, Optional, Sequence

import sympy
from sympy import Integer, Poly, QQ, Rational, expand, sympify
from sympy.polys.orderings import monomial_key
from sympy.polys.polytools import groebner, reduced

FORMAT = "theoremata.cert-log.v1"
KIND = "nullstellensatz"

# Same upgrade-path note the sibling checker carries: this reference checker is
# the offline stand-in for a machine-verified checker that consumes the same
# theoremata.cert-log.v1 documents unchanged.
CAKEML_TARGET = (
    "cake_certlog_check: a CakeML/HOL4-verified checker whose specification is "
    "the per-step semantics of this module; consumes theoremata.cert-log.v1 "
    "documents unchanged. This sympy reference checker is the offline stand-in."
)


# --------------------------------------------------------------------------- #
# Polynomial <-> JSON serialization (sympy Poly over QQ, exact).
# --------------------------------------------------------------------------- #

def _as_expr(p: Any) -> Any:
    """Accept a sympy expression, an int/Rational, or a string; return an expr."""
    if isinstance(p, str):
        return sympify(p, rational=True)
    return sympify(p, rational=True)


def _serialize_poly(expr: Any, gens: Sequence) -> dict:
    """Serialize ``expr`` as ``{"terms": [[ [e1,..,en], "coeff" ], ...]}``.

    Coefficients are exact rationals rendered via ``str`` (``"p"`` or ``"p/q"``)
    so the document is plain JSON with no float drift.  Monomials are sorted by
    ``lex`` for a deterministic, canonical serialization.
    """
    poly = Poly(_as_expr(expr), *gens, domain="QQ")
    key = monomial_key("lex")
    terms = sorted(poly.terms(), key=lambda mc: key(mc[0]), reverse=True)
    return {"terms": [[list(int(e) for e in monom), str(Rational(coeff))]
                      for monom, coeff in terms]}


def _deserialize_poly(obj: Any, gens: Sequence) -> Poly:
    """Rebuild a sympy ``Poly`` over ``QQ`` from a serialized poly dict.

    Independent of the producer: every coefficient is re-parsed exactly and the
    ``Poly`` is reconstructed from raw exponent tuples.  Malformed input raises,
    which the checker converts into a rejection.
    """
    if not isinstance(obj, dict) or "terms" not in obj:
        raise ValueError("malformed polynomial (missing 'terms')")
    n = len(gens)
    data: dict[tuple, Any] = {}
    for entry in obj["terms"]:
        exp, coeff = entry
        exp = tuple(int(e) for e in exp)
        if len(exp) != n or any(e < 0 for e in exp):
            raise ValueError("malformed monomial (arity/negativity)")
        data[exp] = Rational(str(coeff))
    if not data:
        return Poly(Integer(0), *gens, domain="QQ")
    return Poly.from_dict(data, *gens, domain="QQ")


# --------------------------------------------------------------------------- #
# Rabinowitsch trick: turn an inequation into an equation with a fresh variable.
# --------------------------------------------------------------------------- #

def rabinowitsch(a: Any, b: Any, z: Any) -> Any:
    """Normal-form polynomial encoding the inequation ``a != b``.

    Hilbert's Rabinowitsch trick: ``a != b`` (equivalently ``a - b`` is
    invertible) iff there exists a fresh ``z`` with ``(a - b)*z + 1 = 0``.
    Adjoining this polynomial as an extra generator lets an inequation hypothesis
    enter a purely equational Nullstellensatz/ideal computation.

    Returns the sympy expression ``(a - b)*z + 1``.
    """
    a, b, z = _as_expr(a), _as_expr(b), _as_expr(z)
    return expand((a - b) * z + 1)


# --------------------------------------------------------------------------- #
# Cofactor computation (reuses sympy: reduced + Gröbner/ideal machinery).
# --------------------------------------------------------------------------- #

def _cofactors(target: Any, polys: Sequence, gens: Sequence, order: str) -> list:
    """Return cofactors ``[q_i]`` (sympy exprs) with ``Σ q_i p_i == target``.

    Membership is DECIDED with a Gröbner basis (``target`` reduces to ``0`` modulo
    ``groebner(polys)``); the cofactors are then taken w.r.t. the ORIGINAL
    generators.  Fast path: the ``reduced`` division algorithm already yields such
    cofactors when it leaves a zero remainder.  Otherwise (e.g. the weak
    Nullstellensatz ``1 ∈ <p_i>``, which plain division cannot witness) sympy's
    ideal ``in_terms_of_generators`` — Gröbner/syzygy backed — supplies them.
    Raises ``ValueError`` if ``target`` is not in the ideal.
    """
    tgt = _as_expr(target)
    gen_exprs = [_as_expr(p) for p in polys]

    # Independent membership decision via a Gröbner basis.
    gb = groebner(gen_exprs, *gens, order=order)
    _q, rem = reduced(tgt, gb.exprs, *gens, order=order)
    if expand(rem) != 0:
        raise ValueError("target is not in the ideal generated by the polynomials")

    # Fast path: division by the original generators already gives cofactors.
    qs, rem0 = reduced(tgt, gen_exprs, *gens, order=order)
    if expand(rem0) == 0:
        return [expand(q) for q in qs]

    # General path: exact representation w.r.t. the original generators.
    ring = QQ.old_poly_ring(*gens, order=order)
    ideal = ring.ideal(*gen_exprs)
    cof = ideal.in_terms_of_generators(ring.convert(tgt))
    return [expand(ring.to_sympy(c)) for c in cof]


# --------------------------------------------------------------------------- #
# Exporter.
# --------------------------------------------------------------------------- #

def export_nullstellensatz_cert(polys: Sequence, *, gens: Sequence,
                                target: Any = 1, order: str = "lex",
                                mode: Optional[str] = None,
                                claim: Optional[str] = None) -> dict:
    """Export a ``kind = "nullstellensatz"`` cofactor certificate.

    ``polys`` are the generators ``p_i`` (sympy exprs, ints, or strings); ``gens``
    is the ordered list of ring variables.  ``target`` is ``1`` for the weak
    Nullstellensatz (no common zero) or a goal polynomial ``g`` for ideal
    membership; ``mode`` (``"nullstellensatz"``/``"membership"``) is inferred from
    the target when omitted.

    The emitted document carries the ``p_i``, the computed cofactors ``q_i`` and
    the target as serialized poly dicts, plus a single ``assert`` step the checker
    re-verifies by re-expanding ``Σ q_i p_i`` and comparing to the target.
    """
    gens = [sympify(g) for g in gens]
    tgt = _as_expr(target)
    if mode is None:
        mode = "nullstellensatz" if tgt == 1 else "membership"

    cofactors = _cofactors(tgt, polys, gens, order)

    steps = [
        {"op": "declare_ring", "nvars": len(gens),
         "names": [str(g) for g in gens], "order": order},
        {"op": "generators", "polys": [_serialize_poly(p, gens) for p in polys]},
        {"op": "cofactors", "polys": [_serialize_poly(q, gens) for q in cofactors]},
        {"op": "target", "mode": mode, "poly": _serialize_poly(tgt, gens)},
        {"op": "assert_combination_equals_target"},
    ]
    if claim is None:
        if mode == "nullstellensatz":
            claim = ("the polynomials have no common zero: 1 = Sum q_i p_i "
                     "(weak Nullstellensatz cofactor certificate)")
        else:
            claim = "target g is in the ideal <p_i>: g = Sum q_i p_i"
    return {
        "format": FORMAT,
        "kind": KIND,
        "claim": claim,
        "steps": steps,
        "meta": {
            "producer": "cert_nullstellensatz.export_nullstellensatz_cert",
            "method": "sympy groebner/reduced + ideal.in_terms_of_generators",
            "order": order,
            "cakeml_target": CAKEML_TARGET,
        },
    }


# --------------------------------------------------------------------------- #
# REFERENCE CHECKER (sound boundary; independent re-expansion).
# --------------------------------------------------------------------------- #

class _Reject(Exception):
    """Raised to reject a certificate with a human-readable reason."""


def _need(cond: bool, reason: str) -> None:
    if not cond:
        raise _Reject(reason)


def check(log: Any) -> dict:
    """Independently RE-VERIFY a ``nullstellensatz`` cert-log document.

    Returns ``{valid, reason, checked_steps, kind, claim}``.  The checker rebuilds
    every ``p_i``, ``q_i`` and the target from the raw serialized rationals, then
    RE-EXPANDS ``Σ q_i p_i`` with its own exact ``Poly`` arithmetic over ``QQ`` and
    asserts it equals the target exactly.  It never trusts the producer: a
    tampered cofactor (or generator, or target) makes the recomputed combination
    differ and yields ``valid=False``.  Malformed input is rejected, not raised.
    """
    checked = 0
    try:
        _need(isinstance(log, dict), "log is not a JSON object")
        _need(log.get("format") == FORMAT, f"unknown format: {log.get('format')!r}")
        _need(log.get("kind") == KIND, f"unknown kind: {log.get('kind')!r}")
        steps = log.get("steps")
        _need(isinstance(steps, list) and steps, "steps must be a non-empty list")
        _need(isinstance(log.get("claim", ""), str), "claim must be a string")

        ctx: dict[str, Any] = {}
        gens: Optional[list] = None
        for i, step in enumerate(steps):
            _need(isinstance(step, dict), f"step {i} is not an object")
            op = step.get("op")
            try:
                if op == "declare_ring":
                    n = int(step["nvars"])
                    names = list(step["names"])
                    _need(len(names) == n, "declare_ring: #names != nvars")
                    _need(len(set(names)) == n, "declare_ring: duplicate variable")
                    gens = [sympify(x) for x in names]
                    ctx["gens"] = gens
                elif op == "generators":
                    _need(gens is not None, "generators before declare_ring")
                    ctx["p"] = [_deserialize_poly(p, gens) for p in step["polys"]]
                elif op == "cofactors":
                    _need(gens is not None, "cofactors before declare_ring")
                    ctx["q"] = [_deserialize_poly(q, gens) for q in step["polys"]]
                elif op == "target":
                    _need(gens is not None, "target before declare_ring")
                    ctx["target"] = _deserialize_poly(step["poly"], gens)
                    ctx["mode"] = step.get("mode")
                elif op == "assert_combination_equals_target":
                    _need("p" in ctx and "q" in ctx and "target" in ctx,
                          "assert before generators/cofactors/target declared")
                    p, q, target = ctx["p"], ctx["q"], ctx["target"]
                    _need(len(p) == len(q),
                          f"#cofactors ({len(q)}) != #generators ({len(p)})")
                    # Independent re-expansion of Sum q_i p_i with exact arithmetic.
                    # Poly over QQ is canonical, so equality is exact term-by-term.
                    combo = Poly(Integer(0), *gens, domain="QQ")
                    for qi, pi in zip(q, p):
                        combo = combo + qi * pi
                    diff = combo - target
                    _need(diff.is_zero,
                          "cofactor identity FAILS: Sum q_i p_i - target != 0 "
                          f"(residual has {len(diff.terms())} term(s)); tampered "
                          "or invalid certificate")
                    # Weak-Nullstellensatz certs must actually target the unit 1.
                    if ctx.get("mode") == "nullstellensatz":
                        _need(target == Poly(Integer(1), *gens, domain="QQ"),
                              "nullstellensatz mode but target is not the constant 1")
                    ctx["concluded"] = True
                else:
                    raise _Reject(f"step {i}: unknown op {op!r}")
            except _Reject:
                raise
            except (KeyError, IndexError, TypeError, ValueError,
                    ZeroDivisionError, sympy.SympifyError) as exc:
                raise _Reject(f"step {i} ({op}): malformed data ({exc})")
            checked += 1

        _need(ctx.get("concluded"), "log reached no verified conclusion step")
        return {"valid": True, "reason": "cofactor identity independently re-expanded",
                "checked_steps": checked, "kind": KIND, "claim": log.get("claim")}
    except _Reject as exc:
        return {"valid": False, "reason": str(exc), "checked_steps": checked,
                "kind": log.get("kind") if isinstance(log, dict) else None,
                "claim": log.get("claim") if isinstance(log, dict) else None}


# --------------------------------------------------------------------------- #
# Worker dispatch.
# --------------------------------------------------------------------------- #

def run(request: dict) -> dict:
    """Worker entrypoint.  ``request["op"]`` is ``export`` or ``check``.

    * ``export`` -> build a certificate.  Requires ``polys`` and ``gens``;
      optional ``target`` (default ``1``), ``order`` (default ``"lex"``),
      ``mode`` and ``claim``.  Returns ``{"log": <document>}``.
    * ``check`` -> :func:`check` on ``request["log"]``.
    """
    op = request.get("op", "check")
    if op == "check":
        return check(request["log"])
    if op == "export":
        log = export_nullstellensatz_cert(
            request["polys"],
            gens=request["gens"],
            target=request.get("target", 1),
            order=request.get("order", "lex"),
            mode=request.get("mode"),
            claim=request.get("claim"),
        )
        return {"log": log}
    raise ValueError(f"unknown op: {op!r}")
