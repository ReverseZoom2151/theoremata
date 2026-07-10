# FormaRL — Enhancing Autoformalization with no Labeled Data

Paper-mining report. Source paper: `math-papers/FormaRL - Enhancing Autoformalization
with no Labeled Data.pdf` (arXiv:2508.18914v1, 26 Aug 2025). Repo:
`resources/FormaRL-main/FormaRL-main/`.

**Security / injection note:** all paper text, repo code, prompt templates, and
dataset samples were treated as UNTRUSTED DATA. I read the full PDF (20 pp.,
extracted with `pypdf` to scratchpad), the README, LICENSE, and every training/tool
source file. **No prompt-injection or embedded instruction directed at the reader
was found.** The two natural-language templates in the appendix (problem-extraction,
consistency-check) and the RL translation prompt are ordinary task prompts for
*their* pipeline, not instructions aimed at us. If any of this content is later fed
into a Theoremata model prompt it must go through `guard::wrap_untrusted` like every
other external string.

**License:** MIT (`Copyright (c) 2025 Carlos-Mero`). Permissive — NOT copyleft. We may
read, adapt, and port the implementation directly with attribution; no clean-room
constraint. (Contrast the copyleft repos in the mining set that require ideas-only
porting.)

---

## 1. What it is + authors / venue

FormaRL is a **label-free reinforcement-learning framework for autoformalization**
(translating a natural-language math *statement* into a Lean 4 theorem, proof left as
`sorry`). Accepted at **COLM 2025**. Authors: Yanxing Huang, Xinling Jin, Sijie Liang,
Peng Li, Yang Liu (Tsinghua University — Math Sciences, IIIS/Institute for AI, AIR —
with Tongji and Beijing Forestry). Official code: THUNLP-MT/FormaRL.

Core claim: with **859 *unlabeled* statements** (the miniF2F + ProofNet statement
pools, ground-truth translations NOT used) and GRPO, they raise pass@1
autoformalization accuracy of Qwen2.5-Coder-7B-Instruct by **4–6×** (ProofNet
4.04% → 26.15%; their new `uproof` OOD benchmark 2.4% → 9.6%), beating SFT baselines
trained on 25.2k–243k *labeled* pairs — using ~1% of the data and no human
annotation. They also release **`uproof`**, a 5,273-problem OOD benchmark of
undergraduate proof problems mined from 14 classical textbooks (analysis, algebra,
topology, probability, …), to expose that current formalizers collapse on advanced
(non-contest) math.

Two contributions matter to us: (a) the **reward design** — a hybrid
compiler-syntax + LLM-consistency signal that needs no labels — and (b) evidence that
this label-free signal *generalizes better* than large-scale SFT (the OOD gap is the
whole point).

---

## 2. Key mechanisms

### 2.1 The label-free reward (the crux)

For each candidate formalization the reward is computed from two sequential checks
(adapted from the Lean Workbook data-filter):

1. **Syntax Check (SC)** — run the Lean 4 compiler + Mathlib4 over
   `header + candidate` (proof = `sorry`). "Accepted" = no `error`-severity messages
   (a `sorry`'d-but-syntactically-valid statement compiles to "no goals").
   Implementation: `tools/verifier.py::verify_lean4_file` shells `lake exe repl` on a
   JSON REPL command, parses `messages`, sets `pass = not errors`. The fixed header
   (`tools/header.py`) is `import Mathlib / import Aesop / set_option maxHeartbeats 0 /
   open Topology BigOperators Nat Real Rat`.
2. **Consistency Check (CC)** — after stripping comments/metadata
   (`utils.remove_lean_comments`), an LLM judges whether the Lean statement and the NL
   problem have **"exactly the same conditions and conclusions."** Prompt (verbatim
   template in `pipeline/pipe.py` / `eval.py::consistency_check`) asks the model to
   reply `$\boxed{true}$` / `$\boxed{false}$`; the verdict is parsed by
   `utils.find_boxed`. Default judge = DeepSeek-V3 (remote, OpenAI-compatible API);
   ablated against Qwen2.5-7B-Instruct and GPT-4o. Sampling: temp 0.6, min-p 0.05,
   2048 tokens; a missing/unparseable answer defaults to `false`.

**Reward rule (training): AND-gate.** The paper states reward `1.0` iff SC ∧ CC else
`0.0`. In code this is `beta_1·SC + beta_2·CC` with a bonus `beta_3` when BOTH pass;
the shipped `configs/grpo-online.yaml` sets `beta_1=0, beta_2=0, beta_3=4.0` — i.e.
reward = 4.0 only when SC ∧ CC, else 0.0 (`eval.py::Evaluator.__call__`).

**Anti-reward-hack guards (small but load-bearing — worth porting):**

- CC-only would let the model emit one trivial always-compiling statement → the
  `flp != ''` and `"1+1=2" not in flp` filters in `Evaluator.consistency_check`, plus
  the compiler-check guard `response_dict==''  AND flps[i]!=''` (empty / comment-only
  strings pass the bare compiler but are rejected).
- SC-only would let the model paste the NL statement back in (great "consistency," not
  a formalization) → keeping CC in the product blocks it.
- The ablation (Table 5) shows removing *either* check collapses training into these
  exact exploits. **Both components are necessary**; neither alone is a usable reward.

### 2.2 Training method

Simplified **GRPO** (DeepSeek-R1 style): sample a group of G outputs per prompt,
advantage = group-normalized reward `(r − mean)/std`, **no KL term** (`beta=0.0`,
following SimPO/DAPO findings — cheaper, stabler here). Built on TRL `GRPOTrainer` +
`accelerate` (`training_scripts/grpo.py`), 8-bit Adam. Hyperparams: lr 1e-6, G
(num_generations) 4, 3 epochs, max completion 1024–2048, bf16, 2×A100-80G. The reward
function is the `Evaluator` object passed as TRL `reward_funcs`.

### 2.3 Quality assessment of CC + curriculum

They audit CC as binary classification: **recall** (accept known-good pairs) and
**specificity** (reject LLM-perturbed pairs where a condition/conclusion was
added/removed/altered). Table 1: DeepSeek-V3 recall/specificity 88.5/98.6% on miniF2F
but 76.8/93.5% on ProofNet, and specificity **craters under pass@k** (16.25% on
ProofNet@8). Consequence, and a **key design lesson for us**: they do **NOT** use CC
for *selection* in high-throughput sampling — for pass@k they select the first
candidate that passes **SC only**, and let CC contribute to the *final* reported pass
rate. So CC gates the *training reward* but SC alone gates *sampling selection*. No
formal curriculum beyond "warm-up 1 epoch of SFT if the base model is too weak to
generate any compilable output" (needed for DeepSeek-Math-7B, not for Qwen-Coder).

### 2.4 Benchmark results (headline)

- ProofNet pass@1 (in-distribution, advanced math): GPT-4o 4.04% / DeepSeek-V3 2.70%
  → FormaRL **58.49% SC / 26.15% final**.
- `uproof` OOD pass@1: RAutoformalizer (243k-pair SFT) 14.1%/6.2% → FormaRL
  20.1%/**11.9%** (+5.7 final); pass@16 24.4% → **33.6%**.
- FormaRL from scratch (no SFT) gives the *largest* gains; applying it on top of a
  heavily-SFT'd formalizer plateaus quickly (limited upper bound).
- Manual review (Table 7) found **no evidence of reward hacking** after FormaRL.

---

## 3. Mapping to Theoremata (per module)

FormaRL's SC∧CC reward maps almost 1:1 onto Theoremata's **statement-validation
cluster + verifier gate + flywheel/GRPO trainer**. The striking finding: **we already
have every component; FormaRL's contribution is the one wire we have NOT connected —
the faithfulness (CC) signal into the training reward.**

| FormaRL piece | Theoremata analog | Status of the mapping |
|---|---|---|
| **CC (LLM consistency judge)** | `components/verify/python/theoremata_tools/statement_roundtrip.py` (`roundtrip_validate`) + `components/reason/orchestration/statement_validation.rs` | Present and *richer* than FormaRL's boolean judge: it back-translates Lean→English and emits a typed **divergence taxonomy** (quantifier-flip / relation-flip / dropped-hypothesis / added-constraint / negation-mismatch / lexical-drift) with a `[0,1]` `faithful_score`. **But it is ADVISORY-only — it never feeds a reward and never gates.** FormaRL is the recipe to promote it into a reward. |
| **CC as a specificity-tested classifier** | Our roundtrip already models the *perturbations* FormaRL tests for (dropped hypothesis, flipped relation/quantifier) as first-class divergence kinds — exactly the failure classes their specificity audit injects. | We can reuse those divergence kinds to build FormaRL's recall/specificity audit offline (§4). |
| **SC (Lean compile / accept `sorry`'d statement)** | `components/verify/python/theoremata_tools/lean_repl.py`, `axioms.py`, `lean_soundness.py`; the formal 3+1 gate | Present and STRONGER (compile + `#print axioms` + kernel typecheck + soundness scan). FormaRL's SC ≈ our "well-formed statement compiles" check — a proper subset of our gate. `formalize_portfolio::StatementScreen.well_formed` is exactly SC; `.trivial` (the `triviality` tool) is an anti-hack guard FormaRL lacks. |
| **Reward = f(SC, CC)** | `components/train/python/theoremata_tools/reward.py` | `correctness_reward` = SC-analog (compile+axioms → 1.0/0.0). **There is NO CC/faithfulness term in the training reward today.** Adding `roundtrip.faithful_score` (or a hard SC∧CC gate) is a direct port of FormaRL's `Evaluator`. |
| **GRPO trainer, no-KL, group-normalized** | `components/train/python/theoremata_tools/grpo.py` | Already TRL-GRPO-shaped, verifier-as-reward, Goldilocks filter (drop all-pass/all-fail groups — same "no gradient" logic). FormaRL's `beta=0.0` no-KL matches; its AND-gate reward slots straight into `reward_funcs`. |
| **Auto-labeled data engine (unlabeled → reward)** | `components/train/python/theoremata_tools/flywheel.py`, `star_harvester.py` | FormaRL = a label-free flywheel for the *formalization* task specifically. Our flywheel already supports a formal oracle (hard 1.0/0.0) — FormaRL adds the CC dimension so *statements* (not just proofs) get auto-labeled. |
| **Best-of-N formalize, select first SC-pass** | `components/reason/proving/formalize_portfolio.rs` | Direct analog: fan one informal statement into N formal candidates, screen each (`well_formed`+`trivial`), surface the best. FormaRL's "first candidate passing SC proceeds" is our portfolio's screen-and-pick, and its pass@k selection rule (SC gates sampling, CC scores the winner) is a policy we can adopt directly here. |
| **Faithful-metric / dependency retrieval (future work they cite)** | our retrieval / dense index + `subsumption` dedup | FormaRL explicitly says BEq (bidirectional definitional equivalence) and dependency-retrieval augmentation "easily integrate" — we already have retrieval and canonical subsumption to supply both. |

**Net:** the statement-validation cluster maps *closely* — it is the same SC+CC idea,
but Theoremata currently uses CC (roundtrip) as a **review annotation**, while FormaRL
uses it as a **training reward**. That is the whole adoptable delta.

---

## 4. Buildable-now vs GPU/model-gated (honest split)

### Offline-buildable NOW (no GPU, no RL, high value)

The **reward / consistency-check *architecture* is entirely offline-buildable** — it
is a pure scoring function over (informal, formal) pairs plus a Lean compile. None of
the GRPO training machinery is needed to get the signal.

1. **A composite `formalization_reward(informal, formal)` in `reward.py`** =
   `SC ∧ CC`, reusing existing tools:
   - SC = `formalize_portfolio` `well_formed` (Lean compiles as a statement) AND NOT
     `trivial` (our `triviality` tool — an anti-hack guard FormaRL had to bolt on
     post-hoc; we get it for free).
   - CC = `statement_roundtrip.roundtrip_validate(...).faithful_score`, thresholded
     (or its `verdict != mismatch`). Port FormaRL's AND-gate: reward high only when
     SC ∧ CC both clear. Add FormaRL's exact anti-hack filters (reject empty /
     comment-only / NL-echoing `flp`; reject a degenerate always-compiling stub).
   - This is a deterministic offline function today (lexical roundtrip backend needs
     no model; `THEOREMATA_MODEL_MOCK=1` path exists).
2. **CC quality audit (recall/specificity), offline.** Reproduce FormaRL Table 1 for
   *our* roundtrip validator: take known-good (informal, gold-Lean) pairs from our
   corpus → measure recall; programmatically perturb the Lean (drop a hypothesis, flip
   ≤/≥, swap ∀/∃, inject ¬) — which our divergence taxonomy already enumerates → measure
   specificity. This tells us the threshold at which roundtrip is trustworthy as a
   reward vs. only as triage, and reproduces FormaRL's crucial pass@k-specificity
   warning before we ever spend a GPU-hour.
3. **Adopt the pass@k *selection policy* in `formalize_portfolio`.** FormaRL's lesson —
   **SC gates sampling selection, CC only scores the final winner** (because CC
   specificity collapses under high-throughput sampling) — is a pure control-flow
   change: pick the first SC-clean, non-trivial candidate; run the (expensive/noisy)
   CC roundtrip once on the winner, not on all N.
4. **STAR/flywheel harvesting of statements.** Feed unlabeled informal statements
   (e.g. our `uproof`-style corpus) through the portfolio + composite reward to
   auto-label a formalization dataset with provenance — offline, mock-oracle-driven —
   ready to become SFT/GRPO rows without touching a model.

### GPU / model-gated (honest — do NOT claim offline)

- **The actual GRPO RL training run** (the "4–6× pass@1" result): needs 2×A100-80G, a
  7B policy, TRL+accelerate, a live Lean-REPL pool at throughput, and a strong CC
  judge model (DeepSeek-V3 quality — they show a weaker Qwen judge degrades results).
  Our `grpo.py` is dry-run-by-default and explicitly never touches a GPU unless asked;
  wiring the composite reward into `reward_funcs` is offline, but *running* it is not.
- **The CC judge itself in production** wants a capable model. Offline we have the
  lexical/mock roundtrip backend (good enough for the audit and for triage); a
  training-grade CC reward wants the model backend, i.e. an inference budget.
- **`uproof`-scale data curation** (PDF→markdown→chunk→GPT-4o extract/validate) is a
  model-gated pipeline; the *prompt templates* are in the appendix and portable, but
  running them needs an LLM.

---

## 5. Prioritized adopt-list

1. **[Offline, high] Composite `SC ∧ CC` formalization reward in `reward.py`.** Add a
   `formalization_reward(informal, formal)` combining `formalize_portfolio` well-formed
   + `triviality` (SC) with `statement_roundtrip.faithful_score` (CC), AND-gated per
   FormaRL, plus their anti-hack filters (empty / comment-only / NL-echo / trivial-stub
   rejection). This is the single missing wire — CC already exists as advisory; make it
   a scorable reward. Keep the formal 3+1 gate as ground truth; this rides on top.
2. **[Offline, high] Roundtrip recall/specificity audit** (FormaRL Table 1 for our
   validator), using our divergence taxonomy to generate the perturbation set.
   Deliverable: a trust threshold + a pass@k-specificity caveat, so we know whether CC
   is reward-grade or triage-only on advanced math. Do this BEFORE any RL run.
3. **[Offline, medium] Adopt FormaRL's selection policy in `formalize_portfolio`:**
   SC gates candidate selection; run CC once on the SC-winner, not on all N. Cheap,
   directly reduces CC noise/cost.
4. **[Offline, medium] STAR/flywheel statement-harvesting** for label-free formalization
   data: portfolio + composite reward over an unlabeled informal corpus → provenance-
   tagged SFT/GRPO rows. Reuses `star_harvester.py` + `flywheel.py`.
5. **[Model-gated, medium] Wire the composite reward into `grpo.py`'s `reward_funcs`**
   and run the label-free GRPO loop when GPU budget exists — no-KL GRPO already matches
   our config. Expect their finding: biggest gains from-scratch, plateau on top of heavy
   SFT; a strong CC judge matters.
6. **[Model-gated, low] Curate a Theoremata `uproof` OOD split** via the appendix
   extract/validate prompts to measure our formalizer's advanced-math generalization
   (the gap FormaRL exists to expose).
7. **[Later] BEq + dependency-retrieval augmentation** (FormaRL's own future work):
   feed retrieval context into the formalize prompt and use bidirectional definitional
   equivalence in CC — we already have retrieval + `subsumption` to supply both.
