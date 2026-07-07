# flare-main — resource-mining report

Repo: `resources/flare-main/flare-main`.

## Scope inspected

Large monorepo: 762 files, including 213 Python files, 189 Lean files, 193 JSON files, 77 Markdown files, experiment configs, dataset packages, Docker/agent harness code, and site docs. Read top-level docs, `AGENTS.md`, package READMEs, dataset docs, verifier interfaces, factory code, scripts index, and representative Lean/Python/data files; catalogued full structure and skipped only repetitive dataset/generation artifacts.

## Core contribution

FLARE verifies mixed-integer linear-program reformulations with LLM-based formal proof synthesis in Lean. The repo contains:

- FormulationBench: 20 optimization problems, 116 MILP formulations, 96 reformulation pairs, 70 positive and 26 negative.
- `formulation-bench`: Python package for loading/rendering dataset problems.
- `milp-flare`: agent harness that runs Claude Code/Codex/OpenCode in Docker to synthesize Lean reformulation proofs.
- experiment scripts comparing execution, EquivaMap, FLARE-NL, and FLARE.

## Architecture / data format

Important interface:

- `ReformulationVerifier.start(a, b, output_path) -> ReformulationRun`
- `ReformulationRun.cancel()`
- `ReformulationRun.result() -> ReformulationResult`

`ReformulationResult` records `is_reformulation`, `method`, `artifacts_dir`, duration, cost, and metadata. The factory builds verifier instances from YAML specs.

FLARE adapts dataset formulations into:

- Markdown formulation text,
- generated `solve.py`,
- Lean formulation/proof artifacts.

## What Theoremata should reuse

1. Reuse the verifier-run abstraction: start/cancel/result is cleaner than blocking one-shot proof calls.
2. Add artifact-directory discipline for each proof attempt: inputs, generated Lean, logs, verifier outputs, cost, duration.
3. Add benchmark-level comparison of multiple harnesses/models on the same obligation.
4. Borrow the normalized agent JSONL trace analysis scripts for tool/time/cost telemetry.
5. Add MILP/formulation equivalence as a specialized benchmark track for proof-generating agents.

## Benchmark / eval value

Very high for agentic harness evaluation. It is less about general math and more about “can an agent turn structured problem artifacts into a machine-checked equivalence proof?” That is exactly the operational harness question Theoremata is trying to solve.

## Risks / gaps

- Requires Docker and external harness credentials.
- Gurobi license is needed for execution baseline/dataset `solve.py` checks.
- MILP proofs are domain-specific; do not overgeneralize performance to all math.
- Running arbitrary generated agent code in Docker reinforces need for sandboxing.

## Adopt list

- P1: add generic `AttemptRun` start/cancel/result abstraction.
- P1: persist per-attempt artifact directories.
- P2: add FormulationBench/FLARE benchmark adapter.
- P2: import trace normalization concepts into Theoremata telemetry.
- P3: add harness-vs-harness experiment configs.

