# open-atp-main resource mining report

## Scope and files inspected

Resource path: `resources/open-atp-main/open-atp-main`.

Inventory: 298 files, about 4.5 MB total. Text/code inventory is about 286 files and 58k lines: 97 Python files, 132 Markdown files, 23 shell scripts, 6 Lean examples/fixtures, plus TOML/YAML/JSON/docs. I made a full pass over the authored package/docs/tests and a targeted pass over the large vendored skill trees.

Inspected in detail:

- Top-level docs/config: `README.md`, `AGENTS.md`, `pyproject.toml`, `Makefile`, `docs/index.md`, `docs/guides/*`, `docs/provers/*`, `docs/api/*`, `docs/provers.yaml`.
- Core package: `src/open_atp/lean.py`, `verify.py`, `benchmark.py`, `config.py`, `__main__.py`, `provers/base.py`, `provers/agent_prover.py`, `provers/numina.py`, `provers/numina_tracker.py`, `harness/base.py`, concrete harness files, backend files, examples, and image helpers.
- Tests/fixtures: API, CLI, benchmark, verifier, prover, harness, and backend tests plus bundled Lean examples.
- Vendored material catalogued and sampled at entry points: `vendor/numina/VENDOR.md`, `vendor/numina/prompts/main_entry.md`, Numina skills index; `vendor/lean4-skills/VENDOR.md`, plugin `README.md`, `/lean4:autoprove` command, key references; `vendor/leanprover-skills/VENDOR.md`.

Generated/bulk artifacts:

- `uv.lock` (~571 KB), `banner/banner.png` (~954 KB), `docs/brand/logo.ai` (~396 KB), SVG/logo assets.
- Vendored Lean skill/plugin trees are third-party snapshots rather than OpenATP-native source; they are highly relevant but should be tracked as vendored provenance, not copied blindly.

## Core idea

OpenATP is a common interface for agentic Lean theorem proving. It turns a Lean task into a complete lake project, runs a prover in an isolated Docker or Modal sandbox, captures logs/cost/files, and verifies the resulting project through a shared verifier. The main abstraction split is clean:

- `LeanProject` / `ProofTask`: input contract.
- `ComputeBackend` / `ComputeSession`: where commands run.
- `Harness`: how a specific agent CLI is launched/authenticated/log-parsed.
- `AutomatedProver` / `ProofResult`: lifecycle and output schema.
- `run_benchmark`: task × prover matrix execution.

## Reusable architecture/code patterns for Theoremata

- Adopt the `LeanProject` + `ProofTask` shape: complete project, pinned toolchain, optional target files, optional task-specific prompt, metadata.
- Reuse the backend/session separation. The persistent hot session pattern is exactly right for Theoremata: agent generation and final verification run in the same sandbox without paying a second startup.
- Borrow the harness boundary. Agent-specific launch/auth/log parsing should stay outside the proof-task and verification logic.
- Borrow the `ProofResult` schema: verifier verdict, changed Lean files, output/log directories, duration, cost, and harness metadata.
- Borrow benchmark matrix execution with `max_per_prover` concurrency gates and per-cell artifact directories.
- Borrow Numina’s statement-change guard concept: snapshot theorem/lemma headers before a run, reject or restore deleted/weakened statements after each round.
- Borrow the vendored-skill provenance model: pin upstream commit, list copied assets, record local adaptations and resync procedure.

## Benchmark/eval value

High. OpenATP can be both a source of baselines and a harness-design reference:

- It supports FATE-H/M/X, PutnamBench, and examples as Lean proof-synthesis datasets.
- It already records per-run cost/time/success and writes reproducible artifacts.
- The standard prover catalog gives immediate comparisons against Claude Code, Codex, OpenCode, Leanstral/Vibe, AxProverBase, Numina, and Aristotle, subject to credentials.

## Gaps and risks

- The verifier’s docstring says it extracts `#print axioms`, but `_compile_script` currently only runs `lake env lean "<file>"`; it parses axiom lines if they appear in logs but does not inject axiom queries. Fix before relying on its axiom gate.
- Verification is project-level binary success/failure, not proof-DAG/node-level progress.
- Statement tracking is regex/indentation based and may miss Lean syntax edge cases.
- `create_project` copies bare `.lean` files by basename into one root, so same-named files from nested directories can collide.
- Heavy dependence on external CLIs, credentials, Docker/Modal, and vendored tools.

## Concrete integration recommendations

1. Port the object model: `LeanProject`, `ProofTask`, `ProofResult`, `VerificationReport`, and `run_benchmark`-style artifact layout.
2. Fix and harden the axiom check before adoption: inject `#print axioms` per target declaration or wire to Theoremata’s stronger verifier.
3. Keep `Harness` as an adapter layer and make Theoremata’s planner/prover logic independent of Codex/Claude/OpenCode specifics.
4. Use OpenATP datasets as external benchmark baselines, but add Theoremata’s DAG/node telemetry and proof repair traces.
5. Import the statement-change guard as a safety layer, then replace its parser with a Lean-aware extractor when possible.
