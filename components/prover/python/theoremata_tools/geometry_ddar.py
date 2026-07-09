"""DD+AR geometry engine (AlphaGeometry-style deductive database + algebraic reasoning).

This module is the *chasing* companion to :mod:`theoremata_tools.geometry` (the
five hand-written forward-chaining rules + numeric checker) and
:mod:`theoremata_tools.geometry_algebraic` (Wu's-method coordinate prover). It
re-implements the fast symbolic core of AlphaGeometry -- **DD** (a deductive
database that forward-chains a *larger* sound rule set to a fixpoint DAG) plus
**AR** (algebraic reasoning: angle/ratio/length chasing unified as linear
equations over exact rationals and closed by Gaussian elimination) -- and runs
the two **alternately** until their joint closure stops growing.

Why this reaches facts :mod:`geometry` cannot
----------------------------------------------
:func:`geometry.deductive_prove` has exactly five rules and **no angle rule at
all**, so it cannot chase angles (it cannot even prove that ``eqangle`` is
transitive). AR fixes that: every angle equality is normalized -- following
AlphaGeometry -- to a linear relation among *line-direction* unknowns
``theta(line)`` (slope mod pi), with ``pi`` a distinguished variable; every
length/ratio equality to a linear relation among *log-length* unknowns
``L(segment)``. A goal is *proved* iff its normalized linear form lies in the
**row space** of the hypothesis relations, and the witnessing linear combination
(found by exact Gaussian elimination over :class:`fractions.Fraction`) is
returned as the **AR certificate**. Because the whole angle/length system is a
formal linear identity in these unknowns, a zero-remainder combination is a
*sound* certificate modulo the stated non-degeneracy (no two coincident points
on a referenced line).

The two engines feed each other:

* **DD -> AR.** Every ``parallel`` / ``perpendicular`` / ``cong`` fact DD derives
  (via the reused :data:`geometry._RULES` plus the extra sound rules here) is
  injected as a new AR linear equation.
* **AR -> DD.** After each Gaussian pass we read off any *new* ``parallel`` /
  ``cong`` equalities implied by the row space and add them back as DD facts, so
  the geometric rules can fire on algebraically-discovered equalities.

Sound additions to the rule set (beyond geometry.py's five)
    * inscribed-angle: ``concyclic(A,B,C,D)`` => directed ``eqangle`` on every
      chord (``<APB = <AQB`` for the two points P,Q off chord AB) -- injected as
      an *oriented* AR angle relation (directed angles mod pi make this exact and
      case-free);
    * all of angle **transitivity / addition** and ratio/length chasing, which
      AR performs natively through Gaussian elimination rather than as discrete
      rules.

Ops (see :func:`run`)
    * ``prove``   -- DD+AR joint closure; returns ``proved`` plus a DD
      ``derivation`` and/or an ``ar_certificate`` (linear combination). An
      optional ``construction`` triggers a numeric falsify-before-prove screen.
    * ``falsify`` -- numeric: reuses :func:`geometry.numeric_check` to seek a
      hypotheses-satisfying realization where the goal fails.
    * ``check``   -- numeric: reuses :func:`geometry.numeric_check`.

Honest scope
------------
* CAN: chase directed angles (parallel/perpendicular/eqangle/inscribed-angle,
  angle sums, transitivity) and log-length ratios (cong/eqratio) to closure and
  emit an exact linear certificate; still prove the geometry.py DD-only family
  (midpoint => equal segments & collinear, perpendicular/parallel transport,
  transitivities).
* CANNOT: introduce auxiliary points/constructions (no construction search);
  reason about betweenness/orientation of *unsigned* angles (AR is deliberately
  a *directed*-angle-mod-pi and *log-length* theory, so it decides equalities,
  not inequalities); mix the angle and length systems multiplicatively (that is
  Wu's job -- use :mod:`geometry_algebraic`). A proved goal is certified *modulo
  non-degeneracy* (referenced lines/segments are genuinely determined, i.e. no
  coincident endpoints). A goal that is neither in the DD closure nor the AR row
  space is reported ``proved=False`` -- *inconclusive*, never a disproof (falsity
  is decided only by the numeric screen).

Pure standard library (``math``, ``random``, ``fractions``, ``itertools``);
numpy is **not** required (an optional test cross-checks a solve against numpy if
it is installed). Determinism: every numeric step is driven by the caller's
``seed``; there is no ambient randomness.
"""
from __future__ import annotations

import itertools
from fractions import Fraction
from typing import Any

from theoremata_tools import geometry

# Reused verbatim from geometry.py (its predicate model + audited DD rules):
#   _canonical  -- canonical hashable fact keys (symmetry/transitivity baked in)
#   _describe   -- human rendering of a fact
#   _RULES      -- the five sound forward-chaining rules
#   _seg_key    -- unordered segment key
#   numeric_check -- the seeded numeric diagram checker (falsify/check ops)
_canonical = geometry._canonical
_describe = geometry._describe
_seg_key = geometry._seg_key
_GEOMETRY_RULES = geometry._RULES
Fact = geometry.Fact


# --------------------------------------------------------------------------- #
# Exact rational linear algebra: solve A x = b over Fraction, returning a
# particular solution (free variables set to 0) or None if inconsistent. This is
# the single primitive behind every AR query (row-space membership + the
# witnessing linear combination).
# --------------------------------------------------------------------------- #
def _solve_exact(mat: list[list[Fraction]], rhs: list[Fraction],
                 ncols: int) -> list[Fraction] | None:
    """Solve ``mat @ x = rhs`` exactly. ``mat`` has ``ncols`` columns.

    Returns one solution (free variables pinned to 0) or ``None`` when the system
    is inconsistent. All arithmetic is over :class:`fractions.Fraction`, so the
    answer is exact -- no floating error can enter the certificate.
    """
    # Augmented matrix; a copy so callers' rows are untouched.
    aug = [list(row) + [rhs[i]] for i, row in enumerate(mat)]
    nrows = len(aug)
    pivot_col_of_row: list[int] = []
    r = 0
    for c in range(ncols):
        # find a pivot in column c at or below row r
        piv = None
        for rr in range(r, nrows):
            if aug[rr][c] != 0:
                piv = rr
                break
        if piv is None:
            continue
        aug[r], aug[piv] = aug[piv], aug[r]
        pivinv = Fraction(1) / aug[r][c]
        aug[r] = [x * pivinv for x in aug[r]]
        for rr in range(nrows):
            if rr != r and aug[rr][c] != 0:
                factor = aug[rr][c]
                aug[rr] = [a - factor * b for a, b in zip(aug[rr], aug[r])]
        pivot_col_of_row.append(c)
        r += 1
        if r == nrows:
            break
    # consistency: any all-zero left side with nonzero rhs is a contradiction
    for rr in range(nrows):
        if all(aug[rr][c] == 0 for c in range(ncols)) and aug[rr][ncols] != 0:
            return None
    x = [Fraction(0)] * ncols
    for row_i, c in enumerate(pivot_col_of_row):
        x[c] = aug[row_i][ncols]
    return x


# --------------------------------------------------------------------------- #
# AR atoms and the normalization of each predicate to a linear relation.
#
# Atom keys (hashable):
#   ("pi",)                    -- the constant pi (an angle unknown pinned by use)
#   ("ang", frozenset{P,Q})    -- theta(line PQ): direction of line PQ mod pi
#   ("len", frozenset{P,Q})    -- L(PQ) = log|PQ|
#
# Angle relations live in the ANGLE system, length/ratio relations in the LENGTH
# system; the two are independent linear spaces (never mixed).
# --------------------------------------------------------------------------- #
PI: tuple = ("pi",)
Coeffs = dict  # atom -> Fraction


def _ang(p: str, q: str) -> tuple:
    return ("ang", frozenset((p, q)))


def _len(p: str, q: str) -> tuple:
    return ("len", frozenset((p, q)))


def _nondegenerate_line(p: str, q: str) -> bool:
    return p != q


class _ARRelation:
    """One linear equation ``sum coeff*atom == 0`` with a human label + system tag."""

    __slots__ = ("system", "coeffs", "label")

    def __init__(self, system: str, coeffs: Coeffs, label: str):
        self.system = system  # "angle" or "length"
        self.coeffs = {a: Fraction(c) for a, c in coeffs.items() if Fraction(c) != 0}
        self.label = label


def _relation_for(pred: str, pts: list[str], label: str) -> _ARRelation | None:
    """Translate one *ordered* predicate instance into an AR linear relation.

    Orientation matters for ``eqangle`` (directed angle mod pi), so this consumes
    the argument order directly -- it does **not** go through geometry's
    orientation-losing canonical form. Returns ``None`` for predicates AR does
    not model, or for degenerate (coincident-endpoint) lines/segments.
    """
    if pred == "parallel":
        a, b, c, d = pts
        if not (_nondegenerate_line(a, b) and _nondegenerate_line(c, d)):
            return None
        return _ARRelation("angle", {_ang(a, b): 1, _ang(c, d): -1}, label)

    if pred == "perpendicular":
        a, b, c, d = pts
        if not (_nondegenerate_line(a, b) and _nondegenerate_line(c, d)):
            return None
        # theta(AB) - theta(CD) - pi/2 = 0  (directed, mod pi)
        return _ARRelation(
            "angle",
            {_ang(a, b): 1, _ang(c, d): -1, PI: Fraction(-1, 2)},
            label,
        )

    if pred == "eqangle":
        p0, p1, p2, p3, p4, p5 = pts
        if not (_nondegenerate_line(p1, p2) and _nondegenerate_line(p1, p0)
                and _nondegenerate_line(p4, p5) and _nondegenerate_line(p4, p3)):
            return None
        # <p0 p1 p2 = <p3 p4 p5  (directed):
        #   [theta(p1p2)-theta(p1p0)] - [theta(p4p5)-theta(p4p3)] = 0
        return _ARRelation(
            "angle",
            {_ang(p1, p2): 1, _ang(p1, p0): -1, _ang(p4, p5): -1, _ang(p4, p3): 1},
            label,
        )

    if pred == "eqangle_ll":
        # directed angle between two named lines: <(ab,cd) = <(ef,gh)
        a, b, c, d, e, f, g, h = pts
        if not all(_nondegenerate_line(x, y)
                   for x, y in ((a, b), (c, d), (e, f), (g, h))):
            return None
        return _ARRelation(
            "angle",
            {_ang(c, d): 1, _ang(a, b): -1, _ang(g, h): -1, _ang(e, f): 1},
            label,
        )

    if pred in ("cong", "eqlen"):
        a, b, c, d = pts
        if not (_nondegenerate_line(a, b) and _nondegenerate_line(c, d)):
            return None
        return _ARRelation("length", {_len(a, b): 1, _len(c, d): -1}, label)

    if pred == "eqratio":
        # |AB|/|CD| = |EF|/|GH|  ->  L(AB)-L(CD)-L(EF)+L(GH) = 0
        a, b, c, d, e, f, g, h = pts
        if not all(_nondegenerate_line(x, y)
                   for x, y in ((a, b), (c, d), (e, f), (g, h))):
            return None
        return _ARRelation(
            "length",
            {_len(a, b): 1, _len(c, d): -1, _len(e, f): -1, _len(g, h): 1},
            label,
        )

    return None  # collinear / midpoint / concyclic are DD-only, not AR relations


# --------------------------------------------------------------------------- #
# The AR store: two independent linear systems (angle, length). Answers row-space
# membership queries and returns the witnessing linear combination (certificate).
# --------------------------------------------------------------------------- #
class _ARStore:
    def __init__(self) -> None:
        self.relations: dict[str, list[_ARRelation]] = {"angle": [], "length": []}
        self._seen: set[tuple] = set()

    def add(self, rel: _ARRelation | None) -> bool:
        if rel is None or not rel.coeffs:
            return False
        # de-dup on the (system, normalized-coeff) signature
        sig = (rel.system,) + tuple(sorted((repr(a), c) for a, c in rel.coeffs.items()))
        if sig in self._seen:
            return False
        self._seen.add(sig)
        self.relations[rel.system].append(rel)
        return True

    def _query(self, system: str, target: Coeffs
               ) -> list[tuple[Fraction, str]] | None:
        """Is ``target`` in the row space of ``system``? If so return the
        certificate as a list of ``(coefficient, equation-label)`` pairs."""
        rels = self.relations[system]
        atoms = set(target)
        for rel in rels:
            atoms.update(rel.coeffs)
        atom_list = sorted(atoms, key=repr)
        ncols = len(rels)
        mat = [[rel.coeffs.get(atom, Fraction(0)) for rel in rels]
               for atom in atom_list]
        rhs = [Fraction(target.get(atom, 0)) for atom in atom_list]
        sol = _solve_exact(mat, rhs, ncols)
        if sol is None:
            return None
        return [(sol[i], rels[i].label) for i in range(ncols) if sol[i] != 0]

    def prove_relation(self, pred: str, pts: list[str]
                       ) -> list[tuple[Fraction, str]] | None:
        """Try to certify one predicate as an AR consequence."""
        if pred == "perpendicular":
            # perpendicular is theta(AB)-theta(CD) == +/- pi/2 (mod pi); try both.
            a, b, c, d = pts
            if not (_nondegenerate_line(a, b) and _nondegenerate_line(c, d)):
                return None
            base = {_ang(a, b): Fraction(1), _ang(c, d): Fraction(-1)}
            for sign in (Fraction(-1, 2), Fraction(1, 2)):
                target = dict(base)
                target[PI] = sign
                cert = self._query("angle", target)
                if cert is not None:
                    return cert
            return None
        rel = _relation_for(pred, pts, label="<goal>")
        if rel is None:
            return None
        cert = self._query(rel.system, rel.coeffs)
        return cert

    # -- AR -> DD read-off ------------------------------------------------- #
    def readoff_equalities(self, max_atoms: int = 24
                           ) -> list[tuple[str, list[str]]]:
        """Read off *new* parallel / cong equalities implied by each system so
        they can be fed back to DD. Bounded (skips systems with many atoms)."""
        out: list[tuple[str, list[str]]] = []
        # parallel: theta(l1) - theta(l2) == 0
        ang_lines = sorted(
            {a for rel in self.relations["angle"] for a in rel.coeffs
             if isinstance(a, tuple) and a and a[0] == "ang"},
            key=repr,
        )
        if len(ang_lines) <= max_atoms:
            for l1, l2 in itertools.combinations(ang_lines, 2):
                target = {l1: Fraction(1), l2: Fraction(-1)}
                if self._query("angle", target) is not None:
                    p, q = tuple(l1[1])
                    r, s = tuple(l2[1])
                    out.append(("parallel", [p, q, r, s]))
        # cong: L(s1) - L(s2) == 0
        len_segs = sorted(
            {a for rel in self.relations["length"] for a in rel.coeffs
             if isinstance(a, tuple) and a and a[0] == "len"},
            key=repr,
        )
        if len(len_segs) <= max_atoms:
            for s1, s2 in itertools.combinations(len_segs, 2):
                target = {s1: Fraction(1), s2: Fraction(-1)}
                if self._query("length", target) is not None:
                    p, q = tuple(s1[1])
                    r, s = tuple(s2[1])
                    out.append(("cong", [p, q, r, s]))
        return out


# --------------------------------------------------------------------------- #
# Extra sound DD rules (over geometry.py's canonical fact form). These emit
# geometric facts; the inscribed-angle rule additionally hands AR an *oriented*
# angle relation, which is why it is handled inside the closure loop rather than
# as a pure fact->fact rule.
# --------------------------------------------------------------------------- #
def _fact_points(fact: Fact) -> Any:
    return fact


def _inscribed_angle_emissions(concyclic_fact: Fact
                               ) -> list[tuple[list[str], _ARRelation]]:
    """concyclic(pts) => directed eqangle on every chord of every 4-subset.

    For a chord {X,Y} and the two other points P,Q of a con-cyclic quadruple,
    the directed angles <XPY and <XQY are equal (inscribed-angle theorem, exact
    mod pi). Returns ``(eqangle_arg_points, ar_relation)`` pairs.
    """
    pts = sorted(concyclic_fact[1])
    out: list[tuple[list[str], _ARRelation]] = []
    for quad in itertools.combinations(pts, 4):
        a, b, c, d = quad
        # three ways to split a 4-set into (chord, viewers)
        for (x, y), (p, q) in (
            ((a, b), (c, d)),
            ((a, c), (b, d)),
            ((a, d), (b, c)),
        ):
            eq_pts = [x, p, y, x, q, y]  # <x p y = <x q y
            label = f"inscribed-angle: <{x}{p}{y}=<{x}{q}{y} (concyclic)"
            rel = _relation_for("eqangle", eq_pts, label)
            if rel is not None:
                out.append((eq_pts, rel))
    return out


def _parallel_perp_cong_to_relation(fact: Fact) -> _ARRelation | None:
    """Turn a DD-derived orientation-free fact into its AR relation.

    ``parallel`` / ``cong`` are symmetric so any orientation is fine;
    ``perpendicular`` is emitted with a fixed orientation (goal queries test both
    pi signs, so a single consistent orientation here is sound)."""
    kind = fact[0]
    if kind == "parallel":
        s1, s2 = tuple(fact[1])
        (a, b), (c, d) = tuple(s1), tuple(s2)
        return _relation_for("parallel", [a, b, c, d], _describe(fact))
    if kind == "cong":
        s1, s2 = tuple(fact[1])
        (a, b), (c, d) = tuple(s1), tuple(s2)
        return _relation_for("cong", [a, b, c, d], _describe(fact))
    if kind == "perpendicular":
        s1, s2 = tuple(fact[1])
        (a, b), (c, d) = tuple(s1), tuple(s2)
        return _relation_for("perpendicular", [a, b, c, d], _describe(fact))
    return None


# --------------------------------------------------------------------------- #
# The joint DD+AR closure.
# --------------------------------------------------------------------------- #
class _Closure:
    def __init__(self, hypotheses: list[dict[str, Any]]):
        self.facts: set[Fact] = set()
        self.reasons: dict[Fact, tuple[str, list[Fact]]] = {}
        self.ar = _ARStore()
        self.hypotheses = hypotheses
        self._seed()

    def _add_fact(self, fact: Fact, rule: str, premises: list[Fact]) -> bool:
        if fact in self.facts:
            return False
        self.facts.add(fact)
        # keep first (shallowest) reason only; hypotheses carry no reason
        if rule is not None and fact not in self.reasons:
            self.reasons[fact] = (rule, premises)
        return True

    def _seed(self) -> None:
        for h in self.hypotheses:
            pred = h["pred"]
            pts = list(h.get("points", h.get("args", [])))
            # DD fact (only for predicates geometry can canonicalize)
            try:
                fact = _canonical(pred, pts)
                self._add_fact(fact, None, [])
            except ValueError:
                pass  # e.g. eqangle_ll / eqratio: AR-only predicates
            # AR relation seeded from the *ordered* hypothesis
            self.ar.add(_relation_for(pred, pts, label=_hyp_label(pred, pts)))

    def run(self, max_rounds: int = 32) -> None:
        for _ in range(max_rounds):
            changed = False
            # ---- DD round: geometry's five rules + extra rules ------------- #
            for rule_name, rule in _GEOMETRY_RULES:
                for new_fact, premises in rule(self.facts):
                    if self._add_fact(new_fact, rule_name, premises):
                        changed = True
                        rel = _parallel_perp_cong_to_relation(new_fact)
                        if rel is not None and self.ar.add(rel):
                            pass
            # inscribed-angle rule (concyclic => oriented eqangle into DD + AR)
            for fact in [f for f in self.facts if f[0] == "concyclic"]:
                for eq_pts, rel in _inscribed_angle_emissions(fact):
                    eq_fact = _canonical("eqangle", eq_pts)
                    if self._add_fact(eq_fact, "inscribed-angle (concyclic)", [fact]):
                        changed = True
                    if self.ar.add(rel):
                        changed = True
            # feed any parallel/perp/cong hypotheses/facts not yet in AR
            for fact in self.facts:
                rel = _parallel_perp_cong_to_relation(fact)
                if rel is not None:
                    self.ar.add(rel)
            # ---- AR -> DD read-off ---------------------------------------- #
            for pred, pts in self.ar.readoff_equalities():
                try:
                    fact = _canonical(pred, pts)
                except ValueError:
                    continue
                if self._add_fact(fact, "AR (Gaussian elimination)", []):
                    changed = True
            if not changed:
                break

    # -- goal queries ------------------------------------------------------ #
    def dd_derivation(self, goal_fact: Fact) -> list[dict[str, Any]] | None:
        if goal_fact not in self.facts:
            return None
        order: list[Fact] = []
        seen: set[Fact] = set()

        def emit(f: Fact) -> None:
            if f in seen:
                return
            seen.add(f)
            if f in self.reasons:
                for p in self.reasons[f][1]:
                    emit(p)
                order.append(f)

        emit(goal_fact)
        return [
            {"fact": _describe(f), "rule": self.reasons[f][0],
             "from": [_describe(p) for p in self.reasons[f][1]]}
            for f in order
        ]


def _hyp_label(pred: str, pts: list[str]) -> str:
    return f"{pred}({','.join(pts)})"


# --------------------------------------------------------------------------- #
# Numeric falsify-before-prove screen (reuses geometry.numeric_check).
# --------------------------------------------------------------------------- #
# geometry.numeric_check evaluates these predicates:
_NUMERIC_PREDS = {"collinear", "midpoint", "cong", "eqlen", "parallel",
                  "perpendicular", "eqangle", "concyclic"}


def _numeric_screen(request: dict[str, Any]) -> dict[str, Any] | None:
    """Run the numeric diagram check if a ``construction`` + seed are supplied and
    the goal predicate is numerically evaluable. Returns a falsification result
    (to short-circuit ``prove``) or ``None`` to continue to the symbolic engine."""
    construction = request.get("construction")
    goal = request["goal"]
    if construction is None or goal["pred"] not in _NUMERIC_PREDS:
        return None
    res = geometry.numeric_check(
        construction,
        goal,
        seed=int(request["seed"]),
        trials=int(request.get("trials", 40)),
        tol=float(request.get("tol", 1e-6)),
        spread=float(request.get("spread", 10.0)),
    )
    if not res["holds"] and res.get("trials_valid", 0) > 0 and "counterexample" in res:
        return {
            "op": "geometry_ddar",
            "proved": False,
            "falsified": True,
            "counterexample": res["counterexample"],
            "reason": "numeric counterexample: goal fails on a clean realization",
            "trials_valid": res["trials_valid"],
        }
    return None


# --------------------------------------------------------------------------- #
# Top-level prover.
# --------------------------------------------------------------------------- #
def prove(hypotheses: list[dict[str, Any]], goal: dict[str, Any],
          request: dict[str, Any]) -> dict[str, Any]:
    # 1. numeric falsify-before-prove (optional; needs a construction)
    screen = _numeric_screen(request)
    if screen is not None:
        return screen

    # 2. DD+AR joint closure
    closure = _Closure(hypotheses)
    closure.run(max_rounds=int(request.get("max_rounds", 32)))

    pred = goal["pred"]
    pts = list(goal.get("points", goal.get("args", [])))

    # 3a. AR certificate (angle/length chasing)
    ar_cert = closure.ar.prove_relation(pred, pts)
    # 3b. DD derivation (geometry-canonicalizable predicates)
    dd_derivation = None
    try:
        goal_fact = _canonical(pred, pts)
        dd_derivation = closure.dd_derivation(goal_fact)
    except ValueError:
        goal_fact = None

    proved = ar_cert is not None or dd_derivation is not None
    result: dict[str, Any] = {
        "op": "geometry_ddar",
        "proved": bool(proved),
        "falsified": False,
        "goal": {"pred": pred, "points": pts},
        "closure_size": len(closure.facts),
        "ar_angle_relations": len(closure.ar.relations["angle"]),
        "ar_length_relations": len(closure.ar.relations["length"]),
    }
    if ar_cert is not None:
        result["ar_certificate"] = [
            {"coeff": _fmt_frac(c), "equation": label} for c, label in ar_cert
        ]
        result["ar_method"] = (
            "Gaussian elimination over exact rationals: the goal's normalized "
            "linear form is the above rational combination of hypothesis "
            "relations (angles = line slopes mod pi; lengths = log-lengths)."
        )
    if dd_derivation is not None:
        result["derivation"] = dd_derivation
    if proved:
        result["certificate"] = (
            "goal is in the DD closure and/or the AR row space -- a sound "
            "consequence modulo non-degeneracy (no coincident points on a "
            "referenced line/segment)."
        )
    else:
        result["inconclusive"] = True
        result["note"] = (
            "goal not reached by DD+AR closure. This is INCONCLUSIVE, not a "
            "disproof: try Wu's method (geometry_algebraic) or supply a "
            "construction for the numeric screen."
        )
    return result


def _fmt_frac(c: Fraction) -> str:
    return str(c.numerator) if c.denominator == 1 else f"{c.numerator}/{c.denominator}"


# --------------------------------------------------------------------------- #
# Worker entrypoint.
# --------------------------------------------------------------------------- #
def run(request: dict[str, Any]) -> dict[str, Any]:
    """Worker entrypoint (suggested op/tool name: ``geometry_ddar``).

    Inputs
      * ``hypotheses`` -- list of ``{"pred": ..., "points": [...]}``.
      * ``goal``       -- one ``{"pred": ..., "points": [...]}``.
      * ``seed``       -- int seed for the numeric ops / screen.
      * ``construction`` (optional, for ``prove``/``check``/``falsify``) -- a
        geometry.py construction list; enables the numeric falsify-before-prove
        screen and the numeric ops.

    Predicates
      DD + AR: ``collinear(A,B,C)``, ``concyclic(A,B,C,D)``, ``midpoint(M,A,B)``,
      ``parallel(A,B,C,D)`` (AB||CD), ``perpendicular(A,B,C,D)``,
      ``cong(A,B,C,D)`` / ``eqlen`` (|AB|=|CD|), ``eqangle(A,B,C,D,E,F)``
      (directed <ABC = <DEF).
      AR-only (chasing): ``eqangle_ll(A,B,C,D,E,F,G,H)`` (directed angle between
      lines AB,CD equals that between EF,GH) and ``eqratio(A,B,C,D,E,F,G,H)``
      (|AB|/|CD| = |EF|/|GH|).

    Ops
      * ``prove``   -- DD+AR joint closure. Returns ``proved``; when proved, a DD
        ``derivation`` and/or an ``ar_certificate`` (the exact linear
        combination). Optional numeric pre-screen if ``construction`` is given.
      * ``falsify`` -- reuses :func:`geometry.numeric_check`; returns
        ``falsified`` + ``counterexample`` (requires ``construction``).
      * ``check``   -- reuses :func:`geometry.numeric_check` (requires
        ``construction``).
    """
    op = request.get("op", "prove")

    if op == "prove":
        return prove(request.get("hypotheses", []), request["goal"], request)

    if op in ("check", "falsify"):
        result = geometry.numeric_check(
            request["construction"],
            request["goal"],
            seed=int(request["seed"]),
            trials=int(request.get("trials", 40)),
            tol=float(request.get("tol", 1e-6)),
            spread=float(request.get("spread", 10.0)),
        )
        if op == "falsify":
            return {
                "op": "geometry_ddar",
                "sub_op": "falsify",
                "goal": result["goal"],
                "falsified": (not result["holds"]
                              and result["trials_valid"] > 0
                              and "counterexample" in result),
                "counterexample": result.get("counterexample"),
                "trials_valid": result["trials_valid"],
                "trials_degenerate": result["trials_degenerate"],
                "reason": result.get("reason"),
            }
        return {"op": "geometry_ddar", "sub_op": "check", **result}

    raise ValueError(f"unknown op: {op!r}")
