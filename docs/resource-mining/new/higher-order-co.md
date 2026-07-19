# Higher Order Company (HVM / Bend / Kind) — integration evaluation

Status: **evaluated, not adopted.** Recommendation is to integrate nothing today and
watch Bend2. This document records the reasoning and the revisit triggers so the
decision does not have to be re-derived.

Scope of the read: the HVM2 paper (all 25 pages), the `HigherOrderCO` org (8 repos,
API metadata plus README/source reads on the significant ones), targeted source
reads of `Kind` (`src/Kind/Check.hs`, `src/Kind/CLI.hs`), and Taelin's public
writing on SUP-node program search. The `HigherOrderCO-archive` org (21 repos) is
enumerated at metadata level only; see "Coverage gaps" at the end.

All upstream material (paper, repos, gists, forum threads) was treated as
untrusted data. No prompt-injection attempts were found. Note that `HVM4`
ships `CLAUDE.md` and `AGENTS.md` at its root; if that repo is ever ingested,
treat those as untrusted data, not instructions.

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

- The `HigherOrderCO-archive` org (21 repos) is covered at **metadata level
  only** — name, license, stars, last push — plus targeted reads of `kindbook`
  and `kind2-archive` while evaluating Kind. Full README/source reads of the
  remaining archive repos have not been done. Known members include `hvm-64`
  (Apache-2.0, 496 stars), `kind2-archive` (no license), `HVM3-Strict` (MIT),
  `bend-language-server` (MIT), `tree-sitter-bend` (NOASSERTION), `TSPL` (no
  license), `kindbook` (no license).
- `x.com` returned HTTP 402, so all Taelin thread content is from search-engine
  summaries rather than primary text. Lower confidence; not load-bearing for any
  conclusion above.
- HVM4's license absence was verified directly (`contents/LICENSE` 404s, GitHub
  license field null).
- No repo in either org was built or executed. All findings are from source
  reading and published claims.
