# Machine Learning and Automated Theorem Proving (James P. Bridge, 2010)

Source: `math-papers/Machine learning and Automated Theorem Proving.pdf` (Cambridge Computer Laboratory Technical Report UCAM-CL-TR-792; PhD dissertation, Oct 2010). 180 pages; read in full via chapters 1–7 (methodology, features, heuristic-selection experiment, feature selection, conclusions) — the rest is background (logic/SVM tutorial) and bibliography.

## Core contribution
A pre-deep-learning PhD thesis showing that **machine learning (SVMs) can select a good proof-search heuristic per problem** in a first-order-logic theorem prover (a modified **E** prover), beating any single fixed heuristic and far beating random selection. Introduces **dynamic features** (measured a short way into proof search) alongside **static features** (of the conjecture+axioms), and shows via systematic **feature selection** that only ~2–3 features are needed for optimal heuristic choice. The idea is essentially **learned portfolio/algorithm selection** for ATP.

## Key techniques / architecture
- **Setup**: convert a conjecture (negated) + axioms into a numeric **feature vector**; train per-heuristic binary SVM classifiers (SVMLight); at inference, run all classifiers and pick the heuristic whose classifier gives the **most positive (or least negative) margin**. No proof search needed to choose. 5 heuristics in the working set (the 5 most-selected by E's auto-mode over TPTP).
- **Feature families** (53 total investigated; 14 static + 39 dynamic): clause **size** (length/#literals, term **depth** of nesting, prover **weight**); clause **type** (e.g. proportion of Horn clauses); **connections** between clauses (shared term-structure scores). Static = measured on initial conjecture+axioms; **dynamic** = measured on the proof state after a fixed number (100) of given-clause selections (comparing processed vs unprocessed vs generated clause sets and their change from the start).
- **Kernel**: compared linear / polynomial / sigmoid-tanh / **radial basis (RBF)**; RBF won (single param γ, nearest-neighbour-like). Used throughout.
- **Two-class-per-heuristic labeling**: run the prover with each heuristic on each problem, record CPU time; "heuristic H is best" ⇒ positive class. Time-consuming to label but gives exact ground truth.
- **Feature selection**: wrapper method (ML as a subroutine, run over feature subsets); exhaustive small-subset search once it was clear few features suffice. Key subtlety: score features by **overall heuristic-selection performance**, NOT per-classifier accuracy — because it is the **relative margins across classifiers** that pick the heuristic. Optimizing individual classifiers can be counter-productive.
- **H0 pre-filter**: an optional classifier that rejects conjectures predicted too hard to prove, saving total time at the cost of some proved theorems.

## Results / benchmarks (TPTP library, 5 heuristics)
Number of theorems proved / total CPU seconds:
- Single heuristics: H1 **1,514** (162,029s), H2 1,352, H3 1,424, H4 1,421, H5 1,339.
- Random selection: **827** proved with H0 filtering (98,145s mean); 1,427 without (169,337s).
- **Learned heuristic choice**: **1,602** proved with H0 filtering (149,323s, γ=48.05); **1,609** without H0 filtering (150,700s, γ=10).
- So learned selection beats the best single heuristic (1,514 → ~1,605) while using less total time, and crushes random (827/1,427). Improvement is modest in absolute count but robust and clearly "learned intelligence."
- **Feature economy**: with H0 filtering, just **two features** gave optimal results; the best pair were a **static and a dynamic measure of the same feature**.
- RBF kernel best (from the initial SET-domain proof-of-concept experiment).

## Novel vs SOTA-2026
- **Historically foundational, technically dated.** SVMs + hand-crafted features are superseded by GNNs/transformers over clause graphs and by modern learned clause/premise selection (and now LLM provers). Do NOT adopt the model.
- Still-valid, durable insights: (1) **no universally best heuristic** → per-problem selection wins; (2) **dynamic (in-search) features add signal** over static ones; (3) **select for the end-to-end objective (relative margins / which heuristic gets picked), not per-component accuracy**; (4) **a few well-chosen features beat the full set** (over-fitting/curse-of-dimensionality); (5) an **H0 difficulty pre-filter** trades coverage for total time.

## Adopt-relevance to Theoremata
This is a "principles, not code" paper. Relevance is to **portfolio proving**, **MCTS/graph-search scheduling**, and **falsify/triage** — not to the LLM reward loop.
- **Portfolio proving** — DIRECT conceptual adopt. Our portfolio (Lean/Rocq/Isabelle + hammers + tactics) is exactly algorithm selection. Bridge's result says: learn a cheap per-goal selector that predicts which backend/tactic-family/hammer will close a goal, and **select by relative predicted success across backends**, not each in isolation. Even a light model beats "always run the single best" and vastly beats round-robin.
- **Dynamic features → MCTS graph-search value/priority.** His dynamic features (proof-state snapshot after k steps: processed vs unprocessed clause growth) are a template for **node features in our proof-DAG / MCTS**: features of the *partial* proof state predict which subgoal/branch is worth compute. This is more actionable for us than the static-only view.
- **H0 filter → falsify-before-prove + triage.** The "predict too-hard, skip" pre-filter maps onto our falsification gate and a compute-budget triage: a cheap classifier that routes hopeless goals away from expensive hammer/MCTS runs, raising total throughput per compute budget.
- **Feature economy → keep selectors cheap.** For any learned scheduler/ProofGrader-style ranker, prefer a tiny feature/signal set; validate on the *end-to-end* proving metric, not proxy accuracy.
- **What we already do vs gap**: we already have portfolio proving and a falsify gate. REAL GAP: we likely select backends by fixed policy/round-robin rather than a **learned, relative-margin selector conditioned on goal (and partial-proof-state) features**; and our MCTS node scoring may not exploit *dynamic* in-search features. Adopt: (a) a learned backend/hammer selector trained on our own proof logs (which system closed which goal, and time), scored on total-solved-per-budget; (b) partial-state features as MCTS priors; (c) a difficulty pre-filter for triage.

## Verbatim-worthy details
- Selection rule: per-heuristic SVM classifiers; choose heuristic with **most positive / least negative margin**; correctness of overall choice depends on **relative** margin magnitudes across classifiers, not any single classifier's accuracy.
- Dynamic features measured after a fixed **100 given-clause selections** (experimented up to 500; 100 = best compromise).
- Feature counts: 53 total; extended set = 14 static + 39 dynamic; optimal with H0 filtering = **2 features** (one static + one dynamic of the same underlying quantity).
- Kernels compared: linear `K=x·x'`; polynomial `K=(s·x·x'+c)^d`; sigmoid `K=tanh(s·x·x'+c)`; **RBF `K=exp(−γ‖x−x'‖²)`** (chosen; best).
- Key results table: best single heuristic 1,514 proved; learned choice 1,602 (H0, γ=48.05) / 1,609 (no H0, γ=10); random 827 (H0) / 1,427 (no H0).
- Prover: modified **E** (Schulz); tool unmodified in its search engine, only instrumented to emit features/timings; SVM via **SVMLight** (Joachims). Data: **TPTP** library.
