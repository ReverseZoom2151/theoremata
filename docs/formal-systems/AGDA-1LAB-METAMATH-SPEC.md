# Agda, 1Lab, and Metamath Integration Specification

Status: implementation hardening specification, 2026-07-10.

This document separates the authoritative checker, source policy, corpus
ingestion, and retrieval layers. A corpus is never trusted merely because it
came from a public website.

## Agda

### Role

Agda is a constructive, dependently typed proof assistant. The authoritative
operation is type-checking a module with a pinned Agda executable and library
path. Compilation to Haskell or JavaScript is optional execution output, never
the proof-verification result.

### Accepted artifact

- `.agda` and `.lagda` source files.
- The top-level module name must agree with the path layout (`A.B` in
  `A/B.agda`).
- The workspace records the source hash, Agda version, library roots, command
  arguments, and generated `.agdai` interface files.

### Trust policy

The default gate runs safe mode and fails closed on:

- `postulate`, `primTrustMe`, unsolved metas, `--type-in-type`, `--unsafe`;
- disabled termination, positivity, coverage, or universe checks;
- imports or FFI that are outside the declared project closure.

`COMPILED*` and `IMPORT` pragmas are warnings requiring an explicit tainted-FFI
policy. They are not proof axioms, but they can change compiled execution.
Imported postulates must be reported transitively, not only lexically scanned.

### Operations

1. Probe `agda --version` and record the exact version.
2. Type-check with `--safe` plus the project’s explicit library configuration.
3. Optionally run `--interaction-json` for goal/error feedback; interaction
   output is advisory and never replaces batch type-checking.
4. Re-run batch checking in a fresh workspace for the kernel-recheck layer.

### Tests

- Clean constructive module passes.
- Comment/string occurrences of unsafe words do not trigger the scanner.
- Every unsafe pragma and postulate is rejected.
- Literate modules, module/path mismatch, missing imports, and unsolved metas
  fail closed.
- Tool version, source hash, library closure, and command are persisted.

## 1Lab

1Lab is an Agda library and documentation corpus, not a separate proof kernel.
The ingestion layer indexes module declarations, source paths, imports, and
dependency closure. It must preserve the 1Lab checkout revision and Agda
version/configuration used to check it. Cubical modules must be tagged and
checked with their required flags/library roots rather than treated as ordinary
Agda modules.

The benchmark loader returns formalization tasks and source provenance; the
Agda backend performs the actual verification. Documentation pages are
retrieval material, not proof artifacts.

## Metamath

### Role

Metamath is an explicit substitution-proof language with a small checker. The
authoritative operation is batch verification of the loaded database, not a
regex over `$p` labels.

### Accepted artifact

- `.mm` databases with nested `$[ file $]` includes.
- `$a` statements are database axioms/hypotheses and must be recorded, not
  blindly rejected.
- `$p` statements carry explicit proof tokens; compressed proofs require the
  Metamath checker to decode them.
- The dependency closure must include included files and referenced labels.

### Operations

Use the noninteractive command sequence equivalent to:

```text
metamath "read set.mm" "verify proof *" exit
```

The backend records the executable version, database hash, include closure,
command, resource limits, and verifier output. A second checker such as
`mmverify.py` or `smetamath-rs` is an optional independent cross-check; it is
not silently substituted for the configured primary checker.

### Tests

- A small database with `$a` and `$p` verifies.
- A tampered proof, label, include, or compressed proof fails.
- Missing/cyclic includes fail closed.
- A database with ordinary `$a` declarations is not incorrectly rejected.
- The benchmark loader indexes only parsed `$p` statements and preserves the
  database revision/path provenance.

## Common contract

Every result includes:

```json
{
  "system": "agda|metamath",
  "checker": {"binary": "...", "version": "...", "command": ["..."]},
  "source_sha256": "...",
  "dependency_sha256": "...",
  "limits": {"timeout_seconds": 300, "max_output_bytes": 16777216},
  "verified": true
}
```

Missing tools, unknown versions, malformed sources, timeout, output truncation,
unsafe policy violations, and incomplete dependency closures are failures, not
successful skips. Mock backends may be used only in tests and must be marked in
the result provenance.

## Implementation sequence

1. Complete scanners and parser/dependency reports.
2. Add version/hash/provenance to `Workspace`, `CompileReport`, and
   `RecheckReport`.
3. Add live Agda safe-mode and Metamath batch integration tests behind tool
   availability gates.
4. Add 1Lab and Metamath corpus fixtures with pinned revisions.
5. Run mock tests, installed-tool tests, tamper tests, and CI matrix jobs.
