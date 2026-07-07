# IMO2025-main — resource-mining report

Repo: `resources/IMO2025-main/IMO2025-main`.

## Scope inspected

22 files: README, Lake project files, `HarmonicLean/Imports.lean`, statement-only files for IMO 2025 problems, and proof files for Problems 1, 3, 4, 5 plus an alternate P4 solution. Meaningful Lean/prose files were inspected.

## Core contribution

Harmonic’s public IMO 2025 Lean result repo. The README states Aristotle achieved gold-medal performance, solving 5 problems; the repo contains Lean statement/proof files for Problems 1–5.

The Lean files are substantial generated formalizations of Olympiad problems. Examples:

- `IMO2025P1.lean`: geometry/combinatorics on “sunny” lines; answer set `{0,1,3}`.
- `IMO2025P3.lean`: number theory on “bonza” functions; answer constant `4`.
- `IMO2025P4.lean`: divisor-sequence classification; answer `6 * 12^a * m` with `m` coprime to 10.

## Architecture / data format

Each problem file contains:

- original problem statement in a top comment,
- `import HarmonicLean.Imports`,
- aggressive proof options (`maxHeartbeats 0`, high recursion depth, full pretty-printing),
- local namespace per problem,
- definitions, lemmas, and final theorem/proof.

The `StatementOnly_*` files provide formal statement targets separate from proof-heavy files. This separation is directly valuable for benchmarking.

## What Theoremata should reuse

1. Use statement-only files as formalization/proof tasks and full files as reference solutions.
2. Add an Olympiad formalization track distinct from “answer-only” AIME: score by statement match, compile, axiom whitelist, and proof completion.
3. Use the local-definition-heavy generated style as a stress test for our hidden-helper fan-out budgeting.
4. Add a proof-option policy: allow per-task options, but record and audit options like `maxHeartbeats 0`.

## Benchmark / eval value

Very high. This is a compact, modern, high-signal benchmark for:

- NL-to-Lean statement construction,
- Lean proof search on Olympiad math,
- generated-helper management,
- theorem-level verification.

The statement/proof split makes it easier to evaluate partial systems.

## Risks / gaps

- Generated proofs can be brittle and expensive to compile.
- Some problem files contain `exact?`-style placeholders in inspected portions; the repo must be compiled before treating any file as solved.
- The statement-only formalizations need semantic review before being used as authoritative ground truth.

## Adopt list

- P1: ingest `StatementOnly_*` as proof obligations.
- P1: verify full problem files in CI or optional heavy suite.
- P2: add “formalization target + reference proof” benchmark schema.
- P3: mine tactic telemetry from these proofs for our proof strategy prior.

