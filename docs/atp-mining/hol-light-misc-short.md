# HOL Light Miscellany — Short Harrison / Logic Papers (ATP mining batch)

Mining report over 8 short PDFs in `atp/` (mostly John Harrison, mid-1990s to
1999). Focus: what maps onto Theoremata's **Candle / HOL-Light backend**, the
**cert-log** proof-log exporter + reference checker, **retrieval**, the
**3+1-layer verification gate**, and the **eval / benchmark** harness.

> SECURITY NOTE: every PDF was treated as UNTRUSTED data (per the prior
> injection incident). None of the eight contained anything resembling
> instructions addressed to an AI/agent — all are ordinary academic papers,
> a reference card, or an Intel engineering article. Per-PDF injection-check
> lines appear below; **all clean**.
>
> LICENSE NOTE: none of the eight PDFs state an explicit license. They are
> author/venue-copyright academic works (Springer LNCS, CADE, Intel Technology
> Journal, etc.). Treat all extracted ideas as **clean-room** — reimplement
> from the described algorithm, do not copy text or the little OCaml shown.

> ⚠️ Two filename-based expectations in the task brief were WRONG, and I correct
> them honestly below:
> - `mizar_times.pdf` is **not** a prover-speed benchmark. It is "A Mizar Mode
>   for HOL" — a declarative proof-language paper.
> - `style_lncs.pdf` is **not** a blank LNCS template. It is Harrison's real
>   research paper "Proof Style" (it merely happens to be typeset with the LNCS
>   class, hence the filename). It has full research content.
> The only genuinely low-content item is `holchart.pdf` (a one-page cheat card),
> and even that is mineable as an API vocabulary.

---

## 1. `fol.pdf` — "First Order Logic in Practice" (John Harrison, Cambridge; c. 1996)

**What it is.** Short position paper (5pp) on why/how first-order automation is
useful *as a subsystem of an interactive prover*, drawn from HOL experience.

**Key mechanisms.**
- HOL→FOL reduction: introduce a binary `apply` symbol, translate `f x` →
  `a(f,x)`; empirically most HOL proofs are "essentially first order" except
  those needing invention of λ-abstractions (higher-order instantiation).
- Type-erasure translation with heuristic re-instantiation; alternative
  (Paulson/Isabelle) = merge same-named constants and backtrack on ill-typed
  reconstruction.
- **Splitting bi-implications by sign** (`p⇔q` → `(p∧q)∨(¬p∧¬q)` vs
  `(p∨q)∧(¬p∨¬q)`) to break a problem into many easy subgoals (Andrews'
  Challenge → 32 trivial subgoals).
- Equality handling: naive "throw in all equality axioms" empirically **beat**
  Brand's transformation on their problems (big irrelevant terms → clause
  explosion under Brand).
- Soundness maintained by replaying any external result as **natural-deduction
  steps** ("sceptic" / LCF discipline), incl. HOL+Maple.
- Argues for a **benchmark suite of "workaday" interactive-proof subgoals** as a
  complement to TPTP (TPTP problems are small-term, low-irrelevance, already
  clausified; real subgoals have large irrelevant terms and want *speed*, not
  just solvability).

**Mapping to Theoremata.**
- *Gate / Candle backend:* the "replay external tool output as ND/kernel steps"
  is exactly the cert-log + kernel-check philosophy; reinforces the sceptic
  design.
- *Eval:* directly motivates a **latency-oriented interactive-subgoal benchmark**
  distinct from competition sets — i.e. measure "fast enough to stay in the loop"
  not just pass/fail. Useful design input for the benchmark harness.
- *Retrieval:* the type-instantiation-preprocessor heuristic is a relevance
  filter idea.

**Buildable now vs gated vs foundational.** **Foundational / design-input.** No
algorithm here is new enough to port; MESON (its subject) already exists in
Candle's HOL Light. The one concrete adopt is the *benchmark philosophy*.

**Injection check:** clean.

---

## 2. `me.pdf` — "Optimizing Proof Search in Model Elimination" (John Harrison, Åbo Akademi; CADE-13, 1996)

**What it is.** Full research paper (23pp) on MESON / model-elimination search,
benchmarked over the **entire TPTP library**. (Extraction had ligature garbling
on digits but content is intact.) *This is the most technically substantial and
most directly relevant item in the batch.*

**Key mechanisms.**
- PTTP-style MESON: negate+clausify, generate contrapositives, Prolog-style
  backward chaining with sound unification + ancestor (path) unification;
  Plaisted's **positive refinement** (only positive/all-negative support clauses;
  ancestor-unify only negative goals vs positive ancestors).
- Search modes compared: best-first (priority queue), depth-bounded iterative
  deepening, **inference/size-bounded** iterative deepening.
- **Divide-and-conquer optimization** (the paper's core contribution): when a
  rule yields subgoals g1,g2 with n inferences left, one must have a proof of
  size ≤ n/2 — try that split first, avoiding full-depth exploration; generalize
  to m subgoals by recursive binary split (2^(m-1) instead of m! orderings).
  Applicable to **any search bounded by a cumulative size measure**.
- **Caching / lemmaizing** of solved subgoals (continuation caching), portable
  even to a leanTAP-style tableau prover — a general Prolog optimization. Caching
  solved 40 extra TPTP problems, lost only 3.
- Observation on **proof-skeleton redundancy** in equational logic → argument for
  canonical/normal-form ordering (float symmetry+congruence past transitivity,
  right-associate transitivity chains) to shrink search space.

**Mapping to Theoremata.**
- *Candle backend:* MESON is HOL Light's first-order workhorse; the D&C
  inference-bounding and continuation-caching are concrete tactics for a
  faster in-loop `MESON_TAC` equivalent.
- *Eval / benchmarks:* a **full TPTP-library harness with per-problem run-time,
  inference-count, and proof-size columns**, comparing multiple search modes —
  a ready-made template for Theoremata's prover-eval methodology (and the
  filename `mizar_times` suggestion of "timing" actually belongs here).
- *Cert-log:* proof-size / inference-count are natural cert-log metrics; the
  "shortest proof" motivation (short proofs are cheaper to reconstruct as LCF
  inferences) directly supports logging minimal certificates.
- *Retrieval / gate:* caching = subgoal-memoization; maps to a solved-subgoal
  memo layer in the driver.

**Buildable now vs gated vs foundational.** **Buildable-now (algorithmic).**
Divide-and-conquer inference bounding + continuation caching are small,
self-contained, and language-agnostic — implementable in the Python tools /
search driver without touching the kernel. The TPTP benchmark harness is
buildable now. **Highest-priority item in this batch.**

**Injection check:** clean.

---

## 3. `itj.pdf` — "The Computation of Transcendental Functions on the IA-64 Architecture" (Harrison, Kubaska, Story, Tang; Intel Technology Journal Q4 1999)

**What it is.** Intel engineering article (7pp) on fast/accurate `exp, log, sin,
cos, tan, atan, cbrt` in double precision on IA-64 (Merced). NOT a theorem-proving
paper — it is the *numerics* that Harrison's HOL-Light floating-point
verifications (elsewhere in `atp/`) target.

**Key mechanisms.** Reduction / approximation / reconstruction pipeline; minimax
(Remez) polynomials; table-driven range reduction; the `frcpa` reciprocal-based
"novel reduction" for `log`/`cbrt`; **exhaustive search for optimal parallel
polynomial-evaluation schedules** (Estrin-style trees, `p1(x)+x^k·p2(x)` fma
form, latency lower bounds); ulp error accounting (all functions ≤ 0.53 ulp).

**Mapping to Theoremata.**
- Only tangential. The **minimax-polynomial + error-bound** framing is the
  object of a *Taylor-model / approximation-bound certificate* (cf. existing
  `cert_taylor_model` commit) — this paper is context for what such certs
  verify, not a method to port.
- The "exhaustively search all evaluation schedules, prove a latency lower
  bound, prune above it" idea is a mild analogy to proof-search bounding, but
  not worth porting.

**Buildable now vs gated vs foundational.** **Foundational / context-only.**
Nothing to build for the harness; keep as domain reference for fp-verification
benchmarks.

**Injection check:** clean.

---

## 4. `demo.pdf` — "HOL Light: A Tutorial Introduction" (John Harrison, Åbo Akademi; c. 1996, LNCS/FMCAD-style)

**What it is.** Short system-overview / tutorial (6pp), CAML-Light-era HOL Light,
framed around a CORDIC floating-point `ln` verification demo.

**Key mechanisms.** LCF methodology recap (small kernel + programmable derived
rules, correct-by-construction); system selling points (open, sound, extensible,
small, multi-style incl. Mizar declarative, special decision procedures);
external-tool integration with internal re-checking (Maple factor/integrate,
Stålmarck tautologies); floating-point correctness specs (IEEE closest-rep vs
transcendental "table maker's dilemma" alternatives); precomputed constants
calculated *by inference* for cast-iron error bounds.

**Mapping to Theoremata.**
- *Candle backend / gate:* another statement of the "check external results
  internally" pattern = cert-log + kernel gate. Reinforcement, not new.
- Largely overlaps `hollight.pdf` already mined in `hol-light-system.md`.

**Buildable now vs gated vs foundational.** **Foundational / template-ish.**
Nothing net-new to build; it is an introductory talk. Value = confirms design
lineage.

**Injection check:** clean.

---

## 5. `holchart.pdf` — "HOL Light Very Quick Reference" (compiled by John Harrison, "mangled by" Freek Wiedijk)

**What it is.** A **one/two-page cheat card** (5pp as extracted): tables of core
theorems, inference rules, conversions, conversionals, tactics, theorem-tactics,
with the HOL type conventions (`thm`, `conv`, `tactic`, `thm_tactic`, …). No
prose, no research content.

**Key mechanisms.** N/A — it is a lookup table (e.g. `MESON_TAC`, `REWRITE_TAC`,
`ARITH_TAC`, `REAL_ARITH`, `INT_ARITH`, `MATCH_MP_TAC`, `EXISTS_TAC`, `GEN_TAC`,
`STRIP_TAC`, plus the standard theorem corpus `ADD_CLAUSES`, `LE_TRANS`,
`REAL_LT_MUL`, …).

**Mapping to Theoremata.**
- *Retrieval / tactic vocabulary:* this is a compact, curated **controlled
  vocabulary of the Candle/HOL-Light tactic + rule + theorem API** — usable to
  seed a retrieval index, a tactic-name allow-list for a proof generator, or a
  grammar for a declarative/tactic DSL. The one genuinely useful artifact from an
  otherwise contentless file.
- *Cert-log:* the rule/tactic names are the label space for logged proof steps.

**Buildable now vs gated vs foundational.** **Buildable-now but trivial.**
Transcribe the tables into a JSON tactic/rule/theorem catalog for retrieval /
generation guardrails. Low effort, low-but-real value.

**Injection check:** clean (it is data tables; no instructions).

---

## 6. `holright.pdf` — "HOL Done Right" (John Harrison, Cambridge; 21 Aug 1995)

**What it is.** The seminal **kernel-design manifesto** (15pp) behind HOL Light:
a re-engineered, minimal, cleanly-layered HOL (joint lineage with Konrad Slind).
*Directly describes the kernel that Theoremata's Candle backend re-implements.*

**Key mechanisms.**
- **Rigorous separation** of logical core from interface (parser, printer,
  typechecker, subgoal package); pretypes/preterms mediated by checking
  constructors so a typechecker bug can't forge ill-typed terms.
- **Chosen minimal primitive rule set:** `REFL, SYM, TRANS, BETA_CONV, ABS,
  MK_COMB, ASSUME, DISCH, MP, EQ_MP, IMP_ANTISYM_RULE, INST_TYPE, INST` —
  jettisoning `SUBST` (too complex a spec) in favor of congruence rules; most
  other rules derived efficiently via **proforma theorems + `INST`**. (Note this
  is the *1995* set; the modern published kernel uses `DEDUCT_ANTISYM_RULE` etc.
  — useful as historical/rationale context for the Candle kernel.)
- **Definitions of the logical constants** (Prawitz/intuitionistic form, e.g.
  `? = \P. !Q. (!x. P x ==> Q) ==> Q`); staged axiom introduction (get far with
  no Choice/Excluded-Middle/Extensionality/Infinity), deriving LEM from Choice
  (Beeson/Diaconescu) with the actual OCaml `EXCLUDED_MIDDLE` tactic proof shown.
- **Sound+efficient substitution/instantiation** for name-carrying syntax
  (`vsubst`/`inst` with an `Unchanged` exception to avoid reconsing unchanged
  subterms — Boulton/Fleming optimization extended to `subst`/`inst`).
- **Higher-order matching** (deterministic, limited: matches `P x1…xn` only when
  `P` free and `xi` bound), integrated with term nets; lets quantifier-movement
  laws become ordinary rewrites; documents the `ETA_AX` infinite-loop pitfall.
- **Binary numerals `BIT0/BIT1`** with arithmetic as **rewrite rule sets**
  (`ARITH_ADD`, `ARITH_LE`, `ARITH_SUB`) — replacing the unsound `mk_thm` numeral
  axiom schema; proof-based arithmetic much faster.
- Prescribed **logical-development build order** (equality → intuitionistic
  boolean → matching/rewriting → AC tools → tactics → inductive defs → classical
  axioms → infinity/naturals → recursive types → reals).

**Mapping to Theoremata.**
- *Candle / HOL-Light backend:* this is essentially the **spec of the kernel**
  Candle verifies. The primitive-rule rationale, definitional discipline, and
  numeral representation are the exact semantics the cert-log reference checker
  must honor. Strong reference doc for the FormalSystem abstraction's HOL
  instance.
- *Cert-log:* the small primitive set = the minimal instruction set a proof log
  should serialize; "proforma theorem + INST" pattern shows how derived-rule logs
  compress.
- *Gate:* the core/interface separation and "typechecker bug can't forge terms"
  is the trust-boundary argument (cf. `docs/TRUST_BOUNDARIES.md`).

**Buildable now vs gated vs foundational.** **Foundational (reference), not a
port target** — Candle already provides the kernel. Value: authoritative
rationale + the `BIT0/BIT1` numeral-as-rewrites trick and HO-matching design if
the harness ever needs its own term layer. **Second-highest-value read** in the
batch (after `me.pdf`).

**Injection check:** clean.

---

## 7. `mizar_times.pdf` — "A Mizar Mode for HOL" (John Harrison, Åbo Akademi; c. 1996, LNCS)

**What it is.** (18pp) A **declarative proof-language** paper — NOT the
prover-timing benchmark the filename suggests. Adds a Mizar-style declarative
mode on top of HOL's LCF tactic machinery, backed by first-order automation to
discharge "obvious" steps.

**Key mechanisms.**
- **Skeleton constructs → HOL tactics** map: `assume`→`DISCH_TAC`(⇒-intro),
  `let`→`GEN_TAC`(∀), `take`→`EXISTS_TAC`(∃), `consider`→`CHOOSE_TAC`,
  `given`→`DISCH_THEN∘X_CHOOSE_TAC`, `suffices to show`→`MATCH_MP_TAC`,
  `set`→`ABBREV_TAC`; `thesis` tracked automatically; `then/hence/thus`
  linkage; iterated-equality (`... = c by …`) calculational chains.
- **`per cases`/`suppose`** implemented incrementally (each `suppose` splits into
  two subgoals, `end` proves under falsity) so proofs stay **interactively
  single-steppable** rather than batch — a deliberate improvement over Mizar's
  edit-compile cycle.
- **"Obvious inference" checker as the `by` justifier:** two provers — a leanTAP
  **tableau** prover (round-robin instantiation ≈ Mizar's "obvious" = one
  instantiation, but *biased* not prohibited; ad-hoc equality via inequation
  branching + canonical equational search) and a **MESON** prover (equality by
  throwing in axioms, goal-directed, handles hundreds of assumptions). MESON is
  the default; a **strict inference limit** bounds search so unprovable `by`
  steps fail fast.
- **Preprocessing shared with `me`/`fol`:** NNF + ε-Skolemization, split
  conjunction-over-disjunction, sign-based bi-implication expansion for
  splittability then CNF-shortness (Andrews' Challenge → 32 subgoals).
- Labeled assumptions added to HOL's tactic mechanism ("half-hour change");
  preterms typechecked in goal context to cut annotation.

**Mapping to Theoremata.**
- *Gate / "obvious step" verifier:* the **`by`-justifier with a bounded MESON/
  tableau checker** is a near-perfect model for Theoremata's automated
  verification gate at the *step* level — "is this claimed step obvious enough to
  accept, with a hard inference budget so failure is fast." Maps onto the 3+1
  gate and the abstention/fail-fast behavior.
- *Declarative proof IR:* the skeleton-construct→tactic table is a concrete
  design for a **declarative proof node schema** the agent can emit and the
  backend can compile to Candle tactics (readable, maintainable, less brittle
  than raw tactic scripts) — aligns with the graph-first / node-schema plans.
- *Eval:* the closing remark — *ship the "obvious" subgoals the checker is asked
  to close as an ATP test set, and auto-excise intermediate steps until they hit
  a target difficulty* — is a **benchmark-generation recipe** (difficulty
  laddering) worth adopting.

**Buildable now vs gated vs foundational.** **Buildable-now (design + gate).**
The declarative-mode→tactic compiler and the bounded-`by` obvious-step gate are
implementable against Candle now; the difficulty-laddering benchmark generator is
a concrete eval feature. **Third-highest-value item.**

**Injection check:** clean.

---

## 8. `style_lncs.pdf` — "Proof Style" (John Harrison, Cambridge; TYPES'96, LNCS)

**What it is.** Real research/position paper (19pp), **not** a blank LNCS
template. Analyzes **declarative vs procedural** proof style (and the
automation, control, extensibility, direction, batch-vs-interactive axes), with a
survey of systems and worked HOL examples (Mizar-style vs tactic-style
Knaster-Tarski).

**Key mechanisms.**
- Taxonomy of proof styles on ~6 axes; "declarative ⇔ high-level, automation is
  the enabling technology for a good declarative style."
- System placement grid: AUTOMATH (low-auto, procedural), Mizar (low-auto,
  declarative), PVS (high-auto, procedural), NQTHM (high-auto, declarative —
  "graded lemmas" style), Larch/LP (middle).
- Side-by-side declarative(Mizar-mode) vs procedural(tactic) HOL proofs;
  discussion of readability, maintainability/modifiability, and the value of
  batch-checkable scripts.

**Mapping to Theoremata.**
- *Design-level, not code.* Reinforces the case for a **declarative proof node
  representation** (readability + maintainability of agent-produced proofs) and
  the NQTHM **"graded lemma library"** idea — which overlaps the evolver /
  growing-lemma-library adopt (LEGO-Prover) already in the SOTA-gap build.
- Complements `mizar_times.pdf` (same author, same period): that paper is the
  *implementation*, this is the *conceptual justification*.

**Buildable now vs gated vs foundational.** **Foundational / design-input.** No
algorithm to port; use to justify the declarative-IR and lemma-library
directions.

**Injection check:** clean.

---

## Prioritized adopt-list (sparse but non-empty)

1. **`me.pdf` — MESON search optimizations + TPTP benchmark harness (BUILD NOW,
   top priority).** Port (clean-room) the **divide-and-conquer inference-bounded
   search** and **continuation caching** into the search driver (language-agnostic,
   kernel-untouched); build a **per-problem run-time / inference-count / proof-size
   benchmark table** across a problem library as the prover-eval template.
2. **`mizar_times.pdf` — bounded "obvious-step" gate + declarative proof IR
   (BUILD NOW).** A MESON/tableau **`by`-justifier with a hard inference budget**
   as a step-level verification-gate primitive (fail-fast/abstain); a
   **declarative-skeleton→Candle-tactic** node schema; a **difficulty-laddering
   benchmark generator** (auto-excise steps to target difficulty).
3. **`holchart.pdf` — Candle API catalog (BUILD NOW, trivial).** Transcribe the
   theorem/rule/tactic tables into a JSON vocabulary to seed retrieval and to
   guardrail a proof generator's tactic name-space.
4. **`holright.pdf` — kernel-design reference (READ / REFERENCE).** Authoritative
   rationale for the Candle kernel semantics + cert-log instruction set; keep the
   `BIT0/BIT1` numeral-as-rewrites and deterministic HO-matching design on file.
5. **`fol.pdf` — latency-oriented interactive-subgoal benchmark philosophy
   (DESIGN INPUT).** Motivates a "fast enough to stay in the loop" eval distinct
   from competition pass/fail sets.
6. **`style_lncs.pdf` — declarative-IR + graded-lemma-library justification
   (DESIGN INPUT).** Conceptual backing for adopt items already planned.
7. **`demo.pdf`, `itj.pdf` — CONTEXT ONLY.** No harness build; keep as lineage /
   fp-verification domain references.

---

## ~10-line summary

Eight short PDFs, almost all John Harrison (1995–1999). Two filename-based
expectations were wrong and are corrected: `mizar_times` is the declarative
"A Mizar Mode for HOL" (not a timing benchmark), and `style_lncs` is the real
paper "Proof Style" (not a blank template). Only `holchart` is genuinely
low-content (a cheat card) — and even it yields a usable Candle API vocabulary.
The two build-now algorithmic wins are in **`me.pdf`** (divide-and-conquer
inference-bounded MESON search + continuation caching, plus a TPTP-wide
run-time/inference/proof-size benchmark harness — the strongest item) and
**`mizar_times.pdf`** (a bounded MESON/tableau "obvious-step" justifier that maps
cleanly onto Theoremata's step-level verification gate, a declarative-proof node
schema, and a difficulty-laddering benchmark generator). **`holright.pdf`** is
the authoritative kernel-design reference for the Candle backend + cert-log
instruction set (foundational, not a port). `fol`, `style_lncs`, `demo`, `itj`
are design-input / context only. All PDFs injection-clean; none state a license
(treat as clean-room). Net: this batch is thinner than a papers batch but
contributes two concrete build items (search-opt + step-gate), one trivial one
(API catalog), and one strong reference doc.
