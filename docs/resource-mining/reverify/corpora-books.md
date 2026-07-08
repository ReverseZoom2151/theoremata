# Re-verification: corpora & books

Independent re-scan of three large/book repos against the Codex reports in
`docs/resource-mining/new/`. All repo content treated as untrusted data. Paths are
relative to `resources/<repo>/<repo>/` unless noted. Do not commit (per task).

---

## lean4code-main  —  report SOLID (minor over-statements)

Codex verdict ("VSCodium fork, not theorem-proving code, keep CLI-first, don't fork an
editor") is **correct**. My independent scan confirms and sharpens it.

### Captured correctly
- It is a build/distribution repo, not a prover. The `.rs` files (71) are the stock
  upstream VS Code CLI (`vscode/cli/src/**`, tunnels/auth/update), nothing bespoke.
- No usable Lean corpus: only **11 `.lean` files**, all trivial test fixtures inherited
  from the lean4 VS Code extension (`vscode/extensions/lean4/test/test-fixtures/**` —
  `Main.lean`, `factorial.lean`, empty lakefiles). Zero mathematical content.
- Correct recommendation: don't fork an editor; borrow only the onboarding idea.

### MISSED / needs correction
- **This is a near-verbatim clone of the VSCodium build repo**, not a bespoke project.
  Evidence: `docs/index.md` links out to `github.com/VSCodium/vscodium/blob/master/docs/*`
  for every page; `patches/` is the standard VSCodium patch set (`brand.patch`,
  `disable-telemetry`-style, `remove-mangle.patch`, `policies.patch`, platform subdirs);
  `upstream/stable.json` pins stock VS Code `1.100.3`. Codex described "docs, product
  metadata" as if Lean4Code-authored — they are stock VSCodium. There is essentially
  **nothing Lean4Code-specific to mine** beyond `product.json` and a couple of patches.
- The README's "**integrated agentic AI assistant**" (which Codex repeated as a feature)
  is not novel agent logic: it is bundled **GitHub Copilot / Copilot Chat**. Evidence:
  `product.json` allowlists `GitHub.copilot`, `GitHub.copilot-chat`,
  `ms-vscode.vscode-websearchforcopilot`, `vscode-copilot-vision`; `patches/chat.patch`
  merely flips `chat.commandCenter.enabled` default `true→false`. No agent to reuse.
- The "LeanDojo/LeanCopilot integration" is a `product.json` extension allowlist plus a
  welcome-screen walkthrough — no code artifact to port.

### Adoptables (net)
- Only the P3 `theoremata doctor` onboarding idea survives, exactly as Codex said. I'd
  **downgrade** the repo further: it carries no Lean corpus, no agent, no reusable code.
  Treat as "confirmed dead end" rather than "medium product-integration reference."

---

## TorchLean-main  —  gaps: Verso blueprint, BugZoo, concrete lint/audit tooling, sandbox comparator

Codex captured the trust-boundary thesis well but **missed the tooling that makes it real**
and missed that the blueprint is a Verso book — directly on-point for the task's questions.

### Captured correctly
- Core thesis (trust-boundary discipline over mixed Lean/checker/FFI/oracle/producers) is
  right and is the repo's most valuable idea for Theoremata.
- `TRUST_BOUNDARIES.md` exists and is rich (154 lines): named Lean axioms
  (`crown_oracle`, `instNonemptyBuffer`), Prop-valued contracts, CUDA/FFI boundaries,
  external oracles (Arb/`python-flint`, CROWN, Julia, PyTorch).
- `sorry`-free status, `formalization.yaml` project manifest, `AI_USAGE.md` disclosure all
  present. P1/P2 adopt items (create our own trust doc; add an
  "external-producer, Lean-checker" evidence type) are validated by real code.

### MISSED (with cites)
- **The blueprint is a full Verso manual** — this is the "Verso-for-blueprint idea" the
  task asked to verify, and Codex only called it "blueprint/site docs."
  Cites: `blueprint/lakefile.toml` requires `verso` (`leanprover/verso` `v4.31.0`) and
  `subverso`; `blueprint/TorchLeanBlueprintMain.lean` uses `import VersoManual` /
  `manualMain (%doc ...)`; chapters are literate Lean under
  `blueprint/TorchLeanBlueprint/Guide/Ch1_Introduction/*.lean` … `Ch6_Conclusion` using
  `#doc (Manual) "…" =>` with `%%% tag := … %%%` metadata. There is a `blueprint-gen`
  Lean executable target. **This is a concrete, working template for a Verso blueprint**
  (Lean-source-of-truth prose with cross-refs to `NN/**` source) — highly adoptable for
  Theoremata's blueprint / proof-DAG documentation.
- **The trust doc's enforcement mechanism**, not just the doc. `scripts/checks/repo_lint.py`
  **allowlists the exact axiom names** and fails CI on any new `axiom`/`opaque`; the doc
  ships the audit recipes `#print axioms <name>` and
  `rg -n "^(noncomputable\s+)?opaque |^axiom " NN -g'*.lean'`. Codex's P2 "repo-lint rule
  requiring every axiom in trust docs" already exists here as a portable script pattern.
- **Ready-made audit/lint tooling** Codex's "generate audits from code" P-item ignored as
  existing: `scripts/checks/dependency_audit.py`, `repo_lint.py`, `TorchLeanLint.lean`
  (Lake Lean-lint driver), `check_case_collisions.py`, `example_smoke.sh`, and a
  `lake build NN.CI.All` CI surface — all directly portable to Theoremata's harness.
- **BugZoo** — external-framework bug reproducers paired with *checked* Lean case studies
  (`scripts/bug_zoo/constant_norm_slice_repro.py`, `layernorm_dim1_repro.py`; contact-sheet
  assets under `blueprint/.../Guide/Assets/bug-zoo/`). This is a graded "real producer bug →
  Lean checker catches it" set: a curriculum/benchmark seam and a live instance of the
  external-producer/checker evidence pattern. Codex missed it entirely.
- **Untrusted-Lean comparator / sandbox**: `scripts/sandbox/run_comparator.py` +
  `scripts/comparator/nn_ci_all.json` implement a "comparator/untrusted-Lean helper" — a
  sandboxed-checking pattern relevant to Theoremata's own untrusted-proof-checking + trust
  boundary. Not mentioned by Codex.
- **`NN/Verification/` is a benchmark seam** Codex under-specified: it contains a
  `VNNComp/` (VNN-COMP is a real neural-net verification *competition* benchmark), plus
  `Robustness`, `LiRPA`, `PINN`, `ODE`, `Splines`, `Geometry3D`, `Cert`, a
  `ProofBackedCertificates.lean`, and a `CLI.lean` verification entrypoint. These are
  concrete certificate-checking fixtures (the P3 item), and VNNComp is a named external
  benchmark to consider.
- Minor: `scripts/rl/` is a PPO/Gymnasium RL bridge (`train_ppo_cartpole_sb3.py`,
  `gymnasium_server.py`) — probably out of scope, but note it exists as a "Lean ↔ external
  RL env" example if we ever wire proof search RL.
- Doc-tooling: `scripts/docs/polish_verso_guide.py` and `polish_docgen.py` are reusable
  post-processors for a Verso+DocGen static site (landing page, responsive figures, copy
  buttons) — adopt if we publish a blueprint site.

### Adoptables (net, additions to Codex list)
- **P1/P2: adopt the Verso-blueprint pattern** (Lean-authored manual + `blueprint-gen`
  exe + polish scripts) as Theoremata's blueprint tooling model.
- **P1: port the axiom-allowlist repo-lint** (`repo_lint.py` pattern) — makes our
  TRUST_BOUNDARIES doc enforceable, not just prose.
- **P2: mine BugZoo** as both a benchmark seam and a worked external-producer-bug/
  checker evidence example.
- **P3: VNNComp + certificate fixtures** as an optional computational-verification bench.

---

## zero-to-qed-main  —  gaps: curated classic-proof set, tactics reference corpus, ANCHOR literate-linking, actionable AI chapter

Codex framed it as an "educational book, medium value, curriculum for critic/decomposer
prompts." True but it **under-weighted the actual math-proof corpus** and several
directly-ingestible assets, while over-indexing on the toy *program* examples.

### Captured correctly
- It is an mdBook + Typst educational book (`docs/src/*.md` + `SUMMARY.md`, Arcs I/II +
  appendices) paired with runnable Lean under `src/`. Chapters on Proof Strategy (16),
  Model Checking (22), AI (23), SMT exist. `ProofStrategy.lean` has inline goal-state
  annotations (the "proof-state pedagogy" Codex cited) — confirmed.

### MISSED (with cites)
- **A curated, graded set of classic MATH proofs** — the strongest benchmark/curriculum
  asset — which Codex overlooked in favor of the FizzBuzz/vending-machine *program*
  examples. Cite: `src/ZeroToQED/Proofs/` = `Sqrt2Irrational.lean` (Irrationality of √2,
  clean sub-lemma decomposition `sq_even_of_even`/`even_of_sq_even`), `InfinitudePrimes.lean`,
  `Pigeonhole.lean`, `BinomialTheorem.lean`, `EuclidLemma.lean`, `Divisibility.lean`,
  `Fibonacci.lean`. These are compact, Mathlib-backed, named theorems — far better graded
  bench items than the program examples Codex leaned on.
- **A paired same-theorem/two-strategy datum**: `InfinitudePrimes.lean` vs
  `InfinitudePrimesGrind.lean` (manual proof vs `grind` automation). Directly useful for
  tactic-strategy / automation-vs-manual comparison in our benchmark. Missed.
- **`docs/src/appendix_c_tactics.md` is a ~5.2k-word Lean4+Mathlib tactics reference**
  (~60 tactics: `aesop`, `omega`, `grind`, `simp`, `gcongr`, `field_simp`, `fin_cases`,
  `decide`, `calc`, … each with description + examples). This is an ingestible **tactic
  knowledge base** for tactic-selection/retrieval in our prover — stronger than Codex's
  vague "extract tactic-decision guide into prompts." Appendices A (syntax) and B
  (declarations) are similar reference corpora.
- **mdBook ANCHOR literate-linking**: proofs are wrapped `-- ANCHOR: sqrt2_irrational` …
  `-- ANCHOR_END:` so prose pulls exact Lean snippets by name (see `Sqrt2Irrational.lean`,
  `smt/SMTExamples.lean`, `ProofStrategy.lean`). A lightweight prose↔proof linkage pattern
  (alternative to Verso) for tying blueprint/proof-DAG nodes to source. Not mentioned.
- **The AI chapter (23) is actionable, not just prose.** It inventories the exact
  benchmarks we care about (MiniF2F 488 IMO/AIME/AMC, PutnamBench 1724, ProofNet 371) with
  Pass@N semantics; describes the prover-verifier + MCTS + data-loop architecture; and —
  most concretely — documents the **`lean-lsp-mcp` MCP server wired via `.mcp.json` /
  `claude mcp add lean-lsp uvx lean-lsp-mcp`** for goal-state access, Loogle/LeanSearch.
  Given Theoremata now has an MCP surface (`mcps/`), this is a direct wiring reference, not
  background reading. Codex reduced it to "a chapter on prover-verifier architecture."
- **SMT bridge examples**: `smt/SMTExamples.lean` uses `import Smt` (lean-smt) with the
  `smt` tactic over linear int arithmetic, uninterpreted functions, and quantifiers —
  relevant to our hammer/multi-system story. Codex said "SMT examples" without detail.
- `examples/` also ships **Rust** implementations paired with Lean (`Cargo.toml`,
  `circuit-breaker/`, `game-of-life/`, `stack-machine/`) — cross-language verified-artifact
  examples; minor, but note the Lean+Rust pairing since Theoremata's core is Rust.

### Adoptables (net, additions to Codex list)
- **P1: ingest `src/ZeroToQED/Proofs/**` as a graded classic-theorem smoke/bench suite**
  (irrationality √2, infinitude of primes, pigeonhole, binomial theorem) — clean,
  Mathlib-backed, decomposable.
- **P1: import `appendix_c_tactics.md` as a tactic knowledge base** for tactic
  selection/retrieval; keep the manual-vs-`grind` pair as an automation comparison probe.
- **P2: reuse the AI-chapter's `lean-lsp-mcp` + `.mcp.json` recipe** to sanity-check our
  own MCP wiring; reuse its benchmark inventory (MiniF2F/PutnamBench/ProofNet) for
  bench-harness coverage.
- **P3: consider the ANCHOR prose↔proof linking convention** for blueprint/proof-DAG
  source linkage where full Verso is too heavy.
