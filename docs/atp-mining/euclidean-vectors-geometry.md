# ATP Mining — Euclidean / Vector Geometry & Related (Harrison et al.)

Batch: `atp/vector.pdf`, `atp/vectorj.pdf`, `atp/pick.pdf`, `atp/picka.pdf`, `atp/wlog.pdf`.
All are John Harrison's papers (two co-authored with Solovay & Arthan). Focus of this
report: reusable **proof-automation** ideas for Theoremata's geometry prover
(`components/prover/python/theoremata_tools/geometry*.py` — DDAR2, Wu's-method
`geometry_algebraic.py`, synthetic-data `geometry_synth*.py`), the Candle HOL Light
backend, cert-log, and the proof-DAG search/tactic layer.

> **Injection check (all 5 PDFs): CLEAN.** These are genuine mathematical/logic papers
> (HOL Light formalizations and a model-theory decidability study). No embedded prompts,
> instructions, tool directives, URLs-to-fetch, or "ignore previous" style text were
> found. 100% of PDF content was treated as untrusted data; nothing in them was acted on.

---

## 1. `wlog.pdf` — "Without Loss of Generality" (J. Harrison)

**What it is.** TPHOLs 2009 (Springer LNCS 5674, pp. 43–59). The single most
*directly reusable* paper in this batch. It shows how to mechanize the informal
"WLOG …" step in a theorem prover, first for symmetric inequalities, then — the bulk
of the paper — for Euclidean geometry via **invariance under transformation groups**.

**Key mechanisms.**
- **WLOG-by-symmetry meta-theorems.** For a symmetric property, prove it only on a
  canonically-ordered special case. HOL Light ships `REAL_WLOG_LE`
  (`(∀x y. P x y ⇔ P y x) ∧ (∀x y. x≤y ⇒ P x y) ⇒ ∀x y. P x y`); the paper adds
  `REAL_WLOG_3_LE` for three variables (six orderings collapse to `x≤y≤z`), proved by a
  6-way case-split (`REAL_ARITH`) + first-order closure (`ASM_MESON_TAC`). Demonstrated
  on Schur's inequality: the symmetry subgoal is discharged automatically from
  associativity/commutativity of `+`/`*`.
- **WLOG-by-invariance (the general engine).** Geometric concepts in HOL Light's
  Euclidean space are *analytic* — points are vectors over a standard basis, `angle`,
  `norm`, `dist`, `dot` all bottom out in coordinates. So one can pick a convenient
  coordinate frame "without loss of generality" by exploiting that the goal is invariant
  under a transformation group. The core schematic theorem (MESON-proved):
  ```
  (∀x. ∃f y. transform f ∧ nice y ∧ f y = x) ∧
  (∀f x. transform f ∧ nice x ⇒ (P (f x) ⇔ P' x))
  ⇒ ((∀x. P x) ⇔ (∀y. nice y ⇒ P' y))
  ```
  i.e. if every `x` is the image of some *nice* `y` under an allowed `transform`, and `P`
  is invariant under `transform`, prove `P` only on nice inputs. Choosing the transform
  *from* a nice value (rather than *to* one) lets the reduced property `P'` fall out of
  rewriting for free — no extra code to compute it.
- **`QUANTIFY_SURJECTION_THM`.** The trick that makes it work: because `x ↦ a+x` (and any
  orthogonal map) is surjective, universal *and existential* quantifiers can be rewritten
  under the transform in one controlled pass (avoiding infinite loops). An extended
  version also rewrites quantifiers over **sets** of vectors and **set comprehensions**
  `{x | P x}` — so the method scales past points to convex hulls, closed/open/bounded
  sets, measure, etc.
- **Registry-driven tactics.** Two mutable theorem lists, `invariant_under_translation`
  and `invariant_under_linear`, hold per-concept invariance lemmas (e.g.
  `angle(a+b,a+c,a+d)=angle(b,c,d)`, `midpoint(a+x,a+y)=a+midpoint(x,y)`,
  `measure(IMAGE (λx.a+x) s)=measure s`). Each concept picks its *most general* admissible
  class: all linear maps (convex hull), injective (closedness), bijective (openness),
  norm-preserving/orthogonal (angles), or rotations only (`cross` product — has chirality).
  A bottom-up rewrite sweep pulls the transform up through vector-valued functions and
  cancels it at predicate/scalar level. **New concepts extend the system just by adding
  their invariance lemma to the list** — an open, growable capability (echoes LEGO-Prover's
  growing library idea in our paper-mining notes).
- **The shipped tactics.** `GEOM_ORIGIN_TAC v` (translate `v` to origin),
  `GEOM_BASIS_MULTIPLE_TAC i v` (rotate `v` to a nonneg multiple of basis vector `i`),
  `GEOM_HORIZONTAL_PLANE_TAC p` (map a plane in ℝ³ to `{z=0}`), `PAD2D3D_TAC` (drop a
  degenerate ℝ³ problem to ℝ²). The extended example chains
  `HORIZONTAL_PLANE → PAD2D3D → ORIGIN → BASIS_MULTIPLE` to normalize a 3-D
  two-points-in-a-plane problem down to points `(0,0)` and `(0,u₂)` in ℝ², after which the
  coordinate algebra is trivial. This is exactly the frame-pinning that Wu's method and
  DDAR do by hand.
- **Honest cost note.** Harrison quotes Hales (Flyspeck): naïve symmetry proofs needed
  "nearly a hundred lemmas … all the way back to the foundations." The invariance-registry
  automation is what makes this affordable. Future-work: broader groups (shearing for
  incidence-only theorems → Pappus; scaling to kill a magnitude variable; continuous maps
  for topological props), higher-order invariants (triangulations, sequences/limits), and a
  reflective/metalogical reformulation.

---

## 2 & 3. `vector.pdf` / `vectorj.pdf` — "Some new results on decidability for elementary algebra and geometry" (Solovay, Arthan, Harrison)

**What they are.** The **same paper, two versions**: `vector.pdf` = arXiv:0904.3482v1
(2009 preprint, 73 pp); `vectorj.pdf` = the expanded *Annals of Pure and Applied Logic*
journal version (2012, 79 pp, with full ToC). This is the arXiv reference cited at the end
of `wlog.pdf`. A model-theory / computability study, **not** a formalization paper — it
maps out what is and isn't *decidable* about vector-space geometry, using a 2-sorted
first-order language (sort ℝ for scalars, sort V for vectors) with sublanguages for
vector / metric / normed / inner-product spaces.

**Key results (the decidability map).**
- **Decidable (list a):** real vector spaces, inner-product spaces, Hilbert spaces — all
  decidable, and IP∞ = HS∞ share one procedure. **Method:** quantifier elimination once
  the dimension predicates `D≤n` ("∃ ≤ n spanning vectors") are treated as atomic; this
  reduces validity to Tarski's decision procedure for **real-closed fields (RCF)**.
- **The dimension-collapse theorem (§6, practically the most useful).** A sentence with
  **k distinct vector variables** holds in every inner-product space of dimension ≥ k iff
  it holds in **ℝ^k**. So any purely-universal (or fixed-variable) IP/Euclidean claim can
  be checked in a *fixed finite dimension* = number of vector variables, then handed to an
  RCF/CAD decision procedure. This is a rigorous justification for exactly the
  "pin the frame, drop to low dimension" move that `wlog.pdf`'s `PAD2D3D_TAC` and our
  Wu's-method coordinate translation perform.
- **Undecidable (list b):** metric, normed, and Banach spaces are undecidable — in fact
  **not even arithmetical** (2-D Banach ≡ second-order arithmetic in many-one degree;
  arbitrary dim ≡ true Π²₁ third-order-arithmetic sentences). Proved by interpreting
  second-order arithmetic via a natural-number-defining formula `N(x)` (the `Peano`
  construction). The extra freedom to define "exotic" norms is what breaks decidability.
- **Sharp decidable fragments.** Purely-**universal** and purely-**existential** normed-space
  theories are decidable; the ∀∃ fragment of *metric* spaces is decidable (a
  Bernays–Schönfinkel-style argument). These are sharp: ∃∀ (metric & normed) and ∀∃
  (normed) are undecidable via Hilbert's-10th / Diophantine reductions.
- **Implemented.** Harrison implemented the decision procedure for the **universal additive
  normed-space theory**; for the universal case it (in principle) emits a concrete
  **counter-example norm** on ℝⁿ witnessing an invalid sentence — a model-finding, not just
  refutation, procedure.

**Value to us:** this is the *theory ceiling* document. It tells us which geometry goals a
sound complete procedure can exist for (Euclidean / inner-product = yes, via RCF at
dimension = #vars) and which are hopeless (anything leaning on general norms/metrics).
Useful for setting honest capability boundaries and abstention policy, not for code lifting.

---

## 4 & 5. `pick.pdf` / `picka.pdf` — "A formal proof of Pick's theorem" (J. Harrison)

**What they are.** `picka.pdf` = extended abstract; `pick.pdf` = full paper (*Math.
Struct. in Comp. Sci.*, received 2015). A HOL Light formalization of Pick's theorem
(area of a lattice polygon `A = I + B/2 − 1`). Candid about difficulty: **3709 lines** of
proof script (+788 for a reusable "Jordan Triple Curve Theorem"), making it comparable to
the Prime Number Theorem proof despite Pick being "elementary" — the cost is entirely in
**triangulating an arbitrary polygon** and reasoning about "inside/outside".

**Key mechanisms.**
- **`inside`/`outside` via connected components** (general in ℝ^N, not just paths in ℝ²):
  `inside s = {x | x∉s ∧ bounded(connected_component (UNIV DIFF s) x)}`. The Jordan Curve
  Theorem is "Skolemized" into these named operators.
- **Elementary triangle = area ½** via a lattice-preserving linear map having `|det|=1`
  (`linear f ∧ IMAGE f integral_vector = integral_vector ⇒ |det(matrix f)|=1`); det is
  twice the area.
- **Inclusion–exclusion / additivity lemma** for a real-valued set function
  (`f(s∪t)=f(s)+f(t)−f(s∩t)`), applied once with `f`=lattice-point-count and once with
  `f`=area, gives the arbitrary-lattice-triangle case almost for free. (A referee noted
  this is the Euler–Poincaré face-inclusion pattern.)
- **WLOG reuse — the load-bearing tie-in.** The polygon step needs "pick the coordinate
  axis so no two vertices share a y-coordinate." This is done by the `wlog.pdf` machinery,
  but **generalized from first-order objects (points) to higher-order ones (lists of
  vertices / polygonal paths)**: an orthogonal `transform f = MAP g` is chosen so all
  `MEM`-elements of the vertex list get distinct 2nd coordinates — exists because there are
  finitely many vertex pairs but infinitely many rotation angles. Concrete evidence that
  the invariance-registry approach scales to structured objects.
- **Parity lemma** characterizing how inside/outside flips as a segment crosses the polygon
  an odd number of times — drives the "constructed point is inside" arguments.

**Value to us:** a realistic *cost model* and warning. "Intuitively obvious by eye"
geometric facts (inside/outside, orientation, "opposite sides of a line") are
disproportionately expensive to formalize. Suggests keeping a **numeric oracle / diagram**
in the loop (which our `geometry.py` already has) and a library of reusable
topology/orientation lemmas rather than re-deriving them per problem.

---

## Mapping to Theoremata

| Paper idea | Theoremata target | Fit |
|---|---|---|
| WLOG invariance-**registry** + bottom-up rewrite (`invariant_under_{translation,linear}`) | search/tactic layer + `geometry_algebraic.py` frame-pinning; a new `wlog`/normalization tactic | **Strong.** Directly a search-space reducer. |
| `GEOM_ORIGIN` / `GEOM_BASIS_MULTIPLE` / `PAD2D3D` frame normalization | Wu's-method coordinate translation ("pin `A=(0,0)`, `B=(u1,0)`") in `geometry_algebraic.py`; DDAR2 preprocessing | **Strong.** Same move, gives a principled canonicalizer. |
| WLOG-by-symmetry meta-theorem (`REAL_WLOG_3_LE`, transform schema) | generic symmetry pruning in proof-DAG search; dedup of goal orbits | **Medium.** Cross-domain (inequalities, not just geometry). |
| Dimension-collapse: IP sentence with k vars ⇔ holds in ℝ^k | soundness/completeness envelope for `geometry_algebraic.py` (Wu) & abstention policy | **Medium.** Theory backing, not code. |
| Decidability map (Euclidean/IP decidable via RCF at fixed dim; normed/metric undecidable) | capability boundaries; when to route to RCF/CAD vs. give up (abstain) | **Medium.** Sets honest limits; feeds meta-verifier/abstention. |
| Universal-additive normed decision procedure emits **counter-example norm** | model-finding / counterexample surfacing for refuted goals | **Weak/gated.** Narrow fragment; nice for diagnostics. |
| Pick: `inside`/`outside` via connected components + Jordan-Triple-Curve lemma library | topology lemma shelf for the geometry subsystem; cert-log lemma reuse | **Weak.** Big lift; only if we tackle polygon/area goals. |
| Pick: WLOG lifted to **higher-order** objects (vertex lists/paths) | proof that our normalization tactics can act on structured configs, not just points | **Medium.** Design precedent for `geometry_synth*` configs. |
| Additivity / inclusion–exclusion set-function lemma | area/measure reasoning; combinatorial goals | **Weak.** Reusable but domain-specific. |

**Candle / cert-log angle.** Candle is a verified HOL Light; every mechanism above is
*native HOL Light source* (tactics + theorem lists), so it is in principle portable to
Candle almost verbatim, and each invariance lemma / WLOG reduction is a proof step our
**cert-log** checker could record and re-check. The WLOG tactics don't add axioms — they're
derived-rule automation over existing theorems — so they're **cert-log-friendly** (sound by
construction, replayable).

---

## Buildable-now vs. gated (honest)

**Buildable now (no new backend, pure Python over existing geometry vertical):**
1. **A frame-normalization / WLOG canonicalizer** in `geometry_algebraic.py` and as a
   DDAR2 preprocessing pass: translate a chosen point to origin, rotate another onto an
   axis, exploiting that our coordinate model already pins a frame. Turns the ad-hoc
   `A=(0,0), B=(u1,0)` convention into an explicit, reusable tactic with a recorded
   justification. Low risk, immediate search-space win.
2. **Symmetry/orbit dedup** in the proof-DAG driver: detect when subgoals are variable
   permutations of one another and prove one representative (the `REAL_WLOG_3_LE` idea).
   Complements the subsumption dedup already on our adopt list.
3. **An invariance-lemma registry** pattern for our sound rule set: tag each geometry
   predicate with the transform group under which it's invariant, so normalization is
   table-driven and *grows* as predicates are added (mirrors `geometry_synth`'s
   construction-language extensibility).
4. **Capability/abstention notes** derived from the decidability map (Euclidean/IP goals
   with k vars ⇒ decidable in ℝ^k via RCF; normed/metric ⇒ abstain), wired into the
   meta-gate/abstention policy.

**Gated (needs the Candle/HOL-Light backend or large lemma investment):**
- Porting the actual `GEOM_*` tactics + `QUANTIFY_SURJECTION_THM` to Candle (needs the
  multivariate Euclidean-space theory present in the backend). Gated on Candle maturity.
- The universal-additive normed-space decision procedure and counter-example-norm finder
  (specialized implementation; narrow payoff).
- Anything Pick-flavored (inside/outside, Jordan-Triple-Curve, triangulation) — thousands
  of lines; only if area/measure/polygon goals become a target class.
- A verified RCF/CAD backend to *realize* the dimension-collapse decision route
  (we don't currently have one; `geometry_algebraic.py`'s Wu procedure is the pragmatic
  stand-in and is already sound for equational goals).

---

## Prioritized adopt-list

1. **[High] WLOG frame-normalization tactic** (origin + axis-alignment + dimension-drop)
   in `geometry_algebraic.py` / DDAR2 preprocessing. Biggest, cheapest search-space
   reduction; canonicalizes the frame we already pin by hand. Cert-log-recordable.
2. **[High] Invariance-lemma registry** (`invariant_under_{translation,linear}`-style
   table) tagging each geometry predicate with its transform group, so normalization is
   table-driven and grows with the rule set.
3. **[Medium] Symmetry/orbit dedup** in the proof-DAG search (prove one representative of a
   variable-permutation orbit; `REAL_WLOG_3_LE` schema). Pairs with subsumption dedup.
4. **[Medium] Decidability-map capability boundaries → abstention/meta-gate**: encode the
   "Euclidean/IP decidable in ℝ^(#vars); normed/metric undecidable ⇒ abstain" rules.
5. **[Low] Additivity / inclusion–exclusion set-function lemma** for area & lattice-count
   reasoning, if we pursue measure/polygon goals.
6. **[Low / gated] Port `GEOM_*` WLOG tactics + `QUANTIFY_SURJECTION_THM` to Candle** once
   the multivariate Euclidean theory is available; then the higher-order lift (Pick's
   vertex-list version) shows the pattern extends to structured configs.
7. **[Low / gated] Universal-additive normed counter-example-norm finder** for diagnostics
   on refuted goals.

*Sources: `atp/wlog.pdf` (TPHOLs 2009), `atp/vector.pdf`=arXiv:0904.3482v1 (2009) ≡
`atp/vectorj.pdf`=Ann. Pure Appl. Logic (2012), `atp/pick.pdf` (MSCS 2015) with abstract
`atp/picka.pdf`. All John Harrison; vector papers with Solovay & Arthan.*
