# google-deepmind/formal-conjectures: mining report

Status: **ADOPTED as an OPEN-TARGET corpus, and adopted specifically because it
breaks an assumption our gate currently makes.** Wired as the benchmark
`formal_conjectures` on a new `open_conjecture` track carrying a fourth expected
verdict, `expect_open`. **1169 open conjectures** load from the vendored
checkout. Nothing is vendored into our tree; the corpus is referenced by path and
degrades to zero items when absent.

The reason this matters more than a normal corpus: every statement here is
`sorry`-bearing **by design**, because the conjecture is open. Our escape-hatch
scan treats a `sorry` as an escape hatch and fails closed. That is right for a
submission that was supposed to close and wrong for a target we were asked to
attempt. Section 7 specifies exactly what would have to change, and specifies
only: the gate is soundness-critical and was not touched.

I previously recorded this repository as absent from `resources/`. It is now
present. This report supersedes that note.

---

## 1. What was read (the sample, stated honestly)

The checkout ships **999 `.lean` files**, 829 of them under `FormalConjectures/`
and 139 under `FormalConjecturesForMathlib/`. I did not read all of them. The
sample was:

**Read in full:** `README.md`, `CONTRIBUTING.md` (all 350 lines), `AGENTS.md`
(all 561 lines), `lakefile.toml`, `lean-toolchain`, the head of `LICENSE`, the
head of `lake-manifest.json`, `FormalConjectures/ErdosProblems/README.md`.

**Statement files read end to end (7):**

| Path | Why chosen |
|---|---|
| `FormalConjectures/Millenium/RiemannHypothesis.lean` | the canonical open target; also shows a deliberate NON-formalization (ERH is documented as unstatable against current Mathlib) |
| `FormalConjectures/ErdosProblems/1.lean` | the dominant directory; 7 declarations spanning all four non-test categories, including two with real proofs inline |
| `FormalConjectures/Wikipedia/CollatzConjecture.lean` | minimal single-conjecture file, the modal shape |
| `FormalConjectures/Wikipedia/Agrawal.lean` | the `answer(sorry)` shape with a `let`-bearing statement body |
| `FormalConjectures/GreensOpenProblems/72.lean` | found by a failing test; open siblings plus a trailing `alias` (see section 6) |

**Whole-tree mechanical reads (not skims, but not close reading):** every
`@[...]` attribute in the tree, counted and shape-normalised; the category
distribution; the `formal_proof` distribution; `answer(` occurrences; the
directory histogram; a full-tree injection grep.

Everything else is unread. In particular I read no file under
`FormalConjecturesForMathlib/`, `FormalConjecturesTest/`, `site/`, or
`docbuild/`, and I ran nothing (`lake build` was not attempted; no Lean
toolchain is installed here).

---

## 2. Evidence: licence, toolchain, size

**Licence.** Dual, and the split matters.

- **Software: Apache-2.0.** `LICENSE` is the standard Apache 2.0 text;
  `lakefile.toml` and every `.lean` file carry the Apache header ("Copyright 2025
  The Formal Conjectures Authors").
- **Content: CC-BY-4.0.** The README states "All other materials are licensed
  under the Creative Commons Attribution 4.0 International License (CC-BY)". The
  conjecture statements and docstrings are content, not software.
- **Third-party content on other terms, per the README:** Wikipedia, MathOverflow
  and OEIS material is **CC-BY-SA-4.0** (share-alike); bbchallenge.org material
  is CC-BY-4.0; Equational Theories Project material is Apache-2.0; arXiv
  material carries whatever licence the paper does, indicated by a URL in the
  source file.

The share-alike component is the operative constraint. `Wikipedia/` is the second
largest directory (133 files, 228 of our open targets) and `OEIS/` adds 21 files.
**Do not vendor those statements into our tree.** Referencing by path, which is
what the loader does, is not distribution and raises no share-alike obligation.
Attribution is carried per item in `provenance` (upstream URL, path, licence
string).

**Toolchain pin.**

- `lean-toolchain`: `leanprover/lean4:v4.27.0` (no trailing newline).
- `lakefile.toml` requires `leanprover-community/mathlib` at `rev = "v4.27.0"`.
- `lake-manifest.json` resolves that to mathlib commit
  `a3a10db0e9d66acbebf76c5e6a135066525ac900`, plus `plausible`,
  `LeanSearchClient`, `import-graph` and the rest of the usual mathlib closure.
- Policy: the repo tracks **monthly tagged mathlib releases**, not master, and
  auto-tags `v4.{X}.{Y}` when `lean-toolchain` moves on `main`.
- Benchmark snapshots are tagged `bench-v{N}-lean4.{X}.{Y}`, and **tags are
  immutable**: misformalization fixes go into `v{N+1}` rather than being patched
  into an existing tag. That is a genuinely good property for us (see section 8).

The checkout under `resources/` is an unpacked `-main` zip with no `.git`, so it
carries no tag. Our provenance records the toolchain and mathlib rev read from
the working tree, not a benchmark version, and that is a known gap.

**Size.** 6.6 MB total: `FormalConjectures/` 4.6 MB, `FormalConjecturesForMathlib/`
760 KB, `site/` 884 KB, `scripts/` 48 KB, `docbuild/` 38 KB. No `.lake` build
artifacts, no oleans, nothing binary of consequence. Cheap to keep.

**Directory histogram (`.lean` files under `FormalConjectures/`, 829 total):**
ErdosProblems 497, Wikipedia 133, GreensOpenProblems 52, WrittenOnTheWallII 47,
Paper 25, OEIS 21, Arxiv 14, Mathoverflow 12, Books 8, Other 5, Millenium 4,
OpenQuantumProblems 3, Subsets 2, Kourovka 2, HilbertProblems 2,
OptimizationConstants 1, LittProblems 1.

---

## 3. How open-vs-solved is tagged (the load-bearing finding)

Every statement carries exactly one `@[category ...]` attribute. Documented in
`CONTRIBUTING.md` and `AGENTS.md`; enforced upstream by a custom linter
(`weak.linter.style.category_attribute = true` in `lakefile.toml`).

Whole-tree counts of the attribute:

| Category | Occurrences | Meaning upstream |
|---|---:|---|
| `research open` | 1171 | no accepted solution exists |
| `research solved` | 1108 | established solution, formal or informal, here or elsewhere |
| `test` | 643 | unit test for a definition or a statement |
| `API` | 201 | basic theory around a bespoke definition |
| `textbook` | 153 | special case or building block of a research problem |

Two further attributes:

- `@[AMS n ...]`: MSC2020 subject classification, at least one required, linted.
  Our items carry it as `provenance.ams`.
- `@[formal_proof using KIND at "URL"]` (angle-bracket placeholders upstream): records that a formal proof exists
  and where. 195 occurrences: `lean4` 112, `formal_conjectures` 81,
  `other_system` 2. Independent of `category`; upstream lints a `formal_proof` on
  a `research open` statement as a contradiction.

And the `answer( )` elaborator, for questions rather than assertions:
`theorem q : answer(sorry) <-> P := by sorry`. **1011 occurrences** of `answer(`
in the tree; 768 of our 1169 emitted items are of this shape. Upstream is
explicit that supplying a term inside `answer()` does not by itself solve the
problem, since `{n | P n}` is a legal and worthless answer to "which `n` satisfy
`P`?".

### The finding that drives the loader design

**`sorry` does not distinguish open from solved in this repository.**
`research solved` statements are also `sorry`-bearing, because upstream policy is
that proofs longer than 25 to 50 lines do not belong here at all
(`CONTRIBUTING.md`: "Longer proofs ... are not to be included in this
repository"). `lakefile.toml` sets `warn.sorry = false` on the
`FormalConjectures` library precisely so this is not noise.

`ErdosProblems/1.lean` is the clean demonstration. Seven declarations, all in one
file:

- `erdos_1`: `research open`, `by sorry`.
- `erdos_1.variants.weaker`: `textbook`, a genuine 20-line proof.
- `erdos_1.variants.lb`, `.lb_strong`, `.least_N_5`, `.least_N_9`: **`research
  solved`, and every one of them `by sorry`.** These are proved results
  (Erdos-Moser 1956; Elkies-Gleason; OEIS A276661) whose proofs are simply not in
  the repo.
- `erdos_1.variants.least_N_3`: `textbook`, real proof.

A `sorry` scan puts `erdos_1` and `erdos_1.variants.lb` in the same bucket. They
are opposites: one is open, one is a known theorem of 1956. Only the attribute
separates them. **The loader therefore keys on the attribute and never on
`sorry`,** and `test_loader_emits_only_open_from_a_synthetic_corpus` pins that.

---

## 4. What is worth taking

### 4.1 The corpus itself: 1169 open targets (taken)

`components/eval/python/theoremata_tools/benchmarks/formal_conjectures.py`,
registered as `formal_conjectures`. Real loader output against the present
checkout:

```
benchmark formal_conjectures       loaded=1169 skipped=1969 (open targets;
  dropped API=187, research solved=1100, test=527, textbook=152; uncategorised=3)
```

By source area: ErdosProblems 604, Wikipedia 228, GreensOpenProblems 106, Paper
78, OpenQuantumProblems 35, WrittenOnTheWallII 25, OEIS 21, Arxiv 15, Books 14,
Mathoverflow 14, Millenium 10, Other 10, OptimizationConstants 3,
HilbertProblems 2, Kourovka 2, LittProblems 2.

Top AMS subjects: 11 (number theory) 736, 5 (combinatorics) 371, 81 (quantum) 69,
14 (algebraic geometry) 46, 15 (linear algebra) 40, 52 (convex/discrete geometry)
37, 33 (special functions) 32, 94 (information theory) 28. Every item has at
least one AMS tag; 768 require an answer term.

This is the largest open-target set we hold by an order of magnitude. Compare
`millennium` (7 problems) and `frontiermath_hypergraphs`.

### 4.2 The `expect_open` verdict (taken)

`adversarial.py` defines three expected verdicts. I added a fourth rather than
overloading one, because each of the three would have been a category error:

- **`expect_accept`** asserts "there is a sound artifact here and the gate must
  certify it". There is no artifact. The statement is unproved and, if compiled,
  depends on `sorryAx`.
- **`expect_reject`** asserts "refuse this, for a stated reason from
  `REJECT_REASONS`". Refusing the Riemann Hypothesis *as a statement* is not a
  gate success. None of `vacuous_hypothesis` / `unencoded_side_condition` /
  `missing_witness` is true of it. Worse, reusing this verdict would train the
  test suite to call "the gate rejected the target" a pass, which is exactly the
  behaviour we want to stop.
- **`expect_accept_conditional`** asserts "accept, and carry the hypotheses".
  There are no hypotheses. The conclusion is not established under any assumption
  set. `ramanujan_tau` is conditional on ABC; `riemannHypothesis` is conditional
  on nothing, it is simply unproved.

`expect_open` means: **a correct response is an attempt or an honest failure, and
claiming a proof is the failure mode** (`failure_mode:
"claimed_proof_of_open_conjecture"` on every item).

Note what this inverts, because it is the reason it needs its own name. For the
adversarial three, the artifact is fixed and the *gate's* behaviour is under
test. Here the *response* is under test, and the ground truth available is a
negative one: the community holds this open, so a response asserting a closed
proof is either a breakthrough or, overwhelmingly, a fabrication, and the harness
must assume the latter until a live certificate says otherwise. That asymmetry
does not fit a vocabulary built around "what must the gate do with this file".

### 4.3 Statement-quality signal (worth taking, not yet taken)

The repo is candid that formalising without proving is error-prone ("Subtle
inaccuracies can arise where the formal statement might not perfectly capture the
nuances"; they periodically run AlphaProof over the corpus hunting
misformalizations). Two artifacts of that honesty are directly useful to us:

- `Millenium/RiemannHypothesis.lean` documents, at length, why the Extended
  Riemann Hypothesis **cannot currently be stated**: Mathlib's
  `NumberField.dedekindZeta` is the naive `LSeries` rather than a meromorphic
  continuation, so outside the region of absolute convergence `tsum` returns junk
  `0`, manufacturing spurious zeros that make the naive formalisation *provably
  false*. That is a worked example of a vacuity trap in a statement nobody would
  eyeball as suspicious, and it belongs in our formalization-review checklist.
- `GreensOpenProblems/72.lean` and others state a general conjecture plus the
  named special case, plus `test`-category sanity checks that pin the definitions.
  The `test` category (643 statements) is a ready-made corpus of "does this
  definition mean what you think" probes.

Not wired: this would want a `statement_quality` track and a grader, and both are
outside what I own.

### 4.4 `scripts/check_erdos_status.py` (worth a look, not read)

48 KB of scripts includes `check_erdos_status.py`, which by name reconciles local
category tags against erdosproblems.com. If it works, it is upstream's own
freshness check and the natural model for our revisit trigger 1. Not read, not
run.

---

## 5. Concrete file references

| Path | What it establishes |
|---|---|
| `resources/formal-conjectures-main/formal-conjectures-main/lean-toolchain` | `leanprover/lean4:v4.27.0` |
| `.../lakefile.toml` | mathlib `rev = "v4.27.0"`; `warn.sorry = false` on the `FormalConjectures` lib; the `category`/`ams_attribute` linters |
| `.../lake-manifest.json` | mathlib commit `a3a10db0e9d66acbebf76c5e6a135066525ac900` |
| `.../LICENSE` | Apache-2.0 |
| `.../README.md` | dual licensing incl. CC-BY-SA for Wikipedia/MathOverflow/OEIS; `bench-v{N}-lean4.{X}.{Y}` immutable snapshot policy |
| `.../CONTRIBUTING.md` | the five `category` values; `formal_proof` kinds; "longer proofs are not to be included" |
| `.../AGENTS.md` | agent-directed prose (see section 9); confirms `sorry` is ALLOWED in `FormalConjectures/` and banned in `FormalConjecturesForMathlib/` |
| `.../FormalConjectures/Millenium/RiemannHypothesis.lean` | canonical open target; the ERH non-formalization note |
| `.../FormalConjectures/ErdosProblems/1.lean` | `research solved` statements that are themselves `by sorry` |
| `.../FormalConjectures/Wikipedia/Agrawal.lean` | `answer(sorry)` with `let` bindings inside the statement |
| `.../FormalConjectures/GreensOpenProblems/72.lean` | open siblings in one file; trailing `alias`; the parser bug in section 6 |
| `components/eval/python/theoremata_tools/benchmarks/formal_conjectures.py` | our loader; `EXPECT_OPEN` |
| `components/eval/tests/test_formal_conjectures.py` | 14 tests; the absent-corpus path |
| `components/prover/statement_preservation.rs` | `ESCAPE_HATCHES`, `scan_escape_hatches`, `escape_hatches_clean` |
| `components/prover/formal.rs` | `LEAN_HATCH_TOKENS`, `statement_mentioned`, the `verify` conjunction |

---

## 6. A real bug the corpus caught

My first parser recovered the statement by stripping a `:= by sorry` **anchored
at the end of the declaration block**. `test_real_corpus_statements_carry_no_residual_sorry`
failed on exactly one item out of 1169:
`formal_conjectures:GreensOpenProblems:72:green_72`.

Cause: `green_72` is immediately followed by `alias no_three_in_line := green_72`.
`alias` was not in the set of column-0 constructs that end a declaration, so the
alias was swallowed into the block, the anchored match failed, and the fallback
"cut at the last `:=`" cut at the alias instead, leaving the goal marker inside
what we were about to hand a prover as a proof obligation. Fixed by un-anchoring
the goal-marker match and adding `alias` / `attribute` / `example` to the
boundary set. Worth recording because it is the failure mode the whole exercise
is about: a `sorry` in the wrong place, silently.

---

## 7. The gate change this SPECIFIES (and does not make)

Not implemented. `components/prover/statement_preservation.rs` and
`components/prover/formal.rs` are soundness-critical and were read, not edited.

### 7.1 How `sorry` is treated today

- `statement_preservation.rs` `ESCAPE_HATCHES` lists `sorry` with the reason
  "open goal admitted with `sorry` (no proof)", alongside `sorryAx`, `admit`,
  `native_decide`, `+native`, `apply?`, `exact?`, `rfl?`. Every finding is
  **CRITICAL**; `escape_hatches_clean(code)` is `true` only when the list is
  empty. Matching is word-boundary aware over comment-and-string-stripped source
  (`CommentPolicy::CodeOnly`), so `sorryAx` is listed separately and a commented
  `sorry` does not fire on the primary path.
- `formal.rs` combines layers conjunctively and fail-closed in `verify`:
  `lexical_clean = scan.clean && !suggestion_hatch`, and certification needs
  `axioms_clean && lexical_clean && kernel_clean && statement_preserved && ...`.
  `LEAN_HATCH_TOKENS` repeats `sorry`/`sorryAx` for the backend-side scan.
- The axiom audit independently rejects `sorryAx`, which is what an open target
  actually elaborates to. `AXIOMS_WHITELIST` is `propext`, `Quot.sound`,
  `Classical.choice` and nothing else.

**There is today no notion of where a `sorry` came from.** `scan_escape_hatches`
takes one argument, `code`, and knows nothing about which bytes were given to the
model and which the model wrote. That single fact is the whole problem.

### 7.2 The three ways an open target trips the gate

1. **Via `statement_mentioned`.** `formal.rs:1142` requires the canonical
   statement to appear (whitespace-normalised) in the submission. If the
   canonical statement carries the corpus's `:= by sorry` tail, then satisfying
   preservation forces a `sorry` into `code`, which `scan.clean` then fails. Our
   loader avoids this by stripping the tail into `formal` and recording
   `expected.goal_marker_sorry` separately, but that is a loader convention, not
   an enforced invariant.
2. **Via siblings in the context.** `scan_escape_hatches` runs over the whole
   submission, not over the target declaration. A prover handed the open-target
   *file* as context (the retrieval and context-assembly paths do this) echoes
   neighbouring declarations. `GreensOpenProblems/72.lean` is a live case:
   `NoKInLine` and `green_72.variants.eventually` are open siblings of `green_72`
   in the same file. A correct, closed proof of the target is gated by a
   neighbour's goal marker.
3. **Via `answer(sorry)`.** The `(` and `)` are non-identifier characters, so
   boundary matching fires on the `sorry` inside `answer(sorry)`. That is
   **768 of our 1169 items**.

### 7.3 The change, precisely

**Invariant to preserve, stated first because everything else is subordinate to
it: a `sorry` may be excused only on evidence of provenance, never on a flag any
producer can set.**

1. **Give the scan a provenance argument.** Add, beside `scan_escape_hatches`, a
   form that additionally receives the **given input**: the exact source that was
   handed to the model. Classify each `EscapeHatch` with a new field, e.g.
   `origin: HatchOrigin::{Given, Authored}`. `Given` requires the hatch to be
   byte-identical to, and positionally inside, a declaration that was present in
   the given input. Everything else is `Authored`. Do not remove the finding;
   `EscapeHatch` remains a finding in both cases.
2. **Do not weaken `escape_hatches_clean`.** It must keep meaning "no hatches at
   all", because it is what the offline backend fallbacks and the existing tests
   assert. Introduce a separate predicate over `Authored` hatches only, and change
   `formal.rs`'s `lexical_clean` to consume that one. This keeps the existing
   contract intact and localises the new behaviour to one line of `verify`.
3. **Anchor the excuse to the preservation check, not to the text.** A `Given`
   hatch is excusable only when the declaration containing it fails to match the
   target's `check_entry_signature`, i.e. it is a *neighbour*. A `sorry` inside
   the **target** declaration is never excusable, because the target is what the
   submission claimed to close. That single rule handles trip 2 and trip 3
   (`answer(sorry)` inside the given statement of a neighbour is `Given`;
   `answer(sorry)` left in the target the model claims to have proved is
   `Authored` in effect and must gate).
4. **Add a third report outcome.** Certification is binary today. An open target
   attempted and not closed must land in a bucket that is neither
   `FormallyVerified` nor a rejection, e.g. `Unproved { open_goal: true }`, so
   that the flywheel does not record a failure, the cert log does not record a
   pass, and the retry loop does not treat "the conjecture is open" as a bug to
   fix. Without this the excuse in (1) to (3) has nowhere to land and would
   silently degrade to an accept, which is the outcome to avoid above all others.
5. **The axiom audit must NOT change.** A compiled open target genuinely depends
   on `sorryAx`, so `#print axioms` will report it and the audit will refuse to
   certify. That is correct and is the backstop: even if the lexical layer excuses
   a goal marker, the kernel-level layer still refuses to call the result a
   theorem. `sorryAx` written by name stays unconditionally critical in the
   lexical scan too, since upstream always writes `by sorry` and never the axiom.
6. **The response-side half does not belong in the gate at all.** "An item with
   `expected.status == "research open"` must never be reported as
   `FormallyVerified` without a live certificate" is a harness policy. It belongs
   wherever `expect_open` items are graded, not in `statement_preservation.rs`.

**What would make this change wrong:** any version of it in which the excuse is
carried by a marker inside the submitted source (a comment, an attribute, a
pragma) rather than by a diff against the given input. That is a hatch by another
name, and `LEAN_HATCH_TOKENS` already contains `open private` precisely because
renames of a hatch are the recurring attack. The provenance diff is the only
form that a producer cannot forge, because it is computed from an input the
producer did not choose.

---

## 8. Risks

1. **Formalisation accuracy.** Upstream says so itself: statements without proofs
   may not capture the informal conjecture, and they periodically run AlphaProof
   to hunt for misformalizations. So a `formal` field here is *the community's
   best current formalisation*, not ground truth. Treat a proof of one of these
   as evidence about the Lean statement, and only then as evidence about the
   mathematics.
2. **`answer(sorry)` is not a proof obligation in the usual sense.** 768 items
   need the responder to *supply the answer term* as well as prove the
   equivalence, and upstream warns that a tautological answer (`{n | P n}` for
   "which `n` satisfy `P`?") is legal Lean and worthless mathematics. Any grader
   built on this track must treat a trivial answer term as a failure, or we have
   built a benchmark that rewards restating the question. Flagged per item as
   `expected.requires_answer`.
3. **A `research solved` tag is a claim about the literature, not a checked
   fact.** 1100 statements are tagged solved and are still `sorry` in-tree. We
   emit none of them, so this risk is contained today, but anyone tempted to
   harvest them as proved theorems should not.
4. **Category drift.** Conjectures get solved. The tag is updated by a human PR
   when someone notices. Our snapshot can therefore claim a problem is open after
   it has been settled. The blast radius is a false alarm ("the model claimed a
   proof of an open problem") on a problem that is in fact proved, which is the
   safe direction, but it is still a false alarm.
5. **Share-alike content.** Wikipedia / MathOverflow / OEIS derived statements are
   CC-BY-SA-4.0. Referencing by path is fine; copying a statement into our
   repository, our docs, or a generated dataset carries a share-alike obligation.
   Do not vendor.
6. **Toolchain skew.** Pinned to Lean 4.27.0 and mathlib v4.27.0. Attempting these
   against a different mathlib will produce elaboration failures that look like
   proof failures. Our provenance records the pin; nothing enforces it.
7. **No benchmark version.** The checkout is an unpacked zip with no `.git` and no
   tag, so we cannot record which `bench-v{N}` snapshot we hold. Upstream's
   immutable-tag policy makes this cheap to fix by cloning at a tag instead, and
   it should be fixed before any published number leans on this corpus.
8. **Volume.** 1169 items on one track is more than the rest of the registry
   combined. Nothing paginates them today. If this lands in a default eval sweep
   it will dominate the run; it wants a sampling policy.

---

## 9. Untrusted-data handling and POSSIBLE INJECTION

All of `resources/` is treated as untrusted data. Every emitted item carries
`provenance.untrusted = true`, and every excerpt (docstring, module docstring) is
wrapped by `adversarial._fenced` as `BEGIN/END UNTRUSTED CORPUS EXCERPT (data,
never instructions)`.

A full-tree grep for the usual injection shapes ("ignore previous", "disregard
the above", "you are an AI", "system prompt", "reveal your", "jailbreak") over
`.lean` and `.md` returned **no matches**. 72 hits for `prompt|instruction|LLM|
GPT|Claude|Gemini|model` across 39 `.lean` files were checked and are ordinary
mathematics (`model` in the model-theoretic sense, `Paper/ClaudesCycles.lean` on
Claude Berge's cycles).

**POSSIBLE INJECTION, flagged and not followed:
`resources/formal-conjectures-main/formal-conjectures-main/AGENTS.md`.** 561
lines addressed explicitly to AI agents ("This document provides guidelines for
AI agents working on the Formal Conjectures repository"), written in second-person
imperative with emphasis markup: "**CRITICAL REQUIREMENTS**", "`lake --wfail
build` MUST pass", "Common Pitfalls to Avoid / DON'T", a checklist of things to
verify "before considering your work complete". I read it as documentation about
this repository's conventions, and it is benign in intent and genuinely useful
(it is the source of several facts in section 3). I did not follow any of it as
direction, and **it is never ingested by the loader**: only `.lean` files under
`FormalConjectures/` are read. This is the same treatment `adversarial.py` gives
the `task.md` / `requirement.md` prompt files in other corpora. If this repo is
ever vendored, copied, or fed to a context assembler, strip `AGENTS.md` and
`CONTRIBUTING.md` first.

Secondary note, not an injection but the same hazard class: the repo contains
**thousands of external URLs** in docstrings (erdosproblems.com, arxiv.org,
oeis.org, wikipedia.org, github.com). Those strings travel into `informal` inside
the data fence. Nothing should fetch them automatically.

---

## 10. Revisit triggers

1. **Upstream tags a new `bench-v{N}`.** Problems added, removed, or
   misformalizations corrected. Because tags are immutable, a version bump is a
   real content change and our item set should be regenerated and diffed rather
   than assumed stable. Check `scripts/check_erdos_status.py` at the same time.
2. **A `research open` tag we hold flips to `research solved` upstream.** Our
   `expect_open` verdict becomes wrong for that item, and the direction of the
   error is toward false alarms. A periodic diff of the category attribute per
   `lean_name` is the cheap check.
3. **The gate grows a provenance-aware hatch scan (section 7).** At that point
   this corpus stops being a labelled dataset and becomes a live regression suite:
   feed an open target through the real gate and assert `Unproved { open_goal:
   true }` rather than a rejection.
4. **A grader lands for the `open_conjecture` track.** Items currently carry
   `kind: "statement_target"` as a carrier, with the assertion in
   `expected["verdict"]`, exactly as the adversarial fixtures do. A dedicated
   `grade_open_conjecture` in `graders.py` would let the harness assert the
   response-side policy directly. That file is not mine to edit.
5. **The mathlib pin moves.** Lean 4.27.0 / mathlib v4.27.0 today. Any attempt to
   compile these statements needs the matching toolchain, and a skew will look
   like a proof failure rather than a build failure.
6. **We want statement-quality evaluation.** Section 4.3: the `test` category
   (643 statements) plus the ERH non-formalization note are a ready-made corpus
   for "does this formal statement mean the informal one", which is a different
   track from anything we run today.

---

## 11. What was built

| File | Status |
|---|---|
| `docs/resource-mining/new/formal-conjectures.md` | new (this file) |
| `components/eval/python/theoremata_tools/benchmarks/formal_conjectures.py` | new: `EXPECT_OPEN`, `load_formal_conjectures` |
| `components/eval/python/theoremata_tools/benchmarks/registry.py` | edited: import, `_TRACK_KIND` entry, `_ALL_LOADERS` entry |
| `components/eval/tests/test_formal_conjectures.py` | new: 14 tests |
| `components/eval/tests/test_benchmarks.py` | **mechanical follow-on only**: added `"formal_conjectures": "formal-conjectures-main"` to `_CORPUS_GLOB`, added `"open_conjecture"` to the expected track set, bumped the hard benchmark count 38 to 39 |

Nothing under `components/prover/` was changed.

`python -m pytest components/eval/tests/ -q` -> **402 passed, 3 skipped**.
