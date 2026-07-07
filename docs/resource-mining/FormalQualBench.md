# Resource Mining: FormalQualBench

Full-pass study of `resources/FormalQualBench-main/FormalQualBench-main/`.
Every code and prose file was read in full (README, PROBLEMS.md, `FormalQualBench.lean`,
`Basic.lean`, all 23 problem `Main.lean` files, `lakefile.toml`, `lean-toolchain`,
`lake-manifest.json`, `.gitignore`). There is **no Python, no config JSON committed, and no
runner code in the repo** — the entire repo is a Lean 4 + Mathlib library plus two markdown files.

---

## 1) What it is

**FormalQualBench** is a Lean 4 + Mathlib benchmark of **math PhD qualifying-exam-level theorem
statements** (`README.md:1-10`). Its explicit purpose is autoformalization / agent-design evaluation:
> "Our benchmark enables practitioners to rapidly iterate on and evaluate agent design decisions in
> practical formalization workflows." (`README.md:3-4`)
> "We expect this benchmark to approach saturation within a few months as frontier autoformalization
> systems improve." (`README.md:10`)

**Core artifact** (`README.md:6-8`): a collection of small self-contained Lean modules, each containing
(a) the definitions needed to *state* a result, and (b) a **single main theorem statement closed with
`by sorry`** — no proofs. The task the benchmark poses is: **replace `sorry` with a real proof** (or,
for autoformalization, produce the statement itself).

**Size:** 23 problems (23 `Main.lean` files; all 23 checked-off `[x]` in `PROBLEMS.md`). This is a
small, hand-curated, high-difficulty set — not a large corpus.

**Curation philosophy** (`PROBLEMS.md:3-15`) — this is the distinctive design choice:
- Intentionally **research-level**, explicitly **no IMO/Putnam/competition** problems.
- Statements of **proven** results from the literature **not yet available as a finished Lean/Mathlib
  theorem** — "mathematically established results rather than unsolved conjectures."
- Two curation signals (`PROBLEMS.md:13-15`):
  1. Mathlib `proof_wanted` declarations (known results with missing Lean proofs), and
  2. Famous results whose *statements* are expressible with existing Mathlib definitions "even if the
     proof is far out of reach."

So the difficulty axis is deliberately **frontier / near-impossible to prove** but **easy to state** —
the benchmark tests whether an agent can produce a faithful formal statement and (aspirationally) a
proof of results like Green–Tao, Kakeya, Quillen–Suslin.

**Domains covered** (from `PROBLEMS.md` section headers — the domain taxonomy is itself reusable):
Topology/geometry; geometric functional analysis; logic/model theory; logic/Ramsey theory; number
theory; group theory; commutative algebra; real algebraic geometry; complex analysis; additive
combinatorics/analytic number theory; operator algebras; topological groups/harmonic analysis; graph
theory; convex geometry; geometric measure theory; fixed-point theory; arithmetic dynamics/recurrence
sequences; dynamical systems.

Full problem list (name → file, all under `FormalQualBench/<Name>/Main.lean`):
Banach–Stone, Borsuk–Ulam, Burnside prime-degree, Collatz almost-bounded values (Tao), Colorful
Carathéodory, De Bruijn–Erdős, DLO quantifier elimination, Erdős discrepancy (Tao), Gleason–Kahane–
Zelazko, Green–Tao, Hilbert 17th (Artin), Jordan cycle, Jordan derangement, Kakeya 3D, Maynard–Tao
bounded prime gaps, Paris–Harrington, Pontryagin duality, Quillen–Suslin, Runge, Schauder fixed point,
Skolem–Mahler–Lech, ternary Goldbach (Helfgott), von Neumann double commutant.

**Structure / layout** (`README.md:16-19`):
- Each problem: `FormalQualBench/<ProblemName>/Main.lean`.
- Library entrypoint `FormalQualBench.lean` → `import FormalQualBench.Basic`.
- `FormalQualBench/Basic.lean` is just 23 `import` lines aggregating every problem module (so
  `lake build` compiles all statements).
- Build: `lake build` (`README.md:22-23`).

**Toolchain pinning** (matters for ingestion reproducibility):
- `lean-toolchain`: `leanprover/lean4:v4.28.0`
- `lakefile.toml`: Mathlib pinned to `rev = "v4.28.0"`; `defaultTargets = ["FormalQualBench"]`;
  lean options `relaxedAutoImplicit = false`, `weak.linter.mathlibStandardSet = true`,
  `maxSynthPendingDepth = 3`.
- `lake-manifest.json` pins the exact Mathlib commit `8f9d9cff6bd728b17a24e163c9402775d9e6a365` plus
  the full transitive dep set (aesop, batteries, Qq, ProofWidgets, importGraph, LeanSearchClient,
  plausible, Cli).

---

## 2) Reusable ideas / patterns / code for Theoremata  (PRIORITY SECTION)

### 2a. The benchmark item SCHEMA (this is the key finding)

There is **no JSON/YAML metadata schema**. A "benchmark item" *is a Lean file*. The implicit schema,
inferred uniformly across all 23 files, is:

```
one directory  FormalQualBench/<ProblemName>/
  └── Main.lean:
        <minimal imports>                    -- either `import Mathlib` (whole) or targeted imports
        namespace <ProblemName>              -- namespace == directory name == problem id
        [open ...]                           -- optional
        [noncomputable section]              -- optional, when defs are noncomputable
        <auxiliary def/abbrev ...>           -- the "necessary definitions to state a result"
        /-- docstring: named theorem + attribution -/
        theorem MainTheorem <binders> : <goal> := by
          sorry
        end <ProblemName>
```

**Load-bearing naming conventions (the de-facto schema contract):**
- The **theorem is always named `MainTheorem`**, and its fully-qualified name is
  `<ProblemName>.MainTheorem` — e.g. `DeBruijnErdos.MainTheorem` (used verbatim as the
  `theorem_names` key in the comparator config, `README.md:50`).
- The **namespace equals the directory name equals the problem identifier** — a clean, stable primary
  key. This is exactly the kind of stable node ID Theoremata's proof-DAG needs.
- The **proof body is always `by sorry`** — the single, greppable "hole" that defines the task.
- Every theorem carries a **docstring** naming the result and often its attribution/context; several
  cite provenance, e.g. Jordan cycle notes the Mathlib `proof_wanted` origin
  (`JordanCycleTheorem/Main.lean:11-12`: *"this statement already appears in mathlib as a
  `proof_wanted` declaration `alternatingGroup_le_of_isPreprimitive_of_isCycle_mem`"*).

Two import styles coexist (a real signal for our ingestion):
- **Whole-Mathlib** `import Mathlib` (Borsuk–Ulam, Collatz, Colorful Carathéodory, Erdős discrepancy,
  Hilbert17, Quillen–Suslin) — convenient, slow to compile.
- **Targeted imports** (Banach–Stone imports 3 specific modules; Burnside imports 7; De Bruijn–Erdős a
  single `Mathlib.Combinatorics.SimpleGraph.Coloring`) — faster, and encodes a *dependency hint* about
  which Mathlib area the result lives in.

### 2b. How correctness is judged  (answers the "Lean compile? answer match? judge?" question)

**Correctness = Lean type-checks the proof AND uses only a whitelisted axiom set.** Judged by an
**external tool, not committed code**: `leanprover/comparator` + `Zouuup/landrun` sandbox
(`README.md:26-52`). The workflow (`README.md:39-52`) is the reusable grader design:

1. Write the proof in `Solution.lean`; copy the original `sorry` stub to `Challenge.lean`.
2. Run comparator with a `config.json`:
```json
{"challenge_module":"Challenge","solution_module":"Solution",
 "theorem_names":["DeBruijnErdos.MainTheorem"],
 "permitted_axioms":["propext","Quot.sound","Classical.choice"],
 "enable_nanoda":false}
```
3. **`exit 0` = valid** (`README.md:43`).

Key mechanics worth porting to Theoremata's formalize→Lean-compile + **axioms gate**:
- **`challenge` vs `solution` module comparison**: the grader checks the solution's `MainTheorem` has
  the *same statement* as the original stub (prevents cheating by weakening the theorem) and that it is
  proved. This is precisely a "statement-preservation + proof-present" check — directly the semantics
  our axioms gate + statement-equality check should implement.
- **`permitted_axioms` whitelist**: `["propext","Quot.sound","Classical.choice"]` — the standard "Lean
  is consistent" trio. Anything else (notably `sorryAx`) fails the gate. This is the concrete, correct
  default axiom allowlist for Theoremata's axioms gate. `sorry` leaves `sorryAx` in the axiom set, so
  an unfilled hole is automatically rejected.
- **`landrun` sandbox**: proofs are checked under a Linux Landlock sandbox — untrusted model-generated
  Lean is run with filesystem restrictions. Relevant to Theoremata's hardening/guardrail modules:
  compile untrusted proofs in a sandbox.
- **`enable_nanoda:false`**: a comparator toggle (extra checker) left off.
- Setup pins comparator to the **same `v4.28.0` tag** as the toolchain (`README.md:33`) — grader and
  library must share a Lean/Mathlib version.

### 2c. Difficulty tiers

**No explicit numeric difficulty field.** Difficulty is encoded implicitly/uniformly: *all* items are
"research-level, far out of reach to prove" (`PROBLEMS.md:3-15`). The only structured stratification is
the **domain taxonomy** (18 subject headers in `PROBLEMS.md`) and the binary `proof_wanted`-vs-
"statement-only-feasible" curation signal. Takeaway for Theoremata: if we ingest this, difficulty tier
= a single "frontier / qual-exam" tier; domain = the taxonomy label; sub-signal = whether a Mathlib
`proof_wanted` exists.

### 2d. Directly reusable code/patterns

- The **stub template** (§2a) is a ready-made formalization-target format: minimal imports + defs +
  `theorem MainTheorem … := by sorry`. Theoremata can adopt `MainTheorem` naming + `namespace ==
  problem-id` as a node convention for Lean formalization tasks.
- The **aggregation entrypoint pattern** (`Basic.lean` importing every problem) gives a single
  `lake build` smoke-test that all statements *elaborate* (compile without the proof) — a cheap
  "statement well-formedness" gate independent of proving. Theoremata can reuse this: elaborating the
  statement with `sorry` allowed is the "is this a valid formal statement?" check for the formalize
  step; the full comparator run is the "is the proof valid?" check.
- The **comparator `config.json` schema** is a compact, reusable grader contract (challenge module,
  solution module, theorem names, permitted axioms) — a good model for Theoremata's grader I/O.

### 2e. Real schema/prompt/statement quotes

There are **no prompts** in the repo (no LLM prompt files at all). The "schema" quotes are the Lean
stubs themselves. Representative extremes:

Shortest / cleanest stub — Green–Tao (`GreenTaoTheorem/Main.lean`), pure statement, no aux defs:
```lean
import Mathlib.Data.Nat.Prime.Basic
namespace GreenTaoTheorem
/-- Green-Tao theorem: the primes contain arbitrarily long arithmetic progressions. -/
theorem MainTheorem :
    ∀ k : ℕ, ∃ a d : ℕ, 0 < d ∧ ∀ i : Fin k, Nat.Prime (a + i.1 * d) := by
  sorry
end GreenTaoTheorem
```

Def-heavy stub — Collatz (`CollatzMapAlmostBoundedValues/Main.lean`) defines `collatz`,
`AttainsBelow`, `logWeightSum`, `LogDensityZero` before `MainTheorem`, showing the "necessary
definitions to state the result" pattern with `noncomputable def`.

Statement-as-`def`-then-trivial-theorem pattern (Erdős discrepancy,
`ErdosDiscrepancyProblem/Main.lean`): the content is a `def DiscrepancyUnbounded : Prop := …` and then
`theorem MainTheorem : DiscrepancyUnbounded := by sorry`. Same for DLO (`EliminatesQuantifiers` def).

---

## 3) Data / eval format

- **Data format:** none tabular. Each item is a directory + one `Main.lean`. No `.json`/`.jsonl`/`.csv`
  dataset, no metadata sidecar, no license-per-item. Metadata lives only as (a) the `namespace`, (b) the
  theorem docstring, and (c) the `PROBLEMS.md` domain heading + checkbox.
- **Catalogue file:** `PROBLEMS.md` is the human-readable index (checkbox list grouped by domain, each
  linking to its `Main.lean`). Machine-parseable enough to scrape (regex on `` `FormalQualBench/…` ``).
- **Eval format:** external. Inputs to the grader = the `config.json` (§2b) naming challenge/solution
  modules, target theorem names, permitted axioms. Output = process exit code (`0` = valid). Sandbox =
  `landrun`. No accuracy aggregation script is shipped — the repo grades one theorem at a time.
- **Toolchain reproducibility:** fully pinned (`lean-toolchain` v4.28.0, Mathlib rev pinned in
  manifest). Ingestion must match this exact Lean/Mathlib to elaborate the stubs.

### Sampling done for this study
I read **all 23** `Main.lean` files in full (not a sample) plus every prose/config file. Verified the
schema is uniform across all 23: every file has `namespace <Name>` = dir name, exactly one
`theorem MainTheorem`, body `by sorry`, a docstring on the theorem, and either `import Mathlib` or
targeted imports. Auxiliary `def`/`abbrev` present in ~11 of 23; the rest are pure statements.

---

## 4) What our earlier targeted pass MISSED

Likely gaps a targeted skim would have left, now filled:
1. **The grader is real and specified, just external.** The earlier framing "benchmark with any eval
   harness" — the eval harness is the `comparator`+`landrun` recipe in `README.md:26-52`, including the
   exact `config.json` schema and the **`permitted_axioms` whitelist** `["propext","Quot.sound",
   "Classical.choice"]`. This is the single most reusable artifact and is easy to miss because it's
   prose in the README, not code.
2. **The challenge-vs-solution statement-preservation mechanism** (copy stub to `Challenge.lean`,
   proof to `Solution.lean`; comparator checks they match) — a concrete anti-cheating design, not just
   "does it compile."
3. **`sorryAx` rejection is implicit**: because only three axioms are permitted, any residual `sorry`
   fails automatically — the axioms gate *is* the "no cheating with sorry" gate.
4. **The curation rule** "no competition problems; only proven-but-unformalized results; seeded from
   Mathlib `proof_wanted`" (`PROBLEMS.md:3-15`) — a deliberate difficulty/selection policy, plus the
   explicit provenance note in Jordan cycle pointing to a named Mathlib `proof_wanted` decl.
5. **Two distinct import strategies** (whole `import Mathlib` vs targeted) — a real ingestion/perf
   consideration and a dependency-domain hint.
6. **Exact version pinning** down to Mathlib commit — the grader and library share `v4.28.0`.
7. **Statement-as-`def` idiom** (Erdős, DLO, Collatz) — the mathematical content sometimes lives in a
   supporting `def`, with `MainTheorem` being a one-line wrapper. A naive "extract the theorem" parser
   would miss the actual content.

---

## 5) Test / benchmark value — can we ingest it as a formalization track?

**Yes, high value, low ingestion cost, with one caveat.**

- **As a formalization/proving track:** directly ingestible. 23 clean, uniform, pinned Lean stubs with
  a stable id scheme (`<Name>.MainTheorem`) and an already-specified pass/fail grader (comparator axioms
  gate). Maps almost 1:1 onto Theoremata's *formalize → Lean compile → axioms gate → grader* pipeline.
  We can wrap each `Main.lean` as a benchmark node: id = namespace, domain = `PROBLEMS.md` heading,
  goal = `MainTheorem` statement, success = comparator exit 0 with the three-axiom whitelist.
- **Two evaluation modes it supports:**
  1. *Proof track* (hard, likely ~0% for now): fill the `sorry`. Author expects near-saturation-later,
     ~impossible now — good as a frontier stress test / regression tripwire, not a metric that moves.
  2. *Autoformalization track* (more tractable): give the model the natural-language statement (the
     docstring) + allowed imports and ask it to **reproduce the formal statement**; grade by
     definitional/statement equality against the reference `MainTheorem` (the comparator's
     challenge-match mechanism supports exactly this). This is the more useful near-term signal and is
     the benchmark's stated intent ("formalization workflows").
- **Caveat / cost:** requires the full Mathlib `v4.28.0` build (heavy) and the comparator+landrun setup
  (landrun is Linux/Landlock — **won't run natively on this Windows dev box**; needs WSL/Linux CI). The
  three whole-`import Mathlib` files are slow to elaborate. Small N (23) means it's a qualitative
  tripwire, not a statistically rich leaderboard.
- **Reusable even without running Lean:** the 23 statements are a curated corpus of "hard, faithful
  formalizations of famous theorems" — usable as few-shot exemplars / a style guide for Theoremata's own
  formalizer, and the domain taxonomy is a ready node-tagging vocabulary.

---

## 6) New vs. already-in-our-design

**Already in our design (confirmatory, validates our approach):**
- Formalize → **Lean compile** as the correctness substrate — matches our proof-DAG core + Lean compile.
- **Axioms gate** — FormalQualBench operationalizes exactly this via `permitted_axioms`. Confirms the
  gate is the right mechanism and gives us the canonical whitelist
  `["propext","Quot.sound","Classical.choice"]` and the free `sorryAx` rejection.
- **Benchmark ingestion** as a first-class concern — matches our eval-harness/grader design.
- Model-agnostic: the benchmark is model-agnostic by construction (it only defines targets), consistent
  with our LiteLLM provider layer.

**New / worth adopting:**
- **`MainTheorem` + `namespace == problem-id` naming as a node ID contract** for Lean formalization
  targets — cleaner than ad-hoc ids; makes grader config trivial.
- **Challenge-vs-solution statement-preservation check** (not just "compiles") — an anti-cheat step our
  grader should implement explicitly.
- **`landrun`/Landlock sandbox for untrusted proof compilation** — a concrete hardening pattern for our
  guardrail modules (run model-generated Lean sandboxed).
- **"Statement elaborates with `sorry`" as a cheap statement-well-formedness gate** distinct from the
  full proof grade — a useful two-stage gate (aggregate `Basic.lean` build = all statements elaborate).
- **Curation policy**: seed formalization targets from Mathlib `proof_wanted` and "famous-but-
  unformalized" results; explicitly exclude competition problems — a target-sourcing strategy for our
  benchmark ingestion.
- **The autoformalization-grading mode** (NL docstring → reproduce formal statement, graded by statement
  equality) — a concrete, near-term-measurable eval track for Theoremata.

**Not present (don't expect from this resource):** no LLM prompts, no runner/aggregation code, no
per-item difficulty scores, no natural-language↔formal paired dataset beyond the docstrings, no Python.
