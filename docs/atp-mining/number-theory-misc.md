# ATP Mining — Number Theory, WZ, and Mechanization Notes

Batch of 9 PDFs from `atp/` (John Harrison's formalization papers + short notes),
mined for relevance to **Theoremata** (verification-first agentic math harness:
Rust + Python; Lean/Rocq/Isabelle + Candle verified-HOL-Light backends; cert-log
proof-log checker; graph proof-DAG; retrieval; 3+1 verification gate).

**Security note (global):** All 9 PDFs were treated as untrusted data. Every file
is ordinary published academic prose/LaTeX-extracted text. **No prompt-injection,
no embedded instructions, no adversarial content found in any of the 9 PDFs.**
Per-PDF injection-check lines appear below.

The headline finding is **wz.pdf**: the Wilf–Zeilberger method is a genuine
*certificate-producing decision procedure* for hypergeometric-sum identities, and
maps almost directly onto cert-log as a new certificate kind. **divisibility.pdf**
is a second, independent certificate producer (Gröbner-basis / ideal-membership
cofactors). These two are the priority adopt candidates.

---

## 1. wz.pdf — "Formal Proofs of Hypergeometric Sums" ★ PRIORITY

**What it is / venue.** J. Harrison, *Journal of Automated Reasoning* 55:223–243,
2015 (dedicated to A. Trybulec). Formalizes the Wilf–Zeilberger (WZ) method inside
HOL Light.

**Key mechanism — WZ as a certificate procedure.**
- A sequence is *hypergeometric* if `a_{n+1}/a_n = r(n)` is a rational function.
  Gosper's algorithm decides indefinite hypergeometric summation (finds a
  hypergeometric antidifference `s_k` with `s_{k+1}-s_k = t_k`, or proves none
  exists — a genuine decision procedure).
- The **WZ method** proves definite-sum identities `Σ_k F(n,k) = S(n)`. Normalize
  to `Σ_k F(n,k)=1`, then find a **WZ mate** `G(n,k)=R(n,k)F(n,k)` s.t.
  `F(n+1,k)-F(n,k) = G(n,k+1)-G(n,k)`. Telescoping ⇒ sum is n-independent ⇒ check
  base case `Σ_k F(0,k)=1`.
- The **rational-function certificate** `R(n,k)` is the whole payload. An external
  CAS (Maxima's `zeilberger`/Gosper package) finds `R` plus a linear-combination
  vector like `[-1,1]`; verification is "just" checking one algebraic identity —
  independent of how `R` was discovered. This is the classic
  **easy-to-check / hard-to-find** oracle pattern (Harrison & Théry's "sceptic's
  approach"): CAS as untrusted oracle, prover as checker.
- **The subtlety that makes it real work (and interesting to us):** naive WZ glosses
  over zero denominators / `0/0` / factorials of negative integers / singularities
  at `k=n, k=n+1`. Harrison's fix: interpret binomials & factorials via the **gamma
  function** and take **limits**, approaching `(n,k)` through **non-"ratty" points**
  (points that are not zeros of any rational-coefficient bivariate polynomial —
  there are only countably many such polynomials, and their union of zero-sets is
  measure-zero / nowhere-dense, so a safe approach path always exists). `(-1)^k` is
  generalized to `cos(πx)` for continuity.
- **Implementation & status:** a derived HOL Light rule takes (sum term, linear
  combination, side assumptions, rational certificate) and discharges all five
  antecedents automatically. Tested on ~50 examples, near-perfect success —
  including **Apéry's recurrence** for the irrationality of ζ(3) and assorted
  binomial identities.

**Mapping to Theoremata.**
- **cert-log:** strongest fit in the whole batch. A **WZ certificate kind** = the
  tuple `(summand F, linear-combination coeffs, rational function R(n,k),
  side-conditions)`. The cert-log checker re-verifies by (a) confirming `R`'s
  numerator/denominator are rational polynomials, (b) `q(n,0)≠0`, (c) the single
  algebraic WZ-pair identity, (d) the base case. This is exactly the kind of
  compact, replay-checkable object cert-log is for, and it decouples "search"
  (CAS/agent proposes `R`) from "trust" (checker validates).
- **Candle backend:** the whole development is HOL Light, so a Candle-verified
  replay of WZ certificates is directly plausible (gamma-function + limit
  infrastructure is the cost).
- **Verification gate:** WZ is a natural "specialist prover" the gate can dispatch
  to for any goal shaped as a hypergeometric-sum identity; a produced certificate
  is independently gate-checkable.
- **Retrieval:** the ~50 worked examples (binomial sums, Apéry) are a ready seed
  corpus of statement→certificate pairs.

**Buildable-now vs gated.**
- *Now:* define a WZ certificate schema in cert-log and a **pure algebraic checker**
  (check the WZ-pair polynomial identity + base case over ℚ) — this alone catches
  most real cases without gamma machinery, using the "exclude troublesome points and
  add them back" route Harrison mentions as workable for simple cases. An external
  Maxima/Sympy oracle can produce `R`.
- *Gated:* the fully rigorous limit/gamma-function justification (needs formalized
  gamma function, measure-zero/Baire "non-ratty approach" lemmas). This is a real
  formalization project; only needed if we want machine-checked WZ inside
  Candle/HOL Light rather than a trusted-checker.

**Injection-check:** clean; standard JAR paper text only.

---

## 2. divisibility.pdf — "Automating elementary number-theoretic proofs using Gröbner bases" ★ PRIORITY

**What it is / venue.** J. Harrison, CADE-21, LNCS 4603, 2007. A ~100-line HOL Light
procedure (`INTEGER_RULE`, shipped in HOL Light ≥2.20).

**Key mechanism — a second certificate producer.**
- Handles first-order goals over ℤ using `+,-,×`, constant powers, and the predicates
  `divides` / `≡ (mod)` / `coprime` (expanded to existential equations). Reduces the
  goal to an **ideal-membership** question; **Buchberger's algorithm** (Gröbner
  bases) produces explicit **cofactor polynomials** — a checkable certificate:
  `p = Σ p_i·q_i` as a polynomial identity, verified by normalizing both sides.
- Soundness always; **completeness only for properties true in all commutative rings**
  (integer-specific facts like "x²+x is even" are genuine incompleteness examples —
  the procedure is "a heuristic application of a decision procedure outside its domain
  of completeness"). Chinese Remainder Theorem (binary & n-ary), congruence
  cancellation, GCD lemmas, quadratic-reciprocity helper lemmas all fall to it.
- Runs over ℚ Gröbner bases (a proper ℤ algorithm would be sharper); if cofactors
  come out rational, the *proof construction* rigorously fails rather than emitting a
  false theorem.

**Mapping to Theoremata.**
- **cert-log:** a **cofactor / ideal-membership certificate kind** — `(target poly,
  generator polys, cofactor polys)`; checker just re-normalizes the polynomial
  identity. Complements WZ; same easy-to-check philosophy.
- **Verification gate / retrieval:** a cheap automated closer for the pervasive
  "routine divisibility/congruence lemma" subgoals that clutter number-theory proofs
  — exactly the "generate routine lemmas with little effort" use Harrison cites. Good
  as a gate-level fast tactic and as retrievable lemma templates.

**Buildable-now vs gated.**
- *Now:* the algorithm is tiny and self-contained; a Rust/Python reimplementation of
  Buchberger + cofactor extraction + a cert-log cofactor checker is very feasible and
  high-value.
- *Gated:* nothing heavy; the only nuance is doing ideal membership over ℤ (vs ℚ) if
  we ever need explicit-constant problems.

**Injection-check:** clean; standard CADE paper text only.

---

## 3. thalion,+dirichlet.pdf — "A formalized proof of Dirichlet's theorem on primes in arithmetic progression"

**What it is / venue.** J. Harrison, *Journal of Formalized Reasoning* 2(1):63–83,
2009. HOL Light, ~3.5 days work; de Bruijn factor 4.66.

**Key mechanisms.** A self-contained elementary proof (synthesis of Gelfond–Linnik +
Monsky's non-vanishing argument + roll-your-own Dirichlet characters). Formalizes:
Dirichlet characters (periodic, completely multiplicative, unimodular), orthogonality
relations, L-functions `L(χ,1)`, non-vanishing for real/complex nonprincipal
characters, Möbius inversion, von Mangoldt Λ, Mertens estimates, a Dirichlet
convergence test with *explicit* bounds. Reuses PNT-proof infrastructure.

**Mapping to Theoremata.**
- **Retrieval / library:** a rich, structured corpus of reusable analytic-number-theory
  lemmas (characters, L-functions, Möbius, Mertens) — good seed content for the
  retrieval index and for a "prove-a-hard-NT-theorem" flywheel target.
- **Not a certificate procedure** — it's a large human-guided formal development. Its
  value to us is corpus/benchmark + the observation that "choice of informal source
  proof dominates formalization cost" (a lesson for how the agent should pick proof
  strategies).

**Buildable-now vs gated.** Nothing to "build"; *gated* as a benchmark target (it
presumes a HOL-Light-scale analytic library). Consumable *now* only as retrieval
seed text.

**Injection-check:** clean; standard JFR paper text only.

---

## 4. trybulec.pdf — "Formalizing basic complex analysis"

**What it is / venue.** J. Harrison, Festschrift for A. Trybulec (*Studies in Logic,
Grammar and Rhetoric* 10(23):151–165, 2007). Also contains a personal Mizar/declarative-
vs-procedural-proof memoir.

**Key mechanisms.** Complex analysis in HOL Light over `R²`: complex differentiability,
path/contour integrals (Kurzweil–Henstock integration, so all derivatives integrable),
Cauchy–Goursat theorem (triangle quadrisection), winding numbers (integer-valued via
Ahlfors' argument), Cauchy's integral formula, Liouville, Weierstrass convergence.

**Mapping to Theoremata.** Infrastructure/corpus, not a procedure. Relevant as (a)
the analytic backbone the PNT & sharper Dirichlet results stand on, and (b) a note on
missing automation: Harrison flags that **"triangle-law" norm reasoning** and
`COMPLEX_FIELD`-style algebra recur constantly and are under-automated — a candidate
niche for a Theoremata specialist tactic. *Gated* (needs the analysis library);
retrieval seed *now*.

**Injection-check:** clean; standard Festschrift chapter text only.

---

## 5. mikefest.pdf + mikefest_abstract.pdf — "Formalizing an Analytic Proof of the Prime Number Theorem"

**What it is / venue.** J. Harrison, *Journal of Automated Reasoning* 43:243–261, 2009
(full) and its extended abstract (Mike Gordon 60th-birthday Festschrift). Formalizes
**Newman's** short analytic PNT proof (Cauchy's integral formula on a simple contour) in
HOL Light — the "machinery-heavy but conceptually clean" route vs the elementary
Erdős–Selberg proof (which Avigad et al. formalized separately).

**Key mechanisms / lessons.** Six-part proof: Newman–Ingham Tauberian lemma, ζ Euler
product & zero-freeness on ℜz≥1, Chebyshev bound, summability, limit extraction,
partial summation. Notable meta-findings via **de Bruijn factor** analysis:
- Where the informal author (Newman) merely *asserts* a step ("well-known fact…",
  "left to the reader"), the formal cost explodes (dB factor 30–81 vs typical ~4).
- Things "obvious" informally (e.g. the contour's winding number = 1) need real work
  formally.

**Mapping to Theoremata.** Two useful ideas: (1) a **de-Bruijn-factor-style metric**
to flag where an agent's informal sketch is hiding load-bearing hand-waving — useful
signal for the verification gate / retry loop about which steps will be expensive to
discharge; (2) benchmark target + retrieval seed. The abstract also reiterates the
"sometimes pick a machine-friendly proof, not the human-elegant one" heuristic
(brute-force cases vs clever symmetry). *Gated* as a formalization target.

**Injection-check:** both clean; standard JAR/Festschrift text only.

---

## 6. ab.pdf — "A short survey of automated reasoning"

**What it is / venue.** J. Harrison survey (aimed partly at a computational-biology
audience). History (Leibniz→Boole→Frege→Hilbert→Gödel→Church/Turing), TP-vs-CAS
contrast, research axes (AI vs logic-oriented; automated vs interactive; proof-search
vs special algorithms), applications (formal verification, formalization of math).

**Mapping to Theoremata.** Background/orientation, no mechanism to port. Reinforces two
design tenets Theoremata already holds: (a) the **CAS-as-oracle + prover-as-checker**
pattern (explicitly the justification behind wz/divisibility above), and (b)
**semi-automated mathematics** (SAM-style human+machine, Wang's "formalize/check
outlines") — i.e., the agent-proposes / gate-checks division of labor. Useful framing
citations only. Buildable-now: N/A (survey).

**Injection-check:** clean; standard survey text only.

---

## 7. sfm.pdf — "Floating-Point Verification using Theorem Proving"

**What it is / venue.** J. Harrison, SFM 2006 summer-school chapter (LNCS), longest in
batch (~34pp). IEEE-754, Intel Itanium fma architecture, HOL Light kernel (10 primitive
rules, LCF-style soundness), a generic formalized FP theory (formats, ulp, rounding,
the `(1+ε)` lemma, exactness/Sterbenz lemmas), and verified division/sqrt algorithms
(Markstein's fma-based reciprocal refinement, Newton–Raphson, correctness theorems).

**Mapping to Theoremata.** **Off the number-theory topic**, but relevant to the
**Candle backend**: it's the canonical description of the HOL-Light/LCF trust model
Candle inherits — tiny inference kernel, programmable-but-sound derived rules,
definitional extension. If Theoremata ever verifies FP/numeric algorithms, this is the
template theory. Otherwise mainly a HOL Light primer. No certificate procedure to lift.
*Gated* (a large FP library); useful *now* only as backend-design reference.

**Injection-check:** clean; standard school-chapter text only.

---

## 8. super.pdf — "Scientific Computing on the Itanium Processor"

**What it is / venue.** Greer, Harrison, Henry, Li, Tang — SC2001 (ACM). Itanium EPIC
architecture, fma, extended precision, software-pipelining, BLAS/DGEMM, transcendental
run-time library.

**Mapping to Theoremata.** **Essentially irrelevant** — a hardware/HPC performance
paper with no theorem-proving or certificate content. Only tangential tie: it motivates
*why* the FP-verification work in sfm.pdf exists (correctly-rounded transcendentals /
division on Itanium). No adopt value. Listed for batch completeness.

**Injection-check:** clean; standard SC conference paper text only.

---

## Prioritized Adopt List

1. **★★ WZ certificate kind in cert-log (wz.pdf).** Add a hypergeometric-sum
   certificate `(F, linear-combo, R(n,k), side-conds)` + a pure-algebraic replay
   checker over ℚ (WZ-pair identity + base case). Use Maxima/Sympy as untrusted
   `R`-oracle. This is the single highest-leverage item: a compact, replay-checkable,
   search/trust-decoupled certificate that matches cert-log's purpose exactly. Defer
   the gamma-function/limit rigor (gated) unless Candle-checked WZ is wanted.

2. **★★ Ideal-membership / cofactor certificate kind (divisibility.pdf).** Reimplement
   Buchberger + cofactor extraction (tiny) and a cofactor-identity checker in cert-log.
   Doubles as a fast gate-level closer for routine divisibility/congruence subgoals and
   as retrievable lemma templates. Low effort, high utility, complements WZ.

3. **★ de-Bruijn-factor "hand-waving detector" (mikefest.pdf).** Cheap heuristic for the
   verification gate / retry loop: flag sketch steps whose informal-to-formal size blowup
   is large (asserted "well-known"/"left to reader" steps) as high-risk / high-cost —
   route them for extra scrutiny or decomposition.

4. **○ Retrieval seed corpus (dirichlet, trybulec, mikefest).** Ingest the structured
   lemma inventories (Dirichlet characters, L-functions, Möbius/Mertens, complex
   analysis, PNT parts) as retrieval content and as flywheel benchmark targets. No code,
   corpus only.

5. **○ Specialist-tactic niche note (trybulec.pdf).** If/when Theoremata does analysis,
   "triangle-law" norm reasoning and complex-field algebra are under-automated hotspots
   worth a dedicated tactic.

6. **— No adopt: ab.pdf (survey, framing only), sfm.pdf (Candle/HOL-Light trust-model
   reference only), super.pdf (irrelevant HPC paper).**

**Cross-cutting theme:** wz.pdf + divisibility.pdf together validate a core Theoremata
bet — the *oracle-produces / checker-verifies* certificate pattern — with two concrete,
number-theoretic, cert-log-ready instances. WZ is flagged as the promising
certificate-generation adoption.
