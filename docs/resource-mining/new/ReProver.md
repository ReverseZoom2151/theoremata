# ReProver — Full Resource-Mining Pass

Source: `resources/ReProver-main/ReProver-main/`
Pass type: full over README, common data layer, retrieval/generation/prover code, configs, scripts, and examples; image assets catalogued only.

---

## 1) What it is

ReProver is the retrieval-augmented Lean theorem prover from the LeanDojo paper. It trains:

- a premise retriever;
- a tactic generator;
- a retrieval-augmented tactic generator;
- a best-first proof-search loop over LeanDojo interaction.

It is the clearest implementation of the classic LeanDojo neural-prover pipeline:

```text
LeanDojo traced data
-> theorem context + accessible premises
-> train retriever
-> retrieve premises for a proof state
-> prepend premises to state
-> generate tactics
-> best-first search in Lean
-> Pass@1 evaluation
```

## 2) Core architecture

Main files inspected:

- `common.py` — shared `Context`, `Premise`, `Corpus`, accessible-premise logic, augmented-state formatting.
- `retrieval/model.py`, `retrieval/datamodule.py`, `retrieval/index.py`, `retrieval/evaluate.py` — premise retriever, training examples, corpus indexing, R@K/MRR eval.
- `retrieval/bm25/*` — BM25 baseline.
- `generation/model.py`, `generation/datamodule.py`, `generation/preprocess.py` — tactic generation and retrieval-augmented generation data.
- `prover/tactic_generator.py` — generator interfaces: fixed, GPT-4, Hugging Face, retrieval-augmented, vLLM.
- `prover/proof_search.py`, `prover/search_tree.py`, `prover/evaluate.py` — best-first search, node/status model, distributed evaluation.
- `scripts/*.py`, `configs/*.yaml` — data download, tracing, conversion, training/eval configs.

The shared `Corpus` is the backbone. It loads a `corpus.jsonl`, builds file/import relationships, and defines which premises are accessible from a theorem context: imported-file premises plus earlier same-file premises.

## 3) Reusable ideas and code patterns

**Accessible-premise retrieval.** `Corpus.get_nearest_premises` ranks candidate premises by embedding similarity while filtering by Lean visibility. This is essential: a theorem prover must not use future same-file declarations or premises from modules that are not imported.

**Premise serialization with provenance.** `Premise.serialize()` marks the premise name with `<a>...</a>` and carries `(path, full_name, start, end, code)`. This is a compact bridge between source-level Lean objects and model input text.

**Byte-budgeted augmentation.** `format_augmented_state()` prepends retrieved premises to the proof state under a byte-length budget. This is a practical detail worth copying: retrieval should degrade by dropping lower-ranked premises, not by overflowing the model context unpredictably.

**Retriever training with in-batch positives/negatives.** `PremiseRetriever` uses a ByT5 encoder, masked-average pooling, L2-normalized embeddings, and an MSE-style objective over label matrices. The datamodule creates positive premise examples and mines negatives from accessible dependency files and same-file context.

**Generator/retriever separation.** `RetrievalAugmentedGenerator` retrieves premises and delegates tactic generation to a Hugging Face generator. This is the right abstraction for swapping retrieval, reranking, and model backends independently.

**Search-tree telemetry.** `BestFirstSearchProver` records status (`PROVED`, `FAILED`, `OPEN`), cumulative log probabilities, actor/environment time, node counts, and the proof path. This is exactly the data Theoremata needs for search diagnostics and progress-model training.

**Full-stack eval scripts.** The repo has separate retrieval eval (R@1/R@10/MRR) and theorem proving eval (Pass@1 over theorem JSON with LeanDojo cache checks). Keeping these metrics separate is important: good retrieval does not automatically imply successful proving.

## 4) Benchmark and evaluation value

ReProver is directly useful as a benchmark reference:

- retrieval metrics over LeanDojo Benchmark 4;
- BM25 baseline for retrieval;
- tactic-generation top-k accuracy;
- proof-search Pass@1;
- distributed prover timing/node-count telemetry.

The theorem data layout (`theorem JSON + corpus.jsonl + traced files`) is a good interchange target for Theoremata. If Theoremata can export into ReProver-like artifacts, existing retrievers/generators/evaluators become easier to reuse or compare against.

## 5) Gaps and risks

- The code is tied to the original LeanDojo stack and Benchmark 4 assumptions; modern Lean/Pantograph integration may require adaptation.
- The pipeline has heavy dependencies: Hugging Face, PyTorch Lightning, DeepSpeed, Ray, vLLM, and large checkpoints.
- Some search/eval semantics need care: timeouts/open nodes are distinct from failures, and Pass@1 denominators must be audited when comparing systems.
- Model input sanitation is backend-specific. GPT/HF/vLLM tactic parsing can silently change tactic strings.
- Pickled indexed corpora and distributed workers make reproducibility and artifact portability harder.
- Retrieval over all premises followed by accessibility filtering can be inefficient at Mathlib scale if not indexed carefully.
- The proof search is best-first over generated tactics, not a complete proof procedure; repeated bad tactic suggestions can dominate.

## 6) Adopt list for Theoremata

P0:

- Adopt the **accessible-premise filter**: imported modules plus earlier same-file declarations.
- Export theorem/proof datasets with ReProver-compatible fields: `Context`, `Premise`, annotated tactics, `corpus.jsonl`, and traced files.
- Track proof-search status and telemetry with enough detail to reconstruct proof paths and train value/progress models.

P1:

- Use retrieval metrics and proof Pass@1 as separate CI/eval tracks.
- Add byte-budgeted premise augmentation and premise-dropout training as standard model-input utilities.
- Keep tactic generator interfaces backend-polymorphic: local HF, external API, vLLM, fixed/debug generator.

P2:

- Do not copy the distributed Ray stack unless needed. Start with a single-process deterministic prover and add distribution after the artifacts and statuses are stable.
- Prefer LeanDojo-v2/Pantograph for new interaction, but preserve ReProver’s data contracts and evaluation vocabulary.

