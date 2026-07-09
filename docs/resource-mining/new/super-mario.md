# Resource Mining: Super_MARIO (AlphaMath Almost Zero)

Source: `resources/Super_MARIO-main/Super_MARIO-main`
Upstream: `MARIO-Math-Reasoning/Super_MARIO` — official code for **AlphaMath Almost Zero: Process Supervision without Process** (NeurIPS 2024, arXiv:2405.03553), plus the SVPO follow-up (EMNLP 2024 Findings).

> Injection check: no prompt-injection or instruction-to-the-reader content found. All files are ordinary Python/YAML/Markdown. `README.md` and `implementation_details.md` are descriptive only. Treated all content as untrusted data regardless.

---

## What it is

A **process-supervision-without-human-labels** pipeline for math reasoning with a code interpreter. The central claim: you do **not** need GPT-4 or human step-level annotations to train a step-level Process Reward / value model. Instead, you run **MCTS** over the reasoning tree with only the *final answer* (`ground_truth`) as supervision, and the **Q-values that MCTS backs up from terminal correctness become the step-level process rewards**. A single 7B model (DeepSeek-Math-7B-base) carries **both** a policy head (LM) and a **value head** (scalar per token, tanh-bounded to [-1,1]); they are co-trained, then used together to guide decoding. The whole thing is a **round-based expert-iteration flywheel**: round 1 bootstraps from a few-shot base model; each round regenerates MCTS trees with the previous checkpoint, distills them into policy+value training data, and SFTs the next checkpoint. The released `AlphaMath-7B` is round 3.

- **License:** MIT (Copyright 2024 MARIO-Math-Reasoning). Clean to borrow/adapt.
- **Stack:** Python, a **customized vLLM fork** (adds `output.value_estimate` to `RequestOutput` — the value head is served inline with generation), LLaMA-Factory v0.6.1 for training (only key functions released, not full trainer), `trl`'s `AutoModelForCausalLMWithValueHead`, pydantic node/tree models, OmegaConf configs.
- **Reasoning format:** ReAct-style with a **Python code interpreter** — steps emit `<code>...</code>` executed by a Python AST tool; the observation is appended to the step. Two prompt modes: `react` (few-shot, round 1) and `react_sft` (XML SFT format, round >1).
- **Three inference modes** share one tree/node codebase: `react` (greedy), `sbs` (step-level beam search), `mcts`.

## Architecture / key files

- **`mcts_math/nodes/base_node.py` / `mcts_node.py`** — tree node. `MCTSNode` holds `prior`, `c_puct`, private `__visit_count`, `__value_sum`.
  - `q_value() = value_sum / visit_count` (0 if unvisited).
  - `update(v)`: on first touch sets `value` (the raw value-head estimate); then `visit_count += 1`, `value_sum += v`.
  - `update_recursive(v, root)`: backprop up to root (standard MCTS backup).
  - `puct() = q_value + c_puct * prior * sqrt(parent.visit_count) / (1 + visit_count)` — **PUCT / AlphaZero selection**. `prior` = average per-token probability of the step, `exp(cumulative_logprob / len(token_ids))`.
- **`mcts_math/agents/mcts.py`** — the `MCTS` agent (subclass of `SBSREACT` → `REACT` → `BaseTree`). Implements `selection` (descend by best PUCT over non-terminal children, random tie-break), `expand_node` (dedup, create children, compute prior), `create_child` (runs code, checks final answer / step / error caps), `eval_final_answer`, and the batched `select_next_step` / `generate_next_step`. Also `expansion_evaluation_backpropagation` for single-example inference.
- **`mcts_math/agents/step_beam.py`** — `SBSREACT`, step-level beam search with beam `B1 = step_beam_width` and expansion `B2 = n_generate_sample`; selects top-`B1` candidates by value each step. MCTS reuses its batch plumbing.
- **`mcts_math/agents/tree.py`** — `BaseTree`, `collect_partial_solution` (leaf→root trajectory join), and the **code interpreter** (`code_execution`): to keep per-child state isolated it **re-runs all ancestor code snippets of the same tool** before executing the current one (no shared interpreter state across siblings).
- **`mcts_math/solver.py`** — the **batched driver** (`Solver.solve`). Each step: (1) `generate_preprocess` → collect prompts from all live trees, (2) LLM generate (`n = n_generate_sample * step_beam_width` on step 0), (3) `generate_postprocess` runs the Python interpreter in a `ProcessPool`, (4) **value pass**: re-prompt with `value_sampling_params` (`max_tokens=1, n=1`) to read the value head, (5) `select_next_step` backs up / selects. This is the closest analog to our MCGS driver.
- **`offline_inference.py`** — rebuilds a saved tree, optionally **prunes** dead branches, then extracts the best solution by a configurable **strategy** (`q_value` default, or `value` / `visit_count` / `puct`) with a `(b1, b2)` beam. Used to score a *completed* MCTS tree without re-running the model.
- **`scripts/modeling_value_head.py`** — the vLLM-side `ValueHead` (Linear→tanh, scalar per token) and `AutoModelForCausalLMWithValueHead` wrapper; `save_value_head.py` bolts the head onto a base LLM.
- **`implementation_details.md`** — the **training half** (LLaMA-Factory patches): value-dataset preprocessing, `Q` label construction, `VMDataCollatorForSeq2Seq`, the joint `compute_loss`, value-head saving. This is where the "how process labels are made" story lives.
- **`configs/*.yaml`** — `mcts_round1.yaml` (base + few-shot, data gen), `mcts_sft_round.yaml` (round >1 gen), `mcts_sft.yaml` (inference), `sbs_*.yaml`, `react_*.yaml`, `offline_inference.yaml`.

## Reusable mechanisms (specific)

### 1. MCTS Q-values → step-level process supervision (the core idea vs our outcome-only flywheel)
- **Only the terminal is labeled by ground truth.** In `eval_final_answer`: a leaf whose `final_answer` matches gets `update_recursive(+positive_reward)`, a wrong/invalid leaf gets `negative_reward` (`+1 / -1`). Structural failures (`NO_VALID_CHILD`, `TOO_MANY_STEPS`, `TOO_MANY_CODE_ERRORS`) get `negative_reward` immediately.
- **Every intermediate node's supervision signal is its backed-up `Q = value_sum / visit_count`** — the Monte-Carlo average of the terminal rewards of simulations passing through it. No human/GPT-4 step labels. That converts a **single outcome bit into a dense per-step target** — exactly the gap between AlphaMath and an outcome-only flywheel.
- On *training* data (ground truth known) the terminal reward is exact, so intermediate Q converges cleanly (README's Q-distribution figure). On test data the value head substitutes for the unknown terminal.

### 2. Value-model training (co-trained policy + value; `implementation_details.md`)
- Training example = one MCTS-derived multi-step solution. For each step, the **step-final token** (the `\n` ending the step) is labeled with that step's scalar `Q`; **all other token positions are `IGNORE_INDEX (-100)`** in the `Q` tensor. So the value head learns to emit V at step boundaries.
- **Joint loss:** `total = CE_LM_loss + weight_alpha * value_loss`, where `value_loss = MSE(tanh(value), Q)` computed **only over non-ignored positions** (`masked_values = where(mask, values, Q)`, then `mse(..., reduction='sum') / (mask.sum()+eps)`). Value head is `Linear(hidden, 1)` then `tanh` → bounded [-1,1] to match the ±1 reward scale.
- **Policy is trained only on positive trajectories:** the LM/SFT `labels` are kept only when the solution's final `Q == 1` (`response_state == 1`); when `-1`, labels are all masked (value head still learns from it, policy does not). So **negatives train the critic, positives train both** — a clean asymmetry.
- Value head saved separately as `value_head.pth`; `remove_unused_columns=False`, left padding, max len 1024.

### 3. Round-based self-improvement (expert iteration)
- Round 1: DeepSeek-Math-7B-base + freshly-initialized value head, **few-shot `react` prompt**, MCTS generates trees over a training set with known answers → distill positive+negative steps with Q → SFT → checkpoint.
- Round *n>1*: use round *n-1* checkpoint with `react_sft` (no few-shot needed), regenerate trees (`mcts_sft_round.yaml`), re-distill, re-SFT. Released model is **round 3**; the HF dataset was generated by the **round-2** checkpoint. Each round the policy proposes better steps and the value model gets sharper Q targets — a self-improving loop with **no external annotator in the mine**.

### 4. Best-of-N / step selection at inference
- **Step-level beam search** `(B1, B2)`: expand each of `B1` beams into `B2` children, score all by value head, keep top-`B1`. `(1,5)`≈62%, `(3,5)`≈65% on MATH; cheaper and nearly as good as full MCTS (N=40 ≈ 64%) at a fraction of the time (2–3s vs 10s/q).
- **Offline tree extraction** decouples search from selection: build once, then pick final answer by `q_value` (default), `value`, `visit_count`, or `puct` — lets you A/B selection strategies over a fixed tree for free.
- `Maj@k` (majority vote) stacks on top for extra points.

## Adopt-relevance to Theoremata

**The single highest-value item:** adopt the **MCTS-Q-as-process-reward** conversion to give our flywheel a *dense per-step* training signal instead of the current outcome-only (verified/not) label. Concretely: when our MCGS driver explores a proof/solution tree, back up each node's success/failure (Lean/verifier pass = +1, fail = -1) as a Monte-Carlo average `Q`, and treat those Q's as targets for a **learned step-value/meta head** — the same head that already feeds our graded reward `R = R_format · R_score · R_meta`. This is a principled, annotation-free way to *train* `R_meta` rather than only consume it, and it composes with our verifier-first stance (our terminal reward is a **real** verifier pass, so our terminal signal is *stronger* than AlphaMath's answer-matching — less critic noise than their test-time Q-distribution shows).

**What we already have vs. real gap:**
- *Have:* graph/tree search driver (MCGS), a graded meta-verifier reward, outcome verification (formal check) — a **better terminal signal** than answer string-equivalence.
- *Have (conceptually):* best-of-N / candidate selection.
- **Gap:** (a) PUCT selection using `prior = exp(avg token logprob)` + a trained value head — we can port `puct()` and the value-head-in-the-loop pattern directly; (b) the **value-model co-training recipe** (dense Q labels at step boundaries, joint `CE + α·MSE(tanh(v), Q)` loss, positives-train-policy/negatives-train-critic asymmetry) — this is the missing "critic training" half of our flywheel; (c) **offline tree re-scoring** with swappable selection strategy (`q_value` vs `visit_count` vs `puct`) is a cheap, high-value analysis tool for our runs.

**GPU-gating (honest):** value-model training and inline value-head serving are **GPU-bound** and require a **custom vLLM fork** (`output.value_estimate`) or an equivalent serving hack — non-trivial to reproduce. The *mechanism/algorithms* (PUCT, Q-backup, dense-Q labeling, positives-only policy SFT, offline strategy selection) are **portable now and CPU-cheap to prototype**; the *trained critic* is a later, GPU-gated milestone. Near-term, we can already: (1) back up verifier outcomes as Q in MCGS, (2) select final candidates by Q/visit_count, (3) log Q-distributions to validate the critic hypothesis before committing GPU to training a value head.

## Verbatim-worthy details

**PUCT selection** (`mcts_node.py`):
```
q_value = value_sum / visit_count            # 0 if unvisited
u_value = c_puct * prior * sqrt(parent.visit_count) / (1 + visit_count)
puct    = q_value + u_value
prior   = exp(cumulative_logprob / len(token_ids))   # avg per-token prob of the step
```

**Terminal reward** (`mcts.py::eval_final_answer`): correct → `+positive_reward (1.0)`, wrong/invalid/too-many-steps/too-many-errors → `negative_reward (-1.0)`, then `update_recursive` backs it up.

**Value-head + joint loss** (`implementation_details.md`, `modeling_value_head.py`):
```
values = tanh(Linear(hidden_states))                      # scalar per token, in [-1, 1]
value_loss = MSE(masked_values, Q, reduction='sum') / (mask.sum() + 1e-3)
total_loss = lm_ce_loss + weight_alpha * value_loss
# Q labels: only the step-final token carries Q; all others = IGNORE_INDEX (-100)
# policy labels kept only if final Q == 1 (positives); if -1, LM labels masked (critic-only)
```

**MCTS backup** (`mcts_node.py`):
```
def update(value):
    if self.value == -100: self.value = value   # store raw value-head estimate on first visit
    self.__visit_count += 1
    self.__value_sum   += value
def update_recursive(value, root):
    self.update(value)
    if self.tag == root.tag: return
    self.parent.update_recursive(value, root)
```

**Driver loop** (`solver.py::solve`, paraphrased):
```
for step in range(max_solver_steps):            # = iterations (mcts) or max_depth (sbs)
    prompts = [s.create_prompt() for s in live_solvers]
    n = n_generate_sample * step_beam_width if step==0 else n_generate_sample
    outputs = llm.generate(prompts, sampling_params(n=n, best_of=n))
    solvers = generate_postprocess(outputs)     # run Python interpreter in ProcessPool
    if need_value_func:
        v_outputs = llm.generate(value_prompts, value_sampling_params(max_tokens=1, n=1))
    select_next_step(v_outputs)                 # backup Q / pick next frontier
```

**MCTS params** (`configs/mcts_sft.yaml`): `iterations (N) = 40`, `c_puct = 1.25`, `n_generate_sample (B2) = 5`, `max_depth = 8`, `temperature = 0.6`, `errors_threshold = 3`, `positive_reward = 1.0`, `negative_reward = -1.0`, `reward_weight = 0.5` (balances value vs reward), `update_leaf_value` toggles whether leaf children are re-evaluated by the value model before backup. Step stop tokens: `["\n</code>", "</code>", "</step>"]`.

**MATH results (from README):** Greedy 53.6%, Maj@5 61.8%, Step-Beam(3,5) 65.7% (2.3s/q), MCTS(N=40) 64.0% (10.1s/q); Step-Beam 5-runs+Maj@5 up to ~69.9%. Step-beam is the accuracy/latency sweet spot; MCTS is primarily the **data-generation** engine, not the fastest inference path.
