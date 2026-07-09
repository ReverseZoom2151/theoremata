"""Minimal-dependency traceback + ``check_good`` requires-auxiliary synthesis.

A **second, cleaner synthesis path** over the same sound geometry core as
:mod:`theoremata_tools.geometry_synth`. Where ``geometry_synth`` mints examples
by *sampling* a construction and peeling off the AlphaGeometry "dependency
difference", this module offers two sharper, provenance-driven primitives that
turn a deductive-closure run into short, auditable proofs and aux-labelled hard
problems:

1. **Minimal-dependency traceback** (item 2, AlphaGeometry v1 ``Table.why``).
   Given a derivation DAG (each derived fact + the rule and parent facts that
   produced it), trace a goal fact back to its *minimal* premise subgraph. For
   the equality-style facts our rule set chains through (``cong`` = equal length,
   ``parallel``/``perpendicular`` = related directions), a derived equality is
   justified by a **linear combination** of source equalities; we model each such
   fact as an exact rational linear equation and run a **minimal-support solve**
   (an exact minimum-cardinality subset search over the linear certificate, the
   stdlib analogue of AG's ``linprog`` traceback) to drop every redundant
   premise. Non-linear goals fall back to the DAG's own leaf set (already minimal
   because unused premises never enter the goal's dependency cone). We then
   re-prove the goal from the minimal premises via :func:`geometry.deductive_prove`
   so the emitted proof is trustworthy, not merely plausible.

2. **``check_good`` requires-auxiliary detector** (item 5, TongGeometry, CLEAN
   ROOM). For every derived fact we compute its **statement footprint** (the
   points named in the fact itself) and its **proof footprint** (every point that
   appears anywhere in its minimal-dependency subgraph -- the leaf premises and
   the intermediate derived facts). A fact whose *proof* footprint **strictly
   exceeds** its *statement* footprint provably could not be proved using only the
   points it names: it required an auxiliary construction. We flag it and emit
   ``{goal, aux_points = proof_footprint - statement_footprint, proof}`` -- a hard
   synthetic problem with a *known* auxiliary answer. This is re-implemented from
   the mathematics described in ``docs/resource-mining/new/tong-geometry.md``
   (``context_actions = prune(fact_points)`` vs ``ca_actions = prune(proof_points)``,
   flag when ``set(context) < set(proof)``); **no GPLv3 source was copied** --
   TongGeometry is GPLv3 and only its *idea* (footprint difference) is reused.

``run(request)`` ops (all offline, deterministic; any sampling seed is passed in):
    * ``traceback``   -- goal + premises (optionally an explicit derivation DAG)
      -> minimal proof + minimal/dropped premises.
    * ``check_good``  -- premises (or a ``seed`` to sample a construction) -> the
      list of derived facts that require auxiliary points, each with its aux set.
    * ``harvest``     -- a seeded batch of constructions -> aux-labelled examples.

Reuse (never edited): :func:`geometry._canonical`, :func:`geometry._describe`,
``geometry._RULES``, :func:`geometry.deductive_prove`, :func:`geometry.numeric_check`;
:func:`geometry_synth._closure`, :func:`geometry_synth._traceback`,
:func:`geometry_synth._fact_to_goal`, :func:`geometry_synth._canon`,
:func:`geometry_synth._sample_construction`. The linear model and the
minimal-support solve are new here.

Honest scope
------------
* The linear model covers exactly the equality relations geometry.py chains
  through (``cong``/``parallel``/``perpendicular``); ``collinear``/``midpoint``
  goals use the DAG leaf-set traceback (already minimal for our rule set). There
  is no angle/ratio (full DD+AR) chase -- that needs a richer engine than
  geometry.py exposes.
* The minimal-support solve is an exact minimum-cardinality subset search
  (worst case exponential in the number of *linearly relevant* premises); fine
  for the small proofs this core produces, not a scalable MILP.
* Aux examples are as rich as the sampled gadgets in ``geometry_synth`` allow;
  ``harvest`` piggy-backs on that sampler for its deterministic diagrams.

Pure standard library (+ geometry.py / geometry_synth.py, both stdlib-only).
"""
from __future__ import annotations

from fractions import Fraction
from itertools import combinations
from typing import Any

from theoremata_tools import geometry, geometry_synth

Fact = tuple[Any, ...]

# Coordinate name used for the affine constant of a linear equation. Prefixed so
# it can never collide with a real variable ("dir:..." / "len:...").
_CONST = "__const__"

_NUMERIC_TRIALS = 24


# --------------------------------------------------------------------------- #
# The linear model: equality-style facts as exact rational linear equations.
#
# Each fact becomes a vector over abstract variables plus a constant coordinate,
# encoding an equation ``sum(coeff * var) = const``:
#   parallel(AB, CD)       ->  dir(AB) - dir(CD) = 0
#   perpendicular(AB, CD)  ->  dir(AB) - dir(CD) = 1/2      (half-turns)
#   cong(AB, CD)           ->  len(AB) - len(CD) = 0
# Direction and length live in disjoint variable spaces, so the solver never
# mixes an angle relation with a length relation -- exactly as intended.
# --------------------------------------------------------------------------- #
Vector = dict[str, Fraction]

_LINEAR_KINDS = ("parallel", "perpendicular", "cong")


def _seg_var(prefix: str, seg: frozenset[str]) -> str:
    return f"{prefix}:{''.join(sorted(seg))}"


def _fact_equation(fact: Fact) -> Vector | None:
    """Return the exact rational equation vector for an equality-style ``fact``.

    ``None`` for facts outside the linear model (``collinear``/``midpoint``/...).
    Orientation (which segment is +1 vs -1) is irrelevant to linear span, so any
    consistent choice works; we sort for determinism.
    """
    kind = fact[0]
    if kind not in _LINEAR_KINDS:
        return None
    segs = sorted(fact[1], key=lambda s: sorted(s))
    s1, s2 = segs[0], segs[1]
    if kind == "cong":
        d1, d2 = _seg_var("len", s1), _seg_var("len", s2)
        const = Fraction(0)
    else:  # parallel / perpendicular use direction variables
        d1, d2 = _seg_var("dir", s1), _seg_var("dir", s2)
        const = Fraction(0) if kind == "parallel" else Fraction(1, 2)
    return {d1: Fraction(1), d2: Fraction(-1), _CONST: const}


def _clean(vec: Vector) -> Vector:
    return {k: v for k, v in vec.items() if v != 0}


def _reduce(vec: Vector, basis: list[tuple[str, Vector]]) -> Vector:
    """Reduce ``vec`` against an echelon ``basis`` of (pivot_var, pivot_vector)."""
    vec = dict(vec)
    for pivot, bvec in basis:
        coeff = vec.get(pivot, Fraction(0))
        if coeff != 0:
            factor = coeff / bvec[pivot]
            for k, v in bvec.items():
                vec[k] = vec.get(k, Fraction(0)) - factor * v
    return _clean(vec)


def _in_span(target: Vector, sources: list[Vector]) -> bool:
    """Exact test: is ``target`` in the linear span of ``sources`` (over Q)?

    Builds a reduced echelon basis of the sources by Gaussian elimination over
    :class:`fractions.Fraction`, then reduces ``target`` against it; ``target`` is
    spanned iff it reduces to the zero vector.
    """
    basis: list[tuple[str, Vector]] = []
    for src in sources:
        residual = _reduce(src, basis)
        if residual:
            pivot = next(iter(residual))  # deterministic: dict preserves order
            basis.append((pivot, residual))
    return not _reduce(target, basis)


def minimal_support(sources: list[Vector], target: Vector) -> list[int] | None:
    """Exact **minimal-support solve**: smallest subset of ``sources`` spanning ``target``.

    Returns the indices of a minimum-cardinality subset ``S`` with
    ``target in span(S)`` (AlphaGeometry v1's ``Table.why`` idea, done exactly
    over the rationals instead of via ``linprog``). ``None`` if ``target`` is not
    entailed by all sources together. Deterministic: subsets are tried in
    increasing size, then lexicographic index order, so the first genuine minimal
    subset is returned.
    """
    if not _in_span(target, sources):
        return None
    n = len(sources)
    for size in range(0, n + 1):
        for combo in combinations(range(n), size):
            if _in_span(target, [sources[i] for i in combo]):
                return list(combo)
    return []  # unreachable (empty target handled by size 0)


# --------------------------------------------------------------------------- #
# Provenance helpers over geometry.py's canonical facts.
# --------------------------------------------------------------------------- #
def _fact_points(fact: Fact) -> set[str]:
    """The set of point names a fact mentions (its footprint contribution)."""
    return set(geometry_synth._fact_to_goal(fact)["points"])


def _proof_steps(order: list[Fact],
                 reasons: dict[Fact, tuple[str, list[Fact]]]) -> list[dict]:
    """Render an ordered list of derived facts as JSON proof steps."""
    return [{"fact": geometry._describe(f), "rule": reasons[f][0],
             "from": sorted(geometry._describe(p) for p in reasons[f][1])}
            for f in order]


def _closure_from_premises(premises: list[dict]) -> tuple[
        dict[Fact, dict], set[Fact], dict[Fact, tuple[str, list[Fact]]]]:
    """Canonicalize ``premises`` and forward-chain to closure.

    Returns ``(premise_map, facts, reasons)`` where ``premise_map`` maps each
    premise fact-key back to its original ``{pred, points}`` dict.
    """
    premise_map = {geometry_synth._canon(p): p for p in premises}
    facts, reasons = geometry_synth._closure(set(premise_map))
    return premise_map, facts, reasons


# --------------------------------------------------------------------------- #
# Item 2: minimal-dependency traceback.
# --------------------------------------------------------------------------- #
def traceback(goal: dict[str, Any], premises: list[dict[str, Any]]) -> dict[str, Any]:
    """Trace ``goal`` back to its minimal premise subgraph and emit the proof.

    Strategy:
      * Forward-chain ``premises`` to closure (geometry.py's own rules) so ``goal``
        (if derivable) is realized with a recorded DAG.
      * If ``goal`` is an equality-style fact, model every equality premise as an
        exact rational equation and run :func:`minimal_support` to find the
        minimum subset that still entails the goal -- dropping redundant premises
        that the raw DAG might have chained through.
      * Otherwise use the DAG's own leaf set (already minimal for this rule set).
      * Re-prove the goal from the minimal premises via
        :func:`geometry.deductive_prove` (soundness gate) and return that ordered
        derivation.

    Returns ``{proved, goal, minimal_premises, dropped_premises, proof, proof_len,
    method, entails}``. When the goal is a bare premise, ``minimal_premises`` is
    just that premise and ``proof`` is empty.
    """
    goal_fact = geometry_synth._canon(goal)
    premise_map, facts, reasons = _closure_from_premises(premises)

    if goal_fact not in facts:
        return {"proved": False, "goal": goal, "minimal_premises": [],
                "dropped_premises": list(premises), "proof": None,
                "proof_len": 0, "method": None, "entails": False}

    method = "dd-traceback"
    minimal_keys: list[Fact]

    equation = _fact_equation(goal_fact)
    if equation is not None:
        # Linear (AR-style) minimal-support solve over equality premises.
        lin = [(k, _fact_equation(k)) for k in premise_map]
        lin = [(k, e) for k, e in lin if e is not None]
        support = minimal_support([e for _, e in lin], equation)
        if support is not None:
            minimal_keys = [lin[i][0] for i in support]
            method = "ar-minimal-support"
        else:
            _order, leaves = geometry_synth._traceback(goal_fact, reasons)
            minimal_keys = list(leaves)
    else:
        _order, leaves = geometry_synth._traceback(goal_fact, reasons)
        minimal_keys = list(leaves)

    minimal = [premise_map[k] for k in
               sorted(minimal_keys, key=geometry._describe) if k in premise_map]
    minimal_key_set = {geometry_synth._canon(p) for p in minimal}
    dropped = [p for p in premises
               if geometry_synth._canon(p) not in minimal_key_set]

    # Soundness gate: re-prove from the minimal premises alone.
    check = geometry.deductive_prove(minimal, goal)
    if not check.get("proved") and method == "ar-minimal-support":
        # Fall back to the raw DAG leaves if the linear minimum is not
        # reconstructible by the rule chainer (should not happen for our rules).
        _order, leaves = geometry_synth._traceback(goal_fact, reasons)
        minimal = [premise_map[k] for k in sorted(leaves, key=geometry._describe)]
        minimal_key_set = {geometry_synth._canon(p) for p in minimal}
        dropped = [p for p in premises
                   if geometry_synth._canon(p) not in minimal_key_set]
        method = "dd-traceback"
        check = geometry.deductive_prove(minimal, goal)

    proof = check.get("derivation") or []
    return {
        "proved": True,
        "goal": goal,
        "minimal_premises": minimal,
        "dropped_premises": dropped,
        "proof": proof,
        "proof_len": len(proof),
        "method": method,
        "entails": bool(check.get("proved")),
    }


# --------------------------------------------------------------------------- #
# Item 5: check_good requires-auxiliary detector (clean-room reimplementation).
# --------------------------------------------------------------------------- #
def _footprints(fact: Fact, reasons: dict[Fact, tuple[str, list[Fact]]]
                ) -> tuple[set[str], set[str], list[Fact], set[Fact]]:
    """Statement and proof footprints of a derived ``fact``.

    * statement footprint = the points the fact itself names.
    * proof footprint = every point over the fact's minimal-dependency subgraph
      (its leaf premises and every intermediate derived fact, plus itself).
    """
    order, leaves = geometry_synth._traceback(fact, reasons)
    statement = _fact_points(fact)
    proof = set(statement)
    for node in order:
        proof |= _fact_points(node)
    for leaf in leaves:
        proof |= _fact_points(leaf)
    return statement, proof, order, leaves


def check_good(premises: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Flag every derived fact whose proof footprint strictly exceeds its statement.

    Such a fact provably required an auxiliary construction (a point it never
    names). Returns one record per flagged fact::

        {"goal", "aux_points", "statement_footprint", "proof_footprint",
         "proof", "proof_len", "used_premises", "requires_aux": True,
         "verified": True}

    ``aux_points = proof_footprint - statement_footprint`` is the *known*
    auxiliary answer. Facts whose proof stays within their statement points
    (e.g. a midpoint's equal segments) are not returned.
    """
    premise_map, facts, reasons = _closure_from_premises(premises)
    results: list[dict[str, Any]] = []

    for fact in sorted((f for f in facts if f in reasons), key=geometry._describe):
        statement, proof_fp, order, leaves = _footprints(fact, reasons)
        if not statement < proof_fp:   # strict subset => auxiliary required
            continue
        goal = geometry_synth._fact_to_goal(fact)
        used = [premise_map[l] for l in sorted(leaves, key=geometry._describe)
                if l in premise_map]
        # Soundness gate: the goal must re-prove from its own traceback leaves.
        check = geometry.deductive_prove(used, goal)
        results.append({
            "goal": goal,
            "aux_points": sorted(proof_fp - statement),
            "statement_footprint": sorted(statement),
            "proof_footprint": sorted(proof_fp),
            "proof": _proof_steps(order, reasons),
            "proof_len": len(order),
            "used_premises": used,
            "requires_aux": True,
            "verified": bool(check.get("proved")),
        })
    return results


# --------------------------------------------------------------------------- #
# harvest: aux-labelled examples over a seeded batch of sampled constructions.
# --------------------------------------------------------------------------- #
def _numeric_ok(construction: list[dict], goal: dict, seed: int) -> bool | None:
    try:
        res = geometry.numeric_check(construction, goal, seed=(seed * 2 + 1),
                                     trials=_NUMERIC_TRIALS)
        return bool(res.get("holds"))
    except Exception:
        return None


def harvest_seed(seed: int) -> list[dict[str, Any]]:
    """All requires-aux examples from the construction sampled at ``seed``.

    Reuses :func:`geometry_synth._sample_construction` for a deterministic diagram,
    runs :func:`check_good`, and attaches grounding (``construction``, ``seed``,
    ``numeric_ok``) to every flagged fact.
    """
    construction, premises = geometry_synth._sample_construction(seed)
    out: list[dict[str, Any]] = []
    for rec in check_good(premises):
        example = dict(rec)
        example["seed"] = seed
        example["all_premises"] = premises
        example["construction"] = construction
        example["numeric_ok"] = _numeric_ok(construction, example["goal"], seed)
        out.append(example)
    return out


def harvest(seed: int, n: int) -> list[dict[str, Any]]:
    """Aux-labelled examples over seeds ``seed, seed+1, ..., seed+n-1``."""
    if n < 0:
        raise ValueError("harvest size n must be non-negative")
    examples: list[dict[str, Any]] = []
    for i in range(n):
        examples.extend(harvest_seed(seed + i))
    return examples


# --------------------------------------------------------------------------- #
# Worker entrypoint. Wire in worker.py as tool "geometry_synth2".
# --------------------------------------------------------------------------- #
def run(request: dict[str, Any]) -> dict[str, Any]:
    """Worker entrypoint. ``request["op"]`` selects the capability.

    * ``traceback``  -> :func:`traceback` (``goal``, ``premises``). Returns the
      minimal premise subgraph + ordered proof.
    * ``check_good`` -> :func:`check_good`. Input is either ``premises`` directly
      or a ``seed`` (a construction is then sampled via geometry_synth).
      Returns ``{"op": "check_good", "count", "examples"}``.
    * ``harvest``    -> :func:`harvest` (``seed``, ``n``). Returns
      ``{"op": "harvest", "count", "examples"}``.

    A ``goal``/``premise`` is ``{"pred": <name>, "points": [<names>...]}``.
    """
    op = request.get("op", "check_good")

    if op == "traceback":
        return {"op": "traceback",
                **traceback(request["goal"], request.get("premises", []))}

    if op == "check_good":
        if "premises" in request:
            examples = check_good(request["premises"])
        else:
            examples = harvest_seed(int(request["seed"]))
        return {"op": "check_good", "count": len(examples), "examples": examples}

    if op == "harvest":
        examples = harvest(int(request["seed"]), int(request["n"]))
        return {"op": "harvest", "count": len(examples), "examples": examples}

    raise ValueError(f"unknown op: {op!r}")
