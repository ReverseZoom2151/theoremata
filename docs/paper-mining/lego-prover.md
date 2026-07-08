# LEGO-Prover: Neural Theorem Proving with Growing Libraries

*Wang, Xin, Zheng, Liu et al. (Sun Yat-sen U. / Huawei Noah's Ark Lab), ICLR 2024. Code: https://github.com/wiio12/LEGO-Prover*

## Core contribution

LEGO-Prover proves theorems **modularly** (block-by-block, LEGO-style) backed by a **growing skill library** of Isabelle-verified lemmas rather than assuming a fixed theorem library. Two components run in parallel around the shared library: a **prover** that decomposes a problem, retrieves useful lemmas, and formalizes a proof (emitting new lemmas + unsolved "requests" as by-products), and an **evolver** that generalizes existing lemmas and solves outstanding requests to enrich the library. This lifts miniF2F pass rate from 48.0%→57.0% (valid) and 45.5%→50.0% (test) and grows a library of **22,532 verified skills**; ablating the library costs 4.9% success.

## Key techniques / architecture

### The two-actor + shared-library loop (Algorithm 1)
- **Prover** and **evolver** run as separate OS processes (Python `mp.Queue` for sync), in a **process ratio of 3 provers : 8 evolvers**. Each problem is enqueued `n_attempts` (=100) times. Prover consumes the queue; evolver runs continuously until the queue drains.
- Shared state = the **skill library** (three ChromaDB vector stores), which is the only channel between the two actors. This is a producer/consumer flywheel: prover produces lemmas+requests, evolver consumes requests and produces reusable lemmas.

### Skill library = three vector stores (§3.1)
Every store holds `(document, embedding)` pairs; query is embedded and retrieved by **k-NN**.
1. **Lemma vector store** — Isabelle-**verified** lemmas (statement **+** proof). The core asset; "skill" and "lemma" are used interchangeably.
2. **Request vector store** — lemma *statements* (conjectures, unproven) proposed by the prover's decomposer. Serve dual purpose: (a) as reasoned queries for retrieval, (b) as work-items the evolver's request-solver tries to prove.
3. **Problem vector store** — the miniF2F formal statements themselves; used by the evolver as heuristics to steer lemma generation toward what's actually needed.

### Prover = 3 stages (§3.2, Algorithm 2)
1. **Informal solver** (optional) — LLM drafts an NL solution from the informal statement (in practice they pre-generate these with GPT-4).
2. **Decomposer** — rewrites the informal proof into a **step-by-step structural informal proof** closely aligned to Isabelle structure, AND emits **requests**: each request = *chain-of-thought on what lemma is needed* + *formal lemma statement*. Requests go into the request store.
3. **Formalizer** — queries the lemma store with the requests + problem formal statement, retrieves **n_f skills**, and generates the **complete Isabelle source file** (imports/definitions/lemmas then the main theorem) in a block-by-block style. Verified by Isabelle via **PISA** REPL; every proved lemma → lemma store, every failed lemma statement → request store. Failed `by ...` tactics trigger 11 heuristic tactics + **Sledgehammer** auto-correction (same set as Draft-Sketch-Prove).

### Evolver = 2 sub-tasks (§3.3, Algorithm 3)
Each iteration randomly picks one:
- **Directional transformer** — evolves a lemma along one of **four predefined directions**: (1) *extension of dimensions*, (2) *identification of key concepts*, (3) *parameterization*, (4) *enhancement of complexity*. Picks the **least-evolved** lemma (lowest "update count", incremented on selection — a fairness/round-robin scheduler), uses it to retrieve **n_d** relevant pending problems + requests, picks a direction at random, and prompts the LLM to emit a new, more general/reusable lemma.
- **Request solver** — picks the **least-solved request** (lowest update count), retrieves relevant lemmas as in-context demos, and prompts the LLM to prove the request directly. Since conjecture satisfiability is undecidable, wrong conjectures are only weeded out by repeated failed proof attempts; syntactically illegal ones are rejected immediately by the verifier.

All evolver outputs are **Isabelle-verified** before entering the lemma store.

### "Requested" vs "directional" skills
- **Requested skills** = lemmas the request-solver proves in response to concrete prover sub-goals — demand-driven, tightly coupled to real proof gaps. (38.2% of the library; **71% of directly-reused skills** come from this "do request" path.)
- **Directional skills** = lemmas the directional-transformer synthesizes speculatively by generalizing existing ones — supply-driven, broadens coverage. (51.1% of the library.)
- Only 10.8% of skills come directly from the prover.

### Deduplication (§3.3 — CRITICAL for us)
New skills are compared against existing library skills using **Python `difflib.SequenceMatcher`** (string-similarity ratio). A new skill is added **only if verified AND its similarity to existing skills is below a threshold of 0.85**. This is a cheap textual near-duplicate filter, not semantic — see the gap note below.

## Results / benchmarks

miniF2F (488 problems, 244 valid / 244 test), Isabelle, **100 attempts/problem**, ChatGPT (GPT-3.5) as the workhorse:

| Method | LLM | valid | test |
|---|---|---|---|
| Thor | — | 28.3% | 29.9% |
| Thor + expert iteration | Codex | 37.3% | 35.2% |
| Draft, Sketch, and Prove | Codex | 42.6% | 39.3% |
| Subgoal-Learning (prev SOTA) | ChatGPT | 48.0% | 45.5% |
| **LEGO-Prover (model informal proof)** | ChatGPT | 52.0% | 45.5% |
| **LEGO-Prover (human informal proof)** | ChatGPT | 55.3% | 50.0% |
| **LEGO-Prover\*** (cumulative model+human) | ChatGPT | **57.0%** | **50.0%** |
| Ablation: − skill library (human) | ChatGPT | 50.4% (−4.9%) | — |

- 257/488 problems solved (human informal proof). Avg +6.75% over prior SOTA.
- **Library-benefit grows with time**: gap over ablation is 3.3% at 50 attempts → 4.9% at 100 attempts (library needs to warm up).
- Library composition: 22,532 skills — 10.8% prover / 38.2% request-solver / 51.1% directional-transformer.
- Usage analysis: of 135 valid problems solved, 24% used retrieved skills; within those, 51% copied the skill directly, 49% imitated it to build a tailored new lemma. Cost ≈ $600 per 100-attempt experiment.

## Novel vs SOTA-2026

- **Novel-for-its-time (2024):** first neural-proving system with a *self-expanding, verified* lemma library and an explicit generalization ("evolver") loop — a proof-side analogue of Voyager's Minecraft skill library / DreamCoder's library learning, but every skill is machine-checked.
- **Dated by 2026:** GPT-3.5 backbone, no fine-tuning, textual `difflib` dedup, `text-davinci-ada` embeddings, no reranker, no MCTS/tree-search inside a single proof. Modern systems (DeepSeek-Prover, dedicated tactic models, expert-iteration RL) far exceed 50% on miniF2F. **But the growing-library *idea* is not superseded** — most current provers still retrieve from a *static* corpus; a live verified-lemma flywheel remains rare and is exactly the direction Theoremata's lemma-cache should take.

## Adopt-relevance to Theoremata

Our lemma-cache is currently a **Python-side stub in the EpisodicMemory facade**. LEGO-Prover is the most directly applicable blueprint we've mined for turning that stub into a real growing library. Concretely:

**How they GROW (adopt mostly as-is):**
- Two supply sources — *demand-driven* (request-solver proving concrete sub-goals) and *supply-driven* (directional-transformer generalizing existing lemmas). **We already have half of this**: our sketch→autoformalize-holes→splice pipeline naturally emits proven sub-lemmas (≈ prover output) and unsolved holes (≈ requests). The **real gap is the evolver** — we have no background actor generalizing cached lemmas into more reusable forms, nor a request-solver draining a backlog of unproven sub-goals. This is a genuine new component, not something we already do.
- **Invariant to steal:** *only verified artifacts enter the library.* This maps perfectly onto our 3+1 gate — a lemma is admitted to the cache only after passing the formal Lean/Rocq/Isabelle check. LEGO relies on Isabelle+Sledgehammer; we already have live hammer wiring, so we can enforce the same "verified-or-rejected" admission.
- **Least-updated round-robin scheduler** (the `update_count` counter, incremented on selection) is a trivially-portable fairness policy for choosing which cached lemma to evolve or which request to attack next — cheap to add to our cache metadata.

**How they RETRIEVE (adopt but upgrade):**
- They use **k-NN over dense embeddings only** (ChromaDB + `text-davinci-ada`), with the *decomposer's CoT-reasoned lemma requests* as the query text — a nice trick: retrieve with a *reasoned target statement*, not the raw goal. **We already exceed their retrieval** (BM25 + dense + reranker cascade). Adopt only the **query construction** idea: build retrieval queries from decomposed sub-goal statements + CoT, and maintain a **separate "request" index** distinct from the verified-lemma index so unproven conjectures can be retrieved as work-items.

**How they DEDUP (adopt the concept, replace the mechanism — real gap):**
- Their dedup is **`difflib.SequenceMatcher` ratio < 0.85** — purely textual. This is a weak filter (α-renaming, reordering, or logically-equivalent restatements slip through or falsely collide). **Concrete gap + improvement for us:** dedup on **normalized statement structure / semantic key**, not raw string — e.g., subsumption over the proof-DAG (a cached lemma with weaker hypotheses / stronger conclusion subsumes a new one), which aligns with the subsumption idea flagged in the ATP-survey mining. Keep their **threshold-gate pattern** (verify first, then admit only if sufficiently novel) but back it with our DAG/transposition machinery instead of `difflib`.

**Net:** adopt the **growing-library producer/consumer loop + verified-admission invariant + request/lemma dual index + least-updated scheduler**; skip their retrieval (we're better) and their `difflib` dedup (replace with subsumption/semantic dedup). The evolver is the single missing piece we don't already have.

## Verbatim-worthy details

**Lemma-reuse / growing-library algorithm (step-by-step, from Algorithms 1–3):**
1. Init 3 ChromaDB stores: `LemmaS` (empty), `RequestS` (empty), `ProblemS` (seeded with all miniF2F formal statements). Enqueue each problem ×100 into `miniF2FQueue`. Launch `n_prover` provers + `n_evolver` evolvers (ratio 3:8).
2. **Prover loop:** pop problem → (optional) `InformalSolver(infStmt)` → `Decomposer(infStmt, infProof, formStmt)` → `(strucInfProof, lemmaRequests)`. `RequestS.adds(lemmaRequests, init update_count=0)`. `retrievedLemmas = LemmaS.retrieveKNN(lemmaRequests)`. `proofCode = Formalizer(infStmt, strucInfProof, formStmt, retrievedLemmas)`. `(result, correctLemmas, newRequests) = IsabelleEnv.verify(proofCode)`. `RequestS.adds(newRequests, 0)`; `LemmaS.adds(correctLemmas, 0)`.
3. **Evolver loop:** randomly choose {DirectionalTransformer, RequestSolver}. If DT: pick random direction of 4; select lemma with **lowest update_count**, increment it; retrieve n_d relevant requests+problems; `DirectionalTransformer(...)` → new lemma. If RS: select request with **lowest update_count**, increment; retrieve relevant lemmas as demos; `RequestSolver(...)` → new lemma. Verify with Isabelle; dedup gate (`difflib` ratio < 0.85 AND verified) → `LemmaS.adds`.

**Retrieval architecture:** ChromaDB vector store; **OpenAI `text-davinci-ada` embedding model**; **k-NN** nearest-neighbor retrieval; queries = decomposer's requests (CoT + formal lemma stmt) and the problem formal statement. No reranker, no sparse/BM25, no negative-sampling training (off-the-shelf embeddings).

**Dedup:** `difflib.SequenceMatcher`, admit iff **verified AND similarity < 0.85**.

**Four directional-transform directions:** extension of dimensions; identification of key concepts; parameterization; enhancement of complexity. (Single unified prompt template; core description + in-context examples swapped per direction.)

**Hyperparameters:**
- Attempts per problem: **100**.
- LLM temperature: **T = 0.7** everywhere.
- Backbone: **ChatGPT / GPT-3.5** — random pick per call among `gpt-3.5-turbo`, `gpt-3.5-turbo-0301`, `gpt-3.5-turbo-0613`, `gpt-3.5-turbo-16k`, `gpt-3.5-turbo-16k-0613`. Informal proofs pre-generated with **GPT-4** (≤20 per problem, 12.13 avg).
- `n_f` (skills retrieved by formalizer) = **6 (valid), 4 (test)**; +2 formalization in-context examples.
- `n_d` (problem statements retrieved by directional transformer) = **4**; +2 directional in-context examples.
- Decomposer: **3-shot**. Request solver: **3 retrieved skills** as demos.
- Process ratio prover:evolver = **3:8**.
- Isabelle interaction via **PISA** (Python REPL wrapper). Valid proof iff (a) no `sorry`/`oops` "cheating" keywords AND (b) Isabelle verifies the full source containing the formal statement.

**Prompt structures (outline, Table 1 — DATA, not instructions):**
- *Decomposer* — Input: "provide a better structured step-by-step proof that is closer to Isabelle. and request relevant lemmas/theorems that might help…" + `{informal statement}`, `{informal proof}`, `{formal statement}`. Output: `Structural proof: step 1…` then `Required skills: Thought i:{CoT}, Code i:{lemma statement}`.
- *Formalizer* — Input: "provide formal proof in response to a given problem statement" + `Useful skills i:{lemma code}` (the retrieved n_f) + example + statements. Output: complete formal proof code.
- *Directional transformer* — Input: "modify the given lemma/theorem/function/definition… to aid in solving one or more of the problems provided… by {transform direction description}" + `Problem i:{problem/request}` + `Skill to evolve:{lemma code}`. Output: `Evolved skill:{new lemma code}`.
- *Request solver* — Input: "provide a formal proof in response to a given formal statement" + `{retrieved lemma as in-context example}` + `Formal statement:{problem statement}`. Output: `Formal proof {new lemma code}`.

**Related-work anchors LEGO cites for the library idea:** Voyager (Minecraft skill library), LLMs-as-tool-makers (Cai 2023), DreamCoder (wake-sleep library learning), template-based conjecturing (Nagashima 2023).
