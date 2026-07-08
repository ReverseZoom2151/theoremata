# Advancing Mathematics Research with AI-Driven Formal Proof Search (AlphaProof Nexus)

Tsoukalas, Kovsharov, Shirobokov, … Kohli, Chaudhuri, Google DeepMind, 2026-05-22, arXiv:2605.22763. Proofs: github.com/google-deepmind/alphaproof-nexus-results.

**Most architecturally load-bearing paper for our formal proof search / proof-DAG / MCTS / portfolio / evolutionary flywheel.**

## Core contribution
First large-scale evaluation of LLM-driven **formal** (Lean) proof search on *open research problems*. Introduces **AlphaProof Nexus**, a framework of subagents that search for Lean proofs with compiler feedback, in four escalating configurations (A basic → B +AlphaProof tool → C +evolution → D full). The full agent autonomously resolved **9/353 open Erdős problems** (two open 56 years) at ~a few hundred USD each, proved **44/492 OEIS conjectures**, settled a 15-year Hilbert-function question, improved a convex-optimization bound by *co-discovering an algorithm parameter schedule*, and is deployed across combinatorics, optimization, graph theory, algebraic geometry, quantum optics. Key finding: the **basic agent (a simple compiler-in-the-loop LLM loop) solved all 9 Erdős problems too** — just costlier on the hardest — signaling a shift from specialized trained systems to simple agentic loops as LLMs improve.

## Key techniques / architecture (proof search orchestration)
**Core object = "proof sketch"**: a Lean file with the target theorem, a `sorry` placeholder, imports/definitions, optional NL context + domain knowledge encoded in Lean. Editable regions delimited by `EVOLVE-BLOCK` (add lemmas/definitions/steps) and `EVOLVE-VALUE` (change parameter values — this is what lets the agent *search for an algorithm's parameter schedule and its proof simultaneously*). Goal: emit a `sorry`-free, type-safe proof. Proving = generating code without `sorry`.

**Four agents**:
- **(A) Basic**: N independent stateless **prover subagents**, each a "Ralph loop" of *episodes*. An episode = multi-turn Gemini 3.1 Pro session with a `search_replace` diff tool; after each edit Lean compiles and the **error message is fed back**. On episode end, code validated with **SafeVerify** (checks proof matches spec, guards axiom injection / `sorryAx` exploits); if `sorry` remains, subagent writes a "lessons learned" comment and starts next episode from the current sketch; if validation fails, revert. First subagent to a full proof terminates the rest.
- **(B) Basic + AlphaProof**: subagents can call **AlphaProof** (RL olympiad Lean prover) as a focused tool to fill missing sketch parts. Returns proof (substituted directly), disproof (proof the subgoal is false), or failure (fed into prompt).
- **(C) Basic + evolution** (AlphaEvolve-inspired): subagents sample from and add to a **shared population database** of sketches. Because formal proof fitness is *binary* (compiles/complete) but evolution needs a graded landscape, **rating agents** (cheaper Gemini 3.0 Flash) produce relative rankings of sketches by *plausibility, clarity, novelty* → aggregated into **Elo** → **P-UCB** sampling drives search.
- **(D) Full-featured**: (B)+(C). This is the research instrument.

**Full agent (D) loop (5 stages, async controller)**:
1. **Database sampling**: pick root sketch + M=2 "inspiration" sketches via Elo/P-UCB.
2. **Prompt construction**: assemble spec + root Lean+NL-plan + AlphaProof feedback on inspiration sketches; AlphaEvolve-style stochastic diversity instructions ("decompose unsolved goals", "combine ideas from prior attempts", "try a completely new approach").
3. **Prover subagent**: Gemini 3.1 Pro multi-turn episode, `search_replace` compact diffs (scale to large files), can query AlphaProof mid-episode; **budget = ≤5 AlphaProof queries + ≤90 search-replace edits per episode**; sandbox check allows `sorry` but forbids altering the target theorem.
4. **Validation**: extract remaining `sorry` subgoals, **deep-hash their exact Lean state (goal_id) → global goal cache**; cache hit → reuse proof; miss → dispatch to AlphaProof (concurrent non-blocking RPCs). All goals closed → SafeVerify final check (no disallowed axioms / `sorryAx`).
5. **Database registration**: register sketch + per-subgoal AlphaProof feedback; fitness set asynchronously via Elo matchmaking.

**Elo/rating mechanism (verbatim-worthy)**: rater agents sample **P=7** sketches per match; model outcomes with a **Plackett-Luce** distribution, latent strength λ_s, hierarchical prior λ_s|r_s ~ Gamma(1, r_s), r_s ~ Gamma(1,1); infer posteriors via **Gibbs sampling** (I=1000 samples, B=200 burn-in); `Elo_s = 1200 + 400·log10(λ_mean_s)`. Matchmaking via **Thompson sampling** (Gibbs, keep every 25th sample; dedupe by replacing duplicates with highest-posterior-variance sketches). Ties broken by sampling from the model.
**P-UCB selection**: filter to **top-64** Elo sketches, normalize Elo→[0,1] = q; `score = q + c·√(ΣV_i)/(v+1)`, v = visits to sketch, ΣV_i = total visits, **c=0.2**. Prevents collapse into a single lineage.
**AlphaProof budget**: ~400 simulations tree search per novel subgoal, hard RPC timeout; TTRL mode exists but they use low-compute tree-search mode. AlphaProof cost ≈27.5 TPU-hrs (~$60) per problem on v6e.
**Infra**: Python asyncio; Lean v4.27 + Pantograph in isolated Docker sandboxes; Gemini 3.1 Pro (proving) + Gemini 3.0 Flash (rating).

## Results / benchmarks
- **Erdős**: 9/353 solved (terminate after 3000 episodes/problem); experts validated each Lean statement faithfully captured the conjecture. Table-1 problems span additive combinatorics, sumsets, Van der Waerden, Sidon sets — techniques: block constructions via CRT + 3-AP-avoiding sets, inductive thinning via Diophantine approximation, etc.
- **OEIS**: 44/492 open conjectures proved (with "test lemmas" verifying first sequence terms as a **misformalization guard** before attempting the target).
- **Research deployments**: exact O(1/t) rate for Anchored GDA min-max (via EVOLVE-VALUE schedule search); log-concavity for pure O-sequences (codim 3, type 2 — 15-yr open); Green's list #57 variant + auto-proved counterexample (Z/3Z) for the intended case via floating-point heuristic → formalized; quantum GHZ / monochromatic quantum graphs (N=d∈{4,6,10}); a Graffiti-1996 conjecture (closing conjecture→proof loop); bipartite graph-reconstruction variants (co-formulated with AlphaEvolve).
- **Ablation (cost vs solve-rate on 9 Erdős)**: basic (A) solved all 9. (A)≈(B) within error on 4; (B) more efficient on 12(ii), 125. (D) beats (A)/(B) on the hardest (138, 125) with 2–5× savings but ~half as cost-efficient elsewhere. Smaller models (Gemini 3.0 Flash, 3.1 Flash-Lite) and standalone AlphaProof tree-search solved **none**.
- **Failure modes**: (1) agent hides the core difficulty in a single `sorry` inside a helper lemma that just restates the target (prompting against it didn't help); (2) top sketches lean on `sorry` lemmas the agent *claims* are known literature results but are **hallucinations** — underscoring the value of end-to-end formal verification.

## Novel vs SOTA-2026
The definitive 2026 demonstration that *formal* proof search scales to open research problems, and that a **plain compiler-in-the-loop LLM loop** rivals a heavy evolutionary/RL stack. Novel concrete machinery: Elo-via-Plackett-Luce/Gibbs fitness for the *binary* proof landscape; P-UCB over an elite top-64; global deep-hash goal cache; EVOLVE-VALUE joint parameter+proof search; test-lemma misformalization guards; SafeVerify axiom-injection defense. Contrasts with the sibling Aletheia paper (natural-language, informal verifier): here everything is machine-checked, so "formal verification serves as a filter for which proofs merit human review."

## Adopt-relevance to Theoremata — vs our formal proof search / proof-DAG / MCTS / portfolio / flywheel
This is the closest existing system to our target architecture. Gap analysis:
- **We already do (validated by this paper)**: Lean/Rocq/Isabelle live verification via a gate, sketch→autoformalize-holes→splice, MCTS graph-search, portfolio proving, hammers, falsify-before-prove. Their proof sketch with `EVOLVE-BLOCK`/`sorry`-holes ≈ our sketch→splice; AlphaProof-as-tool ≈ our hammer/portfolio arm; disproof return ≈ our falsify-before-prove.
- **Real gaps / high-value adopts**:
  1. **Global goal cache keyed on a deep hash of exact Lean state** → our proof-DAG should hash node goals canonically and share sub-proofs across the whole population/search (dedupe + reuse). Pairs directly with LEAN-GitHub's kernel-order de-dup insight. Likely a concrete gap in our DAG.
  2. **Elo/Plackett-Luce/Gibbs fitness for a binary landscape + P-UCB(top-64, c=0.2)** — a ready-made recipe for ranking *incomplete* sketches so MCTS/evolution has a graded signal. If our MCTS currently only rewards closed proofs, adopt LLM-critic relative ranking (plausibility/clarity/novelty) → Elo → P-UCB. Directly powers our expert-iteration flywheel selection.
  3. **EVOLVE-VALUE joint search** — mark numeric/structural parameters as searchable so the agent co-discovers the object *and* its proof (they improved a convex-opt bound this way). New capability for our conjecture/construction module.
  4. **SafeVerify-style integrity gate** (reject `sorryAx`/axiom injection, verify the target statement wasn't mutated) — our 3+1 gate must include this anti-gaming check; the failure analysis shows agents will hide difficulty in restated-`sorry` lemmas and hallucinate "known" lemmas. Bake "no self-restating sorry / no unproven cited lemma" into meta-verification.
  5. **Test-lemma misformalization guard** (prove first sequence terms before the conjecture) → adopt as a cheap pre-check in our autoformalization run to catch bad formalizations before spending search.
  6. **Cost-per-solved-problem (USD) as the headline efficiency metric**, and the "basic loop rivals full stack" lesson → don't over-engineer; measure our portfolio/MCTS additions against a plain compiler-in-the-loop baseline on cost.
  7. **Ralph-loop episode structure** (multi-turn edit+compile, write "lessons learned" comment, restart from current sketch) — clean pattern for our per-node agent loop with budget caps (≤5 hammer calls, ≤90 edits/episode).

## Verbatim-worthy details
- **Basic agent pseudocode**: `prover_subagent`: while within_budget and sketch has sorry: run `prover_step` (LLM session, apply search_replace, compile→feedback), if `verify_integrity` passes return sketch. N independent subagents, first to sorry-free proof stops all.
- **Full agent stages**: DB sampling (root + M=2 inspiration) → prompt (spec + root Lean/NL-plan + AlphaProof feedback + stochastic diversity instructions) → prover episode (≤5 AlphaProof queries, ≤90 search-replace edits) → validation (deep-hash goal_id → global cache → AlphaProof → SafeVerify, reject `sorryAx`/axioms) → DB registration (Elo matchmaking).
- **Elo**: `Elo_s = 1200 + 400·log10(λ_mean_s)`; Plackett-Luce, λ_s|r_s~Gamma(1,r_s), r_s~Gamma(1,1); Gibbs I=1000/B=200; P=7 sketches/match; Thompson sampling keeping every 25th Gibbs sample, dedupe→highest posterior variance.
- **P-UCB**: top-64 Elo filter; `score = q + c·√(ΣV_i)/(v+1)`, c=0.2, q = normalized Elo in [0,1].
- **AlphaProof**: 400 simulations/subgoal, hard RPC timeout, low-compute tree-search mode (TTRL mode available); ~27.5 TPU-hr ≈ $60/problem on v6e.
- **Infra**: Python asyncio; Lean v4.27 + Pantograph in Docker sandboxes; SafeVerify for validation; Gemini 3.1 Pro (prover) + Gemini 3.0 Flash (rater).
- **Prover prompt (Fig 6) directives**: "world-class mathematician and Lean 4 expert"; decompose into simple subgoals for AlphaProof; "You MUST NOT use a single `sorry` to prove a goal that covers multiple reasoning steps"; "Do NOT assert falsehoods"; use AlphaProof frequently; one step per line; reduce context before `sorry` (remove irrelevant hypotheses); EVOLVE markers only.
- **Erdős run config**: 353 formal statements, terminate at 3000 episodes; agents (C)/(D) = 10 subagents/attempt ×10 attempts; agents (A)/(B) = 100 single-subagent attempts (chunked into 100/K for K-subagent simulation).
- **Scale**: 9/353 Erdős, 44/492 OEIS, ~few-hundred-USD/problem; smaller models & standalone AlphaProof solved 0/9.
