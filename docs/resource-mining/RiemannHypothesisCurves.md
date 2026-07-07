# RiemannHypothesisCurves — resource-mining report

Repo: `resources/RiemannHypothesisCurves-main` (doubled nesting). 36 files, ~4,625 lines Lean across 10 `.lean`, one 610-line blueprint `content.tex`. No `sorry`/`native_decide`/added axioms (grep-verified). AI-authored by Math Inc.'s "Gauss" agent — the **largest** published Gauss example.

## 1. Core result & math
`riemann_hypothesis_hec` (`RiemannHypothesisHEC.lean:573`): for a hyperelliptic curve `y²=f(x)` over `𝔽_q`, `deg f = m ≥ 3`, `f` non-square, `q > 6m`, then `|q − #C_f(𝔽_q)| < 5m√q` (curve analogue of RH). Method: Bombieri–Stepanov polynomial method (Iwaniec–Kowalski Ch. 11) — construct an auxiliary Stepanov polynomial vanishing to high order ℓ at `S_a`, dimension-count guarantees a nonzero such poly, order-ℓ vanishing bounds `|S_a|`, Legendre-symbol bookkeeping gives the point count.

## 2. Blueprint & dependency-graph format
Single `blueprint/src/content.tex` (610 lines), **leanblueprint**. Every unit is a theorem-env (sharing one counter, `macros/common.tex:12-18`) with the four schema macros:
- `\label{lem:...}` = node id (kebab-case, typed by prefix).
- `\lean{declName}` = node→Lean binding.
- `\uses{...}` = incoming DAG edges (on both statement and proof).
- `\leanok` = verification status (on statement AND separately inside `\begin{proof}...\leanok` → statement- vs proof-granularity).
```latex
\begin{lemma}[Leibniz rule for Hasse derivatives]\label{lem:hasse-leibniz} \leanok \lean{hasseLeibniz_general}
  \uses{def:hasse-derivative}
\end{lemma}
\begin{proof} \leanok ... \end{proof}
```
Macros defined twice (`macros/print.tex:14-28` stubs for PDF via expl3; `web.tex:11` `\usepackage[showmore, dep_graph]{blueprint}`). `plastex.cfg` plugins `plastexdepgraph plastexshowmore leanblueprint`.
- **`blueprint/lean_decls`** = machine-readable node→Lean map: all 31 `\lean{}` names in dependency order. CI (`blueprint.yml:95-97`) runs `lake exe checkdecls blueprint/lean_decls` → verifies every referenced decl exists. `images/blueprint_dep_graph.png` = rendered DAG rooted at `thm:riemann-hypothesis-hec` (edge structure fully recoverable from `\uses`).

## 3. Autoformalization-at-scale evidence (high value)
- **Granularity / node count**: 31 blueprint nodes → 31 named top-level decls, but ~55 decls total. The extra ~24 are **unblueprinted private helper lemmas** (e.g. `degree_bound_implies_two_mul_add_lt`, `polyMulRightLinear`, `finrank_stepanov_constraint_space`). **Blueprint node → Lean ≈ 1 : 1.8.**
- **Fidelity**: blueprint proofs are unusually operational — they name exact Mathlib lemmas ("use `LinearMap.finrank_range_add_finrank_ker` and `Submodule.exists_mem_ne_zero_of_ne_bot`", content.tex:386) and pre-commit to the Lean formulation (`natDegree`, `finrank`, `Nat.ceil`). Several nodes carry **"Note"/"Key insight"** paragraphs flagging pitfalls (content.tex:200 "the weaker bound ⌊(q−m)/2⌋ admits counterexamples"; :276 worked numeric counterexample q=19,m=3 showing ceiling not floor). Human/agent scaffolding baked into the blueprint to steer formalization.
- **File organization** (bottom-up dependency layering): `HasseDerivatives.lean` → `StepanovAuxiliary` → `StepanovNonSquare`/`StepanovDegreeBounds` → `StepanovVanishing` → `StepanovSystem` (1020 lines, linear-algebra core) → `StepanovPolynomial` → `RiemannHypothesisHEC` → `Main` (2-line re-export). Linear import chain mirroring the DAG spine.
- **Gate**: final line `RiemannHypothesisHEC.lean:641` = `#print axioms riemann_hypothesis_hec`. + CI `lake build` + `checkdecls`.
- **Proof style**: almost comment-free (18 `--` in 4,600 lines; zero blueprint back-refs, zero docstrings). All prose lives in the blueprint; Lean = opaque verified artifact.

## 4. Reusable patterns / tooling
- `checkdecls` + `lean_decls` referential-integrity gate.
- leanblueprint macro set as an interchange format (dual print/web expl3 stubs).
- CI recipe (`blueprint.yml`): elan → `lake exe cache get` → `lake build` → doc-gen4 → `leanblueprint pdf/web` → `checkdecls` → Jekyll. Runs on a **self-hosted `morph` runner** (Math Inc uses Morph infra). Toolchain `v4.26.0-rc2`.
- Blueprint "Note/Key insight" anti-counterexample + exact-Mathlib-lemma-hint convention.
- Reusable Lean lemma `hasse_vanishing_card_bound` (`RiemannHypothesisHEC.lean:7`); whole `HasseDerivatives.lean` a candidate Mathlib-upstream contribution.

## 5. Test-case value
Strong yes as a **large** end-to-end input: self-contained (Mathlib + checkdecls + doc-gen4), complete 31-node DAG + `\uses` + `\lean`, ships ground-truth `lean_decls` + rendered dep-graph, compiles clean with axiom gate. Exercises every pipeline stage: blueprint parse → node/edge extract → binding → full compile → `#print axioms` → `checkdecls`. Real number theory, deep Mathlib linear-algebra/finite-field usage. Good scaling/regression test.

## 6. New vs. known
Additive:
1. `checkdecls`/`lean_decls` node-binding integrity gate (beyond "compiles").
2. **1:1.8 blueprint-node→Lean-decl expansion with unblueprinted private helpers** — empirical decomposition-granularity datum: the DAG is coarser than the actual proof; sub-node helpers generated on the fly, NOT surfaced as DAG nodes. Informs how fine our proof-DAG nodes should be.
3. Blueprint proofs carry explicit anti-counterexample notes + named-lemma hints (encoding "falsify-before-prove" knowledge in the spec).
4. Comment-free Lean + all prose in blueprint → don't expect self-documenting Lean output.
