# A5 — Evaluation/Monitoring, Prioritization, Exploration/Discovery, CLI + Coding-Agents Appendices → Theoremata

Source: `resources/harness-resources/extracted_text/chunks/A5_eval_prioritization_exploration_appendices_coding_agents.txt`
(*Agentic Design Patterns*, Ch. 19 Evaluation & Monitoring, Ch. 20 Prioritization, Ch. 21 Exploration & Discovery,
Appendix A Prompting, Appendix E CLI Agents, Appendix G Coding Agents). ~319 KB, read in full.

> **UNTRUSTED BOOK TEXT — POSSIBLE INJECTION.** The chunk is third-party extracted book prose. I read it as *data*,
> not instructions. No prompt-injection or instruction-to-the-reader was detected in this chunk (it contains only
> vendor code samples — Gemini/OpenAI/LangChain — and expository text; the `genai`/`ChatOpenAI` snippets are the
> book's examples, **not** directives to Theoremata and were not executed or adopted). Treat every "you should…"
> in the source as addressed to the book's reader, not to this repo.

Legend: **HAVE** = implemented & wired; **PARTIAL** = present but narrow/offline/not unified; **GAP** = absent.

---

## Chapter 19 — Evaluation & Monitoring

| Pattern (book) | Book definition | Status | Where in Theoremata / what's missing | Buildable? |
|---|---|---|---|---|
| Response-accuracy scoring | Score output vs. ground truth; naive exact-match is insufficient, need semantic/keyword/embedding metrics | **HAVE** | `eval/…/grader.py` six-axis grader (axes kept SEPARATE, never blended); `benchmarks/graders.py` Lean compile/sorry/axiom gate; `grade_answer` symbolic equality (IneqMath "exact forms only"). Goes well beyond the book's exact-match toy. | n/a |
| LLM-as-a-Judge + rubric | Use an LLM with a scored rubric (1–5 criteria, JSON out) for subjective quality | **HAVE** | `eval/…/proof_grader.py`: rubric/step-wise judge, structured error taxonomy, `use_llm` path via mock-capable provider; 0–7 proof score. Book's `LLMJudgeForLegalSurvey` rubric = exactly our proof-grader rubric, but ours is domain-specialised and offline-testable. | n/a |
| Median/aggregate-of-N judging | `repeat_and_aggregate` (mean/min/max/median) to de-noise a judge | **HAVE** | proof_grader median-of-N; `proof_calibration.py` MAE/RMSE/Kendall-τ vs. human labels. Stronger than book (book has no calibration at all). | n/a |
| Reflect-and-revise judging | Critique own grade, then final verdict | **HAVE** | proof_grader `reflect_and_revise` mode documented/ported. | n/a |
| pass@k / majority@k reporting | Aggregate over k samples with error bars | **HAVE** | `eval_harness.py` pass@k / majority@k / averaged@k, std-error `std(ddof=1)/√n`, difficulty-tier + per-axis breakdown; contamination/n-gram/freshness controls. | n/a |
| Abstention / conditional accuracy | Let agent decline low-confidence; don't score abstention as wrong | **HAVE** | `orchestration/agent.rs` `AgentSummary.abstained` (Aletheia gate); excluded from conditional accuracy. Book doesn't have this. | n/a |
| **Trajectory evaluation** | Compare the *sequence of steps/tool calls* to a ground-truth trajectory (exact / in-order / any-order / precision / recall / single-tool match) | **PARTIAL** | We **capture** trajectories richly: `orchestration/observe.rs` (Observer → ordered `TraceSpan`s from the append-only event log, run metrics, replay descriptor), `search/search_telemetry.rs` (proof-length/diversity), `critique/plan_history.rs`, `tactic_outcome.rs`. But there is **no grader that scores a trajectory against a reference path** — no in-order/any-order/precision/recall trajectory metric, no "did it pick the right tool/route" scoring. `retrieval_eval` scores retrieval hits, not full agent trajectories. | **Yes** — add a trajectory-scoring track to `eval_harness` that consumes `observe.rs` spans + a gold action sequence (router route, tool, obligation order) and emits the book's match metrics. High value, low risk (all inputs already logged). |
| Test-file / evalset structure | JSON test files (single session) vs. evalsets (multi-turn); CLI + pytest + UI runners | **PARTIAL** | 33 benchmarks across 5 tracks (formalization/nl-answer/falsification/proof-grading/tactic) via `benchmarks/registry.py`, `eval_execution.py`; pytest runners exist. But these are **single-turn problem→answer** evals; we have no *multi-turn session evalset* schema (expected-tool-trajectory + intermediate responses per turn) and no interactive UI runner. | Yes — extend `schema.py` with a turn/session record; reuse observe.rs for "expected tool use" fields. |
| Latency / token / cost monitoring | Persist per-call latency + input/output token counts to durable store | **PARTIAL** | Event log (`graph/db.rs`) + `observe.rs` run metrics give durable, append-only per-event traces; TTC tracks compute budget. But there is **no first-class latency/token/$-cost meter** persisted per model call, and no cost aggregation surface. | Yes — add token/latency fields to model-provider call events; cheap. |
| **Online/continuous monitoring, drift & anomaly detection, dashboards, A/B** | Live production monitoring: performance tracking, drift/anomaly detection, A/B of agent versions, alerting, observability dashboards (Grafana/Datadog-style) | **GAP** | `observe.rs` is **pull/offline** (derive traces from a DB snapshot on demand); there is no streaming monitor, no drift detector, no anomaly alerting, no A/B harness comparing agent versions/prompts on a live stream, no dashboard. This is the ABSENT "continuous online monitoring/observability" surface noted in the brief. | Partly — drift/A/B are buildable on top of the event log + eval_harness (compare run cohorts); a live dashboard is a bigger, lower-priority lift. |
| Unified eval/telemetry surface | One coherent place to see eval results + telemetry | **GAP** | Eval lives in Python (`eval/…`), telemetry in Rust (`observe.rs`, `search_telemetry.rs`, `plan_history.rs`); they are **not unified**. No single report/CLI folds "how well did it score" with "how did it get there." | Yes — a thin aggregator CLI over both. Medium value. |
| "Contractor" model (formal contract, negotiation, self-validation, subcontracts) | Move from underspecified prompts to a verifiable contract with deliverables, a negotiation lifecycle, quality-focused self-validation, and hierarchical subcontracts | **PARTIAL** | Self-validation & hierarchical decomposition are strong: `prover/repair.rs`+`retry.rs` (generate→verify→repair), certification/axiom gate = verifiable "deliverable"; `proving/decompose.rs`, `blueprint.rs`, `team.rs` = subcontracts. **Missing**: the *negotiation/clarification* lifecycle (agent flags ambiguity / renegotiates the task before executing) and an explicit machine-readable task "contract" object. | Yes — a statement-validation-time "contract" object + a clarify/abstain-and-ask step. |

---

## Chapter 20 — Prioritization

| Pattern (book) | Book definition | Status | Where / missing | Buildable? |
|---|---|---|---|---|
| Criteria-based task ranking | Rank tasks by urgency/importance/dependencies/cost-benefit | **HAVE** | `search/fitness.rs` (fitness scoring), `search/ttc.rs` (budget-by-difficulty), `curriculum.py` difficulty curriculum, `prover/router.rs` route selection, `search/critic_scorer.rs`/`process_reward.rs`. Richer than the book's P0/P1/P2 LangChain toy. | n/a |
| Dependency-aware scheduling | Order steps by prerequisite dependencies | **HAVE** | Claim DAG + `search/dag_projection.rs`, `team.rs` independent-obligation dispatch, `scheduler`. | n/a |
| Cost/benefit under resource limits | Allocate limited compute to highest-value actions | **HAVE** | `ttc.rs` test-time-compute budgeting by difficulty; `minimize.rs` (minimise proof/obligation); `goal_cache.rs`. | n/a |
| Dynamic re-prioritization | Re-rank as new critical events arrive (falsify-before-prove, failed branch) | **HAVE / PARTIAL** | `falsification.rs` falsify-before-prove reorders work; `retry.rs`/`repair.rs` re-route on failure. Re-prioritisation is *reactive within a run*; there is no cross-run learned prioritiser that updates from historical outcomes. | Minor gap; buildable from `preference_pairs.rs` + telemetry. |

**Verdict: prioritization is essentially HAVE** — the strongest-covered chapter; nothing meaningful missing.

---

## Chapter 21 — Exploration & Discovery

| Pattern (book) | Book definition | Status | Where / missing | Buildable? |
|---|---|---|---|---|
| Hypothesis generation | Propose novel hypotheses/conjectures | **HAVE** | `proving/conjecture_engine.rs` (propose→prove→graduate), `tools/…/funsearch.py` (program/expression search), `search/discovery_game.rs`. Matches Co-Scientist "Generation agent". | n/a |
| Reflection / peer-review of hypotheses | Critically assess correctness/novelty/quality | **HAVE** | `critique/` (guard.rs, critic), `search/critic_scorer.rs`, adversarial critique in `agent.rs`. = Co-Scientist "Reflection agent". | n/a |
| Ranking / tournament (Elo) | Rank hypotheses via tournament/Elo debate | **PARTIAL** | We rank via `fitness.rs` / `process_reward.rs` / `best_first.rs` / `mcts.rs`, and pair up via `preference_pairs.rs`. But there is **no Elo/tournament ladder** over conjectures specifically (book's Ranking agent). | Yes — small: wrap preference_pairs in an Elo table. Low priority. |
| Evolution / refinement of top ideas | Iteratively simplify/synthesise best hypotheses | **HAVE** | `proving/evolve_sketch.rs`, `evolve.py`, `optimize.rs`, `refine_ops.rs`. = "Evolution agent". | n/a |
| Proximity / clustering & novelty | Cluster similar ideas; detect "unknown unknowns"; dedup | **HAVE** | `retrieval/…/novelty.py` (novelty checker), `search/subsumption.rs`, `search/symmetry_dedup.rs`. = "Proximity agent" + subsumption dedup. | n/a |
| Meta-review / cumulative library | Synthesise across reviews; grow a reusable lemma library (LEGO-Prover / AgentRxiv) | **HAVE** | `proving/library.rs` growing lemma library, `mathlib_export.rs`, `orchestration/consolidate.rs`, `method_transfer.rs`. | n/a |
| Multi-agent research hierarchy | Professor/PostDoc/Reviewer role team (Agent Laboratory) | **PARTIAL** | `orchestration/team.rs` (concurrent obligation workers) + `research.rs` exist, but they're a *homogeneous* worker pool, not distinct *role personas* (director / executor / reviewer) with role prompts. | Yes — see #4 spec below (role personas = the coding-agent pattern). |

**Verdict: exploration/discovery is near-complete** — the generate/reflect/evolve/cluster/library loop is all present. Only the *tournament ranking* and *role-differentiated multi-agent team* are thin.

---

## Appendix A/E/G — Prompting, CLI Agents, Coding Agents (the #4 analog)

| Pattern (book) | Book definition | Status | Where / missing | Buildable? |
|---|---|---|---|---|
| System prompting / role prompting | A foundational system prompt sets persona, rules, behaviour for the whole session | **GAP** | No unified system-prompt in Theoremata; behaviour is encoded imperatively in `agent.rs` pipeline + per-tool logic. This is the brief's "no unified system-prompt". | Yes — see #4 spec. |
| Structured output / Pydantic facade | Force JSON, validate into typed objects | **HAVE** | `benchmarks/schema.py`, worker dispatch returns structured JSON; grader/harness emit typed records. | n/a |
| Context engineering / staging area | Meticulously assemble codebase + docs + brief per task into one payload; "primacy of context" | **PARTIAL** | Retrieval component + `research.rs` seed context per obligation; claim-DAG carries state. But there is no explicit per-task **Context Staging Area** ("briefing package" object) and context assembly is code-driven, not a first-class artifact. | Yes — a `TaskContext` object assembling brief + retrieved lemmas + prior attempts. |
| Specialist-agent team (Scaffolder / Tester / Documenter / Optimizer / Reviewer-with-critique-then-reflection) | One frontier model invoked as distinct role personas via role prompts; orchestrator delegates & is final quality gate | **PARTIAL** | The *functions* exist as modules — Scaffolder≈`formal_generate.rs`, Tester≈`falsification.rs`/Lean gate, Documenter≈`exposition.py`, Optimizer≈`optimize.rs`/`minimize.rs`, Reviewer(critique→reflect)≈`critique/`+`repair.rs`. **Missing**: they are hard-wired pipeline stages, not *invokable role personas* selected by a self-directing agent, and there's no shared prompt library. | Yes — #4 spec. |
| Version-controlled prompt library | `/prompts` dir, one md per role, treated as code | **GAP** | No prompt library; prompts are inline in Rust/Python. | Yes — trivial infra; high leverage for #4. |
| **Self-directing coding-agent loop** (Claude Code / Terminus): system prompt + free tool loop + planning/todo + self-correction, agent chooses tools | ACI/tool-loop agent that reads repo, plans, calls tools autonomously, self-corrects, holds a todo | **PARTIAL→GAP** | `orchestration/agent.rs` **is** our "proof-writing agent" (generate→verify→repair, bounded budgets, graceful degradation) and `worker.py` gives a **94-tool** dispatch surface — the *ingredients* exist. But the loop is a **fixed imperative pipeline**, not a model-driven `while(not done){ model picks next tool }` loop with a system prompt, a todo/plan the model maintains, and free tool selection. No **meta-tools** (tools that plan/spawn sub-agents/edit the todo). This is exactly the brief's #4 open question. | Yes — #4 spec below. |
| Agent-Computer Interface / tool availability | Sandboxed, well-described tools the agent selects among | **HAVE (substrate)** | `worker.py` 94-tool dispatch; `router.rs` `ToolAvailability`; Lean/Python/Mathlib tools with sandboxing. The *tools* are there; what's missing is a model-in-the-loop that freely selects among them. | n/a |

---

## What "Theoremata as a self-directing agent" (question #4) concretely requires — spec

The book's Coding-Agents (Appendix G) + Claude-Code CLI (Appendix E) pattern, mapped onto what we lack, gives a precise
target. We already own the **substrate** (94-tool `worker.py` dispatch, `router.rs` tool-availability, `agent.rs`
bounded loop, `observe.rs` trajectory log, verification gates). To become a *self-directing* agent rather than a fixed
pipeline, we need to add, in priority order:

1. **A unified system prompt** (the missing "operating charter"). One versioned prompt establishing the persona
   ("rigorous mathematician; never assert without a verifiable step; falsify before prove; abstain under low
   confidence"), the invariants (Lean compile/axiom gate is the only soundness authority; LLM grades are triage only),
   and the tool-use contract. Today this is scattered across `agent.rs` + tool code.

2. **A model-driven tool loop** replacing the hard-wired pipeline in `agent.rs`: `while not done: model observes state
   → selects a tool from the 94-tool surface → executes → observes result → updates plan`. Keep the current pipeline as
   the *default policy / safety rails*, but let the model choose deviations. `router.rs` becomes advisory, not
   mandatory.

3. **A first-class plan/todo object the model maintains** (extend `plan_history.rs` from a *log* into a *live,
   model-editable* todo the agent reads and rewrites each step) — the book's "Planning pattern" + todo. This is what
   turns reactive re-routing into genuine self-direction.

4. **Meta-tools** (absent today): tools whose job is to manage the loop, not the math — e.g. `update_plan`,
   `spawn_subgoal_agent` (wrap `team.rs`/`decompose.rs`), `request_clarification`/`abstain` (the "contractor"
   negotiation step), `self_review` (invoke critique→reflection over own trajectory). These let the agent restructure
   its own work.

5. **Role personas + a version-controlled prompt library** (`/prompts/*.md`): expose the already-existing
   Scaffolder/Tester/Documenter/Optimizer/Reviewer *functions* as prompt-selectable personas the orchestrator loop can
   invoke, instead of fixed stages. Treat prompts as code (git-versioned), per Appendix G.

6. **A Context Staging Area object** (`TaskContext`): a per-obligation "briefing package" (goal brief + retrieved
   lemmas + prior failed attempts + style/invariants) assembled once and handed to each persona — the book's "primacy
   of context" made concrete.

7. **Trajectory-scored eval loop closure**: feed `observe.rs` spans into a new `eval_harness` trajectory track
   (in-order/any-order/precision/recall vs. a gold action sequence) so the self-directing loop can be *measured and
   improved*, not just run. Without this, a free tool loop is unfalsifiable.

Net: #4 is **not** a from-scratch build — it is a **re-architecture of `agent.rs` from a fixed pipeline into a
system-prompt-anchored, plan-carrying, meta-tool-equipped model loop over the existing 94-tool surface**, plus the
trajectory eval to keep it honest.

---

## TOP 3 GAPS

1. **Self-directing agent loop for #4 (system prompt + model-driven tool loop + live todo + meta-tools).** We have every
   ingredient (94-tool `worker.py`, `agent.rs` loop, `router.rs`, verification gates) but they're wired as a *fixed
   imperative pipeline* with no unified system prompt, no model-editable plan, and no meta-tools. This is the single
   highest-leverage build and the direct answer to open question #4. **Buildable** as a re-architecture, not a rewrite.

2. **Trajectory evaluation + online monitoring / unified telemetry surface.** We capture trajectories richly
   (`observe.rs`, `search_telemetry.rs`, `plan_history.rs`) and score *outcomes* well (`eval_harness`, `proof_grader`),
   but nothing **scores the trajectory itself** against a reference path (book's in-order/any-order/precision/recall),
   there is no online/streaming monitor, drift/anomaly detection, A/B-over-versions, latency/token/cost meter, or a
   *single* surface uniting eval (Python) with telemetry (Rust). **Buildable** on the existing event log.

3. **Role-differentiated multi-agent team + versioned prompt library + Context Staging Area.** The specialist
   *functions* (scaffold/test/document/optimize/review, plus Co-Scientist generate/reflect/rank/evolve) all exist as
   modules, but as hard-wired stages — not invokable role personas selected by an orchestrator, with git-versioned role
   prompts and an explicit per-task "briefing package." Also missing: the "contractor" **clarify/negotiate** step before
   execution. **Buildable**; unlocks both #4 and the Ch.21 research-hierarchy gap.
