# KakeyaFiniteFields — resource-mining report

Repo: `resources/KakeyaFiniteFields-main` (nested). Extracted zip — **no own `.git`** (zero Gauss commit history to mine). AI-authored by Math Inc.'s "Gauss" agent from a LaTeX blueprint (~315 lines Lean, `Main.lean`).

## 1. Core result & math
Dvir's finite-field Kakeya bound: any Kakeya set `K ⊆ 𝔽_q^n` (contains a line in every direction) has `|K| ≥ C_n·q^n`, `C_n = 1/n!`. Polynomial method: if `|K| < C(q+n-1,n)`, dimension-count yields a nonzero `P`, `deg ≤ q-1`, vanishing on `K`; restricting to each line forces top homogeneous component to vanish everywhere; induction kills `P` — contradiction.

## 2. Blueprint & dependency-graph format
Single `blueprint/src/content.tex` (79 lines), standard **`leanblueprint`**:
- Node = `\label{...}` on a def/lemma/theorem env. Ordering encoded in names: `def:kakeya-set`, `lem:kakeya-set-bound-step1..step4`, `thm:kakeya-set-bound`.
- `\lean{...}` = node→Lean binding (e.g. `KakeyaFiniteFields.step1`).
- `\leanok` = status (on every item here). Other flags in `macros/print.tex`: `\mathlibok`, `\notready`, `\discussion`.
- `\uses{...}` = edges:
  ```latex
  \begin{theorem}[Kakeya Set Lower Bound]\label{thm:kakeya-set-bound} \leanok \lean{KakeyaFiniteFields.kakeya_set_bound}
  \uses{def:kakeya-set, lem:kakeya-set-bound-step1, lem:kakeya-set-bound-step4}
  ```
  Exact DAG: `step1←{}`, `step2←def`, `step3←{def,step2}`, `step4←{def,step3}`, `thm←{def,step1,step4}`. Here `\uses` is statement-level only.
- Print/web dual rendering: `macros/print.tex` makes `\lean/\leanok/\uses/\proves` no-ops for PDF (expl3 `\NewDocumentCommand`); `web.tex` loads `[showmore, dep_graph]`.
- **`blueprint/lean_decls`** lists all 10 decl names; CI `lake exe checkdecls blueprint/lean_decls` asserts existence. `plastex.cfg` plugins as usual.

## 3. Autoformalization workflow evidence
- README: *"All the Lean statements and proofs were produced by Gauss, Math Inc.'s autoformalization agent, guided by a LaTeX blueprint"*; workflow = *"AI-generated formalization from a LaTeX blueprint with human scaffolding."* No prompts/logs/per-attempt artifacts shipped.
- **Granularity**: 5 math items (`def`+`step1-4`+`thm`) → named Lean lemmas, but Lean file has **4 extra helper lemmas with no blueprint node** (`card_finsupp_sum_le_eq_sum_range`, `finrank_restrictTotalDegree_choose`, `coeff_prod_top_of_natDegree_le`, `coeff_top_eval₂_linear_hc_lt`) — the "human/agent scaffolding" (low-level Mathlib bridges). They ARE in `lean_decls` (so `checkdecls` covers them) but carry no `\lean`/`\uses` node. **~5 blueprint nodes → 10 Lean decls (~2× fan-out).**
- No `sorry`; `\leanok` presence/absence is the sorry-tracking mechanism.
- `step1..step4` naming → agent decomposed the proof into a **linear chain of obligations** matching blueprint order.

## 4. Reusable patterns / code
- `checkdecls` + `lean_decls` node-existence gate.
- leanblueprint dual-macro DSL — ingest/emit directly for ecosystem interop.
- CI (`blueprint.yml`): `lake exe cache get` → `lake build` → doc-gen4 → `leanblueprint pdf/web` → `checkdecls` → Jekyll. Self-hosted `morph` runners. Toolchain `v4.26.0-rc2`, mathlib `f9a3323…`.
- Lean retrieval-target exemplars (Main.lean): dimension-counting `LinearMap.ker_ne_bot_of_finrank_lt` + `Submodule.exists_mem_ne_zero_of_ne_bot`; `MvPolynomial.homogeneousComponent` + `IsHomogeneous.eq_zero_of_forall_eval_eq_zero_of_le_card`; `Polynomial.card_le_degree_of_subset_roots`.

## 5. Test-case value
**Strong.** Self-contained, `import Mathlib` only, ~315 lines, 10 decls, complete DAG, no `sorry`, no local imports. Ideal end-to-end input: feed `content.tex` → expect agent to (a) reconstruct the 5-node statement DAG, (b) discover the ~4 unblueprinted support lemmas, (c) compile clean, (d) pass `#print axioms kakeya_set_bound`. The 5-node vs 10-decl gap stress-tests obligation-discovery/graph-completeness. Caveat: pin the mathlib rev (`f9a3323…`, Lean v4.26.0-rc2) or tactic proofs bit-rot.

## 6. New vs. known
Output artifact, not an agent — confirms rather than extends. One thing to consider adopting: the **`leanblueprint`/`checkdecls`/`lean_decls` standard itself** (exact `\uses`/`\lean`/`\leanok`/`\proves` vocabulary + flat decl manifest) → free interop with the Lean-community dep-graph tooling + PDF/web render + a second independent existence check. Plus the empirical ~2× hidden-scaffolding-lemma fan-out for the obligation router to anticipate.
