"""AlphaGeometry-style synthetic-data engine over our sound geometry vertical.

This module mints *unlimited supervised training examples* for the flywheel with
**no human problems**, following AlphaGeometry's four-ingredient recipe (Trinh et
al., Nature 2024) applied to the small, sound reasoning core in
:mod:`theoremata_tools.geometry`:

1. **Premise sampler** (:func:`_sample_construction`). Given a caller-supplied
   ``seed`` (host has no ``Math.random``; entropy is passed in), assemble a random
   but *valid* geometric construction from a menu of gadgets. Each gadget appends
   construction steps in geometry.py's construction language (``free``, ``foot``,
   ``midpoint``, ``reflect_point``, ``reflect_line``) *and* the symbolic premises
   that hold *by construction* (a foot gives ``perpendicular``; a parallelogram
   built via midpoint+reflection gives ``parallel``; equal radii give ``cong``;
   ...). Because the coordinates are built to satisfy every premise, the premise
   set is jointly realizable -- never self-contradictory.

2. **Deductive closure** (:func:`_closure`). Run geometry.py's *own* sound
   forward-chaining rule set (``geometry._RULES``) to fixpoint, recording the
   derivation DAG: for every derived fact we store the rule that produced it and
   its parent facts (``reasons``). Soundness lives entirely in geometry.py -- we
   only drive its rules; we never invent one.

3. **Goal selection** (:func:`_select_goal`). Pick a non-trivial *derived* fact
   (never a bare hypothesis) as the goal, deterministically from the seed.

4. **Traceback + dependency difference** (:func:`_traceback`, :func:`_build`).
   Trace the goal back through ``reasons`` to its minimal leaf set -- the premises
   the proof actually needs (the *proof-dependency set*). The premises whose
   points all appear in the goal's statement are the *statement-dependency set*;
   the leftover used premises -- those mentioning a point the goal never names --
   are the **auxiliary constructions** (the holes). These are moved out of the
   given premises and into ``aux_holes``: the free supervised signal for a
   sketch-hole-filler, exactly AlphaGeometry's dependency difference.

Every emitted example is *verified*: :func:`geometry.deductive_prove` re-proves
the goal from ``premises + aux_holes`` (public-API soundness gate), and (when a
numeric realization is available) :func:`geometry.numeric_check` confirms the
goal holds across random diagrams.

Training-example schema (all fields JSON-serializable)::

    {
      "seed":            int,                 # the seed that produced it
      "prefer":          "aux"|"no_aux"|...,  # goal-selection policy used
      "goal":            {"pred", "points"},  # the selected derived fact
      "premises":        [{"pred","points"}], # GIVEN premises (statement-dependency)
      "aux_holes":       [{"pred","points"}], # dependency difference = auxiliary
      "proof":           [{"fact","rule","from"}],  # ordered derivation (DAG topo)
      "proof_len":       int,                 # number of derivation steps
      "used_premises":   [{"pred","points"}], # proof-dependency set (premises+aux)
      "all_premises":    [{"pred","points"}], # every sampled premise (the diagram)
      "construction":    [ <geometry step> ], # grounding coordinates recipe
      "numeric_ok":      bool|None,           # goal verified numerically (or None)
      "verified":        True,                # geometry.deductive_prove re-proved it
    }

``run(request)`` ops:
    * ``sample`` -- one seeded example (``seed``; optional ``prefer``).
    * ``batch``  -- ``n`` seeded examples with seeds ``seed, seed+1, ...``.

Honest scope (bounded by geometry.py's small sound rule set)
------------------------------------------------------------
CAN mint: (a) *no-auxiliary* examples -- a midpoint yields equal segments /
collinearity whose only premise is inside the goal; (b) *auxiliary-construction*
examples -- a pencil of perpendiculars to a hidden common line, a parallelogram
bridge, or an equal-radius hub, where the bridge object never appears in the goal
and is therefore a genuine hole; (c) shallow multi-step proofs (perp-perp=>||
then ||-transitivity).

CANNOT mint: anything outside geometry.py's five rules -- no angle chasing, ratio
/ similar-triangle, power-of-a-point, or algebraic (AR/Gaussian-elimination)
reasoning; no MILP/BFS *minimal* traceback (our traceback is a straightforward
minimal-leaf trace, not AG's NP-hard minimum-spanning-tree/MILP pruning). Proofs
are therefore short (typically 1-2 steps) and auxiliary examples tend to be
"fully auxiliary" (the entire bridge is the hole, so ``premises`` can be empty) --
because for a relation among otherwise-free points to be *true by construction*
in this engine, the bridge must be built from those points, and the derived
fact's statement never needs to name it. Richer, mixed premise/aux examples need
a richer rule set (DD+AR) than geometry.py currently exposes.

Pure standard library (+ geometry.py, itself stdlib-only); offline & deterministic.
"""
from __future__ import annotations

import random
from typing import Any, Callable

from theoremata_tools import geometry

Fact = tuple[Any, ...]

_MAX_ROUNDS = 32
_NUMERIC_TRIALS = 24


# --------------------------------------------------------------------------- #
# Canonicalization helpers (thin wrappers around geometry.py's public-in-spirit
# internals so soundness/canonical form stays defined in ONE place).
# --------------------------------------------------------------------------- #
def _canon(premise: dict[str, Any]) -> Fact:
    return geometry._canonical(premise["pred"], list(premise["points"]))


def _describe(fact: Fact) -> str:
    return geometry._describe(fact)


def _fact_to_goal(fact: Fact) -> dict[str, Any]:
    """Inverse of ``geometry._canonical``: a concrete ``{pred, points}`` goal.

    The chosen point order round-trips through ``geometry._canonical`` back to
    ``fact`` (every predicate here is symmetric/set-based, so any consistent
    order is fine); we sort for determinism.
    """
    kind = fact[0]
    if kind in ("collinear", "concyclic"):
        return {"pred": kind, "points": sorted(fact[1])}
    if kind == "midpoint":
        a, b = sorted(fact[2])
        return {"pred": "midpoint", "points": [fact[1], a, b]}
    if kind in ("cong", "parallel", "perpendicular"):
        segs = sorted(fact[1], key=lambda s: sorted(s))
        s1, s2 = sorted(segs[0]), sorted(segs[1])
        return {"pred": kind, "points": [s1[0], s1[1], s2[0], s2[1]]}
    if kind == "eqangle":
        angs = sorted(fact[1], key=lambda a: (a[0], sorted(a[1])))
        (v1, r1), (v2, r2) = angs
        r1, r2 = sorted(r1), sorted(r2)
        return {"pred": "eqangle",
                "points": [r1[0], v1, r1[1], r2[0], v2, r2[1]]}
    raise ValueError(f"cannot invert fact of kind {kind!r}")


# --------------------------------------------------------------------------- #
# Ingredient 2: deductive closure over geometry.py's OWN sound rule set.
# --------------------------------------------------------------------------- #
def _closure(premise_facts: set[Fact]) -> tuple[set[Fact], dict[Fact, tuple[str, list[Fact]]]]:
    """Forward-chain ``geometry._RULES`` to fixpoint, recording the DAG.

    ``reasons[f] = (rule_name, [parent_facts])`` for every *derived* fact.
    Rule outputs are inserted in a deterministic order so ``reasons`` (the
    first-writer for a multiply-derivable fact) does not depend on set-iteration
    order across processes.
    """
    facts: set[Fact] = set(premise_facts)
    reasons: dict[Fact, tuple[str, list[Fact]]] = {}
    for _ in range(_MAX_ROUNDS):
        added = False
        for rule_name, rule in geometry._RULES:
            produced = rule(facts)
            produced.sort(key=lambda np: (_describe(np[0]),
                                          tuple(_describe(p) for p in np[1])))
            for new_fact, premises in produced:
                if new_fact not in facts:
                    facts.add(new_fact)
                    reasons[new_fact] = (rule_name, list(premises))
                    added = True
        if not added:
            break
    return facts, reasons


# --------------------------------------------------------------------------- #
# Ingredient 4: traceback (minimal dependency subgraph of a goal).
# --------------------------------------------------------------------------- #
def _traceback(goal: Fact, reasons: dict[Fact, tuple[str, list[Fact]]]
               ) -> tuple[list[Fact], set[Fact]]:
    """Return (ordered derived facts, leaf premise facts) for ``goal``.

    ``order`` is a topological listing (parents before children) of only the
    *derived* nodes; ``leaves`` are the facts with no reason -- the original
    premises the proof genuinely depends on (the proof-dependency set).
    """
    order: list[Fact] = []
    leaves: set[Fact] = set()
    seen: set[Fact] = set()

    def emit(f: Fact) -> None:
        if f in seen:
            return
        seen.add(f)
        if f in reasons:
            for p in sorted(reasons[f][1], key=_describe):
                emit(p)
            order.append(f)
        else:
            leaves.add(f)

    emit(goal)
    return order, leaves


# --------------------------------------------------------------------------- #
# Ingredient 1: the premise sampler -- a menu of by-construction-sound gadgets.
# Each gadget appends coordinate steps AND the symbolic premises they realize.
# --------------------------------------------------------------------------- #
def _namer(seed: int) -> Callable[[], str]:
    """Fresh point-name generator; names embed the seed so different seeds
    produce disjoint (hence distinct) examples while a fixed seed is stable."""
    counter = [0]

    def nm() -> str:
        name = f"g{seed}_{counter[0]}"
        counter[0] += 1
        return name

    return nm


def _g_midpoint(rng: random.Random, nm: Callable[[], str],
                cons: list[dict], prem: list[dict]) -> None:
    """M = midpoint(A,B). Closure => cong(MA,MB), collinear(A,M,B).
    Both derived facts live entirely inside the goal's points => NO auxiliary."""
    a, b, m = nm(), nm(), nm()
    cons += [{"op": "free", "point": a}, {"op": "free", "point": b},
             {"op": "midpoint", "point": m, "of": [a, b]}]
    prem.append({"pred": "midpoint", "points": [m, a, b]})


def _g_perp_pencil(rng: random.Random, nm: Callable[[], str],
                   cons: list[dict], prem: list[dict]) -> None:
    """A pencil of perpendiculars to a common (hidden) base line PQ.
    Two feet-segments perpendicular to PQ are parallel; PQ is the auxiliary
    construction (it never appears in the goal parallel(R1F1, R2F2))."""
    p, q = nm(), nm()
    cons += [{"op": "free", "point": p}, {"op": "free", "point": q}]
    for _ in range(rng.randint(2, 3)):
        r, f = nm(), nm()
        cons += [{"op": "free", "point": r},
                 {"op": "foot", "point": f, "from": r, "of": [p, q]}]
        prem.append({"pred": "perpendicular", "points": [r, f, p, q]})


def _g_parallel_chain(rng: random.Random, nm: Callable[[], str],
                      cons: list[dict], prem: list[dict]) -> None:
    """Two parallelograms sharing an edge CD (built via midpoint+central
    reflection so D = B+C-A, F = D+E-C). AB || CD || EF, so AB || EF by
    transitivity; the bridge CD is the auxiliary construction."""
    a, b, c = nm(), nm(), nm()
    m1, d = nm(), nm()
    cons += [{"op": "free", "point": a}, {"op": "free", "point": b},
             {"op": "free", "point": c},
             {"op": "midpoint", "point": m1, "of": [b, c]},
             {"op": "reflect_point", "point": d, "of": a, "center": m1}]
    prem.append({"pred": "parallel", "points": [a, b, c, d]})
    e, m2, f = nm(), nm(), nm()
    cons += [{"op": "free", "point": e},
             {"op": "midpoint", "point": m2, "of": [d, e]},
             {"op": "reflect_point", "point": f, "of": c, "center": m2}]
    prem.append({"pred": "parallel", "points": [c, d, e, f]})


def _g_cong_hub(rng: random.Random, nm: Callable[[], str],
                cons: list[dict], prem: list[dict]) -> None:
    """Equal-radius hub: B, C are reflections of a radius-source A across lines
    through centre O, so |OB| = |OA| = |OC|. Hence |OB| = |OC|; the source A is
    the auxiliary construction (absent from goal cong(OB, OC))."""
    o, a, l1, l2 = nm(), nm(), nm(), nm()
    b, c = nm(), nm()
    cons += [{"op": "free", "point": o}, {"op": "free", "point": a},
             {"op": "free", "point": l1}, {"op": "free", "point": l2},
             {"op": "reflect_line", "point": b, "of": a, "over": [o, l1]},
             {"op": "reflect_line", "point": c, "of": a, "over": [o, l2]}]
    prem.append({"pred": "cong", "points": [o, a, o, b]})
    prem.append({"pred": "cong", "points": [o, a, o, c]})


def _g_perp_then_parallel(rng: random.Random, nm: Callable[[], str],
                          cons: list[dict], prem: list[dict]) -> None:
    """A genuine TWO-step proof: R1F1 _|_ PQ and R2F2 _|_ PQ give R1F1 || R2F2
    (step 1); a parallelogram makes GH || R2F2 (premise); transitivity gives
    R1F1 || GH (step 2). Everything but R1,F1,G,H is auxiliary."""
    p, q = nm(), nm()
    r1, f1, r2, f2 = nm(), nm(), nm(), nm()
    cons += [{"op": "free", "point": p}, {"op": "free", "point": q},
             {"op": "free", "point": r1},
             {"op": "foot", "point": f1, "from": r1, "of": [p, q]},
             {"op": "free", "point": r2},
             {"op": "foot", "point": f2, "from": r2, "of": [p, q]}]
    prem.append({"pred": "perpendicular", "points": [r1, f1, p, q]})
    prem.append({"pred": "perpendicular", "points": [r2, f2, p, q]})
    g, mid, h = nm(), nm(), nm()
    cons += [{"op": "free", "point": g},
             {"op": "midpoint", "point": mid, "of": [g, f2]},
             {"op": "reflect_point", "point": h, "of": r2, "center": mid}]
    # h = g + f2 - r2  =>  GH vector == R2F2 vector  =>  GH || R2F2.
    prem.append({"pred": "parallel", "points": [r2, f2, g, h]})


_AUX_GADGETS: list[Callable] = [
    _g_perp_pencil, _g_parallel_chain, _g_cong_hub, _g_perp_then_parallel,
]


def _sample_construction(seed: int) -> tuple[list[dict], list[dict]]:
    """Deterministically sample (construction steps, all premises) from ``seed``.

    Always includes a midpoint gadget (so a no-auxiliary derived fact always
    exists) plus 1-2 auxiliary gadgets (so an auxiliary example exists too)."""
    rng = random.Random(seed)
    nm = _namer(seed)
    cons: list[dict] = []
    prem: list[dict] = []
    _g_midpoint(rng, nm, cons, prem)
    for gadget in rng.sample(_AUX_GADGETS, rng.randint(1, 2)):
        gadget(rng, nm, cons, prem)
    return cons, prem


# --------------------------------------------------------------------------- #
# Ingredient 3 + assembly: goal selection and example construction.
# --------------------------------------------------------------------------- #
def _numeric_ok(construction: list[dict], goal: dict, seed: int) -> bool | None:
    try:
        res = geometry.numeric_check(construction, goal, seed=(seed * 2 + 1),
                                     trials=_NUMERIC_TRIALS)
        return bool(res.get("holds"))
    except Exception:
        return None


def _candidate(fact: Fact, reasons, premise_map: dict[Fact, dict]) -> dict[str, Any]:
    """Build the full traceback record for a derived ``fact`` (a goal candidate)."""
    goal = _fact_to_goal(fact)
    goal_points = set(goal["points"])
    order, leaves = _traceback(fact, reasons)
    used = [premise_map[l] for l in sorted(leaves, key=_describe)]
    given = [p for p in used if set(p["points"]) <= goal_points]
    aux = [p for p in used if not set(p["points"]) <= goal_points]
    proof = [{"fact": _describe(f), "rule": reasons[f][0],
              "from": sorted(_describe(p) for p in reasons[f][1])}
             for f in order]
    return {"fact": fact, "goal": goal, "used": used, "given": given,
            "aux": aux, "proof": proof, "proof_len": len(order)}


def _select_goal(candidates: list[dict], prefer: str) -> dict:
    """Deterministically choose one candidate under the ``prefer`` policy."""
    def key(c: dict) -> tuple:
        return (c["proof_len"], len(c["aux"]), _describe(c["fact"]))

    if prefer == "no_aux":
        pool = [c for c in candidates if not c["aux"]] or candidates
    elif prefer == "aux":
        pool = [c for c in candidates if c["aux"]] or candidates
    else:  # "deepest" / "any"
        pool = candidates
    return max(pool, key=key)


def build_example(seed: int, prefer: str = "aux") -> dict[str, Any]:
    """Mint one training example for ``seed`` under selection policy ``prefer``.

    ``prefer`` in {``"aux"``, ``"no_aux"``, ``"deepest"``}. Raises ``ValueError``
    if the sampled construction yields no derivable fact (should not happen: the
    midpoint gadget always derives one)."""
    if prefer not in ("aux", "no_aux", "deepest", "any"):
        raise ValueError(f"unknown prefer policy: {prefer!r}")
    construction, all_premises = _sample_construction(seed)
    premise_map = {_canon(p): p for p in all_premises}
    premise_facts = set(premise_map)
    facts, reasons = _closure(premise_facts)

    derived = sorted((f for f in facts if f in reasons), key=_describe)
    if not derived:
        raise ValueError("no derivable fact from sampled construction")
    candidates = [_candidate(f, reasons, premise_map) for f in derived]
    chosen = _select_goal(candidates, prefer)

    goal = chosen["goal"]
    given, aux, used = chosen["given"], chosen["aux"], chosen["used"]

    # Public-API soundness gate: geometry.py must re-prove the goal from the
    # given premises PLUS the auxiliary holes (i.e. from the proof-dependency
    # set). This is what makes the emitted proof trustworthy.
    check = geometry.deductive_prove(used, goal)
    if not check.get("proved"):
        raise RuntimeError(f"soundness gate failed for seed {seed}: goal not "
                           f"provable from its own traceback leaves")

    return {
        "seed": seed,
        "prefer": prefer,
        "goal": goal,
        "premises": given,
        "aux_holes": aux,
        "proof": chosen["proof"],
        "proof_len": chosen["proof_len"],
        "used_premises": used,
        "all_premises": all_premises,
        "construction": construction,
        "numeric_ok": _numeric_ok(construction, goal, seed),
        "verified": True,
    }


# --------------------------------------------------------------------------- #
# Worker entrypoint.  Wire in worker.py as tool "geometry_synth".
# --------------------------------------------------------------------------- #
def run(request: dict[str, Any]) -> dict[str, Any]:
    """Worker entrypoint. ``request["op"]`` selects the capability.

    * ``sample`` -> one seeded example (``seed``; optional ``prefer``).
      Returns ``{"op": "sample", "example": <example>}``.
    * ``batch``  -> ``n`` examples with seeds ``seed, seed+1, ..., seed+n-1``
      (``seed``, ``n``; optional ``prefer``).
      Returns ``{"op": "batch", "count": n, "examples": [<example>...]}``.
    """
    op = request.get("op", "sample")
    prefer = str(request.get("prefer", "aux"))

    if op == "sample":
        example = build_example(int(request["seed"]), prefer=prefer)
        return {"op": "sample", "example": example}

    if op == "batch":
        base = int(request["seed"])
        n = int(request["n"])
        if n < 0:
            raise ValueError("batch size n must be non-negative")
        examples = [build_example(base + i, prefer=prefer) for i in range(n)]
        return {"op": "batch", "count": n, "examples": examples}

    raise ValueError(f"unknown op: {op!r}")
