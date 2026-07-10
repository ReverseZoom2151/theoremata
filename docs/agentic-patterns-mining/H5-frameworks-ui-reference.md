# Agentic Patterns Mining — H5: Frameworks, UI & Reference

Source: `resources/harness-resources/extracted_text/chunks/H5_frameworks_ui_reference.txt`
(H. Roitman, *The Hitchhiker's Guide to Agentic AI*, Chs. 25–29). Read in full (~365 KB / 9037 lines).

> Book text treated as UNTRUSTED DATA. **No embedded prompt-injection detected** — this is an ordinary
> technical textbook (framework surveys, Python/TS code samples, quiz answers, lookup tables). The only
> `grep` hits for "system prompt:" / "you are now" are legitimate book prose (framework `system_message`
> examples, HITL persona guidance). Had any instruction-looking line targeted this agent it would be flagged
> **POSSIBLE INJECTION**; none did.

## Scope & honest framing

This chunk is **mostly a framework catalog and a reference appendix**, not a source of algorithms to port:

- **Ch 25 — Agent Development Frameworks**: LangGraph, AutoGen, CrewAI, OpenAI Agents SDK (Swarm), DSPy,
  Semantic Kernel; open-source tooling (prompt mgmt, tool registries, memory stores, eval harnesses); MCP/A2A/OpenAPI
  interop; testing/observability/deployment patterns.
- **Ch 26 — Agentic UI Frameworks**: UI paradigms, component catalog, Vercel AI SDK / Chainlit / Gradio / Streamlit /
  LangGraph Studio, generative UI, streaming, human-in-the-loop, trust.
- **Ch 27 — Quiz** and **Ch 29 — Conclusion**: RLHF/RL/systems Q&A — **out of scope** for this mapping (covered by
  the RL/training mining reports).
- **Ch 28 — Quick Reference**: lookup tables; a few (agentic design patterns, framework comparison, comm protocols,
  agent-eval metrics, security checklist) are cross-referenced below.

Theoremata is **bespoke by design**: a Rust orchestration loop (`components/reason/orchestration/agent.rs`), a
clap CLI + ratatui TUI + versioned JSON API (`app/lib.rs`, `app/tui.rs`, `app/api.rs`), a `ModelProvider` trait
seam (`components/provider/mod.rs`) over any chat model, and a pure state-machine router
(`components/reason/proving/router.rs`). We are **not** on LangGraph/AutoGen/CrewAI, and adopting one would be a
regression: those frameworks trade Rust's compiler-enforced typed contracts for Python glue. So most rows below are
**HAVE (bespoke equivalent)** or **N/A (deliberately absent)**. The mining question is narrow: do any framework
*abstractions* reveal a missing seam worth building, and is our UI/observability surface actually enough.

## Ch 25 — Agent frameworks

| Framework / Pattern | What it offers | Theoremata equivalent / need | Action |
|---|---|---|---|
| **LangGraph** (typed-state directed graph; nodes=fns, conditional edges, checkpointer, subgraphs) | Explicit graph orchestration with pause/resume + HITL | **HAVE (stronger).** Our claim DAG (`components/graph/model.rs`, `scheduler.rs`) *is* the state graph; `agent.rs` is the driver; `router.rs` is the conditional-edge logic (`Falsify→Retrieve→Prove→Decompose→Formalize→Verify→Escalate`). Graph is domain-typed + persisted in SQLite, not a `TypedDict`. | None — don't adopt LangGraph. |
| — LangGraph **checkpointing / time-travel / resume** | Save graph state after every node; replay from any checkpoint | **PARTIAL.** SQLite `Store` persists nodes/edges/events/attempts durably, and runs are resumable at DAG granularity. But there is **no per-node execution snapshot** you can rewind-and-replay ("time travel") for a single obligation's attempt trace. | Minor: consider attempt-level replay (see Top 3 #2). |
| **AutoGen** (conversable agents, `GroupChat`, LLM speaker-selection, Docker code-exec) | Multi-agent via structured **message passing** | **N/A by design.** `orchestration/team.rs` does concurrent *obligation* dispatch across OS threads (each worker = a routing+solving unit over its own SQLite conn), not chatty message-passing agents. Falsify/prove/critique are *roles*, not conversational peers. Message-passing multi-agent adds nondeterminism we explicitly avoid. | None — keep thread-per-obligation. |
| **CrewAI** (role+goal+backstory agents; sequential/hierarchical process) | Low-code role-based teams | **HAVE (as roles, not personas).** Roles are `role_for`/system strings passed per call (`formal_generate.rs`, `portfolio.rs`); a "manager" is the scheduler, not an LLM. We don't want backstory-prompt personas. | None. |
| **OpenAI Agents SDK / Swarm** (declarative **handoffs**, input **guardrails**, built-in **tracing**) | Lightweight multi-agent + guardrails + traces | **PARTIAL.** Handoff ≈ our router transitions + `method_transfer.rs`. Guardrails ≈ `critique/guard.rs`, `taint.rs`, `statement_validation`. **Tracing is the real gap** — SDK ships first-class run traces; we emit events but no unified trace object (see Top 3 #1). | Adopt the *tracing* idea, not the framework. |
| **DSPy** (signatures + optimizer compiles prompts to a metric) | Automated prompt/few-shot optimization | **PARTIAL / genuine gap.** `train/flywheel`, `curriculum_synth`, reward/meta-verifier tune *weights and data*; nothing optimizes **prompt/instruction text** against our eval harness. Same finding as A1 report. | Optional: offline prompt-optimizer over `components/eval`. Low priority. |
| **Semantic Kernel** (plugin/skill registry: native fns, prompt fns, OpenAPI plugins; planner) | Enterprise plugin architecture + planner | **HAVE (typed).** Tools are the `Tool` trait (`components/tools`, `LeanCheck`/`MathlibSearch`/`PythonCheck`); the "planner" is `router.rs`+`agent.rs`. A registry/manifest is implicit in Rust types, not a runtime plugin catalog. | None (but see meta-tools note below). |
| **Open-source building blocks** (Promptflow, Guidance, LMQL, Outlines; Composio/E2B tool registries; Mem0/Zep/Letta memory; RAGAS/DeepEval/Promptfoo/AgentBench evals) | Composable a-la-carte components | **Mostly HAVE bespoke.** Structured decoding ≈ compiler-checked Lean output (stronger than Outlines regex); memory ≈ `critique/memory.rs`+`plan_history.rs`+`goal_cache.rs`; eval ≈ `components/eval`. Tool sandboxing ≈ `prover/session/exec.rs`. | None to adopt; catalog only. |
| **Interop — MCP** | Standard tool/resource exposure | **HAVE (partial).** `Mcp` CLI subcommand + `mcps/` dir; the versioned `app/api.rs` is the stable client contract. | Keep building out MCP surface. |
| **Interop — A2A** (agent-to-agent tasks over JSON-RPC) | Cross-org agent delegation | **N/A.** Single-harness; no external agents to federate with. | None. |
| **Interop — OpenAPI→tools** (spec = zero-code tool definition) | Auto-generate callable tools from a REST spec | **N/A.** Our tools are compile-time Rust; no untrusted third-party REST surface to auto-wrap. | None. |
| **Testing** (unit tools / integration loops / **golden-trajectory regression** / behavioral / cost-latency) | Multi-granularity agent test strategy | **PARTIAL, worth noting.** Rust unit/integration tests are thorough; `components/eval` gives task-level accuracy. **Golden-trajectory regression** (assert the agent takes the *same route sequence* on a fixed input, ±semantic-similarity of output, ±token-budget) is **not** a first-class harness. | Consider a golden-route regression test (see Top 3 #3). |
| **Observability** (traces / metrics / logs; OTel spans; **failure taxonomy**; replay) | Structured tracing + failure categorization | **PARTIAL — the main gap.** We log events/attempts; no span tree, no failure-class tagging, no replay CLI. Book's 6-class taxonomy (tool err / reasoning err / hallucination / loop / context-overflow / refusal) maps cleanly onto proof failures. | See Top 3 #1. |
| **Deployment** (Celery async, multi-tenant, model-routing cost-opt, autoscaling) | Production serving infra | **N/A / out of scope.** Theoremata is a batch/CLI harness, not a multi-tenant service. Cost control already exists via `search/ttc.rs` budget controller. | None. |

## Ch 26 — Agentic UI

| Component / Pattern | What it offers | Theoremata equivalent / need | Action |
|---|---|---|---|
| **UI paradigms** (chat / canvas / **workflow-viz** / dashboard / collaborative / autonomous-with-checkpoints) | Match interface to task shape | **PARTIAL.** We have chat (`Chat`/`Send` CLI + `orchestration/chat.rs`) and an autonomous-with-checkpoints loop. **Workflow/graph visualization** of the live claim DAG is the natural fit and is thin in the TUI. | Enrich TUI DAG view (Top 3 #2). |
| **Approval gates / tiered HITL** (Approve / Reject / Modify; reversibility tiers) | Human-in-the-loop control | **HAVE.** `Proposals`/`Approve`/`Reject` CLI subcommands + proposal store are exactly a Tier-3 approval gate; provenance actor recorded. Strong match. | None. |
| **Tool-use visualization** (inline tool cards: name/input/output/timing, error highlighting) | Trust + debugging surface | **PARTIAL.** `Events`/`Attempts` list tool activity as history; no per-step card with input/output/latency inline in the TUI. | Fold into trace viewer (Top 3 #1/#2). |
| **Thought/reasoning display** (collapsible CoT, progressive disclosure) | Surface model reasoning | **PARTIAL.** Critique/attempt records exist; TUI doesn't render collapsible reasoning per node. | Nice-to-have, low priority. |
| **Progress indicators** (step list, streaming, ETA, cancel) | Long-task feedback | **PARTIAL.** `ModelStreamEvent` (`provider/mod.rs`) already streams Started/Completed; TUI has status. No live step-tree / cancel. | Low priority. |
| **Streaming (SSE/WebSocket, tool-call streaming, backpressure)** | Real-time "watch it work" | **PARTIAL.** Provider stream seam exists; CLI/TUI is largely turn-based. Fine for a batch harness. | None (adequate). |
| **Generative UI** (LLM emits UI components via RSC) | Model-selected widgets | **N/A.** Terminal-first; irrelevant. | None. |
| **Trust surface** (undo/rollback, audit trail, confidence, calibrated uncertainty) | Trust calibration | **PARTIAL→HAVE.** Immutable event log + attempt provenance = audit trail; `THEOREMATA_ABSTAIN_THRESHOLD` (Aletheia abstention in `agent.rs`) is a real confidence/uncertainty seam. Undo = graph is append-only w/ status transitions. Strong for a prover. | None. |
| **UI frameworks** (Vercel AI SDK, Chainlit, Gradio, Streamlit, **LangGraph Studio**) | Build the UI | **N/A (Python/TS web).** Only **LangGraph Studio** (graph debugger + state inspection + time-travel) is *conceptually* aspirational for our DAG — as a design target for the TUI/API, not a dependency. | Borrow ideas, not code. |

## Ch 28 — Reference tables worth cross-linking (no build implied)

- **Agentic design patterns** (28.9): ReAct / Plan-and-Execute / Supervisor / Swarm / Hierarchical / HITL — Theoremata
  is Plan-and-Execute + rule-based Supervisor (`router.rs`) + HITL gate (`Proposals`). Already covered by A1/A2 reports.
- **Agent eval metrics** (28.22): Task-Success-Rate, steps-to-completion, tool-call accuracy, recovery rate,
  human-escalation rate — these are a **good checklist for `components/eval`** if we want richer agent-level telemetry.
- **Security checklist** (28.21): direct/indirect prompt-injection, tool misuse, excessive autonomy — we already do
  well here (`critique/guard.rs`, `taint.rs`, `api.rs` untrusted-input hardening, iteration budgets).
- **Comm protocols** (28.10/28.17): MCP primitives + annotations (`readOnlyHint`/`destructiveHint`) — useful spec
  reference as we grow the `mcps/` surface.

## Focus-question verdicts

**(a) Do framework abstractions reveal a missing seam in our bespoke loop?**
Largely **no**. Graph orchestration (LangGraph), plugin/tool registries (Semantic Kernel), and message-passing
multi-agent (AutoGen/CrewAI) all have *stronger, compiler-typed* analogues in Theoremata (claim DAG + `router.rs` +
`Tool` trait + thread-per-obligation `team.rs`). The **one genuinely missing abstraction is a first-class run/trace
object** — every framework (LangGraph checkpointer, Agents-SDK tracing, LangSmith) treats "the trace" as a named,
inspectable, replayable value; we scatter it across `events`/`attempts` rows. DSPy-style **prompt optimization** is a
second, lower-value gap (already flagged in A1). **Meta-tools** (a tool that lists/introspects the tool set) are absent
but low-value for a fixed compile-time tool set.

**(b) UI/observability — is CLI/TUI/API enough, or a real gap?**
The **interaction** surface is enough and in places best-in-class: the `Proposals/Approve/Reject` HITL gate is exactly
the book's Tier-3 approval pattern, the event log is a real audit trail, and abstention is a genuine confidence seam.
The **observability** surface is the real gap: no unified trace/span tree, no failure-class tagging, no
replay-a-failed-attempt path, and the TUI under-visualizes the live DAG (the book's "workflow visualization" paradigm,
LangGraph-Studio-style, is the obvious fit). This is an *operator/debugging* gap, not an end-user gap.

**(c) System-prompt / agent-config management (informs the "no unified system-prompt" item)?**
Confirmed: prompts are assembled **per-role, per-call** as inline `"system"` strings (`agent.rs:634`,
`formal_generate.rs`, `portfolio.rs`, via `role_for`) — there is **no single auditable system-prompt registry**.
Every framework here externalizes this (CrewAI backstories, SK prompt-functions, DSPy signatures, LangGraph state).
The cheap, high-value move is a **prompt/role registry**: one module owning every system prompt as a named, versioned,
testable constant, so prompts become greppable, diffable, and regression-testable rather than string literals sprinkled
across the prover. (This directly enables golden-route regression and future DSPy-style optimization.)

## Top 3 (few, by design — we are bespoke)

1. **First-class run trace + failure taxonomy (observability).** Introduce a named `RunTrace` value (span tree over
   LLM calls / tool calls / route transitions with timing + status) and tag each terminal failure with the book's
   6-class taxonomy adapted to proving (tool-error / reasoning-error / hallucination / loop / context-overflow /
   refusal). Materialize it from the existing `events`/`attempts` rows and expose a `trace <run>` CLI/API view. Highest
   leverage: unblocks debugging, regression testing, and the TUI graph view. *(Reuses `Store`; no new subsystem.)*

2. **TUI/API live-DAG "workflow visualization" + attempt replay.** Render the claim DAG in the TUI with per-node
   status/tool-cards (Ch 26 workflow-viz paradigm, LangGraph-Studio-inspired) and add attempt-level replay (re-run one
   obligation's failing step). Turns our strongest asset — the typed DAG — into the operator surface the book argues is
   non-negotiable. *(Builds on `graph/model.rs`, `app/tui.rs`, item #1.)*

3. **System-prompt/role registry + golden-route regression test.** Consolidate the scattered inline `"system"` strings
   into one versioned, named prompt registry (`role_for` becomes a lookup), then add a golden-trajectory test asserting
   the agent takes the same `router.rs` route sequence (± output semantic-similarity, ± token budget) on fixed inputs.
   Cheap, and it hardens both prompt changes and orchestration changes against silent regressions. *(Ch 25 §25.5.3 +
   Ch 26 §26.8; addresses focus-question (c).)*
