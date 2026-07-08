# Reliable Fine-Grained Evaluation of Natural Language Math Proofs (PROOFBENCH / PROOFGRADER)

Ma, Cojocaru, Kolhe, Louie, Sharif, Zhang, Zhuang, Zaharia, Min (UC Berkeley + Google DeepMind + PKU + AI2). ICLR 2026, arXiv:2510.13888v2. Dataset: huggingface.co/datasets/wenjiema02/ProofBench.

**THE key paper for our ProofGrader + proof_calibration. Mine deeply.**

## Core contribution
Identifies the lack of a reliable *fine-grained* evaluator for natural-language (informal) math proofs as the bottleneck for proof generation. Contributes (1) **PROOFBENCH**, the first expert-annotated dataset of fine-grained (0–7) proof ratings — 145 problems × 3 models = 435 graded solutions; (2) a **systematic search over the evaluator design space** (backbone / context / instruction / workflow) yielding **PROOFGRADER** (O3 + reference-solution + marking-scheme + ensembling), MAE 0.926 vs experts; (3) proof that fine-grained scoring beats binary in downstream **best-of-n** selection, closing 78% of the gap to a human oracle. Notably: they find the optimal evaluator *without training* — pure prompt/config search.

## Key techniques / architecture (the grading rubric + reliability method)
**Task formalization**: evaluator `E` maps (problem p, solution s, optional context C) → integer score ŷ ∈ {0..7}. Find config `c* = argmin (1/|D|) Σ L(E_c(p,s,C), y)` over a discrete config space; y = expert ground truth. 0–7 scale mirrors premier-competition grading (Putnam's 0–10 normalized to 0–7).

**PROOFBENCH construction (2-stage annotation)**:
- Problems: 145 from APMO/EGMO/IMO/Putnam/USA-TST/USAMO, 2022–2025, parsed from official PDFs + human solutions.
- Generators: OpenAI o3, Gemini-2.5-Pro, DeepSeek-R1-0528 (3 proofs/problem, standardized "complete self-contained proof" prompt).
- **Stage 1 — Marking-scheme generation**: an LLM `M_MS` (chosen = **Gemini-2.5-Pro, zero-shot**) generates a problem-specific rubric from problem + reference solution. Rubric = 3 sections (from Evan Chen's USEMO captain-guidance framework): **(1) Checkpoints (max 7, additive/[max k], ≥4 pts to the main idea, ≤3 to routine work, parallel chains for alternate approaches — score one chain, take the max, no cross-chain addition), (2) Zero-credit items (conjectures without proof, restatements, dead ends), (3) Deductions (flat −1/−2/"cap at x/7", apply only the single largest, never below 0, cosmetic slips don't count)**. ~85% of generated schemes judged high-quality; experts rated schemes 0–3, 35/36 rated 2–3.
- **Stage 2 — Proof grading**: 5 experts (Putnam/national-olympiad level) score each proof 0–7, treating the scheme as a **detailed reference, NOT a rigid checklist** (credit valid alternative approaches). Calibration phase first. 41% double-annotated, within-1-point agreement **87.5%**; disagreements resolved by discussion. Scores cluster into 4 bands: **incorrect (0) / partial (1–3) / nearly complete (4–6) / fully correct (7)**; annotators must agree on band + exact score for 0 and 7.

**Evaluator design axes (single-pass)**: backbone (O3, GPT-5, Gemini-2.5-Pro, o4-mini, R1, GPT-4o); context (REF+MS / MS / REF / NONE); instruction (NORM flexible / STRICT literal / BASIC minimal). Advanced: **ensembling** (5 independent runs → mean/median/majority), and **staged workflows** (Binary+Errors→Fine-Grained; Evaluate→Reflect→Verdict).

**PROOFGRADER = O3 backbone + REF+MS context + NORM instruction + ensemble (median of 5 runs).**

## Results / benchmarks
- **Effect sizes ordered: backbone ≫ context ≫ instruction.** Recommended: strongest backbone, marking scheme by default, match instruction rigidity to model capacity.
- Best single-pass O3 REF+MS: MAE 0.964, RMSE 1.273, WTA≤1 76.5%, Kendall-τ 0.502, bias −0.008. Ensemble → **MAE 0.926**, RMSE 1.169 (mean), Kendall-τ 0.578.
- Context: MS contributes most of the gain over REF alone; REF+MS adds a little more (mainly for strongest backbone). NONE is far worse.
- Instruction: strongest model (O3) best with flexible NORM (near-zero bias); mid-tier (Gemini, o4-mini) best with prescriptive STRICT (reduces over-crediting/variance).
- Staged workflows help weak backbones (o4-mini RMSE 1.816→1.650) but HURT strong ones (O3 1.273→1.375); Reflect→Verdict adds ~nothing.
- **Within-generator bias**: each evaluator has highest MAE on its own model family; Gemini/R1 over-score their own outputs (O3 doesn't).
- **Best-of-n (n=16, downstream utility)**: PROOFGRADER avg **4.14/7**, closing **78%** of the gap between naive binary evaluator (2.48) and human oracle (4.62). Beats Tournament/Knockout pairwise selection at far lower cost (O(n) scoring vs O(n²) comparisons). Binary evaluators much worse — collapsing correct/incorrect loses ranking ability.
- Model capability finding: SOTA reasoning models score ≥6 on <30% of problems (mean 2.38); best on Putnam (3.09), worst on TST (1.26). Consistent score drop on post-knowledge-cutoff problems (contamination signal, esp. Gemini +1.01).
- Open-source evaluators (Qwen3-235B, Llama-3.1-70B) lag badly (MAE 1.35–3.2); strong evaluators currently require closed models.

## Novel vs SOTA-2026
First systematic evaluator-design study + first fine-grained (non-binary) expert proof-rating dataset. Prior work is binary (Open Proof Corpus / Dekoninck), specialized (IneqMath), or manual (Petrov "Proof or Bluff"). The "LLM-generated marking scheme, human-graded, then LLM-graded-against-scheme" pipeline and the backbone≫context≫instruction ordering are the durable, adoptable findings.

## Adopt-relevance to Theoremata — vs our ProofGrader + proof_calibration
This paper is essentially a reference implementation of what ProofGrader should be. Concrete gap analysis:
1. **Marking-scheme-conditioned grading is the single highest-value adoption.** If our ProofGrader currently grades a NL proof from problem+proof alone (NONE-like), the paper shows that is the *worst* config and systematically **over-scores low-quality proofs by ~1.7 pts** and mis-rates "sophisticated but wrong" proofs as correct. Adopt: generate a per-problem marking scheme (checkpoints/zero-credit/deductions) from a reference solution first, feed it as advisory context. This is likely a real gap.
2. **0–7 fine-grained scale + 4-band structure** (0 / 1–3 / 4–6 / 7) instead of binary pass/fail — required for our proof_calibration to be a useful reward/ranking signal (binary destroys best-of-n). If proof_calibration currently emits pass/fail or a coarse score, move to 0–7.
3. **Ensembling (median of 5 runs)** for variance reduction — cheap, robust, drop-in. Match instruction rigidity to backbone strength (NORM for our strongest model).
4. **Calibration metrics to add to proof_calibration**: MAE, RMSE, Bias (signed), WTA≤1 (within-1-point agreement %), Kendall-τb (within-problem ranking). These are exactly the metrics our calibration suite should report against a held-out expert-graded set. Within-1-point agreement 87.5% is a concrete human-ceiling target.
5. **Within-generator bias warning**: our grader should not be the same model family as our generator, or we must correct for self-over-scoring — directly relevant to our meta-verification/critique loop (don't let the prover grade itself).
6. **Failure-mode taxonomy → critique checklist**: over-crediting (appearance-of-completeness, fatal-gap-as-minor-omission, over-trust in sophisticated-but-wrong frameworks) and under-crediting (penalizing abandoned early attempts, double-penalizing one flaw, over-strict micro-justification). Geometry especially unstable. Bake these as explicit checks/prompts into ProofGrader and meta-verification.
7. **Best-of-n as our selection/reward harness**: use fine-grained score to pick among n generated proofs; O(n) scoring beats O(n²) tournaments — informs how our portfolio/expert-iteration flywheel ranks candidates.
- **We already do**: formal verification (the paper explicitly says formal/Lean gives "absolute certainty" but is detached from NL and autoformalization is brittle — so our 3+1 gate covers the *formal* side; ProofGrader covers the *NL* side they focus on). The two are complementary; this paper is the blueprint for our NL grading arm.

## Verbatim-worthy details
- **Config-search objective**: `c* = argmin_{c∈C} (1/|D_test|) Σ_{(p,s,y)} L(E_c(p,s,C_context), y)`; ŷ ∈ {0,...,7}.
- **Metrics** (per-problem, macro-averaged; n responses each):
  - MAE_p = (1/n)Σ|ŷ_pi − y_pi|; RMSE_p = sqrt((1/n)Σ(ŷ−y)²); Bias_p = (1/n)Σ(ŷ−y) (signed, + = over-scoring); WTA_p(≤1) = (1/n)Σ 1{|ŷ−y|≤1}.
  - Kendall-τb (ties-adjusted): τb(p) = (C−D)/sqrt((C+D+T_exp)(C+D+T_eval)), over all pairs i<j within a problem; C/D concordant/discordant, T ties.
- **Headline numbers**: PROOFGRADER MAE 0.926; best-of-16 = 4.14 vs binary 2.48 vs oracle 4.62 (78% gap closed); double-annotation within-1-pt agreement 87.5%; ~85% auto marking schemes high-quality.
- **Marking scheme = 3 sections**: (1) Checkpoints max 7, ≥4 pts main idea / ≤3 routine, [additive] or [max k] tags, parallel chains scored max-not-sum; (2) Zero-credit items; (3) Deductions (flat −1/−2/"cap at x/7", single largest only, floor 0, cosmetic slips excluded). Source framework: Evan Chen USEMO captain-guidance.
- **Example scheme (USAMO 2025 P2)**: [1pt] reduction/setup; [2pts] pigeonhole; [2pts] deduce consecutive zeros; [2pts] lemma (Rolle/Descartes); caps: 6/7 no-reduction, 5/7 flawed lemma, 3/7 stops after PHP; −1 minor gaps; zero credit for unjustified WLOG / merely stating theorems.
- **Evaluator prompt (REF+MS, NORM)** — core principles in precedence order: (1) mathematical validity, (2) problem constraints, (3) advisory mapping to marking scheme (allow different order/technique), (4) reference solution as anchor for sufficiency not exclusivity. Alternative-approach policy: map valid different methods to equivalent checkpoints; don't penalize re-ordering/different lemmas/shortcuts unless forbidden; apply deductions only when the issue actually occurs; award the larger of mutually-exclusive items; wrong unique final answer → only partial credit. Rigor: credit intermediate claims only if adequately justified; under-justified → conservative partial credit. Output: well-formed XML `<score>`(int 0–7) `<assessment>`(step-by-step rationale + scoring breakdown in prose) `<errors>`(list, empty if 7).
- **Basic scoring guideline bands**: 0 completely incorrect; 1–2 very poor/major flaws; 3–4 partial progress but invalid overall; 5–6 largely correct, minor issues only; 7 fully correct.
- **Generator prompt**: "creating a proof, not an outline"; only well-known named theorems (Wikipedia-level); no results beyond low-level bachelor courses (else zero); no skipped computation; self-contained; state uncertainty rather than assert; Markdown+LaTeX, no unicode, no code fences.
- **Marking-scheme model selection**: Gemini-2.5-Pro zero-shot beat O3 and few-shot; original *human* schemes beat any regenerated scheme (evaluator accuracy depends on scheme alignment).
- Context effect: no-context evaluator over-scores in >60% of cases; correlation proof-quality vs over-scoring gap r=0.699 (p<0.001); low-quality proofs over-scored by 1.7 pts; on problems the evaluator itself can't solve (≤2 as prover), over-scores by 1.4 pts.
