# Lean Copilot — Towards LLMs as Copilots for Theorem Proving in Lean

Song, Yang, Anandkumar (Caltech / UCSB), arXiv:2404.12534, NeurIPS MATH-AI workshop. Open source: https://github.com/lean-dojo/LeanCopilot (MIT).

## Core contribution
A framework for running LLM inference **natively inside Lean 4** via the Foreign Function Interface (FFI) to C++, rather than the usual Python-server round-trip. On top of it they build three in-editor human-AI collaboration tools — `suggest_tactics`, `search_proof`, `select_premises` — that display suggestions live in the VS Code infoview. The thesis is that current LLMs fail on truly novel/out-of-domain theorems, so the near-term win is a *copilot* that incrementally automates the tedious parts while the human supplies insight.

## Key techniques / architecture (in-editor tactic-suggestion / inference)
- **Native inference via FFI.** Default model is LeanDojo's **ReProver** (encoder-decoder ByT5, tokenizer-free, works on UTF-8 bytes). Wrapped as a C++ function operating on strings, compiled to a shared library, linked to Lean dynamically. Fast local inference uses **CTranslate2** (C++ Transformer inference); no GPU required — responds in "no more than a few seconds" on a laptop. Alternative: run on a server. Decoding = **beam search**, configurable temperature and `numReturnSequences` (top-k) → multiple candidate tactics per call.
- **Two low-level interfaces** (the general capability): text-to-text generation (`NativeGenerator` / `ExternalGenerator`) and text-to-vector encoding (`NativeEncoder` / `ExternalEncoder`). User-brought models plug in via a HuggingFace URL + tokenizer + `params`; config fields are individually overridable (`{model with params := {numReturnSequences := 4}}`). External variants take a host+port to a Python API server.
- **`suggest_tactics`**: feeds current goal to LLM, gets candidate tactics, and **executes each candidate in Lean to categorize**: (1) errors → discarded; (2) no error but doesn't close goal → shown blue, with the *resulting remaining subgoals* displayed; (3) closes proof → shown green. This checking is the verification-first filter — only error-free tactics surface. "No goals" appears iff ≥1 suggestion closes the proof.
- **`search_proof`**: augments **aesop** (Lean's white-box best-first proof search; search tree of goal nodes, fixed user-configured rule set) by injecting *goal-dependent* LLM-generated tactics into aesop's rule set **at every node expansion**. This makes the rule set adaptive per-goal instead of fixed. Drop-in replacement for aesop (toggle LLM tactics on/off).
- **`select_premises`**: retrieval-augmented. Encode goal → vector; multiply against **precomputed premise embedding matrix** (from ReProver) → per-premise relevance scores → return top-k. Uses CTranslate2 matmul + Libnpy `.npy` reader in C++ via FFI (premise embedding precomputed, so fast). Returned premises are **annotated**: in-scope (module imported) → type signature + docstring; out-of-scope → the module to import + the full source definition.

## Results / benchmarks
Eval on 50 randomly selected theorems from "Mathematics in Lean" (233 exercises; avg 5.52 tactics/proof). Ground-truth tactics entered one-by-one to mimic a human; after each, the tool tries to finish. Metrics: avg #human-entered tactics, % theorems proved fully autonomously, avg % proof steps automated.

| Method | Avg #human tactics (↓) | % proved autonomously (↑) | Avg % steps automated (↑) |
|---|---|---|---|
| aesop (rule-based baseline) | 3.62 | 12% | 35.2% |
| suggest_tactics | 2.72 | 34% | 48.6% |
| search_proof | **1.02** | **64%** | **81.2%** |

`search_proof` = 1.67× suggest_tactics and 2.31× aesop on % steps automated.

## Novel vs SOTA-2026
By 2026 standards the *models* are dated (ReProver/ByT5, pythia-2.8b) and results are on a beginner textbook, not olympiad-scale. The durable novelty is the **engineering pattern**: native in-proof-assistant LLM inference with zero Python setup, and the **"benchmark inside the real ITP environment" argument** — static datasets have limited ground-truth proofs and undercount multi-path provability, so evaluating in-environment is more faithful. Also the human-AI iterative collaboration protocol as an explicit evaluation paradigm.

## Adopt-relevance to Theoremata
- **We already do**: verification-first execution of candidate tactics (our 3+1 gate), portfolio/hammer proving, premise retrieval, per-system generators.
- **Real gaps / adopt**:
  1. **The candidate-categorization UX (green/blue/discard + show resulting subgoals).** Directly maps to how our proof-DAG should surface partial-progress nodes: a tactic that doesn't close but advances is a *valid DAG edge to a new node*, not a failure. Adopt the tri-state labeling (closes / advances-with-new-subgoals / errors) as node/edge status in the proof-DAG and as agent feedback.
  2. **aesop-rule-set augmentation pattern** = inject model-generated, goal-conditioned rules into an otherwise-fixed best-first search. This is a concrete recipe for our MCTS graph-search: use the LLM to *expand the action set per node* rather than sampling from a fixed tactic vocabulary.
  3. **The copilot flywheel argument** (copilot → more/better formalized math → more data → better models) is exactly our expert-iteration flywheel rationale; cite it.
  4. Retrieval mechanics (precomputed premise matrix, matvec scoring, in/out-of-scope annotation with import + source) is a clean spec for our premise-selection tool's output schema.
- Not a gap: native-in-Lean FFI is an implementation detail we don't need (our core is Rust+Python orchestrating the provers).

## Verbatim-worthy details
- Tactic categorization rule: "checks each tactic candidate to see if they (1) lead to errors; (2) result in no errors but cannot finish the proof; (3) successfully finish the proof." Green = category 3, Blue = category 2 (also display remaining subgoals), discard category 1. "The resulting tactic state contains no goals iff at least one suggested tactic can finish the proof."
- Premise selection: "encode the goal into a vector, then perform a matrix-vector multiplication between the premise embedding and the goal vector … return the k premises that have the highest scores."
- Decoding: beam search, hyperparameters temperature + numReturnSequences (top-k).
- Default model: ReProver, ByT5 encoder-decoder, tokenizer-free (UTF-8 bytes). Local inference: CTranslate2; npy reading: Libnpy; both linked via FFI.
- Collaboration eval protocol: "whenever a goal exists, humans first call the copilot to see if it can solve it directly. If not, humans attempt to proceed one step and simplify the goal. The copilots are then tried on the remaining goal. … repeated until the copilots successfully solve the remaining goal … or humans solve all steps."
- Dataset: 50 of 233 "Mathematics in Lean" exercises, avg 5.52 tactics/proof.
