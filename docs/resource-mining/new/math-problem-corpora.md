# Math problem corpora — mining pass (14 vendored repos)

Scope: 14 repos under `resources/` that look like individual mathematics problems or
proof attempts rather than tools. The mining question is not "what mechanism can we
port" but "which of these can become an evaluation fixture our gate must ACCEPT or
must REJECT".

Read date: 2026-07. All paths below are relative to
`C:\Users\adria\Downloads\math-agent\resources\`. Every repo is nested one level
(`X-main/X-main/`).

---

## 0. Safety and licensing line

**POSSIBLE INJECTION: none found.** A full grep across `*.md`, `*.tex`, `*.lean`,
`*.bend` in all 14 repos for agent-directed imperative text
(`you are/must/should`, `as an AI`, `ignore previous`, `system prompt`,
`new instructions`) produced exactly one hit:

- `HigherDyson-main/HigherDyson-main/Batch3/Input/task_batch3.md:45` —
  "lemmas you must DERIVE, never assume."

This is the repo author instructing their own formalizer about the task, not text
addressed at a downstream reader. It is benign, but recorded here because it is
imperative text inside untrusted vendored data and would be picked up verbatim if we
ever ingest `task.md` files as prompt context. Everything under `resources/` stays
untrusted regardless.

**Licences.**

| Repo | Licence | Constraint |
| --- | --- | --- |
| Granville, PartitionElliptic, ramanujan-tau-misses-primes, andrews_dhar_problem, fel-polynomial, dead-ends, challenge_3, HigherDyson, Biswal, erdos-public | MIT, "Copyright (c) 2026 Axiom Math." | Clean. Reusable with attribution, including verbatim Lean. |
| Super_MARIO | MIT, Copyright (c) 2024 MARIO-Math-Reasoning | Clean. |
| kepler98 | **No licence file** | All rights reserved. Clean-room only, ideas not code. Already recorded in `new/flyspeck-kepler-repos.md`. |
| WanShi | **No licence file** | All rights reserved. Clean-room only. |
| goldbach-collatz-proof | CC BY 4.0 (claimed via README badge; no LICENSE file present) | Attribution required. Already registered. |

No GPL or AGPL anywhere in this batch, so no copyleft contamination risk. The ten
Axiom Math repos are a single MIT-licensed family and can be vendored as fixtures
directly.

**Already covered elsewhere in `docs/resource-mining/`, do not re-mine:**

- `kepler98-master` — covered by `new/flyspeck-kepler-repos.md` (characterized, no
  licence, clean-room only).
- `Super_MARIO-main` — covered by `new/super-mario.md` (AlphaMath MCTS harness; it is
  a *tool*, not a problem corpus, and does not belong in this catalogue).
- `goldbach-collatz-proof-main` — covered by
  `AutoMathText-and-goldbach-collatz.md`, and already wired into the registry as
  `goldbach_collatz` (track `falsification`), loader at `loaders.py:965`.
- Erdős material — `Erdos1196.md` covers `Erdos1196-main` (a different repo:
  a blueprint-scale single-problem development). `erdos-public-main` is **not**
  covered anywhere and is new material.

---

## 1. The dominant pattern

Ten of the fourteen repos are the same artifact type: **Axiom Math / AxiomProver
release repos**. Each has an identical skeleton:

```
input/<run>/task.md          natural-language task given to the prover
input/<run>/requirement.md   formalization constraints (optional)
input/<run>/*.tex            the source paper or informal proof
<Name>/<run>/problem.lean    the formal statement, every proof body = `sorry`
<Name>/<run>/solution.lean   the same statement with proofs filled in
lakefile.toml, lean-toolchain, lake-manifest.json, LICENSE, logo.svg
```

This is exactly the shape of a formalization benchmark item: `task.md` + `*.tex` is
the informal input, `problem.lean` is the ground-truth statement scaffold (with
`sorry` holes), `solution.lean` is the reference proof. **The `sorry`s in
`problem.lean` are by design** and are not a soundness signal; they are the holes the
prover is meant to fill.

**Verified sorry-freeness of the solutions.** Grepping every `solution.lean` in all
ten repos for `sorry`, `admit`, `axiom`, and `native_decide`:

- **No live `sorry` in any `solution.lean`.** The three grep hits are all inside
  comments: `Granville/wronskian/solution.lean:423` (a section header reading
  "new sorry"), `HigherDyson/Batch2/Output/solution.lean:513` (a design note), and
  `andrews_dhar/thm2_split4/solution.lean:3104` (see §3).
- **No `axiom` declarations anywhere.**
- **No `native_decide` anywhere.** Where decision procedures are used it is kernel
  `decide`, which is the safe form.
- Exactly one `#print axioms` in the whole batch:
  `andrews_dhar/thm2_split4/solution.lean:16199`, on the final theorem.

So on the crude gate signals, this family is clean. The interesting failure modes are
all one level up: hypothesis-conditional statements, opaque abstractions, and
statement drift. Those are covered in §3.

The Lean toolchains are 4.26.0 and 4.28.0, both recent. We cannot build any of these
here (no Lean on this box), so "complete and checkable" below means "structurally
complete and plausibly checkable", not "we ran it".

---

## 2. Corpus catalogue

Sizes are total lines of `solution.lean` across the repo.

| Repo | Content | Formal system | Complete + checkable | Size | Proposed use |
| --- | --- | --- | --- | --- | --- |
| **erdos-public** | 7 Erdős-problem formalizations, 3 of them **disproofs with explicit counterexamples** (231, 328, 441), plus 276, 403, 209, 1134 | Lean 4.26/4.28 + Mathlib | Yes, solutions sorry-free | 3,255 lines over 7 independent problems (79 to 1,203 each) | **ACCEPT-fixture (top pick)** |
| **ramanujan-tau-misses-primes** | ABC implies τ misses almost all primes; conditional on ABC + a strengthened Xiong Prop 5.4; τ **axiomatized as a structure** | Lean 4.26 + Mathlib | Yes, sorry-free, 497 lemmas | 5,601 lines, 1 theorem chain | **ACCEPT-fixture + vacuity probe** (see §4) |
| **PartitionElliptic** | Two "Key Formulas" from Ono's partition/elliptic-curves paper, reduced to complex-field algebra over free variables | Lean 4.28 + Mathlib | Yes, sorry-free | 87 lines | **REJECT-fixture (statement drift)** |
| **HigherDyson** | Elliptic corrections for higher Dyson ranks; 4 batches; kernels defined as `tsum`, normalizer `Pd` left **opaque**, summability taken as unproved hypotheses | Lean 4.28 + Mathlib | Yes, sorry-free | 3,916 lines over 4 batches | **REJECT-fixture (unwitnessed hypotheses / vacuity)** |
| **Granville** | 3 runs: local-global section 2, sharpened n=3 deficit inequality, Wronskian degree bounds. The n=3 run **takes two of Granville's results as hypotheses** and says so | Lean 4.26 + Mathlib | Yes, sorry-free | 5,843 lines over 3 runs | ACCEPT-fixture (wronskian run only) + hypothesis-provenance probe |
| **andrews_dhar_problem** | Andrews–Dhar D_3 partition equidistribution, 5 runs; thm2_split4 is 16,199 lines and contains a comment stating a lemma it once relied on **is FALSE** | Lean 4.28 + Mathlib | Yes for thm1 and split1–3; split4 needs a real build to trust | 21,739 lines total; split4 alone is 16,199 | thm1 = ACCEPT-fixture; **split4 = blueprint-scale target + provenance case study** |
| **Biswal** | Chebyshev quotients / Demazure multiplicities / Dyck paths, 2 runs. theorem2and3 is fed theorem1's solution verbatim as an input file | Lean 4.28 + Mathlib | Yes, sorry-free | 3,368 lines | ACCEPT-fixture (theorem1) + **lemma-reuse / library-growth fixture** |
| **fel-polynomial** | Fel's conjecture on syzygies of numerical semigroups; genuine research statement; ships a `.sage` file used to check the paper's worked examples | Lean 4.26 + Mathlib | Yes, sorry-free | 2,544 lines | ACCEPT-fixture (blueprint-scale) |
| **dead-ends** | Density of base-b dead ends in square-free digit walks. README notes the result **was already proved by Mirsky in 1947** | Lean 4.26 + Mathlib | Yes, sorry-free | 4,124 lines | ACCEPT-fixture + **novelty/dedup fixture** |
| **challenge_3** | Two lemmas about a divisor-sum quantity q_n and colored marked rectangles; the statement explicitly **substitutes a closed form for the generating-function origin and declines to prove the equivalence** | Lean 4.28 + Mathlib | Yes, sorry-free | 1,685 lines | REJECT-fixture (mild statement drift) |
| **WanShi** | A standard library for **Bend2** (HigherOrderCO): algebra structures, Bool/Nat/List, `Proof/` namespace with And/Or/Exists/Iff. Propositions are `Set`-valued definitions | **Bend2, not Lean** | Definitions and small lemmas only; not a theorem corpus | 246 `.bend` files, mostly a few lines each | **Non-Lean formal-system probe** (small); otherwise SKIP |
| **kepler98** | Hales/Ferguson 1998 informal Kepler proof: C++, Java, Mathematica, CPLEX logs | None (informal + numeric) | No, and no licence | 2,040 files | SKIP (already mined in `new/flyspeck-kepler-repos.md`) |
| **goldbach-collatz-proof** | Crank claim of constructive proofs of Goldbach and Collatz. `main.tex` is 20 lines; the `sections/*.tex` the README links to **do not exist in the repo** | None (LaTeX prose) | No, structurally incomplete | 4 files | SKIP — **already registered** as `goldbach_collatz` |
| **Super_MARIO** | AlphaMath MCTS training harness (a tool, not a problem corpus) | n/a | n/a | 53 files | SKIP — already mined in `new/super-mario.md` |

---

## 3. The genuinely interesting findings

### 3.1 erdos-public contains three formal disproofs (rare and valuable)

Erdős 231, 328 and 441 are all formalized as **refutations**, not proofs. This is a
proof shape our gate sees almost never, and there are three of them in one MIT repo.

`Erdos231/solution.lean` (79 lines) is the cleanest specimen in the whole batch. It:

1. defines `IsAbelianSquare` as a `Prop` and `isAbelianSquare` as a `Bool`;
2. proves the two are equivalent (`isAbelianSquare_iff`, `containsAbelianSquare_iff`)
   — the **Bool/Prop bridge lemmas are proved, not assumed**;
3. discharges the concrete counterexample by `decide` through that bridge:

```lean
theorem erdos_problem_231_k4 :
    let S : List (Fin 4) := [0, 1, 0, 2, 0, 1, 0, 3, 0, 1, 0, 2, 1, 0, 1]
    S.length = 2 ^ 4 - 1 ∧ IsAbelianSquareFree S :=
  ⟨rfl, fun hcontra => absurd ((containsAbelianSquare_iff _).mpr hcontra) (by decide)⟩
```

This is precisely the pattern our gate should demand of any decision-procedure-backed
claim, and it uses kernel `decide` rather than `native_decide`. `Erdos441`
(124 lines) is the same shape at N = 336 with an explicit 21-element witness set.
Both are small enough to be single fixture files.

### 3.2 ramanujan-tau: the prior assessment holds, but for a different reason

See §4 — it warrants its own section.

### 3.3 PartitionElliptic is a textbook statement-drift fixture

The README sells this as formalizing two Key Formulas from Ono's *The partition
function and elliptic curves*, involving E_2, its nonholomorphic completion, weight-k
operators, weak Maass forms, and the CM tangent to the modular polynomial. The Lean
contains none of that. `output/problem.lean` opens:

```lean
variable (y : ℝ) (hy : 0 < y)
variable (E2 E4 E6 F DF J : ℂ)
variable (ΦY ΦXX ΦXY ΦYY : ℂ)
```

Every modular object is a **free complex variable**. Three separate drifts:

- The prose says the ground data is "subject to the constraints `E_4 != 0`,
  `J != 1728`, and `Phi_Y != 0`". **None of `E4 ≠ 0` or `J ≠ 1728` is encoded as a
  hypothesis anywhere.** `E4`, `E6` and `J` are declared and then never used.
- `hy : 0 < y` is declared and, for `key_formula_one`, never consumed.
- `key_formula_two` reduces to substituting `ΦXX = ΦYY` into
  `((1/2)ΦXX - ΦXY + (1/2)ΦYY)/ΦY` and comparing against
  `tauCM := (ΦYY - ΦXY)/ΦY`. It is true by `ring` after the rewrite.

The README even concedes the point ("follow by elementary field arithmetic from the
definitions") while framing it as a virtue. The result is an 87-line "solution" whose
mathematical content is nil, attached to a claim about modular forms. If our gate
accepts this as "formalized Ono's Key Formulas", it has a drift hole. This is the
single best small drift fixture in the batch.

### 3.4 HigherDyson Batch4: six unwitnessed `Summable` hypotheses

`Batch4/Output/problem.lean` defines the correction kernels as honest `tsum`s, then
takes summability as hypotheses:

```lean
(hSummB : ∀ w, Summable (fun n : ℕ => ...))
(hSummBshift : ∀ w, Summable (fun n : ℕ => ...))
(hSummAr : ∀ (r : ℕ) (w : ℂ), Summable (fun n : ℕ => ...))
(hSummArshift : ...) (hSummAext : ...) (hSummAextshift : ...)
```

plus an **opaque** normalizing product `Pd : ℂˣ → ℂ` of which the problem header says
"only its unit status is used", and a root of unity `ω : ℂˣ` of which "only its unit
status is used" (i.e. the primitivity that the mathematics depends on is dropped).

Nothing anywhere constructs a tuple `(m, x, elv, q, ω, Pd)` satisfying all six
summability hypotheses. If no such tuple exists, every theorem in Batch4 is vacuously
true and the 448-line solution proves nothing. Batches 2 and 3 have **zero**
`Summable` occurrences at all while still manipulating the same objects. This is a
strong vacuity-probe fixture: our gate should either demand an instantiation witness
or mark the result conditional.

### 3.5 andrews_dhar thm2_split4: an AI proof narrating a false lemma

`Bijection/thm2_split4/solution.lean:3096` contains this comment inside a 16,199-line
generated proof:

```
/- NOTE: processInsertionsLabeled_hard_not_FR is FALSE.
   ...
   The lemma claims hard elements are never FR, which is FALSE.
   The scan proof (s2_labeled_scan_records_perm_easyLabels) should use
   FrontierFRImpliesEasy instead of global I3 that depends on this false lemma.
   For now, we sorry the scan proof's I3 establishment (hI3_initial hard case). -/
```

**Be precise about what this is and is not.** The named lemma
`processInsertionsLabeled_hard_not_FR` appears **nowhere else in the repo** — the sole
occurrence is this comment. There is no live `sorry` in the file; the "For now, we
sorry" sentence is stale text describing an abandoned intermediate state. The file
ends with `#print axioms phi3_bijOn`, which suggests the author did check axiom
provenance on the final result.

So this is not a false proof. It is something almost as useful: **a residual audit
trail showing a generated proof passed through a stage where it depended on a lemma
the generator itself later determined to be false**, and the only surviving evidence
is a prose comment that no gate would read. Two lessons:

1. Comments are not gates. If the false dependency had survived, nothing in our
   pipeline would notice.
2. This file is the best available specimen for testing whether our provenance and
   axiom-audit machinery can independently confirm what the comment asserts, and for
   testing our critique loop's ability to flag "the artifact admits a problem in
   natural language."

At 16,199 lines it is not a unit fixture. It is a blueprint-scale target.

### 3.6 dead-ends is a free novelty-detection fixture

The README states that after arXiv posting the authors learned from Soundararajan
that "the result in this paper was previously obtained by Mirsky in 1947." A
formalized, sorry-free, MIT-licensed 4,124-line development of a result that is
**correct but 79 years old and already known**. If we ever build a novelty or
subsumption check (the LEGO-Prover / dedup direction in the paper-mining notes), this
is a labelled positive with the ground truth written in the README.

### 3.7 Granville and challenge_3: documented hypothesis and drift admissions

- `Granville/sharpening_n3/problem.lean` header: "These are taken as hypotheses
  (axioms of the structure) and not proved here." An honest conditional result. Good
  probe for whether our gate records the hypothesis set as part of the claim rather
  than reporting an unconditional theorem.
- `challenge_3/lemma-b2/problem.lean` header: `q_n` originates as
  `-[t^n] P(t) S(t)` but "We adopt this closed form as the definition ... The two
  formulas are equal as integers; this equivalence is not a lemma to be proven here."
  That is an explicit, self-declared drift from the source text. Mild, and disclosed,
  but exactly the kind of substitution a drift detector must surface.

### 3.8 WanShi is the only non-Lean formal system here

246 `.bend` files for **Bend2**. Propositions are encoded as `Set`-valued
definitions returning proof combinators:

```
def group<A>(f:A->A->A, u:A) -> Set:
  is_ass = Algebra/associative<A>(f)
  is_uni = Algebra/unit<A>(f, u)
  is_div = Algebra/divisible<A>(f)
  Proof/And(is_ass, Proof/And(is_uni, is_div))
```

It is a standard library of definitions and small structural lemmas, not a theorem
corpus, and it has no licence. Its only value to us is as a cheap negative probe for
the `FormalSystem` abstraction: a body of formal-looking mathematics in a system we
do not support, that our backend dispatcher must route or refuse cleanly rather than
misidentify as Lean. Low priority, and blocked on licensing for anything beyond
"point our detector at the file extensions."

---

## 4. The ramanujan-tau verdict

**Prior note:** "ready regression fixture for a soundness gap." **Verdict: the
assessment holds, and the repo is even better than that framing suggests, but the
specific gap is not the one the phrase implies. There is nothing wrong with this
proof.**

What the repo actually contains:

- `input/task.md` and `input/requirement.md` explicitly instruct the prover **not** to
  prove ABC or Xiong's Proposition 5.4, but to assume them:
  "the task is **neither** proving ABC conjecture nor proposition 5.4 ... but prove a
  theorem **assuming** them."
- `requirement.md` also instructs: "instead of defining τ via coefficients of power
  series, we will define τ by specifying a list of properties it should satisfy."
- Accordingly, `problem.lean` declares

  ```lean
  structure RamanujanTau where
    τ : ℕ+ → ℤ
    hecke_mult   : ...
    hecke_rec    : ...
    parity       : ...
    deligne_bound: ...
    non_unit     : ...

  variable (R : RamanujanTau)
  ```

  and the main theorem is
  `theorem main_theorem (habc : ABC) (h54 : Proposition5_4 R) : ... S R X ≤ C * X^(13/22)`.
- `solution.lean` is 5,601 lines, 497 lemmas, **sorry-free, axiom-free,
  `native_decide`-free**. It is, as far as static inspection can tell, a real and
  careful proof.

The soundness exposure is **triple-conditional vacuity**, and it is disclosed rather
than hidden:

1. `RamanujanTau` is a structure with five simultaneous constraints and **no instance,
   `Nonempty` witness, or `example` is provided anywhere in the repo**. Every theorem
   is universally quantified over `R : RamanujanTau`. If the structure is uninhabited,
   the entire 5,601-line development is vacuous.
2. `ABC` is a hypothesis (open conjecture) — fine and standard, but the *stated*
   `ABC : Prop` must actually be the ABC conjecture and not a weaker or malformed
   variant.
3. `Proposition5_4` is a **deliberately strengthened** form of a published result
   (`N^{1/2}` in place of the published `N^{9/10}`), assumed as a hypothesis, and its
   second conjunct asserts `X2k R k ∩ [-N,N] = ∅` for large `k`. Combined with an
   axiomatized `τ`, an inconsistency between hypothesis 1 and hypothesis 3 would make
   `main_theorem` vacuously provable and no gate signal would fire.

That is why this is such a good fixture. It is a **hard positive**: a sound, expert,
sorry-free proof that any naive vacuity or hypothesis-provenance gate will either
wave through with an unconditional verdict (wrong — the result is conditional on ABC
and on a strengthened unpublished bound) or reject outright (also wrong — the proof
is correct). The correct gate behaviour is to accept and to **carry the hypothesis
set and the missing-instance flag into the certificate**. Nothing else in this batch
tests that.

The prior note's framing of it as a "soundness gap" fixture is therefore right in
substance. The gap is: our pipeline does not currently distinguish "proved" from
"proved conditional on ABC, a strengthened Xiong bound, and an unwitnessed
axiomatization of τ."

---

## 5. Ranked shortlist: what to actually wire into the registry

Current state check. `components/eval/python/theoremata_tools/benchmarks/registry.py`
registers 33 corpora via a `_TRACK_KIND` dict, with per-corpus loaders in
`loaders.py` keyed by the same name. Relevant existing entries:
`goldbach_collatz` (`falsification`), `brokenmath` (`falsification`), `erdos1196`
(`formalization`), plus 30 more.

**No repo in this batch duplicates a registered benchmark**, with one exception:
`goldbach-collatz-proof-main` **is** the already-registered `goldbach_collatz` corpus
(loader at `loaders.py:965`, emits a single negative fixture `goldbach_collatz:crank`).
Do not re-add it. `erdos-public` does **not** overlap `erdos1196`: different repo,
different problems (209/231/276/328/403/441/1134 versus 1196), different shape
(single-file solutions versus a leanblueprint development).

The gap this batch fills is that **every existing `formalization` entry is an
accept-shaped fixture, and the only two negative-shaped entries (`brokenmath`,
`goldbach_collatz`) are crank prose**. We have nothing that tests the gate against
*technically valid Lean that nonetheless should not be reported as an unconditional
theorem*. That is what items 1 through 4 below supply.

### ACCEPT-fixture shortlist (ranked)

1. **`erdos_public` — new corpus, track `formalization`, 7 items.**
   MIT, sorry-free, self-contained single-file solutions, sizes 79 to 1,203 lines.
   Three of the seven are disproofs, which we have zero of today. `Erdos231` (79
   lines) and `Erdos441` (124 lines) are small enough for CI on every commit.
   Highest value per unit of wiring effort in the whole batch. Wire all seven; tag
   231/328/441 with a `refutation` sub-kind so the gate is exercised on the
   "counterexample discharged by kernel `decide` through a proved Bool/Prop bridge"
   pattern.

2. **`ramanujan_tau` — new corpus, track `formalization`, 1 item, flagged
   `conditional`.** The hard positive of §4. One item, but it is the only fixture we
   would have that distinguishes a correct conditional proof from an unconditional
   one. Expected gate behaviour to assert: ACCEPT, with
   `hypotheses = {ABC, Proposition5_4_strengthened}` and
   `warnings = {RamanujanTau uninhabited-unchecked}` in the certificate. Wire this
   second precisely because it is the assertion that is hardest to get right.

3. **`axiom_math_formalization` — new corpus, track `formalization`, ~6 items.**
   The straightforward accept-shaped runs from the family: `Granville/wronskian`,
   `andrews_dhar/thm1`, `Biswal/theorem1`, `fel-polynomial`, `dead-ends`,
   `challenge_3/lemma-b3`. All MIT, all sorry-free, all ship `task.md` + `*.tex` +
   `problem.lean` + `solution.lean`, so each yields a complete
   informal-input → statement → proof triple. These are 2,000 to 4,000 lines each, so
   they are nightly-tier, not per-commit. Their real use is as **blueprint-scale
   targets**: they are the right size to exercise the sketch pipeline and the
   blueprint-scale run end to end.

### REJECT-fixture shortlist (ranked, and the rarer half)

4. **`PartitionElliptic` — REJECT (statement drift).** 87 lines, the cheapest fixture
   in the batch. The gate must not report "Ono's Key Formulas formalized". Assertion:
   flag that three prose-stated side conditions (`E4 ≠ 0`, `J ≠ 1728`, and the
   consumption of `0 < y`) are absent from the formal statement, and that the claimed
   modular content is absent. Wire this first among the rejects — it is small, it is
   unambiguous, and it targets the drift detector directly.

5. **`HigherDyson/Batch4` — REJECT or ACCEPT-with-vacuity-warning.** Six `Summable`
   hypotheses with no witness plus an opaque `Pd` and a de-primitivized `ω`.
   Assertion: the gate must emit a vacuity warning for an unwitnessed hypothesis
   bundle. Batches 2 and 3 are the paired control (same mathematics, zero summability
   hypotheses at all). 448 lines for Batch4 alone.

6. **`challenge_3/lemma-b2` — REJECT (mild, disclosed drift).** The generating-function
   origin of `q_n` is replaced by a closed form and the equivalence is explicitly
   declined. 596 lines. Lower priority than 4 and 5 because the drift is disclosed in
   the docstring, which makes it a good *calibration* case: the detector should flag
   it but at lower severity than PartitionElliptic.

### Not for the registry, but keep

- **`andrews_dhar/thm2_split4`** — blueprint-scale target (16,199 lines) and the
  case study of §3.5. Use it to exercise the axiom-audit path and the critique loop's
  ability to notice a natural-language self-admission of error inside an artifact. Do
  not put a 16k-line file in CI.
- **`dead-ends`** — already listed under item 3, but note separately that its README
  supplies ground truth for a novelty/subsumption check (Mirsky 1947).
- **`Biswal`** — `input/theorem2and3/theorem1.lean` is a verbatim copy of
  `Biswal/theorem1/solution.lean`, fed back in as an input. That is a real, small,
  labelled instance of lemma reuse across runs: exactly the growing-lemma-library
  pattern from the paper-mining notes, with the dependency documented in the README.
- **`WanShi`** — non-Lean (Bend2) probe for the `FormalSystem` dispatcher. No licence,
  so read-only detection testing at most.

### SKIP outright

`kepler98` (mined, no licence), `Super_MARIO` (mined, it is a tool),
`goldbach-collatz-proof` (mined and already registered; also structurally broken —
`main.tex` is 20 lines and the `sections/*.tex` its README links to are not in the
repo, so it is not even a complete crank document).

---

## 6. Risks

1. **We have not compiled any of this.** Every "sorry-free" and "complete" claim above
   is from static inspection on a machine with no Lean or Lake. Before any of these
   becomes an ACCEPT fixture, it must actually build under its pinned toolchain
   (4.26.0 or 4.28.0) with the pinned Mathlib from `lake-manifest.json`. A fixture
   that our gate must accept but that does not compile is worse than no fixture.

2. **Toolchain pinning is a maintenance liability.** Two different Lean versions
   across ten repos, both pinned to exact Mathlib revisions, with READMEs stating
   "compatibility with earlier or later versions is not guaranteed." Wiring these
   into CI means either vendoring several Mathlib checkouts or accepting that fixtures
   rot. Prefer the small self-contained files (Erdos231, Erdos441, PartitionElliptic)
   which touch little of Mathlib and are most likely to survive version drift.

3. **The REJECT fixtures are judgement calls, not ground truth.** PartitionElliptic
   and challenge_3 are drift *by our standard*; their authors disclosed the
   simplifications and would say they formalized what they set out to formalize.
   HigherDyson Batch4 may well have inhabited hypotheses that simply were not
   exhibited. If we assert "the gate must reject these" we are encoding a policy, and
   we should write the policy into the fixture metadata rather than pretending it is a
   fact about the mathematics. This is the main risk in this document.

4. **Single-vendor monoculture.** Ten of the fourteen repos are one organization's
   prover output with one house style: axiomatize the hard objects, assume the deep
   results, prove the rest carefully. Tuning our gate against this family risks
   overfitting to AxiomProver's habits. Balance with fixtures from other sources
   before drawing conclusions about gate quality.

5. **`task.md` and `requirement.md` are untrusted imperative text.** If we ingest them
   as informal-input for formalization items (which is their whole value), we are
   feeding author-written instructions into a prompt. The HigherDyson Batch3 line
   ("lemmas you must DERIVE, never assume") is harmless, but the ingestion path must
   treat these as data and not as system-level direction.

6. **The `#print axioms` signal is nearly absent.** Exactly one occurrence across
   21,000+ lines of Axiom Math solutions. We cannot rely on upstream axiom audits; our
   own audit has to run on every fixture, which means we need a working build for
   every fixture, which loops back to risk 1.

7. **`decide` at scale.** Erdos441 and Erdos231 discharge nontrivial finite checks by
   kernel `decide`. That is sound but can be slow, and `decide` on a large `Finset`
   powerset can blow the elaborator's limits on a different toolchain. Budget for it.
