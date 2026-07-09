"""Extended **sound** forward-chaining geometry rule set for Theoremata.

This is a *new* module layered on top of :mod:`theoremata_tools.geometry`. It does
not modify ``geometry.py``; it reuses that module's predicate model and low-level
numeric helpers and adds:

* a small set of **extra predicate types** needed for advanced olympiad
  reasoning that the seven ``geometry.py`` predicates cannot express:
  ``deqangle`` (directed angle equality, mod pi), ``eqratio`` (segment-ratio
  equality), ``simtri`` / ``contri`` (triangle similarity / congruence),
  ``radax`` (radical axis of two circles), ``concurrent`` (three lines meet at a
  point), ``harmonic`` (harmonic range / cross-ratio -1) and ``simcenter``
  (external similitude center);
* a **catalog of sound deduction rules** -- each a theorem of Euclidean geometry
  -- expressed as premise-pattern => conclusion matchers over a fact set, in the
  same shape as ``geometry._RULES`` so a chainer (``geometry.py`` /
  ``geometry_ddar2``) can consume them;
* a standalone fixpoint driver :func:`apply_rules` so the catalog is testable
  without the DDAR2 engine;
* for **every** rule a ``numeric_verify`` hook: a concrete seeded construction
  realizing the rule's premises, against which the conclusion is checked in many
  random non-degenerate realizations. An unsound rule is therefore caught by the
  test-suite sweep.

Soundness discipline
--------------------
* Angle reasoning uses **directed angles mod pi** (``deqangle``), never
  ``geometry.py``'s *unsigned* ``eqangle`` for derived conclusions, because the
  unsigned inscribed-angle identity is only true up to supplement. (Unsigned
  ``eqangle`` *is* emitted where magnitudes are genuinely equal, e.g. from a
  similar-triangle premise.)
* Rules that hold only away from a degeneracy carry a numeric guard
  (``ncoll`` / ``npara`` / distinctness). When a coordinate assignment
  ``points`` is supplied to the chainer the guard is enforced against it; with no
  coordinates the guard assumes generic position (the DDAR contract).
* Anything that could not be made sound over this predicate model is **omitted**,
  never shipped (see module-level ``OMITTED`` note at the bottom).

Provenance: rule *statements* are re-derived from the mathematics. The
``source`` tag records lineage -- ``"v1"`` (AlphaGeometry ``rules.txt``),
``"newclid"`` (Newclid ``new_rules.txt`` r44-r51), ``"clean-room-tong"``
(re-implemented from the *mathematical statement* of a TongGeometry rule; **no
GPLv3 source was copied**). No network, no third-party deps; pure stdlib.
"""
from __future__ import annotations

import math
import random
from dataclasses import dataclass, field
from itertools import permutations
from typing import Any, Callable, Optional

from theoremata_tools import geometry
from theoremata_tools.geometry import (
    DegenerateRealization,
    _add,
    _circumcenter,
    _cross,
    _dot,
    _foot,
    _line_intersection,
    _norm,
    _reflect_over_line,
    _scale,
    _sub,
)

Point = tuple[float, float]
Fact = tuple[Any, ...]

# Predicate heads reused verbatim from geometry.py (same canonical form).
_SHARED = {
    "collinear", "concyclic", "parallel", "perpendicular",
    "cong", "eqlen", "midpoint", "eqangle",
}
# All predicate heads (for point extraction).
_HEADS = _SHARED | {
    "deqangle", "eqratio", "simtri", "contri",
    "radax", "concurrent", "harmonic", "simcenter",
}


# --------------------------------------------------------------------------- #
# Complex-number helpers for rotations / similarities.
# --------------------------------------------------------------------------- #
def _cn(p: Point) -> complex:
    return complex(p[0], p[1])


def _pt(z: complex) -> Point:
    return (z.real, z.imag)


def _rot90(v: Point) -> Point:
    return (-v[1], v[0])


def _unit(v: Point) -> Point:
    n = _norm(v)
    if n < 1e-12:
        raise DegenerateRealization("unit: zero vector")
    return (v[0] / n, v[1] / n)


# --------------------------------------------------------------------------- #
# Construction realizer. Superset of geometry._realize's ops, adding the
# circle / similarity / harmonic ops the advanced witnesses need. Free points
# accept a per-step ``spread`` override so witnesses can keep circle centers
# close while pushing radius-reference points far (guaranteeing intersection).
# --------------------------------------------------------------------------- #
def _need(env: dict[str, Point], name: str) -> Point:
    if name not in env:
        raise ValueError(f"construction references undefined point {name!r}")
    return env[name]


def _circle_intersect(o1: Point, r1: float, o2: Point, r2: float,
                      which: int) -> Point:
    d = _norm(_sub(o2, o1))
    if d < 1e-9:
        raise DegenerateRealization("cc: concentric")
    if d > r1 + r2 - 1e-9 or d < abs(r1 - r2) + 1e-9:
        raise DegenerateRealization("cc: circles do not properly intersect")
    a = (r1 * r1 - r2 * r2 + d * d) / (2 * d)
    h2 = r1 * r1 - a * a
    if h2 < 1e-12:
        raise DegenerateRealization("cc: tangent")
    h = math.sqrt(h2)
    ux = _scale(_sub(o2, o1), 1.0 / d)
    mid = _add(o1, _scale(ux, a))
    perp = _rot90(ux)
    off = _scale(perp, h if which == 0 else -h)
    return _add(mid, off)


def _realize(construction: list[dict[str, Any]], rng: random.Random,
             spread: float) -> dict[str, Point]:
    env: dict[str, Point] = {}
    scal: dict[str, float] = {}
    for step in construction:
        op = step["op"]
        name = step.get("point")
        if op == "free":
            s = float(step.get("spread", spread))
            env[name] = (rng.uniform(-s, s), rng.uniform(-s, s))
        elif op == "rand_angle":
            scal[name] = rng.uniform(0.3, math.pi - 0.3)
        elif op == "midpoint":
            a, b = (_need(env, p) for p in step["of"])
            env[name] = _scale(_add(a, b), 0.5)
        elif op in ("on_line", "on_segment"):
            a, b = (_need(env, p) for p in step["of"])
            lo, hi = (0.15, 0.85) if op == "on_segment" else (-1.0, 2.0)
            t = rng.uniform(lo, hi)
            if abs(t) < 1e-3 or abs(t - 1.0) < 1e-3:
                raise DegenerateRealization("on_line: coincides with endpoint")
            env[name] = _add(a, _scale(_sub(b, a), t))
        elif op == "foot":
            p = _need(env, step["from"])
            a, b = (_need(env, q) for q in step["of"])
            env[name] = _foot(p, a, b)
        elif op == "reflect_point":
            of = _need(env, step["of"])
            center = _need(env, step["center"])
            env[name] = _sub(_scale(center, 2.0), of)
        elif op == "reflect_line":
            of = _need(env, step["of"])
            a, b = (_need(env, q) for q in step["over"])
            env[name] = _reflect_over_line(of, a, b)
        elif op == "intersection":
            a, b = (_need(env, q) for q in step["line1"])
            c, d = (_need(env, q) for q in step["line2"])
            env[name] = _line_intersection(a, b, c, d)
        elif op == "circumcenter":
            a, b, c = (_need(env, q) for q in step["of"])
            env[name] = _circumcenter(a, b, c)
        elif op == "on_circle":
            o = _need(env, step["center"])
            r = _norm(_sub(_need(env, step["through"]), o))
            if r < 1e-6:
                raise DegenerateRealization("on_circle: zero radius")
            th = rng.uniform(0.0, 2.0 * math.pi)
            env[name] = _add(o, (r * math.cos(th), r * math.sin(th)))
        elif op == "cc":
            o1 = _need(env, step["c1"][0])
            r1 = _norm(_sub(_need(env, step["c1"][1]), o1))
            o2 = _need(env, step["c2"][0])
            r2 = _norm(_sub(_need(env, step["c2"][1]), o2))
            env[name] = _circle_intersect(o1, r1, o2, r2, int(step.get("which", 0)))
        elif op == "extsim":
            o1 = _need(env, step["c1"][0])
            r1 = _norm(_sub(_need(env, step["c1"][1]), o1))
            o2 = _need(env, step["c2"][0])
            r2 = _norm(_sub(_need(env, step["c2"][1]), o2))
            if abs(r1 - r2) < 1e-6 * max(r1, r2, 1.0):
                raise DegenerateRealization("extsim: equal radii")
            env[name] = _scale(_sub(_scale(o1, r2), _scale(o2, r1)), 1.0 / (r2 - r1))
        elif op == "perp_from":
            at = _need(env, step["at"])
            a, b = (_need(env, q) for q in step["line"])
            d = _rot90(_sub(b, a))
            t = rng.uniform(0.4, 1.6) * (1 if rng.random() < 0.5 else -1)
            env[name] = _add(at, _scale(d, t))
        elif op == "para_from":
            at = _need(env, step["at"])
            a, b = (_need(env, q) for q in step["dir"])
            t = rng.uniform(0.4, 1.6) * (1 if rng.random() < 0.5 else -1)
            env[name] = _add(at, _scale(_sub(b, a), t))
        elif op == "seg_len":
            frm = _need(env, step["from"])
            a, b = (_need(env, q) for q in step["len_ref"])
            r = _norm(_sub(b, a))
            if r < 1e-6:
                raise DegenerateRealization("seg_len: zero reference")
            th = rng.uniform(0.0, 2.0 * math.pi)
            env[name] = _add(frm, (r * math.cos(th), r * math.sin(th)))
        elif op == "incenter":
            a, b, c = (_need(env, q) for q in step["of"])
            la = _norm(_sub(b, c))
            lb = _norm(_sub(c, a))
            lc = _norm(_sub(a, b))
            s = la + lb + lc
            if s < 1e-9:
                raise DegenerateRealization("incenter: degenerate triangle")
            env[name] = _scale(_add(_add(_scale(a, la), _scale(b, lb)),
                                    _scale(c, lc)), 1.0 / s)
        elif op in ("sim_image", "iso_image"):
            a, b, c = (_cn(_need(env, q)) for q in step["tri"])
            p, q = (_cn(_need(env, r)) for r in step["base"])
            if abs(a - b) < 1e-9:
                raise DegenerateRealization("image: degenerate base")
            alpha = (p - q) / (a - b)
            env[name] = _pt(alpha * c + (p - alpha * a))
        elif op == "harm":
            a, b = (_need(env, q) for q in step["base"])
            c = _need(env, step["pt"])
            u = _sub(b, a)
            dd = _dot(u, u)
            if dd < 1e-12:
                raise DegenerateRealization("harm: degenerate base")
            tc = _dot(_sub(c, a), u) / dd
            if abs(2.0 * tc - 1.0) < 1e-3:
                raise DegenerateRealization("harm: conjugate at infinity")
            td = tc / (2.0 * tc - 1.0)
            env[name] = _add(a, _scale(u, td))
        elif op == "ray_at_angle":
            v = _cn(_need(env, step["vertex"]))
            a = _cn(_need(env, step["ray"]))
            th = scal[step["angle_ref"]]
            dirn = a - v
            if abs(dirn) < 1e-9:
                raise DegenerateRealization("ray_at_angle: zero ray")
            env[name] = _pt(v + dirn * complex(math.cos(th), math.sin(th)))
        else:
            raise ValueError(f"unknown construction op: {op!r}")
    return env


# --------------------------------------------------------------------------- #
# Fact canonicalization. Shared predicates delegate to geometry._canonical so
# facts unify across the two modules; new predicates get their own canonical
# (symmetry-respecting) keys.
# --------------------------------------------------------------------------- #
def _seg(a: str, b: str) -> frozenset:
    return frozenset((a, b))


def _canon_deqangle(pts) -> Fact:
    a, b, c, d, e, f = pts
    # directed angle at vertex b, from ray b->a to ray b->c: (b, a, c)
    da1, da2 = (b, a, c), (e, d, f)
    opt1 = frozenset((da1, da2))
    opt2 = frozenset(((b, c, a), (e, f, d)))  # simultaneous flip negates both
    return ("deqangle", min((opt1, opt2), key=lambda s: sorted(map(str, s))))


def canon(pred: str, pts) -> Fact:
    """Canonical, hashable key for a predicate instance."""
    pts = list(pts)
    if pred in _SHARED:
        return geometry._canonical(pred, pts)
    if pred == "deqangle":
        return _canon_deqangle(pts)
    if pred == "eqratio":
        # |ab|/|cd| = |ef|/|gh|  <=>  |ab|.|gh| = |cd|.|ef|. Canonicalize as the
        # unordered pair of the two equal products (each product an unordered
        # pair of segments) so all valid rearrangements unify to one key.
        a, b, c, d, e, f, g, h = pts

        def _prod(s1, s2):
            return tuple(sorted((s1, s2), key=lambda s: tuple(sorted(s))))
        p1 = _prod(_seg(a, b), _seg(g, h))
        p2 = _prod(_seg(c, d), _seg(e, f))
        return ("eqratio", frozenset((p1, p2)))
    if pred in ("simtri", "contri"):
        a, b, c, p, q, r = pts
        return (pred, frozenset(((a, b, c), (p, q, r))))
    if pred == "radax":
        p, q, o1, o2 = pts
        return ("radax", _seg(p, q), _seg(o1, o2))
    if pred == "concurrent":
        a, b, c, d, e, f = pts
        return ("concurrent", frozenset((_seg(a, b), _seg(c, d), _seg(e, f))))
    if pred == "harmonic":
        a, b, c, d = pts
        return ("harmonic", frozenset((_seg(a, b), _seg(c, d))))
    if pred == "simcenter":
        e, o1, o2 = pts
        return ("simcenter", e, _seg(o1, o2))
    raise ValueError(f"unknown predicate: {pred!r}")


def F(pred: str, *pts) -> Fact:
    return canon(pred, pts)


def _collect_points(fact: Any, out: set) -> None:
    if isinstance(fact, str):
        out.add(fact)
    elif isinstance(fact, (tuple, frozenset, set, list)):
        for x in fact:
            _collect_points(x, out)


def points_in(facts) -> set:
    """All point names appearing in a fact set (predicate heads excluded)."""
    out: set = set()
    for f in facts:
        for part in f[1:]:
            _collect_points(part, out)
    out -= _HEADS
    return out


# --------------------------------------------------------------------------- #
# Numeric predicate evaluation (soundness witness engine). Shared predicates
# defer to geometry._eval_predicate; new predicates evaluated here. Each returns
# bool and raises DegenerateRealization when too degenerate to judge.
# --------------------------------------------------------------------------- #
def _slope(p: Point, q: Point) -> float:
    v = _sub(q, p)
    if _norm(v) < 1e-9:
        raise DegenerateRealization("slope: coincident points")
    return math.atan2(v[1], v[0]) % math.pi


def _dir_angle(env, v: str, a: str, b: str) -> float:
    """Directed angle (mod pi) from line v-a to line v-b."""
    return (_slope(env[v], env[b]) - _slope(env[v], env[a])) % math.pi


def _len(env, a: str, b: str) -> float:
    return _norm(_sub(env[a], env[b]))


def _eval(pred: str, pts, env: dict[str, Point], scale: float, tol: float) -> bool:
    pts = list(pts)
    if pred in _SHARED:
        return geometry._eval_predicate(pred, pts, env, scale, tol)

    a_tol = tol * scale

    if pred == "deqangle":
        a, b, c, d, e, f = pts
        d1 = _dir_angle(env, b, a, c)
        d2 = _dir_angle(env, e, d, f)
        diff = (d1 - d2) % math.pi
        return min(diff, math.pi - diff) <= max(tol * 10, 1e-7)

    if pred == "eqratio":
        a, b, c, d, e, f, g, h = pts
        lhs = _len(env, a, b) * _len(env, g, h)
        rhs = _len(env, c, d) * _len(env, e, f)
        return abs(lhs - rhs) <= (tol * 10) * scale * scale

    if pred == "simtri":
        a, b, c, p, q, r = pts
        s = [_len(env, a, b), _len(env, b, c), _len(env, c, a)]
        t = [_len(env, p, q), _len(env, q, r), _len(env, r, p)]
        if min(t) < 1e-9 or min(s) < 1e-9:
            raise DegenerateRealization("simtri: degenerate triangle")
        ratios = [s[i] / t[i] for i in range(3)]
        return max(ratios) - min(ratios) <= (tol * 10) * max(ratios)

    if pred == "contri":
        a, b, c, p, q, r = pts
        return (abs(_len(env, a, b) - _len(env, p, q)) <= a_tol
                and abs(_len(env, b, c) - _len(env, q, r)) <= a_tol
                and abs(_len(env, c, a) - _len(env, r, p)) <= a_tol)

    if pred == "radax":
        p, q, o1, o2 = pts
        return (abs(_len(env, o1, p) - _len(env, o1, q)) <= a_tol
                and abs(_len(env, o2, p) - _len(env, o2, q)) <= a_tol)

    if pred == "concurrent":
        a, b, c, d, e, f = pts
        z = _line_intersection(env[a], env[b], env[c], env[d])
        cr = _cross(_sub(env[f], z), _sub(env[e], z))
        return abs(cr) <= a_tol * scale

    if pred == "harmonic":
        a, b, c, d = pts
        u = _sub(env[b], env[a])
        dd = _dot(u, u)
        if dd < 1e-12:
            raise DegenerateRealization("harmonic: degenerate base")

        def param(name):
            return _dot(_sub(env[name], env[a]), u) / dd
        # collinearity precondition
        for name in (c, d):
            proj = _add(env[a], _scale(u, param(name)))
            if _norm(_sub(env[name], proj)) > a_tol:
                raise DegenerateRealization("harmonic: point off the line")
        tc, td = param(c), param(d)
        if abs(tc - 1) < 1e-9 or abs(td - 1) < 1e-9 or abs(tc) < 1e-9 or abs(td) < 1e-9:
            raise DegenerateRealization("harmonic: coincident with base")
        cross = (tc / (tc - 1.0)) / (td / (td - 1.0))
        return abs(cross + 1.0) <= tol * 10

    if pred == "simcenter":
        e, o1, o2 = pts
        return abs(_cross(_sub(env[o1], env[e]), _sub(env[o2], env[e]))) <= a_tol * scale

    raise ValueError(f"unknown predicate: {pred!r}")


def _scale_of(env: dict[str, Point]) -> float:
    pts = list(env.values())
    if len(pts) < 2:
        return 1.0
    span = max(_norm(_sub(p, q)) for i, p in enumerate(pts) for q in pts[i + 1:])
    return span if span > 1e-9 else 1.0


# --------------------------------------------------------------------------- #
# Matcher helpers over a fact set. A matcher returns a list of
# (new_fact, [premise_facts]) pairs, mirroring the geometry._RULES contract.
# Numeric guards consult an optional ``points`` coordinate map; with points=None
# a non-degeneracy guard passes (generic-position assumption).
# --------------------------------------------------------------------------- #
def _coll_sets(facts):
    return [(set(f[1]), f) for f in facts if f[0] == "collinear"]


def _has(facts, pred, *pts) -> bool:
    return canon(pred, pts) in facts


def _third_on_line(coll, known):
    """Points x s.t. {*known, x} is a stored collinear triple -> (x, fact)."""
    out = []
    for s, f in coll:
        if set(known) <= s:
            rest = s - set(known)
            if len(rest) == 1:
                out.append((next(iter(rest)), f))
    return out


def _rel_partner(facts, head, seg):
    """For a symmetric 2-segment relation (cong/parallel/perpendicular): given
    one segment ``seg`` present in a fact, yield (other_segment, fact)."""
    out = []
    for f in facts:
        if f[0] == head and seg in f[1]:
            other = f[1] - {seg}
            if len(other) == 1:
                out.append((next(iter(other)), f))
    return out


def _ncoll(points, *names) -> bool:
    if points is None:
        return True
    try:
        p = [points[n] for n in names]
    except KeyError:
        return True
    if len(p) == 3:
        return abs(_cross(_sub(p[1], p[0]), _sub(p[2], p[0]))) > 1e-6 * _scale_of(points) ** 2
    return True


def _npara(points, a, b, c, d) -> bool:
    if points is None:
        return True
    try:
        u = _sub(points[b], points[a])
        v = _sub(points[d], points[c])
    except KeyError:
        return True
    if _norm(u) < 1e-9 or _norm(v) < 1e-9:
        return False
    return abs(_cross(u, v)) > 1e-6 * _norm(u) * _norm(v)


def _distinct(*names) -> bool:
    return len(set(names)) == len(names)


# ---- transitivity-style generic matchers (fact[1] is a set of two segkeys) --
def _transitive(facts, head):
    rel = [f for f in facts if f[0] == head]
    out = []
    for i, f1 in enumerate(rel):
        for f2 in rel[i + 1:]:
            shared = f1[1] & f2[1]
            if len(shared) == 1:
                others = (f1[1] - shared) | (f2[1] - shared)
                if len(others) == 2:
                    out.append(((head, frozenset(others)), [f1, f2]))
    return out


# --------------------------------------------------------------------------- #
# CORE rule matchers.
# --------------------------------------------------------------------------- #
def m_midpoint(facts, points=None):
    out = []
    for f in facts:
        if f[0] == "midpoint":
            m, seg = f[1], f[2]
            a, b = tuple(seg)
            out.append((canon("cong", (m, a, m, b)), [f]))
            out.append((canon("collinear", (a, m, b)), [f]))
    return out


def m_perp_perp_parallel(facts, points=None):
    out = []
    perps = [f for f in facts if f[0] == "perpendicular"]
    for i, f1 in enumerate(perps):
        for f2 in perps[i + 1:]:
            common = f1[1] & f2[1]
            if len(common) == 1:
                others = (f1[1] - common) | (f2[1] - common)
                if len(others) == 2:
                    # In the plane two lines perpendicular to a common line are
                    # parallel unconditionally; only guard against a collapsed
                    # (zero-length) line, handled structurally by len==2 above.
                    out.append((("parallel", frozenset(others)), [f1, f2]))
    return out


def m_perp_para_transport(facts, points=None):
    out = []
    perps = [f for f in facts if f[0] == "perpendicular"]
    paras = [f for f in facts if f[0] == "parallel"]
    for pf in perps:
        for qf in paras:
            shared = pf[1] & qf[1]
            if len(shared) == 1:
                po = next(iter(pf[1] - shared))
                qo = next(iter(qf[1] - shared))
                if po != qo:
                    out.append((("perpendicular", frozenset((po, qo))), [pf, qf]))
    return out


def m_para_transitive(facts, points=None):
    return _transitive(facts, "parallel")


def m_cong_transitive(facts, points=None):
    return _transitive(facts, "cong")


def m_perp_bisector_equidist(facts, points=None):
    """v1 r22: midp(M,A,B), perp(O,M,A,B) => cong(O,A,O,B)."""
    out = []
    for mid in [f for f in facts if f[0] == "midpoint"]:
        m = mid[1]
        a, b = tuple(mid[2])
        ab = _seg(a, b)
        for other, pf in _rel_partner(facts, "perpendicular", ab):
            if m in other:
                o = next(iter(other - {m}))
                if _distinct(o, a, b):
                    out.append((canon("cong", (o, a, o, b)), [mid, pf]))
    return out


def m_equidist_perp(facts, points=None):
    """v1 r23: cong(A,P,B,P), cong(A,Q,B,Q) => perp(A,B,P,Q).
    P,Q each equidistant from A and B => both on the perpendicular bisector of
    AB => line PQ perpendicular to AB."""
    out = []
    apex_bases = {}
    for f in [f for f in facts if f[0] == "cong"]:
        s1, s2 = tuple(f[1])
        common = s1 & s2
        if len(common) == 1:
            apex = next(iter(common))
            base = frozenset((next(iter(s1 - {apex})), next(iter(s2 - {apex}))))
            if len(base) == 2:
                apex_bases.setdefault(base, []).append((apex, f))
    for base, lst in apex_bases.items():
        a, b = tuple(base)
        for i in range(len(lst)):
            for j in range(i + 1, len(lst)):
                p, fp = lst[i]
                q, fq = lst[j]
                if _distinct(a, b, p, q):
                    out.append((canon("perpendicular", (a, b, p, q)), [fp, fq]))
    return out


def m_thales_converse(facts, points=None):
    """v1 r19: perp(B,A,B,C), midp(O,A,C) => cong(O,A,O,B)."""
    out = []
    for mid in [f for f in facts if f[0] == "midpoint"]:
        o = mid[1]
        a, c = tuple(mid[2])
        for f in facts:
            if f[0] == "perpendicular":
                segs = list(f[1])
                for sA, sC in ((segs[0], segs[1]), (segs[1], segs[0])):
                    if a in sA and c in sC:
                        bA = sA - {a}
                        bC = sC - {c}
                        if len(bA) == 1 and bA == bC:
                            b = next(iter(bA))
                            if _distinct(o, a, b, c):
                                # O is the midpoint of AC so |OA|=|OC|; the right
                                # angle at B puts B on the circle => |OB| equals
                                # both. Emit both congruences (endpoint-symmetric).
                                out.append((canon("cong", (o, a, o, b)), [f, mid]))
                                out.append((canon("cong", (o, c, o, b)), [f, mid]))
    return out


def m_semicircle(facts, points=None):
    """v1 r20 (angle in a semicircle): cong(O,A,O,B), cong(O,A,O,C),
    coll(O,A,C) => perp(B,A,B,C)  (AC a diameter => angle ABC = 90)."""
    out = []
    for s, cf in _coll_sets(facts):
        for o in s:
            rest = tuple(s - {o})
            if len(rest) != 2:
                continue
            a, c = rest
            # O must be the centre / diameter midpoint: equidistant from A and C.
            if not _has(facts, "cong", o, a, o, c):
                continue
            # A third circle point B: |OB| = |OA| via either radius endpoint.
            cand: dict = {}
            for x, y in ((o, a), (o, c)):
                for other, cf2 in _rel_partner(facts, "cong", _seg(x, y)):
                    if x in other:
                        b = next(iter(other - {x}))
                        cand.setdefault(b, cf2)
            for b, cf2 in cand.items():
                if b in (a, c) or not _distinct(o, a, b, c):
                    continue
                if _ncoll(points, a, c, b):
                    prem = [cf, canon("cong", (o, a, o, c)), cf2]
                    out.append((canon("perpendicular", (b, a, b, c)), prem))
    return out


def m_midsegment(facts, points=None):
    """v1 r7: midp(E,A,B), midp(F,A,C) => para(E,F,B,C)."""
    out = []
    mids = [f for f in facts if f[0] == "midpoint"]
    for f1 in mids:
        for f2 in mids:
            if f1 is f2:
                continue
            e, p1 = f1[1], f1[2]
            fpt, p2 = f2[1], f2[2]
            common = p1 & p2
            if len(common) == 1 and e != fpt:
                a = next(iter(common))
                b = next(iter(p1 - {a}))
                c = next(iter(p2 - {a}))
                if _distinct(e, fpt, b, c):
                    out.append((canon("parallel", (e, fpt, b, c)), [f1, f2]))
    return out


def m_parallelogram_diag(facts, points=None):
    """v1 r26: midp(M,A,B), midp(M,C,D) => para(A,C,B,D) and para(A,D,B,C)."""
    out = []
    mids = [f for f in facts if f[0] == "midpoint"]
    for i, f1 in enumerate(mids):
        for f2 in mids[i + 1:]:
            if f1[1] != f2[1]:
                continue
            a, b = tuple(f1[2])
            c, d = tuple(f2[2])
            if not _distinct(a, b, c, d):
                continue
            out.append((canon("parallel", (a, c, b, d)), [f1, f2]))
            out.append((canon("parallel", (a, d, b, c)), [f1, f2]))
    return out


# --------------------------------------------------------------------------- #
# ANGLE / SIMILARITY rule matchers.
# --------------------------------------------------------------------------- #
_ENUM_MAX = 10  # cap point count for combinatorial-enumeration rules


def _flip(t):
    v, x, y = t
    return (v, y, x)


def m_inscribed_angle(facts, points=None):
    """v1 r4 (directed inscribed angle): concyclic(A,B,C,D) =>
    deqangle(A,C,B,A,D,B)  i.e. directed <(CA,CB) = <(DA,DB)."""
    out = []
    for f in facts:
        if f[0] != "concyclic":
            continue
        S = list(f[1])
        n = len(S)
        for i in range(n):
            for j in range(i + 1, n):
                apex = {S[i], S[j]}
                base = [p for p in S if p not in apex]
                if len(base) != 2:
                    continue
                a, b = base
                c, d = S[i], S[j]
                if _ncoll(points, a, b, c) and _ncoll(points, a, b, d):
                    out.append((canon("deqangle", (a, c, b, a, d, b)), [f]))
    return out


def m_inscribed_converse(facts, points=None):
    """v1 r5 (converse): deqangle with a shared ray-pair and distinct vertices,
    <(CA,CB) = <(DA,DB) and A,B,C,D not collinear => concyclic(A,B,C,D)."""
    out = []
    for f in facts:
        if f[0] != "deqangle":
            continue
        t1, t2 = tuple(f[1])
        v1, x1, y1 = t1
        v2, x2, y2 = t2
        if {x1, y1} == {x2, y2} and v1 != v2 and _distinct(v1, v2, x1, y1):
            a, b = x1, y1
            if _ncoll(points, a, b, v1) and _ncoll(points, a, b, v2):
                out.append((canon("concyclic", (a, b, v1, v2)), [f]))
    return out


def m_deqangle_transitive(facts, points=None):
    """v1 r10 (directed angle chase): <1=<2, <2=<3 => <1=<3."""
    adj: dict = {}
    for f in facts:
        if f[0] != "deqangle":
            continue
        ta, tb = tuple(f[1])
        for (x, y) in ((ta, tb), (_flip(ta), _flip(tb))):
            adj.setdefault(x, []).append((y, f))
            adj.setdefault(y, []).append((x, f))
    out = []
    for t2, nbrs in adj.items():
        for i in range(len(nbrs)):
            for j in range(len(nbrs)):
                if i == j:
                    continue
                t1, f1 = nbrs[i]
                t3, f3 = nbrs[j]
                if f1 is f3 or t1 == t3:
                    continue
                v1, x1, y1 = t1
                v3, x3, y3 = t3
                out.append((canon("deqangle", (x1, v1, y1, x3, v3, y3)), [f1, f3]))
    return out


def _tri_pair(fact):
    t1, t2 = tuple(fact[1])
    return t1, t2


def m_simtri_to_eqratio(facts, points=None):
    out = []
    for f in facts:
        if f[0] != "simtri":
            continue
        (a, b, c), (p, q, r) = _tri_pair(f)
        out.append((canon("eqratio", (a, b, p, q, b, c, q, r)), [f]))
        out.append((canon("eqratio", (b, c, q, r, c, a, r, p)), [f]))
    return out


def m_simtri_to_eqangle(facts, points=None):
    """Similar triangles => equal *unsigned* angles (magnitudes match regardless
    of orientation), emitted as geometry.py's eqangle."""
    out = []
    for f in facts:
        if f[0] != "simtri":
            continue
        (a, b, c), (p, q, r) = _tri_pair(f)
        out.append((canon("eqangle", (b, a, c, q, p, r)), [f]))
        out.append((canon("eqangle", (a, b, c, p, q, r)), [f]))
    return out


def m_contri_to_cong(facts, points=None):
    out = []
    for f in facts:
        if f[0] != "contri":
            continue
        (a, b, c), (p, q, r) = _tri_pair(f)
        out.append((canon("cong", (a, b, p, q)), [f]))
        out.append((canon("cong", (b, c, q, r)), [f]))
        out.append((canon("cong", (c, a, r, p)), [f]))
    return out


def m_contri_to_simtri(facts, points=None):
    out = []
    for f in facts:
        if f[0] == "contri":
            (a, b, c), (p, q, r) = _tri_pair(f)
            out.append((canon("simtri", (a, b, c, p, q, r)), [f]))
    return out


def m_sss_similar(facts, points=None):
    """v1 r39 (SSS~): three proportional side-ratios => similar."""
    pts = points_in(facts)
    if len(pts) > _ENUM_MAX:
        return []
    out = []
    for a, b, c, p, q, r in permutations(sorted(pts), 6):
        if _has(facts, "eqratio", a, b, p, q, b, c, q, r) and \
           _has(facts, "eqratio", b, c, q, r, c, a, r, p) and \
           _ncoll(points, a, b, c) and _ncoll(points, p, q, r):
            out.append((canon("simtri", (a, b, c, p, q, r)),
                        [canon("eqratio", (a, b, p, q, b, c, q, r)),
                         canon("eqratio", (b, c, q, r, c, a, r, p))]))
    return out


def m_aa_similar(facts, points=None):
    """v1 r35 (AA~): two pairs of equal directed angles => similar."""
    pts = points_in(facts)
    if len(pts) > _ENUM_MAX:
        return []
    out = []
    for a, b, c, p, q, r in permutations(sorted(pts), 6):
        if _has(facts, "deqangle", b, a, c, q, p, r) and \
           _has(facts, "deqangle", a, b, c, p, q, r) and \
           _ncoll(points, a, b, c) and _ncoll(points, p, q, r):
            out.append((canon("simtri", (a, b, c, p, q, r)),
                        [canon("deqangle", (b, a, c, q, p, r)),
                         canon("deqangle", (a, b, c, p, q, r))]))
    return out


def m_sss_congruent(facts, points=None):
    """v1 r32 (SSS): three equal sides => congruent triangles."""
    pts = points_in(facts)
    if len(pts) > _ENUM_MAX:
        return []
    out = []
    for a, b, c, p, q, r in permutations(sorted(pts), 6):
        if _has(facts, "cong", a, b, p, q) and _has(facts, "cong", b, c, q, r) and \
           _has(facts, "cong", c, a, r, p) and _ncoll(points, a, b, c):
            out.append((canon("contri", (a, b, c, p, q, r)),
                        [canon("cong", (a, b, p, q)),
                         canon("cong", (b, c, q, r)),
                         canon("cong", (c, a, r, p))]))
    return out


# --------------------------------------------------------------------------- #
# NEWCLID (r44-r51) rule matchers.
# --------------------------------------------------------------------------- #
def m_third_altitude(facts, points=None):
    """newclid r44: perp(A,B,C,D), perp(A,C,B,D) => perp(A,D,B,C)."""
    perps = [f for f in facts if f[0] == "perpendicular"]
    out = []
    for i, f1 in enumerate(perps):
        for f2 in perps[i + 1:]:
            pts = set()
            for f in (f1, f2):
                for s in f[1]:
                    pts |= set(s)
            if len(pts) != 4:
                continue
            for a, b, c, d in permutations(pts, 4):
                if canon("perpendicular", (a, b, c, d)) == f1 and \
                   canon("perpendicular", (a, c, b, d)) == f2:
                    out.append((canon("perpendicular", (a, d, b, c)), [f1, f2]))
    return out


def m_pappus(facts, points=None):
    """newclid r45 (Pappus): a,b,c collinear; p,q,r collinear; the three cross
    intersections x=aq^pb, y=ar^pc, z=br^cq are collinear."""
    coll = _coll_sets(facts)
    triples = [(s, f) for s, f in coll if len(s) == 3]
    out = []
    for s1, f1 in triples:
        for s2, f2 in triples:
            if s1 == s2:
                continue
            for a, b, c in permutations(s1, 3):
                for p, q, r in permutations(s2, 3):
                    if not _distinct(a, b, c, p, q, r):
                        continue
                    xs = {x for x, _ in _third_on_line(coll, (a, q))} & \
                         {x for x, _ in _third_on_line(coll, (p, b))}
                    ys = {y for y, _ in _third_on_line(coll, (a, r))} & \
                         {y for y, _ in _third_on_line(coll, (p, c))}
                    zs = {z for z, _ in _third_on_line(coll, (b, r))} & \
                         {z for z, _ in _third_on_line(coll, (c, q))}
                    for x in xs:
                        for y in ys:
                            for z in zs:
                                if _distinct(x, y, z):
                                    prem = [f1, f2,
                                            canon("collinear", (a, q, x)),
                                            canon("collinear", (p, b, x)),
                                            canon("collinear", (a, r, y)),
                                            canon("collinear", (p, c, y)),
                                            canon("collinear", (b, r, z)),
                                            canon("collinear", (c, q, z))]
                                    out.append((canon("collinear", (x, y, z)), prem))
    return out


def m_simson(facts, points=None):
    """newclid r46 (Simson line): P on circumcircle of ABC; feet of the
    perpendiculars from P to the three sides are collinear."""
    coll = _coll_sets(facts)
    out = []
    for f in facts:
        if f[0] != "concyclic":
            continue
        S = list(f[1])
        for pi in range(len(S)):
            p = S[pi]
            tri = [x for x in S if x != p]
            if len(tri) != 3:
                continue
            a, b, c = tri
            sides = [("L", a, c), ("M", b, c), ("N", a, b)]
            feet = {}
            prem_ok = True
            side_prem = []
            for tag, u, v in sides:
                found = None
                for foot, cf in _third_on_line(coll, (u, v)):
                    if _has(facts, "perpendicular", p, foot, u, v):
                        found = foot
                        side_prem += [cf, canon("perpendicular", (p, foot, u, v))]
                        break
                if found is None:
                    prem_ok = False
                    break
                feet[tag] = found
            if prem_ok:
                L, M, N = feet["L"], feet["M"], feet["N"]
                if _distinct(L, M, N):
                    out.append((canon("collinear", (L, M, N)), [f] + side_prem))
    return out


def m_incenter(facts, points=None):
    """newclid r47 (angle-bisector concurrency / incenter): if AX bisects
    angle A and BX bisects angle B then CX bisects angle C."""
    pts = points_in(facts)
    if len(pts) > _ENUM_MAX:
        return []
    out = []
    for a, b, c, x in permutations(sorted(pts), 4):
        if _has(facts, "deqangle", b, a, x, x, a, c) and \
           _has(facts, "deqangle", a, b, x, x, b, c) and \
           _ncoll(points, a, b, c):
            out.append((canon("deqangle", (b, c, x, x, c, a)),
                        [canon("deqangle", (b, a, x, x, a, c)),
                         canon("deqangle", (a, b, x, x, b, c))]))
    return out


def _midpoint_triangles(facts):
    """Yield (A,B,C, midAB, midBC, midCA, [three midpoint facts]) for every
    triangle whose three side-midpoints are all present."""
    mids = {}
    for f in facts:
        if f[0] == "midpoint":
            mids[f[2]] = (f[1], f)  # frozenset{A,B} -> (M, fact)
    segs = list(mids)
    n = len(segs)
    for i in range(n):
        for j in range(i + 1, n):
            for k in range(j + 1, n):
                pset = set()
                for s in (segs[i], segs[j], segs[k]):
                    pset |= set(s)
                if len(pset) != 3:
                    continue
                a, b, c = tuple(pset)
                need = {_seg(a, b), _seg(b, c), _seg(c, a)}
                if need != {segs[i], segs[j], segs[k]}:
                    continue
                yield (a, b, c,
                       mids[_seg(a, b)], mids[_seg(b, c)], mids[_seg(c, a)])


def m_perp_bisector_concurrency(facts, points=None):
    """newclid r48 (circumcentre): X on perp-bisectors of AB and BC => X on
    perp-bisector of CA."""
    out = []
    for a, b, c, mab, mbc, mca in _midpoint_triangles(facts):
        info = {_seg(a, b): mab, _seg(b, c): mbc, _seg(c, a): mca}
        keys = list(info)
        for ci in range(3):  # which side is the *derived* perpendicular bisector
            s3, s1, s2 = keys[ci], keys[(ci + 1) % 3], keys[(ci + 2) % 3]
            (M1, fM1), (M2, fM2), (M3, fM3) = info[s1], info[s2], info[s3]
            for xo, fp1 in _rel_partner(facts, "perpendicular", s1):
                if M1 not in xo:
                    continue
                X = next(iter(xo - {M1}))
                e2a, e2b = tuple(s2)
                e3a, e3b = tuple(s3)
                if _has(facts, "perpendicular", X, M2, e2a, e2b) and \
                        _distinct(X, M3, e3a, e3b):
                    out.append((canon("perpendicular", (X, M3, e3a, e3b)),
                                [fM1, fp1, fM2,
                                 canon("perpendicular", (X, M2, e2a, e2b)), fM3]))
    return out


def m_median_concurrency(facts, points=None):
    """newclid r49 (centroid): X on medians C-M(AB) and A-N(BC) => X on median
    B-P(CA)."""
    coll = _coll_sets(facts)
    out = []
    for a, b, c, mab, mbc, mca in _midpoint_triangles(facts):
        # median from a vertex hits the midpoint of the opposite side.
        med = {c: mab, a: mbc, b: mca}  # vertex -> (midpt, midpt-fact)
        verts = [a, b, c]
        for ci in range(3):  # which median is the *derived* one
            vc, v1, v2 = verts[ci], verts[(ci + 1) % 3], verts[(ci + 2) % 3]
            (Mc, fc), (M1, f1), (M2, f2) = med[vc], med[v1], med[v2]
            xs = {x for x, _ in _third_on_line(coll, (v1, M1))} & \
                 {x for x, _ in _third_on_line(coll, (v2, M2))}
            for X in xs:
                if _distinct(X, vc, Mc):
                    out.append((canon("collinear", (vc, Mc, X)),
                                [f1, canon("collinear", (v1, M1, X)),
                                 f2, canon("collinear", (v2, M2, X)), fc]))
    return out


def m_concyclic_center(facts, points=None):
    """newclid r50: O centre of circle ABC and A,B,C,D concyclic => cong(O,A,O,D)."""
    pts = points_in(facts)
    out = []
    for f in facts:
        if f[0] != "concyclic":
            continue
        S = list(f[1])
        for o in pts:
            if o in S:
                continue
            for ai in range(len(S)):
                a = S[ai]
                eq = [m for m in S if m != a and _has(facts, "cong", o, a, o, m)]
                if len(eq) >= 2:
                    for miss in [m for m in S if m != a and m not in eq]:
                        out.append((canon("cong", (o, a, o, miss)),
                                    [f] + [canon("cong", (o, a, o, m)) for m in eq]))
    return out


def m_circumcenter_unique(facts, points=None):
    """newclid r51: A,B,C,D concyclic, O equidistant from {A,B} and from {C,D},
    AB not parallel CD => O equidistant from all (cong O A O C)."""
    pts = points_in(facts)
    out = []
    for f in facts:
        if f[0] != "concyclic":
            continue
        S = list(f[1])
        for o in pts:
            if o in S:
                continue
            pairs = [(x, y) for i, x in enumerate(S) for y in S[i + 1:]
                     if _has(facts, "cong", o, x, o, y)]
            for i, (a, b) in enumerate(pairs):
                for (c, d) in pairs[i + 1:]:
                    if _distinct(a, b, c, d) and _npara(points, a, b, c, d):
                        prem = [f, canon("cong", (o, a, o, b)),
                                canon("cong", (o, c, o, d))]
                        # O is now the circumcentre: equidistant from all four.
                        # Emit every cross-pair congruence (endpoint-symmetric).
                        for x in (a, b):
                            for z in (c, d):
                                out.append((canon("cong", (o, x, o, z)), prem))
    return out


# --------------------------------------------------------------------------- #
# ADVANCED rule matchers (clean-room re-implementations from the mathematical
# statements; no GPLv3 source consulted for code).
# --------------------------------------------------------------------------- #
def _equidist_pairs(facts):
    """base {P,Q} -> list of (centre O, cong-fact) with |OP| = |OQ|."""
    m: dict = {}
    for f in facts:
        if f[0] != "cong":
            continue
        s1, s2 = tuple(f[1])
        common = s1 & s2
        if len(common) == 1:
            o = next(iter(common))
            base = frozenset((next(iter(s1 - {o})), next(iter(s2 - {o}))))
            if len(base) == 2:
                m.setdefault(base, []).append((o, f))
    return m


def m_radax_intro(facts, points=None):
    """clean-room (radical axis): two circles centred O1,O2 both through P and Q
    => line PQ is their radical axis."""
    out = []
    for base, lst in _equidist_pairs(facts).items():
        p, q = tuple(base)
        for i in range(len(lst)):
            for j in range(i + 1, len(lst)):
                o1, f1 = lst[i]
                o2, f2 = lst[j]
                if _distinct(p, q, o1, o2):
                    out.append((canon("radax", (p, q, o1, o2)), [f1, f2]))
    return out


def m_radical_center(facts, points=None):
    """clean-room (radical centre): the three pairwise radical axes of three
    circles are concurrent."""
    rad = [f for f in facts if f[0] == "radax"]
    out = []
    n = len(rad)
    for i in range(n):
        for j in range(i + 1, n):
            for k in range(j + 1, n):
                fs = [rad[i], rad[j], rad[k]]
                centres = set()
                for f in fs:
                    centres |= set(f[2])
                if len(centres) != 3:
                    continue
                pairs = {f[2] for f in fs}
                a, b, c = tuple(centres)
                if pairs != {_seg(a, b), _seg(b, c), _seg(c, a)}:
                    continue
                chords = [tuple(f[1]) for f in fs]
                pts = [p for ch in chords for p in ch]
                if len(set(pts)) == 6:
                    (p1, q1), (p2, q2), (p3, q3) = chords
                    out.append((canon("concurrent",
                                       (p1, q1, p2, q2, p3, q3)), fs))
    return out


def m_monge(facts, points=None):
    """clean-room (Monge): the three external similitude centres of three
    circles are collinear."""
    sc = [f for f in facts if f[0] == "simcenter"]
    out = []
    n = len(sc)
    for i in range(n):
        for j in range(i + 1, n):
            for k in range(j + 1, n):
                fs = [sc[i], sc[j], sc[k]]
                centres = set()
                for f in fs:
                    centres |= set(f[2])
                if len(centres) != 3:
                    continue
                if {f[2] for f in fs} != {_seg(*p) for p in
                                          ((tuple(centres)[0], tuple(centres)[1]),
                                           (tuple(centres)[1], tuple(centres)[2]),
                                           (tuple(centres)[0], tuple(centres)[2]))}:
                    continue
                e, ff, g = fs[0][1], fs[1][1], fs[2][1]
                if _distinct(e, ff, g):
                    out.append((canon("collinear", (e, ff, g)), fs))
    return out


def _perp_apex(facts, u, v):
    """P such that perp(P,u,P,v): segments {P,u},{P,v} share apex P."""
    res = []
    for f in facts:
        if f[0] != "perpendicular":
            continue
        s1, s2 = tuple(f[1])
        common = s1 & s2
        if len(common) == 1:
            P = next(iter(common))
            o1 = next(iter(s1 - {P}))
            o2 = next(iter(s2 - {P}))
            if {o1, o2} == {u, v} and P not in (u, v):
                res.append((P, f))
    return res


def m_harmonic_bisector(facts, points=None):
    """clean-room (harmonic pencil): if (A,B;C,D) = -1 and PC _|_ PD, then PC
    bisects angle APB (directed): <(PA,PC) = <(PC,PB)."""
    out = []
    for f in facts:
        if f[0] != "harmonic":
            continue
        pr = list(f[1])
        for base, conj in ((pr[0], pr[1]), (pr[1], pr[0])):
            a, b = tuple(base)
            c, d = tuple(conj)
            for P, pf in _perp_apex(facts, c, d):
                if _distinct(a, b, c, d, P):
                    # PC and PD are the internal/external bisectors of angle APB;
                    # both satisfy the directed bisector relation (mod pi).
                    out.append((canon("deqangle", (a, P, c, c, P, b)), [f, pf]))
                    out.append((canon("deqangle", (a, P, d, d, P, b)), [f, pf]))
    return out


def m_desargues(facts, points=None):
    """clean-room (Desargues): two triangles perspective from a point O (AA',
    BB', CC' concurrent at O) are perspective from a line -- the three
    corresponding-side intersections are collinear."""
    coll = _coll_sets(facts)
    pts = points_in(facts)
    if len(pts) > _ENUM_MAX + 3:
        return []
    out = []
    # collinear triples through a common vertex O: pairs (V, V') with coll(O,V,V')
    by_center: dict = {}
    for s, f in coll:
        for o in s:
            rest = tuple(sorted(s - {o}))
            by_center.setdefault(o, []).append((rest, f))
    for o, lines in by_center.items():
        m = len(lines)
        for i in range(m):
            for j in range(i + 1, m):
                for k in range(m):
                    if k in (i, j):
                        continue
                    (aa, faa), (bb, fbb), (cc, fcc) = lines[i], lines[j], lines[k]
                    for a, ap in (aa, aa[::-1]):
                        for b, bp in (bb, bb[::-1]):
                            for c, cp in (cc, cc[::-1]):
                                verts = {a, ap, b, bp, c, cp, o}
                                if len(verts) != 7:
                                    continue
                                xs = {x for x, _ in _third_on_line(coll, (b, c))} & \
                                     {x for x, _ in _third_on_line(coll, (bp, cp))}
                                ys = {y for y, _ in _third_on_line(coll, (c, a))} & \
                                     {y for y, _ in _third_on_line(coll, (cp, ap))}
                                zs = {z for z, _ in _third_on_line(coll, (a, b))} & \
                                     {z for z, _ in _third_on_line(coll, (ap, bp))}
                                for X in xs:
                                    for Y in ys:
                                        for Z in zs:
                                            if _distinct(X, Y, Z):
                                                prem = [
                                                    canon("collinear", (o, a, ap)),
                                                    canon("collinear", (o, b, bp)),
                                                    canon("collinear", (o, c, cp)),
                                                    canon("collinear", (b, c, X)),
                                                    canon("collinear", (bp, cp, X)),
                                                    canon("collinear", (c, a, Y)),
                                                    canon("collinear", (cp, ap, Y)),
                                                    canon("collinear", (a, b, Z)),
                                                    canon("collinear", (ap, bp, Z)),
                                                ]
                                                out.append(
                                                    (canon("collinear", (X, Y, Z)),
                                                     prem))
    return out


# --------------------------------------------------------------------------- #
# Rule catalog. Each rule bundles its matcher with a seeded numeric witness so
# the whole set is soundness-tested. ``source`` records lineage; ``kind`` marks
# core (v1 43-rule set + Newclid r44-r51) vs advanced (clean-room Tong/AG math).
# --------------------------------------------------------------------------- #
@dataclass
class Rule:
    name: str
    source: str          # 'v1' | 'newclid' | 'clean-room-tong'
    kind: str            # 'core' | 'advanced'
    category: str
    match: Callable
    witness: dict


def _F(op, point=None, **kw):
    d = {"op": op}
    if point is not None:
        d["point"] = point
    d.update(kw)
    return d


# ---- witness construction fragments -------------------------------------- #
_SIM_TRI = [
    _F("free", "A"), _F("free", "B"), _F("free", "C"),
    _F("free", "P"), _F("free", "Q"),
    _F("sim_image", "R", tri=["A", "B", "C"], base=["P", "Q"]),
]
_ISO_TRI = [
    _F("free", "A"), _F("free", "B"), _F("free", "C"),
    _F("free", "P"), _F("seg_len", "Q", **{"from": "P", "len_ref": ["A", "B"]}),
    _F("iso_image", "R", tri=["A", "B", "C"], base=["P", "Q"]),
]
_CIRCLE4 = [
    _F("free", "O"), _F("free", "A"),
    _F("on_circle", "B", center="O", through="A"),
    _F("on_circle", "C", center="O", through="A"),
    _F("on_circle", "D", center="O", through="A"),
]


def _circles(n):
    steps = []
    for i in range(1, n + 1):
        steps.append(_F("free", f"O{i}", spread=2.5))
        steps.append(_F("free", f"R{i}", spread=11.0))
    return steps


RULES: list[Rule] = [
    # ------------------------------- CORE (v1) ------------------------------ #
    Rule("midpoint-expand", "v1", "core", "midpoint", m_midpoint, {
        "construction": [_F("free", "A"), _F("free", "B"),
                         _F("midpoint", "M", of=["A", "B"])],
        "premises": [("midpoint", ("M", "A", "B"))],
        "conclusion": ("cong", ("M", "A", "M", "B"))}),
    Rule("two-perpendiculars-parallel", "v1", "core", "parallel",
         m_perp_perp_parallel, {
        "construction": [_F("free", "A"), _F("free", "B"), _F("free", "C"),
                         _F("perp_from", "D", at="C", line=["A", "B"]),
                         _F("free", "E"),
                         _F("perp_from", "F", at="E", line=["C", "D"])],
        "premises": [("perpendicular", ("A", "B", "C", "D")),
                     ("perpendicular", ("C", "D", "E", "F"))],
        "conclusion": ("parallel", ("A", "B", "E", "F"))}),
    Rule("perpendicular-transport", "v1", "core", "perpendicular",
         m_perp_para_transport, {
        "construction": [_F("free", "A"), _F("free", "B"), _F("free", "C"),
                         _F("perp_from", "D", at="C", line=["A", "B"]),
                         _F("free", "E"),
                         _F("para_from", "F", at="E", dir=["C", "D"])],
        "premises": [("perpendicular", ("A", "B", "C", "D")),
                     ("parallel", ("C", "D", "E", "F"))],
        "conclusion": ("perpendicular", ("A", "B", "E", "F"))}),
    Rule("parallel-transitive", "v1", "core", "parallel", m_para_transitive, {
        "construction": [_F("free", "A"), _F("free", "B"), _F("free", "C"),
                         _F("para_from", "D", at="C", dir=["A", "B"]),
                         _F("free", "E"),
                         _F("para_from", "F", at="E", dir=["C", "D"])],
        "premises": [("parallel", ("A", "B", "C", "D")),
                     ("parallel", ("C", "D", "E", "F"))],
        "conclusion": ("parallel", ("A", "B", "E", "F"))}),
    Rule("congruence-transitive", "v1", "core", "cong", m_cong_transitive, {
        "construction": [_F("free", "A"), _F("free", "B"), _F("free", "C"),
                         _F("seg_len", "D", **{"from": "C", "len_ref": ["A", "B"]}),
                         _F("free", "E"),
                         _F("seg_len", "F", **{"from": "E", "len_ref": ["C", "D"]})],
        "premises": [("cong", ("A", "B", "C", "D")),
                     ("cong", ("C", "D", "E", "F"))],
        "conclusion": ("cong", ("A", "B", "E", "F"))}),
    Rule("perp-bisector-equidistant", "v1", "core", "cong",
         m_perp_bisector_equidist, {
        "construction": [_F("free", "A"), _F("free", "B"),
                         _F("midpoint", "M", of=["A", "B"]),
                         _F("perp_from", "O", at="M", line=["A", "B"])],
        "premises": [("midpoint", ("M", "A", "B")),
                     ("perpendicular", ("O", "M", "A", "B"))],
        "conclusion": ("cong", ("O", "A", "O", "B"))}),
    Rule("equidistant-implies-perpendicular", "v1", "core", "perpendicular",
         m_equidist_perp, {
        "construction": [_F("free", "A"), _F("free", "B"),
                         _F("midpoint", "M", of=["A", "B"]),
                         _F("perp_from", "P", at="M", line=["A", "B"]),
                         _F("perp_from", "Q", at="M", line=["A", "B"])],
        "premises": [("cong", ("A", "P", "B", "P")),
                     ("cong", ("A", "Q", "B", "Q"))],
        "conclusion": ("perpendicular", ("A", "B", "P", "Q"))}),
    Rule("thales-converse", "v1", "core", "cong", m_thales_converse, {
        "construction": [_F("free", "A"), _F("free", "C"),
                         _F("midpoint", "O", of=["A", "C"]),
                         _F("on_circle", "B", center="O", through="A")],
        "premises": [("perpendicular", ("B", "A", "B", "C")),
                     ("midpoint", ("O", "A", "C"))],
        "conclusion": ("cong", ("O", "A", "O", "B"))}),
    Rule("angle-in-semicircle", "v1", "core", "perpendicular", m_semicircle, {
        "construction": [_F("free", "O"), _F("free", "A"),
                         _F("reflect_point", "C", of="A", center="O"),
                         _F("on_circle", "B", center="O", through="A")],
        "premises": [("cong", ("O", "A", "O", "B")),
                     ("cong", ("O", "A", "O", "C")),
                     ("collinear", ("O", "A", "C"))],
        "conclusion": ("perpendicular", ("B", "A", "B", "C"))}),
    Rule("midsegment-parallel", "v1", "core", "parallel", m_midsegment, {
        "construction": [_F("free", "A"), _F("free", "B"), _F("free", "C"),
                         _F("midpoint", "E", of=["A", "B"]),
                         _F("midpoint", "G", of=["A", "C"])],
        "premises": [("midpoint", ("E", "A", "B")),
                     ("midpoint", ("G", "A", "C"))],
        "conclusion": ("parallel", ("E", "G", "B", "C"))}),
    Rule("parallelogram-diagonals", "v1", "core", "parallel",
         m_parallelogram_diag, {
        "construction": [_F("free", "M"), _F("free", "A"),
                         _F("reflect_point", "B", of="A", center="M"),
                         _F("free", "C"),
                         _F("reflect_point", "D", of="C", center="M")],
        "premises": [("midpoint", ("M", "A", "B")),
                     ("midpoint", ("M", "C", "D"))],
        "conclusion": ("parallel", ("A", "C", "B", "D"))}),
    Rule("inscribed-angle", "v1", "core", "angle", m_inscribed_angle, {
        "construction": _CIRCLE4,
        "premises": [("concyclic", ("A", "B", "C", "D"))],
        "conclusion": ("deqangle", ("A", "C", "B", "A", "D", "B"))}),
    Rule("inscribed-angle-converse", "v1", "core", "angle",
         m_inscribed_converse, {
        "construction": _CIRCLE4,
        "premises": [("deqangle", ("A", "C", "B", "A", "D", "B"))],
        "conclusion": ("concyclic", ("A", "B", "C", "D"))}),
    Rule("directed-angle-transitive", "v1", "core", "angle",
         m_deqangle_transitive, {
        "construction": [_F("rand_angle", "TH"),
                         _F("free", "V1"), _F("free", "R1"),
                         _F("ray_at_angle", "B1", vertex="V1", ray="R1",
                            angle_ref="TH"),
                         _F("free", "V2"), _F("free", "R2"),
                         _F("ray_at_angle", "B2", vertex="V2", ray="R2",
                            angle_ref="TH"),
                         _F("free", "V3"), _F("free", "R3"),
                         _F("ray_at_angle", "B3", vertex="V3", ray="R3",
                            angle_ref="TH")],
        "premises": [("deqangle", ("R1", "V1", "B1", "R2", "V2", "B2")),
                     ("deqangle", ("R2", "V2", "B2", "R3", "V3", "B3"))],
        "conclusion": ("deqangle", ("R1", "V1", "B1", "R3", "V3", "B3"))}),
    Rule("similar-implies-eqratio", "v1", "core", "similar",
         m_simtri_to_eqratio, {
        "construction": _SIM_TRI,
        "premises": [("simtri", ("A", "B", "C", "P", "Q", "R"))],
        "conclusion": ("eqratio", ("A", "B", "P", "Q", "B", "C", "Q", "R"))}),
    Rule("similar-implies-eqangle", "v1", "core", "similar",
         m_simtri_to_eqangle, {
        "construction": _SIM_TRI,
        "premises": [("simtri", ("A", "B", "C", "P", "Q", "R"))],
        "conclusion": ("eqangle", ("B", "A", "C", "Q", "P", "R"))}),
    Rule("congruent-implies-cong", "v1", "core", "congruent",
         m_contri_to_cong, {
        "construction": _ISO_TRI,
        "premises": [("contri", ("A", "B", "C", "P", "Q", "R"))],
        "conclusion": ("cong", ("A", "B", "P", "Q"))}),
    Rule("congruent-implies-similar", "v1", "core", "congruent",
         m_contri_to_simtri, {
        "construction": _ISO_TRI,
        "premises": [("contri", ("A", "B", "C", "P", "Q", "R"))],
        "conclusion": ("simtri", ("A", "B", "C", "P", "Q", "R"))}),
    Rule("sss-similar", "v1", "core", "similar", m_sss_similar, {
        "construction": _SIM_TRI,
        "premises": [("eqratio", ("A", "B", "P", "Q", "B", "C", "Q", "R")),
                     ("eqratio", ("B", "C", "Q", "R", "C", "A", "R", "P"))],
        "conclusion": ("simtri", ("A", "B", "C", "P", "Q", "R"))}),
    Rule("aa-similar", "v1", "core", "similar", m_aa_similar, {
        "construction": _SIM_TRI,
        "premises": [("deqangle", ("B", "A", "C", "Q", "P", "R")),
                     ("deqangle", ("A", "B", "C", "P", "Q", "R"))],
        "conclusion": ("simtri", ("A", "B", "C", "P", "Q", "R"))}),
    Rule("sss-congruent", "v1", "core", "congruent", m_sss_congruent, {
        "construction": _ISO_TRI,
        "premises": [("cong", ("A", "B", "P", "Q")),
                     ("cong", ("B", "C", "Q", "R")),
                     ("cong", ("C", "A", "R", "P"))],
        "conclusion": ("contri", ("A", "B", "C", "P", "Q", "R"))}),

    # ----------------------------- CORE (Newclid) --------------------------- #
    Rule("third-altitude", "newclid", "core", "perpendicular",
         m_third_altitude, {
        "construction": [_F("free", "A"), _F("free", "B"), _F("free", "C"),
                         _F("perp_from", "U", at="C", line=["A", "B"]),
                         _F("perp_from", "V", at="B", line=["A", "C"]),
                         _F("intersection", "D", line1=["C", "U"],
                            line2=["B", "V"])],
        "premises": [("perpendicular", ("A", "B", "C", "D")),
                     ("perpendicular", ("A", "C", "B", "D"))],
        "conclusion": ("perpendicular", ("A", "D", "B", "C"))}),
    Rule("pappus", "newclid", "core", "collinear", m_pappus, {
        "construction": [_F("free", "A"), _F("free", "B"),
                         _F("on_line", "C", of=["A", "B"]),
                         _F("free", "P"), _F("free", "Q"),
                         _F("on_line", "R", of=["P", "Q"]),
                         _F("intersection", "X", line1=["A", "Q"], line2=["P", "B"]),
                         _F("intersection", "Y", line1=["A", "R"], line2=["P", "C"]),
                         _F("intersection", "Z", line1=["B", "R"], line2=["C", "Q"])],
        "premises": [("collinear", ("A", "B", "C")), ("collinear", ("P", "Q", "R")),
                     ("collinear", ("A", "Q", "X")), ("collinear", ("P", "B", "X")),
                     ("collinear", ("A", "R", "Y")), ("collinear", ("P", "C", "Y")),
                     ("collinear", ("B", "R", "Z")), ("collinear", ("C", "Q", "Z"))],
        "conclusion": ("collinear", ("X", "Y", "Z"))}),
    Rule("simson-line", "newclid", "core", "collinear", m_simson, {
        "construction": [_F("free", "A"), _F("free", "B"), _F("free", "C"),
                         _F("circumcenter", "O", of=["A", "B", "C"]),
                         _F("on_circle", "P", center="O", through="A"),
                         _F("foot", "L", **{"from": "P", "of": ["A", "C"]}),
                         _F("foot", "M", **{"from": "P", "of": ["B", "C"]}),
                         _F("foot", "N", **{"from": "P", "of": ["A", "B"]})],
        "premises": [("concyclic", ("A", "B", "C", "P")),
                     ("collinear", ("A", "L", "C")),
                     ("perpendicular", ("P", "L", "A", "C")),
                     ("collinear", ("B", "M", "C")),
                     ("perpendicular", ("P", "M", "B", "C")),
                     ("collinear", ("A", "N", "B")),
                     ("perpendicular", ("P", "N", "A", "B"))],
        "conclusion": ("collinear", ("L", "M", "N"))}),
    Rule("incenter-bisector-concurrency", "newclid", "core", "angle",
         m_incenter, {
        "construction": [_F("free", "A"), _F("free", "B"), _F("free", "C"),
                         _F("incenter", "X", of=["A", "B", "C"])],
        "premises": [("deqangle", ("B", "A", "X", "X", "A", "C")),
                     ("deqangle", ("A", "B", "X", "X", "B", "C"))],
        "conclusion": ("deqangle", ("B", "C", "X", "X", "C", "A"))}),
    Rule("perp-bisector-concurrency", "newclid", "core", "perpendicular",
         m_perp_bisector_concurrency, {
        "construction": [_F("free", "A"), _F("free", "B"), _F("free", "C"),
                         _F("midpoint", "M", of=["A", "B"]),
                         _F("midpoint", "N", of=["B", "C"]),
                         _F("midpoint", "P", of=["C", "A"]),
                         _F("circumcenter", "X", of=["A", "B", "C"])],
        "premises": [("midpoint", ("M", "A", "B")),
                     ("perpendicular", ("X", "M", "A", "B")),
                     ("midpoint", ("N", "B", "C")),
                     ("perpendicular", ("X", "N", "B", "C")),
                     ("midpoint", ("P", "C", "A"))],
        "conclusion": ("perpendicular", ("X", "P", "C", "A"))}),
    Rule("median-concurrency", "newclid", "core", "collinear",
         m_median_concurrency, {
        "construction": [_F("free", "A"), _F("free", "B"), _F("free", "C"),
                         _F("midpoint", "M", of=["A", "B"]),
                         _F("midpoint", "N", of=["B", "C"]),
                         _F("midpoint", "P", of=["C", "A"]),
                         _F("intersection", "X", line1=["C", "M"],
                            line2=["A", "N"])],
        "premises": [("midpoint", ("M", "A", "B")), ("collinear", ("C", "M", "X")),
                     ("midpoint", ("N", "B", "C")), ("collinear", ("A", "N", "X")),
                     ("midpoint", ("P", "C", "A"))],
        "conclusion": ("collinear", ("B", "P", "X"))}),
    Rule("concyclic-center-equidistant", "newclid", "core", "cong",
         m_concyclic_center, {
        "construction": _CIRCLE4,
        "premises": [("cong", ("O", "A", "O", "B")), ("cong", ("O", "A", "O", "C")),
                     ("concyclic", ("A", "B", "C", "D"))],
        "conclusion": ("cong", ("O", "A", "O", "D"))}),
    Rule("circumcenter-unique", "newclid", "core", "cong",
         m_circumcenter_unique, {
        "construction": _CIRCLE4,
        "premises": [("concyclic", ("A", "B", "C", "D")),
                     ("cong", ("O", "A", "O", "B")), ("cong", ("O", "C", "O", "D"))],
        "conclusion": ("cong", ("O", "A", "O", "C"))}),

    # ------------------------------- ADVANCED ------------------------------- #
    Rule("radical-axis", "clean-room-tong", "advanced", "radical-axis",
         m_radax_intro, {
        "construction": _circles(2) + [
            _F("cc", "P", c1=["O1", "R1"], c2=["O2", "R2"], which=0),
            _F("cc", "Q", c1=["O1", "R1"], c2=["O2", "R2"], which=1)],
        "premises": [("cong", ("O1", "P", "O1", "Q")),
                     ("cong", ("O2", "P", "O2", "Q"))],
        "conclusion": ("radax", ("P", "Q", "O1", "O2"))}),
    Rule("radical-center", "clean-room-tong", "advanced", "radical-axis",
         m_radical_center, {
        "construction": _circles(3) + [
            _F("cc", "P", c1=["O1", "R1"], c2=["O2", "R2"], which=0),
            _F("cc", "Q", c1=["O1", "R1"], c2=["O2", "R2"], which=1),
            _F("cc", "S", c1=["O1", "R1"], c2=["O3", "R3"], which=0),
            _F("cc", "T", c1=["O1", "R1"], c2=["O3", "R3"], which=1),
            _F("cc", "U", c1=["O2", "R2"], c2=["O3", "R3"], which=0),
            _F("cc", "V", c1=["O2", "R2"], c2=["O3", "R3"], which=1)],
        "premises": [("radax", ("P", "Q", "O1", "O2")),
                     ("radax", ("S", "T", "O1", "O3")),
                     ("radax", ("U", "V", "O2", "O3"))],
        "conclusion": ("concurrent", ("P", "Q", "S", "T", "U", "V"))}),
    Rule("monge", "clean-room-tong", "advanced", "similitude", m_monge, {
        "construction": _circles(3) + [
            _F("extsim", "E", c1=["O1", "R1"], c2=["O2", "R2"]),
            _F("extsim", "G", c1=["O1", "R1"], c2=["O3", "R3"]),
            _F("extsim", "H", c1=["O2", "R2"], c2=["O3", "R3"])],
        "premises": [("simcenter", ("E", "O1", "O2")),
                     ("simcenter", ("G", "O1", "O3")),
                     ("simcenter", ("H", "O2", "O3"))],
        "conclusion": ("collinear", ("E", "G", "H"))}),
    Rule("harmonic-right-angle-bisector", "clean-room-tong", "advanced",
         "harmonic", m_harmonic_bisector, {
        "construction": [_F("free", "A"), _F("free", "B"),
                         _F("on_segment", "C", of=["A", "B"]),
                         _F("harm", "D", base=["A", "B"], pt="C"),
                         _F("midpoint", "N", of=["C", "D"]),
                         _F("on_circle", "P", center="N", through="C")],
        "premises": [("harmonic", ("A", "B", "C", "D")),
                     ("perpendicular", ("P", "C", "P", "D"))],
        "conclusion": ("deqangle", ("A", "P", "C", "C", "P", "B"))}),
    Rule("desargues", "clean-room-tong", "advanced", "projective",
         m_desargues, {
        "construction": [_F("free", "O"), _F("free", "A"), _F("free", "B"),
                         _F("free", "C"),
                         _F("on_line", "D", of=["O", "A"]),
                         _F("on_line", "E", of=["O", "B"]),
                         _F("on_line", "G", of=["O", "C"]),
                         _F("intersection", "X", line1=["B", "C"], line2=["E", "G"]),
                         _F("intersection", "Y", line1=["C", "A"], line2=["G", "D"]),
                         _F("intersection", "Z", line1=["A", "B"], line2=["D", "E"])],
        "premises": [("collinear", ("O", "A", "D")), ("collinear", ("O", "B", "E")),
                     ("collinear", ("O", "C", "G")), ("collinear", ("B", "C", "X")),
                     ("collinear", ("E", "G", "X")), ("collinear", ("C", "A", "Y")),
                     ("collinear", ("G", "D", "Y")), ("collinear", ("A", "B", "Z")),
                     ("collinear", ("D", "E", "Z"))],
        "conclusion": ("collinear", ("X", "Y", "Z"))}),
]

CATALOG = {r.name: {"source": r.source, "kind": r.kind, "category": r.category}
           for r in RULES}


# --------------------------------------------------------------------------- #
# Standalone fixpoint driver + helpers.
# --------------------------------------------------------------------------- #
def facts_from(specs) -> set:
    """Turn [(pred, (pt, ...)), ...] into a canonical fact set."""
    return {canon(p, pts) for p, pts in specs}


def apply_rules(facts, rules=None, seed: int = 0, points=None,
                max_rounds: int = 64):
    """Forward-chain ``rules`` to a fixpoint over ``facts`` (a canonical set).

    Deterministic: within each round new facts are sorted before insertion so
    the derivation order does not depend on set iteration order. ``seed`` is
    accepted for signature/interface stability (numeric guards, when a
    coordinate ``points`` map is absent, assume generic position). Returns
    ``(closure, reasons)`` where ``reasons[fact] = (rule_name, [premises])``."""
    if rules is None:
        rules = RULES
    rng = random.Random(seed)  # reserved for guard sampling hooks
    facts = set(facts)
    reasons: dict = {}
    for _ in range(max_rounds):
        pending = []
        for rule in rules:
            for new_fact, premises in rule.match(facts, points):
                if new_fact not in facts:
                    pending.append((new_fact, rule.name, premises))
        if not pending:
            break
        pending.sort(key=lambda t: (str(t[0]), t[1]))
        added = False
        for new_fact, rname, premises in pending:
            if new_fact not in facts:
                facts.add(new_fact)
                reasons[new_fact] = (rname, premises)
                added = True
        if not added:
            break
    return facts, reasons


def prove(hypotheses, goal, seed: int = 0, points=None, rules=None):
    """True iff ``goal`` (a (pred, pts) pair) is in the forward closure of
    ``hypotheses`` under ``rules``."""
    facts = facts_from(hypotheses)
    goal_fact = canon(goal[0], goal[1])
    closure, _ = apply_rules(facts, rules, seed=seed, points=points)
    return goal_fact in closure


def to_geometry_rules(points=None):
    """Adapt the catalog to geometry._RULES' shape: a list of
    (name, callable(facts)->[(fact, [premises])]). Feed this straight into
    geometry.py / geometry_ddar2's forward chainer."""
    return [(r.name, (lambda facts, _r=r: _r.match(facts, points))) for r in RULES]


def numeric_verify(rule: Rule, seed: int, trials: int = 24,
                   tol: float = 1e-6) -> dict:
    """Realize ``rule``'s premises many times from ``seed`` and check the
    conclusion holds numerically in every non-degenerate realization. A trial
    whose premises fail (a degenerate similitude, a conjugate at infinity, ...)
    is skipped, not counted. Returns ``holds`` plus counts."""
    w = rule.witness
    rng = random.Random(seed)
    valid = attempts = 0
    max_attempts = trials * 80
    ok_all = True
    while valid < trials and attempts < max_attempts:
        attempts += 1
        try:
            env = _realize(w["construction"], rng, w.get("spread", 10.0))
        except (DegenerateRealization, ZeroDivisionError, ValueError):
            continue
        scale = _scale_of(env)
        try:
            if not all(_eval(p, pts, env, scale, tol)
                       for p, pts in w.get("premises", [])):
                continue
            c_ok = _eval(w["conclusion"][0], w["conclusion"][1], env, scale, tol)
        except (DegenerateRealization, ZeroDivisionError):
            continue
        valid += 1
        if not c_ok:
            ok_all = False
            break
    return {"holds": ok_all and valid > 0, "valid": valid, "attempts": attempts}


# --------------------------------------------------------------------------- #
# Deliberately OMITTED (could not be made sound over this predicate model, so
# not shipped -- omission over unsoundness):
#   * AlphaGeometry's unsigned inscribed-angle form (cyclic => eqangle) -- only
#     true up to supplement; superseded here by the directed ``deqangle`` form.
#   * eqratio3 / sameside-gated ratio rules (r8, r28, r43) and the aconst /
#     rconst constant-value family -- require signed ratios / oriented
#     half-plane predicates absent from geometry.py's model.
#   * The converse harmonic bisector rule (deqangle + harmonic => perp): a
#     bisector relation holds for BOTH bisectors mod pi, so it does not force
#     the right angle; kept only the sound forward direction.
# --------------------------------------------------------------------------- #
OMITTED = (
    "unsigned-inscribed-angle", "eqratio3-sameside-rules", "aconst-rconst-family",
    "harmonic-bisector-to-perp-converse",
)
