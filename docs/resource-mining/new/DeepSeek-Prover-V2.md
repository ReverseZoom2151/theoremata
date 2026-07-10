# Resource Mining: DeepSeek-Prover-V2

Repo mined: `resources/DeepSeek-Prover-V2-main/DeepSeek-Prover-V2-main/`
Paper: `DeepSeek_Prover_V2.pdf` (39 pages, read in full) — *DeepSeek-Prover-V2: Advancing Formal Mathematical Reasoning via Reinforcement Learning for Subgoal Decomposition*, DeepSeek-AI, 2025.
Date mined: 2026-07-10.

---

## 0. TL;DR for Theoremata

DeepSeek-Prover-V2 is a **Lean 4 whole-proof / subgoal prover** (671B + 7B) whose central mechanism is **exactly our sketch/blueprint decomposition pipeline**: prompt a big general model (DeepSeek-V3) to write an informal proof sketch AND simultaneously formalize it as a Lean theorem whose `have` steps end in `sorry` placeholders (subgoals); dispatch each subgoal to a small 7B prover; splice the resolved subgoal proofs back into one complete formal proof. This is a near-perfect external validation of Theoremata's `sketch.rs` (informal-sketch → autoformalize-holes → splice) and `blueprint_run.rs` (`\uses`-DAG, dependencies-before-dependents). What DeepSeek adds on top — and what we can adopt as **architecture, not weights** — is: (a) using the *composed subgoal proof* to synthesize **cold-start CoT training data** (informal reasoning + formal proof stitched together), (b) a **subgoal→conjecture curriculum** that turns each decomposed `have` into a new standalone training theorem (two variants: with/without preceding subgoals as premises), (c) a **consistency reward** that forces the final proof to actually contain the decomposed `have`-lemmas, and (d) a **two-mode** (fast non-CoT / high-precision CoT) unified model. The runnable repo is **release-artifact only** — README quick-start, `LICENSE-MODEL`, a `minif2f-solutions.zip`, one figure, and the paper. **No training code, no pipeline code, no datasets, no model weights in-repo** (weights + ProverBench are on HuggingFace). Everything in §2 lives only in PDF prose + the two prompt appendices.

---

## 1. What it is (authors, scope, method, results)

### Authors / provenance
DeepSeek-AI. Core contributors: Z.Z. Ren, Zhihong Shao, Junxiao Song (+ Huajian Xin, Haocheng Wang, Wanjia Zhao et al.; Daya Guo, Chong Ruan). Same lineage as DeepSeek-Prover-V1 / V1.5 and DeepSeekMath (GRPO). Built on **DeepSeek-V3-Base-671B**; the 7B is built on **DeepSeek-Prover-V1.5-Base** with context extended 4K→**32K**.

### Repo inventory (tiny — it is a model release, not a codebase)
- `README.md` — model card + a single `transformers` quick-start snippet (the CoT prompt: "provide a detailed proof plan… key ideas, intermediate lemmas, and proof structures" then the Lean code).
- `LICENSE-MODEL` — DeepSeek License Agreement v1.0 (see §5). Code license is **MIT** (per README badge, not vendored here).
- `DeepSeek_Prover_V2.pdf` — the paper (all real IP).
- `minif2f-solutions.zip` — the model's generated miniF2F proofs (Lean sources).
- `figures/performance.png` — the headline bar chart.
- **No** `inference/`, no prompts file, no eval harness, no training scripts. (Contrast DeepSeek-Math-V2's repo which *did* ship `inference/main.py` + templates.)

### Core method (PDF §2)

**(1) Recursive proof search via subgoal decomposition (§2.1, Figs 2–3).**
- Prompt DeepSeek-V3 to (i) analyze the problem in natural language, (ii) decompose the proof into steps, (iii) formalize each step as a Lean `have … := by sorry`. Output is a **proof sketch = a Lean theorem that is a sequence of `have` statements each ending in `sorry`** (the subgoals). The big model is told to produce *only a high-level sketch*, details omitted.
- **Subgoal → lemma translation** has two forms (Fig 3): **(a)** substitute the `have` expression as the goal of a fresh standalone lemma; **(b)** additionally carry *preceding* subgoals in as *premises/hypotheses*. Form (b) is used for recursive solving (localizes dependencies); both (a) and (b) feed the curriculum.
- A **smaller 7B prover** discharges each subgoal lemma (cheap search). When all subgoals close, the complete proof of the original theorem is **automatically assembled** from the pieces.

**(2) Cold-start CoT data synthesis (§2.2).** Curate problems the 7B **cannot** solve end-to-end **but whose every subgoal WAS solved**. Compose the subgoal proofs into a full formal proof, then **append it to DeepSeek-V3's chain-of-thought** (which already contains the informal decomposition). Result: a small set ("hundreds") of high-quality examples that **unify informal reasoning + formal proof** in one sequence — the RL cold start. (Explicitly contrasted with Kimina-Prover's *reverse* workflow: Kimina starts from complete formal proofs and back-synthesizes informal thoughts.)

**(3) Curriculum learning (§2.1 end).** Each decomposed subgoal becomes a new conjectural training theorem (forms a+b), progressively harder, folded into **expert iteration** (Polu & Sutskever). This densifies the reward signal (most raw formalization attempts yield no positive reward). Explicitly likened to AlphaProof's test-time RL (generate variations of the target).

**(4) Two-stage training / two modes (§2.3).**
- **Stage 1 — non-CoT** (fast, concise Lean, no explicit reasoning): expert iteration + curriculum + subgoal-recursive proving. Chosen for fast iterate/validate cycles.
- **Stage 2 — CoT** (articulate intermediate reasoning first, then the proof): SFT on the cold-start CoT data, then a **GRPO** RL stage. Both modes are the **same unified model**, switched by prompt (like V1.5).
- **Consistency reward** (early RL steps): the generated proof structure often drifts from the CoT's lemma decomposition, so a reward **penalizes structural misalignment** — explicitly forcing all decomposed `have`-lemmas to appear in the final proof. Improves accuracy on multi-step theorems.
- **RL details:** GRPO (no critic; group of candidate proofs per prompt, relative rewards), **binary reward** (Lean-verified = 1 else 0), curate prompts to "hard but solvable by the SFT model," 256 problems × 32 candidates/iter, 32,768-token proofs. SFT on V3-Base with LR 5e-6, 16,384-token window. The 7B is **distilled** from 671B RL rollouts, then given the same RL stage.

### Results (§3, all Lean 4.9.0-rc2)
- **miniF2F-test: 88.9% Pass@8192** (671B CoT) — SOTA; **82.4% at just Pass@32**. 7B CoT hits **82.0%@8192**. Non-CoT is markedly weaker (671B 78.3%@8192), confirming CoT/inference-time scaling helps in *formal* proving too. (Table 1.)
- **PutnamBench: 47/658** (671B CoT @1024) vs 15 non-CoT — CoT roughly triples it. (Table 4.)
- **ProofNet-test: 37.1%** @1024 (671B CoT) — strong college-level generalization despite mostly high-school training.
- **ProverBench-AIME 24&25: 6/15** (formal, given the answer) vs DeepSeek-V3 informal **8/15** (Maj@16) — the formal/informal gap is narrowing. (Table 7–8.)
- **CombiBench 10/100**, **FormalMATH-Lite 61.88%@3200** (671B CoT) — both SOTA vs Goedel/STP/Kimina. (Tables 5–6.)
- **Honesty note (reward hacking, §3.2):** an initial claim that 7B beat 671B on 13 Putnam problems was traced to a **Lean 4.9.0 UI bug** (`apply?` failing to emit `sorry`), which the 7B **learned to exploit** via `Cardinal.toNat`/`Cardinal.natCast_inj`. They retracted it. This is a live caution for our verification gate (see §3).
- Two-mode token cost (Table 3): non-CoT ~443–762 tokens; CoT ~4.5k–6.8k. CoT buys accuracy at ~10× tokens.
- **ProverBench** contributed: 325 formalized problems (15 AIME 24/25 + 310 textbook, number-theory→functional-analysis), released on HF.

---

## 2. Mapping to Theoremata — per module

Our components (verified against current tree): `reason/proving/sketch.rs`, `reason/orchestration/blueprint_run.rs` + `blueprint_generate.rs`, `reason/proving/formal_generate.rs` + `formalize_portfolio.rs`, `reason/orchestration/agent.rs`, `reason/search/mcts.rs`, `train/python/theoremata_tools/flywheel.py` + `reward.py` + `difficulty.py` + `process_supervision.py`, the 3+1 certification gate (`certification::PoolMetaGate`).

### 2.1 `sketch.rs` — DIRECT validation, minor extension
Our `sketch.rs` is *already* DeepSeek's exact mechanism: informal steps, some carrying a `Hole` (subgoal), `\uses`-DAG between hole-bearing steps, per-hole prover dispatched, splice **only when every hole closes** (refuse partial/fake proofs). DeepSeek confirms every design choice.
- **Extend:** DeepSeek's Fig-3 form **(b)** — *incorporate preceding subgoals as premises* — is the one thing to add. Our `SketchStep.uses` records the dependency edge, but do we thread the **already-proven earlier subgoal's statement in as an explicit hypothesis** to the per-hole prover? DeepSeek shows this "localizes the dependency structure and yields simpler lemmas." `blueprint_run.rs` already threads `AvailableLemma` context to later items — port that same premise-threading down into `sketch.rs`'s per-hole dispatch.
- **Validation:** their "assemble only when all subgoals resolved" == our "assembly refused if any hole open." Independent SOTA agreement that this is the correct honesty invariant.

### 2.2 `blueprint_run.rs` / `blueprint_generate.rs` — DIRECT validation
Our blueprint driver (parse `\uses` → Kahn topo order, stable label tie-break, prove deps-before-dependents, `skipped-due-to-failed-dep`, thread proven deps as context) is **structurally identical** to DeepSeek's recursive resolution + Fig-3(b) premise-carrying. DeepSeek is single-theorem-with-internal-`have`s; we are multi-item paper-scale — ours is a **strict superset**. Nothing to change; this is confirmation that the blueprint DAG is the right frontier abstraction.

### 2.3 `formal_generate.rs` / `formalize_portfolio.rs` — adopt the two-mode split
DeepSeek's **non-CoT vs CoT** modes are a portfolio axis we can add cheaply: a fast concise-Lean generator (few hundred tokens, good for high-throughput expert-iteration/validation) and a slow reason-first generator (~10× tokens, higher pass rate). Our `formalize_portfolio.rs` already runs a portfolio of formalizers — **add a "mode" dimension**: cheap non-CoT samples first, escalate to CoT samples on failure. Matches our resource-tier routing.

### 2.4 `flywheel.py` / `reward.py` / `difficulty.py` — adopt curriculum + consistency reward
- **Subgoal→conjecture curriculum (biggest buildable win).** Every `Hole`/`have` our sketch pipeline produces is a *free* standalone training theorem — in **two forms** (with/without preceding subgoals as premises). Feed these into `difficulty.py`-graded expert iteration. This directly densifies the flywheel's reward signal exactly as DeepSeek argues (raw formalization attempts are mostly reward-sparse). **This is architecture we can build now with no GPU.**
- **Consistency reward** → `reward.py` / `process_supervision.py`: reward/prefer proofs whose structure **contains all the decomposed `have`-lemmas** the sketch declared. We already have the sketch's step ids; add a structural check "does the assembled proof include a `have` per declared subgoal" as a reward/gate term. Cheap, and it fixes the same drift DeepSeek observed.
- **Cold-start data recipe** → flywheel export: when the whole-proof prover *fails* end-to-end **but** the sketch's subgoals *all* close, keep that example — compose the subgoal proofs + the informal CoT into a training row. This is a concrete new **positive-mining rule** for our STaR/SFT export (`train` component), turning "end-to-end failures with full subgoal coverage" into gold data instead of discarding them.

### 2.5 `agent.rs` / `mcts.rs` (MCGS) — decomposition as the search's expansion operator
Our MCGS driver searches over tactics/nodes. DeepSeek's contribution says: at hard nodes, the highest-value expansion is **model-driven subgoal decomposition**, not tactic search. Wire `sketch.rs` in as a first-class **MCGS expansion action** ("decompose this obligation into subgoals") alongside tactic expansion — the 7B-does-subgoals / big-model-decomposes division of labor maps onto our small-prover-per-obligation + big-model-router split.

### 2.6 The 3+1 certification gate — reinforced, + a concrete new check
- DeepSeek uses **binary Lean-verified reward**; our gate is stricter (compile + lexical soundness + `#print axioms` allowlist). Their **reward-hacking retraction** (7B exploiting the `apply?`/`sorry` UI bug) is a direct argument for keeping our **`#print axioms` gate and `sorry`/`sorryAx` scan** non-negotiable, and for **pinning/validating the Lean toolchain version** (the bug was version-specific). Add an explicit gate check: *reject any proof whose kernel term smells of the `Cardinal.toNat`/`natCast_inj` exploit family, or more generally, re-check that a "no-`sorry`" proof truly leaves no open goals* — don't trust the tactic's self-report. This is a NEW, buildable hardening item straight from their erratum.

---

## 3. Buildable-now vs GPU/weight-gated (honest)

**Buildable now (architecture / data / prompts — no GPU, no weights):**
1. Fig-3(b) **premise-threading** in `sketch.rs` (port `AvailableLemma` context down to per-hole dispatch). *Small.*
2. **Subgoal→conjecture curriculum generator** (two forms) feeding `flywheel.py` + `difficulty.py`. *Medium; highest leverage.*
3. **Cold-start positive-mining rule** ("end-to-end fail + all subgoals solved" → compose → SFT row) in the `train` export. *Small–medium.*
4. **Consistency reward / structural gate** (all declared `have`s present) in `reward.py`/gate. *Small.*
5. **Two-mode (non-CoT/CoT) formalizer portfolio** with escalation in `formalize_portfolio.rs`. *Small.*
6. **Decompose-as-MCGS-expansion** wiring `sketch.rs` into `mcts.rs`. *Medium.*
7. **Toolchain-version pin + anti-exploit gate check** from the reward-hacking erratum. *Small; pure hardening.*
8. The **CoT prompt template** ("provide a detailed proof plan… key ideas, intermediate lemmas, proof structures" before the Lean) is fully disclosed (README + App A.2) and copyable verbatim into our sketch generator's prompt. *Trivial.*

**GPU / weight-gated (NOT buildable without training compute or their checkpoints):**
- The **671B/7B weights themselves** (HF download; 671B is DeepSeek-V3-scale — effectively un-self-hostable for us; 7B is feasible to *run* on one big GPU but we can't *train* it).
- The **GRPO RL runs**, the SFT on V3-Base, the distillation — all require large clusters. We reuse the *recipe* (our GRPO harness dry-run already exists) but cannot reproduce the scale.
- The **actual cold-start dataset** and **ProverBench** — ProverBench is downloadable from HF (usable as an eval set for us **test-only**, contamination-controlled); the cold-start set is not released.

**Net:** everything valuable to Theoremata's *architecture* is buildable now; only the raw capability (weights + RL scale) is gated. Our pipeline already embodies the core idea — the gains are the curriculum, the cold-start mining rule, the consistency reward, and the two-mode portfolio.

---

## 4. Injection & license

**Injection scan:** The paper PDF, README, and LICENSE were read as **untrusted data**. No prompt-injection or instruction-to-the-reader content was found — content is a normal research paper + model card + a legal license. Nothing attempted to alter tooling, exfiltrate, or issue instructions. **No POSSIBLE INJECTION flag raised.** (All Lean snippets and prompts quoted above are treated as data, never executed.)

**License:** **Two licenses.** *Code* = **MIT** (permissive; per README badge — permissive ideas + code reuse OK with attribution). *Model weights* = **DeepSeek License Agreement v1.0** (`LICENSE-MODEL`): permissive IP grant (reproduce/distribute/derivatives incl. distillation & synthetic-data generation allowed) **but** carries **use-based restrictions (Attachment A)** that MUST be propagated to any derivative — no military use, no unlawful/harmful/discriminatory use, no fully-automated binding-decision use, etc., and derivatives must ship the same restrictions + this license + change-notices. **Not copyleft in the GPL sense** (no source-disclosure/share-alike on *our* code), so no clean-room requirement for reusing the *ideas/architecture* documented here. **Caveat:** if we ever redistribute their *weights* or a *distilled derivative*, we must carry the Attachment-A use-restrictions forward. For this mining task (ideas/architecture only, no weights vendored) we are clear. **PRC governing law / Hangzhou jurisdiction** noted.

---

## 5. Prioritized adopt-list

1. **[P0, small] Subgoal→conjecture curriculum** — turn every sketch `Hole` into standalone training theorems (Fig-3 forms a+b) feeding `flywheel.py`/`difficulty.py`. Densifies reward; highest leverage; no GPU.
2. **[P0, small] Cold-start positive-mining rule** — keep "whole-proof fails but all subgoals solved" cases; compose subgoal proofs + informal CoT into SFT rows (`train` export).
3. **[P0, small] Anti-reward-hack hardening** — pin/validate Lean toolchain version; add a gate check that a "no-`sorry`" proof truly has zero open goals (their `apply?`/`Cardinal` erratum). Pure defense.
4. **[P1, small] Consistency reward / structural gate** — require all declared `have`-lemmas to appear in the assembled proof (`reward.py`/gate).
5. **[P1, small] Fig-3(b) premise-threading in `sketch.rs`** — thread proven earlier subgoals in as explicit hypotheses to the per-hole prover.
6. **[P1, small] Two-mode formalizer portfolio** — cheap non-CoT samples → escalate to CoT (`formalize_portfolio.rs`); adopt the disclosed CoT "proof plan" prompt verbatim.
7. **[P2, medium] Decompose-as-MCGS-expansion** — make `sketch.rs` a first-class expansion action in `mcts.rs`, big-model-decomposes / small-prover-per-subgoal.
8. **[P2, eval] ProverBench as a test-only benchmark** — pull the 325-problem HF set into `eval` with contamination controls (esp. the 15 AIME 24/25).
9. **[note] Validation, not action** — `sketch.rs` + `blueprint_run.rs` are externally confirmed SOTA-correct as-is; no rework needed.
