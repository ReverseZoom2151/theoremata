# LeanDojo-v2 — Full Resource-Mining Pass

Source: `resources/LeanDojo-v2-main/LeanDojo-v2-main/`
Pass type: full over README, examples, agent/prover/trainer/database/data-extraction/external-API code; generated/build artifacts excluded.

---

## 1) What it is

LeanDojo-v2 is a newer end-to-end framework for Lean 4 neural theorem-prover agents. It absorbs the original LeanDojo tracing idea and adds:

- lifelong repository ingestion;
- a dynamic theorem/premise/tactic database;
- retrieval-augmented and plain language-model provers;
- SFT, GRPO, retrieval, and proof-progress trainers;
- Pantograph-based proof search;
- external generator/encoder APIs for large model serving.

It is less a single polished library and more a reference stack for the full loop:

```text
clone/trace Lean repo
-> add theorems/premises/proofs to DynamicDatabase
-> export merged datasets
-> train SFT / GRPO / retriever / progress scorer
-> run prover over `sorry` theorems
-> feed successful tactics back into the database
```

## 2) Core architecture

Main files inspected:

- `agent/base_agent.py` — tracing, database ownership, theorem proving orchestration.
- `agent/hf_agent.py`, `agent/external_agent.py`, `agent/lean_agent.py` — concrete agent variants.
- `prover/base_prover.py`, `prover/hf_prover.py`, `prover/external_prover.py`, `prover/retrieval_prover.py` — Pantograph search plus tactic proposal backends.
- `trainer/sft_trainer.py`, `trainer/grpo_trainer.py`, `trainer/retrieval_trainer.py`, `trainer/progress_trainer.py` — training/export paths.
- `database/dynamic_database.py`, `database/models/*.py` — normalized repository/theorem/premise/tactic data model and merged dataset generation.
- `lean_dojo/data_extraction/dataset.py` — benchmark/dataset export from traced LeanDojo repositories.
- `external_api/*` — FastAPI model server and model runner abstractions.
- `utils/constants.py`, `utils/difficulty.py` — global paths/tokens and curriculum scoring.
- `examples/*` — intended user workflows.

The agent layer owns a `DynamicDatabase`, traces a repository, adds it to the database, exports merged data, trains a model, and calls a prover on unproved/sorry theorems. The provers use Pantograph servers rather than the original pexpect REPL.

## 3) Reusable ideas and code patterns

**Dynamic lifelong database.** `DynamicDatabase` is the main reusable artifact. It stores repositories, theorem records, premises, traced files, annotations, and tactic traces; deduplicates theorems by `(file_path, full_name, start, end)`; prefers later processed versions; exports `proofs`, `corpus.jsonl`, `traced_files.jsonl`, and metadata; and supports random plus novel-premise splits.

**Repository-level curriculum.** `utils/difficulty.py` maps theorem proofs to a rough difficulty score, uses `inf` for sorry/unproved items, and sorts repositories by easy theorem counts. This is crude, but the pattern is right: repository ingestion should be scheduled, not purely FIFO.

**Backend-polymorphic agents.** The three agent classes split useful modes:

- `HFAgent` — local Hugging Face model plus SFT/GRPO trainer.
- `ExternalAgent` — API-served generator such as DeepSeek/Novita-style services.
- `LeanAgent` — retrieval-augmented prover and retriever training.

This is a good interface idea: Theoremata should separate theorem search/control from the concrete model runtime.

**Trainer/export separation.** Trainers do not read the whole database object arbitrarily; they call export routines to produce merged datasets. This is the right boundary for reproducibility: training should consume explicit artifacts, not live mutable database internals.

**Progress prediction as a first-class reward.** `ProgressTrainer` trains a regression-style scorer from `goal`, `prefix`, `tactic`, and `steps_remaining`. The external API can use a `LeanProgressScorer` and return negative predicted remaining steps as reward. This is the cleanest integration point between LeanProgress and a prover loop.

**Pantograph proof search wrapper.** `BaseProver.search()` creates a goal, tries tactic candidates, tracks a directed graph, and returns a search result plus used tactics on success. Even if the implementation is prototype-level, the return shape is valuable: proof success should preserve the search path and tactic provenance.

## 4) Benchmark and evaluation value

LeanDojo-v2 gives Theoremata a ready set of evaluation axes:

- repo ingestion success/failure;
- theorem deduplication stability across commits;
- retrieval metrics over exported corpora: R@1, R@10, MRR;
- SFT/GRPO dataset generation correctness;
- pass/fail proof search over sorry theorems;
- proof-progress scoring as an ablation in search ranking.

The “novel premises” split is especially worth adopting. Random splits over theorem-state pairs are too easy; a theorem prover benchmark should ask whether the system can generalize to premises that are not simply repeated from training.

## 5) Gaps and risks

Several implementation details look research-prototype rather than production-ready:

- `utils/constants.py` requires `GITHUB_ACCESS_TOKEN` at import time and hardcodes `RAID_DIR` as `os.getcwd()/raid`; this makes offline/read-only tooling fragile.
- Some path handling is brittle; for example a `~/.elan/...` path is checked without expanding `~`.
- `GRPOTrainer` has placeholder reward behavior in examples, so the algorithmic surface is not complete by itself.
- `HFProver` samples and then randomly chooses from generated tactics; invalid/empty generation cases need explicit handling.
- `BaseProver.search()` uses an ad-hoc stack/graph search with several prototype edges: stack indices and node IDs are easy to confuse after pops, failure returns can omit tactic paths, and the search policy is not obviously deterministic.
- Database storage is JSON-oriented and likely not concurrency-safe for multi-agent ingestion.
- External API and model runners assume large model environments, GPU availability, and/or third-party service keys.
- The code mixes LeanDojo-derived extraction, Pantograph interaction, and agent training concerns in one repo; integrating wholesale would drag in too much surface area.

## 6) Adopt list for Theoremata

P0:

- Adopt the **dynamic database schema idea**: repositories, theorem instances, premises, traced files, annotations, tactics, proof status, and processed date should be explicit records.
- Add **merged-dataset export** as a deterministic artifact, including theorem deduplication and corpus export.
- Preserve **novel-premise splits** as a default evaluation mode.

P1:

- Implement an agent interface similar to `BaseAgent`, but keep model runtime, prover, trainer, and database as separate services/modules.
- Add a **progress scorer hook** to tactic ranking. The exact LeanDojo-v2/LeanProgress models can be swapped, but the search API should accept a value estimate.
- Use difficulty/curriculum signals to schedule repository ingestion and proof attempts.

P2:

- Do not port global constants, token requirements, or hardcoded `raid` paths.
- Treat the Pantograph prover code as a reference for search-result shapes, not as a final search implementation.
- Convert database persistence to a typed, transactional store before doing concurrent lifelong ingestion.

