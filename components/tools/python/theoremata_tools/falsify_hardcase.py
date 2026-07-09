"""Exclusion-zone enumeration and worst-case test-point generators.

Two companion oracles for :mod:`theoremata_tools.falsify`. Where ``falsify``
brute-forces a claim over a dense integer product, this module targets the two
places that brute force misses:

* **#9 Exclusion-zone template** (Harrison, *The verification of a floating
  point square root*): many analytic bounds are provable *except* on a finite
  Diophantine "hard-case" set -- the mantissas whose square root lands so close
  to a rounding boundary that the smooth estimate no longer decides them. Those
  hard cases are exactly the integer solutions of a congruence such as
  ``2**(p+2) * m = k**2 + d``. :func:`exclusion_zone` *enumerates that finite
  set* (solving ``k**2 ≡ -d (mod N)`` with ``sympy.ntheory.sqrt_mod``, which
  performs the even/odd split and 2-adic Hensel lifting internally), so each
  hard case can be discharged one by one instead of by a uniform bound.

* **#10 Worst-case generators**: deterministic generators of *adversarial* test
  points to feed the falsify engine, rather than a dense scan --
  ``balanced_factorization`` (divisor pairs of ``2**(2p)+d`` nearest ``sqrt``,
  via :func:`sympy.factorint`), ``near_root`` (lattice points hugging the real
  roots of a polynomial, where a numeric goal is most fragile), and ``hensel``
  (the same Diophantine hard cases as above, surfaced as candidate inputs).

Design reuse from ``falsify.py`` / ``triviality.py``:

* the ``{start, stop}`` integer-range shape;
* ``safe_eval.compile_expression`` + ``ALLOWED_NAMES`` to evaluate an untrusted,
  model-emitted analytic-bound predicate in a restricted, builtin-free scope
  (same ``eval(..., {"__builtins__": {}}, scope)`` pattern);
* the ``run(request)`` JSON-worker adapter with an ``op`` selector.

SOUNDNESS: every enumerated hard case is re-checked to *exactly* satisfy the
integer equation before it is returned (``k**2 + d == N * m``), and every
balanced-factorization pair is re-checked to multiply back to ``N`` -- these are
hard facts. Whether a hard case is a genuine counterexample is left to the
caller (or to the optional ``bound_predicate``): the enumeration only certifies
that the returned set *is* the complete finite hard-case set inside the stated
range, so the analytic bound is safe everywhere else.

Everything is deterministic (no wall-clock, no RNG) and offline. sympy is a hard
dependency of the enumerator/factoriser; the module imports it lazily so the
rest of the toolbox still imports without it.
"""
from __future__ import annotations

from math import isqrt
from typing import Any

from .safe_eval import ALLOWED_NAMES, compile_expression

OP_EXCLUSION = "exclusion_zone"
OP_WORST = "worst_cases"

#: Cap on how many hard cases one enumeration may return (guards a caller who
#: hands in an unbounded ``m``-range). Mirrors the ``falsify`` spirit of a hard
#: bound on total work.
DEFAULT_MAX_CASES = 100_000

#: Refuse to factor integers wider than this many bits by default -- balanced
#: factorization is only as fast as ``sympy.factorint`` on the given number, so
#: the large-p regime is honestly gated on the difficulty of factoring.
DEFAULT_MAX_FACTOR_BITS = 80


# --- shared helpers --------------------------------------------------------


def _modulus(spec: Any) -> tuple[int, int | None, int | None]:
    """Resolve a modulus ``N`` from either an int or ``{"base", "exp"}``.

    Returns ``(N, base, exp)`` where ``base``/``exp`` are ``None`` for a bare
    integer modulus. ``N = base ** exp`` for the dict form (e.g. ``2 ** (p+2)``).
    """
    if isinstance(spec, bool):  # avoid ``True`` sneaking in as ``1``
        raise ValueError("modulus must be an int or {base, exp}")
    if isinstance(spec, int):
        if spec <= 0:
            raise ValueError("modulus must be positive")
        return spec, None, None
    if isinstance(spec, dict):
        base = int(spec["base"])
        exp = int(spec["exp"])
        if base <= 0 or exp <= 0:
            raise ValueError("modulus base/exp must be positive")
        return base ** exp, base, exp
    raise ValueError("modulus must be an int or {base, exp}")


def _inclusive_range(spec: dict[str, Any] | None, *, name: str) -> tuple[int, int]:
    """Parse a ``{"start", "stop"}`` bound as an inclusive integer interval."""
    if not isinstance(spec, dict):
        raise ValueError(f"{name} must be an object with start/stop")
    start = int(spec["start"])
    stop = int(spec["stop"])
    if stop < start:
        raise ValueError(f"{name}: stop must be >= start")
    return start, stop


# --- #9 exclusion-zone enumeration -----------------------------------------


def exclusion_zone(request: dict[str, Any]) -> dict[str, Any]:
    """Enumerate the finite Diophantine hard-case set ``N*m = k**2 + d``.

    Request::

        {
          "op": "exclusion_zone",
          "modulus": 64 | {"base": 2, "exp": 6},   # N (e.g. 2**(p+2))
          "d": -17,                                 # signed offset in N*m = k^2 + d
          "m_range": {"start": 1, "stop": 50},      # inclusive mantissa bound
          "k_range": {"start": 0, "stop": 100},     # alternative: bound k directly
          "bound_predicate": "k*k > N",             # optional; see below
          "max_cases": 100000                       # optional guard
        }

    Exactly one of ``m_range`` / ``k_range`` selects the finite window. The
    hard cases are all integers ``k`` in range with ``k**2 ≡ -d (mod N)``; for
    each, ``m = (k**2 + d) / N``. Solutions of the congruence come from
    :func:`sympy.ntheory.sqrt_mod` (all roots), which handles the even/odd split
    and 2-adic Hensel lifting for ``N = 2**e`` internally; every returned case is
    then re-verified to satisfy ``k**2 + d == N*m`` exactly.

    ``bound_predicate`` (optional) is a safe expression over ``k, m, d, N, base,
    exp, p`` that states when the *analytic bound holds* on a case. A case is
    flagged ``potential_counterexample`` when the predicate is **False** (the
    bound does not cover it). With no predicate, every hard case is flagged --
    the whole exclusion zone must be discharged individually.

    Response::

        {"op": "exclusion_zone", "modulus": N, "d": d,
         "equation": "N*m = k**2 + d", "residues": [...],
         "count": int, "hard_cases": [{"k","m","value","potential_counterexample"}...],
         "potential_counterexamples": [...subset...], "truncated": bool, "note": str}
    """
    from sympy.ntheory import sqrt_mod

    modulus_spec = request.get("modulus")
    if modulus_spec is None:
        raise ValueError("exclusion_zone requires 'modulus'")
    N, base, exp = _modulus(modulus_spec)
    d = int(request["d"])
    max_cases = int(request.get("max_cases", DEFAULT_MAX_CASES))

    has_m = "m_range" in request
    has_k = "k_range" in request
    if has_m == has_k:
        raise ValueError("provide exactly one of 'm_range' or 'k_range'")

    if has_m:
        m_min, m_max = _inclusive_range(request["m_range"], name="m_range")
        # k**2 = N*m - d, so k lives between the sqrt of the two endpoints.
        lo_val = N * m_min - d
        hi_val = N * m_max - d
        if hi_val < 0:
            k_min, k_max = 0, -1  # window holds no nonnegative k
        else:
            k_min = isqrt(lo_val) if lo_val > 0 else 0
            k_max = isqrt(hi_val) + 1
    else:
        k_lo, k_hi = _inclusive_range(request["k_range"], name="k_range")
        k_min = max(0, k_lo)
        k_max = k_hi
        m_min, m_max = None, None

    # Residues r in [0, N) with r**2 ≡ -d (mod N). sqrt_mod does the 2-adic
    # Hensel lift / even-odd split for prime-power moduli internally.
    target = (-d) % N
    residues = sqrt_mod(target, N, all_roots=True)
    residues = sorted(set(int(r) for r in residues)) if residues else []

    pred_code = None
    if "bound_predicate" in request:
        pred_code = compile_expression(
            str(request["bound_predicate"]),
            {"k", "m", "d", "N", "base", "exp", "p"},
        )

    p_val = exp if base == 2 else None
    cases: list[dict[str, Any]] = []
    truncated = False
    for r in residues:
        if k_min > k_max:
            break
        # First k >= k_min with k ≡ r (mod N).
        first = k_min + ((r - k_min) % N)
        k = first
        while k <= k_max:
            value = k * k + d
            # By construction value ≡ 0 (mod N); re-verify exactly (soundness).
            if value >= 0 and value % N == 0:
                m = value // N
                if m_min is None or (m_min <= m <= m_max):
                    assert k * k + d == N * m  # hard equality re-check
                    potential = True
                    if pred_code is not None:
                        scope = {
                            **ALLOWED_NAMES,
                            "k": k, "m": m, "d": d, "N": N,
                            "base": base, "exp": exp, "p": p_val,
                        }
                        potential = not bool(
                            eval(pred_code, {"__builtins__": {}}, scope)  # noqa: S307
                        )
                    cases.append({
                        "k": k, "m": m, "value": value,
                        "potential_counterexample": potential,
                    })
                    if len(cases) >= max_cases:
                        truncated = True
                        break
            k += N
        if truncated:
            break

    cases.sort(key=lambda c: c["k"])
    potentials = [c for c in cases if c["potential_counterexample"]]
    note = (
        "Complete finite hard-case set of N*m = k**2 + d in the given window "
        "(k**2 ≡ -d mod N via sqrt_mod's 2-adic Hensel lift); each case is "
        "re-verified to satisfy the equation exactly. The analytic bound is safe "
        "off this set. "
    )
    if truncated:
        note += f"TRUNCATED at max_cases={max_cases}; widen with a smaller window. "
    if pred_code is None:
        note += (
            "No bound_predicate given: every hard case is flagged as a potential "
            "counterexample that must be discharged individually."
        )
    return {
        "op": OP_EXCLUSION,
        "modulus": N,
        "base": base,
        "exp": exp,
        "d": d,
        "equation": "N*m = k**2 + d",
        "residues": residues,
        "count": len(cases),
        "hard_cases": cases,
        "potential_counterexamples": potentials,
        "truncated": truncated,
        "note": note.strip(),
    }


# --- #10 worst-case generators ---------------------------------------------


def _target_n(request: dict[str, Any]) -> int:
    """Resolve the integer under test: explicit ``n``, or ``2**(2p) + d``."""
    if "n" in request:
        return int(request["n"])
    if "p" in request:
        p = int(request["p"])
        d = int(request.get("d", 0))
        return 2 ** (2 * p) + d
    raise ValueError("worst_cases requires 'n' or 'p'")


def _balanced_factorizations(request: dict[str, Any]) -> dict[str, Any]:
    """Divisor pairs ``m*b = N`` nearest to ``sqrt(N)`` (most balanced first).

    ``N`` defaults to ``2**(2p) + d`` -- the balanced-factorization worst case
    for a ``p``-bit multiplier where a near-square split of the significand is
    the adversarial input. Uses :func:`sympy.factorint` / :func:`sympy.divisors`;
    honestly gated on the cost of factoring ``N`` (``max_bits``).
    """
    from sympy import divisors, factorint

    N = _target_n(request)
    if N <= 0:
        raise ValueError("balanced_factorization needs N > 0")
    limit = int(request.get("limit", 8))
    max_bits = int(request.get("max_bits", DEFAULT_MAX_FACTOR_BITS))
    if N.bit_length() > max_bits:
        raise ValueError(
            f"N has {N.bit_length()} bits (> max_bits={max_bits}); "
            "balanced factorization is gated on factoring cost -- raise "
            "max_bits explicitly to attempt it"
        )

    factors = factorint(N)
    divs = divisors(N)  # sorted ascending, deterministic
    pairs: list[dict[str, Any]] = []
    for da in divs:
        if da * da > N:
            break
        db = N // da
        assert da * db == N  # re-verify the factorization exactly
        pairs.append({"m": da, "b": db, "balance": db - da})
    # Most balanced (smallest gap) first; tie-break by the smaller factor.
    pairs.sort(key=lambda pr: (pr["balance"], pr["m"]))
    candidates = pairs[:limit]
    return {
        "op": OP_WORST,
        "kind": "balanced_factorization",
        "n": N,
        "bits": N.bit_length(),
        "factorization": {int(k): int(v) for k, v in factors.items()},
        "count": len(candidates),
        "candidates": candidates,
        "note": (
            "Divisor pairs m*b = N ordered by |b - m| (most balanced first); each "
            "re-verified to multiply back to N. The top pair is the near-square "
            "adversarial split. Large N is gated on factoring cost (max_bits)."
        ),
    }


def _real_roots(request: dict[str, Any]) -> list[float]:
    """Real roots to hug: explicit ``roots`` list, or the real roots of a
    univariate polynomial ``poly`` (via sympy)."""
    if "roots" in request:
        roots = request["roots"]
        if not isinstance(roots, list):
            raise ValueError("roots must be a list of numbers")
        return [float(r) for r in roots]
    if "poly" in request:
        from sympy import Poly, symbols, sympify

        var = str(request.get("var", "x"))
        x = symbols(var)
        expr = sympify(str(request["poly"]))
        poly = Poly(expr, x)
        return [float(r.evalf()) for r in poly.real_roots()]
    raise ValueError("near_root requires 'roots' or 'poly'")


def _near_root(request: dict[str, Any]) -> dict[str, Any]:
    """Lattice points hugging the real roots -- where a numeric goal is most
    fragile (the function is near zero and may change sign).

    For each root ``r`` and lattice ``step s``, emit the ``radius`` nearest
    multiples of ``s`` strictly below and strictly above ``r`` (default
    ``radius=1`` -> the immediate floor/ceil bracket), plus the lattice point
    itself when ``r`` lands exactly on the lattice (distance 0 -- the sharpest
    worst case). Candidates are de-duplicated across roots and annotated with
    their distance to the nearest root; every candidate is within
    ``radius * step`` of some root.
    """
    from math import floor

    roots = _real_roots(request)
    if not roots:
        return {
            "op": OP_WORST, "kind": "near_root", "roots": [],
            "count": 0, "candidates": [],
            "note": "no real roots -> no near-root worst cases",
        }
    step = request.get("step", 1)
    radius = int(request.get("radius", 1))
    if radius < 1:
        raise ValueError("radius must be >= 1")
    # Integer step keeps candidates integral; a rational/float step keeps the
    # multiples exact-ish but still deterministic.
    is_int_step = isinstance(step, int) and not isinstance(step, bool)
    s = int(step) if is_int_step else float(step)
    if s <= 0:
        raise ValueError("step must be positive")

    def _point(idx: int) -> float:
        return int(idx) * s if is_int_step else idx * s

    xs: set[float] = set()
    for r in roots:
        base_idx = r / s
        floor_idx = floor(base_idx)
        on_lattice = floor_idx == base_idx
        below = floor_idx - 1 if on_lattice else floor_idx  # strictly below r
        above = floor_idx + 1  # strictly above r (ceil for non-integral base)
        if on_lattice:
            xs.add(_point(floor_idx))  # the exact root: distance 0
        for j in range(radius):
            xs.add(_point(below - j))
            xs.add(_point(above + j))

    candidates = []
    for x in sorted(xs):
        dist = min(abs(x - r) for r in roots)
        nearest = min(roots, key=lambda r: abs(x - r))
        candidates.append({
            "x": x,
            "nearest_root": nearest,
            "distance": dist,
        })
    return {
        "op": OP_WORST,
        "kind": "near_root",
        "roots": roots,
        "step": s,
        "radius": radius,
        "count": len(candidates),
        "candidates": candidates,
        "note": (
            "Lattice points bracketing each real root (numeric goals are most "
            "fragile near a zero). Every candidate is within radius*step of a root."
        ),
    }


def _hensel_worst(request: dict[str, Any]) -> dict[str, Any]:
    """Diophantine hard cases surfaced as candidate falsifying inputs.

    Thin wrapper over :func:`exclusion_zone`: the ``k`` (and derived ``m``) of
    each hard case are exactly the adversarial points a smooth bound cannot
    decide, so they are the worst-case inputs to hand to the falsify engine.
    """
    zone = exclusion_zone({**request, "op": OP_EXCLUSION})
    candidates = [
        {"k": c["k"], "m": c["m"], "value": c["value"]}
        for c in zone["hard_cases"]
    ]
    return {
        "op": OP_WORST,
        "kind": "hensel",
        "modulus": zone["modulus"],
        "d": zone["d"],
        "residues": zone["residues"],
        "count": len(candidates),
        "candidates": candidates,
        "note": (
            "Hard cases of N*m = k**2 + d as worst-case inputs (see "
            "exclusion_zone). Each k is a Diophantine near-boundary point."
        ),
    }


def worst_cases(request: dict[str, Any]) -> dict[str, Any]:
    """Dispatch a worst-case generator by ``kind``.

    ``kind`` in ``{"balanced_factorization", "near_root", "hensel"}``. Each
    returns ``{"op": "worst_cases", "kind": ..., "candidates": [...], ...}`` --
    a bounded, deterministic list of adversarial inputs for the falsify engine.
    """
    kind = request.get("kind")
    if kind == "balanced_factorization":
        return _balanced_factorizations(request)
    if kind == "near_root":
        return _near_root(request)
    if kind == "hensel":
        return _hensel_worst(request)
    raise ValueError(
        "worst_cases 'kind' must be one of "
        "balanced_factorization | near_root | hensel"
    )


# --- public JSON-worker adapter --------------------------------------------


def run(request: dict[str, Any]) -> dict[str, Any]:
    """JSON-worker adapter. Selects on ``request['op']``.

    Ops:

    * ``"exclusion_zone"`` -> :func:`exclusion_zone` -- enumerate the finite
      Diophantine hard-case set of ``N*m = k**2 + d``.
    * ``"worst_cases"`` -> :func:`worst_cases` -- generate adversarial test
      points (``kind``: ``balanced_factorization`` | ``near_root`` | ``hensel``).

    Suggested worker op name: ``"falsify_hardcase"`` (wire in
    ``worker.dispatch``).
    """
    op = request.get("op", OP_EXCLUSION)
    if op == OP_EXCLUSION:
        return exclusion_zone(request)
    if op == OP_WORST:
        return worst_cases(request)
    raise ValueError(f"unknown op: {op!r}")
