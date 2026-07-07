# Resource Mining: DeepMath (IntelLabs)

Full-pass study of `resources/DeepMath-main/DeepMath-main` for the Theoremata project.
Every code and prose file was read in full (the repo is small: ~2,400 LOC of Python + configs +
prose; no dataset directories exist inside it). Build artifacts and binary images (`assets/*.jpg|png`)
were skipped as instructed.

> **Headline framing correction.** DeepMath is **not** a proof / DAG / Lean / formalization system, and
> it has **no falsification, retrieval, MCTS, or evolution loop**. It is a *GRPO-trained, tool-using math
> agent* for **numeric contest answers** (MATH500 / AIME / HMMT / HLE). Its relevance to our
> "model-derived falsification" idea is by **analogy**: the model emits **executable Python** that runs in
> a hard sandbox and folds results back into the trace. DeepMath uses that pattern for *computation*, not
> for *falsification* — but the sandbox, token-budget, parse/execute/observe, and agentic-RL-rollout
> machinery are directly reusable. Confirmed: this is a genuine "model emits executable code" precedent,
> just aimed at arithmetic rather than counterexample search.

---

## 1) What it is (scope, size, structure)

- **Product**: `Intel/deepmath-v1`, a Qwen3-4B-Thinking model fine-tuned with **GRPO** so it prefers
  emitting **short Python snippets** (run in a sandbox, results reinjected) over verbose hand arithmetic.
  Blog: `huggingface.co/blog/intel-deepmath`. Authors: Fleischer, Berchansky, Wasserblat (Intel AI Labs).
  License Apache-2.0. (`README.md`, `pyproject.toml`)
- **Claimed result**: up to **66% shorter output** while improving accuracy on hard sets; ablations show
  *both* GRPO training *and* agentic inference are needed (they are independent, not additive). Metric of
  record is **majority@16**. (`README.md` §Evaluation)
- **Size / structure** (nested one level: `DeepMath-main/DeepMath-main/`):
  - `deep_math/agent.py` (621 lines) — **the core**: `MathAgent` (smolagents `CodeAgent` subclass),
    `VLLMCustom` model wrapper, `CodeExecutionTimeout` sandbox.
  - `deep_math/vllm_serve_agent.py` (664) — a **modified TRL vLLM server** that runs the *agent* as the
    GRPO rollout generator (data-parallel worker processes over a Pipe).
  - `deep_math/vllm_client.py` (357) — TRL vLLM client fork (weight sync via safetensors files, NCCL
    disabled).
  - `deep_math/models.py` (286) — `HFInference` / `HFTrain` Hydra-instantiated wrappers.
  - `deep_math/prompts.py` (49), `fewshot.txt` (134) — prompts + 4 worked in-context examples.
  - `deep_math/rewards.py` (62), `metrics.py` (124), `training_utils.py` (101) — GRPO rewards, eval
    metrics, temperature scheduler.
  - `training.py` (168) / `inference.py` (54) / `evaluation.py` (81) — Hydra entrypoints.
  - `configs/*.yaml` — Hydra configs (training/inference/evaluation/vllm/zero2/slurm).
  - `utils/combine.py` (31) — merge sharded JSONL outputs.
- **Stack** (`requirements.txt`, `pyproject.toml`): `smolagents==1.22.0`, `trl==0.17.0`, `vllm==0.20.0`,
  `transformers==5.0.0rc3`, `math-verify==0.7.0`, `hydra-core`, `peft`, `deepspeed`, `accelerate`.
  Python 3.10+. **No Lean, no LiteLLM, no graph/DAG library, no retrieval stack.**

---

## 2) Reusable ideas / patterns / code for Theoremata — THE priority

### 2.1 Hard-kill code sandbox with pickle→thread fallback (`agent.py:47-132`)
Directly reusable for our **executable falsifier** sandbox. It runs a callable in a **child process** and
`terminate()`/`kill()`s it on timeout (strong enforcement); if the callable isn't picklable it falls back
to a thread that can raise `TimeoutError` but can't be force-killed. This is more robust than a bare
`signal.alarm`/thread and handles the "bound method not picklable" case explicitly.

```python
class CodeExecutionTimeout:
    def run_with_timeout(self, func, *args, **kwargs):
        try:
            pickle.dumps((func, args, kwargs))          # validate picklability early
            q = mp.Queue(); p = mp.Process(target=self._process_wrapper, args=(q, func, args, kwargs), daemon=True)
            p.start(); p.join(self.timeout_seconds)
            if p.is_alive():
                p.terminate(); p.join(1.0)
                if p.is_alive(): p.kill(); p.join()
                raise RuntimeError(f"Code execution timed out after {self.timeout_seconds} seconds")
            ...
        except (pickle.PicklingError, AttributeError, TypeError):
            # fallback: run in a daemon thread, wait, raise on timeout (cannot kill thread)
```
Default `code_execution_timeout=20`s per snippet. **Lesson for us**: our falsifier runner should prefer a
subprocess we can actually kill, with a thread fallback — DeepMath already worked out the picklability
gotcha.

### 2.2 Import allow-list for the executor (`agent.py:258-266`)
The sandbox restricts imports to a math allow-list; anything else raises "Import of X is not allowed" which
the agent catches and surfaces as a hint (`agent.py:600-604`). Reuse verbatim for a numeric-falsifier
sandbox:
```python
additional_authorized_imports = ["cmath","numpy","numpy.*","scipy","scipy.*","sympy","sympy.*"]
```
The **system prompt** additionally advertises the allow-list to the model
(`prompts.py:33`: "only from the following list of modules: cmath, numpy, scipy, math, random, sympy,
statistics"). README stresses: **no file I/O, no network, strict timeouts** (`README.md:14`).

### 2.3 Global **token-budget** governor across agent steps (`agent.py:523-537`)
Instead of `steps × per_step_max`, DeepMath caps **total** output tokens per run (`max_agent_output`) and
dynamically shrinks each step's `max_tokens` as the budget drains; when < 50 tokens remain it forces a
final step. Cleanly separates "how many reasoning turns" from "how much text total" — useful for our
retry-tier / best-of-N budgeting.
```python
self.sampling_params.max_tokens = min(max(self._max_agent_output - self._output_tokens, 1), self._typical_step)
if self.sampling_params.max_tokens < 50: out_of_tokens = True
```

### 2.4 Parse / execute / observe loop with **bidirectional tag rewriting** (`agent.py:432-621`)
The step loop: generate → detect `\boxed{}` (terminal) → convert `<tool_call>…</tool_call>` ⇄
```` ```python…``` ```` → parse code blob → execute in sandbox → wrap result as an `observation`. Concrete
reusable bits:
- **Termination signal**: presence of `\boxed{` in model output ends the run (`agent.py:514`). Analogous
  to our "final answer / QED emitted" detector.
- **Stop sequences** for interleaved code (`agent.py:483-489`):
  `["```\n", "</tool_call>", "<end_code>", "Observation:", "Calling tools:"]`.
- **Regex rewrites** (`agent.py:540-552`): `<tool_call>(.*?)</tool_call>` ↔ ```` ```python\n\1\n``` ````
  with `re.DOTALL`. Keeps the model's native tag format while feeding smolagents' `parse_code_blobs`.
- **Observation format** the model is trained to expect (`agent.py:609`):
  ```` "```output\n" + execution_logs + "\n\n" + truncated_output + "\n```\n" ````. Note (`agent.py:579`):
  they deliberately *omit* raw execution logs when they'd diverge from the few-shot pattern — trace/format
  consistency between few-shot and runtime matters.

### 2.5 Agentic-RL **rollout server** — the biggest architectural idea (`vllm_serve_agent.py`)
DeepMath **modified TRL's vLLM server so the GRPO rollout is a full multi-step agent run**, not a single
completion. Each data-parallel worker owns a `MathAgent`; `/generate/` fans prompts across workers via
`multiprocessing.Pipe`, each worker runs `agent.run_batch(...)` and returns **tokenized completion ids**.
Weight sync is done by **writing safetensors/`.pt` to `/tmp` and loading by path** (NCCL broadcast
commented out; `WeightSyncWorkerExtension.update_named_param`). `training.py:23-26` monkey-patches
`trl.extras.vllm_client.VLLMClient` with their fork. If we ever RL-train a prover/falsifier, this is a
worked template for "**agent trace as the RL sample**." Robustness detail: on any worker error they return
the token ids `[40, 1513, 944, 1414, 13]` = "I don't know." as a graceful fallback
(`vllm_serve_agent.py:281,504-508`).

### 2.6 GRPO reward design (`rewards.py`, `training.yaml`)
Two reward functions, **weighted 0.1 : 1.0** (`training.yaml:81-84`):
- `open_r1_accuracy_reward` — parse+verify via `math_verify` with a tuned `LatexExtractionConfig`
  (`boxed="all"`, `boxed_match_priority=0`, `try_extract_without_anchor=False`); returns `None` to **skip
  unparseable golds** rather than penalize (`rewards.py:56-59`).
- `code_format_reward` — 1.0 if output contains `<tool_call>…</tool_call>` (`rewards.py:8-15`).
The README describes the intent as accuracy +1, code-use +1 weighted 10:1 (`README.md:75-79`) — a **shaping
reward that rewards *using the tool at all*, decoupled from correctness**. Analogous shaping for us: reward
"produced a runnable falsifier/formalization attempt," separate from "it passed."

### 2.7 Temperature scheduling for exploration (`training_utils.py`)
`GRPOTrainerTemperature` overrides `training_step` to linearly anneal sampling temperature
(**1.2 → 0.7** over training; `training.yaml:85-89`). Cheap, self-contained, portable to any best-of-N /
MCTS sampler — start hot to explore diverse formalizations, cool to stabilize.

### 2.8 Prompt engineering assets (`prompts.py`, `fewshot.txt`)
- `agent_math_instruction` (`prompts.py:19-49`): a tight, imperative tool-use system prompt — "You **MUST**
  use python … don't calculate by hand," "Use only variables that you have defined," "An `output` block
  will be inserted … don't add it yourself," "Never say you don't know," "final answer within `\boxed{}`."
- `fewshot.txt`: **4 fully worked examples** demonstrating the exact `<tool_call>` → ```` ```output ````
  → reasoning → `\boxed{}` pattern (sympy solve, Rational diffs + numpy argmin, sphere-inscribed box,
  complex-exponential bee walk). This is a reusable **template for teaching a small model an
  emit-code/read-result protocol** — directly analogous to teaching a model our falsifier/formalizer DSL.
- Note discrepancy: `example.sh` references `agent_math_instruction_2`, which **does not exist** in
  `prompts.py` (only `agent_math_instruction`). Dead/renamed reference.

### 2.9 Config-driven everything via Hydra (`configs/`, `models.py`)
Models/metrics/rewards are `hydra.utils.instantiate`d from `_target_` strings; inference/eval/train are all
`@hydra.main` entrypoints with `-m` (multirun) + a Slurm `submitit` launcher (`configs/hydra/launcher/
slurm.yaml`). This "pick component by dotted path in YAML" pattern is a clean way to make our
falsifier/retriever/formalizer strategies swappable without code changes.

---

## 3) Blueprint / DAG / schema / eval / data format

- **No DAG / proof graph / node schema exists.** (Confirms nothing to port on the graph side.)
- **Generation record schema** (JSONL, `inference.py:43`): `{"text": <output>, "target": <gold answer>}`.
  When `model.sampling>1`, `text` is a **list** of samples (handled downstream). Sharded runs use
  `_{start:03}_{end:03}.jsonl` suffixes, merged by `utils/combine.py`.
- **Eval schema** (`evaluation.py`, `metrics.py`): produces a YAML with `{local, global, raw}`; `local`
  metrics carry `_std` and `_se` (standard error `std(ddof=1)/sqrt(n)`). Metrics:
  - `MathVerify` (`metrics.py:56-100`) — parses gold as `"$"+target+"$"` (latex env required "from
    experimentation"), reports `majority@k`, `pass@k`, `averaged@k`, and single `math_verify`. Order of
    verify calls matters (comment `agent.py`/`metrics.py:88`).
  - `OutputLength` — mean tokenized length (brevity metric).
  - `majority(...)` aggregation (`metrics.py:7-30`) with careful **hashability handling** (str-key map,
    keeps last object per key) — reusable for any best-of-N vote where samples aren't hashable.
- **Training data**: `fastragdev/openmathreasoning-tir` (config) / `nvidia/OpenMathReasoning` tool-use
  subset (README). **GRPO uses only the `problem`, never the solution** (`README.md:85`) — reward is
  computed against `answer` via math-verify, so it's essentially outcome-supervised RL.

---

## 4) What our earlier TARGETED pass likely MISSED

1. **The subprocess hard-kill timeout with picklability fallback** (`agent.py:47-132`) — the single most
   reusable artifact; easy to overlook as boilerplate.
2. **The global token-budget governor** (`agent.py:523-537`) — non-obvious `min/max` budgeting that
   decouples turns from total tokens; forces graceful termination at <50 tokens.
3. **The agent-as-GRPO-rollout server design** (`vllm_serve_agent.py`) — that TRL's vLLM server was
   surgically modified so the RL *sample* is a multi-step agent trace, with **file-based weight sync and
   NCCL disabled** (a pragmatic hack worth knowing).
4. **The "I don't know" token-id fallback** `[40, 1513, 944, 1414, 13]` returned on any worker/EOF/timeout
   error (`vllm_serve_agent.py:281,504-508`) — a graceful-degradation pattern for batch RL.
5. **The 130k-token prompt guard** (`agent.py:202-208`) — aborts with "I don't know." rather than OOM when
   a runaway trace exceeds context.
6. **Bidirectional `<tool_call>` ⇄ ```` ```python ```` rewriting** and the exact **stop-sequence set**
   (`agent.py:483-489,540-552`).
7. **Temperature scheduling in GRPO** via a `training_step` override (`training_utils.py`).
8. **Reward returns `None` to *skip* unparseable golds** instead of scoring 0 (`rewards.py:56-59`) — subtle
   but important for clean RL signal.
9. **The exact `math_verify` extraction config** (`boxed="all"`, `boxed_match_priority=0`,
   `try_extract_without_anchor=False`) and the "gold must be wrapped in `$…$`" quirk (`metrics.py:74`).
10. **`_step_stream` finalize bug smell** (`agent.py:401-407`): a `FinalAnswerStep` is yielded inside the
    `finally` on *every* step, plus once after the loop — worth noting if we lift this loop, don't copy the
    double-yield blindly.

---

## 5) Test / benchmark value

- **Benchmarks named**: MATH500, AIME, HMMT, HLE (README). Concrete HF tags used:
  `HuggingFaceH4/MATH-500`, `fastragdev/math500_test`, `fastragdev/openmathreasoning-tir`,
  `nvidia/OpenMathReasoning`. These are ready-to-use eval sets for a **numeric-answer** track of Theoremata
  (final-answer contest math), complementary to our formal-proof track.
- **Metrics worth adopting**: `majority@k` / `pass@k` / `averaged@k` via `math-verify`, plus mean output
  length (brevity), reported with std + standard error. The `math_verify.parse/verify` latex-equality check
  is a drop-in **answer-equivalence gate** for numeric outputs (different from a Lean `#print axioms` gate,
  but useful as a cheap pre-Lean filter).
- **No unit tests / CI test suite** ship in the repo (only a Scorecard security workflow + pre-commit ruff).
  So benchmark value is in the *datasets and metric definitions*, not in a test harness to reuse.

---

## 6) New vs. already-in-our-design

**Validates / reinforces our design:**
- **Model-derived executable artifact** — confirmed precedent: model emits code, runs in a hard sandbox,
  folds output back. We generalize this from *computation* (DeepMath) to *falsification* (our falsifier).
  DeepMath is concrete proof the emit-execute-reingest loop works with a small model.
- **Executable check over hardcoded logic** — DeepMath's whole thesis ("offload deterministic computation
  to a verifiable executor") is the same instinct behind our "executable falsifier, not hardcoded check."
- **Best-of-N + majority voting** — `sampling=16` + `majority@16` mirrors our best-of-N formalize/vote.
- **QED-style termination** — `\boxed{}` detection is a lightweight analog of our final-answer gate.

**New/concrete to lift:**
- Subprocess hard-kill sandbox with pickle fallback; import allow-list; token-budget governor; observation
  formatting; temperature scheduling; reward-shaping that rewards *tool use* independent of correctness;
  reward-skip on unparseable gold; Hydra `_target_` component swapping; agent-as-RL-rollout server (only if
  we ever RL-train).

**NOT present in DeepMath (so no help here):** falsify-before-prove ordering, retrieval/RAG,
Lean/formalization, `#print axioms` / LeanParanoia gates, proof-DAG core, MCTS with LLM prior,
AlphaEvolve-style evolution loop, LiteLLM model-agnostic provider. DeepMath is hard-wired to vLLM +
Qwen3-4B and numeric contest answers. Its contribution to us is **sandbox + agent-loop + RL-rollout +
eval-metric craft**, not proof/graph architecture.

---

### File index (all read in full)
`README.md`, `deep_math/{agent,vllm_serve_agent,vllm_client,models,prompts,rewards,metrics,training_utils}.py`,
`deep_math/fewshot.txt`, `training.py`, `inference.py`, `evaluation.py`, `utils/combine.py`,
`configs/{training,inference,evaluation,vllm_agent_server,zero2}.yaml`,
`configs/hydra/launcher/slurm.yaml`, `example.sh`, `pyproject.toml`, `requirements.txt`, `ruff.toml`,
`.pre-commit-config.yaml`. (Skipped: `assets/*` images, `LICENSE`, `CODE_OF_CONDUCT.md`, `SECURITY.md`,
`.github/workflows/scorecard.yml` — non-technical.)
</content>
</invoke>
