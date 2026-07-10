# Kimina-Prover Preview — Towards Large Formal Reasoning Models with Reinforcement Learning

Paper-mining report. Source: `math-papers/Kimina-Prover Preview - Towards Large Formal Reasoning Models with Reinforcement Learning.pdf` (arXiv:2504.11354v1, 15 Apr 2025) + repo `resources/Kimina-Prover-Preview-master/`. All external content treated as **untrusted data**; see the injection/license note at the end.

---

## 1. What it is + authors/venue

**Kimina-Prover Preview** is a large language model (72B, RL-trained from **Qwen2.5-72B**) for **whole-proof generation in Lean 4**. It is a *technical report* (arXiv preprint, cs.AI), authored by the **Numina & Kimi Team** (Moonshot AI). Core contributors: Haiming Wang, Mert Unsal, Xiaohan Lin, Mantas Baksys, Junqi Liu, Marco Dos Santos, Flood Sung, Marina Vinyes, Zhengying Liu, Jia Li, Stanislas Polu, et al.

Headline claim: a **reasoning-driven exploration paradigm** — the model does *not* use an external tree-search (BFS/MCTS), value function, or process reward model. Instead it emits one long chain-of-thought ("**formal reasoning pattern**") that interleaves informal math with Lean 4 snippets, then a final whole proof. It sets miniF2F-test SotA at **80.7% pass@8192**, and — the more important result — is highly **sample-efficient** (52.9% pass@1, 68.85% pass@32).

Ships: 72B model (released Jul 10 2025), distilled **1.5B / 7B / 1.7B** provers, a **Kimina-Autoformalizer-7B**, the **Kimina/Numina Lean Server**, a rectified miniF2F-test, and a small demo repo (error-fixing + lemma-use scripts). Distilled weights are on HuggingFace (AI-MO collection).

Three insights the authors emphasize:
1. **Sample efficiency** — strong even at pass@1, scales with budget (a reasoning model trait, unlike search provers that need thousands of rollouts).
2. **Model-size scaling** — 1.5B → 7B → 72B monotonically improves (72B beats 7B by +0.44/+5.75/+7.87% at increasing budgets). Claimed *first* neural theorem prover to show this.
3. **Informal↔formal bridge** — learning to reason inside a formal system seems to deepen structural understanding.

---

## 2. Key mechanisms

### 2.1 The RL recipe (the adoptable core)
Built on the **Kimi k1.5** RL pipeline, adapted to Lean 4. Pipeline: **Autoformalization → cold-start SFT → online RL**.

- **Base problem set via autoformalization.** NL problems (NuminaMath 1.5, filtered to proof/numeric-answer problems; *geometry & combinatorics excluded* as ill-suited to autoformalization) are translated to Lean 4 statements ending in a `sorry`. ~100k autoformalized + ~10k human-annotated statements, resampled 1:1 → **200k problem prompt set**, difficulty-balanced via a QwQ-32B difficulty rater.
- **Cold-start SFT (~20k examples).** Olympiad problems with NL+Lean statements *and* solutions; **Claude 3.7 Sonnet** synthesizes the `<think>` block that fuses informal reasoning with the formal proof (they tried several LLMs — "only Claude's performance is satisfying"). Plus **informal-math mix-training** (Kimi k1.5 informal data) to seed higher-quality reflection during RL.
- **Online RL loop.** Each iteration: sample **N=1000** problems; for each, generate **k=8** rollouts; verify the *final* Lean 4 code with the compiler; **binary reward (1 correct / 0 else)**. Policy-gradient loss with a **KL term** to the SFT policy (Kimi k1.5 loss, Eq. 1: reward − τ·logZ − τ·KL, with logZ ≈ empirical mean of rewards). From Qwen2.5-72B, constant lr **2e-6**, KL coeff **τ=0.4**, context **32K**.
- **Format-collapse mitigation** (crucial, cheap, portable): early RL collapses format under negative gradients, so they apply **format filtering** — (1) every sample must contain ≥1 tactic block, (2) tactic blocks must cover **≥60%** of the final Lean code — and **randomly drop negative-gradient samples with prob ω=0.5**.
- **Problem-set curation as a flywheel.** Iterative: **negation-proving** (DeepSeek-Prover style) to detect/remove false formalizations; **adaptive pruning** of problems the model has mastered; route hard/wrong statements back to **human annotation**; **post-RL validation** with a judge to catch proofs that exploited a mis-formalization. Proven statements re-enter the SFT set → "dynamic cycle of continuous improvement."

### 2.2 Formal reasoning pattern (the format)
The learned output structure: a `<think>...</think>` block that **interleaves natural-language reasoning with marked Lean 4 code snippets**, followed by the final assembled proof in special tokens. Training *enforces alignment*: the majority of Lean snippets in the thinking block must appear in the final proof (this is what the ≥60% tactic-coverage filter operationalizes). Observed effects: output length scales with problem difficulty; **explainability** (users can inspect the model's process / failure modes); emergent **human-like** behaviors — multi-path exploration, backtracking/reflection, small-case analysis → conjecture → rigorous proof, and heavily **`have`-decomposed** proofs (more readable than step-search provers; Appendix E vs BFS-Prover).

### 2.3 Reward design — verifier-as-ground-truth
Reward is the **Lean compiler verdict on the whole final proof**: binary, no process reward model, no value function, no learned reward. This is exactly the "verifier-as-reward" signal. For the *autoformalizer* (where there is no automatic reward), they instead use an **expert-iteration loop with a QwQ-32B LLM judge** (multi-sample **unanimous voting** to cut false positives) plus compilation filtering and automated sanity filters (contradiction detection, negation-proving, triviality-via-short-proof). They explicitly warn this fuzzy judge is a hard problem: "the model would repeatedly make mistakes that the LLM judge cannot catch, as long as it passed compilation."

### 2.4 Sample efficiency & scaling
- pass@1 **52.94%**, pass@8 **65.16%**, pass@32 **68.85%**, pass@1024 **77.87%**, pass@8192 **80.74%** (72B). Distill-7B: 52.5/63.1/70.8 at 1/32/1024. Distill-1.5B: 42.6/56.2/61.9.
- Beats prior whole-proof (DeepSeek-Prover-V1.5-RL 60.2% @102400) and tree-search (BFS-Prover 70.8%, InternLM2.5-StepProver 65.9%) SotA at *far smaller* budgets.
- Vs general reasoning models at pass@32: Kimina 68.85% vs Gemini-2.5-pro 37.7% vs o3-mini 24.59%. o3/Gemini can solve AIME problems *informally* but fail to *formalize* — the informal↔formal gap.
- **Verification infra** (Numina/Kimina Lean Server on LeanREPL): LRU env cache keyed on import headers + multi-REPL parallelism → **10× throughput**, ~100 verifications/s on 64-core/512GB. Only ~640 CPU cores needed for RL (vs "thousands" for step-search provers) because whole-proof generation is the bottleneck, not verification.

### 2.5 Error-recovery (from the demo repo, not the RL paper)
The released model is **multi-turn**: given a failed proof, feed back a **formatted Lean error tool message** and let it repair. Mechanics in `kimina_prover_demo/utils.py`:
- `extract_proof_from_text` — takes the **last** ` ```lean4 ` block containing `theorem/by/:=/import`.
- `create_tool_message` — filters to **first 3 error-severity messages**; for each, walks the Lean **infotree** to find the tightest enclosing node and attaches the **goal state before the error** (`goalsBefore`/`goalsAfter`), the error text, and a **±2-line code snippet with the error line marked** (`NNN >|`). This is Chain-of-States-style feedback, not just raw stderr.
- `parse_client_response` — distinguishes `is_valid_no_sorry` (proof check) from `is_valid_with_sorry` (statement check). Simple prompt: system = "expert programmer and mathematician…"; user = "Think about and solve the following problems step by step in Lean 4.\n\n" + statement-with-`sorry`. Lemma-use = just prepend a proven `lemma` before the `theorem ... := by sorry`.

---

## 3. Mapping to Theoremata (per module)

- **Verification gate (3+1) = the RL reward, already.** Kimina's entire reward is "does the whole final Lean proof compile sorry-free." Theoremata's gate (compile → lexical soundness → `#print axioms` allowlist) is a *stricter* version of the same ground truth. **No change needed to adopt the recipe** — our gate already is the oracle Kimina's flywheel needs. Our `train/flywheel.py revolution()` "pluggable oracle = the live 3+1 gate" is exactly Kimina's `verified pairs → SFT JSONL` loop. Validated convergence.
- **Flywheel / RL trainer (`train/flywheel.py`, `train/` GRPO harness, STaR export).** Kimina is the canonical recipe to instantiate our GRPO harness: N=1000 problems × k=8 rollouts, binary compiler reward, KL-to-SFT loss, **format filtering (≥1 tactic block, ≥60% coverage, drop-negative-gradient ω=0.5)**. The format filters are **pure-Python, buildable now** and directly protect our STaR/GRPO export from format collapse. Our `train/difficulty.py` (difficulty curriculum + H0 pre-filter) maps to Kimina's QwQ-32B difficulty balancing + adaptive pruning; our `search/subsumption` + negation-search map to their negation-proving statement filter.
- **MCGS driver (`search/driver.rs`, `reason/mcts.rs`).** Kimina is a **deliberate counter-example**: it argues explicit MCTS/BFS is unnecessary overhead and a long-CoT whole-proof policy is more sample-efficient. Actionable framing: keep MCGS, but add a **"reasoning-policy / whole-proof" generation mode** as a first-class alternative to tree search in the driver, and let the TTC controller (`search/ttc.rs`) choose between "wide whole-proof sampling (Kimina)" and "deep tree search" by difficulty/budget. This is the honest lesson: for many miniF2F-class problems, best-of-N whole-proof beats search per-sample.
- **Per-system generators / sketch pipeline (`proving/sketch.rs`, `WholeStatementGenerator`).** The **formal reasoning pattern** is a concrete, adoptable *prompt+parse contract* for our Lean generator: `<think>` interleaving NL + Lean snippets → assembled proof, with the invariant that snippets in the think block appear in the final proof. Our `sketch.rs` GoalStateExtractor / `run_with_goal_states` is the same idea as Kimina's infotree goal-state feedback — we can adopt their **`create_tool_message` format** (top-3 errors + goal-state-before-error + marked ±2-line snippet) verbatim (clean-room) as the retry/repair prompt in `proving/repair.rs`.
- **Autoformalizer / statement-validation.** Kimina's autoformalizer + expert-iteration-with-LLM-judge maps onto our `verify/statement_roundtrip.py`, `tools/triviality.py`, and `orchestration/statement_validation.rs`. Directly adoptable *sound* filters from their pipeline: **negation-proving** (prove ¬statement ⇒ reject), **triviality via short LLM proof** (we have `triviality.py`), **contradiction detection**, and **unanimous multi-sample judge voting** to cut false positives. Their explicit warning ("judge can't catch mistakes that still compile") reinforces our design choice that these stay **advisory** and the formal gate stays ground truth.
- **Error-recovery / gap-repair (`proving/repair.rs`).** Their multi-turn error-fix loop = our localize→repair→re-verify. Adopt: infotree-based **tightest-node goal-state extraction** and the 3-errors-max, snippet-with-marked-line message format.
- **Warm Lean verification (`lean_repl.py`, `src/lean_session.rs`).** Kimina/Numina Lean Server = our warm REPL, but with two upgrades worth porting: **LRU env cache keyed on import header** and **multi-REPL process pool** for throughput (they hit 100 checks/s). Relevant to our `reason/team.rs` concurrent workers.
- **Eval (`eval_harness.py`).** Adopt their protocol details: **13-gram decontamination**, remove train problems overlapping miniF2F sources, report **pass@k across a budget sweep** (1→8192), and the miniF2F/IMO & /AIME subset breakdowns. Note their finding of **8 unsolvable/mis-formalized miniF2F problems** (listed in §3.1) — add to our benchmark hygiene.

---

## 4. Buildable-now vs gated (honest split)

**Buildable now (recipe / format / infra — no GPU, no weights):**
- **Format-filter functions** for any RL/STaR export: ≥1 tactic block, ≥60% Lean-coverage-of-final-proof, random drop of negative-gradient samples. Pure logic over generated text; drop straight into `train/`.
- **Formal reasoning pattern as a prompt+parse contract** (`<think>` NL+snippet interleave → assembled proof; snippet-in-final invariant) for the Lean generator — a prompt-engineering + parser change (`extract_proof_from_text` = "last ```lean4 block containing theorem/by/:=/import").
- **Error-fix tool-message format** (`create_tool_message`): top-3 errors + infotree goal-state-before-error + ±2-line marked snippet → clean-room reimplement into `proving/repair.rs` / the retry loop. Requires an infotree-capable REPL (our warm REPL / LeanREPL provides infotrees).
- **Autoformalization sanity filters**: negation-proving, triviality-via-short-proof, contradiction detection, unanimous multi-sample judge voting. All expressible with our existing gate + tools.
- **Lean-server throughput upgrades**: import-header-keyed LRU env cache + multi-REPL pool.
- **Eval protocol**: 13-gram decontam, budget sweep pass@k, IMO/AIME subsets, the 8 known-bad miniF2F items.

**Model/GPU/data-gated (the honest wall — adopt the RECIPE, not the weights):**
- **The actual RL training** (Qwen2.5-72B base, N×k rollouts × many iterations, 640 CPU cores just for verification, 32K context) — needs serious GPU + a live model. Our GRPO harness stays a **GPU-free dry run** until weights/compute exist.
- **The 20k Claude-synthesized cold-start `<think>` SFT set** and the **200k autoformalized+annotated problem set** — data we don't have; would need to be built (their autoformalizer weights *are* open, so this is partly reproducible).
- **Distilled prover weights** (1.5B/7B/1.7B/72B on HuggingFace) — *usable off-the-shelf* if the user supplies a vLLM endpoint; this is the fastest path to a real `THEOREMATA_MODEL_COMMAND` for the Lean-generation role. Not something we train.
- **Model-size scaling claim** — an empirical result, not a component; informs whether it's worth wiring a large model but nothing to "build."

Bottom line: everything in §4-buildable is **format/reward-shaping/infra** and is genuinely adoptable this week; the differentiated capability (the 80.7% policy) is **the RL-trained weights**, which are gated — but the **distilled open weights are a drop-in generator** behind our existing gate.

---

## 5. Injection scan + license

**Injection scan:** No prompt-injection or instruction-to-the-agent content found in the paper, READMEs, or demo code. The PDF contains long Lean proofs and a `<think>` transcript; the demo `SYSTEM`/`prompt` strings and `create_tool_message` output are **data describing Kimina's own prompts**, not instructions to this agent. Treated throughout as untrusted data; **no embedded instruction was followed**. (No POSSIBLE INJECTION flag warranted.)

**License:** The repo `resources/Kimina-Prover-Preview-master/` contains **no LICENSE/COPYING file** (verified). No SPDX headers in the demo `.py` files. The distilled models on HuggingFace carry their own model licenses (not vendored here). Because the code license is **unspecified/absent**, treat it as **all-rights-reserved → CLEAN-ROOM: adopt ideas/format/recipe only, never copy source**. Our `create_tool_message` / `extract_proof_from_text` adoption above is described at the algorithm level for clean-room reimplementation, not copying.

---

## 6. Prioritized adopt-list

1. **[High, buildable now] RL/STaR format filters** — ≥1 tactic block, ≥60% coverage-of-final-proof, ω=0.5 negative-gradient drop. Cheap format-collapse insurance for `train/flywheel.py` + GRPO/STaR export. Convergent with our verifier-as-reward flywheel.
2. **[High, buildable now] Formal reasoning pattern** as the Lean-generator prompt+parse contract (`<think>` NL+snippet interleave → assembled proof; last-lean4-block parser). Improves explainability + `have`-decomposition; feeds the sketch pipeline.
3. **[High, buildable now] Infotree error-fix tool message** (top-3 errors + goal-state-before-error + marked snippet) → `proving/repair.rs` retry loop; supersedes raw-stderr feedback. Clean-room from `utils.py`.
4. **[High] Distilled Kimina weights as a drop-in Lean generator** behind our 3+1 gate (vLLM endpoint → `THEOREMATA_MODEL_COMMAND`). Fastest route to a real model for the generation role; weights are open.
5. **[Med, buildable now] Autoformalization sanity filters** — negation-proving, triviality-via-short-proof, contradiction detection, unanimous multi-sample judge voting → strengthen `statement_validation.rs` / `triviality.py` (advisory; gate stays ground truth).
6. **[Med] Lean-server throughput** — import-header LRU env cache + multi-REPL pool for `reason/team.rs`.
7. **[Med] Eval protocol** — 13-gram decontam, pass@k budget sweep, IMO/AIME subsets, the 8 known-bad miniF2F items → `eval_harness.py`.
8. **[Framing] "Whole-proof reasoning vs tree search" as a TTC mode** — add a Kimina-style whole-proof best-of-N generation mode to `search/driver.rs`, let `ttc.rs` pick it vs MCGS by difficulty/budget. Honest: for many problems it beats search per-sample; keep MCGS for the hard tail.
9. **[Gated] Full RL training** (Qwen2.5-72B, N×k rollouts, KL-to-SFT loss) — recipe documented; harness stays GPU-free dry-run until compute/weights.

Cross-paper convergence: verifier-as-reward (DeepSeekMath-V2, LEGO-Prover, PROOFGRADER), negation-proving statement filtering (DeepSeek-Prover), and goal-state feedback (ImProver Chain-of-States) are each re-endorsed here. Kimina's distinctive additions are the **format-collapse filters**, the **≥60% snippet-coverage alignment invariant**, and the **whole-proof-reasoning-beats-search** sample-efficiency thesis.
