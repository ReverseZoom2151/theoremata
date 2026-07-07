# Resource mining: `strongpnt` and `ZkLinalg`

Full-pass study of two Math Inc. Lean 4 blueprint repos, both produced by the **Gauss** autoformalization agent from a hand-written LaTeX blueprint. This supersedes the earlier targeted skim. Paths below are relative to `resources/`.

Both repos share the *identical* `leanblueprint` scaffolding (LaTeX blueprint + plasTeX dependency graph + Jekyll `home_page/` + GitHub Pages CI + `checkdecls` gate). The two are the closest public analogs to what Theoremata is building: a proof-DAG whose nodes carry an explicit `blueprint-node -> Lean-decl` binding, gated by "does it compile" + "does the named decl exist."

---

## Repo 1: `strongpnt` (Strong Prime Number Theorem)

Path: `strongpnt-main/strongpnt-main/`

### 1) What it is

An **AI-generated** Lean formalization of the *strong* Prime Number Theorem (PNT with error term `x·exp(-c(log x)^{1/2})`) plus the complex-analysis infrastructure behind it. From `README.md`: statements and proofs "produced by Gauss, an autoformalization agent... completed with targeted human scaffolding and review," ~25k lines / ~1.1k theorems-definitions, **three weeks** wall-clock. Reuses `PrimeNumberTheoremAnd` (Kontorovich's Medium-PNT project) for `PNT5_Strong`, `Z0`, `ZetaZeroFree`.

**Size / structure** (Lean, artifacts excluded):

| File | Lean lines | top-level decls | blueprint `\lean{}` nodes |
|---|---|---|---|
| `StrongPNT/PNT1_ComplexAnalysis.lean` | 5,839 | 334 | 237 |
| `StrongPNT/PNT2_LogDerivative.lean` | 4,206 | 200 | 142 |
| `StrongPNT/PNT3_RiemannZeta.lean` | 4,778 | 238 | 99 |
| `StrongPNT/PNT4_ZeroFreeRegion.lean` | 6,620 | 272 | 136 |
| `StrongPNT/PNT5_Strong.lean` | 6,599 | 66 | 1 |
| `StrongPNT/Z0.lean` | 68 | 2 | 0 |
| `StrongPNT/ZetaZeroFree.lean` | 292 | 6 | 0 |
| **Total** | **28,407** | **1,118** | **615** |

`StrongPNT.lean` is a 5-line import aggregator. Blueprint = 5 chapters (`blueprint/src/PNT/PNT{1..5}_*.tex`, 6,598 lines) wired by `content.tex` via `\input`. Toolchain `leanprover/lean4:v4.21.0`, mathlib pinned `rev = "v4.21.0"`. **Zero `sorry`** across all files.

### 2) Reusable ideas / patterns for Theoremata

**The blueprint-node -> Lean-decl binding is 1:1 and name-exact.** Every blueprint node carries `\lean{<name>}` and the Lean file has a decl of *exactly* that name. Example (`PNT1_ComplexAnalysis.tex:4` -> `PNT1_ComplexAnalysis.lean:3`):

```latex
\begin{lemma}[Log growth]\label{lem:2logOlog} \lean{lem_2logOlog} \leanok
For $t> 1$ we have $2\log t \le O(\log t)$
\end{lemma}
\begin{proof} \leanok
Uses definition of big $O(.)$
\end{proof}
```
```lean
lemma lem_2logOlog : (fun t : ℝ => 2 * Real.log t) =O[Filter.atTop] (fun t : ℝ => Real.log t) :=
  Asymptotics.isBigO_const_mul_self 2 Real.log Filter.atTop
```

Note the LaTeX label (`lem:2logOlog`, used for `\uses` edges) is *distinct* from the Lean name (`lem_2logOlog`, used for the code binding). Two namespaces, deliberately: the DAG is keyed on labels, the compile-gate is keyed on Lean names.

**Decomposition granularity is extreme — micro-lemmas.** Blueprint nodes are often a single rewrite: `log(t^2) = 2 log t`, `Re(bw) = b·Re(w)`, `cos(-a)=cos(a)`, `0 ≤ y²`. Many map to one-line Lean proofs (`exact Real.log_pow t 2`). A node's `\uses{}` frequently lists only the 1-2 immediately-prior micro-lemmas, and the LaTeX proof body is often literally "Apply Lemma \ref{...} and \ref{...}." This is the DAG doing the reasoning; each Lean obligation is trivial. Good model for **how finely Theoremata should shard an obligation** before handing it to a prover: aim for nodes provable in <10 tactic lines.

**Hidden scaffolding ratio ≈ 45%.** 1,118 Lean decls vs 615 blueprint nodes -> **~503 Lean lemmas exist with no blueprint node**. These are un-annotated plumbing lemmas the agent emitted below blueprint granularity — names like `real_part_of_diff`, `mul_comm_div_cancel`, `abs_ofReal_mul_complex`, `factor_I_from_integrand`, `continuous_real_parameterization`. Implication for Theoremata: **the DAG is a skeleton, not the whole proof**; expect the executor to synthesize ~1 hidden helper per named node, and don't try to force every helper into the graph.

**Proof-engineering idiom: verbose narrated proofs.** strongpnt proofs are heavily commented with step-by-step English narration mirroring the blueprint prose (see `Z0.lean:8-64`, `Z0bound_aux`, where every `have` gets a comment explaining the math). Contrast with ZkLinalg's terse style. This is plausibly a Gauss-generation artifact and a signal that the agent "thinks in prose then emits tactics" — useful precedent for Theoremata's own proof-with-rationale traces.

**`\uses` doubles as retrieval hints.** Later chapters cite mathlib lemma names *inside the LaTeX proof prose* (e.g. `PNT4` end: "by Mathlib: `Complex.abs_cpow_le`", "`ArithmeticFunction.LSeries_vonMangoldt_eq_deriv_riemannZeta_div`"). The blueprint carries the exact mathlib API the formalizer should retrieve — directly reusable as **grounding for Theoremata's mathlib retrieval step**.

### 3) Blueprint / DAG schema

Annotation vocabulary (leanblueprint standard):
- `\label{lem:foo}` — DAG node id (LaTeX namespace).
- `\lean{lean_name}` — binds node to a Lean declaration (compile-gate key).
- `\uses{lem:a, lem:b}` — DAG edges; may appear on the statement (uses in *statement*) or inside `\begin{proof}` (uses in *proof*). Both are edges.
- `\leanok` — human/agent assertion "this is formalized & compiles." Appears on the *statement* (Lean decl exists) and separately in the *proof* (proof complete). **Every node in strongpnt has `\leanok` on both** — the whole blueprint is green.
- `\ref{}` / `\cref{}` — prose cross-refs (also render as edges via `\uses`).

Env types (all 5 chapters): 599 `lemma`, 16 `definition`, 6 `theorem`, 0 corollary. Counter is shared/never-reset (`macros/common.tex`: `\newtheorem{lemma}[theorem]{Lemma}`), so numbering is global. Custom macro `\prob{...}` in ZkLinalg only.

**The gate** (`.github/workflows/blueprint.yml`): (a) `lake build` compiles everything (fails on `sorry`/error); (b) `lake exe checkdecls blueprint/lean_decls` — Patrick Massot's [`checkdecls`](https://github.com/PatrickMassot/checkdecls) tool verifies **every name in `blueprint/lean_decls` resolves to a real compiled Lean decl**. `lean_decls` is auto-generated from the `\lean{}` annotations by `leanblueprint web` (strongpnt does *not* commit it — only `blueprint/src/` + screenshot). This is precisely Theoremata's "compile + `#print axioms` gate" pattern, minus the axiom check: **compile-gate + decl-existence-gate**. `checkdecls` is declared as a lake dependency in `lakefile.toml`.

### 4) What the earlier targeted pass MISSED

- **The 45% hidden-scaffolding ratio** (1,118 vs 615) — quantifies how much proof lives *below* the DAG. This is the single most important number for calibrating Theoremata's decomposition granularity.
- **Two distinct name namespaces** (`lem:foo` label vs `lem_foo` Lean name) — a schema detail that matters if we auto-generate blueprints.
- **`checkdecls` is the existence-gate mechanism**, and strongpnt *doesn't commit* `lean_decls` (generated), whereas ZkLinalg *does* commit it — two valid workflows.
- **mathlib API names embedded in LaTeX proof prose** as retrieval grounding (PNT4 tail).
- **Verbose-narration proof style** as a Gauss generation fingerprint (vs ZkLinalg terseness — same agent, different register, likely different prompt/era).
- **Dependency on `PrimeNumberTheoremAnd` at a pinned git rev** (`47f29a1e...`) — the project stands on a large upstream, i.e. Gauss formalized the *delta*, not from scratch.

### 5) Test / benchmark value as end-to-end inputs

- **615 statement/proof pairs at graded difficulty** — from `0 ≤ y²` up to `Strong_PNT` — is a ready-made **decomposition + autoformalization benchmark**. Each node has {LaTeX statement, `\uses` deps, gold Lean statement, gold Lean proof}. Ideal for evaluating Theoremata's blueprint->formalize->verify loop node-by-node with ground truth.
- The **~503 hidden helpers** are a "can your agent invent the missing scaffolding?" test: give it a named node + neighbors, hide the helpers, measure whether it reconstructs them.
- Realistic **mathlib-retrieval eval**: the prose already names target lemmas; measure retrieval precision/recall against those.
- Large enough (28k lines, real analytic number theory) to stress compile-time/olean-cache handling end to end.

### 6) New vs. already-in-our-design

- **Already in design:** proof-DAG core; blueprint->formalize->verify; Lean-compile gate; mathlib retrieval; decompose into obligations. strongpnt validates all of these at scale.
- **New / sharpening:** (a) `checkdecls` as the concrete **decl-existence gate** (cheaper than, and complementary to, `#print axioms`); (b) empirical **~45% hidden-helper budget** to size obligations; (c) the **label vs lean-name two-namespace** schema; (d) mathlib names carried *in the blueprint prose* as first-class retrieval anchors; (e) hard evidence that a single agent can drive 25k lines in 3 weeks with "human writes blueprint + reviews key lemmas" — a workflow shape for our human-in-the-loop.

---

## Repo 2: `ZkLinalg` (Certifying the FRI protocol)

Path: `ZkLinalg-main/ZkLinalg-main/`

### 1) What it is

AI-generated Lean 4 formalization of **FRI** (Fast Reed–Solomon IOP of Proximity) soundness — the core of STARK-style transparent zero-knowledge proofs — built up from elementary linear algebra (matrix mult, code distance, random sampling) to a machine-checked security bound. Follows Evans–Angeris, *Succinct Proofs and Linear Algebra* (ePrint 2023/1478). Again "all Lean statements and proofs produced by **Gauss**, guided by a LaTeX blueprint." Headline result (`README.md`): `fri_security_complete`, soundness error `< 2^{-79}` at `O(n^{0.585})` query complexity.

**Size / structure:** one monolithic Lean file `ZkLinalg/Main.lean` (2,095 lines, **55 top-level decls**, 0 `sorry`), plus a 1-line `ZkLinalg.lean` aggregator. Blueprint `blueprint/src/content.tex` is a single **493-line** self-contained file (no `\input` split), **34 nodes**. `blueprint/lean_decls` is **committed** (34 names, exactly matching). Toolchain `leanprover/lean4:v4.24.0`, mathlib pinned `rev = "v4.24.0"`. Namespace `ZkLinalg`, `import Mathlib` (whole-library).

Decl count vs blueprint: **55 Lean vs 34 nodes -> ~21 hidden helpers (~38%)**, plus 6 `structure`s (bundled hypotheses) and one extra top-level theorem not in the blueprint (see below).

### 2) Reusable ideas / patterns for Theoremata

**Same 1:1 `\lean{}` binding, but with a richer schema** — ZkLinalg annotates definitions too and uses namespaced Lean names. Example (`content.tex:3-7`):

```latex
\begin{definition}[Probabilistic Implication]\label{def:probabilistic_implication}\lean{ZkLinalg.ProbImplies}\leanok
... we write $P(r) \implies_p Q(r')$ iff $\prob{P(r) \land \neg Q(r')} \leq p$
\end{definition}
```
maps to `Main.lean:106`:
```lean
@[simp] def ProbImplies {Ω} [MeasurableSpace Ω] (μ : Measure Ω) [IsProbabilityMeasure μ]
    {R R'} (r : Ω → R) (P : R → Prop) (r' : Ω → R') (Q : R' → Prop) (p : ℝ) : Prop :=
  (0 ≤ p ∧ p ≤ 1) ∧ μ {ω | P (r ω) ∧ ¬ Q (r' ω)} ≤ ENNReal.ofReal p
```

**Bundled-hypothesis `structure`s as reusable "obligation contexts."** ZkLinalg packages the many hypotheses of the main theorem into `structure ... : Prop where` bundles: `FRISubspaceStructure`, `UniformFin`, `UniformSubset`, `FRIDimensionSchedule`, `FRIProverData`, `FRISamplingParams` (`Main.lean:27-101`). The final theorem then takes `(params : FRISamplingParams k m_seq)` instead of 6 loose args. For Theoremata this is the pattern for **carrying an obligation's context object** through decomposition without argument explosion — and these bundle structs are *not all* blueprint nodes (they're inferred infrastructure).

**Executable "bad event" = the derived falsifier, in Lean.** `friRoundBadEvent` (`Main.lean:1671-1700`) is a `def ... : Set Ω` spelling out *exactly* the event "verifier accepts AND data is bad" as a decidable predicate (two `Finset.filter ... .card ≤ q` checks + a disjunction). The whole security theorem bounds `μ` of a `⋃` of these. This is a concrete template for Theoremata's **model-derived falsification / executable check** idea: the property under test is written as a computable set/predicate, and the theorem bounds its measure — the "check" and the "theorem" are the same object.

**Proof-engineering idiom: terse, dense mathlib golf.** Opposite register to strongpnt. Proofs are one-liner `simpa`/`calc`/`classical` chains (e.g. `chaining_probabilistic`, `Main.lean:113-126`, proves the union-bound in a single `simpa ... using (measure_mono ...).trans (...)`). Rich `/-- ... -/` docstrings on every decl carry the *math intuition* (the prose that strongpnt put in `--` comments). The main theorem (`Main.lean:1927-2038`) is a ~110-line structured proof: define `A i`/`B i`/`p i` per-round terms, prove `h_round_le` via `round_wise_error`, sum, then close with `geometric_summation`. Clean model of **"reduce to per-round lemma, then aggregate"** decomposition.

**A production-vs-idealized theorem pair.** `fri_security_complete_production` (`Main.lean:2047-2094`) is a *second* top-level theorem, **not in the blueprint**, that adds deterministic square-root query expansion (`sq_map`, 2-to-1) and then discharges by *directly calling* `fri_security_complete`. Evidence the agent produced deployment-relevant variants beyond the blueprint. For Theoremata: a node can spawn a "hardened"/real-world sibling that reduces to the idealized one.

### 3) Blueprint / DAG schema

Identical annotation vocabulary to strongpnt (`\label`/`\lean`/`\uses`/`\leanok`/`\ref`). Differences worth noting:
- **Node types actually used:** definitions (`def:*`), lemmas (`lem:*`), theorems (`thm:*`) — all three carry `\lean{}`. 34 nodes total, all `\leanok` (fully green).
- **`\uses` present on definitions too** and cross-links the DAG densely, e.g. `thm:subspace_distance_check_n2` (`content.tex:309`) `\uses{def:subspace_distance,def:q_close_subspace,lem:matrix_sparsity_check,lem:linear_independence_from_distance,lem:unique_decoding_radius}` — 5 parents.
- **`blueprint/lean_decls` is committed** (34 lines, `ZkLinalg.ProbImplies` ... `ZkLinalg.fri_security_complete`) and is the exact `checkdecls` manifest. This is the explicit, inspectable DAG->code manifest that strongpnt leaves auto-generated.
- **LaTeX proofs carry full math arguments** (unlike strongpnt's terse "apply Lemma X"), e.g. the ~35-line proof of `subspace_distance_check_n2_deterministic_core` (`content.tex:254-271`) — closer to a paper proof. So the blueprint node granularity is *coarser* than strongpnt (a node can be a substantial theorem), and the Lean proof is correspondingly longer.
- Custom macro `\prob[1]{\Pr\left[ #1 \right]}` (`macros/common.tex`).

Gate identical: `lake exe cache get && lake build` then `lake exe checkdecls blueprint/lean_decls` in CI (`blueprint.yml:97`). plasTeX plugins `plastexdepgraph plastexshowmore leanblueprint`; web preamble `\usepackage[showmore, dep_graph]{blueprint}`.

### 4) What the earlier targeted pass MISSED

- **`fri_security_complete_production`** — an entire second main theorem outside the blueprint (deterministic query expansion), reducing to the idealized one. Missed on a skim; important as a "hardening" pattern.
- **The 6 bundled `Prop` structures** as the mechanism for taming hypothesis count — and that they are hidden infrastructure (not blueprint nodes).
- **`friRoundBadEvent` as an explicit `Set Ω`** — the falsifier-as-def pattern; the theorem bounds its measure.
- **`concrete_parameters` (`Main.lean:1867`) does real numeric verification** in Lean: `|F|≈2^128, n=2^32, q=n/8, k=28, η=470 -> p_total < 2^-79`, closed with `nlinarith [Real.log_two_lt_d9]`. A fully machine-checked *numeric* bound, not just symbolic — a distinct capability to test.
- **`content.tex` is monolithic** (single 493-line file) vs strongpnt's 5-file `\input` split — two organizational conventions for the same tool.
- **~38% hidden-helper ratio** (21/55), plus named helper idioms: `measure_inter_preimage_finset_le_mul_sum`, `choose_ratio_eq_descFactorial_ratio_real`, `descFactorial_ratio_le_pow_div_real`, `one_sub_pow_le_exp_neg_of_mul_ge` (the `(1-λ)^γ ≤ e^{-γλ}` step), etc.

### 5) Test / benchmark value as end-to-end inputs

- **Small, complete, self-contained** (2k Lean lines, one file, 34-node DAG) — the ideal **first end-to-end vertical-slice smoke test** for Theoremata: small enough to run the whole blueprint->formalize->verify->checkdecls loop in one shot, real enough to be non-trivial (measure theory, finite fields, `Nat.choose`, `Real.exp` bounds).
- **34 gold {LaTeX, deps, Lean stmt, Lean proof} tuples** spanning definitions, probability lemmas, coding-theory lemmas, and a capstone theorem — a graded decomposition benchmark in a *different* domain than PNT (crypto/linear algebra vs analytic number theory), good for cross-domain generalization checks.
- **The numeric capstone** (`concrete_parameters` -> `< 2^{-79}`) tests whether the harness can drive a *quantitative* proof to a concrete inequality, not just symbolic manipulation.
- **`fri_security_complete_production` reducing to `fri_security_complete`** is a ready-made "prove B by reducing to already-proved A" task.

### 6) New vs. already-in-our-design

- **Already in design:** blueprint->formalize->verify; DAG; compile gate; mathlib retrieval; decompose into obligations; model-derived / executable falsification (this repo *is* that pattern — `friRoundBadEvent` + measure bound).
- **New / sharpening:** (a) **bundled-`Prop`-structure obligation contexts** to prevent argument explosion during decomposition; (b) **committed `lean_decls` manifest** as an inspectable, diff-able DAG->code artifact (vs strongpnt's generated one); (c) **numeric/quantitative proof closing** (`nlinarith`/`Real.log_two_lt_d9` to hit `2^{-79}`) as a distinct obligation class our verifier must support; (d) **idealized + hardened theorem pair** as a decomposition shape; (e) confirmation that a **monolithic single-file** Lean development pairs fine with a **single-file blueprint** — a lightweight layout for small Theoremata slices.

---

## Cross-repo synthesis (for Theoremata design)

1. **The DAG schema to adopt is leanblueprint's:** node = `{\label (id), \lean (code binding), \uses (edges), \leanok (green flag)}`; two namespaces (LaTeX label vs Lean name). Gate = **`lake build` (compile, no `sorry`) + `checkdecls blueprint/lean_decls` (every bound name exists)**. `lean_decls` may be generated (strongpnt) or committed (ZkLinalg).
2. **Blueprint nodes are a skeleton covering ~55–62% of decls;** budget **~40–45% hidden helper lemmas** the executor must invent below node granularity. Do not force every helper into the graph.
3. **Granularity is a dial:** strongpnt = micro-lemmas (one rewrite/node, trivial proofs, DAG does the reasoning); ZkLinalg = coarse nodes (substantial theorems/node, real proofs). Both work; choose per-domain.
4. **Falsification-as-def** (ZkLinalg `friRoundBadEvent`) and **mathlib-names-in-prose** (strongpnt PNT4) are directly portable: the executable check and the retrieval anchors both live in the blueprint.
5. **Both are gold-labeled benchmarks:** ZkLinalg (34 nodes, one file) is the fast smoke test; strongpnt (615 nodes, 28k lines, 45% hidden) is the scale/decomposition stress test. Same tooling, two domains (analytic number theory vs ZK/coding theory), same agent (Gauss) — a natural eval matrix.
