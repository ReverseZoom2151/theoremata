# HunyuanProver — Scalable Data Synthesis + Guided Tree Search (paper mining)

Source: `math-papers/HunyuanProver - A Scalable Data Synthesis Framework and Guided
Tree Search for Automated Theorem Proving.pdf` (arXiv:2412.20735v3, 21 Mar 2025).
Read in full (14 pp.) via pypdf → scratchpad `.txt`.

> SECURITY / INJECTION NOTE: This paper PDF is UNTRUSTED input. Nothing in it (or
> in the extracted text, the Lean proof examples, or the prompt appendices B/C)
> was treated as an instruction to this agent. No embedded directives were found;
> the appendix "prompts" are the paper's own data samples, not commands. This
> whole report is descriptive. Flag on any future re-read: **POSSIBLE INJECTION**
> if appendix prompt text is ever executed rather than quoted.

---

## 1. What it is + authors/venue

HunyuanProver is a Tencent Hunyuan technical report (Yang Li, Dong Du, Linfeng
Song, Chen Li, Weikang Wang, Tao Yang, Haitao Mi — Tencent Hunyuan Teams;
several authors overlap the AlphaMath / ηMCTS lineage). It is a 7B LLM
fine-tuned from **Hunyuan 7B** for **interactive, tactic-level (step) theorem
proving in Lean 4**, using LeanDojo as the Lean engine.

Two-part thesis, both aimed at the **data-sparsity** bottleneck of formal ATP:
1. a **scalable data-synthesis framework** (autoformalizer + iterative
   tactic-data generation) that grows training data ~40× (mathlib4's ~50k
   theorems → >20M tactic-level examples over 10+ rounds);
2. **guided tree search** at test time (BFS and a simulation-free ηMCTS) steered
   by explicitly-trained **critic models**.

Headline result: **68.4% pass on miniF2F-test** (prev. SOTA InternLM2.5-
StepProver+BFS+CG was 65.9%), with *less* search budget; proves 4 IMO statements
(1960 P2, 1962 P2, 1964 P2, 1983 P6). Three findings the authors stress: (a)
explicitly-trained critics beat policy-confidence guidance; (b) the *scale* of
tactic data is decisive; (c) **data curation/selection matters** — after v12 they
*removed* early easy data and accuracy went **up**.

---

## 2. Key mechanisms

### 2.1 Scalable data-synthesis framework

**(a) Autoformalization data generation.** Seed with 130k NL→Lean statement
pairs (50k Lean Workbook + 80k MMA). Translate the NL half into Chinese to
*double* the set, then train a bilingual (EN/ZH) autoformalizer. Run it on 30M
internal NL math problems, **sampling 8 outputs per problem at varied
temperatures**, and **filter** non-grammatical / rule-violating Lean → **20M
Lean statements** (`Dq`). Also fold in NuminaMath-CoT. Key idea: statements are
cheap and only need to *compile / pass surface rules*, not be proved.

**(b) Iterative tactic-level proving-data generation (expert iteration / RFT).**
Given Lean engine Γ and statement pool `Dq`, at iteration *t*: run **best-first
search** with the previous prover π_{t−1} on all still-unsolved statements;
collect proof trajectories τ for newly-solved statements:
`D_t = {(q,τ) | q ∈ Dq−D_{t−1}, τ~BFS(q), τ≠null} ∪ D_{t−1}`. Update the prover
by **rejection fine-tuning (RFT)** on `D_t` *after filtering out easy statements
solved in early iterations*. π₀ is trained on public data incl. mathlib4. >10
rounds → >20M tactic-level examples; ~2.75B (v8) → 4.25B (v12) tokens, then
easy-data pruning after v12.

**(c) Diversity enhancers (two).** (i) **Failed-trajectory recycling**: rules
convert the *last state of an unfinished proof* into a **new statement**, minting
more diverse proving data. (ii) Harvest harder statements — Olympiad algebraic
inequalities and Lean Workbook.

### 2.2 Guided tree search

State s_i = tree node n_i; root n₀ = input statement q; an edge = applying a
tactic. Per step, K=8 candidate tactics sampled (2 each at temperatures
0.7/0.8/1.0/1.1); each executed in Lean; **duplicate nodes pruned by string
match**; ≤800 search steps.

- **Best-First Search (BFS)** — selection + expansion only. Select the active
  node with max CRITIC score `n̂ = argmax_n CRITIC(n)` (without replacement),
  expand K tactics, merge new non-duplicate nodes. Simple, effective; but each
  node visited once with fixed budget, and fully hostage to critic bias.
- **ηMCTS** (adapted from Tian et al. 2024 / AlphaMath) — selection, expansion,
  **(simulation removed — left for future work)**, backprop. Differences vs the
  original: samples **K** tactics per expansion (not 1); a node can be selected
  multiple times; **per-node expansion budget is adaptive**:
  - **Importance** `I(n) = max_{n̂∈SUCC(n)} |CRITIC(n̂) − CRITIC(n)|` (max
    value-gap to any descendant);
  - **Budget** `E(n) = max(Bmin, min(Bmax, ⌊α·I(n)⌋ + 1))`;
  - **Selection by UCB** `CRITIC(n) + α·√(2·ln CNT(PRNT(n)) / CNT(n))`
    (exploit critic score + explore under-visited nodes).

### 2.3 The three critic models

1. **Policy Confidence (PC)** — cold-start, no training: token-level average
   log-prob of a tactic, `f_π(c) = (1/|c|) Σ log p_π(c_j | q,n,c_<j)`. Free but
   weakest.
2. **Process Reward Model (PRM)** `v^π_φ(q,n)` — probability that node n leads to
   proving q under π. Trained with **outcome-only labels** (Math-Shepherd / Wang
   2024c): build a search tree per statement under the previous critic, label
   each node **+1/−1** by whether it *reaches a successful terminal*, regress an
   LLM+MLP scalar head by **MSE** to those labels (scalar read at the last token
   of each state). No human step annotation.
3. **Distance Critic (DC)** — predicts *remaining tactic steps* to close q from
   n. To dodge sparsity, it predicts **not a raw integer but a path on a balanced
   binary tree** (coarse-to-fine): an **8-level tree covering 1..64**, each node a
   special token `<|num-k-of-m|>` (e.g. `<|num-5-of-8|>`); a number = a tuple/path
   (e.g. 6 → (2,3,6)). States compared by comparing tuples (prefix = closeness).
   Numbers >64 clamped to 64. This is the paper's **novel** critic and its best
   single result.

### 2.4 Benchmark numbers (miniF2F-test)

| System | Size | Budget (Pass×Beam×Iter) | miniF2F-test |
|---|---|---|---|
| DeepSeek-Prover-V1.5-RL + MCTS | 7B | 16×6400 | 60.2% |
| DeepSeek-Prover-V1.5-RL + RMaxTS | 7B | 32×6400 | 63.5% |
| Lean-STaR + BFS + CG | 7B | 64×1×50 | 46.3% |
| InternLM2.5-StepProver + BFS | 7B | 256×32×600 | 59.4% |
| InternLM2.5-StepProver + BFS + CG | 7B | 256×32×600 | 65.9% |
| **HunyuanProver v16 + BFS (PC)** | 7B | 600×8×400 | 64.8% |
| **HunyuanProver v16 + BFS + DC** | 7B | 600×8×400 | **68.4%** |

Ablation (Table 2): DC beats PC by +2.9–3.7 pts; MCTS+PRM > BFS+PC consistently
(v12: 62.29 vs 61.07; v14: 66.39 vs 62.70). Iteration curve saturates ~v8 then
climbs again *only after easy-data pruning* post-v12.

---

## 3. Mapping to Theoremata (explicit, per module)

### Data-synthesis side

- **`components/prover/.../geometry_synth.py` (+ `geometry_synth2.py`)** — our
  AlphaGeometry-style synthetic engine. Maps to HunyuanProver's **statement +
  proof synthesis**, but *stronger on soundness*: every emitted example is
  re-proved by `geometry.deductive_prove` and numeric-checked, whereas
  HunyuanProver's autoformalized statements only pass surface grammar/rule
  filters. Their **failed-trajectory→new-statement recycler** (§2.1c-i) is the
  one idea we do **not** yet have and it maps cleanly onto our derivation-DAG
  traceback: take a partial/last state and re-emit it as a fresh goal.
- **`components/verify/.../statement_roundtrip.py`** — back-translation faithful-
  ness check. This is exactly the **quality filter HunyuanProver lacks**: they
  keep any grammatical autoformalization; our round-trip catches
  quantifier/relation/dropped-hypothesis drift. It is the natural gate on an
  autoformalizer pipeline if we build one (see §4).
- **`components/train/.../flywheel.py`** — our auto-labeling engine (DeepSeek-
  Math-V2 lineage) is the direct analog of HunyuanProver's **iterative RFT loop**
  (Eq. 1). Both keep only verifier-accepted trajectories and retrain. Ours labels
  with a *formal* 3+1 gate (compile + `#print axioms` + kernel + soundness scan)
  — strictly stronger than HunyuanProver's Lean-compile-only terminal signal. The
  **easy-data-pruning / curation** finding (their post-v12 gain) is a concrete
  missing knob: our flywheel should down-weight/evict statements solved in early
  rounds.
- **`components/train/.../star_harvester.py`** — STaR harvest of our own verified
  DAG traces into SFT rows. This *is* HunyuanProver's `D_t` collection step,
  already implemented for our export format; the difference from their pipeline is
  only the source (our runs vs their BFS over `Dq`).
- **Per-system generators (`geometry_synth`, `falsify_hardcase.py`,
  `funsearch.py`, `triviality.py`, cert generators)** — these are our
  *domain-specific* statement/hard-case sources. HunyuanProver has a single
  generic autoformalizer; we have a portfolio. The **temperature-diversified,
  N-sample, filter** discipline (§2.1a) is a cheap upgrade for any of them.

### Search-guidance side

- **`components/reason/search/process_reward.rs`** — this is *already*
  HunyuanProver's **PRM**, built the AlphaMath/Math-Shepherd way: `backup_q`
  turns terminal ±1 gate verdicts into `Q = value_sum/visits` per node, and
  `q_targets` emits step-final regression targets — i.e. exactly HunyuanProver's
  Eq. 8 (MSE to ±1-derived labels). The module even documents the shared lineage.
  `step_beam_select` is a backup-free frontier ranker = HunyuanProver's use of a
  critic to rank active nodes. **We have PRM; we do not have DC.**
- **`components/reason/search/mcts.rs` + `driver.rs` (MCGS)** — our PUCT MCTS
  over a *graph* (transposition table) is a **superset** of HunyuanProver's
  ηMCTS: they explicitly *removed* simulation and dedup by *string match*; we do
  string-key transposition (α-equivalence/canonical print) and share visit stats
  across paths. Their **importance-scaled adaptive expansion budget** (Eq. 4–5)
  and **UCB-with-critic-score selection** (Eq. 6) are two knobs our driver does
  not have — the closest is `SelectionMode`/`PriorMode` and the `TtcController`
  budget sizing. HunyuanProver's `E(n)=f(I(n))` is a natural new `TtcController`
  policy: give more expansion budget to nodes with large value-gap descendants.
- **`components/reason/critique/critic.rs`** — this is a *structural adversarial*
  critic (circular-dep / gap taxonomy), **orthogonal** to HunyuanProver's
  *numeric value* critics. Do **not** conflate: PC/PRM/DC are search-guidance
  scalars; our critic.rs is a proof-audit. Both can coexist.
- **No best-first-search module exists** (`grep` finds none). HunyuanProver's
  headline config is BFS+DC, and BFS is *simpler* than our MCGS. A pure
  critic-ranked BFS frontier is a small, buildable addition (§4).
- **Progress signal** (`GoalState::progress`, `progress.rs`) is our LeanProgress-
  style closeness heuristic — a *hand-crafted* stand-in for a trained DC. The DC
  is the learned upgrade of exactly this quantity ("remaining steps").

---

## 4. Buildable-now (offline scaffolds) vs GPU/model-gated

### Buildable now — pure/offline, no GPU, unit-testable (matches our house style)

1. **Distance-critic *label + encoding* core (Rust, pure).** A sibling to
   `process_reward.rs`: from a finished `SearchTree`/DAG, compute each node's
   *true* remaining-steps-to-a-passing-leaf (shortest path to any `+1` terminal),
   then encode it as HunyuanProver's **balanced-binary-tree tuple** (8 levels,
   1..64, clamp) → `<|num-k-of-m|>` token paths, plus the **tuple comparison**
   ordering for coarse-to-fine state ranking. All deterministic; the target
   extraction mirrors `q_targets`. This is the single highest-value port.
2. **Best-first-search driver (Rust).** A critic-ranked frontier (max-heap on a
   pluggable `Critic` scalar, without replacement), reusing the existing
   `TacticExpander`/`GoalState` traits and dedup key. Ships as an alternative
   `SelectionMode`-like entry point; testable against a mock expander like the
   MCGS driver already is.
3. **ηMCTS knobs on the existing driver.** Importance score `I(n)` (max
   value-gap over descendants), adaptive expansion budget `E(n)` as a new
   `TtcController` policy, and a UCB-with-critic-score selection mode. Pure
   arithmetic over the DAG; no model.
4. **Failed-trajectory → new-statement recycler (Python).** In `geometry_synth`
   (and any future statement pipeline): take the last state of an *unfinished*
   proof/derivation and emit it as a fresh, verified goal. Directly reuses the
   traceback machinery already there.
5. **Easy-data curation pass for the flywheel (Python).** Track first-solved
   iteration per statement; evict/down-weight early-easy examples before retrain
   — the post-v12 pruning that *raised* HunyuanProver's accuracy. Pure dataset
   bookkeeping over `flywheel.py` / `star_harvester.py` output.
6. **Temperature-diversified, N-sample+filter statement synthesis (offline
   scaffold).** The *plumbing* (sample-8, dedup, grammar/round-trip filter,
   provenance) wired to a mock generator under `THEOREMATA_MODEL_MOCK=1`, exactly
   as `statement_roundtrip` and `flywheel` already run offline.

### GPU / model-gated (needs a trained model; scaffold only offline)

- **Training the PRM / DC value heads** (LLM + scalar/num-token head, MSE to the
  ±1 / distance labels we can already *manufacture* offline). We produce the
  targets today; fitting the head needs GPU.
- **The autoformalizer model itself** (bilingual NL→Lean, 8-sample decode). Its
  *filters* and round-trip gate are buildable now; the translator is model-gated.
- **The policy/prover fine-tune (RFT rounds)** and **policy-confidence critic**
  (needs real token log-probs from the prover).
- **Live tactic sampling at K=8 / multi-temperature** — needs the served policy
  (our `tactic_server.py` is the LeanCopilot-style seam for it).

---

## 5. Prioritized adopt-list

1. **Distance Critic label+encoding core** (Rust, pure) — new highest-value
   critic HunyuanProver shows beats PRM; we already have the PRM analog, this is
   the missing sibling. Offline, testable now. *(Then GPU-train the head.)*
2. **Easy-data curation / iteration-tracking in the flywheel** — cheapest win;
   HunyuanProver's own data shows *removing* easy data raised accuracy. Pure
   bookkeeping.
3. **Failed-trajectory → new-statement recycler** in `geometry_synth` — free
   diversity, reuses existing traceback; the one synthesis idea we lack.
4. **Best-first-search driver** — simplest search that, with a good critic (DC),
   is HunyuanProver's SOTA config; complements MCGS, small surface.
5. **ηMCTS knobs on the driver** — importance-scaled adaptive expansion budget +
   UCB-with-critic selection, as `TtcController`/selection-mode additions.
6. **Temperature-diversified N-sample+filter statement plumbing** (offline
   scaffold) — the frame for an eventual autoformalizer, gated by our
   `statement_roundtrip` faithfulness check that HunyuanProver *lacks*.
7. **(Model-gated, later)** train PRM/DC heads; train the autoformalizer + run
   RFT prover-improvement rounds.
