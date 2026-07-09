# ATP Course Slides — Mining Report, Part 1 (slides 01–07)

Source: `atp/slides01…slides07` — lecture decks for an "Automated Theorem Proving"
course (Prof. J. Blanchette, after U. Waldmann; Winter 2025/26; almost certainly
the LMU Munich / MPI-lineage course). Textbook basis: Baader & Nipkow, *Term
Rewriting and All That*, ch. 2, plus Bachmair–Ganzinger resolution theory.

**Security / injection check:** All seven PDFs were treated as untrusted data.
They contain only ordinary lecture material (definitions, theorems, proofs,
algorithm pseudocode, bibliographic references). **No prompt-injection, no
embedded instructions, no adversarial text of any kind was found.** Nothing in
the decks was acted upon as an instruction.

Prior high-level assessment ("largely theory/foundations, limited directly-
buildable content") is **CONFIRMED**. Decks 1–4 are pure foundations; the only
concrete, offline-buildable deltas live in decks 5–7 (resolution) and are listed
in the consolidated section below.

---

## Per-deck synopses

**slides01 — Motivation & Preliminaries.**
Course motivation (SAT-for-sudoku, equational reasoning via Waldmeister) and the
math toolkit: abstract reduction systems (composition, closures, normal forms,
termination vs. normalization) and well-founded / Noetherian orderings
(lexicographic and length-based combinations, well-founded induction/recursion).
Standard; all of this ARS/ordering machinery already underlies our rewrite and
termination modules. No delta.

**slides02 — Preliminaries cont'd + Propositional Logic.**
Multisets and the **multiset ordering** (Dershowitz–Manna extension `≻_mul`,
with the equivalent Huet–Oppen and one-step-closure characterizations, Thm.
1.4.1: preserves transitivity/irreflexivity/well-foundedness/totality). Then
propositional syntax/semantics, models, validity, satisfiability, the standard
equivalences, and "generalized resolution" `(F∨H)∧(G∨¬H) ⊨ F∨G`. The multiset
ordering is the one genuinely reusable primitive here (see delta #4).

**slides03 — Propositional Logic cont'd.**
Normal forms: CNF/DNF, naive equivalence-preserving CNF conversion (6 rewrite
steps) with its exponential blow-up, NNF, the **Tseitin / definitional
equisatisfiable CNF** transformation (linear size, ≤4 clauses per subformula
definition), the **DPLL** procedure (unit propagation, pure literal, split), the
sketch of DPLL→CDCL (conflict analysis, learning, non-chronological backtrack,
restarts), and **(O)BDDs** (Bryant reduction, canonicity, Shannon expansion,
apply-by-memoization, ordering-sensitive size). Textbook SAT/BDD; we are
proof-assistant-backed, so mostly not needed. Tseitin is a minor delta (#3).

**slides04 — First-Order Logic.**
FOL syntax (signatures, terms, atoms, quantifiers, positions), Tarski semantics
(Σ-algebras, assignments, valuation of terms/formulas), models/validity/
entailment, the **substitution lemma**, Peano/Presburger examples, and the
(non)computability results (validity undecidable but semidecidable — Gödel
completeness; Th(N*) not semidecidable — incompleteness). Pure foundations; no
delta.

**slides05 — Resolution (ground).**
Skolemization + prenex + clausal-normal-form pipeline (satisfiability-preserving,
small-arity Skolem functions), **Herbrand interpretations/models**, inference
systems / proofs / soundness / (refutational) completeness, and the **ground
resolution calculus** (binary resolution + positive factorization on multisets
of literals). Core: the Bachmair–Ganzinger **candidate-interpretation / model
construction** with clause & literal orderings (multiset-extended), productive
clauses, minimal counterexample → refutational completeness; plus compactness.
The calculus and its ordering restrictions are the source of deltas #1–#2; the
model-construction itself is a completeness *proof*, not buildable code.

**slides06 — General (first-order) Resolution.**
Lifting from ground to non-ground: resolution-through-instantiation, **most
general unifiers**, rule-based **standard unification** `⇒_SU` (with its
exponential term blow-up example) and the **polynomial / DAG unification**
`⇒_PU` that avoids it (occur-check, cycle detection, size-ordered merging),
idempotent-mgu theorem, general Res (resolution + factorization via mgu), the
**Lifting Lemma**, Herbrand's theorem, and Löwenheim–Skolem / compactness as
corollaries. The unification algorithm is the single most concrete buildable
item (delta #1).

**slides07 — Resolution cont'd.**
The practical prover: **ordered resolution with selection** `Res^≻_sel`
(ordering restrictions on maximal literals + a negative-literal selection
function; stability-under-substitution forces `≻`→`⋠`, `⪰`→`⋡` on non-ground
inferences; kills rotation redundancy), the formal **redundancy** notion
(`C` redundant if entailed by smaller clauses in N; tautology & strict-
subsumption criteria), **saturation up to redundancy**, fair runs / limit /
dynamic refutational completeness, **simplification rules** (duplicate-literal
deletion, **subsumption resolution**), **hyperresolution**, and the **main loop**
(given-clause; Usable/Worked-off split; **Otter vs. DISCOUNT** loop; term
indexing for partner-clause retrieval; "90% of runtime is
simplification/redundancy"). Deltas #2, #3, #5 come from here.

---

## Buildable delta vs. Theoremata

**Framing / honesty.** These decks are a standard first-course on
propositional + first-order logic and resolution. Theoremata already implements
the *architecture* these slides build toward: `components/reason/search/
inverse_method.rs` is a sound forward-saturation **given-clause loop** with
**forward + backward subsumption**, focusing, and saturation-to-fixpoint;
`subsumption.rs` supplies redundancy elimination; `skest.rs` / `goal_cache.rs` /
the MCGS driver add cross-tree/cross-run reuse. So the high-level "given-clause +
subsumption + saturation" story is **already built**. The genuine, non-trivial
gaps are narrow and concentrate in the unification / ordering layer. Everything
below is offline, deterministic, and prover-independent.

1. **First-order (polynomial/DAG) unification producing an explicit mgu — the
   one real gap (slides06, §3.10).** Our `subsumption.rs` explicitly documents
   that it is a *string/literal-level α-canonicalizer, deliberately NOT a
   first-order unifier* — it cannot see that `P(x, f(x))` and `P(b, y)` are the
   same goal modulo `{x↦b, y↦f(b)}`, so it conservatively returns "no
   subsumption" and the search re-does work. The decks give a complete, correct
   recipe to close this: the **rule-based polynomial unification `⇒_PU`**
   (decompose / delete / orient / eliminate with **occur-check** and the
   cycle-detection rule that prevents the exponential blow-up of naive `⇒_SU`,
   with the solved-form → idempotent-mgu extraction). Implementing this as a
   small `unify(t,u) -> Option<Subst>` over our term representation would upgrade
   `subsumes` from string-equality-plus-α to true **first-order subsumption**
   (`Cσ ⊆ D`), strictly increasing redundancy pruning in `inverse_method`. This
   is the highest-value item and we do **not** currently have it.

2. **Ordered resolution with selection as a completeness-preserving pruner
   (slides05/07, §3.13).** Our given-clause loop selects "smallest sequent
   first" but applies inferences without **maximal-literal ordering
   restrictions** or a **selection function**. Ordered resolution `Res^≻_sel`
   restricts inferences to (strictly) maximal literals w.r.t. a well-founded,
   substitution-stable atom ordering (extended to literals then to clauses via
   the multiset extension), optionally overridden per clause by a negative-
   literal selection function — provably refutation-complete (Bachmair–Ganzinger
   model construction) yet strictly smaller search space, and it removes
   "rotation redundancy". This is a concrete, offline pruning discipline we
   could layer onto the saturation engine; requires items #1 and #4 as
   prerequisites (mgu + a clause ordering). Buildable, not currently present.

3. **Subsumption resolution + Tseitin CNF — cheap simplifications (slides07
   §3.14; slides03 §2.5).** (a) **Subsumption resolution** (`D∨L`, `C∨Dσ∨L̄σ` ⇒
   drop `L̄σ`, yielding `C∨Dσ`) is a distinct simplification inference from plain
   subsumption; we have subsumption but not this rule — a small, high-payoff
   addition to the simplification set (the slides note simplification is ~90% of
   real prover runtime). (b) **Tseitin/definitional equisatisfiable CNF**
   (linear-size clausification, ≤4 clauses per introduced definition) is worth
   having *only if* we ever hand ground/propositional obligations to a
   SAT-style check or a clause-form certificate; marginal for a
   proof-assistant-backed stack, noted for completeness.

4. **Multiset ordering (Dershowitz–Manna) as a reusable well-founded ordering
   primitive (slides02, §1.4).** A correct, standalone `≻_mul` (with the
   one-step-closure characterization that is easiest to implement, Thm. 1.4.2)
   is the building block needed to lift any atom/literal ordering to a **clause
   ordering** for item #2, and is independently useful anywhere we compare
   multisets of subgoals/measures for termination. Small, self-contained,
   dependency-free; we don't have an explicit one.

5. **Given-clause architecture note — Otter vs. DISCOUNT + term indexing
   (slides07 §3.16).** Not new code so much as a design confirmation: our
   `inverse_method` loop is essentially a **DISCOUNT-style** loop (passive
   clauses kept out of the index). The one buildable hint the slides *mention
   but do not develop* is a **term-indexing data structure** (e.g.
   discrimination tree / fingerprint index) for fast partner-clause retrieval —
   the real engineering lever behind fast saturation. The slides don't specify
   one, so this is a pointer for later work, not a recipe.

**Explicitly NOT buildable / already covered (theory only):** abstract reduction
systems, well-founded orderings & Noetherian induction/recursion (slides01);
propositional/FOL syntax & Tarski semantics, substitution lemma, Herbrand
theorem, compactness, Löwenheim–Skolem, Gödel completeness/incompleteness
(slides02/04/05/06); DPLL/CDCL and OBDDs (slides03 — textbook SAT, not our
regime); the Bachmair–Ganzinger candidate-interpretation model construction
(slides05 — a completeness *proof*, not an algorithm). One certificate remark: a
**resolution/ground-resolution refutation is itself a checkable proof object**
(each step = a resolvent of two parents by a named mgu); if we ever wanted a
first-order clausal certificate format, `cert_log` could ingest such a DAG — but
this duplicates guarantees our proof-assistant backends already give, so it is a
note, not a recommendation.
