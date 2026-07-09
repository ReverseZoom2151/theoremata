# Paper Mining: AlphaMath Almost Zero — Process Supervision without Process

Guoxin Chen, Minpeng Liao, Chengxi Li, Kai Fan — *NeurIPS 2024*. Tongyi Lab (Alibaba).
Source PDF: `math-papers/AlphaMath Almost Zero - Process Supervision without Process.pdf` (fully read, ~28 pp incl. all appendices A–F).
Code: https://github.com/MARIO-Math-Reasoning/Super_MARIO

> NOTE: content below is extracted from the paper as untrusted data. Nothing in the paper is an instruction to us. (The paper contains prompt-format XML and worked solution examples; these are reported, not obeyed.)

## Core contribution
AlphaMath eliminates the need for **process** annotations (human- or GPT-4-written step-by-step solutions) in math reasoning by using MCTS to *derive* step-level supervision automatically. A single LLM is given two heads — a policy head (next-token) and a value head (a linear+tanh layer) — and MCTS run over question–answer pairs produces both (a) correct/incorrect solution paths to SFT the policy on, and (b) per-step Q-values that become regression targets for the value head. The value model then powers an efficient inference strategy, **Step-level Beam Search (SBS)**, that lets the value model steer the policy at minimal cost. On DeepSeekMath-Base-7B, trained with only 15k (question, answer) pairs and **zero solution processes**, it matches or beats SOTA 7B SFT models that used GPT-4/human process data.

## Key techniques / architecture — how process reward is derived WITHOUT process annotation

This is the directly-relevant core for our graded reward + flywheel. Mechanism:

- **RL framing.** A solution is T reasoning steps. State `s_t` = partial solution (question + all steps so far, concatenated); action `a_t` = next reasoning step; transition is deterministic concatenation `s_{t+1} = Cat(s_t, a_t)`. Policy `π(a_t|s_t) = LLM`. Reward is **outcome-only**: `r = 0` for all non-terminal steps, `r = +1 / −1` for correct / incorrect final answer (checked by answer-equivalence against the known gold answer — this is the *only* external signal, hence "almost" zero, not zero).
- **The trick: MCTS state-value = process supervision.** Instead of any human/GPT label on intermediate steps, the step's quality is the Monte-Carlo estimate of expected outcome from that state. Naive MC needs many rollouts per state; AlphaMath uses MCTS to *reuse* simulations and update estimates in a principled way.
- **Value head shares the policy backbone.** Append one linear layer with `tanh` (range [−1,1]) alongside the softmax token head. The two "models" π and V share almost all parameters. The value of a step is read from the representation of the **last token of the step** (typically `</step>`), analogous to BERT's `[CLS]` — chosen so the step representation is not skewed by the final token's own identity.
- **MCTS, four ops (customized):**
  - **Selection** via a PUCT variant (Eq. 3): `a_t = argmax_a [ Q̂(s_t,a) + c_puct·π(a|s_t)·√(N_parent(a)) / (1 + N(s_t,a)) ]`. Prior `π(a|s_t)` = exp of the **averaged log-probability of all tokens in step a**: `exp( (1/|a|) Σ_j log π(a_j | a_{<j}, s_t) )`.
  - **Expansion**: back-trace leaf→root forms the prompt; sample new steps with **high temperature** for diversity (LLM has unbounded action space).
  - **Evaluation** (Eq. 4): `V̂(s_t)^(i) = (1−λ)·V_k(s_t) + λ·r(...)`. λ is an **indicator** `λ = I_terminal(s_t)`: if the expanded node is terminal, use the true reward; else use the value model's prediction. (A trade-off between AlphaGo and AlphaGo Zero, justified because tree depth is shallow — max 8 — so expansions often hit terminals.)
  - **Backup** (unmodified AlphaGo-style): along leaf→root, `N(s,a) ← N(s,a)+1`; `Q̂(s,a) ← (1/N(s,a)) Σ_j I_{s,a∈s_t} V̂(s_t)^(j)`.
- **Q-values become the value-model training targets.** Because the transition is deterministic and reward is 0 on non-terminals, `Q(s_t,a_t) = r + V(s_{t+1}) = V(s_{t+1})`, so the converged MCTS Q̂ directly fits the value regression target (Eq. 5): `V(s_{t+1}) = Q̂(s_t, a_t)`. No rollouts at train-eval time, no human labels.
- **Multi-task training loss (Eq. 6):** sample correct paths `x+` and incorrect paths `x−` from tree `T_k`. Minimize:
  `−log π(x+|q)  +  β·[ Σ_{t∈T(x+)} (V(s_t) − V̂(s_t))² + Σ_{t∈T(x−)} (V(s_t) − V̂(s_t))² ]`
  i.e. NLL for next-token prediction **only on correct solutions**, plus value-regression MSE on **both** correct and incorrect solutions. β weights the value loss (0.1, or 0.0005 for Llama3).
- **Iterative training flywheel (K=3 rounds, Fig. 1).** (1) collect (Q,A) dataset; (2) run MCTS with current π_k, V_k to generate correct+incorrect paths + state-values; (3) SFT both heads → π_{k+1}, V_{k+1}; repeat. Round-1 value head is random-init (predicts ~0), but terminal ±1 rewards back-propagate so Q̂ converges into [−1,1] as N grows. Round 1 uses REACT few-shot prompt (2 demos from a pool of 20); rounds ≥2 use a zero-shot SFT XML format.
- **Solution filtering (Algorithm 3)** before adding to the SFT set: dedup; **discard solutions where code errors persist across ALL steps yet answer is correct** (classic hallucination); keep incorrect solutions with little processing (value model needs to see diverse failures); tier correct solutions into Level-1 (predicted answer == a code output), Level-2 (all code correct), Level-3 (remaining) and prefer higher tiers.

## Inference strategies (value model steers policy)
- **MCTS inference (Algorithm 1):** set λ=0 (always trust the value model, even at terminals — can't verify correctness at test time). Build a full tree (N=40 sims, ≤5 children, temp 0.6), then top-down select by max child Q-value; re-rank children, keep top-B1.
- **Step-level Beam Search / SBS (Algorithm 2):** the production-friendly approximation. **Drops the backup op** and does not build the full tree. Beam sizes B1 (paths kept) and B2 (expansions per node). At each step: for each of B1 candidates, sample B2 next-steps in parallel; score each `s_{t+1}` by the **direct value prediction V(s_{t+1})** (not converged Q); keep top-B1. B1=1 gives streaming step-by-step output — a fast MCTS approximation.

## Results / benchmarks
- Base: **DeepSeekMath-Base-7B**, trained on only **15k Q–A pairs (GSM8K+MATH), 0 process annotations**. 10 trees/question/round, ≤4 correct + ≤4 incorrect solutions sampled, ~1:1 pos:neg, 57k–59k positives/round.
- Main results (GSM8K / MATH / GaoKao2023 / OCW):
  - Base + our 2-shot prompt (greedy): 59.7 / 33.2 / 21.9 / 9.2
  - + AlphaMath K=3 (greedy policy only): 73.5 / 53.6 / 40.5 / 26.1
  - + SBS (B1=1): 81.1 / 62.8 / 46.2 / 30.5
  - + SBS (B1=3): **84.1 / 66.3 / 51.4 / 33.1**
  - + MCTS (B1=1): 83.2 / 64.0 / 48.4 / 33.8
  - For reference DeepSeekMath-Instruct-7B (776k GPT-4+human seed): 83.7 / 57.4 / 43.9 / 18.0 — AlphaMath beats it on MATH/GK/OCW with **zero** process data.
- Efficiency on MATH (Table 3): Greedy 53.62% @1.6s/3.10 steps; Maj@5 61.84% @2.9s; SBS(B1=1) 62.80% @3.1s; SBS(B1=3) 66.30% @2.3s; MCTS(B1=1) 64.02% @**10.1s**/3.76 steps. SBS achieves MCTS-level accuracy far cheaper; larger B1 counterintuitively *reduces* wall-time (fewer avg steps).
- Generality (Table 4): Llama3-base +AlphaMath ~+20 pts avg; MARIO SFT model (K=2) +SBS reaches 88.3/68.6/54.1/42.3, competitive with / beating GPT-4.
- Value-model behavior (Sec 4.5, Fig 5/8): Q-value distribution for correct solutions skews to +1; incorrect skews to −1 but crosses 0 (incorrect solutions can contain correct intermediate steps — their Q gets bumped toward 1 by backup). Value model cleanly separates correct/incorrect, explaining SBS >> greedy.
- Temperature (Sec 4.7): SBS strongly benefits from temp≈1.0 (needs solution diversity for the value model to pick from); greedy stays at temp 0. B1=3 always > B1=1.
- Error analysis (100 MATH failures): 53% numerical (often Python precision — sympy vs float), 45% logical (ignoring a constraint), 2% other (ambiguous/wrong gold answer).

## Novel vs SOTA-2026
The durable, transferable ideas: (1) **MCTS Q-values as free process supervision** — turning outcome-only reward into dense step-level targets with no PRM annotation; this is now a foundational pattern (echoed by later PRM-free work and by DeepSeekMath-V2's meta-verifier), but AlphaMath is a clean, minimal instance. (2) **Two-heads-one-backbone policy+value** sharing parameters, value read at `</step>` — cheap, no separate reward model, no rollouts at eval. (3) **Step-level Beam Search** as a value-guided, backup-free, streaming-friendly MCTS approximation that matches MCTS accuracy at ~3× less compute. By 2026 standards the outcome-reward-only ceiling ("almost zero", still needs gold answers) and the small 15k dataset are limitations the authors flag; frontier systems add trained meta-verifiers and RL, but AlphaMath's derive-process-from-search recipe remains the conceptual backbone of PRM-free step supervision.

## Adopt-relevance to Theoremata — specific and actionable

- **Graded reward `R = R_format·R_score·R_meta` + flywheel (direct, highest-value).** AlphaMath gives a concrete way to manufacture a *step-level* `R_score` signal without any grader annotation: run MCTS over our proof-DAG, back-propagate terminal verify-outcomes (Lean/Rocq/Isabelle gate = the ±1 reward, which is *sounder* than answer-equivalence), and fit a value head to the converged Q̂. **What we likely already do:** outcome reward from the live gate; expert-iteration collection. **Real gap:** deriving *dense per-node* Q-value targets from search and training a value head on them — this is exactly the "process without process" trick and would give our meta-verifier/reward dense supervision that today probably comes only from terminal outcomes.
- **MCGS driver (direct).** Our MCTS graph-search can adopt (a) the PUCT prior = exp of averaged token log-prob of a step (cheap, no extra forward pass), (b) the `λ = I_terminal` evaluation blend (use the true gate result at terminals, value-model prediction elsewhere) — well-suited to us because a formal gate *is* a verifiable terminal, exactly the shallow-tree/terminal-reachable regime AlphaMath exploits. (c) The **SBS** algorithm as a production inference mode: backup-free, streaming, B1/B2 tunable, value-model-steered — a cheap alternative to full MCGS when we need latency.
- **Value head sharing the generator backbone.** Instead of a separate meta-verifier model, append a `tanh` value head reading the last token of each proof step. Cheaper to serve, and trained jointly via the Eq. 6 multi-task loss (NLL on verified proofs + MSE value regression on both verified and refuted proofs). Note: we may *prefer* a separate meta-verifier (DeepSeekMath-V2 direction) for capacity; AlphaMath shows the shared-head option is viable and cheap.
- **Solution filtering (Algorithm 3) for flywheel data hygiene.** Directly portable: dedup; **drop "all-steps-errored but answer-correct" traces as hallucinations** (for us: proofs where every DDAR/tactic step failed yet the goal was marked closed — a soundness red flag); keep diverse *failures* to train the meta-verifier's negative class; tier positives by internal consistency and prefer higher tiers. This is a ready-made recipe for curating expert-iteration batches.
- **Falsify-before-prove alignment.** Their outcome reward relies on a gold answer; our formal gate is a stronger terminal oracle. This means the "really from zero" limitation they flag (Appendix A.1 — still need gold answers) is *less* binding for us: a verified proof is self-certifying, so our flywheel can in principle mint reward without any gold-answer key, using verifier acceptance as the terminal signal.

## Verbatim-worthy details (formulas, hyperparameters, algorithms)

- **PUCT selection (Eq. 3):** `a_t = argmax_{a∈T_k} [ Q̂(s_t,a) + c_puct · π(a|s_t) · √(N_parent(a)) / (1 + N(s_t,a)) ]`, with prior `π(a|s_t) = exp( (1/|a|) Σ_j log π(a_j|a_{<j},s_t) )`.
- **Leaf evaluation (Eq. 4):** `V̂(s_t)^(i) = (1−λ)·V_k(s_t) + λ·r(a_{≥t}^(i), s_{>t}^(i) | s_t)`, `λ = I_terminal(s_t)`.
- **Backup:** `N(s,a) ← N(s,a)+1`; `Q̂(s,a) ← (1/N(s,a)) Σ_{j=1..i} I_{s,a∈s_t} V̂(s_t)^(j)`.
- **Value target (Eq. 5):** `V(s_{t+1}) = Q̂(s_t, a_t)` (non-terminal; reward 0).
- **Training loss (Eq. 6):** `argmin_{π,V} −log π(x+|q) + β·[ Σ_{t=1..T(x+)} (V(s_t)−V̂(s_t))² + Σ_{t=1..T(x−)} (V(s_t)−V̂(s_t))² ]`.
- **MC baseline (Eq. 2, what MCTS replaces):** `V(s_t) = (1/N) Σ_i r(a_{≥t}^(i), s_{>t}^(i)|s_t)`; regression loss `L_V(s) = (V(s) − V̄(s))²`.
- **Key hyperparameters (Table 6):** `c_puct = 1.25`; K = 3 (2 for MARIO SFT); value-loss weight β = 0.1 (0.0005 for Llama3); B1 ∈ {1,3}; **B2 = 5**; **simulations N = 40**; temperature ∈ {0.6, 1.0, 1.2}; **max depth T = 8**; batch 1024; AdamW; lr 4e-5; cosine schedule; warmup 0.03; 10 epochs; weight decay 0. Data-gen: 10 trees per (Q,A), ≤5 children/node.
- **Step XML format (Appendix C.1):**
  - C-step (code): `<step>\n<p>\n{textual analysis}\n</p>\n<code>\n{code}\n</code>\n<p>\n{code output}\n</p>\n</step>`
  - A-step (answer): `<step>\n<p>\n{textual analysis}\n</p>\n<p>\nFinal Answer:{answer}\n</p>\n</step>`
  - Value read from last token, usually `</step>`.
- **SBS (Algorithm 2), core loop:** keep C=[s0]×B1; per step, for each s_t sample B2 actions in parallel, score children by direct `V(s_{t+1})`, add to max-heap C_{t+1}, keep Top-B1; return Top-1. No backup, no full tree.
- **Solution-filtering (Algorithm 3):** dedup → drop code-error-in-all-steps → incorrect: keep as-is → correct: Level-1 if any code output equals predicted answer; else Level-2 if all code correct; else Level-3.
- **Env:** 8×A100, Python 3.11, PyTorch 2.1.2, customized LlamaFactory + vLLM, DeepSpeed ZeRO-2, FlashAttention-2.
