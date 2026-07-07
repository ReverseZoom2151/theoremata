# aristotle_putnam25-main — resource-mining report

Repo: `resources/aristotle_putnam25-main/aristotle_putnam25-main`.

## Scope inspected

Small Harmonic/Aristotle result repo: 24 files total, including `README.md`, 10 input LaTeX problem statements under `inputs/`, 10 generated Lean proofs under `aristotle_outputs/`, `lakefile.lean`, `lean-toolchain`, and `lake-manifest.json`.

The meaningful files were read. Build artifacts were not present.

## Core contribution

The repo is a public record of Aristotle solving 10 of 12 Putnam 2025 problems. The README records rough wall-clock times from 25 minutes to 7 hours. Each generated proof file includes:

- a generation provenance header with Lean/mathlib versions and request UUID,
- the informal problem restated as comments,
- Lean definitions chosen by the prover,
- a full proof attempt/proof script.

Example: `aristotle_putnam25_a1.lean` defines a recurrence over pairs of naturals and proves eventual coprimality of `2m_k+1` and `2n_k+1`.

## Architecture / data format

This is not a reusable harness; it is an output corpus:

- `inputs/*.tex`: raw LaTeX problem statements submitted to Aristotle.
- `aristotle_outputs/*.lean`: model-generated Lean files.
- Lake project pins Lean `v4.24.0` / Mathlib via manifest.

The generated files use `import Mathlib`, `set_option maxHeartbeats 0`, `noncomputable section`, many intermediate lemmas, and tactic-heavy proofs (`aesop`, `linarith`, `grind`, etc.).

## What Theoremata should reuse

1. Treat Aristotle outputs as an external-prover benchmark track: NL/LaTeX problem → generated Lean → Theoremata verification/hardening.
2. Persist external-prover provenance in our graph evidence table: service, request id, Lean version, Mathlib commit, input hash, output hash, wall-clock time.
3. Add a long-running external-prover job abstraction rather than synchronous blocking; the wall-clock times show proof jobs can be multi-hour.
4. Use these as regression tests for our “trust but verify” stance: Aristotle output must still pass compile, axiom whitelist, `sorry`/escape scan, and LeanParanoia.

## Benchmark / eval value

High as a realistic external-prover corpus. The task distribution is harder and more varied than toy formalization stubs. It is useful for:

- statement extraction from LaTeX,
- generated-proof verification,
- proof telemetry comparison against our internal agent,
- provenance-aware scoring.

## Risks / gaps

- Generated proof style is not guaranteed to be maintainable.
- Some proofs may rely on high-heartbeat/global options that make CI expensive.
- This is a solved-output corpus, not a ground-truth harness with hidden tests.
- License/provenance of contest statements and generated outputs should be checked before redistribution.

## Adopt list

- P1: add an “external prover artifact” evidence type with request provenance.
- P2: ingest the 10 output Lean files as verification/hardening fixtures.
- P2: add wall-clock/cost fields to proof-attempt telemetry.
- P3: compare our proof-DAG decomposition against Aristotle’s generated lemma structure.

