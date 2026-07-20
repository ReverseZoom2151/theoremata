# Proof staleness and proof repair: what is actually known

Research brief. Question: when a formal library changes underneath you, how do you know
which previously-verified proofs are now stale, and how do you repair them?

Scope note on evidence quality. Several primary sources here are PDFs that could not be
converted to text by the fetch tool, so for those the numbers come from the publisher
abstract page or the search index snippet rather than from the full text. Those cases are
marked "abstract only". Claims are separated into what a paper CLAIMS and what it
DEMONSTRATED wherever the distinction was recoverable.

Headline finding, stated up front: nobody has a sound, published, declaration-level
staleness detector for a mathematical library. What exists is (a) build-system trace
hashing, which is sound but coarse and conservative, (b) whole-archive rebuild, which is
sound and expensive and is what the two largest libraries actually do, and (c) Why3-style
goal shape matching, which is explicitly a heuristic reuse mechanism sitting on top of a
sound "everything is obsolete until replayed" default. The distinction between "renamed"
and "the mathematics changed" is not automatically decided anywhere I could find; it is
approximated by human-authored deprecation metadata.

---

## 1. Proof repair as a research area

### 1.1 PUMPKIN PATCH and PUMPKIN Pi (Ringer et al.)

Both tools exist and are real, contrary to the risk of misremembering.

- PUMPKIN PATCH: Coq plugin, "Proof Updater Mechanically Passing Knowledge Into New
  Proofs, Assisting The Coq Hacker". https://github.com/uwplse/PUMPKIN-PATCH and
  https://pumpkin.uwplse.org/
- PUMPKIN Pi: "Proof Repair across Type Equivalences", PLDI 2021.
  https://arxiv.org/abs/2010.00774 , https://dl.acm.org/doi/10.1145/3453483.3454033 ,
  author copy https://dependenttyp.es/pdf/repair.pdf
- Thesis: Talia Ringer, "Proof Repair", University of Washington.
  https://homes.cs.washington.edu/~djg/theses/ringer_dissertation.pdf

Technique. PUMPKIN Pi combines a configurable proof term transformation with a decompiler
from proof terms back to tactic scripts. The transformation implements transport across a
type equivalence in a way that removes all references to the old version of the changed
type, and it does so without adding axioms beyond what Coq already assumes (so no
reliance on univalence or on a transport axiom). Source: the PLDI 2021 abstract at
https://arxiv.org/abs/2010.00774

The earlier PUMPKIN PATCH works differently: it diffs an old and a new proof of a
changed theorem, extracts a reusable "patch" proof term, and applies it to other broken
proofs. That is example-driven generalization rather than transport.

What it was evaluated on. Eight case studies, per the abstract: a benchmark taken from a
user study, easing development with dependent types, porting functions and proofs between
unary and binary number representations, and supporting an industrial proof engineer
interoperating between Coq and other verification tools. The abstract reports no aggregate
success rate, no denominator, and no head-to-head baseline. This is a demonstration of
flexibility, not a measured repair rate. Anyone citing PUMPKIN Pi as "N percent of broken
proofs repaired" is inventing a number.

Failure modes and limits.
- The change must be expressible as a type equivalence between old and new types. A
  library change that strengthens a hypothesis, weakens a conclusion, or fixes an actual
  error in a definition is not an equivalence, and transport does not apply. This is the
  central structural limit.
- The user must supply or let the tool search for the equivalence configuration. It is
  "configurable", which means it is not push-button for arbitrary changes.
- Decompilation from proof term back to tactic script is best-effort. The repaired term is
  what is trusted; the script is a readability convenience.

What it needs. The proof TERM (the transformation operates on terms), plus optionally the
SCRIPT for the decompiled output to resemble the original. Statement alone is not enough.
This matters: a harness that stores only tactic scripts cannot use this class of tool
without re-elaborating first.

### 1.2 The REPLica user study, and why "the proof broke" is usually the wrong frame

Ringer et al. instrumented the Coq REPL and monitored every change eight proof engineers
made to programs, specifications and proofs over a month; the instrumentation shipped in
Coq 8.10. The finding most relevant here: roughly 75 percent of the time an engineer fixed
a broken proof, they did so by fixing something else, such as the program or the
specification, rather than the proof.
Source: dissertation, https://homes.cs.washington.edu/~djg/theses/ringer_dissertation.pdf
(search-index snippet; the PDF did not convert, so treat the exact 75 figure as
abstract-level evidence).

Implication for a harness: "repair the script" is the minority case in human practice. The
majority case is that the breakage is a signal that the statement or the definition was
wrong. A harness that automatically repairs scripts is optimizing the 25 percent, and
worse, it can paper over the 75 percent by making a now-wrong statement compile again.

### 1.3 Sisyphus (PLDI 2023)

"Mostly Automated Proof Repair for Verified Libraries", Gopinathan, Keoliya, Sergey.
https://dl.acm.org/doi/10.1145/3591221 ,
https://verse-lab.github.io/sisyphus/pdfs/sisyphus-pldi23.pdf ,
artifact https://zenodo.org/records/7703886 , code https://github.com/verse-lab/sisyphus

Technique. Given an OCaml program with a verified original version and an evolved,
unverified version, Sisyphus infers new loop invariants for the evolved program and
produces a Coq (CFML-style separation logic) proof for it, discharging most goals
automatically and handing the programmer a small set of residual obligations. It uses the
old proof as a source of candidate invariants and uses testing/enumeration to filter
candidates before attempting proof.

Evaluated on. 10 OCaml programs from popular libraries manipulating arrays and mutable
data structures, each with an original verified version and an evolved version. Sisyphus
repaired proofs for all of them, with correct inferred invariants and a small number of
residual obligations. Distinguished paper at PLDI 2023
(https://www.comp.nus.edu.sg/news/2023-acm-sigplan-isergey/).

Honest reading: 10 out of 10 on 10 hand-selected programs is a feasibility result, not a
repair rate. The class of change is "the program changed", not "the library underneath
changed", which is a different and in some ways easier problem because the specification
is held fixed.

Failure modes. Restricted to a specific verification framework and to imperative array /
mutable structure code. "Mostly automated" is in the title because residual obligations
are expected. Scaling to a mathematics library with no program to test against is not
demonstrated and, since the technique leans on executing the program to filter candidate
invariants, likely does not transfer.

What it needs. The old proof SCRIPT plus both program versions. Not applicable to pure
mathematics.

### 1.4 PRISM: infrastructure and datasets for repair

"Proof Repair Infrastructure for Supervised Models: Building a Large Proof Repair
Dataset", Reichel, Henderson, Touchet, Gardner, Ringer, ITP 2023.
https://drops.dagstuhl.de/entities/document/10.4230/LIPIcs.ITP.2023.26 ,
code https://github.com/Radiance-Technologies/prism

Technique. Mine git commits from open-source Coq projects and align old and new versions
of definitions and proofs across the commit boundary, producing paired (broken, fixed)
data. The paper's stated contribution is as much a complaint as a dataset: building it
surfaced deep gaps in proof assistant infrastructure, and the authors give recommendations
to the community so that machine learning tools target the tasks that actually matter.

Relevance here. This is the closest thing to a benchmark for the exact task "library
changed, repair the proof". Note what alignment requires: you need the version control
history, and you need to solve the matching problem (which new declaration corresponds to
which old one) before you can even state the repair task. That matching problem is the
same problem as staleness classification, and PRISM solves it with heuristics over commit
diffs, not soundly.

### 1.5 CoqPyt

"CoqPyt: Proof Navigation in Python in the Era of LLMs", FSE 2024 demo.
https://arxiv.org/abs/2405.04282 , https://github.com/sr-lab/coqpyt

Technique. Python client for coq-lsp. Executes files, steps through proofs, retrieves
goal context, extracts premise data, and edits proofs by inserting and removing steps.
Supports Coq 8.17 to 8.19. It is plumbing for repair tools, not a repair technique. The
paper's own framing is that it is expected to aid development of LLM-based proof synthesis
and repair tools.

Worth noting for our purposes: the CoqGym dataset, long the standard for Coq machine
learning, was collected in 2019 and supports only Coq 8.10 and 8.12, which is itself an
instance of the staleness problem eating a benchmark.

### 1.6 LLM-based repair, and how it is evaluated

The honest summary is that almost all recent "repair" in the LLM literature is
error-message-driven repair of a proof the model itself just wrote, not repair of a
previously-verified proof after a library bump. These are different problems and the
numbers do not transfer.

- PALM, "Proof Automation with Large Language Models", ASE 2024.
  https://arxiv.org/abs/2409.14274 . Generate-then-repair: the LLM writes a full proof
  script, then deterministic symbolic repair mechanisms plus CoqHammer iteratively fix
  errors. Claimed to prove 180.4, 136.7 and 76.6 percent more theorems than Passport,
  Proverbot9001 and DSP respectively. Those are relative improvements over weak baselines
  on newly-attempted theorems, not repair-after-library-change rates.
- Rango, "Adaptive Retrieval-Augmented Proving for Automated Software Verification",
  ICSE 2025. https://arxiv.org/abs/2412.14063 . Introduces CoqStoq, 2,226 open-source Coq
  projects, 196,929 theorems. Again: synthesis, with retrieval of in-project premises and
  similar proofs.
- APOLLO. https://arxiv.org/abs/2505.05758 . Lean pipeline with syntax cleaning,
  "sorrifying" (replacing failing subproofs with sorry to localize failure), automated
  solving of the isolated subgoals, and targeted LLM repair. Claims state of the art
  84.9 percent on miniF2F among sub-8B models. The sorrify-and-localize trick is the piece
  most transferable to genuine repair: it turns "the file no longer compiles" into a set
  of localized, independently attackable holes.
- VeriSoftBench (https://arxiv.org/abs/2602.18307) includes prompts for agents whose task
  is to fix broken Lean 4 code, at repository scale.

Failure mode common to all of these, and the one that matters for a soundness-first
harness: an LLM repairing a proof against a changed library has a strong incentive to
change the STATEMENT until something compiles. Nothing in the generate-check-repair loop
distinguishes "I fixed the script" from "I weakened the theorem". Unless the statement is
pinned and compared up to a semantic notion, the loop launders a broken result into a
green one.

What they need. The SCRIPT and the error message. None of them need or use the proof term.

---

## 2. Engineering practice in the two large libraries

### 2.1 Mathlib

Primary source: "Growing Mathlib: maintenance of a large scale mathematical library",
arXiv https://arxiv.org/abs/2508.21593 , HTML https://arxiv.org/html/2508.21593v1 ,
also https://link.springer.com/chapter/10.1007/978-3-032-07021-0_4

What the paper states:
- Mathlib openly accepts that code written against one version may not compile against a
  newer one. There is no compatibility guarantee.
- Deprecation is the main mitigation. A renamed declaration gets
  `@[deprecated (since := "YYYY-MM-DD")]` pointing at the replacement, with a grace period
  of "several months" before removal, and the user gets a warning naming the replacement.
  Tooling exists to generate these (`Mathlib.Tactic.DeprecateTo`,
  https://leanprover-community.github.io/mathlib4_docs/Mathlib/Tactic/DeprecateTo.html).
- File reorganizations are handled analogously: old module names are preserved and
  marked so imports keep working, with a linter suggesting the replacement.
- CI merges through bors, enforcing the "not rocket science principle": master always
  passes CI. https://github.com/leanprover-community/mathlib-ci
- Technical debt is tracked by a weekly script, `scripts/technical-debt-metrics.sh`,
  counting adaptation notes, porting notes and backwards-compatibility flags, reported to
  Zulip.
- Lean core changes are absorbed via a branch tracking Lean nightlies, plus
  `lean-pr-testing-NNNN` branches automatically created for Lean PRs so that Mathlib CI
  results are reported back on the Lean PR
  (https://leanprover-community.github.io/contribute/tags_and_branches.html).
- Crucially, the paper states there is no downstream ecosystem testing infrastructure for
  Lean yet, and that such tooling "is being planned by the Lean FRO".

So the honest picture: Mathlib detects breakage in Mathlib by rebuilding Mathlib. It
detects breakage in Lean-core-versus-Mathlib by rebuilding Mathlib against Lean nightly.
It does not detect breakage in your project at all. Your project finds out when you run
`lake update` and the build fails. The community-standard automation for that is a GitHub
Action, https://github.com/oliver-butterley/lean-update , which bumps the toolchain,
rebuilds, and opens a PR on success or an issue on failure. That is regression testing by
brute force, and it is the state of the art on the Lean side.

Guidance for downstream users is at
https://github.com/leanprover-community/mathlib4/wiki/Using-mathlib4-as-a-dependency ,
which recommends not jumping to master in one step for large or long-lagging projects.

### 2.2 Isabelle and the Archive of Formal Proofs

The AFP model is the strongest existing practice, and it works by being conservative and
by centralizing the cost. Entries are tested and maintained continuously against the
current stable Isabelle release; on submission, authors agree to maintain the entry or
nominate a maintainer; in practice, when Isabelle evolution breaks entries, Isabelle
developers do the fixing, because changes to Isabelle and to the core HOL libraries are
always pushed through to AFP applications with fast feedback from build jobs.
Sources: https://isa-afp.org/about/ , https://www.isa-afp.org/submission/ ,
"Isabelle technology for the Archive of Formal Proofs",
https://ar5iv.labs.arxiv.org/html/1905.07244

The empirical study of what actually breaks:
"Why the Proof Fails in Different Versions of Theorem Provers: An Empirical Study of
Compatibility Issues in Isabelle", Luan, Sanan, Hou, Xu, Liu, Cai, Liu, Sun, FSE 2025.
https://dl.acm.org/doi/10.1145/3715787 ,
https://conf.researchr.org/details/fse-2025/fse-2025-research-papers/132/Why-the-Proof-Fails-in-Different-Versions-of-Theorem-Provers-An-Empirical-Study-of-C ,
PDF https://zhehou.github.io/papers/Empirical_Study_of_Compatibility_Issues_in_Isabelle.pdf

- Method: a regression testing framework that builds AFP theories under multiple Isabelle
  versions and collects the failures automatically.
- Scale: four Isabelle versions, more than 21,000 AFP theories, 12,079 compatibility
  issues collected.
- They categorize the incompatibilities into seven types, analyze root causes by automated
  analysis plus sampling, and mine the aligned fixed proofs in AFP to summarize resolution
  strategies. (The seven category names were not recoverable from the abstract pages
  accessible here; the PDF did not convert. Treat the taxonomy as existing and worth
  reading in full before designing our own classification, rather than as something I have
  verified in detail.)
- This is, as far as I can tell, the largest quantitative dataset on what "library changed
  underneath you" actually means in practice.

Note the detection method: rebuild everything, four times. That is the ground truth
against which any cheaper staleness detector would have to be validated, and no such
validation exists in the literature I found.

### 2.3 Coq/Rocq and the Coq Platform

Coq's CI tests a large set of downstream libraries and plugins on every PR, pinning them
by commit hash, and requires the PR author to supply "overlays", which are patches to the
downstream repositories that make them build against the changed Coq. This is the only
practice I found where the upstream is required to demonstrate the downstream repair
before the breaking change lands. Sources: the Rocq release process discussion,
https://github.com/rocq-prover/rocq/issues/19974 , and the long-running
"Regression proving with Coq: past, present, and future" issue,
https://github.com/rocq-prover/rocq/issues/9262

The Coq Platform addresses the complementary problem, reproducing a checked artifact
later: a curated distribution pinning Coq plus libraries plus plugins to a coherent set of
versions, explicitly motivated by reproducing and extending existing verification
artifacts. Palmskog, Tassi, Zimmermann, RRRR 2022.
https://arxiv.org/abs/2203.09835

---

## 3. Direct answers to the three questions

### 3.1 Staleness detection: can you avoid re-running everything, soundly?

Short answer: the only sound mechanism deployed anywhere is input-trace hashing at module
granularity, and it is deliberately conservative rather than precise.

What exists:

(a) Build-system traces. Lake, Lean's build system, computes a trace, generally a hash,
for each target, derived from its inputs: source file, toolchain, imports, and so on. If
the stored trace matches the recomputed trace, the target is up to date. Lake also caches
input hashes to avoid recomputation, and stores the build log inside the trace file
specifically so that a stale log cannot be paired with an up-to-date trace.
Sources: https://lean-lang.org/doc/reference/latest/Build-Tools-and-Distribution/Lake/ ,
https://github.com/leanprover/lean4/blob/master/src/lake/README.md ,
and the release note for the log/trace pairing fix,
https://lean-lang.org/doc/reference/latest/releases/v4.9.0/

Granularity: the module. Any change to any import transitively invalidates the module.
This is sound in the direction that matters (it never says "fresh" when the module would
now fail) but it is massively over-approximate: a docstring edit three modules upstream
invalidates you. It is also not itself a correctness argument. Lean has shipped real bugs
here, for example https://github.com/leanprover/lean4/issues/13449 , "Incremental build
cache can cause incorrect test results". Treat build traces as an optimization with a
history of soundness bugs, not as a verification-grade oracle.

(b) Declaration-level dependency plus axiom closure. Lean can compute the axioms a
declaration depends on, directly or indirectly, with `#print axioms`
(https://lean-lang.org/doc/reference/latest/ValidatingProofs/). The obvious idea is to
fingerprint the transitive closure of declarations a theorem's proof term actually uses,
and mark it stale only if something in that closure changed. Nobody appears to have built
and validated this as a staleness detector. Two cautions:
  - The existing axiom collector is known to be incomplete: `Lean.collectAxioms`, which
    backs `#print axioms`, does not collect axioms referenced by the types of axioms.
    https://github.com/leanprover/lean4/issues/8840 . So even the axiom closure, the
    thing you would most want to trust, is currently not exactly what it claims to be.
  - The transitive closure of a mathlib theorem is large. In practice it will intersect
    almost any nontrivial mathlib change, which collapses back toward "rebuild everything".

(c) Why3 sessions: the most developed staleness model I found. Why3 stores proof attempts
in a session; when the input file or the transformations change (including because Why3
itself was upgraded), the session manager rebuilds the goals and marks affected proof
attempts obsolete, since the provers might now answer differently. It then performs a
merge to decide which parts of the session can be reused, using per-goal checksums and
per-goal "shapes" for fuzzy matching when the exact checksum no longer matches. `why3
replay` re-runs the obsolete attempts; `--ignore-shapes` disables shape-based matching
during the merge. Sources: https://why3.org/doc/starting.html ,
https://why3.gitlabpages.inria.fr/why3/manpages.html ,
https://github.com/AdaCore/why3/blob/master/CHANGES.md

Read the design carefully: the sound part is the default, which is that everything
touched becomes obsolete and must be replayed. Shapes are a heuristic for reattaching
prior results to goals that look the same, and the documentation calling the merge
"clever and sound" refers to the merge operation not losing information, not to a proof
that a shape match implies the old result still holds. The tool ships a flag to turn
shapes off, which tells you the authors know they are heuristic.

(d) Fine-grained verification result caching. Leino and Wüstholz, "Fine-Grained Caching of
Verification Results", CAV 2015, https://link.springer.com/chapter/10.1007/978-3-319-21690-4_22
Uses the call graph and control flow graph to focus verification on the parts affected by
the most recent edit; implemented in Boogie, therefore usable by Dafny and other Boogie
front ends. The Dafny IDE paper states the principle plainly: changes to one entity
usually invalidate only a small fraction of prior results, and you can safely skip
re-verification except when a depended-upon entity changed
(https://arxiv.org/abs/1404.6602). This is the right shape of answer, but it is for a
program verifier over a fixed logic and a fixed background theory, not for a mathematics
library where the definitions themselves move.

Granularity verdict for a harness. Whole-library is what AFP and Mathlib actually do and
is the only thing with a track record. Module granularity is what build systems give you
for free and is sound-but-coarse. Declaration granularity is where the payoff is, is
technically feasible in Lean by walking the proof term, and is unvalidated. Axiom closure
alone is too coarse to discriminate and is currently buggy.

Nobody has solved staleness detection soundly and precisely at declaration level. That is
a real gap, and it is a gap we could actually fill for our own claim database because we
control what we store.

### 3.2 Renamed-versus-remathematized: is the distinction automatic?

No. Not in any tool I found. It is approximated by three human-supplied signals and one
brute-force check.

- Human signal 1: deprecation attributes. `@[deprecated (since := ...)]` in Mathlib
  encodes exactly the claim "this is a rename, the mathematics is unchanged, use that name
  instead". It is machine-readable, it names the replacement, and it is a human assertion,
  not a checked fact. It is also incomplete by construction, because it only covers
  renames the author chose to mark, and it expires after several months.
  https://arxiv.org/html/2508.21593v1
- Human signal 2: changelog markers. Rocq's reference manual marks potentially breaking
  entries as "Changed" (https://rocq-prover.org/changelog). Again a human claim.
- Human signal 3: Coq/Rocq overlays. The upstream author writes the downstream patch, so
  the classification is implicit in whether an overlay was needed and how invasive it was.
- Brute force: statement-level re-elaboration. If you kept the exact statement text and
  it still elaborates to a type that is definitionally equal to the old one under the new
  environment, the mathematics did not change from your point of view. If it elaborates to
  something different, or fails to elaborate, it did. This is checkable, and it is
  effectively what a harness should do, but I found no paper that presents it as a named
  technique for change classification.

The general theoretical obstruction is worth stating because it bounds ambition. Deciding
whether two program versions are semantically equivalent is undecidable by Rice's theorem,
and in the mainstream software ecosystem literature, syntactic breaking-change detection
is mature (Sigtest reported at 98.7 percent detection, Maracas at 96.3 percent precision
and 98.5 percent recall) while behavioral breaking changes, 68.1 percent of npm breaks,
remain hard to detect automatically. Source: the systematic review at
https://arxiv.org/abs/2605.24397 and https://arxiv.org/abs/2008.07069 . In a dependently
typed setting the situation is nominally better, because a statement is a type and type
equality is decidable up to definitional unfolding, but only nominally: definitional
equality checks can be very expensive, and the interesting changes are precisely the ones
where the new definition is not definitionally equal to the old.

The most useful concrete evidence that this distinction matters, and that automation gets
it wrong, is the miniF2F case. "miniF2F-Lean Revisited"
(https://arxiv.org/html/2511.03108v1) documents that `algebra_5778`, a cube-root problem,
became genuinely unprovable because Mathlib defines the nth root via `rpow`, a definition
chosen for compatibility with complex roots, which diverges from the precollege meaning.
The informal statement did not change. The formal statement did not change textually. The
mathematics underneath it changed, and the formalization became wrong. Over 300 formal
statements in the benchmark required correction overall, though the paper does not
separate library drift from original formalization errors. This is the exact failure a
naive repair loop would mask.

The Isabelle FSE 2025 taxonomy of seven incompatibility types is the closest thing to a
principled classification and should be read in full before we invent our own.

### 3.3 Certificates versus tactic scripts: does staleness apply the same way?

The intuition is correct and it is grounded in an old and explicit design principle, but I
found no paper that argues the durability point in the form you want it.

The principle is the de Bruijn criterion: a proof assistant satisfies it if it emits proof
objects checkable by a small independent program that a skeptic could write themselves,
so that proof generation and proof checking are independent. Named by Barendregt after de
Bruijn's Automath. Sources: https://www.pls-lab.org/en/de_Bruijn_criterion ,
https://lawrencecpaulson.github.io/2022/01/05/LCF.html ,
https://www.cs.ru.nl/~herman/PUBS/proofassistants.pdf

The engineering realization in Lean is lean4checker: it reads declarations and proofs from
`.olean` files and replays them through the kernel, without re-elaborating source. Its
trust assumptions are stated explicitly: structural correctness of the olean files, kernel
soundness, and honest library authors. It cannot check proofs that used native evaluation,
because it has no access to the compiler; such uses show up as a `Lean.trustCompiler`
axiom. https://lean-lang.org/doc/reference/latest/ValidatingProofs/ and
https://github.com/leanprover/lean4/issues/12216

What this gives you, precisely. A proof TERM is stale in a much weaker sense than a
script. A script is a program against an API (lemma names, simp set contents, instance
resolution, tactic implementations) and it rots when any of that moves. A term references
only declarations and their types. It stops being checkable only if a declaration it
references disappears or changes its type. So the term's staleness condition is exactly
computable in principle: does the transitive set of constants the term references still
exist, with the same types, in the new environment. That is the declaration-level
fingerprint from 3.1, and the proof term is what makes it computable.

A self-contained certificate over exact rationals is a further step, and here your
intuition holds most strongly. If the certificate is a data object plus a checking
procedure whose correctness does not depend on the library (a Positivstellensatz witness,
an LRAT refutation, a rational interval bound), then the object is replayable as long as
the checker exists, and it does not care about renames at all. See for example
LRAT-Catcher, importing SAT solver certificates into Lean 4 by reflection,
https://arxiv.org/abs/2607.00815 , and the certified-checker-extraction line of work such
as https://arxiv.org/abs/1502.05209 . The cross-system interoperability work makes the
same bet from a different direction: Dedukti, and OpenTheory article files for the HOL
family, treat the proof object as the durable artifact and the system-specific script as
disposable. https://deducteam.github.io/ , http://logipedia.inria.fr/about/about.php ,
https://github.com/Deducteam/Holide , https://europroofnet.github.io/tools/

The caveat you should not lose. A certificate is only as durable as its STATEMENT's
meaning. If the certificate says "this polynomial is nonnegative on this box" in terms of
exact rationals, it is forever. If it says "`Real.sqrt` of this thing is at most that
thing", and `Real.sqrt` gets redefined at the branch cut, the certificate is still a valid
object and the claim it supports has silently changed. The miniF2F cube root case is
exactly this. Certificate durability protects the proof, not the interpretation of the
theorem.

I did not find this argued explicitly in the ITP literature. The de Bruijn criterion
argues independence of checking, the interoperability work argues portability, but the
argument "therefore certificates do not rot the way scripts do, and here is the residual
rot they still have" appears to be unwritten. That is a gap, and it is a defensible thing
for us to state and act on.

---

## 4. Is "verified against environment E" first-class anywhere?

Weakly, and only in the benchmark community. I found no proof assistant or proof library
that treats an environment fingerprint as part of the claim such that an environment
change invalidates the claim.

What does exist:

- Toolchain pinning as a reproducibility convention. `lean-toolchain` plus a pinned
  Mathlib revision in `lake-manifest.json` is the de facto record of "this compiled
  against that". It is a build input, not a claim annotation, and nothing checks it later.
- The Coq Platform, which is explicitly motivated by being able to reproduce and extend a
  verification artifact later, by pinning a coherent set of versions.
  https://arxiv.org/abs/2203.09835
- Formal Conjectures (DeepMind) is the best example of the idea taken seriously. Frozen
  snapshots are tagged `bench-vNN-lean4.XX.YY`, where the first part fixes the problem set
  and the second pins the Mathlib tag and thus the toolchain. Fixes to misformalizations
  go into later releases rather than patching existing ones, so historical baselines stay
  meaningful. CI compiles the frozen subset files across all supported Lean versions on
  each tagged release, which is how breakage is detected. They also note explicitly that
  AI-generated proofs and disproofs serve as an auditing mechanism, because a failed proof
  attempt on something previously proved often reveals semantic drift or statement
  corruption. https://arxiv.org/html/2605.13171
- miniF2F recommends citing the commit or date used. https://github.com/facebookresearch/miniF2F

What does not exist, anywhere I looked: a claim record that carries an environment
fingerprint plus a dependency fingerprint and that flips from verified to unknown when the
environment moves. The general software world has the vocabulary for this (build
provenance, SLSA, hash-chained audit records) but nobody has applied it to proof claims.
The stale green is real: today, in every system I examined, a proof verified last year
against last year's Mathlib remains recorded as verified, with no mechanism that
downgrades it, and the only way to find out is to rebuild.

---

## 5. What a soundness-first harness could adopt

Ordered by ratio of value to effort. Each is stated as a design commitment, with the
source of the idea.

1. Store the proof TERM, not just the script, or at minimum store the transitive set of
   constants the term references together with their type fingerprints. This is the single
   decision that makes everything downstream possible: term-level dependency fingerprints,
   independent replay, and the rename-versus-remathematized distinction. Motivated by the
   de Bruijn criterion and realized by lean4checker.

2. Make the claim record carry an environment fingerprint. Toolchain version, library
   revision, and the dependency fingerprint from item 1. Formal Conjectures' two-part
   snapshot tag is the closest precedent. Nobody has done the invalidation half; we can.

3. Three-valued claim status, never two-valued. Verified-against-E, unknown-since-E-moved,
   refuted. A claim whose dependency fingerprint no longer matches becomes unknown, not
   verified and not failed. Why3's obsolete marking is the precedent, and its default
   (obsolete until replayed) is the sound default to copy.

4. Separate the two rechecks. Cheap recheck is replaying the stored term through the
   kernel against the new environment, which is lean4checker's job and which requires no
   tactic execution. Expensive recheck is re-elaborating the script. Only fall back to the
   expensive one when the cheap one fails. This is the concrete way to avoid re-running
   everything without inventing an unsound heuristic.

5. Pin and re-elaborate the STATEMENT separately from the proof, and treat a change in the
   elaborated statement type as a distinct, louder event than a change in the proof. This
   is the automatic approximation of "the mathematics changed versus the name moved", and
   it is the guard against an LLM repair loop weakening a theorem until it compiles. The
   miniF2F cube root case is the argument for why this must be a hard gate.

6. Consume Mathlib's deprecation metadata as a hint, never as a proof. A deprecation
   attribute is a human assertion that a change is a pure rename. It is exactly the right
   input to an automatic rename repair, and exactly the wrong thing to trust without
   re-checking the resulting term.

7. Prefer certificates for anything that can carry one. Exact-rational bounds, SOS and
   Positivstellensatz witnesses, LRAT refutations. These are replayable indefinitely and
   are immune to renames and tactic drift. Accept the residual risk that the statement's
   meaning can still drift underneath the certificate, and cover that with item 5.

8. Borrow APOLLO's localization trick for repair: replace failing subproofs with holes to
   turn a whole-file failure into a set of independent, small repair tasks, each of which
   can be attempted and each of which fails visibly rather than silently.

9. Validate any cheap staleness detector we build against the expensive ground truth.
   Rebuild everything periodically, compare against what the detector predicted, and
   report the false-fresh rate. The Isabelle AFP study is the model: four versions, 21,000
   theories, 12,079 issues, obtained by actually rebuilding. A staleness detector with no
   measured false-fresh rate is a liability, because its failure mode is a silent green.

10. Do not automate repair of the majority case. REPLica found that about 75 percent of
    human proof fixes were actually fixes to a program or specification. When the
    statement's elaborated type changed, the correct action is to escalate to a human or
    to re-derive, not to repair the script.

---

## 6. Open problems, stated as gaps

- No sound, precise, declaration-level staleness detector exists or has been evaluated for
  any mathematical library. Build traces are sound and coarse; shape matching is precise
  and heuristic; nobody has published the middle.
- No automatic classifier of "rename versus mathematical change" for formal libraries.
  Deprecation metadata is the human-supplied substitute. The Isabelle seven-type taxonomy
  is descriptive, not a detector.
- No published argument, in the form we want it, that certificates are durable where
  scripts rot, and no published account of the residual staleness a certificate still has
  (the meaning of its statement).
- No proof system treats environment provenance as part of a claim with automatic
  invalidation. Stale greens are the normal state of affairs across the field.
- Proof repair benchmarks that specifically target library-bump breakage barely exist.
  PRISM is the main one, is Coq-only, and predates the current wave of models. Most
  reported "repair" numbers in the LLM literature are self-repair of freshly generated
  proofs and do not measure this task at all.
- Lean's own axiom collection is incomplete (issue 8840), so even the coarsest sound
  fingerprint is currently slightly wrong. Worth tracking.

---

## Source list

https://arxiv.org/abs/2010.00774
https://dl.acm.org/doi/10.1145/3453483.3454033
https://dependenttyp.es/pdf/repair.pdf
https://homes.cs.washington.edu/~djg/theses/ringer_dissertation.pdf
https://github.com/uwplse/PUMPKIN-PATCH
https://pumpkin.uwplse.org/
https://dl.acm.org/doi/10.1145/3591221
https://verse-lab.github.io/sisyphus/pdfs/sisyphus-pldi23.pdf
https://zenodo.org/records/7703886
https://github.com/verse-lab/sisyphus
https://www.comp.nus.edu.sg/news/2023-acm-sigplan-isergey/
https://drops.dagstuhl.de/entities/document/10.4230/LIPIcs.ITP.2023.26
https://github.com/Radiance-Technologies/prism
https://arxiv.org/abs/2405.04282
https://github.com/sr-lab/coqpyt
https://arxiv.org/abs/2409.14274
https://arxiv.org/abs/2412.14063
https://arxiv.org/abs/2505.05758
https://arxiv.org/abs/2602.18307
https://arxiv.org/abs/2508.21593
https://arxiv.org/html/2508.21593v1
https://link.springer.com/chapter/10.1007/978-3-032-07021-0_4
https://leanprover-community.github.io/mathlib4_docs/Mathlib/Tactic/DeprecateTo.html
https://github.com/leanprover-community/mathlib4/wiki/Using-mathlib4-as-a-dependency
https://github.com/leanprover-community/mathlib-ci
https://leanprover-community.github.io/contribute/tags_and_branches.html
https://github.com/oliver-butterley/lean-update
https://isa-afp.org/about/
https://www.isa-afp.org/submission/
https://ar5iv.labs.arxiv.org/html/1905.07244
https://dl.acm.org/doi/10.1145/3715787
https://conf.researchr.org/details/fse-2025/fse-2025-research-papers/132/Why-the-Proof-Fails-in-Different-Versions-of-Theorem-Provers-An-Empirical-Study-of-C
https://zhehou.github.io/papers/Empirical_Study_of_Compatibility_Issues_in_Isabelle.pdf
https://github.com/rocq-prover/rocq/issues/19974
https://github.com/rocq-prover/rocq/issues/9262
https://rocq-prover.org/changelog
https://arxiv.org/abs/2203.09835
https://lean-lang.org/doc/reference/latest/Build-Tools-and-Distribution/Lake/
https://github.com/leanprover/lean4/blob/master/src/lake/README.md
https://lean-lang.org/doc/reference/latest/releases/v4.9.0/
https://github.com/leanprover/lean4/issues/13449
https://lean-lang.org/doc/reference/latest/ValidatingProofs/
https://github.com/leanprover/lean4/issues/8840
https://github.com/leanprover/lean4/issues/12216
https://why3.org/doc/starting.html
https://why3.gitlabpages.inria.fr/why3/manpages.html
https://github.com/AdaCore/why3/blob/master/CHANGES.md
https://link.springer.com/chapter/10.1007/978-3-319-21690-4_22
https://arxiv.org/abs/1404.6602
https://arxiv.org/abs/2605.24397
https://arxiv.org/abs/2008.07069
https://arxiv.org/html/2511.03108v1
https://arxiv.org/html/2605.13171
https://github.com/facebookresearch/miniF2F
https://www.pls-lab.org/en/de_Bruijn_criterion
https://lawrencecpaulson.github.io/2022/01/05/LCF.html
https://www.cs.ru.nl/~herman/PUBS/proofassistants.pdf
https://arxiv.org/abs/2607.00815
https://arxiv.org/abs/1502.05209
https://deducteam.github.io/
http://logipedia.inria.fr/about/about.php
https://github.com/Deducteam/Holide
https://europroofnet.github.io/tools/
