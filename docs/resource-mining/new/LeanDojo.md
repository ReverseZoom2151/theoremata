# LeanDojo — Full Resource-Mining Pass

Source: `resources/LeanDojo-main/LeanDojo-main/`
Upstream: `lean-dojo/LeanDojo`; README marks this repo as deprecated in favor of `LeanDojo-v2`, but it is still the canonical source for the original Lean tracing and interaction design.
Pass type: full over prose, docs, core Python/Lean implementation, tests, and examples; non-semantic image assets only catalogued.

---

## 1) What it is

LeanDojo is the original infrastructure layer for learning-based theorem proving in Lean 4. It does two jobs:

1. **Static extraction:** clone/build a Lean repo, run Lean-side instrumentation, and export proof states, tactics, theorem statements, import dependencies, and premise provenance.
2. **Interactive execution:** open a theorem in a controlled REPL, apply tactics one at a time, and receive either the next proof state, an error, timeout, `sorry`, or proof completion.

The important conceptual split is: LeanDojo is not just a tactic runner. It builds the training/evaluation substrate that ReProver, LeanCopilot, and most Lean neural-prover work depend on: theorem identifiers, import DAGs, accessible-premise filtering, annotated tactic traces, and a theorem-local environment where tactics can be replayed.

## 2) Core files and architecture

Main files inspected:

- `src/lean_dojo/data_extraction/lean.py` — `LeanGitRepo`, `Theorem`, `LeanFile`, `Pos`, dependency parsing, GitHub/local repo normalization, Lean toolchain/version resolution.
- `src/lean_dojo/data_extraction/trace.py` — clone/build/trace orchestration, cache handling, traced repo loading, `ExtractData.lean` invocation.
- `src/lean_dojo/data_extraction/ExtractData.lean` — Lean-side instrumentation over command elaboration info trees.
- `src/lean_dojo/data_extraction/traced_data.py` — `TracedRepo`, `TracedFile`, `TracedTheorem`, `TracedTactic`, import DAG post-processing, premise annotation.
- `src/lean_dojo/interaction/dojo.py` — `Dojo`, `check_proof`, tactic/command state classes, pexpect REPL process lifecycle.
- `src/lean_dojo/interaction/Lean4Repl.lean` — the Lean command/tactic JSON REPL injected into target files.
- `src/lean_dojo/interaction/parse_goals.py` — parser for pretty-printed Lean goals.
- `docs/source/*.rst`, tests, notebooks — user guide, limitations, examples, and regression coverage.

Static artifacts produced per traced repo include:

- `*.ast.json` — Lean syntax/AST plus post-processed tactic and premise metadata.
- `*.dep_paths` — import dependency paths.
- `*.trace.xml` — serialized traced repo data.
- copied `Lean4Repl.lean` — used for later interaction.

The tracing path is roughly:

```text
LeanGitRepo -> clone/cache -> lake build -> run ExtractData.lean
            -> AST + dep_paths -> TracedRepo import DAG
            -> theorem/tactic/premise objects
```

The interactive path is roughly:

```text
Theorem -> replace proof with `by lean_dojo_repl sorry`
        -> start `lake env lean` via pexpect
        -> JSON request: run tactic/command on state id
        -> JSON response: next state, error, timeout, proof finished, or given up
```

## 3) Reusable ideas and code patterns

**Stable theorem/repo identity.** `LeanGitRepo` and `Theorem` provide a reproducible identity layer: repo URL/path + commit + file path + fully-qualified theorem name. The details are messy but important: URL normalization, tag/commit resolution, `lean-toolchain` reading, dependency parsing from Lake manifests, and a theorem UID that downstream datasets can carry.

**Lean-side extraction via info trees.** `ExtractData.lean` is the central trick. It enables info-tree collection during elaboration, visits tactic info to capture `state_before`, `state_after`, and tactic source ranges, and visits term info to capture premise uses with declaration ranges and module paths. This is much richer than parsing source text after the fact.

**Import-DAG-aware premise accessibility.** `TracedRepo` builds a NetworkX import DAG and annotates identifiers with definition provenance. `TracedTactic.get_annotated_tactic()` wraps used premises in `<a>...</a>` and records `full_name`, defining path, and source span. This exact pattern later reappears in ReProver.

**Interactive proof state protocol.** `Lean4Repl.lean` stores Lean states by ID, runs tactics against saved states, prints goals, validates final proofs by checking no sorry/metavariables and kernel typechecking, and returns structured outcomes. This is the smallest useful abstraction for an LLM theorem-proving tool.

**Proof replacement rather than whole-file synthesis.** `check_proof()` and `Dojo` surgically replace the target theorem proof while keeping the surrounding file/project context. That is the right granularity for proving existing declarations and for validating candidate tactic scripts.

**Goal parser as a compatibility shim.** `parse_goals.py` is small but valuable: it turns pretty-printed goals into declarations plus conclusion, letting non-Lean components reason over state text without linking Lean internals.

## 4) Benchmark and evaluation value

LeanDojo is a source of:

- theorem/state/tactic/premise datasets;
- reproducible traced-repo cache formats;
- interaction tests over mathlib, Batteries, and Aesop;
- examples of timeout/error handling for live Lean tactic execution.

For Theoremata, the most valuable evaluation use is not to copy the deprecated repo wholesale, but to treat its traced objects and REPL semantics as the reference shape for:

- “given theorem, produce next tactic” tasks;
- premise-retrieval training examples;
- tactic replay/regression tests;
- import-DAG accessibility checks.

## 5) Limitations and risks

- The repo is explicitly deprecated in favor of `LeanDojo-v2`.
- It targets older Lean 4-era assumptions; README/docs mention Lean 4 `v4.3.0-rc2+` and a specific Benchmark 4 snapshot.
- It cannot trace the Lean 4 repo itself and cannot process FFI-heavy repos such as LeanCopilot.
- It does not handle all proof styles equally: term proofs and mixed term/tactic proofs are limited compared with pure tactic proofs.
- The theorem extractor is syntactic: it mainly extracts theorem/lemma declarations, not every `Prop`-valued definition.
- The pexpect-based REPL and global cache design are operationally brittle under concurrency, sandboxing, and long-running agent loops.
- Running extraction requires cloning/building arbitrary Lean projects, so isolation, resource limits, and cache hygiene matter.

## 6) Adopt list for Theoremata

P0:

- Adopt the **theorem identity model**: `(repo, commit, file, full_name, position)` should be a first-class key everywhere.
- Adopt a **stateful tactic execution protocol** with explicit state IDs, structured errors, timeout, `sorry`, and proof-finished outcomes.
- Preserve **import-DAG accessibility** for premise retrieval; do not retrieve arbitrary global premises without checking they are available at the theorem location.

P1:

- Use LeanDojo-style **annotated tactics** (`<a>premise</a>` plus provenance) as a common training/interchange format.
- Build Theoremata search traces so they can later be converted into LeanDojo/ReProver-style retrieval and generation datasets.
- Reuse the proof-replacement pattern for checking candidate scripts against existing declarations.

P2:

- Prefer `LeanDojo-v2` or a modernized extraction bridge for new work, but keep this repo as the reference for exact static/interactive semantics.
- Avoid inheriting the pexpect/global-cache implementation directly; wrap interaction in a session-scoped, cancellable service with deterministic cleanup.

