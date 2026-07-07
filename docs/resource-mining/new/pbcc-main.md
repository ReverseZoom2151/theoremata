# pbcc-main resource mining report

## Scope and files inspected

Resource path: `resources/pbcc-main/pbcc-main`.

Inventory: 15 files, about 320 KB. Text inventory is 12 files and about 6.7k lines: Python compiler, C++ support/templates, one `.proto`, README, CI, and project config.

Inspected in detail:

- `README.md`, `pyproject.toml`, `.github/workflows/ci.yml`.
- Compiler: `pbcc/compile.py`.
- Runtime/templates: `pbcc/pymodule.support.{hh,cc}`, `pymodule.root.in.cc`, `pymodule.impl.in.{hh,cc}`.
- Test schema and test harness: `pbcc/test.proto`, `test.py`.

Generated/bulk artifacts:

- `uv.lock` is generated dependency lock data.
- `test.py` is an authored but very large direct-run regression harness rather than pytest.
- `.build` directories, generated `.cc`, `.o`, `.so`, and `.pyi` files are expected runtime outputs when `pbcc.compile` runs; none are checked in.

## Core idea

pbcc compiles Protocol Buffer message definitions into a fast Python C-extension module plus `.pyi` annotations. It parses descriptors using `grpc_tools.protoc`, builds an internal schema graph, renders C++ templates, compiles/link them, and verifies compatibility against Google’s protobuf implementation.

## Reusable architecture/code patterns for Theoremata

- Descriptor-driven schema collection with comments preserved into generated types.
- Small code-generation IR: `EnumInfo`, `FieldInfo`, `MessageInfo`, `ModuleInfo`, `ModuleCollection`.
- Template engine with explicit compiler tags in comments, nested foreach/if blocks, and optional C++ `#line` directives for debuggability.
- Async subprocess wrappers that capture output and kill children reliably.
- `TemporaryImportSearchPath` pattern for importing generated descriptor modules during compilation.
- Runtime unknown-field retention/deletion as a forward-compatibility design.
- Exhaustive cross-compatibility tests: allowed/disallowed values, serialization round trips, pickle, equality, unknown fields, wrong wire types, field ordering, repr truncation.

## Benchmark/eval value

Low as a theorem-proving resource. The valuable part is engineering discipline: schema-generated artifacts and exhaustive compatibility tests. If Theoremata later needs a compact binary protocol for proof traces, run results, or distributed worker messages, pbcc is a useful case study.

## Gaps and risks

- Not math/proof specific.
- C-extension maintenance cost is high; CPython and platform details matter.
- The runtime assumes little-endian systems and C++20/CPython APIs.
- It supports protobuf messages but not gRPC service definitions.
- Some behavior is intentionally specialized (`oneof` Python type disambiguation, map “cheese it” shortcut).
- Direct-run test harness compiles modules at import time, so it is awkward for normal CI integration.

## Concrete integration recommendations

1. Do not integrate pbcc’s runtime unless Theoremata has a measured serialization bottleneck.
2. Borrow the schema-first pipeline for proof/run artifact contracts: generate types, validate field comments, preserve unknown fields.
3. Borrow the template-engine idea only if codegen is needed; otherwise prefer standard protobuf/JSON Schema/Pydantic.
4. Copy the test philosophy: cross-check every generated artifact against a reference implementation and include malicious/wrong-type fixtures.
5. Use unknown-field retention as a design principle for long-lived benchmark result files.
