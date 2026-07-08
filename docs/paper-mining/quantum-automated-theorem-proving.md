# Quantum Automated Theorem Proving (QATP)

**Source:** Zheng-Zhi Sun, Qi Ye, Dong-Ling Deng (Tsinghua / Shanghai Qi Zhi / Hefei National Lab), arXiv:2601.07953v1 [quant-ph], 12 Jan 2026.

## Core contribution
Proposes a generic framework ("QATP") that embeds the core inference engines of classical automated theorem proving into quantum subroutines while preserving classical logical semantics. It gives (1) quantum resolution algorithms for propositional and first-order logic with a quadratic reduction in *query complexity* for finding valid resolvents, and (2) a quantum implementation of Wu's algebraic method for geometric theorem proving via a novel quantum pseudo-division algorithm, reducing proof to polynomial identity testing (PIT). Demonstrated end-to-end on IMO 2008 Geometry Problem 1.

## Key techniques / architecture
- **Quantum resolution (propositional).** Knowledge base of M clauses in CNF is loaded by an encoding unitary `U_KB` into a many-body state. Each propositional variable is a **ququart** (4-level system = 2 qubits) with basis states: |0⟩ absent, |1⟩ positive literal, |2⟩ negated literal, |3⟩ resolved. A constant-depth resolution unitary `U_R|c1⟩|c2⟩|0⟩ = |c1⟩|c2⟩|R(c1,c2)⟩` resolves complementary literals in parallel across all N variables (reversibility handled by keeping premises + ancilla for the result). A validation circuit `U_J` (QFT-based counter) flags resolvents where *exactly one* variable is resolved. Amplitude amplification (fixed-point quantum search) boosts the s valid resolvents out of M² candidates from probability s/M² to Θ(1).
- **Complexity.** Classical resolution round is O(M²) pairwise; QATP needs O(M/√s) queries to U_KB and O(s log s) samples to recover all valid clauses → total O(M√s log s), a **quadratic speedup** with the same origin as Grover search. Honestly caveated: propositional entailment is co-NP-complete, so worst case remains exponential; quantum doesn't fix that.
- **First-order logic.** Skolemization + CNF, then invoke **Herbrand's theorem** to reduce FOL refutation to propositional resolution over ground instances of the Herbrand universe (avoids needing quantum unification, which they flag as expensive/hard to implement quantumly). Iteratively enlarge finite subsets of the Herbrand base; semi-decidable, matching FOL's inherent semi-decidability.
- **Quantum Wu's method (geometry).** Geometric hypotheses/conclusion → polynomial equations in coordinates. Triangulate hypotheses by pseudo-division so each introduces one new dependent variable; successively pseudo-divide the conclusion, eliminating variables largest-subscript-first; if final pseudo-remainder ≡ 0 the theorem holds.
- **Quantum pseudo-division.** Key innovation: use **point-value (evaluation) representation** of polynomials, not coefficient encoding. A polynomial F is a unitary `U_F|x⟩|0⟩ = |x⟩|F(x)⟩` with depth linear in number of evaluation points M. This sidesteps three obstacles of state-encoding: combining like terms (irreversible), no-cloning (point-value form is reconstructible/clonable), and entanglement blow-up. The remainder circuit `U_{R,y}` is built directly from dividend and divisor circuits, so its depth is independent of the monomial count — contrasting with classical Wu where intermediate polynomials explode (up to 16172 monomials in the IMO example).
- **Interpolation basis.** Uses **Kravchuk polynomials** rather than FFT/complex arithmetic, avoiding complex-valued floating point; D_s queries to U_S, depth O(D_s log D_s).
- **PIT.** Final remainder zero-test = evaluate over a finite point set G; fixed-point quantum search finds a nonzero evaluation in O(log(2/δ)·√(|G|/h)) queries vs classical O(log(1/δ)·|G|/h) → quadratic speedup in |G|.

## Results / benchmarks
No hardware runs — this is theory + concrete circuit constructions. For IMO 2008 Geometry Problem 1: full first-step pseudo-division circuit uses **28 qubits, depth 4068** for conclusion g1 and depth 89 for divisor h16; final pseudo-remainder R6 = 0 proves the conclusion. Claimed speedups are all quadratic in query complexity (resolvent search and PIT), matching known quantum-search lower bounds; authors note the quadratic separation matches the largest known for unstructured/total-Boolean-function problems.

## Novel vs SOTA-2026
Genuinely novel as the first systematic "quantumization" of symbolic ATP inference engines (resolution + Wu's method) with provable query-complexity separations, plus the point-value quantum pseudo-division construction. It is orthogonal to the neural/LLM SOTA (AlphaGeometry, DeepSeek-R1, LLM provers): no learning, purely algorithmic quantum speedup. Practically it is far from runnable — requires fault-tolerant quantum hardware; speedups are only quadratic (query model), and worst-case exponential barriers persist.

## Adopt-relevance to Theoremata — HONEST
**Low direct relevance** to a Lean/Rocq/Isabelle verification-first harness in 2026. There is no quantum backend and the speedups are asymptotic query-complexity results, not something that helps a classical proof pipeline today. Transferable ideas, all modest:
- **Herbrand-ground reduction as a falsify-before-prove primitive.** Their FOL→propositional reduction via progressively larger finite Herbrand-base subsets is exactly a bounded/iterative-deepening refutation search. This mirrors our falsify-before-prove gate: try to derive the empty clause (a contradiction/counter-derivation) on ground instances before committing to a full proof. Worth having a classical model-finder/ground-resolution falsifier in the portfolio.
- **Wu's method as a decision procedure for a QuantumLean/geometry track.** If Theoremata carries a geometry or QuantumLean domain, Wu's pseudo-division + PIT is a strong *classical* automated tactic for equational/algebraic geometry goals — usable as a portfolio prover independent of any quantum angle. The IMO-2008-G1 polynomial encoding is a ready benchmark.
- **Quantum ATP itself is a benchmark/formalization domain.** Given the QuantumLean track, formalizing these circuit constructions (or their complexity claims) is a candidate hard-problem source — but as *content to prove about*, not a tool we run.
- **Point-value polynomial representation** is a general reminder that representation choice controls blow-up in algebraic proof search (relevant to any CAS-backed tactic), but not quantum-specific.

Bottom line: keep on the radar for the QuantumLean benchmark track and as motivation for a classical Wu's-method geometry tactic + a Herbrand-ground falsifier; no core-harness architecture to port.

## Verbatim-worthy details
- Ququart clause encoding: `A ∨ ¬C` ↦ `|1_A⟩|0_B⟩|2_C⟩|0_D⟩` (0 absent, 1 positive, 2 negative, 3 resolved).
- Reversible resolution: `U_R|c1⟩|c2⟩|0⟩ = |c1⟩|c2⟩|R(c1,c2)⟩`; valid iff exactly one variable ends in |3⟩; `U_J` counts resolved vars via QFT-adder, depth O(N log N).
- Query complexity: classical O(M²) → quantum O(M/√s) queries + O(s log s) samples = O(M√s log s).
- Point-value polynomial unitary: `U_F|x⟩|0⟩ = |x⟩|F(x)⟩`, depth linear in M point-value pairs; remainder circuit built directly from dividend/divisor circuits so depth is independent of monomial count.
- PIT quantum search: O(log(2/δ)·√(|G|/h)) vs classical O(log(1/δ)·|G|/h).
- IMO-2008-G1 first pseudo-division: 28 qubits, depth 4068 (g1) / 89 (h16); pseudo-remainders peak at 16172 monomials classically; R6 ≡ 0 closes the proof.
- FOL handled via Skolemization + Herbrand's theorem (unsatisfiable ⇒ finite unsatisfiable subset of Herbrand base), iteratively propositionalized — no quantum unification needed; FOL remains semi-decidable.
