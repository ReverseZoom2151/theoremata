"""Exact-rational LP **dual certificates** + reusable polytope geometry.

This module sits next to :mod:`linprog_cert` (the exact Farkas/feasibility
kernel) and strengthens the LP/Farkas/asymptotic layer with *primal + dual
optimality* certificates and a handful of textbook computational-geometry
primitives that the (NC-licensed, float-based) ``gilp`` package computes but
never surfaces.  Everything here is pure-Python over :class:`fractions.Fraction`
-- exact, no floating point -- so every certificate is independently
re-checkable.

Capabilities
------------
* :func:`primal_dual` -- solve ``max/min c.x`` over a standard-form LP and
  surface the simplex **dual vector** ``y = c_B A_B^{-1}`` as an explicit
  optimality certificate: dual feasibility, **complementary slackness**, and
  strong duality ``c.x* == h.y*`` are all reported and exactly checkable.
  (``gilp`` computes ``y = c_B A_B^{-1}`` internally at ``simplex.py:233`` but
  only ever returns the primal; we expose *and verify* it.)
* :func:`chebyshev_center` -- a strictly-interior point of ``{x : G x <= h}``
  via the **Chebyshev-center LP** (max inscribed-ball radius).  Norms are
  *over*-approximated by exact rationals so the returned ball is provably
  inside the polytope.
* :func:`vertex_enumeration` -- **brute-force n-choose-m** vertex enumeration
  (solve every square subsystem exactly, keep feasible intersections),
  ``gilp``'s degenerate/flat-polytope fallback.
* :func:`redundant_constraints` -- LP-based redundant-constraint detection (the
  generalisation of ``gilp``'s Phase-I all-zero-row test): a constraint is
  redundant iff maximising its own row over the *other* constraints never
  exceeds its bound.

Worker dispatch key: ``lp_geometry`` (see :func:`run`); ``op`` selects the
capability.  The existing :mod:`linprog_cert` API is left untouched.
"""
from __future__ import annotations

import math
from fractions import Fraction
from itertools import combinations
from typing import Any, Iterable, Sequence

from .linprog_cert import Inequality, _coerce, ineq_variables

Frac = Fraction


# --------------------------------------------------------------------------- #
# Exact linear algebra helpers.
# --------------------------------------------------------------------------- #

def _mat_inverse(rows: list[list[Frac]]) -> list[list[Frac]] | None:
    """Exact inverse of a square rational matrix, or ``None`` if singular."""
    n = len(rows)
    aug = [list(r) + [Frac(int(i == j)) for j in range(n)] for i, r in enumerate(rows)]
    for col in range(n):
        piv = next((r for r in range(col, n) if aug[r][col] != 0), None)
        if piv is None:
            return None
        aug[col], aug[piv] = aug[piv], aug[col]
        pv = aug[col][col]
        aug[col] = [v / pv for v in aug[col]]
        for r in range(n):
            if r != col and aug[r][col] != 0:
                f = aug[r][col]
                aug[r] = [a - f * b for a, b in zip(aug[r], aug[col])]
    return [row[n:] for row in aug]


def _solve_square(a: list[list[Frac]], b: list[Frac]) -> list[Frac] | None:
    """Solve ``A x = b`` exactly for square ``A`` (``None`` if singular)."""
    inv = _mat_inverse(a)
    if inv is None:
        return None
    return [sum(inv[i][k] * b[k] for k in range(len(b))) for i in range(len(b))]


# --------------------------------------------------------------------------- #
# Core exact simplex (two-phase, Bland's rule) for   max/min c.x, [G|I]z = h.
#
# ``_solve_primal_leq`` solves   max c.x  s.t.  G x <= h,  x >= 0   exactly and
# returns the optimal basis together with the base matrix [G | I] so the dual
# ``y = c_B A_B^{-1}`` can be reconstructed.
# --------------------------------------------------------------------------- #

def _pivot(M: list[list[Frac]], b: list[Frac], basis: list[int],
           r: int, c: int) -> None:
    pv = M[r][c]
    M[r] = [v / pv for v in M[r]]
    b[r] = b[r] / pv
    m = len(M)
    for i in range(m):
        if i != r and M[i][c] != 0:
            f = M[i][c]
            M[i] = [a - f * bb for a, bb in zip(M[i], M[r])]
            b[i] = b[i] - f * b[r]
    basis[r] = c


def _simplex(M: list[list[Frac]], b: list[Frac], basis: list[int],
             cost: list[Frac], forbid: set[int] | None = None) -> str:
    """Minimise ``cost . x`` in-place via Bland's rule.  Returns status."""
    m = len(M)
    N = len(cost)
    forbid = forbid or set()
    basis_set = set(basis)
    while True:
        cb = [cost[basis[i]] for i in range(m)]
        entering = None
        for j in range(N):
            if j in basis_set or j in forbid:
                continue
            rc = cost[j] - sum(cb[i] * M[i][j] for i in range(m))
            if rc < 0:
                entering = j  # Bland: first eligible index
                break
        if entering is None:
            return "optimal"
        leave = None
        best: Frac | None = None
        for i in range(m):
            if M[i][entering] > 0:
                ratio = b[i] / M[i][entering]
                if (best is None or ratio < best
                        or (ratio == best and basis[i] < basis[leave])):
                    best = ratio
                    leave = i
        if leave is None:
            return "unbounded"
        basis_set.discard(basis[leave])
        basis_set.add(entering)
        _pivot(M, b, basis, leave, entering)


def _solve_primal_leq(n: int, G: list[list[Frac]], h: list[Frac],
                      c: list[Frac]) -> dict[str, Any]:
    """Solve ``max c.x  s.t.  G x <= h, x >= 0`` exactly (two-phase simplex).

    Columns: ``0..n-1`` structural, ``n..n+m-1`` slacks (the identity block),
    then one artificial per negative-rhs row.  Returns status and, when
    optimal, ``x``, ``value``, the final ``basis`` and the base matrix
    ``base = diag(sign) . [G | I]`` used for the dual reconstruction.
    """
    m = len(G)
    ncols = n + m  # structural + slacks
    # base = [G | I]; working matrix M starts equal to base (+ artificials).
    base = [[Frac(0)] * ncols for _ in range(m)]
    for i in range(m):
        for j in range(n):
            base[i][j] = Frac(G[i][j])
        base[i][n + i] = Frac(1)
    b = [Frac(h[i]) for i in range(m)]
    sign = [Frac(1)] * m
    M = [list(base[i]) for i in range(m)]
    for i in range(m):
        if b[i] < 0:  # flip row so rhs >= 0 (surplus + artificial start)
            M[i] = [-v for v in M[i]]
            b[i] = -b[i]
            sign[i] = Frac(-1)
    basis = [n + i for i in range(m)]
    art_cols: list[int] = []
    need_art = [i for i in range(m) if sign[i] < 0]
    if need_art:
        for i in range(m):
            M[i].extend([Frac(0)] * len(need_art))
        for k, i in enumerate(need_art):
            col = ncols + k
            M[i][col] = Frac(1)
            basis[i] = col
            art_cols.append(col)
    N = ncols + len(art_cols)
    art_set = set(art_cols)

    if art_cols:
        cost_i = [Frac(0)] * N
        for a in art_cols:
            cost_i[a] = Frac(1)
        _simplex(M, b, basis, cost_i)
        if any(cost_i[basis[i]] * b[i] != 0 for i in range(m)):
            return {"status": "infeasible"}
        # Drive any remaining artificials out of the basis.
        for i in range(m):
            if basis[i] in art_set:
                piv = next((j for j in range(ncols) if M[i][j] != 0), None)
                if piv is not None:
                    _pivot(M, b, basis, i, piv)

    cost_ii = [Frac(0)] * N
    for j in range(n):
        cost_ii[j] = -Frac(c[j])  # maximise c  ==  minimise -c
    status = _simplex(M, b, basis, cost_ii, forbid=art_set)
    if status == "unbounded":
        return {"status": "unbounded"}

    x = [Frac(0)] * n
    for i in range(m):
        if basis[i] < n:
            x[basis[i]] = b[i]
    value = sum(Frac(c[j]) * x[j] for j in range(n))
    return {
        "status": "optimal",
        "x": x,
        "value": value,
        "basis": list(basis),
        "base": base,
        "sign": sign,
        "n": n,
        "m": m,
        "cost_min": cost_ii,
    }


def _dual_from_basis(sol: dict[str, Any]) -> list[Frac] | None:
    """Reconstruct the max-LP dual ``y = c_B A_B^{-1}`` from the primal basis.

    Uses the *unflipped* base matrix ``[G | I]``; the row-flip signs are folded
    back so ``y`` is the dual of the original ``G x <= h`` rows (``y >= 0``).
    Returns ``None`` if the basis is degenerate (an artificial remains basic).
    """
    n, m = sol["n"], sol["m"]
    basis, base, sign = sol["basis"], sol["base"], sol["sign"]
    if any(bcol >= n + m for bcol in basis):
        return None
    # A' = diag(sign) . base   (the flipped base actually solved).
    a_prime_b = [[sign[i] * base[i][bcol] for bcol in basis] for i in range(m)]
    inv = _mat_inverse(a_prime_b)
    if inv is None:
        return None
    cost_min = sol["cost_min"]
    c_b = [cost_min[bcol] for bcol in basis]
    # pi' = c_B (A'_B)^{-1}  (row vector);  pi = diag(sign) pi';  y = -pi.
    pi_prime = [sum(c_b[k] * inv[k][i] for k in range(m)) for i in range(m)]
    y = [-(sign[i] * pi_prime[i]) for i in range(m)]
    return y


# --------------------------------------------------------------------------- #
# General optimiser over arbitrary senses + optionally-free variables.
# --------------------------------------------------------------------------- #

def _rows_to_leq(n: int, rows: list[tuple[list[Frac], str, Frac]]
                 ) -> tuple[list[list[Frac]], list[Frac]]:
    """Convert mixed-sense rows over ``n`` vars to ``G x <= h`` (eq -> two)."""
    G: list[list[Frac]] = []
    h: list[Frac] = []
    for coeffs, sense, rhs in rows:
        coeffs = [Frac(v) for v in coeffs]
        rhs = Frac(rhs)
        if sense == "leq":
            G.append(list(coeffs)); h.append(rhs)
        elif sense == "geq":
            G.append([-v for v in coeffs]); h.append(-rhs)
        elif sense == "eq":
            G.append(list(coeffs)); h.append(rhs)
            G.append([-v for v in coeffs]); h.append(-rhs)
        else:  # pragma: no cover - defensive
            raise ValueError(f"bad sense {sense!r}")
    return G, h


def _optimize(n: int, rows: list[tuple[list[Frac], str, Frac]],
              obj: list[Frac], maximize: bool = True,
              nonneg: bool = False) -> dict[str, Any]:
    """Optimise ``obj . x`` over ``rows``; ``nonneg`` toggles ``x >= 0`` vs free.

    Free variables are handled by the standard ``x = x+ - x-`` split so the
    underlying ``_solve_primal_leq`` (which assumes ``x >= 0``) still applies.
    """
    G, h = _rows_to_leq(n, rows)
    c = [Frac(v) for v in obj]
    if not maximize:
        c = [-v for v in c]
    if nonneg:
        sol = _solve_primal_leq(n, G, h, c)
        x = sol.get("x")
    else:
        # Split each var: columns 2j = x_j+, 2j+1 = x_j-.
        G2 = [[val for v in row for val in (v, -v)] for row in G]
        c2 = [val for v in c for val in (v, -v)]
        sol = _solve_primal_leq(2 * n, G2, h, c2)
        xs = sol.get("x")
        x = [xs[2 * j] - xs[2 * j + 1] for j in range(n)] if xs else None
    out = {"status": sol["status"]}
    if sol["status"] == "optimal":
        val = sum(Frac(obj[j]) * x[j] for j in range(n))
        out.update({"x": x, "value": val})
    return out


# --------------------------------------------------------------------------- #
# Public: variable-ordering helpers.
# --------------------------------------------------------------------------- #

def _order_vars(ineqs: list[Inequality], objective: dict | None) -> list[str]:
    names = set(ineq_variables(ineqs))
    if objective:
        names.update(str(k) for k in objective)
    return sorted(names)


def _row_of(ineq: Inequality, variables: Sequence[str]) -> list[Frac]:
    return [ineq.coeffs.get(v, Frac(0)) for v in variables]


# --------------------------------------------------------------------------- #
# 1. Primal + dual optimality certificate.
# --------------------------------------------------------------------------- #

def primal_dual(objective: dict | Sequence,
                constraints: Iterable,
                sense: str = "max") -> dict[str, Any]:
    """Solve a standard-form LP and return a primal+dual optimality certificate.

    The LP is ``opt  c.x   s.t.  (constraints),  x >= 0`` where ``opt`` is
    ``max`` (default) or ``min``.  Constraints are :class:`linprog_cert.Inequality`
    or dicts; senses ``leq/geq/eq`` (aliases accepted).  Each constraint is
    normalised to the ``<=`` form ``G x <= h`` (``geq`` negated, ``eq`` split)
    that indexes the returned dual.

    Returns a dict with the optimal ``primal`` witness, the **dual vector**
    ``y`` (``= c_B A_B^{-1}``, one entry per normalised ``<=`` row, ``y >= 0``),
    and a ``certificate`` block reporting, exactly:

    * ``strong_duality`` -- ``c.x* == h.y*`` (bool + both values);
    * ``complementary_slackness`` -- per row ``y_k * (h_k - G_k x*) == 0``;
    * ``dual_feasible`` -- ``y >= 0`` and ``(G^T y)_j >= c_j`` with equality
      wherever ``x_j > 0`` (stationarity for ``x >= 0``).

    ``dual_from_basis`` echoes the same ``y`` recomputed straight from the
    optimal basis via ``c_B A_B^{-1}`` (``None`` iff the basis is degenerate).
    """
    ineqs = _coerce(list(constraints))
    obj_map: dict[str, Frac]
    if isinstance(objective, dict):
        obj_map = {str(k): Frac(str(v)) for k, v in objective.items()}
    else:
        # positional -> matched against sorted variable order below
        obj_map = {}
    variables = _order_vars(ineqs, obj_map or None)
    if not isinstance(objective, dict):
        seq = list(objective)
        if len(seq) != len(variables):
            raise ValueError(
                "positional objective length must match number of variables; "
                "pass a {var: coeff} dict to disambiguate")
        obj_map = {v: Frac(str(seq[i])) for i, v in enumerate(variables)}

    n = len(variables)
    c = [obj_map.get(v, Frac(0)) for v in variables]

    # Normalise all constraints to  G x <= h.
    G: list[list[Frac]] = []
    h: list[Frac] = []
    row_src: list[dict[str, Any]] = []
    for idx, ineq in enumerate(ineqs):
        row = _row_of(ineq, variables)
        if ineq.sense in ("leq", "lt"):
            G.append(row); h.append(ineq.rhs)
            row_src.append({"index": idx, "orientation": "leq"})
        elif ineq.sense in ("geq", "gt"):
            G.append([-v for v in row]); h.append(-ineq.rhs)
            row_src.append({"index": idx, "orientation": "geq(neg)"})
        else:  # eq -> two rows
            G.append(list(row)); h.append(ineq.rhs)
            row_src.append({"index": idx, "orientation": "eq(+)"})
            G.append([-v for v in row]); h.append(-ineq.rhs)
            row_src.append({"index": idx, "orientation": "eq(-)"})

    solve_c = c if sense == "max" else [-v for v in c]
    sol = _solve_primal_leq(n, G, h, solve_c)
    if sol["status"] != "optimal":
        return {"status": sol["status"], "sense": sense, "variables": variables}

    x = sol["x"]
    primal_value = sum(c[j] * x[j] for j in range(n))

    # Dual via the explicit dual LP (robust, y >= 0 by construction):
    #   min h.y  s.t.  G^T y >= c,  y >= 0     (for the max problem in solve_c)
    mrows = len(G)
    gT_rows = [([G[k][j] for k in range(mrows)], "geq", solve_c[j])
               for j in range(n)]
    dual_obj = list(h)
    dual_sol = _optimize(mrows, gT_rows, dual_obj, maximize=False, nonneg=True)
    y = dual_sol.get("x") or [Frac(0)] * mrows
    # Map dual value back to the original objective sense.

    y_basis = _dual_from_basis(sol)
    # Prefer the basis dual when it is available and verifies (it equals y).
    y_cert = y_basis if y_basis is not None else y

    # --- exact verification of the certificate ---
    slack = [h[k] - sum(G[k][j] * x[j] for j in range(n)) for k in range(mrows)]
    comp_rows = []
    comp_ok = True
    for k in range(mrows):
        prod = y_cert[k] * slack[k]
        ok = prod == 0
        comp_ok = comp_ok and ok
        comp_rows.append({
            "row": k,
            "source": row_src[k],
            "y": str(y_cert[k]),
            "slack": str(slack[k]),
            "y*slack": str(prod),
            "tight": slack[k] == 0,
        })
    nonneg_ok = all(v >= 0 for v in y_cert)
    # G^T y  vs  solve_c  (dual feasibility for x >= 0)
    gTy = [sum(G[k][j] * y_cert[k] for k in range(mrows)) for j in range(n)]
    dual_feas_ok = all(gTy[j] >= solve_c[j] for j in range(n))
    stat_ok = all((x[j] == 0) or (gTy[j] == solve_c[j]) for j in range(n))
    dual_value_solvec = sum(h[k] * y_cert[k] for k in range(mrows))
    strong = dual_value_solvec == sum(solve_c[j] * x[j] for j in range(n))

    return {
        "status": "optimal",
        "sense": sense,
        "variables": variables,
        "primal": {v: str(x[j]) for j, v in enumerate(variables)},
        "objective_value": str(primal_value),
        "dual": [str(v) for v in y_cert],
        "dual_from_basis": None if y_basis is None else [str(v) for v in y_basis],
        "rows": row_src,
        "certificate": {
            "type": "lp_primal_dual",
            "strong_duality": {
                "holds": bool(strong),
                "primal_objective": str(sum(solve_c[j] * x[j] for j in range(n))),
                "dual_objective": str(dual_value_solvec),
            },
            "dual_feasible": {
                "y_nonneg": bool(nonneg_ok),
                "GT_y_ge_c": bool(dual_feas_ok),
                "stationarity": bool(stat_ok),
            },
            "complementary_slackness": {
                "holds": bool(comp_ok),
                "rows": comp_rows,
            },
            "verified": bool(strong and comp_ok and nonneg_ok
                             and dual_feas_ok and stat_ok),
        },
    }


# --------------------------------------------------------------------------- #
# 2. Chebyshev-center interior point.
# --------------------------------------------------------------------------- #

def _norm_upper(coeffs: list[Frac]) -> Frac:
    """A rational ``u >= ||coeffs||_2`` (exact over-approximation)."""
    sq = sum(v * v for v in coeffs)
    if sq == 0:
        return Frac(0)
    num, den = sq.numerator, sq.denominator
    # sqrt(num/den) = sqrt(num*den)/den.  Use a large scale so the rational
    # upper bound hugs the true norm (kept >= it), and stays exact when the
    # norm itself is rational (e.g. axis-aligned rows).
    scale = 10 ** 12
    k = num * den * scale * scale
    root = math.isqrt(k)
    if root * root != k:
        root += 1  # ceil -> guarantees an over-approximation
    return Frac(root, den * scale)


_RADIUS_CAP = Frac(10 ** 9)


def chebyshev_center(constraints: Iterable) -> dict[str, Any]:
    """Strictly-interior point of ``{x : G x <= h}`` via the Chebyshev-center LP.

    Solves ``max r  s.t.  G_k . x + ||G_k|| r <= h_k, r >= 0`` (``x`` free).
    The row norms are *over*-approximated by exact rationals, so the inscribed
    ball of radius ``r`` -- and in particular its centre -- is provably inside
    the polytope.  ``radius > 0`` certifies a non-empty interior; ``radius == 0``
    means the region is empty, a single point, or lower-dimensional.

    Returns ``{status, center: {var: "p/q"}, radius, interior: bool}``.
    """
    ineqs = _coerce(list(constraints))
    variables = _order_vars(ineqs, None)
    n = len(variables)

    # Normalise to G x <= h.
    G: list[list[Frac]] = []
    h: list[Frac] = []
    for ineq in ineqs:
        row = _row_of(ineq, variables)
        if ineq.sense in ("leq", "lt"):
            G.append(row); h.append(ineq.rhs)
        elif ineq.sense in ("geq", "gt"):
            G.append([-v for v in row]); h.append(-ineq.rhs)
        else:
            G.append(list(row)); h.append(ineq.rhs)
            G.append([-v for v in row]); h.append(-ineq.rhs)

    if not G:
        return {"status": "optimal",
                "center": {v: "0" for v in variables},
                "radius": str(_RADIUS_CAP), "interior": True}

    # Variables for the LP: x_j = xp_j - xn_j (2n cols), then r (1 col).
    # Columns: 0..2n-1 splits, 2n = r.
    ncol = 2 * n + 1
    rows_lp: list[tuple[list[Frac], str, Frac]] = []
    for k, row in enumerate(G):
        lp_row = [Frac(0)] * ncol
        for j in range(n):
            lp_row[2 * j] = row[j]
            lp_row[2 * j + 1] = -row[j]
        lp_row[2 * n] = _norm_upper(row)
        rows_lp.append((lp_row, "leq", h[k]))
    # Cap radius so an unbounded polytope still yields a finite interior point.
    cap = [Frac(0)] * ncol
    cap[2 * n] = Frac(1)
    rows_lp.append((cap, "leq", _RADIUS_CAP))

    obj = [Frac(0)] * ncol
    obj[2 * n] = Frac(1)  # maximise r
    sol = _solve_primal_leq(ncol, *_rows_to_leq(ncol, rows_lp), obj)
    if sol["status"] != "optimal":
        return {"status": sol["status"], "variables": variables}

    z = sol["x"]
    center = [z[2 * j] - z[2 * j + 1] for j in range(n)]
    radius = z[2 * n]
    # Independent re-check: centre satisfies every original halfspace.
    inside = all(sum(G[k][j] * center[j] for j in range(n)) <= h[k]
                 for k in range(len(G)))
    return {
        "status": "optimal",
        "center": {variables[j]: str(center[j]) for j in range(n)},
        "radius": str(radius),
        "interior": bool(radius > 0),
        "center_inside": bool(inside),
        "variables": variables,
    }


# --------------------------------------------------------------------------- #
# 3. Brute-force n-choose-m vertex enumeration.
# --------------------------------------------------------------------------- #

def vertex_enumeration(constraints: Iterable) -> dict[str, Any]:
    """Enumerate the vertices of ``{x : G x <= h}`` by brute force.

    For ``n`` variables, every choice of ``n`` rows whose ``n x n`` subsystem
    ``G_S x = h_S`` is non-singular yields a candidate vertex; it is kept iff it
    satisfies *all* constraints.  This is ``gilp``'s degenerate/flat-polytope
    fallback (H -> V conversion), done exactly.

    Returns ``{status, count, vertices: [{var: "p/q"}, ...]}``.
    """
    ineqs = _coerce(list(constraints))
    variables = _order_vars(ineqs, None)
    n = len(variables)

    G: list[list[Frac]] = []
    h: list[Frac] = []
    for ineq in ineqs:
        row = _row_of(ineq, variables)
        if ineq.sense in ("leq", "lt"):
            G.append(row); h.append(ineq.rhs)
        elif ineq.sense in ("geq", "gt"):
            G.append([-v for v in row]); h.append(-ineq.rhs)
        else:
            G.append(list(row)); h.append(ineq.rhs)
            G.append([-v for v in row]); h.append(-ineq.rhs)

    m = len(G)
    if n == 0 or m < n:
        return {"status": "ok", "count": 0, "vertices": [],
                "variables": variables}

    seen: set[tuple[str, ...]] = set()
    vertices: list[dict[str, str]] = []
    for combo in combinations(range(m), n):
        sub = [G[i] for i in combo]
        rhs = [h[i] for i in combo]
        pt = _solve_square([list(r) for r in sub], list(rhs))
        if pt is None:
            continue
        if all(sum(G[k][j] * pt[j] for j in range(n)) <= h[k]
               for k in range(m)):
            key = tuple(str(v) for v in pt)
            if key not in seen:
                seen.add(key)
                vertices.append({variables[j]: str(pt[j]) for j in range(n)})
    return {"status": "ok", "count": len(vertices), "vertices": vertices,
            "variables": variables}


# --------------------------------------------------------------------------- #
# 4. LP-based redundant-constraint detection.
# --------------------------------------------------------------------------- #

def redundant_constraints(constraints: Iterable) -> dict[str, Any]:
    """Detect redundant constraints in ``{x : G x <= h}`` via LP.

    Constraint ``k`` (``G_k x <= h_k``) is **redundant** iff the maximum of
    ``G_k . x`` over the *remaining* constraints is bounded and ``<= h_k`` --
    i.e. the other constraints already imply it.  This is the LP generalisation
    of ``gilp``'s Phase-I all-zero-row redundancy test (``simplex.py:365-379``).
    Equality constraints are normalised to their two ``<=`` rows first.

    Returns ``{status, redundant: [row indices], detail: [...]}``.
    """
    ineqs = _coerce(list(constraints))
    variables = _order_vars(ineqs, None)
    n = len(variables)

    G: list[list[Frac]] = []
    h: list[Frac] = []
    origin: list[int] = []
    for idx, ineq in enumerate(ineqs):
        row = _row_of(ineq, variables)
        if ineq.sense in ("leq", "lt"):
            G.append(row); h.append(ineq.rhs); origin.append(idx)
        elif ineq.sense in ("geq", "gt"):
            G.append([-v for v in row]); h.append(-ineq.rhs); origin.append(idx)
        else:
            G.append(list(row)); h.append(ineq.rhs); origin.append(idx)
            G.append([-v for v in row]); h.append(-ineq.rhs); origin.append(idx)

    m = len(G)
    detail = []
    redundant_rows = []
    for k in range(m):
        others = [(G[i], "leq", h[i]) for i in range(m) if i != k]
        res = _optimize(n, others, list(G[k]), maximize=True, nonneg=False)
        if res["status"] == "optimal":
            is_red = res["value"] <= h[k]
            note = str(res["value"])
        elif res["status"] == "unbounded":
            is_red = False
            note = "unbounded"
        else:  # infeasible others => vacuously redundant
            is_red = True
            note = "infeasible-without"
        detail.append({
            "row": k,
            "source_index": origin[k],
            "bound": str(h[k]),
            "max_over_others": note,
            "redundant": bool(is_red),
        })
        if is_red:
            redundant_rows.append(k)
    return {"status": "ok", "redundant": redundant_rows, "detail": detail,
            "variables": variables}


# --------------------------------------------------------------------------- #
# Worker dispatch.
# --------------------------------------------------------------------------- #

def run(request: dict[str, Any]) -> dict[str, Any]:
    """Worker entrypoint.  ``request["op"]`` selects the capability.

    * ``primal_dual`` -> :func:`primal_dual` (``objective``, ``constraints``,
      ``sense``);
    * ``chebyshev`` -> :func:`chebyshev_center` (``constraints``);
    * ``vertices`` -> :func:`vertex_enumeration` (``constraints``);
    * ``redundant`` -> :func:`redundant_constraints` (``constraints``).
    """
    op = request.get("op", "primal_dual")
    cons = request.get("constraints", request.get("inequalities", []))
    if op == "primal_dual":
        return primal_dual(request.get("objective", {}), cons,
                           request.get("sense", "max"))
    if op == "chebyshev":
        return chebyshev_center(cons)
    if op == "vertices":
        return vertex_enumeration(cons)
    if op == "redundant":
        return redundant_constraints(cons)
    raise ValueError(f"unknown op: {op!r}")
