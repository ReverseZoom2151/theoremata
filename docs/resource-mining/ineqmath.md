# Resource Mining: IneqMath

Full-pass study of `resources/ineqmath-main/ineqmath-main/` (NeurIPS 2025 Spotlight, Lu et al., "Solving Inequality Proofs with Large Language Models", arXiv:2506.07927). Repo `lupantech/ineqmath`, dataset `AI4Math/IneqMath`. License: code MIT, data CC-BY-SA-4.0 (test split may be used only as a *test* set, never for training).

This supersedes our earlier targeted skim. All paths below are relative to `resources/ineqmath-main/ineqmath-main/`.

---

## 1) What it is (scope, size, dataset structure)

IneqMath is **an inequality dataset + a zero-shot LLM eval harness + an LLM-as-judge grader**, plus optional prompting "strategies" and data-curation pipelines. The paper's central move: recast Olympiad inequality *proving* (hard to auto-check) into two **informal-yet-verifiable** subtasks, each with a machine-checkable final answer:

- **`bound`** — "find the largest/smallest constant `C` such that the inequality holds for all variables". Answer is a real number, e.g. `$C = 4$`.
- **`relation`** — a multiple-choice fill-in-the-relation: options **(A) ≤ (B) ≥ (C) = (D) < (E) > (F) None of the above**. Answer is one option.

The 2,703-file count is almost entirely **data artifacts**, not code. Actual surface:
- **23 Python files, 32 shell scripts, 1 README (~52 KB), 3 judge prompt `.md` files.**
- ~1,305 `.md` and ~1,317 `.json` files are per-problem *visualizations* and *result dumps* (query/response pairs) under `results/**/raw/` and `data_curation/**/raw/`.

**Dataset sizes** (README "Dataset Overview"): 200 test / 100 dev (public GT) / 1,252 train (626 bound + 626 relation). Train has stepwise solutions for all 1,252, named-theorem annotations for 962 (76.8%), avg 1.05 / max 4 solutions per problem. **83 named theorems across 29 categories** (`theorems.json`, downloaded from HF at runtime — not vendored here). Test problems authored/reviewed by IMO-level medalists.

**What is NOT in the repo** (lives on the HF Spaces "evaluation platform" only): the actual **test/dev ground truth JSON** (`test.json`, `dev.json`, `train.json`, `theorems.json` are all `wget`-ed from HF at runtime) and — critically — the **four step-wise judge prompts** (see §4). Only the Final-Answer judge is open-source here.

Repo layout:
- `models/engines/` — model-agnostic provider adapters (OpenAI, Anthropic, Gemini, DeepSeek, xAI/Grok, Together, vLLM) behind a `factory.create_llm_engine(model_string)`.
- `models/utils/` — `solve.py` (generation), `generate_results.py` (aggregate raw → `results.json`), `compute_score.py` (**the Final Answer Judge**), plus `solve_few_shot.py`, `solve_solution_as_hints.py`, `solve_theorem_as_hints.py`.
- `models/prompts/` — 3 files: `answer_extraction_bound.md`, `answer_extraction_relation.md`, `answer_verification_bound.md`.
- `data/json/` — `few_shot_examples.json`, `frequent_solution_set.json`, `training_data_sampled_200.json` (the only vendored GT sample).
- `data_curation/` — the **reformulation pipeline** (proof → bound/relation) and **training-data enhancement** (solution rewriting + SFT JSONL).
- `scripts/`, `strategies/`, `experiments/` — bash runners for each provider / strategy.

---

## 2) Reusable ideas / patterns / code for Theoremata (THE priority)

### 2a. The two-stage Final Answer Judge (extract → verify), fully in `models/utils/compute_score.py`

This is the piece we drew "grader" ideas from. It is a **deterministic-first, LLM-fallback** pipeline — cheap string/regex path, then an LLM only when needed. Directly portable to our grader / `eval_harness`.

**Pipeline per problem** (`ScoreComputer.evaluate_single_result`):
1. `locate_answer(prediction)` — regex-free string scan for the answer sentence. Trigger phrases in priority order: `"final answer is"` → `"answer is"` → `"final answer"`, taken at the **last** occurrence (`rfind`), normalizing `\(`,`\)` to `$`. Fallback: last 3 newline-separated lines. Empty prediction / model error → `is_solved=False, is_correct=False`.
2. Branch on `type`:
   - **bound**: `extract_bound_answer` then `verify_bound_answer`.
   - **relation**: `extract_relation_answer` then `verify_relation_answer` (exact option-string match).

**Answer extraction is layered** (regex → LLM):
- `extract_bound_answer`: optional quick path grabs the first `$...$`, strips a `\boxed{...}` wrapper. Otherwise falls back to an **extractor LLM** with few-shot examples from `answer_extraction_bound.md`, forced to output `C=<value>`.
- `extract_relation_answer`: first tries exact/loose token match (`(A)`, `leq`, `≤`, `<`, …); only if that fails calls the extractor LLM with `answer_extraction_relation.md`; last resort picks a **random** option (`random.seed(42)`) — a pragmatic choice we should NOT copy for a rigor-focused harness (bias the failure toward "wrong/None", don't randomize).

**Bound equivalence verification** is the reusable gem — `verify_bound_answer` → `verify_bound_answer_with_llm`. Clean both sides (`.replace("$","").replace(" ","")`); exact match short-circuits to `True`; otherwise a **symbolic-equivalence LLM judge** with a Pydantic structured output:

```python
class EquivalenceCheck(BaseModel):
    analysis: str
    equivalent: bool
```

The judge prompt (verbatim, `compute_score.py:196-229`) encodes strict math-equivalence rules that are exactly the ambiguities our grader must resolve:

> Required Analysis Steps: … For numerical expressions: Direct equality (2 = 2) → True; Different representations of same value (1/2 = 0.5, √4 = 2) → True; **Decimal approximations vs exact values (2π ≠ 6.28318) → False**. For algebraic expressions: Must have clear, valid transformation path… Equivalence Criteria: Must have exactly same deterministic value… **Decimal approximations are NOT equivalent to exact expressions… No rounding or approximations allowed… If equivalence cannot be conclusively proven → False.**

`answer_verification_bound.md` supplies **16 worked few-shot pairs** with `Analysis`/`Equivalent` labels — e.g. `1/√2 = √2/2 → True` (rationalization), `2π vs 6.28318 → False`, `√(3/2) vs 3/(2√2) → False` (proved by squaring), `C=1 vs "given in the required format" → False`. This is a ready-made, battle-tested rubric + few-shot set for a **symbolic answer-equivalence checker**. Note the design bias: **exact forms only, decimals never equal exact** — good default for us, but our SymPy/Lean-backed falsifier can do this deterministically instead of via LLM (see §6).

**Scoring** (`calculate_score`): per `data_split`, tallies `all` / `bound` / `relation` totals, correct, wrong, accuracy (%), and `empty_responses`. Concurrency via `ThreadPoolExecutor(max_workers)`; results keyed by id. CLI supports `--direct_input --results_file X.json` to grade any results file standalone. Accepts results as either a dict-keyed-by-id or a list of records.

### 2b. The reformulation pipeline = the "falsify-before-prove" / verifiability angle

`data_curation/data_reformulation/{bound_reformulation.py, relation_reformulation.py, solution_reformulation.py}` is an **automatic pipeline that converts any (problem, proof) pair into a checkable subtask**. This is IneqMath's answer to the same problem our falsifier addresses: *proofs are hard to auto-verify, so reduce to a checkable numeric/relational claim.* Both are LLM-driven, structured-output (`TransformedProblem{analysis, conclusion, rephrased_problem, answer}`), with retry.

- **bound_reformulation** (prompt at `bound_reformulation.py:30-86`): introduce constant `C`, decide min vs max ("if the side containing C is larger, find the smallest C; if smaller, find the largest C"), enforce that **optimal C is a real number, not an expression** (`$n$ not allowed`). Handles homogeneity, domains, double inequalities.
- **relation_reformulation** (prompt at `relation_reformulation.py:30-106`): blank out the relation symbol, offer options A–F, and — key falsification hook — *"If the relation depends on specific values of the variables or cannot be definitively determined, use (F) None of the above."* That `(F)` option is precisely where a **counterexample** decides the answer.
- **solution_reformulation** (`solution_reformulation.py`): for `bound`, appends an **equality-condition analysis** ("equality holds when a=b=c=1, so C is the tight max/min") — i.e. it forces the tightness/witness reasoning that distinguishes a real bound proof from a loose one. `reformulation_example.md` shows two fully-worked examples of this equality-witness augmentation.

**Takeaway for Theoremata:** IneqMath validates our falsify-before-prove design at the *task* level — reduce a proof goal to a claim whose negation is a searchable witness (a value violating the bound, or a variable assignment flipping the relation). Their "(F) None of the above" + equality-condition step is exactly a counterexample/tightness check. We can (a) ingest their bound/relation tasks as falsifier test cases, and (b) reuse the reformulation prompts to auto-mint verifiable subgoals from arbitrary inequality proofs in our corpus.

### 2c. Task-instruction prompts (generation side)

`solve.py:23-24` — the two "hint" prompts pinned to a machine-parseable answer format (this is what makes 2a's extractor cheap):
- bound: *"…state your answer in exactly this format: 'The answer is $C=X$'…"*
- relation: *"…'The answer is (Letter) Symbol'… Example: 'The answer is (A) $\leq$'."*

Both insist on "clear, rigorous, and logically sound steps" — the stepwise judges (§4) then check whether the model actually delivered that. **Lesson: co-design the generation prompt and the grader's extraction regex** so the answer is unambiguous to locate; leave the *reasoning* free-form for the step judges.

### 2d. Model-agnostic engine layer (compare to our LiteLLM provider)

`models/engines/` is a hand-rolled equivalent of our LiteLLM abstraction: `factory.create_llm_engine(model_string)` dispatches by substring (`gpt/o1/o3/o4` → OpenAI, `claude` → Anthropic, `gemini`, `grok`, `deepseek`, `vllm-`, `together-`). Each engine subclasses `EngineLM` + `CachedEngine`. Worth noting:
- **Disk cache** (`base.py`): `diskcache` keyed by `sha256(system_prompt+prompt)`, with `__getstate__/__setstate__` so cached engines survive pickling across threads. Cheap, deterministic, reproducible — a good pattern for our eval reruns.
- **Structured output + reasoning-model handling** (`openai.py`): auto-detects chat vs reasoning vs pro-reasoning models; uses `client.beta.chat.completions.parse(response_format=PydanticModel)` for structured JSON; distinct `max_completion_tokens` + `reasoning_effort="medium"` path for `o1/o3/o4/gpt-5`; robust `LengthFinishReasonError`/`RateLimitError` handling returning error dicts (not exceptions) so a bad problem doesn't kill the batch. `tenacity` retry with exponential backoff on all engines.
- Cache-key subtlety: `max_tokens` only appended to the cache key when `!= 10000` — a footgun (10k runs with different-but-both-10k configs could collide); we should key on the full config.

We already have LiteLLM, so this is mostly *validation* of our choices plus two concrete borrowings: the diskcache-by-prompt-hash pattern and the error-dict-instead-of-raise batch resilience.

---

## 3) Dataset + result JSON schema (sampled)

**Sampled**: `data/json/training_data_sampled_200.json` (200-record list; inspected record[0] and first record with `theorems`), `results/models_results_test_data/gpt-4o-mini_tokens_10000/results.json` (100-record list, record[0]), `results/**/scores.json`, `data/json/few_shot_examples.json`, `data/json/frequent_solution_set.json`.

**Problem / training record** (`training_data_sampled_200.json`):
```
data_id    : str            e.g. "626"
problem    : str            LaTeX problem statement (relation problems embed "()" blank + choices)
type       : "bound" | "relation"
data_split : "train"|"dev"|"test"
answer     : str            bound: "(B) $\geq$" for relation; "$C = 4$" for bound
solution   : str            stepwise human solution (LaTeX); may be a list (≤4) in full train set
theorems   : dict           { "Theorem 35": {Nickname:[...], Theorem:"<LaTeX statement>", Theorem_Category:"..."}, ... }  (or {} / "{}" )
choices    : list|"NaN"      relation: ["(A) $\leq$", ... "(F) None of the above"]; bound: NaN
difficulty : str            often empty in the sampled file
```
Note the id key is inconsistent: generation code reads `data["annot_id"] if "annot_id" in data else data["data_id"]` everywhere — worth normalizing on ingest.

**Theorem entry** sub-schema: `{ "Nickname": [str], "Theorem": "<LaTeX>", "Theorem_Category": str }`.

**Generation result record** (`results/**/results.json`, a list) = the full problem record **plus**:
```
prompt   : str    the exact query sent (task hint + problem + "Solution:")
response : str    raw model output
success  : bool
error    : str|null
```
(When serialized from an environment where fields were absent, `theorems` may be the string `"{}"` and `choices` the string `"NaN"` — type coercion to watch for.)

**Graded record** (added by `compute_score.py`, keyed by id) adds an `evaluation`:
```
evaluation: { ground_truth, answer_sentence, extracted_answer, is_solved: bool, is_correct: bool }
```

**`scores.json`** — nested by split then category:
```json
{ "dev": { "all": {total, correct, wrong, accuracy, empty_responses},
           "bound": {total, correct, wrong, accuracy},
           "relation": {total, correct, wrong, accuracy} } }
```

**`few_shot_examples.json` / `frequent_solution_set.json`** — both are dicts keyed `"<Theorem N>_<bound|relation>"` (e.g. `"Theorem 35_bound"`), each value `{problem, answer, type, solution}`. Used to inject top-k frequent demonstrations.

**SFT export** (`sft_data_generation.py`) — JSONL of OpenAI chat format: `{"messages":[{"role":"user","content": hint+problem},{"role":"assistant","content": enhanced_solution}]}`.

**Submission format** for the leaderboard (README) minimally requires: `data_id, problem, type, prompt, response`.

---

## 4) What our earlier targeted pass MISSED

1. **The four step-wise judges are NOT in this repo.** The paper's headline metric — *overall* accuracy (a solution passes only if it clears all 5 judges) — relies on four step judges the README names but does **not** ship:
   - **NTC** — No Toy Case (rejects "verified on examples ⇒ general conclusion")
   - **NLG** — No Logical Gap (rejects unjustified leaps)
   - **NAE** — No Approximation Error
   - **NCE** — No Calculation Error
   Only the **Final Answer judge** (§2a) is open-source. The step judges run server-side on the HF Spaces "evaluation platform"; you submit `results.json` to get step accuracy. So "we drew grader ideas from IneqMath" is only partly reproducible from code — we have the *answer-equivalence* rubric verbatim, but the *step-soundness* rubrics only as prose definitions (NTC/NLG/NAE/NCE) + the F1=0.93 human-alignment claim. **If we want stepwise judges, we must author the prompts ourselves** from these four failure-mode definitions (which is itself a strong, reusable taxonomy for our proof-DAG step checker).
2. **The headline empirical result** (directly relevant to our thesis): top models reach ~66–76% *answer* accuracy but **<10% *overall*** accuracy — a drop up to 65.5%. "Finding the answer" ≠ "constructing a sound proof." Scaling model size / test-time compute barely moves *overall* accuracy (the scaling curve flattens). This is the strongest external evidence for why a **proof-DAG + step-level verification harness like ours is necessary** rather than answer-matching.
3. **The reformulation pipeline** (§2b) is a genuine methodological asset we under-weighted: an executable proof→verifiable-subgoal converter with equality-witness augmentation — conceptually our falsify-before-prove, and reusable to expand our own benchmark.
4. **Two In-depth-Study findings** we can lift as design hypotheses: (a) *theorem hints help strong models but hurt weak ones* (misapplication/distraction) — argues for careful RAG/theorem-retrieval gating in our loop; (b) *self-critique as feedback* raised Gemini 2.5 Pro overall accuracy 43%→48% — supports our self-refinement/critic loop.
5. **Random-guess fallback in the grader** (`extract_relation_answer` returns `random.choice`) — a correctness smell to avoid porting.
6. **Engine details**: diskcache-by-hash, structured-output via Pydantic, reasoning-model `reasoning_effort` branch, error-as-dict batch resilience (§2d).

---

## 5) Test / benchmark value: can we ingest IneqMath as a Theoremata track?

**Yes, with caveats.**
- **Fit**: every problem already reduces to a machine-checkable claim — `bound` (real number, our falsifier can numerically/​symbolically test tightness and violation) and `relation` (6-way MC, decidable by counterexample search for the `(F)` case). This maps cleanly onto our falsify-before-prove core and gives an *answer-level* ground truth to gate proofs against.
- **How**: pull `test.json`/`dev.json`/`train.json`/`theorems.json` from `huggingface.co/datasets/AI4Math/IneqMath`. Dev (100) has public GT → use for our own harness tuning. Test (200) GT is answer-only public via leaderboard; **step-level** GT is server-side. Train (1,252) has GT + stepwise solutions + theorem annotations → excellent for retrieval/RAG and for training/evaluating our step checker.
- **Ingest shape**: normalize `annot_id`/`data_id`; keep `type`, `problem`, `answer`, `choices`, `solution`, `theorems`. Reuse their generation-prompt format so answers are locatable, but replace their LLM answer-equivalence with our **SymPy/Lean** deterministic check (see §6).
- **Licensing constraint (important)**: test split is **test-only, no training** (CC-BY-SA-4.0 + explicit no-commercial-training clause). Safe to use as a benchmark track; do **not** fold test problems into any training/finetuning corpus.
- **Value-add we can uniquely provide**: their bottleneck is that *overall* (step) grading is closed and LLM-judge-based. Our proof-DAG can produce **executable, deterministic step verification** (a real differentiator vs. their LLM step judges) and our falsifier can *decide* bound/relation answers rather than guess — potentially a stronger, cheaper grader than their pipeline for this exact domain.

---

## 6) New vs. already-in-our-design (grader, eval_harness, falsifier)

**Already in our design (IneqMath validates / offers concrete borrowings):**
- **Grader**: our LLM-judge grader parallels their Final Answer judge. Borrow the `answer_verification_bound.md` 16-example symbolic-equivalence rubric and the "decimals ≠ exact, no approximations" policy — but back it with SymPy/Lean so we're deterministic where they're LLM-stochastic. Adopt the *extract-then-verify* two-stage split (locate answer sentence → normalize → equivalence-check) and the exact-match short-circuit.
- **eval_harness**: their `solve.py → generate_results.py → compute_score.py` (generate → aggregate → score, per-split bound/relation/all tallies with `empty_responses`, `--direct_input` standalone scoring, threadpool, resume-by-existing-file) is a clean template to mirror. Borrow the diskcache-by-prompt-hash and error-dict batch resilience.
- **falsifier**: their reformulation `(F) None of the above` + equality-condition/tightness step is a task-level instance of our counterexample-before-prove. Confirms the approach; the bound/relation tasks are ready-made falsifier test cases.
- **Model-agnostic provider**: our LiteLLM already generalizes their per-provider `engines/`; no need to port, but their reasoning-model branching (`reasoning_effort`, `max_completion_tokens`) and structured-output-by-Pydantic are patterns to ensure LiteLLM path handles.

**New / not previously emphasized in our design:**
- **The four-axis step-soundness taxonomy (NTC / NLG / NAE / NCE)** — a reusable rubric for our proof-DAG step checker even though the prompts aren't shipped. Map each DAG edge/step to these four failure modes.
- **Overall = AND of 5 judges** (answer + 4 steps) as the headline metric — a stricter, more honest scoring contract than answer-accuracy that we should adopt for reporting.
- **Auto proof→verifiable-subgoal reformulation pipeline** — an ingestion/augmentation tool we didn't have; can expand our benchmark from raw inequality proofs.
- **Training-data enhancement** (solution rewriting for detail/rigor → SFT JSONL) — relevant only if we ever finetune; the equality-condition augmentation prompt is a nice rigor-forcing template.
- **Empirical anti-pattern to avoid**: `random.choice` guess fallback in grading; loose `max_tokens` cache-key collision.

**Net**: IneqMath is high-value as (a) a **benchmark track** for our inequality/falsifier path, (b) a **verbatim answer-equivalence rubric + few-shot set** for our grader, (c) a **failure-mode taxonomy** for our step checker, and (d) strong **external evidence** (answer≈70% vs overall<10%) that step-level, DAG-based verification — our core bet — is the missing piece. The closed step-judges are the main gap; we out-do them by making step verification deterministic/executable rather than LLM-judged.
