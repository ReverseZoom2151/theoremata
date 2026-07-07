# TorchLean-main — resource-mining report

Repo: `resources/TorchLean-main/TorchLean-main`.

## Scope inspected

Large Lean framework: 1,409 files, including 1,157 Lean files, 71 Markdown files, 50 Python scripts, CUDA/C sources, blueprint/site docs, trust-boundary docs, Lake files, and formalization metadata. Read README, `TRUST_BOUNDARIES.md`, `AI_USAGE.md`, `formalization.yaml`, subsystem README/index files, and representative Lean files in tensor/API/IR/runtime/proofs/verification.

## Core contribution

TorchLean is a Lean 4 framework for formalizing neural-network programs: typed tensors, model APIs, graph IR, executable runtime/autograd, finite-precision models, certificate checkers, CUDA/runtime boundaries, verification examples, and documentation.

For Theoremata, the most important artifact is not “neural nets” per se; it is the trust-boundary discipline around mixing Lean proofs, executable checkers, external numeric oracles, Python/Julia producers, CUDA/FFI, and generated artifacts.

## Architecture / data format

Key subsystems:

- `NN/Tensor`: shape-indexed tensors and constructors.
- `NN/Spec`: mathematical semantics for tensors/layers/models.
- `NN/IR` and `NN/GraphSpec`: graph IR, typed architecture descriptions, denotation.
- `NN/Runtime`: executable autograd, optimizers, training loops, external runtime bridges.
- `NN/Floats`: finite-precision and interval arithmetic infrastructure.
- `NN/Verification`: certificate checkers for robustness/PINN/ODE/splines/LiRPA.
- `scripts/checks`: repo lint/dependency audit/trust-boundary checks.

Trust model explicitly distinguishes Lean kernel proofs, executable checkers, Prop-valued contracts, FFI/native runtime, and external producers.

## What Theoremata should reuse

1. Adopt a first-class trust-boundary manifest for every non-kernel component.
2. Add “external producer, Lean checker” as an explicit evidence pattern.
3. Reuse certificate-checker architecture for numerical/asymptotic/optimization subdomains.
4. Add dependency/axiom audit docs generated from code, not only prose.
5. Borrow shape/IR discipline for proof-DAG and tactic-trace artifacts.

## Benchmark / eval value

Medium-high, but specialized. TorchLean is too large to ingest wholesale, yet its verification examples and certificate-checker patterns are valuable for testing Theoremata on computational mathematics and ML verification.

## Risks / gaps

- Very large dependency/build surface, Lean v4.31.0 pin, optional CUDA/native components.
- External numeric oracles and FFI are explicitly outside the proof kernel.
- Direct vendoring would bloat Theoremata; treat it as reference architecture and optional benchmark source.

## Adopt list

- P1: create `docs/TRUST_BOUNDARIES.md` for Theoremata.
- P2: add evidence type `ExternalProducerCheckedCertificate`.
- P2: add repo-lint rule requiring every axiom/oracle to appear in trust docs.
- P3: add optional TorchLean verification fixtures only after version pinning.

