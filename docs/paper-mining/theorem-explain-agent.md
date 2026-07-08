# TheoremExplainAgent (TEA) — Video-based Multimodal Explanations for LLM Theorem Understanding

Source: `math-papers/TheoremExplainAgent - Towards Video-based Multimodal Explanations for LLM Theorem Understanding.pdf` (arXiv:2502.19400v2, May 2025; Ku, Chong, Chen et al., Waterloo/Votee/Vector). 22 pages, fully read.
Project: https://tiger-ai-lab.github.io/TheoremExplainAgent/

## Core contribution
Introduces the task of **AI-generated multimodal (video) theorem explanations** and TEA, a two-agent (planner + coder) pipeline that generates long-form (>5 min, up to 10 min) Manim-animated + narrated explanation videos. Ships **TheoremExplainBench (TEB)**: 240 theorems across CS/Chem/Math/Physics (68 sub-fields, 3 difficulty tiers) with **5 automated evaluation metrics**. Key finding: **video explanations expose reasoning flaws that text-based evaluation misses.**

## Key techniques / architecture

### Two-agent pipeline
1. **Planner agent** — from a theorem + short description, builds a hierarchical plan: Scene Outline → Vision Storyboard Plan → Animation & Narration Plan → Technical Implementation Plan, decomposed into scenes (each scene = one video segment with visual elements, animations, transitions).
2. **Coding agent** — translates each scene into executable **Manim** Python scripts (objects, timings, transitions); voiceover via TTS; scripts rendered to video.
3. **Error-correction loop** — on render error, the coder reads the error, emits a revised version (a `<THINKING>` block diagnoses root cause + fix). **Max N=5 attempts**; at N=0 success is near-zero, N=5 reaches ~90%.
4. **Agentic RAG (router)** over Manim docs: first classifies whether the theorem suits specific Manim plugins, then generates stage-specific queries — (1) storyboard: visual examples/concepts; (2) technical impl: code snippets/patterns; (3) error correction: diagnose + fix. Queries cached; documents selected by a relevance threshold.
- Agentless / text-to-video baselines (LTXVideo, Veo2) cannot exceed ~20s and produce incoherent noise — **agentic planning is essential** for long-form coherent output.

### TheoremExplainBench + 5 metrics
240 theorems (80 easy/high-school, 80 medium/undergrad, 80 hard/grad), sourced from OpenStax/LibreTexts. Overall score = **geometric mean** of five dimensions (0–1), LLM judges at temperature 0:
- **Accuracy & Depth** — narration precise, intuitive + rigorous (text eval, GPT-4o over SRT transcript).
- **Visual Relevance** — frames align with concepts (keyframe extraction + GPT-4o).
- **Logical Flow** — coherent progression (text eval, GPT-4o).
- **Element Layout** — elements well-positioned/sized, no overlap (keyframe + GPT-4o).
- **Visual Consistency** — smooth motion, uniform style (Gemini 2.0-Flash over chunked segments).
Validated against 12 STEM annotators (scores in {0,0.5,1}); Spearman metric-human correlation + Krippendorff's α inter-rater agreement.

## Results / benchmarks
- **Success rate (complete video)**: o3-mini (medium) **93.8%** overall (best); GPT-4o 55.0%; Gemini 2.0-Flash 14.6%; Claude 3.5-Sonnet v1 2.1% (much better with RAG, 14.6%). RAG *hurt* the strong models (o3-mini 93.8% → 82.1%).
- **Overall metric score** on successful videos: o3-mini **0.77**, GPT-4o 0.78, human-made Manim videos 0.77. AI beats humans on Logical Flow (rigid structure) but loses on Visual Relevance & Element Layout (overlap/misalignment).
- **Retry ablation**: hard-difficulty cumulative success 3% (N=0) → 96% (N=5).
- **Metric-human correlation**: strong for Visual Relevance (ρ=0.72) and Element Layout (ρ=0.42, p=0.03); weak for Accuracy&Depth (ρ=0.14), Logical Flow (ρ=0.16), Visual Consistency (ρ=0.17) — text/coherence metrics are the weak links.
- **Interpretability study**: 15 participants all judged a subtly-flawed *text* explanation correct; after the *video* of the same flaw, **9/60% flipped to "incorrect"**; intuitiveness 3.3 → 3.9. Videos surface reasoning errors better than text.
- Errors: mostly **Manim API hallucinations** (nonexistent functions/params), then LaTeX rendering, then generic Python errors. ~$1500 API spend; A100.

## Novel vs SOTA-2026
- First framework + benchmark for **multimodal (video) theorem explanation** and its automated eval.
- Confirms **agentic hierarchical planning > agentless/text-to-video** for long structured generation.
- The transferable scientific insight: **forcing a model to encode structure/procedure visually exposes reasoning flaws hidden in fluent text** — a multimodal "verification" surface.
- Weakly correlated text metrics (Accuracy/Logical Flow) show current VLM/LLM judges are unreliable for narrative coherence — a caution for any LLM-as-judge.

## Adopt-relevance to Theoremata
Lower priority than DeepSeekMath-V2/ImProver (this is explanation/pedagogy, not proving), but two concrete, transferable ideas:
- **Multimodal / structured error exposure → meta-verification & critique.** The core finding (visual/structural encoding reveals flaws text hides) argues for a Theoremata "explain/diagram the proof step" surface as an *additional* verifier signal: force the model to render a proof's dependency structure (our proof-DAG!) or a worked diagram, and check consistency — flaws that survive a prose critique may fail a structural render. This complements our meta-verification gate as a cheap orthogonal check before the formal gate.
- **Staged agentic RAG + bounded retry-with-diagnosis.** The router pattern (classify → stage-specific queries → cache) and the `<THINKING>` root-cause-then-fix retry (N=5, near-0 → ~90% success) map directly onto our per-system generators' compile-error repair loop and our Manim/tooling-adjacent code generation. Adopt the retry-with-explicit-diagnosis structure and cached, stage-scoped retrieval.
- **Eval caution for ProofGrader**: their weak text-metric correlations (ρ≈0.14–0.17) are a warning that LLM-judge scores of *coherence/rigor* need human/formal grounding and calibration (reinforces DeepSeekMath-V2's meta-verification and our formal-oracle reward). Use geometric-mean aggregation across independent judged dimensions rather than a single blended score.
- **What we already do vs gap**: we already have a proof-DAG, retry loops, retrieval. REAL GAP (optional): a *visual/structural* explanation surface as an extra verification signal, and adopting geometric-mean multi-dimension grading + retry-with-diagnosis. Not core to proving; park as a "verifier-augmentation" idea.

## Verbatim-worthy details
- Planner hierarchy: Scene Outline → Vision Storyboard Plan → Animation & Narration Plan → Technical Implementation Plan.
- Retry: max N=5; N=0 ≈ 0% success, N=5 ≈ 90%; success = all constituent scenes render.
- Overall score = **geometric mean** of {Accuracy&Depth, Visual Relevance, Logical Flow, Element Layout, Visual Consistency}, each 0–1, judges at temperature 0.
- Judge assignment: text dims (Accuracy&Depth, Logical Flow) via GPT-4o on SRT transcript; Visual Relevance & Element Layout via keyframe extraction + GPT-4o; Visual Consistency via Gemini 2.0-Flash on chunked segments.
- Agentic RAG stages: storyboard / technical-implementation / error-correction; plugin-suitability classification first; cached queries; relevance-threshold doc selection.
- Human study: 15 raters, text→video flip 0/15 → 9/15 (60%); intuitiveness 3.3→3.9. Metric human study: 12 annotators, 40 videos, scores {0,0.5,1}, Krippendorff's α.
