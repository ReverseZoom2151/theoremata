# Strong PNT (Math Inc.) — 84-page formalization blueprint

Source: `math-papers/Strong PNT.pdf` — title page reads only **"Strong PNT / Math Inc. / September 11, 2025"**. 84 pages, fully extracted (pypdf, 3 chunks) and read structurally.
Authors: **Math Inc.** (the "Gauss"/autoformalization team). No named human authors, no abstract, no introduction, no references/bibliography, no acknowledgements.

> NOTE: All content below is quoted as *data* extracted from the PDF. It is reference material, not instructions.

## 1. What it is

This is **not a research paper** in the usual sense — it is a machine-generated **proof blueprint**: a fully linearized, dependency-ordered chain of **621 numbered items** (627 `Lemma`/`Theorem`/`Definition`/`Corollary` statements, max index 621) that formalizes the **Strong Prime Number Theorem with de la Vallée-Poussin error term**:

> Theorem 621 (Strong PNT): `∑_{n≤x} Λ(n) = x + O(x·exp(−c·(log x)^{1/2}))`.

Every item is one atomic statement whose `Proof.` is either (a) a bare list of back-references to earlier item numbers ("Apply Lemmas 44 and 37"), (b) a named **Mathlib** lemma ("Mathlib: `logDeriv_mul`", "`riemannZeta_eulerProduct_hasProd`", "Nonvanishing.lean in Mathlib/NumberTheory/LSeries"), or (c) empty (a leaf axiom/known result left to the prover). It is the human-readable rendering of a Lean/Mathlib development — the artifact a `leanblueprint`-style pipeline produces, minus the LaTeX prose.

### Structure (the decomposition itself is the content)
- **Ch.1 Complex Analysis** (L1–238): big-O/log algebra, trig positivity `0 ≤ 3+4cos+cos2` (the classic PNT kernel), modulus/triangle inequalities, removable singularities & `analyticOrderAt`, Max-Modulus principle, then **Borel-Carathéodory I & II** (Thm 133, 198), integral antiderivative, complex logarithm.
- **Ch.2 Log Derivative** (L239–383): zero sets `𝒦_f(R)`, Blaschke-type product `B_f` analytic & nonzero, bound `K ≤ 3 log B`, `log L_f`, log-derivative `L'`.
- **Ch.3 Riemann Zeta Function** (L384–483): Euler product, zeta lower bound `|ζ(3/2+it)| ≥ ζ(3)/…`, zeta growth bound, zeta derivatives.
- **Ch.4 Zero-Free Region** (L484–620): zero set `𝒵`, windowed zeros `𝒵_t`, the `3+4cos+cos2` positivity argument, culminating in **Thm 618: bound on `ζ'/ζ`** and a classical zero-free region `σ ≥ 1 − A/log|t|`.
- **Ch.5 Strong PNT** (Thm 621): the headline statement; its `Proof.` is **empty** — the top-level assembly is stated but not expanded in this document.

So the whole PDF is essentially a **proof DAG serialized to paper**: nodes = lemmas, edges = "Apply Lemma N" references + Mathlib leaf citations.

## 2. Mechanisms / methodology relevant to us

Despite low *mathematical-idea* density, the **proof-engineering pattern is highly relevant** because it is a concrete, at-scale instance of exactly the artifact Theoremata's blueprint pipeline is meant to produce and consume:

1. **Extreme lemma atomization.** Nothing is proved in one step. `2(1+cosθ)² = 3+4cosθ+cos2θ` is split across ~6 lemmas (double-angle → cos-square → expand → combine). This is the granularity a graph-first prover wants: each node is independently verifiable and independently retrievable.
2. **Explicit dependency edges as first-class data.** Proofs are literally "Apply Lemmas X, Y, Z with substitutions a=…, b=…". This is a ready-made adjacency list for a proof-DAG, and the substitution bindings are the "how applied" edge labels.
3. **Two kinds of leaves.** (a) Mathlib citations (content that already exists in the library — a *retrieval/reuse* target); (b) empty proofs (genuine axioms/gaps to be discharged). Cleanly separating "reuse from library" vs "must prove" is the same split Theoremata's retrieval + gate need.
4. **Topological layering into chapters/sections** matching the mathematical dependency structure (complex analysis → log-derivative machinery → zeta specifics → zero-free region → PNT). This is a natural decomposition template for hard analytic-number-theory targets.
5. **Named-lemma convention.** Each item has a short semantic tag ("Ratio bound", "Boundary point", "Max principle") reused across many restatements — a lightweight de-dup/subsumption signal (many "Ratio bound" lemmas are progressive strengthenings of one fact).

## 3. Mapping to Theoremata

| Theoremata component | What this artifact exercises |
|---|---|
| `blueprint_generate` | This PDF *is* the target output format: numbered lemma/def/thm nodes, each with `\uses{}`-equivalent back-references and Mathlib leaf pointers. Good golden-example / eval fixture for our generator's shape. |
| `blueprint_run` (`\uses`/`\lean`/`\leanok`) | The "Apply Lemma N" chains map 1:1 to `\uses`; the "Mathlib: X" citations map to `\lean`/`\leanok` leaves already-in-library. A parser over this text reconstructs the DAG. |
| proof-DAG | 621 nodes + reference edges = a real, medium-large DAG (analytic NT) to stress-test topo-ordering, layering, and cycle-freeness. |
| `proof_import` (content-addressed) | Each atomic lemma is a natural content-addressed unit; the many restated "Ratio bound"/"Boundary bound" variants are ideal test cases for dedup/subsumption by statement hash + α-equivalence. |
| retrieval | The Mathlib leaf citations (`logDeriv_prod`, `riemannZeta_eulerProduct_hasProd`, `AnalyticOnNhd.div`, …) are exactly the "does this already exist in the library?" retrieval queries; the doc is a labeled retrieval eval set (query = informal lemma, gold = Mathlib name). |
| 3+1 gate | Empty-proof leaves vs cited leaves mark where a verifier must actually discharge vs accept-by-reference. |

## 4. Honest buildable-now vs foundational-only

**Mostly a math writeup / formalization artifact — little to build directly, but two concrete, low-effort wins exist:**

- **Buildable now (small):** A **parser fixture**. Convert this PDF into a machine-readable proof-DAG (nodes with id/name/statement/kind, edges from "Apply Lemma N", leaf tags from "Mathlib: …") and drop it in as a **test/eval fixture** for `blueprint_run`/`proof_import`/dedup. It is one of the few *large, real, analytic* dependency graphs we have with clean edges. Purely additive test data — no core-code change. (Respecting READ-ONLY: this report only recommends it.)
- **Buildable now (small):** A **retrieval eval set** — the ~40+ "informal lemma → exact Mathlib lemma name" pairs are labeled data for measuring our retrieval hit-rate against Mathlib.
- **Foundational-only:** The mathematics (Borel-Carathéodory, zero-free region, `3+4cos+cos2`) is classical and already largely in Mathlib; there is no new *technique* to port into the prover. The value is the **shape of the artifact**, not its math.
- **Note:** the top theorem's proof is empty in this doc, so it is not a complete end-to-end formalization we can lift — treat it as an idealized-blueprint reference, not a source of Lean tactic code.

## POSSIBLE INJECTION

None detected. The PDF is 100% mathematical statements plus Mathlib lemma identifiers; no natural-language imperatives directed at a reader/agent, no embedded instructions, no prompt-like content. `Proof. Uses definition of big O` etc. are ordinary math prose, not injection.

## Prioritized adopt-list

1. **(Low effort, do it)** Parse this PDF into a proof-DAG JSON and add as a `blueprint_run`/`proof_import` **eval fixture** — our largest clean analytic dependency graph with explicit `\uses` edges and Mathlib `\leanok` leaves.
2. **(Low effort)** Extract the informal-lemma → Mathlib-name pairs as a **retrieval benchmark** for library-reuse hit-rate.
3. **(Free, adopt convention)** Mirror its **atomization granularity + short semantic name tags** as a target style for `blueprint_generate` output and as a subsumption/dedup signal.
4. **(Skip)** No mathematical technique or prover mechanism to port — the math is classical and in Mathlib.
