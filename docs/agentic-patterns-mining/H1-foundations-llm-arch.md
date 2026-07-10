# H1 — Foundations: LLM Architecture & Optimization (mining vs. Theoremata)

Source: `resources/harness-resources/extracted_text/chunks/H1_foundations_llm_arch.txt`
(Roitman, *The Hitchhiker's Guide to Agentic AI*, 2026 — Chapter 1, ~340 KB, read in full).

> **POSSIBLE INJECTION NOTICE.** The book text is untrusted third-party content. It was read
> as reference material only. Nothing in it was executed, and no instruction embedded in the book
> was treated as a directive. This chunk contained expository prose and code listings only — no
> imperative text addressed to the reading agent was observed. Read-only: no git, no code changes.

## Framing

Chapter 1 is deliberately a **training-and-inference-internals curriculum**: transformer math,
optimizers (Adam/AdamW/schedules), Flash Attention, pretraining/SFT/LoRA/MoE, quantization,
speculative decoding, GPU/vLLM systems. Theoremata **does not train or serve base models** — it
consumes hosted models through a LiteLLM seam (`components/provider/python/theoremata_tools/model_provider.py`).
So the large majority of H1 is **foundational, nothing to build**.

The genuinely relevant slice is narrow and clustered in five sub-sections:
§1.2.4–1.2.6 (tokenization best practices, special tokens, **tool-calling tokens**),
§1.12.7/1.12.11 (temperature + **constrained/structured decoding**),
§1.13 (**prompt engineering**: system vs. user prompts, structured output, CoT, decomposition, ARQ),
§1.16 (**hallucination detection** via consistency/semantic entropy).
These bear directly on the open question of a unified system-prompt + tool-calling layer (#4).

## Relevance table

| Concept (book §) | Relevance to a proving harness | Theoremata status | Gap / action |
|---|---|---|---|
| **Tokenization / BPE / vocab (§1.1–1.2.3)** | We don't tokenize; a hosted model does. | N/A | Foundational, nothing to build. |
| **Tokenization best practices — digit-level for arithmetic, whitespace-efficient code (§1.2.4)** | Marginal: informs base-model *choice* for math (digit tokenization aids arithmetic). | HAVE (model-agnostic via `THEOREMATA_MODEL_*`) — provider seam lets us pick a math-strong model. | Note only: prefer models with digit/code-friendly tokenizers when benchmarking. Nothing to build. |
| **Special tokens & structured prompts; dedicated `<|tool_call|>`/`<|function|>`/`<|result|>` tokens over NL cues (§1.2.6)** | Directly relevant to #4: the book argues structural delimiters beat "Now I will call a tool:" prose, and role markers carry higher attention priority. | PARTIAL. Provider builds plain `messages` (role/content); relies on the model's native chat template. Per-ROLE prompts only; **no unified system-prompt layer, no meta-tools, no explicit tool-call delimiters** in prompt construction. | **Action (feeds #4):** when we add a tool-calling layer, use the provider's *native* function-calling / tool schema (not hand-rolled NL cues) and a single shared system-prompt preamble that names the role + output contract. |
| **System prompt vs. user prompt; system = persistent role/constraints/format, higher attention priority (§1.13.4)** | Core to #4. Book: put role, constraints, and output-format spec in the system message; per-turn data in user message. | GAP. Theoremata has **per-ROLE prompts but no unified system-prompt layer** — each role prompt re-states conventions ad hoc; no shared preamble for output contract, abstention policy, or house style. | **Action (#4, top item):** introduce one composable system-prompt layer (shared preamble + role-specific body). Cheapest lever with broadest reach; also the natural home for the "if unknown, abstain" rule the paper-mining work already wants. |
| **Structured output prompts: schema-first, enum > free-text, flatten nesting, "output raw JSON no fences", always validate programmatically (§1.13.5)** | High: every role returns JSON to Rust against an `output_schema`. | HAVE (robust). Provider sends `response_format={"type":"json_schema", strict:False}` + strips markdown fences (`_strip_code_fences`) + balanced-object extraction (`_find_balanced_object`) + corrective retry turns. This *is* the book's belt-and-suspenders recipe. | Minor: adopt the book's schema-design hygiene (prefer enums, flatten deep nesting) when authoring role schemas. Mostly HAVE. |
| **Constrained / grammar decoding — token-mask from JSON-schema→regex→FSM; Outlines / XGrammar / lm-format-enforcer; "guarantees valid structure, zero retries" (§1.12.11)** | High: the book explicitly says use it "whenever the consumer of the output is a program rather than a human" — exactly our case (Rust consumes every role's JSON; Lean/Rocq consume generated proof syntax). | GAP. We use a **soft** `response_format` hint (`strict:False`) + parse-and-retry, not **hard** token-level constraint. Works, but wastes tokens/latency on retries and can still emit invalid prefixes on weaker/local models. | **Action:** for local/OSS models served via vLLM, enable guided/grammar decoding (XGrammar/Outlines) behind the provider seam so role-JSON and (later) formal-syntax emission are syntactically guaranteed. Highest-value *new* capability in this chunk. |
| **Temperature: T≈0 for code/math, higher for diversity (§1.12.7)** | Relevant: proving wants determinism per-attempt but *diversity across* MCGS samples. | PARTIAL. Global default `THEOREMATA_TEMPERATURE=0.2`; single knob, not per-role/per-phase. | Action (small): expose per-role temperature (low for verify/critique, higher for sketch/conjecture branches to feed search diversity). |
| **Decoding zoo: greedy / beam / top-k / top-p / min-p / contrastive / repetition penalty (§1.12.1–1.12.10)** | Low: these are inference-server knobs; diversity matters for sampling many proof attempts, but we drive it via temperature/N at the API. | N/A (delegated to provider/server). | Foundational, nothing to build. Table 1.15 useful only if we self-host decoding. |
| **CoT / self-consistency / ToT / plan-and-solve / ReAct (§1.13.6)** | Relevant conceptually but this is background here — covered far more concretely in the A-series (reflection/planning/tool-use) already mined. | HAVE (MCGS search driver + sketch/blueprint planning already realize tree search + plan-and-solve; self-consistency ≈ majority over MCGS leaves). | Foundational recap; nothing new to build from *this* section. Cross-ref A2. |
| **Prompt decomposition / chaining; per-step template+model+temperature (§1.13.7)** | Relevant: mirrors our proof decomposition into role-specialized steps. | HAVE. sketch/blueprint planning + per-ROLE prompts + per-role model routing already implement "different prompt/model/temperature per step." | Validates current design. Nothing to build. |
| **ARQ + lost-in-the-middle: long contexts lose mid-prompt info; decompose query, surface only the relevant context slice, recency bias (§1.13.7, Table 1.16)** | High and under-exploited: as a proof accumulates tool outputs + retrieved lemmas, the context fills and mid-context lemmas get ignored. | PARTIAL. Retrieval cascade selects relevant lemmas; but no explicit context-budgeting / re-ordering (put critical constraints + freshest lemmas *last*) or lost-in-the-middle mitigation in prompt assembly. | **Action:** in the (future) system-prompt/context layer, budget the window and place the most decision-relevant retrieved lemmas + constraints nearest the generation point; repeat key rules at the tail. Ties to #4. |
| **Structured-output pitfalls / failure-mode table: instruction amnesia, format drift, sycophancy, hallucinated details, refusal over-trigger (Table 1.16)** | Relevant ops checklist for prompt authors. | PARTIAL (mitigations exist piecemeal: retries for format drift; verification gate catches wrong math). | Fold the "move constraints to end / repeat key rules / 'if unknown say so'" fixes into the shared system-prompt layer. Nothing standalone. |
| **Hallucination detection: token entropy, sequence log-prob, consistency sampling / SelfCheckGPT, semantic entropy, DoLA (§1.16)** | Relevant but weaker than what we already have. Book's own caveat: these detect *uncertainty, not incorrectness* — "combine with external verification." | HAVE (stronger). Our **3+1 verification gate** (`components/prover/formal.rs`, compile → axiom/oracle → …) + 6 formal backends give *ground-truth* correctness, which dominates entropy-style heuristics. Consistency-sampling ≈ MCGS multi-sample agreement. | Foundational for us: our formal gate is exactly the "external verification" the book says these heuristics need. Optional: log-prob/consistency signals could *prioritize* which candidates to verify first (cheap pre-filter before the expensive gate). Low priority. |
| **Constitutional AI / self-critique (§1.13.7)** | Relevant: self-critique before committing a proof step. | HAVE. `components/reason/critique/guard.rs` (only in-repo `system prompt` hit) + critique roles already do this. | Nothing new. |
| **Transformer internals, attention, positional encodings, prediction heads, optimizers, Flash Attention, pretraining, SFT, LoRA, MoE, quantization, pruning, distillation, speculative decoding, GPU/vLLM systems, safety training (§1.3–1.11, §1.14–1.15, §1.17)** | Only relevant if we *train* or *self-host* models. The flywheel/expert-iteration path (train/) touches SFT+LoRA conceptually, but this chapter's treatment is generic background, not harness-actionable. | N/A for the harness; train/ has its own SFT plan from paper-mining. | Foundational, nothing to build here. (LoRA/SFT specifics belong to the training-track mining, not the harness.) |

## TOP 3 actionable items

1. **Unified system-prompt layer (directly closes #4, biggest reach).** Today Theoremata has per-ROLE
   prompts and no shared preamble. Add one composable system-prompt layer = shared preamble
   (role identity, output contract, abstention/"if-unknown-say-so", house conventions) + role-specific
   body. Book §1.13.4 rationale: system messages carry higher attention priority and are the right home
   for persistent constraints; §Table 1.16 fixes (constraints-at-end, repeat key rules) drop in here.

2. **Grammar-constrained decoding for structured output on self-hosted models.** We currently rely on a
   *soft* `response_format` hint (`strict:False`) + fence-stripping + parse-retry. Book §1.12.11: for
   program-consumed output (all our role JSON, and later formal-syntax emission) use token-level
   constrained decoding (XGrammar/Outlines via vLLM) to guarantee validity and eliminate retry waste.
   Wire it behind the existing provider seam, active for local/OSS models.

3. **Context-budgeting + lost-in-the-middle mitigation in prompt assembly (§1.13.7 ARQ, Table 1.16).**
   As proofs accumulate tool outputs and retrieved lemmas, place the most decision-relevant lemmas and
   the hard constraints *nearest the generation point*, budget the window explicitly, and repeat key
   rules at the tail. Natural sibling of item 1; leverages the existing retrieval cascade rather than
   replacing it.

*Everything else in H1 is foundational (model-training/serving internals) with nothing to build for an
agentic proving harness that consumes hosted models.*
