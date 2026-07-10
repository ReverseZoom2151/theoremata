# Resource mining: metamath-knife (metamath-rs)

Source: `resources/metamath-knife-main/metamath-knife-main/`
Upstream: https://github.com/metamath/metamath-knife (a friendly fork of sorear's smetamath-rs / SMM3)
Version: `0.3.9` (both crates). Rust 2021, MSRV 1.56.

## 0. Injection + license (do this first)

- **INJECTION SCAN: clean.** Read the workspace/crate Cargo.tomls, both READMEs, `lib.rs`,
  `database.rs`, `verify.rs`, `main.rs`, plus targeted greps of `parser.rs`/`formula.rs`.
  No embedded instructions, no "ignore previous", no prompt-directed text, no data
  exfiltration, no suspicious `build.rs` (there is none). Doc-comments are ordinary
  technical prose. Treat set.mm databases fed *through* it as untrusted data as always,
  but the crate source itself shows no injection vector. This is a well-known, real OSS
  project (authors David A. Wheeler + Stefan O'Rear).
- **LICENSE: `MIT OR Apache-2.0`** (dual, permissive). Confirmed in every location:
  workspace `Cargo.toml` (`license = "MIT OR Apache-2.0"`), `metamath-rs/Cargo.toml`
  (`license.workspace = true`), both READMEs ("SPDX ... (MIT OR Apache-2.0)"), and the
  two license files `LICENSE-MIT` (Copyright (c) 2016 Stefan O'Rear) + `LICENSE-APACHE`
  (v2.0). **We can vendor OR depend on it as a normal Cargo dependency**, no copyleft
  obligation beyond attribution. Same license as our own workspace uses elsewhere.

## 1. What it is + crate structure

A **workspace with two crates**, so it is **both a library and a CLI**:

- **`metamath-rs`** (`metamath-rs/`, the library, v0.3.9) — the whole engine. Public API
  is a set of analysis "passes" hung off a `Database` object (`database.rs`). Public
  modules: `database`, `verify`, `scopeck`, `nameck`, `parser`, `formula`, `grammar`,
  `proof`, `statement`, `segment_set`, `outline`, `axiom_use`, `diag`, `typesetting`,
  `comment_parser`, `export`, `discouraged`, `verify_markup` (feature-gated). Re-exports
  `Database`, `Formula`, `FormulaRef`, `Label`, `Symbol`, `StatementRef`, `StatementType`,
  `is_valid_label`, etc. from the crate root.
- **`metamath-knife`** (`metamath-knife/src/main.rs`, the CLI) — a thin clap wrapper that
  builds a `Database`, calls passes based on flags, and renders diagnostics. It is ~320
  lines and contains **no verification logic of its own**; everything lives in the library.
  This is the key fact for us: **all functionality is reachable in-process without the binary.**

Dependencies are light and sane: `itertools`, `fnv`, `regex` (no_default), `tinyvec`,
`log`, `annotate-snippets`, `typed-arena`, `filetime`. Optional features `dot`/`xml`/
`verify_markup` pull `dot-writer`/`xml-rs`/`html5ever`+`scraper`. **For pure verification we
need none of the optional features** — we could depend with `default-features = false` and
drop `scraper`/`html5ever` entirely (they are only for markup/typesetting checking).

Performance claim (README): >28,000 set.mm proofs verified in <1s; parallel (`--jobs`) and
incremental (reparse only changed segments).

## 2. Verification approach (the trusted kernel)

Pipeline is a chain of lazily-computed, cached, per-segment passes (`database.rs`):

1. **parse** (`parser.rs` + `segment_set.rs`) — tokenizes `.mm` into **segments**. A source
   file with N `$[ include $]` statements becomes N+1 segments; includes are *detected* by
   the parser (`get_file_include`, `FileInclude` token) but *followed* by `segment_set`
   (`db.parse(start, text)` resolves an include from in-memory `text` pairs first, else from
   disk relative to CWD). Files >1MiB auto-split at chapter headers for parallelism.
2. **nameck** (`name_pass` → `Nameset`) — builds the symbol/label → atom lookup tables.
3. **scopeck** (`scope_pass` → `ScopeResult`) — builds the **`Frame`** for each assertion:
   mandatory `$f`/`$e` hypotheses, mandatory + optional **`$d` disjoint-variable** sets,
   the target expression, and variable numbering. This *is* the logical system.
4. **verify** (`verify_pass` → `VerifyResult`) — **the trusted kernel** (`verify.rs`).

The kernel (`verify.rs`) is a small, allocation-free (after warmup) **stack machine**
(`VerifyState`): for each `$p`, it walks the proof, pushing hypotheses and applying prior
assertions. Each `Assert` step pops the frame's hypotheses off the stack, checks each `$f`
typecode and each `$e` matches under a computed substitution (`do_substitute_eq`), pushes the
substituted target, then checks all mandatory `$d` constraints against the accumulated
variable bitsets (`ProofDvViolation`). `finalize_step` confirms the single remaining stack
entry equals the claimed statement (typecode + exact expression) — i.e. "did we prove the
*right* thing." Circular-reasoning and scope guards (`StepUsedBeforeDefinition`,
`StepUsedAfterScope`) use the `SegmentOrder` oracle. Errors are `Diagnostic` values.

It **supports all four proof formats** (README differentiator): normal, compressed
(`( labels ) A-T/U-Y/Z/?` varint roster), packed, and explicit (`fwdref:hyptok=label`).
Incomplete `?` steps yield `ProofIncomplete`. A `ProofBuilder` trait lets a caller collect a
proof tree as a side effect; the `()` impl is the fast "one-shot, no output" verifier.

Trust surface is exactly scopeck + verify + nameck + parser — small and self-contained.

## 3. Mapping to Theoremata

Our Metamath backend today is `components/prover/backends/external.rs` (`ExternalBackend`,
shared with Agda). It **shells out**: `command()` builds
`[binary, "read <file>", "verify proof *", "exit"]` and runs it via `exec::run`; trust holes
the CLI cannot see are caught by a **lexical `metamath_source_findings` scan** (`$a`, bare
`?`, escaping `$[ ]` includes). Statement-preservation is a **token-compare**
(`statement_preservation.rs::check_metamath_signature` / `parse_metamath_assertion`).

**Could we call metamath-knife in-process instead of shelling to `metamath`? Yes — cleanly.**

- **In-process verify.** Replace the `read/verify proof */exit` subprocess with:
  ```
  let mut db = Database::new(DbOptions { jobs: N, incremental: false, ..default });
  db.parse(entry.to_string(), vec![("Generated.mm".into(), code.into_bytes()), /* set.mm bytes */]);
  db.verify_pass();                       // or db.verify_one(&mut (), stmt) for just our $p
  let diags = db.diag_notations();        // parse+scope+verify errors, structured
  // render_diags(..) gives annotate-snippets output for the UI/artifacts
  ```
  `db.parse` takes **in-memory buffers** (`Vec<(String, Vec<u8>)>`) and resolves `$[ ]`
  includes from those pairs *before* touching disk — so we can feed the reviewed `set.mm`
  as bytes and a proof that `$[ set.mm $]` includes it, with **no CWD dependency and no
  disk write of the corpus at all**. That directly removes the fragile "copy includes into
  the workspace so the proof can't depend on a host-global DB" dance in `scaffold()`.

- **`verify_one` is the ideal fit for our gate.** `Database::verify_one::<P: ProofBuilder>`
  verifies a **single `$p` statement** against an already-scoped database and returns
  `Result<P::Item, Diagnostic>`. That is exactly our layer-2b/kernel-recheck granularity: load
  reviewed set.mm once (warm), then verify each generated theorem with a structured pass/fail
  + typed error, instead of parsing CLI stdout strings.

- **FormalBackend / ProofSession fit.** Introduce a `MetamathKnifeBackend` (new
  `components/prover/backends/metamath_knife.rs`) implementing `FormalBackend`:
  - `compile` + `kernel_recheck` → `verify_pass`/`verify_one` (the same in-process check
    can serve *both* layers, or we keep the current `metamath` binary as the independent
    second checker in `kernel_recheck`'s `secondary_binary` slot for defense-in-depth — a
    two-implementation cross-check is *stronger* than today's single external checker).
  - `audit_axioms` → **now implementable for real.** Metamath's whitelist is currently
    `Vec::new()` (empty, a no-op). metamath-knife has an **`axiom_use`** pass
    (`db.verify_usage_pass()` / `write_stmt_use` / the `-X` axiom-use file) that computes the
    exact set of `$a` axioms each theorem transitively depends on. That turns our stubbed
    axiom audit into a genuine layer-2a: assert the generated theorem's axiom closure ⊆ the
    reviewed set.mm axiom base (e.g. `ax-*`), catching a proof that smuggles in a new `$a`.
  - `source_scan` → keep the existing lexical `metamath_source_findings` as a cheap
    pre-filter (it is conservative and independent — good), but the kernel now *also*
    structurally rejects new `$a`/incomplete `?`, so the scan becomes belt-and-suspenders.
  - `ProofSession::submit_unit` → in-process `parse`+`verify_pass`; still whole-file
    (Metamath has no tactic stepping, so `step_tactic` stays `Unsupported`, unchanged).

- **Statement-signature checking (our per-system statement-preservation).** This is the
  strongest single win. Today `check_metamath_signature` compares **raw token vectors** of
  the `$p` — brittle to whitespace/label renaming and blind to whether two symbol strings
  are the *same statement up to substitution*. metamath-knife exposes:
  - `db.statement(label)` / `statement_by_label` → `StatementRef`, `label_typecode(label)`;
  - the **grammar / stmt_parse passes** (`grammar_pass` + `stmt_parse_pass`, require
    `incremental: true`) which turn each statement into a **`Formula`** (`formula.rs`);
  - `Formula::unify(other, &mut Substitutions)`, `substitute`, `as_sexpr`, `get_by_path`,
    `is_singleton`.
  So the "does the submitted `$p` prove the *canonical* statement" check can become a real
  **formula-level comparison** (parse both, check equality/unifiability of the parsed trees),
  replacing token-string compare with something that understands Metamath syntax — a robust
  upgrade to `PreservationVerdict` for the Metamath arm.

**More robust / faster / safer?** All three:
- *Safer*: no subprocess, no CWD-relative include resolution, no shell/argv boundary; the
  corpus never needs to be written to a shared workspace; typed `Diagnostic`s instead of
  stdout scraping; a real axiom audit; a real signature check.
- *Faster*: one warm in-process `Database` (set.mm parsed/scoped once, cached), parallel
  `--jobs`, incremental reparse; per-proof `verify_one` avoids re-reading the corpus per job
  that a fresh `metamath read set.mm` subprocess pays every time.
- *More robust*: pure Rust, no external binary to locate/version/probe; deterministic; the
  kernel is small and auditable.

## 4. Buildable-now vs gated

**Buildable now (Rust-native Metamath verify path):**
- Add `metamath-rs = "0.3"` (or vendor) as a `components/prover` dependency,
  `default-features = false` (drop markup deps).
- New `MetamathKnifeBackend` implementing `FormalBackend`, wired into `backend_for()` in
  `formal.rs` alongside/replacing the `ExternalBackend` Metamath arm — same trait, so the
  3+1-layer `verify()` orchestration is unchanged.
- In-process `verify_pass`/`verify_one` for compile + kernel_recheck.
- Real `audit_axioms` via `verify_usage_pass`.
- Formula-level statement-preservation via grammar/stmt_parse + `Formula::unify`.
- Keep the current `metamath` CLI as an *optional* independent second checker
  (`secondary_binary`) — defense in depth, not required.

**Gated / later:**
- The generated proof must `$[ set.mm $]`-style include a corpus we supply as bytes; we need
  to pin a reviewed set.mm blob (already implied by `default_imports() == ["set.mm"]`) and
  feed it through `db.parse` text pairs. (Corpus provisioning, not a code blocker.)
- Grammar/formula parsing requires `DbOptions.incremental = true` and a grammar-complete
  database (set.mm is); for signature checks on partial snippets we parse against the loaded
  corpus, so this is fine but needs the incremental config path.
- API is `&mut self` pass-based with `expect`-on-missing-prereq panics; wrap in a small
  helper that always runs `name→scope→verify` in order (mirrors how `main.rs` sequences them)
  to avoid the panic guards.

## Prioritized adopt list

1. **In-process kernel via `Database::verify_pass` / `verify_one`** — replace the
   `read/verify proof */exit` subprocess in `ExternalBackend`. Removes shell boundary + CWD
   include hazard, gives typed `Diagnostic`s. *(highest value, buildable now)*
2. **Real axiom audit via the `axiom_use` pass** — fills our currently-empty Metamath
   `axiom_whitelist` layer 2a with a genuine transitive-`$a`-closure ⊆ base check.
3. **Formula-level statement-preservation** (`grammar`+`stmt_parse`+`Formula::unify`) —
   upgrade `check_metamath_signature` from token-compare to syntactic formula comparison.
4. **In-memory corpus feeding** (`db.parse(start, text_pairs)`) — supply reviewed set.mm as
   bytes; delete the "copy includes into workspace" logic in `scaffold()`.
5. **Two-implementation cross-check** — keep the upstream `metamath` binary as the optional
   `secondary_binary` kernel recheck; two independent verifiers agreeing > one.
6. **`annotate-snippets` diagnostic rendering** (`db.render_diags`) — nicer proof-error
   surfacing in artifacts/UI; metamath-knife already uses the same crate.

---

### ~10-line summary

metamath-knife is a permissively-licensed (**MIT OR Apache-2.0**, confirmed in all files;
vendor/depend freely) pure-Rust Metamath processor split into a library (`metamath-rs`) and a
thin CLI — **all verification lives in the library**, reachable in-process with no binary. Its
trusted kernel (`verify.rs`) is a small allocation-free stack machine over scopeck-built
Frames, handles **all four proof formats**, checks typecodes/substitutions/`$d`/final-goal, and
resolves `$[ ]` includes from **in-memory buffers** (no CWD dependency). Our Metamath backend
currently *shells out* to `metamath` and does a lexical source scan; **we should swap that for
an in-process `metamath-rs` dependency.** It is more robust (typed diagnostics, no subprocess,
no corpus-on-disk dance), faster (warm cached `Database`, parallel + incremental, per-proof
`verify_one`), and safer (small auditable kernel). It also unlocks two things our gate stubs
today: a **real axiom audit** (via the `axiom_use` pass, replacing our empty whitelist) and a
**formula-level statement-preservation check** (via `Formula::unify`, replacing token-compare).
No injection found. **Recommendation: ADOPT as the primary in-process Metamath kernel**, and
optionally retain the `metamath` CLI as an independent second checker for defense-in-depth.
Buildable now; only gating item is provisioning a pinned reviewed set.mm blob to feed as bytes.
