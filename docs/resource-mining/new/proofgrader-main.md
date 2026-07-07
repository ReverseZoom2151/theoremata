# proofgrader-main resource mining report

## Scope and files inspected

Resource path: `resources/proofgrader-main/proofgrader-main`.

Inventory: 66 files, about 720 KB. Text inventory is 64 files and about 15k lines: 33 Python files, 6 Markdown guides, 5 YAML templates, JSONL/CSV sample outputs, shell scripts, and requirements/setup files.

Inspected in detail:

- `README.md`, `guides/ARCHITECTURE.md`, `guides/EVALUATION_OUTPUTS_STRUCTURE.md`, `guides/MARKING_SCHEMES_GUIDE.md`, `EXPERT_GRADINGS_FORMAT.md`.
- Scripts: `scripts/generate.py`, `scripts/generate_marking_schemes.py`, `scripts/evaluate.py`.
- Core: `proofgrader/api_client.py`, `inference.py`, `workflow_runner.py`, `data_validation.py`, `dataset_handler.py`, `prompt_formatter.py`.
- Workflows: `single.py`, `decompose_then_judge.py`, `repeat_and_aggregate.py`, `reflect_and_revise.py`, `utils.py`.
- Metrics/dashboard code and output examples under `data/test_data/evaluation_outputs/metrics`.
- Templates: `templates/evaluation.yaml`, `generation.yaml`, `workflows.yaml`, `binary.yaml`.

Generated/bulk artifacts:

- `data/test_data/problems.jsonl`, `model_solutions.jsonl`, and `expert_gradings.jsonl` are Git LFS pointer files in this checkout, not actual datasets.
- `data/test_data/evaluation_outputs/*` contains small checked-in sample outputs and metrics CSV/JSON.
- Top-level `api_client.py`, `config.py`, `dataset_handler.py`, `prompt_formatter.py`, `vllm_client.py` duplicate package files/legacy entry points.

## Core idea

ProofGrader is an LLM proof-grading experiment harness. It separates expensive candidate generation from evaluator experiments: generate model solutions once, then run many evaluator workflows over the same solution set and compare evaluator scores to expert gradings.

## Reusable architecture/code patterns for Theoremata

- Strong generate/evaluate separation: candidate production is immutable input to evaluator sweeps.
- JSONL schemas for problems, generated solutions, evaluator raw outputs, per-generator parsed gradings, and expert scores.
- Workflow plugins:
  - single-shot grading;
  - decompose proof into steps, then judge;
  - repeat and aggregate with mean/min/max/discrete median/consistency;
  - reflect-and-revise with initial report, critique, and final verdict.
- Robust parser layer that accepts XML, JSON, fenced JSON, and lenient score extraction.
- Composite IDs of the form `<problem_id>::<generator>` for unambiguous evaluator-output routing.
- Evaluator metrics: MAE, RMSE, bias, Pearson, Spearman, within-tolerance accuracy, pairwise order accuracy, macro-by-problem metrics, normalized-by-problem metrics.
- Data validation pass before generation/evaluation.

## Benchmark/eval value

Medium-to-high, but as a grader-evaluation harness rather than a proof verifier. The metrics are directly useful for calibrating LLM critics against human or formal labels. For Theoremata, this is best used to evaluate auxiliary critic/scoring modules, not to decide proof truth.

The sample metric output is revealing: even a configured evaluator can have substantial bias and weak correlation with expert scores, which reinforces Theoremata’s need for formal verification as the acceptance gate.

## Gaps and risks

- LLM grading is not soundness. Scores and XML assessments are useful signals but not proof certificates.
- Main datasets are LFS pointers in this checkout; the full benchmark content is absent unless LFS is fetched.
- API clients have hardcoded assumptions such as a default Vertex project/location and current model names.
- Parser is intentionally forgiving, which is good for salvage but risky if used as a strict data contract.
- Duplicate top-level/package modules create maintenance drift risk.
- Several workflows write output directories and dashboards; isolate them if reused in Theoremata.

## Concrete integration recommendations

1. Reuse the evaluator workflow shell and metrics code to evaluate Theoremata critics against formal labels.
2. Treat LLM scores as ranking/triage signals only; final acceptance must remain compile/sorry/axiom/DAG verification.
3. Adopt composite IDs and per-generator output directories for all multi-model evals.
4. Replace free-form XML/JSON with a stricter schema in new Theoremata integrations, while keeping lenient parsing for legacy runs.
5. Add formal-verification fields to the metrics tables: compile success, sorry-free, axiom-clean, statement-preserved, node completed.
