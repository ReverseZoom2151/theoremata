# Resource Mining: Newclid_Transformer

Path: `resources/Newclid_Transformer-main/Newclid_Transformer-main/`
(note the tarball double-nests the repo one level down).

## What it is

`Newclid_Transformer` (published by LMCRC, package name `alphageo`, author Mathis
Federico) is a **PyTorch re-implementation of DeepMind's AlphaGeometry**, wired to
run against the **Newclid** symbolic engine instead of the original Meliad/Flax
stack. The README is DeepMind's AlphaGeometry README (Apache-2.0 software +
CC-BY-4.0 for weights/vocab); the code itself is the thin LM front-end + a
DDAR-compatible problem language. The actual DD+AR deduction engine is **NOT
vendored here** — `pyproject.toml` declares `dependencies = ["newclid>=2.0.0", ...]`,
so the interesting solver lives in the external `newclid` pip package. This repo
contributes: (1) a clean PyTorch decoder + beam search, (2) the LM↔solver
translation layer, and (3) two resource files — `new_rules.txt` and `new_defs.txt`
— that are the **Newclid-format** rule and definition tables, which are the real
mining payload.

- **License:** Apache-2.0 (software) / CC-BY-4.0 (model params & vocab). Portable.
- **Relation to AlphaGeometry:** Faithful re-implementation. Reproduces Table 1
  numbers: DDAR solves 14/30 IMO-AG-30 and 198/231 JGEX; +LM reaches 25/30 and
  228/231. The novelty vs vanilla AG is the **Newclid rule/def additions** and a
  **priority beam search** that matches AG's `beam_search.py` behavior.

## Architecture / key files

| File | Role |
|------|------|
| `src/alphageo/alphageometry.py` | Proof-search driver `run_alphageometry()`: run `solver.run()` (DDAR); on failure, LM beam-search over auxiliary constructions, translate, `solver.add_auxiliary_construction()`, re-run. `BeamQueue` keeps top-k. |
| `src/alphageo/inference.py` | `priority_beam_search` (priority-queue beam, brevity penalty), `simple_beam_search`, `brevity_penalty`. |
| `src/alphageo/translate.py` | `translate_constrained_to_constructive()` — maps LM's constraint tokens (perp/para/cong/coll/eqangle/cyclic) to Newclid **constructive** clauses (`on_tline`, `on_pline`, `on_bline`, `on_circle`, `on_line`, `angle_bisector`, `eqangle3`, `on_aline`, `on_circum`, `on_dia`); `MAP_SYMBOL` token dictionary; `check_valid_args()` degeneracy pre-filter. |
| `src/alphageo/model.py` | PyTorch decoder (AG LM). |
| `new_rules.txt` | 51 Newclid deduction rules (premise `=>` conclusion). |
| `new_defs.txt` | Newclid construction-definition table (name / signature / non-degeneracy / predicate body / basis clauses). |
| `problems_datasets/new_problems.txt` | Problems exercising **constant-angle** predicates (`s_angle`, `aconst`) — e.g. IMO 1975 P3. |
| `convert_ag_to_pt.py` | Converts original Flax/Meliad weights → PyTorch. |

Solver interface (what any engine we build must expose to reuse this LM loop):
`solver.run()`, `get_setup_string()`, `get_proof_state()/load_state()`,
`get_existing_points()`, `get_defs()`, `validate_clause_txt()`,
`add_auxiliary_construction()`, `write_solution()`, `draw_figure()`.

## Reusable mechanisms — concrete ports into `geometry_ddar.py` / `geometry_synth.py`

1. **Constant-value predicates (the headline AR extension).** `new_defs.txt`
   adds `aconst a b c x r` (angle a-b-c = constant `r`, e.g. `7pi/30`),
   `rconst a b c x r` (ratio = constant, e.g. `1/2`), `lconst x a y` (length =
   constant), and `s_angle a b x y` (construct point at a specific angle). The
   translate `MAP_SYMBOL` adds AR-computation predicate types `acompute` (`A`),
   `rcompute` (`R`), and "fix" tokens `fixc/fixl/fixb/fixt/fixp` (`Q/E/V/H/Z`)
   that pin a coordinate/length/angle in the AR Gaussian-elimination table. This
   lets DDAR handle problems with **numeric angle/ratio/length constants** — a
   class AlphaGeometry's original AR could not express. If `geometry_ddar.py`'s
   AR currently only tracks *equalities* among angles/ratios, adding constant
   right-hand sides (a distinguished "1" column and rational/`k·pi` constants) is
   the single highest-value port.

2. **New sound rules (rules 44–51 of `new_rules.txt`, beyond AG's r00–r42).**
   Verbatim additions worth porting into the rule chainer:
   - r44 `perp A B C D, perp A C B D => perp A D B C` — **orthocenter/third-altitude** (two altitudes ⇒ third).
   - r45 `coll a b c, coll p q r, coll x a q, coll x p b, coll y a r, coll y p c, coll z b r, coll z c q => coll x y z` — **Pappus**.
   - r46 `cyclic a b c p, coll a l c, perp p l a c, coll m b c, perp p m b c, coll n a b, perp p n a b => coll l m n` — **Simson line**.
   - r47 `eqangle a b a x a x a c, eqangle b a b x b x b c => eqangle c b c x c x c a` — isogonal/third-vertex angle relation.
   - r48 `midp m a b, perp x m a b, midp n b c, perp x n b c, midp p c a => perp x p c a` — **perpendicular-bisector concurrency** (circumcenter).
   - r49 `midp m a b, coll m x c, midp n b c, coll n x c, midp p c a => coll x p b` — **median/centroid concurrency**.
   - r50 `circle O A B C, cyclic A B C D => cong O A O D` — new concyclic point is equidistant from center.
   - r51 `cyclic A B C D, cong O A O B, cong O C O D, npara A B C D => cong O A O C` — circumcenter uniqueness.
   These are all directed-angle/length sound and map directly onto the 5-rule
   chainer's format.

3. **Constraint→construction translation table.** `translate_constrained_to_constructive()`
   is a compact, reusable recipe for turning a *predicate the LM asserts about a
   new point* into a *constructive clause the engine can execute*, including the
   degenerate collapses (`perp` with shared apex → `on_dia`; `cong` with shared
   apex → `on_bline`; `cong` sharing a leg → `on_circle`; `eqangle` with equal
   pivot → `angle_bisector`/`eqangle3`). Port this if `geometry_synth.py` ever
   ingests free-form constraint proposals (e.g. from an LM) rather than only its
   own constructor DSL.

4. **`check_valid_args()` grammar/degeneracy pre-filter** — cheap per-predicate
   arity + distinctness checks (`para` needs 4 distinct points, `coll` 3 distinct,
   `eqangle` ≥3 distinct per side) that reject malformed aux before the expensive
   solver call. Directly reusable as an input gate.

5. **`priority_beam_search` + `brevity_penalty`.** A priority-queue beam
   (`live_sequences` kept sorted; early-exit once enough finished sequences beat
   the best live one) with AG's length normalization
   `((length+5)/6)**0.6`. Reusable as-is for any LM-guided aux proposer in the
   geometry vertical.

## Adopt-relevance to Theoremata's geometry vertical

- **Port now (no model needed):** constant-value predicates (`aconst`/`rconst`/
  `lconst`/`s_angle`) into `geometry_ddar.py`'s AR + the seven extra sound rules
  (Pappus, Simson, third-altitude, bisector/median concurrency, circumcenter
  facts) into the chainer. These are pure symbolic wins and immediately widen
  coverage on constant-angle olympiad problems (IMO 1975 P3 is the canonical case
  in `new_problems.txt`).
- **Already have:** the DDAR core loop + numerical checks (our `geometry.py`/
  `geometry_ddar.py`), traceback synthesis (`geometry_synth.py`), Wu's method
  (`geometry_algebraic.py`). This repo's `alphageometry.py` loop is the same
  DDAR-fail → propose-aux → retry shape we already run.
- **Needs a model/scale:** the beam-search aux proposer requires the trained AG
  LM checkpoint (downloaded from S3 via `common_folder_downloader.py`; CC-BY-4.0).
  Only worth wiring if we want to reproduce the 14→25 IMO jump without training
  our own proposer.
- **Strategic note:** the real engine is the external `newclid` package, not this
  repo. If we want its full rule set / better AR, that package (Newclid ≥2.0) is
  the thing to vendor and mine next — this repo only exposes its rule/def *tables*
  and interface.

## Verbatim-worthy details

Constant-predicate definitions (`new_defs.txt`):
```
rconst a b c x r   ->  x : rconst a b c x r      # segment ratio ab:ac = r
aconst a b c x r   ->  x : aconst a b c x r       # directed angle = r  (e.g. 7pi/30)
s_angle a b x y    ->  x : s_angle a b x y        # point x with ∠(ab, bx) = y
lconst x a y       ->  x : lconst x a y           # |xa| = y   (radiuscircle a y)
```

LM-token → predicate map (`translate.py MAP_SYMBOL`), showing the AR/"fix"
extension surface:
```
A: acompute   R: rcompute   Q: fixc   E: fixl   V: fixb   H: fixt   Z: fixp   Y: ind
```

Degeneracy collapses inside `translate_constrained_to_constructive`:
```
perp, apex shared (a==c==point)      -> on_dia   [a,b,d]
cong, apex shared (a==c==point)      -> on_bline [a,b,d]
cong, leg shared  (b in {c,d})       -> on_circle[a,b,d]
eqangle, equal pivot (x==y, pt==d)   -> angle_bisector [point,b,x,c]
eqangle, point is pivot (point==x)   -> eqangle3 [x,a,b,y,c,d]
```

**Injection check:** none. All inspected files (`new_rules.txt`, `new_defs.txt`,
`new_problems.txt`, Python sources, README) are legitimate rule tables / code /
docs. No embedded instructions to the reader.
