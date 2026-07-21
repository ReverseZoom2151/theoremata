# TauCeti (core library) mining report

Target: `resources/TauCeti-main/TauCeti-main/` (664 files: 590 .lean, 14 .py, 13 .yml, 8 .md).
Read as data only. Nothing under `resources/` was executed: no `lake build`, no script, no
workflow. The two Lean compile attempts reported in section 4 ran on a **copy** of one file in the
scratchpad, with our own toolchain, after first confirming the corpus contains no elaboration-time
escape hatch (section 2).

Companion report already in this directory: `tauceti-review.md`, which mines
`TauCetiProject/TauCetiReview` (the rubrics and the review engine). This report is the other half:
the actual Lean library those rubrics judge. There is no overlap in the adopt lists.

---

## 1. What it is

Tau Ceti is a **live, AI-authored Lean 4 mathematics library downstream of Mathlib**, incubated by
the Lean FRO and the Mathlib Initiative. It is not a benchmark, not a proof-search dump, and not a
single-target formalization project. `README.md:5` states the governing constraint: everything
under `TauCeti/` is written by AIs; humans own only the roadmaps (a separate repo,
`TauCetiRoadmap`), the review rubrics (`TauCetiReview`), and the CI/governance files.

Measured shape of the corpus:

| Quantity | Value |
|---|---|
| `.lean` files under `TauCeti/` | 579 |
| Total lines under `TauCeti/` | 108,253 |
| `theorem` | 4,171 |
| `lemma` | 2,153 |
| `def` | 704 |
| `instance` | 162 |
| `abbrev` | 119 |
| `structure` / `class` / `inductive` | 54 / 21 / 0 |
| `example` | 15 |
| files importing Mathlib directly | 353 of 579 |
| files with no import line at all | 0 |

Organization is Mathlib-shaped: `TauCeti/Algebra`, `Analysis`, `AlgebraicGeometry`,
`AlgebraicTopology`, `NumberTheory`, `Probability`, `MeasureTheory`, `Geometry`, `KnotTheory`,
`LowDimTopology`, `FieldTheory`, `LinearAlgebra`, `Topology`, `Data`. The dense clusters are
`Analysis/Contour` (17 files), `Probability/Exchangeability` (12),
`Algebra/Coalgebra/Comodule` (12), `KnotTheory/Grid` (11), `Geometry/Symplectic` (10).

Every file opts into the **Lean module system** (`module` keyword, `public import`, `public
section`), which is unusual and is machine-enforced (section 5, item A2). The root
`TauCeti.lean` deliberately re-exports nothing; `lakefile.toml` builds via the glob
`TauCeti.*`, so an orphaned module still fails the build.

`web/` is a separate small Lake project (9 files) that renders the project's static site;
it pins `leanprover/lean4:v4.31.0-rc1` and is not part of the audited library.

The declaration style is high quality and Mathlib-idiomatic: module docstrings with a `## Main
declarations` index, docstrings on essentially every declaration, and, notably, an explicit house
rule against hidden constants. `TauCeti/Analysis/PDE/EnergyForm/Basic.lean:29` reads: "with their
constants left explicit (never hidden in a `exists C`)". That is the same defect class our
`vacuity.rs` and `hypothesis_audit.rs` exist to catch, stated as an authoring convention.

---

## 2. Sorry and axiom census

**Exact result: zero. Not "few". Zero.**

Across all 590 `.lean` files:

| Pattern | Hits in `TauCeti/` (the library) | Hits anywhere in repo |
|---|---|---|
| `sorry` | 0 | 2, both inside prose comments (`scripts/Axioms.lean:14`, `web/Site/About.lean:15`) |
| `admit` | 0 | 2, same two comment lines |
| `native_decide` | 0 | 1, the same comment at `scripts/Axioms.lean:14` |
| `axiom` declarations | 0 | 1, the word inside a docstring at `scripts/Axioms.lean:110` |
| `sorryAx` | 0 | 0 |
| `@[implemented_by]` | 0 | 0 |
| `@[extern]` | 0 | 0 |
| `opaque` declarations | 0 | 1, the word inside a docstring at `TauCeti/KnotTheory/Grid/Grading/Change.lean:128` |
| `unsafe` | 0 | 2, both in the human-owned governance executables (`scripts/Axioms.lean:37`, `scripts/ModuleSystem.lean:63`) |
| `nolint` | 0 | 0 |
| `set_option` | 0 | 0 |
| `#eval` / `run_cmd` / `initialize` / `elab` / `macro_rules` / `IO.` | 0 | present only in `scripts/` |

`decide` (the kernel-checked tactic, not `native_decide`) appears in a handful of proofs, e.g.
`TauCeti/Analysis/Contour/Residue/Basic.lean:164,165,171`. That is sound: `decide` produces a
kernel-checkable `Decidable.decide` reduction and adds no axiom. Only `native_decide` would add
`Lean.ofReduceBool`, and there is none.

The zero is not merely asserted, it is **enforced by a kernel-level audit that ships in the repo**
(`scripts/Axioms.lean`) and is wired as a Lake executable in `lakefile.toml`. See adopt item A1.
`formalization.yaml:57` declares `sorry_count: 0`, `sorry_in_definitions: 0`, allowed axioms
exactly `propext`, `Classical.choice`, `Quot.sound`, which is byte-identical to our own
`DEFAULT_ALLOWED` in `components/verify/python/theoremata_tools/axioms.py:26`.

The corpus is also **free of elaboration-time code execution**: no `#eval`, no `run_cmd`, no
`initialize`, no `elab`, no `macro_rules`, no `IO.` anywhere under `TauCeti/`. It is pure
mathematics. This is the fact that made it safe to attempt a compile on a copied file.

---

## 3. Triviality census

**Verdict: the corpus is CLEAN of `name_claims_more_than_statement`.** This is a reportable
negative result and it is the first third-party Lean corpus we have measured that comes back
clean.

Method: parse each `theorem` / `lemma` / `example` block (6,330 blocks recovered), split statement
from proof term, and test four shapes.

**Shape 1, conclusion is `True`.** One hit in 6,330.

- `TauCeti/Algebra/AlgebraicGroup/Trivial.lean:83`
  `theorem convPoint_eq_one_iff (f : WithConv (R alghom A)) : f = 1  iff  True`

  This is **not** the defect. It is a `@[simp]` normal-form lemma in a file about the *trivial*
  algebraic group, and the substance lives one declaration above it at
  `Trivial.lean:76`, `theorem convPoint_eq_one (f) : f = 1`, proved by
  `WithConv.ofConv_injective` plus `Subsingleton.elim`. The ` iff  True` form is the standard
  Mathlib idiom for teaching `simp` to discharge the hypothesis. The name says exactly what the
  statement says.

**Shape 2, unconstrained existential of the form "exists x, x = bigExpr".** Zero hits in 6,330.
This is the exact shape we named `name_claims_more_than_statement`, the one provable by an
anonymous constructor while carrying a PDE-property name. It does not occur.

**Shape 3, hypothesis-free existential conclusions.** 257 statements contain an existential;
65 of those have no hypothesis binder. All 65 were inspected. Every one is substantive. A
representative sample:

- `TauCeti/AlgebraicTopology/SemilocallySimplyConnected/Basic.lean:98`
  `exists_isOpen_mem_nhds_loops_nullhomotopic (x : X) : exists U, IsOpen U /\ x in U /\ forall
  (g : Path x x), (forall t, g t in U) implies g.Homotopic (Path.refl x)`. The existential is
  constrained by three real conjuncts, and it is the definitional unfolding of semilocal simple
  connectedness. Name and statement agree.
- `TauCeti/Analysis/InnerProductSpace/Laplacian/Comparison.lean:135`
  `exists_mem_frontier_isMaxOn_of_harmonicOnNhd : exists x in frontier K, IsMaxOn f K x`. This is
  the maximum principle. Substantive.
- `TauCeti/Algebra/Coalgebra/Subcomodule/Lattice.lean:124` `mem_sup : m in N sup P  iff  exists n in
  N, exists p in P, n + p = m`. A lattice characterization, both directions with content.

**Shape 4, trivially-discharged proofs (`rfl`, `Iff.rfl`, `trivial`, `by simp`, anonymous
constructor).** 525 of 6,330 blocks, which is 8.3%. Filtering out names carrying a standard
defeq-lemma marker (`_apply`, `_def`, `_coe`, `_symm`, `_iff`, `_comp`, `_id`, `_val`, `_mk`,
`_zero`, `_one`, `_add`, `_mul`, `_neg`, `_inv`, `_sub`, `_smul`) leaves 182. Those 182 were
inspected by name and every one is still a bona fide API/simp lemma:
`toSubmodule_carrier`, `mem_toSubmodule`, `forget2_commHopfAlgCat_obj`, `top_toSubmodule`,
`sSup_toSubmodule`, `zero_toLinearMap`, and so on. A `rfl` proof for a `toSubmodule_*` bridge
lemma is the correct proof; the name claims a definitional identity and delivers exactly that.
There is no case where a mathematician's name, a named theorem, or a property word sits on top
of a `rfl`.

**Why this corpus is clean and the previous two were not.** The mechanism is visible in the repo.
`README.md:33` states that the review rubrics are *adversarial* and explicitly instruct reviewers
to find "mis-formalizations, vacuous statements, and 'pushing around the lump in the carpet'".
That is our defect class, named upstream, with a judge assigned to it. The rubric text itself
lives in the sibling repo and is mined in `tauceti-review.md`.

---

## 4. Does it build here? Real output.

**Toolchain: compatible, and the exact pin is already installed.**

- `lean-toolchain` pins `leanprover/lean4:v4.32.0-rc1`.
- Our PATH `lean` is `4.32.0` (release, commit `8c9756b28d64dab099da31a4c09229a9e6a2ef35`), lake
  `5.0.0-src+8c9756b`.
- `~/.elan/toolchains/` already contains `leanprover--lean4---v4.32.0-rc1` **and**
  `leanprover--lean4---v4.32.0`, plus 4.21.0, 4.25.0, 4.31.0. So the pinned toolchain is present.

**Dependency: Mathlib at `f4e566ca02d995d16c590cdfe4dc051cc80f4624`**, taken off `master`
(`lake-manifest.json`). Transitive pins: plausible, LeanSearchClient, importGraph, ProofWidgets4,
aesop, Qq, batteries, lean4-cli, all at fixed SHAs. `lakefile.toml` deliberately leaves
`rev = "master"` in the require and pins the real revision only in the manifest, so a daily bump
job can move it forward.

**Attempt 1, elaborate a real TauCeti file on the pinned toolchain.**
Copied `TauCeti/Algebra/AlgebraicGroup/Trivial.lean` to the scratchpad and ran
`elan run leanprover/lean4:v4.32.0-rc1 lean Trivial.lean`:

```
Trivial.lean:5:0: error: unknown module prefix 'TauCeti'

No directory 'TauCeti' or file 'TauCeti.olean' in the search path entries:
c:\Users\adria\.elan\toolchains\leanprover--lean4---v4.32.0-rc1\lib\lean
```

**Attempt 2 and 3, module-system syntax smoke test**, our own three-line file
(`module` / `public section` / `theorem foo : 1 + 1 = 2 := rfl`), on both
`v4.32.0-rc1` and `v4.32.0` release: **both compile with no diagnostics, exit 0**. So the module
system syntax that every TauCeti file uses is supported by our stock 4.32.0 as well as by the pin.

**Honest conclusion.** The blocker is not the toolchain and not the syntax. It is that Mathlib at
`f4e566ca` is not present in this environment and building it requires running `lake` inside the
untrusted tree, which is out of scope for this task. We do have `~/.cache/mathlib` populated with
8,620 `.ltar` files (438 MB) from an earlier session, but without a Mathlib checkout at the right
revision those artifacts cannot be resolved. A future build wave that wants live TauCeti fixtures
must, in our own tree and not theirs: clone mathlib4 at `f4e566ca`, `lake exe cache get`, then
point `THEOREMATA_MATHLIB_ROOT` at it and drive TauCeti files through our existing
`components/verify/python/theoremata_tools/lean_workspace.py` path dependency scaffolder. That is
a real cost (a Mathlib olean set is tens of GB) and should be a deliberate decision, not a
side effect.

---

## 5. Licence and attribution obligations

- **Licence: Apache-2.0.** `LICENSE` is the **unmodified stock Apache 2.0 text with the appendix
  placeholders left in**: the file ends with the literal boilerplate `Copyright {yyyy} {name of
  copyright owner}`. There is no filled-in holder line in `LICENSE` and **there is no NOTICE
  file** in the repo.
- **Actual copyright holders are in the per-file headers.** Measured across the corpus:
  - 520 files: `Copyright (c) 2026 The Tau Ceti contributors. All rights reserved.`
  - 4 files: `Copyright (c) 2026 Lean FRO, LLC. All rights reserved.`
  - 1 file: `Copyright (c) 2026 daouid. All rights reserved.`
  - Named `Authors:` lines include Chris Birkbeck (60), Claude (16), Codex (7), Kim Morrison (4).
  - `formalization.yaml:17` declares `license: "Apache-2.0"`.
  - Note: 54 of the 579 files carry no header at all, because `lakefile.toml` sets
    `weak.linter.style.header = false`.
- **What we would owe if we vendor or port.** Apache-2.0 sections 4(a)-(d): ship a copy of the
  licence; state prominently in changed files that we changed them; retain all copyright, patent,
  trademark, and attribution notices from the source, meaning the per-file headers above must be
  preserved verbatim on any file we copy; and, since upstream ships **no** NOTICE file, we inherit
  no NOTICE obligation and must not invent one.
- **Section 3 patent grant** is in our favour and is worth noting: contributors grant a patent
  licence, terminated on patent litigation. Nothing unusual.
- **Specific to the adopt list below.** `scripts/Axioms.lean` internally attributes two borrowed
  pieces: `withImportedEnv` is "inlined from importGraph's `Core.withImportModules` (Kim Morrison,
  Paul Lezeau; Apache 2.0)" (`scripts/Axioms.lean:24`), and `reachesDisallowedAxiom` is "adapted
  from Robin Arnez's mathlib-wide version (leanprover Zulip, #general)" (`scripts/Axioms.lean:82`).
  If we port that file we must carry **both** of those attributions forward as well as the Tau Ceti
  header, and mark our modifications.

---

## 6. Adopt list

Judged against the existing inventory of our own tree. Four adopts, ranked. Everything else is in
section 7 as an explicit rejection.

### A1. Port `scripts/Axioms.lean` as a whole-library, one-pass, memoized `collectAxioms` audit. ADOPT, highest value.

**Source:** `resources/TauCeti-main/TauCeti-main/scripts/Axioms.lean`, whole file (147 lines).
Key parts: `allowedAxioms` at `:23`, `withImportedEnv` at `:31`, `collectLeanModules` at `:48`,
`reachesDisallowedAxiom` at `:88`, `audit` at `:113`, `main` at `:139`.

**What it does.** A Lake executable that builds the library environment from compiled `.olean`s
and, for every declaration *defined in* the audited namespace, decides whether it transitively
reaches an axiom outside `[propext, Classical.choice, Quot.sound]`, using
`Lean.Environment` traversal rather than `#print axioms` stdout.

**Why it beats what we have.** Our kernel-level axiom auditing is *entirely* `#print axioms`
stdout parsing: `components/verify/python/theoremata_tools/axioms.py:66` appends
`#print axioms THM` to the source and runs `lake env lean`, and
`components/prover/backends/lean.rs:1266` writes a `Generated_axioms.lean` doing the same. That
is one compile per theorem. TauCeti measured the cost of the naive version and documented it at
`scripts/Axioms.lean:82`: "the stock `collectAxioms` rebuilds its state and redoes per-call setup
on each invocation, which is the ~77ms/decl that dominated the audit". Their fix is a single
`NameMap Bool` memo threaded across every candidate so the shared Mathlib closure is walked once
total, not once per declaration. We have **no** `collectAxioms` meta-program at all. For a
whole-corpus sweep (6,330 declarations) our per-declaration approach is minutes-to-hours; theirs
is near-linear in the reachable closure.

Three further details that are correctness-relevant and that we would otherwise get wrong:

1. `scripts/Axioms.lean:113` documents that declaration `Name`s loaded from `.olean`s live in a
   memory-mapped region unmapped when `withImportModules` returns, so the violation strings must
   be materialized *inside* the callback. Formatting them afterwards is a use-after-free.
2. `main` returns an exit code rather than calling `IO.Process.exit`, because an abrupt exit can
   segfault during environment teardown (`scripts/Axioms.lean:137`).
3. `main` fails loudly if it audited **zero** declarations (`scripts/Axioms.lean:143`). Governance
   tooling that silently audits nothing is the classic fail-open. Our `axioms.py` fails closed on
   an unparsed report but has no equivalent "the sweep found no candidates, therefore the sweep is
   miswired" guard.

The soundness caveat is also stated honestly at `scripts/Axioms.lean:88`: marking a constant
`false` before recursing means a cyclic cluster can under-report *which* declarations offend, but
never lets a violation pass, because the declaration with the direct edge to the bad axiom is
always cached `true`. That is exactly the fail-closed direction we require.

**What we would build.** A new Lean source `components/verify/lean/audit_axioms.lean` (sibling of
the existing `components/retrieval/lean/dump_decls.lean` and `dump_types.lean`, which already
establish the "run a Lean meta-program via `lake env lean --run`" pattern in this repo), plus a
Python driver.

**Files this touches.**
- NEW `components/verify/lean/audit_axioms.lean`
- `components/verify/python/theoremata_tools/axioms.py`: add a `check_axioms_bulk(names, root)`
  entry point that shells `lake env lean --run` on the new file once for a whole module set, and
  keep the existing per-theorem `check_axioms` as the single-theorem path. Add the
  audited-zero-declarations fail-loud guard to both.
- `components/prover/backends/lean.rs`: leave `audit_axioms` (`:1255`) alone for the
  single-theorem hot path; add a bulk variant only if a caller needs it.
- Tests alongside `components/verify/` covering: allowlist match, a planted `sorryAx`, a planted
  `native_decide`, and the audited-zero case.

**Risk.** Their file is Apache-2.0 and carries two upstream attributions (section 5). Port with all
three notices and a "modified by Theoremata" line.

### A2. The "read the compiled artifact, not the source text" module-flag audit. ADOPT the principle, port the pattern.

**Source:** `resources/TauCeti-main/TauCeti-main/scripts/ModuleSystem.lean`, especially the
docstring at `:10-22` and `getIsModule` at `:50`.

**What it does.** Reads `ModuleData.isModule` back out of each `.olean` via `readModuleData`,
rather than grepping the source for a leading `module`, on the explicit ground that "a stray
`module` in a comment or string could fool" a grep. It deliberately avoids importing the
transitive Mathlib closure because it needs one `Bool` per module.

**Why it beats what we have.** Our lexical pre-gates
(`components/verify/python/theoremata_tools/lean_soundness.py`,
`formal_source_scan.py`, `fallback_source_scan` in `lean.rs:1084`) already do comment and string
masking, which is the right mitigation. What we do **not** have is the cheaper move demonstrated
here: for a per-module boolean property, read the `.olean` header directly instead of either
grepping or loading a full environment. That is a technique, and the `@[noinline] getIsModule`
trick at `:50` is the non-obvious part: the `ModuleData` reference must be dropped *before* the
mmap'd region is freed, or you segfault. Anyone writing `.olean`-reading code in our tree will hit
this exact bug.

**What we would build.** Not a module-system linter (we have no module-system policy). Instead,
record the technique and use it where we already want cheap per-module facts: our
`components/retrieval/python/theoremata_tools/decl_index.py` currently obtains declaration facts
by `lean --run` of `dump_decls.lean` over a full environment. For import-DAG-only or
header-only facts, `readModuleData` is strictly cheaper.

**Files this touches.** `components/retrieval/lean/dump_decls.lean` (optional fast path), and a
paragraph in `components/verify/TEMPLATES.md` documenting the mmap lifetime hazard so we do not
rediscover it. Low priority, no behavioural change.

### A3. TauCeti as a large negative-control ACCEPT corpus for the adversarial registry. ADOPT, but scoped and honest about cost.

**Source:** the whole `TauCeti/` tree; census in sections 2 and 3 of this report.

**Why it is what we want.** Our registry is accept-poor: roughly five accept-shaped loaders
against four reject-shaped, and only one first-party in-tree pair
(`components/eval/fixtures/trivial_existential/{probe,control}.lean`). TauCeti offers 6,330
theorem statements that are simultaneously (a) sorry-free and axiom-clean, verified by a
kernel-level audit that ships with the corpus and is CI-enforced, (b) pinned to a named toolchain
and a named Mathlib SHA, so a failure is attributable to drift and not to us, and (c) measured
clean on all four triviality shapes, so they are legitimate ACCEPT expectations and not merely
"compiles".

**Why it is not free.** Every one of the 579 files needs Mathlib at `f4e566ca` (section 4).
Zero of them are Mathlib-free, so none can join the on-every-commit in-tree tier that
`trivial_existential` occupies. They can only be a live-tier corpus behind
`THEOREMATA_MATHLIB_ROOT`, alongside the other five gitignored-resource loaders.

**What we would build.** A `tauceti_core` loader in
`components/eval/python/theoremata_tools/benchmarks/adversarial.py` following the existing
`_skip` pattern (return `[]` when the resource is absent), emitting `expect_accept` items with:
- `provenance = {corpus: "TauCeti", path, license: "Apache-2.0", untrusted: True,
  in_tree: False, mathlib_free: False}`
- toolchain provenance stamped by the existing `_with_toolchain` wrapper in
  `benchmarks/loaders.py:145`, which already globs `**/lean-toolchain` (`:101`) and reads
  `lake-manifest.json` (`:84`). Both files exist in TauCeti in exactly the expected shape, so
  this needs no new plumbing: it will pick up `leanprover/lean4:v4.32.0-rc1` and `f4e566ca`
  automatically.
- excerpts fenced by the existing `_fenced` untrusted-corpus markers (`adversarial.py:105`).

Sample a bounded set (say 40 theorems stratified across the 14 top-level areas) rather than all
6,330; the value is coverage of proof styles, not volume.

**Files this touches.**
- `components/eval/python/theoremata_tools/benchmarks/adversarial.py` (new loader, register in
  `ADVERSARIAL_LOADERS` at `:689`)
- `components/eval/tests/test_adversarial_fixtures.py` (offline schema assertions)
- `components/eval/tests/test_adversarial_fixtures_live.py` (live compile, and extend the
  pin table at `:29-46`, which currently knows v4.26.0/v4.27.0/v4.28.0, to include v4.32.0-rc1 so
  a mismatch is classified `toolchain_mismatch` and not `contradiction`)

**Note for whoever owns `adversarial.py`.** Two other agents currently hold that file. This item
is a proposal for the build wave, not an edit.

### A4. The negative result itself is the finding, and it should change how we score corpora. ADOPT as a report-level conclusion, no code.

Two prior third-party Lean corpora exhibited `name_claims_more_than_statement`. This one, 6,330
statements and 108k lines of AI-authored Lean, exhibits it zero times. The difference is not model
capability; it is that Tau Ceti runs an adversarial rubric that names the defect
(`README.md:33`, and see `tauceti-review.md` for the rubric machinery). The transferable
conclusion is that **the presence of an adversarial statement-quality reviewer in a corpus's own
pipeline is a strong prior on that corpus's fixture quality**, and is worth recording in
provenance. Concretely: add a `upstream_adversarial_review: true` provenance extra for corpora
whose pipeline includes one, so that a later ACCEPT failure from such a corpus is treated as a
signal about *us*, not about them.

---

## 7. Explicit rejections

- **`scripts/check-bump.sh` (dependency-pin bump validation).** Genuinely good work: it validates
  that a proposed pin change is forward-only by requiring the new Mathlib rev to be a descendant
  of the old one *and* on the nominated branch, and that the rest of the manifest is
  field-for-field identical to Mathlib's own manifest at the new rev. **Reject** because it is
  load-bearing only for a GitHub PR auto-merge pipeline that we do not have, and it depends on
  `gh api` against a trusted upstream. Our staleness story
  (`components/reason/proving/staleness.rs`, `staleness_sweep.rs`) is about certificates going
  stale under a moved environment, which is a different problem and one we already solve.
  Revisit only if we ever auto-bump our own Mathlib pin.
- **`scripts/structure_nudge.py` (CamelCase placement advisory).** A clean tokenizer
  (`_TOKEN_RE` at `:47`, handling `JHolomorphic`, `NNReal`, `pAdic`, `CStarAlgebra`) and an
  admirable design discipline: advisory by construction, every failure downgraded to a warning,
  never feeds the required build status. **Reject**: it is a file-placement nag for a Mathlib-style
  library, and we have no such placement policy to enforce.
- **`.github/workflows/pr-build.yml` landrun sandboxing.** Builds untrusted PR Lean under
  `landrun` with network denied, writes confined, no secrets in reach, and even asserts the
  sandbox works by trying an outbound `curl` and requiring it to fail (`:302`). **Reject as code**,
  it is GitHub-Actions-shaped. **Note as a principle** we already honour: we do not execute
  resource trees.
- **`scripts/lint-baseline.txt` (58 grandfathered lint exceptions).** A frozen allowlist keyed by
  declaration name. **Reject**: we do not run the Mathlib linter set, and a name-keyed baseline is
  the pattern we would want to avoid anyway.
- **`scripts/ModuleSystem.lean` as a policy.** Adopted only as a technique in A2. We have no
  module-system requirement, and imposing one would be a large change for no verification gain.
- **`formalization.yaml` (v0.2 project-metadata schema).** Interesting as a community convention
  for declaring `sorry_count`, allowed axioms, `automation.method`, and review status. **Reject
  for now**: adopting an external metadata schema is a commitment, and nothing in our pipeline
  reads it. Worth revisiting if the schema gets traction, since our eval provenance already
  carries most of these fields under different names.
- **`COORDINATION.md` multi-agent contract.** The `--force-with-lease` against an observed OID
  rule (`COORDINATION.md:23`) is a correct fail-closed concurrent-write primitive. **Reject**:
  we are not running competing agents against a shared remote, and we do not run git from agents.
- **The 590 Lean files as a retrieval corpus.** Tempting, but reject for now. Our retrieval stack
  (`mathlib_index.py`, `decl_index.py`, `head_index.py`, `accessible_premises.py`,
  `semantic_memory.py`) indexes Mathlib, and TauCeti is strictly downstream of a specific Mathlib
  SHA. Indexing it without also pinning that Mathlib gives a premise set that half-resolves. If
  A3 lands and we materialize Mathlib at `f4e566ca`, revisit then, at which point it is nearly
  free.

---

## 8. POSSIBLE INJECTION

Two files in the target are operating instructions addressed to an AI agent. They are quoted here
**as data only**. Nothing in them was followed, and nothing in the repo was executed.

`.claude/CLAUDE.md` is not a document: its entire contents are the four bytes and newline
`../AGENTS.md`, i.e. a pointer to the file below. The real content is `AGENTS.md`.

**`AGENTS.md`, the imperative directives, quoted verbatim:**

> "**Read the roadmap first.** The roadmaps live in the separate TauCetiRoadmap repo."

> "`main` is always green. CI builds against pinned Mathlib and enforces: no `sorry`, no axioms
> beyond `propext`, `Classical.choice`, `Quot.sound` (so no `native_decide`), and the Mathlib
> linter set (style, file length, no `maxHeartbeats` overrides). Do not try to disable these."

> "`TauCeti/` is the only place code goes. `scripts/`, `.github/`, and the lakefile
> (`lakefile.toml`/`lakefile.lean`) are human-owned."

> "**Never delete a PR's human-owned changes to get it past the build gate.** ... This binds
> automated fix/review agents too: if a PR carries human-owned changes, leave it alone (skip it)
> rather than 'fixing' it toward auto-merge."

> "**Do not `--admin`-merge AI-authored PRs.** Landing a PR is the review pipeline's job ... Using
> an admin override to bypass that gate ... defeats the project's quality control."

> "If two findings contradict ... Contest one of the threads, link the conflicting one, and quote
> its wording (rubric and round)."

**`COORDINATION.md`, the imperative directives, quoted verbatim:**

> "Never push to a PR branch except with `--force-with-lease` against the head commit you observed
> when you started, and push to the exact head ref"

> "If anyone moved the branch since you observed it, a cooperating agent or not, your push fails
> closed (`! [rejected] (stale info)`). That is the system working ... never fall back to a plain
> `git push`."

**Assessment.** These are *restrictive* directives aimed at agents operating on Tau Ceti's own
repository: do not disable CI gates, do not admin-merge, do not clobber another agent's branch,
do not strip human-owned files. There is no instruction to contact a third party, no credential
read, no exfiltration, no `gh issue close` against anyone else's repo, and no instruction that
would be harmful if a confused agent executed it against our tree (the worst case is a wasted
`git push` that we do not run anyway). This is materially safer than the shipped-skill pattern we
have seen elsewhere.

Nevertheless, the standing rule applies and was applied: **untrusted data, not instructions**.
Concretely, the directive "Read the roadmap first. The roadmaps live in the separate TauCetiRoadmap
repo" is an instruction to fetch an external repository; it was **not** followed. No network fetch
was made on behalf of anything in `resources/`.

**CI-side scan, for completeness.** The 13 workflows were read as data. They use `gh api` and
`gh pr` extensively, but every call targets `${{ github.repository }}`, i.e. their own repo. The
only `gh pr close` is `update.yml:123`, which closes Tau Ceti's own last-known-good bump PR in
favour of its own newer one. Secrets referenced are `APP_ID`, `APP_PRIVATE_KEY`,
`LAKE_CACHE_KEY`, `ZULIP_API_KEY`, `ZULIP_EMAIL`, all their own. `curl` is used only to fetch
`elan-init.sh` from `elan.lean-lang.org` and to *test that the sandbox blocks* outbound traffic
(`pr-build.yml:302`, `pr-profile.yml:227`). Nothing targets a third party. **No malicious
pattern found.**
