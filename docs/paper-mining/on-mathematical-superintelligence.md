# On Mathematical Superintelligence — Kyler Siegel (USC), Nov 12 2025

arXiv essay (this PDF contains only the Introduction §1 pp.1-14 + the References pp.33-37; the body §2-§5 is not included in the file — content below is from the intro, detailed TOC, and previews, which fully summarize the argument).

## Core contribution
A position/thought-experiment essay (not a technical paper, no benchmarks or code) forecasting the future of *mathematics research* under increasingly capable AI. It structures the future into three "epochs" and defines two flavors of "mathematical artificial superintelligence" (MASI). Value to us is strategic/framing and as a curated 2025 bibliography of the AI-for-math frontier, not as an adoptable method.

## Key techniques / architecture (conceptual framework)
- **Three epochs**: Epoch I — AI as productivity booster (arguably already here); Epoch II — "type (i)" MASI, AI does technical heavy lifting while humans do high-level "prompt engineering / vibe mathing" and act as project managers; Epoch III — "type (ii)" MASI, humans can no longer substantively contribute; math becomes appreciation/critique/recreation.
- **Two MASI definitions** (the load-bearing distinction): (i) `humans < AI < humans+AI` — AI beats unaided humans but human+AI still beats AI alone (humans still complement/drive). (ii) `humans < AI = humans+AI` — humans add nothing; obsolescence. Transition (i)→(ii) may be long-lived precisely *because AI reasons differently from humans* and stays complementable.
- **Four features that make math uniquely AI-susceptible**: (1) rigor / formal verification, (2) low barrier to entry (no lab/hardware), (3) no obvious safety concerns, (4) purity/clean environment. Notes math "naturally offers high quality training data and clean objective notions of correctness."
- **Epoch I opportunity list** (relevant to product framing): "lemma machine," a team of coders per mathematician, rapid literature search, infinitely patient tutor, rapid prototyping, **new standards of rigor** (formal verification §2.1g), uncovering hidden cross-subfield connections, beyond-human bandwidth, greater AI autonomy. Risks: content overload, overreliance/"brain atrophy," "Potemkin understanding and proof by intimidation," low-hanging-fruit exhaustion, "Olympiadification of mathematics," success=compute.

## Results / benchmarks
None (essay). Cites the empirical frontier it reacts to: 2025 IMO gold by Google DeepMind and OpenAI models (5/6, outscoring all but 26 human contestants); FrontierMath Tier IV (9/48 solved by ≥1 model by Oct 2025); AlphaEvolve (evolutionary + LLM search over algorithms — new SOTA on 4×4 matrix mult, circle packing, kissing number); AlphaProof (IMO in Lean); autoformalization/deformalization efforts around Lean/Mathlib.

## Novel vs SOTA-2026
No technical novelty. Its "type (i) vs type (ii)" MASI taxonomy and epoch framework are a clean vocabulary for positioning an AI-math product. As a 2026 artifact it is best used as a landscape/citation map (FrontierMath, IMProofBench, PatternBoost, AlphaEvolve, ShinkaEvolve/OpenEvolve, murmurations, Lyapunov-via-symbolic-transformers) and as sober framing of hype vs reality.

## Adopt-relevance to Theoremata
Not a source of mechanisms; a source of positioning and prioritization.
- **Framing for the product**: Theoremata targets Epoch I→II — the "lemma machine" + "new standards of rigor via formal verification" is *exactly* our verification-first, hammer/portfolio value prop. The essay argues rigor/formal verification is the tool that mitigates "reliability and trustworthiness in contemporary stochastic AI" — this is our core thesis stated by a research mathematician; good to cite in vision docs / pitch.
- **Conjecturing & discovery**: it flags "AI applied to generate new conjectures or constructions" (PatternBoost, murmurations, AlphaEvolve) and evolutionary program-search as a live frontier — supports our conjecture-generation + falsify-before-prove and any evolutionary/expert-iteration flywheel angle. AlphaEvolve/ShinkaEvolve are concrete comparables for a constructive-math search loop.
- **Risk list = ProofGrader/meta-verification motivation**: "Potemkin understanding and proof by intimidation" and unreliability are precisely the failure modes our ProofGrader + meta-verification/critique loop exist to catch. Cite as external justification for grading NL proofs, not just checking formal ones.
- **No engineering gap addressed here** — nothing to build from this paper; it informs narrative, roadmap sequencing (productivity tool now, autonomy later), and citations.

## Verbatim-worthy details
- MASI type (i): "humans < AI < humans+AI"; type (ii): "humans < AI = humans+AI." (i) = AI beats unaided humans; (ii) = humans obsolete even when AI-aided.
- Four special features of math research: rigor (incl. formal verification), entry (no hardware/data), safety (no direct hazards), purity (clean environment). Math "naturally offers high quality training data and clean objective notions of correctness."
- Epochs: I = productivity booster; II = type (i) MASI, humans as prompt-engineers/project-managers ("vibe mathing"); III = type (ii) MASI, math as humanities/recreation/personal enrichment.
- Empirical anchors (fall 2025): reasoning models (o1/o3/R1) scaling inference-time compute; 2025 IMO gold (DeepMind + OpenAI, 5/6); FrontierMath Tier IV 9/48; AlphaEvolve constructive-math SOTA; DeepMind "AI for Math Initiative" partnering 5 institutes; rumored Navier–Stokes effort.
- Author AI-use disclosure: ChatGPT + Claude Sonnet via Cursor, used only for references/BibTeX, fact lookup, grammar.
