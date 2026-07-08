# Paper Mining — LEGO-Prover: Neural Theorem Proving with Growing Libraries

- **Source:** Wang, Xin, Zheng, Liu, Cao, Huang, Xiong, Shi, Xie, Yin, Li, Liang. *LEGO-Prover: Neural Theorem Proving with Growing Libraries.* ICLR 2024.
- **Code:** https://github.com/wiio12/LEGO-Prover
- **PDF:** `math-papers/LEGO-Prover - Neural Theorem Proving with Growing Libraries.pdf` (fully read, incl. appendix algorithms & prompts)
- **System target:** Isabelle/HOL (via PISA REPL wrapper), Sledgehammer for auto-close.

---

## Core contribution

LEGO-Prover attacks a structural weakness of prior LLM provers: they assume a **fixed** theorem library and re-prove everything from scratch per problem. Instead it maintains a **growing skill library of verified lemmas** that a *prover* retrieves and reuses to build proofs modularly (block-by-block, LEGO-style), while an *evolver* continuously generalizes those lemmas and solves outstanding sub-goal requests to enrich the library on a second axis. This raises miniF2F SOTA from 48.0%→57.0% (valid) and 45.5%→50.0% (test), and in the process the system accumulates 20,000+ verified reusable lemmas, with an ablation showing the library itself is worth +4.9%.

---

## Key techniques / architecture

Two components share one growing library and run **in parallel** (Algorithm 1): `n_prover` provers and `n_evolver` evolvers as separate processes, ratio **3:8** (prover:evolver). Each problem is replicated `n_attempts=100` times into a multiprocessing queue.

### The growing skill library (THE key mechanism)

The library is **three ChromaDB vector stores**, each holding `(document, embedding)` pairs, embedded with OpenAI `text-*-ada` embeddings, retrieved by **k-NN**:

1. **Lemma vector store** — the core. Holds **Isabelle-verified lemmas** = *statement + proof* together. This is what grows and what makes the LLM progressively stronger. "Lemma" and "skill" are used interchangeably.
2. **Request vector store** — holds **lemma statements proposed by the decomposer** (conjectured sub-goals, no proof yet). Serves double duty: (a) a "deeply-reasoned query" for retrieving useful skills for the prover, and (b) a worklist of targets for the evolver's request-solver. Failed prover lemmas' statements are also dumped here.
3. **Problem vector store** — the 488 miniF2F formal statements. Used as *heuristics* to steer the evolver toward generating lemmas that will actually help pending problems.

**What a stored skill is:** a verified Isabelle lemma (`lemma name: fixes … assumes … shows … <proof>`), embedded by its text, retrievable by k-NN over statement/request queries.

**Dedup:** before adding a new (verified) skill, compare it against existing skills using Python `difflib.SequenceMatcher`; **only add if verified AND similarity ratio < 0.85** (below threshold). This bounds redundancy.

**Provenance / update-count:** each lemma and each request carries an `update_count` (init 0). The evolver always picks the **least-evolved** lemma / **least-solved** request (lowest update_count) and increments it — a fairness/round-robin scheduler so the whole library keeps evolving rather than over-mining a few popular skills.

### Prover (Algorithm 2) — three LLM steps

1. **Informal solver** (optional): if no human proof is used, an LLM drafts a natural-language solution from the informal statement. (They pre-generate up to 20 model informal proofs per problem with GPT-4, avg 12.13; one is sampled per attempt.)
2. **Decomposer:** rewrites the informal proof into a **step-by-step structural informal proof aligned to Isabelle structure** AND emits **"Required skills" = requests**: each request is `Thought` (CoT on what lemma is needed) + `Code` (a formal `lemma … shows …` statement). Requests are pushed to the request store; they are also used as retrieval queries against the lemma store.
3. **Formalizer:** given the problem's formal statement + retrieved skills (query = requests + formal statement, retrieve **`n_f` skills**; n_f=6 valid, 4 test), the LLM writes the **entire Isabelle source file** — imports, helper lemmas, then the main theorem — in a modular block-by-block form. This is the key difference from DSP/Subgoal-Learning: the LLM may define/apply its own lemmas before the main goal.

**Verification & harvest:** PISA/Isabelle verifies. `by …` gaps are auto-closed with **Sledgehammer + 11 heuristic tactics** (same heuristic set as DSP). A proof counts only if it (a) has no `sorry`/`oops` and (b) Isabelle verifies it. Every verified lemma/theorem → lemma store (update_count=0). Every failed lemma's *statement* → request store.

### Evolver (Algorithm 3) — two LLM sub-tasks, runs continuously

Each iteration randomly picks one of two modes:

- **Directional transformer** (51.1% of all library skills): picks the least-evolved lemma, uses it to retrieve `n_d` relevant pending **problem statements + requests** (n_d=4), then randomly picks **one of four evolution directions** and prompts the LLM to produce a new, more general/reusable lemma. The four directions (Table 4):
  - **Identify key concepts** — extract the essential idea/method/theorem crucial to the source problem.
  - **Parameterize** — replace specific numbers with variables to generalize.
  - **Scale complexity** — try simpler and harder variants to see how the approach adapts.
  - **Extend dimensions** — if defined in N dimensions, consider more/fewer dimensions.
- **Request solver** (38.2% of library skills): picks the least-solved request (a conjectured sub-goal from some prover), retrieves 3 lemmas as in-context demos, and prompts the LLM to prove it. Only verified proofs are accepted; semantically-wrong conjectures are only weeded out by repeated failed attempts (undecidable in general — no purely symbolic rejection).

Prover-origin lemmas are only **10.8%** of the final library — i.e., ~89% of the growing library is manufactured by the evolver. New verified evolver skills go back into the lemma store (subject to the 0.85 dedup gate).

### Skill-evolving forest & the request-for-skill loop

Prover/request-solver lemmas are **root nodes**; the directional transformer generalizes them into **child nodes**, producing a "forest" of evolving trees (Fig 3c: an imo_1988_p6 helper gets parameterized, then key-concept-identified, spawning `division_remainder`, `div_mult_le`, etc., and eventually an old skill receives a *new, better proof*). The **request-for-skill mechanism** is the closed loop: prover's decomposer conjectures a needed lemma → request store → evolver's request-solver proves it → lemma store → prover retrieves it on the next round.

---

## Results / benchmarks (miniF2F, Isabelle, 100 attempts, GPT-3.5/ChatGPT)

| Method | LLM | valid | test |
|---|---|---|---|
| Thor | Codex | 28.3% | 29.9% |
| Thor + expert iteration | Codex | 37.3% | 35.2% |
| Draft, Sketch, Prove (DSP) | Codex | 42.6% | 39.3% |
| Subgoal-Learning | ChatGPT | 48.0% | 45.5% |
| **LEGO-Prover (model informal proof)** | ChatGPT | **52.0%** | 45.5% |
| **LEGO-Prover (human informal proof)** | ChatGPT | **55.3%** | **50.0%** |
| **LEGO-Prover\* (cumulative)** | ChatGPT | **57.0%** | **50.0%** |
| Ablation: − skill library (human proof) | ChatGPT | 50.4% (−4.9%) | — |

- 257/488 problems solved (human proof). +7.3%/+4.5% over Subgoal-Learning (valid/test); +6.75% avg over prior SOTA.
- **Library ablation:** removing library+evolver drops valid 55.3%→50.4% (−4.9% @100 attempts; gap grows: 3.3%@50 → 4.9%@100 as library matures). Early attempts show no benefit (library empty) — the payoff compounds.
- **Balanced-compute ablation:** prover:evolver token cost ≈ 1:0.89 (131M vs 117M tokens/experiment). Giving the no-library prover the extra budget (189 attempts) still loses: 53.2% vs 55.3% (−2.1%). So the win is not just "more compute."
- **Library stats:** 22,532 skills total — 10.8% prover, 38.2% request-solver, 51.1% directional-transformer. Evolver proof success rate only 24.1%; after dedup filtering only **9.1%** of evolver outputs actually enter the library.
- Also beats **Lyra** (GPT-4 auto-correction DSP): LEGO-Prover with ChatGPT beats DSP-GPT-4 (+4.1/+7.0) and Lyra-GPT-4 (+3.3/+2.9), comparable to Lyra@200 attempts.
- **Hyperparameters:** T=0.7 everywhere; decomposer 3-shot; formalizer n_f=6(valid)/4(test) + 2 in-context examples; directional transformer n_d=4 + 2 examples; request solver retrieves 3 skills. ChatGPT variants (gpt-3.5-turbo family) sampled randomly per call. ~$600/experiment.

---

## Novel vs SOTA-2026

- **Still novel & directly relevant:** the *growing verified-lemma library as a first-class agentic memory*, with a two-axis producer (prover harvests + evolver generalizes), request/lemma/problem tri-store retrieval, and difflib dedup. This is exactly the "LEMMA-CACHE / growing-library" seam Theoremata has stubbed. The **request store as a reasoned retrieval query** (query by the conjectured sub-goal, not the raw problem) is a subtle, portable idea.
- **Dated by 2026:** the base model (GPT-3.5) and absolute pass rates are far below modern provers (DeepSeek-Prover-V2, whole-proof RL provers, Lean-based systems now clear miniF2F-test well above 80–90%). Retrieval is plain OpenAI-ada + k-NN — no reranker cascade, no BM25, no dense+sparse fusion (Theoremata already plans stronger retrieval). Dedup by `difflib` string-ratio is crude vs semantic/embedding dedup. Sledgehammer-only hammering; single system (Isabelle), no portfolio. The evolver's 9.1% yield is very inefficient. Expert-iteration/fine-tuning is absent — the library is the *only* learning signal (no weight updates), which is both its charm and its ceiling.

---

## Adopt-relevance to Theoremata (specific & actionable)

Goal: turn the `EpisodicMemory` facade's Python-side lemma-cache stub into a real growing library. Map LEGO-Prover onto our proof-DAG + portfolio + sketch→autoformalize-holes→splice pipeline.

**What we already do (keep):** verification-first (their "only verified lemmas enter the library" is our 3+1 gate); sketch→holes pipeline (their decomposer→requests→formalizer is our sketch→hole-request→splice); hammers (their Sledgehammer-only maps onto our Sledgehammer/CoqHammer/aesop portfolio — we are strictly ahead here); portfolio across Lean/Rocq/Isabelle (they are single-system).

**The real gap = the growing library + evolver.** Concretely:

1. **Store schema (per verified lemma / skill):** `{ statement (formal), proof (verified proof term/script), system (Lean|Rocq|Isabelle), embedding, provenance (origin ∈ {prover, request_solver, directional:<dir>}, parent_skill_id, source_problem_id, evolve_direction), update_count, verified_by_gate, created_at }`. Store statement+proof together (LEGO's core lesson) so retrieval returns something *directly spliceable*, not just a name.

2. **Three retrieval indices, mirror LEGO:** (a) **lemma store** (verified, statement+proof); (b) **request store** = our *open sketch holes / sub-goal conjectures* (statement only, no proof) — doubles as a retrieval query and an evolver worklist; (c) **problem store** = our target theorem statements, to bias evolution toward pending goals. We already have a proof-DAG; make failed/open DAG nodes populate the request store automatically.

3. **Retrieve during sketch-hole proving:** when the sketch pipeline emits a hole, embed the *hole statement* (LEGO's "request" — a reasoned CoT sub-goal, not the raw problem) and k-NN the lemma store for `n_f` skills (start n_f≈4–6). Feed retrieved verified lemmas into the hole-filling LLM as in-context context AND as importable premises the splicer can `using`/`apply`. Near drop-in for our premise-retrieval cascade — add the *self-grown lemma store* as one more retrieval corpus alongside BM25/dense/reranker.

4. **Abstract/generalize a proven lemma (the evolver):** after any hole/sub-lemma verifies, enqueue it for a background "evolver" worker that applies one of the four directions (parameterize / identify-key-concept / scale-complexity / extend-dimensions) via prompt, re-verifies through the 3+1 gate, and (if it passes dedup) adds the generalized child. Use the least-`update_count` scheduler so evolution spreads. This is our **expert-iteration flywheel realized without weight updates** — a cheap first cut before RL.

5. **Request-for-skill loop:** a hole the prover cannot close becomes a *request* in the request store; a background **request-solver** worker (with retrieved lemmas as demos) tries to prove it; success feeds back into the lemma store for the next MCTS/portfolio pass. Closes our sketch→autoformalize-holes loop with a shared, persistent memory instead of per-problem scratch.

6. **Dedup:** minimally, port LEGO's `difflib.SequenceMatcher` ratio < 0.85 gate. Better (2026): dedup on embedding cosine similarity + statement-normalization; drop skills that are alpha-equivalent to Isabelle/Mathlib built-ins.

7. **Concurrency model:** run provers and evolvers as parallel workers over a shared store (LEGO's 3:8 ratio, mp.Queue). Fits our Rust core orchestrating Python tools — the store is the sync point.

**Flag:** LEGO's evolver yield is low (9.1% admitted, 24.1% proof success) and costs ~half the compute. Budget accordingly: gate evolver spend, and lean on our multi-system hammers (which they lack) to raise the admit rate. Their retrieval (ada k-NN) is weaker than our planned cascade — so our library retrieval should be *better* than the paper out of the box.

---

## Verbatim-worthy details

**Library data:** three ChromaDB vector stores, `(document, embedding)` pairs, `text-*-ada` embeddings, k-NN retrieval. Lemma store = statement + proof; request store = decomposer sub-goal statements + failed-prover statements; problem store = miniF2F formal statements.

**Dedup rule:** "Only skills that are verified and show a difference below the threshold of 0.85 [via `difflib.SequenceMatcher`] are added to the library."

**Validity:** proof valid iff (a) no `sorry`/`oops` cheating keywords AND (b) Isabelle (via PISA) verifies the code. Auto-close gaps with Sledgehammer + 11 heuristic tactics.

**Four evolution directions (Table 4):**
- *Identify key concepts:* "Determine the essential ideas, methods, or theorems that are crucial to solving the initial problem."
- *Parameterize:* "If the problem involves specific numbers, generalize it by replacing these with variables."
- *Scale complexity:* "Try both simpler and more complicated versions of the problem to see how the approach adapts."
- *Extend dimensions:* "If the problem is defined in a specific number of dimensions, consider if it holds in more or fewer dimensions."

**Decomposer output format:** `Structure proof: Step 1… Step N` then `Required skills:` a list of `Thoughts k: {CoT}` + `Code k: {lemma statement}` pairs (each = one request).

**Formalizer system prompt (key lines):** "You are strongly encouraged to create useful and reusable lemmas to solve the problem. The lemmas should be as general as possible (generalizable), and be able to cover a large step in proofs (non-trivial)." Emits full `theory Scratch imports Complex_Main begin … end` file with helper lemmas before the main theorem; gaps use `sledgehammer`.

**Algorithm 1 (main):** init 3 ChromaDB stores; replicate each problem `n_attempts` into `mp.Queue`; launch `n_prover` provers + `n_evolver` evolvers (3:8).

**Algorithm 2 (prover):** `infStmt,infProof,formStmt ← queue.pop()`; `if model proof: infProof ← InformalSolver(infStmt)`; `strucInfProof, lemmaRequests ← Decomposer(...)`; `RequestS.adds(lemmaRequests, update_count=0)`; `retrievedLemmas ← LemmaS.retrieveKNN(lemmaRequests)`; `proofCode ← Formalizer(infStmt, strucInfProof, formStmt, retrievedLemmas)`; `proofResult, correctLemmas, newRequests ← IsabelleEnv.verify(proofCode)`; `RequestS.adds(newRequests, 0)`; `LemmaS.adds(correctLemmas, 0)`.

**Algorithm 3 (evolver):** loop: `transType ← random.choice([DirectionalTransformer, RequestSolver])`. If DT: `dir ← random.choice(['Identify key concepts','Parameterize','Scale complexity','Extend dimensions'])`; pick lemma with lowest update_count, increment it; query request+problem stores for relevant items; `newLemma ← DirectionalTransformer(lemma, dir, retrieved)`. If RS: pick least-updated request, increment; retrieve relevant lemmas as demos; `newLemma ← RequestSolver(request, demos)`. Then Isabelle verifies; if correct (and passes dedup) → LemmaS.adds.

**Hyperparameters:** T=0.7; decomposer 3-shot; formalizer n_f=6(valid)/4(test) + 2 examples; directional transformer n_d=4 + 2 examples; request solver 3 retrieved skills; 100 attempts/problem; prover:evolver process ratio 3:8; token cost prover:evolver ≈ 1:0.89 (131M:117M tokens); ~$600/experiment.

**Library composition:** 22,532 skills — 10.8% prover / 38.2% request-solver / 51.1% directional-transformer; evolver proof success 24.1%; only 9.1% of evolver outputs admitted after dedup.
