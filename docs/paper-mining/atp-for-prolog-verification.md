# Automated Theorem Proving for Prolog Verification

**Source:** Fred Mesnard, Thierry Marianne, Étienne Payet (LIM, Université de La Réunion). ICLP 2025, EPTCS 439, 2026, pp. 469–481. Updated version of an LPAR-2024 Complementary-Volume paper.

## Core contribution
Takes **LPTP** (Logic Program Theorem Prover — Stärk's mid-1990s interactive natural-deduction prover for pure Prolog with negation-as-failure and occurs-check unification) and, instead of proving properties interactively, **compiles Stärk's first-order axiomatization IND(P) of a Prolog program plus a target property into TPTP FOF files**, then dispatches them to off-the-shelf saturation provers **E and Vampire**. First experiment automating Prolog termination + partial-correctness verification this way; ~83% success on ~400 library properties with a 60s timeout.

## Key techniques / architecture
- **Three-viewpoint semantics.** For each user predicate R, LPTP's specification language L̂ replaces R with three predicates Rs (success), Rf (failure), Rt (termination), plus a groundness constraint gr. Syntactic operators **S, F, T** map Prolog goals to L̂ formulas (e.g. `S(G,H)=SG∧SH`; `F(G;H)=FG∧FH`; `T(\+G)=TG∧gr(G)` — termination of negation requires groundness, the "safeness" condition).
- **IND(P) = nine axiom schemas**, all compiled to FOF: (1–3) **Clark's equality theory** (function injectivity, clash, occurs-check/no-infinite-trees); (4–5) groundness axioms for gr; (6) uniqueness `¬(Rs ∧ Rf)`; (7) totality `Rt ⇒ (Rs ∨ Rf)`; (8) **fixed-point/completion** axioms `Rs(x)↔S⟦DP_R⟧`, `Rf↔F⟦DP_R⟧`, `Rt↔T⟦DP_R⟧` using Clark's iff-completion; (9) an **induction schema** along the inductive definition of predicates.
- **Static induction-axiom generation.** LPTP generates induction axioms interactively/on-demand; here the compiler *statically* generates one induction axiom per conjecture (when the conjecture rewrites to `∀x(Rs(x) ⇒ φ(x))`), builds `closed(φ/R) ⇒ sub(φ/R)`, and adds it as an axiom. Simplifications that lose precision but stay **sound**: axiom 3 (rational-term exclusion) omitted; only directly-recursive (no mutual recursion) predicates; **no nested induction** (an inductive argument inside an inductive proof is unsupported).
- **Pipeline.** For a target property, gather requirements (the program P + its dependencies + prerequisite proof files, assumed acyclic), compile P into FOF IND(P), emit each lemma/corollary/theorem as a FOF `conjecture` (with its induction axiom); previously proved facts are reused as FOF `axiom`s. Run both provers: `vampire --mode casc -m 16384 --cores 7 -t $TO` and `eprover --auto-schedule=8 --proof-object …`. A refutation = property proved.

## Results / benchmarks
~400 properties across LPTP libraries (nat, gcd, ack, int, list, suffix, reverse, permutation, sort, mergesort, taut). Success rates (E+Vampire combined): 1s / 10s / 60s per library; e.g. nat 88/95/97%, list 94/96/99%, suffix 100/100/100%, int 87/90/91%, taut 81/84/84%; weak spots gcd 45% (mutual recursion, out of scope) and ack 33% (needs nested induction). **Overall ≈83% at 60s** on a MacBook Pro M2. E and Vampire are complementary (Vampire usually faster; E sometimes proves what Vampire can't). Public compiler + benchmark at github.com/atp-lptp/automated-theorem-proving-for-prolog-verification.

## Novel vs SOTA-2026
Modest but clean: first push-button ATP evaluation of Stärk's LPTP theory, positioned explicitly as **the first step toward a hammer for LPTP** (premise selection and proof reconstruction still TODO). Rides the maturity of E/Vampire and TPTP infrastructure. Not a new proving technique — a translation + delegation study. Related to Dafny (SMT), Why3 (multi-prover export), Anthem+Vampire (ASP equivalence), and superposition-with-datatypes/induction work.

## Adopt-relevance to Theoremata — HONEST
**Medium relevance — the *methodology* transfers even though the domain (Prolog) does not.** This is the clearest of the four papers for our hammer/portfolio design:
- **Hammer architecture, concretely.** The paper is literally an early-stage hammer: (a) encode the ITP/spec theory into a common FOF interchange, (b) fire multiple ATPs in parallel, (c) reconstruct. This is exactly our hammer wiring (Sledgehammer/CoqHammer/aesop) generalized. The named missing pieces — **premise selection** (step 1) and **proof reconstruction** (step 3) — are precisely the two components that make or break a hammer, and are a good checklist for auditing our own hammer track.
- **Portfolio-of-provers with complementarity.** Empirical confirmation that E + Vampire in parallel beats either alone, at multiple timeouts — direct support for our portfolio-proving design and for reporting per-prover, per-timeout success curves rather than a single number. The 1s/10s/60s breakdown is a good template for our eval harness.
- **Soundness-preserving simplification.** Their discipline — drop/weaken axioms (rational-term exclusion, nested induction) but *stay sound* — is a reusable principle for any lossy autoformalization/abstraction step in our sketch→autoformalize-holes pipeline: it's fine to under-approximate provability as long as you never make a false thing provable.
- **Induction-axiom synthesis from the conjecture.** The `closed(φ/R) ⇒ sub(φ/R)` construction (statically instantiating an induction principle from the goal's shape) is a concrete recipe for auto-supplying induction lemmas to a first-order backend — relevant if we feed goals to E/Vampire-style provers and need induction (a known ATP weakness; cf. their ack failures and the cited superposition-with-induction work).
- **TPTP FOF as a lingua franca.** Reinforces having a normalized problem-interchange format so *any* first-order backend in the portfolio is pluggable.

Caveats: the target logic is pure Prolog, not Lean/Rocq/Isabelle mathematics; the completion/three-viewpoint (Rs/Rf/Rt) machinery is Prolog-specific and not reusable. Value is architectural, not the object theory.

## Verbatim-worthy details
- Three-viewpoint predicates per Prolog predicate R: Rs (success), Rf (failure), Rt (termination); operators S/F/T map goals to L̂. Notably `T(\+G) := TG ∧ gr(G)` (safe negation needs groundness).
- IND(P) axioms: uniqueness `¬(Rs(x)∧Rf(x))`; totality `Rt(x) ⇒ (Rs(x)∨Rf(x))`; completion `Rs(x) ↔ S⟦DP_R(x)⟧` (and F, T variants).
- Induction axiom (simplified schema, directly-recursive R): `closed(φ/R) ⇒ sub(φ/R)`, where `sub = ∀x(Rs(x) ⇒ φ(x))` and `closed` replaces `Rs` by `φ` on the RHS and each `R(t)` on the LHS by `φ(t) ∧ R(t)`.
- Prover invocations: `vampire --mode casc -m 16384 --cores 7 -t $TO`; `eprover --delete-bad-limit=2000000000 --definitional-cnf -s --auto-schedule=8 --proof-object --cpu-limit=$TO`.
- Overall ≈83% success at 60s; E and Vampire complementary; failures traced to mutual recursion (gcd) and nested induction (ack) — both explicitly out of the translation's scope but sound.
- Framed as "a first approach towards a hammer for LPTP": premise selector (step 1) and proof-reconstruction module (step 3) are future work.
