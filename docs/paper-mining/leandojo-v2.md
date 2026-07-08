# LeanDojo-v2: A Comprehensive Library for AI-Assisted Theorem Proving in Lean

*Mining report. Source: Hsiang, Adkisson, George, Anandkumar (NTU / WUSTL / Caltech). NeurIPS 2025 Workshop: MATH-AI. `math-papers/LeanDojo-v2 - A Comprehensive Library for AI-Assisted Theorem Proving in Lean.pdf`, 11 pages incl. appendices, fully read. All content below is a report on an untrusted PDF; no embedded instructions were followed.*

> **Framing caveat up front:** This is a **systems / library integration paper**, not a new-algorithm paper. It unifies existing components (LeanDojo v1 extraction, ReProver retrieval, LeanAgent curriculum training, Pantograph REPL/search, HuggingFace SFT/RL) behind one Python API plus a VSCodium-based IDE ("Lean4Code"). It contains **no benchmark tables, no ablations, and almost no new retrieval-model detail**. The deep retrieval architecture our task asks to mine (encoder, embedding dim, negative sampling, top-k) lives in the **original LeanDojo v1 / ReProver paper [ref 1, arXiv:2306.15626]**, which this paper only reuses. Every place a detail is *inherited-but-unspecified* vs. actually stated here is flagged.

## Core contribution

LeanDojo-v2 is an end-to-end, single-library unification of the previously fragmented Lean+AI tooling stack: repository data extraction, LLM fine-tuning (SFT / LoRA / GRPO / PPO), API-based inference, proof search, and whole-proof generation, all behind standardized abstract-class APIs (`BaseAgent`, `BaseProver`, `SFTTrainer`, `HFAgent`, `LeanAgent`, `ExternalAgent`). It ships **Lean4Code**, a VSCodium-based editor with the Lean4 extension pre-bundled (claimed "first Lean-native code editor") plus one-click panels for LeanDojo tracing, LeanCopilot tactic suggestion, and ByteDance Agent-TARS. The stated goal is to let mathematicians fine-tune models on domain-specific Lean codebases and run LLM-assisted proving without a complex setup.

## Key techniques / architecture

**Extraction pipeline (how they trace Lean).** They do *not* reimplement extraction — they call **LeanDojo** (v1) to trace a repo. Given a GitHub URL + commit hash (+ Lean version + optional dependency build toggle), LeanDojo extracts **abstract syntax trees, dependencies, theorems, proofs, and premises**. Proofs are then **decomposed into a sequence of (proof state, tactic) pairs**: the proof state is the model input, the tactic is the prediction target. This is the standard LeanDojo tracing contract; the paper adds a one-click wrapper (Lean4Code's LeanDojo panel) that clones the repo + LeanDojo-v2, generates a configured Python trace script, and dumps trace artifacts into an `out/` folder.

**Premise-retrieval model (ReProver, inherited).** Training here operates on **ReProver's premise retriever**. The only *new* thing this paper says about retrieval is the **training schedule**: they adopt LeanAgent's **progressive / curriculum learning**. Theorems are bucketed into **easy / medium / hard by the 33rd and 67th percentiles of proof length (number of proof steps)**; repositories are **sorted by how many *easy* theorems they contain**; the retriever is trained repo-by-repo from easiest to hardest (one training run per additional repo). Appendix mentions a **"Byte5 retriever model"** downloadable locally to power LeanCopilot (likely a ByT5-based dense encoder — the ReProver retriever in v1 is a ByT5 dense encoder; the encoder, embedding dim, negative-sampling scheme, and top-k are **NOT restated in this paper**). The in-file vs. cross-file premise distinction — a central v1 contribution — is **not discussed here**.

**Proof-state interaction environment (Gym-like tactic stepping).** Notably, v2 **replaces LeanDojo's own REPL with Pantograph** [ref 8] as both REPL and search driver. The formulation: each **goal state = node**, each **tactic = edge**, search starts at the theorem's initial goal and traverses until a terminal node with no remaining goals (proof complete). **Failed/erroring tactics leave the goal state unchanged** (no-op edge). Pantograph supplies both **DFS and MCTS** to build the "goal-tactic graph." After a proof is found, they run a **shortest-path algorithm over the graph to reconstruct a minimal-length tactic sequence** from initial to terminal state. Tactic generators are pluggable: LeanAgent, fine-tuned HF models, or API inference. The prover is a **base class (`BaseProver`) extensible to ML and non-ML provers.**

**What v2 ADDS over v1 (the crux):**
- **Unified library + standardized APIs** over previously separate research repos; abstract `BaseAgent`/`BaseProver` extension points.
- **HuggingFace SFT trainer** for fine-tuning *any* HF model on the (state → tactic) data, **with LoRA** support for cheap memory.
- **RL trainers: GRPO and PPO**, with **user-defined reward functions** (e.g., syntax correctness, proof success).
- **API inference path**: direct HuggingFace inference-provider calls (e.g., `DeepSeek-Prover-V2-671B:novita`) for models too large to fine-tune — no local inference, no C++ libs. Explicitly contrasts with LeanCopilot: removes CTranslate2 build, **cutting build time from >10 min to seconds**, and reduces latency.
- **Whole-proof generation** path (single forward pass) in addition to step-by-step search.
- **REPL swap to Pantograph** (DFS + MCTS + shortest-path proof recovery) instead of v1's own interaction gym.
- **Lean4Code IDE**: VSCodium fork, Lean4 extension pre-bundled, LeanDojo-v2 injected as a config folder into any created Lean project; three one-click panels (LeanDojo trace, LeanCopilot, Agent-TARS).
- **Lean version robustness claim**: they commit to updating Lean4Code's bundled Lean4 extension "with every major release"; projects use the Lean4 version of the latest LeanDojo-v2 release (acknowledged as a maintenance-driven, not architectural, guarantee).

## Results / benchmarks

**None.** There are no evaluation tables, no pass@k numbers, no benchmark splits, no ablations, and no timing tables other than the qualitative "build time >10 min → seconds" claim for the API-inference path vs. LeanCopilot/CTranslate2. This is purely a capabilities/architecture paper. Any quantitative retrieval or proving numbers must be sourced from the underlying papers (LeanDojo v1 [1], LeanAgent [3], DeepSeek-Prover-V2, Pantograph [8]).

## Novel vs SOTA-2026

- **Novelty is integration/UX, not method.** The genuinely new artifacts are (a) the single unified Python API surface with pluggable trainers/provers, and (b) Lean4Code as a "batteries-included" Lean-native editor for non-programmer mathematicians. Both are engineering contributions.
- Against 2026 SOTA provers (DeepSeek-Prover-V2, Kimina, whole-proof RL systems), the paper **does not compete on capability** — it wraps them as backends (API inference, whole-proof generation).
- The **shortest-path proof-minimization over the search DAG** and the **failed-tactic-is-a-no-op-edge** graph semantics are clean, reusable formalizations, though not themselves novel.
- Compared to our own harness ambitions, LeanDojo-v2 is *behind* on: falsify-before-prove, portfolio across Lean/Rocq/Isabelle (it is Lean-only), transposition/dedup in search (unspecified — just DFS/MCTS via Pantograph), reranker cascades, and lemma-cache/growing-library. It is *ahead* on: turnkey packaging and a real IDE UX.

## Adopt-relevance to Theoremata

**Premise retrieval (our BM25 + dense + reranker cascade).** Low direct yield from *this* paper — it reuses ReProver and adds no new retrieval architecture. The one adoptable idea is the **curriculum/progressive training of the retriever**: bucket theorems easy/medium/hard by proof-step-count percentiles (33/67), sort corpora by easy-theorem count, train easiest-first. This is a cheap, concrete schedule we could apply to our dense encoder and/or reranker fine-tuning. For the actual encoder/negatives/top-k design we should mine **LeanDojo v1 (2306.15626)** directly, not this paper. **Gap vs. us:** they have *no* reranker and *no* BM25 stage — our cascade is strictly more capable; nothing to borrow there.

**Proof-state environment (our 3+1 gate / interaction env).** Here is the highest-value takeaway. They **abandoned LeanDojo v1's homegrown REPL in favor of Pantograph** [ref 8, arXiv:2410.16429] as a machine-to-machine Lean 4 interface exposing apply-tactic-to-goal → new-goal, with DFS/MCTS built in. Two things worth adopting:
1. **The goal-tactic-graph + shortest-path proof recovery**: run search that may find redundant paths, then extract the minimal-length tactic sequence via shortest path. This maps directly onto our **proof-DAG** and gives us proof minimization for free once we track edges. We likely already have the DAG; the shortest-path *minimization* pass is a concrete, small add we may be missing.
2. **Failed-tactic = unchanged-state (no-op self-edge)** as explicit graph semantics — a useful invariant for our MCTS transposition/dedup: an erroring tactic never mutates state, so it should never spawn a new node.

**Evaluate Pantograph as a Lean backend.** For our Lean gate we should benchmark **Pantograph vs. our current Lean driver** (and vs. Kimina Lean Server [9]) for throughput/latency; the paper treats Pantograph as the default and it has a clean Python interface. This is a real, testable integration decision.

**RL + custom reward hook** (GRPO/PPO with `reward_func(completions, **kwargs)` returning per-completion rewards; rewards like syntax-correctness / proof-success) aligns with our expert-iteration flywheel — a familiar pattern, not a gap, but their minimal reward-function interface is a clean API shape to mirror.

**Not relevant / already ahead:** Lean4Code IDE, Agent-TARS, HuggingFace one-click panels — product surface we don't need. Portfolio/multi-system, hammers, falsify-before-prove, lemma cache — we already exceed them.

## Verbatim-worthy details

- **Difficulty bucketing:** easy / medium / hard split at the **33rd and 67th percentiles of proof length (number of proof steps)**. Repos sorted by **count of easy theorems**; progressive training easiest → hardest, **one training run per additional repository**.
- **Training data schema:** proofs decomposed into a **sequence of (proof state, tactic) pairs**; **proof state = model input, tactic = prediction target**.
- **LeanDojo trace extracts:** abstract syntax trees, dependencies, theorems, proofs, premises. Trace inputs (Lean4Code panel): project name, repo GitHub URL, commit hash, GitHub PAT, Lean version, build-dependencies toggle → artifacts dumped to `out/`.
- **Fine-tuning stack:** HuggingFace **SFT trainer**; **LoRA** [ref 11]; **GRPO** [ref 12] and **PPO** [ref 13] RL trainers with user-defined `reward_func`.
- **REPL / search:** **Pantograph** [ref 8] as REPL *and* search; supports **DFS and MCTS**; **shortest-path** algorithm reconstructs minimal-length tactic sequence; failed tactic → goal state unchanged.
- **Retriever:** ReProver premise retriever (curriculum-trained); **"Byte5 retriever model"** downloadable locally for LeanCopilot. *(Encoder, embedding dim, negatives, top-k — not stated in this paper; see LeanDojo v1.)*
- **Model names referenced:** `deepseek-ai/DeepSeek-Prover-V2-7B` (fine-tune target), `deepseek-ai/DeepSeek-Prover-V2-671B` (API-only, via HuggingFace inference; example provider string `DeepSeek-Prover-V2-671B:novita`).
- **Hyperparameters from example code:** SFT — `epochs_per_repo=1, batch_size=2, lr=2e-5`. GRPO — `epochs_per_repo=1, batch_size=8, lr=2e-5`.
- **API surface (verbatim class/method names):**
  - `lean_dojo_v2.agent.hf_agent.HFAgent`
  - `lean_dojo_v2.agent.lean_agent.LeanAgent`
  - `lean_dojo_v2.agent.ExternalAgent(model_name=...)`
  - `lean_dojo_v2.trainer.sft_trainer.SFTTrainer`
  - `lean_dojo_v2.trainer.grpo_trainer.GRPOTrainer`
  - Abstract base classes: `BaseAgent`, `BaseProver`.
  - Lifecycle: `agent.setup_github_repository(url=, commit=)` → `agent.train()` → `agent.prove()`; whole-proof: `agent.prove(whole_proof=True)`.
  - Reward fn signature: `def reward_func(completions, **kwargs): return torch.tensor([...])`.
- **Perf claim:** API-inference path removes CTranslate2/C++ build → **build time >10 min reduced to seconds**; also lower latency, enables larger hosted models.
- **Benchmark splits:** **NONE reported** (no novel-premises split, no pass@k). The novel-premises split our task referenced belongs to the **LeanDojo v1** benchmark, not this paper.
- **IDE:** Lean4Code = VSCodium fork, Lean4 extension pre-bundled, LeanDojo-v2 injected as config folder; panels: Lean4 (modified — downloads/configures Lean4/LeanDojo-v2), LeanDojo (one-click tracing), LeanCopilot (calls models to complete theorems), Agent-TARS (general agentic assistance).
- **Key inherited references to mine next:** LeanDojo v1 [1] arXiv:2306.15626 (retrieval architecture, benchmark, novel-premises split); Pantograph [8] arXiv:2410.16429 (REPL/search API); LeanAgent [3] arXiv:2410.06209 (curriculum); Kimina Lean Server [9] arXiv:2504.21230 (alt REPL).
