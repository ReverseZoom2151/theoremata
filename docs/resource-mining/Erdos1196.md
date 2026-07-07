# Erdős #1196 — resource-mining report

Repo: `resources/Erdos1196-main` (nested one level: real root `Erdos1196-main/Erdos1196-main/`). ~39 content files: a Lean4/Mathlib development (13 `.lean`, ~4,000 lines), a `leanblueprint` blueprint, and a standalone TeX writeup.

## 1. Core result & math
Formalizes **Erdős Problem #1196**: every primitive set `A ⊆ ℕ ∩ [x,∞)` (no element divides another) satisfies `∑_{a∈A} 1/(a log a) ≤ 1 + O(1/log x)` as `x→∞` (hence the `1+o(1)` conjecture). Proof (in `source.tex`) is elementary: builds an explicit **sub-Markov chain on the divisibility poset** whose visiting probabilities are exactly `v_x(n) = 1/(B_x · n log n)`; since a primitive set meets any divisibility chain (sample path) at most once, `∑ v_x(a) ≤ 1`. Supporting analytic estimates: Chebyshev `ψ(x)≪x`, a Mertens estimate, tail estimate `T(m,y)`.

## 2. Blueprint & dependency-graph format (highest priority)
Standard **`leanblueprint` + plasTeX** stack (`uvx leanblueprint pdf/web/serve`). Maps ~1:1 to a proof-DAG node/edge schema.
- Layout `blueprint/src/`: `web.tex`/`print.tex` build entry points; both `\input{content.tex}` → `sections/{introduction,preliminaries}.tex`. `macros/{common,web,print}.tex`. `plastex.cfg` sets `plugins=leanblueprint`, `dep_graph`.
- Node types declared in package options: `\usepackage[dep_graph,thms=definition+lemma+proposition+theorem+corollary+...]{blueprint}`.
- Per-node tags (the schema):
  ```latex
  \begin{lemma}\label{lem:tail}
  \lean{PrimitiveSetsAboveX.tailEstimate}   % node → Lean declaration binding
  \leanok                                    % statement formalized/verified
  \uses{lem:mertens}                         % DAG dependency edges (comma-sep)
  \end{lemma}
  \begin{proof}
  \leanok                                    % the PROOF is formalized
  \uses{lem:chebyshev}                       % proof-level dependency edges
  \end{proof}
  ```
  Vocabulary: `\lean{<FQN>}` (node→Lean; also on inline defs), `\uses{...}` (edges, appear in **both** statement and proof blocks → statement-deps vs proof-deps), `\leanok` (verified; separately markable on statement vs proof), plus declared-but-unused `\notready`, `\mathlibok`, `\discussion`, `\proves`.
- Dummy-macro trick (`macros/print.tex`): web-only macros stubbed for PDF (`\newcommand{\lean}[1]{}` etc.); `\uses`/`\proves` emit invisible `\ref`s so undefined labels still warn.
- Machine-readable node↔Lean map: `blueprint/depviz/decls.json` (JSON array of decl names) + `blueprint/lean_decls` (newline-delimited). Observed **drift**: `lean_decls` had 19 entries, `decls.json` only 16 — a real consistency signal. `checkdecls` Lake dep (`PatrickMassot/checkdecls`, in `lakefile.toml`) verifies every `\lean{}` name exists in the compiled library.

## 3. Reusable patterns / code
- **`checkdecls` as a CI gate** — fails build if a `\lean{}` tag points to a nonexistent decl; a cheap "binding is live" check between blueprint and compile. Portable as a DAG-integrity check.
- **`#print axioms` used once, on the top result** (`Main.lean:115`), not per-lemma. Suggests gating only terminal nodes. No `sorry`/`native_decide`/`axiom` anywhere.
- **Two-statement bridge pattern**: internal quantitative theorem (`mainTheorem`, `1+C/log x`) separated from a canonical restatement (`Erdos1196.erdos_1196`) matching the external `formal-conjectures` signature (`o(1)` form). `FormalConjecturesErdos1196.lean` derives one from the other. Template for "prove convenient form → adapt to required benchmark form."
- **Module structure mirrors blueprint DAG**: `Basic` → `Preliminaries*` → `Markov` → `HitMass` → `PrimitiveWeight` → `Main`. Concrete blueprint-section → Lean-file granularity example.
- Uses Lean's newer `module`/`public import`/`@[expose] public section` (toolchain `v4.30.0-rc1`).

## 4. What earlier targeted pass missed
The whole leanblueprint schema and its statement-vs-proof edge distinction; the `checkdecls`/`lean_decls`/`decls.json` node-binding machinery and the observed drift; the benchmark-restatement adapter node.

## 5. Test-case value
**High** — nearly ideal end-to-end input: complete blueprint (with all `\node/\uses/\lean/\leanok` tags) → completed `sorry`-free Lean passing `#print axioms`. Three fidelity checkpoints (informal `source.tex`, blueprint DAG, Lean). Caveat: full Mathlib dep (rev `f369f66…`), genuinely hard analytic NT (~4,000 lines) — a stress test, not a smoke test. Individual preliminary lemmas (Chebyshev, Mertens, tail) are good smaller sub-targets.

## 6. New vs. known
Mostly confirms our design. Additive items:
1. **`\uses` distinguishes statement-deps from proof-deps**, and `\leanok` is independently markable on statement vs proof — a strictly richer node/edge schema than one "depends-on" edge + one "verified" flag. Captures the "statement formalized but proof not yet" state a blueprint→formalize pipeline lives in.
2. **`checkdecls` binding-integrity check** as a distinct cheap gate between blueprint and compile.
3. **Dual node↔decl manifest** as an externalized artifact (+ observed drift) → make node→artifact binding a single source of truth with a consistency check.
4. **Benchmark-restatement node** (adapt to exact required goal signature) — a workflow step our falsify→formalize→verify loop may not model explicitly.
5. `\lean{}` attaches to inline prose definitions too → nodes need not be theorem-environments; allow "definition/notation" nodes bound to Lean `def`s.
