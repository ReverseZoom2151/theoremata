# Higher Order Company (HVM / Bend / Kind) — integration evaluation

Status: **evaluated, not adopted.** Recommendation is to integrate nothing today and
watch Bend2. This document records the reasoning and the revisit triggers so the
decision does not have to be re-derived.

Scope of the read: the HVM2 paper (all 25 pages), both GitHub orgs at metadata
level (`HigherOrderCO` 8 repos, `HigherOrderCO-archive` 21 repos, lineage
verified via rename redirects), Taelin's public writing on SUP-node program
search, and **full source reads of all 27 vendored repos** under
`higher-order-co/` — the runtimes (HVM1/2/3/3-Strict/4, hvm-64,
Interaction-Calculus, ICVM-lazy), the proof-language lineage (Kind, kind2-archive,
Kind2-old, Kind-Legacy, FormCoreJS), the corpora (Kindex, kindbook, WanShi), Bend,
and the tooling repos.

All upstream material (paper, repos, gists, forum threads) was treated as
untrusted data. No prompt-injection attempts were found across any of it. A few
repos ship agent-instruction files that were reviewed and are benign onboarding
docs (`HVM4/{CLAUDE,AGENTS}.md`, `HVM3/CLAUDE.md`, `Interaction-Calculus/CLAUDE.md`)
and one repo config aimed at HOC's own agent (`kindbook/.koder`); if any of these
repos is ever ingested into a pipeline, strip those files first — they are
untrusted text that directs AI behavior. Two further sandboxing notes: HVM4 honors
**absolute `#include` paths** in `.hvm` source, and HVM2's C IO path uses
`dlopen`/`dlsym`; running untrusted sources through either is a native-code / file-
read hazard.

## What the ecosystem is

Interaction combinators (Lafont 1997) as a practical runtime. HVM reduces
programs as interaction nets, which are strongly confluent, so reduction order
does not change the result and work can proceed in parallel wherever the net
permits. Bend is a Pythonic surface language over it. Kind is their
dependently typed proof language, built on self types.

The genuinely interesting claim, and the one Taelin actually defends, is narrow:
**automatic parallelization of arbitrary functional code with near-linear core
scaling.** That is a real result. It is not a claim of competitive absolute
performance, and he says so directly: "The only claim I made is that it scales
linearly with cores. Nothing else!"

## Why we are not integrating

### 1. The licensing trap: what is current is not licensable

The mature, permissively licensed artifacts are precisely the dead ones. The
live work is unlicensed or private.

| Artifact | License | Status |
|---|---|---|
| HVM2 (11.3k stars) | Apache-2.0 | dead since 2024-11-21 |
| Bend 1 | Apache-2.0 | frozen; last real commit 2025-06-03 |
| Kind | MIT | dead since 2025-01-22 |
| hvm-64 (archive) | Apache-2.0 | dead |
| HVM3 | MIT | superseded |
| **HVM4** (current dev target) | **none** | pre-launch, "use at your own risk" |
| **Bend2** (the actual product) | — | **private, 404** |

No LICENSE file means all rights reserved. There is no path that is
simultaneously current and legal to vendor into an Apache/MIT project. There is
no GPL or source-available contamination hazard anywhere in either org, so the
risk is purely the unlicensed newest work.

Embedding maturity is weak independently of licensing: the `hvm` crate is
v2.0.22 (2024-08), roughly 1,500 downloads/month, and its library surface is
compiler internals rather than a designed embedding API. HOC's own docs direct
integrators to shell out to the CLI. HVM3 and HVM4 are Haskell and C
respectively, so the Rust-crate path is attached to the abandoned branch.

### 2. HVM cannot enter the trusted path

The HVM2 paper states the invariant plainly: **"A higher-order lambda that clones
its variable can not be cloned."** Its own worked example, Church-encoded 2^2:

```
C4 = (λf.λx.(f (f x)) λf.λx.(f (f x)))
```

"can not be soundly reduced by HVM2." Enforcement is explicitly delegated to
whatever source language compiles to it; HVM2 does not check the invariant and
will return a wrong answer. The known fix (Lamping bookkeeping nodes) is omitted
because it costs roughly 10x performance.

Against our core invariant — the gate is the sole soundness authority — this is
categorical. A reducer that is unsound on an unchecked class of terms cannot sit
anywhere near verification.

The corollary matters and is not negative: in an **untrusted** role the
unsoundness is harmless, since a wrong reduction merely produces a candidate
that fails the gate. That is our existing untrusted-generator / trusted-verifier
split. So the question was never "is HVM sound enough" but "does it accelerate
what we are actually slow at." See section 4.

### 2a. The exit-code trap — the most important finding in the vendored source

Verified in `HVM2/src/hvm.rs:660`, inside `interact_call`, on the exact
unsoundness the paper describes:

```rust
if b.get_tag() == DUP {
  if def.safe {
    return self.interact_eras(net, a, b);
  } else {
    // TODO:
    // Currently, we'll not allow copying of REFs with DUPs. While this is perfectly valid on
    // IC semantics (i.e., if the user know what they're doing), this can lead to unsound
    // reductions when compiling λ-terms to HVM. So, for now, we'll just disable this feature,
    // and consider it undefined behavior. We should add a `--unsafe` flag that allows it.
    println!("ERROR: attempt to clone a non-affine global reference.\n");
    std::process::exit(0);
  }
}
```

**A soundness violation prints to stdout and exits with status 0.** Any harness
that shells out to HVM2 and reads the exit code sees an unsound-reduction abort
as success.

This is precisely the bug class we fixed in `c1042ba fix(prover): Metamath
backend must not trust the exit code (soundness)` — the `metamath` binary also
returns 0 on a failed verification, printing `?Error` to stdout instead. Two
independent tools in the same week, same failure mode. That is a strong argument
for making the sentinel-over-exit-code rule a **general** backend requirement
rather than a Metamath special case: any new backend should have to state
explicitly what constitutes proof of success, and exit status alone should never
qualify.

Worth recording alongside it that HVM2's own safety analysis is documented as
incomplete, in `ast.rs:566`:

> "This does not completely solve the cloning safety in HVM. It only stops
> invalid **global** definitions from being cloned, but local unsafe code can
> still be cloned and can generate seemingly unexpected results, such as placing
> eraser nodes in weird places."

So the guard that produces the `exit(0)` above does not catch all cases; unsafe
local code can slip through and simply produce wrong answers with no diagnostic
at all.

Two further hazards from the same read. HVM2's C runtime IO path (`run.c`, behind
`#ifdef IO`) contains `dlopen`/`dlsym` for arbitrary dynamic libraries — running
untrusted `.hvm` through `run-c --io` is arbitrary native code execution by
design. And the Rust entry point hardcodes `GNet::new(1 << 29, 1 << 29)`, roughly
4GB + 2GB reserved per net, which rules out spawning many small nets without
patching.

Also relevant to our platform: HVM2's README states "Windows is currently not
supported, please use WSL", and its CUDA "feature" is auto-detected by `build.rs`
rather than user-controlled, so build output silently depends on whether `nvcc`
is present on the machine.

Paper quality is also worth recording. It is an unfinished draft: section 11
("Benchmarks") is an empty TODO while the headline MIPS figures live in the
abstract, there is no methodology section, no baselines, and no error bars. It
contains no theorems or proofs of its own; strong confluence is asserted in one
sentence inherited from Lafont, and concurrent linker correctness is argued by
three worked interleavings plus "it is easy enough to see." Operate-2 rewrites
are excluded from interaction counts, so MIPS is a mildly flattered metric.

### 3. Kind fails cheat detection outright

Evaluated as a candidate 7th `FormalSystem` backend. It fails requirement (c),
and not in a way we can patch.

- **Holes type-check silently at any expected type.** In `src/Kind/Check.hs`,
  `Hol` is logged as information and returned as `Ann False (Hol nam ctx) typx`;
  logging is not failure in that monad. So `theorem : Hard = ?x` **passes**.
  Metavariables do not even log. There is no `--no-holes` or trust-audit flag.
- **No termination or positivity checking.** Kind is a general-recursion
  language, so `foo : False = foo` type-checks and every proposition is
  inhabited.

Our layer-2c source scan catches `sorry`/`admit`/`postulate` because those are
keywords. **No amount of source scanning detects non-termination.** A Kind
backend would be a 7th system that can "prove" anything, laundering false
results through a gate that reports them as verified — strictly worse than
having six sound ones.

Reading the five vendored generations directly (`Kind`, `kind2-archive`,
`Kind2-old`, `Kind-Legacy`, `FormCoreJS`) verified both findings verbatim in
source and added a **third, independent disqualifier**:

- **`Type : Type` in all five generations** — Girard's paradox. In
  `Kind/src/Kind/Check.hs`, `go Set = return $ Ann False Set Set`; the `Set`
  constructor carries no level field, so a universe hierarchy is not merely
  disabled, it is unrepresentable. Even with a perfect termination checker the
  logic would be inconsistent. The authors treat this as a deliberate
  expressivity choice: `Kind-Legacy/CONTRIBUTE.md` states "expressivity is
  default and consistency is a planned opt-in... programs that do not halt, and
  logical paradoxes, aren't prohibited." Five years and four rewrites later the
  opt-in is still unimplemented, and `kind2-archive/formal/kind2.agda` — the file
  that claims to verify Kind's soundness — contains one line: `-- TODO :D`.
- **Holes are universally equal, not merely accepted.** `Equal.hs` returns
  `True` for any conversion check touching a `Hol`, so a hole satisfies *any*
  type anywhere in a term, not just its expected one.
- **Two generations always exit 0**, even on hard type errors (`kind2-archive`
  and `FormCoreJS` never call `exitFailure`); the current one exits 0 on holes
  with a green ✓. Verdict must be parsed out of ANSI-colored stdout, never the
  exit code — the same soundness trap as HVM2 and Metamath, a third instance.
- **The cheat surface is not a keyword.** Kind has no `sorry`/`admit`/`axiom`
  mechanism to grep for and no kernel axiom list to enumerate. The cheat is
  `foo : P = foo`, syntactically indistinguishable from a legitimate recursive
  definition. You cannot write a cheat-detector for Kind, which is the deepest
  reason it cannot be a backend under our model.

The lineage also shows the trend is toward a smaller, faster, more ergonomic
checker, not a sounder one: `Kind2-old` (generation 3) is the *only* one where an
inspection hole is fatal; generations 4 and 5 dropped that. There is a genuine
research artifact here — `Kind2-old/crates/kind-checker/checker.hvm` is a compact
850-line bidirectional type checker written as interaction-net rewrite rules,
i.e. type checking as a parallel workload — but it belongs in a notes folder, not
a backend list.

Ecosystem signals corroborate rather than contradict:

- HOC **archived Kind's own proof corpus**: `Kindex` archived, `KindBook` moved
  to the archive org. When the vendor archives the stdlib, that is the verdict.
- Last commit 2025-01-22. Latest release `v2.0.3-alpha` (2022-12); the only
  releases with binaries are Kind 1.x from 2021, a generation that no longer
  exists.
- Four to five rewrites in about four years: FormCoreJS, Kind2-old,
  Kind-Legacy, kind2-archive, Kind (Haskell). Each abandoned the prior corpus.
- `Nat/` contains 43 files and **zero proofs** — all executable functions, no
  `add/assoc`, no `comm`. No real mathematics has been formalized in Kind.

The corpus question deserves hard numbers, since "is there anything to harvest"
is the only reason a dead system would still be worth touching. Measured against
the local checkout, `Kindex` is 1,088 `.kind2` files:

| Namespace | `.kind2` files | What it is |
|---|---:|---|
| `Data/` | 717 | stdlib — and where the actual theorems live |
| `Apps/` | 325 | application code (322 of them the Kind compiler itself) |
| `Math/` | 29 | algebraic structure records — proves almost nothing |
| `Prop/` | 15 | the equality/logic kernel |
| `Trait/` | 7 | typeclass-style dictionaries |

The namespace names mislead. `Math/Algebra/{Magma, Semigroup, Monoid, Group}` is
26 files of **record declarations, type aliases, and one-line field
projections** — not one of them proves anything, and the hierarchy stops dead at
Group (no ring, no field, no module, no homomorphism). The whole `Math/` tree
contains exactly **one** real proof: `Antisymmetric.to_Antisymmetric_alt`.

The genuine mathematics is under `Data/`: `Nat.add.{assoc,comm,identity}`,
`Nat.mul.{comm,distr}`, `Nat.Le.{transitive,antisymmetric}`, decidable equality,
constructor injectivity/disjointness, `List.{concat.assoc, reverse.involutive,
map.length}`, `String.concat.assoc`.

Correct totals: **~56 files that state and discharge a proposition, ~85 proved
statements, or 5.1% of the corpus.** The remaining 94.9% is executable code.

Three things make it worse than the raw count suggests:

- **The ceiling is `mul.comm` on unary naturals.** There is no ℤ, no ℚ, no ℝ, no
  number theory beyond `mod`/`div`, no analysis, no combinatorics. `gcd`,
  `lcm`, and `factorial` exist as functions with no theorems attached; `Prime`
  is a record definition with no instance and no result about it.
- **Eight axioms are formatted exactly like theorems.** Every file under
  `Data/U120/` (`mod.is_less_than`, `shift_right.pass_or`, …) carries a
  signature and doc comment with **no proof body**. They are postulates about
  120-bit arithmetic, asserted and never discharged, syntactically
  indistinguishable from proved results. Against ~85 real theorems that is ~9%
  contamination that any naive extractor would ingest as ground truth.
- **It does not typecheck as shipped.** Kindex's own README, frozen at the final
  commit three years ago: *"Kindex is being reorganized right now (May 9,
  2023)... many types will not type-check over the next few days."*

There is also no tactic language — every proof is a hand-built term of
`Equal.{refl, apply, chain, mirror, rewrite}` — so there is no signal here for
tactic prediction, proof search, or premise selection, which are the three
things we would actually want data for.

`kindbook` (988 `.kind` files, the post-refactor syntax) is strictly worse: it
**dropped the theorem layer entirely**. Only ~7 files state a proposition, all of
them the equality kernel (`refl`, `sym`, `trans`, `subst`), with nothing built on
top. What it has instead is 2,140 `#test:` ground-term assertions — checking
`Nat/add #3 #4 == Nat/add #4 #3` is not proving commutativity. It is also
**unlicensed**, hence all-rights-reserved.

For calibration: mathlib4 is >200,000 theorems and miniF2F is 488 competition
problems. Kindex is ~0.04% of mathlib4, and its ~85 statements are `a+b=b+a`
and `rev(rev xs)=xs` rather than AMC/AIME problems. Nothing here is harvestable
as training or eval data, and every statement in it already exists in mathlib4,
Agda's stdlib, and Coq's stdlib, proved more generally in a syntax with orders
of magnitude more training representation.
- Type theory is self types with `Set` as the sole universe (no visible
  hierarchy), a single ~130KB Haskell implementation, no formal metatheory.

What does work: MIT licensed, clean exit codes (`ExitSuccess` / `ExitFailure 1`),
and the checker is self-contained (does not require HVM). Output is ANSI-colored
text with no machine-readable mode. Install is a source GHC/cabal build only —
no cargo, no npm, no current binary release.

### 4. Superposition does not transfer to proof search

This is the most interesting idea in the ecosystem and deserves a precise
refutation rather than a dismissal.

SUP nodes are the dual of DUP: DUP clones one value into two locations, SUP
merges two values into one. A term containing SUPs represents many terms at
once, and any subcomputation the branches share is performed **once**. Taelin's
slogan is exact: **"why parallelize when we can share?"**

So the mechanism is not parallelism. It is automatic structural memoization
across candidates, obtained free from the reduction rules. The demos are real:
ADD-CARRY searches 2^16 = 65,536 candidates in ~36k interactions versus ~262M
sequential, roughly 7,277x, under one interaction per candidate.

It does not transfer, for three reasons:

1. **The demos are hole-filling in a supplied template, not search.** ADD-CARRY
   fills 16 holes in a hand-written 8-case skeleton. Proof search has unbounded
   depth and structure and no supplied template. Proposing the skeleton is the
   hard part, and it is exactly the part the gist concedes is hand-supplied.
   The gist also concedes "the general problem remains exponential."
2. **The sharing factor decays toward 1x as search gets interesting.** The
   speedup requires candidates to share reduction work. Templated candidates
   share nearly everything; distinct proof terms for a nontrivial theorem share
   little, and dependent typechecking kills branches *early* — a type error
   terminates a candidate before shared work accrues. The demos are constructed
   to avoid precisely this regime. No one has measured the degradation.
3. **The checker must run inside HVM.** Superposition accelerates only terms
   reduced by the HVM runtime. Superposing proof candidates therefore requires
   reimplementing a dependent typechecker inside HVM and trusting it — swapping
   Lean's battle-tested auditable kernel for a from-scratch one inside a runtime
   that is itself unsound on a characterized class. This is the inverse of our
   thesis, and is likely why HOC is building Bend2 with its own dependent types
   rather than integrating with an existing prover: the technique cannot reach
   an external checker.

Evidential status: SupGen (productized as NeoGen) is **closed source**; the
headline result required a bespoke `dup_labels` fork of **HVM1**, the oldest
dead runtime; no third-party replication exists (the one public attempt is
abandoned mid-way); and after ~18 months there is still no CASC, TPTP, or
miniF2F number. On HN the single substantive question — could it compete in
CASC — went unanswered.

**The strongest confirmation of point 2 came from HOC's own code, not from
skeptics.** The vendored `HVM3/examples/enum_coc_smart.hvm` is a full dependently
typed proof-term synthesizer written in HVM: it implements a WNF reducer, a
definitional-equality check up to α/β with de Bruijn levels, and type-directed
`@intr`/`@elim`/`@pick` synthesis, then solves `(?X λt(t A B)) == λt(t B A)` by
superposed enumeration. It brings that search down to ~3k interactions — and the
author's own comment explains why, honestly:

> "the real win is that now we only need to make a choice when selecting an
> element from context. Intros and elims follow directly from types, no need for
> choices / superpositions."

That is the whole thing in the author's words. In the implementation, `@intr`
introduces on Π/∀ with **no superposition at all**; `@pick` is the *only*
superposing function, and it superposes solely over which hypothesis to draw from
context. So the dramatic numbers come from **types collapsing the choice points**,
not from superposition evaluating a large candidate space in parallel. The
residual role of superposition is narrow: share the reduction work across the few
genuinely irreducible choices. This is exactly the sharing-decays-as-branches-
diverge argument in point 2, now confirmed by the reference implementation rather
than argued a priori. The demonstrated space is a two-candidate inversion problem;
there is no in-repo evidence it scales to real proof search, and no reproducible
benchmark of any kind (the "42x vs Bend" figure does not appear in any vendored
repo).

There is also a hard soundness reason not to put this on the trusted path,
separate from all of the above: the Interaction Calculus is **total by design** —
"there are no runtime errors." A mis-labeled or non-affine encoding does not
fault; it silently computes something else. Label capacity is bounded (8 labels
in a 32-bit build, 65,536 in 64-bit), and label overflow `printf`s a warning and
**continues with a truncated (colliding) label** rather than aborting. A search
engine whose miscompilations manifest as quietly-incomplete results rather than
errors is acceptable as an untrusted generator feeding a checker we trust, and
disqualifying anywhere near the gate.

### 5. Performance claims rest on a weak baseline

Recorded because it bears on whether HVM would help as an untrusted accelerator.
The critique is well founded and Taelin conceded the core of it: the flagship
`sum` example ran 4.5s under PyPy versus 42+ minutes under Bend, and he replied
"I think I made a huge mistake of using a 'sum' example ... HVM2's codegen is
still abysmal." The sharpest general form: reporting MIPS measures GPU
utilization rather than useful work, since the interaction-net encoding inflates
operation count (~2 IC nodes per numeric op). HOC's own HVM3 benchmarks claim
"up to 42x faster than Bend" single-threaded, which is to say the shipping stack
is very slow per core. The paper itself concedes ~5x slower than GHC
single-threaded, up to 100x on loops and mutable arrays. No independent
comparison against hand-written CUDA exists in either direction.

Other admitted limitations: eager-only evaluation, so ordinary recursion such as
`foo(x) = x==0 ? 0 : 1+foo(x-1)` hangs without manual restructuring; GPU
throughput requires hand-annotating branching recursive redexes with `!`;
24-bit numerics and a 29-bit address space (~4GB); minimal IO; no meaningful
FFI; NVIDIA-only GPU with no Windows support; essentially no debugging story.

### 6. Bottleneck mismatch

Independent of every point above: our proof loop is dominated by LLM inference
latency and proof-checker subprocess time (`agda --safe`, Lean elaboration).
HVM accelerates pure functional term reduction, which is not our bottleneck. A
faster reducer does not help a pipeline waiting on a model and on a typechecker.

### 7. Project churn: nothing here has survived two years

The `HigherOrderCO-archive` org (21 repos) is the hard evidence on durability.
Lineage was verified through GitHub rename redirects, which are conclusive — a
redirect means the same repo object, same creation date, renamed in place.

**The proof language has been reimplemented five times in seven years.**
`moonad/Formality` redirects to `HigherOrderCO/Kind`: it is one continuous repo,
created 2018-07-13, rewritten in place across Formality, Formality-Core /
FormCoreJS (700-LOC JS kernel), Kind1 (JS), Kind2 (Rust), Kind2-on-HVM2 (Rust),
and Kind (Haskell). Implementation language changed nearly every generation.

**The runtime has been reimplemented six times in four years:** HVM1, HVM2
(`HigherOrderCO/HVM` renamed in place), hvm-core / hvm-64 (496 stars, an entire
parallel effort, discarded), HVM3 (Haskell), HVM3-Strict (C, **alive five
weeks**), HVM4 (C). A C rewrite in 2025 following a Haskell rewrite in 2024 is
thrash, not convergence.

Median created-to-last-push across the archive is ~7 months; restricted to major
projects it is ~12 months. **No project in the archive survived two years of
active development.**

Three specifics that bear directly on us:

1. **The archive org was created 2026-01-29** — and HVM3's final push carries the
   same date. This was a single mass-cleanup event, not organic decay. The
   current generation has not yet been tested against that pattern.
2. **Proof work is the part they abandoned, not the part they kept.** Bend is the
   only repo pushed within the last month; Kind has been static 18 months and its
   stdlib is archived. Integrating anything proof-related means adopting the
   exact subsystem the vendor deprioritized.
3. **Version pinning is impossible.** They rename repos in place (Formality→Kind,
   HVM→HVM2, hvm-core→hvm-64), silently changing what a URL means. Combined with
   10 of 21 archive repos carrying **no license**, and `Kindex`'s own README
   pointing at a `Kind1` URL that now 404s, there is no stable artifact to
   depend on.

A softer signal worth recording: Bend's own tooling was archived out from under
it — `bend-language-server` and `tree-sitter-bend` both died 2024-10-18 while
Bend remains "active."

Domain trajectory across the whole history: proof assistant (2018) → crypto
(`kindelia/Kindelia`, 615 stars, dead since 2023-11; `moonad/Moonad`) →
parallel-computing runtime and GPU language (2023-2025) → AI program synthesis
(2025-2026). `HigherOrderCO/Kindelia` redirecting *out* to `kindelia/Kindelia`
shows an entire org's attention redirected to crypto and then divested.

### 8. HVM4 fixes the technical objections — and is still unusable

Reading the vendored HVM4 source (6,435 lines of C, one file, 68 doc files, 218
tests) overturns two of the technical criticisms above. They applied to the HVM2
*paper*; they do not apply to the current runtime.

**It is lazy.** `wnf()` is a weak-head-normal-form evaluator; `ALO` terms expand
the book one layer at a time on demand; only `USE` forces. Ordinary recursion
works, and decisively, infinite corecursive data works —
`@X = &Z{#Z{}, #S{@X}}` is an infinite superposition of every natural number and
it evaluates. That is impossible under HVM2's eager model.

**The higher-order cloning restriction is gone.** HVM4 has 24-bit labels (~16.7M)
on `SUP`/`DUP`/`DP0`/`DP1`, plus *runtime-computed* labels via `DSU`/`DDU`.
`DUP-SUP` branches on label equality: same label annihilates, different label
commutes. The Church 2² term that HVM2 declared out of bounds now evaluates —
the docs walk it in 14 interactions and call that optimal, with passing tests
using distinct labels for the two copies. In place of the old silent-wrong-answer
rule, the parser enforces a **static affinity check**: each variable is used at
most once, and multiple uses require a `λ&x` annotation that makes the compiler
insert a fresh-label DUP chain. The obligation moved from "the runtime
misbehaves" to "annotate, and the compiler picks non-colliding labels."

**The superposed search machinery is real and working.** `-C[N]` collapses a
superposed term into enumerated solutions via a priority queue ordered by SUP
depth minus `INC` credit, with `ERA` pruning failed branches.
`devs/test/enum_nat.hvm` solves `x + 2 = 4` by enumeration over an infinite
superposed nat; `enum_primes.hvm` factors a 20-bit semiprime the same way. This
is no longer aspirational — it runs.

None of which makes it adoptable:

- **No license, definitively.** A whole-repo grep for
  `copyright|licen[sc]e|MIT|Apache|GPL|SPDX` returns **zero matches**. No
  LICENSE, NOTICE, author, or contact anywhere. All rights reserved.
- **No library API.** `#define fn static inline` on essentially every function;
  the whole file exports **one symbol, `main`**. Integration means
  subprocess-and-scrape-stdout, not FFI. Global mutable state, non-reentrant,
  not thread-safe, `exit(1)` on every error path.
- **POSIX-only** (`sys/mman.h`, `mmap`, `realpath`) — will not build on Windows
  without a shim, which is our platform.
- **No types, no proof machinery.** It is a substrate. Anything proof-shaped is
  ours to build on top.
- **Live wrong-answer bugs.** `devs/issues/` contains `dyn_dup_bug.hvm`
  (dynamic-label dup crashes on a numeric label, works on a variable one),
  `fork_syntax_bug.hvm` ("the lines below should be equivalent, but aren't"),
  and a regression test recording a recently-fixed miscompilation where a 3-use
  auto-dup had the wrong de Bruijn index, producing "crashes or wrong results."
- **Two unbounded writes to fixed stack buffers in the parser** — `u32 cs[4096]`
  for string literals and `Term es[4096]` for list literals, no bounds check.
  Since any harness would be *machine-generating* `.hvm` source, that is a
  realistic hazard, not a theoretical one.
- **An unguarded label-collision path.** Static labels are base64-packed from
  their name while auto-dup labels allocate upward from `0x800000`, so a 4+
  character label starting with an uppercase letter F-Z can collide with a
  compiler-generated one — silently turning a commute into an annihilate, i.e. a
  wrong answer. Nothing detects it. (Code-reading finding; nothing was executed.)

Also worth noting for sandboxing: `#include` in an `.hvm` file honors **absolute
paths**, so an untrusted source can make the parser read any file the process
can read.

### 9. Bend2 is a dependently typed proof assistant — evidence found

The `WanShi` repo was listed in metadata as "Python, undocumented, 248 files."
That is wrong: it contains **zero Python**. It is 246 `.bend` files, and its
README reads "WanShi - A Standard Lib for Bend2."

It is largely **dependently typed theorem proving**: `Set/` (`And`, `Or`, `Not`,
`Iff`, `Exists`, `Forall`, `Implies`, `Iso`, Hilbert axioms, injectivity and
bijection), `Algebra/` (magma through quasigroup, loop, cancellative,
distributive), a large `Nat/`, plus `Proof/`, `LC/`, `Lambda/`, `TermRwtSys/`.
The proofs are real: `Nat/add/commutative.bend` is an inductive proof using
`match` / `rewrite` / `finally` tactics against an
`Algebra/commutative<Nat,Nat>(Nat/add)` type. `Bend/Bend_LM.bend` defines Bend's
own `Term` type in Bend — a self-representation.

This is the strongest public evidence that **Bend2 is a proof assistant**, not
just a parallel language, and it is a direction entirely absent from Bend 1
(which greps zero for `dependent`/`theorem`/`proof assistant`). WanShi is the
library *written in* Bend2, so it leaks the surface syntax and tactic vocabulary
without disclosing the implementation. Unlicensed, so it is readable as
intelligence and not usable as a corpus.

Separately, a correction to the public record: **Bend 1 does have a type
system** — an optional Hindley-Milner checker (`src/fun/check/type_check.rs`,
785 lines of Algorithm W) enabled by default since 0.2.37. `FEATURES.md` still
claims Bend is "an untyped language"; that text is stale.

## What is worth taking: the idea, not the dependency

"Why parallelize when we can share?" is a legitimate critique of our current
direction. `components/reason/search/concurrent.rs` parallelizes proof attempts
but does nothing to share work between candidates that overlap. Candidates from
one decomposition routinely share lemma prefixes and subgoals, and we re-verify
those from scratch every time.

A hash-consed subterm cache keyed on (statement, context), memoizing checker
results across candidates, captures the same economics as superposition in plain
Rust with no soundness cost — caching a *verified* result is safe in a way that
superposed reduction is not. This composes with the subsumption-dedup item
already on the adopt list from the paper mining.

The full source reads surfaced four more ideas worth transplanting, all
independent of adopting any HVM code:

1. **Inline-heuristic best-first enumeration.** `HVM3/src/HVM/Collapse.hs` flattens
   a search tree with a stable min-priority-queue (`flattenPRI`) where two term
   constructors, `CInc` and `CDec`, let the *generator emit its own search-priority
   scores inline* — a promising branch marks itself and is dequeued earlier. That
   is best-first proof search with the scoring function written in the object
   language. We can adopt the shape (a priority queue over partial proofs where the
   generator annotates promise) in Rust without any interaction-net machinery. It
   is MIT-licensed, so the code is even readable as a reference.
2. **Type-directed generation that only branches on hypothesis selection.** The
   `enum_coc_smart.hvm` architecture — introductions and eliminations forced by the
   goal type, choice confined to "which hypothesis to apply" — is a good template
   for a tactic generator in any substrate, and it is the real source of its
   speedups. Our decomposition/sketch pipeline should branch as little as the goal
   structure forces.
3. **The repeated-invariant discipline.** Bend's `desugar_book` re-runs
   `check_unbound_vars()` three times between passes, explicitly commented as a
   sanity check. Cheap insurance for our own long gate/transform pipelines.
4. **The Agda interaction protocol.** `agda-cli` drives Agda over
   `--interaction-json` with hand-written `IOTCM` s-expressions, getting goal
   types, contexts, and errors back as JSON rather than scraped stderr. If our Agda
   backend currently batch-runs and regexes stderr, this is the upgrade path — with
   three fixes the repo itself gets wrong and that matter for a prover: add
   `--safe`, fail on non-empty `visibleGoals` / `UnsolvedMetas` (not just on
   `type:'error'`), and never trust the exit code (Agda exits 0 with type errors).

None of these requires vendoring HOC code; item 1 is the only one where reading
their MIT source helps, and even it is a re-implementation, not a dependency.

Finally, a general hardening item this exercise argues for on its own: **make
"exit status is not proof of success" a standing backend requirement.** Three
independent systems in the vendored set — Metamath, HVM2, and two Kind
generations — return 0 on failure. Our gate should force every backend to declare
what positively constitutes proof of success (a stdout sentinel, a JSON verdict
field, an empty-goals check) and treat exit status alone as never sufficient.

## Revisit triggers

1. **Bend2 ships publicly with a license and its promised proof system.** Taelin
   has said it will have "a complete proof system (like Lean and Kind)."
   Evaluate it specifically on two questions: does it check termination, and
   does it reject holes? Absent both, the Kind verdict applies unchanged.
2. **A published CASC / TPTP / miniF2F result for SupGen or NeoGen.** This would
   directly test the sharing-decay argument in section 4.
3. **HVM4 gains a LICENSE file** and a stable embedding API. Necessary but not
   sufficient — it would only reopen the untrusted-accelerator question, which
   section 6 independently disfavors.

## Organizational risk

Three public org members (`VictorTaelin`, `nicolas-abril`, `dellamora`); 2026
commit activity is overwhelmingly Taelin solo. Seed was ~$4M in 2023; the
company is currently running a Wefunder community round for Bend2 as "The AI
Programming Language," with NeoGen positioned as a **paid** feature — the first
stated monetization path. (Wefunder returned HTTP 403; the round's terms come
from search summaries and are **unverified**.) The website still footers
"© 2024" and markets Bend 1 and HVM with no mention of Bend2 or HVM4.

Strategic direction has shifted from "parallel GPU language" to "AI-assisted,
proof-checked programming language with LLM synthesis." HVM has been demoted
from the product to the substrate. For a verification-first project, taking a
dependency on a pre-launch unlicensed runtime from a three-person team mid-pivot
is disproportionate risk for the available upside.

## Coverage gaps

Recorded honestly so the next reader knows what was and was not checked:

- **All 27 vendored repos were source-read.** Both orgs are also fully enumerated
  at metadata level (29 repos), with lineage verified through rename redirects.
  The one thing deliberately not done is a full source read of the archive-org
  repos that were *not* vendored locally; given that every one is dead and ~half
  are unlicensed, deeper reads would inform nothing actionable.
- `interaction-calculus-of-constructions` (Taelin's "minimal proof checker", 81
  stars) was **not** in the local set and was not read. It is the one remaining
  artifact that could bear on the proof-search question; unlicensed, so readable
  as literature only. A follow-up read is the single loose end worth picking up
  if this line is revisited.
- **Nothing was built or executed.** All findings are from source reading. Two
  are explicitly code-reading-only and would need a test to confirm: HVM4's
  label-collision path (4+ char labels colliding with the auto-dup range) and its
  two fixed-buffer parser overflows.
- `x.com` returned HTTP 402 during the web pass, so any Taelin thread content is
  from search-engine summaries rather than primary text. Lower confidence; not
  load-bearing for any conclusion here — the load-bearing findings are all from
  vendored source.
- The "42x faster than Bend" figure is **not present in any vendored repo** and
  could not be substantiated; treat it as an unverified public claim.
