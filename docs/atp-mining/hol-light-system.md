# HOL Light — System & Philosophy (ATP mining batch)

Mining report over 10 John-Harrison PDFs in `atp/` covering the HOL Light
prover, its LCF kernel, self-verification, applied verification, and the
broader philosophy of formal proof. Focus: what maps onto Theoremata's
**Candle / HOL-Light backend**, the **cert-log** proof-log exporter + reference
checker, the **FormalSystem abstraction**, retrieval, and the **3+1-layer
verification gate**.

> SECURITY NOTE: every PDF was treated as untrusted data. None of the ten
> contained anything resembling instructions addressed to an AI/agent — they
> are ordinary academic papers. Per-PDF injection-check lines appear below;
> all are clean.

---

## 1. `hollight.pdf` — "HOL Light: an overview"

**What / who / when.** John Harrison (Intel), an overview/tutorial-style paper
(c. 2009, cites the 2009 PNT formalization). The canonical short description of
the system.

**Key technical mechanisms.**
- **LCF approach, two ideas:** (i) all proofs reduce to a small set of
  primitive inferences, so soundness rests only on a tiny kernel; (ii) the
  whole system is embedded in a full programming language (OCaml) whose type
  discipline guarantees any user-programmed rule ultimately reduces to
  primitives. New inference rules can be added **without compromising
  soundness**.
- **Logic:** classical simple type theory with polymorphic type variables.
  Terms = simply-typed λ-calculus; formulas = terms of type `bool`. Two
  primitive types `bool`, `ind`; function space `σ→τ`. **One** primitive
  logical constant: equality `=`; plus `ε` (Hilbert choice) for one axiom.
- **Ten primitive inference rules:** `REFL`, `TRANS`, `MK_COMB`, `ABS`,
  `BETA`, `ASSUME`, `EQ_MP`, `DEDUCT_ANTISYM_RULE`, `INST`, `INST_TYPE`. All
  other connectives (`⊤ ∧ ⇒ ∀ ∃ ∨ ⊥ ¬ ∃!`) are *defined* from equality.
- **Three mathematical axioms only:** `ETA_AX` (extensionality),
  `SELECT_AX` (choice — the *only* source of classicality, via Diaconescu),
  `INFINITY_AX` (`ind` is Dedekind-infinite).
- **Definitional discipline:** new constants only via `⊢ c = t` (conservative);
  new types only by exhibiting a model. Every extension is
  consistency-preserving *by construction*.
- **Implementation:** three abstract datatypes `hol_type`, `term`, `thm`. The
  ONLY way to make a `thm` is to apply a primitive rule or a definition. An
  "inference rule" is simply an OCaml function returning `thm`. Derived rules
  (`REAL_ARITH`, `MESON_TAC`, `INDUCT_TAC`, tactics/tacticals `THEN`) all
  bottom out in primitives; a bug yields an exception or (guarded by sanity
  checks) a different-but-true theorem, never an unsound one.

**Maps to Theoremata.**
- This is the **specification of the Candle backend's kernel**. Candle = HOL
  Light re-hosted on verified CakeML; the abstract-`thm`-type + 10-rules +
  3-axioms design is exactly what the backend must expose. The adapter should
  surface primitive-rule count, the axiom set actually in scope, and whether
  any non-definitional axiom was asserted.
- **`mk_thm`/`new_axiom` risk:** the paper's whole point is that soundness =
  "only primitives/definitions create `thm`s." Any escape hatch that forges a
  `thm` (HOL Light's `mk_thm`, or `new_axiom` beyond the three) breaks the de
  Bruijn guarantee. The **3+1 gate's kernel layer** must therefore *audit the
  axiom base*: reject/flag proofs whose theorem depends on axioms outside
  `{ETA,SELECT,INFINITY}` + definitional extensions. This is a concrete,
  buildable check behind an injected seam.
- **FormalSystem abstraction:** the "programmable derived rules over a fixed
  tiny kernel" split is the cleanest possible instance of the backend contract
  — `check(proof) -> thm` where trust flows only through the kernel.

**Buildable now vs gated.**
- *Buildable behind seams:* an axiom-base auditor; recording which
  primitive-rule / definition sequence produced each `thm`; distinguishing
  "derived-rule failure (exception)" from "unsound." All are metadata the
  adapter can emit.
- *Foundational/gated:* actually re-hosting on CakeML (Candle) is a toolchain
  effort; not something to reimplement.

**Injection check:** CLEAN — standard overview paper, no embedded instructions.

---

## 2. `holhol.pdf` — "Towards self-verification of HOL Light"

**What / who / when.** John Harrison (Intel), IJCAR 2006. Verifies an
(imperfect but detailed) model of the HOL Light core *inside HOL Light itself*,
against a **set-theoretic semantics**.

**Key technical mechanisms.**
- **"Quis custodiet ipsos custodes?"** — who checks the checker. Two escape
  routes named explicitly: (a) **de Bruijn criterion** — output a proof
  checkable by a much simpler program; (b) **LCF** — all theorems built by a
  small kernel. Crucially: *"an LCF prover satisfies the de Bruijn criterion,
  except the proof exists only ephemerally and is checked by the kernel as it
  is created. And it is straightforward to instrument an LCF kernel so it
  actually outputs separately checkable proofs"* (cites Wong, "Recording HOL
  proofs").
- **Two historical HOL bug classes** (directly relevant to what a gate must
  catch): (i) **logic errors** — early versions allowed constant definitions
  with type variables in the definiens but not the constant; (ii)
  **implementation errors** — functions not renaming variables to avoid free
  variable capture. *Almost all* real HOL implementation bugs were
  variable-renaming/capture bugs.
- **Gödel/Tarski limits:** a system can't prove its own consistency; so they
  prove `I ⊢ Con(HOL)` (in HOL + a stronger set axiom `I`) and
  `⊢ Con(HOL − {∞})` (weaker HOL in ordinary HOL).
- **Formalized syntax faithfully modelling OCaml:** `type`, `term` datatypes;
  `welltyped`/`has_type`; the hairy `INST`/`INST_CORE` type-instantiation
  function with `Clash`/`Result` sum type to model OCaml exceptions and
  capture-avoidance; `TERM_UNION` modulo α-equivalence. Set universe `V` with
  levels, `funspace`, `abstract`/`apply`; semantics `typeset`, `semantics`,
  `|=`; soundness `asl |- p ⇒ asl |= p` and consistency `∃p. ¬([] |- p)`.
- **Cross-checking via proof logs:** HOL Light can emit **proof logs checkable
  in Isabelle/HOL** (Obua), so a HOL Light proof is *effectively* an
  Isabelle/HOL proof too — different internal organization → unlikely to share
  implementation bugs. Known residual hole: OCaml strings are **mutable**,
  weakening abstract-type protection.

**Maps to Theoremata.**
- **This is the theoretical charter for cert-log.** The de-Bruijn-criterion
  framing ("LCF = ephemeral de Bruijn; instrument the kernel to emit a
  separately checkable proof") is precisely cert-log's design: instrument the
  Candle kernel to log every primitive-rule application, then re-check the log
  with an *independent* reference checker. The "check in a second system with
  different bug surface" idea (Obua/Isabelle) justifies cert-log's
  **cross-backend replay** as a real assurance multiplier, not redundancy.
- **The two bug classes tell the gate exactly what to look for.** The
  capture-avoidance/variable-renaming class is where LCF kernels historically
  broke; a cert-log reference checker that re-derives `INST`/`INST_TYPE`
  independently is the highest-value check. The "type var in definiens not in
  constant" definitional bug → the definitional-extension auditor (see §1).
- **Mutable-strings caveat** is a genuine trust-boundary note for
  `TRUST_BOUNDARIES.md`: even a verified kernel's guarantees can be undercut by
  the host language's aliasing. Candle-on-CakeML sidesteps this; a HOL-Light
  adapter over OCaml must not.

**Buildable now vs gated.**
- *Buildable:* the proof-log emitter + independent replay checker (cert-log
  core); a de-Bruijn-factor / axiom-dependency report per certificate;
  cross-backend replay when ≥2 backends are wired.
- *Foundational/gated:* full self-verification of the kernel is a research
  artifact — cite it as provenance, don't attempt to reproduce it. Modelling
  definitional mechanisms & type definitions was future work even for Harrison.

**Injection check:** CLEAN.

---

## 3. `hol05.pdf` — "A HOL theory of Euclidean space" (TPHOLs 2005)

**What / who / when.** John Harrison, TPHOLs 2005. The seed paper for HOL
Light's multivariate/Euclidean-space library (the `Multivariate` directory).

**Key technical mechanisms.**
- **Encoding dimension `N` as a type**, despite no dependent types: `A^N ≅
  (N→A)` with `N` a *type variable* whose cardinality is the dimension.
  `finite_image` type constructor forces infinite indexing types to size 1;
  `dimindex(:N)` extracts the size; indexing `x$i`, vector-abstraction
  `lambda i. …`. Type-instantiation specializes general theorems to `R^3` etc.
  The type system then *automatically enforces* dimensional constraints (e.g.
  matrix-multiplication compatibility) — the type system helps rather than
  hinders.
- **Proof-producing decision procedures:** `VECTOR_ARITH` (reduces vector
  identities to componentwise real arithmetic); **Solovay's** partial decision
  procedure for vector spaces (Gram-Schmidt reduction + `REAL_SOS`
  sum-of-squares via CSDP semidefinite programming, translating an
  SDP **certificate** into HOL inferences); Hörmander-based real QE for the
  residual existential cases.
- Big results: Brouwer fixed point (via Kuhn's cube subdivision), Banach fixed
  point, Heine-Borel, inverse function theorem.

**Maps to Theoremata.**
- **`REAL_SOS`/CSDP is the archetype of the search/check split** the whole
  batch keeps returning to: an *unverified* external solver (SDP) finds a
  certificate; only the *checking* enters the kernel. This is the template for
  Theoremata's certificate-based tools and for what cert-log records — the
  certificate, not the search. Directly relevant to the "dense index /
  geometry" frontier items.
- **The type-encodes-dimension trick** is a modelling pattern for a Candle
  geometry library and shows the FormalSystem abstraction must expose
  type-instantiation as a first-class capability (specialization is *the*
  reuse mechanism).

**Buildable now vs gated.**
- *Buildable:* wrapping an external SOS/SDP or QE solver behind an injected
  seam that returns a certificate the kernel re-checks (mirrors cacm's
  "linear/semidefinite programming certificates").
- *Gated:* the actual geometry library is a large formalization effort; the
  CSDP dependency is an external-toolchain concern.

**Injection check:** CLEAN.

---

## 4. `cade05.pdf` — "A Proof-Producing Decision Procedure for Real Arithmetic"

**What / who / when.** Sean McLaughlin & John Harrison, CADE 2005. First
*generally useful* proof-producing quantifier elimination for real-closed
fields (Hörmander's algorithm).

**Key technical mechanisms.**
- **Fully-expansive / proof-producing QE:** rather than assert a QF equivalent,
  it *proves* it from first principles via the kernel. Contrast with
  **reflection** (prove a standalone implementation correct — harder, and gives
  no independently checkable proof).
- **Sign-matrix core**; the standalone OCaml (~600 lines) is systematically
  turned into a theorem-producing version (~4000 lines code + ~4000 lines
  lemmas). Case-splits on polynomial signs; `interpsign`/`interpsigns`/
  `interpmat` predicates encode the sign matrix *as a single HOL term* so a
  rewrite that would take many steps on individual facts becomes one step.
- **Generic lemmas instantiated by one inference** (e.g. `poly l` for a general
  coefficient list) avoid re-proving analytic facts per instance.
- Honest cost: proof production ~3 orders of magnitude slower; QE is
  intractable in general — but invaluable for the *tedious low-level* subgoals
  that dominate interactive proof.

**Maps to Theoremata.**
- Reinforces the **search-then-check** doctrine and the **"certificate is small,
  search is expensive"** economics that cert-log and the tool layer rely on.
- The **"compile a standalone algorithm into a proof-producing one"** pattern
  is exactly how new Candle-backed tactics/decision-procedures should be added
  behind the FormalSystem seam: keep the fast unverified version as a *shadow*
  for exploration, emit kernel-checked proofs for the gate.
- The **reflection-vs-LCF tradeoff** is a design decision for the backend: LCF
  (proof-producing) gives independently checkable artifacts (good for
  cert-log); reflection gives speed but no external certificate. Theoremata
  should default to proof-producing at the gate.

**Buildable now vs gated.**
- *Buildable:* the shadow-function pattern (unverified explore, verified
  commit); encoding intermediate proof state as compact terms.
- *Gated:* implementing full real QE is a large effort and often too slow;
  treat as an optional tool, not a core dependency.

**Injection check:** CLEAN.

---

## 5. `fmcad00.pdf` — "Formal verification of floating point trigonometric functions"

**What / who / when.** John Harrison, FMCAD 2000. Deep case study: verifying
IA-64 `sin`/`cos` (range reduction + core polynomial evaluation).

**Key technical mechanisms.**
- Whole verification runs **inside HOL Light**, founded on formalized real
  analysis + floating-point theory. Reiterates LCF: proofs reduce to primitives;
  proofs are large & usually not stored; abstract `thm` type maintains the
  reduction (here CAML Light).
- **Custom proof-producing derived rules** for the domain: `MACHIN_RULE`
  (arctan linear forms), `PI_APPROX_RULE`, `MCLAURIN_COS_POLY_RULE`
  (Taylor-polynomial + error bound), a polynomial-approximation error bounder
  (root isolation via interlocking derivative recursions), diophantine-
  approximation via Stern-Brocot/mediant convergents to bound how close a float
  can be to a multiple of π/2. **Shadow functions** produce polynomials without
  proof for fast exploration, then the proving version certifies them.
- The `(1+ε)` rounding property and exactness/cancellation lemmas.

**Maps to Theoremata.**
- Canonical demonstration that **programmability of derived rules over a fixed
  tiny kernel** is what makes hard verification feasible — the core value
  proposition of the Candle backend and the FormalSystem contract.
- **Shadow-function pattern again** (explore unverified, certify at commit) →
  a concrete idiom for Theoremata tools behind injected seams.
- Motivates why the backend must expose a **library of pre-proved analysis
  lemmas** as retrievable units (see retrieval/dense-index items): these
  verifications are "incessant" users of the real-analysis library.

**Buildable now vs gated.**
- *Buildable:* the shadow/certify idiom; treating domain lemmas as retrievable
  library entries.
- *Gated:* the FP theory + these specific verifications are large, specialized
  formalizations — reference, don't rebuild.

**Injection check:** CLEAN.

---

## 6. `notices.pdf` — "Formal Proof — Theory and Practice" (Notices of the AMS, Dec 2008)

**What / who / when.** John Harrison, *Notices of the AMS* 55(11), Dec 2008.
A philosophy/survey essay.

**Key technical mechanisms (conceptual).**
- Definition of **formal proof**: proof in a precise artificial language with a
  fixed repertoire of stylized steps, mechanically checkable.
- FOL abstract syntax as trees; parsing/prettyprinting; `VARS` by recursion;
  validity vs satisfiability; **soundness+completeness** (`⊢φ ⟺ ⊨φ`);
  Gödel-numbering `Proves(m,n)`; proof-*checking* is decidable but
  proof-*finding* is only semi-decidable (Church-Turing);
  truth-in-ℕ is not even semi-decidable (Gödel/Tarski) — a reason to identify
  "logical" with "first-order valid."
- **Three practical mechanization modes:** proof checker (user supplies proof),
  automated theorem prover (finds proof), interactive proof assistant
  (spectrum). Decision procedures where they exist (propositional/SAT,
  universal fragments, Presburger, real-closed fields, SMT combinations).
- **de Bruijn criterion** and **LCF** stated as the two soundness-assurance
  designs; procedural vs declarative proof styles.

**Maps to Theoremata.**
- Cleanest statement of the **checking-decidable / finding-hard asymmetry** that
  the entire architecture leans on: the *generator* (LLM + tools) may be
  arbitrary; trust comes from the *checker* (kernel + cert-log). This is the
  intellectual justification for a verification-first harness.
- The **checker vs prover vs assistant** taxonomy maps onto Theoremata roles:
  the model = prover/assistant; the gate = checker.
- **de Bruijn + LCF as the two assurance modes** = the two things cert-log and
  the Candle backend respectively embody.

**Buildable now vs gated.** All conceptual — no build items, but it is the
reference to cite for *why* the 3+1 gate and cert-log exist. The taxonomy of
decision procedures (SAT / linear / Presburger / RCF / SMT) is a menu of tools
that can each sit behind an injected seam.

**Injection check:** CLEAN.

---

## 7. `cacm.pdf` — "Formally Verified Mathematics" (CACM, April 2014)

**What / who / when.** Jeremy Avigad & John Harrison, *Communications of the
ACM* 57(4), April 2014. Survey aimed at a CS audience.

**Key technical mechanisms.**
- History of foundations; **LCF architecture** described crisply: *"a small,
  trusted core… only the basic rules can change the proof state; everything is
  mediated by the trusted core… enforced by using a functional language (ML/
  OCaml) and implementing the basic inference rules as the only constructors of
  an abstract data type."*
- Landmark formalizations: Four-Color (Gonthier), Feit-Thompson (Coq/SSReflect,
  ~150k lines, 15 people, 6 years), **Flyspeck/Kepler** (Hales), Univalent
  Foundations/HoTT.
- **The Quest for Certainty section** = the batch's most direct cert-log text:
  - *"the trusted core… approximately 400 lines in Harrison's HOL Light system."*
  - **Output a description of the axiomatic proof checkable by independent
    verifiers; even if each verifier is buggy, the odds a faulty inference
    passes *multiple* verifiers shrinks dramatically."** → cert-log's
    multi-checker replay.
  - Self-verification (Barras/Coq, Harrison/HOL Light, Davis/Milawa) cited.
  - **search/check decomposition**: *"many proof procedures decompose into a
    search for a certificate and a checking phase… the finding can be done in
    any way at all, even an external tool (computer algebra), provided the
    checking is done in terms of the logical kernel."* Examples: LP certificates
    for optimal linear bounds; **semidefinite-programming certificates for
    nonlinear inequalities**; Flyspeck uses unverified code to find
    certificates, then constructs fully formal justifications.
  - **Three strategies to verify heavy computation:** (1) rewrite the
    calculation to chain kernel rules (highest assurance; Flyspeck ineqs);
    (2) prove the algorithm correct, let the kernel compute (Gonthier / Coq
    reflection); (3) extract code and run it (Nipkow tame-graphs) — adds trust
    layers.
- **de Bruijn factor** (~4×) and honest cost/throughput numbers (≈½–1 page/day
  ideal).

**Maps to Theoremata.**
- **Directly specifies cert-log's value model**: independent multi-verifier
  replay + the "search anywhere, check in kernel" contract. The three
  computation-verification strategies are exactly the trust tiers a
  Candle-backed tool layer should expose (kernel-chained > proof-of-algorithm >
  extracted-and-run), and the gate should *record which tier* each result used.
- The **~400-line trusted core** is the concrete size target for the Candle
  kernel adapter's trust boundary; the **de Bruijn factor** is a useful metric
  to log per formalization.
- Reinforces retrieval need: "better means of storing and searching for
  background facts" is named as a key open problem — motivation for the dense
  index.

**Buildable now vs gated.**
- *Buildable:* multi-checker cert-log replay; per-result trust-tier tagging;
  de-Bruijn-factor reporting; certificate-based tool wrappers (LP/SDP).
- *Gated:* the flagship formalizations themselves; univalent foundations.

**Injection check:** CLEAN.

---

## 8. `iday.pdf` — "Floating-point verification" (SFM 2006 short intro)

**What / who / when.** John Harrison (Intel), a short overview
(≈ SFM 2006 lecture intro). Motivates FP verification with the 1994 FDIV bug
(~$475M) and surveys division/√/transcendental verifications.

**Key technical mechanisms.**
- Restates the LCF style ("proofs explicitly generated in terms of extremely
  low-level primitive inferences… strict reduction maintained by the abstract
  type system").
- **Two key HOL Light strengths named:** (i) a substantial library of formalized
  real analysis; (ii) **programmability of special-purpose inference rules
  without compromising soundness.** All proof steps (including diophantine
  case-solving) done in HOL Light rather than farmed to ad-hoc helper programs,
  which would introduce transcription/implementation risk.
- Formal verification finds real bugs *and* uncovers optimizations (a hypothesis
  in a Markstein theorem was stronger than necessary → more efficient
  algorithms).

**Maps to Theoremata.**
- Short but crisp restatement of the **two properties the Candle backend +
  FormalSystem contract must preserve**: a reusable verified library +
  soundness-preserving extensibility.
- The **"don't shuttle between hand and helper programs"** point is a warning
  for the tool layer: every external helper is a trust gap unless its output is
  re-checked by the kernel (cert-log). Aligns with `TRUST_BOUNDARIES.md`.

**Buildable now vs gated.** Conceptual/overview; no new build items beyond what
§5/§7 give. Reference for the "why verify in-kernel not via helpers" rationale.

**Injection check:** CLEAN.

---

## 9. `neworleans.pdf` — "The HOL Light Theory of Euclidean Space" (JAR 2013)

**What / who / when.** John Harrison, *J. Automated Reasoning* 50:173–190
(2012/2013). The mature description of the `Multivariate` library (started 2005,
revision 130: **9724 named theorems, ~118k lines** across `vectors.ml`,
`topology.ml`, `convex.ml`, `integration.ml`, `measure.ml`, `cauchy.ml`, etc.).

**Key technical mechanisms.**
- Reiterates the **type-encodes-dimension** encoding; `dimindex(:N)`; `lift`/
  `drop` between `R^1` and `R`.
- Big theorems (Brouwer, Krein-Milman, Riemann Mapping, Jordan Curve, Tietze,
  Stone-Weierstrass, Fashoda, dominated convergence, invariance of domain) —
  but the paper's *thesis* is that **modest technical results and outright
  "trivialities" dominate the real work.** Formalized libraries must record
  many facts a textbook wouldn't dignify with "Lemma n" because automation is
  too weak to reproduce them on demand; degenerate side-conditions (e.g. a
  "hyperplane" with `a=0`) must be handled that informal proofs silently skip.
- **Named automation tools:** `MESON` (built-in FO prover); a componentwise
  vector-algebra reducer; a decision procedure for the universal additive
  theory of normed spaces; and a **"without loss of generality" tool** that
  exploits a *database of invariance theorems* (translation/scaling/orthogonal
  transformation) to justify convenient coordinate choices automatically.
- Notes the value of **importing HOL Light into Coq** (Keller-Werner) and the
  wish to reuse proofs across systems / generalize "morally the same" proofs.

**Maps to Theoremata.**
- **Retrieval / dense-index gold:** 9724 named theorems with a clear
  build-order file structure = a realistic model for how a Candle library should
  be indexed and retrieved. The "trivialities must be stored and named" point is
  a *direct argument for a lemma library + retrieval*: automation won't
  re-derive them, so they must be findable. This backs the growing-lemma-library
  / dense-index frontier items.
- The **invariance-database "WLOG" tool** is a reusable-automation pattern:
  tag theorems with the transformations under which properties are invariant,
  then let a tactic apply them — a concrete, buildable retrieval-driven tactic
  behind a seam.
- **Cross-system import (HOL Light → Coq)** is a real precedent for the
  FormalSystem abstraction and for cert-log's cross-backend replay: proofs
  crossing kernels is both an assurance multiplier and a portability story.

**Buildable now vs gated.**
- *Buildable:* indexing a theorem library by name/statement/file for retrieval;
  a WLOG/invariance-database tactic; measuring library coverage gaps as a
  driver signal (mirrors "each new application finds fundamental results
  missing").
- *Gated:* producing a comparable 118k-line library is a multi-year effort;
  algebraic-topology machinery was still an open gap.

**Injection check:** CLEAN.

---

## 10. `ESSLLI94.pdf` — early survey on inductive definitions in type theory (ESSLLI 1994)

**What / who / when.** A 1994 (ESSLLI — European Summer School in Logic,
Language & Information) survey/lecture-note on **inductive definitions as the
core of type theory** and their role bridging computer science, logic and
mathematics. Attribution to Harrison is consistent with the batch but not
verifiable from the file itself (see caveat).

> EXTRACTION CAVEAT: this PDF was produced by *GNU Ghostscript 7.07* with a
> custom/embedded font encoding that pypdf cannot map to Unicode. ~90% of the
> body extracted as garbage glyphs; only scattered English islands survived.
> The characterization below is reconstructed from those islands and the
> readable abstract/reference fragments and should be treated as **lower
> confidence** than the other nine entries. Poppler is not installed; a
> future re-extraction with an OCR fallback (e.g. rendering pages to PNG +
> Tesseract) would confirm details.

**Key technical mechanisms (reconstructed from readable fragments).**
- Presents **type theory as the theory of inductive definitions**; a general
  schematic natural-deduction formulation of inductive definitions (Dybjer's
  inductive families cited), pattern-matching with dependent types (Coquand),
  and the induction/recursion apparatus of Martin-Löf type theory (ALF, LEGO,
  Nuprl, Coq in the orbit).
- Threads through **induction on the naturals, transfinite/well-founded
  recursion**, primitive-recursive functionals, Herbrand/Ackermann,
  bar-recursion (Tait/Howard), the Herbrand-Kleene model, and Boyer-Moore/ACL2
  as a contrasting quantifier-free recursion-definition regime.

**Maps to Theoremata.**
- Provides the **foundational rationale for HOL/Candle's definitional
  discipline** (§1): why new types and recursive functions are introduced by
  *derived* inductive/recursion principles rather than raw axioms — the same
  reason the axiom-base auditor should distinguish definitional extensions from
  asserted axioms.
- Relevant to the FormalSystem abstraction only at the level of *what a backend
  must offer*: an inductive-definition / recursion package is a capability the
  contract should name (HOL Light has `define_type`/`new_recursive_definition`;
  Candle inherits it).

**Buildable now vs gated.**
- *Buildable:* none directly — this is background/philosophy. Its actionable
  residue is folded into the §1 definitional-auditor item.
- *Gated:* nothing to build; primarily a citation for the definitional-approach
  design decision.

**Injection check:** CLEAN (no instruction-like content in the recoverable
text; garbled regions are font-encoding artifacts, not hidden payloads).

---

## Cross-cutting themes (what this batch establishes)

1. **The de Bruijn criterion ⇄ LCF duality** (holhol, notices, cacm) is the
   spine of both cert-log (emit + independently re-check proofs) and the Candle
   kernel (only the tiny kernel makes `thm`s). An LCF prover *is* a de Bruijn
   checker with an ephemeral proof; **instrumenting the kernel to persist that
   proof = cert-log.**
2. **Tiny trusted core** (≈400 lines), **10 primitive rules**, **3 axioms**
   (`ETA_AX`/`SELECT_AX`/`INFINITY_AX`), **definitional-only extension** — the
   exact spec the Candle adapter and the 3+1 gate's kernel layer must honor and
   audit.
3. **`mk_thm`/`new_axiom` and variable-capture bugs** are the named historical
   failure modes → the gate's highest-value automated checks: an
   **axiom-base/definitional-extension auditor** and **independent re-derivation
   of `INST`/`INST_TYPE`** in the reference checker.
4. **Search-anywhere / check-in-kernel** (hol05 SOS, cade05 QE, cacm LP+SDP,
   fmcad00 shadow functions): the universal contract for tools behind injected
   seams — the certificate is small and kernel-checked; the search is untrusted.
5. **Cross-backend replay as assurance** (holhol→Isabelle; neworleans→Coq;
   cacm multi-verifier): different kernels have different bug surfaces →
   cert-log's cross-backend replay is a genuine multiplier, not redundancy.
6. **Library + retrieval are load-bearing** (neworleans' 9724 theorems, the
   "trivialities must be named/stored" argument, cacm's "better search for
   background facts"): motivates the dense index and growing-lemma-library.

---

## Prioritized adopt-list (this batch)

**P0 — buildable now, behind existing seams, highest assurance-per-effort**
1. **Axiom-base / definitional-extension auditor** in the gate's kernel layer:
   flag any theorem depending on axioms outside `{ETA,SELECT,INFINITY}` +
   conservative definitions; reject forged `thm`s (`mk_thm`, stray
   `new_axiom`). (hollight, holhol, ESSLLI94)
2. **cert-log = instrumented-kernel proof log + independent reference
   checker**, prioritizing independent re-derivation of `INST`/`INST_TYPE`
   (capture-avoidance) — the historical bug class. (holhol, notices, cacm)
3. **Search/check tool contract** for every external helper behind a seam:
   accept an untrusted certificate, re-check only in the kernel; tag each
   result with its **trust tier** (kernel-chained > proof-of-algorithm >
   extracted-run). (cacm, hol05, cade05, fmcad00, iday)

**P1 — buildable, medium effort**
4. **Cross-backend replay** in cert-log once ≥2 backends (Candle + one of
   Lean/Rocq/Isabelle) are wired — exploit differing bug surfaces. (holhol,
   neworleans, cacm)
5. **de-Bruijn-factor + axiom-dependency report** per certificate as a standard
   gate artifact / driver metric. (cacm)
6. **Shadow-function idiom** for Candle tactics: keep a fast unverified explorer
   alongside the proof-producing version. (fmcad00, cade05)
7. **Retrieval over a named-theorem library** with a WLOG/invariance-database
   tactic as the first retrieval-driven automation. (neworleans)

**P2 — foundational / toolchain-gated (adopt as design constraints, don't
rebuild)**
8. Candle = HOL Light on verified CakeML: honor the ≈400-line trusted-core
   boundary; log which axioms/definitions are in scope. Do not reimplement the
   kernel or the self-verification. (hollight, holhol, cacm)
9. Large formalized libraries (Euclidean space, FP theory, real analysis) and
   heavy decision procedures (full real QE) are references/optional tools, not
   core deliverables. (hol05, neworleans, fmcad00, cade05)

**Follow-up chore:** re-extract `ESSLLI94.pdf` via an OCR fallback (render pages
→ Tesseract) to raise confidence on §10; poppler/pypdf both fail on its font
encoding.
