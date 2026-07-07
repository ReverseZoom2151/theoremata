# datasets-main — resource-mining report

Repo: `resources/datasets-main/datasets-main`.

## Scope inspected

6 files: root README/LICENSE plus MiniF2F README and train/validation/test JSON files.

## Core contribution

Harmonic’s Lean 4 MiniF2F variant. The README describes improvements over older MiniF2F splits:

- Lean 4 translations,
- random train/validation/test split,
- associated natural-language statements,
- corrected many bad formalizations,
- known issue: 3 missing train problems.

Local counts:

- train: 389 records
- validation: 48 records
- test: 48 records
- total present: 485 records

Each record has `id`, `name`, `natural`, and `formal`.

## Architecture / data format

JSON arrays of problem objects. `formal` fields are Lean theorem statements ending in `by sorry`. This is directly suited to proof completion and statement-preservation grading.

Example shape:

- natural-language olympiad/math contest problem,
- Lean theorem with explicit variables/hypotheses,
- expected theorem name/id.

## What Theoremata should reuse

1. Add this MiniF2F split as a standard formal theorem-proving benchmark.
2. Use `natural` + `formal` pairs for autoformalization evaluation.
3. Use `formal` theorem bodies as proof-completion tasks.
4. Track problem `name` and `id` as stable benchmark identifiers.

## Benchmark / eval value

Very high as a canonical formal-math benchmark. It fills the gap between answer-only AIME and large research formalization repos.

## Risks / gaps

- It is not enough to pass Lean by changing the statement; statement preservation must be enforced.
- The three missing train items mean counts differ from the nominal 488.
- Need exact Lean/mathlib pin before bulk checking.

## Adopt list

- P1: build MiniF2F loader and proof-completion runner.
- P1: wire statement-preservation comparator.
- P2: add train/validation/test split support to benchmark CLI.

