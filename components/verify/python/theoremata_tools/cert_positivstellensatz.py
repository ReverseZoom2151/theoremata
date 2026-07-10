"""Positivstellensatz **infeasibility** proof-log exporter + pure checker.

This is the unifying nonlinear sibling of *both* the linear Farkas certificate
in :mod:`theoremata_tools.cert_log` and the sums-of-squares certificate in
:mod:`theoremata_tools.cert_sos`.  It certifies that a **polynomial constraint
system**

    { p_i = 0   (equalities)         ,
      q_j >= 0   (non-strict) ,
      r_k >  0   (strict)             }

is **INFEASIBLE** (has no real solution) by exhibiting a *Positivstellensatz
refutation identity*

    P + Q + R  ==  0           (as a polynomial identity in the variables)

whose three parts are, respectively, provably ``= 0`` / ``>= 0`` / ``> 0`` on
the feasible set:

* ``P = Sum_i a_i * p_i``   -- the **ideal** part.  Each cofactor ``a_i`` is an
  *arbitrary* polynomial; because ``p_i = 0`` on the feasible set, ``P = 0``
  there.
* ``Q = Sum_S sigma_S * prod_{j in S} q_j``   -- the **cone / preordering**
  part.  ``S`` ranges over (multi)subsets of the non-strict generators, each
  ``sigma_S`` is a **sum of squares** (SOS), so ``sigma_S >= 0`` everywhere and
  the products of ``q_j >= 0`` are ``>= 0`` on the feasible set: ``Q >= 0``.
* ``R = c * prod_{k in T} r_k``   -- the **strict** part: a *positive rational
  constant* ``c`` times a product of strict generators (``T`` a subset of the
  strict indices; ``T`` empty gives the pure ``"+1"`` contradiction term).
  Because ``c > 0`` and each ``r_k > 0`` on the feasible set, ``R > 0`` there.

If such an identity holds, then at *any* feasible point
``0 = P + Q + R = 0 + (>= 0) + (> 0) > 0`` -- a contradiction -- so the system
has no solution.  This single scheme specializes to:

* **LP / Farkas** when all parts are linear (``P`` + a cone of degree-0
  ``sigma_S`` times linear ``q_j``, ``R = c``): the nonnegative multiplier
  combination summing to a negative constant *is* ``Q + R = 0``.
* **SOS nonnegativity** when the cone is a single ``sigma_emptyset`` (``Q`` only).

Soundness boundary
------------------
:func:`check` is the sound boundary and trusts **nothing** from the producer.
It RE-EXPANDS ``P + Q + R`` from the *declared* system polynomials, the supplied
cofactors and the supplied SOS multipliers -- with exact rational arithmetic
(sympy over ``QQ``) -- and asserts it is *exactly* the zero polynomial.  It
independently re-verifies that every ``sigma_S`` is a genuine SOS (either
manifestly, as a sum of explicit squares, or via the exact-rational Gram /
``LDL^T`` PSD congruence test reused from :mod:`cert_sos`), that the strict
constant is ``> 0``, and that every index references a declared generator.  A
tampered cofactor, a non-SOS multiplier, or a non-zero sum is REJECTED.

Because finding the SOS multipliers is an SDP problem (as in ``cert_sos``'s
multivariate case), the **GENERATOR is gated** -- :data:`generate` is a
documented stub -- but the module is *checker-complete*: any refutation produced
by an external solver can be validated here without trusting the solver.

Worker dispatch key: ``cert_positivstellensatz`` (see :func:`run`).
"""
from __future__ import annotations

from fractions import Fraction
from typing import Any, Callable, Iterable, Optional

import sympy
from sympy import Rational, Symbol, expand, sympify

# Reuse cert_sos's exact helpers so the trust-critical arithmetic is shared with
# the SOS checker: rational parsing, poly (de)serialization, the monomial-vector
# builder and -- crucially -- the exact-rational PSD witness test (LDL^T).
from theoremata_tools.cert_sos import (
    _R,
    _expr_to_polydict,
    _find_negative_point,
    _monomial_expr,
    _polydict_to_expr,
    _psd_witness,
    _sym,
)

FORMAT = "theoremata.cert-log.v1"
KIND = "positivstellensatz"
KINDS = ("positivstellensatz",)


# --------------------------------------------------------------------------- #
# GENERATOR (SDP-gated stub).
# --------------------------------------------------------------------------- #

# Finding the SOS multipliers ``sigma_S`` and the ideal cofactors ``a_i`` of a
# Positivstellensatz refutation is a semidefinite-programming search (a
# numerically-solved Gram matrix must then be rounded/projected to an EXACT
# rational PSD matrix, and the cofactors solved over a monomial template).  As in
# ``cert_sos``, that solver + exact-rounding step is intentionally NOT bundled
# here.  The CHECKER below is pure and complete on its own, so a refutation
# produced by any external solver can be validated without trusting it.  Supply
# such a refutation to :func:`export_positivstellensatz_cert`.
generate: Optional[Callable] = None


# --------------------------------------------------------------------------- #
# SOS-multiplier (de)serialization + genuine-SOS verification.
# --------------------------------------------------------------------------- #
#
# A cone multiplier ``sigma`` is supplied in one of two checker-verifiable forms:
#
#   {"squares": [polydict, ...]}                  sigma = Sum s_i^2   (manifest)
#   {"monomials": [[e,...], ...], "Q": [[...]]}   sigma = z^T Q z, Q PSD (Gram)
#
# Both yield a concrete polynomial (used verbatim in the identity) plus an
# independent nonnegativity proof.  The Gram form reuses cert_sos._psd_witness.

def _sos_spec_to_dict(sos: Any, gens: list[Symbol]) -> dict:
    """Serialize an SOS-multiplier spec (sympy exprs) to plain JSON."""
    if "squares" in sos:
        return {"squares": [_expr_to_polydict(s, gens) for s in sos["squares"]]}
    if "monomials" in sos and "Q" in sos:
        return {"monomials": [[int(e) for e in m] for m in sos["monomials"]],
                "Q": [[str(_R(v)) for v in row] for row in sos["Q"]]}
    raise ValueError("SOS spec needs 'squares' or ('monomials','Q')")


def _eval_sos(spec: Any, gens: list[Symbol]) -> tuple[Any, bool, str, Optional[dict]]:
    """Rebuild ``sigma`` and independently decide whether it is a genuine SOS.

    Returns ``(sigma_expr, is_sos, reason, witness)``.  For the explicit-squares
    form ``sigma = Sum s_i^2`` is manifestly SOS.  For the Gram form it rebuilds
    ``sigma = z^T Q z`` and runs the exact-rational PSD test; a non-PSD ``Q``
    yields ``is_sos=False`` with a ``sigma(u) < 0`` witness when one exists.
    """
    if not isinstance(spec, dict):
        raise ValueError("SOS spec must be an object")

    if "squares" in spec:
        squares = spec["squares"]
        if not isinstance(squares, list):
            raise ValueError("'squares' must be a list")
        sigma = Rational(0)
        for s in squares:
            se, _g = _polydict_to_expr(s)
            sigma += se ** 2
        return expand(sigma), True, "sum of explicit squares (manifestly SOS)", None

    if "monomials" in spec and "Q" in spec:
        monomials = spec["monomials"]
        Q = spec["Q"]
        if not (isinstance(monomials, list) and monomials):
            raise ValueError("'monomials' must be a non-empty list")
        n = len(monomials)
        for m in monomials:
            if len(m) != len(gens) or any(int(e) < 0 for e in m):
                raise ValueError("malformed monomial in SOS spec")
        if not (isinstance(Q, list) and len(Q) == n and all(len(r) == n for r in Q)):
            raise ValueError("Gram matrix shape != #monomials")
        Qr = [[_R(Q[i][j]) for j in range(n)] for i in range(n)]
        for i in range(n):
            for j in range(n):
                if Qr[i][j] != Qr[j][i]:
                    return Rational(0), False, "Gram matrix is not symmetric", None
        z = [_monomial_expr(gens, list(m)) for m in monomials]
        sigma = expand(sum(Qr[i][j] * z[i] * z[j]
                           for i in range(n) for j in range(n)))
        psd, direction = _psd_witness(Qr)
        if psd:
            return sigma, True, "sigma = z^T Q z with Q PSD (exact)", None
        witness = _find_negative_point(sigma, gens, [list(m) for m in monomials],
                                       direction)
        # Duplicate monomials can make the represented polynomial nonnegative
        # everywhere even though the supplied Gram matrix is indefinite. In
        # that case a point witness cannot exist; retain the exact negative
        # Gram direction as the independently checkable witness.
        if witness is None and direction is not None:
            witness = {
                "gram_direction": [str(v) for v in direction],
                "note": "v^T Q v < 0 although the represented polynomial has no negative point",
            }
        return sigma, False, "Gram multiplier is not PSD", witness

    raise ValueError("SOS spec needs 'squares' or ('monomials','Q')")


# --------------------------------------------------------------------------- #
# Exporter -> theoremata.cert-log.v1, kind "positivstellensatz".
# --------------------------------------------------------------------------- #

def export_positivstellensatz_cert(*, gens: Iterable, equalities: Iterable = (),
                                   nonstrict: Iterable = (), strict: Iterable = (),
                                   ideal: Iterable = (), cone: Iterable = (),
                                   strict_part: Optional[dict] = None,
                                   claim: Optional[str] = None) -> dict:
    """Serialize an *externally-supplied* Positivstellensatz refutation.

    Parameters (sympy exprs unless noted):

    * ``gens``        -- the polynomial variables (symbols or names).
    * ``equalities``  -- the ``p_i`` (each ``p_i = 0``).
    * ``nonstrict``   -- the ``q_j`` (each ``q_j >= 0``).
    * ``strict``      -- the ``r_k`` (each ``r_k > 0``).
    * ``ideal``       -- ``[{"equality_index": i, "cofactor": expr}, ...]`` giving
      ``P = Sum a_i p_i``.
    * ``cone``        -- ``[{"cone_indices": [j,...], "sos": <spec>}, ...]`` giving
      ``Q = Sum sigma_S prod_{j in S} q_j``; ``<spec>`` is ``{"squares":[...]}`` or
      ``{"monomials":[...],"Q":[[...]]}``.
    * ``strict_part`` -- ``{"const": c, "strict_indices": [k,...]}`` giving
      ``R = c prod_{k in T} r_k`` (defaults to ``{"const": 1}`` -> the ``"+1"``
      contradiction term).

    Emits a self-describing log the checker re-verifies from scratch.
    """
    gens = [(_sym(str(g)) if not isinstance(g, Symbol) else g) for g in gens]
    equalities = [sympify(p) for p in equalities]
    nonstrict = [sympify(q) for q in nonstrict]
    strict = [sympify(r) for r in strict]

    ideal_out = []
    for entry in ideal:
        ideal_out.append({"equality_index": int(entry["equality_index"]),
                          "cofactor": _expr_to_polydict(entry["cofactor"], gens)})
    cone_out = []
    for entry in cone:
        cone_out.append({"cone_indices": [int(j) for j in entry.get("cone_indices", [])],
                        "sos": _sos_spec_to_dict(entry["sos"], gens)})

    sp = strict_part or {"const": 1}
    strict_out = {"const": str(_R(sp.get("const", 1))),
                  "strict_indices": [int(k) for k in sp.get("strict_indices", [])]}

    steps = [
        {"op": "psatz_system", "vars": [str(g) for g in gens],
         "equalities": [_expr_to_polydict(p, gens) for p in equalities],
         "nonstrict": [_expr_to_polydict(q, gens) for q in nonstrict],
         "strict": [_expr_to_polydict(r, gens) for r in strict],
         "note": "certify { p_i=0, q_j>=0, r_k>0 } is infeasible via P+Q+R==0"},
        {"op": "psatz_refutation", "ideal": ideal_out, "cone": cone_out,
         "strict_part": strict_out,
         "note": "P=Sum a_i p_i (=0), Q=Sum sigma_S prod q_j (>=0), R=c prod r_k (>0)"},
        {"op": "assert_psatz_identity"},
    ]
    return {
        "format": FORMAT, "kind": KIND,
        "claim": claim or "the polynomial system is infeasible (Positivstellensatz)",
        "steps": steps,
        "meta": {"producer": "cert_positivstellensatz", "generator": "external"},
    }


# --------------------------------------------------------------------------- #
# REFERENCE CHECKER.
# --------------------------------------------------------------------------- #

class _Reject(Exception):
    """Raised to reject a certificate with a human-readable reason."""


def _need(cond: bool, reason: str) -> None:
    if not cond:
        raise _Reject(reason)


def _prod(exprs: list) -> Any:
    out = Rational(1)
    for e in exprs:
        out *= e
    return out


def _check_refutation(steps: list, log: dict) -> dict:
    _need(len(steps) >= 3, "psatz: need system, refutation, and identity steps")
    system, refut, concl = steps[0], steps[1], steps[2]
    _need(system.get("op") == "psatz_system", "first step must be psatz_system")
    _need(refut.get("op") == "psatz_refutation", "second step must be psatz_refutation")
    _need(concl.get("op") == "assert_psatz_identity",
          "third step must be assert_psatz_identity")

    names = system.get("vars")
    _need(isinstance(names, list) and names, "psatz_system: vars must be a non-empty list")
    gens = [_sym(v) for v in names]

    def _polys(key: str) -> list:
        raw = system.get(key, [])
        _need(isinstance(raw, list), f"psatz_system: {key} must be a list")
        out = []
        for d in raw:
            e, g = _polydict_to_expr(d)
            _need([str(s) for s in g] == names, f"{key}: variable mismatch")
            out.append(e)
        return out

    P_eq = _polys("equalities")   # p_i, each = 0
    Q_ns = _polys("nonstrict")    # q_j, each >= 0
    R_st = _polys("strict")       # r_k, each > 0

    # ----- ideal part P = Sum a_i * p_i ------------------------------------- #
    P = Rational(0)
    ideal = refut.get("ideal", [])
    _need(isinstance(ideal, list), "ideal must be a list")
    for entry in ideal:
        i = int(entry["equality_index"])
        _need(0 <= i < len(P_eq), f"ideal: equality index {i} out of range")
        a_i, g = _polydict_to_expr(entry["cofactor"])
        _need([str(s) for s in g] == names, "ideal cofactor: variable mismatch")
        P += a_i * P_eq[i]

    # ----- cone part Q = Sum sigma_S * prod_{j in S} q_j -------------------- #
    Q = Rational(0)
    cone = refut.get("cone", [])
    _need(isinstance(cone, list), "cone must be a list")
    for entry in cone:
        idxs = entry.get("cone_indices", [])
        _need(isinstance(idxs, list), "cone_indices must be a list")
        for j in idxs:
            _need(0 <= int(j) < len(Q_ns), f"cone: nonstrict index {j} out of range")
        sigma, is_sos, reason, witness = _eval_sos(entry["sos"], gens)
        if not is_sos:
            exc = _Reject(f"cone multiplier is not a genuine SOS ({reason})")
            exc.witness = witness
            raise exc
        Q += sigma * _prod([Q_ns[int(j)] for j in idxs])

    # ----- strict part R = c * prod_{k in T} r_k --------------------------- #
    sp = refut.get("strict_part") or {}
    _need(isinstance(sp, dict), "strict_part must be an object")
    c = _R(sp.get("const", 0))
    _need(c > 0, "strict_part: constant must be > 0 (need a strictly positive term)")
    t_idx = sp.get("strict_indices", [])
    _need(isinstance(t_idx, list), "strict_indices must be a list")
    for k in t_idx:
        _need(0 <= int(k) < len(R_st), f"strict_part: strict index {k} out of range")
    R = c * _prod([R_st[int(k)] for k in t_idx])

    # ----- the refutation identity: P + Q + R == 0 ------------------------- #
    _need(expand(P + Q + R) == 0,
          "Positivstellensatz identity does not hold: P + Q + R != 0")
    return {"valid": True,
            "reason": "Positivstellensatz refutation re-verified exactly (P+Q+R==0, "
                      "cone SOS, strict const > 0)",
            "checked_steps": 3, "kind": KIND, "claim": log.get("claim"),
            "witness": None}


def check(log: Any) -> dict:
    """Independently RE-VERIFY a Positivstellensatz cert-log (the sound boundary).

    Returns ``{valid, reason, checked_steps, kind, claim, witness}``.  Rebuilds
    ``P + Q + R`` from the declared system + supplied cofactors/multipliers with
    exact arithmetic and asserts it is the zero polynomial; re-verifies each cone
    multiplier is a genuine SOS and the strict constant is ``> 0``.  A tampered
    cofactor, non-SOS multiplier, non-zero sum, or bad index yields
    ``valid=False`` (with a ``sigma(u) < 0`` witness for a non-PSD Gram
    multiplier when one exists).
    """
    try:
        _need(isinstance(log, dict), "log is not a JSON object")
        _need(log.get("format") == FORMAT, f"unknown format: {log.get('format')!r}")
        _need(log.get("kind") == KIND, f"unknown kind: {log.get('kind')!r}")
        steps = log.get("steps")
        _need(isinstance(steps, list) and steps, "steps must be a non-empty list")
        _need(isinstance(log.get("claim", ""), str), "claim must be a string")

        op0 = steps[0].get("op") if isinstance(steps[0], dict) else None
        _need(op0 == "psatz_system", f"unknown leading op {op0!r} for kind {KIND!r}")
        return _check_refutation(steps, log)
    except _Reject as exc:
        wit = getattr(exc, "witness", None)
        return {"valid": False, "reason": str(exc), "checked_steps": 0,
                "kind": log.get("kind") if isinstance(log, dict) else None,
                "claim": log.get("claim") if isinstance(log, dict) else None,
                "witness": wit}
    except (KeyError, IndexError, TypeError, ValueError, ZeroDivisionError) as exc:
        return {"valid": False, "reason": f"malformed cert ({exc})",
                "checked_steps": 0,
                "kind": log.get("kind") if isinstance(log, dict) else None,
                "claim": log.get("claim") if isinstance(log, dict) else None,
                "witness": None}


# --------------------------------------------------------------------------- #
# Worker dispatch.
# --------------------------------------------------------------------------- #

def run(request: dict) -> dict:
    """Worker entrypoint.  ``request["op"]`` is ``export`` or ``check``.

    * ``export`` -> build a cert-log document from an externally-supplied
      refutation: needs ``gens`` and the ``equalities``/``nonstrict``/``strict``
      system plus ``ideal``/``cone``/``strict_part``.  Returns ``{"log": ...}``.
    * ``check`` -> :func:`check` on ``request["log"]``.
    """
    op = request.get("op", "check")
    if op == "check":
        return check(request["log"])
    if op == "export":
        log = export_positivstellensatz_cert(
            gens=request["gens"],
            equalities=request.get("equalities", ()),
            nonstrict=request.get("nonstrict", ()),
            strict=request.get("strict", ()),
            ideal=request.get("ideal", ()),
            cone=request.get("cone", ()),
            strict_part=request.get("strict_part"),
            claim=request.get("claim"))
        return {"log": log}
    raise ValueError(f"unknown op: {op!r}")


def main() -> None:  # pragma: no cover
    import json
    import sys
    if len(sys.argv) >= 2:
        with open(sys.argv[1], encoding="utf-8") as fh:
            request = json.load(fh)
    else:
        request = json.load(sys.stdin)
    print(json.dumps(run(request), indent=2, default=str))
    raise SystemExit(0)


if __name__ == "__main__":  # pragma: no cover
    main()
