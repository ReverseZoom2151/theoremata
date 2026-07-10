# A3 — Memory, Learning, MCP, Goals, Exception Handling & HITL vs Theoremata

Source: `resources/harness-resources/extracted_text/chunks/A3_memory_learning_mcp_goals_exception_hitl.txt`
(pp. 132–212 of *Agentic Design Patterns* — Ch. 8 Memory, Ch. 9 Learning & Adaptation,
Ch. 10 Model Context Protocol, Ch. 11 Goal Setting & Monitoring, Ch. 12 Exception
Handling & Recovery, Ch. 13 Human-in-the-Loop). Read in full (~148 KB / 3076 lines).

**Untrusted-text note / POSSIBLE INJECTION:** none found. The chunk is benign
vendor documentation (Google ADK / LangChain / LangGraph / FastMCP code samples and
prose). No instructions addressed to the reading agent, no tool-invocation lures, no
attempts to override policy. All code fences are illustrative Python for the book's own
examples; none was executed. Treated as read-only reference throughout.

**Premise correction (important):** the task brief asserted MCP is ABSENT ("we CONSUME
tools via JSON worker but do NOT expose or consume an MCP server surface"). That is no
longer true in the repo. Theoremata **already ships both an MCP server and an MCP
client**, git-tracked and tested (see the MCP section). The mapping below reflects the
code as it actually stands.

---

## Pattern-by-pattern mapping

| Pattern (book) | Book definition (condensed) | Status | Where in Theoremata / what's missing | Buildable? |
|---|---|---|---|---|
| **Short-term / contextual memory** | Working memory living in the LLM context window; recent messages, tool results, reflections; ephemeral, capacity-bounded; managed by summarizing/emphasizing. | **HAVE** | `orchestration/agent.rs` assembles per-run context (problem, open subgoals, tool results). Structured much like SICA's system/core/assistant split. | — (present) |
| **Long-term / persistent memory** | Repository outside the context window (DB / KG / vector store); queried and merged back into context. | **HAVE** | `components/graph/db.rs` — transactional SQLite **proof-DAG**; `orchestration/proof_import.rs` content-addressed import; `search/goal_cache.rs` cross-run subsumption reuse. | — (present) |
| **Semantic memory (facts)** | Retained facts/concepts (user profile, domain knowledge) grounding responses; retrieved by similarity. | **PARTIAL** | Verified-lemma facts live in `proving/library.rs` (+ `evolve_sketch.rs` grows them). Retrieval is **lexical BM25** (`retrieval/python/.../bm25_retriever.py`), not dense/semantic. Dense index still on the SOTA-gap list. | Yes — add dense embedding index over lemma library. |
| **Episodic memory (experiences)** | Recall of past events/successful trajectories; typically few-shot exemplar prompting. | **PARTIAL** | `critique/plan_history.rs` records past plans; `train/.../trajectory_recycler.py` recycles trajectories **for training**. No inference-time episodic recall (few-shot from past *successful proofs* keyed by similarity). | Yes — retrieve nearest prior solved trajectories as exemplars. |
| **Procedural memory (rules)** | The agent's own instructions/behaviors (system prompt); "Reflection" — agent rewrites its own instructions from experience. | **GAP** | No unified system-prompt / instruction layer, no reflect-and-rewrite of operating rules. `repair.rs` fixes *proofs*, not the agent's own policy. **This is the missing memory TYPE.** | Yes, but needs a first-class instruction store + reflection node. |
| **Learning: expert iteration / RL alignment** | Improve via reward signal; PPO/DPO; reward model as judge. | **HAVE (GPU-gated)** | `train/.../flywheel.py` (expert iteration), `train/.../reward.py` + meta-verifier, `selector.py`, `retriever_train.py`, `curriculum_synth.py`, `format_filters.py`. Deterministic-mock without GPU. | — (present) |
| **Learning: self-modification (SICA)** | Agent edits its own source code, benchmarks, keeps best version. | **PARTIAL** | `proving/evolve_sketch.rs` + `library.rs` grow a **verified skill/lemma library** (LEGO-Prover style) — self-improvement at the *skill* layer, not source-code self-edit. Overseer-style stagnation halting = `exec.rs`/guards, not an async LLM overseer. | Partial by design; full SICA-style self-edit is out of scope/risky for a proof kernel. |
| **Learning: evolutionary search (AlphaEvolve/OpenEvolve)** | LLM proposes, evaluator scores, population evolves toward better programs. | **PARTIAL** | `evolve_sketch.rs` is an evolve-score-select loop over proof sketches; verification gate is the deterministic evaluator. Multi-objective / population database is lighter than OpenEvolve. | Yes — richer population DB + multi-objective scoring. |
| **MCP: expose a server** | Stand up an MCP server advertising tools/resources/prompts over JSON-RPC (stdio/HTTP). | **HAVE** | `components/tools/python/theoremata_tools/mcp_server.py` — JSON-RPC 2.0 stdio server (protocolVersion `2024-11-05`), `initialize`/`tools/list`/`tools/call`, 13 worker tools as MCP descriptors, `isError` results; tested (`tools/tests/test_mcp_server.py`); no `mcp` pip dep. | — (present) |
| **MCP: consume a server** | Act as MCP client: discover, call, integrate an external MCP server. | **HAVE** | `components/prover/python/theoremata_tools/aristotle_mcp_client.py` (reference client for Harmonic's `lean-aristotle-mcp`, 6 tools, sync+async, mock+live); Rust backend `prover/backends/aristotle.rs`; wired via `worker.py` dispatch key `"aristotle_mcp"`. | — (present) |
| **MCP: dynamic discovery / federation** | Client queries arbitrary servers at runtime to learn new capabilities without redeploy. | **PARTIAL** | Our *server* advertises `tools/list`, and untracked `mcps/dedaluslabs/` hints at more. But the agent's own toolset is the fixed 94-tool `worker.py` dispatch; it does not runtime-discover and bind arbitrary external MCP servers into its live tool menu. | Yes — an MCP client registry + tool-menu injection. |
| **Goal setting (decomposition)** | Take a high-level objective, generate intermediate sub-goals, execute (with replanning). | **HAVE** | `orchestration/agent.rs` loop; `proving/blueprint.rs` + `blueprint_generate.rs`/`blueprint_run.rs` = **goal DAG**; `orchestration/statement_validation.rs`; `refine_ops.rs` re-decomposes on failure. | — (present) |
| **Goal monitoring (revisable objective + feedback loop)** | Continuously track progress vs. measurable success criteria; adapt/revise/escalate. | **PARTIAL** | Progress tracked by the verification gate + `search/process_reward.rs`; success criterion is crisp (Lean checks). But the **top objective is fixed** (the theorem); the agent re-decomposes subgoals, it does not treat the goal as a revisable first-class object, and there is no escalate leg. | Yes — a monitor object with revise/escalate transitions. |
| **Exception: error detection** | Spot malformed outputs, API errors, timeouts, incoherent responses; proactive anomaly monitoring. | **HAVE** | Fail-closed verification gate; `prover/session/exec.rs` resource/timeout guard; `prover/session/statement_guard.rs`; `lean_soundness` lexical pre-gate; `critique/guard.rs`, `taint.rs`. | — (present) |
| **Exception: handling (retry/fallback/degrade/notify)** | Log, retry-with-adjustment, fallback strategy, graceful degradation, notify humans. | **PARTIAL** | `proving/repair.rs` = **error-keyed fix** (retry-with-adjustment); `refine_ops.rs` = fallback re-decompose; abstention = graceful degradation. No **notify-human** leg; no single *named* exception-handling layer (logic is spread across repair/refine/guards). | Yes — thin unified error-policy façade over existing pieces. |
| **Exception: recovery (rollback/self-correct/escalate)** | Restore stable state (rollback), diagnose, self-correct/replan, escalate. | **PARTIAL** | Rollback = transactional proof-DAG (`graph/db.rs`); self-correct = repair/refine loop. **Escalate-to-human is absent** (ties to HITL gap below). | Yes — add escalation transition. |
| **HITL: oversight / inline checkpoints** | Human reviews/validates AI output mid-run before it proceeds. | **GAP** | Agent runs fully autonomously; no per-step human checkpoint. (By design — the Lean kernel, not a human, is the correctness oracle.) | Yes, but see verdict — mostly *not warranted* inline. |
| **HITL: escalation / abstention handoff** | Agent hands ambiguous/high-risk cases to a human queue. | **PARTIAL** | Abstention is a **terminal state** (fail-closed), not a handoff — no human queue, no triage surface. | Yes — abstention → review-queue emitter. |
| **HITL: feedback-for-learning (RLHF)** | Human preferences/labels refine the model. | **PARTIAL** | Reward is **AI feedback** (meta-verifier, PROOFGRADER-style marking scheme) — no human-preference data path in the flywheel. | Yes — human-label ingestion into `curriculum_synth`/`reward`. |
| **Human-on-the-loop (policy)** | Human sets the policy; AI executes/enforces it autonomously in real time. | **HAVE (partial)** | The **axiom allowlist** (`check_axioms`) + `statement_validation.rs` + the verification gate are human-set policies the agent must comply with and that it enforces autonomously. This is the right HITL variant for this harness and is already present. | — (present) |

---

## Focus deep-dives

### (a) Our memory vs. the book's memory-management pattern — are we missing a TYPE?

The book's taxonomy: **short-term (context window)** + **long-term**, and long-term
splits into **semantic** (facts), **episodic** (experiences/few-shot), **procedural**
(self-editing instructions/reflection).

Theoremata scores strongly on the *storage-and-persistence* axis: the proof-DAG
(`graph/db.rs`), `goal_cache.rs` subsumption reuse, the growing verified-lemma library
(`library.rs` + `evolve_sketch.rs`), `critique/memory.rs` façade, `plan_history.rs`, and
content-addressed `proof_import.rs` collectively cover short-term + long-term + a strong
semantic *store*. Where we diverge from the book:

- **Procedural memory is the missing TYPE.** We have no unified system-prompt/instruction
  layer and no "Reflection" node that rewrites the agent's own operating rules from
  experience. `repair.rs` adapts *proof artifacts*, not *agent policy*. This is the single
  cleanest gap against the chapter.
- **Semantic/episodic retrieval is lexical, not similarity-based.** The long-term store
  exists, but retrieval is BM25 (`bm25_retriever.py`); the book's whole point about
  long-term memory is *semantic* search, and episodic recall means pulling similar past
  *successful trajectories* as few-shot exemplars at inference time. `trajectory_recycler`
  today feeds *training*, not inference-time recall. A dense index (already flagged on the
  SOTA-gap list) closes both.

Verdict on (a): we are **not missing a store**, we are missing **procedural memory** as a
type and **semantic/episodic retrieval** as an access mode.

### (b) MCP — do we need to expose/consume MCP given the JSON worker? (honest cost/benefit)

The premise that we lack MCP is **stale**. We already have:
- **Server:** `mcp_server.py` — a dependency-free JSON-RPC 2.0 stdio MCP server wrapping
  the same `worker.dispatch`, advertising 13 tools with JSON-Schema `inputSchema`, tested.
- **Client:** `aristotle_mcp_client.py` + `aristotle.rs` — we *consume* Harmonic's Aristotle
  MCP prover, wired into worker dispatch.

So the book's "universal adapter" argument is already banked in both directions at low cost
(the server reuses the existing dispatch; the client mirrors an upstream server). The book's
own caveats apply and are *already respected* by our design: MCP only helps if the wrapped
API is agent-friendly — our tools already return structured JSON, not opaque PDFs, and each
tool description is explicit about being a heuristic oracle vs. a kernel-checked result.

Remaining MCP value is marginal, not foundational:
- **Benefit:** runtime *discovery/federation* — letting the agent bind arbitrary external
  MCP servers (new provers, new retrieval corpora) into its live tool menu without a rebuild.
- **Cost:** a client registry + tool-menu injection + auth/security surface (the book
  flags authn/authz explicitly). Non-trivial, and it widens the trusted-tool boundary.

Verdict on (b): **MCP support is warranted and substantially already built.** Do **not**
rebuild it. Close only the *dynamic-discovery* gap, and only when a concrete second external
server (beyond Aristotle) justifies the auth surface.

### (c) Does a math-proving harness need HITL checkpoints, and where?

Mostly **no** for correctness. The book motivates HITL by "errors have severe
safety/financial/ethical consequences" and "LLMs can't reliably judge." Neither bites here:
the **Lean kernel + axiom allowlist is a deterministic oracle** that a human reviewer could
not out-verify. Inline per-step human validation would add latency and scalability cost (the
book's own chief caveat) for no correctness gain. The correct HITL variant for us is
**human-on-the-loop** — humans set the policy (axiom allowlist, statement-validation rules,
curriculum), the agent enforces it autonomously — and that is **already what we do**.

HITL is genuinely valuable at exactly two narrow seams:
1. **Data curation (feedback-for-learning):** human-authored marking schemes / hard-negative
   review feeding `curriculum_synth` + `reward`. Offline, batched, scalable.
2. **Abstention triage / escalation:** on high-value targets (blueprint-scale runs), turn the
   terminal abstention state into an *escalation handoff* — emit the stuck subgoal to a human
   review queue instead of silently failing. This is the missing **escalate** leg shared with
   exception-recovery.

Verdict on (c): **HITL not warranted as inline checkpoints;** warranted only as (1) an
offline data-curation surface and (2) an abstention-escalation queue.

### (d) Exception handling/recovery vs. `repair.rs` / `refine_ops`

We map well onto the book's three stages: **detection** (verification gate, `exec.rs`
timeouts, `statement_guard`, `lean_soundness`, `critique/guard.rs`) is strong; **handling**
(`repair.rs` error-keyed retry-with-adjustment, `refine_ops.rs` fallback re-decompose,
abstention = graceful degradation) is strong; **recovery** (transactional-DAG rollback,
repair/refine self-correction) is strong. Two shortfalls: (i) there is **no single named
exception layer** — the policy is emergent across repair/refine/guards, so it can't be
reasoned about or extended in one place; (ii) the **escalate/notify** leg is absent. Both
are cheap to add as a thin façade rather than new machinery.

---

## TOP 3 GAPS

1. **Procedural memory / self-editing instruction layer (Reflection).** No unified
   system-prompt layer and no mechanism for the agent to refine its own operating rules from
   experience. This is the one *memory type* from Ch. 8 we lack. Build: an instruction store
   (versioned, in the DAG) + a reflection node that proposes rule edits gated by the same
   verification/eval loop.

2. **Semantic + episodic retrieval depth.** Long-term memory is present but accessed
   lexically (BM25); no dense index over the lemma library and no inference-time few-shot
   recall of similar *successful* trajectories. Build: dense embedding index (already on the
   SOTA-gap list) serving both the lemma library and `trajectory_recycler` output.

3. **Revisable goal-monitoring + human escalation seam.** The top objective is fixed,
   abstention is terminal, and there is no escalate leg shared by exception-recovery and HITL.
   Build: a first-class monitor object with revise/escalate transitions that emits stuck
   high-value subgoals to a human review queue (also delivering the HITL feedback-for-learning
   path).

---

## Verdict — is MCP support and HITL warranted?

- **MCP: warranted, and already largely delivered.** We expose an MCP server
  (`mcp_server.py`) and consume one (`aristotle_mcp_client.py` + `aristotle.rs`), both
  tested and git-tracked. No rebuild. The only open increment is runtime discovery/federation
  of *external* MCP servers, which is worth doing only when a second concrete external server
  justifies the added auth/security surface.

- **HITL: not warranted as inline checkpoints; warranted narrowly.** For a harness whose
  correctness oracle is the Lean kernel + axiom allowlist, per-step human validation adds cost
  without correctness gain, and "human-on-the-loop" policy control is already how we operate.
  Invest in HITL only at two seams: (1) offline human data curation into the training
  flywheel, and (2) an abstention→human-review-queue escalation for high-value/blueprint-scale
  runs.
