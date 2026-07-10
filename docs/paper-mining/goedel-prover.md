# Goedel-Prover (V1 + V2): Paper & Repo Mining

**Sources**
- Paper (V1): *Goedel-Prover: A Frontier Model for Open-Source Automated Theorem Proving* — arXiv:2502.07640v3 (Apr 2025). Lin, Tang, Lyu, Wu, H. Lin, Yang, Li, Xia, Chen, Arora, Jin (Princeton PLI + Tsinghua + Numina; Kaiyu Yang / Meta FAIR advisory).
- Paper (V2): *Goedel-Prover-V2: Scaling Formal Theorem Proving with Scaffolded Data Synthesis and Self-Correction* — arXiv:2508.03613v1 (Aug 2025). Same core group + NVIDIA / Stanford / Amazon collaborators.
- Repo: `resources/Goedel-Prover-V2-main/` — README, `scripts/pipeline.sh`, `src/{inference,compile,summarize,utils}.py`, `lean_compiler/repl_scheduler.py`, `dataset/{minif2f,MOBench,test}.jsonl`.

**License**: Repo has **no `LICENSE` file**; README badge + HF model cards (Goedel-Prover-V2-8B/32B, MathOlympiadBench) declare **Apache-2.0** (permissive). Ideas *and* code are reusable with attribution — but treat the missing repo-level license as a reason to prefer **clean-room re-implementation** and to confirm the Apache grant on the actual HF artifacts before vendoring any file. Datasets built on Numina/Lean-Workbook/Compfiles/IMOSLLean4 carry their own upstream terms.

**Injection check**: No prompt-injection found. The repo/PDFs contain LLM-judge *prompt templates* (faithfulness judging, sub-problem generation, difficulty judging) — these are the system's own data-synthesis prompts, not instructions addressed to a reading agent. Nothing tries to redirect the task. **No POSSIBLE INJECTION.**

---

## 1. What it is

An open-source **whole-proof-generation** theorem-prover line for **Lean 4** (Mathlib, Lean 4.9.0). The through-line across both papers is that the binding constraint in formal ATP is **data scarcity**, and the answer is a **verifier-in-the-loop data flywheel**: autoformalize huge pools of informal competition math into Lean statements, prove what you can with the current model, keep only Lean-compiler-verified proofs, retrain, repeat. V2 adds three innovations on top: **scaffolded (difficulty-targeted) data synthesis**, **verifier-guided self-correction**, and **model averaging (model soups)**.

Key results:
- **V1 (Goedel-Prover-SFT)**: pure SFT (no RL) → **57.6% Pass@32 on miniF2F** (beats DeepSeek-Prover-V1.5-RL's 50.0% by 7.6 pts); 62.7% @3200. Solved 7 PutnamBench (#1 at the time), and found proofs for **29.7K Lean-Workbook problems** (nearly doubling the prior 15.7K). DPO/GRPO pushed >60% Pass@32 but **overfit to tactic "shortcuts"** (`try`, `all_goals`) and scaled worse at high Pass@N.
- **V2 (Qwen3-8B / 32B base)**: **8B = 84.6% Pass@32 miniF2F**, matching/beating **DeepSeek-Prover-V2-671B** at ~80× fewer params; **32B = 88.1%** standard, **90.4% with self-correction**. PutnamBench: **86 solved @Pass@184** (self-correction), #1 open-source, vs DeepSeek-V2-671B's 47 @Pass@1024. Released **MathOlympiadBench** (360 human-verified IMO/shortlist/national-olympiad formalizations from Compfiles + IMOSLLean4).

---

## 2. Key mechanisms

### 2.1 Autoformalization + filtering pipeline (both versions)
- **Two-formalizer diversity (V1)**: Formalizer A (SFT on Lean-Workbook informal↔formal pairs) and Formalizer B (SFT on 170K Claude-Sonnet-3.5 formalizations of Numina that passed Lean compile). Training a prover on a *mixture* of both styles beats either alone — formalization *style* materially changes prover pass-rate on the same problem (Table 6 shows 14/16 vs 0/16 swings). Yielded **Goedel-Pset-v1: 1.64M–1.78M formal statements**.
- **Two-gate statement QA** (adopted verbatim as a pattern worth stealing):
  1. **CC (Compiling Correctness) Test** — statement must compile in Lean with body `:= by sorry`.
  2. **FC (Faithfulness & Completeness) Test** — an LLM judge (Qwen2.5-72B) scores whether the Lean statement faithfully captures the informal problem (all assumptions/conditions/goal). Four independent judgments → FC score = fraction "Appropriate"; **filter if < 0.5**.
- **V2 formalizer** is trained with **reasoning traces** (bootstrapped from 50K Claude-Sonnet-4 formalizations), and beats Kimina-Autoformalizer 228/300 vs 161/300 on OmniMath. Semantic check moves to **majority-vote of Qwen3-8B ×3**. Finding: **>80% of *unsolved* Goedel-Pset-v1 problems were mis-formalized** — statement quality, not prover weakness, was a dominant failure mode.

### 2.2 Expert iteration (the flywheel)
V1 runs **8–9 iterations**: iter-0 uses DeepSeek-Prover-V1.5-RL to generate 16 proofs/statement → Lean-verify → keep one correct proof/statement → SFT DeepSeek-Prover-V1.5-**Base** → iter-1 prover; each round *re-proves from Base* on the cumulatively-grown solved set (Table 8 traces 20.6K→30.3K Lean-Workbook + 0→928K formalized solved). Scaling the statement pool monotonically improves prover accuracy.

### 2.3 Scaffolded data synthesis (V2's headline idea)
Generate synthetic problems **at the right difficulty** to give the model gradient signal it can actually learn from. Two engines:
- **Formal-based**: when the prover *fails* a hard problem, run Lean's **`extract_goal`** on the incomplete proof to harvest the open subgoals (with preconditions) as new, well-formed, *easier* statements. Since an extracted goal may be false, **also add its negation** — training the model to recognize both true and false propositions (Appendix B shows the `¬ ∀ …` negation transform).
- **Informal-based** (Fig. 2 pipeline): prompt Qwen3-32B — if a problem is unsolved, generate **simpler/sub-problems** reflecting core solution steps; if solved, generate **harder variants** (change constants, complicate expressions, lift ℝ→ℂ/vector/matrix, substitute variables, chain conclusions). First have the LLM write a natural-language solution to use as context. Then formalize (×2), semantic-check (Qwen3-8B majority vote ×3), and run an **LLM difficulty+correctness filter** (Qwen3-32B ×4: strict-majority correctness, unanimous "too-simple" to drop; incorrect statements added as negations). Final exact-match dedup. Prompt templates are in the paper's Appendix D.

### 2.4 Verifier-guided self-correction (V2)
The whole-proof loop is wrapped so that a **failed proof + parsed Lean error messages are fed back** into a long-CoT model, which analyzes the error and emits a repair. Standard mode = 2 correction rounds, 40K total tokens (vs 32K single-shot). Ablations (Fig. 7): **removing compiler error messages hurts most**; removing prior-round CoT hurts slightly; extending to 128K/5 rounds reaches 92.7% — beating no-correction @Pass@8192 with far fewer samples. Repo shows the exact mechanics (see §2.6). A separate **targeted-repair** variant uses `extract_goal` to isolate only the faulty subgoal, re-prove it, and splice it back — +1–2 pts on the amortized budget curve.

### 2.5 Training algorithms
- **SFT + expert iteration** across staged datasets S1→S2→S3 (S1 from DeepSeek-Prover-V2-7B/671B inference; S2 adds self-correction annotations; S3 adds scaffolded synthesis).
- **RL**: hybrid **GRPO** with **Dr.GRPO** (no group norm, to kill length bias) + **DAPO** tricks (clip-higher, overlong penalty, dynamic sampling), **no KL term** (encourage exploration). **Multi-task rollout: 50% whole-proof / 50% first-round self-correction** — trains proof + repair jointly without bespoke tool-calling or multi-turn RL machinery. Dynamic sampling keeps only problems with **pass-rate in (0, 0.75]** ("challenging yet manageable"). Reward +8 correct / −8 fail; **timeout should be scored as failure** (V1 Table 11). VeRL framework.
- **Model averaging (model soups)**: interpolate `(1−α)·θ_base + α·θ_finetuned` after SFT and after RL. Directly counters **diversity collapse** in late training (pass@1 up but pass@N down); there's an optimal α that maximizes pass@N, and it **amplifies the RL self-correction benefit**.

### 2.6 Repo mechanics worth noting (offline-reusable, no GPU needed to understand)
- `scripts/pipeline.sh` = a clean **round loop**: for round 0..MAX, run `inference.py` → `compile.py` → `summarize.py`; round 0 samples N=8/problem, correction rounds sample N=2 per *failed* variant only.
- `utils.py::get_error_str` — the **error-rendering** function: takes Lean REPL errors, caps to 8, and renders each with 4 lines of leading context and `<error>…</error>` span markers, truncating long spans. This is the concrete "how to feed a compiler error to an LLM" recipe.
- `utils.py::load_data_for_correction` — reads the previous round's `to_inference_codes` + `code_compilation_repl`, marks an origin-problem **solved if any variant is pass && complete**, and only re-queues *unsolved* problems' failed variants (with full message history) for the next round.
- `utils.py::replace_statement_in_proof` / `return_theorem_to_prove` — **statement-preservation guard**: strips comments, regex-extracts the original `theorem … := by`, and **splices the model's proof body onto the *canonical* statement** so the model cannot silently weaken/alter the goal it was asked to prove. Also **bans `apply?`/`exact?`** (interactive search tactics) in submitted proofs.
- Multiple `InferenceHandler` subclasses (DeepSeek-CoT / DeepSeek-nonCoT / Kimina-CoT) abstract per-model prompt + extraction formats behind one interface — a per-backend adapter pattern.
- `compile.py` + `lean_compiler/repl_scheduler.py` — parallel Lean-REPL scheduler over N CPU workers returning `{pass, complete, errors}`.

---

## 3. Mapping to Theoremata modules

| Goedel mechanism | Theoremata component | Fit |
|---|---|---|
| Expert iteration (prove→verify→keep→retrain) | **Flywheel / STAR trainer** | Direct. Goedel is a reference implementation of exactly our expert-iteration loop; adopt the *re-train-from-Base-each-round* + cumulative-solved-set discipline and Table-8-style per-iteration bookkeeping. |
| CC + FC statement tests; ">80% unsolved were mis-formalized" | **statement-validation / `augment_statement`** + **verification gate** | Direct. Add a two-gate `validate_statement`: (1) backend-compile with `sorry` body (CC); (2) LLM faithfulness judge with N-vote threshold (FC ≥ 0.5). This is a new *gate stage* before proving. |
| Verifier-guided self-correction loop (error → analyze → repair) | **MCGS proof-search driver** + **verification gate** | Strong. Our gate already produces pass/fail + certs; wrap a bounded repair loop that renders backend errors (á la `get_error_str`) back into the generator. Slots in as an alternative/complement to MCGS search. |
| `extract_goal` → subgoal harvesting + negation | **sketch/blueprint pipeline** + **graph proof-DAG** + flywheel | Strong. Failed-proof subgoals become new DAG nodes / new statements to prove; negation-augmentation feeds the trainer both-polarity signal. Maps cleanly to a proof-DAG that records open subgoals. |
| Informal-based difficulty scaffolding (simpler-if-unsolved / harder-if-solved) | **Flywheel curriculum** + **sketch pipeline** | Strong, and *new to us*: an offline data-generation stage that targets the model's current difficulty frontier. |
| Two-formalizer style diversity; style→pass-rate coupling | **per-system generators** + statement-validation | Medium. Argues for multiple formalization strategies + keeping style variants as distinct candidates. |
| `replace_statement_in_proof` (splice proof onto canonical statement; ban `apply?`/`exact?`) | **verification gate / fail-closed resource guard** | Direct & cheap. A must-have anti-cheat: enforce the submitted proof targets the *exact* asked statement, and blocklist search tactics. |
| Model averaging vs diversity collapse | **STAR trainer** | Medium (GPU-side). A late-training knob; relevant only once we train weights. |
| Per-backend `InferenceHandler` adapters | **per-system generators / FormalSystem abstraction** | Direct pattern match — same adapter shape we want for Lean/Rocq/Isabelle/Candle. |
| RL shortcut/reward-hacking + timeout=failure findings | **verification gate resource guard** + trainer | Medium. Concrete guardrails: penalize `try`/`all_goals` spam, score timeouts as failures, length-penalize. |
| MathOlympiadBench; miniF2F mis-formalization case studies (V2 App. A) | **eval harness / benchmark set** | Direct. Adopt MathOlympiadBench (Apache-2.0) as a hard eval; heed that miniF2F has weaker-than-informal and mismatched statements (3+ documented). |

---

## 4. Buildable-now vs model/GPU-gated

### Buildable now (offline scaffolds/pipelines — no training required)
1. **Two-gate statement validator** (CC compile-check + FC LLM-faithfulness N-vote) as a gate stage — pure orchestration over our existing backends + any judge model. *Highest ROI.*
2. **Verifier-guided self-correction loop** — round loop (`pipeline.sh` shape) + error-renderer (`get_error_str` shape) + repair prompt, driving any generator through our gate. Backend-agnostic.
3. **Statement-preservation anti-cheat** (`replace_statement_in_proof` + ban `apply?`/`exact?`) — small, deterministic, immediately hardens the gate.
4. **`extract_goal`-style subgoal harvesting + negation augmentation** — a data/graph stage: on failure, extract open subgoals into the proof-DAG and mint (statement, ¬statement) training/search tasks. Lean has the tactic; needs per-backend equivalents.
5. **Difficulty-scaffolded synthesis stage** (simpler-if-unsolved / harder-if-solved) — LLM-prompt pipeline; prompts are published (App. D). Feeds the flywheel a curriculum.
6. **Expert-iteration driver + bookkeeping** (cumulative solved set, per-iteration deltas) — orchestration around our trainer/gate; the *loop* is buildable even before we own weights (can iterate with API/base models).
7. **Eval harness upgrades**: ingest MathOlympiadBench; add a miniF2F statement-quality lint (flag weaker-than-informal / mismatched statements).

### Model/GPU/data-gated (needs weights, clusters, or large inference budgets)
- The trained **Goedel-Formalizer-V2** and **Goedel-Prover-V2-8B/32B** weights themselves (we can *call* them as generators, but can't reproduce training cheaply).
- **RL stage** (hybrid GRPO/Dr.GRPO/DAPO, VeRL, ~144 H100s for self-correction data) and **model averaging** — only meaningful once we train.
- **Full-scale expert iteration** (1.78M statements × Pass@16 verify = the paper's "6 h × 64 H100 inference + 10 h × 8000 CPU verify" per round) — the *method* is free; the *scale* is gated.

---

## 5. Prioritized adopt-list

1. **Two-gate statement validation (CC + FC)** — bolt onto `augment_statement`/gate. Cheap, and V2's "80% of unsolved were mis-formalized" says this is where marginal quality lives.
2. **Statement-preservation + tactic-blocklist anti-cheat** in the verification gate — tiny, deterministic, prevents silently-weakened goals and `exact?`/`apply?` cheating.
3. **Verifier-guided self-correction loop** (bounded rounds, span-marked error rendering, keep prior CoT) as a gate/MCGS-adjacent mode — the single biggest accuracy lever V2 reports (+2 pts, and sample-efficiency wins).
4. **`extract_goal` subgoal harvesting → proof-DAG nodes + negation tasks** — turns failures into curriculum and graph structure; unifies with our sketch/blueprint + flywheel.
5. **Difficulty-scaffolded informal→formal synthesis** curriculum stage feeding the flywheel (published prompts).
6. **Adopt MathOlympiadBench** + miniF2F statement-quality lint into the eval harness.
7. **Per-backend InferenceHandler adapter pattern** → align with our FormalSystem/per-system-generator abstraction.
8. **Trainer guardrails** (timeout=failure, length penalty, shortcut-tactic penalty; model-averaging knob) — stage for when we own the RL/SFT loop.

---

### ~10-line summary
Goedel-Prover V1/V2 (Princeton PLI et al., Apache-2.0 models; **repo has no LICENSE file — verify before copying, prefer clean-room**) is an open-source whole-proof Lean 4 prover whose thesis is that **data scarcity, not model size, is the bottleneck**. V1 = massive autoformalization of Numina/Lean-Workbook (1.64M statements) + 8-round **expert iteration** with **Lean-verified** proof keeping → 57.6% miniF2F Pass@32 by SFT alone. V2 adds three levers: **scaffolded data synthesis** (harvest `extract_goal` subgoals + negations; LLM-generate simpler/harder variants at the difficulty frontier), **verifier-guided self-correction** (feed parsed Lean errors back into a long-CoT repair loop), and **model averaging** (fights late-training diversity collapse) — a 32B model hits 90.4% miniF2F and 86 PutnamBench, beating the 671B DeepSeek-Prover-V2. For Theoremata the **buildable-now** wins are offline and backend-agnostic: a **two-gate statement validator (compile + LLM-faithfulness)**, a **self-correction loop** around our gate, a **statement-preservation anti-cheat**, and **subgoal-harvesting + difficulty-scaffolded synthesis** feeding the flywheel/STAR trainer and proof-DAG. RL, model averaging, and full-scale iteration are GPU/weight-gated but the loop logic is free. **No prompt injection detected.**
