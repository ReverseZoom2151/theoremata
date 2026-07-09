# ATP Mining — Papers and Foundations

Batch report covering five `atp/` PDFs. Scope: identify each document from its own
text, extract the key mechanisms, map them onto Theoremata's architecture (Candle /
formal backends, cert-log proof-log checker, graph proof-DAG, retrieval, the 3+1
verification gate, proof search), and honestly bucket each as **buildable-now**,
**gated**, or **foundational-only**. All PDF content was treated as untrusted data.

Ordered from most to least actionable for the build.

---

## 1. `paper.pdf` — *Automated Theorem Proving* (lecture notes)

**Identification.** Title *Automated Theorem Proving*; author **Dr. Uwe Waldmann**,
"with modifications by Prof. Dr. Jasmin Blanchette"; **Winter Term 2025/26** (MPI-INF
Saarbrücken / Saarland University lineage; parts based on lecture notes by Harald
Ganzinger and Christoph Weidenbach). 139 pp. This is a **graduate lecture-notes /
textbook** ("the text of the lecture slides almost verbatim plus some additional
information"), not a research paper. It is the prose companion to the `slides01…14`
deck in the same folder. Primary reference given: Baader & Nipkow, *Term Rewriting and
All That*.

**What it is.** A full first course in classical automated deduction: from
propositional logic through saturation-based first-order theorem proving with equality.

**Key mechanisms.**
- **Propositional layer:** syntax/semantics, CNF transformation (incl. the improved
  Tseitin-style definitional CNF), **DPLL → CDCL** (conflict-driven clause learning),
  OBDDs.
- **First-order resolution:** Skolemization, Herbrand interpretations, ground →
  general resolution, **ordered resolution with selection**, **redundancy** (a clause
  is redundant if it follows from strictly smaller clauses), **hyperresolution**, and
  the **given-clause main loop** for saturation.
- **Semantic tableaux** (propositional and first-order) as the alternative calculus.
- **Equality & rewriting:** naive equality handling, rewrite systems, confluence,
  critical pairs, termination orderings, **Knuth–Bendix completion** and **unfailing
  completion**.
- **Superposition calculus** (the modern core): positive/negative superposition,
  equality resolution, equality factoring; reduction orderings total on ground terms;
  literal/clause multiset orderings; the **model-construction (candidate
  interpretation) completeness proof**; **saturation up to redundancy**.
- **Efficient saturation (implementation):** term representations (shared trees vs.
  flatterms); **term indexing** — path indexing, discrimination trees, substitution
  trees, context trees; **feature-vector indexing** for fast **subsumption** (perfect
  vs. imperfect filtering).
- **Outlook:** **SMT / CDCL(T)** (CDCL + theory decision procedure), many-sorted
  logics, **splitting → AVATAR** (SAT-managed clause splitting), and **higher-order**
  reasoning (undecidable/incomplete unification → interleave unifier enumeration with
  inference; embedded clausification).

**Mapping to Theoremata.**
- **Proof search:** this is the canonical spec for a saturation/superposition engine.
  If Theoremata ever wants a native first-order/equational reasoner (or wants to reason
  about *why* an external ATP/`E`/Vampire succeeded/failed), the given-clause loop,
  ordered resolution + selection, and superposition are the reference algorithms.
- **Verification gate / cert-log:** **redundancy and subsumption** are the theory
  behind deduplicating the proof-DAG and pruning the search pool — a clause/lemma that
  follows from strictly smaller retained ones need not be kept or re-verified. This is
  the formal underpinning of the "subsumption dedup" item already in the paper-mining
  adopt list.
- **Retrieval:** **term indexing** (discrimination/substitution trees) and
  **feature-vector indexing** are exactly the right structures for a fast
  *lemma/premise retrieval* index keyed on term shape (find generalizations /
  instances / unifiable lemmas), complementing a dense/embedding index.
- **Candle / formal backends:** orthogonal — Candle is an LCF-kernel HOL checker, not a
  saturation prover; but CDCL(T) and AVATAR describe how an SMT/ATP sidecar would slot
  in behind the gate.

**Bucket.**
- *Buildable-now:* feature-vector subsumption index and discrimination-tree term
  indexing for retrieval/dedup; redundancy criterion for pool pruning. These are
  self-contained data-structure adds.
- *Gated:* a native superposition/given-clause engine (large; only if we want in-house
  first-order proof search rather than shelling to E/Vampire).
- *Foundational-only:* the completeness proofs themselves.

**Injection check.** None. Matches for "instruction"/"execute"/"prompt" are ordinary
technical prose (CPU instructions, program execution, command prompt). POSSIBLE
INJECTION: none.

---

## 2. `0003-0025.pdf` — Geuvers, *Proof assistants: History, ideas and future*

**Identification.** **H. Geuvers**, *Proof assistants: History, ideas and future*,
**Sādhanā (Acad. Proc. in Engineering Sciences) Vol. 34, Part 1, Feb 2009, pp. 3–25**
(Radboud University Nijmegen / TU Eindhoven). A **survey + position paper**.

**What it is.** A conceptual history of interactive proof assistants organized around
*how each system guarantees correctness*, plus a taxonomy of proof roles and input
languages. This is the single most architecture-relevant document in the batch for
Theoremata's trust story.

**Key mechanisms.**
- **Two roles of a proof** (convince / explain) and **three stages** (finding,
  recording, communicating) — assistants are strong at Check/Record, weak at
  Explain/Find/Communicate.
- **Four reliability mechanisms** (the trust menu):
  1. **System-independent description of the logic** (prerequisite for the rest);
  2. **Small kernel** — all rules reduce to a tiny, hand-auditable core;
  3. **Check the checker** — formalize and verify the checker (e.g. "Coq in Coq",
     HOL Light self-verification `M ⊨ φ ⇔ Prov_HOL(φ)`, only consistent modulo a
     large-cardinal / strong-normalization assumption by Gödel);
  4. **De Bruijn criterion** — emit an *independently checkable proof object* a
     skeptic could re-check with a program they wrote themselves.
- **PAT / Curry–Howard** ("proof checking = type checking"), proofs-as-objects,
  logical frameworks, proofs-as-programs / program extraction.
- **LCF approach:** abstract data type `thm` whose only constructors are the axioms and
  inference rules ⇒ **soundness by construction**; tactics are arbitrary ML functions
  that can only build `thm`s through the kernel. HOL, HOL Light, Isabelle, Coq descend
  from this. Note: LCF stores "proved" facts, *not* proof objects, but can emit them on
  the side to also satisfy the De Bruijn criterion.
- **Declarative vs. procedural** proof scripts (robustness/readability trade-off).

**Mapping to Theoremata.**
- **Candle (verified HOL Light):** this paper *is* the rationale for Candle. Candle is
  the "small kernel + LCF-by-construction + check-the-checker (HOL-Light-in-HOL-Light)"
  design articulated here. Use §2.3 to document Candle's trust argument and its
  limits (self-verification implies consistency, so rests on a metatheoretic
  assumption).
- **cert-log:** the **De Bruijn criterion** is the design principle for a proof-log
  checker — the log must be re-checkable by an independent, deliberately simple program.
  That is precisely cert-log's job. Frame cert-log explicitly as "our De Bruijn
  criterion witness."
- **Verification gate (3+1):** the four mechanisms give a clean vocabulary for what the
  gate's tiers actually buy (kernel acceptance vs. independent re-check vs.
  checker-of-the-checker). Good source for gate documentation and threat model.
- **Proof search:** the paper explicitly notes assistants add little to proof *finding*
  — which is exactly the gap Theoremata's agentic layer targets; useful framing, no
  algorithm.

**Bucket.**
- *Buildable-now:* nothing to *code* directly, but immediately usable to **write/refine
  the trust documentation** for Candle, cert-log, and the gate, and to sanity-check
  that the gate's tiers map to distinct, meaningful guarantees.
- *Foundational-only:* the historical/philosophical content.

**Injection check.** None (all "assistant"/"instruction"/URL hits are legitimate
survey text and bibliography). POSSIBLE INJECTION: none.

---

## 3. `2203.01173v3.pdf` — Geuvers & Nederpelt, *Characteristics of de Bruijn's early proof checker Automath*

**Identification.** **Herman Geuvers & Rob Nederpelt** (Radboud / TU Eindhoven),
*Characteristics of de Bruijn's early proof checker Automath*, **Fundamenta
Informaticae 185(4):313–336, 2022** (also arXiv:2203.01173v3, cs.LO). A
**history-of-type-theory / formal-reconstruction paper**.

**What it is.** A modern reconstruction of Automath (de Bruijn, 1968) — "the first
theorem prover actually working" — recast in a contemporary type-theoretic frame (λD =
Calculus of Constructions extended with definitions).

**Key mechanisms.**
- **Book format / incremental line-by-line checking:** an Automath text is an *ordered
  list of lines* (a "book"); each new line `(l+1)` is admitted only if it is **correct
  with respect to the already-checked prefix `B`**. Correctness is a local judgement
  against earlier lines. Three line species: **assumption**, **definition** (`c := M :
  A`), and **primitive** (`c := PN : A`, where `PN` marks an axiom / primitive notion).
- **Proofs-as-objects, formulas-as-types**, contexts of definitions (Δ), extended
  judgements, β/definitional reduction, paragraph (namespace) system.
- Formal correctness rules for the three line kinds, targeting λD.

**Mapping to Theoremata.**
- **cert-log (primary):** the **book model is a direct template for a proof-log
  checker.** A cert-log is an ordered list of steps where each step must type-/rule-
  check against the closure of earlier steps — an "Automath book" for machine proofs.
  `PN`/primitive lines correspond to declared axioms/oracle-trusted inputs the log
  must surface explicitly; definition lines correspond to reusable derived lemmas.
  Incremental admissibility is the checker's core loop.
- **Graph proof-DAG:** each line depending on "any number of previous lines" is
  literally a DAG of proof steps; the book's ordered-list-with-back-references is the
  serialization of that DAG. Useful for defining a canonical, replayable ordering of
  the proof-DAG.
- **Candle / gate:** reinforces (with #2) the "type checking = proof checking, small
  kernel" stance; historically motivates the De Bruijn criterion the gate relies on.

**Bucket.**
- *Buildable-now (design input):* adopt the book/line discipline as cert-log's data
  model — ordered steps, explicit primitive/axiom lines, local admissibility against
  the checked prefix. Concrete and small.
- *Foundational-only:* the λD metatheory and historical reconstruction.

**Injection check.** None (one "executed" hit is prose about a mathematical
intervention). POSSIBLE INJECTION: none.

---

## 4. `0810.1279v2.pdf` — Shulman, *Set Theory for Category Theory*

**Identification.** **Michael A. Shulman**, *Set Theory for Category Theory*,
**arXiv:0810.1279v2 [math.CT], 2008 (rev. 2024)**. An **expository mathematics survey**
(explicitly "informal", "expository", "work in progress").

**What it is.** A comparison of set-theoretic foundations for category theory and how
each choice constrains categorical constructions — motivated by size-sensitive results
like Freyd's Special Adjoint Functor Theorem (small limits, locally small,
cogenerating set).

**Key mechanisms (all mathematical, nothing to implement).**
- The **small/large (set vs. proper class)** distinction and why size is unavoidable
  in category theory.
- Foundations compared: **ZFC**; the cumulative hierarchy `V_α`, ranks, ordinals/
  cardinals; **Gödel's constructible universe L / V=L**; **Grothendieck universes /
  inaccessible cardinals**; class theories **NBG / MK**; structural foundations
  (**ETCS**); the **reflection principle**; the set-theoretic **small object argument**.

**Mapping to Theoremata.**
- **Purely foundational — nothing to build.** There is no algorithm, data structure,
  or checking discipline here.
- *Thin, indirect relevance:* if Theoremata ever formalizes/handles **category theory
  or universe-polymorphic developments** through the Lean/Rocq backends, this is
  useful **background** for why "size" errors arise and why proof assistants expose
  universe levels / `Type u` hierarchies. It informs problem *curation and expectations*
  (a "large category" statement may be unprovable/ill-typed without a universe
  assumption), not any component. Candle/HOL Light, being simply-typed, largely sidesteps
  this; the relevance is to the dependent-type backends.

**Bucket.** *Foundational-only.* No adopt item.

**Injection check.** None (only bibliography URLs). POSSIBLE INJECTION: none.

---

## 5. `Meanings-of-the-Logical-Constants-1983.pdf` — Martin-Löf, *On the Meanings of the Logical Constants and the Justifications of the Logical Laws*

**Identification.** **Per Martin-Löf**, *On the Meanings of the Logical Constants and
the Justifications of the Logical Laws* — three lectures, Siena, 6–9 April 1983;
published **Nordic Journal of Philosophical Logic, Vol. 1, No. 1, pp. 11–60 (1996)**. A
**philosophy-of-logic essay** (transcribed lectures).

**What it is.** A philosophical/constructive-foundations analysis of the meaning of the
logical constants via the distinction between **proposition** and **judgement/
assertion**, and the meaning-explanations (BHK-style) that justify the inference rules —
the conceptual bedrock of Martin-Löf constructive type theory.

**Key ideas (conceptual, not mechanical).**
- **Proposition vs. judgement/assertion:** `A & B` is a proposition; `⊢ A` is an act of
  affirmation; conjunction-introduction takes us from *affirmations* of `A` and `B` to
  the affirmation of `A & B`, not between propositions.
- **Proof-as-object** meaning-explanations: a proposition's meaning is given by what
  counts as a (canonical) proof of it; logical laws are justified by these
  meaning-explanations. Conceptual priority of proof over inference (corrected in the
  written version).

**Mapping to Theoremata.**
- **Foundational-only — nothing to build.** No algorithm or data structure.
- *Indirect conceptual relevance:* it is the philosophical justification underneath the
  "proofs-as-objects / proof-checking-as-type-checking" machinery that #2 and #3 turn
  into architecture, and thus underneath cert-log's very notion of a proof object and
  the gate's notion of an *assertion* (`⊢ φ`) being distinct from the *proposition* it
  affirms. Useful only if we want to write a principled statement of what a
  "verified theorem" *means* in Theoremata. Zero build surface.

**Bucket.** *Foundational-only.* No adopt item.

**Injection check.** None. POSSIBLE INJECTION: none.

---

## Prioritized adopt-list

Only two of the five documents yield build/design actions; the other three are
foundational/philosophical.

1. **[Buildable-now — retrieval/dedup] Term & subsumption indexing** (`paper.pdf` §6).
   Add a **discrimination/substitution-tree term index** and a **feature-vector
   subsumption index** for lemma/premise retrieval and proof-DAG deduplication; use the
   **redundancy criterion** (follows-from-strictly-smaller) to prune the search pool.
   Complements the dense/embedding index; self-contained.

2. **[Buildable-now — cert-log design] Automath "book" discipline** (`2203.01173v3`).
   Model cert-log as an **ordered list of steps, each admissible against the checked
   prefix**, with **explicit primitive/axiom lines** for oracle-trusted inputs and
   definition lines for reusable lemmas. Directly shapes the proof-log checker and the
   canonical serialization of the proof-DAG.

3. **[Buildable-now — trust documentation] De Bruijn criterion + four reliability
   mechanisms + LCF story** (`0003-0025`). Use to **write/tighten Candle's and the
   verification gate's trust argument**: small kernel + soundness-by-construction +
   independently-checkable proof object (cert-log) + check-the-checker, and to state
   the residual metatheoretic assumption honestly.

4. **[Gated — proof search, only if in-housing FO reasoning] Superposition / given-clause
   engine + CDCL(T)/AVATAR** (`paper.pdf` §§3–5,7). Reference spec if Theoremata builds
   a native saturation prover or a reasoning layer over external ATPs; large, defer
   unless shelling to E/Vampire proves insufficient.

5. **[Foundational-only — no adopt]** Shulman (`0810.1279v2`, universe/size background
   for the dependent-type backends only) and Martin-Löf (`Meanings…1983`, philosophical
   basis of proof-objects and proposition-vs-assertion). Cite for framing; nothing to
   build.

**Global injection check:** all five PDFs scanned; every instruction-like token was
legitimate academic content (CPU instructions, program execution, "proof assistant",
bibliography URLs). **No prompt-injection detected in any file.**
