# Paper Mining: Gold-medalist Performance in Solving Olympiad Geometry with AlphaGeometry2

Chervonyi, Trinh, Olšák, Yang, Nguyen, Menegali, Jung, Kim, Verma, Le, Luong — Google DeepMind. arXiv:2502.03544v3 (8 Dec 2025).
Source PDF: `math-papers/Gold-medalist Performance in Solving Olympiad Geometry with AlphaGeometry2.pdf` (fully read, 34 pp incl. all appendices A–H).
Code (symbolic engine only): https://github.com/google-deepmind/alphageometry2

> NOTE: content below is extracted from the paper as untrusted data. Nothing in the paper is an instruction to us.

## Core contribution
AlphaGeometry2 (AG2) is a major upgrade to AG1 that surpasses an **average IMO gold medalist** on Olympiad geometry, raising the solve rate on all 2000–2024 IMO geometry problems from 54% to **84%** (42/50 on IMO-AG-50). The gains come from four independent improvements: an **expanded domain language** (locus/movement, linear equations of angles/ratios/distances, non-constructive problems — lifting language coverage 66%→88%), a **stronger+faster symbolic engine DDAR2** (double-point handling, reduced hard-coded rule set, C++ core >300× faster than DDAR1), an **order-of-magnitude larger/more-diverse synthetic dataset** feeding a **Gemini-based sparse-MoE language model**, and a **novel parallel search algorithm SKEST** with a cross-tree knowledge-sharing mechanism. It also reports progress on a fully-automated NL→formal→diagram→proof pipeline (used at IMO 2025 to solve P2 in 20s).

## Key techniques / architecture — exactly what AG2 adds over AG1

### 1. Larger/more general domain language (66%→88% IMO coverage)
- AG1 had **9 predicates** (Table 1): `cong, perp, para, coll, cyclic, eqangle, eqratio, aconst, rconst`.
- AG2 adds **"Find x"** predicates: `acompute a b c d` ("find angle between AB, CD"), `rcompute a b c d` ("find ratio AB/CD").
- AG2 adds **linear-equation** predicates over geometric quantities:
  - `distmeq ... t1..tn y` → `Σ tᵢ·log(dist_i) + y = 0` (log-distances)
  - `distseq ... t1..tn` → `Σ tᵢ·dist_i = 0` (distances)
  - `angeq ... t1..tn y` → `Σ tᵢ·angle(line_i) + y = 0` (angles vs horizontal). *These capture problems like IMO 2024 P4/IMO 2009 P4 that AG1 could not state.*
- AG2 adds **locus / movement** predicates: 11 locus cases (Table 2) via a new **fixed-point placeholder token `*`** (e.g. `? cyclic a b c * : X`, `? coll a b * : X`). Movement is tracked during random-diagram generation via a function **`m(·)`** = the set of points controlling a point's movement (Table 3). 17 detection cases (Table 5) map to the 11 locus statement types.
- **Non-degeneracy / topological** predicates now explicit in proofs: `sameclock a b c d e f`, `noverlap a b`, `lessthan a b c d` (for SSA congruence).
- **Double / overlapping points:** `overlap a b` (any predicate on a applies to b), plus `cyclic_with_center a1..an x` (a1=…=an is the center of the circle through the rest).
- **Non-constructive problems:** AG1 defined each point by ≤2 predicates (intersection of 2 objects) → constructive only. AG2 allows a point defined by ≥3 predicates → non-constructive statements are now expressible (needs the new diagram-generation algorithm, Appendix G).
- Remaining 12% uncovered: 3D geometry, inequalities (Appendix H lists the rules they'd need), non-linear equations, countably-many-points problems.

### 2. Stronger + faster symbolic engine (DDAR2)
DDAR = **Deductive Database + Arithmetic Reasoning** — computes the deduction closure (all deducible facts) by iterating a fixed rule set. Three improvements:
- **Double-point handling (Sec 3.1, Fig 1)** — the key capability for hard problems. To prove "intersection of AB, CD lies on circle Ω", the LM suggests an auxiliary point X = intersection of AB with Ω; DDAR proves X lies on CD, concludes X = P (both on AB and CD), hence P on Ω. This "reformulation via a double point" is what unlocks many IMO problems. (AG2 replaced Thales with the more general Central Angle Theorem while re-implementing.)
- **Faster algorithm (Sec 3.2, DDAR2).** AG1's rule loop is polynomial in #points for candidate search + **exponential in #clauses per premise** for clause matching (worst case O(N⁸) for similar-triangle search). AG2 **hard-codes search for the essential rules** (reduces AR queries to ≤cubic) and **discards explicit angle/distance rules** (perp/parallel etc.) — those deductions now happen automatically inside the AR engine. Two hot spots optimized: **similar triangles** — iterate all point triples, hash their "shape", flag a match when a shape recurs; **cyclic quadrilaterals** — iterate all (point P, segment AB) pairs, hash the value of angle(P,A,B), flag when a triple recurs. "Value" = a symbolic **normal form** from the AR submodule, which tracks known linear equations among angles/distances/log-distances and reduces any linear expression to normal form.
- **Faster implementation (Sec 3.3).** Core computation (Gaussian Elimination) reimplemented in **C++**, exported via pybind11 → **>300× faster than DDAR1**. Benchmark on 25 hard IMO problems: DDAR1 = 1179.57 ± 8.06 s vs DDAR2 = **3.45 ± 0.05 s**.

### 3. Better synthetic training data (Sec 4)
- Same core recipe as AG1: sample a **random diagram** (no human problems/diagrams — avoids contamination, unlike TongGeometry which uses human diagrams), run symbolic engine to deduce all facts, **traceback** each fact to its minimal premises + auxiliary points + deduction steps → training example. Auxiliary construction = the **dependency difference** (premise points the proof needs but the statement doesn't).
- Scaled up vs AG1: **2× larger random diagrams**, up to **2× more complex theorems** (points+premises), up to **10× longer proofs**, and crucially a **rebalanced distribution** — 50:50 with/without auxiliary points (AG1 was 9:91), and balanced across question types.
- New theorem types: generates **locus-type** problems ("when X moves on Y, Z moves on fixed T"), enabled by the `m(·)` movement-dependency tracking.
- **Faster minimal-problem traceback (Fig 3).** AG1 exhaustively removed point subsets and reran DDAR (exponential). AG2 uses a **greedy reverse-topological prune**: `prune_points` iterates points in reverse-topological order, drops p if `check_provable(pruned−{p})` still holds — linear number of checks, guaranteed minimal-by-inclusion when the check is monotonic (reverse-topological order fixes the construction-dependency non-monotonicity).
- Total training set ≈ **300M theorems** (order of magnitude over AG1's 100M).

### 4. Better language model (Sec 6)
- AG1 was a 151M custom transformer, 2-phase unsupervised training. AG2 uses a **sparse mixture-of-experts Transformer built on Gemini**, single-phase unsupervised on all data. Multiple sizes (51m, 176m, 3.3B) and three setups: (1) from scratch with custom AG tokenizer, (2) **fine-tune math-specialized Gemini** (Appendix B), (3) **multimodal** from scratch with diagram image input (Appendix C).
- **Ablation surprises (Appendix B):** tokenizer choice (custom word-level ~few-k vocab vs generic 300k Gemini) makes **no difference** to IMO solve rate; training in **natural language vs AG DSL** gives the **same** result (opens fine-tuning NL-pretrained LLMs). Multimodal/diagram input alone doesn't improve solve rate but **adds diversity** to auxiliary-point proposals, helping the SKEST ensemble.
- **Analysis string (Sec 6.2) — enriched neuro-symbolic interface.** Unlike AG1 (LM sees only `<problem_statement>`), AG2 feeds the LM what DDAR already deduced: `S1` = facts DDAR derives from premises; `S2` = facts derivable assuming the goal is also true; `S3` = facts numerically true in the diagram. Note `S1 ⊆ S2 ⊆ S3`. Input = `<problem_statement> serialized(S1) serialized(S2−S1) serialized(S3−S2)`.
- **Inference (Sec 6.2):** top-k sampling with **temperature τ = 1.0, k = 32** — essential. Greedy (τ=0,k=1,no tree) solves only 2/26 aux-requiring problems; τ=1.0,k=32 (no tree) solves 9/26. τ<1.0 → too little aux diversity; τ>1.0 → more wrong-syntax outputs.

### 5. SKEST — Shared Knowledge Ensemble of Search Trees (Sec 5, the novel search algorithm)
- AG1 used a single beam search. AG2 runs **several differently-configured beam searches in parallel**, each possibly a different LM, that help each other via a **shared facts database**.
- A **node** = one auxiliary-construction attempt + one symbolic-engine run. On success, all trees terminate. On failure, the node writes the facts DDAR proved into the **shared workspace**, but **filtered to facts relevant to the original problem** (not specific to that node's own aux point = "interesting" facts DDAR couldn't get without a particular aux point). These shared facts help other nodes in the same and other trees.
- Tree configurations run in the ensemble:
  - **Classic**: AG1-style, one aux point per node.
  - **Multi-aux**: LM emits as many aux points as it wants per node (model is trained to produce full proofs) — effectively deepens the search.
  - **Uniform-type aux**: prompt LM with `x00 a : cong`, `x00 a : coll`, `x00 a : cyclic`, `x00 a : perp`, … to force uniform distribution over the first aux-predicate token.
  - **Deep-narrow** (beam 64, depth 10) and **shallow-wide** (beam 512, depth 4).
- System: TPUv4 serving multiple replicas per model; LM workers and DDAR workers run **asynchronously** (LM writes explored nodes to a DB, DDAR workers pick them up and divide work); a single DDAR pool is shared across problems so solved problems release compute to unsolved ones.

## Results / benchmarks
- **IMO-AG-50** (all 45 IMO 2000–2024 geometry problems → 50 AG problems): AG2 full setting **42/50 (84%)**, surpassing avg gold medalist (40.9). AG2 single search tree (AG1 setup) = 38/50.
- **Comparison table (IMO-AG-50 / IMO-AG-30):** OpenAI o1 = 0/0; Gemini thinking = 0/0; AG1 DDAR = 14/14; **AG2 DDAR = 16/15**; TongGeometry DD = −/18; avg bronze = 27.1/19.3; Wu+AG1 DDAR = −/21; avg silver = 33.9/22.9; AG1 = 27/25; **avg gold = 40.9/25.9**; Wu+AG1 = −/27; TongGeometry w/o value = −/28; AG2 single tree = 38/28; TongGeometry full = −/30; **AG2 full = 42/30**.
- Symbolic engine alone (DDAR2, no LM) solves **16** IMO problems.
- **IMOSL-AG-30** (30 hardest formalizable IMO-shortlist problems, never used at IMO): AG2 full solves **20/30**.
- Fast learning: one LM + classic tree solves 27/50 after only **250 training steps** (batch 256, ~200M tokens). Larger models → lower perplexity on train/eval/imo_eval.
- Optimal single-tree inference: **beam size 128, beam depth 4, 32 samples**; more samples / larger beam don't help.
- Full-proof generation (Appendix F): with greedy, LM+DDAR solves only 2 IMO problems, but LM-only full proofs are mostly **valid steps** (few syntax errors); small and large models similar.
- Unsolved: 2 attempted-but-unsolved (IMO 2018 P6, 2023 P6 — need inversion/projective/radical-axis not in DDAR) + 6 unformalizable (inequalities, variable #points).
- Autoformalization (Appendix G): Gemini few-shot (5 queries + 1 combine) formalizes **33/44** IMO problems. Automated diagram generation (3-stage: point init → Adam on constraint/topological/non-degeneracy losses → Gauss-Newton-Levenberg) finds diagrams for **43/44**, sequentially within 1 hour.

## Novel vs SOTA-2026
Durable novel ideas: (1) **SKEST** — an ensemble of heterogeneous beam searches sharing *problem-relevant* proved facts across trees, with different LMs/configs contributing diversity; the cross-tree knowledge-sharing of intermediate facts is the transferable search insight. (2) **DDAR2** — reduced hard-coded rule set + hashing for similar-triangle/cyclic-quad detection + all angle/distance deductions folded into the AR normal-form engine + C++ Gaussian elimination = >300× speedup; this is the current strong symbolic-geometry core. (3) **Analysis string** enriching the neuro-symbolic interface by feeding DDAR's S1⊆S2⊆S3 deductions (incl. numeric-only facts) into the LM before it proposes aux points. (4) **Double-point reformulation** as a general proof tactic. Notably AG2 achieves gold **without any RL** (they hypothesize RL + subproblem decomposition as future work). The tokenizer/DSL-vs-NL ablations (both irrelevant to solve rate) are a useful negative result for anyone deciding whether to build a custom formal tokenizer.

## Adopt-relevance to Theoremata — specific and actionable

- **`geometry_ddar.py` (DD+AR engine) — direct, highest fit.** AG2 gives the concrete recipe to make our DD+AR fast and stronger: (a) **fold all angle/parallel/perpendicular/distance rules into the AR normal-form engine** rather than keeping explicit deduction rules — reduces query complexity to ≤cubic; (b) **hash-based detection** for the two hot spots — hash triangle "shape" for similar triangles, hash `angle(P,A,B)` value for cyclic quads — instead of O(N⁸) combination search; (c) implement Gaussian elimination in a compiled core (our Rust core is the natural home, vs their C++/pybind11) for the >300× win. **Gap check:** if our AR is already a unified coefficient matrix (per the AG1 mining note), the new wins here are the *hashing detection* and *rule-set reduction*, not the matrix itself.
- **Double-point handling — real capability gap.** Our DDAR likely cannot accept two differently-named points with identical coordinates. Adding `overlap`/`cyclic_with_center` and the 4-step reformulation (construct aux X, prove X on the other object, conclude X=P) unlocks a class of hard problems — a concrete `geometry_ddar.py` feature to add.
- **`geometry_synth.py` (traceback synthetic data) — direct.** Adopt two things: (1) the **greedy reverse-topological `prune_points`** (Fig 3) to replace any exponential minimal-premise search — linear checks, monotonic-safe; this also sharpens our proof-DAG minimality. (2) **Rebalance** synthetic data to ~50:50 with/without auxiliary points and across question types (AG1's 9:91 aux imbalance hurt) — a cheap data-distribution fix for whatever we generate. (3) Scale diagram size / proof length deliberately (2× / 10×) to mine harder problems.
- **Locus/movement + linear-equation predicates — language coverage.** If our geometry language mirrors AG1's 9 predicates, adding `acompute/rcompute`, `distmeq/distseq/angeq`, the 11 locus cases with the `*` placeholder, and `m(·)` movement tracking during generation lifts coverage the same 66%→88% and lets us state "find the angle/ratio" and moving-object problems.
- **MCGS driver — SKEST is a portable pattern.** Our MCTS graph-search can adopt SKEST's **cross-tree shared workspace of *problem-relevant* proved facts** and **heterogeneous tree configs** (deep-narrow vs shallow-wide, multi-aux vs single, uniform-type prompting) run against a shared verifier-worker pool with async LM/verifier workers. The "share only facts relevant to the original problem, not node-specific artifacts" filter is the key design detail. Distinct from AlphaMath's single-tree value-guided MCTS — SKEST is *ensemble+knowledge-sharing*, complementary.
- **Analysis string — enrich our neuro-symbolic prompt.** Before the model proposes a sketch/aux step, feed it the gate's current deduction sets: `S1` (facts provable now), `S2−S1` (facts provable if the goal is assumed), `S3−S2` (numerically-true-in-model-but-unproven facts). This is a cheap, high-value change to how we prompt the generator — gives it the symbolic engine's state. Directly relevant to our sketch pipeline (the "holes" the model must fill are AG2's aux points, and S3−S2 tells it what's numerically true but not yet proven).
- **Autoformalization + diagram generation (Appendix G).** Our NL→formal front-end can copy: few-shot LM formalization with **N-sample + combine** (5 queries + 1 merge → 33/44), and the **3-stage diagram solver** (random/ordered/heuristic init → Adam on exact-constraint² + softplus topological + non-degeneracy losses → Gauss-Newton-Levenberg refine). This is our "sound geometry vertical needs a real diagram/model" answer for non-constructive statements.
- **Negative results worth banking.** (1) Custom tokenizer and DSL-vs-NL don't matter for solve rate — do **not** invest in a bespoke formal tokenizer; fine-tuning a math-pretrained model in natural language is fine. (2) Multimodal diagram input alone doesn't raise solve rate (only adds ensemble diversity) — deprioritize vision for our geometry vertical; "the core of geometry solving is algebraic reasoning, not visual." (3) LLM-as-verifier is explicitly rejected in favor of a symbolic engine for consistent feedback — validates our **verification-first / formal-gate** stance over LLM-judge-only.

## Verbatim-worthy details (predicates, rules, algorithms, hyperparameters)

- **New linear-equation predicates:** `distmeq a1 b1..an bn t1..tn y` = `Σ tᵢ·log(aᵢbᵢ) + y = 0`; `distseq ...` = `Σ tᵢ·(aᵢbᵢ) = 0`; `angeq ... y` = `Σ tᵢ·φ(aᵢbᵢ) + y = 0`, φ = angle of undirected line vs horizontal.
- **Locus syntax (Table 2), `*` = fixed-point placeholder:** circumcircle→`cyclic a b c * : X`; line-through→`coll a b * : X`; bline→`cong a * b * : X`; pline→`para b c a * : X`; tline→`perp b c a * : X`; point-on-line→`coll a * * : X`; point-on-circle→`cyclic a * * * : X`; fixed distance→`cong a b * * : X`; fixed direction→`para a b * * : X`; fixed angle→`eqangle a b a c * * * * : X`.
- **`m(·)` movement source (Table 3):** `a=midpoint b c, d=midpoint a c ⇒ m(d)={b,c}`; `a=on_line b c ⇒ m(a)={a,b,c}`.
- **Greedy minimal-premise prune (Fig 3):**
  ```python
  def prune_points(points, check_provable):
      pruned = set(points)
      for p in reverse_topological(points):
          if check_provable(pruned - {p}):
              pruned = pruned - {p}
      return pruned
  ```
- **DDAR2 detection:** similar triangles — hash "shape" over all point triples, match on recurrence; cyclic quads — hash `angle(P, A, B)` value over all (point, segment) pairs, match on recurrence; "value" = AR-submodule symbolic normal form.
- **DDAR2 speed benchmark (25 hard IMO, AMD EPYC 7B13 64-core, 50 runs):** DDAR1 1179.57±8.06 s; DDAR2 3.45±0.05 s (>300×).
- **Analysis-string input:** `<problem_statement> serialized(S1) serialized(S2−S1) serialized(S3−S2)`, with `S1 ⊆ S2 ⊆ S3` (provable / provable-assuming-goal / numerically-true).
- **Inference:** top-k, τ=1.0, k=32. Single-tree optimum: beam 128, depth 4, 32 samples. SKEST configs: classic; multi-aux; uniform-type (`x00 a : cong|coll|cyclic|perp|…`); deep-narrow (beam 64, depth 10); shallow-wide (beam 512, depth 4).
- **AG token classes (Appendix B):** point names a..z, a1..z1; predicate names (17 listed); numbers/fractions 1..9, -, /; predicate reference tokens (000)..(999); reserved {Analysis},{Numerical},{FromGoal},{Proof}, x00, :, ;, ..
- **NL↔DSL example:** DSL `d e f g : coll a d g (000) coll f a b (001) ... cong d b d c (004)` ≡ NL "Construct points d e f g such that a d g are collinear (000), …, |d b|=|d c| (004)".
- **Diagram-generation loss (Appendix G):** minimize Σ f_e(x̄)² (exact constraints) + Σ softplus(f(x̄)) (topological <0) + Σ softplus(min(f,−f)) (topological =0) + non-degeneracy terms (1/‖·‖² and 1/(|PᵢPⱼ|²+ε)); Adam over 10 inits → filter by loss threshold + topological satisfaction → Gauss-Newton-Levenberg final solve. 43/44 diagrams within 1 hour.
- **Scale:** ~300M training theorems; models 51m/176m/3.3B params; sparse-MoE on Gemini; TPUv4; linear-warmup+cosine LR from scaling laws.
- **Inequality rules (Appendix H)** — NOT in AG2, but a ready spec if we want an inequality-geometry extension: defines σ (clockwise sign), τ (betweenness sign), oriented angle measures α0/α1/α2, and ~60 deduction rules relating them (convexity, in-polygon, acute/obtuse, circle membership). Useful reference for a future `geometry` inequality module.
