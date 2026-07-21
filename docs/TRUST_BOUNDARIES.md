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

This table was audited on 2026-07-20 and five of its ten rows claimed a producer
that does not exist. The Status column now records what is actually written. A row
marked RESERVED is declared in `components/graph/evidence.rs` and emitted by nothing,
so an audit trail built from the database will not contain it. A drift guard in that
file fails if a reserved type gains a producer, or if a declared type has neither a
producer nor a reservation, so this table can no longer silently drift from the code.

| `evidence_type` | Producer | Status | Meaning |
|---|---|---|---|
| `lean_compile` | `agent.rs`, `observe.rs` | EMITTED | File typechecked |
| `axiom_audit` | none | RESERVED | The audit DOES run; its result is folded into the `lean_compile` verdict and the `formal_verify` payload, so no standalone row exists |
| `k_consecutive_clean` | `agent.rs` | EMITTED | N consecutive clean passes before certify |
| `hardening` | `agent.rs` | EMITTED | Adversarial battery on certified nodes |
| `falsification` | `agent.rs` | EMITTED | Numeric/symbolic counterexample screen |
| `retrieval` | `agent.rs` | EMITTED | Candidate lemmas (untrusted hints) |
| `external_prover_artifact` | the external-prover backends | EMITTED | Externally generated Lean + request provenance |
| `external_producer_checked` | `session/verify.rs` | EMITTED | Output locally re-verified (trust-but-verify) |
| `reformulation_check` | none possible | RESERVED | No FLARE or MILP code exists in the tree. Either build the track or delete the constant |
| `repair_loop` | `proving/repair.rs` | EMITTED | Structured verifier output from repair |
| `statement_citation` | `graph/citation.rs` | EMITTED | What a statement CLAIMS to encode, and where that claim comes from |

`statement_citation` is the one evidence type that is not a verdict about our own work.
It records an assertion made by whoever wrote a corpus: this statement is meant to encode
that published result. It NEVER contributes to a green. Its status lattice is two-valued,
`Unverified` (the default and the ceiling) or `Contradicted`, so there is no state meaning
"the statement does match its citation" for a gate to pattern-match a pass out of. Absence
of a citation is NOT a defect: most statements will have none, and there is deliberately no
"nodes missing citations" query, because a coverage query is the shape a coverage metric
grows out of and a coverage metric would turn absence into a finding.

Note also that `components/verify/hardening.rs` writes kind `lean_paranoia` with source
`hardening`, which is the two fields in the opposite order from `agent.rs`. It is easy
to misread as a `hardening` producer and is not one.

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
- **Evidence**: PLANNED, not written. The intent is a row of kind
  `external_prover_artifact` carrying request UUID, toolchain pin, input hash, output
  hash, and wall-clock duration. `components/graph/evidence.rs::external_prover_payload`
  is the intended builder and currently has zero callers, so no such row exists in any
  database today.

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
2. Add an evidence type or extend an existing one, and register it in
   `components/graph/evidence.rs::ALL`. This is now enforced: a declared type with
   neither a producer nor a `RESERVED_UNEMITTED` entry fails the drift guard.
3. If you declare a type you do not yet emit, add it to `RESERVED_UNEMITTED` and mark
   it RESERVED in the table above, so the documentation cannot claim an audit trail
   the database does not contain. That is the exact failure this section had.
4. Wire the acceptance gate in Rust before `FormallyVerified` status.
5. Update this file in the same PR.