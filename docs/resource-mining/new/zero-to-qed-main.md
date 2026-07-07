# zero-to-qed-main — resource-mining report

Repo: `resources/zero-to-qed-main/zero-to-qed-main`.

## Scope inspected

175 files: Lean source, mdBook/Typst docs, examples, illustrations, tests, SMT examples, and build/devcontainer config. Read README/BUILD, book chapter index and key chapters (`Proofs`, `Proof Strategy`, `Artificial Intelligence`), Lean examples in `src/ZeroToQED`, verification examples, tests, and illustration scripts.

## Core contribution

Zero to QED is an educational Lean book/project. It teaches Lean programming, theorem proving, tactics, proof strategy, dependent types, Mathlib, verified programs, model checking, SMT, and AI-assisted theorem proving.

For Theoremata it is most useful as a curriculum/source of proof-state pedagogy and beginner-to-intermediate proof-strategy templates.

## Architecture / data format

The repo pairs prose chapters with runnable Lean source:

- `docs/src/*.md`: conceptual chapters.
- `src/ZeroToQED/*.lean`: corresponding Lean examples.
- `src/ZeroToQED/ProofStrategy.lean`: tactic patterns and proof templates.
- `src/ZeroToQED/Verification.lean`: typed expression language, evaluator, constant folding, correctness theorem.
- `src/Examples/*`: verified programs and domain examples.

The book includes a chapter on the prover-verifier architecture and modern reasoning models.

## What Theoremata should reuse

1. Turn proof-strategy patterns into decomposer/critic rubrics: goal shape, hypothesis shape, tactic family, stuck-state response.
2. Add educational explanations to the TUI/CLI for why a proof attempt failed.
3. Use examples as small smoke tests for proof-state extraction and tactic telemetry.
4. Use the verified-program examples as lightweight benchmark items before larger datasets.

## Benchmark / eval value

Medium. It is not a research benchmark, but it is valuable for regression tests because examples are compact, varied, and pedagogically clean.

## Risks / gaps

- Educational examples may be too easy for serious model comparisons.
- Book prose is not an agent harness architecture.
- Need avoid conflating tutorial proof templates with robust automated proof search.

## Adopt list

- P2: extract tactic-decision guide into critic/decomposer prompts.
- P2: add small “Lean pedagogy smoke suite”.
- P3: add proof-state explanation mode in CLI/TUI using these patterns.

