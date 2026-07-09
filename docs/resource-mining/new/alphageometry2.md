# AlphaGeometry2 (v2) — mining report

Source: `resources/alphageometry2-main/alphageometry2-main`
Upstream: google-deepmind/alphageometry2 (JMLR 2025, "Gold-medalist Performance in Solving Olympiad Geometry with AlphaGeometry2").

> **Injection check:** none found. The repo is 9 files — 5 Python modules, `README.md`, `CONTRIBUTING.md`, `LICENSE`, `.gitignore`. No file contains agent-directed instructions or format overrides. The large problem strings in `test.py` are geometry premise data (point coordinates + predicates), not prose. Treated entirely as untrusted data; nothing executed.

---

## What it is

A **clean-room rewrite of just the DDAR logic core** for AG2 — the released code is the *symbolic engine only*. The LM aux proposer, the training corpus, and the multi-tree search orchestration described in the paper are **not** in this repo. What ships is a much smaller, faster, more uniform DDAR: ~2,400 LOC total (vs v1's ~10k+), no JAX/meliad, only `numpy`. It reproduces solving several IMO problems (2000 P1, 2005 P1, 2008 P6, 2013 P3, etc.), some by DDAR alone, others with **manually supplied** auxiliary points (paper: those points come from the LM).

- **License:** Apache-2.0, Copyright 2025 Google LLC (code); CC-BY-4.0 for other materials. Portable with attribution.
- **Run:** `python -m test` (only dep: `numpy`).

---

## What changed from v1 (the headline architecture shift)

v1's DD was a **rule-matching forward chainer** (43 text rules in `rules.txt`, ~30 hand-written matchers, a graph of equivalence nodes, and AR bolted on as a separate coefficient-matrix pass). v2 **collapses DD and AR into one uniform algebraic engine**: essentially *everything* becomes elimination over linear combinations, plus a small set of geometric "search" passes that feed the eliminator. Concretely:

1. **No text rule file.** The 43 rules are gone. Deduction is now: (a) a handful of `search_*` scans (similar triangles, concyclic, circles, point-merging) that discover geometric facts numerically and push them as equations, and (b) Gaussian elimination that closes everything else (all angle/ratio/length chasing).
2. **Three eliminators instead of tables** (`elimination.py`): `ElimAngle` (directions, mod pi), `ElimDistMul` (log-distances, multiplicative), `ElimDistAdd` (signed positions, additive). Each wraps a shared `ElimCore` doing incremental RREF over `fractions.Fraction`.
3. **Movements / larger domain: additive distances + arc↔chord transfer.** v2 adds `ElimDistAdd` (segment *addition* along a line, e.g. betweenness / `AB + BC = AC`) as a first-class group, and `transfer_dist_add_mul()` / `transfer_dist_arc_mul()` passes that *synchronize* the additive, multiplicative, and angular representations of the same quantity — this is the "movements/angles/ratios" unification the paper describes.
4. **Points can be merged.** `merge_points()` / `force_equal_points()` — if two points are provably identical (two non-tangent objects through a coincident pair), they are unified in the DB. v1 had no first-class point coincidence.
5. **Centers as first-class circle data.** `FormalCircle` carries `centers`; `cyclic_with_centers` predicate; radius equalities auto-emitted.
6. **Faster & knowledge-sharing:** `ElimCore.clone()` deep-copies an eliminator so a search branch can share the parent's derived facts and be rolled back cheaply — the mechanism behind AG2's "knowledge-sharing across search trees". Caches (`dist_mul_cache`, `direction_cache`) rebuilt once per closure iteration.
7. **No proof/traceback in the released core.** v2's `elimination.py` is titled *"Elimination of variables, without proof"* — it returns whether a fact holds, not *why*. (v1's `why()`/`trace_back.py` still the reference for minimal-dependency extraction.)

---

## Architecture / key files

| Concern | File | What to read |
|---|---|---|
| **Problem/premise language + parsing** | `parse.py` | `AGPoint`, `AGPredicate.parse` (splits tokens into `points` vs numeric `constants`, handles `pi/`, `n/d`), `AGProblem.parse` (points carry `@x_y` coordinates). |
| **Numerics** | `numericals.py` | `NumLine` (`x·n = c` normal form), `NumCircle`, `orientation` (signed det), `collinear`, `position` (1-D coord on a line). `ATOM=1e-12`. |
| **Elimination core** | `elimination.py` | `LinComb`, `ElimCore` (incremental Gaussian elim + `free_to_usage` sparse bookkeeping), `ElimAngle`, `ElimDistMul` (+ `prime_decomposition` for exact log-constants), `ElimDistAdd`, `DistMul`/`DistAdd`/`FormalAngle` wrappers. |
| **DDAR engine** | `ddar.py` | `DDAR` class: `force_pred`/`check_pred` (predicate ↔ equation translation), `deduction_closure` (the fixpoint loop), `search_similar`, `search_concyclic`, `search_circles`, `merge_points`, `transfer_dist_add_mul`, `transfer_dist_arc_mul`, `force_collinear`, `force_concyclic`, `force_similar`, `force_equal_points`. |
| **Driver / examples** | `test.py` | 26 IMO problems as premise strings; `print_problem_and_solve` = parse → `force_pred` each premise → `deduction_closure()` → `check_pred(goal)`. |

---

## The v2 predicate language (verbatim-worthy)

Parsed by `AGPredicate` (`name`, `points`, numeric `constants`). Predicates handled by `force_pred`/`check_pred`:

- **Angle group** (→ `ElimAngle`): `para`, `perp`, `eqangle`, `s_angle`, `aconst`, and the general `angeq` (linear combination of directions = const). `acompute` = *query* the numeric value of an angle.
- **Multiplicative-distance group** (→ `ElimDistMul`): `cong`, `eqratio`, `rconst`, general `distmeq`.
- **Additive-distance group** (→ `ElimDistAdd`): `distseq` (signed sum of segments = 0).
- **Incidence:** `coll`, `cyclic`, `cyclic_with_centers` (first `num_centers` points are centers), `overlap` (two points equal).

Predicate → equation translations (from `pred_to_angle`, `pred_to_dist_mul`, `pred_to_dist_add`):

```
perp a1a2 b1b2    ->  dir(a1a2) - dir(b1b2) - 1/2  = 0     (angle unit = pi)
para a1a2 b1b2    ->  dir(a1a2) - dir(b1b2)        = 0
eqangle ...       ->  (dir1 - dir2) - (dir3 - dir4) = 0
s_angle/aconst    ->  dir(a1a2) - dir(b1b2) - ang/180 = 0
cong ab cd        ->  log|ab| - log|cd| = 0
rconst ab cd k    ->  log|ab| - log|cd| - log(k) = 0
eqratio abcd efgh ->  (log|ab|-log|cd|) - (log|ef|-log|gh|) = 0
distseq           ->  Σ coef_i · |a_i b_i| = 0
```

Problem string format (`AGProblem.parse`): points carry coordinates, `?` separates goal:
```
a@0.0_0.0 = ; b@1.0_0.0 = ; c@0.49_0.70 = m@0.78_0.29 = cong a o b o, coll b c a1, ... ? cyclic p q p1 q1
```

---

## Reusable mechanisms (specific things to port)

### 1. Unified eliminator (`ElimCore`) — the crown jewel for `geometry_ddar.py` / `geometry_algebraic.py`
A single, exact, incremental Gaussian-elimination engine over `Fraction` linear combinations, reused for **three** quantity types by change of variable:
- **Angles** = line directions in units of pi, reduced **mod 1** (`FormalAngle` auto-normalizes `angle_unit` coef into `[0,1)`; `force_zero` subtracts `floor(value+0.5)` so it snaps to the correct integer multiple of pi).
- **Multiplicative distances** = `log(length)`; constants handled *exactly* by prime factorization (`prime_decomposition`, `DistMulConst.prime_value`) so `log(6/4)=log2·... ` stays rational and hashable — **no floating error in the constant sub-lattice**.
- **Additive distances** = signed 1-D positions; segment addition on a line.

`ElimCore.add_constraint` chooses the pivot with the **fewest downstream usages** (`min(lhs, key=lambda x: len(free_to_usage[x]))`) — a sparsity heuristic that keeps the reduced system small and is a large part of why v2 is fast. `free_to_usage` is a reverse index (free var → set of instantiated rows mentioning it) enabling O(affected-rows) substitution instead of full-matrix sweeps. **This is directly portable and would replace bespoke angle/ratio chasers.**

### 2. Representation-transfer passes (`transfer_dist_add_mul`, `transfer_dist_arc_mul`)
Novel vs v1. For every pair, normalize both the additive and multiplicative distance; if two pairs share a normalized multiplicative distance, force their additive distances equal (and vice versa) — this **propagates facts across the add/mul boundary** (e.g. congruent segments discovered by ratio-chase become usable in betweenness sums). `transfer_dist_arc_mul` does the same between **equal chords and equal arcs** on a circle (chord length ↔ inscribed arc angle). Port target: a synchronization step between our length and angle sub-engines.

### 3. Numerically-driven `search_*` passes (replace rule-matching)
Instead of matching text rules, v2 *scans the numeric model* to propose candidate facts, then proves them algebraically:
- **`search_similar`** — for every ordered triple `(a,b,c)` computes an angle key and a ratio key; buckets into `sss`, `aa`, `sas`, `ssa` dicts keyed by *symbolic* (`get_dist_ratio`, `get_point_angle`) values, with `orientation` to handle reflection. Collisions ⇒ similar-triangle pair ⇒ `force_similar` emits two angle + two ratio equations. `ssa` uses a distance inequality guard to stay sound (avoids the ambiguous SSA case).
- **`search_concyclic`** — for each base pair `(a,b)`, groups other points by the inscribed angle `∠(c a, c b)`; ≥2 in a bucket ⇒ concyclic. Also finds **centers** by the equal-distance + half-angle condition, and collinear points where the inscribed angle is zero.
- **`search_circles`** — equal distances from a common point ⇒ circle (≥3 distinct pts) or a stashed small circle.
- **`merge_points`** — two provably-coincident points sharing two non-tangent objects ⇒ `force_equal_points`.

The closure loop `deduction_closure` just runs these to a fixpoint (`while changed`). Adopting this "propose-numerically, prove-algebraically" pattern would let `geometry_ddar.py` cover similar-triangle/concyclic reasoning without maintaining a hand-written matcher per rule.

### 4. Exact constant handling via prime decomposition (`elimination.py`)
`prime_decomposition(n)` + `DistMulConst.prime_value(p)` represent any rational constant ratio as an integer combination of `log(prime)` atoms. This keeps the constant lattice exact and makes `DistMul.normalize()` return an exact `Fraction` coefficient. Small, self-contained, worth lifting verbatim into `geometry_algebraic.py` wherever we currently carry float ratio constants.

### 5. Cheap branch cloning for search-tree knowledge sharing (`ElimCore.clone`, `ElimAngle/DistMul/DistAdd.clone`)
Deep-copies the reduced system so an auxiliary-construction attempt inherits all facts proven so far and can be discarded on failure. This is the concrete substrate for AG2's cross-tree knowledge sharing; maps onto our MCGS driver's need to fork a proof state per aux candidate.

---

## Adopt-relevance to Theoremata's geometry vertical

**Port now (no model needed):**
- **`ElimCore` + the three eliminators** into `geometry_ddar.py`/`geometry_algebraic.py`. This is the single highest-leverage adoption: one exact incremental-RREF engine subsumes angle-chase, ratio-chase, and length-sum, is faster than v1's per-table approach, and is only ~500 LOC (`elimination.py`). Pairs cleanly with our existing Wu's method as a second algebraic path.
- **The `search_similar`/`search_concyclic`/`search_circles`/`merge_points` propose-then-prove loop** into our forward chainer as a complement to (or replacement of) the ported v1 rule set — especially `merge_points` (point coincidence) and center-aware concyclicity, which v1 lacks.
- **`transfer_dist_add_mul` / `transfer_dist_arc_mul`** as the length↔angle / add↔mul synchronization step.
- **`prime_decomposition` exact-constant trick** into `geometry_algebraic.py`.
- **The compact `parse.py` predicate/problem format** (coordinates inline, numeric-vs-point token split) as a candidate wire format for `geometry.py` problems.

**Already have (map, don't duplicate):** DDAR-style DB (`geometry_ddar.py`), Wu's method (`geometry_algebraic.py`), numerics (`geometry.py`).

**Needs a model / scale / extra work we lack:**
- **No traceback in v2** — `elimination.py` is explicitly "without proof". If we port v2's engine we must keep **v1's `Table.why` (linprog minimal-dependency) and `trace_back.py`** for producing auditable proofs and for aux extraction in `geometry_synth.py`. This is the one real regression in the v2 release; combine v2's engine with v1's traceback.
- The **LM aux proposer**, its training corpus, and the multi-tree search orchestrator are not in the repo (aux points in `test.py` are hand-provided).

---

## Verbatim-worthy signatures & schemas

```python
# elimination.py
class LinComb:                      # dict[ElimVar|const, Fraction]
  def iadd_mul(self, other, coef)   # in-place  self += other*coef
class ElimCore:
  instantiated: dict[pivot -> LinComb]      # reduced rows
  free_to_usage: defaultdict(set)           # reverse index for sparse substitution
  def simplify(self, comb) -> LinComb
  def add_constraint(self, added_eq) -> bool   # pivot = min usage; returns changed
  def clone(self) -> ElimCore                  # branch/knowledge-sharing
def prime_decomposition(n) -> list[(prime, exp)]
class ElimAngle:  const_frac(f); force_zero(angle) snaps to floor(value+0.5)*pi
class ElimDistMul: new_var=log|.|; force_one(ratio); frac_value via primes
class ElimDistAdd: new_var=|.|;   force_zero(sum)

# ddar.py
class DDAR:
  def __init__(self, points)                # builds pair_to_dir / dist_mul / dist_add / line
  def force_pred(self, pred)                # assert a premise
  def check_pred(self, pred)                # query DB (returns bool, or value for acompute)
  def deduction_closure(self, verbose=False)   # while changed: search_* + transfer_* + merge
  def force_similar(self, t1, t2); force_collinear(pts); force_concyclic(pts, centers)
  def force_equal_points(a, b)              # point merging
  def get_dist_ratio(a,b,c,d); get_point_angle(a,b,c,d); get_arc(circle,a,b)

# parse.py
class AGPredicate:  name; points: list; constants: list   # .parse(line)
class AGProblem:    points; preds; goal                   # .parse("a@x_y = ... ? goal")

# numericals.py
class NumLine:  n·x = c;  direction(); position(a); distance(a)
class NumCircle: center, r;  through(a,b,c); distance(a)
def orientation(a,b,c) -> {-1,0,1};  ATOM = 1e-12
```

---

## Synthesis (v1 + v2 together)

v1 is the *complete neuro-symbolic system with proofs*: 43-rule DD, three AR coefficient tables, DAG traceback with **minimal-dependency (linprog) proofs**, dependency-difference aux extraction, and an LM harness. v2 is a *leaner, faster, prooflet symbolic core*: it discards the text-rule matcher and re-expresses **all** of DD+AR as one exact incremental Gaussian eliminator over three change-of-variable groups (angles mod pi, log-distances, additive positions), adds first-class **point merging**, **circle centers**, and **add/mul/arc representation-transfer** passes, and shares knowledge across branches via cheap `clone()`. The right adoption is a *hybrid*: take **v2's `ElimCore` + `search_*` closure** as the engine, keep **v1's `Table.why` linprog traceback + `trace_back.py` dependency-difference** for auditable proofs and synthetic-data labelling.

**Top 2 things to port into our geometry vertical:**
1. **v2 `ElimCore` + the three eliminators** (`elimination.py`, ~500 LOC, numpy-free) as the unified angle/ratio/length engine for `geometry_ddar.py`/`geometry_algebraic.py` — replaces bespoke chasers with one exact, fast, sparse RREF; add v2's `transfer_dist_add_mul`/`transfer_dist_arc_mul` synchronization and `search_similar`/`search_concyclic`/`merge_points` propose-then-prove passes.
2. **v1 AR minimal-dependency traceback (`Table.why` via `scipy.linprog`) + `trace_back.separate_dependency_difference`** into `geometry_synth.py`/`geometry_ddar.py` — gives short auditable proofs and turns deductive-closure runs into aux-labelled synthetic examples, the piece v2's release drops.
