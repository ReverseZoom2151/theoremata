# lean4code-main — resource-mining report

Repo: `resources/lean4code-main/lean4code-main`.

## Scope inspected

Large VSCodium-derived editor build repo: 8,908 files, including thousands of upstream TypeScript/JS/CSS/assets plus Lean4Code-specific top-level build scripts, README, `product.json`, docs, release scripts, and VSCodium upstream tree. Read Lean4Code README, build scripts/docs, product metadata, extension/telemetry docs, and catalogued the upstream editor tree.

## Core contribution

Lean4Code is a Lean-native VSCodium distribution intended to lower the entry barrier for Lean and LeanDojo tooling. README-listed features:

- built-in VSCode Lean4 extension,
- automatic LeanCopilot integration,
- one-click LeanDojo tracing,
- integrated agentic AI assistant.

The repo is mostly a product/build distribution rather than theorem-proving code.

## Architecture / data format

Top-level scripts handle:

- source preparation,
- upstream VSCodium update/versioning,
- asset/checksum preparation,
- platform builds,
- release packaging.

`product.json` and docs configure marketplace/extension behavior. Most files under `vscode/` are upstream editor source and not directly relevant to Theoremata’s CLI-first agent.

## What Theoremata should reuse

1. Do not build a desktop fork now; keep Theoremata CLI/TUI-first.
2. Borrow the onboarding idea: one-click LeanDojo tracing and integrated local setup checks.
3. Treat editor integration as a later thin extension/webview, not the core product.
4. If we build VS Code integration later, expose Theoremata through stable CLI/MCP APIs rather than forking an editor.

## Benchmark / eval value

Low as a math benchmark; medium as product-integration reference. It helps clarify what not to do in the current phase: editor distribution is heavy operational surface area.

## Risks / gaps

- Huge upstream VSCodium codebase, build complexity, platform-specific packaging.
- Extension marketplace/licensing/telemetry issues.
- Would distract from core theorem-proving harness if adopted prematurely.

## Adopt list

- P3: add `theoremata doctor` setup diagnostics inspired by Lean4Code onboarding.
- P4: expose CLI/MCP hooks that an editor extension could call.
- Defer any full desktop/editor build.

