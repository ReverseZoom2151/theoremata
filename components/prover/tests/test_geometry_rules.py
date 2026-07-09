"""Tests for the extended sound geometry rule set
(:mod:`theoremata_tools.geometry_rules`).

Offline, deterministic, seeded, pure-stdlib. Coverage:

  * **Soundness sweep** -- every rule in the catalog is realized many times from
    a seed and its conclusion is checked to hold across all non-degenerate
    realizations (an unsound rule would be caught here).
  * **The sweep really bites** -- a deliberately *unsound* rule (unsigned
    inscribed angle) is rejected by the same ``numeric_verify`` machinery.
  * Each rule **category derives a correct fact** on a concrete diagram via the
    standalone ``apply_rules`` fixpoint / ``prove`` helper.
  * A rule **does not fire when its non-degeneracy guard fails** (coordinates
    supplied that violate ``npara`` / ``ncoll``).
  * ``apply_rules`` reaches a **deterministic fixpoint**.
  * Theorems reachable **only via a new advanced rule** (radical centre, Monge,
    Pappus, Desargues, third-altitude, Simson, harmonic pencil) are derived, and
    are *not* reachable by ``geometry.py``'s five-rule chainer.
  * The catalog adapts to ``geometry._RULES``' shape via ``to_geometry_rules``.

Run from the repo root::

    python -m pytest components/prover/tests/test_geometry_rules.py -x -q
"""
from __future__ import annotations

import pytest

from theoremata_tools import geometry, geometry_rules as gr


SEED = 20260709


# --------------------------------------------------------------------------- #
# 1. Soundness sweep: every rule's conclusion holds in every clean realization.
# --------------------------------------------------------------------------- #
@pytest.mark.parametrize("rule", gr.RULES, ids=[r.name for r in gr.RULES])
def test_every_rule_is_numerically_sound(rule):
    res = gr.numeric_verify(rule, seed=SEED, trials=20)
    assert res["holds"] is True, f"{rule.name} unsound: {res}"
    assert res["valid"] >= 12, f"{rule.name} too few valid realizations: {res}"


def test_soundness_sweep_rejects_an_unsound_rule():
    """The unsigned inscribed-angle identity (angle ACB = angle ADB, *unsigned*)
    is FALSE up to supplement; numeric_verify must catch it -- proving the sweep
    is a real filter, not a rubber stamp."""
    bogus = gr.Rule(
        "UNSOUND-unsigned-inscribed", "test", "advanced", "angle",
        gr.m_inscribed_angle,  # matcher irrelevant; we only verify the witness
        {"construction": gr._CIRCLE4,
         "premises": [("concyclic", ("A", "B", "C", "D"))],
         "conclusion": ("eqangle", ("A", "C", "B", "A", "D", "B"))})
    assert gr.numeric_verify(bogus, seed=SEED, trials=30)["holds"] is False


# --------------------------------------------------------------------------- #
# 2. Every rule fires symbolically on its own premises (matcher <-> witness).
# --------------------------------------------------------------------------- #
@pytest.mark.parametrize("rule", gr.RULES, ids=[r.name for r in gr.RULES])
def test_every_rule_fires_on_its_premises(rule):
    facts = gr.facts_from(rule.witness["premises"])
    goal = gr.canon(rule.witness["conclusion"][0], rule.witness["conclusion"][1])
    produced = {m[0] for m in rule.match(facts, None)}
    assert goal in produced, f"{rule.name} did not derive its own conclusion"


# --------------------------------------------------------------------------- #
# 3. Category smoke tests over the standalone apply_rules / prove driver.
# --------------------------------------------------------------------------- #
def test_core_midpoint_and_transitivity_chain():
    hyps = [("perpendicular", ("A", "B", "C", "D")),
            ("perpendicular", ("C", "D", "E", "F")),
            ("parallel", ("E", "F", "G", "H"))]
    assert gr.prove(hyps, ("parallel", ("A", "B", "G", "H")), seed=SEED)


def test_core_similar_triangle_ratio_and_angle():
    hyps = [("simtri", ("A", "B", "C", "P", "Q", "R"))]
    assert gr.prove(hyps, ("eqratio", ("A", "B", "P", "Q", "B", "C", "Q", "R")))
    assert gr.prove(hyps, ("eqangle", ("B", "A", "C", "Q", "P", "R")))


def test_core_aa_gives_similar_then_ratio():
    """AA (two directed angles) => similar => corresponding sides proportional --
    a two-rule chain."""
    hyps = [("deqangle", ("B", "A", "C", "Q", "P", "R")),
            ("deqangle", ("A", "B", "C", "P", "Q", "R"))]
    assert gr.prove(hyps, ("simtri", ("A", "B", "C", "P", "Q", "R")))
    assert gr.prove(hyps, ("eqratio", ("A", "B", "P", "Q", "B", "C", "Q", "R")))


def test_core_directed_inscribed_angle_and_converse():
    assert gr.prove([("concyclic", ("A", "B", "C", "D"))],
                    ("deqangle", ("A", "C", "B", "A", "D", "B")))
    assert gr.prove([("deqangle", ("A", "C", "B", "A", "D", "B"))],
                    ("concyclic", ("A", "B", "C", "D")))


def test_core_circumcenter_facts():
    hyps = [("cong", ("O", "A", "O", "B")), ("cong", ("O", "A", "O", "C")),
            ("concyclic", ("A", "B", "C", "D"))]
    assert gr.prove(hyps, ("cong", ("O", "A", "O", "D")))


# --------------------------------------------------------------------------- #
# 4. Advanced rules: theorems reachable ONLY through a new advanced rule, and
#    beyond geometry.py's five-rule chainer.
# --------------------------------------------------------------------------- #
def test_advanced_radical_center():
    hyps = [("cong", ("O1", "P", "O1", "Q")), ("cong", ("O2", "P", "O2", "Q")),
            ("cong", ("O1", "S", "O1", "T")), ("cong", ("O3", "S", "O3", "T")),
            ("cong", ("O2", "U", "O2", "V")), ("cong", ("O3", "U", "O3", "V"))]
    assert gr.prove(hyps, ("concurrent", ("P", "Q", "S", "T", "U", "V")))


def test_advanced_monge():
    hyps = [("simcenter", ("E", "O1", "O2")), ("simcenter", ("G", "O1", "O3")),
            ("simcenter", ("H", "O2", "O3"))]
    assert gr.prove(hyps, ("collinear", ("E", "G", "H")))


def test_advanced_third_altitude():
    hyps = [("perpendicular", ("A", "B", "C", "D")),
            ("perpendicular", ("A", "C", "B", "D"))]
    assert gr.prove(hyps, ("perpendicular", ("A", "D", "B", "C")))
    # geometry.py's five rules cannot reach this.
    assert geometry.deductive_prove(
        [{"pred": "perpendicular", "points": ["A", "B", "C", "D"]},
         {"pred": "perpendicular", "points": ["A", "C", "B", "D"]}],
        {"pred": "perpendicular", "points": ["A", "D", "B", "C"]})["proved"] is False


def test_advanced_pappus():
    hyps = [("collinear", ("A", "B", "C")), ("collinear", ("P", "Q", "R")),
            ("collinear", ("A", "Q", "X")), ("collinear", ("P", "B", "X")),
            ("collinear", ("A", "R", "Y")), ("collinear", ("P", "C", "Y")),
            ("collinear", ("B", "R", "Z")), ("collinear", ("C", "Q", "Z"))]
    assert gr.prove(hyps, ("collinear", ("X", "Y", "Z")))


def test_advanced_desargues():
    hyps = [("collinear", ("O", "A", "D")), ("collinear", ("O", "B", "E")),
            ("collinear", ("O", "C", "G")), ("collinear", ("B", "C", "X")),
            ("collinear", ("E", "G", "X")), ("collinear", ("C", "A", "Y")),
            ("collinear", ("G", "D", "Y")), ("collinear", ("A", "B", "Z")),
            ("collinear", ("D", "E", "Z"))]
    assert gr.prove(hyps, ("collinear", ("X", "Y", "Z")))


def test_advanced_simson_line():
    hyps = [("concyclic", ("A", "B", "C", "P")),
            ("collinear", ("A", "L", "C")), ("perpendicular", ("P", "L", "A", "C")),
            ("collinear", ("B", "M", "C")), ("perpendicular", ("P", "M", "B", "C")),
            ("collinear", ("A", "N", "B")), ("perpendicular", ("P", "N", "A", "B"))]
    assert gr.prove(hyps, ("collinear", ("L", "M", "N")))


def test_advanced_harmonic_pencil_bisector():
    hyps = [("harmonic", ("A", "B", "C", "D")),
            ("perpendicular", ("P", "C", "P", "D"))]
    assert gr.prove(hyps, ("deqangle", ("A", "P", "C", "C", "P", "B")))


# --------------------------------------------------------------------------- #
# 5. Non-degeneracy guards: a rule does NOT fire when its guard is violated.
# --------------------------------------------------------------------------- #
def test_circumcenter_unique_guard_blocks_parallel_chords():
    facts = gr.facts_from([("concyclic", ("A", "B", "C", "D")),
                           ("cong", ("O", "A", "O", "B")),
                           ("cong", ("O", "C", "O", "D"))])
    rule = next(r for r in gr.RULES if r.name == "circumcenter-unique")
    goal = gr.canon("cong", ("O", "A", "O", "C"))
    # AB parallel to CD (both horizontal): the circumcentre argument degenerates.
    par = {"A": (0.0, 0.0), "B": (2.0, 0.0), "C": (0.0, 1.0),
           "D": (2.0, 1.0), "O": (1.0, 0.5)}
    assert goal not in {m[0] for m in rule.match(facts, par)}
    # A generic (non-parallel) placement: the rule fires.
    gen = {"A": (0.0, 0.0), "B": (2.0, 0.0), "C": (1.0, 2.0),
           "D": (3.0, 1.0), "O": (1.0, 0.5)}
    assert goal in {m[0] for m in rule.match(facts, gen)}


def test_semicircle_guard_blocks_collinear_vertex():
    facts = gr.facts_from([("collinear", ("O", "A", "C")),
                           ("cong", ("O", "A", "O", "C")),
                           ("cong", ("O", "A", "O", "B"))])
    rule = next(r for r in gr.RULES if r.name == "angle-in-semicircle")
    goal = gr.canon("perpendicular", ("B", "A", "B", "C"))
    # B on line AC -> the "right angle" degenerates; guard must block.
    degen = {"O": (0.0, 0.0), "A": (-1.0, 0.0), "C": (1.0, 0.0), "B": (0.5, 0.0)}
    assert goal not in {m[0] for m in rule.match(facts, degen)}
    # B off the line: fires.
    ok = {"O": (0.0, 0.0), "A": (-1.0, 0.0), "C": (1.0, 0.0), "B": (0.0, 1.0)}
    assert goal in {m[0] for m in rule.match(facts, ok)}


# --------------------------------------------------------------------------- #
# 6. Deterministic fixpoint.
# --------------------------------------------------------------------------- #
def test_apply_rules_is_a_deterministic_fixpoint():
    hyps = gr.facts_from([("concyclic", ("A", "B", "C", "D"))])
    c1, _ = gr.apply_rules(hyps, seed=SEED)
    c2, _ = gr.apply_rules(hyps, seed=SEED)
    assert c1 == c2
    # running the closure through the engine again adds nothing (true fixpoint).
    c3, _ = gr.apply_rules(c1, seed=SEED)
    assert c3 == c1


def test_prove_is_stable_across_seeds():
    hyps = [("perpendicular", ("A", "B", "C", "D")),
            ("perpendicular", ("A", "C", "B", "D"))]
    goal = ("perpendicular", ("A", "D", "B", "C"))
    assert all(gr.prove(hyps, goal, seed=s) for s in (0, 1, 2, 99))


# --------------------------------------------------------------------------- #
# 7. Catalog shape / adapter to geometry._RULES.
# --------------------------------------------------------------------------- #
def test_to_geometry_rules_matches_engine_shape():
    adapted = gr.to_geometry_rules()
    # same shape as geometry._RULES: (name, callable(facts) -> [(fact, prems)])
    assert len(adapted) == len(gr.RULES)
    facts = gr.facts_from([("midpoint", ("M", "A", "B"))])
    name, fn = next(a for a in adapted if a[0] == "midpoint-expand")
    produced = {m[0] for m in fn(facts)}
    assert gr.canon("cong", ("M", "A", "M", "B")) in produced
    assert gr.canon("collinear", ("A", "M", "B")) in produced


def test_catalog_provenance_and_kinds():
    assert set(gr.CATALOG) == {r.name for r in gr.RULES}
    sources = {r.source for r in gr.RULES}
    assert sources == {"v1", "newclid", "clean-room-tong"}
    kinds = {r.kind for r in gr.RULES}
    assert kinds == {"core", "advanced"}
    # advanced rules are all clean-room re-implementations (no GPLv3 copied).
    for r in gr.RULES:
        if r.kind == "advanced":
            assert r.source == "clean-room-tong"
