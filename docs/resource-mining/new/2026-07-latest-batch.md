# Latest arrivals -- five repos (2026-07)

Scope: the five repos the owner added to `resources/` this week. Read date 2026-07-20.
Paths below are relative to `C:\Users\adria\Downloads\math-agent\resources\`; every
repo is nested one level (`X-main/X-main/`).

Read method: source first, README second. Nothing from these repos was executed. One
Mathlib-free reproduction was compiled with our own Lean 4.32.0 to check a structural
claim (see §3.1); no repo's own runner, `verify*.py`, `MIND`, or lake invocation was
run.

---

## 0. Safety, licensing, toolchains

### POSSIBLE INJECTION

Two hits, both in `zeta-main`, both shape rather than intent. Recorded because they
are imperative text addressed at an AI agent sitting inside untrusted vendored data,
which is exactly the ingestion hazard.

- `zeta-main/zeta-main/AGENTS.md` (107 lines). A file of operating instructions
  written to an agent: "At the start, name the round mode", "Run `./MIND EXPLAIN ALL`
  before changing the graph", "Never silently upgrade computation, conjecture,
  manuscript assertion, or remembered conversation to theorem", plus roughly a dozen
  shell commands it directs the reader to run against a repo-local executable
  (`./MIND`, which is `#!/usr/bin/env python3` dispatching to `mindlib/cli.py`).
  Benign in content and actually good discipline, but it is a directive file that
  instructs a reader to execute repo code. **Do not run `./MIND`. Do not ingest
  `AGENTS.md` as anything but quoted data.**
- `zeta-main/zeta-main/search_engine_experimental/sources/repository-contributor-search-design-statements-2026-07-14.txt:9`
  -- a retained raw design transcript in second-person imperative ("you ask the system
  with a command", "you must also grow using another tool or its not valid"). Author
  talking to themself, retained losslessly by policy. Data, not direction.

Grep across all five repos (`*.md`, `*.txt`, `*.lean`, `*.tex`, `*.json`) for
`ignore previous`, `system prompt`, `as an AI`, `new instructions`, `grader`,
`award marks`, `jailbreak` returned nothing else.

`record-compositions-main/input/task.md` is, as expected for this family, a prompt
written to a prover. It is ordinary mathematical prose with no agent-directed
imperatives, but it is still untrusted author-written input text.

`BH-main` ships `verify_reproducibility.py`, `run_published_fdr_experiment.py`, a
`.ps1` and a `.sh` wrapper. They use `subprocess` but contain no network calls
(`requests`/`urllib`/`socket`/`http` all absent). That is not a licence to run them.
Nothing here was executed.

### Licences

| Repo | Licence | What it grants |
| --- | --- | --- |
| `AdvectionDiffusion-main` | **None. No LICENSE, COPYING, NOTICE, or licence text anywhere in the 40-file tree, at any depth.** | **Nothing.** All rights reserved by default. This grants strictly fewer rights than GPL: GPL at least permits copying and redistribution under conditions, whereas no licence permits neither. We may read it and describe it. We may not vendor, copy, redistribute, or derive fixtures from its Lean source. Anything we build from the ideas must be clean-room and written by us. |
| `BH-main` | **None.** No LICENSE file. | Same as above: all rights reserved, fewer rights than GPL. Moot, since we are not using it. |
| `zeta-main` | MIT, "Copyright (c) 2026 ultimussaeculi" (author named in file headers as Joshuah Rainstar) | Clean. Vendorable with attribution, including verbatim Lean. |
| `record-compositions-main` | MIT, "Copyright (c) 2026 Axiom Math." | Clean. Same family and same terms as the ten Axiom Math repos in `math-problem-corpora.md`. |
| `RogersRamanujan-main` | **None.** Confirmed: no licence file, and no file in the 99-file tree contains the string "licen". | All rights reserved, fewer rights than GPL. **Do not vendor.** This restates an existing finding (`2026-07-new-arrivals.md:248`, `backlog-rm-new.md:217`), which is why nothing here changes that entry. |

No GPL or AGPL in the batch, so no copyleft contamination. Three of five repos are
unlicensed, which is the dominant practical constraint on this batch: the single best
reject-shaped specimen we found (§3.1) sits in an unlicensed repo.

### Toolchains

Per-item toolchain metadata matters here because two of these pin versions we cannot
currently run against a checked-out Mathlib, and one pins nothing at all.

| Repo | `lean-toolchain` | Mathlib pin | Notes |
| --- | --- | --- | --- |
| `AdvectionDiffusion-main` | **absent** | **absent** | No `lakefile`, no `lake-manifest.json`, no toolchain file. Every proof file opens with a bare `import Mathlib`. There is no reproducible build here at all; "which Mathlib" is unanswerable from the repo. A build failure against any Mathlib would be uninterpretable. |
| `BH-main` | n/a | n/a | Not a Lean repo. Pins Python 3.12.3, NumPy 1.26.4, SciPy 1.14.1, python-flint 0.8.0 in `requirements.txt` and `.python-version`. |
| `zeta-main` | `leanprover/lean4:v4.32.0` | `mathlib` rev `v4.32.0`, `AXIOMS.md` records commit `81a5d257c8e410db227a6665ed08f64fea08e997` | Matches the Lean 4.32.0 we have installed. Best-pinned repo in the batch. |
| `record-compositions-main` | `leanprover/lean4:v4.31.0` | `lake-manifest.json` present | 4.31.0 toolchain is installed here. |
| `RogersRamanujan-main` | `leanprover/lean4:v4.31.0` | `mathlib` rev `v4.31.0` | Also uses the Lean module system (`module`, `public import`, `@[expose] public section`) and sets `weak.linter.mathlibStandardSet`, so it is tightly coupled to 4.31 and will not survive version drift. |

We have Lean 4.21.0, 4.25.0, 4.31.0, 4.32.0, 4.32.0-rc1 under `~/.elan/toolchains`,
but **no Mathlib checkout anywhere in this project**. So none of the four Lean repos
can actually be built here today. Every sorry count below is a static count on the
source, which is exactly the right way to count them, but "compiles" is not claimed
for anything.

---

## 1. Catalogue

| Repo | What it actually is (from source) | Licence | Size | Problem/solution contract | `sorry` counts | Verdict |
| --- | --- | --- | --- | --- | --- | --- |
| `AdvectionDiffusion-main` | Machine-generated output from "Lanyon" (lanyon.ai): for 9 PDE variants (linear advection, isotropic advection-diffusion, full advection-diffusion, each in 1D/2D/3D) it emits a Racket S-expression spec, a C solver, and a Lean file proving algebraic identities about the flux and reconstruction expressions. Not a formalization of the PDE, its discretization, or the C code. | **NONE** | 9 `.lean`, 9,569 lines, 273 theorems; 9 `.c`; 9 `.rkt`; 9 `.mov` screencaps; 40 files | No. Single-file-per-variant; no spec/proof split | 0 in all 9 files. Also 0 `admit`, 0 `axiom`, 0 `native_decide` | **REJECT-shaped material, but UNUSABLE as a vendored fixture (no licence).** Clean-room lesson only |
| `BH-main` | A reproducibility bundle for a claimed **counterexample to Benjamini–Hochberg FDR control** under a two-sided Gaussian factor model: an outward-rounded Arb (python-flint) interval certificate proving `liminf FDR > 0.01041682... > alpha = 0.01`, plus a seeded stratified Monte Carlo experiment at N = 50/100/200. Statistics, not formal mathematics | **NONE** | 8 `.py` (1,463 lines), 20 CSV/JSON, 43 files. **0 Lean** | No | n/a | **SKIP** |
| `zeta-main` | Two things. (a) `MIND`: a local causal knowledge graph for Riemann-hypothesis research -- 197 Python files, 26,608 lines, with factoid/citation/certificate records, a BM25F-ish search index, and a replayable-certificate discipline. (b) `proof/formal/PF4`: 7,975 lines of Lean proving a **generic** total-positivity / transport engine, plus a claim ledger mapping paper claims to Lean declarations with an explicit status vocabulary | MIT | 822 files; 30 `.lean` (7,975 lines, of which `proof/formal/PF4/` is 4,624 and `work/` holds per-round copies); 197 `.py` | No (blueprint-style, not spec/proof pairs) | 0 `sorry`, 0 `admit`, 0 `axiom`, 0 `native_decide` across all 30 files | **ACCEPT-CONDITIONAL fixture + a genuine tooling mining target** |
| `record-compositions-main` | Standard Axiom Math / AxiomProver artifact for arXiv:2607.12873 (record compositions of alternating permutations and NSym). `input/task.md` + `problem.lean` spec + `solution.lean` proof | MIT | 11 files; 2 `.lean`, 4,028 lines | **Yes**, canonical | `problem.lean` **8** (lines 216, 224, 234, 240, 249, 257, 265, 277); `solution.lean` **0**. 9 theorems declared, 8 sorried -- `wex_isDownUp` is proved `by decide` in the spec too | **ACCEPT fixture (solution.lean only), low novelty** |
| `RogersRamanujan-main` | A serious Mathlib-style research library: the two Rogers–Ramanujan identities via Bailey's lemma, over any topological ring where `q` is topologically nilpotent, plus Pentagonal Number Theorem, Jacobi triple product, and a `Counterexamples/NotStrong.lean` construction. Roughly three quarters of it is missing-Mathlib substrate (non-archimedean topology, `MvLaurentSeries`, `MvPowerSeries` evaluation) rather than q-series | **NONE** | 99 files; 92 `.lean`, 9,859 lines | No | **0 live `sorry`.** The single grep hit is `NumberTheory/QTheory/HypergeometricSeries.lean:89`, a commented-out `--   sorry`. 0 `axiom`, 0 `native_decide`, 0 `admit` | **SKIP for vendoring (no licence).** Already licence-flagged in existing docs |

---

## 2. How `RogersRamanujan-main` differs from `RogersRamanujan-artifacts-main`

They share a name and nothing else. `-artifacts` is already mined
(`math-artifacts-batch2.md:47`, shortlist item at `:87`) and registered on the
backlog; this section exists only so the two are never conflated again.

| | `RogersRamanujan-artifacts-main` (already mined) | `RogersRamanujan-main` (this batch) |
| --- | --- | --- |
| Kind | Axiom Math artifact release: 6 independent problem/solution pairs in flat directories (`jacobi-identity/`, `jacobi-triple-product/`, `limit-topologically-nilpotent/`, `pentagonal-number-theorem/`, `pfaff-saalschuetz/`, `prescribed-open/`) | One Mathlib-shaped library with a 93-line root `RogersRamanujan.lean` importing a 92-file namespace tree |
| Contract | Yes: `problem.lean` + `solution.lean` per directory, some with `task.md`, several with `*_updated.lean` re-runs | No. Ordinary library modules |
| Licence | MIT, Axiom Math | **None** |
| Toolchain | **Three different pins in one repo**: 4.26.0 (`jacobi-triple-product`), 4.28.0 (`prescribed-open`), 4.31.0 (the other four); one `lakefile` per subdirectory | Single pin, 4.31.0, one `lakefile.toml`, one Mathlib rev |
| Size | 42 files, 3,168 solution lines | 99 files, 9,859 lines |
| Notable content | `prescribed-open` is a **proved refutation** -- the only negative result in ~40 problems and the reason it is on the backlog | The final theorems `first_rogers_ramanujan` / `second_rogers_ramanujan` at `RogersRamanujan/NumberTheory/QTheory/RogersRamanujan.lean:351,358`; a Frechet-space counterexample separating nonarchimedean from strongly nonarchimedean; and a custom Lean parser elaborator in `Util/Unconditional.lean` |
| Overlap | -- | The two repos overlap in *subject* (both cover Jacobi triple product, pentagonal number theorem, topologically nilpotent limits) but share no files and no build. The artifacts repo is the AxiomProver run; this is what a human library for the same material looks like |

The genuinely interesting contrast is the size ratio: the same mathematical
territory is 3,168 lines as spec/proof artifacts and 9,859 lines as a library, and
the library's bulk is substrate. That is a real datum for cost modelling, and it is
already recorded at `2026-07-new-arrivals.md:214`. Nothing new to wire.

---

## 3. The findings worth acting on

### 3.1 `AdvectionDiffusion`: 18 theorems named for a property they do not state

This is the sharpest specimen in the batch and it is the one we cannot use directly.

`proofs/linear_advection_1d.lean:100`:

```lean
theorem xHyperbolicity (C : Coordinates) (P : Parameters) (U : State) :
    (∃ r1 : Real, r1 = (xFluxJacobianEigenExprs C P U).lambda1) := by
  . refine ⟨(xFluxJacobianEigenExprs C P U).lambda1, rfl⟩
```

Hyperbolicity of a conservation law means the flux Jacobian has real eigenvalues and
a complete set of eigenvectors. The statement above says only that some real number
equals `lambda1`, which is `rfl` for any expression whatsoever of type `Real`. It
carries no information. The same statement, with `y`/`z` variants, appears 18 times
across the 9 files (1 in each 1D file, 2 in each 2D file, 3 in each 3D file).

We checked the reading by compiling a Mathlib-free reproduction under our own Lean
4.32.0, with the eigenvalue body replaced by the constant `12345.0`: the proof term
is unchanged and still succeeds, and `#print axioms` reports only `Classical.choice`
(from `noncomputable`). The statement shape cannot distinguish a real eigenvalue
computation from a wrong constant.

Two more drift signals in the same files:

- `Coordinates` (i.e. the spatial position) is bound in every theorem and consumed by
  none. The generator emits `let x := C.x` at the head of each definition and then
  never uses `x`. This is the same shape as the `PartitionElliptic` `0 < y` finding
  already registered as `partition_elliptic:free_variables`.
- The README claims "**End-to-end** formally verified solvers" and "8,292 lines of
  formally verified C code". The Lean files contain **zero** references to the C
  sources -- no extraction, no refinement relation, no compiler-correctness argument,
  no discretization or convergence statement. The Lean proves algebraic identities
  about symbolic flux expressions; the C is a separate emission from the same Racket
  spec. The C's own `linear_advection_1d_state_valid` is a function whose body is
  `return true;`. "End-to-end verified" is a claim the artifact does not support.

**Gate layer stressed:** the one we currently have no probe for. Our reject
vocabulary has `vacuous_hypothesis`, `unencoded_side_condition`, `missing_witness`.
This is none of them precisely: the hypotheses are fine, no side condition was
dropped, nothing is unwitnessed. The defect is that the *conclusion* is trivial while
the theorem's name and surrounding prose assert a substantive property. Call it
name/claim drift. `unencoded_side_condition` is the closest existing reason and it
does not fit well.

**And we cannot vendor it.** No licence means we may not copy the file into
`resources/`-backed fixtures, quote it into an item `excerpt`, or ship a derived
copy. The actionable form is therefore: write our own 20-line Lean file exhibiting
the pattern (a theorem named for a property, whose statement is `∃ r, r = e`), and
register that as the probe. That is clean-room and costs an afternoon. The
AdvectionDiffusion repo becomes a citation in the doc, not a fixture on disk.

### 3.2 `zeta-main`: an honest claim ledger over a proof that never reaches its subject

`zeta-main` is the strongest repo in the batch and the only one worth wiring on its
own merits.

The paper claims strict Pólya-frequency order four (and exact order four) for the
Riemann kernel, an RH-adjacent result. The Lean is 7,975 sorry-free, axiom-free lines.
But: **the string `riemannZeta` does not occur anywhere in the Lean, nor does
`completedRiemann`, `Gammaℝ`, or any xi function.** The whole development is
universally quantified over abstract `Q Q1 Q2 Q3 Q4 : ℝ → ℝ` with derivative,
continuity and positivity hypotheses. `proof/formal/PF4/FinalAssembly.lean:88`:

```lean
theorem coordinatePartialXiPsi_neg_from_determinantC4
    {Q Q1 Q2 Q3 Q4 : ℝ → ℝ} {p z w : ℝ}
    (hpz : p < z) (hzw : z < w)
    (hQ : ∀ t, HasDerivAt Q (Q1 t) t)
    ... (hQpos : ∀ t, 0 < Q t) (hκpos : ∀ t, 0 < curvature Q2 t)
    (hdetC4pos : ∀ t, 0 < PF4.C4Invariant.determinantC4Function Q Q1 Q2 Q3 Q4 t) :
    Q p * deriv (...) p < 0
```

Nine hypotheses, and nothing anywhere instantiates them at the Riemann kernel. Its
docstring calls it "Premise-free PO-0041 assembly relative to the named upstream
analytic inputs", which is a careful phrase doing a lot of work.

What makes this valuable rather than merely another conditional result is that **the
repo says so itself, in machine-readable form.** `proof/CLAIM_INDEX.md` is a table of
26 claims, each with a paper anchor, a certificate reference, proof obligations, the
Lean declaration name, and a status drawn from a fixed vocabulary:
`FORMALLY_PROVED`, `FORMAL_FRAGMENT`, `CERTIFIED` (replayable numeric certificate,
not Lean), `CONVENTIONALLY_PROVED` (pen and paper). The top claim reads:

> | strict PF4 | S01/S10 | R164, CERT5/9/12 | PO-0042 | generic engine checked; translation/`Ψ` instance unset | CERTIFIED |

"Instance unset" is the author disclosing exactly the gap we found. And
`proof/formal/AXIOMS.md` records the audit we usually have to perform ourselves: 36
named declarations, `lake env lean PF4/Audit.lean`, all depending on exactly
`propext`, `Classical.choice`, `Quot.sound`, with the Mathlib commit written down.
`proof/formal/PF4/Audit.lean` is nothing but 36 `#print axioms` lines. Recall from
`math-problem-corpora.md` §6 that across 21,000+ lines of Axiom Math solutions there
was exactly **one** `#print axioms`. This repo has 36 and a written-up result.

**Gate layers stressed, and this repo stresses three of them at once:**

1. **Formalization drift from the informal claim, at maximum distance.** The paper is
   about the Riemann kernel; the Lean is about arbitrary smooth positive functions.
   A gate that reports "PF4 for the Riemann kernel: formally proved" has failed.
2. **Mixed evidence kinds.** `CERTIFIED` here means a replayable numeric certificate
   (`certificates/pf4-tail-jb-residuals.bin.xz`, 38,103 + 53,001 residual integers),
   not a Lean proof. Our certificate schema currently has no way to say "this step is
   backed by interval arithmetic, that one by a kernel proof, that one by pen and
   paper". This repo is the concrete case that forces the distinction.
3. **Hypothesis-conditional statements whose conditions are never encoded** -- the
   opposite of `PartitionElliptic`. Here the hypotheses *are* encoded, carefully; what
   is missing is the instantiation. That is the `ramanujan_tau` shape
   (`RamanujanTau` uninhabited-unchecked) at a larger scale and with the author's own
   ledger as the answer key.

The `MIND` half is a separate mining target and is out of scope for a fixture doc,
but it should not be lost: 26,608 lines of Python implementing a citation/certificate
provenance graph with a rule that a factoid is established only via a direct external
citation or a replayable local certificate, and cycles/missing refs are rejected. That
is our provenance problem, solved by someone else, MIT-licensed. It deserves its own
read. **Do not run `./MIND`.**

### 3.3 `record-compositions`: contract holds, novelty is low

Counted rather than assumed, per the standing instruction:
`RecordCompositions/problem.lean` has **8** `sorry`s and
`RecordCompositions/solution.lean` has **0**. Nine theorems are declared in both files
with matching names; the ninth, `wex_isDownUp`, is discharged `by decide` in the spec
itself, which is why the count is 8 and not 9. No `axiom`, no `admit`, no
`native_decide` in either file. The contract holds.

Four of the nine theorems are the mathematics (`N_eq_rhsBinom`, `N_eq_rhsRat`,
`factorial_smul_Anc_eq`, `chi_Anc_eq`) and four are deliberate anti-mismodelling
checks (`euler_values` against OEIS A000111, `euler_recurrence` proved the hard way,
`rhsBinom_eq_rhsRat` cross-checking the two closed forms, `rc_worked_example`
discharging the paper's own worked example by `decide`).

**This is not a new finding.** Those sanity theorems are already written up as adopt
item 25 in `2026-07-new-arrivals.md:344`, and the elaboration-option mismatch between
this repo's two files (`problem.lean` sets
`backward.isDefEq.respectTransparency false`, `solution.lean` does not) is already
handled in code at `components/prover/statement_preservation.rs:399`. What this pass
adds is only the exact sorry counts and the confirmation that the contract is not
violated. As a fixture it is one more MIT accept-shaped Axiom Math item among many;
its marginal value over `record-compositions`' ten siblings is small.

### 3.4 `BH-main`: not our object

A statistics reproducibility bundle. The mathematical claim -- that BH does not control
FDR at level alpha under a specific two-sided Gaussian factor model -- is genuinely
falsification-shaped and therefore superficially attractive, since falsification
fixtures are what we lack. It is not usable:

- **Zero Lean.** The evidence is a python-flint Arb certificate plus Monte Carlo.
- **No licence.** All rights reserved, fewer rights than GPL.
- **Structurally incomplete.** Its own README states "`main.tex` is the sole
  authoritative manuscript"; neither `main.tex` nor `main.pdf` is in the repo. The
  legacy README points at the same absent files. This is the same defect as
  `goldbach-collatz-proof`, whose `sections/*.tex` were likewise missing.
- The scripts are the repo's own runners and must not be executed.

There is one idea worth remembering without touching the repo: the certificate output
format (`bh_two_sided_gaussian_counterexample_certificate_output.txt`) is a
per-subinterval outward-rounded lower bound plus a total plus a one-line
`CERTIFIED: ... > alpha` verdict. That is a clean shape for a numeric-certificate
record. We already have interval-arithmetic work in `poly_minimax` / Bernstein bounds
(commit `4770dbc`), so this is a footnote, not a task.

---

## 4. Ranked shortlist: what to actually wire

Two items. That is the honest count.

### 1. `zeta_pf4` -- new adversarial corpus, `expect_accept_conditional`, 1 item

**Path:** `resources/zeta-main/zeta-main/proof/formal/PF4/FinalAssembly.lean`
(107 lines; the whole `PF4/` library is 4,624 lines across 18 modules).
**Licence:** MIT. **Toolchain:** Lean 4.32.0, Mathlib rev
`81a5d257c8e410db227a6665ed08f64fea08e997` -- the only repo in this batch whose pin
matches a toolchain we have installed. **Sorries:** 0, verified by grep across all 30
`.lean` files. **Axioms:** none custom; upstream `AXIOMS.md` records
`propext`/`Classical.choice`/`Quot.sound` for 36 audited declarations.

Verdict `expect_accept_conditional`, following the `ramanujan_tau` precedent in
`adversarial.py:494`. The Lean is real, careful and sorry-free, so rejecting it is a
false positive; but the paper's claim is about the Riemann kernel and the Lean never
mentions it, so an unconditional accept asserts a theorem nobody proved. Proposed
hypothesis set for the certificate:

- `Q_positive_smooth_generic` (the nine binders of
  `coordinatePartialXiPsi_neg_from_determinantC4`)
- `determinantC4_positive_assumed`
- `riemann_kernel_instantiation_unset` (the repo's own words: "translation/`Ψ`
  instance unset")

`required_report_fields = ["hypotheses", "warnings"]`, as with `ramanujan_tau`.

**Point at `FinalAssembly.lean`, not at the claim ledger.** This corpus has no
`problem.lean`, so the stub trap that produced the four bad ACCEPT fixtures does not
arise here -- but the analogous trap does: `CLAIM_INDEX.md` and `AXIOMS.md` are
*documentation asserting* the proof's status, and pointing a fixture at them would
assert that our gate must accept a claim about a proof rather than the proof. They are
the answer key for the test, not the input.

Why this and not something cheaper: it is the only item in the batch that is
MIT-licensed, buildable in principle against an installed toolchain, sorry-free, and
tests a layer we handle badly. It also ships its own ground truth, which is what makes
the assertion checkable rather than a policy judgement -- the weakness flagged as
risk 3 in `math-problem-corpora.md` §6.

### 2. A clean-room `trivial_conclusion` probe, modelled on `AdvectionDiffusion`

**Not a vendored fixture.** `AdvectionDiffusion-main` has no licence, so we write our
own file. Roughly 20 lines: a structure with a field, a definition computing it, and a
theorem named for a substantive property whose statement is `∃ r : ℝ, r = f x`,
proved by `⟨_, rfl⟩`. Reference the source pattern in a comment
(`resources/AdvectionDiffusion-main/.../proofs/linear_advection_1d.lean:100`, 18
occurrences) without copying it.

Verdict `expect_reject`. **This needs a new reason string.** None of
`vacuous_hypothesis`, `unencoded_side_condition`, `missing_witness` describes "the
conclusion is trivially true and the theorem's name claims otherwise". Adding a fourth
member to `REJECT_REASONS` in `adversarial.py:70` is a schema change and should be
decided deliberately rather than by shoehorning this into the closest existing label --
a reject for the wrong reason is, by that module's own docstring, a coincidence rather
than a passing test.

Cost is low and the fixture is ours, so it has no licensing or toolchain rot exposure
and can run on every commit. It is the only per-commit-tier item in the batch.

### Considered and not wired

- **`record_compositions`** -- MIT, contract-clean, 0 sorries in `solution.lean`. Wire
  it only if we are batch-loading the Axiom Math family; on its own it duplicates
  coverage we already have and its two genuinely novel features (sanity theorems,
  elaboration-option mismatch) are already written up and, in the second case, already
  implemented. If it is wired, the fixture path is
  `RecordCompositions/solution.lean`, **never** `problem.lean` (8 sorries).
- **`RogersRamanujan-main`** -- technically the best mathematics in the batch and
  unusable. No licence. Already flagged in two existing docs; nothing changes.
- **`AdvectionDiffusion-main` as a vendored fixture** -- no licence. See item 2 for the
  usable form.
- **`BH-main`** -- no Lean, no licence, missing manuscript. SKIP outright.

---

## 5. Risks

1. **Nothing in this batch was compiled.** We have Lean 4.21/4.25/4.31/4.32 installed
   and **no Mathlib checkout in the project**, so all four Lean repos are unbuildable
   here today. Every sorry, axiom and `native_decide` count above is a static grep on
   the source. Before `zeta_pf4` becomes an assertable fixture it must build under
   Lean 4.32.0 against Mathlib `81a5d257c...`, which means acquiring that Mathlib. The
   only thing we compiled is our own 12-line reproduction in §3.1, which validates a
   reading of a statement shape and nothing about any repo.

2. **Three of five repos are unlicensed, including the one with the best specimen.**
   The `xHyperbolicity` pattern is the most instructive thing here and we may not copy
   a byte of it. The clean-room rewrite is straightforward, but it means the fixture no
   longer has provenance to a real artifact, which weakens it: a probe we wrote to fail
   our own gate is a weaker test than a probe found in the wild.

3. **`zeta_pf4`'s conditional verdict is a judgement call, and a harder one than
   usual.** The author would say, correctly, that they proved a generic engine and
   labelled it as such in a public ledger. Our verdict is a policy about what a
   certificate must carry, not a finding about the mathematics. The mitigation is
   better than usual -- `CLAIM_INDEX.md` supplies the author's own status per claim, so
   we can assert agreement with their ledger rather than with our reading of it -- but
   the policy is still ours. Same caveat as `math-problem-corpora.md` §6.3.

4. **`zeta-main` mixes evidence kinds and our schema cannot express that.**
   `CERTIFIED` (Arb interval replay), `FORMALLY_PROVED` (Lean), `FORMAL_FRAGMENT`
   (partial Lean) and `CONVENTIONALLY_PROVED` (pen and paper) are four different
   epistemic states in one dependency chain. If we register this repo and flatten it
   to accept/reject, we throw away the distinction the repo exists to make. Either
   extend the certificate schema first or register the item knowing the fixture is
   coarser than its source.

5. **`AGENTS.md` is a live ingestion hazard.** It is a well-written file of agent
   instructions that tells the reader to run a repo-local Python executable, sitting in
   a directory we treat as a corpus. If any retrieval or context-assembly path ever
   globs `resources/**/*.md`, that file is a directive-shaped document entering a
   prompt. `adversarial.py` already fences excerpts via `_fenced`; anything else that
   reads `resources/` needs the same treatment.

6. **`record-compositions` risks padding the accept side.** We already have an
   accept-heavy fixture set. Adding a twelfth MIT Axiom Math accept-shaped item makes
   the numbers look better without testing anything new, and deepens the
   single-vendor monoculture already flagged as risk 4 in `math-problem-corpora.md`.

7. **Toolchain spread is still unmanaged.** This batch adds 4.31.0 and 4.32.0 to a
   corpus already spanning 4.21 through 4.31, plus one repo (`AdvectionDiffusion`) with
   no pin at all whose build is therefore not reproducible even in principle. Per-item
   toolchain metadata in the loaders is still NOT-BUILT (`backlog-rm-new.md`, item
   B10); until it exists, any build failure on these items will be indistinguishable
   from a proof failure.
