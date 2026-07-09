# Mining report: arXiv 2212.11082v1

> Scope note / correction: The task brief guessed this PDF was "a large (359pp)
> survey/thesis/handbook on automated/interactive theorem proving or ML for
> theorem proving." **It is not.** After extracting and reading all 359 pages,
> the document is a graduate-level **mathematics textbook on type theory**. This
> report records what the document actually is, honestly maps its (limited)
> relevance to Theoremata, and flags that the ML-for-proving / proof-search /
> premise-selection / RL / benchmark material the brief asked me to extract
> **does not exist in this source**. Nothing was invented to fill those buckets.

## 1. Exact identity — what it is

- **Title:** *Introduction to Homotopy Type Theory*
- **Author:** Egbert Rijke (Univerza v Ljubljani; work done at CMU and UIUC)
- **arXiv:** 2212.11082v1 [math.LO], submitted 21 Dec 2022 (preface dated 19 Dec 2022, Ljubljana)
- **Publisher:** to be published by Cambridge University Press; the arXiv PDF is a free pre-publication draft (© Egbert Rijke 2020). Every page carries a Cambridge/CUP copyright-and-errata boilerplate footer — repeated verbatim ~120 times (this is the only "repeated instruction-like text" in the file; it is a copyright notice, not an injection).
- **Length / shape:** 359 pages, 22 chapters in 3 parts, 216 exercises, an index, and a bibliography. It grew out of Rijke's Spring-2018 CMU course notes.
- **Genre:** a rigorous, definition/theorem/proof mathematics textbook — *not* a survey, *not* about algorithms, *not* about machine learning. The word pairs "machine learning", "neural", "premise selection", "proof search", "benchmark", "reinforcement learning", "expert iteration", "MCTS/MCGS", and "retrieval" appear **zero** times in the entire document.

The one and only bridge to Theoremata's world is **formalization**: the book (incl. most exercise solutions) is fully formalized in the **agda-unimath** library (github.com/UniMath/agda-unimath, ref [22]), and the text encourages readers to formalize exercises in a proof assistant (Agda / Coq / Lean named in passing). Proof assistants are described as "capable of performing this task" and as giving "instant feedback."

## 2. Taxonomy / structure of the document

- **Part I — Martin-Löf's Dependent Type Theory** (ch. 1–8): judgments, contexts, inference rules, derivations; dependent function types (Π); natural numbers and pattern matching; inductive types (unit, empty, coproducts, ℤ, Σ-types); identity types and the groupoid structure; universes; Curry–Howard and modular arithmetic; decidability, well-ordering, gcd, infinitude of primes, boolean reflection.
- **Part II — The Univalent Foundations of Mathematics** (ch. 9–20): equivalences and homotopies; contractible types/maps; the fundamental theorem of identity types; propositions/sets/truncation levels; function extensionality; propositional truncation and logic-in-type-theory; image factorizations and surjections; finite types; **the univalence axiom**; set quotients; groups in univalent mathematics (Eckmann–Hilton); general (W-type) inductive types and Russell's paradox.
- **Part III — The circle** (ch. 21–22): the circle as a higher inductive type, its induction/universal properties, the universal cover, descent, and the computation π₁(S¹) = ℤ.

The intellectual "taxonomy" is the standard HoTT arc: dependent type theory → equivalences/truncation → univalence → higher inductive types → synthetic homotopy.

## 3. Techniques vs. the buckets the brief asked for

The brief asked me to map premise selection/retrieval, neural/symbolic proof search, autoformalization, benchmarks/eval, RL/expert-iteration, and tactic prediction onto Theoremata's components (retrieval, MCGS driver, flywheel, formalize portfolio, eval harness). **The source contains none of these techniques.** Honest mapping:

| Brief's expected topic | Present in this doc? | Reality |
|---|---|---|
| Premise selection / retrieval | No | Pure math text; no algorithmic content. |
| Neural / symbolic proof search | No | No search algorithms at all. |
| Autoformalization | Only tangentially | It is a *hand-formalized* corpus (agda-unimath), not auto-formalization. |
| Benchmarks / evaluation | No | 216 human exercises, no eval methodology. |
| RL / expert iteration | No | Absent. |
| Tactic prediction | No | Absent. |

What the document *does* give Theoremata is **domain content, not method**: a clean, formalized, dependent-type-theory foundation. Relevant angles:

- **Inference-rule discipline (ch. 1–6).** Judgments `a : A`, contexts, formation/introduction/elimination/computation rules for every type former. This is exactly the kernel-level object language of Theoremata's Lean/Rocq (dependent-type) backends. Maps loosely to the **FormalSystem abstraction** — a reference for how a type-theoretic backend's proof terms and definitional equality are structured.
- **agda-unimath corpus.** A large, uniformly-structured, machine-checked library. If Theoremata ever wants a *dependent-type* (as opposed to Lean-mathlib) formalization corpus for retrieval indexing or as autoformalization targets, this is a candidate dataset — but that's a use of the *library*, not of this PDF.
- **HoTT-specific gotchas.** Definitions of image, surjectivity, finiteness, and equality "require type-theoretical finesse"; equality is a *type* (identifications), univalence equates identity-of-types with equivalence. These are correctness traps a verifier/autoformalizer over dependent type theory must respect (e.g. propositional truncation, truncation levels, function extensionality as an axiom). Relevant to the **formalize portfolio** and **verification gate** only as domain knowledge, not as a technique to implement.

## 4. Buildable-now vs. gated

Because the source supplies no algorithms, there is essentially nothing here that is "buildable-now" in the ML/search sense. Honest breakdown:

- **Buildable-now (data/plumbing, low value):** ingest agda-unimath as an optional retrieval corpus / autoformalization target set; add HoTT/dependent-type test theorems (π₁(S¹)=ℤ, infinitude of primes, gcd well-ordering) as fixtures for the eval harness. No GPU/model needed.
- **Gated:** N/A from this source — there is no model/data/GPU-gated technique in the document to gate on.
- **Not applicable:** everything the brief anticipated (proof search, premise selection models, RL/expert-iteration, meta-verifier, tactic LMs). The document simply isn't about these.

## 5. Techniques we have NOT yet built

From this source specifically: **none that are new to us.** The only Theoremata-adjacent gap it even gestures at is **first-class support for a dependent-type / HoTT backend** (Agda-cubical / univalent foundations) alongside Lean/Rocq/Isabelle — i.e., whether the FormalSystem abstraction can host a univalent library where equality is proof-relevant. That is a scope question, not a technique this book teaches.

## 6. Injection check

**No prompt-injection detected.** All 359 pages are ordinary mathematical prose, inference rules, and proofs. The only repeated text is the CUP copyright/errata footer (an author email + a Zulip chat link), repeated once per page — a legitimate publisher notice, not an instruction to any reader or agent. No "ignore previous instructions", role-play, system-prompt, or tool-directed content anywhere. 100% of content was treated as untrusted data; nothing in it was acted upon.

## Prioritized adopt-list (calibrated to what this source actually offers)

1. **Re-file this PDF.** It is mis-shelved under `atp/`. It is a HoTT textbook, not ATP/ML literature; treat it as *mathematical domain content*, not a methods source. (No code impact.)
2. **(Optional, low) HoTT/dependent-type eval fixtures.** Add a handful of the book's headline results (π₁(S¹)=ℤ, infinitude of primes, injectivity of succ / Peano 7–8, gcd via well-ordering) as target statements for the eval harness — useful *only if* Theoremata wants coverage of proof-relevant-equality math.
3. **(Optional, speculative) agda-unimath as a corpus.** If a dependent-type/univalent retrieval or autoformalization corpus is ever wanted, agda-unimath [22] is a clean, uniformly-structured, fully machine-checked candidate. This is adopting the *library*, not this PDF.
4. **(Scope flag, not an adopt) univalent FormalSystem backend.** Note for the formal-systems roadmap: a univalent/cubical-Agda backend would stress the FormalSystem abstraction differently (proof-relevant equality, truncation levels, univalence-as-axiom). Park as a research question; nothing in this book obliges building it.

Net: **near-zero method transfer, minor optional data value.** The high-signal takeaway is the correction itself — the brief's premise about this file was wrong, and the ML-for-proving material it wanted must be sourced elsewhere (it is well covered by the paper-mining and SOTA-gap notes already in memory).
