# ImProver — Agent-Based Automated Proof Optimization

Source: `math-papers/ImProver - Agent-Based Automated Proof Optimization.pdf` (ICLR 2025; Ahuja, Avigad, Tetali, Welleck, CMU). 23 pages, fully read.
Repo: https://github.com/riyazahuja/ImProver · Target: Lean 4 / Mathlib.

## Core contribution
Defines a new task — **automated proof optimization**: rewrite a *correct* Lean proof so it stays correct and optimizes an arbitrary user metric (length, declarativity, etc.). ImProver is an LLM agent (over a black-box GPT-4o) that does this via a novel **Chain-of-States** prompting technique plus error-correction, retrieval, structured output, and best-of-n/refinement sampling. Proof optimization strictly generalizes neural theorem proving (optimizing the "completion" metric = proving from an empty proof).

## Key techniques / architecture

### The optimization objective
Given theorem `x`, context `c`, initial proof `y0`, produce correct `y` that minimizes/maximizes a metric `µ(x,c,y0,y) → ℝ`. Four metrics:
- **Length**: number of tactic invocations (fewer = better).
- **Declarativity**: ratio of explicitly-typed `have` tactics to total tactics (structure/readability proxy).
- **Mixed**: −1 per tactic, +5 per declarative `have` (each `have` "pays for" 4 tactics); maximize net.
- **Completion**: number of errors (0 = proved) — this makes NTP a special case.
- Warns about **degenerate solutions** (e.g., spamming useless `have`s to game declarativity); mitigate with human examples + reward *models* rather than raw reward functions.

### The agent loop (features layered on GPT-4o)
1. **Chain-of-States (CoS)** — the key novelty. Uses Lean metaprogramming (`InfoTree`, `Syntax`/`Expr` from elaboration) to extract the **intermediate proof state (hypotheses + goals) before each tactic** and inject it as comments into the proof text sent to the LLM. Analogous to chain-of-thought but with symbolic, ground-truth intermediate states. Cached for reuse.
2. **Output formatting** — string / **string list** (tactic sequence) / string tree; structured outputs improved correctness and structure.
3. **Sampling** — **best-of-n** (temperature 1) with a comparison function that first prioritizes correctness then metric delta; **refinement** (iterative self-debug carrying last `prev_num` iterations' input/output/metric/correctness/error messages); and **compound** methods that nest them, e.g. `refinement((best_of_n,m),n)` — always `m·n` total iterations.
4. **Retrieval (RAG)** — **MMR** (Maximal Marginal Relevance) retrieval of top-k relevant proof-optimization examples, plus two vector DBs: TPiL handbook (syntax/tactic docs, retrieved against theorem + current error messages) and Mathlib (theorems/lemmas, retrieved against the theorem).

### Best-of-n selection function (verbatim)
`S(y,y')` = pick max metric if both error-free; pick the error-free one if exactly one has errors; pick min-error if both have errors. Extended over n by induction.

## Results / benchmarks
Performance metrics defined: **improvement** (expected metric gain incl. zeros for failures), **nonempty improvement** (given a correct nontrivial output), **accuracy** (% with any correct output — ImProver always 100% since it can return the input), **improved accuracy** (% with correct + strictly-better output).
- Aggregate (Table 1): Length — ImProver 20.96 improvement / 55.29 nonempty / 35.44% improved-acc vs GPT-4o 3.7 / 15.15 / 8.31% (**+566%**). Declarativity **+423%**. Mixed **+778%**.
- Per-dataset (MIL undergrad → Compfiles competition → Mathlib research): improvement decreases with difficulty; bottleneck is *generating any correct proof*, not the optimization itself.
- **Ablation-derived optimal config**: GPT-4o, **string list** output, **CoS on**, **10 retrieved examples**, **5-step refinement each iteration best-of-3**, RAG on → 34.88 improvement / 57.56 nonempty / 100% acc / 54.55% improved-acc. CoS nearly doubled declarativity improvement.
- NTP (completion metric, Pass@k): ImProver 39.13% vs GPT-4o 21.73% on MIL; 16.39% vs 9.02% on MiniF2F-test — but specialized provers (Lean Expert Iteration 34.5% MiniF2F) still win, since ImProver is just GPT-4o + agent scaffolding.

## Novel vs SOTA-2026
- First to frame **proof optimization** as a distinct, metric-parameterized task generalizing NTP.
- **Chain-of-States** — symbolic intermediate proof states as prompt context — is a genuinely useful, transferable trick for any Lean-facing LLM agent.
- Positions optimization as **training-data augmentation/curation**: it can turn unstructured proofs into declarative, sketch-friendly ones (feeds Draft-Sketch-Prove-style pipelines).
- Limitation: expensive, proprietary-LLM dependent; no SFT/RL yet (future work).

## Adopt-relevance to Theoremata
- **Chain-of-States is a direct, high-value adopt for our Lean/Rocq/Isabelle verification loop.** We already compile and re-check proofs; CoS says: extract per-tactic goal states from the successful/partial compilation and feed them back to the generator as comments. This tightens our sketch→autoformalize-holes→splice loop (each hole gets its exact local goal state) and our critique step (the model reasons over ground-truth states, not guessed ones). GAP: our current retry likely feeds only error messages, not intermediate states.
- **Proof-pool / ProofGrader**: ImProver's metrics (length, declarativity, mixed, completion) are ready-made *secondary* objectives for ranking equivalent proofs in the pool once correctness is established by our gate. Adopt its selection function `S` (correctness first, then metric delta) as the pool's tie-breaker. Its degenerate-solution warning is a caution for ProofGrader reward-hacking.
- **Sketch pipeline / data curation**: run ImProver-style declarativity optimization to convert successful proofs into declarative `have`-structured proofs → better training data for the flywheel and better splice targets (structured proofs decompose into subgoals cleanly).
- **Portfolio/sampling**: compound `refinement((best_of_n,m),n)` with a correctness-then-metric selector is a concrete scheduler for our portfolio proving; the ablation (5-step refinement × best-of-3, 10 examples, RAG) is a tested starting config.
- **Retrieval**: MMR against (theorem + current error messages) for syntax docs and against the theorem for lemmas is a clean retrieval recipe for our per-system generators.
- **What we already do vs gap**: we have portfolio proving, retrieval, and a formal gate. REAL GAP: (1) CoS-style symbolic-state feedback into the generator; (2) treating optimization/declarativity as a data-curation step for the flywheel; (3) a correctness-first-then-metric ranking inside the proof-pool.

## Verbatim-worthy details
- Objective: minimize/maximize `µ(x,c,y0,y) → ℝ` subject to correctness.
- Declarativity metric = `#(explicitly-typed have tactics) / #(total tactics)`.
- Mixed metric: value −1 per tactic, +5 per declarative tactic; maximize sum.
- Best-of-n selector `S(y,y')`: if `E(y)=E(y')=0` → argmax µ; if exactly one error-free → that one; if both have errors → argmin errors (`E` = #errors).
- CoS extraction: from Lean elaboration/evaluation, convert CST (`Syntax`) + AST (`Expr`) into `Lean.Elab.InfoTree` proof trees; annotate each proof state as a comment before each tactic; cache.
- Compound sampling: `best_of_n((refinement,m),n)` and `refinement((best_of_n,m),n)`, always `m·n` LLM calls.
- Optimal ablated config: GPT-4o, string-list output, CoS on, 10 examples, RAG on, refinement(best-of-3, 5 steps).
- Metric prompts (Appendix A.2) are terse system+user pairs, e.g. Length: "You are an AI assistant who shortens Lean 4 proofs while ensuring their correctness … reduce the number of lines … while ensuring it properly compiles."
