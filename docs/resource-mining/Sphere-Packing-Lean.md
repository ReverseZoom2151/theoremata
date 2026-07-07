# Resource Mining: Sphere-Packing-Lean

Full-pass study of `resources/Sphere-Packing-Lean-main/Sphere-Packing-Lean-main` for the Theoremata
project. Focus: the leanblueprint DAG, how a large proof is decomposed into files/obligations,
sorry-tracking, CI/gates, and blueprint→Lean granularity. All prose/config read in full; Lean
sources catalogued and sampled.

Repo: `github.com/thefundamentaltheor3m/Sphere-Packing-Lean` (a.k.a. "Sphere Packing in Lean").
It formalises Viazovska's Fields-Medal proof that the `E8` lattice is the optimal sphere packing in
`R^8` (`Δ8 = π^4/384`), plus Lee's algebraic modular-form inequalities.

---

## 1) What it is (scope, size, module structure)

**Scope.** A blueprint-driven Lean 4 formalisation of a single, deep theorem
(`SpherePackingConstant 8 = E8Packing.density`, in `SpherePacking/MainTheorem.lean`). Toolchain
`leanprover/lean4:v4.30.0`, Mathlib pinned at `v4.30.0` (`lean-toolchain`, `lakefile.toml`).

**Size (excluding build artifacts / .git).**
- 141 tracked files total.
- **~90 Lean modules** (`SpherePacking/**`), **~18,484 LOC** of Lean.
- **11 blueprint `.tex` subsection files** + orchestration TeX (`content.tex`, `web.tex`,
  `print.tex`, macros).
- At this snapshot: **82 `sorry`s across 23 files** (this is the human, in-progress version — see
  §5 on the "sorry-free" milestone).

**Lean module structure** (component-first, mirrors the blueprint's mathematical sections). Root
`SpherePacking.lean` is a generated aggregator that `public import`s every module (checked in CI via
`lake exe mk_all --check --module`). Top-level dirs under `SpherePacking/`:
- `Basic/` — `SpherePacking.lean` (core `structure`), `PeriodicPacking.lean`, `E8.lean`.
- `CohnElkies/` — `LPBound.lean`, `Prereqs.lean` (the linear-programming bound).
- `MagicFunction/` — the heart. Split into `a/`, `b/`, `g/` (the two eigenfunctions and their
  combination), each further split: `a/Integrability/{ComplexIntegrands,CuspPath,Integrability,
  RealDecay,RealIntegrands}.lean`, `a/IntegralEstimates/{I1..I6}.lean` (one file per contour
  integral bound), plus `Basic/Eigenfunction/Schwartz/SpecialValues`. `PolyFourierCoeffBound.lean`,
  `IntegralParametrisations.lean`.
- `ModularForms/` — ~40 files: `Delta`, `E2`, `Eisenstein*`, `JacobiTheta/{Basic,Defs,Derivative,
  JacobiIdentity,MDifferentiable}`, `RamanujanIdentities`, `QExpansion`, `IsCuspForm`, plus many
  small `*_lems.lean` helper files (`clog_arg_lems`, `exp_lems`, `limunder_lems`, `tendstolems`, …).
- `ForMathlib/` — ~26 upstreamable lemma files (`Fourier`, `ZLattice`, `VolumeOfBalls`,
  `RadialSchwartz/Multidimensional`, `CauchyGoursat/OpenRectangular`, `Cusps`, …). A staging area
  for results that "belong in Mathlib" but aren't there yet.
- `Tactic/` — bespoke tactics `NormNumI` (norm_num over `ℂ`/`I`) and `TendstoCont`, each with a
  `Test/` sibling.

Supporting infra: `blueprint/` (LaTeX + plasTeX config), `home_page/` (Jekyll landing page),
`.github/workflows/` (5 workflows), `Makefile`, `requirements.txt`, `lake-manifest.json`.

---

## 2) Reusable ideas/patterns for Theoremata (THE priority)

### 2a. The leanblueprint DAG is exactly the "proof-DAG core" we want, already battle-tested

The blueprint is a LaTeX document whose theorem environments carry machine-readable annotations that
define a dependency graph and a formalisation-status overlay. Real macro syntax, quoted from
`blueprint/src/subsections/*.tex`:

```latex
\begin{theorem}\label{theorem:CE_Main}
  \uses{E8-Lattice,SpherePackingConstant,SpherePacking.density,E8Packing-density,thm:g,
        thm:Cohn-Elkies-general}
  All periodic packing P ⊆ R^8 has density ≤ Δ_{E8} = π^4/384.
\end{theorem}
\begin{proof}
  Directly follows from \Cref{thm:Cohn-Elkies-general} applied to ...
\end{proof}
```

```latex
\begin{definition}\label{SpherePacking.density}
  \lean{SpherePacking.density}
  \uses{SpherePacking, SpherePacking.finiteDensity}\leanok
  ...
\end{definition}
```

The four load-bearing macros (this is the entire schema — see §3):
- `\label{key}` — the node's unique id (a *blueprint* key, not the Lean name).
- `\lean{Fully.Qualified.LeanName}` — binds the node to one or more Lean declarations. Comma-lists
  are allowed, e.g. `\lean{MagicFunction.a.RealIntegrals.a',MagicFunction.a.RadialFunctions.a}`,
  `\lean{F,G}`, `\lean{ModularGroup.S,ModularGroup.T,α,β}`.
- `\uses{key1,key2,...}` — edges to prerequisite nodes (drives the dep graph). Can appear on the
  statement *and independently on the proof* (`\begin{proof}\uses{...}`) — statement-deps vs
  proof-deps are distinguished.
- `\leanok` — "this is formalised & compiles". Placed on a statement it means the *statement* is
  formalised; placed inside `\begin{proof}` it means the *proof* is done. This is the sorry-tracking
  signal.

**Key insight for us:** granularity is at the *mathematical-statement* level, and each node's Lean
binding is the Lean identifier, verified to exist (see checkdecls below). The `\uses` graph is the
obligation DAG; `\leanok` is per-node status. This is precisely our "decompose into obligations +
compile gate + status overlay" — and here it exists as plain TeX that generates an interactive
`dep_graph_document.html`.

### 2b. `checkdecls` / `lean_decls` — the compile-time binding gate

`lakefile.toml` requires a second dependency alongside Mathlib:
```toml
[[require]]
name = "checkdecls"
git = "https://github.com/PatrickMassot/checkdecls.git"
```
leanblueprint emits every `\lean{...}` name into `blueprint/lean_decls` (git-ignored, regenerated),
and `checkdecls` verifies each named declaration actually exists in the compiled environment. This
is the mechanism that keeps the blueprint honest: a node claiming `\lean{Foo.bar}` fails CI if
`Foo.bar` isn't a real, compiling decl. **For Theoremata this is the pattern for "blueprint node ↔
Lean obligation" integrity** — a cheap, non-proof-level check that the claimed symbol exists,
separate from whether its proof has a `sorry`.

### 2c. How a LARGE proof is decomposed (file/obligation granularity)

The decomposition is *isomorphic* between blueprint and Lean directory tree:
- Blueprint `\section` → Lean top dir (`Cohn-Elkies` section → `CohnElkies/`; `Modular forms`
  section → `ModularForms/`; "Fourier eigenfunctions" section → `MagicFunction/`).
- A single hard proposition is sharded into many small obligations. Example: proving `a(x)` is
  Schwartz is broken into `\begin{lemma}` nodes `lem:integral-bound`, `lem:bound-I1-I3-I5`,
  `lem:bound-I2-I4-I6`, feeding `prop:a-schwartz` — and in Lean this is one file *per integral*
  (`IntegralEstimates/I1.lean … I6.lean`). The six contour integrals `I1..I6` are literally six
  blueprint equation labels (`eqn:a-I1..a-I6`) and six Lean files.
- Helper "plumbing" lemmas that don't map to headline math get their own tiny Lean files
  (`ModularForms/exp_lems.lean`, `tendstolems.lean`, …) and often *no* blueprint node.

**Blueprint→Lean granularity ratio (measured across the 11 subsection files):**
- `\label` (nodes/equations): **284**
- `\lean{}` bindings: **94**
- `\leanok` marks: **144** (statements + proofs)
- `\uses{}` edges: **124**
- `\proves{}`: **0** (unused — they rely on `\lean` on the statement rather than the `\proves`
  variant)

So ≈**1 Lean-bound node per 3 labels** (many labels are equations/eqns, not decls), and roughly one
`\uses` edge per node — a fairly sparse but deliberate DAG. Densest files: `modular-forms.tex` (103
labels, 39 `\lean`, 69 `\leanok`, 44 `\uses`) and `construct-a-b.tex` (93 labels, 28 `\uses`).

### 2d. Sorry-tracking

Two independent signals, which we should copy:
1. **Blueprint-level:** absence of `\leanok` = not yet done. The dep graph colours each node
   (statement formalised / proof formalised / blueprinted-only), giving a live "what's left" map
   *before* any Lean is written. Notably `\leanok` on statement and proof are separate, so a node
   can be "stated in Lean, proof still open."
2. **Lean-level:** literal `sorry` (82 here). The two can disagree — a proof `\begin{proof}\leanok`
   should have no `sorry`; the blueprint is the source of truth for intended structure, Lean for
   actual status.

The repo also defines but *doesn't yet use* richer status macros in `macros/print.tex`:
`\mathlibok`, `\notready`, `\discussion{...}` (all defined as no-ops for print; the web/plasTeX
plugin renders them). Worth stealing the vocabulary: "done", "in mathlib", "not ready", "has open
discussion thread".

### 2e. CI / gates (`.github/workflows/build.yml`)

Gate sequence on every push/PR to `main`:
1. **Hygiene grep gate** (fast, pre-build): fails if any file does `import Mathlib` (the whole
   library), `public import Mathlib`, or contains `#check` (stray debug). Enforces narrow imports.
   ```bash
   ! (find SpherePacking -name "*.lean" -exec grep -Hn '^import Mathlib$' {} +)
   ! (find SpherePacking -name "*.lean" -exec grep -Hn '^#check' {} +)
   ```
2. **`lean-action` build** (compiles everything = the axioms/compile gate).
3. **`lake exe mk_all --check --module`** — verifies the aggregator `SpherePacking.lean` imports
   every module (no orphan files).
4. **Blueprint + docs build** via `dwrensha/docgen-action` (blueprint:true, homepage) — this is
   where `checkdecls` runs and the dep graph + API docs are generated and deployed to Pages.

Other workflows: `update.yml` (scheduled Mathlib bump via `mathlib-update-action`, auto-PR on
success / auto-issue on failure), `intentions.yml` + `05-awaiting-review.yml` (a
**comment-driven task board**: contributors type `claim`/`disclaim`/`propose #PR`/`withdraw`/
`awaiting-review` on issues and a bot moves cards across a GitHub Projects board and assigns work).
`lakeOptions` also enforce style: `autoImplicit=false`, `relaxedAutoImplicit=false`, mathlib linter
standard set on.

### 2f. Build/authoring ergonomics

- `Makefile`: `make {pdf,web,serve,all}` → wipes `blueprint/print`, runs `leanblueprint`.
- `requirements.txt`: `invoke==2.2.0` + `leanblueprint` (from Patrick Massot's git). That's the
  whole Python toolchain for the blueprint.
- `plastex.cfg`: plugins `plastexdepgraph plastexshowmore leanblueprint`; `toc-depth=3`,
  `split-level=1`. Two build targets share one `content.tex` via `\ifplastex` conditionals
  (web vs print differ only in decorations).

---

## 3) The blueprint/DAG schema in detail

A blueprint node = one LaTeX theorem-like environment (`theorem/lemma/proposition/corollary/
definition/remark`, from `macros/common.tex`) carrying:

| Field | Macro | Meaning | Cardinality |
|-------|-------|---------|-------------|
| id | `\label{key}` | unique node key (blueprint namespace, arbitrary string e.g. `thm:g`, `E8-Set`, `prop:a-schwartz`) | 1 (required to be a node) |
| lean binding | `\lean{A,B,...}` | fully-qualified Lean decl name(s) this node formalises | 0..n names |
| deps | `\uses{k1,k2}` | prerequisite node keys → DAG edges; may appear on statement and/or on `\begin{proof}` | 0..n, may be split stmt/proof |
| stmt status | `\leanok` (on env) | statement is formalised & compiles | flag |
| proof status | `\leanok` (in proof) | proof is complete (no sorry) | flag |
| (unused here) | `\proves{key}` | mark that this env proves another node | 0 |
| extra status | `\mathlibok`, `\notready`, `\discussion{url}` | defined, not yet used | 0 |

Semantics observed:
- The **dep graph** is built from `\uses` edges over `\label` nodes; direction = "uses/depends-on".
  A leaf with `\leanok` on both stmt and proof and all deps green = fully verified subtree.
- **Node keys are decoupled from Lean names.** `\label{MainTheorem}` binds `\lean{SpherePacking.
  MainTheorem}`; `\label{E8-Set}` binds `\lean{Submodule.E8}`. The blueprint can restructure without
  renaming Lean, and vice-versa.
- **A statement can bind several Lean decls** (e.g. a bound proved as two lemmas
  `finiteDensity_le,finiteDensity_ge`), and **equations get their own labels** (`eqn:a-I1`,
  `eqn:Cohn-Elkies-condition-1`) so proofs can `\cref`/`\uses` sub-parts.
- **Proof-only deps**: `\begin{proof}\uses{thm:Poisson-summation-formula}\leanok` — the proof pulls
  in a lemma the statement doesn't mention. This distinction (statement-deps vs proof-deps) is
  valuable and something our schema should preserve.
- **`\todo{...}`** (red text) marks known gaps/open sub-goals inline in proofs (`macros/common.tex`
  defines it). Used liberally as human TODO markers separate from `\leanok`.
- Orchestration: `content.tex` is a spine of `\section` + `\input{subsections/...}`; `web.tex` and
  `print.tex` both `\input{content}` under `\plastextrue`/`\plastexfalse`. Nodes are authored once,
  rendered twice.

Concrete top-level DAG spine (from `main-result.tex`): `MainTheorem` ← `corollary:upper-bound-E8`
← {`thm:periodic-packing-optimal`, `theorem:CE_Main`}; `theorem:CE_Main` ← {`E8-Lattice`,
`SpherePackingConstant`, `SpherePacking.density`, `E8Packing-density`, `thm:g`,
`thm:Cohn-Elkies-general`}. This is the whole proof in ~6 nodes at the top, fanning out to ~90.

---

## 4) What our earlier targeted pass MISSED

1. **The comment-driven agentic task board** (`intentions.yml`, `05-awaiting-review.yml`,
   `CONTRIBUTING.md`): a fully mechanised claim→work→propose→review lifecycle where GitHub issue
   comments (`claim`, `propose #PR`, `awaiting-review`) drive a Projects V2 board via GraphQL. This
   is a concrete model for how *multiple agents/humans* coordinate obligation ownership — directly
   relevant to a multi-worker Theoremata loop.
2. **`checkdecls` + git-ignored `blueprint/lean_decls`**: the exact mechanism binding blueprint nodes
   to real Lean decls at compile time. Earlier pass noted `\lean` existed but not that a dedicated
   checker gates it.
3. **The statement-vs-proof `\leanok` split and proof-scoped `\uses`** — richer than a single "done"
   bit; our design should carry both.
4. **Per-obligation file sharding** (`IntegralEstimates/I1..I6`, `Integrability/*`) — the physical
   granularity at which a single hard proposition is cut into independently-checkable Lean files,
   one per lemma. Good template for how our decomposer should size obligations.
5. **`ForMathlib/` staging convention** — a first-class directory for "upstreamable" lemmas,
   separating project-specific glue from generically-useful results. Useful category for our
   retrieval/reuse layer.
6. **Hygiene gates as cheap pre-checks**: the `! grep '^import Mathlib$'` / no-`#check` gate runs
   before the expensive build. A model for fast fail-early guardrails.
7. **The unused-but-defined status vocabulary** (`\mathlibok`, `\notready`, `\discussion`) — a
   richer node-status enum than binary done/not-done.
8. **Bespoke tactic + Test/ pairing** (`Tactic/NormNumI` with `Tactic/Test/NormNumI`) — project
   ships its own tactics with regression tests; relevant if Theoremata generates helper tactics.
9. **`\ifplastex` dual-render** — one source, two outputs (web dep-graph vs print PDF), with no-op
   macro shims in `macros/print.tex` for web-only commands.

---

## 5) Test / benchmark value

**This repo is a first-class benchmark for an agentic math harness — and it has already been used as
one.** From `home_page/index.md` (dated content, 2026):

> On 23 February 2026 the team announced a **`sorry`-free proof**… Particularly notable is the role
> played by **Gauss, an autoformalisation agent developed by Math, Inc.** Working off the repository
> as of mid-January, **Gauss autonomously formalised all remaining project goals** to achieve a
> complete `sorry`-free proof… Gauss also produced an autoformalisation of the dimension-24 case.

Benchmark value for Theoremata:
- **Ground-truth blueprint DAG with known-good Lean bindings** — we can test our decomposer by
  checking whether it reproduces the human `\uses` graph, and our binder against `\lean{...}` truth.
- **A partially-complete snapshot (82 sorries / 23 files)** = a realistic "resume an in-progress
  proof" task. Each `sorry` is a well-scoped obligation with a blueprint statement already written —
  ideal fixtures for an "close this obligation" agent eval.
- **checkdecls / mk_all / compile / blueprint-build** form a ready-made objective reward signal
  (does it compile? do claimed decls exist? is the aggregator complete?).
- **A public existence proof** that end-to-end blueprint→formalise→verify at this scale is
  achievable by an agent (Gauss), with a real target to compare our harness against. The M4R_Thesis
  is the human companion doc for the same math.
- **Contour-integral sharding (I1..I6), modular-form helper lemmas** are good medium-difficulty unit
  tasks: self-contained, numeric, Mathlib-adjacent.

Concrete eval harnesses this enables: (a) DAG-reconstruction accuracy; (b) obligation-closing pass
rate on the 82 sorries; (c) retrieval quality against `ForMathlib/` (which lemmas should have been
Mathlib lookups); (d) blueprint-node → Lean-name binding accuracy via checkdecls.

---

## 6) New vs. already-in-our-design

**Already in our design (this validates it):**
- Proof-DAG core with obligations and dependency edges → the `\uses` graph.
- blueprint→formalize→verify pipeline → literally leanblueprint's model.
- Lean compile + axioms/decl gate → `lean-action` build + `checkdecls` + `mk_all`.
- Mathlib retrieval / reuse → the `ForMathlib/` staging pattern is the manifestation.
- Per-obligation status tracking → `\leanok`.

**New / refinements to adopt:**
1. **Split status into (statement-formalised, proof-complete)**, not one bit — mirrors `\leanok` on
   env vs proof.
2. **Separate statement-deps from proof-deps** in our DAG edges (`\uses` on env vs on `\begin{proof}`).
3. **A cheap "declaration exists" gate distinct from the full proof gate** (`checkdecls` model) — our
   binder can verify the symbol compiles before we attempt/trust its proof.
4. **Decouple obligation IDs from Lean names** (blueprint keys ≠ decl names) so we can restructure
   the DAG without touching code and bind one node to *n* decls.
5. **Equation/sub-part labels as first-class DAG nodes** (`eqn:a-I1`) so proofs can depend on pieces
   of a statement, and one hard prop shards into one-file-per-lemma.
6. **A richer node-status enum**: done / in-mathlib / not-ready / has-open-discussion
   (`\leanok`/`\mathlibok`/`\notready`/`\discussion`) rather than binary.
7. **Comment/label-driven obligation ownership + review lifecycle** (intentions bot) — a model for
   multi-agent work assignment and a `awaiting-review` gate in our loop.
8. **Hygiene pre-gates** (narrow-imports, no stray debug) as fast guardrails before expensive
   verification.
9. **Dual-audience rendering** (interactive dep-graph for navigation + linear doc) from a single
   annotated source — a UX target for our TUI/graph view.
10. **`ForMathlib/`-style "upstreamable" bucket** as an explicit category in our reuse layer,
    distinct from project-local glue.

---

### Key file map (cite paths)
- Blueprint spine: `blueprint/src/content.tex`; render targets `blueprint/src/{web,print}.tex`.
- Macro/status defs: `blueprint/src/macros/{common,print,web}.tex` (see `print.tex` for the
  `\lean/\uses/\proves/\leanok/\mathlibok/\notready/\discussion` no-op shims + expl3 `\uses`/`\proves`).
- DAG-rich subsections: `blueprint/src/subsections/{modular-forms,construct-a-b,E8-defs,
  cohn-elkies,packings-density,fourier-analysis,lattice-periodic-packings,main-result,
  modform-ineq,sphere-packings,sphere-packings-scaling}.tex`.
- Blueprint build cfg: `blueprint/src/plastex.cfg`, `Makefile`, `requirements.txt`,
  `blueprint/src/latexmkrc`.
- Gates/CI: `.github/workflows/{build,update,intentions,05-awaiting-review,create-release}.yml`.
- Lean binding gate: `lakefile.toml` (`checkdecls` require), `lake-manifest.json`, ignored
  `blueprint/lean_decls` (`.gitignore`).
- Lean spine: `SpherePacking.lean` (aggregator), `SpherePacking/MainTheorem.lean` (top goal, `sorry`).
- Milestone/benchmark note: `home_page/index.md` (Gauss autoformalisation, sorry-free proof).
- Contribution lifecycle: `CONTRIBUTING.md`.
</content>
</invoke>
