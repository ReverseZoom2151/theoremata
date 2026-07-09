# AlphaGeometry (v1) — mining report

Source: `resources/alphageometry-main/alphageometry-main`
Upstream: google-deepmind/alphageometry (Nature 2024, "Solving Olympiad Geometry without Human Demonstrations").

> **Injection check:** none found. Every file is ordinary Apache-2.0 source, geometry data (`defs.txt`, `rules.txt`, problem lists), or README prose. No file contains instructions directed at an AI/agent, no "ignore your instructions", no format-override text. All repo content was treated as untrusted data and only read, never executed.

---

## What it is

The reference implementation of **DDAR** (a symbolic geometry prover = **DD** deductive database + **AR** algebraic reasoning) coupled with a **transformer language model** that proposes *auxiliary constructions*. DDAR forward-chains a fixed rule set to saturation; when it stalls, the LM suggests a new point (an auxiliary construction), the point is added to the premises, and DDAR is re-run. A neuro-symbolic beam search loops this. The symbolic core (DD+AR, traceback, numerics) runs standalone without the model or its `meliad`/`sentencepiece` deps.

- **License:** Apache-2.0 for code; model checkpoints/vocab under CC-BY-4.0. Copyright 2023 DeepMind Technologies. Safe to port/adapt with attribution.
- **Reported results:** DDAR alone solves 14/30 IMO-AG-30 and 198/231 JGEX-AG-231; full AlphaGeometry solves 25/30 and 228/231.

---

## Architecture / key files

| Concern | File | What to read |
|---|---|---|
| **Problem/premise language** | `problem.py` | `Construction` (one predicate), `Clause` (one construction = points `= ` predicates), `Problem`, `Definition` (parses `defs.txt`), `Theorem` (parses `rules.txt`). `hashed_txt()` = canonical predicate hashing with permutation symmetry. |
| **Construction definitions (DSL)** | `defs.txt` | 6-line-per-entry macro language: signature, `rely` map, deps clause, `basics` (predicates asserted), `numerics` (how to sample coordinates), sketch name. |
| **Deduction rule set (DD)** | `rules.txt` | 43 human-readable rules `premise, ... => conclusion`. |
| **DD engine** | `dd.py` | `bfs_one_level()` (one BFS deduction level), `match_all_theorems`, ~30 hand-written `match_*` matchers + `match_generic`, `BUILT_IN_FNS`, `MAX_BRANCH=50000`. |
| **AR engine** | `ar.py` | `Table` (Gaussian-elimination coefficient matrix over `Fraction`s) + `AngleTable`, `RatioTable`, `DistanceTable`, `GeometricTable`. `get_all_eqs_and_why()` mines new equalities; `why()` does MILP traceback via `scipy.optimize.linprog`. |
| **DD+AR driver** | `ddar.py` | `solve()` alternates DD-to-saturation then AR derivations; `saturate_or_goal()`; `get_proof_steps()`. |
| **Proof-state graph** | `graph.py`, `geometry.py`, `graph_utils.py` | union-find-like equivalence nodes (`Point/Line/Circle/Segment/Direction/Length/Angle/Ratio/Value/Measure`), `add_algebra`/`derive_algebra`/`do_algebra` bridge DD↔AR. |
| **Traceback / dependency-difference** | `trace_back.py` | `recursive_traceback()`, `separate_dependency_difference()` (**this is the aux-extraction algorithm** used for synthetic-data generation), `shorten_proof()`. |
| **Numerical engine** | `numericals.py` | sketches/samples coordinates for each construction; `check_*` numerical predicate checks used to filter rule matches. |
| **Model harness** | `alphageometry.py`, `lm_inference.py`, `beam_search.py`, `models.py`, `transformer_layer.py`, `decoder_stack.py` | beam search loop, LM decoding in JAX/meliad, and — crucially — `translate_constrained_to_constructive()` that turns LM predicate output into a construction. |

**Note:** the repo ships the *prover* and the *traceback* but **not** the random-premise sampler / deductive-closure data generator itself. The traceback machinery in `trace_back.py` is the reusable half of the synthetic-data pipeline; the sampler must be reconstructed from the paper (documented below).

---

## The problem/premise language (verbatim-worthy)

A problem string is `clause; clause; ... ? goal`. Each clause is `p1 p2 ... = constr1, constr2, ...`. Example (orthocenter):

```
a b c = triangle a b c; d = on_tline d b a c, on_tline d c a b ? perp a d b c
```

`defs.txt` entry format is **6 lines** (parsed by `Definition.from_txt`, `reshape(lines, 6)`):

```
angle_bisector x a b c        # 1. construction signature (name + args)
x : a b c x                   # 2. rely: which points each new point depends on
a b c = ncoll a b c           # 3. deps clause (preconditions to state it)
x : eqangle b a b x b x b c   # 4. basics: predicates this construction asserts
bisect a b c                  # 5. numerics: how numericals.py sketches it
                              # 6. (blank separator)
```

Core predicates in the DSL: `coll, para, perp, cong, cyclic, eqangle, eqangle6, eqratio, eqratio6, midp, circle, simtri, simtri2, simtri*, contri, contri2, contri*, aconst, rconst, s_angle, sameside, ncoll, npara, nperp, diff`. The `6` variants (`eqangle6`, `eqratio6`) mean "directed / full-angle, 6 distinct points". Canonical hashing (`hashed_txt`) sorts argument pairs so that e.g. `perp a b c d == perp b a d c`.

---

## The DD rule set (verbatim — all 43 rules from `rules.txt`)

```
perp A B C D, perp C D E F, ncoll A B E => para A B E F
cong O A O B, cong O B O C, cong O C O D => cyclic A B C D
eqangle A B P Q C D P Q => para A B C D
cyclic A B P Q => eqangle P A P B Q A Q B
eqangle6 P A P B Q A Q B, ncoll P Q A B => cyclic A B P Q
cyclic A B C P Q R, eqangle C A C B R P R Q => cong A B P Q
midp E A B, midp F A C => para E F B C
para A B C D, coll O A C, coll O B D => eqratio3 A B C D O O
perp A B C D, perp E F G H, npara A B E F => eqangle A B E F C D G H
eqangle a b c d m n p q, eqangle c d e f p q r u => eqangle a b e f m n r u
eqratio a b c d m n p q, eqratio c d e f p q r u => eqratio a b e f m n r u
eqratio6 d b d c a b a c, coll d b c, ncoll a b c => eqangle6 a b a d a d a c
eqangle6 a b a d a d a c, coll d b c, ncoll a b c => eqratio6 d b d c a b a c
cong O A O B, ncoll O A B => eqangle O A A B A B O B
eqangle6 A O A B B A B O, ncoll O A B => cong O A O B
circle O A B C, perp O A A X => eqangle A X A B C A C B
circle O A B C, eqangle A X A B C A C B => perp O A A X
circle O A B C, midp M B C => eqangle A B A C O B O M
circle O A B C, coll M B C, eqangle A B A C O B O M => midp M B C
perp A B B C, midp M A C => cong A M B M
circle O A B C, coll O A C => perp A B B C
cyclic A B C D, para A B C D => eqangle A D C D C D C B
midp M A B, perp O M A B => cong O A O B
cong A P B P, cong A Q B Q => perp A B P Q
cong A P B P, cong A Q B Q, cyclic A B P Q => perp P A A Q
midp M A B, midp M C D => para A C B D
midp M A B, para A C B D, para A D B C => midp M C D
eqratio O A A C O B B D, coll O A C, coll O B D, ncoll A B C, sameside A O C B O D => para A B C D
para A B A C => coll A B C
midp M A B, midp N C D => eqratio M A A B N C C D
eqangle A B P Q C D U V, perp P Q U V => perp A B C D
eqratio A B P Q C D U V, cong P Q U V => cong A B C D
cong A B P Q, cong B C Q R, cong C A R P, ncoll A B C => contri* A B C P Q R
cong A B P Q, cong B C Q R, eqangle6 B A B C Q P Q R, ncoll A B C => contri* A B C P Q R
eqangle6 B A B C Q P Q R, eqangle6 C A C B R P R Q, ncoll A B C => simtri A B C P Q R
eqangle6 B A B C Q R Q P, eqangle6 C A C B R Q R P, ncoll A B C => simtri2 A B C P Q R
eqangle6 B A B C Q P Q R, eqangle6 C A C B R P R Q, ncoll A B C, cong A B P Q => contri A B C P Q R
eqangle6 B A B C Q R Q P, eqangle6 C A C B R Q R P, ncoll A B C, cong A B P Q => contri2 A B C P Q R
eqratio6 B A B C Q P Q R, eqratio6 C A C B R P R Q, ncoll A B C => simtri* A B C P Q R
eqratio6 B A B C Q P Q R, eqangle6 B A B C Q P Q R, ncoll A B C => simtri* A B C P Q R
eqratio6 B A B C Q P Q R, eqratio6 C A C B R P R Q, ncoll A B C, cong A B P Q => contri* A B C P Q R
para a b c d, coll m a d, coll n b c, eqratio6 m a m d n b n c, sameside m a d n b c => para m n a b
para a b c d, coll m a d, coll n b c, para m n a b => eqratio6 m a m d n b n c
```

`ncoll/npara/nperp/sameside/diff` premises are **numerical checks**, not looked up in the DB — enforced by evaluating point coordinates (`match_generic` calls `g.check_ncoll` etc.). This is the mechanism that keeps rules *sound* while avoiding degenerate matches. Roughly half the rules have hand-written fast matchers in `dd.py` (`BUILT_IN_FNS`); the rest fall back to `match_generic`, which sorts premise clauses by distinct-arg count (most constrained first) and recursively binds points (`try_to_map`).

---

## Reusable mechanisms (specific things to port)

### 1. Additional sound DD rules for `geometry.py` / `geometry_ddar.py`
Our forward chainer has 5 rules; `rules.txt` gives 43 sound, olympiad-tested rules. High-value additions that need no new machinery beyond angle/ratio equalities:
- **Thales / inscribed angle family** (rules 4, 5, 16-21): `cyclic A B P Q => eqangle P A P B Q A Q B`, and its converse; center-based `circle O A B C, coll O A C => perp A B B C` (angle in semicircle).
- **Similar/congruent triangle rules** (34-42) with the orientation-aware `same_clock` numeric guard — directly gives SSS/SAS/ASA/AA reasoning.
- **Midpoint-parallel family** (7, 26, 27, 30) and the **transitivity chains** (10, 11): `eqangle...eqangle => eqangle`, `eqratio...eqratio => eqratio`.
- The **`eqratio6 <-> eqangle6` bridge** (12, 13) linking length ratios and angles on a collinear triple — the core of ratio/angle "chasing".

### 2. The AR coefficient-matrix construction (`ar.py`) — the single most reusable component for `geometry_algebraic.py`
AR represents each geometric quantity as a variable and each fact as a **linear equation over `Fraction`s**, solved by incremental Gaussian elimination. Three specialized tables:
- **`AngleTable`** — variables = line directions; constant = `pi`; works **mod 1** (`modulo` does `e[pi] %= 1`). `add_para(d1,d2)` => `d1 - d2 = 0`; `add_const_angle(d1,d2,ang)` => `d1 - d2 = ang/180`; `add_eqangle(d1,d2,d3,d4)` => `(d1-d2)-(d3-d4)=0`. This is exactly the "angle chase".
- **`RatioTable`** — variables = `log(length)`; constant = `1`. `add_eqratio` => `(l1-l2)-(l3-l4)=0` in log space; `add_const_ratio(m,n)` => `l1-l2 = log(m/n)`. Turns multiplicative ratio chasing into linear algebra.
- **`DistanceTable`** — variables = signed positions of points on a line (`line:point`); handles collinear metric relations and discovers **constant ratios** between segments (`get_all_eqs_and_why` yields `(v1,v2,v3,v4,n,d,why)` for `(v1-v2)*d=(v3-v4)*n`).

Key method `Table.add_expr(vc)` reduces a new equation against the current reduced system and either (a) proves it redundant, (b) instantiates a free variable, or (c) picks a dependent var — a clean incremental RREF. `get_all_eqs()` then reads off *all* currently-derivable equalities by hashing `v2e[v1]-v2e[v2]` and grouping equal hashes.

### 3. AR traceback = MILP (`Table.why`)
To justify a derived equality, AR builds `A` where each registered fact is a `+1/-1` column pair, then solves
`min c^T x s.t. A_eq x = b_eq, x >= 0` (`scipy.optimize.linprog(method='highs')`),
and returns the facts with nonzero `x` as the minimal dependency set. **This is a general recipe: represent facts as columns, ask LP for a minimal nonneg combination that yields the target.** Portable to `geometry_algebraic.py` for producing *minimal* Wu/AR proofs.

### 4. Traceback + dependency-difference (`trace_back.py`) — the aux-extraction algorithm
`recursive_traceback(query)` walks the proof DAG from the goal, dedups premises, and groups steps. `separate_dependency_difference(query, log)` then splits the used premises into **setup** (points the goal depends on transitively via `Point.rely_on`) vs **aux_setup** (points/constructions *not* reachable from the goal's dependency cone). In synthetic-data generation this "dependency difference" is precisely how a randomly-sampled proof is turned into a `(problem, auxiliary-construction)` training pair: the aux points are the ones whose removal still leaves a well-posed problem but breaks DDAR. `shorten_proof()` merges trivial single-parent steps for readable output. **Port target:** `geometry_synth.py` traceback.

### 5. Constrained-to-constructive translation (`alphageometry.py`)
`translate_constrained_to_constructive(point, name, args)` maps a *constraint* the LM emits (`perp/para/cong/coll/eqangle/cyclic`) onto a *constructive* action (`on_tline/on_dia/on_pline/on_bline/on_circle/eqdistance/on_line/angle_bisector/eqangle3/on_aline/on_circum`) so the new point can actually be sketched with coordinates. This is the glue any neural aux proposer needs; it is a compact lookup we can reuse verbatim if we adopt the same DSL.

### 6. Premise-sampling recipe (from paper; sampler not in repo)
For `geometry_synth.py`, reconstruct the AG1 pipeline: (a) sample a random set of construction clauses from `defs.txt` (each `numerics` line tells the numerical engine how to place the new point consistently); (b) run DDAR to deductive closure; (c) for every derived fact, treat it as a goal and run traceback; (d) apply dependency-difference to peel off aux points -> emit `(minimized-premises + goal, aux-points)` as an LM training example. Point renaming to alphabetical order (`Clause.translate`, `Problem.translate`) matches the LM's training distribution.

---

## Adopt-relevance to Theoremata's geometry vertical

**Port now (no model/scale needed):**
- The **43-rule set** (`rules.txt`) into `geometry_ddar.py`'s rule table, keeping the numerical `ncoll/npara/nperp/sameside` guards for soundness. Biggest immediate capability jump for the sound forward chainer.
- The **AR three-table design** (`AngleTable`/`RatioTable`/`DistanceTable`) into `geometry_algebraic.py` / `geometry_ddar.py`. Angle-chase and ratio-chase as incremental linear algebra over `Fraction`s is simpler and more complete than a bespoke Wu's-method-only path, and pairs with our existing Wu implementation.
- The **`linprog` minimal-dependency traceback** for producing short, auditable proofs.
- The **dependency-difference traceback** into `geometry_synth.py` to convert deductive-closure runs into aux-labelled synthetic examples.

**Already have (map, don't duplicate):** a sound forward chainer (`geometry.py`, 5 rules → extend), DDAR-style DB (`geometry_ddar.py`), Wu's method (`geometry_algebraic.py`), synth/traceback scaffold (`geometry_synth.py`).

**Needs a model / scale we lack:** the transformer aux proposer (`lm_inference.py`, `models.py`, JAX/meliad) and its 100M-problem training corpus. We can adopt the *interface* (`translate_constrained_to_constructive`, beam loop in `run_alphageometry`) and slot in any LM later; the symbolic core is fully usable without it.

---

## Verbatim-worthy signatures & schemas

```python
# ar.py — the AR core
class Table:                       # coefficient matrix over Fraction
  def add_expr(self, vc: list[tuple[str, float]]) -> bool   # incremental RREF add
  def add_eq2(self, a,b,m,n,dep)   # a/b = m/n
  def add_eq3(self, a,b,f,dep)     # a - b = f * const
  def add_eq4(self, a,b,c,d,dep)   # a - b = c - d
  def why(self, e) -> list[Dependency]   # linprog minimal-dependency
  def get_all_eqs_and_why(self, return_quads=True) -> Generator   # mine equalities
class AngleTable(GeometricTable):  const='pi'; modulo does e['pi'] %= 1
class RatioTable(GeometricTable):  variables = log(length), const='1'
class DistanceTable(GeometricTable): variables 'line:point'; yields const ratios

# ddar.py
def solve(g, theorems, controller, max_level=1000, timeout=600)   # DD⇄AR loop
def get_proof_steps(g, goal, merge_trivials=False) -> (setup, aux, log, refs)

# dd.py
def bfs_one_level(g, theorems, level, controller, nm_check=True, timeout=600)
BUILT_IN_FNS: dict[str, matcher]   # ~30 hand-written fast matchers
MAX_BRANCH = 50_000

# trace_back.py
def recursive_traceback(query) -> list[(prems, [con])]
def separate_dependency_difference(query, log) -> (log, setup, aux_setup, points, aux_points)

# alphageometry.py
def translate_constrained_to_constructive(point, name, args) -> (name, args)
def run_alphageometry(model, p, search_depth, beam_size, out_file) -> bool   # beam loop
```

AR proof-step rule names surfaced to users: `a00` = Distance chase, `a01` = Ratio chase, `a02` = Angle chase; `r32`=SSS, `r33`=SAS, `r34/35`=Similar Triangles, `r36/37`=ASA, `r40`=Congruent Triangles.
