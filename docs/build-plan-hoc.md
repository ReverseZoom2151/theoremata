# Build plan: HOC-derived items + gap-module activation

Consolidated from the Higher Order Co evaluation
(`docs/resource-mining/new/higher-order-co.md`) plus the standing backlog of
gap modules that are built and unit-tested but wired into nothing.

## Execution constraints (read first)

- **The local Rust toolchain cannot link** (`ld: cannot find -lkernel32`). Every
  Rust change here is authored blind and verified only through CI. Python is
  locally testable.
- **`agent.rs` (1,338 lines) and `db.rs` are shared** by nearly all of B1-B9.
  These are integrated **serially by the maintainer**, one module per commit, not
  by parallel agents. Parallel agents do the disjoint work: research, new-file
  modules, and per-module call-site mapping, returning integration specs.
- **Commit granularly, push immediately.** One logical change per commit.
- Observability (B4+B9) lands first so the remaining CI-blind wiring is
  debuggable from persisted traces.

## A. Soundness & backend hardening (from the HOC evaluation)

| ID | Item | State | Effort |
|----|------|-------|--------|
| A1 | Generalize the backend success predicate | build now | ~half day |
| A2a | Verify no checker-result cache already exists | verify | ~1h |
| A2 | Hash-consed checker-result cache (statement, context) | build after A2a | 1-2 days |
| A3 | Inline-promise hints on the search frontier | verify then build delta | small |
| A4 | Agda `--interaction-json` enrichment | **deferred** | - |

**A1** — Today `external.rs` reads `match system { Metamath => sentinel, _ =>
out.success() }`. Three vendored systems (Metamath, HVM2, two Kind generations)
return 0 on failure, so exit-code trust is a recurring footgun. Replace the
blanket default with a required `SuccessSignal` each backend must declare
(`NonZeroExitIsHonest` for Lean/Rocq/Isabelle/Candle/Agda-`--safe`;
`StdoutSentinel{..}` for Metamath). No default: a new backend author must choose.

**A2/A2a** — The soundness-free version of "why parallelize when we can share":
memoize *verified* subproof results across candidates. Confirm nothing equivalent
exists under `critique/memory.rs` / `consolidate.rs` before building.

**A3** — `best_first.rs` + `critic_scorer.rs` already exist. Delta: let the
generator emit an inline priority hint (the `CInc`/`CDec` shape from HVM3's
`Collapse.hs`) rather than relying solely on an external scorer.

**A4 (deferred, needs green light)** — Our Agda backend is already sound (batch
`--safe`, honest exit code). Switching the *verdict* to `--interaction-json`
would REINTRODUCE the exit-0 problem. Value is limited to richer error/goal
feedback for the agent loop, kept strictly separate from pass/fail. Low value,
real trap; held pending an explicit decision.

## B. Activate the 8 gap modules into the live loop

Built and unit-tested; zero call sites outside their own definitions (verified).
Each wires into `agent.rs` and/or `db.rs`.

| ID | Module | Wires into | Order |
|----|--------|-----------|-------|
| B4+B9 | `RunTrace` + `FailureTaxonomy` + persistence | agent.rs loop, db.rs | 1st |
| B1 | `PromptAssembler` (context_assembly) | agent.rs request build | 2nd |
| B3 | `LivePlan` | agent.rs plan state | |
| B5 | `Guardrails` untrusted-input screen | agent.rs pre-gate | |
| B2 | `MetaToolRegistry` | MCP server + tool dispatch | |
| B7 | `route_model` + `FallbackLadder` | inference call sites | |
| B6 | `run_concurrent` | portfolio/search driver (flagged off) | |
| B8 | `GraphView` | retrieval path | |

## C. Watch / blocked (not build)

- Bend2 when it ships with a license: does it check termination? reject holes?
- `interaction-calculus-of-constructions` read (the one unread artifact).
- HVM4 as untrusted accelerator: blocked on any license; disfavored by the
  bottleneck-mismatch argument anyway.

## Standing principle this exercise argues for

**Exit status is never proof of success.** Metamath, HVM2, and Kind all return 0
on failure. A1 encodes this for the formal backends; the same rule should apply
to any future tool the harness shells out to.
