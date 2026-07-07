# M4R_Thesis — "Viazovska's Magic Function in Dimension 8: An Attempt at Formalisation"

Sidharth Hariharan, MSci thesis, Imperial, 2025. Human-written companion to `Sphere-Packing-Lean`. Real base: `M4R_Thesis-main/M4R_Thesis-main/`. `main.tex:13` flags it as a deliberately expanded, non-submitted version meant as a bridge to the blueprint: *"The text here, particularly in Chapter 4, is meant to mimic the arguments that we have and will formalise."* Prose engineered 1:1 with Lean obligations is the central methodological artifact.

## 1. Core content
Detailed exposition of §4 of Viazovska's 2017 Annals paper (construction of the Fourier eigenfunctions) + a report on formalising it. Reduces sphere-packing in R^8 to a Schwartz "magic function" `g` satisfying Cohn–Elkies (CE1–CE3), decomposed via radial Fourier-eigenfunction splitting `g = g₊ + g₋` (`Chapters/3_Roadmap/`); constructs `a`,`b` as sums of six contour integrals of modular-form integrands, proving Schwartzness, eigenfunction property, double zeroes, specific values (`Chapters/4_Magic_Fun/`). Main **formalization** contributions: near-complete Lean proof of the polynomial-Fourier-coefficient bound (blueprint Lemma 7.4), a Schwartz-across-dimensions bridge, an unbounded-contour Cauchy–Goursat, and the `norm_numI` tactic for ℂ computation.

## 2. Formalization methodology & lessons (highest priority)
- **The prose IS the decomposition spec.** `5_Lean.tex:8`: *"we provided incredibly detailed arguments … to ensure a correspondence between the informal text and the formal proof path."* `1_2_Formalisation.tex:15` singles out the blueprint's two load-bearing features: linking definitions/theorems to code, and a **colour-coded dependency graph** showing formalisation progress. Direct human precedent for a proof-DAG with per-node status.
- **Fine-grained obligation decomposition beats monolithic lemmas — measurably.** `5_1_Project.tex:10`: *"breaking computations into several lemmas … improved not only readability but also compilation time. The author's formal proof of [PolyFourierCoeffBound] consists of thirteen auxiliary lemmas."* And it **isolates hard sub-obligations**: *"isolation of dependencies that are difficult to formalise, such as convergence results for sums, products and integrals."* → DAG whose leaves are exactly the convergence/measurability side-conditions.
- **Definitions chosen to fit available library lemmas, even at math cost.** Six rectangular-contour integrals instead of four unspecified, *"because a formal version of the Cauchy–Goursat Theorem exists in mathlib for rectangular contours"* (`5_1_Project.tex:8`, `4_1_Defs.tex:167`). **Statement shape is a free variable to optimize against the toolchain.**
- **A local optimization can globally backfire — track it.** The rectangular-contour choice that eases the double-zero proof *breaks* the eigenfunction proof (`5_1_Project.tex:8`), which needs z↦−1/z sending contours to quarter-circles → forces a still-unformalized "Squares and Circles" Cauchy–Goursat (`5_3_Cauchy-Goursat.tex:98-150`). A definitional choice is a shared upstream node; changing it re-colors all dependents (helpfully AND harmfully).
- **mathlib gaps that actually hurt**: Poisson summation only over ℤ⊂ℝ not general lattices (`2_2_Cohn_Elkies.tex:53`); no Jordan Curve Theorem → no general Cauchy–Goursat/contour deformation (`5_3_Cauchy-Goursat.tex:14`); no automation for ℂ computation (`norm_num`/`simp`/`field_simp` fail on `i`, `5_2_norm_numI.tex`); weak infinite-product theory.
- **Naïve "obvious" bridges are the traps.** Proving `a_rad,b_rad` are 1-D Schwartz does NOT give 8-D Schwartzness (`5_1_Project.tex:50`, headlined in `6_2_Summary_Formal.tex:3`); integrands are only Schwartz-*like* on [0,∞) → bespoke `SchwartzLike` theory. **Obligations phrased "clearly X follows" are exactly where autoformalization silently fails; a falsify-first pass should target them.**
- **Exploit library definitional conventions as free lemmas.** mathlib's integral-of-non-integrable = 0 makes some convergence proofs unnecessary; monotonicity needs only the upper function integrable (`5_1_Project.tex:10,40`). Encode toolchain conventions as node preconditions to prune obligations.
- **Proof-engineering recipe for integral bounds** (`5_1_Project.tex:14-43`, one file per integral): (1) `_eq` lemmas strip parametrization + pull scalars out; (2) change of variables s=1/t via a mathlib Jacobian lemma; (3) **bound the integrand not the integral** so inequalities chain and only ONE integrability obligation remains; (4) bound via triangle inequality + monotonicity. Template-instantiable across near-identical integrals.
- **Honest surfacing of incompleteness**: `sorry`s explicitly located; a full `sorry`-free proof framed as the acceptance gate (`6_Conclusion.tex:7`). Mirrors a `#print axioms`/no-`sorry` gate.

## 3. Reusable patterns
- Blueprint = DAG with colour-coded per-node status + bidirectional code↔prose links (`leanblueprint`).
- **Template obligation families** (I₁/I₃/I₅; I₂/I₄; I₆; J-analogues) generated by swapping the integrand — emit as parameterized nodes sharing a proof skeleton.
- "Bound the integrand, defer one integrability obligation" as a general side-condition-minimizing tactic.
- Statement-shape selection against tool capability as an explicit tracked decision.
- **`norm_numI` architecture** (`5_2_norm_numI.tex`): recursive `parse` splits a ℂ-expr into (Re,Im) *with a proof term* via `split_add`/`split_mul`, delegates each real part to `norm_num`. Generalizable to any `R[X]/(f)` normal form (quaternions, splitting fields). Pattern for **synthesizing a domain-specific normalization tactic** when a whole obligation class is blocked by missing automation.
- CI auto-builds the human artifact (PDF+HTML, dep-graph) on every push — "living document."

## 4. What earlier targeted pass missed
The entire methodology layer above — this repo is a human *reflecting on the act of formalizing*, which the earlier skim (focused on the Lean/blueprint) didn't extract.

## 5. Test-case value
Several self-contained end-to-end inputs, increasing scope: **`norm_numI` unit goals** (smallest, ℂ-only, e.g. `(1+I)*(1+I*I*I)=2`, good tactic-synthesis + falsify test); **PolyFourierCoeffBound / Lemma 7.4** (`4_2_Schwartzness.tex:10-45`, complete informal proof in ~13 steps with exact mathlib lemmas cited — ideal medium blueprint→formalize→verify item); **Schwartz-from-Schwartz-like bridge** (known-hard, partially-open); **integral-bound family** (template instantiation). Full magic-function pipeline too large/open for one input but a realistic large-graph stress test. Caution: intentionally non-final draft with visible TODOs and small typos (e.g. `b` def erroneously says "eigenfunction a", `24!` for `4!`) — do NOT treat prose as a verified oracle.

## 6. New vs. known
Additive (not obviously covered):
1. **Statement-shape as an optimization variable** — choose among equivalent statements by toolchain-fit; the choice is a shared upstream DAG node whose change re-colors all dependents.
2. **Compile-time as a decomposition signal** — a measurable objective beyond success/failure.
3. **Isolate hard side-conditions as DAG leaves** + encode toolchain conventions (zero-on-non-integrable) as node preconditions to prune obligations.
4. **"Bound the integrand, defer one integrability proof"** heuristic for analysis obligations.
5. **Auto-generating domain-specific normalization tactics** (`norm_numI`) — "synthesize a reusable tactic for this recurring blocker" is higher-leverage than per-obligation best-of-N.
6. **Inventing intermediate definitions** ("Schwartz-like") to unblock a proof — beyond retrieving existing ones.
7. **Naïve-bridge pitfalls as a falsification target** — strong prior for where falsify-before-prove pays off in analysis.
