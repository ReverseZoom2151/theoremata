# TheoremExplainAgent (TEA) — Resource Mining Report

- **Repo:** `resources/TheoremExplainAgent-main/TheoremExplainAgent-main`
- **Paper:** Ku, Chong, Leung, Shah, Yu, Chen — *TheoremExplainAgent: Towards Video-based Multimodal Explanations for LLM Theorem Understanding*, arXiv 2502.19400, **ACL 2025 main (Oral, top 3%)**.
- **Authors' lab:** TIGER-Lab (Wenhu Chen). MIT License.
- **Size:** 89 files. Pure Python (~2.5k LoC core + ~30 prompt `.txt` files + benchmark JSON). No Rust/Lean.

---

## 1) What it is

TEA is an **agentic pipeline that turns a theorem statement into a long-form narrated Manim (animation) video** that visually explains it. The thesis of the paper: forcing a model to *produce a multimodal explanation* (visuals + narration) surfaces reasoning flaws that pure-text explanations hide. It ships two artifacts:

1. **TheoremExplainAgent** — the generation agent (planner → code generator → renderer, with optional RAG and visual self-reflection).
2. **TheoremExplainBench (TEB / "THB")** — a **240-theorem benchmark** (4 subjects × 3 difficulties) plus an **automatic LLM/VLM evaluation suite** that scores generated explanation videos on a rubric.

It is NOT a prover — there is no formal verification, no Lean/Rocq/Isabelle, no proof DAG. Its transferable value to Theoremata is **orchestration patterns, staged planning, self-critique loops, the LLM-judge rubric methodology, and prompt engineering** — not proof machinery.

### Pipeline (the "method")
`generate_video.py` → `VideoPlanner` → `CodeGenerator` → `VideoRenderer` → `evaluate.py`/`eval_suite`.

The planner is a **4-stage cascade per topic**, then a **per-scene 3-stage refinement**:

1. **Scene outline** (`generate_scene_outline`, `prompt_scene_plan.txt`) — one LLM call produces `<SCENE_OUTLINE>` of 3–7 scenes, each with Title / Purpose / Description / Layout. Constraints baked in: "scenes must build progressively," "total duration < 15 min," safe-area margins, no external assets.
2. Per scene, run concurrently (`asyncio` + semaphore, `generate_scene_implementation_concurrently`):
   - **Vision storyboard** (`prompt_scene_vision_storyboard.txt`) → `<SCENE_VISION_STORYBOARD_PLAN>`
   - **Technical implementation plan** (`prompt_scene_technical_implementation.txt`) → `<SCENE_TECHNICAL_IMPLEMENTATION_PLAN>`
   - **Animation + narration plan** (`prompt_scene_animation_narration.txt`) → `<SCENE_ANIMATION_NARRATION_PLAN>`
   Each stage's output is fed forward as input to the next (`src/core/video_planner.py:180-346`).
3. **Code generation** (`CodeGenerator.generate_manim_code`) turns the implementation plan into Manim Python; **render** executes `manim -qh` as a subprocess; failures loop back into fix prompts.

Every stage extracts its payload from XML tags via regex and writes it to disk (resumable — `--check_status`, `--only_plan`, `--only_render`).

---

## 2) Reusable ideas for Theoremata — THE priority

### 2.1 Staged plan cascade with typed hand-offs → our `reason`/planner
The single most transferable pattern: **decompose "produce artifact X" into a fixed cascade of specialized LLM roles, each emitting an XML-tagged block that becomes the next stage's input.** Outline → storyboard → technical plan → concrete artifact. This is directly analogous to our proof pipeline (informal sketch → proof DAG / leanblueprint node → tactic-level plan → formal code). TEA proves the value of:
- **Separating "what/why" planning from "how" implementation** — the outline reasons about pedagogy/progression; the technical stage reasons about concrete API calls. Map to: *proof strategy node* vs *tactic emission*.
- **Feed-forward context accumulation** — `implementation_plan += vision + technical + narration` (`video_planner.py:248,294,332`). Our plan-history / QED-retry should similarly carry the full staged trace.
- **Concurrency with a semaphore per sub-unit** (`scene_semaphore`, `max_scene_concurrency`) — scenes are independent; our independent DAG nodes / lemmas can render the same way (`generate_scene_implementation_concurrently`, `video_planner.py:385-417`).

### 2.2 Generator / verifier / fixer split with a bounded retry loop → our critic + QED retry
`CodeGenerator` + `VideoRenderer` implement a **generate → execute → observe error → fix** loop that is structurally identical to our falsify-before-prove / hammer-retry loop, minus the formal checker:
- **Execution as ground-truth verifier:** `subprocess.run(["manim", "-qh", ...])`; `returncode != 0` raises with `stderr` as the error signal (`video_renderer.py:58-68`). For us the analogue is the Lean/Rocq/Isabelle compiler exit + goal state — TEA validates the *architecture* of "run the real tool, capture stderr, feed it back."
- **Error-fix prompt** (`fix_code_errors` → `prompt_fix_error.txt`) takes `(implementation_plan, code, error)` and returns patched code. Directly maps to our per-system proof-repair generators.
- **Bounded retries with reset-on-progress:** `while retries < max_retries` (default 3); on a successful visual-fix iteration `retries = 0` to give more budget when making progress (`video_renderer.py:53-108`).

### 2.3 Visual self-reflection (VLM critic) → our critic/taint
The `--use_visual_fix_code` path is a **self-critique loop against a rendered artifact**, worth stealing as a pattern for our critic:
- `visual_self_reflection()` feeds the **rendered mp4/snapshot back into a multimodal model** with `prompt_visual_self_reflection.txt` and asks for issues + corrected code (`code_generator.py:392-454`).
- **Explicit termination sentinel:** the critic must return `<LGTM>` when the artifact is good, else `<reflection>...</reflection><code>...</code>`. The loop breaks on `<LGTM>` (`video_renderer.py:100`). Clean "critic says done" protocol we can adopt for QED/critic convergence.
- **Anti-refusal guardrail — `banned_reasonings.txt`:** a hard list of judge cop-out phrases ("cannot evaluate", "unable to provide the evaluation", "do not have the capability"…). If the critic response contains any, the loop terminates rather than treating the refusal as signal (`code_generator.py:61`, `get_banned_reasonings`, checked at `video_renderer.py:100`). **We should add exactly this to our critic/LLM-judge to stop non-answers from poisoning retries.**

### 2.4 The LLM/VLM-judge evaluation rubric → our `eval` component (highest-value steal)
`eval_suite/` is a clean, multi-dimension **LLM-as-judge harness** with a reusable methodology:
- **Multi-modal, multi-dimension scoring**, each dimension 1–5 with fixed anchor definitions (1 = "completely fails", 5 = "fully meets or exceeds"), and every score paired with a `comprehensive_evaluation` justification. JSON output enforced. Three judges:
  - **Text/transcript** (`text_eval_new.txt`): *Accuracy & Depth*, *Logical Flow*. Notably instructs the judge to **ignore modalities it can't see** ("you do not have access to the visual portion… just assume reasonable visuals") and **do not double-count a violation across criteria** — good rubric hygiene for our multi-signal eval.
  - **Image/frame** (`image_eval.txt`): *Visual Relevance*, *Element Layout* (placement, overlap, clarity).
  - **Video chunk** (`video_eval_new.txt`): *Visual Consistency* (style consistency, smoothness).
- **Score plumbing** (`eval_suite/utils.py`): `extract_json` (tolerant to ```json fences), `convert_score_fields` (recursively coerces `score` → int, raises on garbage), and **`calculate_geometric_mean`** — they aggregate dimension scores with a **geometric mean** so any single near-failing dimension drags the whole score down (product-based, punishes imbalance). Worth adopting for our proof/explanation quality aggregate instead of a mean.
- **Retry-until-parseable judge calls:** `evaluate_text` retries `retry_limit` times, re-parsing JSON, raising only after exhausting (`text_utils.py:54-80`). Same pattern for all judges.
- **Transcript normalization before judging:** `parse_srt_to_text` dedups repeated SRT lines; `fix_transcript` (`fix_transcript.txt`) repunctuates auto-generated transcripts inside a `<SCRIPT>` block before scoring — a pre-normalization step so the judge scores content, not formatting noise.

**Mapping:** our `eval` / benchmark harness should mirror this exactly — per-dimension 1–5 with anchored definitions, mandatory written justification per score, JSON-enforced output, banned-refusal list, geometric-mean aggregation, and a parse-retry wrapper. The rubric *dimensions* differ (we'd want: statement fidelity, proof correctness, gap-freeness, formal-check pass, minimality) but the *scaffolding* transfers 1:1.

### 2.5 Prompt-engineering patterns worth porting
- **XML-tagged contract I/O everywhere** — `<SCENE_OUTLINE>`, `<SCENE_TECHNICAL_IMPLEMENTATION_PLAN>`, `<reflection>`/`<code>`, `<LGTM>`, `<SCRIPT>`. Regex extraction with graceful fallback to the raw text if the tag is missing (`video_planner.py:166-167`). Robust and cheap; we already use similar but their tag-per-stage discipline is tidy.
- **`_extract_code_with_retries`** (`code_generator.py:208-250`): if the model's output doesn't match the expected ```python fence, it re-prompts with "return the EXACT same code in the correct format, NO CONTENT EDITING" up to 10× before giving up. A reusable "format-repair" sub-loop separate from the "logic-repair" loop.
- **Prompt modularization + build step:** prompts live as raw `.txt` in `task_generator/prompts_raw/`, compiled into importable `get_prompt_*` functions via `task_generator/parse_prompt.py` (regenerates `__init__.py`). Clean way to keep long prompts out of code and version-controlled. Constraint fragments are separate files composed in (`code_font_size.txt`, `code_disable.txt`, `code_limit.txt`, `banned_reasonings.txt`, `prompt_best_practices.txt`, `prompt_manim_cheatsheet.txt`).
- **Pedagogical meta-prompt:** `prompt_teaching_framework.txt` is a long "how to structure an educational explanation" doc (Bloom's taxonomy objectives, Hook 5-10% / Context / Core 60-70% / Practice / Summary, cognitive-load chunking). Not proof-relevant, but the idea of a **reusable domain "framework" prompt injected into the planner** maps to injecting a "proof-writing methodology" preamble into our reason stage.

### 2.6 RAG / tool-use pattern → our retrieval
`src/rag/` is a **staged, cached RAG** worth comparing to our retrieval:
- **Query generation is itself an LLM step, per stage:** `_generate_rag_queries_storyboard/_technical/_narration/_code/_error_fix` each ask the helper model to emit a JSON list of retrieval queries tailored to that stage (`code_generator.py:94-206`, `rag_integration.py`). I.e. retrieval queries are *generated from the current plan*, not from the raw topic.
- **Per-scene disk cache** of generated queries (`rag_cache/rag_queries_*.json`) to avoid re-querying on resume.
- **"Plugin detection" pre-step:** `detect_relevant_plugins(topic, description)` picks which Manim extension libs are relevant up front and scopes all retrieval to them (`prompt_detect_plugins.txt`, `video_planner.py:150-153`). Analogous to us selecting relevant mathlib areas / tactics before retrieving.
- ChromaDB vector store, `find_relevant_docs(queries, k=2)` (`vector_store.py`). LiteLLM embeddings (`azure/text-embedding-3-large` or `vertex_ai/text-embedding-005`).

### 2.7 Provider layer → confirms our LiteLLM choice
`mllm_tools/`: a `LiteLLMWrapper` plus dedicated `GeminiWrapper`/`VertexAIWrapper` for native video/multimodal input, dispatched by model-name prefix (`model_name.startswith(('gemini/','vertex_ai/'))`, `video_renderer.py:79`). `src/utils/allowed_models.json` whitelists usable models. Langfuse tracing via `metadata={generation_name, trace_id, session_id, tags}` threaded through **every** model call — a clean observability convention we could adopt (per-call generation_name + trace_id).

---

## 3) Dataset / benchmark + eval schema

- **TheoremExplainBench (TEB):** 240 theorems on HF (`TIGER-Lab/TheoremExplainBench`), schema `['uid','subject','difficulty','theorem','description','subfield']`. Subjects: **math, comp_sci, physics, chemistry**; difficulties **easy/medium/hard** (`data/thb_{easy,medium,hard}/{subject}.json`). Local copies are the generation inputs (`--theorems_path`), each record `{theorem, description, difficulty, remark, subfield}` (see `data/thb_easy/math.json`: Pythagorean Theorem, Euler's formula, etc.).
- **Eval schema:** per-artifact JSON, `{overall_analysis, evaluation: {<dimension>: {comprehensive_evaluation, score:1-5}}}`; dimensions per modality (§2.4); aggregate via geometric mean. `evaluate.py` supports `--eval_type {text,video,image,all}`, `--bulk_evaluate`, `--combine`, parallel `--max_workers`. Requires a video + SRT pair.
- Only ~math subset is directly relevant to us; the **rubric + aggregation code** is more valuable than the data.

## 4) Test / benchmark value

- No unit tests in the repo (research code). Value is not a test suite to import.
- **Directly liftable code:** `eval_suite/utils.py` (`extract_json`, `convert_score_fields`, `calculate_geometric_mean`) and the judge-call retry wrapper — ~80 LoC, dependency-light, drop-in for our `eval` harness.
- **Liftable prompt scaffolding:** the 1–5 anchored rubric template, `banned_reasonings.txt`, the `<LGTM>` critic contract, and `_extract_code_with_retries` format-repair loop.
- The Manim/TTS/render stack (kokoro, manim subprocess, video combine, `parse_video.py`) is **not** relevant to Theoremata.

## 5) New vs. what we already built

**Already have (TEA confirms our choices, adds little):**
- LiteLLM provider abstraction — we have it; TEA validates + shows a multimodal-dispatch variant.
- Retry-on-error proof/repair loop, plan history — we have richer versions (hammers, portfolio, formal checker).
- RAG retrieval — we have retrieval; TEA's *per-stage LLM-generated queries* + *up-front scope detection* + *query caching* are refinements worth checking against ours.
- MCTS / STaR / GRPO — TEA has **none** of these; strictly less than us on search/RL.

**New / worth adopting:**
1. **LLM-judge eval rubric methodology** (§2.4) — anchored 1–5 dimensions + mandatory justification + JSON + **geometric-mean aggregation** + parse-retry. Our strongest gap-filler for `eval`.
2. **`banned_reasonings` anti-refusal guard** for critic/judge loops (§2.3) — cheap, high-value robustness fix.
3. **`<LGTM>` explicit critic-convergence sentinel** (§2.3) — clean termination protocol for critic/QED loops.
4. **Format-repair sub-loop distinct from logic-repair** (`_extract_code_with_retries`, §2.5) — separate "wrong format" from "wrong content" recovery.
5. **Prompt-as-`.txt` + build-step modularization** (`parse_prompt.py`, composable constraint fragments) — nicer prompt hygiene than inlining.
6. **Multimodal self-reflection against the rendered artifact** — conceptually maps to critiquing a *compiled/checked* proof object, not just the source.

**Explicitly NOT transferable:** Manim/video/TTS rendering, storyboard/animation stages, visual-layout rubric dimensions, the video dataset itself.

---

### Key file map
- Orchestration entry: `generate_video.py`, `evaluate.py`
- Planner cascade: `src/core/video_planner.py`
- Generator/fixer/self-reflection: `src/core/code_generator.py`
- Execute + retry loop: `src/core/video_renderer.py`
- LLM-judge harness + rubrics: `eval_suite/` (`text_utils.py`, `utils.py`, `prompts_raw/{text_eval_new,image_eval,video_eval_new,fix_transcript}.txt`)
- Prompts: `task_generator/prompts_raw/*.txt` (+ `parse_prompt.py` build step)
- RAG: `src/rag/{rag_integration,vector_store}.py`
- Providers: `mllm_tools/{litellm,gemini,vertex_ai}.py`, `src/utils/allowed_models.json`
- Benchmark: `data/thb_{easy,medium,hard}/{math,comp_sci,physics,chemistry}.json`
