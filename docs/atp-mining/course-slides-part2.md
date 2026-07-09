# ATP Course Slides — Mining Report, Part 2 (decks 08–14)

Source: `atp/slides08…14` — lecture slides for an Automated Theorem Proving
course (Prof. J. Blanchette, based on U. Waldmann's slides, Winter 2025/26;
LMU Munich). This part covers the equality/rewriting/superposition arc:
semantic tableaux → term rewrite systems → termination → Knuth–Bendix
completion → superposition → efficient saturation.

**Security / injection check:** All seven PDFs were treated as untrusted data.
Content is standard graduate ATP theory (definitions, theorems, proof sketches,
worked examples). **No prompt-injection, no embedded instructions, no
directives to the reader/agent were found in any deck.** Nothing actionable
beyond the mathematics.

Scope note: this batch is, as previously assessed, **mostly foundational
theory**. But unlike the tableaux/resolution foundations, the
rewriting/termination/completion/superposition material contains several
*concrete, fully-specified, offline-implementable algorithms* that Theoremata
does **not** currently have. Those are surfaced in the "Buildable delta"
section; the honest bottom line is that the value here is a small number of
well-defined data structures and orderings, not a new research idea.

---

## Per-deck synopsis

### slides08 — Semantic Tableaux
Analytic, goal-oriented, *global* refutation calculus: α/β expansion rules,
closed/open/maximal/strict paths, Hintikka sets and Hintikka's Lemma for
completeness, proof-confluence ("don't-care" expansion order). FO extension via
ground instantiation vs. **free-variable tableaux** (γ → fresh free var, δ →
Skolem function of the free vars, plus a global substitution rule; AMGU
tableaux restrict σ to `mgu` of complementary literals). Contrasts tableaux
(backward, goal-directed) with resolution (forward, better redundancy/equality).
*Buildable: essentially nothing new — the free-variable substitution machinery
duplicates ideas already in the inverse-method engine.*

### slides09 — Rewrite Systems
Foundations of FO logic with equality. Naïve handling (add reflexivity/
symmetry/transitivity/congruence axioms) is complete but hopelessly
inefficient. Defines the rewrite relation `→_E`, rewrite rules/TRS, E-algebras,
Birkhoff's Theorem (`s ↔*_E t ⇔ E ⊢ s≈t ⇔ E ⊨ s≈t`), confluence /
local confluence / Church–Rosser / convergence, Newman's Lemma, and the
**Critical Pair Theorem** (a TRS is locally confluent iff all critical pairs
are joinable), with the overlap-at-nonvariable-position construction and a
worked example. This is the theoretical backbone for completion and
superposition.

### slides10 — Termination
Termination of TRSs is undecidable, so criteria are sound-only. **Reduction
orderings** (well-founded rewrite orderings): a TRS terminates iff `l ≻ r` for
all rules under some reduction ordering. Three concrete constructions:
(i) **polynomial/interpretation orderings** (monotone `N`-polynomials);
(ii) **simplification orderings** + Kruskal/Dershowitz (subterm property ⇒
well-founded on finite signatures); (iii) the two workhorses —
**LPO** (lexicographic path ordering, with mpo/rpo-with-status variants) and
**KBO** (Knuth–Bendix ordering: weight function + precedence, admissibility
conditions), both fully specified with worked precedence/weight examples.

### slides11 — Knuth–Bendix Completion
Turns a set `E` of equations into an equivalent convergent TRS. Inference rules
on `(E, R)` pairs: **Orient, Delete, Deduce (add critical pairs), Simplify-Eq,
R-Simplify-Rule, L-Simplify-Rule** (the last guarded by the encompassment
quasi-ordering). Strategy: simplify/delete > orient > deduce. Correctness via
the Bachmair–Dershowitz–Hsiang proof-ordering argument (fair run + empty `E∞`
⇒ `R∞` convergent). **Unfailing completion** (use orientable ground instances of
unorientable equations; ground-convergence) is the bridge to superposition.
Worked example: completing the `add/succ/zero` axioms under LPO.

### slides12 — Superposition (core)
Combines ordered resolution (overlap maximal literals) with completion (overlap
maximal sides). Inference rules: **positive/negative superposition, equality
resolution, equality factoring** (needed for clauses like `b≈d ∨ b≈c`).
Literal/clause ordering built from a ground-total reduction ordering via
multisets (`s≈t ↦ {s,t}`, `s≉t ↦ {s,s,t,t}`); non-ground rules unify instead of
match, never overlap at/below variable positions, and restrict inferences by
maximality. Completeness via candidate-interpretation (model) construction over
a productive-clause TRS; redundancy defined by "follows from smaller clauses."
Worked `div/inv` refutation.

### slides13 — Superposition (completeness + refinements)
Full Bachmair–Ganzinger model-construction proof (`E_C`/`R_C`/`R∞`, lifting
lemmas, static & dynamic refutational completeness, fair runs with redundancy-
based deletion). Then the *practically important* part —
**concrete simplification / redundancy criteria**: subsumption, trivial-literal
elimination, **condensation**, **(equational) tautology deletion via congruence
closure**, and **demodulation/rewriting by unit equations**. Also selection
functions and the sharper notion of **redundant inferences** (conclusion
follows from clauses smaller than the *premise*, not the conclusion).

### slides14 — Efficient Saturation & Outlook
The engineering deck. Term representations (shared trees vs. flatterms).
**Term-indexing** for the "find instances / generalizations / unifiable terms"
retrieval problem, with perfect vs. imperfect filtering: **path indexing**
(paths → trie, little space, no backtracking, intersect per-path result sets),
**discrimination trees** (preorder string → trie, one term per leaf, great for
generalizations), and **feature-vector indexing** (Schulz — clause-level
subsumption via monotone integer features in a trie). Outlook: SMT (CDCL + T),
sorted logics, **splitting/AVATAR** (SAT-managed clause splitting), and
higher-order superposition (no mgus, embedded booleans, order compatibility
loss).

---

## Buildable delta vs. Theoremata

**What Theoremata already has (so is *not* delta):**
- A forward-saturation **inverse-method / given-clause engine** with
  **subsumption-based** forward/backward redundancy and focusing
  (`components/reason/search/inverse_method.rs`, `…/subsumption`). This is the
  *non-equational* analogue of the superposition loop — it already implements
  the given-clause loop, fair saturation to fixpoint, and subsumption redundancy
  described abstractly in slides13.
- A thin **SymPy** boundary for simplify/factor/solve/diff/integrate
  (`components/tools/python/theoremata_tools/symbolic.py`) — but this is CAS
  normalization, **not** a term-rewriting engine with orderings or completion.
- **Retrieval-level** indexes over Lean declarations (`head_index.py`,
  `decl_index.py`, `mathlib_index.py`) — head-symbol / declaration buckets, i.e.
  NL/Lean-string retrieval, **not** first-order term indexing over a saturation
  set.

**Concrete, offline-implementable items we do NOT have (genuine delta):**

1. **A term-ordering module: LPO and/or KBO** (slides10). Fully specified
   algorithms — LPO from a precedence, KBO from precedence + weight function with
   the stated admissibility conditions. Both are ~100–200 lines of pure,
   deterministic, dependency-free Rust, decidable and easily unit-tested against
   the slides' worked examples (`add≻succ≻zero`; the `f/h/g` weight example).
   This is the single highest-leverage adopt: **a total-on-ground, stable,
   well-founded term order is the missing primitive** that every item below
   needs, and it would also give the existing inverse-method engine a principled
   ordering for ordered inference/redundancy instead of "smallest sequent first."

2. **Critical-pair computation + a Knuth–Bendix (unfailing) completion loop**
   (slides09 + slides11). Given equational lemmas (associativity/commutativity,
   ring/group axioms, the `add` recursion, rewrite hint sets), complete them to
   a convergent/ground-convergent TRS → a **decision procedure for that
   equational theory** and a canonical normal-form rewriter. The inference rules
   (Orient/Delete/Deduce/Simplify) are explicit and finite; the encompassment
   guard on L-Simplify is spelled out. Deterministic, verifiable, no ML. Pairs
   naturally with a normalization-based **simplification/`normalize` tool** for
   the symbolic worker.

3. **A first-order term index — a discrimination tree** (slides14). The natural
   substrate for "find all stored terms that are instances/generalizations/
   unifiable with `t`." Directly speeds up any equality-aware saturation and
   would also back demodulation and subsumption retrieval in the existing
   engine. Discrimination trees (preorder-string trie with `*` for variables)
   are the simplest high-value choice; **feature-vector indexing** (slides14,
   Schulz 2013) is an even smaller, self-contained win specifically for
   **clause subsumption** and could accelerate the *existing* subsumption
   redundancy check in `inverse_method.rs` (monotone integer features → trie,
   imperfect filter + exact recheck).

4. **Demodulation / rewriting-by-unit-equations as a simplification rule**
   (slides13): if a unit `s≈t` with `sσ ≻ tσ` occurs, rewrite `C[sσ] → C[tσ]`.
   Once (1) exists this is a small, sound, terminating simplification that
   strictly strengthens the current redundancy machinery. Likewise **equational
   tautology deletion** needs a **congruence-closure** routine (slides13) — a
   standard, self-contained union-find-over-terms algorithm, independently
   useful as a fast ground-equality decision procedure.

5. **A completion/superposition proof *certificate*** (cross-cutting). Every
   step in these calculi is a local, checkable object: a critical-pair overlap
   (`⟨position p, mgu σ⟩`), an orient decision (`l ≻ r` under a named ordering),
   a rewrite/demodulation step, or a superposition inference (premises + unifier
   + ordering side-conditions). A completion run or superposition refutation can
   emit a **replayable, independently-checkable proof log** in exactly the style
   of Theoremata's existing cert-log checker — arguably the most on-mission
   adopt, since it turns an equality proof into a verification-gate artifact.

**Honest caveats.**
- The vast majority of these decks (tableaux, Birkhoff, Newman, all the
  completeness/model-construction proofs) is standard textbook theory with **no**
  new buildable content — do not mistake the volume of material for volume of
  delta.
- Full first-order **superposition** (slides12–13) is a large, subtle engine
  (equality factoring, selection functions, the Bachmair–Ganzinger redundancy
  criterion). It is **not** a "port in a week" item and likely exceeds what
  Theoremata needs, given that Lean/Rocq/Isabelle backends already discharge
  equational goals. The pragmatic reading is: adopt the **reusable primitives**
  (KBO/LPO, a term index, congruence closure, demodulation, feature-vector
  subsumption, completion for small equational theories + a certificate), not
  the whole superposition prover.
- Everything listed is **deterministic and dependency-free** — it fits the
  verification-first, replayable-certificate ethos and needs no model, no
  network, no new heavy dependency.

**Suggested priority:** (1) LPO/KBO ordering module → (3) feature-vector
subsumption index to speed the existing engine → (4) demodulation + congruence
closure → (2) small-theory Knuth–Bendix completion → (5) certificate emission.
