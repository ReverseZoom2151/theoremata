"""Wilf--Zeilberger (WZ) certificate exporter + self-contained reference checker.

For a hypergeometric summation identity ``Sum_k F(n, k) = RHS(n)`` the WZ method
supplies a *rational certificate* ``R(n, k)`` that reduces the whole identity to a
single, mechanically checkable rational-function equation.  Normalizing the
summand to ``F(n, k)`` with a constant sum (``Sum_k F(n, k) = 1``), the WZ pair
``(F, G)`` with ``G(n, k) = R(n, k) * F(n, k)`` satisfies the **telescoping
identity**

    F(n+1, k) - F(n, k) = G(n, k+1) - G(n, k).

Summing over all ``k`` collapses the right-hand side (telescoping), so
``Sum_k F(n, k)`` is independent of ``n`` -- the heart of the WZ proof.

This module follows the project rule *reuse existing libs first*: it uses
**sympy** (Gosper's algorithm / creative telescoping via
:func:`sympy.concrete.gosper.gosper_term`) to *find* ``R(n, k)``, and then
serializes the result into the shared ``theoremata.cert-log.v1`` proof-log
envelope (see :mod:`theoremata_tools.cert_log`).

The trust-critical part is the **checker** (:func:`check`): it re-parses ``F``
and ``R`` from the log, forms ``G = R * F`` with *its own* sympy session, and
symbolically simplifies

    (F(n+1, k) - F(n, k)) - (G(n, k+1) - G(n, k))

confirming it reduces to exactly ``0``.  A tampered ``R(n, k)`` breaks the
telescoping and is REJECTED.  The checker imports nothing from the producer path
(it only reads the serialized ``srepr`` strings) and never trusts a "verified"
flag.

Serialization uses sympy's :func:`~sympy.srepr` (canonical, round-trippable, and
assumption-preserving so the ``integer=True`` symbol assumptions survive) as the
load-bearing field, plus a human-readable ``str`` alongside it.

Worker dispatch key: ``cert_wz`` (see :func:`run`).
"""
from __future__ import annotations

from typing import Any, Optional

try:  # cert_log owns the shared envelope constant.
    from theoremata_tools.cert_log import FORMAT
except Exception:  # pragma: no cover - keep the module importable in isolation.
    FORMAT = "theoremata.cert-log.v1"

KIND = "wz"


class WZError(Exception):
    """Raised when a WZ certificate cannot be derived (e.g. not Gosper-summable)."""


# --------------------------------------------------------------------------- #
# sympy helpers (import lazily so importing this module never requires sympy).
# --------------------------------------------------------------------------- #

def _sympy():
    import sympy  # noqa: WPS433 (intentional lazy import)
    return sympy


def _resolve_symbol(sp, name: Any, *exprs):
    """Find the symbol named ``name`` inside ``exprs`` (so ``subs`` matches its
    assumptions); fall back to a fresh integer symbol if it does not occur."""
    name = str(name)
    for expr in exprs:
        for sym in getattr(expr, "free_symbols", ()):
            if str(sym) == name:
                return sym
    return sp.Symbol(name, integer=True)


def _parse(sp, s: str):
    """Parse a serialized expression (srepr string) back to a sympy expression."""
    return sp.sympify(s)


def _symbol_by_name(expr, name: str, sp):
    for sym in expr.free_symbols:
        if str(sym) == name:
            return sym
    # Not present in the expression: a fresh integer symbol shifts to a no-op.
    return sp.Symbol(name, integer=True)


# --------------------------------------------------------------------------- #
# Derivation: use sympy/Gosper to FIND the WZ certificate R(n, k).
# --------------------------------------------------------------------------- #

def derive_wz_certificate(summand: Any, rhs: Any = 1, *,
                          n: Any = "n", k: Any = "k",
                          normalize: bool = True) -> dict:
    """Derive the WZ pair for ``Sum_k summand = rhs`` using sympy's Gosper solver.

    Returns a dict of sympy expressions ``{"F", "R", "G", "n", "k"}`` where ``F``
    is the (optionally normalized) summand, ``R`` the rational WZ certificate and
    ``G = R * F``.  Raises :class:`WZError` if the required auxiliary sequence is
    not Gosper-summable (so no rational certificate exists via this route).
    """
    sp = _sympy()
    from sympy.concrete.gosper import gosper_term  # reuse sympy's Gosper impl.

    summand = sp.sympify(summand)
    rhs = sp.sympify(rhs)
    n_sym = _resolve_symbol(sp, n, summand, rhs)
    k_sym = _resolve_symbol(sp, k, summand, rhs)
    if normalize:
        if rhs == 0:
            raise WZError("cannot normalize: RHS(n) is identically zero")
        F = sp.simplify(summand / rhs)
    else:
        F = sp.simplify(summand)

    # a_k := F(n+1, k) - F(n, k); WZ needs a rational G with G(k+1)-G(k) = a_k.
    a = sp.simplify(F.subs(n_sym, n_sym + 1) - F)

    if a == 0:
        # F is independent of n; the sum is trivially constant. R = G = 0.
        R = sp.Integer(0)
        G = sp.Integer(0)
    else:
        term = gosper_term(a, k_sym)
        if term is None:
            raise WZError(
                "auxiliary sequence F(n+1,k)-F(n,k) is not Gosper-summable; "
                "no rational WZ certificate exists via Gosper's algorithm")
        G = sp.simplify(term * a)
        if F == 0:
            raise WZError("normalized summand F is identically zero")
        R = sp.simplify(G / F)

    # Sanity check at derivation time (the independent checker re-does this).
    residual = sp.simplify(
        (F.subs(n_sym, n_sym + 1) - F) - (G.subs(k_sym, k_sym + 1) - G))
    if sp.simplify(residual) != 0:
        raise WZError(
            f"derived certificate failed self-check (residual={residual}); "
            "the identity may not admit a WZ proof in this form")

    return {"F": F, "R": R, "G": G, "n": n_sym, "k": k_sym}


# --------------------------------------------------------------------------- #
# Exporter: serialize to the theoremata.cert-log.v1 envelope, kind "wz".
# --------------------------------------------------------------------------- #

def _expr_fields(sp, expr) -> dict:
    return {"srepr": sp.srepr(expr), "str": str(expr)}


def export_wz_cert(summand: Any, rhs: Any = 1, *,
                   n: Any = "n", k: Any = "k",
                   normalize: bool = True,
                   claim: Optional[str] = None) -> dict:
    """Derive and serialize a WZ certificate for ``Sum_k summand = rhs``.

    Returns a ``theoremata.cert-log.v1`` document with ``kind == "wz"`` whose
    steps carry the normalized summand ``F(n, k)``, the WZ certificate
    ``R(n, k)``, the original identity, and a single ``assert_wz_telescoping``
    conclusion.  All expressions are stored as sympy ``srepr`` (canonical,
    round-trippable) with a human-readable ``str`` alongside.

    Raises :class:`WZError` if no rational WZ certificate can be derived.
    """
    sp = _sympy()
    parts = derive_wz_certificate(summand, rhs, n=n, k=k, normalize=normalize)
    F, R = parts["F"], parts["R"]
    n_sym, k_sym = parts["n"], parts["k"]

    summand_e = sp.sympify(summand)
    rhs_e = sp.sympify(rhs)

    steps = [
        {"op": "wz_summand",
         "n": str(n_sym), "k": str(k_sym),
         "normalized": bool(normalize),
         "F": _expr_fields(sp, F),
         "note": "normalized summand: Sum_k F(n,k) is claimed constant in n"},
        {"op": "wz_certificate",
         "R": _expr_fields(sp, R),
         "note": "rational WZ certificate; G(n,k) = R(n,k) * F(n,k)"},
        {"op": "wz_identity",
         "summand": _expr_fields(sp, summand_e),
         "rhs": _expr_fields(sp, rhs_e),
         "note": "original identity Sum_k summand(n,k) = rhs(n)"},
        {"op": "assert_wz_telescoping"},
    ]
    return {
        "format": FORMAT,
        "kind": KIND,
        "claim": claim or f"Sum_k ({summand_e}) = {rhs_e} by a WZ certificate",
        "steps": steps,
        "meta": {
            "producer": "cert_wz.export_wz_cert",
            "method": "Wilf-Zeilberger (Gosper creative telescoping)",
            "sympy_apis": ["sympy.concrete.gosper.gosper_term", "sympy.simplify",
                           "sympy.srepr", "sympy.sympify"],
        },
    }


# --------------------------------------------------------------------------- #
# REFERENCE CHECKER: re-verify the WZ telescoping identity independently.
# --------------------------------------------------------------------------- #

class _Reject(Exception):
    """Raised to reject a certificate with a human-readable reason."""


def _need(cond: bool, reason: str) -> None:
    if not cond:
        raise _Reject(reason)


def _read_expr(sp, field: Any, what: str):
    _need(isinstance(field, dict) and "srepr" in field,
          f"{what}: missing serialized expression")
    try:
        return _parse(sp, field["srepr"])
    except Exception as exc:  # noqa: BLE001 - any parse failure rejects
        raise _Reject(f"{what}: cannot parse expression ({exc})")


def check(log: Any) -> dict:
    """Independently RE-VERIFY a ``kind == "wz"`` cert-log document.

    Recomputes ``G = R * F`` in a fresh sympy session and confirms the WZ
    telescoping identity ``(F(n+1,k) - F(n,k)) - (G(n,k+1) - G(n,k))`` simplifies
    to exactly ``0``.  Returns ``{valid, reason, checked_steps, kind, claim}``.
    A tampered ``R`` (or ``F``) breaks telescoping and yields ``valid=False`` --
    this is the sound boundary; the checker never trusts the producer.
    """
    checked = 0
    kind = log.get("kind") if isinstance(log, dict) else None
    claim = log.get("claim") if isinstance(log, dict) else None
    try:
        sp = _sympy()
    except Exception as exc:  # pragma: no cover - sympy required to check
        return {"valid": False, "reason": f"sympy unavailable ({exc})",
                "checked_steps": 0, "kind": kind, "claim": claim}
    try:
        _need(isinstance(log, dict), "log is not a JSON object")
        _need(log.get("format") == FORMAT, f"unknown format: {log.get('format')!r}")
        _need(log.get("kind") == KIND, f"not a WZ cert (kind={log.get('kind')!r})")
        _need(isinstance(log.get("claim", ""), str), "claim must be a string")
        steps = log.get("steps")
        _need(isinstance(steps, list) and steps, "steps must be a non-empty list")

        F = R = None
        n_name = k_name = None
        concluded = False

        for i, step in enumerate(steps):
            _need(isinstance(step, dict), f"step {i} is not an object")
            op = step.get("op")
            if op == "wz_summand":
                n_name = str(step.get("n", "n"))
                k_name = str(step.get("k", "k"))
                F = _read_expr(sp, step.get("F"), f"step {i} (wz_summand F)")
            elif op == "wz_certificate":
                R = _read_expr(sp, step.get("R"), f"step {i} (wz_certificate R)")
            elif op == "wz_identity":
                # Record only; the checker re-verifies telescoping, not bounds.
                _read_expr(sp, step.get("summand"), f"step {i} (summand)")
                _read_expr(sp, step.get("rhs"), f"step {i} (rhs)")
            elif op == "assert_wz_telescoping":
                _need(F is not None, "assert before summand F declared")
                _need(R is not None, "assert before certificate R declared")
                n_sym = _symbol_by_name(F, n_name or "n", sp)
                k_sym = _symbol_by_name(F, k_name or "k", sp)
                G = R * F
                lhs = F.subs(n_sym, n_sym + 1) - F
                rhs = G.subs(k_sym, k_sym + 1) - G
                residual = sp.simplify(lhs - rhs)
                _need(residual == 0,
                      "WZ telescoping FAILS: "
                      "(F(n+1,k)-F(n,k)) - (G(n,k+1)-G(n,k)) simplifies to "
                      f"{residual}, not 0 (certificate tampered or invalid)")
                concluded = True
            else:
                raise _Reject(f"step {i}: unknown op {op!r} for kind {KIND!r}")
            checked += 1

        _need(concluded, "log reached no verified conclusion (assert_wz_telescoping)")
        return {"valid": True, "reason": "WZ telescoping independently re-verified",
                "checked_steps": checked, "kind": KIND, "claim": log.get("claim")}
    except _Reject as exc:
        return {"valid": False, "reason": str(exc), "checked_steps": checked,
                "kind": kind, "claim": claim}
    except Exception as exc:  # noqa: BLE001 - malformed data must reject, not crash
        return {"valid": False, "reason": f"malformed WZ cert ({exc})",
                "checked_steps": checked, "kind": kind, "claim": claim}


# --------------------------------------------------------------------------- #
# Worker dispatch.
# --------------------------------------------------------------------------- #

def run(request: dict) -> dict:
    """Worker entrypoint.  ``request["op"]`` is ``export`` or ``check``.

    * ``export`` -> derive + serialize a WZ certificate.  Requires ``summand``;
      accepts ``rhs`` (default 1), ``n``/``k`` names, ``normalize``, ``claim``.
      Returns ``{"ok": True, "log": <document>}`` on success, or
      ``{"ok": False, "reason": ...}`` if not Gosper-summable (never crashes).
    * ``check`` -> :func:`check` on ``request["log"]``.
    """
    op = request.get("op", "check")
    if op == "check":
        return check(request["log"])
    if op == "export":
        try:
            log = export_wz_cert(
                request["summand"], request.get("rhs", 1),
                n=request.get("n", "n"), k=request.get("k", "k"),
                normalize=request.get("normalize", True),
                claim=request.get("claim"))
            return {"ok": True, "log": log}
        except WZError as exc:
            return {"ok": False, "reason": str(exc)}
    raise ValueError(f"unknown op: {op!r}")
