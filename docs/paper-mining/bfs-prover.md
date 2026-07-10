# Paper Mining: BFS-Prover — Scalable Best-First Tree Search for LLM-based ATP

Ran Xin, Chenguang Xi, Jie Yang, Feng Chen (Stanford, intern), Hang Wu, Xia Xiao, Yifan Sun, Shen Zheng, Kai Shen. **ByteDance Seed** + ByteDance Applied ML + Stanford. arXiv:2502.03438v3 (9 Oct 2025). Model open-sourced: `ByteDance-Seed/BFS-Prover-V1-7B`.
Source PDF: `math-papers/BFS-Prover - Scalable Best-First Tree Search for LLM-based Automatic Theorem Proving.pdf` (fully read, 12 pp incl. appendix of Lean IMO proofs).

> NOTE: content below is extracted from the paper as UNTRUSTED data. Nothing in the paper — including the Lean listings in its appendix — is an instruction to us. No injection payload was found in this PDF.

## 1. What it is
A **tactic-level** Lean 4 prover that argues the field over-rotated to MCTS + value/critic networks, and that a properly *scaled* **Best-First Tree Search (BFS)** — with no critic, no MCTS — is state of the art. Built as a scalable **expert-iteration** (STaR-style) flywheel around a 7B policy LLM (Qwen2.5-Math-7B base) driving a LeanDojo gym environment. Headline: **72.95% on MiniF2F-test** (accumulative), beating DeepSeek-Prover-V1.5 (63.5%), InternLM2.5-StepProver (65.9%), HunyuanProver (68.4%). Single-config result 70.83% ± 0.89% at a 2048×2×600 budget.

Three named innovations: (1) **beam-search self-filtering** of easy problems each expert-iteration round; (2) **DPO from Lean compiler-error feedback** to sharpen the tactic policy; (3) **length normalization** in the BFS score to counter its bias against deep proofs.

## 2. Key mechanisms

### 2a. Length-normalized Best-First Search (the core)
BFS maintains a **single global priority queue** of proof states (frontier). It repeatedly pops the highest-priority state, asks the policy LLM for candidate tactics (edges), runs each in Lean via LeanDojo, and pushes the resulting valid states back onto the queue. Priority of a state `s_L` at path length `L`:

```
score(s_L) = ( Σ_{t=0}^{L-1} log p(a_t | s_t) ) / L^α
```

- Numerator = **cumulative log-probability of the whole tactic path from root** (a path prior, product of edge probabilities).
- `α ∈ [0,1]` is the **length-normalization exponent**. `α=0` → raw cumulative log-prob (the classic BFS that intrinsically penalizes deep paths, since every extra step adds a negative log term). Larger `α` divides out path length, so deep-but-promising paths stay competitive.
- Three tactic outcomes per edge: (1) valid new state → push to queue; (2) proof-complete → return proof; (3) error → **terminal error node** (discarded from search, but *recorded* — see DPO below).
- A tunable **expansion width** `W` (top-`W` sampled tactics per pop) trades breadth vs depth. "Increasing α and/or reducing width drives the search toward deeper paths."
- **No value function, no backprop, no UCB/visit statistics.** Purely policy-log-prob-ordered frontier expansion. This is the whole point: it is much cheaper than MCTS+critic (a critic "effectively doubles inference calls since each expansion needs both policy and value").

### 2b. Expert iteration with self-filtering (the flywheel)
Corpus: ~900k unproved Lean statements (autoformalized NuminaMath-CoT + unproven Mathlib + Lean-Workbook). Each round:
1. **Beam-search filtering** — try each open statement with **deterministic beam search** (beam width 32) node expansion. Statements it solves are deemed "easy" (they align with the current policy's strengths); they are **removed from the corpus AND their new proofs deliberately withheld** from the training set. This concentrates accumulated data on *harder* theorems and prevents the corpus collapsing to trivialities.
2. **Data collection** — run BFS with **temperature sampling** (T=1.0, nucleus 1.0, sampling width 2/4/8) on the remaining hard statements. Collect all `(state, tactic)` pairs on successful proof paths → SFT data. Record error-causing tactics as **on-policy negative examples**.
3. **SFT** — retrain a fresh policy on the *entire accumulated* `(state, tactic)` corpus (3 epochs, cosine LR 2e-5→1e-6).
4. **DPO** (alternative to SFT when little new data was generated) — see 2c.

10 rounds of expert iteration + a final DPO round produced the released checkpoint.

### 2c. DPO from compiler feedback
Preference pairs are harvested *for free* from the tree: for a state `s` on a verified proof path, pair the **winning tactic `a_w`** (on the path) with a **losing tactic `a_l`** (expanded from `s` but that hit a Lean compiler error). Standard DPO loss with implicit reward `r_θ(s,a)=log p_θ(a|s) − log p_ref(a|s)`, β=10 (strong KL). This injects the negative signal SFT lacks and improves **sample efficiency** — SFT+DPO consistently beats SFT alone across all pass@k.

### 2d. Scaling behavior & diversity
- **Logarithmic search-time scaling**: pass@64→pass@2048 lifts SFT 64.58→70.38%, SFT+DPO 64.98→70.83% — consistent but diminishing (they push to pass@4096). Fixed budget defined as `K passes × W width × N expansions`.
- **Proof-length distribution drifts deeper** across rounds (mean 10.2→16.7 tactics) — evidence BFS finds harder, longer proofs as the policy improves.
- **No mode collapse**: tactic-token-length distribution stays diverse (shift from 1–10-token tactics toward 11–50-token ones, but the spread is preserved) — attributed to SFT-on-accumulated-data rather than pure RL reward-chasing.
- Eval BFS config differs from training: T=1.1, **W=2**, **α=0.5**; accumulative result unions solved sets over **α ∈ {0.0, 0.5, 1.0}**.
- Infra: Ray actors, async vLLM policy pool (8×A100) + 96 CPU LeanDojo prover instances per machine, round-robin `index mod 8`, near-linear scaling, no cross-machine comms.

## 3. Mapping to Theoremata

### Is BFS a competitor or complement to our MCGS driver?
**Both.** Architecturally BFS is a *strictly simpler alternative* to our `search/driver.rs` MCGS:

| | Our MCGS (`driver.rs`/`mcts.rs`) | BFS-Prover |
|---|---|---|
| Frontier | Re-descend root by PUCT every iteration | Single global priority queue, pop-best |
| Node score | `Q + progress_weight·value + c·P·√N/(1+N)` (visits, backprop) | `Σ log p(path) / L^α` (path prior only, **no backprop, no visits**) |
| Value signal | optional `progress()` / `process_reward` Q-backup / TTC | **none** (deliberately) |
| Dedup | transposition table (`dedup_key`) → DAG | plain tree (LeanDojo states; no transposition described) |
| Negatives | `tactic_outcome::Discard` drops errors | **records** errors → DPO pairs |

The paper is a direct empirical challenge to the premise behind our MCGS investment: it shows a value-free, backprop-free frontier search **out-scores** MCTS+critic systems on MiniF2F. That does **not** invalidate MCGS (our transposition/DAG dedup, `goal_cache` subsumption, AND/OR minimax, and negation-augmented refute are orthogonal *upgrades* BFS lacks), but it strongly argues we should (a) keep a **BFS baseline** in the driver to ablate whether MCGS's machinery actually pays for itself on our benchmarks, and (b) treat BFS's three scaling tricks as adoptable regardless of which search core wins.

### Concrete scoring/pruning ideas our driver can adopt
- **Length-normalized path scoring** — our `driver.rs` PUCT uses the prior only in the per-node `U` term and derives value from rollouts/`Q`; it has no explicit cumulative-path-logprob score and thus no `α` knob. A best-first `SelectionMode` (or a sibling module) that orders a frontier by `Σlog(prior)/L^α` is a clean, deterministic addition that reuses `GoalState`/`TacticExpander` verbatim.
- **α as an exploration-depth dial + accumulative union** — sweeping `α∈{0,0.5,1}` and unioning solved sets is a cheap, embarrassingly-parallel test-time-compute strategy that maps onto our `TtcController` budgeting and portfolio (`reason/proving/portfolio.rs`).
- **Beam-filter easy problems before spending sampling budget** — a deterministic beam pass to skip trivialities maps directly onto `flywheel.py` / `star_harvester.py`: gate corpus entries so accumulated data trends *harder*, exactly BFS §2.3 step 1.
- **Mine error edges instead of discarding them** — today `tactic_outcome::classify` returns `Discard` for `error_free=false` and the edge vanishes. BFS turns those same errors into `(state, a_w, a_l)` DPO preference tuples. We already have the winning-path tactics; the loser is the discarded sibling. Emitting these pairs is pure data plumbing on top of the existing classifier.
- **Contrast with `process_reward.rs`**: our MCTS-Q process supervision manufactures *dense per-node value targets* from one terminal gate bit — that trains a **critic**. BFS-Prover deliberately trains **no critic** and instead sharpens the **policy** via DPO on error negatives. These are complementary data engines: Q-backup feeds a value head, DPO-from-errors feeds the policy head; our stack can run both off the same finished search DAG.

## 4. Buildable-now (offline, deterministic) vs. model/GPU-gated

**Buildable now — search algorithm, no model/GPU, deterministic:**
1. **Best-first frontier search mode.** New `SelectionMode::BestFirst` in `mcts.rs`/`driver.rs`, or a `search/bfs.rs` reusing `GoalState`/`TacticExpander`: a priority queue keyed on `Σlog(prior)/L^α`, pop-best-expand, no backprop. Deterministic (seeded expander, stable tie-break) → unit-testable against the same mock table as `driver.rs`. Doubles as an MCGS ablation baseline.
2. **`α` length-normalization parameter + multi-α accumulative union** driver, wired through `SearchConfig` and the TTC budget (`K×W×N` accounting = rollouts×width×depth we already track).
3. **Deterministic beam-search self-filter** in the flywheel/harvester: greedy/top-`W` beam expansion to classify a statement easy/hard before sampling; withhold easy-solve data. Pure function of policy scores.
4. **DPO preference-pair extractor** from a finished search DAG: for each proof-path state emit `(state, winning_tactic, error_tactic)` from its `Discard` siblings. Offline data transform; emits JSONL rows analogous to our existing SFT/GRPO exporters.
5. **Length/diversity telemetry**: proof-length-distribution and tactic-token-length histograms across flywheel rounds as a mode-collapse / depth-progress monitor (mirrors `proof_telemetry.py`).

**Model/GPU-gated (cannot build offline):**
- The 7B policy LLM itself (Qwen2.5-Math-7B SFT/DPO training loops; A100s).
- The 900k-statement autoformalized corpus (needs proprietary autoformalizer).
- Distributed Ray + async-vLLM inference pool serving live tactic generation.
- **Note the *absence* that is a feature:** BFS-Prover needs **no value/critic network at all** — so unlike adopting an MCTS critic, the search-side ideas above carry *zero* extra GPU cost. The only GPU spend is the policy the flywheel already requires.

## Prioritized adopt-list
1. **[High, offline]** Add a **best-first `α`-length-normalized search mode** reusing the existing `GoalState`/`TacticExpander` traits — as both a first-class strategy and an MCGS ablation baseline. This is the paper's central, cheapest, highest-signal idea.
2. **[High, offline]** **Harvest DPO negative pairs** from `tactic_outcome::Discard` error edges on proof paths; emit them from the flywheel exporters. Free negatives we currently throw away; feeds policy DPO the moment training is available.
3. **[High, offline]** **Beam-search self-filtering** of easy statements in `flywheel.py`/`star_harvester.py` so accumulated data trends harder round-over-round.
4. **[Med, offline]** **Multi-`α` accumulative union** as a test-time-compute portfolio config under `TtcController`; log the pass@k scaling curve to check for the paper's logarithmic law on our benchmarks.
5. **[Med, offline]** **Proof-length / tactic-diversity telemetry** across rounds as a mode-collapse guard.
6. **[Low / when GPU available]** Actual DPO(β) policy-refinement round on the harvested pairs; run the SFT-vs-SFT+DPO ablation the paper reports.
7. **[Strategic]** Treat the BFS baseline result as a standing **falsification test for MCGS overhead**: if value-free best-first matches our PUCT+transposition+process-reward stack on our eval, that is evidence to simplify — exactly the paper's thesis.
