# LEAN-GitHub — Compiling GitHub LEAN Repositories for a Versatile LEAN Prover

Wu, Wang, Lin, Chen (Shanghai AI Lab / CUHK), arXiv:2407.17227. Model: InternLM2-StepProver (7B). Data: https://huggingface.co/datasets/InternLM/Lean-GitHub.

## Core contribution
A **scalable corpus-compilation pipeline** that extracts formal training data (proof states + tactics) from *almost all* Lean 4 repos on GitHub — not just Mathlib — yielding **28,597 theorems / 218,866 tactics / 0.131B tokens** across diverse fields. Fine-tuning InternLM-math-plus-7B on it (plus Mathlib + synthetic) gives InternLM2-StepProver, which sets SOTA on Lean 4 miniF2F (54.5% cumulative@64), ProofNet (18.1% Pass@1), and Putnam (5/640), beating whole-proof DeepSeek-Prover (52%). Two engineering contributions matter beyond the data: (a) massive-parallel extraction of *uncompilable / isolated* files, and (b) a **tree-search state de-duplication** method.

## Key techniques / architecture (corpus-compilation pipeline)
Pipeline (Fig. 2): **237 Lean 4 repos → Pre-Filter → 147 repos → Build Import Graph → 10K+ files → Parallel Compile → 8369 compiled → Extract AST → 6352 AST trees → Extract Tactics → 28K theorems / 219K tactics / 0.131B tokens.**
- **Repo selection**: keyword-search ("theorem"/"lemma") estimated ~48,091 theorems across 237 repos. Obstacles: won't compile, missing deps, deprecated Lean versions, non-tactic proofs. Discarded 90 deprecated repos; only 61/147 compiled unmodified. Automated scripts heuristically find the closest official Lean release for repos on nonstandard versions.
- **Bypass Lake, call `leanc` directly.** Lake fails the whole project if any build target fails (discarding everything) and has a concurrency bottleneck; also many math files are *isolated files in an empty project* Lake can't build. Solution: **extend Lake's file-dependency import graph, expose it, augment with isolated-file info, rebuild a global import graph**, then compile with a custom script calling `leanc` with higher parallelism.
- **Extraction** built on LeanDojo but restructured: removes LeanDojo's requirement that the *whole project* compile first (implements isolated-file extraction), removes network dependency, and **decouples data extraction from live Lean interaction** to cut redundancy → higher parallelism. Of 8639 files, 6352 extracted; 2133 files / 28K theorems have valid tactic info.
- **Training format** (GPT-f proofstep objective): `DECL <declaration>\nGOAL <tactic-state>\nPROOFSTEP <tactic>\n`. Fine-tune InternLM-math-plus-7B (decoder-only, continued-pretrained on 200B informal+formal tokens). Batch 512, LR 1e-5, 2 epochs, 3% warm-up + cosine-to-zero, ~6h on 32×A100.
- **Evaluation = best-first tree search**: expand state Sᵢ with S=32 tactic candidates, max K=100 expansions/iteration; validate each step in Lean.
- **State de-duplication (novel).** Lean's dependent type theory + definitional proof equivalence means tactics like `intro`/`have` create states that differ only by *hypothesis names* → >50% of intermediate states in long searches are duplicates. Fix: use Lean's runtime meta-programming to **rename hypotheses by their internal (kernel) storage order**, giving a canonical form so states with identical hypotheses+goals are identified and merged.

## Results / benchmarks
- **miniF2F-test**: 48.8% Pass@1, 54.5% cumulative@64 (valid 59.8%/63.9%) — beats DeepSeek-Prover (52.0% test) and all prior tree-search (HyperTree 41.0% test). 
- **ProofNet**: 18.1% Pass@1 (prev SOTA ReProver 13.8%); found 24 new proofs.
- **Putnam**: 5/640 single-pass (vs DSP-Isabelle 4/640 @pass@10); solved **Putnam 1988 B2** — not previously solved by any ITP. Also first Lean 4 proof of **IMO 1983 P6**.
- **Data-source ablation (Pass@1 miniF2F test)**: Mathlib 37.3% → +LEAN-GitHub 41.0% → +synthetic 46.7% → all three 48.8%. ProofNet: 15.1 → 16.2 → 17.0 → 18.1%. GitHub data clearly additive.
- Dataset comparison: Lean-Workbook (57K, synthetic, no intermediate states), DeepSeek-Prover (870K, synthetic, no states), LeanDojo-Mathlib (60K, human, has states), LEAN-GitHub (28K, human, has states, **diverse level** vs others' HS/undergrad).

## Novel vs SOTA-2026
By 2026 the model/scores are eclipsed (DeepSeek-Prover-V2, Kimina, Aristotle, etc.). Enduring contributions: (1) the **beyond-Mathlib GitHub-scale extraction pipeline** (Lake-bypass + isolated-file + global import graph) — still the reference recipe for turning raw formal repos into (state, tactic) training data; (2) **kernel-order hypothesis-renaming state de-dup** for tree search, a cheap high-impact efficiency trick; (3) empirical evidence that *human-written diversity* beats synthetic volume for out-of-distribution generalization.

## Adopt-relevance to Theoremata (corpus / flywheel data pipeline)
- **Real gaps / adopt**:
  1. **The de-dup canonicalization is directly relevant to our proof-DAG.** Our DAG dedupes nodes by goal identity; naive string comparison will treat α-renamed states as distinct and bloat the graph / MCTS. Adopt kernel-order hypothesis renaming (or an equivalent canonical form via our FormalSystem abstraction) as the node-hashing key. High value, concrete.
  2. **Extraction pipeline for the flywheel.** Our expert-iteration flywheel needs a data ingestion stage; this is a battle-tested spec: enumerate repos → pre-filter by keyword → build/repair a global import graph → bypass the package manager and call the compiler directly for parallelism/fault-isolation → extract (DECL, GOAL, PROOFSTEP) triples via an AST pass decoupled from live interaction. Flag: we should extract across Lean **and** Rocq/Isabelle behind FormalSystem, not just Lean.
  3. **Training/eval hyperparameters and the `DECL/GOAL/PROOFSTEP` schema** are a ready-made target format for our corpus and for finetuning any local step-prover.
  4. **Ablation lesson for our data strategy**: mix human-written diverse + Mathlib + synthetic; diversity (not just volume) drives OOD gains — informs how we weight synthetic autoformalized data vs mined human proofs.
- **We already do / partially**: best-first / MCTS search, per-system provers, synthetic autoformalization. The extraction pipeline itself is likely a genuine gap in our data tooling.

## Verbatim-worthy details
- Dataset: 28,597 theorems, 218,866 tactics, 0.131B (0.138B) tokens, from 2133 files / 147 repos (of 237 found; ~48,091 keyword-estimated theorems).
- Pipeline funnel: 237 repos → 147 (pre-filter) → 10K+ files → 8369 compiled → 6352 AST trees → 28K theorems w/ valid tactics. 61/147 compiled unmodified; 90 deprecated repos discarded.
- Training prompt: `DECL <DECLARATION>\nGOAL <GOAL>\nPROOFSTEP <PROOFSTEP>\n` (GPT-f objective). Example: `DECL MyNat.mul_pow / GOAL a b n : ℕ ⊢ (a*b)^n = a^n*b^n / PROOFSTEP induction n with t Ht`.
- Hyperparams: base InternLM-math-plus-7B (200B-token continued-pretrain); global batch 512; LR 1e-5; 2 epochs; 3% warm-up + cosine→0; ~6h on 32×A100.
- Search config: S=32 candidates/expansion, K=100 max expansions/iteration; multi-pass eval at temps 0.7 and 1.0, 32 independent inferences (not beam) for cumulative@64.
- De-dup: ">50% of intermediate states are duplicates" in long searches; fix = rename hypotheses/metavariables "based on their internal storage order" in Lean's kernel for a unified representation.
- Compilation: call `leanc` directly instead of `Lake`; extend + expose Lake's import graph, augment with isolated files, rebuild global import graph.
