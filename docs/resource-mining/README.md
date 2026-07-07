# Resource mining — synthesis & adopt list

This folder is the durable resource-mining record for Theoremata.

Coverage as of 2026-07-07:

- 45 idea-bearing repos have full reports: the original 21-resource sweep plus 24 newly added repos under `docs/resource-mining/new/`.
- `harness-resources/` contains the two agentic-systems books already read separately.
- `aime24/25/26` are benchmark datasets characterized as eval inputs, not architecture sources.
- `mathlib4` is treated as the retrieval corpus/dependency, not a repo to mine for product architecture.

## What changed after the new 24-resource sweep

The first sweep established the core Theoremata shape: persistent proof DAG, leanblueprint interoperability, falsify-before-prove, layered verification, QED-style retry, hardening, benchmark ingestion, and training scaffolds.

The second sweep adds four major themes:

1. **Lean interaction infrastructure is its own product surface.** LeanDojo/ReProver/LeanAgent/LeanProgress/LeanCopilot/open-atp all point to the same separation: theorem proving needs a clean environment/task/backend interface, not ad-hoc shell calls. Theoremata should expose this internally as stable proof-task and proof-result contracts.

2. **External provers must be async, provenance-rich, and locally verified.** Aristotle outputs, IMO/Putnam results, and `lean-aristotle-mcp` show that cloud provers can be strong but slow. The correct integration is submitted/queued/in-progress/proved/partial/failed/counterexample jobs with sparse polling, request provenance, and mandatory local compile/axiom/hardening checks.

3. **Evaluation must split theorem proving, verified programming, domain formalization, and statement design.** MiniF2F, BRIDGE-178, QuantumLean-Bench, FormulationBench/FLARE, IMO/Putnam, and Millennium statements are not one benchmark. They are separate tracks with different acceptance criteria.

4. **Trust-boundary discipline needs to be explicit documentation plus checks.** TorchLean’s strongest contribution is not neural-net code; it is the trust inventory separating kernel proofs, executable checkers, Prop-valued contracts, FFI/native runtime, and external producers. Theoremata needs the same discipline for LLMs, Python tools, Lean, external provers, and generated certificates.

## Cross-cutting themes retained from the first sweep

1. **`leanblueprint` is the de-facto proof-DAG standard.** The formalization repos converge on theorem-env labels, `\lean{}` declaration bindings, `\uses{}` dependency edges, `\leanok`, and `lean_decls`/`checkdecls`. Theoremata now has a leanblueprint dialect, statement/proof status split, and node-binding gate.

2. **The blueprint DAG is a skeleton, not the full implementation.** Real projects invent roughly 40–100% more helper declarations than blueprint nodes. Theoremata should keep helper fan-out as an explicit budget and granularity dial.

3. **Answer matching is not proof soundness.** IneqMath’s answer accuracy versus proof-soundness gap supports Theoremata’s graph/verification thesis.

4. **Falsify-before-prove is always executable.** Counterexample search, symbolic checks, Lean `native_decide` fixtures, bad-event formalizations, and asymptotic LP refutations all point to executable falsification before proof search.

5. **Verifier discipline and anti-sycophancy are first-class.** k-consecutive acceptance, critic taxonomies, taint propagation, repair provenance, and adversarial corpora matter as much as generation prompts.

## Adopted already

These items from the first adopt list are now built and pushed:

- LeanParanoia hardening bugs fixed: trusted modules passed, fail-closed handling, target parsing, and failure taxonomy surfaced.
- LeanParanoia adversarial-corpus regression tests added.
- `leanblueprint` import/export, `lean_decls`, statement/proof split, checkdecls-style binding gate, hidden-helper budgeting, and typed-claim enums.
- QED-style plan history, mechanical escalation, phase-prior selector, k-consecutive acceptance, critic taxonomy, meta-critic, and three-valued taint propagation.
- Estimates-inspired asymptotic/LogLinarith/Farkas modules.
- Hardened Python falsifier sandbox.
- LM-as-scorer retrieval reranker.
- Lemma cache, proof telemetry, verified-decide certificate template.
- Benchmark harness and training/STaR/GRPO scaffolds.
- Comparator invocation for formalization grading.

## Current prioritized adopt list

### P0 — validation and documentation

- Run a live end-to-end real-model chain. The full system is tested through mocks and deterministic modules; the remaining unvalidated claim is live model behavior through falsify → decompose → formalize → verify → certify.
- Keep `docs/PLAN.md` and this resource synthesis current whenever new repos are added or a roadmap tier ships.
- ~~Add a top-level `docs/TRUST_BOUNDARIES.md` modeled after TorchLean.~~ **Done** (`docs/TRUST_BOUNDARIES.md`).

### P1 — Lean/prover interaction layer

- ~~Add a first-class `ProofTask` / `ProofResult` contract, drawing from LeanDojo/open-atp/ReProver.~~ **Done** (`components/prover/`).
- ~~Add external-prover job state: submitted, queued, in-progress, proved, partial, failed, counterexample, error.~~ **Done** (`proof_jobs`, CLI).
- ~~Add sparse polling/resume for external prover jobs.~~ **Partial** (poll count + `AttemptRun`; full resume scheduler deferred).
- ~~Add Aristotle adapter behind the existing tool/provider seam, with mock mode and local hardening of returned Lean.~~ **Done** (mock/live + `verify.rs` + statement guard).
- ~~Add LeanDojo/ReProver-style premise retrieval and proof-state environment adapters as optional backends.~~ **Partial** (mock LeanDojo, `accessible_retrieve`, ReProver backend; live Pantograph deferred).

### P2 — evaluation expansion

- ~~MiniF2F/Harmonic datasets: proof-completion + autoformalization track.~~ **Partial** (loaders + `proof_completion` op; corpus vendoring + live smoke deferred).
- ~~BRIDGE-178: verified programming track with Lean oracle tests and repair-loop rows.~~ **Partial** (loader + structural grader + `verified_programming` op; live oracle execution deferred).
- ~~QuantumLean-Bench: scientific/quantum formalization track with scaffolded prompts.~~ **Partial** (loader + domain filter; scaffold prompts in `expected` metadata).
- ~~FormulationBench/FLARE: MILP reformulation proof track and harness-vs-harness experiments.~~ **Partial** (`formulationbench` loader + `trace_normalize`; harness-vs-harness configs deferred).
- ~~IMO2025 and Aristotle Putnam outputs: external-prover artifact verification track.~~ **Partial** (`imo2025`, `putnam_artifacts` loaders + structural graders; live compile suite deferred).
- ~~Lean Millennium statements: statement-quality/definition-design track, not proof-completion.~~ **Partial** (`millennium` loader + `statement_target` kind).

### P3 — product and operations

- ~~Add `theoremata doctor` for Lean/Lake/Python/model-provider/mathlib setup checks.~~ **Partial** (`theoremata doctor` + corpus/env inventory).
- ~~Add artifact-directory discipline for every attempt: inputs, generated Lean, logs, verifier output, cost, duration.~~ **Done** (`AttemptRun` artifact dirs).
- ~~Add trace normalization for tool/time/cost analysis, borrowing FLARE's JSONL analysis pattern.~~ **Done** (`trace_normalize` worker tool).
- Expose stable CLI/MCP APIs that an editor integration can call later; do not fork/build a desktop editor now.

### P4 — future/deferred

- Web viewer / browser UI for graph inspection.
- Editor extension integration.
- Heavy domain-specific optional fixtures such as TorchLean.
- GPU/RL training runs beyond dry-run scaffolds.

## Per-repo reports

Original reports:

- AgentMathOlympiadMedalist
- AutoMathText-and-goldbach-collatz
- DeepMath
- Erdos1196
- FlashSampling
- FormalQualBench
- FrontierMathOpen-Hypergraphs
- KakeyaFiniteFields
- LeanParanoia
- M4R_Thesis
- MathResearchPrompts
- QED
- RiemannHypothesisCurves
- Sphere-Packing-Lean
- alethfeld
- estimates
- ineqmath
- mathcode
- strongpnt-and-ZkLinalg

New reports are under `docs/resource-mining/new/`:

- BRIDGE-main
- IMO2025-main
- LeanAgent
- LeanCopilot
- LeanDojo
- LeanDojo-v2
- LeanDojoChatGPT
- LeanMillenniumPrizeProblems-main
- LeanProgress
- LeanVision-main
- QuantumLean-Bench-main
- ReProver
- TorchLean-main
- aristotle_putnam25-main
- datasets-main
- flare-main
- gilp-master
- lean-aristotle-mcp-main
- lean4code-main
- open-atp-main
- pbcc-main
- proofgrader-main
- python-memtools-main
- zero-to-qed-main
