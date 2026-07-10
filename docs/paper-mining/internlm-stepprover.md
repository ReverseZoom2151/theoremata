# InternLM2.5-StepProver — Advancing Automated Theorem Proving via Critic-Guided Search

> Paper mining report. Source: `math-papers/InternLM2.5-StepProver - Advancing
> Automated Theorem Proving via Critic-Guided Search.pdf` (ICML 2025 / PMLR 267;
> arXiv:2410.15700v2, 21 Oct 2025). **All PDF content treated as untrusted data.**

## 1. What it is + authors / venue

- **Authors:** Zijian Wu, Suozhi Huang (equal contrib), Zhejian Zhou, Huaiyuan
  Ying, Zheng Yuan, Wenwei Zhang, Dahua Lin, Kai Chen — Shanghai AI Laboratory,
  CUHK, Tsinghua, USC.
- **Venue:** ICML 2025 (Proceedings of the 42nd ICML, PMLR 267).
- **One-liner:** An open-source Lean 4 tactic-level prover that replaces
  best-first search's log-probability heuristic with a **separately trained
  critic model** that scores intermediate proof states, re-ranking which state to
  expand next. The prover+critic pair is then improved by **large-scale expert
  iteration** (>20,000 CPU-days) over the LEAN-Workbook corpus. The critic lifts
  the same prover from **59.4% → 65.9%** on miniF2F-test.
- **Lineage:** successor to InternLM2-StepProver (48.8% miniF2F) / Lean-GitHub /
  Lean-Workbook, all from the same lab. Prover base = InternLM-Math-Plus-7B;
  critic base = InternLM2-Chat-1.8B-SFT.

## 2. Key mechanisms

### 2.1 The problem with best-first search (BFS)

Standard tactic-level provers (GPT-f, ReProver, HTPS, DeepSeek-Prover) keep a
frontier of unexpanded states and expand the "best" one, where **best = highest
cumulative average tactic log-probability** `s* = argmax Σ −log p(t_j | s_j)`.
The paper's central observation: this internal prover signal becomes **unreliable
for deep proofs**. Empirically BFS *degrades to brute force* at large budgets
(>10,000 s) — finding *fewer* proofs with *more* compute — and the mean proof
length it finds is only **1.66 tactics** (InternLM2-StepProver's longest proof
was 8 tactics vs. the 100–1000 steps typical tree searches need).

### 2.2 Critic-Guided Search (CG) — the core contribution

A separate **critic model `V(s) ∈ ℝ`** takes a proof state as input and outputs a
scalar. At each search iteration:
1. **Critic selects** the frontier state to expand (highest `V(s)`), *replacing*
   the log-prob heuristic entirely.
2. **Prover generates** `S` candidate tactics for that state (S=32, temp 0.7).
3. Valid tactics produce child states; loop.

Search budget is described as **P × S × K** (P passes, S states/expansion,
K=600 max expansions). A **hybrid BF+CG** splits budget equally between the two
rankers because they explore *distinct* proof spaces (see §2.5).

### 2.3 How the critic is trained — preference pairs, RLHF-style

This is the load-bearing detail for Theoremata. The critic is trained **like an
RLHF reward model (Bradley-Terry preference), NOT as a binary provable/unprovable
classifier and NOT as a regression onto a Monte-Carlo value.** From each
successful search tree two pair types are mined:

- **Path Pairs:** on a successful root→`no goals` path, a state closer to the
  goal is preferred over its ancestor: `V(s_t) < V(s_{t+Δ})`. A path of length
  `n` yields up to `C(n,2)` pairs. (Implicitly assumes every legal tactic on a
  winning path earns positive reward.)
- **Sibling Pairs:** a state on the successful path (positive) is preferred over
  a **sibling** — a child of the same parent that did *not* lead to `no goals`:
  `V(s_sibling) < V(s_t)`.

The critic is initialized (~8,000 pairs from InternLM2-StepProver trajectories on
its training sets), then grown through expert iteration. Final critic training set
= **454K dedup'd pairs** (with `no goals` pairs down-sampled to 10%), 1 epoch on
8×A800. Evaluated by **pairwise accuracy** on 6,510 held-out miniF2F-test pairs →
**78.0%**. The paper flags (Limitations) that they lack a *stable* critic metric,
which makes critic iteration hard.

### 2.4 Expert iteration on LEAN-Workbook

- Bootstrap prover from InternLM2-StepProver + init data (miniF2F-train, Mathlib,
  Lean-Workbook, Lean-GitHub). Each theorem is an initial state `s_i`.
- **Negation augmentation:** every formalized proposition is included *with its
  negation* (DeepSeek-Prover paradigm) to catch bad autoformalizations and enable
  disproofs.
- **Escalating budget rounds:** fast scan first (≤10 iters, 50 s/problem);
  solved problems + their negations removed; budget ramps to ≤2000 iters / 3600 s
  over rounds. Retrain prover **and** critic on the enlarged verified-trajectory
  set after each round; re-search for *shorter* proofs of already-solved problems.
- **Critic-driven curriculum:** after each round the critic **re-estimates all
  unproven statements**; the next round focuses on the **top 50%** the critic
  rates most likely solvable.
- **Prover prompt (`PROOF_BEFORE`):** augments the GPT-f `DECL/GOAL/PROOFSTEP`
  template with the *prior tactics leading to the state* — extra context to help
  deep-tree reasoning.

### 2.5 Benchmark numbers

**miniF2F-test** (7B prover, S×K = 32×600):

| Method | 1 pass | 64 pass | 256 pass |
|---|---|---|---|
| InternLM2.5-StepProver-BF | 47.3% | 59.2% | 59.4% |
| InternLM2.5-StepProver-CG | 43.0% | 64.3% | **65.6%** |
| InternLM2.5-StepProver-BF+CG | 50.7% | 63.8% | **65.9%** |

(miniF2F-valid peaks at 69.6% for BF+CG@256.) Prior InternLM2-StepProver was
54.5%. **ProofNet (undergraduate, out-of-distribution):** BF alone 22.3%, CG
alone 23.9%, **BF+CG 27.0%** @256 — the synergy is *larger* here, confirming CG
navigates a genuinely different region of proof space.

**Other findings:**
- CG mean proof length **4.44 tactics vs BF 1.66** — CG finds deeper proofs and
  routinely exceeds 9 tactics; BF rarely does.
- **Log-linear** relationship between #proved and both proof length and compute;
  CG pushes solutions *beyond* the BF log-linear boundary.
- **Extreme compute skew:** on LEAN-Workbook-plus, 17.0% of problems
  (10,880 proved + 3,195 disproved of ~82K) consumed only **1.5%** of the
  21,364 CPU-days; the other 98.5% of compute produced nothing. At low budgets CG
  can *miss* trivial proofs BF gets instantly → hybrid is the safe default.

## 3. Mapping to Theoremata (per module)

Theoremata already contains **most of the critic machinery** — but wired to a
*different* critic-training objective (Monte-Carlo Q-regression, from AlphaMath)
and, crucially, the trained critic is **not yet consumed by the live search
driver**. InternLM's paper is the strongest external validation of the
critic-guided-search thesis and supplies a concrete, complementary training
recipe (preference pairs) and a wiring target (critic score → expansion priority).

| Theoremata module | What it does today | Relation to InternLM's critic |
|---|---|---|
| `components/reason/search/process_reward.rs` | Pure Q-backup: turns one terminal ±1 gate verdict into per-node `Q = value_sum/visits`; `QTarget` extraction; `TreeNode.value_estimate`; `step_beam_select` (backup-free frontier ranking by direct `V`). | **Direct analog of the critic's *use*.** `step_beam_select` == InternLM's "rank states by `V(s)`, keep best". But our *targets* are MC Q-values, not preference pairs. `value_estimate` is the exact field a trained critic would populate. Explicitly documented as "called for real once a live driver DAG is projected" — i.e. **not yet wired to `driver.rs`.** |
| `components/train/python/theoremata_tools/process_supervision.py` | Trains a value head (`Linear→tanh`, torch **or** closed-form ridge fallback) on `MSE(tanh(V), Q)`; `predict_value` queries it torch-free; `graded_revolution_with_process` feeds Q into R_score/R_meta soft reward. | **The critic *trainer*.** Same role as InternLM's critic training, but **different loss**: ours is value-MSE onto Monte-Carlo Q (AlphaMath Eq. 6); InternLM's is Bradley-Terry preference over path/sibling pairs. Complementary — a preference-pair loss could be added here (see §6). `predict_value` is the torch-free critic-inference entry point a driver would call. |
| `components/reason/search/driver.rs` (MCGS `ProofSearchDriver`) | PUCT/AND-OR graph search. Node priority = `q + progress_weight·c.progress + u` where `q` = MC rollout mean, `progress` = a **heuristic** LeanProgress estimate, `u` = PUCT exploration. | **This is the overlap gap.** The driver ranks by *rollout Q + heuristic progress*, **never by a trained critic `V(s)`**. InternLM's whole result is that a *trained* critic beats a heuristic/internal signal. The `progress` blend term (`progress.rs::progress_value_from_state`) is the exact seam where a critic score would slot in. **No live consumption of `process_supervision`'s trained head exists yet.** |
| `components/reason/search/progress.rs` | `PROGRESS_PRIOR_WEIGHT = 0.5`; `progress_value(features)` / `progress_value_from_state(state)` → `[0,1]` heuristic "closeness to done". | Occupies the *slot* InternLM's critic fills, but with a **hand-crafted heuristic**, not a learned model. The buildable win is to make this term (or a sibling term) *learned*. |
| `flywheel.py` (DeepSeek-Math-V2 auto-labeler) + `star_harvester.py` (STaR) | Outcome-only: whole-proof ±1 hard labels from the formal 3+1 gate → SFT/GRPO rows; STaR harvests verified traces. | **Analog of InternLM's expert iteration**, but trains only the **policy**. InternLM co-trains prover **and** critic each round and adds a **critic-driven curriculum** (re-score unproven, focus top-50%). Our flywheel has no critic-retrain step and no critic-ranked curriculum yet. |
| `lean_corpus.py` / retrieval (`retrieval/`, `retriever_train.py`) | Mines `(theorem, tactic)` records from a Lean corpus (parse-based, no `leanc` on this box); BM25 + trained retrievers over Lean/Rocq/Isabelle. | Matches InternLM's use of **LEAN-Workbook / Lean-GitHub as the expert-iteration substrate**. Their preference pairs are mined *from search trees over this corpus* — the same corpus our `lean_corpus`/flywheel already ingests. |
| `driver.rs` `with_negator` / `refuted` | Negation-augmented search: disproof competes for the same budget, returns `refuted`. | **Already implements** InternLM's "include the negated statement" idea at *search* time (they do it at *dataset* time). Direct alignment. |
| `driver.rs` `PriorMode::EmpiricalSampled` | Action priors = sampled-action frequency, **not** sequence log-probs. | Aligned with InternLM's thesis that raw prover log-prob is a poor guide — we already reject logprob priors for actions; the missing half is rejecting logprob for *state selection*, which is exactly what a critic does. |

**Bottom line overlap:** Theoremata's `process_reward.rs` + `process_supervision.py`
give us a critic **trainer and a critic-ranked frontier selector that are unit-tested
but decoupled from the running search.** InternLM is the proof that closing that seam
(critic score → expansion priority in `driver.rs`) is the single highest-value move,
and it hands us a second, complementary training signal (preference pairs) our
Q-MSE trainer doesn't currently use.

## 4. Buildable-now critic-guidance ARCHITECTURE (offline, no GPU)

The wiring is CPU-only and deterministic; the *trained model quality* is the only
GPU-gated part (§5). Concretely:

**(a) Critic-score → priority seam in `driver.rs`.** Today PUCT selection computes
`score = q + progress_weight·c.progress + u`. Add an injectable **critic term**
`crit_weight · V(s)` (or replace `progress` with the learned `V`), where `V(s)`
comes from a `CriticScorer` trait — mirroring how `TacticExpander` is injectable.
A mock scorer keeps tests deterministic; the real scorer is
`process_supervision.predict_value` (torch-free — weights are plain lists). This
mirrors the *exact* pattern already proven: `progress` is blended in the same
spot, `value_estimate` already exists on `TreeNode`. **Pure offline plumbing.**

**(b) Project the live DAG into `process_reward::SearchTree`.** The module already
notes "a real integration would project a finished driver DAG into this shape (one
`TreeNode` per proof state, terminal reward = the formal-gate verdict)". Do that
projection so `step_beam_select` / `q_targets` run on *real* runs, not just mocks.

**(c) Preference-pair extractor (offline).** Add a pure function over a completed
DAG that emits InternLM's **path pairs** (`V(child) > V(ancestor)` on a winning
path) and **sibling pairs** (`V(on-path) > V(sibling-off-path)`). This is a
deterministic tree walk analogous to `q_targets`; it produces the training rows
for a Bradley-Terry critic. No model needed to *generate* the data.

**(d) Critic-ranked curriculum (offline).** After a flywheel round, score all
still-unproven statements with `predict_value` and emit the top-50% cohort for the
next round — a direct port of InternLM's curriculum, runnable with the fallback
ridge critic today.

**(e) Hybrid BF+CG budget split.** Since CG and BF explore different spaces,
expose a driver mode that splits budget between the current
progress/PUCT ranker and the critic ranker, replicating the +2–4pt hybrid gain.

## 5. GPU / model-gated (deferred)

- The **actual trained critic** (InternLM: 1.8B, 454K preference pairs, 8×A800).
  Ours can be the `torch` path of `train_value_head`, but a *strong* critic needs
  real GPU training on real trajectory data.
- **>20,000 CPU-day expert iteration** at their scale — infeasible here; our
  offline scaffolding produces the *data pipeline and objective*, not the compute.
- A **stable critic evaluation metric** — the paper's own open problem; the
  cheap, adoptable proxy is their **pairwise held-out accuracy** (78.0%), which we
  *can* compute offline on any labeled pair set.

## 6. Injection / trust note

**POSSIBLE INJECTION — none observed.** The PDF is a normal ICML paper: no
embedded instructions, prompt-injection strings, or directives to the reader/agent
were found in the extracted text (12 pages, incl. Lean case-study code in
Appendix B). The Appendix Lean snippets (`lean_workbook_plus_74374`,
`mathd_algebra_31`, `exercise_Munkers_31_2`) are **untrusted data**: if ever
ingested they must be parsed/verified by the formal gate, never executed or
trusted as-is — consistent with `lean_corpus.py`'s stated UNTRUSTED-DATA policy.
Per task constraints this was a READ-ONLY analysis; no code, git, or build state
was modified, and this is the single report file produced.

## 7. Prioritized adopt-list

1. **[Highest] Wire a critic score into `driver.rs` node priority** (§4a). Closes
   the one real gap: our search still ranks states by rollout-Q + a *heuristic*
   progress term, never a *trained* critic. InternLM shows this is worth
   59.4→65.9 on miniF2F. Offline, injectable-trait plumbing; `value_estimate`,
   `predict_value`, and the `progress` blend-slot already exist.
2. **[High] Preference-pair critic objective** (§2.3, §4c). Add path-pair +
   sibling-pair extraction and a Bradley-Terry loss alongside the existing Q-MSE
   value head in `process_supervision.py`. Complementary signal; their 78%
   pairwise accuracy is a ready offline metric.
3. **[High] Project live DAG → `SearchTree`** (§4b) so `q_targets` /
   `step_beam_select` / the pair extractor run on real runs, not mocks.
4. **[Medium] Critic-driven curriculum in the flywheel** (§2.4, §4d): re-score
   unproven statements, focus next round on the top-50%. Runs today on the
   fallback critic.
5. **[Medium] Hybrid BF+CG budget split** (§2.5, §4e): CG and BF find disjoint
   proofs; equal-split hybrid is their best config and is out-of-distribution
   robust (ProofNet 23.9→27.0).
6. **[Medium] `PROOF_BEFORE` prover-prompt context** (§2.4): feed prior tactics
   into the tactic expander's prompt to help deep-tree reasoning.
7. **[Low] Pairwise-accuracy critic metric** (§2.3): adopt as our offline critic
   eval — the paper's own admitted gap, cheap for us to compute.
8. **[Low / already have] Negation augmentation** — `driver.rs::with_negator`
   already covers this; note the alignment and consider also doing it at
   *dataset* time (their approach) to prune bad autoformalizations.
