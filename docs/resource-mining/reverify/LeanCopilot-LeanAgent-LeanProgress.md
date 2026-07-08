# Re-verification: LeanCopilot / LeanAgent / LeanProgress

Independent re-scan of the three repos against Codex's prior reports in
`docs/resource-mining/new/{LeanCopilot,LeanAgent,LeanProgress}.md`.
For each repo: what the prior report **captured**, what it **MISSED** (with file
cites), and **new adoptables** for Theoremata given we already built multi-system
gates + generators + hammers + a progress-prior-in-MCTS + a difficulty curriculum.

Repo paths carry a doubled root, e.g.
`resources/LeanCopilot-main/LeanCopilot-main/...`. Cites below use the inner path.

---

## LeanCopilot

### Captured (report is correct)
- `TextToText` / `TextToVec` typeclasses, model registry + Lean `register_option`
  indirection, external `/generate` + `/encode` server split, CTranslate2 native
  path, LlmAesop-as-tactic-source pattern, `curl`-per-request weakness, global
  process state, heuristic self-reference filter, CPU-forced premise retrieval.
  All accurate.

### MISSED (material gaps)
1. **`Frontend.lean` verified-suggestion loop is entirely absent from the report.**
   `LeanCopilot/Frontend.lean` (`hint`, lines 58–79) is the real internal we did
   NOT get told about: it parses each candidate tactic (`runParserCategory`),
   runs it under `observing? (withMessageLog (withoutInfoTrees (evalTactic …)))`,
   harvests any `Try this:` rewrite (e.g. the `aesop?` expansion) from the message
   log, sorts survivors by **fewest remaining goals**
   (`Nondet` / `MLList.takeUpToFirst (·.isEmpty)`), commits the first goal-closing
   candidate via `setMCtx r.2.term.meta.meta.mctx`, and `admitGoal`s otherwise.
   This is a compact **verified best-first filter over model candidates** — closer
   to our "gate" concept than to a mere UI hint. Report framed suggestions as
   "optionally checked" text; in fact `LeanCopilot.suggest_tactics.check`
   **defaults to `true`** (`Options.lean:31–39`) so tactics are executed, verified,
   ranked by remaining-goal count, and the winning mctx is spliced in.
2. **Exact model-server wire schema + OpenAPI file uncited.** `Models/External.lean`
   defines the precise JSON contract: `GeneratorRequest {name,input,«prefix»}` →
   `GeneratorResponse {outputs: [Generation {output, score}]}`; encoder
   `{name,input}` → `{outputs: [Float]}` (`External.lean:22–49`). The Python side
   mirrors it exactly (`python/server.py:66–104`) and there is a standalone
   `external_model_api.yaml` OpenAPI spec the report never mentions. Field-level
   fidelity matters if we want drop-in compatibility with their released servers.
3. **`search_proof` / registration internals.** `LlmAesop.lean:37–40`:
   `#configure_llm_aesop` registers the generator at Aesop priority
   `@[aesop 100%]` and `search_proof` expands to `aesop? (add 100% tacGen)`.
   The `aesop?` (with `?`) is what produces the `Try this:` script the frontend
   harvests. The exact priority + `aesop?` coupling was not documented.
4. **Concrete model zoo + brittle prompt/parse layer.** `python/server.py:10–63`
   ships a named registry: `gpt4`, InternLM-math-plus-1.8b, **Kimina-Prover-Distill-7B**,
   Pythia `llmstep-mathlib4`, `t5-small`, and LeanDojo byT5 tacgen/retriever.
   `external_models/external_parser.py` holds per-model prompt templates
   (`pre_process_input`), fragile `split`-based output extraction
   (`post_process_output`), and `choices_dedup` (dedup by tactic string keeping
   max score). Scores come from `np.exp(cumulative_logprob)`
   (`vllm_runner.py:57–63`) — sequence probability as rank signal.
5. **Native/FFI generation + retrieval signatures.** `Models/FFI.lean:25–46`:
   `generate(name, inputTokens, targetPrefixTokens, numReturnSequences, beamSize,
   minLength, maxLength, lengthPenalty, patience, temperature)` (CT2 beam search)
   and `retrieve(queryEmb, k) -> Array (name × path × code × score)`. Retrieval
   feature math is spelled out in `scripts/validate_retrieval.py`: byT5 encoder,
   **attention-masked mean pooling**, then `premise_embeddings @ state_embedding`
   + `topk` — the exact premise-selection recipe. `Tactics.lean:78–86`
   `annotatePremise` enriches hits with type + docstring, or an "needs to be
   imported from …" fallback with code.

### New adoptables
- **Port the `hint` verified-filter** as our editor/gate primitive: generate →
  parse → run under `observing?` with info-tree suppression → rank by
  remaining-goal count → commit best mctx. It is a self-contained, model-agnostic
  verified reranker we can reuse across Lean tactic sources.
- **Adopt the exact `/generate` `/encode` JSON schema** (incl. `external_model_api.yaml`)
  so any Theoremata tactic server is wire-compatible with LeanCopilot clients and
  vice-versa. Cheaper than inventing our own.
- **Reuse `choices_dedup` + `np.exp(cumulative_logprob)` scoring** for candidate
  ranking in our generator layer; keep the per-model prompt templates as a
  cautionary example (they are the fragile part — replace `split`-parsing with
  structured decoding).
- If we ever expose in-Lean premise selection, replicate the masked-mean-pool +
  matmul + topk recipe but add import-accessibility filtering (the known gap).

---

## LeanAgent

### Captured (report is correct)
- Dynamic-DB object model (`Repository/Theorem/Premise/...`), `change_sorry_to_proven`
  transition, difficulty = `exp(#steps)` with sorry→∞, progressive retriever
  training, EWC/Fisher scaffold, unsafe text-based `sorry` patching, provenance
  loss on re-added proofs, monolithic-script risk. Accurate.

### MISSED (specifics beyond "difficulty = exp(#steps)")
1. **The curriculum is a percentile-binned, easy-first repo ordering, not just a
   per-theorem score.** `leanagent.py:842–909`: `calculate_difficulty` returns
   `exp(len(traced_tactics))`, `inf` if any step contains `sorry`, `None` if zero
   steps. `sort_repositories_by_difficulty` computes **33rd/67th percentile
   thresholds** over all finite difficulties (`np.percentile(all,[33,67])`), bins
   each theorem into Easy/Medium/Hard/`Hard (No proof)`, then **round-robin
   distributes the `None`/`To_Distribute` theorems across the three real buckets**
   (`:895–903`), and finally **sorts repositories by their count of Easy theorems,
   descending** (`:906–907`). During training it can subsample the 3 hardest
   theorems per category for logging (`:1020–1023`). Our `exp(#steps)` captured the
   scalar but not the percentile binning, the None-distribution trick, or the
   easy-count repo ordering — those are the actual curriculum policy.
2. **Lifelong de-dup / resumability infrastructure.** `prove_sorry_theorems`
   (`:665–739`) persists an `all_encountered_theorems` set to pickle
   (`ENCOUNTERED_THEOREMS_FILE`, `save_progress`/`load_encountered_theorems`
   `:639–663`), checkpoints every 30 min, dedups by a
   `theorem_identifier = (full_name, file_path, start, end)` tuple, sorts repos by
   `date_processed` desc to prefer the newest version of a repo, and **skips
   dependency sorries** (`theorem.url != repo_url`, `:695–696`). This
   cross-run resumable prove-loop is more than "a loop" and is directly relevant
   to a long-running Theoremata agent.
3. **Batched distributed proving contract.** `process_theorem_batch` (`:604–637`)
   calls `prover.search_unordered(LeanGitRepo, [LeanDojoTheorem], [Pos])` in
   batches of 12, maps `SearchResult → Theorem`, and on `Status.PROVED` writes
   `AnnotatedTactic(tactic, (tactic,[]), state_before="", state_after="")` — i.e.
   the **provenance loss the report flagged is concrete here**: proved tactics are
   stored with empty states, degrading them as future retriever training data.
4. **EWC math is fully specified and cheaply portable.** `retrieval/model.py:96–121`:
   `ewc_loss = λ · Σ_name fisher[name] · (θ − θ_prev)²`, gated off when `λ==0` or no
   Fisher (default `λ=0`, `:81`); added to training loss at `:242`.
   `set_previous_params()` is snapshotted at `on_fit_start` (`:230`).
   `fisher_computation_module.py` computes Fisher as **accumulated squared grads
   over a frozen pass** (`SGD lr=0`, `:100`), `all_reduce`-summed across GPUs and
   normalized by `dataset_size × world_size` (`:71–98`). The driver runs
   `lambdas=[0.1]` with Fisher vs `[0.0]` without (`leanagent.py:961–967`). This is
   a complete, self-contained EWC recipe we could wire into our own retriever with
   ~40 lines.
5. **Retriever architecture confirmed = ReProver byT5.** `retrieval/model.py:168–216`:
   masked-mean-pool, `F.normalize` unit vectors, cosine-as-inner-product,
   `F.mse_loss(similarity, label)` contrastive objective with in-batch pos+neg
   premises; corpus (re)embedding is lazily staled (`load_corpus:143–161`). Report
   said "retriever trained" but not that it is exactly the LeanCopilot/ReProver
   byT5 encoder — so the LeanCopilot native retriever and this share one model.
6. **Report slightly overstated PR automation.** No PyGithub/PR-creation code is
   present in this snapshot — only a `pr_url` metadata field
   (`unittest_dynamic_database.py:126–134`) and a `replace_files.sh`. The
   "cloning/branching/committing/pushing/PR creation" the report lists as in-scope
   is aspirational here; the real external mutation is `replace_sorry_with_proof`
   (`leanagent.py:812–840`), a bottom-up line-span `str.replace('sorry', …, 1)`.

### New adoptables
- Replace our flat `exp(#steps)` schedule with LeanAgent's **percentile-bin +
  easy-repo-first + None-distribution** curriculum policy (as a typed module).
- Adopt the **resumable prove-loop primitives**: `theorem_identifier` dedup key,
  pickled encountered-set, periodic checkpoint, newest-version-first repo sort,
  dependency-sorry skip. These make lifelong runs restart-safe.
- Fix the **provenance-loss bug at the source**: capture `state_before/state_after`
  from the prover's search tree when writing proved tactics back, so the DB stays
  usable as retriever/generator training data.
- Keep the **EWC recipe** as an optional plug-in for our retriever/value model:
  frozen squared-grad Fisher + `λ·Σ fisher·(θ−θ_prev)²`, default `λ=0`.

---

## LeanProgress

### Captured (report is correct)
- Progress-from-traces idea, parent-graph reconstruction to `no goals`, sibling
  sampling, dual targets (`steps_to_no_goals` / `relative_progress`), multiple
  prompt formats, distribution-bucketed eval, "progress as negative-remaining-steps
  reward", placeholder-path and leakage risks. Accurate.

### MISSED (exact model / features / training we approximated with a heuristic)
1. **The predictor is a generative SFT LLM, not a scalar head.** `models/steps_*.py`
   fine-tune **`deepseek-ai/deepseek-coder-1.3b-base`** via **XTuner/MMEngine**
   `SupervisedFinetune` (`steps_adj_relative.py:24, 73–94`), emitting the label as
   *text* the model completes. Our MCTS progress prior is a heuristic; their real
   approach is a 1.3B code LLM predicting the number in-band. Hyperparameters are
   concrete: `max_length=4096`, `pack_to_max_length`, AdamW `lr=1e-6`, cosine
   schedule w/ 3% warmup, `max_epochs=5`, grad-clip 1.0, fp32 for NaN stability.
   (`train_qwen_epochs.py` is the Qwen alternative; `train_deepseek_simple.py`
   the plain HF-Trainer path.)
2. **Exact serialization format** (needed to reuse their model/data):
   prompt = `"---\nSTATE_AFTER: <goal>\n\n---\nSTEPS_TO_NO_GOALS:"`, assistant
   completes the integer (or the float for the relative variant)
   (`steps_adj_relative.py:61–65`); wrapped in OpenAI chat via
   `openai_map_fn` + `qwen_chat` template. Eval decodes with vLLM
   (`temp 0.7, max_tokens 50, stop=<|im_end|>`) and **extracts the first
   number via regex** `(\d+(?:\.\d+)?)` (`utils/eval.py:43–47, 70–73`) — the same
   weak extraction the report flagged, now cited.
3. **Exact labeling algorithm** (`collect_steps_data.py:49–140`): it walks the
   BFS `queue` (NOT `searched_states`) building `graph[state_after]=state_before`,
   stops at `state_after=='no goals'`, reconstructs the chain by climbing parents
   from `'no goals'`, then labels each state `steps_to_no_goals = i` and
   **`relative_progress = 1 - i/total_path_length`** (`:108`). Sibling branches from
   the same parent get the **parent's distance + 1** (`steps_to_no_goals = i+1`,
   `:130–138`), capped at `sample_size=5` random siblings. Supports `old`/`new`
   trace schemas (`:67–80`). This exact edge/queue schema is what our proof-search
   traces must emit to reuse their pipeline.
4. **No shipped model or dataset; XTuner is a git submodule.** There are **no HF
   model/dataset links** anywhere in the repo — the trained checkpoint and data are
   not here; `.gitmodules` pulls `InternLM/xtuner` as a submodule and README points
   the "updated version" to **LeanDojo-v2** (paper: arXiv **2502.17925**, TMLR).
   So "use their real data/model" is NOT possible from this repo alone: we would
   either reproduce the SFT (recipe above) on our own traces, or pull the updated
   `ProgressTrainer` from LeanDojo-v2.

### New adoptables
- If we want to graduate from the heuristic prior to a *learned* one, we now have
  the **full reproducible recipe**: deepseek-coder-1.3b SFT, the
  `STATE_AFTER/STEPS_TO_NO_GOALS` text format, `relative_progress = 1 - i/L`
  labels, sibling(+1) augmentation, per-bucket (1-5/6-10/11-15/16-20/21+) MAE eval.
  Our trace schema should record `(state_before, tactic/proof, state_after, status)`
  per edge so the LeanProgress extractor runs unmodified.
- Prefer pulling the **updated LeanDojo-v2 `ProgressTrainer`** over these raw
  scripts (README's own recommendation); treat this repo as the paper-repro spec.
- Keep our heuristic as the cold-start prior, then swap in the SFT model once we
  have enough successful traces — the integration point (negative predicted
  remaining steps as MCTS value/reward) is unchanged.
</content>
