# Theoremata — Canonical Build Plan

> Living document. This merges the original plan drafted with Codex and the discoveries from
> deep-diving every repo under `resources/` plus the two agentic-systems books. Open it any day to
> know **what to build next and why**. When something changes in the design, change it here first.

---

## Table of Contents

1. [Identity & Status](#1-identity--status)
2. [Vision & Thesis](#2-vision--thesis)
3. [Architecture](#3-architecture)
4. [Data Model — the Proof DAG](#4-data-model--the-proof-dag)
5. [Verification & the Soundness Gate](#5-verification--the-soundness-gate)
6. [Tools & Workers](#6-tools--workers)
7. [Orchestration](#7-orchestration)
8. [Mathlib Retrieval](#8-mathlib-retrieval)
9. [Symbolic Proof Tool (Estimates)](#9-symbolic-proof-tool-estimates)
10. [Research → Formal Pipeline](#10-research--formal-pipeline)
11. [Evaluation](#11-evaluation)
12. [Human-in-the-Loop & Approval](#12-human-in-the-loop--approval)
13. [Sampling / Infra Boundary](#13-sampling--infra-boundary)
14. [SOTA-2026 Techniques to Adopt](#14-sota-2026-techniques-to-adopt)
15. [Current State & Known Gaps](#15-current-state--known-gaps)
16. [Prioritized Build Roadmap](#16-prioritized-build-roadmap)
17. [Provenance Appendix — What to Port From Where](#17-provenance-appendix--what-to-port-from-where)

---

## 1. Identity & Status

| | |
|---|---|
| **Product / platform** | **Theoremata** (Greek/Latin plural of *theorema* — "theorems") |
| **Active agent / runtime** | **Theorematon** |
| **Repo** | private `ReverseZoom2151/theoremata` |
| **CLI binary** | `theoremata` (short alias `thm`) |
| **Local state dir** | `.theoremata/` (SQLite db + workspaces + config) |
| **Env prefix** | `THEOREMATA_*` (e.g. `THEOREMATA_MODEL_COMMAND`) |
| **Stack** | **Rust** (CLI, TUI, orchestration, SQLite graph, events, approvals, process supervision) + **Python** (SymPy, falsification, safe eval, Estimates, specialist workers) + **Lean** (Lake workspaces, diagnostics, formal checking, LeanParanoia/Comparator hardening) |
| **Tagline** | "From conjecture to checked proof." |

**Status (current, 2026-07-07):** Rust + Python + Lean-facing harness is implemented as a CLI-first
agent runtime. Full local verification is green: `cargo test` = **94 passed**; `python -m pytest -q` =
**339 passed / 21 skipped**. Reference repos live under `resources/` (git-ignored). Resource mining now
covers 45 idea-bearing repos with reports under `docs/resource-mining/` plus the two agentic-systems
books in `harness-resources/`.

**The full stack is intended, not aspirational:** Rust owns orchestration/TUI/state; Python owns
symbolic/numerical/domain workers; Lean owns formal proof checking and generated project support.

---

## 2. Vision & Thesis

**The landscape is fragmented.** Every math-AI system studied does *one* slice — exploration, informal
proof, computation, formalization, or verification — and treats a proof as **text or a single Lean file**.
No system unifies them.

**The bet:** represent mathematics as a **persistent, typed proof DAG that is the source of truth** — not
chat history, not a `.lean` file. Chat is the interaction layer; the graph is durable state; the event log
records how knowledge changed; verification evidence determines what may be claimed as established.

**The one convergent thesis (from both books + every repo):**

> The **Lean compiler is your reward model, critic, router-prior, selector, and eval-metric — all at once.**
> The LLM only *proposes*; it makes only the soundness judgments Lean can't. And the proof DAG is
> simultaneously your **control-flow graph + process-reward structure + graph-memory (A-MEM) + human-in-the-loop
> diff surface** — build it once, not four times.

This is why Theoremata is graph-first. Cloning a Lean coding agent (like MathCode) would produce another
formalizer. Building around a durable proof graph produces a **research harness**, which is the differentiation.

---

## 3. Architecture

```
            CLI (scriptable)      TUI (Ratatui, interactive chat)
                        \        /
                 Typed Rust orchestration runtime
                 (scheduler · approvals · retries · process supervision)
                                |
          Persistent proof graph + event log (SQLite, .theoremata/)
          conjectures · definitions · assumptions · strategies · lemmas
          obligations · computations · counterexamples · informal/formal
          statements · verification evidence · provenance
                                |
                        Workflow engine
       explore · decompose · retrieve · compute · prove · criticize · formalize · verify
                                |
                              Tools
   Lean/Lake · Mathlib search · Python/SymPy · Estimates · counterexample search
                     · literature retrieval · Comparator · LeanParanoia
```

**Core loop (the differentiated pipeline):**

```
normalize problem
  → assumption / mathematical-object audit
  → strategy portfolio
  → dependency-graph (obligation) construction
  → parallel obligation solving
  → computational + symbolic testing
  → adversarial review
  → selective Lean formalization
  → hardened verification
  → graph repair or acceptance
```

**Design principles (non-negotiable):**

1. The proof graph — not chat history or a Lean file — is the source of truth.
2. Agents receive individual **typed obligations with acceptance criteria**, not unrestricted repo access.
3. **Failed approaches become durable, indexed evidence** (never lost to chat context).
4. **Verification is layered:** numerical → symbolic → adversarial → Lean compile → soundness audit.
5. Informal reasoning and Lean formalization stay **linked but separate**; formalize *selectively* after
   informal + computational checks (forcing Lean too early kills exploration speed).
6. The scheduler allocates models/token budgets by **uncertainty, dependency centrality, and verification cost**.
7. Human intervention at **high-leverage mathematical choices**, not routine syntax repair.
8. **Model providers stay replaceable.** Multi-agent is optional scheduling over obligations, not the default.
9. **Control flow lives in Rust**, prompts/verifiers in thin Python/Lean workers. (See §7 for why.)

---

## 4. Data Model — the Proof DAG

The node schema is a **solved problem** — every serious Lean repo (Kakeya, Riemann, strongpnt, Erdos,
FrontierMath) encodes the DAG identically via LaTeX-blueprint markup: `\label{id}` + `\lean{decl}` +
`\uses{...}` + `\leanok`. Generalize that quadruple and enrich it with Alethfeld's machinery.

**Node** (the core datatype):

```
Node {
  id,
  kind,                    // conjecture | definition | assumption | strategy | lemma |
                           // obligation | computation | counterexample | informal_proof |
                           // formal_statement | formal_proof | evidence
  tier,                    // spine (blueprint-visible) | implementation (agent-introduced)
  informal_statement,
  formal_statement / lean_decl | null,   // the NL↔formal bridge
  statement_uses[],        // deps the STATEMENT needs
  proof_uses[],            // deps the PROOF additionally needs   (blueprints distinguish these)
  status,                  // proposed | active | blocked | rejected |
                           // informally_verified | formally_verified | superseded
  scope[],                 // natural-deduction local assumptions in force (Alethfeld)
  discharges,              // which local-assume this local-discharge closes
  taint,                   // clean | tainted | self-admitted
  strategy_hint,           // intended proof route (the human `sorry`-annotation convention)
  suggested_lemmas[],      // target Mathlib lemmas an agent should try
  provenance,              // {created_by, model, round, revision_of, prompt+version, tool versions}
  content_hash,            // SHA-256 over (statement, sorted deps, justification)
  created_at, updated_at
}
```

**Edge** = directed dependency; kinds `depends_on | supports | contradicts | formalizes | verifies |
derived_from | supersedes`. Each carries an **`evidence_strength`**: `numeric_screen < prose_proof <
lean_checked`. Never let `numeric_screen` mark a node `formally_verified`.

**Two-tier granularity (calibrated from real repos):**
- **Spine nodes** = the blueprint-visible mathematical steps; the unit of scheduling and human review.
- **Implementation nodes** = agent-introduced sub-lemmas owned by a parent spine node.
- Expect ~4–5× fan-out (Kakeya 6-vs-10 declarations; FrontierMath 25-vs-238).
- Target the **Erdos/Kakeya granularity**: many named top-level lemmas that thread hypotheses explicitly,
  with the root theorem a thin composition. Avoid `have`-heavy monoliths (one un-retriable all-or-nothing node).
  Break every side-condition (convergence/integrability/non-vanishing/measurability) into its own node.

**Invariants & operations (port from Alethfeld):** immutable-value ops of shape
`fn(&Graph, args) -> Result<Graph, Vec<OpError>>` with an `assert_valid` postcondition + atomic
temp-write+rename persist; cycle detection (3-color DFS, return `Result`, not silent partial); taint
propagation (full + incremental from a node); **archive-don't-delete**; `revision-of` chains; and the
**6-invariant lemma-extraction** primitive (collapse a verified subgraph to one reusable node — context
compression).

**Status by reconciliation, never by declared flag.** Cross-check any blueprint `\leanok` / stored status
against a real `sorry`-scan + `#print axioms` audit of the linked decl; surface disagreements. "Not done" =
absence of a proof witness.

---

## 5. Verification & the Soundness Gate

**Layered, cheap-before-expensive:**

```
falsify-before-prove  →  symbolic check  →  adversarial (LLM critic on STRUCTURE only)
   →  Lean compile  →  hardened kernel audit
```

- **Falsify first.** Route every conjecture/subgoal to bounded counterexample search *before* spending
  prove-compute. A found counterexample kills the branch instantly and is itself a verifiable artifact.
- **Adversarial critic is for what Lean can't judge** — decomposition soundness, non-circularity, right
  generalization. Never let it judge tactic *correctness* (Lean does that). Reflection with **no grounded
  artifact attached** (compiler error / counterexample / SymPy disagreement) is disallowed by the harness.

**The soundness gate — two layers:**

1. **Cheap lexical pre-gate** (works *without Lean installed*): port MathCode's `_lean_masking.py`
   (nestable `/- -/`, `--`, string mask; preserves newlines → exact line/col) + `axiom_checker.py` regexes:
   `sorry`/`admit` with `'`-aware word boundaries `(?<![\w'])...(?![\w'])`, and `axiom`/`constant`/`postulate`
   even behind modifiers/attributes. This is a **necessary-not-sufficient** filter.
2. **Authoritative kernel gate** (LeanParanoia, ordered by soundness value):
   1. **Kernel replay via `lean4checker` — do not ship without it.** Re-runs the real kernel over compiled
      decls; catches tampered oleans, `debug.skipKernelTC`, native-decide aux.
   2. **Axiom allowlist over the transitive closure** = exactly `{propext, Quot.sound, Classical.choice}`.
   3. `sorry`/`sorryAx`; metavariables; unsafe/partial; native computation (`native_decide`,
      `Lean.ofReduceBool`, compiler-trust); compiler-trust attrs (extern/export/init/implemented_by/csimp —
      csimp scanned *globally*, both source+target audited) with a trusted-core exemption; constructor &
      recursor integrity; optional source-pattern grep.

**Structural immunity.** Binding every node to a **compiled declaration** makes the harness *structurally
immune* to grandiose prose (e.g. `goldbach-collatz`): a claim with no resolving decl is **unsatisfied by
construction** — the verifier's job is not to read the argument, only to observe that no node resolves.
`#print axioms <thm>` is the final trust gate. **Provenance (which model wrote it) is metadata; trust is
mechanical.**

---

## 6. Tools & Workers

- **Rust owns control flow.** Python/Lean workers are **thin and stateless**: take a request, return typed
  JSON, exit. (AgentMathOlympiadMedalist accidentally proves this — see §7.)
- **Tools return typed data or raise — never prose.** `lean_check → {compiles, errors:[{line, expected,
  actual, unsolved_goals}], goals_remaining}`; `mathlib_search → ranked normalized lemma statements+names`.
  The model reasons far better over structured goal-state than a wall of compiler text.
- **Trust levels are explicit.** SymPy / counterexample-search = **heuristic oracle** (suggestive, must be
  re-verified in Lean). Lean = **ground truth** (dispositive). Encode "do NOT use `sympy_simplify` to prove."
- **PAL is central:** never let the LLM *compute* or *assert* correctness — offload arithmetic/algebra to
  SymPy, verification to Lean; every computational claim is witnessed by a stored `tool_evidence`.
- **Sandbox (port DeepMath):** run each snippet in a subprocess with **hard kill** (SIGTERM→SIGKILL),
  tagged framed result (`OK`/`EXC`/empty), and an **import allowlist enforced at the executor** (not just the
  prompt). CPU/mem/**wall-clock** caps are first-class; a non-terminating check fails gracefully.
- **Ship the Python sidecar as MCP servers over stdio** (SymPy, counterexample search, Mathlib retrieval;
  Lean as its own server) — clean Rust↔Python decoupling, hot registration, a standard audit point.
- **Cache** Lean check results keyed on `(statement, proof_text)` hash; prefix-cache the stable
  system + tool-schema + Mathlib preamble.

---

## 7. Orchestration

**Retry as a three-tier nested state machine (port QED — highest-value pattern):**

```
REVISE_PROOF  <  REVISE_PLAN  <  REWRITE      (cheapest → most expensive)
   |                |               |
 new proof vs     new plan        new whole
 fixed plan       (resets proof)  decomposition
 max 8            max 4           max 4
```

- A **regulator** decides on any failure; ambiguity defaults to the cheapest tier.
- **Auto-escalation** when a tier's budget is exhausted without the regulator asking — inject *synthetic*
  guidance ("Automatic Escalation to Plan Revision"), never reuse stale guidance.
- **Two-tier verify:** structural (statement integrity, no-holes, citation faithfulness, plan adherence)
  *before* detailed (step-by-step math). A separate **one-word verdict agent** reduces a verbose report to
  `DONE`/`CONTINUE` (biased conservative) — decouples control signal from prose.
- **Resume = DB/filesystem-as-checkpoint:** infer the resume point purely from which artifacts exist.
- **Acceptance rule (port AgentMathOlympiadMedalist, fixed):** N-consecutive-pass, **reset-on-any-fail**,
  cap max-iters (make N and cap config). **Its bug is the lesson:** its "iterative loop" is a *no-op* — it
  re-verifies the same solution and never feeds corrections forward, because control flow lived in a Python
  client, not the harness. Put counters/gating/typed-state in **Rust**; feed the *corrected solution* forward.
- **Scheduler exploits DAG structure:** parallelize independent subtrees (natural join-points), batch
  templated sibling families (e.g. `I₁…I₆` estimates), share a foundational **`ForMathlib` library-gap layer**
  across proofs, and support an **unproven apex from day one** (top-down decomposition).

**Verifier taxonomy for node states (port AgentMathOlympiadMedalist prompt IP):** *Critical Error* (invalidates
the line of reasoning — stop, but scan independent parts) vs *Justification Gap* (assume the step, continue).
Maps to node states {proven, gap-assumed, refuted}.

---

## 8. Mathlib Retrieval

**Not raw-text RAG.** Three layers:

- **Layer A (offline, source-only, free):** scan every `Mathlib/**/*.lean` import line → module DAG
  (forward/reverse adjacency, public/private edge flags, transitive closures). `path == module == import`
  bijectively. Ingest `docs/*.yaml` as a concept→declaration seed table. Key the index on
  `lean-toolchain` + `lake-manifest.json` hashes so it invalidates exactly when oleans change.
- **Layer B (one-shot `lake env lean` env dump → JSON):** per declaration emit FQ-name, namespace,
  pretty-printed signature, defining module (`env.getModuleFor?`), `file:line` (`declRangeExt`), const-dep
  edges (`FoldConsts`). Over it build a **`#find`-style head-index bucket** + a text/embedding index over
  signatures. (Decl namespace ≠ path, so this metadata is required — string parsing won't do.)
- **Layer C (warm Lean process):** keep a live imported environment; route type-aware retrieval to a
  replicated `#find` (`forallTelescopeReducing` + `isDefEq` re-rank) and goal-directed premise selection to
  `exact?`/`apply?`.

**Retrieval quality:** hybrid **BM25 + dense fused by RRF** (BM25 for exact identifiers/notation where dense
fails; dense for conceptual queries) → **cross-encoder rerank** top-20 → keep the set small (5 precise > 50
marginal). **The compiler is your faithfulness oracle** — a hallucinated lemma name fails at compile time,
which generic RAG cannot catch. Treat lemma selection itself as retrieval (top-k per turn, never stuff all).

Also wrap `lake exe shake` for **import minimization** so generated proofs stay lean and auditable.

---

## 9. Symbolic Proof Tool (Estimates)

A narrow, ergonomic verifier for analytic estimates — many obligations are easier here than in general Lean.

- **Model in Rust:** `ProofState { goal, hypotheses: IndexMap<String, Term> }` (ordered),
  `ProofTree { state, parent, tactic: Option, children }`, and a `Tactic` trait
  `fn activate(&self, &ProofState) -> Vec<ProofState>` with the exact convention: `[]` = closed,
  a single `eq` state = no-op/failure, `[..]` = subgoals. Reuse `find_sorry`/`count_sorries` traversal.
- **Keep SymPy + Z3 in Python.** The clean seam is `feasibility(inequalities) -> (bool, model | Farkas_certificate)`
  as a typed JSON call, so **Rust can independently re-check the Farkas certificate**.
- `linarith` and `log_linarith` are the **same refutation-via-exact-LP kernel** over two embeddings
  (linear vs log-space); implement one kernel + two front-ends, including the integrality-gap rule and
  `Ne`/`Max`/`Min` scenario enumeration.
- Standardize the FFI failure protocol as `{status: proved|progress|noop|error, message, states}` so Rust
  never scrapes stdout. Start with `Trivial`, `SplitGoal`, `Cases`, `Subst`, `Linarith`, `LogLinarith`, `SimpAll`.

---

## 10. Research → Formal Pipeline

MathResearchPrompts is the **closest reference** to Theoremata's research→formal spine.

- **12 domain-neutral stage templates** (adopt as the stage catalog): scope/ideate (with degeneracy notes) →
  sharpen objects → sharpen claims + validation plan (with a **triage gate**: prove vs conjecture-with-numerics)
  → direct proof (mark every extra assumption/external theorem as a **proof obligation**) → prove-or-disprove
  (two parallel branches + verdict merge) → candidate discovery (typed claims + pass/fail/inconclusive) →
  transfer schema → property-constrained synthesis (**emits executable falsifier**) → constant stress-test →
  rate refinement → prompt-variation sensitivity → environment log.
- **Typed claim-DAG:** `Setting → CandidateClaim{type, status} → Proof/Disproof + Verdict →
  FormalizationTarget → LeanTheorem`, edges carry `evidence_strength`.
- **Hard rule: numerics screen, they never prove.** A numeric pass only *unblocks* a formalization node.
- **Two-tolerance falsifiers** encode two epistemic statuses: exact algebraic identity ≈ `1e-12`
  (machine-zero), finite-difference approximation ≈ `5e-3` (loose). Deterministic + Haar-random adversarial seeds.
- **Persist the FormalizationTarget as first-class** (Lean signature stub + paper-symbol→Lean-def dictionary +
  decomposition into Lean-sized sub-targets). MathResearchPrompts' key missing artifact *was exactly this* —
  make it explicit. Default decomposition heuristic: algebraic-identity node + structure/invariance node +
  scalar-analysis node + bridge/join node.
- **Per-node provenance log** (prompt+version, model snapshot, temperature, tool versions, accepted/rejected
  outputs, and the **unresolved-proof-obligations list — which is the DAG frontier and drives the scheduler**).

---

## 11. Evaluation

**Six independent axes — never collapse to one scalar** (IneqMath: GPT-5 47% *overall* vs 66.5% answer-only):

| Axis | Measures | Grader / evidence |
|---|---|---|
| Discovery | found *an* answer/construction at all | `is_solved` + pass@k / majority@k |
| Informal validity | NL proof step-sound, not just answer-right | 4 stepwise judges (NTC/NLG/NAE/NCE) + final-answer judge; overall = AND |
| Formal compilation | Lean `Solution` compiles, no `sorry` | `lake build` in Landrun sandbox |
| Dependency soundness | same statement + axiom closure ⊆ allow-list | Comparator statement-diff + axiom check |
| Efficiency | proof/trace length, tokens, tool-calls | tokenizer length metric, mean ± **SE** |
| Novelty | new vs Mathlib alias / memorized | dependency-graph + n-gram/contamination check |

Report **each axis separately with standard-error bars** (`std(ddof=1)/√n`).

**Two-pipeline grader routed by `grade_kind`:**
- **answer-style** (AIME, MATH500, IneqMath bound/relation, FrontierMath-numeric): extract last
  `answer is`/`\boxed{}` → verify by type (integer `==`; LaTeX via `math_verify` with an LLM symbolic-equivalence
  fallback that enforces *exact* equivalence — decimals ≠ exact; MC-relation canonicalized) → pass@k/majority@k.
  For FrontierMath-style: run the submitted constructor in the sandbox, feed output to a finite-window verifier.
- **proof-style** (FormalQualBench, formalized theorems): seal stub as `Challenge.lean`; run **Comparator +
  Landrun** — `exit 0` requires statement-identity, no `sorry`, and axiom closure ⊆
  `{propext, Quot.sound, Classical.choice}`.

**Contamination controls:** freshness-tier **AIME26 > 24/25** for live eval; honor **IneqMath test-only** and
**AutoMathText training-only** licenses (never grade against training/retrieval corpora); flag **Mathlib-alias**
trivial discharges (proof compiles by `exact Mathlib.someName` — valid but not a discovered proof → human review);
pin toolchain+Mathlib rev per problem. Build a **temporal held-out** private set (theorems newer than model cutoff).

**Superset problem-record schema:** `{id, problem, grade_kind, answer?, choices?, gold_solution?, lean_stub?,
theorem_names?, permitted_axioms?, toolchain_pin?, usage_tag, contamination_risk, difficulty_tier}`.

---

## 12. Human-in-the-Loop & Approval

- **Per-action, tiered — humans see only the 10% that matters.**
  - **Auto-approve** reversible, Lean-verified additions (a compiled sorry-free leaf is low-friction).
  - **Require human sign-off** for semantically consequential mutations: adding an axiom/assumption,
    **changing a theorem statement**, accepting an unverified/`sorry` node, pruning verified work,
    committing a proof as final. Same gate applies to procedural self-edits (prompt/heuristic rewrites).
- **Models never self-grant `formally_verified`** (already enforced in `chat.rs`).
- Show the human the **diff + the Lean verification result**; keep an audit trail of who approved what.
- **Async approval** is mandatory (long Lean compiles must not block); on denial return "rejected by reviewer"
  so the model re-proposes rather than dies; human corrections become high-importance episodic memory.
- **HITL on the *plan*, not just the final answer** — present the lemma-decomposition for the mathematician to
  edit *before* committing DAG nodes.
- **Provenance is metadata; trust is mechanical.**

---

## 13. Sampling / Infra Boundary

**FlashSampling (FMMS)** — a fused, exact categorical-sampling kernel that makes high-branching sampling cheap
(`num_samples = k` candidate continuations in one pass) — must sit **strictly BELOW** the proof representation:

- Wrap behind a **`TacticSampler` trait** (`sample(hidden, temperature, num_samples, seed) -> token_ids`);
  FMMS is one impl, `naive-pt` is the reference/CPU-fallback/test-oracle.
- It is **not bit-exact** (bf16 accumulation) and has had silent hardware bugs → **validate statistically**
  (chi-squared), never assert token-id equality across backends; sampled tokens are **untrusted proposals**
  validated by the checker.
- **Removability is the correctness test of the boundary:** the DAG schema, tactic representation, and tool
  interface must be fully definable with FMMS removed. Record `(provider, seed, temperature)` as edge
  provenance only. Don't reimplement the kernel — vendor it as an optional Python backend, pinned, device-gated.

---

## 14. SOTA-2026 Techniques to Adopt

- **RLVR / GRPO structure even at inference time** — best-of-N with the **compiler as a *perfect* selector**
  (`P(success)=1−(1−p)^N`, no reward-model-hacking ceiling); log every `(goal, tactic, verified?)` as gold data.
  DAPO for long-proof stability; 2-GRPO (G=2 ≈ G=16) since Lean rollouts are the bottleneck; Goldilocks
  20–80%-pass filtering as an automatic curriculum. Process reward is native (`#verified/#total`).
- **AlphaProof-style MCTS over tactics with an LLM prior** (PUCT), then **distill the visit-count policy back**
  into the base model (search → cheap single-shot over time).
- **Graph-of-Thoughts *merge* as a core DAG primitive** — prove A and B independently, merge into a theorem
  needing both (multiple-parents → one child). This is the payoff over linear/tree agents.
- **Recursive-LM / programmatic DAG accessors** (context-rot avoidance) — give the model a REPL over the DAG
  ("get node X," "search Mathlib for Y") instead of dumping the whole graph.
- **Search-R1 agentic retrieval** — the agent decides *mid-proof* it needs a lemma, retrieves, integrates, retries.
- **Sleep-time compute** — between sessions, pre-prove obvious sub-lemmas, pre-retrieve Mathlib neighborhoods,
  dedupe/normalize proven lemmas, promote reused tactic patterns to skills, summarize dead-end classes.
- **A-MEM / Mem0 graph memory** — the proof DAG *already is* this (lemmas = atomic notes, dependency edges =
  links); adding a proved lemma should propagate to re-check what it unblocks.
- **Grammar-constrained Lean decoding** — an EBNF for Lean tactic syntax so the model cannot emit unparseable
  tactics (< 2% throughput cost); structured ≠ correct, so always run the compiler downstream.
- **AlphaEvolve + Co-Scientist for open-problem mode** — program-database of candidate proofs, Lean as evaluator,
  evolutionary multi-objective selection; generate-debate-evolve conjecture layer above the proof layer.
- **Contractor/subcontract abstraction** — a formal *proof contract* {statement, allowed axioms, Mathlib
  version, must-compile-sorry-free, compute budget}; supports negotiation ("this lemma is false — here's a
  counterexample; approve a weakened hypothesis?" *before* burning compute).

---

## 15. Current State & Known Gaps

**Built & committed:**

- Rust CLI/TUI/orchestrator with SQLite graph, runs, attempts, evidence, events, messages, approval proposals,
  cycle detection, content hashes, taint propagation, lemma extraction, scheduler, and typed provider/tools seams.
- Proof-DAG schema upgrades from the resource sweep: leanblueprint import/export, `lean_decls`, statement-vs-proof
  dependency/status split, checkdecls-style binding gate, node tiers, hidden-helper budgeting, and typed-claim enums.
- Reasoning control craft: QED plan history, mechanical escalation, phase-prior selector, k-consecutive-clean
  acceptance, critic taxonomy, meta-critic pruning, and three-valued taint (`clean`/`tainted`/`self-admitted`).
- Verification/hardening: warm Lean session, lexical soundness gate, `#print axioms` path, LeanParanoia integration
  with trusted modules and fail-closed handling, adversarial-corpus regression tests, and Comparator-backed
  formalization grading when `THEOREMATA_COMPARATOR` is configured.
- Python worker modules: safe eval, falsification, symbolic/SymPy checks, estimates/LogLinarith/Farkas certificates,
  sandbox hard-kill/import allow-list/budget governor, LM-as-scorer reranker, lemma cache, proof telemetry,
  verified-decide template, benchmark harness, STaR harvester, reward module, and GRPO dry-run.
- Evaluation/training scaffolds: loaders/graders for formalization, NL/answer, falsification, hardening, and
  training-data harvest tracks.
- Resource mining: original 21 idea-bearing repos + 24 newly added repos reported under `docs/resource-mining/`;
  `harness-resources/` books read; AIME datasets characterized; `mathlib4` retained as retrieval corpus.

**Verification status:** local full suite on 2026-07-07:

- `cargo test`: 94 passed.
- `python -m pytest -q`: 339 passed, 21 skipped.

**Known gaps that still matter:**

- The full falsify → decompose → formalize → verify → certify chain has not yet been validated against a live real
  model/API key; mocks and deterministic modules are green.
- External-prover integration is not yet built: Aristotle/Harmonic should be async job state + sparse polling +
  local verification, not a blocking call.
- LeanDojo/ReProver/open-atp style proof-task/environment abstractions are not yet first-class backends.
- The benchmark harness has broad loaders/graders, but newly mined datasets still need selected smoke runs and
  pinned Lean environments before we claim operational coverage.
- No top-level trust-boundary document yet. TorchLean shows this should be explicit documentation plus lint checks.
- Web viewer/editor integration remains deferred; the product remains CLI/TUI-first.

---

## 16. Prioritized Build Roadmap

Keep each item as a granular build/test/push milestone.

### P0 — validate the built agent

| # | Milestone | Why |
|---|---|---|
| 0.1 | **Live real-model end-to-end run** on a small benchmark item | This is the one unvalidated system claim after the mock/deterministic suite |
| 0.2 | **Record the live trace as a fixture** | Prevents future regressions in the whole chain |
| 0.3 | **Update `docs/TRUST_BOUNDARIES.md`** | Makes LLM/Python/Lean/external-prover trust explicit |

### P1 — Lean/prover interaction backends

| # | Milestone | Why |
|---|---|---|
| 1.1 | **ProofTask / ProofResult contract** from LeanDojo/open-atp/ReProver patterns | Stabilizes theorem-proving backends behind one interface |
| 1.2 | **Aristotle external-prover adapter** with mock/live modes | Converts the Harmonic/Aristotle resources into a usable tool |
| 1.3 | **Async prover job state + sparse polling/resume** | External proofs are minute-to-hour jobs, not synchronous subcommands |
| 1.4 | **Premise/proof-state backend adapters** for LeanDojo/ReProver style retrieval | Upgrades retrieval from raw text to environment-aware proof state |

### P2 — benchmark activation

| # | Milestone | Why |
|---|---|---|
| 2.1 | **MiniF2F/Harmonic smoke track** | Canonical formal proof-completion and autoformalization eval |
| 2.2 | **BRIDGE-178 verified-programming smoke track** | Tests total Lean code + oracle checks + repair rows |
| 2.3 | **QuantumLean-Bench scaffolded formalization track** | Tests scientific/physics formalization, not just olympiad math |
| 2.4 | **FormulationBench/FLARE harness track** | Tests agentic proof generation with artifact dirs, cost, duration |
| 2.5 | **IMO/Putnam external-prover artifact track** | Verifies existing Aristotle-generated Lean as trust-but-verify fixtures |
| 2.6 | **Millennium statement-quality track** | Evaluates honest formalization/definition design without pretending proof completion is near-term |

### P3 — product/ops

| # | Milestone | Why |
|---|---|---|
| 3.1 | **`theoremata doctor`** | Setup diagnostics for Lean/Lake/Python/mathlib/model/provider state |
| 3.2 | **Attempt artifact directories** | Persist inputs, generated Lean, logs, verifier output, cost, and duration per attempt |
| 3.3 | **Trace/time/cost analysis** | Borrow FLARE-style normalized JSONL telemetry for harness optimization |
| 3.4 | **Stable CLI/MCP API surface** | Lets future editor/web integrations call Theoremata without forking the product |

### P4 — deferred

- Web graph viewer.
- Editor extension integration.
- Optional heavy domain fixtures such as TorchLean.
- Real GPU/RL training runs beyond dry-run scaffolds.

---

## 17. Provenance Appendix — What to Port From Where

| Source | The one thing to port |
|---|---|
| **MathCode** | `_lean_masking` comment/string lexer + `axiom_checker` regexes (cheap soundness pre-gate); tiered warm-Lean verification; per-stage model routing; product-shell UX (do **not** build on its closed binary) |
| **Alethfeld** | The node/edge/scope/taint/provenance/content-hash schema, immutable-op + `assert_valid` postcondition + atomic write, archive-don't-delete, **6-invariant lemma extraction** |
| **QED** | Three-tier retry (`REVISE_PROOF < REVISE_PLAN < REWRITE`) with auto-escalation; DB/filesystem-as-checkpoint resume; two-tier verify + one-word verdict; role→CLI routing |
| **DeepMath** | Subprocess-hard-kill sandbox + import allowlist + tagged result framing; delimiter-driven execute-and-feed-back loop |
| **LeanParanoia** | The ordered check battery — **kernel replay first**, then axiom allowlist over the transitive closure, sorry, unsafe/partial, native, compiler-trust attrs, constructor/recursor integrity |
| **Estimates** | ProofState/ProofTree/Tactic model + Z3 exact-LP kernel returning **Farkas certificates** (the JSON seam) |
| **MathResearchPrompts** | The 12-stage research→formal template ladder; typed claim-DAG; "numerics screen, never prove"; two-tolerance falsifiers; first-class FormalizationTarget |
| **AgentMathOlympiadMedalist** | The N-consecutive-pass/reset-on-fail/cap-N acceptance rule + Critical-Error/Justification-Gap taxonomy; **its no-op-loop bug = keep control flow in Rust** |
| **Lean corpora** (Kakeya, Erdos, Riemann, strongpnt, FrontierMath, ZkLinalg, Sphere-Packing, M4R, goldbach-collatz) | The blueprint `\label`/`\lean`/`\uses`/`\leanok` node encoding; two-tier granularity; obligation-tracking by reconciliation; the adversarial-rejection benchmark |
| **FormalQualBench** | Proof-style acceptance = Comparator statement-diff + axiom allowlist `{propext, Quot.sound, Classical.choice}` + Landrun sandbox |
| **IneqMath** | Informal-yet-verifiable grading + the four stepwise judges (NTC/NLG/NAE/NCE); the "answer-acc ≫ overall-acc" lesson |
| **DeepMath (data)** | `math_verify` deterministic grader; pass@k / majority@k / averaged@k + length metric with SE bars |
| **FlashSampling** | Optional `TacticSampler` backend kept strictly below the proof representation (removability = boundary is correct) |
| **Mathlib** | The 3-layer integration surface (import-DAG → env-dump decl index → warm `#find`); `shake` import minimization — **not raw-text RAG** |
| **Agentic Design Patterns** / **Hitchhiker's Guide** | Harness-as-OS separation; explicit state machines; grounded reflection; tiered HITL; RLVR/GRPO; A-MEM graph memory; observability + replay from day one |
| **LeanDojo / LeanDojo v2 / ReProver** | Environment-aware proof interaction, theorem/task extraction, dense premise retrieval, and proof-state-first APIs |
| **LeanCopilot / LeanProgress** | In-Lean suggestion/search hooks and learned progress heuristics as optional tactic/prior backends |
| **LeanAgent / open-atp** | Lifelong-learning/prover loops and a clean LeanProject/ProofTask/ProofResult/backend harness abstraction |
| **lean-aristotle-mcp / Aristotle Putnam / IMO2025** | Async external-prover job contract, provenance-rich generated proofs, and trust-but-verify artifact ingestion |
| **BRIDGE** | Domain-guided verified-programming pipeline: prompt JSONL → generation → fenced-code extraction → checker → verifier-error repair |
| **datasets-main / MiniF2F** | Natural-language + formal statement pairs for proof-completion and autoformalization scoring |
| **QuantumLean-Bench** | Domain-scaffolded scientific formalization benchmark with response-key metadata |
| **FLARE / FormulationBench** | Start/cancel/result verifier abstraction, attempt artifact directories, harness-vs-harness experiments, trace/cost analysis |
| **TorchLean** | Explicit trust-boundary inventory for kernel proofs, executable checkers, Prop-valued contracts, FFI/native runtime, and external producers |
| **zero-to-qed** | Proof-state pedagogy and tactic decision templates for explanations, critic rubrics, and small smoke tests |
| **Lean4Code** | Setup/onboarding ideas only; keep Theoremata CLI/TUI-first and expose APIs for later editor integration |
| **LeanMillenniumPrizeProblems** | Honest statement-target track, reference-file provenance, and parameterized data-package pattern for missing foundations |
