# BRIDGE-main — resource-mining report

Repo: `resources/BRIDGE-main/BRIDGE-main`.

## Scope inspected

38 files: README/RUNNING/MODELS docs, dataset manifest and `datasets/bridge178.jsonl`, prompt docs/templates, scripts for prompt rendering, OpenAI-compatible generation, fenced-code extraction, error-correction loop, Lean evaluation sketch, Modal vLLM template, external benchmark adapters, and Lake project skeleton.

## Core contribution

BRIDGE studies verified program synthesis as a structured intermediate-representation problem across code, specifications, and proofs. The main dataset is BRIDGE-178: 178 algorithmic tasks with natural-language problem statements, Python/Lean signatures, and oracle tests.

Key idea: domain-guided intermediate representations improve final Lean code generation versus direct NL-to-Lean prompting. The repo supports direct, functional, imperative, spec, theorem, and proof prompt conditions plus verifier-error repair.

## Architecture / data format

`datasets/bridge178.jsonl` rows contain:

- `task_id`, `dataset_id`, `title_or_source_id`, `difficulty`, `tags`,
- `problem_statement`,
- `python` metadata,
- `lean` metadata including function signatures,
- `tests.inputs` and `tests.expected_outputs`.

Pipeline:

1. `render_prompts.py` fills Markdown templates and emits JSONL prompts.
2. `openai_compatible_generate.py` calls an OpenAI-compatible endpoint.
3. `extract_fenced_code.py` extracts final Lean/Dafny code blocks.
4. External checker runs compile/termination/oracle tests.
5. `error_correction_loop.py` feeds verifier stderr/stdout back for repair.

## What Theoremata should reuse

1. Add BRIDGE-178 to the benchmark harness as an executable Lean synthesis track.
2. Reuse the staged JSONL pipeline shape: task → prompt → generation → extraction → checker → repair.
3. Add verifier-error repair rows as first-class graph evidence instead of unstructured retry prompts.
4. Adopt prompt-condition experiments as a standard eval axis: direct vs functional/domain-guided vs theorem/proof.

## Benchmark / eval value

High for verified programming in Lean, complementary to theorem proving. It tests whether the agent can synthesize total Lean functions that satisfy executable oracle tests.

This is useful for Theoremata because mathematical agents need both proof search and verified executable subroutines.

## Risks / gaps

- The evaluation sketch leaves the Lean checker implementation to the caller; Theoremata must build the authoritative checker.
- Oracle tests are weaker than proof equivalence.
- Algorithmic synthesis may overfit to tests unless paired with theorem/spec generation.

## Adopt list

- P1: implement a BRIDGE loader and Lean oracle-test runner.
- P2: store repair-loop error messages as structured attempt records.
- P2: add direct/functional/theorem prompt variants to benchmark configs.
- P3: use BRIDGE as a verified-programming subtrack, not as a pure theorem benchmark.

