"""Wolfram Engine as an UNTRUSTED certificate GENERATOR.

A Wolfram Engine, when one happens to be installed, is very good at *finding*
the objects our certificates are made of: SOS multipliers, ideal-membership
cofactors, real-root counts.  It is not, and never becomes, an authority on
whether those objects are correct.  This module therefore uses Wolfram in
exactly one role: a search oracle whose output is immediately fed to the
EXISTING independent checkers in :mod:`theoremata_tools.cert_sos`,
:mod:`theoremata_tools.cert_nullstellensatz` and
:mod:`theoremata_tools.cert_sturm`.

Soundness boundary
------------------
Wolfram is outside it.  The checkers are it.  Structurally, this module has a
single funnel, :func:`_finalize`, which is the ONLY code path that can place a
``cert`` key in a response.  It runs the corresponding existing ``check()`` and
returns the certificate if and only if that checker answered ``valid is True``.
Every op returns through it, so there is no branch in this file that can emit an
unchecked certificate.

A rejection is a NORMAL outcome, not a bug
------------------------------------------
Wolfram's arithmetic, its interval conventions (``CountRoots`` counts *with
multiplicity* on the CLOSED interval ``[a, b]``; Sturm here counts *distinct*
roots on the half-open ``(a, b]``) and its normalizations differ from ours.  When
they disagree, our checker rejects and this module returns no certificate and
reports the checker's reason.  That is the system working.

Exactness
---------
Our checkers depend on exact rational arithmetic.  A machine float arriving from
Wolfram would silently destroy that, so every parsed coefficient is scanned for
inexact atoms and rejected outright (see :func:`_require_exact`).  The generated
Wolfram code asks for exact results (``Rationalize``/``Together``/``Rationals``)
rather than trusting the engine to volunteer them.

Non-determinism
---------------
Wolfram results can vary across engine versions, kernel settings and even run to
run (``FindInstance`` may return a different witness).  A generated certificate
is therefore RE-CHECKED on every single run and is never cached, memoized or
otherwise treated as trusted once it has passed.  There is no "already verified"
fast path in this module by design.

Availability
------------
When no engine or client is present, every op returns a clean ``unavailable``
response.  It never raises and never fabricates a certificate.

Worker dispatch key: ``wolfram_cert`` (see :func:`run`).
"""
from __future__ import annotations

from fractions import Fraction
from typing import Any, Callable

import sympy
from sympy import Symbol, expand, sympify

from theoremata_tools import cert_nullstellensatz as _ns
from theoremata_tools import cert_sos as _sos
from theoremata_tools import cert_sturm as _sturm

PRODUCER = "wolfram_cert"
OPS = ("sos", "nullstellensatz", "sturm")

# The shared bridge lives in a sibling component (components/tools/python).  It
# is imported defensively: a missing bridge is operationally identical to a
# missing engine, and must degrade to "unavailable" rather than to an ImportError
# at module load, which would take down the whole worker.
try:  # pragma: no cover - exercised only when the bridge is absent
    from theoremata_tools.wolfram_link import available, evaluate
except Exception:  # pragma: no cover
    def available() -> bool:
        return False

    def evaluate(code: str, *, timeout: float = 30.0) -> dict:
        return {"ok": False, "result": None, "unavailable": True,
                "error": "wolfram_link bridge is not importable"}


class _Reject(Exception):
    """Raised when Wolfram's output is unusable BEFORE it reaches a checker."""


# --------------------------------------------------------------------------- #
# Wolfram Language text -> exact sympy.
# --------------------------------------------------------------------------- #

def _wl_split(text: str) -> list[str]:
    """Split a Wolfram list ``"{a, b, {c, d}}"`` into its top-level elements.

    Brace-aware so nested lists survive as single elements.  Returns ``[]`` for
    the empty list ``"{}"``.  Raises on anything that is not a braced list, since
    a scalar where a list was expected means we misunderstood the engine's reply
    and must not guess.
    """
    s = text.strip()
    if not (s.startswith("{") and s.endswith("}")):
        raise _Reject(f"expected a Wolfram list, got {text!r}")
    body = s[1:-1].strip()
    if not body:
        return []
    out: list[str] = []
    depth = 0
    cur: list[str] = []
    for ch in body:
        if ch in "{[(":
            depth += 1
        elif ch in "}])":
            depth -= 1
            if depth < 0:
                raise _Reject("unbalanced brackets in Wolfram list")
        if ch == "," and depth == 0:
            out.append("".join(cur).strip())
            cur = []
        else:
            cur.append(ch)
    if depth != 0:
        raise _Reject("unbalanced brackets in Wolfram list")
    out.append("".join(cur).strip())
    return [e for e in out if e]


def _require_exact(expr: Any, where: str) -> Any:
    """Reject any expression carrying an inexact (machine-float) atom.

    Our checkers reconstruct everything over ``QQ``.  A ``Float`` slipping in
    would be silently re-parsed as an approximate rational and could make a
    WRONG certificate look right, so this is a hard rejection rather than a
    coercion.
    """
    bad = expr.atoms(sympy.Float)
    if bad:
        raise _Reject(f"{where}: inexact float coefficient(s) {sorted(map(str, bad))} "
                      "from Wolfram; exact rationals are required")
    return expr


def _wl_expr(text: str, where: str) -> Any:
    """Parse one Wolfram polynomial expression into an exact sympy expression."""
    s = text.strip()
    if not s:
        raise _Reject(f"{where}: empty expression")
    if "$Failed" in s or "Indeterminate" in s:
        raise _Reject(f"{where}: Wolfram returned {s!r}")
    # WL uses ^ for exponentiation; sympify reads ^ as XOR, so rewrite it.  Only
    # safe because these payloads are polynomials, never boolean expressions.
    s = s.replace("^", "**")
    try:
        expr = sympify(s)
    except (sympy.SympifyError, SyntaxError, TypeError) as exc:
        raise _Reject(f"{where}: unparseable Wolfram expression {text!r} ({exc})")
    return _require_exact(expr, where)


def _wl_int(text: str, where: str) -> int:
    """Parse a Wolfram integer reply; a float or non-integer is rejected."""
    s = text.strip()
    if "." in s or "`" in s:
        raise _Reject(f"{where}: inexact value {text!r} where an integer was required")
    try:
        val = Fraction(s)
    except (ValueError, ZeroDivisionError):
        raise _Reject(f"{where}: unparseable integer {text!r}")
    if val.denominator != 1:
        raise _Reject(f"{where}: non-integer value {text!r}")
    return int(val)


def _wl_number(x: Any) -> str:
    """Render a caller-supplied number as exact Wolfram source (never a float)."""
    f = Fraction(str(x)) if not isinstance(x, (int, Fraction)) else Fraction(x)
    return str(f.numerator) if f.denominator == 1 else f"({f.numerator}/{f.denominator})"


def _wl_poly_from_coeffs(coeffs: list, var: str) -> str:
    """Wolfram source for a univariate polynomial from ascending coefficients."""
    parts = []
    for i, c in enumerate(coeffs):
        parts.append(f"({_wl_number(c)})*{var}^{i}")
    return "(" + " + ".join(parts) + ")" if parts else "0"


def _call(code: str, timeout: float) -> str:
    """Evaluate Wolfram source; return the result text or raise ``_Reject``.

    Callers must have already established availability; this only converts an
    engine-level failure into a rejection.
    """
    res = evaluate(code, timeout=timeout)
    if not isinstance(res, dict):
        raise _Reject("wolfram_link.evaluate returned a non-dict response")
    if res.get("unavailable"):
        raise _Reject("wolfram engine became unavailable mid-request")
    if not res.get("ok"):
        raise _Reject(f"wolfram evaluation failed: {res.get('error')!r}")
    result = res.get("result")
    if not isinstance(result, str) or not result.strip():
        raise _Reject("wolfram returned an empty result")
    return result


# --------------------------------------------------------------------------- #
# THE FUNNEL.  The only place in this module that may emit a certificate.
# --------------------------------------------------------------------------- #

def _finalize(op: str, log: dict, checker: Callable[[Any], dict],
              method: str) -> dict:
    """Run ``checker`` on ``log`` and return the cert ONLY if it was accepted.

    This is the structural enforcement of the load-bearing rule.  No other
    function in this module constructs a response containing ``"cert"``, so a
    certificate physically cannot leave here without a passing verdict from one
    of the existing independent checkers.
    """
    verdict = checker(log)
    accepted = verdict.get("valid") is True
    response = {
        "ok": accepted,
        "op": op,
        "unavailable": False,
        "cert": log if accepted else None,
        "check": verdict,
        "checked": True,
        "producer": PRODUCER,
        "method": method,
        # Wolfram output is non-deterministic across engine versions; this flag
        # records that the document above earned its verdict on THIS run only.
        "trusted_without_check": False,
    }
    if not accepted:
        response["reason"] = (
            "wolfram-generated certificate REJECTED by "
            f"{checker.__module__}.check: {verdict.get('reason')}")
    return response


def _unavailable(op: str) -> dict:
    return {"ok": False, "op": op, "unavailable": True, "cert": None,
            "check": None, "checked": False, "producer": PRODUCER,
            "reason": "no Wolfram Engine / wolframclient available"}


def _rejected(op: str, reason: str) -> dict:
    """A pre-checker refusal (bad parse, float, engine error).  No cert, ever."""
    return {"ok": False, "op": op, "unavailable": False, "cert": None,
            "check": None, "checked": False, "producer": PRODUCER,
            "reason": reason}


# --------------------------------------------------------------------------- #
# Op: SOS / Positivstellensatz via FindInstance over Rationals.
# --------------------------------------------------------------------------- #

def _sos_code(p_text: str, var: str, interval, num_squares: int,
              sq_degree: int, num_mult_squares: int, mult_degree: int) -> str:
    """Wolfram source solving an SOS ansatz for EXACT rational coefficients.

    Sets up ``p = Sum s_i^2 + (x-a)(b-x) Sum t_j^2`` with undetermined rational
    coefficients, equates coefficients, and asks ``FindInstance`` to solve over
    ``Rationals`` (never ``Reals``, which would hand back algebraic or inexact
    numbers our checkers cannot consume).
    """
    if interval is None:
        mult = "0"
        num_mult_squares = 0
    else:
        a, b = _wl_number(interval[0]), _wl_number(interval[1])
        mult = f"(({var} - {a})*({b} - {var}))"
    return f"""
Module[{{P, sq, tq, ans, vs, eqs, sol}},
  P = Together[Rationalize[{p_text}, 0]];
  sq = Table[Sum[aa[i, j]*{var}^j, {{j, 0, {sq_degree}}}], {{i, 1, {num_squares}}}];
  tq = Table[Sum[bb[i, j]*{var}^j, {{j, 0, {mult_degree}}}], {{i, 1, {num_mult_squares}}}];
  ans = Total[sq^2] + {mult}*Total[tq^2];
  vs = Join[
    Flatten[Table[aa[i, j], {{i, 1, {num_squares}}}, {{j, 0, {sq_degree}}}]],
    Flatten[Table[bb[i, j], {{i, 1, {num_mult_squares}}}, {{j, 0, {mult_degree}}}]]];
  eqs = Thread[CoefficientList[Expand[ans - P], {var}] == 0];
  sol = FindInstance[eqs, vs, Rationals];
  If[sol === {{}} || Head[sol] =!= List, $Failed,
    ToString[InputForm[Together /@ # & /@ ({{sq, tq}} /. First[sol])]]]]
""".strip()


def generate_sos(request: dict) -> dict:
    """Generate a univariate SOS / Positivstellensatz certificate via Wolfram.

    ``request`` keys: ``p`` (polynomial, sympy-parsable), ``x`` (variable name),
    optional ``interval`` ``[a, b]``, and ansatz-size knobs ``num_squares``,
    ``sq_degree``, ``num_mult_squares``, ``mult_degree``.
    """
    if not available():
        return _unavailable("sos")
    try:
        var = str(request.get("x", "x"))
        x = Symbol(var)
        p_expr = _require_exact(sympify(request["p"]), "p")
        interval = request.get("interval")
        deg = sympy.Poly(p_expr, x).degree()

        code = _sos_code(
            sympy.printing.sstr(p_expr).replace("**", "^"), var, interval,
            int(request.get("num_squares", 2)),
            int(request.get("sq_degree", max(deg // 2, 1))),
            int(request.get("num_mult_squares", 2)),
            int(request.get("mult_degree", max((deg - 2) // 2, 0))),
        )
        raw = _call(code, float(request.get("timeout", 30.0)))

        outer = _wl_split(raw)
        if len(outer) != 2:
            raise _Reject("expected {squares, multiplier_squares} from Wolfram")
        squares = [_wl_expr(t, "sos square") for t in _wl_split(outer[0])]
        msq = [_wl_expr(t, "sos multiplier square") for t in _wl_split(outer[1])]
        # Drop identically-zero terms; they are legal but pure noise in the doc.
        squares = [s for s in squares if expand(s) != 0]
        msq = [t for t in msq if expand(t) != 0]

        log = _sos.export_univariate_sos_cert(
            p_expr, x=x, interval=interval, squares=squares,
            multiplier_squares=msq,
            claim=request.get("claim"))
        log["meta"]["producer"] = "wolfram_cert.sos"
        log["meta"]["oracle"] = "wolfram FindInstance over Rationals (untrusted)"
    except _Reject as exc:
        return _rejected("sos", str(exc))
    except (KeyError, TypeError, ValueError, ZeroDivisionError,
            sympy.SympifyError, sympy.PolynomialError) as exc:
        return _rejected("sos", f"malformed sos request/response ({exc})")
    return _finalize("sos", log, _sos.check, "wolfram FindInstance -> cert_sos.check")


# --------------------------------------------------------------------------- #
# Op: Nullstellensatz cofactors via PolynomialReduce / GroebnerBasis.
# --------------------------------------------------------------------------- #

def _ns_code(target: str, polys: list[str], gens: list[str]) -> str:
    """Wolfram source returning ideal-membership cofactors ``{q_i}``.

    ``PolynomialReduce`` divides the target by the ORIGINAL generators, which is
    exactly the shape our cofactor certificate needs.  When the remainder is
    non-zero the target is not reachable by plain division and we return
    ``$Failed`` rather than a partial answer; the Groebner-basis general case is
    left to the sympy exporter, which already handles it soundly.
    """
    gl = "{" + ", ".join(gens) + "}"
    pl = "{" + ", ".join(polys) + "}"
    return f"""
Module[{{tgt, gs, red}},
  tgt = Together[Rationalize[{target}, 0]];
  gs = Together[Rationalize[#, 0]] & /@ {pl};
  red = PolynomialReduce[tgt, gs, {gl}];
  If[Expand[Last[red]] =!= 0, $Failed,
    ToString[InputForm[Together /@ First[red]]]]]
""".strip()


def generate_nullstellensatz(request: dict) -> dict:
    """Generate an ideal-membership / weak-Nullstellensatz cofactor certificate.

    ``request`` keys: ``polys`` (generators ``p_i``), ``gens`` (ring variables),
    optional ``target`` (default ``1``), ``order`` (default ``"lex"``), ``mode``.
    """
    if not available():
        return _unavailable("nullstellensatz")
    try:
        gens = [sympify(g) for g in request["gens"]]
        polys = [_require_exact(sympify(p), "generator")
                 for p in request["polys"]]
        target = _require_exact(sympify(request.get("target", 1)),
                                "target")
        order = request.get("order", "lex")
        mode = request.get("mode") or ("nullstellensatz" if target == 1
                                       else "membership")

        code = _ns_code(
            sympy.printing.sstr(target).replace("**", "^"),
            [sympy.printing.sstr(p).replace("**", "^") for p in polys],
            [str(g) for g in gens])
        raw = _call(code, float(request.get("timeout", 30.0)))

        cofactors = [_wl_expr(t, "cofactor") for t in _wl_split(raw)]
        if len(cofactors) != len(polys):
            raise _Reject(f"wolfram returned {len(cofactors)} cofactor(s) for "
                          f"{len(polys)} generator(s)")

        # Built by hand rather than via export_nullstellensatz_cert, because that
        # exporter recomputes the cofactors with sympy; the whole point here is to
        # carry WOLFRAM's cofactors into the document and let our checker judge them.
        steps = [
            {"op": "declare_ring", "nvars": len(gens),
             "names": [str(g) for g in gens], "order": order},
            {"op": "generators",
             "polys": [_ns._serialize_poly(p, gens) for p in polys]},
            {"op": "cofactors",
             "polys": [_ns._serialize_poly(q, gens) for q in cofactors]},
            {"op": "target", "mode": mode,
             "poly": _ns._serialize_poly(target, gens)},
            {"op": "assert_combination_equals_target"},
        ]
        log = {
            "format": _ns.FORMAT, "kind": _ns.KIND,
            "claim": request.get("claim") or (
                "the polynomials have no common zero: 1 = Sum q_i p_i "
                "(weak Nullstellensatz cofactor certificate)"
                if mode == "nullstellensatz"
                else "target g is in the ideal <p_i>: g = Sum q_i p_i"),
            "steps": steps,
            "meta": {"producer": "wolfram_cert.nullstellensatz",
                     "oracle": "wolfram PolynomialReduce cofactors (untrusted)",
                     "order": order},
        }
    except _Reject as exc:
        return _rejected("nullstellensatz", str(exc))
    except (KeyError, TypeError, ValueError, ZeroDivisionError,
            sympy.SympifyError, sympy.PolynomialError) as exc:
        return _rejected("nullstellensatz",
                         f"malformed nullstellensatz request/response ({exc})")
    return _finalize("nullstellensatz", log, _ns.check,
                     "wolfram PolynomialReduce -> cert_nullstellensatz.check")


# --------------------------------------------------------------------------- #
# Op: Sturm real-root count via CountRoots.
# --------------------------------------------------------------------------- #

def _sturm_code(p_text: str, var: str, a: str, b: str) -> str:
    """Wolfram source returning the real-root count of ``p`` on the interval."""
    return f"""
Module[{{P}},
  P = Together[Rationalize[{p_text}, 0]];
  ToString[InputForm[CountRoots[P, {{{var}, {a}, {b}}}]]]]
""".strip()


def generate_sturm(request: dict) -> dict:
    """Generate a Sturm real-root-count certificate whose COUNT came from Wolfram.

    ``request`` keys: ``coeffs`` (ascending univariate coefficients),
    ``interval`` ``[a, b]``, optional ``var``.

    Only the root count is oracular.  The Sturm chain and sign variations are
    rebuilt from ``p`` by the checker regardless of what we put in the document,
    so a wrong Wolfram count is caught by ``cert_sturm.check``.  Note the
    conventions genuinely differ: ``CountRoots`` counts with multiplicity on the
    CLOSED ``[a, b]``, our certificate counts distinct roots on ``(a, b]``.  A
    disagreement is therefore an expected, correctly-rejected outcome.
    """
    if not available():
        return _unavailable("sturm")
    try:
        var = str(request.get("var", "x"))
        coeffs = list(request["coeffs"])
        interval = list(request["interval"])
        a, b = _wl_number(interval[0]), _wl_number(interval[1])

        code = _sturm_code(_wl_poly_from_coeffs(coeffs, var), var, a, b)
        raw = _call(code, float(request.get("timeout", 30.0)))
        claimed = _wl_int(raw, "root count")

        # export_sturm_cert derives the chain/sign-variations honestly; we then
        # OVERWRITE the count with Wolfram's claim so the checker adjudicates it.
        log = _sturm.export_sturm_cert(coeffs, interval, var=var,
                                       claim=request.get("claim"))
        log["steps"][0]["root_count"] = claimed
        log["claim"] = request.get("claim") or (
            f"p({var}) has {claimed} distinct real root(s) in "
            f"({interval[0]}, {interval[1]}]")
        log["meta"]["producer"] = "wolfram_cert.sturm"
        log["meta"]["oracle"] = "wolfram CountRoots (untrusted)"
    except _Reject as exc:
        return _rejected("sturm", str(exc))
    except (KeyError, IndexError, TypeError, ValueError, ZeroDivisionError,
            ArithmeticError) as exc:
        return _rejected("sturm", f"malformed sturm request/response ({exc})")
    return _finalize("sturm", log, _sturm.check,
                     "wolfram CountRoots -> cert_sturm.check")


# --------------------------------------------------------------------------- #
# Worker dispatch.
# --------------------------------------------------------------------------- #

_OPS: dict[str, Callable[[dict], dict]] = {
    "sos": generate_sos,
    "nullstellensatz": generate_nullstellensatz,
    "sturm": generate_sturm,
}


def run(request: dict) -> dict:
    """Worker entrypoint.  ``request["op"]`` is one of :data:`OPS`.

    Every op returns ``{ok, op, unavailable, cert, check, checked, producer, ...}``.
    ``cert`` is non-``None`` if and only if the corresponding EXISTING checker
    accepted the Wolfram-generated document on this run.  With no engine present
    every op returns ``unavailable=True`` without raising.
    """
    op = request.get("op")
    handler = _OPS.get(op)
    if handler is None:
        raise ValueError(f"unknown op: {op!r} (expected one of {OPS})")
    return handler(request)
