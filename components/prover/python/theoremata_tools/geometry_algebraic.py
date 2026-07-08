"""Algebraic Euclidean-geometry prover via Wu's method (coordinate approach).

This is the *algebraic* companion to :mod:`theoremata_tools.geometry` (the
5-rule forward chainer + numeric checker). Where the forward chainer proves a
small, hand-coded family of facts by pattern-matching rules, this module proves
a much broader **class of equational Euclidean statements** -- collinearity,
concurrency, parallelism, perpendicularity, equal length, and *ratio* facts
computed through intersections -- by translating the whole configuration into
**polynomial equations over point coordinates** and deciding the goal with
**Wu's characteristic-set / pseudo-remainder** procedure.

The pipeline (classical Wu's method, as summarized in the paper-mining notes
``docs/paper-mining/quantum-automated-theorem-proving.md`` and the AlphaGeometry
baseline table):

1. **Coordinate translation.** Each point gets a pair of coordinates. A
   coordinate is either a fixed rational (to pin the frame -- e.g. ``A=(0,0)``,
   ``B=(u1,0)``) or a named variable. Independent variables (``u``'s) carry the
   degrees of freedom; dependent variables (``x``'s) are pinned by hypotheses.
   Each geometric hypothesis becomes one (or two) polynomial constraints
   ``h_i = 0``; the goal becomes ``g = 0``.

2. **Triangulation.** The hypothesis polynomials are triangulated into an
   *ascending chain* (characteristic set): one polynomial per dependent
   variable, each with a strictly larger "leading" variable, built by repeated
   **pseudo-division** (Ritt--Wu style, greedy -- see scope note below).

3. **Successive pseudo-remainder.** The goal is pseudo-divided by the chain,
   highest leading-variable first, eliminating dependent variables one at a
   time. If the final pseudo-remainder is the zero polynomial, the goal is an
   algebraic consequence of the hypotheses **modulo the non-degeneracy
   conditions** (the leading coefficients / *initials* of the chain must be
   nonzero). This is a sound algebraic certificate.

4. **Falsify-before-prove.** Before claiming a proof we *try to break it*: we
   sample the (triangular) hypothesis variety numerically from a caller-supplied
   ``seed`` -- assign the independent variables random rationals, solve the chain
   for the dependent ones -- and evaluate the goal. A single clean realization
   where the goal fails is a genuine counterexample: the conjecture is FALSE and
   no algebra is needed.

Self-contained: a tiny multivariate rational-coefficient polynomial arithmetic
(``fractions.Fraction``) lives in this file, so the prover and its core tests
run on the pure standard library. ``sympy``/``numpy`` are **not** required (an
optional test cross-checks against sympy if it happens to be installed).

Determinism: every numeric step is driven by the ``seed`` passed in the request;
no ambient randomness.

Honest scope
------------
* CAN: prove equational statements whose configuration is a *well-determined*
  construction (as many independent hypotheses as dependent coordinates), e.g.
  "the diagonals of a parallelogram bisect each other", "the medians are
  concurrent", midpoint/collinearity/parallel/perp/length identities obtained
  through line intersections and ratios -- exactly the ratio/general-collinearity
  class the 5-rule chainer cannot reach.
* CANNOT / caveats: the triangulation is a *pragmatic greedy* Ritt--Wu variant,
  not the full irreducible decomposition. A **zero** final remainder is always a
  sound certificate (modulo the reported non-degeneracy conditions); a
  **nonzero** remainder is *inconclusive*, not a disproof -- a true theorem can
  still leave a nonzero remainder if the chain is reducible (Wu's classic
  subtlety). We therefore never claim "false" from a nonzero remainder; falsity
  is decided only by the numeric falsifier. Inequalities, betweenness/order,
  transcendental (angle-in-radians) goals and 3-D are out of scope.
"""
from __future__ import annotations

import math
import random
from fractions import Fraction
from typing import Any, Iterable


# --------------------------------------------------------------------------- #
# Self-contained multivariate polynomial arithmetic over the rationals.
#
# A polynomial over ``n`` variables is a dict {exponent-tuple -> Fraction},
# exponent-tuple being a length-``n`` tuple of non-negative ints. Zero
# coefficients are pruned on construction so ``is_zero`` == empty dict.
# Variables are referred to by index 0..n-1; the *ordering* index0 < index1 <
# ... is the Wu variable order (independent params low, dependents high).
# --------------------------------------------------------------------------- #
Monomial = tuple[int, ...]


class Poly:
    __slots__ = ("n", "terms")

    def __init__(self, n: int, terms: dict[Monomial, Fraction] | None = None):
        self.n = n
        self.terms: dict[Monomial, Fraction] = {}
        if terms:
            for m, c in terms.items():
                c = Fraction(c)
                if c != 0:
                    self.terms[m] = c

    # -- constructors ------------------------------------------------------- #
    @classmethod
    def zero(cls, n: int) -> "Poly":
        return cls(n)

    @classmethod
    def const(cls, n: int, c) -> "Poly":
        c = Fraction(c)
        if c == 0:
            return cls(n)
        return cls(n, {(0,) * n: c})

    @classmethod
    def var(cls, n: int, i: int) -> "Poly":
        m = [0] * n
        m[i] = 1
        return cls(n, {tuple(m): Fraction(1)})

    # -- predicates --------------------------------------------------------- #
    def is_zero(self) -> bool:
        return not self.terms

    def is_const(self) -> bool:
        return all(all(e == 0 for e in m) for m in self.terms)

    # -- arithmetic --------------------------------------------------------- #
    def __add__(self, other: "Poly") -> "Poly":
        out = dict(self.terms)
        for m, c in other.terms.items():
            nc = out.get(m, Fraction(0)) + c
            if nc == 0:
                out.pop(m, None)
            else:
                out[m] = nc
        return Poly(self.n, out)

    def __neg__(self) -> "Poly":
        return Poly(self.n, {m: -c for m, c in self.terms.items()})

    def __sub__(self, other: "Poly") -> "Poly":
        return self + (-other)

    def __mul__(self, other: "Poly") -> "Poly":
        out: dict[Monomial, Fraction] = {}
        for m1, c1 in self.terms.items():
            for m2, c2 in other.terms.items():
                m = tuple(a + b for a, b in zip(m1, m2))
                nc = out.get(m, Fraction(0)) + c1 * c2
                if nc == 0:
                    out.pop(m, None)
                else:
                    out[m] = nc
        return Poly(self.n, out)

    def scale(self, c) -> "Poly":
        c = Fraction(c)
        if c == 0:
            return Poly.zero(self.n)
        return Poly(self.n, {m: v * c for m, v in self.terms.items()})

    # -- univariate view in variable ``v`` ---------------------------------- #
    def class_index(self) -> int:
        """Highest-order variable actually present; -1 for a constant."""
        cls = -1
        for m in self.terms:
            for i in range(self.n - 1, cls, -1):
                if m[i] > 0:
                    cls = i
                    break
        return cls

    def degree_in(self, v: int) -> int:
        return max((m[v] for m in self.terms), default=0)

    def coeff_in(self, v: int, d: int) -> "Poly":
        """Coefficient of ``x_v**d`` as a Poly (with ``x_v`` exponent zeroed)."""
        out: dict[Monomial, Fraction] = {}
        for m, c in self.terms.items():
            if m[v] == d:
                mm = list(m)
                mm[v] = 0
                out[tuple(mm)] = c
        return Poly(self.n, out)

    def leading_coeff_in(self, v: int) -> "Poly":
        return self.coeff_in(v, self.degree_in(v))

    def mul_x_pow(self, v: int, k: int) -> "Poly":
        if k == 0:
            return self
        out: dict[Monomial, Fraction] = {}
        for m, c in self.terms.items():
            mm = list(m)
            mm[v] += k
            out[tuple(mm)] = c
        return Poly(self.n, out)

    # -- evaluation --------------------------------------------------------- #
    def eval(self, assign: dict[int, float]) -> float:
        total = 0.0
        for m, c in self.terms.items():
            term = float(c)
            for i, e in enumerate(m):
                if e:
                    term *= assign.get(i, 0.0) ** e
            total += term
        return total

    # -- pretty print ------------------------------------------------------- #
    def to_str(self, names: list[str]) -> str:
        if self.is_zero():
            return "0"
        parts = []
        for m in sorted(self.terms, reverse=True):
            c = self.terms[m]
            mono = "".join(
                (names[i] if e == 1 else f"{names[i]}^{e}")
                for i, e in enumerate(m) if e
            )
            if not mono:
                parts.append(_fmt_frac(c))
            elif c == 1:
                parts.append(mono)
            elif c == -1:
                parts.append("-" + mono)
            else:
                parts.append(f"{_fmt_frac(c)}*{mono}")
        s = " + ".join(parts).replace("+ -", "- ")
        return s


def _fmt_frac(c: Fraction) -> str:
    return str(c.numerator) if c.denominator == 1 else f"{c.numerator}/{c.denominator}"


# --------------------------------------------------------------------------- #
# Pseudo-division: the workhorse of Wu's method.
#
# For polynomials g, f viewed as univariate in variable ``v`` with
# ``deg_v f = d`` and leading coeff (initial) I = lc_v(f), the pseudo-remainder
# r = prem(g, f, v) satisfies  I**k * g = q*f + r  with deg_v r < d, for some
# k >= 0 and polynomial q. All coefficient arithmetic is exact (Fraction), so
# no floating error enters the certificate.
# --------------------------------------------------------------------------- #
def pseudo_remainder(g: Poly, f: Poly, v: int) -> Poly:
    df = f.degree_in(v)
    if df == 0:
        raise ValueError("pseudo_remainder: divisor is constant in the variable")
    lcf = f.leading_coeff_in(v)
    r = g
    # Safety bound: each step strictly lowers deg_v(r); start deg is finite.
    for _ in range(g.degree_in(v) - df + 2 if g.degree_in(v) >= df else 0):
        dr = r.degree_in(v)
        if r.is_zero() or dr < df:
            break
        lcr = r.leading_coeff_in(v)
        # r <- lcf*r - lcr * x_v**(dr-df) * f
        r = (lcf * r) - (lcr * f.mul_x_pow(v, dr - df))
    return r


# --------------------------------------------------------------------------- #
# Triangulation into an ascending chain (greedy Ritt--Wu).
#
# Process variables from the highest index downward. Among the polynomials whose
# class (highest present variable) is the current variable ``v``, pick one of
# minimal degree in ``v`` as the pivot for that level, and pseudo-reduce every
# other class-``v`` polynomial against it (dropping the pivot's leading variable
# from them). Remainders fall to a lower class and are reconsidered at a lower
# level. The pivots, read low-to-high, form the characteristic set.
# --------------------------------------------------------------------------- #
def triangulate(polys: list[Poly], nvars: int) -> list[Poly]:
    current = [p for p in polys if not p.is_zero()]
    chain: list[Poly] = []
    for v in range(nvars - 1, -1, -1):
        withv = [p for p in current if p.class_index() == v]
        if not withv:
            continue
        pivot = min(withv, key=lambda p: (p.degree_in(v), len(p.terms)))
        chain.append(pivot)
        rest = [p for p in current if p is not pivot]
        newcurrent: list[Poly] = []
        for p in rest:
            if p.class_index() == v:
                r = pseudo_remainder(p, pivot, v)
                if not r.is_zero():
                    newcurrent.append(r)
                # a zero remainder means p was redundant given pivot: drop it
            else:
                newcurrent.append(p)
        current = newcurrent
    chain.reverse()  # ascending by class index
    return chain


def wu_reduce(goal: Poly, chain: list[Poly]) -> Poly:
    """Successive pseudo-remainder of ``goal`` wrt the chain, highest class first."""
    r = goal
    for f in sorted(chain, key=lambda p: p.class_index(), reverse=True):
        v = f.class_index()
        if v < 0:
            continue
        if r.degree_in(v) >= f.degree_in(v):
            r = pseudo_remainder(r, f, v)
        if r.is_zero():
            break
    return r


# --------------------------------------------------------------------------- #
# Coordinate model + predicate -> polynomial translation.
# --------------------------------------------------------------------------- #
class _Coords:
    """Registry mapping coordinate slots to variable indices / constants."""

    def __init__(self, points: dict[str, list[Any]], var_order: list[str] | None):
        # Collect variable names in a deterministic order.
        names: list[str] = list(var_order) if var_order else []
        for _, xy in points.items():
            for entry in xy:
                if isinstance(entry, str) and entry not in names:
                    names.append(entry)
        self.var_names = names
        self.n = len(names)
        self.index = {name: i for i, name in enumerate(names)}
        # Build (Poly, Poly) per point.
        self.pt: dict[str, tuple[Poly, Poly]] = {}
        for name, xy in points.items():
            if len(xy) != 2:
                raise ValueError(f"point {name!r} needs exactly 2 coordinates")
            self.pt[name] = (self._coord(xy[0]), self._coord(xy[1]))

    def _coord(self, entry: Any) -> Poly:
        if isinstance(entry, str):
            return Poly.var(self.n, self.index[entry])
        return Poly.const(self.n, Fraction(str(entry)))

    def point(self, name: str) -> tuple[Poly, Poly]:
        if name not in self.pt:
            raise ValueError(f"undefined point {name!r}")
        return self.pt[name]


def _predicate_polys(pred: str, pts: list[str], co: _Coords) -> list[Poly]:
    """Translate one predicate instance into polynomial constraints (== 0)."""
    def P(name: str) -> tuple[Poly, Poly]:
        return co.point(name)

    def sub(a, b):
        return (a[0] - b[0], a[1] - b[1])

    def cross(u, v):
        return u[0] * v[1] - u[1] * v[0]

    def dot(u, v):
        return u[0] * v[0] + u[1] * v[1]

    if pred == "collinear":
        a, b, c = (P(x) for x in pts)
        return [cross(sub(b, a), sub(c, a))]

    if pred == "parallel":
        a, b, c, d = (P(x) for x in pts)
        return [cross(sub(b, a), sub(d, c))]

    if pred == "perpendicular":
        a, b, c, d = (P(x) for x in pts)
        return [dot(sub(b, a), sub(d, c))]

    if pred in ("cong", "eqlen"):
        a, b, c, d = (P(x) for x in pts)
        ab, cd = sub(b, a), sub(d, c)
        return [dot(ab, ab) - dot(cd, cd)]

    if pred == "midpoint":  # M is the midpoint of A,B  -> 2M - A - B = 0 (x and y)
        m, a, b = (P(x) for x in pts)
        two = Poly.const(co.n, 2)
        return [two * m[0] - a[0] - b[0], two * m[1] - a[1] - b[1]]

    if pred == "eqprod":
        # |AB|^2 * |EF|^2 == |CD|^2 * |GH|^2  (ratio/similarity without sqrt)
        a, b, c, d, e, f, g, h = (P(x) for x in pts)
        ab, cd, ef, gh = sub(b, a), sub(d, c), sub(f, e), sub(h, g)
        return [dot(ab, ab) * dot(ef, ef) - dot(cd, cd) * dot(gh, gh)]

    raise ValueError(f"unknown predicate: {pred!r}")


def _goal_polys(goal: dict[str, Any], co: _Coords) -> list[Poly]:
    pts = list(goal.get("points", goal.get("args", [])))
    return _predicate_polys(goal["pred"], pts, co)


def _hypothesis_polys(hyps: list[dict[str, Any]], co: _Coords) -> list[Poly]:
    out: list[Poly] = []
    for h in hyps:
        pts = list(h.get("points", h.get("args", [])))
        out.extend(_predicate_polys(h["pred"], pts, co))
    return out


# --------------------------------------------------------------------------- #
# Numeric falsification over the (triangular) hypothesis variety.
#
# Assign the independent variables (those that are not the class variable of any
# chain polynomial) random rationals from the seed, then solve the chain in
# ascending order for each dependent variable (linear or quadratic in that
# variable once the lower ones are fixed). Evaluate the goal at the resulting
# point; a clean nonzero value is a counterexample.
# --------------------------------------------------------------------------- #
def _solve_univariate(coeffs: list[float]) -> list[float]:
    """Real roots of sum coeffs[d]*x**d, for degree 1 or 2. [] if unsolvable."""
    # strip leading (highest-degree) ~zero coeffs
    while coeffs and abs(coeffs[-1]) < 1e-12:
        coeffs = coeffs[:-1]
    deg = len(coeffs) - 1
    if deg <= 0:
        return []  # constant: no way to pin the variable (degenerate branch)
    if deg == 1:
        return [-coeffs[0] / coeffs[1]]
    if deg == 2:
        a, b, c = coeffs[2], coeffs[1], coeffs[0]
        disc = b * b - 4 * a * c
        if disc < 0:
            return []
        s = math.sqrt(disc)
        return [(-b + s) / (2 * a), (-b - s) / (2 * a)]
    return []  # higher degree: out of scope for the numeric sampler


def _sample_variety(chain: list[Poly], nvars: int, rng: random.Random,
                    spread: float) -> dict[int, float] | None:
    class_vars = {p.class_index(): p for p in chain if p.class_index() >= 0}
    assign: dict[int, float] = {}
    for i in range(nvars):
        if i not in class_vars:
            assign[i] = rng.uniform(-spread, spread)
    for v in sorted(class_vars):  # ascending: lower dependents first
        p = class_vars[v]
        deg = p.degree_in(v)
        coeffs = [p.coeff_in(v, d).eval(assign) for d in range(deg + 1)]
        roots = _solve_univariate(coeffs)
        if not roots:
            return None  # degenerate realization (initial vanished): resample
        assign[v] = roots[0]
    return assign


def numeric_falsify(goal_polys: list[Poly], chain: list[Poly], nvars: int,
                    seed: int, trials: int = 40, spread: float = 8.0,
                    tol: float = 1e-6) -> dict[str, Any]:
    """Seek a hypothesis-satisfying realization where some goal poly is nonzero."""
    rng = random.Random(seed)
    valid = 0
    degenerate = 0
    for _ in range(trials * 20):
        if valid >= trials:
            break
        assign = _sample_variety(chain, nvars, rng, spread)
        if assign is None:
            degenerate += 1
            continue
        valid += 1
        scale = max(1.0, max((abs(x) for x in assign.values()), default=1.0))
        tol_abs = tol * scale * scale
        for gp in goal_polys:
            val = gp.eval(assign)
            if abs(val) > tol_abs:
                return {
                    "falsified": True,
                    "counterexample": {i: round(x, 6) for i, x in assign.items()},
                    "residual": val,
                    "trials_valid": valid,
                    "trials_degenerate": degenerate,
                }
    return {
        "falsified": False,
        "trials_valid": valid,
        "trials_degenerate": degenerate,
    }


# --------------------------------------------------------------------------- #
# Top-level Wu's-method prover.
# --------------------------------------------------------------------------- #
def prove(points: dict[str, list[Any]], hypotheses: list[dict[str, Any]],
          goal: dict[str, Any], seed: int, var_order: list[str] | None = None,
          falsify_trials: int = 30) -> dict[str, Any]:
    co = _Coords(points, var_order)
    names = co.var_names
    hyp_polys = _hypothesis_polys(hypotheses, co)
    goal_polys = _goal_polys(goal, co)

    chain = triangulate(hyp_polys, co.n)

    # Falsify-before-prove: try to break the goal on the hypothesis variety.
    falsify = numeric_falsify(goal_polys, chain, co.n, seed, trials=falsify_trials)
    if falsify.get("falsified"):
        return {
            "op": "prove",
            "proved": False,
            "falsified": True,
            "counterexample": {
                names[i]: v for i, v in falsify["counterexample"].items()
            },
            "reason": "numeric counterexample on the hypothesis variety",
            "characteristic_set": [p.to_str(names) for p in chain],
        }

    # Successive pseudo-remainder of every goal component.
    remainders = [wu_reduce(gp, chain) for gp in goal_polys]
    all_zero = all(r.is_zero() for r in remainders)

    # Non-degeneracy conditions = initials (leading coeffs) of the chain.
    nondeg = []
    for p in sorted(chain, key=lambda q: q.class_index()):
        v = p.class_index()
        if v < 0:
            continue
        init = p.leading_coeff_in(v)
        if not init.is_zero() and not init.is_const():
            nondeg.append(init.to_str(names) + " != 0")

    result: dict[str, Any] = {
        "op": "prove",
        "proved": bool(all_zero),
        "falsified": False,
        "characteristic_set": [p.to_str(names) for p in chain],
        "goal_polynomials": [gp.to_str(names) for gp in goal_polys],
        "pseudo_remainders": [r.to_str(names) for r in remainders],
        "non_degeneracy": nondeg,
        "variables": names,
    }
    if all_zero:
        result["certificate"] = (
            "final pseudo-remainder is 0: goal is an algebraic consequence of "
            "the hypotheses modulo the stated non-degeneracy conditions"
        )
    else:
        result["inconclusive"] = True
        result["note"] = (
            "nonzero pseudo-remainder is INCONCLUSIVE (not a disproof): the "
            "greedy characteristic set may be reducible. Numeric falsify found "
            "no counterexample, so the goal is likely true but uncertified."
        )
    return result


# --------------------------------------------------------------------------- #
# Worker entrypoint.
# --------------------------------------------------------------------------- #
def run(request: dict[str, Any]) -> dict[str, Any]:
    """Worker entrypoint. ``request["op"]`` selects the capability.

    Common inputs:
      * ``points``      -- ``{name: [x, y]}``; each coord is a number (fixed) or
        a variable name (string). Pin a frame with numbers (``A=[0,0]``).
      * ``var_order``   -- optional list of variable names, ascending in the Wu
        order (independent parameters first, dependent coordinates last). Any
        variable not listed is appended in first-appearance order.
      * ``hypotheses``  -- list of ``{"pred": ..., "points": [...]}``.
      * ``goal``        -- one ``{"pred": ..., "points": [...]}``.
      * ``seed``        -- int seed for the deterministic numeric falsifier.

    Predicates: ``collinear(A,B,C)``, ``parallel(A,B,C,D)`` (AB||CD),
    ``perpendicular(A,B,C,D)``, ``cong(A,B,C,D)`` / ``eqlen`` (|AB|=|CD|),
    ``midpoint(M,A,B)``, ``eqprod(A,B,C,D,E,F,G,H)`` (|AB|.|EF| = |CD|.|GH|).

    Ops:
      * ``prove``   -> Wu's method. Returns ``proved`` (True iff the final
        pseudo-remainder is 0), the ``characteristic_set``, the
        ``pseudo_remainders``, and the ``non_degeneracy`` conditions. If the
        numeric pre-screen breaks the goal, returns ``proved=False`` with a
        ``counterexample``. A nonzero remainder is reported as ``inconclusive``,
        never as a disproof.
      * ``falsify`` -> numeric-only: triangulate the hypotheses and sample the
        variety from ``seed``; returns ``falsified`` and a ``counterexample`` (a
        realization satisfying the hypotheses where the goal fails), or
        ``falsified=False`` if none was found within the trial budget.
    """
    op = request.get("op", "prove")
    points = request["points"]
    var_order = request.get("var_order")

    if op == "prove":
        return prove(
            points,
            request.get("hypotheses", []),
            request["goal"],
            seed=int(request["seed"]),
            var_order=var_order,
            falsify_trials=int(request.get("falsify_trials", 30)),
        )

    if op == "falsify":
        co = _Coords(points, var_order)
        chain = triangulate(_hypothesis_polys(request.get("hypotheses", []), co), co.n)
        goal_polys = _goal_polys(request["goal"], co)
        res = numeric_falsify(
            goal_polys, chain, co.n,
            seed=int(request["seed"]),
            trials=int(request.get("trials", 40)),
            spread=float(request.get("spread", 8.0)),
            tol=float(request.get("tol", 1e-6)),
        )
        out: dict[str, Any] = {"op": "falsify", **res}
        if res.get("falsified"):
            out["counterexample"] = {
                co.var_names[i]: v for i, v in res["counterexample"].items()
            }
        return out

    raise ValueError(f"unknown op: {op!r}")
