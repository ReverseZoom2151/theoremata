# QuantumLean-Bench-main — resource-mining report

Repo: `resources/QuantumLean-Bench-main/QuantumLean-Bench-main`.

## Scope inspected

100 files: README, benchmarking docs, 11 JSON datasets under `QuantumLean/BenchmarkData`, 38 prompt templates, 28 Python scripts, 10 Lean scaffold files, manual evaluation CSVs, Lake project files, and provider templates.

## Core contribution

QuantumLean-Bench is a unified benchmark for informal and formal quantum reasoning. The repo states it contains 931 problems across quantum chemistry, computing, cryptography, information theory, physics, and quantum machine learning.

Local inventory confirmed 11 JSON dataset files and 931 total records:

- Chemistry: 306
- Computing: 220
- Cryptography: 4
- InfoTheory: 86
- Physics: 215
- QML: 100

## Architecture / data format

Dataset records typically include:

- `id`, `source`, `domain`, `type`,
- `problem`,
- `metadata`,
- `citations`,
- `solution_informal`,
- `solution_formal`,
- `manual_eval`.

Benchmarking supports two modes:

- informal generation into `solution_informal[response_key]`,
- formal Lean generation plus typechecking through `benchmarking/eval/run_formal.py`.

Formal prompts inline shared Lean scaffolds from `benchmarking/formal/Common.lean` and a domain-specific scaffold.

## What Theoremata should reuse

1. Add a domain-specific formalization benchmark track for physics/quantum math.
2. Reuse the response-key model for storing multiple model outputs on one problem without destroying existing annotations.
3. Add domain-scaffold prompt support: common scaffold + task-specific formal context.
4. Add manual-eval metadata slots to Theoremata benchmark results for mixed objective/subjective scoring.

## Benchmark / eval value

High for broadening beyond olympiad/mathlib tasks into scientific mathematical reasoning. The formal Lean slots and domain scaffolds are immediately useful for autoformalization evaluation.

## Risks / gaps

- Some `solution_informal` fields already contain model outputs, not necessarily gold solutions.
- Quantum/scientific tasks may require PhysLean/Mathlib pins not matching our current Lean environment.
- Formal evaluation by typecheck alone may reward shallow snippets unless paired with statement-preservation checks.

## Adopt list

- P1: add QuantumLean loader with domain/type filters.
- P2: support prompt scaffolds in benchmark tasks.
- P2: add response-key metadata to result storage.
- P3: use as a specialized scientific-formalization stress track.

