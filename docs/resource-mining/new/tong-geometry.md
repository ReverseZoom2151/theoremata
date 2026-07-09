# Resource Mining: TongGeometry (bigai-ai/tong-geometry)

Path: `resources/tong-geometry-main/tong-geometry-main/`
(tarball double-nests one level down).

## What it is

**TongGeometry** is the open-source geometry engine behind *"Proposing and solving
olympiad geometry with guided tree search"* (Zhang et al., **Nature Machine
Intelligence 2026**; tech report arXiv:2412.10673). It is a **from-scratch,
AlphaGeometry-independent** system: the README explicitly states it "was developed
concurrently and independently with AlphaGeometry, as can be seen from the
completely different domain specific language." It both **proposes** olympiad
problems (accepted at National High School Math League Beijing and USEMO) and
**solves** them (all of IMO-AG-30), and is the geometry foundation used inside
**ByteDance Seed-Prover** (IMO-2025 silver). ~14k lines of hand-written Python
deduction rules.

- **License: GNU GPLv3.** Copyleft — **do not copy code verbatim** into Theoremata
  (which is not GPL). Port *ideas, rule statements, algorithms* re-implemented in
  our own code; treat every function below as a design reference, not a snippet to
  paste.
- **Relation to AlphaGeometry:** independent, different DSL, and a **substantially
  richer rule set** (radical axis, Pappus, Desargues, Monge, harmonic bundles,
  similitude centers, isogonal conjugate) — this is the mining prize.

## Architecture / key files

Pipeline: a **constructor DSL** builds a diagram; a **fact-triggered forward
chainer** saturates deductions against a **deductive database** of equivalence
classes; a **"good problem" detector** flags derived facts that require auxiliary
constructions; MCTS / priority random search drives both proposal and solving;
LM policy + a **process-reward classifier** guide the search.

| File | Role |
|------|------|
| `tonggeometry/action.py` | `Action(Constructor, from_names, to_names)` — one construction step; equality ignores `to_names`. |
| `tonggeometry/diagram.py` (1092 L) | `Diagram` = MCTS `Node`. `apply_action` (name/static/runtime/numeric checks → construct → `forward_facts`), `check_num` (numerical soundness per predicate, tol 1e-5), `check_good` (auto problem detection), `prune` (minimal sub-diagram), `trace_fact`/`build_fact`/`score_fact` (proof graph + difficulty), `reward` (MCTS), `GoodProblemTree`. |
| `inference_engine/fc.py` | `one_step_fc`: fact-**triggered** chaining — a new fact of type T fires only rules registered for T (`ALL_RULES[fact.type]`). |
| `inference_engine/database.py` | Equivalence-class DB: transitive relations (cong/para/eqangle/eqratio/simtri/contri/eqline/eqcircle) as representative+class; non-transitive (midp/perp) as element+relation set; rich inverse indices (`inverse_eqline`, `lines_points`, `segments_perps`, `h_segments_perps`, `inverse_midp`); circle/line intersection helpers `itsll`/`itscc`. |
| `inference_engine/primitives.py` | `Segment`, `Angle` (directed, 3-point full angle), `Ratio`, `Triangle`, `Circle` (center + concyclic point set). |
| `inference_engine/predicate.py` | `Predicate/Fact` with `parents` (proof DAG), `dependency` points, `depth`; `OrderedFact` priority queue keyed by predicate type; `string_to_fact`. |
| `inference_engine/rule/*.py` (~7k L) | Deduction rules grouped by **conclusion type**: `eqcircle.py` 1691 L, `eqline.py` 1450 L, `eqangle.py` 912, `eqratio.py` 638, `cong.py` 572, `contri.py` 421, `simtri.py` 407, `midp.py` 395, `perp.py` 247, `para.py` 94. |
| `inference_engine/handler/*.py` | Per-type DB handlers: `add_fact`, `filter` (drop trivial), `known` (query), `fact_key` (canonicalization). |
| `constructor/*.py` (~4.6k L) | Constructor library: `point.py` 1877, `triangle.py` 1035, `circle.py` 503, `line.py` 429, `parallelogram.py`, `parent.py`. |
| `model/solve.py` | LM-guided tree search: small policy (`lm_s`) short-range then large (`lm_l`) long-range + `AutoModelForSequenceClassification` **value/PRM** (`num_labels=5`) predicting `steps_to_go`; rank = `value − weight·steps_to_go`. |
| `scripts/priority_mirror_sym.py` (1797 L) | The **distributed data generator**: symmetric-problem proposal via mirror pairing + priority random search + max-clique good-problem dedup. 10k-CPU / 30-day generation. |
| `tests/cases/case*.txt` (196 cases) | DSL problems: a list of `Action(...)` then a goal `Fact("perp", [Angle(*"DGH")])`. |

## Reusable mechanisms — concrete ports into `geometry_ddar.py` / `geometry_synth.py`

1. **Advanced deduction rules far beyond AlphaGeometry's DDAR.** The rule modules
   implement olympiad machinery AG's 43-rule table lacks. Re-implement these
   (statements, not code) in our chainer:
   - **Radical axis / radical center** (`eqcircle.py`): `add_ax`, `add_radax`,
     `perp_and_eqcircle_to_ax`, `eqcircle_and_eqcircle_to_radax`,
     `eqline_and_radax_to_perp`, `eqangle_and_radax_to_perp`,
     `cong_and_cong_and_ax_to_radax`, `eqratio_and_ax_to_radax`. First-class
     radical-axis reasoning is the biggest AR/rule gap vs our engine.
   - **Projective collinearity**: `x_and_x_to_pappus` (Pappus),
     `x_and_x_and_x_to_desargues` (Desargues), with
     `eqline_and_pappus_to_eqline` / `eqline_and_desagues_to_eqline` back-substitution
     (`eqline.py`).
   - **Harmonic bundles / cross-ratio** (`eqline.py`): `eqratio_and_eqline_to_harmonic`,
     `harmonic_and_perp_to_eqangle`, `harmonic_and_eqangle_to_perp` — right-angle ⇔
     angle-bisector characterization of harmonic conjugates.
   - **Monge** (`eqratio.py` `monge`) — three similitude centers collinear /
     radical center.
   - **Similitude centers** (`add_simili`, `eqratio_and_eqline_to_simili`;
     constructors `ExSimiliCenter`, `InSimiliCenter`) — homothety reasoning.
   - **Cevian / centroid**: `eqline_to_cevian_middle`, `eqline_to_cevian_side`,
     `midp_and_eqline_to_centri` (`eqline.py`, `midp.py`).
   - **Inscribed-angle family** (`eqcircle.py`): center-angle = 2·inscribed
     (`eqangle_and_eqcircle_to_eqangle_cen` / `_cir` / `_half`), and the
     tangent–chord / diameter-perp facts (`eqline_and_eqcircle_to_perp`).
   - **Triangle congruence/similarity with explicit criteria**: `contri.py` = SSS,
     SAS, SSA-for-right-triangles; `simtri.py` = SSS, AAA, SAS, SSA-right.

2. **Fact-triggered forward chaining (event-driven DDAR).** Instead of AG's
   re-scan-all-rules DD loop, TongGeometry pops one fact from a type-ordered
   priority queue (`OrderedFact`) and fires only rules keyed to that fact's type
   (`one_step_fc` → `ALL_RULES[fact.type]`). New facts feed back into the queue;
   `filter` drops trivially-implied facts before enqueue. This is a materially more
   scalable saturation strategy to adopt in `geometry_ddar.py` if its DD loop is
   currently rule-table re-scan.

3. **Equivalence-class database design.** Lines and circles are themselves
   equivalence classes (`eqline` groups collinear points into a representative
   line; `eqcircle` groups concyclic points into a circle). Transitive predicates
   use representative + inverse-index; non-transitive (perp, midp) use element +
   relation set. Auxiliary indices (`segments_perps`, `h_segments_perps`,
   `inverse_midp`, `lines_points`) give O(1) rule lookups. A clean blueprint if we
   refactor our fact store.

4. **`check_num` — a complete numerical-soundness reference (tol 1e-5).** Directed
   angle check via **cross-product sign** (`dir_ABC·dir_DEF`; equal-sign ⇒ compare
   cosines, opposite ⇒ `cos+cos≈0`), concyclic via circumcenter coincidence,
   perp via dot=0, para via cross=0, plus cong/eqratio/midp/simtri(3 ratios)/
   contri(3 lengths). Directly cross-checkable against our `geometry.py` numeric
   filters — especially the directed-angle sign handling.

5. **`check_good` — automatic "requires-auxiliary" problem detection (the
   generation trick).** For each newly derived fact it computes `context_actions =
   prune(fact_points)` (constructions the *statement* needs) and `ca_actions =
   prune(proof_points)` (constructions the *proof* needs). When
   `set(context) ⊊ set(proof)` **and** the last proof action is the current step,
   the fact is a genuine problem whose solution needs `aux = proof − context`
   auxiliary points. It also filters *trivial* eqangle/eqratio (those reducible via
   para/perp/eqline/cong/midp). This is a fundamentally different — and arguably
   better — synthesis primitive than AlphaGeometry's random-premises+traceback: do
   forward random construction, then **harvest facts that provably need aux** as
   ready-made hard problems with known auxiliary answers. High-value port into
   `geometry_synth.py`.

6. **Difficulty scoring + proof-graph tracing.** `score_fact` returns
   (proof-edge-count, aux-vs-context construction delta, DAG depth); `trace_fact`
   emits a graphviz DAG; `build_fact` propagates dependency points and depth;
   `prune` yields the minimal sub-diagram. A complete curriculum-labeling toolkit
   for generated data.

7. **Symmetric-problem generation** (`priority_mirror_sym.py`): maintain a mirror
   pairing `pairs` and, for each action, add its mirror across an axis
   (`get_sym_composite`), yielding aesthetically symmetric proposals; score
   diagrams by concurrent/special points on median/incenter/circumcenter/
   orthocenter lines (`l_G/l_I/l_O/l_H`) and reject degenerate over-concurrency
   (>45–60% of points on one line/center). Uses **Bron–Kerbosch max-clique** to
   dedup overlapping good-problems. Idea-level port for a symmetry-biased proposer.

## Adopt-relevance to Theoremata's geometry vertical

- **Port now (re-implemented, not copied — GPL):**
  (a) the advanced rules in §1 into the 5-rule chainer / `geometry_ddar.py` —
  radical axis + Pappus/Desargues + harmonic + inscribed-angle are the coverage
  wins; (b) the **`check_good` requires-aux detector** into `geometry_synth.py` as
  a second synthesis path alongside traceback; (c) fact-triggered chaining (§2) if
  our DD loop is a re-scan.
- **Already have:** numerical checks (`geometry.py` — cross-check against §4 but
  don't replace), traceback synthetic data (`geometry_synth.py`), Wu's method
  (`geometry_algebraic.py` — note Tong has **no** algebraic/Wu path, it is purely
  rule-based, so our `geometry_algebraic.py` is complementary), a DDAR chainer
  (`geometry_ddar.py`).
- **Needs a model / scale:** the LM policy + PRM value model (`model/solve.py`) and
  the 10k-CPU symmetric data generation (`priority_mirror_sym.py`). The
  **PRM-predicts-steps-to-go** value signal (`score = value − weight·steps_to_go`)
  and the **small-then-large two-stage policy** are the transferable design ideas;
  reproducing them needs trained checkpoints and data we don't have.
- **License caveat:** GPLv3 blocks lifting source. Everything above must be a
  clean-room re-implementation from the rule *statements* / algorithm
  *descriptions* in this report and the paper.

## Verbatim-worthy details

Problem DSL (a test case = action list + goal fact):
```
Action(BaseAcuteTriangle, "", "CBA")
Action(PerpendicularLine, "CBA", "E")
Action(IntersectLineLine, "BECF", "H")
Action(CircumscribedCircle, "ABC", "I")
Action(MidPoint, "BC", "M")
Action(IntersectLineCircleOn, "MAI", "D")
Fact("perp", [Angle(*"DGH")])
```

Directed-angle numeric check (`diagram.check_num`, eqangle case):
```
dir_ABC = (A-B).cross(C-B);  dir_DEF = (D-E).cross(F-E)
if dir_ABC*dir_DEF ≈ 0 or > 0:  return cos∠ABC ≈ cos∠DEF
else:                           return cos∠ABC + cos∠DEF ≈ 0
```

Good-problem test (`check_good`, when a derived fact needs auxiliary points):
```
context_actions = prune(fact_points)      # statement footprint
ca_actions      = prune(proof_points)     # proof footprint
if ca_idx[-1] == last_action and set(context_idx) < set(ca_idx):
    aux = set(ca_idx) - set(context_idx)  # the auxiliary construction(s)
```

Rule inventory (function names, by conclusion module) — the additions vs AG:
`eqcircle`: radical-axis (`add_ax/add_radax/radax`, `eqcircle_and_eqcircle_to_radax`,
`eqline_and_radax_to_perp`), inscribed-angle (`_eqangle_cen/_cir/_half`);
`eqline`: `x_and_x_to_pappus`, `x_and_x_and_x_to_desargues`, harmonic
(`eqratio_and_eqline_to_harmonic`, `harmonic_and_perp_to_eqangle`), cevian
(`eqline_to_cevian_middle/side`); `eqratio`: `monge`, `add_simili`; `contri`/`simtri`:
SSS/SAS/AAA/SSA-right; `perp`: right-triangle-hypotenuse-midpoint = circumcenter.

Constructor vocabulary (`model/vocab.txt`): `BaseAcuteTriangle, AnyPoint, MidPoint,
ExtendEqual, CenterCircle, PerpendicularLine, InCenter, CircumscribedCircle, AnyArc,
MidArc, Perpendicular, Parallel, IntersectLineLine, IntersectLineCircleOn,
IntersectLineCircleOff, IntersectCircleCircle, IsogonalConjugate` (plus `Excenter,
Centroid, Orthocenter, Circumcenter, ExCircle, InCircle, ExSimiliCenter,
InSimiliCenter, BisectorLine, Reflect` in the constructor package).

**Injection check:** none. All inspected files (README, Python sources, DSL test
cases, vocab) are legitimate code / data. No embedded instructions to the reader.
