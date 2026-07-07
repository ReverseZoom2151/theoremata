# Trust boundaries

Theoremata mixes untrusted producers (LLMs, Python workers, external provers) with
trusted acceptance gates (Lean compile, axiom audit, statement preservation,
LeanParanoia hardening). This document inventories what each layer is allowed to
trust and what must be re-checked locally.

Modeled after TorchLean's trust-boundary discipline and the resource-mining
synthesis in `docs/resource-mining/README.md`.

## Boundary map

| Layer | Trusts | Must not trust | Acceptance gate |
|---|---|---|---|
| **User / CLI** | Local config, SQLite DAG | Remote model output | Human approval for high-risk mutations |
| **Rust orchestrator** | DB invariants, scheduler, retry policy | Tool stdout as proof | Routes + attempt records + evidence |
| **Model provider** | Nothing downstream | Its own generations | Outputs are always `UntrustedText` |
| **Python worker** | AST allowlist in `safe_eval` | Arbitrary user expressions | Sandboxed eval only inside worker |
| **Retrieval** | Mathlib index files | Raw nearest-neighbor without import scope | Accessible-premise filter (import DAG) |
| **External prover** | Job status JSON from adapter | Returned Lean without local verify | Compile + axiom + statement guard + hardening |
| **Lean toolchain** | Kernel for checked proofs | `sorry`, `admit`, non-whitelist axioms | `#print axioms`, comparator, LeanParanoia |
| **Benchmark harness** | Corpus loaders + graders | Model responses | Track-specific rubric; formal tracks need statement preservation |

## Evidence types

Evidence rows (`graph.db::add_evidence`) record verdicts; they are not proofs.

| `evidence_type` | Producer | Meaning |
|---|---|---|
| `lean_compile` | LeanCheck / LeanSession | File typechecked |
| `axiom_audit` | Python `check_axioms` | Transitive axiom set within whitelist |
| `k_consecutive_clean` | Agent verifier | N consecutive clean passes before certify |
| `hardening` | LeanParanoia | Adversarial battery on certified nodes |
| `falsification` | Python falsifier | Numeric/symbolic counterexample screen |
| `retrieval` | Mathlib / accessible retrieve | Candidate lemmas (untrusted hints) |
| `external_prover_artifact` | Aristotle / Harmonic outputs | Externally generated Lean + request provenance |
| `external_producer_checked` | Any external producer | Output locally re-verified (trust-but-verify) |
| `reformulation_check` | FLARE / formulation bench | MILP reformulation equivalence attempt |
| `repair_loop` | BRIDGE-style verifier | Structured verifier stderr/stdout from repair |

## External producers

### LLM (model provider)

- **Role**: research, decomposition, formalization, critique, tactic proposals.
- **Boundary**: All Lean emitted by models is untrusted until `verify_source` passes.
- **Required checks**: compile, axiom whitelist `{propext, Quot.sound, Classical.choice}`,
  statement-change guard on prover round-trips, optional LeanParanoia when `harden_proofs`.

### Python tools

- **Role**: falsification, grading, retrieval, estimates, telemetry, benchmarks.
- **Boundary**: Worker runs with `-E`; expressions evaluated via AST allowlist only.
- **Required checks**: Never write to graph as `FormallyVerified` without Rust routing
  through Lean gates.

### External provers (Aristotle / Harmonic)

- **Role**: Long-running proof jobs (minutes to hours).
- **Boundary**: Async job state is operational truth; mathematical truth is local only.
- **Required checks**: `verify.rs` lexical gate, axiom gate, statement preservation,
  artifact directory under `.theoremata/artifacts/`, provenance JSON on `ProofResult`.
- **Evidence**: Record as `external_prover_artifact` with request UUID, toolchain pin,
  input hash, output hash, wall-clock duration.

## Statement-change guard

Borrowed from open-atp / Numina: before an external prover or model rewrite, snapshot
theorem/lemma headers in the candidate Lean file. After the round-trip, reject or
restore if declarations were deleted, renamed, or weakened.

Implementation: `components/prover/statement_guard.rs`.

## Axiom and oracle inventory

| Name | Kind | Whitelist? | Documented here |
|---|---|---|---|
| `propext` | Lean axiom | Yes | Lean standard |
| `Quot.sound` | Lean axiom | Yes | Lean standard |
| `Classical.choice` | Lean axiom | Yes | Lean standard |
| `sorryAx` | Tactic artifact | **No** | Residual `sorry` fails gate |
| Mathlib lemmas | Imported deps | Via transitive audit | `check_axioms` |
| Python `safe_eval` | Oracle | N/A | AST allowlist only |
| BRIDGE oracle I/O | Test oracle | N/A | Verified programming track only |
| Gurobi / execution baselines | External numeric | N/A | FLARE track; not proof truth |

## Artifact directories

Each `AttemptRun` persists under `.theoremata/artifacts/{project}/{attempt_id}/`:

- `input.json` — task payload
- `lean/solution.lean` — generated proof
- `logs/` — adapter stderr/stdout
- `verifier/report.json` — local verification summary
- `output.json` — terminal API response

Artifacts are operational audit records, not trusted certificates.

## Deferred / out of kernel

- Web viewer, editor extensions (no trust impact until wired).
- TorchLean CUDA/FFI/native runtime (reference architecture only).
- Live GPU training runs (GRPO/STaR scaffolds are dry-run only).
- Proofgrader LLM scores (ranking signal only; never acceptance).

## Maintenance

When adding a new tool, backend, or axiom:

1. Classify it in the boundary map above.
2. Add an evidence type or extend an existing one.
3. Wire the acceptance gate in Rust before `FormallyVerified` status.
4. Update this file in the same PR.