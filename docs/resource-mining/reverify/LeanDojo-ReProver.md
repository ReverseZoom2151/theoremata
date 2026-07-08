# Re-Verification Gap Report — LeanDojo / LeanDojo-v2 / LeanDojoChatGPT / ReProver

Purpose: independently re-scan four repos whose original resource-mining reports (`docs/resource-mining/new/`) were written by a different tool (Codex), and catalogue what those reports **missed** plus **new adoptables** for Theoremata's *current* stack (multi-formal-system live verification for Lean/Rocq/Isabelle, per-system premise retrieval feeding a BM25+reranker cascade, warm-REPL/exec bridge, LiteLLM provider, QED retry + plan-history, MCTS + progress prior, three-valued taint, benchmark harness, STaR/GRPO).

Method: full independent re-scan (code, prose, configs, data; build artifacts and binaries skipped) done by three parallel scanners, cross-checked against the Codex reports. Line/file citations are to the nested source dirs under `resources/<repo>/<repo>/`.

Verdict up front: **all four Codex reports are architecturally accurate but under-specified** — they captured the *shapes* and correctly identified the P0 adoptables, but omitted implementation-level details (soundness gates, exact hyperparameters, negative-mining recipes, reward wiring, denominators, real bugs) that are the actually-portable substance now that Theoremata can build most of these ideas directly.

---

## 1) LeanDojo (`resources/LeanDojo-main`) ↔ `new/LeanDojo.md`

### What the Codex report captured (solid)
Static extraction via `ExtractData.lean` info-trees; interactive `Dojo` pexpect REPL with state IDs and structured outcomes; `Lean4Repl.lean` JSON protocol; the theorem-identity model `(repo, commit, file, full_name, pos)`; import-DAG premise accessibility (NetworkX) with `<a>premise</a>` provenance annotation; `parse_goals.py`; the proof-replacement pattern; traced artifacts; deprecation and pexpect-brittleness caveats. The P0/P1/P2 adopt list is correct as far as it goes.

### What it MISSED
**Soundness / proof-validation (highest value).** The report said "validates final proofs by checking no sorry/metavariables and kernel typechecking" in one clause but did not surface the actual gate, which is the single most portable artifact here:
- `interaction/Lean4Repl.lean:170-215` — `validateProof` restores the initial goal, checks the assigned `Expr` is def-eq to the target under `withTransparency .all` (187), rejects `hasSorry` (195) and `hasExprMVar` (198), then **kernel-typechecks via `addDecl`** on a synthesized `thmDecl` (203-210). This is the "don't trust the tactic that said `done` — kernel-verify a reconstructed standalone declaration" gate. Theoremata's three-valued taint layer should consume exactly this (a tactic that closes goals but leaves a metavariable or `sorry` must taint as *unverified*, not *verified*).
- `interaction/Lean4Repl.lean:160-167, 111-146` — `abstractAllLambdaFVars` + free-variable/level-param collection needed to close a proof term whose context still has fvars; non-obvious machinery to reconstruct a standalone decl from a mid-proof state.
- `interaction/Lean4Repl.lean:100-101` — throws `[fatal] not_a_theorem` when the target is not a `Prop`; cheap pre-flight guard.
- `interaction/Lean4Repl.lean:218-244, 310-331` — two-channel design: tactic state **and** filtered `MessageSeverity.error` messages are both returned (errors surface even when a tactic "succeeds" syntactically). Informs the structured-outcome model for every system, not just Lean.

**Cache / concurrency (directly reusable for the warm-REPL/traced-artifact cache).**
- `data_extraction/cache.py:27-42` — `Cache` is a frozen dataclass guarding all reads/writes with a **cross-process `filelock.FileLock`** on `CACHE_DIR.with_suffix(".lock")`. Reusable pattern. **Caveat worth NOT copying:** `get()` does `dirpath.exists()` then `assert cache_path.exists()` — a crash mid-`extractall` leaves `dirpath` present but incomplete and the lock is released between download and use (half-extraction race). Also `cache.py:60-72` downloads via `wget` + `tarfile.extractall` with **no checksum and unsanitized tar members** (pre-3.12 path-traversal). If Theoremata mirrors an S3-download-and-extract cache, add member sanitization + a hash gate.
- `utils.py:118-129` — `compute_md5` (64 MiB streaming) binds an XML trace to its exact source file (`traced_data.py:900,934`); cheap "has this file changed" guard.
- `data_extraction/lean.py:471-480` — `RepoInfoCache` + module-level `@cache` on network calls ("repo metadata is immutable during a run"). **Latent bug at 568-573:** stores with key `(url, commit)` but reads with key `(url, self.commit)` (the resolved hash), so the tag→commit cache never hits.

**Dojo lifecycle / crash taxonomy (feeds taint + warm-REPL bridge).**
- `interaction/dojo.py:87-100, 313` — `kill_descendants` recursively `psutil`-walks children before killing in `__exit__`; prevents orphaned `lake`/`lean` when a parent dies. Theoremata's multi-system bridge needs this or it leaks solvers.
- `interaction/dojo.py:477-485, 74-76` — `_check_alive` maps **exit code 137 → OOM** (`is_out_of_memory`) distinct from other crashes: a concrete {alive, OOM, crashed} triple.
- `interaction/dojo.py:176-178, 263-265` — resource limits are passed to Lean itself (`--threads`, `--memory`), not cgroups; `memory_limit` is GB→MB from `TACTIC_MEMORY_LIMIT`.
- `interaction/dojo.py:169-172, 365-368` — deletes `lakefile.olean` / `.lake/lakefile.olean` before every spawn (stale compiled lakefiles otherwise break the modified-file build). Relevant when Theoremata edits-and-rebuilds lakefiles.
- `interaction/dojo.py:266-268` — pexpect spawn tuned `maxread=1, echo=False` with a `REPL>` sentinel-scan loop (`487-518`) separating side-channel `message` text from the JSON response line — the specific tuning behind "pexpect brittleness."
- `interaction/dojo.py:288, 285, 274-277` — enumerated init-failure branches (prelude unsupported, `no goals` at init, unexpected EOF, init timeout): a ready "can't even start" taxonomy.

**ExtractData premise subtleties (mirror these in per-system extractors).**
- `ExtractData.lean:352` — a premise use-site is excluded when it coincides with the definition site (`defPos != posBefore ∧ defEndPos != posAfter`) → self-premise filter.
- `ExtractData.lean:301` — tactic traces skipped when `stateBefore == "no goals" || stateBefore == stateAfter` (no-op drop) → explains extracted-tactic < textual-tactic counts.
- `ExtractData.lean:71-125` — a **custom `ppGoal`** (not `Meta.ppGoal`) is the exact producer contract that `parse_goals.py` consumes (newline-separated hyps, grouped binders, `case <name>` headers). Producer/consumer coupling to preserve if Theoremata parses Lean goals.
- `ExtractData.lean:445-479, 509-527` — self-parallelizing tracer: one `IO.Process.run` subprocess per file, `IO.wait`, per-file failure tolerated with a WARNING. An alternative to Ray-side fan-out.
- `ExtractData.lean:139-267` (mirrored in `utils.py:213-291`) — the fragile `.lake/packages/lean4/{src,lib}` ↔ `build/ir` path mapping is duplicated Python/Lean; version drift breaks both.

**AST / traced_data specifics.**
- `data_extraction/ast.py:30-40, 1547-1559` — name-driven syntax-kind → class dispatch (`camel_case`+`"Node"`), falling back to `OtherNode` which still carries `state_before/after/tactic` so unknown tactic kinds keep state annotation (no hard-coded grammar).
- `data_extraction/traced_data.py:429-442` — `_fix_indentation` re-indents multi-line tactics to be runnable standalone (directly relevant to feeding retrieved tactics back into a REPL).
- `traced_data.py:371-384` — `get_traced_tactics` dedups on `(state_before, tactic, state_after)` (nested info-trees yield dups).
- `traced_data.py:741-782` — `get_traced_theorem` prefers non-private over private on `full_name` collision.
- `traced_data.py:156-159, 275-278, 482-485, 1033-1035` — every traced object nulls back-references in `__getstate__` and `TracedRepo.__setstate__` rewires on load: the required pattern to pickle a cyclic traced-graph across workers.
- `traced_data.py:949-979` — accessibility = NetworkX `G.successors` per file + `get_premise_definitions`; the concrete "which premises are import-accessible to file X" query that feeds BM25.

**Config / toolchain (report under-specified).**
- `constants.py` — full env surface: `CACHE_DIR`, `REMOTE_CACHE_URL` (hardcoded fbaipublicfiles), `DISABLE_REMOTE_CACHE`, `TMP_DIR`, `NUM_PROCS` (cap 32), `LOAD_USED_PACKAGES_ONLY`, `TACTIC_CPU_LIMIT`, `TACTIC_MEMORY_LIMIT` (default `"32g"`; docs say 16 GB — **doc/code mismatch**), plus a startup `check_git_version((2,25,0))` hard gate.
- `data_extraction/lean.py:640-759` — dependency resolution with three fallbacks (`lake-manifest.json` → `lakefile.lean` regex → `lakefile.toml` regex), rejecting local/`path` deps, recursing transitively: the repo-ingestion pinning logic.
- `data_extraction/trace.py:167-186` — post-trace it **appends a `lean_lib Lean4Repl` stanza to the target lakefile** and builds it (warn-not-fail): how the REPL library is injected — relevant to warm-REPL bootstrap.
- Note: there is **no Docker/cgroup container tracing mode in this checkout** — the docs reference `build_lean4_repo.py` files that are absent. Container tracing is aspirational here.

**Real bugs / hazards for the adopt-list:** `cache.py:24` constant misspelled `_CACHE_CORRPUTION_MSG`; `ast.py:1379-1388` `ModulePreludeNode` defined twice (dead dup); `traced_data.py:812-821` comment-stripping asserts `"--"`/`"/-"` absent post-scrub (throws on nested/edge comments); `dojo.py:152-153` theorems ending in ` where` rejected; CI (`.github/workflows`) is only mypy+black (tests are integration-only, never gated).

**Benchmark construction (for the eval harness):** `scripts/generate-benchmark-lean4.ipynb` implements `split_randomly` **and `split_by_premise`/`novel_premises`** (hold out theorems whose premises were never seen in training, `:121-133`) with an explicit warning to report test with train-only models. Corpus export writes `{path, imports, premises}` JSONL per file.

---

## 2) LeanDojo-v2 (`resources/LeanDojo-v2-main`) ↔ `new/LeanDojo-v2.md`

### What the Codex report captured (solid)
`DynamicDatabase` (repos/theorems/premises/traced/annotations/tactics, dedup, later-version preference, merged export, random + novel-premise splits); `utils/difficulty.py` curriculum; the three agents (HF/External/Lean); `BaseProver.search` Pantograph graph + tactic provenance; the four trainers; `ProgressTrainer` regression scorer + negative-remaining reward; the FastAPI model server; trainer/export separation; and the constants/`RAID_DIR`/`~`-path fragility.

### What it MISSED
**EWC continual learning (missed entirely).** `retrieval/model.py:95-120` — **Elastic Weight Consolidation** (`ewc_loss`, Fisher information, `previous_params`, `lamda`) to prevent catastrophic forgetting across progressive per-repo training; `config.py:220-225` sets `lambda_value=0.1` only under `run_progressive_training`. Directly relevant to Theoremata training across many repos/systems without forgetting.

**Retrieval negatives / indexing / staleness (report said "per-system retrieval" only).**
- `lean_agent/retrieval/datamodule.py:166-199` — exact hard-negative recipe: split corpus into in-file (`p.end < theorem_pos`) vs outside-file (transitive-dep successors), then `random.sample(in_file, num_in_file_negatives) + random.sample(outside, num_negatives-k)`.
- `datamodule.py:231-244` — in-batch cross-labeling: `label[j,k]=1` when one example's negative is another's positive (avoids false negatives in the contrastive matrix).
- `retrieval/model.py:74,266-303` — embedding-staleness protocol: `embeddings_staled` flag flipped on `on_train_batch_end`, lazy `reindex_corpus` only when stale. Warm-load analog for Theoremata (don't re-index every call).
- `retrieval/model.py:409-470` + `retrieval_trainer.py:131-151` — per-GPU `test_pickle_{rank}.pkl` keyed by `(file_path, full_name, tuple(start), tactic_idx)`, merged across GPUs for R@1/R@10/MRR (fragile, non-atomic).
- **Confirmed absence:** `common.py:319-346` `get_nearest_premises` is single-stage dense cosine filtered by accessibility — **no BM25, no reranker** despite `rank-bm25` in `pyproject.toml:37` (dead dep). So Theoremata's BM25+reranker cascade has *no precedent here* and is strictly ahead. Bug: `common.py:343-344` raises `ValueError` if fewer than `k` accessible premises exist (crashes instead of returning a short list).

**Difficulty / curriculum formulas (report said "curriculum" only).** `utils/difficulty.py:12-19` — `exp(len(proof_steps))`; `sorry`⇒`inf`; empty⇒`None` (deferred). Percentile cuts at **33rd/67th** (`dynamic_database.py:411`). `dynamic_database.py:422-429` — `To_Distribute` (no-tactic) theorems chunked into thirds and appended to Easy/Medium/Hard by list order (arbitrary, biases curriculum).

**DynamicDatabase export edge cases (the gist was right; these bite).**
- `dynamic_database.py:82-94` — dedup key is a **6-tuple** `(file_path, full_name, start[0], start[1], end[0], end[1])`; tie-break keeps later `date_processed` (`>`), so equal-date dups keep first-seen.
- `dynamic_database.py:227-228` — `_export_proofs` silently drops tactics where `state_before=="no goals"` or the tactic contains `"·"` (structured bullet). Shapes every training set.
- `dynamic_database.py:73,159,193` — fixed `random.seed(3407)`; splits 20% val / 20% test.
- `dynamic_database.py:167-199` — novel-premise split: only premise-annotated theorems are candidates; `t not in theorems_val_test` is O(n²) (perf cliff).
- `dynamic_database.py:101` — `remove_dir(output_path)` mid-export + `to_json` on every `__init__` → non-atomic destructive writes.
- `database/models/annotations.py:9-33` — `parse_pos` parses both `"Pos(x, y)"` strings and `[x,y]` lists via `repr` round-trip: brittle string serialization contract for every position field.

**GRPO / SFT reward + hyperparams (report requested these).**
- `trainer/grpo_trainer.py:149-151` + `examples/grpo.py:13-15` — **the only shipped reward is a stub returning `1.0`**; there is no advantage/KL/reward shaping in-repo. GRPO is scaffolding only — Theoremata's real reward must come from LeanProgress (below) or its own signals.
- `grpo_trainer.py:41-58` / `sft_trainer.py:41-66` — prompt template (system: "ONE Lean tactic, single line, no `by`, no `sorry`/`admit`"); SFT target = first line of tactic.
- `sft_trainer.py:98-105` — `packing=False, completion_only_loss=True` (masks prompt); lr 2e-5, bf16, LoRA `r=16/alpha=32/dropout=0.1` on `q,k,v,o_proj`.
- `grpo_trainer.py:118-137` / `sft_trainer.py:124-139` — **progressive re-export + fresh trainer per repo** (curriculum drip-feed).

**LeanProgress reward wiring (report had the scorer; missed the formula/clamp).**
- `external_api/python/leanprogress.py:56-66` — `predict` returns `float(max(0.0, logits))` (**clamped-at-zero** regression head); reward `= -steps_remaining` (`server.py:111`), so reward is bounded ≤0. Prompt `"Goal:\n{goal}\n\nPrefix:\n{prefix}\n\nCandidate tactic:\n{tactic}"` must match training text exactly.
- `trainer/progress_trainer.py:155-169` — `bert-base-uncased`, `num_labels=1`, `problem_type="regression"`, MSE/MAE, lr 1e-5, wd 0.01; gated by `LEANPROGRESS_MODEL`.
- `lean_progress/create_sample_dataset.py:21-45` — label schema (`goal/prefix/tactic/steps_remaining`), incl. `sorry⇒12` labeling convention.

**External-API model-runner protocol + batching (report treated as black box).**
- `external_api/python/server.py:88-139` — `/generate` + `/encode`; a `use_reward` flag switches plain `(output,score)` vs LeanProgress-scored; `torch.cuda.empty_cache()+gc.collect()` around every request + dedicated `OutOfMemoryError` handling.
- `external_api/python/models.py` — three concrete runners (`DecoderOnlyTransformer` beam + `sequences_scores.exp()`, `EncoderDecoderTransformer`, `EncoderOnlyTransformer` masked-mean); `PythiaTacticGenerator` uses `[GOAL]...[PROOFSTEP]`.
- `external_models/external_parser.py:8-71` — **per-model prompt pre/post-processing table** (internlm, Kimina, gpt-3.5/4, deepseek-7B vs 671B/novita, gemini/claude) with fragile `output.split("lean")[-1].split("```")[0]...` extraction; `choices_dedup` keeps max-score. Reusable for multi-model tactic parsing under LiteLLM.
- `external_models/hf_runner.py:24-58` — calls the **HF router** (`router.huggingface.co/v1/chat/completions`, not LiteLLM) `n=4, temp=0.6`; tracks `used_tactics` to avoid repeats; returns `"sorry"` on exhaustion; bug: hardcodes model id ignoring `self.model_name`.

**Pantograph search internals (report said "graph + provenance").**
- `prover/base_prover.py:99-100, 50` — `max_steps=100`, `max_trials_per_goal=5`; goal priority `1/(len(variables)+1)`; DFS + backtracking stack + nx shortest-path extraction.
- `base_prover.py:198-217` — `TacticFailure` records feedback + a `failure_{node}_{goal}_{trial}` node/edge (failed attempts retained for provenance); `ServerError` re-raised fatal; **no per-tactic wall-clock timeout** (only step/trial counts) — Theoremata's tactic timeouts are an improvement.
- `agent/base_agent.py:123-133` — a **new `pantograph.Server` per theorem** (`imports=["Init", file_path]`), no warm reuse — Theoremata's warm bridge is a genuine improvement.
- `prover/retrieval_prover.py:73-75` — samples tactics **softmax-weighted over log-probs** (better than `hf_prover.py:89`'s uniform `random.choice`).

**Config / infra surface.**
- `lean_agent/config.py:229-281` — `ProverConfig` (`use_vllm`, `num_sampled_tactics=64`, `timeout=600`, `max_expansions`, `batch_size=12`) exists but is **not wired into `BaseProver.search`** (which hardcodes 100/5) — dead config; not authoritative.
- `generator/confs/cli_lean4_random.yaml`, `retrieval/confs/cli_lean4_random.yaml` — real hyperparams: DeepSpeed stage-2, `max_inp_seq_len=2300`, `max_oup_seq_len=512`, `p_drop=0.5`, `num_beams=1`, `max_steps=500k/800k`, retriever `max_seq_len=1024`, monitors `Recall@10_val`/`Pass@1_val`.
- `utils/constants.py:39-252` — a **~210-entry `KNOWN_REPOSITORIES` skiplist with per-repo failure reasons** ("trace problems", "no theorems", "windows-style line endings", "no compatible commit"). A curated corpus-mining blocklist directly reusable for Theoremata's repo discovery / benchmark harness.

**Misc adoptables:** `database/models/repository.py:273-289` `change_sorry_to_proven` appends a timestamped proof-log line (audit trail feeding STaR self-improvement); `database/models/theorems.py:39-50` `Theorem.__str__` returns the raw statement (silent prompt coupling); tests are only two files (thin coverage).

---

## 3) LeanDojoChatGPT (`resources/LeanDojoChatGPT-main`) ↔ `new/LeanDojoChatGPT.md`

### What the Codex report captured (solid)
The two-op HTTP contract (`initialize_proof_search`, `run_tactic`), the process-global `repo/theorem/dojo/states`, state-id + pretty-state JSON, the OpenAPI behavioral guidance, the "thin adapter" boundary, and the security gaps (no auth, permissive CORS, global-state collisions, no cleanup, the `json.dumps(str(res))` bug, no validation). For a small repo this was near-complete.

### What it MISSED
- **The verbatim tool-policy prompt** (`openapi.yaml:4` / `manifest.json:6`, identical) is worth lifting almost unchanged into a QED-retry `run_tactic` tool description: *explain the next tactic in LaTeX and run it; if the state is unpromising, backtrack by decrementing `state_id`; on error, explain it, and on repeated errors decrement `state_id` and try a different approach on a previous state; proved iff no unsolved goal.* A compact retry+backtrack policy statement.
- **A 4-way outcome taxonomy** (`main.py:46-67`) mapping onto Theoremata's three-valued taint **plus one**: `TacticState` (continue) / `ProofFinished` (QED) / `ProofGivenUp` (**`sorry` abandonment — a distinct value, not an error**) / `TacticError`+`TimeoutError` (error). Modeling `sorry`-abandonment separately from error is the notable choice (consistent with `dojo.py:418`).
- **A second real bug** beyond the known `json.dumps(str(res))`: `main.py:58-59` catches the builtin `TimeoutError` and reads `s.error`, which the builtin has no attribute for → latent `AttributeError`. Lesson: don't catch builtin `TimeoutError` for tactic timeouts (ReProver uses `DojoTacticTimeoutError`).
- **Leak mechanism sharpened:** `main.py:29` calls `Dojo(theorem).__enter__()` manually and never `__exit__`; each `initialize` reassigns global `dojo`, orphaning the prior REPL/container. Relevant to the warm-REPL bridge lifecycle.
- Init `assert isinstance(s, TacticState)` (`:30`) crashes if the initial state is already finished/errored; only `TacticState` results are stored (`:32,52`) so you cannot branch from finished/error states by id.
- Startup precondition `is_available_in_cache(repo)` (`:108`); launch knobs `CONTAINER=docker` (tracing backend) and `VERBOSE=1`.
- **The plugin never assembles/returns the final proof script** — reconstruction is left to the model via manual `state_id` bookkeeping. ReProver's `extract_proof` (below) is the missing piece Theoremata should own in its plan-history.

---

## 4) ReProver (`resources/ReProver-main`) ↔ `new/ReProver.md`

### What the Codex report captured (solid)
`common.py` `Context`/`Premise`/`Corpus` + accessible-premise logic; `<a>…</a>` provenance serialization; byte-budgeted `format_augmented_state`; the ByT5 retriever (masked-avg pool, L2-norm, MSE label-matrix) + negative-mining datamodule + `index.py` + R@K/MRR eval; the BM25 baseline; generation model/datamodule/preprocess; backend-polymorphic `tactic_generator.py`; best-first `proof_search.py` + `search_tree.py` status model + distributed Ray eval; separate retrieval vs Pass@1 tracks. This is the strongest of the four reports.

### What it MISSED
**Exact training hyperparameters (the drop-in defaults).**
- Retriever `retrieval/confs/cli_lean4_random.yaml` (+ `novel_premises`): `google/byt5-small`, `lr 1e-4`, `warmup 2000`, `num_retrieved 100`, `max_seq_len 1024`, `batch_size 8` / `eval 64`, **`num_negatives 3`, `num_in_file_negatives 1`**, `max_steps 800000`, `seed 3407`, `bf16-mixed`, DeepSpeed stage-2, `gradient_clip_val 1.0`, EarlyStopping patience 5 + checkpoint both on `Recall@10_val`.
- Generator `generation/confs/cli_lean4_random.yaml`: `lr 5e-4`, `num_beams 1`, `length_penalty 0.0`, `max_inp_seq_len 2300`, `max_oup_seq_len 512`, **`p_drop 0.5`** (premise dropout), `eval_num_retrieved 100`, `eval_num_theorems 250`, `max_steps 500000`, EarlyStopping patience 2 on `Pass@1_val`. Confirms the generator is trained with premises dropped 50% of the time (robustness to imperfect retrieval).

**Reranking — explicit answer:** ReProver has **no reranker and no cross-encoder**. Retrieval is a single bi-encoder with brute-force cosine (`common.py:299-326`: `batch_context_emb @ premise_embeddings.t()` + argsort). Theoremata's BM25+cross-encoder cascade is strictly a superset — this repo is the *baseline to beat*, nothing to port for reranking.

**The loss is subtler than "MSE label-matrix."** `retrieval/model.py:116-140` + `datamodule.py:160-175` — loss is `F.mse_loss(similarity, label)` over a **0/1 matrix of `batch × batch*(1+num_negatives)`** combining in-batch **and** explicit hard negatives, and the label builder (`datamodule.py:164-173`) **flips a negative's label to 1 when it is another example's positive** (de-dups false negatives). MSE-to-binary-label (not InfoNCE/softmax-CE) is a deliberate multi-positive design that handles multiple gold premises per state — a documented alternative if Theoremata's reranker uses contrastive CE.

**Hard-negative construction:** `datamodule.py:99-128` — in-file negatives = premises earlier in the same file (`p.end < theorem_pos`); out-of-file = transitively imported; sample `num_in_file_negatives` from the former, remainder random from the latter. Exact "accessible-but-wrong" recipe.

**Index build is pickle, not FAISS:** `index.py` + `common.py:329-338` — `pickle.dump(IndexedCorpus(corpus, embeddings.float32.cpu()))`; NN is dense matmul + argsort (no ANN). `load_hf` auto-casts bf16 on GPU capability ≥8. At mathlib scale this brute-force search is a known bottleneck — Theoremata should keep its ANN/index abstraction but the `IndexedCorpus` pickle container is a clean warm-load format to mirror.

**Best-first search scoring + edge cases (feeds MCTS/plan-history).**
- `search_tree.py:176-181` — priority = `cumulative_logprob` (sum of root→node edge logprobs), pushed as `-priority` into `asyncio.PriorityQueue`. Pure best-first — **no MCTS, no progress prior** (Theoremata is a superset).
- `search_tree.py:73` — `InternalNode` hashed **only on `state`** (`cumulative_logprob` `compare=False`), so identical goal states from different paths collapse to one node and in-edges accumulate (`proof_search.py:249-273`). Good dedup pattern for a search graph.
- `search_tree.py:132-205` — status back-prop (proved-if-any-child / failed-if-all-children) + `distance_to_proof` (min over edges) + **`extract_proof` shortest-proof reconstruction**. This is exactly the proof-assembly the ChatGPT plugin lacks — **adopt for Theoremata's QED/plan-history assembly.**
- `proof_search.py:149-169` — **bug to NOT replicate:** on hitting `timeout` OR `max_expansions` it forces `root.status = OPEN`, overwriting a proof found *exactly* at the limit (even logs "Found a proof!" one line before discarding it). A benchmark-harness edge case.
- `proof_search.py:224-225` — cross-repo path rewrite into the packages dir when `theorem.repo != self.repo`.

**Tactic postprocessing + generator internals.**
- `tactic_generator.py:196-243` — `HuggingFaceGenerator`: beam search (`num_beams=num_samples, do_sample=False`), `sequences_scores` as logprob, `remove_marks`, **exact-string dedup**, decoder-only prompt-prefix stripping. `VllmGenerator:301-322`: `"[GOAL]\n%s\n[PROOFSTEP]\n"`, `remove_marks().strip()`. `FixedTacticGenerator` wraps in `{…}` braces. Concrete candidate-postprocessing (`<a>…</a>` stripping + dedup + brace-wrapping).
- `tactic_generator.py:46` — `GPT4TacticGenerator.default_prompt`: asks for N **unique tactics with float confidences** as `#(tactic, confidence)#`, requests `int(num_samples/threshold)` extra (`threshold=0.9`), parses on `#`, retries 3, sorts by confidence, raises on unparsable. A ready confidence-eliciting prompt + robust delimiter-parse for a LiteLLM generator.
- `proof_search.py:332-366` — vLLM: `AsyncEngineArgs(tensor_parallel_size=num_gpus, max_num_batched_tokens=8192)`, `SamplingParams(n, temperature=0, use_beam_search=True, logprobs=0)`; **one shared `VllmActor`** across all `ProverActor`s. Warm-engine-sharing pattern.

**DeepSpeed / optimizer / scheduler.** `common.py:381-425` — `FusedAdam`/`DeepSpeedCPUAdam`/`AdamW` selection; **scheduler is `get_constant_schedule_with_warmup`** (constant after warmup — the docstring's "cosine" is wrong); `load_checkpoint` transparently converts DeepSpeed zero → fp32 via `zero_to_fp32.py`. Load-bearing if Theoremata trains under DeepSpeed.

**Corpus construction + filtering (mirror per system).** `common.py:141-297` — corpus from `corpus.jsonl` (imports pre-declared topologically, asserted acyclic) into a transitive-closure DAG; `File.from_data:153-173` filters ill-formed premises (`full_name is None`, `"user__.n"` in name, empty `code`, mutual-definition blocks); `get_accessible_premises` = earlier-same-file (`p.end <= pos`) ∪ transitively imported.

**Eval denominators / sharding (copy for benchmark parity).**
- `prover/evaluate.py:146-162` — **Pass@1 excludes discarded non-theorems**: `None` (from `DojoInitError`) → `num_discarded`; `pass_1 = num_proved/(num_proved+num_failed)` (returns `nan` if all discarded).
- `evaluate.py:64-79` — deterministic sharding: theorems sorted by `md5(file:full_name)`, truncated after sort (reproducible subset); `--name-filter` selects by md5-prefix bucket.
- `model.py:242-244` + `retrieval/evaluate.py:29-31` — **R@k is recall-normalized by #positives**, so R@1 < 1 whenever a tactic has multiple gold premises even if top-1 is right; zero-positive tactics are skipped from the denominator. Apples-to-apples parity requires these exact denominators.

**In-training real-proof-search validation (adoptable).** `generation/model.py:212-262` — `on_validation_epoch_end` spins up the full Ray `DistributedProver` mid-training to compute real `Pass@1_val` over 250 theorems (save generator+retriever+`IndexedCorpus`, `evaluate()`, log, delete). A template for wiring Theoremata's benchmark harness as an in-loop validation metric.

**Data-prep scripts.** `scripts/download_data.py` (Zenodo `records/12740403`, md5-verified); `scripts/trace_repos.py` (scans `data/*/*/*.json` for uncached `(url,commit)`); `generation/preprocess.py` emits **LLaMA-Factory** SFT format with `[GOAL]\n{state}\n[PROOFSTEP]\n` (bridge to external fine-tuning — relevant to STaR/GRPO data formatting); `retrieval/bm25/main.py` (`BM25Okapi`, scores restricted to accessible-premise indexes, Ray-parallel, **emits the identical preds schema as the dense retriever** → the two are drop-in interchangeable as a first stage — the exact abstraction Theoremata's cascade needs).

**Premise-injection ordering (adoptable).** `common.py:357-378` — `format_augmented_state` prepends premises in **reverse** so the highest-scored premise sits nearest the goal (goal at the very end), under a byte-budget with per-premise `p_drop`.

**Minor edge cases:** `get_nearest_premises` raises `ValueError` (for/else) if fewer than `k` accessible premises (brittle at small corpora); `Context.__post_init__` asserts `"⊢" in state` (solved states crash construction); `TopkAccuracy` dedup is `remove_marks`-only (approximate); `RetrievalDataModule.__init__:227-228` builds an unused `repo` (dead code).

---

## Consolidated top new adoptables for Theoremata

1. **Kernel-level `validateProof` soundness gate** (LeanDojo `Lean4Repl.lean:170-215`) — reconstruct a standalone decl, reject `sorry`/mvar, kernel-typecheck; wire into three-valued taint so "closed goals" ≠ "verified" until kernel-checked. Generalize per system.
2. **Proof-graph `extract_proof`** (ReProver `search_tree.py:132-205`) — state-hashed node dedup + status back-prop + shortest-proof reconstruction; the QED/plan-history assembly both the plugin and Pantograph paths lack.
3. **BM25 ⇄ dense identical-preds-schema first stage + reverse-order byte-budgeted premise injection** (ReProver `bm25/main.py`, `common.py:357-378`) — the exact interface Theoremata's BM25+reranker cascade should preserve; ReProver/LeanDojo-v2 confirm the reranker itself is net-new (Theoremata ahead).
4. **EWC continual-learning loss + progressive per-repo re-train** (LeanDojo-v2 `retrieval/model.py:95-120`, trainers) for training across many repos/systems without forgetting.
5. **Position-gated hard-negative mining + in-batch false-negative de-labeling** (ReProver `datamodule.py:99-175`; LeanDojo-v2 `datamodule.py:166-244`) with the concrete `num_negatives=3 / num_in_file_negatives=1`, `byt5-small`, `p_drop=0.5` defaults.
6. **Crash/limit taxonomy for taint:** exit-137→OOM + recursive descendant-kill (LeanDojo `dojo.py`), `sorry`-abandonment as a distinct outcome (plugin `main.py`), and the ReProver "proof-at-limit discarded as OPEN" bug to avoid.
7. **FileLock cache** with the half-extraction/atomicity + unsanitized-tar caveats (LeanDojo `cache.py`).
8. **`novel_premises` split + exact eval denominators** (LeanDojo `generate-benchmark-lean4.ipynb`; ReProver `evaluate.py` Pass@1 excludes discarded, R@k normalized by #positives, md5 sharding) for honest benchmark parity.
9. **Per-model prompt/parse table + confidence-eliciting GPT-4 prompt** (LeanDojo-v2 `external_parser.py`; ReProver `tactic_generator.py:46`) for the LiteLLM multi-model path, and the plugin's verbatim retry/backtrack tool-policy prompt.
10. **Curated `KNOWN_REPOSITORIES` skiplist with failure reasons** (LeanDojo-v2 `constants.py:39-252`) for repo discovery / benchmark harness.

Confirmed absences where Theoremata is already ahead (not gaps to fill): no BM25+reranker cascade, no real GRPO reward shaping (stub `1.0`), no warm-REPL reuse, no per-tactic wall-clock timeout, no MCTS/progress prior in search, `ProverConfig` not wired to the search loop.
