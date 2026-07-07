# lean-aristotle-mcp-main — resource-mining report

Repo: `resources/lean-aristotle-mcp-main/lean-aristotle-mcp-main`.

## Scope inspected

33 files: README, design docs, TODO, pyproject, Makefile, MCP server implementation, models/tools/mock modules, tests, live API tests, sample Lean project, and environment examples. Meaningful code/prose was read.

## Core contribution

An MCP server wrapping Harmonic’s Aristotle Lean theorem prover. It exposes external theorem proving/formalization as assistant tools:

- `prove`
- `prove_file`
- `formalize`
- `check_proof`
- `check_prove_file`
- `check_formalize`

It supports synchronous and async job modes, mock mode, API-key configuration, context files, and polling semantics.

## Architecture / data format

Core result dataclasses:

- `ProveResult(status, code, counterexample, project_id, percent_complete, message)`
- `ProveFileResult(status, output_path, project_id, percent_complete, message)`
- `FormalizeResult(status, lean_code, project_id, percent_complete, message)`

The server uses `FastMCP`, wraps `aristotlelib`, has local mock implementations, stores async job metadata with TTL, limits input sizes, canonicalizes paths, sanitizes API errors, and distinguishes “check status” from “save result”.

## What Theoremata should reuse

1. Add Aristotle/external-prover adapter behind our provider/tool abstraction.
2. Use async job nodes in the proof DAG: submitted → queued/in_progress → proved/partial/failed/counterexample.
3. Support “poll without saving” versus “poll and materialize result” semantics.
4. Reuse status taxonomy and counterexample path.
5. Keep mock mode for deterministic tests without API keys.

## Benchmark / eval value

High as an integration pattern. It is not itself a benchmark, but it defines a practical external-prover contract Theoremata can implement.

## Risks / gaps

- Requires external API key and network.
- Proof jobs can take minutes to hours; polling must be sparse and resumable.
- External proof output still requires local verification/hardening.
- The MCP project is “vibe-coded” per README; use as reference, not unquestioned dependency.

## Adopt list

- P1: add external prover job table/state machine.
- P1: implement an Aristotle adapter with mock and live modes.
- P2: add sparse polling scheduler and resume support.
- P2: pipe returned code through compile, axiom, escape, and LeanParanoia gates.

