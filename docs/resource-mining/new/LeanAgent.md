# LeanAgent — Full Resource-Mining Pass

Source: `resources/LeanAgent-main/LeanAgent-main/`
Pass type: full over README, database, main agent script, retriever/generator/prover modules, Fisher/EWC utilities, shell scripts, and tests; generated artifacts excluded.

---

## 1) What it is

LeanAgent is a monolithic precursor to LeanDojo-v2’s lifelong theorem-proving loop. It combines:

- GitHub repository discovery/cloning;
- LeanDojo tracing and benchmark generation;
- a dynamic JSON theorem/premise database;
- curriculum ordering by theorem difficulty;
- progressive retriever training;
- Elastic Weight Consolidation / Fisher information experiments;
- distributed proof search over sorry theorems;
- optional source replacement and GitHub PR automation.

The repo is useful historically and architecturally, but the implementation is much more brittle than LeanDojo-v2. It should be mined for patterns and ablations, not imported as a library.

## 2) Core architecture

Main files inspected:

- `README.md` — lifelong learning formal theorem proving workflow, setup, model downloads, EWC/Fisher ablations.
- `leanagent.py` — main orchestration script: repo discovery, tracing, training, evaluation, proving, PR flow.
- `dynamic_database.py` — repository/theorem/premise/tactic models and merged dataset export.
- `generate_benchmark_lean4.py` — LeanDojo benchmark generation glue.
- `retrieval/model.py`, `retrieval/datamodule.py`, `retrieval/fisher_computation_module.py`, `retrieval/compute_fisher.py` — retriever plus EWC/Fisher logic.
- `generator/*`, `prover/*`, `common.py` — ReProver-derived generation/proof-search layer.
- `unittest_dynamic_database.py` — large test coverage for database behavior.
- shell scripts for training, Fisher computation, replacement, and runs.

The main loop is:

```text
discover compatible repos
-> trace repo and add to DynamicDatabase
-> sort repos by difficulty
-> export merged dataset
-> train/evaluate retriever, optionally with EWC
-> prove sorry theorems with DistributedProver
-> update DB, optionally replace source sorry and open PR
```

## 3) Reusable ideas and code patterns

**Dynamic database predecessor.** `dynamic_database.py` contains the same basic object model later refined in LeanDojo-v2: `Repository`, `Theorem`, `Premise`, `PremiseFile`, `Annotation`, `AnnotatedTactic`. It can deduplicate theorem records and export proofs/corpus/traced files/metadata.

**Proof-status transition.** `Repository.change_sorry_to_proven` records a theorem moving from sorry/unproved to proven. This explicit transition is important for lifelong systems: successful proofs are new training data, not just final answers.

**Curriculum by difficulty.** The main script computes difficulty from proof-step counts, treats sorry as infinite difficulty, bins by percentile, and sorts repositories by easy theorem counts. The details are crude, but the concept is valuable.

**Progressive retriever training.** The retriever is trained over expanding merged datasets, with evaluation after each repo/lambda setting. This is the lifelong learning loop Theoremata will eventually need.

**EWC/Fisher experiment hooks.** `retrieval/model.py` adds Fisher-weighted penalties against previous parameters; `fisher_computation_module.py` accumulates squared gradients and saves Fisher tensors. The README notes mixed/suboptimal outcomes, but the implementation is a reusable ablation scaffold.

**Automated sorry proving and source patching.** `prove_sorry_theorems()` batches sorry theorems, skips dependencies, calls distributed proof search, and updates the database with successful tactic sequences. `replace_sorry_with_proof()` attempts to patch source text. This is the correct high-level feedback loop, though the concrete patcher is unsafe.

## 4) Benchmark and evaluation value

LeanAgent offers several evaluation ideas:

- lifelong sequence evaluation: performance after each added repository;
- average retrieval R@1/R@10/MRR over all merged datasets;
- EWC vs non-EWC retriever ablation;
- sorry-theorem proof rate after each training stage;
- database regression tests for deduplication/export/status changes.

The database unit test file is especially useful as a source of expected behaviors for a future typed Theoremata DB.

## 5) Gaps and risks

This repo is very prototype-heavy:

- `leanagent.py` contains duplicated imports, global constants, hardcoded paths, placeholders, and environment assumptions.
- GitHub actions are in scope: cloning, branching, committing, pushing, and PR creation. These must not run without explicit user authorization.
- Credentials and repo discovery are tightly coupled to GitHub API behavior.
- `dynamic_database.py` has at least one clear bug: `safe_remove_dir_path` uses `time.sleep` without importing `time`.
- Source replacement finds/replaces a `sorry` inside a span in a brittle text way; it is not AST-verified.
- Successful generated proof tactics can be added back with missing state/provenance information, weakening later training data quality.
- The skip list and repository-compatibility logic are ad hoc.
- Distributed proof-search and retriever code assume specific GPU/process environments.
- EWC is experimental and, per README discussion, not necessarily beneficial.
- The monolithic script mixes read-only analysis, training, database mutation, filesystem mutation, and external PR mutation.

## 6) Adopt list for Theoremata

P0:

- Adopt the **lifelong transition model**: unproved/sorry theorem -> proof search -> successful tactic path -> database update -> future training datum.
- Use LeanAgent’s database tests as inspiration for Theoremata DB invariants: dedup, latest-record preference, merged export, and status transitions.
- Keep external mutations such as PR creation behind explicit approval and separate commands.

P1:

- Add curriculum/difficulty scheduling, but make it a typed policy module rather than global script logic.
- Add progressive retriever evaluation over repository sequences.
- Preserve EWC/Fisher as an optional ablation, not a default path.

P2:

- Do not port `leanagent.py` wholesale. Rebuild the loop from smaller services: repository ingester, database, trainer, prover, patcher, PR publisher.
- Replace text-based `sorry` patching with AST/range-verified edits from Lean metadata.

