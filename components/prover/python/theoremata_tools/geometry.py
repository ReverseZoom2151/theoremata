"""A dependency-light Euclidean-geometry reasoning vertical (AlphaGeometry-style).

This module implements a *pragmatic, honest* core of the "numeric-verify then
symbolically-prove" pattern used by systems such as AlphaGeometry / Aristotle's
geometry engine. It deliberately does **not** reimplement AlphaGeometry: there
is no language model, no auxiliary-point construction search, and no algebraic
(Groebner/Wu) backend. Instead it provides two independent, individually sound
services over a small typed model of planar geometry:

1. **Numeric verification** (:func:`numeric_check`). A *construction* is a
   sequence of point-definition steps that builds a concrete diagram from a few
   free (randomly placed) points; because dependent points are computed to
   satisfy the hypotheses *by construction*, every realization is a valid
   instance of the problem. We realize the diagram many times from a caller-
   supplied ``seed`` (``Math.random`` is unavailable in the host, so entropy is
   passed in) and test whether the goal predicate holds numerically in every
   non-degenerate realization. This is the standard "diagram check" filter and
   mirrors the repo's falsify-before-prove philosophy: a conjecture that fails
   even one clean realization is *false*, and one that survives many is a strong
   (but not certain) candidate.

2. **Deductive closure** (:func:`deductive_prove`). A forward chainer over a
   deliberately *small but sound* rule set (midpoint => equal segments and
   collinearity; two lines perpendicular to a common line are parallel;
   parallel/perpendicular transport; symmetry/transitivity of congruence and
   parallelism). When the goal appears in the closure we return the ordered
   derivation. Every rule is a theorem of Euclidean geometry, so a returned
   proof is trustworthy; the price is very limited coverage.

Scope and honest limitations
-----------------------------
* CAN: check any goal predicate numerically against constructible hypotheses
  (the construction language covers free points, midpoints, points on a
  line/segment, feet of perpendiculars, reflections, line intersections and
  circumcenters); prove the small family of facts reachable from the rule set
  above by pure forward chaining.
* CANNOT: introduce auxiliary points/constructions to bridge a gap (no proof
  search over constructions); do general angle chasing, ratio/similar-triangle,
  power-of-a-point, or trigonometric reasoning; certify a numeric check as a
  formal proof (it is evidence, not a certificate); handle inequalities,
  3-D, or non-constructive hypotheses. Numeric results are subject to floating-
  point tolerance and the chosen number of trials.

Predicates (arguments are point *names*, order as noted)
    collinear(A, B, C)                three points on a line
    concyclic(A, B, C, D)             four points on a circle
    parallel(A, B, C, D)              line AB parallel to line CD
    perpendicular(A, B, C, D)         line AB perpendicular to line CD
    cong(A, B, C, D)                  |AB| = |CD|            (alias: eqlen)
    eqangle(A, B, C, D, E, F)         angle ABC = angle DEF  (unsigned)
    midpoint(M, A, B)                 M is the midpoint of A and B

No network, no GPU; pure standard library (``math``, ``random``).
"""
from __future__ import annotations

import math
import random
from typing import Any, Callable

Point = tuple[float, float]

# --------------------------------------------------------------------------- #
# Small planar-vector helpers (stdlib floats; no numpy dependency).
# --------------------------------------------------------------------------- #
def _sub(a: Point, b: Point) -> Point:
    return (a[0] - b[0], a[1] - b[1])


def _add(a: Point, b: Point) -> Point:
    return (a[0] + b[0], a[1] + b[1])


def _scale(a: Point, t: float) -> Point:
    return (a[0] * t, a[1] * t)


def _dot(a: Point, b: Point) -> float:
    return a[0] * b[0] + a[1] * b[1]


def _cross(a: Point, b: Point) -> float:
    return a[0] * b[1] - a[1] * b[0]


def _norm(a: Point) -> float:
    return math.hypot(a[0], a[1])


class DegenerateRealization(Exception):
    """A random realization collapsed (coincident points, parallel lines when an
    intersection is required, collinear points when a circumcenter is required,
    ...). Such realizations carry no information and are resampled, not counted.
    """


# --------------------------------------------------------------------------- #
# Construction language: build concrete coordinates so hypotheses hold *by
# construction*. Each step is ``{"op": ..., "point": name, ...}`` and is
# executed in order; later steps may reference earlier points by name.
# --------------------------------------------------------------------------- #
def _need(env: dict[str, Point], name: str) -> Point:
    if name not in env:
        raise ValueError(f"construction references undefined point {name!r}")
    return env[name]


def _foot(p: Point, a: Point, b: Point) -> Point:
    """Foot of the perpendicular from ``p`` onto the line through ``a`` and ``b``."""
    ab = _sub(b, a)
    denom = _dot(ab, ab)
    if denom < 1e-12:
        raise DegenerateRealization("foot: base points coincide")
    t = _dot(_sub(p, a), ab) / denom
    return _add(a, _scale(ab, t))


def _reflect_over_line(p: Point, a: Point, b: Point) -> Point:
    f = _foot(p, a, b)
    return _sub(_scale(f, 2.0), p)


def _line_intersection(a: Point, b: Point, c: Point, d: Point) -> Point:
    """Intersection of line AB with line CD (raises if parallel)."""
    r = _sub(b, a)
    s = _sub(d, c)
    denom = _cross(r, s)
    if abs(denom) < 1e-9:
        raise DegenerateRealization("intersection: lines parallel")
    t = _cross(_sub(c, a), s) / denom
    return _add(a, _scale(r, t))


def _circumcenter(a: Point, b: Point, c: Point) -> Point:
    ax, ay = a
    bx, by = b
    cx, cy = c
    d = 2.0 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by))
    if abs(d) < 1e-9:
        raise DegenerateRealization("circumcenter: points collinear")
    a2, b2, c2 = ax * ax + ay * ay, bx * bx + by * by, cx * cx + cy * cy
    ux = (a2 * (by - cy) + b2 * (cy - ay) + c2 * (ay - by)) / d
    uy = (a2 * (cx - bx) + b2 * (ax - cx) + c2 * (bx - ax)) / d
    return (ux, uy)


def _realize(construction: list[dict[str, Any]], rng: random.Random,
             spread: float) -> dict[str, Point]:
    """Execute one construction into concrete coordinates.

    Free points are placed uniformly in ``[-spread, spread]^2``; dependent
    points are computed so the intended relations hold exactly (up to float
    error). Raises :class:`DegenerateRealization` if the diagram collapses.
    """
    env: dict[str, Point] = {}
    for step in construction:
        op = step["op"]
        name = step["point"]
        if op == "free":
            env[name] = (rng.uniform(-spread, spread), rng.uniform(-spread, spread))
        elif op == "midpoint":
            a, b = (_need(env, p) for p in step["of"])
            env[name] = _scale(_add(a, b), 0.5)
        elif op in ("on_line", "on_segment"):
            a, b = (_need(env, p) for p in step["of"])
            lo, hi = (0.0, 1.0) if op == "on_segment" else (-1.0, 2.0)
            t = rng.uniform(lo, hi)
            env[name] = _add(a, _scale(_sub(b, a), t))
        elif op == "foot":
            p = _need(env, step["from"])
            a, b = (_need(env, q) for q in step["of"])
            env[name] = _foot(p, a, b)
        elif op == "reflect_point":
            # reflection of ``of`` through center ``center``: P = 2*center - of
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
        else:
            raise ValueError(f"unknown construction op: {op!r}")
    return env


# --------------------------------------------------------------------------- #
# Numeric predicate evaluation. Each returns True/False and raises
# DegenerateRealization when the configuration is too degenerate to judge.
# Tolerances are relative to a diagram ``scale`` so they are position/size
# invariant.
# --------------------------------------------------------------------------- #
def _seg(env: dict[str, Point], a: str, b: str) -> Point:
    return _sub(_need(env, b), _need(env, a))


def _angle(env: dict[str, Point], a: str, b: str, c: str) -> float:
    """Unsigned angle ABC in [0, pi] (vertex ``b``)."""
    ba = _seg(env, b, a)
    bc = _seg(env, b, c)
    if _norm(ba) < 1e-9 or _norm(bc) < 1e-9:
        raise DegenerateRealization("angle: coincident points at/around vertex")
    return math.atan2(abs(_cross(ba, bc)), _dot(ba, bc))


def _eval_predicate(pred: str, pts: list[str], env: dict[str, Point],
                    scale: float, tol: float) -> bool:
    a_tol = tol * scale            # absolute length tolerance
    a_tol2 = a_tol * scale         # area / product tolerance

    if pred == "collinear":
        a, b, c = (_need(env, p) for p in pts)
        return abs(_cross(_sub(b, a), _sub(c, a))) <= a_tol2

    if pred == "midpoint":
        m, a, b = (_need(env, p) for p in pts)
        return _norm(_sub(m, _scale(_add(a, b), 0.5))) <= a_tol

    if pred in ("cong", "eqlen"):
        return abs(_norm(_seg(env, pts[0], pts[1]))
                   - _norm(_seg(env, pts[2], pts[3]))) <= a_tol

    if pred == "parallel":
        u, v = _seg(env, pts[0], pts[1]), _seg(env, pts[2], pts[3])
        if _norm(u) < 1e-9 or _norm(v) < 1e-9:
            raise DegenerateRealization("parallel: zero-length line")
        return abs(_cross(u, v)) <= tol * _norm(u) * _norm(v)

    if pred == "perpendicular":
        u, v = _seg(env, pts[0], pts[1]), _seg(env, pts[2], pts[3])
        if _norm(u) < 1e-9 or _norm(v) < 1e-9:
            raise DegenerateRealization("perpendicular: zero-length line")
        return abs(_dot(u, v)) <= tol * _norm(u) * _norm(v)

    if pred == "eqangle":
        a1 = _angle(env, pts[0], pts[1], pts[2])
        a2 = _angle(env, pts[3], pts[4], pts[5])
        return abs(a1 - a2) <= max(tol, 1e-6)

    if pred == "concyclic":
        # Four points are concyclic iff the signed 4x4 determinant vanishes.
        rows = []
        for p in pts:
            x, y = _need(env, p)
            rows.append((x, y, x * x + y * y, 1.0))
        det = _det4(rows)
        return abs(det) <= a_tol2 * scale * scale  # 4x4 det scales like L^4

    raise ValueError(f"unknown predicate: {pred!r}")


def _det4(m: list[tuple[float, float, float, float]]) -> float:
    """Determinant of a 4x4 matrix via cofactor expansion (small, exact enough)."""
    def det3(a: list[list[float]]) -> float:
        return (a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
                - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
                + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]))

    total = 0.0
    for col in range(4):
        minor = [[m[r][c] for c in range(4) if c != col] for r in range(1, 4)]
        total += ((-1.0) ** col) * m[0][col] * det3(minor)
    return total


def _diagram_scale(env: dict[str, Point]) -> float:
    """A characteristic length of the diagram, for relative tolerances."""
    pts = list(env.values())
    if len(pts) < 2:
        return 1.0
    span = max(_norm(_sub(p, q)) for i, p in enumerate(pts) for q in pts[i + 1:])
    return span if span > 1e-9 else 1.0


def numeric_check(construction: list[dict[str, Any]], goal: dict[str, Any],
                  seed: int, trials: int = 40, tol: float = 1e-6,
                  spread: float = 10.0) -> dict[str, Any]:
    """Test whether ``goal`` holds across many random realizations.

    Runs up to ``trials`` non-degenerate realizations (resampling degenerate
    ones up to a cap). Returns ``holds`` (True iff every clean realization
    satisfied the goal) plus counts and, on the first failure, a
    ``counterexample`` (the offending coordinates) -- so this one engine backs
    both the "check" and "falsify" ops.
    """
    pred = goal["pred"]
    pts = list(goal.get("points", goal.get("args", [])))
    rng = random.Random(seed)

    valid = 0
    degenerate = 0
    counterexample: dict[str, Any] | None = None
    max_attempts = trials * 20

    for attempt in range(max_attempts):
        if valid >= trials:
            break
        try:
            env = _realize(construction, rng, spread)
            scale = _diagram_scale(env)
            ok = _eval_predicate(pred, pts, env, scale, tol)
        except DegenerateRealization:
            degenerate += 1
            continue
        valid += 1
        if not ok and counterexample is None:
            counterexample = {name: [round(x, 6), round(y, 6)]
                              for name, (x, y) in env.items()}
            break

    holds = counterexample is None and valid > 0
    result: dict[str, Any] = {
        "goal": {"pred": pred, "points": pts},
        "holds": holds,
        "trials_requested": trials,
        "trials_valid": valid,
        "trials_degenerate": degenerate,
        "tolerance": tol,
    }
    if valid == 0:
        result["holds"] = False
        result["reason"] = "no non-degenerate realization found"
    if counterexample is not None:
        result["counterexample"] = counterexample
    return result


# --------------------------------------------------------------------------- #
# Symbolic side: canonical facts + a small SOUND forward-chaining rule set.
# A "fact" is a hashable canonical key; ``_describe`` renders it for humans.
# --------------------------------------------------------------------------- #
Fact = tuple[Any, ...]


def _seg_key(a: str, b: str) -> frozenset[str]:
    return frozenset((a, b))


def _canonical(pred: str, pts: list[str]) -> Fact:
    """Canonicalize a predicate instance so logically-equal facts unify.

    Congruence, parallelism, perpendicularity and concyclicity are symmetric in
    their operands; collinearity/concyclicity are set-based; an angle is a
    vertex plus the unordered pair of rays.
    """
    if pred == "collinear":
        return ("collinear", frozenset(pts))
    if pred == "concyclic":
        return ("concyclic", frozenset(pts))
    if pred == "midpoint":
        return ("midpoint", pts[0], _seg_key(pts[1], pts[2]))
    if pred in ("cong", "eqlen"):
        return ("cong", frozenset((_seg_key(pts[0], pts[1]),
                                   _seg_key(pts[2], pts[3]))))
    if pred == "parallel":
        return ("parallel", frozenset((_seg_key(pts[0], pts[1]),
                                       _seg_key(pts[2], pts[3]))))
    if pred == "perpendicular":
        return ("perpendicular", frozenset((_seg_key(pts[0], pts[1]),
                                            _seg_key(pts[2], pts[3]))))
    if pred == "eqangle":
        ang1 = (pts[1], frozenset((pts[0], pts[2])))
        ang2 = (pts[4], frozenset((pts[3], pts[5])))
        return ("eqangle", frozenset((ang1, ang2)))
    raise ValueError(f"unknown predicate: {pred!r}")


def _fmt_seg(s: frozenset[str]) -> str:
    return "".join(sorted(s))


def _describe(fact: Fact) -> str:
    kind = fact[0]
    if kind in ("collinear", "concyclic"):
        return f"{kind}({','.join(sorted(fact[1]))})"
    if kind == "midpoint":
        return f"midpoint({fact[1]} of {_fmt_seg(fact[2])})"
    if kind == "cong":
        a, b = sorted(fact[1], key=_fmt_seg)
        return f"cong({_fmt_seg(a)}={_fmt_seg(b)})"
    if kind in ("parallel", "perpendicular"):
        a, b = sorted(fact[1], key=_fmt_seg)
        sym = "||" if kind == "parallel" else "_|_"
        return f"{_fmt_seg(a)} {sym} {_fmt_seg(b)}"
    if kind == "eqangle":
        parts = ["<" + v + ":" + "".join(sorted(rays)) + ">"
                 for v, rays in sorted(fact[1], key=lambda x: (x[0], sorted(x[1])))]
        return "eqangle(" + "=".join(parts) + ")"
    return str(fact)


# Each rule takes the current fact set and yields (new_fact, [premise_facts]).
def _rule_midpoint(facts: set[Fact]) -> list[tuple[Fact, list[Fact]]]:
    out = []
    for f in facts:
        if f[0] == "midpoint":
            m, seg = f[1], f[2]
            a, b = tuple(seg)
            # M is midpoint of AB  =>  |MA| = |MB|
            out.append((("cong", frozenset((_seg_key(m, a), _seg_key(m, b)))), [f]))
            # M is midpoint of AB  =>  A, M, B collinear
            out.append((("collinear", frozenset((a, m, b))), [f]))
    return out


def _rule_perp_perp_parallel(facts: set[Fact]) -> list[tuple[Fact, list[Fact]]]:
    """Two lines perpendicular to a common line are parallel to each other."""
    out = []
    perps = [f for f in facts if f[0] == "perpendicular"]
    for i, f1 in enumerate(perps):
        for f2 in perps[i + 1:]:
            common = f1[1] & f2[1]
            if len(common) == 1:
                others = (f1[1] - common) | (f2[1] - common)
                if len(others) == 2:
                    out.append((("parallel", frozenset(others)), [f1, f2]))
    return out


def _rule_perp_parallel_transport(facts: set[Fact]) -> list[tuple[Fact, list[Fact]]]:
    """AB _|_ CD and CD || EF  =>  AB _|_ EF."""
    out = []
    perps = [f for f in facts if f[0] == "perpendicular"]
    paras = [f for f in facts if f[0] == "parallel"]
    for pf in perps:
        for qf in paras:
            shared = pf[1] & qf[1]
            if len(shared) == 1:
                perp_other = next(iter(pf[1] - shared))
                para_other = next(iter(qf[1] - shared))
                if perp_other != para_other:
                    out.append((("perpendicular",
                                 frozenset((perp_other, para_other))), [pf, qf]))
    return out


def _rule_parallel_transitive(facts: set[Fact]) -> list[tuple[Fact, list[Fact]]]:
    """AB || CD and CD || EF  =>  AB || EF."""
    out = []
    paras = [f for f in facts if f[0] == "parallel"]
    for i, f1 in enumerate(paras):
        for f2 in paras[i + 1:]:
            shared = f1[1] & f2[1]
            if len(shared) == 1:
                others = (f1[1] - shared) | (f2[1] - shared)
                if len(others) == 2:
                    out.append((("parallel", frozenset(others)), [f1, f2]))
    return out


def _rule_cong_transitive(facts: set[Fact]) -> list[tuple[Fact, list[Fact]]]:
    """|AB|=|CD| and |CD|=|EF|  =>  |AB|=|EF|."""
    out = []
    congs = [f for f in facts if f[0] == "cong"]
    for i, f1 in enumerate(congs):
        for f2 in congs[i + 1:]:
            shared = f1[1] & f2[1]
            if len(shared) == 1:
                others = (f1[1] - shared) | (f2[1] - shared)
                if len(others) == 2:
                    out.append((("cong", frozenset(others)), [f1, f2]))
    return out


_RULES: list[tuple[str, Callable[[set[Fact]], list[tuple[Fact, list[Fact]]]]]] = [
    ("midpoint=>equal-segments-and-collinear", _rule_midpoint),
    ("two-perpendiculars-to-a-line-are-parallel", _rule_perp_perp_parallel),
    ("perpendicular-transported-along-parallel", _rule_perp_parallel_transport),
    ("parallelism-is-transitive", _rule_parallel_transitive),
    ("congruence-is-transitive", _rule_cong_transitive),
]


def deductive_prove(hypotheses: list[dict[str, Any]], goal: dict[str, Any],
                    max_rounds: int = 32) -> dict[str, Any]:
    """Forward-chain the sound rule set to closure, seeking ``goal``.

    Returns ``proved`` and, when proved, an ordered ``derivation`` (each step
    names the rule and its premises) reconstructed by back-tracing the goal to
    the original hypotheses.
    """
    goal_fact = _canonical(goal["pred"], list(goal.get("points", goal.get("args", []))))
    facts: set[Fact] = set()
    reasons: dict[Fact, tuple[str, list[Fact]]] = {}
    for h in hypotheses:
        facts.add(_canonical(h["pred"], list(h.get("points", h.get("args", [])))))

    if goal_fact in facts:
        return {"proved": True, "derivation": [], "note": "goal is a hypothesis"}

    for _ in range(max_rounds):
        added = False
        for rule_name, rule in _RULES:
            for new_fact, premises in rule(facts):
                if new_fact not in facts:
                    facts.add(new_fact)
                    reasons[new_fact] = (rule_name, premises)
                    added = True
        if goal_fact in facts:
            break
        if not added:
            break

    if goal_fact not in facts:
        return {"proved": False, "closure_size": len(facts),
                "derivation": None}

    # Back-trace to an ordered, minimal derivation.
    order: list[Fact] = []
    seen: set[Fact] = set()

    def _emit(f: Fact) -> None:
        if f in seen:
            return
        seen.add(f)
        if f in reasons:
            for p in reasons[f][1]:
                _emit(p)
            order.append(f)

    _emit(goal_fact)
    derivation = [
        {"fact": _describe(f), "rule": reasons[f][0],
         "from": [_describe(p) for p in reasons[f][1]]}
        for f in order
    ]
    return {"proved": True, "derivation": derivation, "closure_size": len(facts)}


# --------------------------------------------------------------------------- #
# Worker entrypoint.
# --------------------------------------------------------------------------- #
def run(request: dict[str, Any]) -> dict[str, Any]:
    """Worker entrypoint. ``request["op"]`` selects the capability.

    * ``check``   -> :func:`numeric_check` (``construction``, ``goal``, ``seed``;
      optional ``trials``, ``tol``, ``spread``). ``holds`` is True iff the goal
      held in every non-degenerate realization.
    * ``falsify`` -> :func:`numeric_check` reframed: returns ``falsified`` /
      ``counterexample`` (a realization satisfying the hypotheses where the goal
      fails). Same inputs as ``check``.
    * ``prove``   -> :func:`deductive_prove` (``hypotheses``, ``goal``): symbolic
      forward-chaining; returns ``proved`` and a ``derivation`` when found.

    A ``goal``/``hypothesis`` is ``{"pred": <name>, "points": [<names>...]}``.
    """
    op = request.get("op", "check")

    if op in ("check", "falsify"):
        result = numeric_check(
            request["construction"],
            request["goal"],
            seed=int(request["seed"]),
            trials=int(request.get("trials", 40)),
            tol=float(request.get("tol", 1e-6)),
            spread=float(request.get("spread", 10.0)),
        )
        if op == "falsify":
            return {
                "op": "falsify",
                "goal": result["goal"],
                "falsified": not result["holds"] and result["trials_valid"] > 0
                             and "counterexample" in result,
                "counterexample": result.get("counterexample"),
                "trials_valid": result["trials_valid"],
                "trials_degenerate": result["trials_degenerate"],
                "reason": result.get("reason"),
            }
        return {"op": "check", **result}

    if op == "prove":
        result = deductive_prove(request.get("hypotheses", []), request["goal"])
        return {"op": "prove", **result}

    raise ValueError(f"unknown op: {op!r}")
