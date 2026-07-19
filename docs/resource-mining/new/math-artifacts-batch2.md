# Math artifacts batch 2: fixture catalogue (11 repos)

## Correction to the premise of this pass

The brief said these 11 repos "have at most a passing name-drop in our docs and were
never actually read." That is wrong. `docs/resource-mining/new/2026-07-new-arrivals.md`
(404 lines) covers all 11 as part of a 34-repo sweep, read them fully, and reaches
conclusions I re-verified here and agree with: the Axiom Math cluster identification,
the `gdm-formal-conjectures` misnomer, the `SafeVerify.lean` find, the
`TanArctan` scratchpad flag, and the eval-target ranking.

I re-read the 11 anyway and this doc adds only what that one lacks: a per-repo
fixture decision table checked against `registry.py` and `loaders.py`, a specific
wiring recommendation, and one artifact that pass missed (a false-premise pair in
`gdm-formal-conjectures`). Everything else, read the earlier doc.

Licensing: all 11 are MIT. Ten are Axiom Math (`Copyright (c) 2026 Axiom Math`),
`lambda-eval` is `Copyright (c) 2023 Nicolas Abril`. No GPL or AGPL. Code copying is
permitted with attribution.

## What the whole cluster is, structurally

Ten of the 11 are AxiomProver output artifacts with one uniform contract:

- `input/task.md` (or `task.md`): a natural-language prompt, sometimes with a
  `.tex` or `.pdf` source attached.
- `problem.lean`: definitions plus theorem statements whose bodies are `sorry`.
  The `sorry`s are the specification holes, not failures.
- `solution.lean`: restates the same definitions and theorems and proves them.
  Zero live `sorry`s in every solution file in this batch.
- Verification is `problem.olean` against `solution.olean`, not text diffing.

That contract is the reason these are usable at all: an ACCEPT-fixture needs a
statement and a proof that are separately identifiable, and this gives us both.

## Catalogue

Line counts are of the `.lean` files. "Complete" means every stated theorem is
proved with no live `sorry` and no declared `axiom`. Nothing was compiled; this
is textual verification.

| Repo | Content | System | Complete | Size | Proposed use |
|---|---|---|---|---|---|
| `IMO2026-main` | 6 IMO 2026 problems, statements and full proofs | Lean 4.31.0 | yes | 7,722 solution lines | benchmark registry, ACCEPT |
| `Putnam2025-main` | 12 Putnam 2025 problems, plus `SafeVerify.lean` and a `lake` verify script | Lean 4.21.0 | yes | 11,861 solution lines | ACCEPT, and port `SafeVerify` |
| `gdm-formal-conjectures-main` | 2 solved problems drawn from DeepMind's repo, not that repo | Lean 4.28.0 | yes | 1,004 solution lines | REJECT-fixture (see below) |
| `RogersRamanujan-artifacts-main` | 6 independent q-series and topology pairs, one a proved refutation | Lean 4.26 / 4.28 / 4.31 | yes | 3,168 solution lines | ACCEPT, plus the refutation |
| `PartitionPolynomial-main` | 6 conjectures from arXiv:2605.21718, with TeX statements | Lean 4.28.0 | yes | 12,064 solution lines | benchmark registry, ACCEPT |
| `zeta-h123-main` | 4 tasks from arXiv:2606.16239, each with `informal.tex` | Lean 4.28.0 | yes | 5,893 solution lines | benchmark registry, ACCEPT |
| `quadratic-dinv-main` | 1 unpublished paper, 5-part spec with a computable mirror | Lean 4.28.0 | yes | 2,601 solution lines | ACCEPT, single item |
| `LatentError-main` | 1 statistics paper, 3 classical facts as explicit hypotheses | Lean 4.28.0 | yes | 2,674 solution lines | ACCEPT, conditional-report test |
| `TanArctan-main` | 1 task, 3 statements about arctan sums | Lean 4.28.0 | yes | 1,077 solution lines | ACCEPT, plus scanner test |
| `parity-differential-main` | 1 combinatorial lemma from arXiv:2602.03722 | Lean 4.26.0 | yes | 148 solution lines | ACCEPT, cheap smoke test |
| `lambda-eval-main` | Rust lambda-calculus reducer, 2023, unrelated to proving | none | n/a | ~500 Rust lines | SKIP |

Blueprint-scale targets: none. The largest single solution here is 4,229 lines
(IMO2026 Q3) and the largest per-repo total is 12,064. These are fixtures, not
blueprint-scale runs.

## REJECT-fixtures

There is exactly one wrong claim in this batch, and one proved negation. Both are
worth wiring; nothing else here is a REJECT-fixture and I will not pretend otherwise.

### 1. `gdm-formal-conjectures/input/BorweinSineSeries/problem.lean` (false premise, 9 lines)

The repo ships the input statement and the corrected statement side by side. The
input hardcodes an irrationality-measure bound for pi:

```
C / (q : ℝ) ^ (7.6063 : ℝ) < |Real.pi - (p : ℝ) / (q : ℝ)|
```

`docs/BorweinSineSeries.md` states plainly that 7.6063 "is actually slightly lower
than Salikhov's bound 7.606308...", so the hypothesis `PiIrrBound` as originally
stated is false. A false hypothesis makes `borwein_sine_series` **vacuously true**,
and any proof of it is worthless while being kernel-clean, `sorry`-free, and
statement-preserved. The corrected version in `BorweinSineSeries/problem.lean` uses
7.10321 and is sound.

This is the cheapest available regression test for the `VacuousSuccess` work the
earlier doc proposes: two 9-to-12-line statements differing in one numeral, one of
which our gate must refuse to certify and the other of which it must accept. It also
exercises the exact failure shape our gate handles worst, an unproved numeric
constant smuggled in as a hypothesis rather than as an axiom.

### 2. `RogersRamanujan-artifacts/prescribed-open` (proved negation, 6-line statement)

```
theorem nil_stable_not :
    ¬ ∀ (R : Type) [CommRing R] [TopologicalSpace R] [NonarchimedeanRing R],
    {x : R | ∀ q, IsTopologicallyNilpotent q → IsTopologicallyNilpotent (x * q)} ∈ nhds 0
```

A plausible-looking universally quantified claim, machine-checked to be false, with
a 75-line disproof. Use the positive form as the falsifier input: our falsifier
should refute it and our gate must never certify it. The 75-line refutation is the
ground-truth answer key. This is the only negative result in the batch.

## Gate stress points found

- **Vacuity by false numeric hypothesis.** BorweinSineSeries, above.
- **Conditionality carried in the signature.** `LatentError/problem.lean` states
  three classical facts (Kolmogorov SLLN, Weyl, Davis-Kahan) as `def ... : Prop`
  and passes them as hypotheses to all four main theorems, deliberately and with
  the task file demanding it: "They are not axioms, not sorried lemmas, and not
  structure fields." Correct practice, and invisible to `#print axioms` and to a
  `sorry` grep. It belongs in the unaccounted-hypothesis-audit fixture set next to
  `ramanujan-tau`, as the *negative* control: here the conditionality is legitimate
  and declared, so the audit must report it as conditional without failing it.
- **Elaboration options differ between spec and proof.** All six IMO2026
  `problem.lean` files and `RogersRamanujan/jacobi-identity/problem.lean` set
  `set_option backward.isDefEq.respectTransparency false`. A statement-preservation
  check that elaborates the two files under different options is comparing terms
  under different defeq rules. Confirm `statement_preservation.rs` pins options.
- **Comment-grep false positive.** `TanArctan/output/solution.lean` contains no
  live `sorry`, but lines 547 and 557 are prose describing a sketch phase
  ("three named sub-lemmas, each a `sorry`"). A naive comment-inclusive `sorry`
  scan rejects a correct file. The earlier doc argues the scan should not exempt
  comments; this file is the counterexample showing a comment-inclusive scan needs
  to be advisory rather than fail-closed.
- **Toolchain spread.** Lean 4.21, 4.26, 4.28, 4.31 across the batch, and 4.26 vs
  4.28 vs 4.31 *within* `RogersRamanujan-artifacts` alone. There is no single
  Mathlib pin that runs this corpus. Any loader must carry the per-item toolchain.
- **No non-Lean formal system anywhere in the batch.**

## The gdm-formal-conjectures verdict

**It is not Google DeepMind's `formal-conjectures` repository, and it is not a
curated set of open conjectures.** It is an Axiom Math artifact repo (`Copyright (c)
2026 Axiom Math`, MIT, same four maintainers as the rest of the cluster) containing
solutions to **two** problems that DeepMind's repo had listed:
`BorweinSineSeries` and `OeisA6697`. 19 files, 118 KB.

Answering the specific questions asked:

- **How many conjectures:** two, and both are **solved**, not open. Borwein sine
  series was already settled in arXiv:2007.11017 (Boppana); OeisA6697 was derived
  from the Allouche-Shallit closed form in arXiv:1605.02361. AxiomProver was given
  them as test cases, in the Borwein case explicitly "without referencing Boppana's
  paper."
- **Lean and Mathlib version:** `leanprover/lean4:v4.28.0`, Mathlib pinned to
  `v4.28.0` in `lakefile.toml`.
- **Are statements `sorry`-bearing by design:** the `problem.lean` files are,
  because that is this cluster's spec convention, not because the conjectures are
  open. `BorweinSineSeries/solution.lean` (804 lines) and `OeisA6697/solution.lean`
  (200 lines) are both complete.

**Recommendation: do not wire it into the benchmark registry as a conjecture
source.** Two solved problems is not a benchmark. Wire the BorweinSineSeries
false-premise pair as a vacuity fixture, which is the only thing here we cannot get
elsewhere.

If open formalized conjectures are actually wanted, the real DeepMind repo
(`google-deepmind/formal-conjectures`, Apache-2.0, roughly 500 Lean statements over
600-plus open problems, with `sorry` by design because the problems are open) is
**absent from `resources/`** and needs fetching separately. That is a real and
worthwhile registry candidate, and it is the one thing that would force the
distinction the brief cares about: an intentionally unproved conjecture must be
reported as an open target, never as a failed proof attempt. Nothing in this batch
exercises that distinction, because nothing in this batch is open.

## Overlap with what we already register

Checked against `registry.py` (33 entries) and `loaders.py`.

- **`imo2025`** loads `IMO2025-main`. `IMO2026-main` is a different year from the
  same producer, so no duplication, and the loader shape should transfer directly.
- **`putnam_artifacts`** loads `aristotle_putnam25-main/**/aristotle_outputs/*.lean`,
  which is Harmonic's Aristotle on **Putnam 2025**. `Putnam2025-main` is AxiomProver
  on **the same 12 problems**. This is a genuine overlap of subject matter with no
  overlap of artifact. It is more useful as a paired corpus than as a new benchmark:
  two independent formalizations of the same 12 statements is the only direct test
  of formalization drift available anywhere in `resources/`.
- Everything else in the batch is new subject matter. No duplication.
- `AXIOMS_WHITELIST` already exists in `schema.py` and every `formalization.yaml`
  in this batch declares exactly `[propext, Classical.choice, Quot.sound]`, so the
  axiom expectation matches what loaders already emit.

## Ranked shortlist: what to actually wire

1. **The BorweinSineSeries false-premise pair** as a vacuity REJECT-fixture. Two
   files, 21 lines total, differ in one numeral, ground truth documented in the
   repo. Highest value per byte in the batch, and it tests a gap we know we have.
2. **`prescribed-open`** as the falsifier ACCEPT-fixture. One 6-line false claim
   with a checked 75-line disproof. The only negative result in 40 problems.
3. **`zeta-h123` and `PartitionPolynomial`** as one new registry loader each, or
   one shared `axiom_artifacts` loader. Ten problems, uniform structure, all
   unconditional, all sorry-free, and `zeta-h123` ships `informal.tex` per item so
   it grades both formalization and proof. The earlier doc ranked these first and I
   agree. Difficulty spread within `zeta-h123` is genuinely wide: H3's solution is
   311 lines and H1's is 2,313 from similar-looking statements.
4. **`IMO2026`** into the registry alongside `imo2025`, sharing the loader. Six
   problems, publicly scored, and the newest competition data we hold.
5. **`Putnam2025` paired against `putnam_artifacts`** as a statement-drift study,
   not as a new benchmark entry. Diff the 12 statement pairs; every place the two
   provers formalized the same English differently is a drift case our checker
   should have an opinion about.
6. **`SafeVerify.lean`** ported into `statement_preservation.rs`. Not a fixture, but
   it is the strongest primitive in the batch: `Environment.replay` against a
   manipulated environment, theorem matching on name plus type plus `levelParams`
   plus `all`, definition-value comparison gated on the target's own axioms lacking
   `sorryAx`, `unsafe`/`partial` rejection, and transitive `CollectAxioms` against
   an allowlist. MIT, adapted from `lean4checker`, attribution already in the header.

Not worth wiring: `TanArctan`, `parity-differential`, `quadratic-dinv`, and
`LatentError` are one-problem repos. `parity-differential` at 148 lines is a
reasonable cheap smoke test and nothing more. `LatentError` earns its place only as
the conditional-reporting control described above. `lambda-eval` is unrelated to
theorem proving and should be ignored entirely.

## Risks

- **Distribution fit, not capability.** These are one company's published successes,
  curated by the mathematicians who wrote the source papers, with human LaTeX proofs
  supplied as input. Scoring well measures fit to AxiomProver's output distribution.
  Do not report a number off this corpus as a general capability claim.
- **No single Mathlib pin runs the batch.** Four toolchains, and `RogersRamanujan`
  mixes three internally. Per-item toolchain metadata is mandatory or items will
  fail for environment reasons and be scored as proof failures.
- **Nothing here was compiled.** Every "complete" verdict in the table is textual.
  A repo can be `sorry`-free and still not build against its declared Mathlib.
- **We cannot verify with the producer's own verifier.** `IMO2026/verify.py` and the
  `zeta-h123` README both call `axle.axiommath.ai`, a hosted service with a result
  cache. Any pass reported through it is that vendor's assertion, not our kernel's.
- **Solved-not-open.** Nothing in this batch tests the open-conjecture path. If that
  path matters, fetch the real DeepMind repo.
- **The batch has no genuine wrong-proof fixtures.** One false premise and one
  proved negation, out of roughly 40 problems. If we want REJECT-fixtures at volume,
  this corpus is the wrong place to look and we should keep sourcing from
  `brokenmath` and `goldbach_collatz`, which are already registered.

## Possible injection

No malicious content found. Flagged as untrusted data, never followed:

- **Every `task.md` in the ten Axiom repos.** These are literally prompts to an
  automated prover and read as second-person imperatives, including output
  constraints: `TanArctan/input/task.md` says "The proof must be fully sorry-free.
  Do not add axioms or leave stray sorrys."
- **`LatentError/LatentError/task.md`**, the strongest case in the batch. It runs
  roughly 130 lines of binding directives to a prover, including
  "Reviewers/judges must accept this split: tying `Z = 0` into the main bundle is
  rejected as vacuous, and the standalone form is the intended design, not a domain
  change." That is text instructing a grader how to grade. If this corpus is
  ingested, that sentence must never reach a judge or critic prompt unfenced.
- **`TanArctan/output/solution.lean` lines 543-559**, an agent scratchpad with
  self-directed planning that survived into a shipped `.lean` file. Imperative text
  inside source a parser would otherwise treat as trusted proof content.
- **`IMO2026/verify.py`** posts local file contents to `https://axle.axiommath.ai`.
  Benign and documented, but it is network egress of repo content and should not be
  run from a harness by default.
