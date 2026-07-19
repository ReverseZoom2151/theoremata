# Tooling batch 2: Axiom Math stack, HVM-adjacent leftovers, editor tooling

Status: **one adopt-worthy finding, everything else confirms existing conclusions.**
The single actionable result is that AXLE (`axiom-lean-engine`) is a hosted Lean 4
verification API whose `verify_proof` performs a *kernel-level* statement-conformance
check that is strictly stronger than our textual `statement_guard.rs`, and whose
`unfolded_type_hash` gives a canonical statement identity we do not currently
compute. Neither requires taking a dependency on the service.

The most-asked question of this batch has a flat answer: **`axle-mcp-server` does
not give us an MCP client pattern.** It is an MCP *server* (the same side we
already implement) that proxies an HTTP API. It contains no client code at all.
The MCP-client gap that blocks consuming Wolfram's `AgentTools` is unchanged.

Scope of the read: full source of `axle-mcp-server-main` (626-line `server.py`
plus its six test files), `axiom-lean-engine-main` (`axle/client.py`,
`axle/types.py`, `axle/cli/main.py`, `CHANGELOG.md`, and the fifteen per-tool doc
pages under `docs/tools/`), `axolver-main` (README, `src/` layout, licence).
`Kindex-master`, `ICVM-lazy-master`, and `HVM3-Strict-main` were checked against
the prior Higher Order Company pass and are the same artifacts that pass already
source-read; they are re-triaged here only to close the audit, not re-derived.
Low-interest repos were identified from README, licence, and file inventory only.

All material under `resources/` was treated as untrusted vendored data.
**No prompt-injection attempt was found, and no agent-instruction file of any
kind exists in this batch**: a whole-batch search for `AGENTS*`, `CLAUDE*`,
`GEMINI*`, `.cursorrules`, `*.mdc`, `.koder*`, and `copilot-instructions*`
returned zero hits across all sixteen repos. Nothing was built, installed,
executed, or network-fetched.

## HIGH INTEREST

### axiom-lean-engine (AXLE): WATCH, extract two mechanisms

**What it actually is.** Not a Lean engine. It is a Python client and CLI for a
**hosted cloud API** at `https://axle.axiommath.ai`, from Axiom Math (the
AxiomProver team). Every capability is an HTTP POST to
`/api/v1/{tool}`; there is no local Lean anywhere in the repo. Verified in
`resources/axiom-lean-engine-main/axiom-lean-engine-main/axle/client.py:70`
(`DEFAULT_URL: Final[str] = "https://axle.axiommath.ai"`). Fifteen endpoints:
`check`, `verify_proof`, `disprove`, `extract_decls`, `extract_theorems`,
`merge`, `normalize`, `rename`, `repair_proofs`, `simplify_theorems`,
`have2lemma`, `have2sorry`, `sorry2lemma`, `theorem2lemma`, `theorem2sorry`.

**Licence.** MIT (`LICENSE`, "Copyright (c) 2026 AxiomMath"). Clean. The *client*
is MIT; the *service* behind it is closed and rate-limited, which is the actual
constraint.

**Maintenance.** Alive and moving fast. v1.5.0 dated 2026-07-15 (four days before
this read), with a documented changelog going back to v1.0.0 on 2026-03-05, a
technical report at arXiv:2606.26442, and a contributed talk at the ICML 2026 AI
for Math workshop. Lean 4.26 through 4.31 environments are offered. This is the
healthiest repo in the batch by a wide margin.

**Does it expose a proving capability `components/prover/backends/lean.rs` lacks?**
No proving capability, no. AXLE does not search for proofs; there is no tactic
model, no premise selection, no proof search. Our `lean.rs` already does
compilation, `#print axioms` auditing (`lean.rs:722` `audit_axioms`), kernel
recheck (`lean.rs:756` `kernel_recheck`), and source scanning (`lean.rs:804`).
AXLE has no kernel-recheck equivalent at all; its `verify_proof` doc page states
plainly that it "trusts the Lean environment to behave correctly" and that "a
sufficiently creative adversary can exploit this to make invalid proofs appear
valid with Lean metaprogramming," then points at `lean4checker`, `Comparator`,
and `SafeVerify` as the answer. That is exactly our layer-3, and it is a point
where our design is ahead of theirs, not behind.

**What is worth extracting: two mechanisms, both in the statement-conformance
layer, where we are behind.**

*1. Definitional-equality statement conformance, including definition bodies.*
Our `components/prover/session/statement_guard.rs` snapshots
`{kind, name, signature}` headers and compares them after whitespace
normalisation (`snapshot_headers`, `normalize_ws`). AXLE's `verify_proof` instead
elaborates both the sorried `formal_statement` and the candidate `content` and
compares **types after kernel reduction** (`use_def_eq`, default true). Its
documented error vocabulary is the useful part, because each row is a distinct
attack our textual comparison does not cover:

| AXLE error | Covered by our `statement_guard.rs`? |
|---|---|
| `Missing required declaration '{name}'` | yes (`missing`) |
| `Theorem '{name}' does not match expected signature` | textually only; alpha-renaming, notation, and implicit-argument changes evade it |
| `Definition '{name}' does not match expected signature` | **no** |
| `Kind mismatch: candidate has {X} but expected {Y}` | partially (`kind` field) |
| `Unsafe function '{name}' detected` | covered elsewhere, in `lean.rs` source scan |
| `Candidate uses banned 'open private' command` | **no** |

The `Definition` row is the load-bearing one and the repo's own headline example
makes the attack explicit: the statement is `def A := 4; theorem main : A = 5 :=
sorry`, and the candidate is `def A := 5; theorem main : A = 5 := rfl`. The
theorem header is byte-identical before and after; the *definition it depends on*
was silently changed, and the proof becomes trivial. **Our header snapshot passes
this.** This is the same class as the hypothesis-smuggling `hypothesis_audit.rs`
was built for, arriving through a different door: a `def` body rather than a
binder. It is a concrete, testable gap and the highest-value item in this batch.

*2. `unfolded_type_hash` as a canonical statement identity.* `extract_decls`
returns per-declaration `type_hash` and `unfolded_type_hash`
(`axle/types.py:78-79`); per the v1.5.0 changelog the latter is the type hash
"after unfolding module-local elaboration auxiliaries (e.g. `foo.match_1`), so
types differing only by such an auto-generated name deduplicate where `type_hash`
would not." That is a stable key for statement-level dedup and for cache
invalidation. It composes directly with the subsumption-dedup item already on the
paper-mining adopt list. We would have to compute it ourselves against a local
Lean, which is real work, but the *shape* of the key (unfold elaborator-generated
auxiliaries before hashing) is the transferable idea and it is not obvious.

*Two smaller items.* `verify_negation` (v1.5.0) runs the same conformance check
against the negated statement and reports `negation.okay`, giving a cheap
"candidate actually disproves the goal" probe alongside "candidate proves the
goal": a useful signal for our falsification and vacuity paths that costs one
extra pass. And `Document` (`axle/types.py:63-100`) carries a
richer-than-ours per-declaration record: six separate dependency lists split
along type/value/syntactic and local/external axes, plus `tactic_counts`,
`proof_length`, `type_depth`, `term_depth`, `heartbeats`. That split is a better
schema than a flat dependency list for premise-selection features.

**Also worth recording as a negative example.** The CLI defaults to **exit 0
regardless of the verdict**; a non-zero exit requires an explicit `--strict`
flag, and even then only reflects the `okay` field (`axle/cli/main.py:191-198`
and `:417-421`, which returns `3` only under `strict`). Worse, `check`'s own doc
page states that `okay` "stays `true` even when a declaration uses `sorry` or a
disallowed axiom"; those are reported as *warnings*. So `axle check --strict`
exits 0 on a file full of `sorry`. This is a **fourth independent instance** of
the exit-code trap already documented for Metamath, HVM2, and two Kind
generations in `higher-order-co.md`. It further strengthens the standing
recommendation there: make "exit status is not proof of success" a mandatory
declared property of every backend.

**Verdict: WATCH the service, ADOPT the two mechanisms.** Do not take a
dependency: it is a rate-limited hosted API from a single vendor, a network call
in a codebase whose checkers are documented as pure, offline, and deterministic,
and it cannot enter the gate for the same reason Wolfram cannot: no proof object
of ours is re-derivable from its answer, and it self-declares that it trusts its
own environment. Reimplement the conformance check against our local Lean.

### axle-mcp-server: SKIP as a dependency; two portable details

**What it actually is.** An MCP **server** exposing the AXLE HTTP endpoints as
MCP tools. MIT, `axiom-axle-mcp` v0.3.5, Copyright (c) 2026 Axiom Math.
Single 626-line `axle_mcp_server/server.py`, six test files, a Dockerfile, and a
hosted instance at `https://mcp.axiommath.ai/mcp`.

**The direct answer to the batch question.** It contains **no MCP client code**.
A whole-repo search for `ClientSession`, `stdio_client`, `streamablehttp_client`,
and `sse_client` returns zero hits. Its imports are `mcp.server.stdio` and
`mcp.server.Server`, the server half of the Python MCP SDK. Every outbound call
is plain `urllib.request` against AXLE's REST API
(`server.py:96` `_call_endpoint`), wrapped in `asyncio.to_thread`. It answers
"how do I put an HTTP API behind MCP," which is the thing we already do, and says
nothing about "how do I consume someone else's MCP server," which is the thing we
need for `AgentTools`. **The MCP-client gap recorded as revisit trigger 5 in
`wolfram.md` is unchanged by this repo.** If we want a client, the reference is
the MCP SDK's own client half (or a Rust JSON-RPC client we write), not this.

**Two details that are still worth taking, both cheap.**

1. **`oneOf`/`anyOf`/`allOf` are rejected in MCP tool input schemas.** The
   comment at `server.py:172-182` (`_inject_file_uri`) records that "the
   Anthropic Messages API (and Vertex via OpenRouter) reject oneOf/anyOf/allOf in
   a tool's input_schema outright," so their "exactly one of `content` or
   `file_uri`" rule is enforced in the handler rather than in the schema. This is
   a real constraint on the descriptors
   `components/reason/orchestration/meta_tools.rs` emits, and the sort of thing
   found only by shipping. Worth a comment in `meta_tools.rs` next to the
   `tools_list_matches_mcp_shape` test.
2. **MCP roots as a filesystem sandbox.** `server.py:206` `_client_roots` queries
   the client's declared MCP roots capability and `server.py:225`
   `_resolve_file_uri` rejects any `file_uri` that resolves outside them, after
   `.resolve()` symlink normalisation, and rejects `file_uri` entirely in HTTP
   mode. Three-state semantics: no roots capability means unconstrained, an empty
   root list means deny-all, otherwise allow-list. If our MCP surface ever accepts
   a path from a client, this is the pattern to copy: it is a correct
   path-traversal defence, not a naive prefix check.

A third, lesser observation: the server builds its entire tool list *dynamically*
at startup by fetching AXLE's endpoint descriptors and mapping them to JSON
Schema through a small `TYPE_MAP` (`server.py:48`, `field_to_json_schema`,
`build_input_schema`, `_build_tool_defs`). Elegant, but it means the tool surface
depends on a live network fetch at boot, which is the opposite of what we want.

**Verdict: SKIP.** Not a client, not a dependency. Extract the two details.

### axolver: SKIP

**What it actually is.** Not a solver in any theorem-proving sense. It is a
seq2seq **transformer training framework** for mathematical tasks, from Axiom
Math, a complete rewrite of François Charton's `Int2Int` (arXiv:2502.17513). It
trains small encoder-decoder transformers (default 4+4 layers, 256 dim) on 26
synthetic tasks with on-the-fly data generation: GCD, fraction arithmetic,
modular arithmetic, matrix transpose/determinant/rank/eigenvalues/inverse,
shortest path, max clique, Laplacian eigenvalues, polynomial roots, symbolic
integration, and six toy sequence tasks. It replicates four known results
(integration 97.4%, GCD 99.1%, matrix transpose 100%, eigenvalues 99.3%).

**Licence.** Apache-2.0. Clean, and the most permissive thing in the batch.

**Does it expose a proving capability `lean.rs` lacks?** No. There is no prover,
no formal system, no proof object, no Lean anywhere. It maps token sequences to
token sequences and grades the output with a per-task Python `evaluate()`. Its
correctness notion is "the decoded answer equals the generated answer," which has
no bearing on a verification gate.

**The one thing worth noting** is the extension API, which is genuinely tidy:
adding a task is a `Generator` subclass with `generate(rng, is_train) ->
(problem, question, answer)` and `evaluate(...) -> {"is_valid": 1|0|-1}`, plus a
tokenizer pair and a registry entry, with no core-code change
(`src/envs/generators/base.py`, `src/envs/ops/`). The three-valued `is_valid`
(correct / incorrect / **decoding error**) is the right distinction and matches
our own refusal to collapse "screened" into "proved." If we ever build a
synthetic-task harness, that is the shape. That is a design note, not an adoption.

**Verdict: SKIP.** Adjacent research infrastructure with no path into our loop.
Our bottleneck is proof search and verification, not training small transformers
on GCD. Revisit only if we take up neural-numeric conjecture generation as a
workstream.

### Kindex, ICVM-lazy, HVM3-Strict: SKIP; the audit is wrong that these were unread

All three were **already source-read** in `higher-order-co.md`, which lists
"ICVM-lazy" and "HVM3-Strict" in its runtime scope and devotes a full quantitative
section to Kindex. They appear as top-level directories under `resources/` rather
than in a `higher-order-co/` subdirectory, which is the likely cause of the audit
miss. Nothing in re-checking them changes any conclusion.

- **Kindex-master**: the Kind1-era proof corpus, MIT. `higher-order-co.md` has
  the numbers: 1,088 `.kind2` files, ~56 that discharge a proposition, ~85 proved
  statements (5.1%), ceiling `mul.comm` on unary naturals, eight `U120` axioms
  formatted exactly like theorems, and a README stating it does not typecheck as
  shipped. Its own first line confirms the archival: "This is the ecosystem for
  the old Kind, and has been archived since the refactor." Not harvestable.
- **ICVM-lazy-master**: Taelin's Rust reference implementation of the
  Interaction Calculus, MIT, "Copyright 2018 VICTOR HERNANDES SILVA MAIA". About
  eight source files. The README is a good pedagogical exposition of the four IC
  reduction rules and the labelled `dup`/`sup` commutation rule, and it states
  the limitation `higher-order-co.md` builds its section 2 on, in the author's own
  words: "the Lambda Calculus can perform self-exponentiation of church-nats as
  `λx (x x)`, which isn't possible on IC." A 2018 artifact predating every runtime
  we evaluated. No change.
- **HVM3-Strict-main**: the strict-evaluation HVM3 variant, MIT, Copyright (c)
  2024 Victor Taelin. A one-line README ("This is the Strict version of HVM3"),
  eight source files, four example programs. `higher-order-co.md` already records
  it as alive five weeks. One small point in its favour worth stating since the
  brief asked specifically: **its exit codes are correct**:
  `src/Main.hs:75-78` `exitWithError` calls `exitWith (ExitFailure 1)`, and the C
  runtime uses `exit(1)` / `exit(EXIT_FAILURE)` on its error paths, so it is *not*
  a fourth instance of the exit-0-on-failure trap. That does not reopen anything.
  The licensing trap, the trusted-path exclusion, the bottleneck mismatch, and the
  churn argument are all independent of evaluation strategy, and strictness makes
  the ordinary-recursion problem worse, not better. **No change to the HOC
  conclusion.**

## Triage: low-interest repos

| Repo | What it is | Licence | Verdict |
|---|---|---|---|
| `hvm-bench-main` | Rust harness that checks out HVM2 revisions and times them; 17 files, derived from `hvm-compare-perf` | **none found** (no LICENSE, no `license` key in `Cargo.toml`), all rights reserved | SKIP. Benchmark plumbing for a runtime we did not adopt. |
| `hvm-compare-perf-main` | Same idea one generation earlier, for `hvm-core`; checks out commits listed in `commits.cfg` and times each | **none found** | SKIP. Its "important hashes" list is a nice fossil record of the ptr-refactor churn `higher-order-co.md` section 7 describes, and nothing more. |
| `hvm-core-serialization-main` | Binary encoding/decoding for an interaction net (root tree, redex list, wiring) with a variable-length number format; 10 files | **none found** | SKIP. Only meaningful if we ran interaction nets across a process boundary, which we do not. |
| `bend-language-server-main` | LSP server for Bend: semantic token highlighting plus diagnostics | MIT | SKIP. `higher-order-co.md` already notes this was archived out from under Bend on 2024-10-18. We do not author Bend. |
| `tree-sitter-bend-main` | Tree-sitter grammar for Bend, consumed by the above | MIT | SKIP. Same, same date. |
| `tree-sitter-wolfram-master` | Tree-sitter grammar for Wolfram Language (`.wl`, `.m`, `.wls`, `.nb`) | MIT | SKIP. Already in the `wolfram.md` skip list: we call Wolfram, we never write it. |
| `zed-wolfram-lsp-master` | Zed extension wiring `LSPServer` to a local WolframKernel; 6 files | MIT | SKIP. Editor integration, and it requires a local Mathematica or Engine install we cannot depend on. |
| `zed-wolfram-highlighter-master` | Zed syntax highlighting over `tree-sitter-wolfram`; 14 files | MIT | SKIP. Same. |
| `hs-highlight-main` | Small Haskell library for ANSI-colouring spans of source text (underline, bold, error styles); 6 files | permissive-looking header, "Copyright (c) 2024 Lorenzobattistela", **no SPDX identifier** | SKIP. Terminal cosmetics. The nearest thing to a use is prettier Lean error spans, which is not worth a Haskell dependency. |
| `peteroupc.github.io-master` | Peter Occil's personal math-notes site: 915 files of articles on randomness extraction, Bernoulli factories, exact random sampling, PRNG design, approximation theory, plus unrelated graphics and file-format material | public domain (Unlicense: "released into the public domain") | SKIP with one flag. The only adjacent item is `approxtheory.md` / `bernapprox.md`, which catalogue **explicit** polynomial error bounds "with no hidden constants," i.e. exactly the shape `cert_sturm.py`'s `poly_minimax` certificate kind needs. Public domain, so quotable without constraint. Notes, not code, and unreviewed; a reference to mine if the minimax-bound path ever becomes real work. |

## Ranked adopt list

1. **Definition-body statement conformance in `statement_guard.rs`.** Add
   definition *values*, not just theorem headers, to the snapshot, and compare
   after kernel reduction rather than whitespace normalisation. AXLE's own
   worked example (`def A := 4` becoming `def A := 5`) is a live hole in our
   current textual check. Highest value in the batch; testable today with a
   regression case that our guard currently passes and should not.
2. **Ban `open private` in the Lean source scan.** One line, and it is an escape
   hatch AXLE treats as file-level fatal and we do not check at all. Cheap
   enough that it should ride along with item 1.
3. **`unfolded_type_hash` as a canonical statement key.** Unfold module-local
   elaboration auxiliaries before hashing the type, so statements that differ
   only by an auto-generated `foo.match_1` name collapse. Feeds subsumption dedup
   and result caching. Requires local Lean work; the idea, not the code, is what
   transfers.
4. **Negation conformance as a falsification signal.** Run the conformance check
   against the negated statement and record whether the candidate proves it. One
   extra pass, and it distinguishes "wrong proof" from "proof of the opposite,"
   which our current vocabulary does not.
5. **`oneOf`/`anyOf`/`allOf` are inadmissible in MCP tool input schemas.** Record
   in `meta_tools.rs` alongside the existing shape test. Costs a comment,
   prevents a class of descriptor that silently fails at the model boundary.
6. **MCP roots as a path sandbox**, if and when our MCP surface accepts paths.
   Copy the three-state semantics from `_client_roots`, including
   symlink-resolved containment and the deny-in-HTTP-mode rule.
7. **A fourth data point for the exit-code rule.** `axle check --strict` exits 0
   on `sorry`-laden input. Cite it in the backend-requirement text that
   `higher-order-co.md` already argues for.

Items 1 and 2 are the only ones that close an actual soundness gap. Items 3 and 4
are enhancements. Items 5 to 7 are documentation.

## Risks

1. **AXLE is a hosted, rate-limited, single-vendor network service.** Any use
   crosses a process and network boundary, which is incompatible with our
   checkers' documented pure-offline-deterministic property, and it self-declares
   that it trusts its own Lean environment against metaprogramming attacks. It
   is an untrusted oracle at best, on the same footing as Wolfram, and it cannot
   enter the gate. The recommendation here is to reimplement two mechanisms
   locally, not to call the API.
2. **Three HVM-adjacent repos ship with no licence at all** (`hvm-bench`,
   `hvm-compare-perf`, `hvm-core-serialization`), hence all rights reserved. They
   are ideas-only material, and there is nothing in them we want. No GPL or AGPL
   contamination was found anywhere in this batch; the only licence hazard is the
   unlicensed group.
3. **`hs-highlight` has a copyright line but no SPDX identifier**, so its terms
   are ambiguous. Irrelevant given the SKIP verdict, but do not vendor it.
4. **Adopting item 1 requires a Lean toolchain to test.** A definitional-equality
   comparison cannot be validated by our mock backend, so the regression case
   must be gated the same way our other live-Lean tests are, and must not be
   allowed to pass vacuously when the toolchain is absent.

## Revisit triggers

1. **An MCP client lands in the harness.** It will not come from this batch. When
   it does, `axle-mcp-server` becomes trivially consumable alongside
   `AgentTools`, which changes the ordering of the `wolfram.md` adopt list but
   not this one.
2. **AXLE publishes a proof-object or kernel-recheck path.** Today its own docs
   defer to `lean4checker` / `Comparator` / `SafeVerify`. If it ever returns a
   re-checkable artifact rather than a verdict, the trusted-path question would
   be worth reopening; until then it is structurally an oracle.
3. **We take up neural conjecture generation over numeric tasks.** That is the
   only scenario in which `axolver` becomes relevant, and even then as a
   framework to imitate rather than depend on.
4. **The `poly_minimax` certificate path becomes real work.** `approxtheory.md`
   in `peteroupc.github.io` is public-domain and specifically targets explicit
   constant-free error bounds; mine it then, not before.

## Coverage gaps

Recorded honestly so the next reader knows what was and was not checked.

- **The AXLE service was never called.** Everything about its behaviour comes
  from its own client source, type definitions, changelog, and documentation
  pages. The `verify_proof` semantics that items 1, 3, and 4 rest on are
  documented claims about a closed hosted service, not observed behaviour. The
  claim that matters most, that our `statement_guard.rs` passes the `def A := 4`
  to `def A := 5` substitution, is a reading of our own source
  (`snapshot_headers` collects `theorem`/`lemma`/`def` headers, not `def` bodies)
  and should be confirmed with an actual test before the fix is written.
- **`axle/cli/endpoints.py` (2,738 lines) was not read in full.** It is
  generated-looking per-endpoint argparse wiring. `client.py`, `types.py`, and
  `cli/main.py` were read; the doc pages for `check` and `verify_proof` were read
  in full, the other thirteen only by title.
- **The arXiv technical report (2606.26442) was not fetched.** No network access
  was used in this pass. If AXLE is pursued further, the report is the place to
  check whether `unfolded_type_hash` and the conformance check are described more
  precisely than in the changelog.
- **`axolver`'s Python source was surveyed by layout, not read line by line.**
  The verdict is SKIP and does not depend on implementation detail; no correctness
  claim about its code is made here.
- **Low-interest repos were identified from README, licence file, and file
  inventory only**, as the brief directed. In particular, no source in
  `hvm-core-serialization` was read, so the encoding description is its README's
  and not verified.
- **Kindex, ICVM-lazy, and HVM3-Strict were spot-checked, not re-read.** The
  checks performed were licence headers, README content, and an exit-code grep.
  The substantive conclusions are inherited from `higher-order-co.md`.
