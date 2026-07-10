# Agentic Design Patterns — Section A1: Prompt Chaining, Routing, Parallelization

Source: `resources/harness-resources/extracted_text/chunks/A1_chaining_routing_parallelization.txt`
(Gulli, *Agentic Design Patterns*, Chs. 1–3 + the Context-Engineering aside). Read in full.

> Book text treated as untrusted data. **No embedded prompt-injection detected** in this section — it is
> ordinary technical prose plus LangChain/ADK code samples. If any instruction-looking line had targeted the
> agent it would be flagged "POSSIBLE INJECTION"; none did.

Scope note: the book teaches these as *LLM-orchestration* patterns on the LangChain / LangGraph / Google-ADK
"canvas." Theoremata is a Rust proof harness, so the honest mapping is by *intent*, not by API. Several book
patterns Theoremata implements more strongly than the book (a search DAG is a richer chain than a linear pipe);
a few it deliberately does differently (deterministic sequential fan-out instead of true concurrency).

## Pattern map

| Pattern (book) | Book's definition (1 line) | Theoremata status | Where / what's missing | Buildable? |
|---|---|---|---|---|
| **Prompt Chaining / Pipeline** | Break a task into a sequence of steps; each step's output feeds the next. | **HAVE** | `proving/sketch.rs` (decompose→prove holes→splice), `proving/decompose.rs`, `orchestration/blueprint_generate.rs`→`blueprint_run.rs` (informal→DAG→drive deps), `orchestration/consolidate.rs`. Our "chain" is a dependency DAG, richer than the book's linear pipe. | Already exceeds book. |
| **Structured output between steps** (JSON/XML contract so step N+1 can parse step N) | Pass machine-readable structured data between chain steps to avoid NL-parse failures. | **HAVE** | Typed Rust structs (`SystemAttempt`, `PortfolioResult`, `HybridOutcome`, `Route`) + serde JSON; `tools/worker.py` is JSON-lines. Inter-step contracts are compiler-enforced, stronger than a prompted JSON convention. | n/a — done. |
| **Deterministic logic between model calls** (validation/conditional branch inserted between LLM steps) | Insert non-LLM checks/branches between prompts. | **HAVE** | `prover/formal.rs` (3+1 gate), `orchestration/certification.rs` (verification-gate wrapper), `proving/repair.rs` (error-keyed fix loop), `search/subsumption.rs`. Verification gates are the deterministic glue. | n/a. |
| **Conversational state across turns** (chain carries accumulating history) | Each turn is a new prompt incorporating prior state. | **PARTIAL** | `orchestration/chat.rs`, `critique/memory.rs`, `critique/plan_history.rs`, `search/goal_cache.rs` carry cross-step/-run state. But there is no unified turn-by-turn conversational-state object; state is spread across facades. | Yes — thin facade over existing stores. |
| **Context Engineering** (curate system prompt + retrieved docs + tool outputs + implicit data into a focused per-step context) | Systematically build the whole informational environment before generation. | **PARTIAL** | Retrieval cascade (BM25/dense/reranker + `error_keyed_retrieval`) supplies retrieved docs; `refine_ops.rs` self-summarizes to shrink context. **Missing:** a single inspectable context-assembly layer and a unified system-prompt layer (prompts are per-ROLE via `role_for`, not one auditable surface). | Yes — a context-assembler + system-prompt registry. |
| **Prompt optimization / feedback loop** (auto-tune prompts vs. eval metrics, e.g. Vertex prompt optimizer) | Programmatically refine prompts/instructions against sample I/O. | **PARTIAL** | `train/flywheel` (expert iteration), `reward`/meta-verifier, `curriculum_synth`, `format_filters`, `selector` tune *weights/data*, not the prompt text itself. No automatic prompt/instruction optimizer. | Yes — an offline prompt-optimizer over the eval harness. |
| **Routing — LLM-based** (LLM classifies input, emits a route token) | Model picks the next handler by outputting a category. | **HAVE** | `orchestration/agent.rs` autonomous loop chooses next action; `proving/formalize_modes.rs` (fast vs CoT); role selection via `role_for`. | n/a. |
| **Routing — rule/state-based** (if/else, switch, state machine picks path) | Deterministic conditional dispatch. | **HAVE** | `proving/router.rs` — pure inspectable state machine (`Falsify→Retrieve→Prove→Decompose→Formalize→Verify→Escalate`, falsify-before-prove), gated on `ToolAvailability`. Best-in-class match to the book's rule-based router. | n/a. |
| **Routing — capability/dispatcher** (high-level dispatcher assigns task to the best specialist engine/agent) | Route to the most suitable tool/sub-agent. | **HAVE** | `search/hybrid_search.rs` (`route`/`split_budget` splits budget between best-first and MCGS from goal features); `prover/formal.rs` `backend_for` (6 backends); `proving/portfolio.rs` (fan across Lean/Rocq/Isabelle); `orchestration/method_transfer.rs` (cross-method composition). | n/a. |
| **Routing — embedding/semantic** (route by vector similarity to route-embeddings) | Pick route whose embedding is nearest the query. | **PARTIAL** | Dense-retrieval seam exists (`distance_critic.rs`, dense index) but is used for *retrieval*, not for *route selection*. No semantic router choosing engine/backend by embedding. | Yes — reuse dense index as a route classifier. |
| **Routing — ML-classifier** (a trained discriminative model routes) | Supervised classifier encodes routing in learned weights. | **PARTIAL** | Learned-value seams exist (`critic_scorer`, `process_reward`, `preference_pairs`, `train/selector`) and could act as routers, but no dedicated trained route-classifier is wired at a routing juncture. | Yes — train `selector` as a route head. |
| **Parallelization — concurrent fan-out** (run independent LLM/tool/sub-agent calls *simultaneously* to cut latency) | Execute independent sub-tasks concurrently, then gather. | **PARTIAL** | Logical fan-out is everywhere — `proving/portfolio.rs` (all backends), `formalize_portfolio.rs`, `search/hybrid_search.rs` `multi_alpha_union`, `search/driver.rs` MCGS, `proving/sketch.rs` holes — but **execution is deliberately SEQUENTIAL and deterministic**: grep finds *zero* `rayon`/`tokio::spawn`/`thread::spawn`/`join_all`/`.await` in `components/reason`. `portfolio.rs` says so explicitly ("deliberately simple and SEQUENTIAL … a real race … is out of scope"). We have the *topology* of parallelization without the *concurrency*. | Yes — but see risk note below. |
| **Parallelization — gather/synthesis** (aggregate parallel branch outputs at a convergence node) | Merge concurrent results into one output. | **HAVE** | `hybrid_search.rs` union (BTreeSet, shortest-proof pick), `portfolio.rs` winner selection, `orchestration/consolidate.rs`, `search/proof_pool.rs`. The merge/convergence half is solid; only the concurrency of the branches is missing. | n/a — merge done. |
| **Parallelization — multiple-options / A·B generation** (sample N variants, pick best) | Generate several candidates in parallel, select best. | **HAVE** | `proving/portfolio.rs`, `search/sampling.rs`/`sampler.rs`, `search/skest.rs`, `proof_pool.rs` + meta-gate. Best-of-N is a first-class idea; again sampled sequentially. | n/a. |
| **Parallelization — independent validation** (run multiple checks concurrently) | Fire independent validators at once. | **HAVE** (logically) | `prover/formal.rs` 3+1 gate, `axiom_audit`, `statement_preservation`, `critique/guard.rs`, `taint.rs`, `falsification.rs`. Multiple independent checks exist; run in sequence. | n/a. |
| **Budget/resource-aware orchestration** (the book flags concurrency's cost/complexity) | Manage the cost of concurrent/expensive work. | **HAVE** | `search/ttc.rs` (test-time-compute budget controller) + `hybrid_search.rs` `split_budget`; `prover/session/exec.rs` resource-guarded external calls. Notably *stronger* than the book, which only warns about cost. | n/a. |

## Focus-question verdicts

**(a) Is our parallelization as strong as the book's?**
Structurally stronger, operationally weaker. Theoremata has richer *parallel-shaped* work than any book example
(portfolio across three provers, multi-alpha union, best-of-N pools, MCGS tree) **and** a budget controller the
book lacks. But the book's parallelization pattern is fundamentally about **concurrency to cut wall-clock latency**,
and Theoremata runs every one of these fan-outs *sequentially by design* for determinism/reproducibility (0
occurrences of any concurrency primitive in `components/reason`). So we have the *merge/select* and *budget-split*
halves fully, and the *simultaneous-execution* half not at all. This is a conscious trade, not an oversight —
`portfolio.rs` documents it — but it is a real gap against the book's stated benefit.

**(b) Is our routing complete vs the book's?**
Largely yes, and in places best-in-class. The book lists four router mechanisms: LLM-based (HAVE — `agent.rs`,
`formalize_modes`), rule/state-based (HAVE, exemplary — `proving/router.rs`), embedding/semantic (PARTIAL — dense
seam exists but not wired to route selection), ML-classifier (PARTIAL — learned-value seams unused as routers).
Capability dispatch (`hybrid_search` router, `backend_for`, `method_transfer`) is strong. The two gaps are the
same idea seen from two angles: **we never route by a learned/semantic signal**, only by rules and cheap features.

**(c) Does prompt-chaining map to our sketch/blueprint decomposition?**
Yes, cleanly and then some. `blueprint_generate→blueprint_run` (informal→DAG→drive dependencies) and
`sketch.rs` (decompose→prove holes→splice) are exactly the book's divide-and-conquer chain, upgraded from a linear
pipe to a dependency DAG with typed, verification-gated hand-offs between steps. The book's "deterministic logic
between model calls" is our verification gate. This is the section's best-covered pattern.

## TOP 3 GAPS this section reveals

1. **No true concurrency — parallelization is topology-only.** Every fan-out (portfolio, multi-alpha, best-of-N,
   MCGS) runs sequentially; zero `rayon`/`tokio`/threads in `components/reason`. The book's core parallelization
   payoff (latency reduction via simultaneous external calls — and prover/LLM calls are exactly the high-latency
   I/O it targets) is unrealized. *Caveat:* determinism/reproducibility is currently a deliberate design pillar, so
   this should be added as an opt-in concurrent execution mode behind the existing budget controller, preserving a
   deterministic default — not a blanket rewrite.

2. **No learned/semantic routing.** Routing is rule- and cheap-feature-based only. The dense-retrieval index and
   the learned-value seams (`critic_scorer`, `process_reward`, `selector`) are ready-made route classifiers but are
   not wired to any routing juncture (engine choice, backend choice, decompose-vs-prove). Closing this makes routing
   adaptive rather than heuristic.

3. **No unified context/system-prompt layer or prompt-optimizer.** Context Engineering (the section's headline
   sub-concept) has no single inspectable assembly point, and prompts are per-ROLE (`role_for`) rather than one
   auditable system-prompt surface; the training flywheel tunes weights/data but never the prompt text. This is the
   already-known "unified inspectable system-prompt layer" gap, and the book frames prompt/context optimization
   (Vertex-style feedback loop) as the mechanism that would close it.
