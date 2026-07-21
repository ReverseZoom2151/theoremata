# `MaxwellEquations-main` -- 156 theorems, 12 of them empty, and a licence that lets us use them

Path: `resources/MaxwellEquations-main/MaxwellEquations-main/`. Read date 2026-07-21.

Layout: `proofs/*.lean` (6), `implementations/*.c` (6), `specifications/*.rkt` (6),
`screencaps/*.mov` (6), `README.md`, `LICENSE`, one PNG.

Read method: source first. Every number below is measured from the files, not taken
from the README, which is the artifact's own claim about itself and is treated as
evidence of what the author asserts rather than of what is true. All six Lean files
were compiled with the real toolchain (Lean 4.32.0-rc1 against the Mathlib master
checkout already vendored at `resources/mathlib4-master/`). Nothing in the repo was
executed: no `.c` was compiled or run, no `.rkt` was evaluated, no `.mov` was opened.

---

## 0. Safety and licence

### POSSIBLE INJECTION

**None found.** The repo ships no `AGENTS.md`, no `CLAUDE.md`, no `task.md`, no
`requirement.md`, and no `.py`/`.sh`/`.ps1` runner. The only prose file is
`README.md`, which is marketing and mathematics addressed to a human reader, with one
outbound link to `lanyon.ai`. A grep across the tree for `ignore previous`,
`system prompt`, `as an AI`, `new instructions`, `grader`, `award marks` returned
nothing. This repo is unusually clean on that axis relative to the rest of the batch.

It is still untrusted data and the registered fixture still fences its excerpt.

### Licence

`LICENSE`, 21 lines, verbatim MIT:

> MIT License
>
> Copyright (c) 2026 Lanyon AI Inc

This is the material difference from `AdvectionDiffusion-main`, which is the same
author's tooling, the same generator, the same theorem names, and **no licence at
all**. That repo forced a clean-room reproduction
(`docs/resource-mining/new/2026-07-latest-batch.md` §3.1, and the fixture pair under
`components/eval/fixtures/trivial_existential/`). This one may be vendored, excerpted
and used directly as a fixture, provided the notice above travels with any
substantial portion. The registered item therefore carries
`license: "MIT"` and `attribution: "Copyright (c) 2026 Lanyon AI Inc"` in its
provenance, and the test suite pins both.

### Toolchain

The Lean files declare only `import Mathlib` and carry no `lean-toolchain`,
`lakefile.lean` or `lakefile.toml`. There is no build system in the repo, so the
intended Lean and Mathlib versions are undocumented and unpinned. We supplied our own:
Lean 4.32.0-rc1 with the Mathlib master build already on disk. That worked, which is
mild evidence the files were generated against something close to current Mathlib.

---

## 1. The theorem census

This is the central question of the report, so it is answered by counting.

**156 theorems** across the six proof files (13 + 26 + 39 + 13 + 26 + 39), which
matches the README's claim of 156 exactly. They are not 156 distinct theorems. They
are **13 distinct statement shapes**, replicated once per spatial direction per file:
1 direction in each 1D file, 2 in each 2D, 3 in each 3D, so each shape appears
6 + 4 + 2 = 12 times, and 13 x 12 = 156.

| Shape (name modulo the `x`/`y`/`z` prefix) | Count | Verdict |
| --- | --- | --- |
| `Hyperbolicity` | 12 | **Trivially true** |
| `DiffusiveFluxConsistency` | 12 | True, but degenerate for this system (see below) |
| `WaveStability` | 12 | Substantive |
| `WaveConsistency` | 12 | Substantive |
| `WaveJumpCondition` | 12 | Substantive |
| `LeftFluctuationsConsistent` | 12 | Substantive |
| `RightFluctuationsConsistent` | 12 | Substantive |
| `FluxConservative` | 12 | Substantive |
| `LeftReconstructionConsistent` | 12 | Substantive |
| `RightReconstructionConsistent` | 12 | Substantive |
| `LeftReconstructionLinearityPreservation` | 12 | Substantive |
| `RightReconstructionLinearityPreservation` | 12 | Substantive |
| `ReconstructionSymmetric` | 12 | Substantive |

**Counts: 12 trivially true (7.7%), 144 statement-sensitive, of which 12 are
degenerate.**

Three supporting measurements, each of which pins the classification independently:

- The character `∃` occurs exactly 84 times in the six files: 6 in `maxwell_1d`, 12
  in `maxwell_2d`, 18 in `maxwell_3d`, 8 in `hyperbolic_maxwell_1d`, 16 in
  `hyperbolic_maxwell_2d`, 24 in `hyperbolic_maxwell_3d`. **Every one of them is a
  conjunct of a `Hyperbolicity` statement.** No other theorem in the corpus states an
  existential.
- The tactic `rfl` occurs exactly 84 times and `refine` exactly 84 times, corpus-wide.
  Same number, same places: the entire use of reflexivity in this development is the
  discharge of those 84 existential conjuncts. Every other proof is
  `simp` (1,680), `constructor` (1,020), `field_simp` (588), `norm_num` (84),
  `linarith` (84).
- Zero `sorry`, zero `admit`, zero `native_decide`, zero `axiom` declarations, in all
  six files. This corpus is not defective in any way a crude scan detects.

### 1.1 The trivial shape

`proofs/maxwell_1d.lean:215`:

```lean
theorem xHyperbolicity (C : Coordinates) (P : Parameters) (U : State) :
    (∃ r1 : Real, r1 = (xFluxJacobianEigenExprs C P U).lambda1) ∧
    (∃ r2 : Real, r2 = (xFluxJacobianEigenExprs C P U).lambda2) ∧
    (∃ r3 : Real, r3 = (xFluxJacobianEigenExprs C P U).lambda3) ∧
    (∃ r4 : Real, r4 = (xFluxJacobianEigenExprs C P U).lambda4) ∧
    (∃ r5 : Real, r5 = (xFluxJacobianEigenExprs C P U).lambda5) ∧
    (∃ r6 : Real, r6 = (xFluxJacobianEigenExprs C P U).lambda6) := by
  constructor
  . refine ⟨(xFluxJacobianEigenExprs C P U).lambda1, rfl⟩
  ...
```

Hyperbolicity of a first-order system means the flux Jacobian has real eigenvalues and
a complete set of eigenvectors. What is stated is that, for each of six expressions of
type `Real`, some real number equals it. That is `rfl` for any expression whatsoever.
The eigenvector condition is absent entirely; there is no eigenvector anywhere in the
corpus. The word hyperbolicity appears only in the identifier.

The 1D perfectly-hyperbolic variant is the same statement with eight conjuncts instead
of six, for the two extra auxiliary fields.

### 1.2 The degenerate shape, recorded honestly

`DiffusiveFluxConsistency` says that when the spatial gradient is set to zero the
diffusive flux is zero. That is true. It is also uninformative for this system,
because `xDiffusiveFluxExprs` returns the constant record

```lean
    diffusive_flux_Ex := 0.0
    diffusive_flux_Ey := 0.0
    ...
```

for **every** input, gradient or not. The Racket spec says the same thing:
`'diffusive-fluxes (list (list 0.0 0.0 0.0 0.0 0.0 0.0))`. So the theorem holds
because the function is identically zero, not because a vanishing gradient causes a
vanishing flux.

I classify this as **substantive but degenerate** rather than trivial, and the
distinction is not cosmetic: replacing the definition with an unrelated non-zero
constant *does* break the proof, whereas doing the same to `Hyperbolicity` does not.
It is statement-sensitive. It just has nothing to be sensitive about in this
particular system. The reject reason `name_claims_more_than_statement` does not fit
it, and it is not registered.

### 1.3 One further drift, unregistered

`Coordinates` (the spatial position) is bound in every theorem and consumed by none.
Each definition opens `let x := C.x` and never uses `x`. This is the same shape as
`AdvectionDiffusion`, and it is why the files set
`set_option linter.unusedVariables false` at line 3. Not registered, because it is
noise from a generator rather than a claim about mathematics.

The Racket spec declares a `parameters-assumptions` entry holding the quoted form
`(> c 0.0)`, that is, the speed of light is positive. The C
implementation encodes it, as `maxwell_1d_parameters_valid` returning `(c > 0.0)`. The
**Lean does not**: `structure Parameters where c : Real`, with no positivity
hypothesis on any theorem, and no `c > 0` or `0 < c` anywhere in any of the six files.
The nearest thing is a pair of `abs c ≠ 0` hypotheses on `FluxConservative`, which is
weaker and derived rather than stated. This is an `unencoded_side_condition` in the
existing vocabulary. It is not registered here because one corpus should back one
reason cleanly; recorded so it can be picked up if we want a second item.

---

## 2. Compilation: it really does compile

All six files compiled. Real output, from
`lean` 4.32.0-rc1 with `LEAN_PATH` pointed at the vendored Mathlib master build, run
on the unmodified vendored sources with `#print axioms` appended:

```
=== ax_maxwell_1d ===
'maxwell_1d.xHyperbolicity' depends on axioms: [propext, Classical.choice, Quot.sound]
'maxwell_1d.xWaveStability' depends on axioms: [propext, Classical.choice, Quot.sound]
exit=0

=== ax_hyperbolic_maxwell_3d ===
'hyperbolic_maxwell_3d.xHyperbolicity' depends on axioms: [propext, Classical.choice, Quot.sound]
'hyperbolic_maxwell_3d.xWaveStability' depends on axioms: [propext, Classical.choice, Quot.sound]
'hyperbolic_maxwell_3d.yHyperbolicity' depends on axioms: [propext, Classical.choice, Quot.sound]
'hyperbolic_maxwell_3d.yWaveStability' depends on axioms: [propext, Classical.choice, Quot.sound]
'hyperbolic_maxwell_3d.zHyperbolicity' depends on axioms: [propext, Classical.choice, Quot.sound]
'hyperbolic_maxwell_3d.zWaveStability' depends on axioms: [propext, Classical.choice, Quot.sound]
exit=0
```

The three standard Lean axioms and nothing else. No `sorryAx`, no author-declared
axiom. **This corpus passes every gate we ship.** That is the entire point of
registering it.

Practical note for anyone repeating this: each invocation costs about two minutes,
almost all of it loading the Mathlib oleans, and the elaboration itself is fast.

---

## 3. The constant-substitution experiment

The claim "this statement cannot distinguish the real definition from a wrong one" is
testable, so it was tested rather than asserted. Two files were built from the first
251 lines of `proofs/maxwell_1d.lean` (through the end of the `xWaveStability` proof),
differing only in the body of `xFluxJacobianEigenExprs`:

- `exp_orig.lean`: unmodified, `lambda1 := -(c)`, `lambda3 := c`, `lambda5 := 0.0`, etc.
- `exp_subst.lean`: `lambda1 := 42.0`, `lambda2 := -17.0`, `lambda3 := 3.14159`,
  `lambda4 := 1000000.0`, `lambda5 := -2.5`, `lambda6 := 7.0`. Unrelated constants with
  no connection to the speed of light or to any Maxwell system.

**Not one character of either proof script was changed.** Real output:

```
=== exp_orig ===
'maxwell_1d.xHyperbolicity' depends on axioms: [propext, Classical.choice, Quot.sound]
'maxwell_1d.xWaveStability' depends on axioms: [propext, Classical.choice, Quot.sound]
exit=0
```

```
=== exp_subst ===
exp_subst.lean:242:2: error: unsolved goals
case left
C : Coordinates
P : Parameters
U : State
⊢ |42.0| ≤ |P.c|
exp_subst.lean:244:2: error: unsolved goals
case right.left
⊢ |17.0| ≤ |P.c|
exp_subst.lean:246:2: error: unsolved goals
case right.right.left
⊢ |3.14159| ≤ |P.c|
exp_subst.lean:248:2: error: unsolved goals
case right.right.right.left
⊢ |1000000.0| ≤ |P.c|
exp_subst.lean:250:2: error: unsolved goals
case right.right.right.right.left
⊢ |2.5| ≤ |0.0|
exp_subst.lean:251:2: error: unsolved goals
case right.right.right.right.right
⊢ |7.0| ≤ |0.0|
'maxwell_1d.xHyperbolicity' depends on axioms: [propext, Classical.choice, Quot.sound]
'maxwell_1d.xWaveStability' depends on axioms: [propext, sorryAx, Classical.choice, Quot.sound]
exit=1
```

Read the two `#print axioms` lines at the bottom of the failing run, because they are
the finding:

- `xHyperbolicity` **still proves, unchanged, with the same clean axiom set**, over
  eigenvalues that are now arbitrary garbage. Every error in the run belongs to its
  neighbour.
- `xWaveStability`, which is substantive, **breaks in six places** and picks up
  `sorryAx` from the failed elaboration.

Same file, same proof style, same generator, adjacent lines. One statement detects
that the definition is wrong and the other cannot. This disproves nothing about the
mathematics and everything about what `xHyperbolicity` is checking, which is nothing.
It also supplies the specification of the check we do not have: substitute unrelated
constants for the definitions a theorem names, and see whether the proof still closes.

---

## 4. Is there any mechanical link between `.rkt`, `.c` and `.lean`?

**Verdict: they are three independent emissions from one Racket specification. There
is no mechanical link between the Lean proof and the C implementation in either
direction, and the README's "end-to-end formally verified solvers" and "8,516 lines of
formally verified C code" are not supported by anything in the repo.**

What is genuinely there, and it is real:

The `.rkt` is a data structure, not a program. `specifications/maxwell_1d.rkt` is a
`hash` with the keys `name`, `coordinates`, `state`, `state-assumptions`, `parameters`,
`parameters-assumptions`, `fluxes`, `wavespeeds`, `diffusive-fluxes`, plus per-scheme
hashes carrying `waves`, `speeds`, `order`, `left-reconstruction`,
`right-reconstruction`. Those keys map one-for-one onto both the Lean definitions
(`xFluxExprs`, `xWaveSpeedExprs`, `xDiffusiveFluxExprs`, `xWaveFamilyExprs`,
`xSpeedFamilyExprs`, `xLeftReconstructionExprs`, `xRightReconstructionExprs`) and the
C functions (`_x_flux`, `_x_wavespeed`, `_x_diffusive_flux`, `_x_wave_family`,
`_x_speed_family`, `_x_left_reconstruction`, `_x_right_reconstruction`).

The expression trees agree textually, which is strong evidence of a shared
pretty-printer. Racket:

```racket
   'fluxes (list
            (list 0.0
                  `(* (* c c) Bz)
                  `(- (* (* c c) By))
                  0.0
                  `(- Ez)
                  `Ey))
```

Lean:

```lean
    flux_Ex := 0.0
    flux_Ey := ((c * c) * Bz)
    flux_Ez := -(((c * c) * By))
    flux_Bx := 0.0
    flux_By := -(Ez)
    flux_Bz := Ey
```

C:

```c
  flux->flux_Ex = 0.0;
  flux->flux_Ey = ((c * c) * Bz);
  flux->flux_Ez = -(((c * c) * By));
  flux->flux_Bx = 0.0;
  flux->flux_By = -(Ez);
  flux->flux_Bz = Ey;
```

Identical parenthesisation, identical ordering, identical `let`/`double` prologue
binding every state component whether used or not. Same source, three backends.

What is **not** there:

- **No reference in either direction.** The six `.lean` files contain zero
  occurrences of `.c`, `.rkt`, `racket`, or any filename. They contain **zero comments
  of any kind** (0 occurrences of `--` or `/-`). The `.c` files reference no Lean. The
  `.rkt` files reference neither.
- **No extraction and no refinement relation.** Nothing generates the C from the Lean,
  nothing relates a C function to a Lean definition, and there is no compiler-correctness
  or refinement argument anywhere.
- **A type gap that no artifact bridges.** The Lean is about `Real`; the C is about
  IEEE-754 `double`. Nothing in the repo states a floating-point error bound, a
  rounding model, or any relation between the two. A theorem about `Real` transfers to
  `double` only through an argument nobody made here.
- **The C "proofs" are runtime float comparisons.** The C analogue of the Lean
  `xWaveConsistency` theorem is `maxwell_1d_lax_friedrichs_x_waves_consistent`, whose
  body is `return ((fabs(Ex_wave1) < 1.0e-8) && ...)`. That is a tolerance test at
  runtime, not a proof, and it is a different proposition from the Lean one.
- **`main` is empty in all six implementations.** `int main(...) { // Insert
  simulation drivers here. return 0; }`, verbatim, six times. The "8,516 lines of
  formally verified C code" is a header-style library of `static inline` functions with
  no driver, no time-stepping loop, no I/O and no solver. The screencaps show a
  simulator; the `.c` files in this repo do not contain one.
- **`state_valid` returns `true` unconditionally** in all six files (matching the
  spec's empty `'state-assumptions`), and the parameter assumption `c > 0` that the C
  does encode is, as noted in §1.3, absent from the Lean.

So: shared provenance, yes, and it is a legitimately impressive generator. Verified
C, no. The honest description is "a symbolic PDE-scheme specification, from which a
Lean file of algebraic identities and an unrelated C library were both printed".

---

## 5. What was registered

Corpus `maxwell_equations`, one item, in
`components/eval/python/theoremata_tools/benchmarks/adversarial.py`.

| Field | Value |
| --- | --- |
| id | `maxwell_equations:hyperbolicity` |
| verdict | `expect_reject` |
| reason | `name_claims_more_than_statement` |
| path | `proofs/maxwell_1d.lean` |
| licence / attribution | MIT / `Copyright (c) 2026 Lanyon AI Inc` |
| untrusted | `True` |
| expected_to_fail_today | `True` |

Why it earns its place next to the clean-room pair we already have: that pair is our
own restatement of a pattern we described, so passing against it only shows we catch
what we ourselves wrote. This is a genuine published artifact we did not shape,
generated by a third party's tool, and it compiles clean against real Mathlib. A
triviality check that catches the clean-room probe but not this one has not caught the
phenomenon.

Registration details worth knowing:

- The loader globs `MaxwellEquations-main/**/proofs/maxwell_1d.lean` only. `README.md`
  is never ingested; its claims are replaced by the measurements above. A test pins
  that the surfaced path ends in `.lean`.
- It returns `[]` when the corpus is absent, like every other `resources/`-backed
  loader, because `resources/` is gitignored and missing in CI. It deliberately does
  **not** follow `load_trivial_existential`, which raises, because that one is
  committed in-tree where absence means a broken checkout.
- `make_adversarial_item` gained an `excerpt_limit` parameter, defaulting to the
  previous 4,000 characters. `theorem xHyperbolicity` begins 4,116 bytes into the file,
  so at the old budget the excerpt stopped 116 bytes short of the very theorem the item
  is about. This item passes 6,000.
- **No control half.** Every proof file in this corpus contains its own
  `Hyperbolicity` theorem, so no file here is a clean accept, and pairing would mean
  asserting an accept on a file that carries the probe. The clean-room pair supplies the
  control side of the contrast.

### Registration cost outside my ownership

`components/eval/tests/test_benchmarks.py` enumerates the registry, so adding a corpus
requires two one-line changes there. They are reported, not made.

---

## 6. Cross-repo: the pattern is systemic

`xHyperbolicity`, stated as an existential equation and proved by an anonymous
constructor, now appears in two separate repositories from the same author:

| Repo | Files | Trivial theorems | Licence |
| --- | --- | --- | --- |
| `AdvectionDiffusion-main` | 9 `.lean` | 18 | **None** |
| `MaxwellEquations-main` | 6 `.lean` | 12 | MIT |

Thirty instances, two systems, one generator. This is not a slip in one file; it is
what the generator emits whenever it is asked for hyperbolicity, and any future repo
from the same tool will carry it. That has a direct consequence for us: the check
described in §3 would pay for itself across a whole family of artifacts rather than a
single fixture, and it is worth building for that reason and not only for this one.

## 7. Risks and honest caveats

- The reject verdict is **our policy**, not a claim that the author lied. They wrote
  down a property, named it, and proved it. The defect is that the name promises more
  than the proposition delivers, which is a real failure of a formalization to carry
  meaning, but the mathematics stated is true.
- The 144 statement-sensitive theorems are real work and should not be dismissed. The
  scheme-consistency, jump-condition and linearity-preservation results genuinely
  constrain the definitions, and the substitution experiment confirms it for the one
  case tested.
- Only `maxwell_1d.lean` was subjected to the substitution experiment. The other five
  files are the same generator emitting the same shape, and all six compile with clean
  axioms, but the substitution was run on one of them.
- The excerpt of a vendored MIT file now ships in an item that can reach a prompt. It
  is fenced, and the attribution travels with it, which is what MIT requires.
