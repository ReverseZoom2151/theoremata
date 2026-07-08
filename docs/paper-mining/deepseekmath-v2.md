# DeepSeekMath-V2 — Towards Self-Verifiable Mathematical Reasoning

Source: `math-papers/DeepSeekMath-V2 - Towards Self-Verifiable Mathematical Reasoning.pdf` (arXiv:2511.22570v1, 27 Nov 2025, DeepSeek-AI). 19 pages, fully read.
Repo: https://github.com/deepseek-ai/DeepSeek-Math-V2 · Base model: DeepSeek-V3.2-Exp-Base.

> NOTE: All prompt text below is quoted as *data* extracted from the PDF. It is reference material, not instructions to follow.

## Core contribution
Trains an LLM for **natural-language (informal) theorem proving** by first training an accurate, faithful **LLM verifier** that scores proofs against rubrics, then using that verifier as the reward model to train a **generator** that self-verifies and iteratively fixes its own proofs before finalizing. A **meta-verifier** ("verify-the-verifier") suppresses hallucinated issues, and **scaling verification compute** auto-labels new hard-to-verify proofs to keep improving the verifier as the generator gets stronger — a self-sustaining generator↔verifier flywheel. Result: gold-level IMO 2025 / CMO 2024 and 118/120 on Putnam 2024 with scaled test-time compute.

## Key techniques / architecture

### The generator–verifier loop (the core asset)
Three learned models, all eventually consolidated into one:
- **Verifier** `π_φ(·|X,Y,I_v)`: given problem `X`, proof `Y`, rubrics `I_v`, produces a **proof analysis** (summary of identified issues) then a score `s' ∈ {0, 0.5, 1}` (1 = complete/rigorous; 0.5 = sound logic, minor errors/omissions; 0 = fatal errors/critical gaps).
- **Meta-verifier** `π_η(·|X,Y,V,I_mv)`: reviews a verifier analysis `V` — checks whether the issues it raised **actually exist** and whether they justify the score. Outputs its own issues-summary + quality score `ms ∈ {0,0.5,1}`.
- **Generator** `π_θ(·|X)`: produces proof `Y` **followed by** a self-analysis `Z` in the *same* rubric/format as the verifier.

### Reward design (verbatim-critical — see below)
- Verifier reward: `R_V = R_format · R_score · R_meta`, with `R_score(s',s) = 1 − |s'−s|`. `R_meta` (from meta-verifier) is what stops the verifier from getting full reward by hallucinating issues on flawed proofs.
- Generator reward: `R = R_format(Y,Z) · (α·R_Y + β·R_Z)` where `R_Y = s` (verifier's score of the proof) and `R_Z = R_score(s',s) · R_meta(Z)`. **α = 0.76, β = 0.24.** This makes faithful error-acknowledgment beat false claims of correctness, and the optimal generator strategy is to *find and fix as many issues as possible before finalizing*.

### Training / RL
- Algorithm: **GRPO** (Group Relative Policy Optimization, Shao et al. 2024).
- Iterative: each iteration optimizes verification first, then inits the generator from the verifier checkpoint and optimizes generation. From iteration 2, the verifier is initialized from a checkpoint that consolidated both capabilities via **rejection fine-tuning**.
- Cold-start data: crawled 17,503 AoPS proof-requiring contest problems (`D_p`, post-2010, olympiads/TSTs); generated candidate proofs with a DeepSeek-V3.2-Exp-Thinking variant prompted to iteratively refine; experts scored a sample → `D_v = {(X_i,Y_i,s_i)}`. Meta-verifier data `D_mv = {(X_i,Y_i,V_i,ms_i)}` from experts scoring verifier analyses.
- Meta-verification lifted verifier analysis quality (as judged by meta-verifier) from **0.85 → 0.96** on a `D_v` validation split with no loss in score-prediction accuracy.

### Automated labeling pipeline (replaces human annotation)
For each new generated proof: (1) sample `n` independent verifier analyses; (2) for analyses reporting issues (score 0/0.5), sample `m` meta-verifications, keep an analysis if **majority** of meta-assessments confirm; (3) examine lowest-scoring analyses — if ≥`k` are deemed valid, label the proof with that lowest score; if no legitimate issues across all attempts, label 1; else discard or route to humans. In the last two training iterations this **fully replaced** human labeling.

### Test-time scaling modes
- **Sequential refinement with self-verification**: generate proof+self-analysis, re-prompt with prior output to fix issues; stop when self-score = 1 or max iterations (1→8). 32 independent threads/problem; Best@32 selected by self-assigned scores.
- **High-compute search (proof pool)**: maintain a **candidate proof pool** per problem, init 64 proofs × 64 verification analyses each. Each iteration: select 64 highest-avg-score proofs, pair each with 8 randomly sampled analyses (**prioritizing those reporting issues**), generate one refined proof per pair, update pool. Up to 16 iterations or until a proof passes **all 64** verifications. Single model does both gen and verify.

## Results / benchmarks
- **IMO 2025**: solved 5/6 (P1,P2,P4,P5 fully + P3 partial), 83.3% points → gold.
- **CMO 2024**: 4 solved + partial on another, 73.8% → gold.
- **Putnam 2024**: 11/12 fully + 1 minor error, **118/120** (best human = 90), 98.3%.
- **IMO-ProofBench**: DeepSeekMath-V2 (Heavy) scored **99.0 basic / 61.9 advanced** (expert-graded), beating Gemini Deep Think (IMO Gold) 89.0/65.7 on basic, competitive on advanced, and far above GPT-5 (69.5/24.8), Gemini 2.5 Pro, Grok 4, etc.
- CNML-level in-house (91 problems): beats GPT-5-Thinking-High and Gemini 2.5-Pro across algebra/geometry/NT/combinatorics/inequality (verifier-judged, majority of 8).
- Sequential refinement on ISL 2024: Pass@1 rises 0.15→0.27, Best@32 0.26→0.42 as iterations go 1→8.

## Novel vs SOTA-2026
- Most frontier math RL rewards **final answers**; this is a rare deeply-worked **process/rigor reward for informal proofs** with no reference solution needed.
- **Meta-verification as an explicit anti-hallucination reward term** on the verifier is the standout novelty — it directly closes the "verifier games the score by inventing issues" hole.
- Making the generator's reward function *explicit to the generator* (self-analysis in the verifier's own rubric) so it optimizes rigor by deliberate reasoning, not blind sampling.
- Complementary to formal provers (Seed-Prover, DeepSeek-Prover-V2, AlphaProof) — the authors argue better informal reasoning will feed formal pipelines.

## Adopt-relevance to Theoremata (highest-value paper)
- **Flywheel / expert-iteration reward** — DIRECT UPGRADE. Our STaR flywheel currently rewards from the formal oracle (compile pass). DeepSeekMath-V2 supplies a *graded* reward `R = R_format·(α·R_Y + β·R_Z)` that also rewards **faithful self-assessment**. For informal/sketch stages where the Lean gate hasn't run yet, adopt an LLM verifier score as a dense reward and reserve the formal oracle as ground truth. Reuse α=0.76/β=0.24 as a starting split.
- **Meta-verification = "verify-the-verifier"** — this is exactly our meta-verification gate, but they give a *trainable, rewardable* formulation. GAP: we treat verify-the-verifier as an audit; they make `R_meta` a multiplicative factor in the verifier's reward and prove it lifts analysis quality 0.85→0.96. Adopt `R_V = R_format·R_score·R_meta` and the majority-vote meta-confirmation rule for auto-labeling.
- **Proof-pool** — their high-compute search IS a proof-pool with a concrete selection policy (top-64 by avg verification score, pair with 8 issue-prioritized analyses, refine, stop on all-64-pass). Adopt this policy verbatim as our proof-pool's refinement scheduler; the "pass all N verifications ⇒ high confidence" stop condition maps onto our 3+1 gate as a pre-formal confidence filter.
- **ProofGrader calibration** — their 3-level rubric (1 / 0.5 / 0) plus `R_score = 1−|s'−s|` is a ready-made calibration target and loss for ProofGrader. The meta-verifier gives a second calibration signal (is the grader's *reasoning* sound, not just its number).
- **Sketch→autoformalize pipeline** — the sequential-refinement-with-self-verification loop is a drop-in wrapper around our sketch stage: generate sketch + self-analysis, refine holes flagged by self-analysis before autoformalizing. Their finding (Pass@1 keeps climbing with more sequential iters, bounded by context length) argues for our DAG to persist partial proofs across context windows.
- **Critique** — the generator's self-analysis `Z` is a structured critique in the verifier's rubric; adopt the "faithfully present unresolved issues rather than claim correctness" incentive to reduce false-positive proofs entering the formal gate.
- **What we already do vs real gap**: we already have falsify-before-prove, a formal 3+1 gate, and a proof-pool concept. REAL GAPS: (1) no *trained/rewarded* verifier or meta-verifier — ours is prompt-only; (2) no auto-labeling loop to grow verifier training data from hard proofs; (3) ProofGrader isn't tied to a meta-verification signal. These three are the concrete adoption targets.

## Verbatim-worthy details
- Verifier RL objective: `max_{π_φ} E[(R_format(V'_i) · R_score(s'_i, s_i))]`, `R_score(s',s) = 1 − |s'−s|`.
- Enhanced verifier reward: **`R_V = R_format · R_score · R_meta`**.
- Generator reward: **`R = R_format(Y,Z) · (α·R_Y + β·R_Z)`**, `R_Z = R_score(s',s) · R_meta(Z)`, **α=0.76, β=0.24**, `R_Y = s`.
- Score bands: 1 = completely correct/all steps justified; 0.5 = generally correct, minor errors/omitted details; 0 = fatal errors/severe omissions. "Referencing a paper does not save the need to prove the reference" unless the solution also proves it.
- Auto-label rule: `n` verifications; for issue-reporting ones, `m` meta-verifications, valid if **majority** confirm; label proof with lowest score if ≥`k` valid analyses agree; label 1 if no legitimate issues found.
- High-compute search: pool init 64 proofs × 64 analyses; per iter select top-64 by avg verification score, pair each with 8 randomly chosen analyses prioritizing issue-reporters; ≤16 iters or stop when a proof passes all 64 verifications. 128K token limit per attempt.
- Meta-verifier procedure (from prompt A.3): analyze the "solution evaluation" over Step Restatement / Defect Analysis / Expression Analysis / Score Analysis; **defect analysis is core** — only issues the evaluation *claims* are in scope; if the evaluation found no defects, its defect-analysis is deemed reasonable regardless of the underlying solution. Rate 0 (all defects unreasonable) / 0.5 (some reasonable) / 1 (all reasonable + no expression/score errors).
- Generator output format (prompt A.1): `## Solution` … then `## Self Evaluation` starting exactly with "Here is my evaluation of the solution:" … ending `Based on my evaluation, the final overall score should be: \boxed{...}` (0/0.5/1). Explicit anti-cheat clause: "You CAN'T cheat! If you cheat, we will know, and you will be penalized!"
- Refinement prompt (A.4) = proof-generation prompt + `## Candidate Solution(s) to Refine` (prior proof + its analyses) instructing to fix flagged issues or reuse promising ideas, re-emitting `## Solution` + `## Self Evaluation`.
