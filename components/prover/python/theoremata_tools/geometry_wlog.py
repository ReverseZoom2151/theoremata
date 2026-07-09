"""WLOG frame-normalization tactic + invariance-lemma registry.

The *by-hand* coordinate assignment inside
:mod:`theoremata_tools.geometry_algebraic` (Wu's method) silently exploits a
deep fact: most Euclidean-geometry goals are **invariant under a transformation
group** -- translation, rotation, uniform scaling, reflection, and (Harrison's
``wlog.pdf`` / ``vector.pdf``) *dimension embedding*. Because of that, one may
"without loss of generality" pin a convenient coordinate frame -- put a point
at the origin, an edge on an axis, fix the scale, drop to the minimal dimension
-- and prove the specialized statement instead of the general one.

This module turns that hidden convenience into an **explicit, checkable WLOG
step**:

* :func:`normalize_frame` (build #11) applies an invariance-justified change of
  coordinates that pins the frame and returns the transformed configuration,
  the exact affine map used (``x' = M @ (x - t)``), and the invariance
  justification for every group element it applied.

* :data:`INVARIANCE_REGISTRY` (build #12) maps each geometric predicate to the
  set of transformation groups under which it is invariant. ``normalize_frame``
  consults it as a **soundness gate**: it applies a normalization only if every
  predicate occurring in the goal (and hypotheses) is invariant under the group
  the normalization belongs to. If a non-invariant predicate is present, the
  tactic *refuses* rather than silently produce an unsound reduction.

Key dimension-drop theorem (guard, from Harrison's vector.pdf): an
inner-product / Euclidean statement in ``k`` vector variables holds in every
dimension ``>= k`` iff it holds in ``R^k``. So a configuration whose points span
an affine subspace of dimension ``d`` may be embedded in / dropped to ``R^d``
without loss of generality -- provided every predicate is dimension-invariant
(all metric/affine predicates here are).

Reuse: all symbolic affine/linear algebra goes through ``sympy`` (``Matrix``,
``Symbol``, ``sqrt``, ``simplify``, ``Matrix.rank``) -- deterministic, offline,
exact. The point/coordinate representation matches
:mod:`geometry_algebraic`: ``points`` is ``{name: [x, y]}`` where each
coordinate is a number (a pinned constant) or a string (a free variable);
sympy expressions are also accepted so a normalized frame can be re-normalized
or fed back through a predicate evaluator.

Honest scope: normalizations live in the 2-D similarity group (+ reflection) and
the dimension group; the gate is a *necessary* soundness condition on the goal's
predicates, not a decision procedure for the theorem itself (that is Wu's job).
Reflection canonicalization (``reflect_above``) needs numerically evaluable
coordinates to decide the half-plane; it is skipped symbolically.
"""
from __future__ import annotations

from typing import Any, Iterable

import sympy as sp


# --------------------------------------------------------------------------- #
# #12  Invariance-lemma registry.
#
# The transformation groups, and -- per predicate -- the groups that leave the
# predicate's truth value unchanged. Naming:
#   translation  : x -> x + a
#   rotation     : x -> R x               (SO(2))
#   scaling      : x -> s x   (s > 0)     (uniform dilation)
#   reflection   : x -> F x               (orientation-reversing isometry)
#   dimension    : R^k  <->  R^n (n >= k) (embed / drop, Euclidean statements)
# --------------------------------------------------------------------------- #
TRANSLATION = "translation"
ROTATION = "rotation"
SCALING = "scaling"
REFLECTION = "reflection"
DIMENSION = "dimension"

ALL_GROUPS = frozenset({TRANSLATION, ROTATION, SCALING, REFLECTION, DIMENSION})

# The full 2-D similarity group (+ reflection) + dimension: everything that is
# purely affine/projective or a *ratio* of metric quantities is invariant here.
SIMILARITY = ALL_GROUPS
# Isometries (+ dimension): absolute metric quantities (lengths, areas) survive
# translation/rotation/reflection but NOT uniform scaling.
ISOMETRY = frozenset({TRANSLATION, ROTATION, REFLECTION, DIMENSION})

INVARIANCE_REGISTRY: dict[str, frozenset[str]] = {
    # -- affine / projective incidence: invariant under the whole group ------ #
    "collinear": SIMILARITY,
    "parallel": SIMILARITY,
    "perpendicular": SIMILARITY,
    "concurrent": SIMILARITY,
    "midpoint": SIMILARITY,
    # -- ratios of metric quantities: scale-free, so fully invariant --------- #
    "cong": SIMILARITY,          # |AB| = |CD|  (a ratio == 1)
    "eqlen": SIMILARITY,
    "ratio": SIMILARITY,
    "dist_ratio": SIMILARITY,
    "eqratio": SIMILARITY,
    "eqprod": SIMILARITY,        # |AB||EF| = |CD||GH|
    "area_ratio": SIMILARITY,
    # -- UNoriented angles: preserved by scaling and by reflection ---------- #
    "angle": SIMILARITY,
    "aconst": SIMILARITY,
    "eqangle": SIMILARITY,
    # -- absolute metric quantities: NOT scale-invariant -------------------- #
    "distance": ISOMETRY,
    "length": ISOMETRY,
    "lconst": ISOMETRY,          # |XA| = given constant
    "area": ISOMETRY,
    "aconst_area": ISOMETRY,
    # -- ORIENTED quantities: also NOT reflection-invariant ----------------- #
    #    signed area scales by s^2 (drop SCALING) and flips sign under
    #    reflection (drop REFLECTION); oriented angle flips under reflection.
    "signed_area": frozenset({TRANSLATION, ROTATION, DIMENSION}),
    "oriented_angle": frozenset({TRANSLATION, ROTATION, SCALING, DIMENSION}),
}


def invariance_groups(pred: str) -> frozenset[str]:
    """Groups under which ``pred`` is invariant; ``frozenset()`` if unknown.

    An unknown predicate returns the empty set, so the soundness gate *refuses*
    every normalization for it -- fail closed, never fail open.
    """
    return INVARIANCE_REGISTRY.get(pred, frozenset())


def gate(predicates: Iterable[str], groups_used: Iterable[str]) -> dict[str, Any]:
    """Soundness gate: may we apply the ``groups_used`` given these predicates?

    Returns ``{"ok": bool, "justification": {group: [preds...]},
    "violations": [(group, pred), ...]}``. A normalization group is admissible
    iff *every* predicate is invariant under it (the registry is the theorem
    store; a missing predicate is treated as non-invariant everywhere).
    """
    preds = list(dict.fromkeys(predicates))  # dedup, keep order, determinism
    groups = list(dict.fromkeys(groups_used))
    violations: list[tuple[str, str]] = []
    justification: dict[str, list[str]] = {}
    for g in groups:
        justification[g] = []
        for p in preds:
            if g in invariance_groups(p):
                justification[g].append(p)
            else:
                violations.append((g, p))
    return {"ok": not violations, "justification": justification,
            "violations": violations}


# --------------------------------------------------------------------------- #
# Coordinate model (matches geometry_algebraic: number = pinned, str = free).
# --------------------------------------------------------------------------- #
def _coord_expr(entry: Any) -> sp.Expr:
    """One coordinate -> exact sympy expression."""
    if isinstance(entry, sp.Expr):
        return entry
    if isinstance(entry, str):
        # A name ("u1") parses to a Symbol; a numeric/expression string
        # ("13/25", "sqrt(17)/5") parses to its exact value -- so serialized
        # normalized frames round-trip cleanly back through this evaluator.
        return sp.sympify(entry, rational=True)
    return sp.nsimplify(sp.sympify(entry), rational=True)


def _as_points(points: dict[str, list[Any]]) -> dict[str, sp.Matrix]:
    out: dict[str, sp.Matrix] = {}
    for name, xy in points.items():
        if len(xy) != 2:
            raise ValueError(f"point {name!r} needs exactly 2 coordinates")
        out[name] = sp.Matrix([_coord_expr(xy[0]), _coord_expr(xy[1])])
    return out


def _num(e: sp.Expr) -> float | None:
    """Float value of a fully-numeric expression, else ``None``."""
    v = sp.nsimplify(e) if e.free_symbols else e
    if v.free_symbols:
        return None
    try:
        return float(v)
    except (TypeError, ValueError):
        return None


def _serialize(e: sp.Expr) -> Any:
    """sympy expr -> a JSON-friendly value (int / str)."""
    e = sp.simplify(e)
    if e.is_Integer:
        return int(e)
    if not e.free_symbols and e.is_Rational:
        return str(sp.nsimplify(e))  # e.g. "1/2"
    return str(e)


# --------------------------------------------------------------------------- #
# #11  normalize_frame -- the WLOG change of coordinates.
# --------------------------------------------------------------------------- #
def normalize_frame(points: dict[str, list[Any]], spec: dict[str, Any]
                    ) -> dict[str, Any]:
    """Apply an invariance-justified frame normalization.

    ``spec`` keys (each optional, applied in this order):
      * ``origin``       -- point name to translate to ``(0, 0)``   (translation)
      * ``x_axis``       -- point name to rotate onto the +x axis   (rotation)
      * ``unit``         -- point name whose distance from the origin is scaled
                            to ``1`` (needs ``origin``)              (scaling)
      * ``reflect_above``-- point name to force into the upper half-plane
                            (``y >= 0``); numeric coordinates only   (reflection)
      * ``predicates``   -- iterable of predicate names to gate against. If
                            omitted, the caller vouches for soundness and the
                            gate is skipped (``"gated": False``).

    Returns ``{"points": {name:[x,y]}, "transformation": {...}, "invariance":
    {...}, "refused": bool}``. On a soundness violation it returns
    ``refused=True`` with the offending ``(group, predicate)`` pairs and leaves
    the configuration untouched.
    """
    pts = _as_points(points)

    # 1. Which transformation groups does this spec exercise?
    groups_used: list[str] = []
    if "origin" in spec:
        groups_used.append(TRANSLATION)
    if "x_axis" in spec:
        groups_used.append(ROTATION)
    if "unit" in spec:
        groups_used.append(SCALING)
    if "reflect_above" in spec:
        groups_used.append(REFLECTION)

    # 2. Soundness gate against the goal/hypothesis predicates.
    predicates = list(spec.get("predicates", []))
    gated = bool(predicates)
    gate_res = gate(predicates, groups_used) if gated else {
        "ok": True, "justification": {g: [] for g in groups_used},
        "violations": []}
    if gated and not gate_res["ok"]:
        return {
            "refused": True,
            "reason": "non-invariant predicate under a requested normalization",
            "violations": [
                {"group": g, "predicate": p} for g, p in gate_res["violations"]
            ],
            "groups_requested": groups_used,
            "predicates": predicates,
            "points": points,
        }

    # 3. Build the affine map  x' = M @ (x - t).
    t = sp.Matrix([0, 0])
    if "origin" in spec:
        t = pts[_need(spec, "origin", pts)]

    M = sp.eye(2)
    steps: list[dict[str, Any]] = []
    if "origin" in spec:
        steps.append({"group": TRANSLATION, "op": f"translate {spec['origin']} -> (0,0)"})

    if "x_axis" in spec:
        p = pts[_need(spec, "x_axis", pts)]
        v = p - t  # vector we want on the +x axis
        r2 = sp.simplify(v[0] ** 2 + v[1] ** 2)
        r = sp.sqrt(r2)
        if r == 0:
            raise ValueError("x_axis point coincides with the origin")
        c, s = sp.simplify(v[0] / r), sp.simplify(v[1] / r)
        R = sp.Matrix([[c, s], [-s, c]])  # R @ v = (r, 0)
        M = R * M
        steps.append({"group": ROTATION,
                      "op": f"rotate {spec['x_axis']} onto +x axis"})

    if "unit" in spec:
        p = pts[_need(spec, "unit", pts)]
        v = p - t
        r = sp.sqrt(sp.simplify(v[0] ** 2 + v[1] ** 2))
        if r == 0:
            raise ValueError("unit point coincides with the origin")
        M = (sp.Integer(1) / r) * M
        steps.append({"group": SCALING,
                      "op": f"scale so |origin,{spec['unit']}| = 1"})

    # 4. Apply, then (optionally) reflect to canonicalize the half-plane.
    def apply(mat: sp.Matrix, trans: sp.Matrix, q: sp.Matrix) -> sp.Matrix:
        return sp.simplify(mat * (q - trans))

    new = {name: apply(M, t, q) for name, q in pts.items()}

    if "reflect_above" in spec:
        name = _need(spec, "reflect_above", pts)
        yval = _num(new[name][1])
        if yval is not None and yval < 0:
            F = sp.Matrix([[1, 0], [0, -1]])
            M = F * M
            new = {n: apply(M, t, q) for n, q in pts.items()}
            steps.append({"group": REFLECTION,
                          "op": f"reflect across x-axis so {name} has y >= 0"})
        else:
            # Reflection not needed; it was requested but is a no-op here.
            if REFLECTION in groups_used and yval is None:
                steps.append({"group": REFLECTION,
                              "op": "reflection skipped (non-numeric y)"})

    det = sp.simplify(M.det())
    transformation = {
        "form": "x' = M @ (x - t)",
        "M": [[_serialize(M[i, j]) for j in range(2)] for i in range(2)],
        "t": [_serialize(t[0]), _serialize(t[1])],
        "determinant": _serialize(det),
        "orientation": ("reversing" if (_num(det) or 0) < 0 else "preserving"),
        "steps": steps,
    }
    invariance = {
        "gated": gated,
        "groups_used": groups_used,
        "justification": gate_res["justification"],
        "note": ("every listed predicate is invariant under every applied group"
                 if gated else "gate skipped (no predicates supplied)"),
    }
    return {
        "refused": False,
        "points": {n: [_serialize(q[0]), _serialize(q[1])] for n, q in new.items()},
        "points_expr": new,  # sympy Matrices, for re-use / predicate evaluation
        "transformation": transformation,
        "invariance": invariance,
    }


def _need(spec: dict[str, Any], key: str, pts: dict[str, sp.Matrix]) -> str:
    name = spec[key]
    if name not in pts:
        raise ValueError(f"{key} references undefined point {name!r}")
    return name


# --------------------------------------------------------------------------- #
# Dimension drop -- guarded by the R^k theorem.
# --------------------------------------------------------------------------- #
def min_embedding_dimension(points: dict[str, list[Any]]) -> int:
    """Affine dimension spanned by the points (rank of the edge vectors)."""
    pts = _as_points(points)
    if len(pts) <= 1:
        return 0
    names = list(pts)
    base = pts[names[0]]
    cols = [pts[n] - base for n in names[1:]]
    return int(sp.Matrix.hstack(*cols).rank())


def dimension_drop(points: dict[str, list[Any]], predicates: Iterable[str],
                   ambient: int = 2) -> dict[str, Any]:
    """WLOG dimension reduction, guarded by Harrison's ``R^k`` theorem.

    A Euclidean statement in ``k`` vector variables holds in all dimensions
    ``>= k`` iff it holds in ``R^k``; so the configuration may be dropped to the
    affine dimension its points actually span -- but *only* when every predicate
    is invariant under the ``dimension`` group.
    """
    preds = list(dict.fromkeys(predicates))
    g = gate(preds, [DIMENSION])
    target = min_embedding_dimension(points)
    justified = g["ok"] and target <= ambient
    return {
        "ambient_dimension": ambient,
        "spanned_dimension": target,
        "target_dimension": target if justified else ambient,
        "justified": justified,
        "theorem": ("a Euclidean/inner-product statement in k vector variables "
                    "holds in all dim >= k iff it holds in R^k"),
        "violations": [{"group": grp, "predicate": p} for grp, p in g["violations"]],
        "predicates": preds,
    }


# --------------------------------------------------------------------------- #
# Predicate evaluation -- lets callers/tests confirm a normalization preserved
# an invariant predicate's value.
# --------------------------------------------------------------------------- #
def predicate_value(pred: str, pts: list[str],
                    points: dict[str, list[Any]]) -> sp.Expr:
    """Exact value of a predicate's defining expression on ``points``.

    ``== 0`` predicates (collinear/parallel/perpendicular/cong) return the LHS;
    scalar predicates (dist_ratio/distance/angle) return the quantity itself.
    """
    P = _as_points(points)

    def sub(a: str, b: str) -> sp.Matrix:
        return P[a] - P[b]

    def cross(u: sp.Matrix, v: sp.Matrix) -> sp.Expr:
        return u[0] * v[1] - u[1] * v[0]

    def dot(u: sp.Matrix, v: sp.Matrix) -> sp.Expr:
        return u[0] * v[0] + u[1] * v[1]

    def norm(u: sp.Matrix) -> sp.Expr:
        return sp.sqrt(dot(u, u))

    if pred == "collinear":
        a, b, c = pts
        return sp.simplify(cross(sub(b, a), sub(c, a)))
    if pred == "parallel":
        a, b, c, d = pts
        return sp.simplify(cross(sub(b, a), sub(d, c)))
    if pred == "perpendicular":
        a, b, c, d = pts
        return sp.simplify(dot(sub(b, a), sub(d, c)))
    if pred in ("cong", "eqlen"):
        a, b, c, d = pts
        return sp.simplify(dot(sub(b, a), sub(b, a)) - dot(sub(d, c), sub(d, c)))
    if pred in ("dist_ratio", "ratio"):
        a, b, c, d = pts
        return sp.simplify(norm(sub(b, a)) / norm(sub(d, c)))
    if pred in ("distance", "length"):
        a, b = pts
        return sp.simplify(norm(sub(b, a)))
    if pred == "signed_area":  # oriented area of triangle a,b,c
        a, b, c = pts
        return sp.simplify(cross(sub(b, a), sub(c, a)) / 2)
    if pred == "angle":  # unoriented angle at b in a-b-c, via cosine
        a, b, c = pts
        u, v = sub(a, b), sub(c, b)
        return sp.simplify(dot(u, v) / (norm(u) * norm(v)))
    raise ValueError(f"predicate_value: unsupported predicate {pred!r}")


# --------------------------------------------------------------------------- #
# Worker entrypoint.
# --------------------------------------------------------------------------- #
def run(request: dict[str, Any]) -> dict[str, Any]:
    """Worker entrypoint (suggested op/tool name: ``geometry_wlog``).

    Ops
      * ``wlog_normalize`` -- apply :func:`normalize_frame`. Inputs: ``points``
        (``{name:[x,y]}``), ``spec`` (see :func:`normalize_frame`). Predicates
        for the soundness gate may be given directly as ``spec["predicates"]``
        or harvested from ``goal`` / ``hypotheses`` predicate dicts. Returns the
        normalized config + the transformation + the invariance justification,
        or ``refused=True`` on a soundness violation.
      * ``dimension_drop`` -- :func:`dimension_drop`: report the justified target
        dimension guarded by the ``R^k`` theorem.
    """
    op = request.get("op", "wlog_normalize")

    if op == "wlog_normalize":
        spec = dict(request.get("spec", {}))
        # Harvest predicates from goal/hypotheses if not supplied on the spec.
        if "predicates" not in spec:
            preds = _harvest_predicates(request)
            if preds:
                spec["predicates"] = preds
        result = normalize_frame(request["points"], spec)
        result.pop("points_expr", None)  # sympy objects are not serializable
        return {"op": "geometry_wlog", "sub_op": "wlog_normalize", **result}

    if op == "dimension_drop":
        preds = request.get("predicates") or _harvest_predicates(request)
        result = dimension_drop(
            request["points"], preds,
            ambient=int(request.get("ambient", 2)),
        )
        return {"op": "geometry_wlog", "sub_op": "dimension_drop", **result}

    raise ValueError(f"unknown op: {op!r}")


def _harvest_predicates(request: dict[str, Any]) -> list[str]:
    preds: list[str] = []
    goal = request.get("goal")
    if isinstance(goal, dict) and "pred" in goal:
        preds.append(goal["pred"])
    for h in request.get("hypotheses", []) or []:
        if isinstance(h, dict) and "pred" in h:
            preds.append(h["pred"])
    return list(dict.fromkeys(preds))
