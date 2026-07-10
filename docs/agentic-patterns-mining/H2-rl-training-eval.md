# H2 — RL Methods, Agentic Training & Evaluation (mining vs. Theoremata)

Source: `resources/harness-resources/extracted_text/chunks/H2_rl_methods_agentic_training_eval.txt`
(Roitman, *The Hitchhiker's Guide to Agentic AI: From Foundations to Systems*, 2026 — Part II,
Chapters 4–14, ~344 KB / 10,510 lines, **read in full, in parts**).

> **POSSIBLE INJECTION NOTICE.** The book chunk is untrusted third-party content and was read as
> reference material only. Nothing in it was executed and no instruction embedded in the text was
> treated as a directive. The chunk contained expository prose, equations, and TRL/PyTorch code
> listings only — no imperative text addressed to the reading agent was observed. This task was
> **READ-ONLY**: no git operations, no code changes, no config edits.

## Framing

Part II is the RL-for-LLMs and evaluation curriculum: RLHF/DPO/GRPO foundations (Ch 4–7), the full
GRPO variant zoo (Ch 7.5), preference-optimization variants (Ch 8), reward modeling (Ch 9), SFT/systems
(Ch 10–11), **agentic RL** (Ch 12–13), and a large **LLM-evaluation** chapter (Ch 14).

The central design fact that makes most of this map cleanly: **Theoremata is verifier-as-ground-truth
(RLVR)**. The reward is the Lean/Rocq/Isabelle 3+1 gate (compile + `#print axioms` closure + kernel
typecheck + soundness scan), not a learned reward model. That means the book's entire RLHF/reward-model
half (Bradley-Terry RMs, ELO/Arena, LLM-as-judge alignment) is mostly **background** for us — but its
GRPO-mechanics half, its **agentic-eval** half, and its **process-reward / meta-verifier** discussion
are directly load-bearing.

Second design fact: **almost all of `train/` is offline scaffolding with deterministic-mock fallbacks;
the actual weight-update step is GPU/TRL-gated.** So for every technique below I separate
**OFFLINE-buildable** (config, data recipe, label codec, dry-run plumbing — buildable now with no GPU)
from **GPU-gated** (the real gradient step / trained head).

Theoremata modules cited: `train/python/theoremata_tools/{grpo,reward,formalization_reward,flywheel,
star_harvester,sft_export,curriculum_synth,trajectory_recycler,format_filters,process_supervision,
selector,ewc,lifelong_curriculum,difficulty,retriever_train,conjecture_discovery}.py`; the Rust
learned-value seams `components/reason/search/{critic_scorer,distance_critic,preference_pairs,
process_reward}.rs`; eval `eval/python/theoremata_tools/{eval_harness,grader,proof_grader,
proof_calibration,evolve}.py` + `benchmarks/{registry,graders,loaders,...}.py`.

---

## (a) Training techniques — GRPO / expert-iteration / RLHF / DPO / process-reward vs. our train stack

| Technique | Book definition (§) | Theoremata status | Where / what's missing | Buildable-now vs GPU-gated |
|---|---|---|---|---|
| **RLVR — RL from Verifiable Rewards** (§4.1, §9.5) | Reward = deterministic verifier (answer/test correctness), not a learned RM; the DeepSeek-R1 recipe. | **HAVE (core identity).** `grpo.py` `reward_from_verifier` = "1.0 iff Lean accepts"; `reward.py` `correctness_reward` = compiled ∧ axioms_ok. Verifier IS the reward. | This is our foundation; the formal gate is a *stronger* signal than R1's string-match. | Offline plumbing built; real rollouts GPU-gated. |
| **GRPO base** (§7.1–7.4) | Critic-free; group-normalize G rewards `Â=(r−µ)/σ`, clipped surrogate. | **HAVE.** `grpo.py` `grpo_config` (num_generations=G=8), Goldilocks filter = drop zero-variance groups; `dry_run_grpo` exercises the full plumbing minus the model. | — | Config+dry-run offline; gradient step GPU-gated (`train(dry_run=False)` lazily imports TRL). |
| **DAPO** (5 fixes: clip-higher, token-level loss, overlong filter/mask, soft-overlong, dynamic sampling) (§7.5.2) | Asymmetric clip `ε_high>ε`; divide loss by tokens; mask truncated; resample zero-variance groups. | **HAVE (documented knobs).** `grpo_config` sets `epsilon_high=0.28`, `loss_type="dapo"`, `mask_truncated_completions`, `overlong_filter`; Goldilocks = dynamic sampling. | Soft-overlong punishment (smooth length penalty) not represented; only hard mask. | Knobs are config-only (offline); enforcement GPU-gated by TRL. |
| **Temperature annealing** (DeepMath) | Hot→cold sampling schedule. | **HAVE.** `linear_temperature` 1.2→0.7, reported in dry-run. | — | Offline. |
| **GSPO** — sequence-level IS (geometric-mean ratio) (§7.5.3) | Clip one scalar per sequence; correct for off-policy `steps_per_generation>1`. | **GAP.** Not in config. | Add `importance_sampling_level="sequence"` support + doc; matters only if we reuse batches off-policy. | Config-buildable now; effect GPU-gated. |
| **Dr. GRPO** — debiased token weighting (§7.5.4) | Down-weight tokens the ref model already assigns high prob (`1−π_ref`). | **GAP.** | Needs a ref model at train time; note as a knob. | GPU-gated (needs ref logprobs). |
| **2-GRPO** — G=2 ≈ G=16 (§7.5.5) | Contrastive DPO-like signal; 4–6× end-to-end speedup, no accuracy loss on GSM8K/MATH/code. | **GAP (high-value, cheap).** `grpo_config` hardcodes `num_generations=8`. | Expose G=2 recipe as a compute-saving preset. **Directly relevant**: proof rollouts are our dominant cost. | Config-buildable now. |
| **SAPO / VESPO / DPPO / TIS-MIS / ScaleRL-CISPO / GDPO / GOPO** (§7.5.6–7.5.12) | Family of loss/IS refinements: smooth gates, variational kernel, divergence-clip, **vLLM logprob-mismatch correction**, batch-level scaling, per-reward normalization, ordinal-only advantages. | **MOSTLY GAP.** `reward.py` combines correctness+tool+format additively — the exact **multi-reward advantage-collapse** GDPO warns about. | **TIS/MIS** is a real production hazard (vLLM≠train logprobs silently biases gradients) — flag for whenever we go live. **GDPO** (`normalize_then_sum`) applies the moment we use >1 reward channel. **GOPO** irrelevant (we have verifiable, not RM, rewards). | GDPO aggregation is config; TIS/MIS is GPU/vLLM-gated. |
| **Expert iteration / STaR** (§12.3.1, §12.5.1) | Sample → keep correct traces → SFT → repeat; rationalization conditions on the answer for failures. | **HAVE.** `flywheel.py` (auto-label loop), `star_harvester.py` (verified traces→SFT rows), `sft_export.py` (`star_dataset` rejection-sampling + `rationalize` = STaR rationalization on a provided sketch). This is a faithful STaR/RFT pipeline. | V-STaR's *verifier-model* filtering of lucky-but-wrong traces is partially covered by our formal gate (no lucky passes) + `format_filters` snippet-coverage. | Data shaping offline; SFT step GPU-gated. |
| **Reflexion / verbal RL** (§12.3.1, §12.5.2) | Store NL self-critique in episodic memory; retry with reflections injected; no weight update. | **PARTIAL (adjacent).** Reflection/critique lives in the Rust reason/critique layer, not `train/`. `trajectory_recycler` mines *failed* attempts, but for curriculum (new conjectures), not in-context reflection memory. | No episodic-reflection buffer feeding retries. Cross-ref A2 mining. | Offline (prompt/memory), no training. |
| **Rejection-Sampling FT (RFT)** (§8, §12) | Train only on best-of-N successful traces. | **HAVE.** `sft_export.star_dataset` + `format_filters` (drop below-quantile / all-fail-group rollouts, tactic-block presence, snippet coverage — Kimina curation). | — | Offline data shaping. |
| **Process Reward Models (PRM) + automatic annotation** (§9.4) | Score each reasoning step; auto-annotate via MC rollouts / LLM-judge / **formal verification per step**; PBRS keeps optimal policy. | **HAVE (strong, differentiated).** `process_supervision.py` + Rust `process_reward.rs` = AlphaMath "process supervision without process": back-propagate the terminal ±1 through the proof tree, read each node's MC `Q=value_sum/visits` as dense value-head targets. This is exactly the book's "MC rollouts" auto-PRM, with our formal gate as the terminal oracle. | Value head is fit closed-form (ridge) offline; real `Linear→tanh` head is torch-gated. | Label extraction + closed-form fit offline; trained head GPU-gated. |
| **Critic / value-guided search** (implied by PRM + §12 LATS) | Trained `V(s)` guides best-first/PUCT. | **PARTIAL (seams in place, heads untrained).** `critic_scorer.rs`, `distance_critic.rs` (HunyuanProver coarse-to-fine distance codec), `preference_pairs.rs` (InternLM2.5 critic-DPO state pairs). All are **injectable seams with GPU-gated learned heads**; nothing yet feeds `V(s)` back into live node priority. | The wiring `V(s)→driver score` is the open integration. | Label codecs/pair extraction offline; heads + live wiring GPU-gated. |
| **PPO / RLHF 4-model loop** (§5) | Policy+ref+RM+value; GAE, clipped surrogate. | **N/A by design.** GRPO (critic-free) is the choice; no learned RM (verifier replaces it). | Nothing to build — book itself says GRPO dominates for reasoning. | — |
| **DPO / preference optimization** (§6, §8: Online-DPO, KTO, IPO, ORPO) (§6–8) | Closed-form RLHF-from-preferences; variants for on-policy/unpaired/noisy data. | **PARTIAL / mostly N/A.** We are RLVR, not preference-aligned. BUT `preference_pairs.rs` + `best_first.rs` DO mine `(state, win, lose)` DPO-style pairs for policy AND critic. | We mine pairs but don't run a DPO objective; if we ever add a **critic-DPO** head, KTO (unpaired binary = pass/fail!) is a natural fit for our verdicts. | Pair mining offline; DPO/KTO step GPU-gated. |
| **Label-free autoformalization reward (SC∧CC)** — *no book analog* | Book has no label-free formalization reward. | **HAVE (beyond book).** `formalization_reward.py` (FormaRL SC∧CC + anti-hack: trivial-stub, NL-echo; pass@k specificity guard). | This is a Theoremata capability the book does not cover. | Offline defaults; real SC=Lean compile / CC=LLM judge GPU/toolchain-gated. |
| **Meta-verifier reward** (DeepSeek-Math-V2) — *sharper than book's RM* | Book §9 covers RM training + reward hacking generally. | **HAVE (beyond book depth).** `reward.py` `verifier_reward` = `R_format·R_score·R_meta`, `generator_self_verify_reward` = `R_format·(α·R_Y+β·R_Z)`, `flywheel.py` n-verify + m-meta-verify majority auto-labeling. Directly attacks the "hallucinate fake issues yet score right" exploit. | — | Reward math offline; the verifier/meta LLM GPU-gated. |
| **Continual learning — EWC** (LeanAgent) | (not in this chunk; adjacent) | **HAVE.** `ewc.py` Fisher-weighted anchor penalty, torch-free math, λ=0 default. | — | Offline math; real tensors GPU-gated. |
| **Curriculum / difficulty** (§12.6.8 curriculum levels) | Easy→hard progression; keep 10–20% easy to avoid forgetting; advance at >70% success. | **HAVE.** `difficulty.py` (33/67-pct buckets, easiest-first), `lifelong_curriculum.py` (repo binning, round-robin, resumable), `curriculum_synth.py` (subgoal densification), `trajectory_recycler.py` (failure→conjecture). | The book's *dynamic* "advance level at >70% success" gate is a policy we could add on top of the static buckets. | Offline. |
| **Multi-objective reward combination** (§9.6) | weighted-sum / **normalize-then-sum (GDPO)** / lexicographic / constrained / Pareto. | **PARTIAL.** `reward.py` adds correctness + `tool_use_reward` + format via weighted sum (`tool_weight`, α/β blends). | Weighted-sum is the scale-sensitive option the book warns against; adopt **normalize-then-sum** if reward variances diverge. | Offline aggregation choice. |

**Net for (a):** we are **not missing the core training paradigm** — RLVR+GRPO+DAPO+STaR+RFT+PRM are all present, several (formal PRM, meta-verifier, SC∧CC) *ahead* of the book. The **newer named techniques we lack** are the GRPO refinements: **2-GRPO** (cheap, high-value, buildable-now), **GDPO normalize-then-sum** (buildable-now, applies to our multi-channel reward), and **TIS/MIS** (production-critical but GPU/vLLM-gated). GSPO/Dr.GRPO/SAPO/VESPO/CISPO are niche knobs, config-buildable but low priority given our binary verifiable reward.

---

## (b) Agentic EVAL methodology (trajectory-level, tool-use, agent benchmarks) vs. our eval — GAPS

The book's Ch 14.6 defines a **trajectory-level agent-eval vocabulary** that our eval stack, which is
**answer/artifact-level**, largely does not yet express.

| Book metric (§14.6/14.7) | Definition | Theoremata status | Where / missing |
|---|---|---|---|
| **Task Success Rate (TSR)** + graded | Fraction of tasks reaching goal state via deterministic oracle; graded ∈[0,1] for partial credit. | **HAVE.** `eval_harness.py` pass@k / majority@k over the six-axis grader; `benchmarks/registry.py` uniform `{is_solved,is_correct}`; `eval_execution.py` compiles/runs artifact→pass/fail. Our "oracle" = Lean gate. | Solid at the *outcome* level. |
| **pass@k unbiased estimator** (§14.5.6) | `1 − C(n−c,k)/C(n,k)`, n=200 samples. | **PARTIAL.** `reward.pass_at_k` is the "any-of-k passed" boolean (k=group), not the combinatorial unbiased estimator over n≫k samples. | **Buildable-now gap:** add the unbiased `pass_at_k(n,c,k)` estimator for reporting pass@1/10/100 from a large sample pool. |
| **Trajectory efficiency η = L\*/L_agent** (§14.6.2) + redundancy rate | Optimal vs actual action count; η=0 on failure. | **GAP.** We score the *final proof*, not the *search trajectory*'s efficiency. Rust search has step counts, but eval doesn't surface η / redundancy. | **Buildable-now:** emit proof-search step count vs a reference/shortest-known length as an efficiency axis (we already track difficulty as `exp(#steps)`). |
| **Tool-Use Accuracy (TUA)** (§14.6.3) | correct-tool ∧ valid-args ∧ right-moment / total calls. | **GAP (eval side).** `reward.py` gives a *training* tool-use shaping bonus (`used_tool`, legitimacy check) but there is **no eval metric** scoring falsifier/tactic/tool-call correctness at trajectory level. | **Buildable-now:** a TUA grader over logged tool calls (we already record `tool_calls` with ok/error). |
| **Multi-step reasoning / step accuracy (SRA)** via PRM (§14.6.4) | Fraction of reasoning steps correct, scored by PRM/human. | **PARTIAL.** `proof_grader.py` (ProofGrader `decompose_then_judge` / step-wise, error taxonomy: unjustified/gap/computation) is exactly step-level grading; `process_supervision` gives per-node Q. | Present but not wired as a headline **SRA** eval number; could aggregate proof_grader step verdicts into SRA. |
| **Agent benchmarks: SWE-bench / WebArena / AgentBench / ALFWorld** (§14.6.5–6, Tbl 14.4) | Real-task, execution-verified, trajectory benchmarks. | **N/A domain-wise** (we're formal math, not SWE/web) but **methodologically analogous.** Our `benchmarks/` registry (formalization, nl_answer, verified_programming, proof_grading, falsification tracks) is our SWE-bench-equivalent: execution/compile-verified. | No trajectory-length or recovery-rate reporting like ProdBench (§12.6.12: recovery rate, cross-app success). |
| **LLM-as-Judge: pointwise/pairwise/reference-guided, position-bias swap, verbosity bias, multi-judge panels, G-Eval prob-weighting** (§14.7) | Scalable judge with debiasing. | **PARTIAL.** `graders.py` has an LLM-judge *fallback* (mock-capable) only when symbolic parsing is inconclusive; `proof_grader.py` has `repeat_and_aggregate` / `reflect_and_revise` workflows. | **Missing debiasing discipline:** no position-swap augmentation, no multi-judge panel, no G-Eval logprob-weighted scoring. Buildable-now (prompt/aggregation logic) where a judge is used. |
| **Judge validation: Cohen's κ, Spearman ρ, Kendall τ, agreement>80%/κ>0.6** (§14.7.4) + **ECE calibration** (§14.3.1) | Validate judge vs human labels. | **HAVE (strong, differentiated).** `proof_calibration.py` = MAE/RMSE/bias, Pearson/Spearman/Kendall-τb, pairwise-ranking accuracy, bootstrap CIs, evaluator-disagreement, **verify-vs-solve gap**. This *is* the book's judge-validation layer, ahead of it. | — |
| **Ranking systems: ELO / Bradley-Terry / TrueSkill / Wilson CI** (§14.4) | Model leaderboards from pairwise battles. | **N/A / low priority.** We evaluate against a fixed oracle, not by ranking models in an arena. | Nothing to build unless we start comparing prover configs by pairwise battle. |
| **Generation metrics: BLEU/ROUGE/BERTScore/METEOR/EM/F1** (§14.5) | Reference-based text overlap. | **N/A mostly.** `grader.py` uses symbolic-equivalence (SymPy) EM, not BLEU/ROUGE. Correct choice for math. | EM/F1 present in spirit; text-overlap metrics irrelevant to proofs. |
| **Abstention** | (Theoremata SOTA-gap item; book covers refusal loosely via safety) | **PARTIAL.** `eval_harness` "refuses" non-test items (contamination control), and proof_grader can render fatal verdicts, but there is no first-class **answer-abstention** metric (model declines when unsure) as a scored eval axis. | Buildable-now: score abstention precision/recall against the formal verdict. |
| **Evaluation pitfalls: contamination (n-gram/canary/temporal), Goodhart/reward-hacking, prompt sensitivity, eval-deployment mismatch** (§14.8) | Systematic hygiene. | **PARTIAL→HAVE.** `eval_harness` enforces strict `usage_tag=="test"` (contamination control) + per-axis breakdown (no single blended scalar) + standard errors. Reward-hacking is actively defended in `formalization_reward` (trivial/echo) and `reward` (meta-verifier). | Missing explicit **canary-string / n-gram-overlap contamination detection** and prompt-variant robustness reporting. Buildable-now. |

**Net for (b):** our eval is **outcome-verified and calibration-rich** (proof_calibration is genuinely
ahead of the book), but it is **not trajectory-aware**. The concrete, **buildable-now** eval gaps are:
(1) **trajectory efficiency η + redundancy**, (2) **tool-use accuracy** as an eval metric, (3) the
**unbiased pass@k(n,c,k)** estimator, and (4) **LLM-judge debiasing** (position-swap, panels, G-Eval)
wherever the judge fallback fires.

---

## (c) Reward-model / verifier design vs. our meta-verifier + formalization reward

| Book concept (§) | Theoremata mapping |
|---|---|
| **Bradley-Terry RM, margin loss, reward centering, length-bias** (§9.1–9.3) | **N/A by design** — we have no learned scalar RM; the formal gate is the reward. Length-bias / reward-centering are RM pathologies we sidestep entirely. |
| **Rule-based RLVR rewards + pitfalls (format gaming, test leakage, timeout exploit, sparsity)** (§9.5) | **HAVE + defended.** `reward.correctness_reward` (verdict-driven), `format_reward` (marker gate), and anti-hack screens in `formalization_reward`. Test-leakage ≈ our contamination `usage_tag` gate. Sparsity is addressed by `process_supervision` dense Q-targets (PBRS-style, book §9.4). |
| **PRM > ORM for multi-step; auto-annotate by formal verification per step** (§9.4) | **HAVE (this is our sweet spot).** `process_reward.rs` + `process_supervision.py` manufacture step-level value targets from the terminal formal verdict via MC-Q backup — precisely the book's recommended "formal verification / MC rollout" PRM annotation, with a *hard* oracle. |
| **Reward hacking is inevitable; multi-level (format+content+semantic) verification** (§12.6.13) | **HAVE.** Meta-verifier (`R_meta`), SC∧CC anti-hacks (trivial-stub, NL-echo, pass@k specificity collapse), triviality screens. Multi-level = format gate × score × meta. |
| **Generator self-verify blend `R=R_format·(α·R_Y+β·R_Z)`** (DeepSeek-Math-V2) | **HAVE (beyond book).** `generator_self_verify_reward` (α=0.76,β=0.24) rewards *honest* self-evaluation, not claimed correctness. |
| **Listwise / Plackett-Luce rewards for GRPO groups** (§9.7) | **GAP (minor).** We rank groups by scalar verdict; no listwise RM. Low value under binary verifiable rewards, but PL could enrich the critic-DPO state ordering in `preference_pairs.rs`. Buildable-now (loss math) if a graded critic is wanted. |
| **Multi-objective: normalize-then-sum (GDPO)** (§9.6) | **PARTIAL** — see (a); our additive blend risks advantage collapse across correctness/tool/format channels. |

**Net for (c):** our reward/verifier design is a **strength**, not a gap — the formal gate + meta-verifier
+ formal-PRM triad is *stronger* than the book's learned-RM story and directly implements its best
recommendations (formal per-step annotation, multi-level anti-hacking, honest self-verification). The
only refinement worth importing is **normalize-then-sum** for the multi-channel reward, and possibly a
**listwise/PL** objective if the critic head goes graded.

---

## TOP 3 GAPS

1. **[BUILDABLE-NOW] Trajectory-level agentic eval axes.** Add **trajectory efficiency η = L\*/L_agent**,
   **redundancy rate**, **tool-use accuracy (TUA)**, and the **unbiased pass@k(n,c,k)** estimator to
   `eval_harness.py`. We already log search step counts and `tool_calls` with ok/error flags and track
   difficulty as `exp(#steps)`, so all inputs exist — this is pure offline plumbing and closes the
   biggest methodological gap (our eval is outcome-level, the book's is trajectory-level).

2. **[BUILDABLE-NOW] GRPO reward/aggregation upgrades: 2-GRPO preset + GDPO normalize-then-sum.**
   `grpo.py` hardcodes G=8; a **G=2 preset** (§7.5.5) buys ~4–6× end-to-end rollout savings with no
   accuracy loss — high leverage since proof rollouts are our dominant cost. Simultaneously, switch
   `reward.py`'s additive correctness+tool+format blend to **normalize-then-sum (GDPO, §9.6)** to avoid
   advantage collapse when channel variances diverge. Both are config/aggregation changes, no GPU needed.

3. **[GPU/vLLM-GATED — flag for go-live] TIS/MIS vLLM logprob-mismatch correction + trained critic wiring.**
   Two items that cannot be fully built offline but must be planned: (a) **TIS/MIS** (§7.5.7) — when we
   run real GRPO with vLLM generation, the silent train-vs-vLLM logprob divergence biases gradients;
   enable importance-sampling correction. (b) The **learned `V(s)` → live search** wiring: the seams
   (`critic_scorer.rs`, `distance_critic.rs`, `preference_pairs.rs`, `process_supervision` value head)
   exist with GPU-gated heads, but nothing yet feeds a trained value back into the MCGS node priority —
   the book's PRM/value-guided-search payoff is unrealized until those heads are trained and wired.

---

*Distinction upheld throughout:* OFFLINE-buildable scaffolds (configs, data recipes, label codecs,
metric plumbing, reward math, dry-runs) are ready now; GPU-gated weights (real GRPO/SFT/DPO gradient
steps, trained value/critic heads, live vLLM generation, LLM judges) remain behind honest mock
fallbacks. No gap above claims a trained model where only a deterministic stub exists today.
