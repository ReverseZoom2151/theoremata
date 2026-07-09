# ATP Mining — Flyspeck / Kepler Conjecture

Batch: `atp/kepler.pdf`, `atp/revkepler.pdf`, `atp/mark10.pdf`, `atp/joerg.pdf`.
Read fully (kepler 21pp, revkepler 37pp, mark10 55pp, joerg 85pp). All four are legitimate
peer-reviewed / handbook academic PDFs (Hales et al.; Harrison; Harrison–Urban–Wiedijk).

**Injection check:** No prompt-injection or embedded instructions to the reader/agent were
found in any of the four PDFs. All content is ordinary mathematical/technical prose,
formulas, HOL Light/Isabelle code, and bibliography. Treated as untrusted data throughout;
nothing acted upon. **NO INJECTION DETECTED.**

Why this batch matters to Theoremata: Flyspeck is the single most relevant prior art we have
for (a) **blueprint-scale, distributed formal-proof management**, (b) **LP/Farkas dual
certificates for nonlinear bounds** — the exact mechanism our cert-log LP checker implements,
and (c) the **blueprint / formal-abstract split** that our graph proof-DAG orchestration is
modeled on. Two of the four PDFs (kepler, revkepler) are Flyspeck-specific; two (mark10,
joerg) are Harrison surveys that give the general certificate/kernel/verification theory the
Flyspeck LP and nonlinear machinery instantiate.

---

## 1. `kepler.pdf` — "A Formal Proof of the Kepler Conjecture"

**What it is + venue/year.** Hales, Adams, Bauer, Harrison, Kaliszyk, Magron, McLaughlin,
Nipkow, Obua, Solovyev, Urban, Zumkeller et al. arXiv:1501.02155v1 [math.MG], 9 Jan 2015.
The **official published account of the completed Flyspeck project** — the formal proof of
the Kepler conjecture (densest sphere packing = FCC, density π/√18 ≈ 0.74) in a combination
of **HOL Light and Isabelle/HOL**, with a second verification of the main statement in HOL
Zero. Comparable in scale to Feit–Thompson, CompCert, seL4; likely the record for LOC in a
verification project.

**Key mechanisms.**

- **Four-part decomposition of a huge proof.** The Kepler conjecture is split into (i) the
  *text part* (traditional non-computer math), (ii) ~1000 **nonlinear inequalities**
  (`the_nonlinear_inequalities`), (iii) **tame plane-graph classification**
  (`import_tame_classification`, done in Isabelle), (iv) a large collection of **linear
  programs** (`linear_programming_results`). The whole is never proved in one HOL session;
  what is proved in one session is the implication
  `the_nonlinear_inequalities ∧ import_tame_classification ⇒ the_kepler_conjecture`,
  leaving the two computational bodies as named assumptions discharged in separate sessions.
  This is exactly a **proof-DAG with named sub-obligations** discharged independently — our
  graph model at scale.

- **Blueprint / formal-abstract split.** The proof was reorganized into a "blueprint"
  (Hales, *Dense Sphere Packings*, 2012) written *specifically to be formalized*, developed
  in tandem with the formalization. Design principles they call out (§4.3) map 1:1 onto our
  build plan: replace topology with purely combinatorial hypermaps; organize around a
  *small number of major concepts* (spherical trig, volume, hypermap, fan, polyhedra,
  Voronoi, LP, nonlinear inequalities) because "every new concept comes at a cost: libraries
  of lemmas must be developed"; make all hypotheses explicit; **make chapters as independent
  as possible and break long proofs into short lemmas to permit large collaboration**;
  "fully embrace" computer calculation rather than treating it as last resort. Hundreds of
  small errors in the original proof were found and fixed during formalization.

- **LP dual certificates (directly relevant to our LP/Farkas cert-log).** §9. Each tame
  hypermap → a nonlinear system; **linearize by introducing fresh variables for each
  nonlinear quantity** (e.g. dihedral angles), then show the linear relaxation is
  *infeasible* (infeasibility of the relaxation ⇒ inconsistency of the nonlinear original ⇒
  no counterexample). Irrational coefficients are **soundly rationalized** (multiply through
  by a large power of 10; relax √2 → 1.42 etc. in the safe direction). Infeasibility is
  certified by a **modified dual solution**: add slack variables, minimize their sum; a
  positive optimum proves infeasibility. The external solver **GLPK** produces an imprecise
  dual; a C# program *repairs* it into a dual whose weighted sum of the constraints yields
  `0·x + 0·y ≤ −0.297` — a contradiction with **the coefficients of the variables exactly
  zero** ("a key feature of the modified dual solutions"). Formal verification then reduces
  to **summing inequalities weighted by the (integer-scaled) dual coefficients and checking
  the result is absurd** — cheap, no search. 43,078 LPs (after case splitting) verified in
  ~15h. This is precisely the Farkas/LP-duality certificate our cert-log checker should
  emit and replay.

- **Case splitting for LP precision.** For ~half the hypermaps the linear relaxation is too
  weak; a feasible LP is split on `x ≤ a ∨ a ≤ x` and re-relaxed. An informal procedure
  generates the case tree; the formal checker is applied per case. (Directly analogous to
  our branch-and-bound / abstention-on-imprecision path.)

- **Nonlinear inequality verification.** §5. General form `∀x∈D. f₁(x)<0 ∨ … ∨ f_k(x)<0`
  over rectangular domains D⊂Rⁿ, n≤6. Verified by **interval arithmetic + second-order
  Taylor interval approximations** (natural interval extensions are too imprecise / need too
  many subdivisions). Monotonicity via fixed-sign derivative intervals reduces a k-cube to
  a (k−1)-face. Sharp inequalities (f(x₀)=0 at a corner) handled by exact arithmetic at x₀
  plus derivative-sign argument on a neighborhood. **All arithmetic done inside HOL Light
  with formalized finite-precision floats — native FP never trusted**; ~300 inequalities
  partitioned into 23,000, verified in ~5000 CPU-hours on Azure (32-core GNU parallel, <1
  week), independently re-checked at Radboud (~9370 CPU-h, identical results).

- **Combining HOL sessions by MD5 hashing (distributed proof management).** §6. HOL Light
  cannot pass a theorem between sessions without re-proving. They built a *modified HOL
  Light* that canonicalizes a theorem **plus its entire history of constants, types, and
  axioms back to the kernel**, serializes it to a string, and stores the **MD5 hash**; a
  theorem may be imported iff its hash matches. OCaml scoping prevents misuse. This is a
  **content-addressed proof-fragment cache / trust ledger** — how you reassemble 23,000
  parallel obligations into one master theorem. Highly relevant to our memory facade /
  flywheel and to distributing a run across workers.

- **Tame graph classification** (§7, Isabelle). Plane graphs defined *by an executable
  enumeration algorithm* (`next_plane`), tame ones by `next_tame`; completeness proved by
  interactive proof + **evaluation of closed HOL formulas as ML** (computational
  reflection). Approximate pruning is *sound-by-design*: it may emit non-tame graphs (a fake
  counterexample killed downstream) but never drops a real one. Archive grew 2771 → 18762
  graphs; a symmetry bug in the original Java was found and fixed.

- **Cross-system import + auditing.** §8, §10. The Isabelle tame-classification theorem is
  hand-translated verbatim into a HOL Light assumption; justified because it is a bounded
  SAT-expressible statement that transfers between consistent systems. §10 ("Auditing a
  distributed formal proof", after Adams' "Flyspecking Flyspeck") stresses an auditor must
  **assume malicious intent, not innocent error**: re-run scripts, check the *right theorem*
  was stated, check no rogue axioms, check the hand translation and the session-combining
  hack. They name the hand translation as their single greatest vulnerability. Also §4.5:
  proofs **recorded and replayed** — a recorded-proof format runs the main statement in ~40
  min vs ~5 h, and imports/replays into HOL Zero for an independent trust check.

---

## 2. `revkepler.pdf` — "A Revision of the Proof of the Kepler Conjecture"

**What it is + venue/year.** Hales, Harrison, McLaughlin, Nipkow, Obua, Zumkeller.
*Discrete & Computational Geometry* 44(1):1–34, 2010 (the mid-project status report,
predating completion). Part 1 = formal-proof initiatives; Part 2 = errata in the original
proof.

**Key mechanisms.**

- **Blueprint edition rationale.** ~200 elementary-geometry lemmas extracted into a
  standalone collection expressible in the **first-order language of the reals** (decidable
  by quantifier elimination in principle). Confirms the "extract reusable, statement-only,
  Kepler-independent lemma library" pattern — our growing lemma library / LEGO-Prover
  adopt item, seen here in a flagship project.

- **Multi-prover division of labor + import.** HOL Light (text/Euclidean geometry), Coq
  (nonlinear inequalities, Zumkeller), Isabelle/HOL (tame graphs + LP, Bauer/Nipkow/Obua).
  Raises the cross-prover translation problem explicitly (HOL↔Isabelle↔Coq importers cited).

- **LP certificates via "graph systems" (cert-log gold).** §7 + Figure 1. Formal pipeline:
  graph-system axioms applied to each tame graph → one big Isabelle conjunction of
  (in)equalities → normalize to matrix form **`A x ≤ b`** → replace symbolic entries
  (containing π etc.) by **interval bounds** with formally proven `A′ ≤ A ≤ A″`, `b ≤ b′`
  → derive a-priori variable bounds `x′ ≤ x ≤ x″` → obtain a **certificate from an external
  (untrusted, C-optimized) GLPK solver** that lets them formally derive `False` from
  `A x ≤ b`. Their statement of the design principle is quotable and is our cert-log thesis
  verbatim: *"The beauty of a certificate is that we can use results obtained from an
  untrusted source … in a trusted and completely mechanically verifiable way."* Result:
  2565/2771 graph systems refuted (92.5%); the remaining 206 need branch-and-bound
  constraints not yet in the formal `graph system` notion (basic vs full LPs).

- **Nonlinear inequalities via Bernstein bases (alternative to interval/Taylor).** §5.
  Replace transcendental functions (√, 1/·, arctan) with **polynomial upper/lower
  approximations** in the correct direction so `γ(y) ≤ g(y) ≤ pt` by transitivity; choose
  approximations so the transcendental parts factor out and the residual polynomial has
  **rational coefficients**; convert to **Bernstein representation** whose *largest
  coefficient bounds the polynomial* on [0,1]. Example polynomial: 12945 monomials, degree
  18, max = 0 computed in ~10 min. A distinct, formal-proof-friendly certificate style for
  polynomial nonnegativity worth keeping alongside SOS.

- **SOS / semidefinite certificates for universal nonlinear reals.** §3.3. Reduce a
  universally-quantified nonlinear real goal to **semidefinite programming**, solve with an
  external SDP tool, reconstruct a **sum-of-squares certificate** checkable by simple
  algebra (e.g. `b² − 4ac = (2ax+b)² − 4a(ax²+bx+c)`). Solves 9-variable coordinate forms
  of simple Flyspeck inequalities in ~1s. Another external-solver-plus-checkable-certificate
  pattern for our cert-log.

- **Code reimplementation for auditability.** §4. Original code: 50k+ lines (Java/C++/
  Mathematica) + Ferguson's 137k lines of C, with explicit IEEE-754 rounding-mode changes
  ~400 times — hard to trust. Reimplemented in **Standard ML (MLton)**, interval-arithmetic
  *abstracted behind a module type* so a fast float impl and a slow trustworthy MPFI impl
  are interchangeable, everything runnable from one CLI. Lesson for us: **abstract the
  numeric backend behind an interface; keep a slow-but-trusted oracle for cross-checks.**

- **Errata management.** Part 2 documents specific gaps (notably the biconnected-graph /
  simple-polygon argument in the main estimate) found *because* of the blueprint effort —
  evidence that the formalize-and-blueprint loop is itself a bug-finding process.

---

## 3. `mark10.pdf` — Harrison, "Formal Verification" (lecture notes)

**What it is + venue/year.** John Harrison (Intel), broad graduate lecture notes on formal
verification (companion to his 2009 *Handbook of Practical Logic and Automated Reasoning*;
mid/late 2000s). **Not Flyspeck-specific** — it is the general theory that the Flyspeck LP /
nonlinear / kernel machinery instantiates. Sections: propositional logic & SAT/DPLL,
symbolic simulation & BDDs/STE, model checking, automated theorem proving, arithmetical
theories, interactive theorem proving / LCF, HOL Light, and a floating-point square-root
case study.

**Key mechanisms relevant to us.**

- **The LCF kernel discipline** (§6–7). Theorems are an abstract type `thm` whose only
  constructors are the primitive inference rules, so *anything* of type `thm` is provably
  derived however complex the surrounding program. HOL Light: **~600-line OCaml kernel**,
  two primitive types, one primitive constant (`=`), everything else defined not postulated.
  This is the model for our verification gate's trusted core: keep the trusted checker tiny,
  let untrusted search build on top.

- **Arithmetic decision procedures & certificates.** Presburger (linear integer arithmetic,
  QE), complex-field QE, **real-closed-field decision via CAD / Hörmander sign-matrix
  method**, congruence closure, Nelson–Oppen / Shostak combination, and **SMT = SAT core +
  theory solvers**. Notes that in practice most program-verification arithmetic queries are
  purely universal and low-degree, so a cheap specialized solver beats full QE — an
  altitude lesson for our gate (don't reach for the heavy hammer by default).

- **Floating-point / interval verification case study** (§8, Intel Itanium √). The
  `1 + e` rounding model, automatic absolute-magnitude bounding via the triangle
  inequality, over/underflow exclusion, and a notable **"exclusion-zone + explicit
  special-case enumeration" pattern**: prove correctness generally *except* for finitely
  many hard inputs (isolated by solving Diophantine equations `2^{p+1}m = k² + d` via Hensel
  lifting), then check those explicitly by proof. This "general theorem + enumerate the
  finite exceptional set" structure is the same shape as Flyspeck's tame-graph enumeration
  and is a clean model for **abstention + targeted case discharge** in our harness.

- **Certificate philosophy foreshadowed:** foundational systems can be slow because
  everything decomposes to primitives (e.g. a multiprecision multiplication becomes a full
  proof); the escape is to accept external results *with a checkable certificate* rather
  than re-derive — the through-line to joerg §6.3 and the Flyspeck LP work.

---

## 4. `joerg.pdf` — Harrison, Urban & Wiedijk, "History of Interactive Theorem Proving"

**What it is + venue/year.** Harrison, Urban, Wiedijk (reader: L. Paulson). Handbook-style
survey chapter (~2014, covers Flyspeck's just-announced completion). Traces Automath →
LCF/HOL → Coq/Isabelle/Agda/Mizar/PVS, then a thematic §5 (powerful automation) and §6
(research topics). **Not Flyspeck-specific** but contains the definitive framing of
certificates, kernels, distributed proof management, and premise selection that our
architecture leans on.

**Key mechanisms relevant to us.**

- **Certificates vs. reflection (§6.3) — the cert-log design bible.** Two ways to trust an
  untrusted procedure: (a) *reflection* = verify the code and run it inside the logic
  (Coq-style); (b) **produce a certificate that is cheap to check by proof** (Blum's
  "checking results beats verifying code"). Worked examples, all our cert-log families:
  - **Non-primality** ← factorization; **primality** ← Pratt/Pocklington/ECPP certificates.
  - **Linear arithmetic infeasibility ← a linear combination summing to `1 < 0` — "the
    content of Farkas's lemma."** Explicitly credits Boulton (HOL), Necula (proof-carrying
    code), and *"its apotheosis in the work of Alexey **Solovyev** [Solovyev and Hales,
    2011], who checked the very large linear programs in the **Flyspeck** project inside HOL
    Light using such certification, remarkably efficiently."* — i.e. our LP/Farkas cert-log
    is a direct descendant of this exact line.
  - **Nullstellensatz** certificates for integral-domain/field universal theories;
    **sum-of-squares / SDP** certificates for real-closed fields.
  - **SAT/FOL/SMT** proofs replayed fully-expansively; note QBF is a case where checking can
    cost *more* than finding — a real caveat for our "always replay" policy.

- **Distributed / collaborative proof management (§6.5).** Contrasts one-person projects
  (HOL Light) with **loosely-managed distributed ones (Mizar MML)** and **strong-leader
  distributed ones**: Flyspeck (Hales sets the blueprint, a Hanoi team executes) and
  Feit–Thompson (Gonthier controls plan, proof style, naming conventions, automation). The
  explicit lesson: at scale, **library re-use and search become the bottleneck**, driving
  `find_theorems`/`SearchAbout`, MML Query, discrimination-tree subsumption over *millions*
  of lemmas, and machine-learning premise selection. Our memory facade + dense index + graph
  DAG are the same response.

- **Premise selection & the ATP/ITP flywheel (§6.3).** **Sledgehammer** (Isabelle) and
  hammer frameworks for HOL Light/Mizar use **machine-learned premise selection** over huge
  lemma libraries and network servers; Urban et al. describe **AI feedback loops where the
  learner and the prover mutually improve on the growing library** — literally our flywheel
  revolution. Kaliszyk & Urban's learning-assisted reasoning was built *on the Flyspeck
  corpus*.

- **de Bruijn criterion & ultimate reliability (§6.6).** Prefer a system that emits a proof
  checkable by a *much simpler* kernel. Kernel-size comparison is stark: **HOL Light ~600
  lines** vs **Coq ~20,000 lines (2,500 in C)**. Independent re-checking (export to HOL
  Zero / OpenTheory) as a second trust layer. Warns about the *other* failure modes even
  with a correct kernel: wrong/aggressively-totalized definitions, and users misreading
  pretty-printed statements ("did the right theorem get formalized?"). Feeds our gate design
  and our "state the obligation faithfully" checks.

- **Sharing/import across systems (§6.4)** and **wiki/HTML presentation (§6.5)** — Hales
  cross-linked the informal Flyspeck book with the formal HOL Light development via a wiki,
  the model for tying our informal blueprint text to formal obligations.

---

## Prioritized adopt-list for Theoremata

Ranked; LP-certificate and blueprint-management items called out as requested.

1. **[LP CERT — highest] Emit and replay LP/Farkas dual certificates exactly as Flyspeck
   does.** Our cert-log LP checker should: linearize by fresh variables for nonlinear terms;
   soundly rationalize irrational coefficients (relax in the safe direction, then scale to
   integers by a power of 10); get a dual from an untrusted solver (GLPK/HiGHS); **repair it
   into a "modified dual" whose weighted constraint-sum has exactly-zero variable
   coefficients and a contradictory constant**; verify by integer-weighted summation only
   (no search). Store certificates once, reload for replay. (kepler §9; revkepler §7; joerg
   §6.3 — Solovyev–Hales.) *This is the single most load-bearing adoption.*

2. **[BLUEPRINT MGMT — highest] Prove the top-level obligation as an implication with named
   assumptions, discharge each in an isolated worker, reassemble via a content-addressed
   ledger.** Mirror `nonlinear ∧ tame ⇒ goal` + MD5-hash-of-full-history theorem import.
   Our proof-DAG nodes should carry their full dependency/axiom fingerprint; reassembly
   checks fingerprints, not re-proofs. (kepler §3, §6.) Enables sharding a blueprint-scale
   run across the flywheel without a monolithic session.

3. **[BLUEPRINT MGMT] Adopt the blueprint / formal-abstract split as a hard design rule:**
   few major reusable concepts (each earns its lemma library), all hypotheses explicit,
   chapters/lemmas maximally independent and short *specifically to parallelize across
   workers*, computation embraced not deferred. (kepler §4.3; revkepler §2.) This is our
   graph-first orchestration validated at record scale.

4. **[LP CERT] Support certificate case-splitting when a relaxation is too weak** (`x ≤ a ∨
   a ≤ x`, re-relax per case) and **abstain/branch** when even that fails, rather than
   silently failing — Flyspeck could only close ~92.5% with basic LPs; honest partial
   coverage + branch-and-bound is the realistic target. (kepler §9; revkepler §7.)

5. **[CERT-LOG breadth] Add the sibling certificate families to the same untrusted-solver +
   cheap-checker frame:** SOS/SDP for real-closed-field nonnegativity, Nullstellensatz for
   field/integral-domain universals, Bernstein-coefficient bounds for polynomial
   nonnegativity, Pratt/Pocklington for primality. (revkepler §3.3, §5; joerg §6.3;
   mark10 §5.) Bernstein and SOS are two independent, formal-friendly nonlinear-bound
   certificate styles worth having beside interval/Taylor.

6. **[Nonlinear gate] Interval arithmetic + second-order Taylor interval approximations with
   monotonicity dimension-reduction, all in formalized finite-precision arithmetic — never
   trust native floats; keep a slow trusted numeric oracle (MPFI-style) behind an abstract
   numeric interface for cross-checks.** (kepler §5; revkepler §4; mark10 §8.)

7. **[Verification gate] Keep the trusted checker tiny (LCF/de Bruijn discipline) and add a
   second independent replay path** (recorded-proof format; export to a minimal checker à la
   HOL Zero/OpenTheory) for a cheap fast independent confirmation. Our 3+1 gate's trusted
   core should be small; the "+1" can be an independent replay. (mark10 §6–7; joerg §6.6;
   kepler §4.5, §10.)

8. **[Flywheel / retrieval] Machine-learned premise selection over the growing lemma library
   with an ATP/ITP feedback loop (Sledgehammer/hammer model), plus subsumption/discrimination
   -tree search for scale.** Validated directly on the Flyspeck corpus. (joerg §6.3, §6.5.)
   Aligns with our dense-index + flywheel-revolution items.

9. **[Auditing / adversarial trust] Build an audit mode that assumes malicious intent:**
   re-execute scripts, verify the *stated obligation matches intent* (right theorem, no rogue
   axioms/definitions, no aggressive totalization), and specifically guard hand/automated
   translations between backends — Flyspeck named cross-system hand-translation as its top
   vulnerability. (kepler §10; joerg §6.6.)

10. **[Enumeration / abstention pattern] The "general proof + enumerate a finite exceptional
    set, discharge each explicitly" structure** (tame-graph enumeration; FP square-root
    exclusion-zone + Diophantine special cases). Sound-by-design over-approximation is fine
    if downstream kills the spurious cases — a clean template for our abstention + targeted
    case-discharge. (kepler §7; mark10 §8.)

11. **[Provenance / presentation] Cross-link the informal blueprint text with formal
    obligations (wiki/HTML, stable cross-revision identifiers).** (kepler §4; joerg §6.5.)
