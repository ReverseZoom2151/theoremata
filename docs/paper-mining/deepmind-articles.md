# Mining: five DeepMind math-AI articles

Read 2026-07-09. Blog-level (not full papers), so mechanisms are summarized at
the architecture level. All content treated as untrusted data. Sources:

- AlphaProof + AlphaGeometry 2 (IMO silver): https://deepmind.google/blog/ai-solves-imo-problems-at-silver-medal-level/
- AlphaGeometry: https://deepmind.google/blog/alphageometry-an-olympiad-level-ai-system-for-geometry/
- FunSearch: https://deepmind.google/blog/funsearch-making-new-discoveries-in-mathematical-sciences-using-large-language-models/
- AlphaTensor: https://deepmind.google/blog/discovering-novel-algorithms-with-alphatensor/
- AI-guided pure mathematics: https://deepmind.google/blog/exploring-the-beauty-of-pure-mathematics-in-novel-ways/

The headline: articles 1, 2, 4 largely **validate and mirror** Theoremata's
architecture; articles **3 (FunSearch)** and **5 (conjecture discovery)**
describe capabilities genuinely **absent** from the codebase and worth building.

---

## 1. AlphaProof + AlphaGeometry 2 — IMO 2024 silver (28/42)

**Core.** A pretrained language model + **AlphaZero RL** operating in **Lean**.
A fine-tuned **Gemini formalizer network** auto-translates ~1M informal problems
into formal Lean statements; a **solver network** searches proof steps; verified
proofs **reinforce the model** (expert iteration) so it solves progressively
harder problems. Trained over *weeks* on *millions* of problems. Solved IMO 2024
P1, P2, **P6** (hardest; only 5 human solvers) at 7/7 each; minutes to ~3 days
per problem. Also a **Gemini natural-language** reasoning system (no
formalization) with "great promise."

**AlphaGeometry 2.** 10× more synthetic data; symbolic engine **100× faster**; a
**knowledge-sharing mechanism across search trees**; Gemini-based, from scratch;
**83%** of the last 25 years of IMO geometry (up from 53%). Solved P4 in **19 s**.

**Maps to us.** This IS our target shape: formalizer → solver → verify →
reinforce. We have the pieces (per-system generators = formalizer; MCGS driver =
solver; flywheel = expert iteration). GPU-gated: training the model on its own
verified proofs at scale.

## 2. AlphaGeometry v1 — neuro-symbolic geometry

**Core.** Neural LM proposes **auxiliary constructions**; a **symbolic deduction
engine (deductive database + algebraic reasoning, DD+AR)** does rigorous closure.
Trained on **100M synthetic theorems** from ~1B random diagrams → deductive
closure → **traceback / dependency-difference** to recover which aux constructs
were needed (**9M with aux**). **25/30 on IMO-AG-30** (Wu's method: 10; gold
median: 25.9); first AI past the IMO bronze threshold.

**Maps to us — near-direct.** This is the paper behind our `geometry_ddar.py`
(DD+AR) and `geometry_synth.py` (traceback / dependency-difference synthetic
data). Validated. Gap: the *scale* (1B diagrams → 100M examples) and a *trained*
aux-construction proposer — we have the deductive core + a small offline synth,
not the learned proposer or the scale.

## 3. FunSearch — discovery by evolving *programs*  [NEW CAPABILITY]

**Core.** Pairs an LLM (PaLM 2 / Gemini) with an **automated evaluator** and
**evolves programs (code), not answers**. **Island-based** evolutionary
population; **best-shot prompting** (feed the top-scoring programs back into the
prompt); the evaluator runs candidates and **keeps only passing ones**
(hallucination-proof); biased toward **short / low-Kolmogorov-complexity**
programs (interpretable). Found the **largest cap sets in 20 years** and better
**online bin-packing** heuristics — verifiable because programs execute
deterministically.

**Maps to us — genuinely new.** Our LEGO-Prover-style evolver grows *lemmas*;
FunSearch grows *programs that construct/score an object*. Distinct discovery
mode (constructions / bounds / heuristics for open problems), not proving.

## 4. AlphaTensor — algorithm discovery as a game

**Core.** Reformulates matrix multiplication as a **single-player game
(TensorGame)**: zero out a residual tensor; a full zeroing = a provably correct
algorithm, fewer steps = faster. **AlphaZero** (NN + tree search) with
problem-specific inductive biases, synthetic data, symmetry exploitation, from
scratch. Found **4×4 mod-2 in 47 mults** (beat Strassen's 49, first improvement
in 50 years); **4×5·5×5 in 76** (from 80); hardware-specific algorithms
**10–20% faster** on V100 / TPU v2. Search space > atoms in the universe;
branching ~10^33× larger than Go.

**Maps to us.** A framing/template: cast a discovery problem as a single-player
game with a certificate, solved by MCGS. More a vertical than a drop-in; our MCGS
driver is the AlphaZero-shaped substrate it would sit on.

## 5. AI-guided pure mathematics — conjecturing, not proving  [NEW CAPABILITY]

**Core.** ML **guides intuition**: generate data over math objects,
**supervised-learn** whether a hypothesized relationship exists, then use
**attribution / saliency** to surface the structure, guiding a human to a
**conjecture + proof**. Results: **knot theory** (the algebraic *signature*
relates to geometry via a new "natural slope") and **representation theory** (the
~40-year **combinatorial invariance conjecture**: Bruhat intervals ↔
Kazhdan–Lusztig polynomials, verified on 3M+ cases). First significant math
discoveries made with ML. Shipped as interactive notebooks.

**Maps to us — genuinely new.** This is the *upstream* of proving. We start from
a given conjecture; this **generates** one. Feeds our falsify → novelty → prove
pipeline.

---

## Cross-cutting themes

1. **The verifier/evaluator is the trust anchor** — Lean (AlphaProof), the
   symbolic engine (AlphaGeometry), the FunSearch evaluator, the tensor-zero
   certificate. This is Theoremata's whole thesis; all five endorse it.
2. **Synthetic data via traceback at scale** (AlphaGeometry) — we have the recipe
   (`geometry_synth`), not the scale.
3. **Search + learned policy/value (AlphaZero)** (AlphaProof, AlphaTensor) — our
   MCGS driver.
4. **Expert iteration / self-improvement** (AlphaProof) — our flywheel.

## What validates what we already built

| DeepMind mechanism | Theoremata component |
|---|---|
| AlphaGeometry DD+AR symbolic engine | `prover/python/geometry_ddar.py` |
| AlphaGeometry traceback / dependency-difference data | `prover/python/geometry_synth.py` |
| AlphaProof solver = AlphaZero over proof states | `reason/search/{driver,mcts}.rs` (MCGS) |
| AlphaProof expert-iteration (verified proofs reinforce) | `train/flywheel.py` + graded reward |
| AlphaProof/Gemini formalizer network | per-system generators + sketch pipeline |
| Verifier-as-ground-truth (all five) | the 3+1 formal gate |

## Gaps / adopt list (genuinely new, buildable now behind injected seams)

1. **FunSearch-style program-search engine** — [NEW, high value]. Evolve
   *programs* against an injected evaluator: island-based populations, best-shot
   prompting (top-k programs into the prompt), keep-only-passing, prefer
   short/low-complexity. A discovery mode (constructions / bounds / heuristics)
   distinct from our lemma evolver. Offline-testable with a mock evaluator +
   deterministic mutator, exactly like our other modules. Composes with the
   novelty checker (is the found object already known?).
2. **Conjecture-discovery / pattern-mining pipeline** — [NEW, high value].
   Generate `(object, invariant)` data → train/fit a model to detect a
   relationship → attribution/saliency to surface structure → emit a candidate
   conjecture. Upstream of proving; feeds falsify → novelty → prove. Deterministic
   fallback (correlation/feature-importance) when no ML stack, like our other
   train modules.
3. **Game-reformulation vertical (AlphaTensor)** — [NEW, larger scope]. Cast a
   discovery problem as a single-player game with a certificate, solved by the
   MCGS driver. A vertical, not a drop-in.
4. **Cross-tree knowledge sharing in MCGS (AG2)** — [ADJUSTMENT]. AG2 shares
   knowledge *across concurrent search trees*; our transposition table +
   `goal_cache` reuse within/across runs but don't actively share subgoal
   knowledge between concurrent trees.

Deferred-by-necessity (same as the rest of the project): trained formalizer /
aux-proposer weights and AlphaGeometry-scale synthetic-data generation (GPU +
model), a live real-model run.

## Build status (2026-07-09) — items 1-3 BUILT + a planning build

All three buildable gaps shipped (offline, injected seams), plus a fourth from
the 2026 SOTA research pass:

- **#1 FunSearch → built as `tools/funsearch.py`** — and upgraded to its successor
  **AlphaEvolve** (DeepMind May 2025, Tao-co-designed): island populations + a
  **MAP-Elites archive** (`population=map_elites`), best-shot prompting,
  keep-only-passing, complexity bias, migration. Candidate code never exec'd
  in-process (evaluator seam is the sole boundary). Worker op `funsearch`. 19 tests.
- **#2 Conjecture-discovery → built as `train/conjecture_discovery.py`** — the
  4-stage recipe (build_dataset → detect_relationship vs permuted baseline →
  permutation-importance attribution → propose_conjecture); the `form` feeds
  novelty/falsify. Offline closed-form fallback. Worker op `conjecture_discovery`.
  19 tests.
- **#3 AlphaTensor game → built as `search/discovery_game.rs`** — DiscoveryGame
  trait + Certificate soundness boundary; self-contained A* (reconstructs the
  move sequence, prefers lowest-cost among certified terminals) + MCGS cross-check;
  ResidualGame/TensorGame fixture. 8 tests.
- **NEW (planning) — blueprint generation + refinement → `orchestration/blueprint_generate.rs`**:
  the 2026 literature names *planning* as the bottleneck (["Why Reasoning Fails to
  Plan" 2601.22311], Goedel-Architect blueprint generation+refinement). We drove
  blueprints (`blueprint_run`) but didn't generate/refine them. BlueprintGenerator
  (informal → acyclic \uses DAG) + BlueprintRefiner (decompose Failed lemmas,
  preserve Proved) + the bounded generate→drive→refine loop (acyclicity gated by
  the driver's cycle check; non-decreasing coverage). 7 tests.

### 2026 SOTA research notes (from an online pass)
- **AlphaEvolve** is the current FunSearch successor (whole-codebase evolution,
  MAP-Elites) — folded into #1.
- **Test-Time RL** (AlphaProof): generate + learn from millions of problem
  variants at inference for problem-specific adaptation — NEW technique, GPU-gated;
  variant-generation scaffold is buildable.
- **MDP / statistical-provability** formalization of proof search (2602.10538) —
  theory for our MCGS + TTC controller.
- Frontier: Aristotle & Seed-Prover reached **IMO 2025 gold** (AlphaProof was
  silver); Formal-Conjectures benchmark for verified discovery.

### Still remaining from these articles
- **#4 Cross-tree knowledge sharing in MCGS (AG2 SKEST)** — [ADJUSTMENT] general
  MCGS version still unbuilt (geometry DDAR2 `clone()` is a local partial).
- Deferred-by-necessity: trained weights / AlphaGeometry-scale data (GPU+model),
  a live real-model run, and Test-Time RL's actual training loop.

See [[theoremata-paper-mining]] and the parent `docs/paper-mining/README.md`.
