# LeanMillenniumPrizeProblems-main — resource-mining report

Repo: `resources/LeanMillenniumPrizeProblems-main/LeanMillenniumPrizeProblems-main`.

## Scope inspected

47 files: README, umbrella `Problems.lean`, 20 Lean files under `Problems/`, 10 Clay PDF references, scripts for downloading/verifying references, Lake project files, license/citation, and supporting docs. Read README, scripts docs, umbrella import, and representative Lean statements/definitions across all problem areas.

## Core contribution

Formal Lean 4 statements of the Clay Millennium Prize Problems. The repo states it is not a solution repository; it aims to make the official statements precise and machine-checkable while staying `sorry`-free and free of user axioms.

Main statements include:

- `Millennium.PEqualsNP`
- `Millennium.RiemannHypothesis`
- `MillenniumNavierStokes.NavierStokesMillenniumProblem`
- `MillenniumHodge.HodgeConjecture`
- `MillenniumBirchSwinnertonDyer.BirchSwinnertonDyerConjecture`
- `MillenniumYangMills.YangMillsExistenceAndMassGap`
- `MillenniumPoincare.PoincareConjecture3`

## Architecture / data format

Each problem has a `Millennium.lean` plus supporting files. When Mathlib lacks foundations, the repo parameterizes statements over explicit data packages rather than adding fake axioms. Examples:

- Hodge uses `HodgeData`.
- BSD uses `ClayLSeriesData`.
- Yang–Mills uses bundled `QuantumYangMillsTheory`.
- Navier–Stokes splits domain variants and PDE scaffolding.

Safety check described in README: SafeVerify over compiled `.olean` files with permitted axioms `propext`, `Quot.sound`, and `Classical.choice`.

## What Theoremata should reuse

1. Use these as long-horizon target statements, not proof tasks expected to solve soon.
2. Adopt the “parameterize missing foundations” pattern for honest formalization.
3. Add source-reference attachment support: theorem statement linked to official PDF/reference files.
4. Add a benchmark tier for statement-quality/definition-design, separate from proof completion.

## Benchmark / eval value

High for formalization quality and target management; low for near-term proof success. It is useful for evaluating whether Theoremata can preserve statement fidelity, identify missing foundations, and create honest dependency/data packages.

## Risks / gaps

- Solving the statements is not a realistic near-term eval.
- Some formulations are necessarily modeled/parameterized due Mathlib gaps.
- The Clay PDFs are reference material with separate licensing/provenance considerations.

## Adopt list

- P1: add “statement target” benchmark type without expected proof.
- P2: add reference-file provenance fields.
- P2: teach decomposer to emit explicit data-package obligations rather than axioms.
- P3: use Millennium statements as roadmap targets for long-term demos.

