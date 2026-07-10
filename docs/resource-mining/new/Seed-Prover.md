# Seed-Prover (ByteDance Seed AI4Math) — Resource-Mining Report

> Source: `resources/Seed-Prover-main/Seed-Prover-main/`
> Read: `README.md` + three sub-READMEs (`SeedProver/`, `SeedProver-1.5/`, `DeltaProver/`) + full text of all three papers (`SeedProver_0801_2.pdf` 12pp, `SeedProver-1.5.pdf` 21pp, `DeltaProver/delta0722.pdf` 28pp) via pypdf. Lean proof outputs (IMO 2025 P1/P3/P4/P5, miniCTX-v2 solutions, MiniF2F/Putnam zips) are *artifacts/data*, not engine code — spot-checked, not read line-by-line per the size constraint.
> **License: Apache-2.0** (permissive; NOT copyleft). Ideas *and* text may be reused with attribution + NOTICE; no clean-room needed. This is still a **research-artifact release**, not a runnable engine: there is **no** search/lemma/refinement *source code* here — the mechanisms live entirely in the papers. Port from the described designs, not from code.
> **Injection scan:** none found. All prose is descriptive research writing; the Lean files are formal proofs. No embedded instructions to the reader. Treat all extracted text as UNTRUSTED regardless. **POSSIBLE INJECTION: none observed.**

---

## 1. What it is + authors

The repo is ByteDance Seed AI4Math's public page for **three related formal-theorem-proving projects** built on the **Lean 4** verifier:

- **Seed-Prover 1.0** (arXiv 2507.23726) — the IMO-2025 system. A *lemma-style whole-proof* LLM prover with iterative Lean-feedback self-refinement + a 3-tier test-time scaling stack (light/medium/heavy). Solved **4/6 IMO 2025 during the contest** (30 pts, silver-level; 5/6 post-competition), saturates MiniF2F, 331/657 PutnamBench.
- **Seed-Geometry** — a dedicated neuro-symbolic Euclidean geometry engine shipped *with* Seed-Prover 1.0 (Lean lacks geometry support). Forward-chaining C++ reasoning engine + LLM auxiliary-construction proposer. Solved IMO 2025 **P2 in ~2 seconds**; 43/50 on IMO-AG-50 (beats AlphaGeometry 2's 42).
- **Seed-Prover 1.5** (arXiv 2512.17260) — the successor: an **agentic tool-use** Lean prover trained by large-scale **agentic RL**, plus a **sketch model** (NL→Lean-sketch bridge) and a hierarchical multi-agent test-time workflow. 88% PutnamBench, 80% Fate-H (graduate), 33% Fate-X (PhD), 11/12 Putnam 2025 in 9 h.
- **Delta-Prover** (arXiv 2507.15225) — a *separate*, **training-free** agent framework: a general-purpose LLM (Gemini 2.5 Pro) orchestrated over Lean 4 via reflective decomposition + iterative repair + a custom **DSL** for subproblem management. **95.9% MiniF2F-test** with no fine-tuning.

Authors: ByteDance Seed AI4Math (large author lists; Delta-Prover led by Yichi Zhou, Peng Sun, Hang Li, with USTC/PKU co-authors). All Apache-2.0.

---

## 2. Key mechanisms

### 2.1 Lemma-style whole-proof reasoning (Seed-Prover 1.0's central idea)
Instead of emitting one monolithic `theorem … := by …`, the model first emits standalone `lemma round1_… := …` declarations, then proves the main `theorem` by *applying* them (paper Fig. 2). Advantages the paper claims:
- **Progress is legible** — you know exactly which lemmas are proved vs. still `sorry`.
- **Modularity** — lemmas compile independently, are stored independently, and combine freely across inference trajectories.
- **Cross-pollination** — proofs of solved lemmas seed the model for the unsolved ones and the main goal.
Backed by a **lemma pool** per hard problem storing `{statement, name, full proof, difficulty, dependency relations}`, used to (1) retrieve most-relevant lemmas by name/statement, (2) sample the *hardest* lemmas by proof difficulty.

### 2.2 Iterative self-refinement against Lean feedback
Core loop (both 1.0 "light" and Delta-Prover's "Iterative Proof Repair", Delta Algorithm 1): generate proof `P₀` → run Lean → if error, feed back **{failed proof, tactic state + error message, retrieved Mathlib theorems/defs for the offending identifier}** → regenerate `P₁` → repeat up to *n* iterations × *m* rounds. Seed-Prover adds **self-summarization**: when the token budget is exhausted, summarize the trajectory and restart conditioned on the summary — this "surpasses single-pass token budgets". Empirically Pass@8×(8–16 refinements) ≈ Pass@64–256 single-pass; IMO 2022 P2 needed Pass@8192 *without* refinement but fell under light refinement. Two observed behaviors: (a) fixing Lean syntax from compiler feedback, (b) wholesale rewriting of the proof sketch.

### 2.3 Conjecture-and-prove loop / growing lemma library (the "heavy" engine)
A **proposer** module takes an unsolved problem (+ optionally proved lemmas) and emits **10–50 candidate conjectures** about problem *properties* (injective? surjective? periodic? monotone?) — broad, non-committal exploration, explicitly *unlike* Draft-Sketch-Prove which presumes you can already solve it. In **heavy** mode: seed a conjecture pool with **~5000** conjectures; prove/disprove each with the light loop; **proved conjectures graduate into the lemma pool**; new conjectures are proposed from proved lemmas. After days, the pool holds *thousands* of math facts. Each lemma is **scored by proof rate (low proof-rate lemmas are empirically the crucial ones!) × semantic relevance (LLM judge) × proof length**; unrelated/short ones are culled; the top few-hundred are handed to the medium loop to finish the main proof. The RL-trained model is taught to *select and integrate* these lemmas.

### 2.4 Test-time inference scaling — light / medium / heavy
- **Light** — outer self-refinement only, Pass@8–16 with ≤8–16 refinements, ~1–2 h. Depth on *one* proof.
- **Medium** — **outer + inner refinement**. Outer refines the main proof; **inner** takes the *lemmas the outer loop generated but failed to prove* and runs light (8×8) on each; any inner success is fed back into the outer prompt. Handles >1000-line proofs, hours→days. (IMO 2003 P6, 2025 P5.)
- **Heavy** — adds **breadth** via the conjecture/lemma pool above; days of wall-clock. (IMO 2025 P3, P4.)

### 2.5 Seed-Geometry
Neuro-symbolic, TongGeometry-lineage. (a) **Extended DSL**: ruler-and-compass construction grammar with *composite* macro-actions (isogonal conjugate, ex/insimilitude centers) to keep sequences short for both LLM and engine. (b) **C++ forward-chaining engine** via Pybind11 — ~100× the Python TongGeometry backend; forward-chains to deductive closure then **backward-traces fact dependencies** to separate problem context from the *auxiliary constructions* needed. (c) Single **policy** Seed model (they found a value model *hurts* under heavy search; no goal in the prompt) proposing the next auxiliary; **beam search** ranked by cumulative NLL, distributed across GPUs with async CPU reasoning overlapping LLM inference. (d) Data: mined 20+ yrs of olympiad stats, generated **230M unique auxiliary-requiring problems** (38B tokens) to train on.

### 2.6 Seed-Prover 1.5 additions
- **Agentic prover**: instead of regenerate-whole-proof, the model **incrementally invokes tools** — `verify_lean` (LooKeng REPL, one lemma at a time; verified lemmas are **cached as `axiom` in the running Lean context** and reused), `mathlib_semantic_search` (embedding retrieval pinned to Mathlib v4.22.0), and **Python execution** (numeric experiments). Budget: 64K seq, ≤28 tool calls = "Pass@1". Context-efficient because valid lemmas are cached not regenerated; enables pruning/backtracking.
- **Agentic RL** (VAPO + ReTool-style tool-integrated RL): outcome reward +1/−1 from Lean; multi-task prompts (from statement / from NL sketch / from failed-attempt summary); curriculum filtering (drop >3× solvable and never-solvable). RL drove accuracy 50%→90%, and *cut* avg tool calls 15→10 and seq length 28k→17k — the agent internalizes retrieved knowledge and searches less over time.
- **Sketch model** (Rubric RL): converts a formal statement + NL proof into a **lemma-style Lean sketch** with ≥3 `sorry`-admitted sub-lemmas; hybrid reward = Lean structural check **×** LLM-as-judge rubric (alignment / decomposition granularity / difficulty reduction / "junk-value" analysis); cheap NL prover disproves invalid lemmas before expensive formal proving. Binary reward `R=+1 iff N_lemmas≥3 ∧ S_FL≥0 ∧ S_NL≥0.7`.
- **Hierarchical multi-agent TTS**: 3 agents — **NL Prover** (Doubao-Seed-1.6) → **Sketch Model** → **Agentic Lean Prover** (Pass@3×3 per leaf lemma). Recursively re-decomposes unproved lemmas ("NL proof → Lean sketch"); reverts to sketch model when a lemma is *disproved*; a search-tree over sub-lemmas.

### 2.7 Delta-Prover mechanisms (training-free, Apache-2.0)
- **Iterative Proof Repair** (Alg. 1) — as §2.2, with Mathlib retrieval keyed on the *errored identifier*.
- **Reflective Decomposition** (Alg. 2) — NL plan `p` → formal **DSL sketch** `D` → auto-extract sub-problems `S₁…Sₙ` → solve each with repair loop → if any fail, **regenerate `D` informed by the list of unsolved sub-problems** (reflect on the decomposition itself) → consolidate.
- **Custom DSL over Lean 4** (`PlayM` monad over `TacticM`, records intermediate states, re-emits via Lean's delaborator). Four tactics: `Suppose` (introduce hypothesis), `Define` (introduce arbitrary expr, type inferred), `ShowBy` (pose a subgoal, *recording* its proof), `Conclude` (consolidate all recorded sub-proofs + dependency graph into one coherent Lean proof). Solves the gap that native `have` can't extract-as-statement and lemma-lists can't auto-reintegrate.

### 2.8 Benchmarks (headline)
| Bench | Seed-Prover 1.0 | Seed-Prover 1.5 | Delta-Prover | Prev SOTA |
|---|---|---|---|---|
| IMO 2025 | 4/6 contest, 5/6 post | 5/6 (P2 by geometry) | — | 5/6 NL (Gemini) |
| MiniF2F-test | 99.6% (medium) | — | **95.9%** (train-free) | 92.2% Kimina |
| MiniF2F-valid | 100% (medium) | — | — | 90.6% DSP-V2 |
| PutnamBench | 331/657 | **88%** (≈580/660) | — | 86/660 Goedel-V2 |
| Fate-H / Fate-X | — | 80% / 33% | — | — |
| CombiBench | 30% | — | — | 10% DSP-V2 |
| MiniCTX-v2 | 81.8% (light) | — | — | 44.3% o4-mini |
| IMO-AG-50 (geom) | 43/50 | — | — | 42 (AG2) |

---

## 3. Mapping to Theoremata (per module)

| Seed mechanism | Theoremata module | Fit / gap |
|---|---|---|
| Iterative self-refinement vs Lean feedback (§2.2) | `components/reason/proving/repair.rs` | **Strong overlap.** repair.rs already: localizes the broken step from a verifier `Span`, asks an injected `Repairer` for targeted fixes, re-verifies, iterates bounded rounds seeding from best-failing candidate, and *only* reports success when the verifier accepts. **Missing vs Seed:** (a) **self-summarization** to restart when the token/round budget is exhausted (Seed's key "beats single-pass" trick), (b) **Mathlib retrieval keyed on the errored identifier** injected into the repair prompt (both Seed & Delta do this). Both are additive to the existing seams. |
| Statement strengthening | `repair.rs` (`Adapter`) | Already present (weaker→stronger). Orthogonal to Seed but complements the conjecture-strengthening idea. |
| Lemma-style proving + growing lemma library + proof-difficulty sampling (§2.1) | `components/reason/proving/library.rs` | **Strong overlap** (LEGO-Prover port). library.rs has verified-lemma skills, request worklist, k-NN retrieval, dedup seam, 4-axis evolver. **Missing vs Seed:** the **proof-difficulty / proof-rate scoring** ("low proof-rate lemmas are the crucial ones") as a *sampling* signal, and **dependency relations** stored per lemma (Seed's pool keys). library.rs stores lemmas but the "sample the hardest by proof difficulty" scheduler is not the current least-`update_count` policy. |
| Conjecture proposer + conjecture pool → lemma pool graduation (§2.3) | `components/reason/search/proof_pool.rs` + `library.rs` + a *new* proposer seam | **Partial.** proof_pool.rs is a scored candidate pool with provenance lineage + all-pass stop — the right substrate for a *proof* pool, but Seed's **conjecture pool** (5000 property-conjectures, prove/disprove, graduate winners into the lemma library, re-propose from proved) has **no direct module**. The proposer (10–50 property conjectures per call; injective/surjective/periodic enumeration) is a **NEW** injectable seam. This is the "heavy" breadth engine and is Theoremata's biggest structural gap. |
| Lemma scoring (proof-rate × relevance × length) | `library.rs` / `proof_pool.rs` self-eval | Buildable now as a pure ranking fn; relevance already has an embedding path in library.rs. |
| Test-time light/medium/heavy tiers (§2.4) | `components/reason/search/driver.rs` (MCGS) + `ttc` controller | **Conceptual map, not literal.** driver.rs is PUCT/MCGS over goal-state DAG with transposition + a `TtcController` budget. Seed's tiers are an *orchestration* over refine-loop × inner-lemma-refine × conjecture-pool, not a tree search — closer to an **outer/inner refinement scheduler**. Medium's **inner refinement** (recurse light-loop onto the lemmas the outer loop failed to prove) maps cleanly onto library.rs requests + repair.rs and is buildable. |
| Sketch → holes → splice; reflective re-decomposition (§2.6, §2.7) | `components/reason/proving/sketch.rs` + `evolve_sketch.rs` | **Strong overlap.** sketch.rs already: informal sketch → per-hole subgoals as a sub-DAG → dispatch to per-hole prover → splice only when *all* holes close (refuses partial). This is exactly Delta's decompose→solve-each→consolidate and Seed-1.5's sketch-model workflow, minus (a) **reflective re-decomposition** (regenerate the sketch informed by the *list of failed sub-problems* — Delta Alg. 2 line 28), and (b) the **NL-prover → sketch-model → agentic-prover** three-agent recursion. evolve_sketch.rs (EVOLVE-BLOCK Elo evolution) is a different (AlphaProof) evolutionary flavor but shares the "editable sketch region" object. |
| Seed-Geometry (§2.5) | `components/prover/python/theoremata_tools/geometry*.py` (geometry_ddar, _synth, _rules, _wlog, _algebraic) | **Partial, honest gap.** Theoremata's geometry vertical does numeric-check + small sound forward-chainer, explicitly **no** auxiliary-construction search and no algebraic backend. Seed-Geometry's *architecture* (DSL composite actions, forward-chain-to-closure + **backward dependency tracing to isolate auxiliaries**, policy-LLM beam search over auxiliaries, value-model-considered-harmful finding) is a concrete blueprint — but the 230M-problem data + trained policy are GPU-gated. The `geometry_wlog.py` frame-normalization already echoes Seed's canonicalization step. |
| Verification gate | `components/reason/orchestration/certification.rs` + `components/verify/**/cert_*.py` | Seed's gate is simply "Lean compiles" (outcome reward). Theoremata's 3+1 gate + 12 cert kinds is *stronger*; Seed contributes the **caching-verified-lemmas-as-axioms** idea (1.5) to keep the running context small — relevant to how certification.rs threads verified sub-results. |
| Agentic tool-use prover (1.5) | `components/reason/orchestration/agent.rs` | Seed-1.5's incremental `verify_lean` + `mathlib_semantic_search` + `python_exec` tool triad, ≤28 calls, verified-lemma caching, is a direct template for the agent orchestration layer. The **adaptive tool-call reduction** (RL internalizes retrieval) is a training result, not portable, but the *tool set + caching contract* is. |
| Mathlib semantic search | `components/retrieval/python/theoremata_tools/retrieval.py` | Both Seed & Delta rely on embedding retrieval pinned to a fixed Mathlib commit; retrieval.py is the home. Seed adds the **error-identifier-keyed** retrieval trigger. |

---

## 4. Buildable-now vs GPU/model-gated (honest)

**Buildable now (architecture/orchestration — no training):**
- **Self-summarization restart** in `repair.rs` when a refine budget is exhausted (inject a `Summarizer` seam; restart the loop conditioned on the summary). Highest-value, lowest-cost port.
- **Error-identifier-keyed Mathlib retrieval** injected into the repair prompt (Delta Alg. 1 line 12) — wire `retrieval.py` into `repair.rs`.
- **Reflective re-decomposition** in `sketch.rs`: on partial failure, regenerate the sketch given the list of unsolved holes (Delta Alg. 2 line 24–28) rather than only refusing assembly.
- **Inner/outer refinement scheduler** (Seed "medium"): recurse the light loop onto outer-loop-failed lemmas via `library.rs` requests. Pure orchestration over existing seams.
- **Conjecture proposer + conjecture→lemma-pool graduation** (Seed "heavy"): a `Proposer` seam emitting property-conjectures, a conjecture store, prove/disprove via existing verify gate, graduation into `library.rs`. New but self-contained; the *pool/graduation/scoring* logic is deterministic and testable offline.
- **Lemma scoring by proof-rate × relevance × length** and **hardest-lemma sampling** — pure ranking functions over `library.rs`/`proof_pool.rs`.
- **Verified-lemma-as-axiom context caching** contract in the agent/certification layer.
- **Delta DSL pattern** — Theoremata already emits lemma-style sketches; the `Suppose/Define/ShowBy/Conclude` *contract* (extract-as-statement + auto-consolidate + dependency graph) is a design to align `sketch.rs` splice semantics with, even if implemented in Rust rather than Lean metaprogramming.

**GPU / model-gated (cannot reproduce without training runs):**
- The Seed-Prover base models (VAPO multi-stage RL; lemma-style RL; agentic tool-use RL; sketch-model Rubric RL). Theoremata treats these as *injected model seams* — the *harness* is portable, the *weights* are not.
- Seed-Geometry's trained policy model + 230M-problem / 38B-token dataset generation (7+ days of C++ search). The engine *architecture* is buildable; the neural auxiliary proposer needs the data + training.
- The specific benchmark numbers (they reflect trained models + days of compute per hard problem — e.g. AlphaProof cited at ~500 TPU-days/problem for context).

---

## 5. Injection + license line

- **License:** Apache-2.0 (permissive, not copyleft). Reuse of ideas *and text/code* is permitted with attribution + retention of NOTICE/copyright. **No clean-room required.** Nonetheless this repo ships no engine code, so ports are from the *paper descriptions*.
- **Injection scan:** No prompt-injection or embedded instructions found in READMEs, papers, or Lean files. All content is descriptive/formal. Treated as UNTRUSTED per policy. **POSSIBLE INJECTION: none observed.**

---

## 6. Prioritized adopt-list

1. **Self-summarization restart in `repair.rs`** — Seed's single biggest "beats Pass@k" lever; small, injectable, deterministic. **(P0)**
2. **Error-identifier-keyed Mathlib retrieval → repair/agent prompt** (`retrieval.py`↔`repair.rs`/`agent.rs`). Both Seed & Delta depend on it. **(P0)**
3. **Reflective re-decomposition in `sketch.rs`** (regenerate sketch from the failed-hole list) — turns the current refuse-on-partial into Delta's repair loop. **(P0)**
4. **Inner/outer refinement scheduler ("medium")** over `library.rs` requests + `repair.rs`. **(P1)**
5. **Conjecture proposer + conjecture→lemma graduation + proof-rate/relevance/length scoring** ("heavy" breadth engine) — biggest structural gap; new `Proposer` seam + conjecture store feeding `library.rs`/`proof_pool.rs`. **(P1)**
6. **Verified-lemma-as-axiom context caching** contract in agent/certification layer (1.5). **(P1)**
7. **Agentic tool-triad template** (`verify_lean` one-lemma-at-a-time + semantic search + python-exec, bounded calls) for `agent.rs`. **(P2)**
8. **Seed-Geometry architecture blueprint** for the `geometry_*.py` vertical: composite DSL actions, forward-chain-closure + **backward dependency tracing to isolate auxiliaries**, policy-LLM beam search, and the empirical "value-model-harmful-under-heavy-search" caution. Engine buildable; neural proposer data/GPU-gated. **(P2)**
9. **Delta `PlayM` DSL contract** (`Suppose/Define/ShowBy/Conclude`, record + delaborate + dependency graph) as the reference for `sketch.rs` splice + `library.rs` dependency storage. **(P3)**
