# Plan: mathematics at scale

A build plan for the ingest-to-replay pipeline, grounded in a literature review
(`docs/research/alignment.md`, `docs/research/repair.md`) and in what this codebase
verifiably does today.

The pipeline being targeted:

    Source ingestion -> Statement normalization -> Typed dependency graph ->
    Proof-obligation generation -> Backend routing -> Verification ->
    Provenance tracking -> Replay

## Four findings that shape everything below

**1. Our cache can serve a stale green.** `VerificationCacheKey` carries the import
NAMES (`["Mathlib"]`) and `checker_identity` carries binary PATHS, the runner tag,
`lean_project`, and `expected_toolchain`. Nothing hashes the Mathlib revision or the
lake-manifest. Update Mathlib in place at the same path under the same Lean, and the
key is byte-identical while the environment has changed, so a cached verdict is
reused against a different library. `THEOREMATA_CHECKER_CACHE_EPOCH` exists as a
manual escape hatch, and manual invalidation is not detection.

**2. Nobody in the field has solved staleness detection, and stale greens are the
normal state.** Real practice is brute force: Mathlib rebuilds Mathlib, the AFP
rebuilds 21,000+ theories per release. No system treats "verified against environment
E" as first-class provenance that a later change can invalidate. The closest is
snapshot pinning plus cross-version CI. This is not a gap we are behind on; it is a
gap nobody has filled.

**3. Alignment is never certified, and refutation appears unpublished.** Proposing
alignments is well studied and works acceptably within one logic (roughly 80 to 94
percent precision on HOL4 and HOL Light; recall never measured). Certifying them is
not done. MMT is explicit that aligned symbols may have logically inequivalent
definitions. Where transfer is genuinely relied on, soundness comes from replaying
proofs in the target kernel, with the alignment table itself hand-written and trusted
because it is small. The canonical divergence is real: PVS requires a nonzero divisor
while HOL Light and Mizar define `x/0 = 0`, so an alignment recorded as "same" is
simply false. A search for anyone attempting to REFUTE proposed alignments found
nothing.

**4. Certificates do not rot; statements do.** A self-contained rational certificate
replays forever, and a proof term rots only if a referenced constant vanishes or
changes type, whereas a tactic script rots on any lemma-name or tactic-behaviour
drift. But a certificate protects the PROOF, not the MEANING of its statement. The
concrete case is miniF2F `algebra_5778`, which became genuinely unprovable when
Mathlib routed nth roots through `rpow`: informal statement unchanged, formal text
unchanged, mathematics changed. This framing does not appear in the literature and is
the load-bearing idea in Phase 1.

## Design principle

Everything below follows one rule, taken from how HOL(y)Hammer survives a 6 percent
false-alignment rate: **put unchecked claims only where being wrong is cheap, and
make the expensive path re-derive its own truth.** An alignment may steer retrieval
and suggest obligations. It may never license a transfer that the target kernel did
not itself accept.

## Phase 0: make provenance honest (prerequisite for everything)

Nothing downstream is trustworthy while a cached verdict can outlive its environment.

- **0.1 Resolved-environment fingerprint.** Extend the cache identity from requested
  import names to the RESOLVED dependency set: the lake-manifest content hash (or
  per-package rev), the toolchain, and the resolved package paths. Key on what was
  actually elaborated against, not what was asked for.
- **0.2 Pin the elaborated statement type, not just its source text.** Store the
  statement's elaborated type alongside the verdict. This is what makes finding 4
  actionable, and it is the input to Phase 1.
- **0.3 Emit the three declared-but-never-emitted evidence types**
  (`external_prover_artifact`, `external_producer_checked`, `repair_loop`), so the
  audit trail `TRUST_BOUNDARIES.md` documents actually exists.
- **0.4 Record the known limit of layer 2.** Our axiom audit goes through
  `#print axioms`, which is backed by `Lean.collectAxioms`, which misses axioms
  referenced by axiom TYPES (lean4 issue 8840). Document it at the gate rather than
  implying the closure is complete.

Cost: small. Value: without it, every green below is provisional.

## Phase 1: staleness with a rename-versus-mathematics discriminator

The research found one checkable operation that nobody has named as a technique:
**re-elaborate the pinned statement under the new environment and compare the
resulting type.** That single operation separates the two failure modes the field
currently conflates.

- **1.1 Detect.** A verified result is stale when its resolved-environment
  fingerprint (0.1) no longer matches the current environment. Sound and coarse.
- **1.2 Discriminate.** For each stale result, re-elaborate the pinned statement.
  - Type unchanged: the mathematics is intact. A failure here is a rename, a moved
    lemma, or tactic drift, and it is a REPAIR task.
  - Type changed or fails to elaborate: the mathematics moved under us. This is not a
    repair task, it is a re-proof or a re-formalization task, and the old green must
    be withdrawn rather than patched.
  - Cannot re-elaborate the environment at all: unknown, and unknown is not clean.
- **1.3 Classify by artifact type.** A cert-log certificate survives environment
  drift (finding 4), so it needs only its statement re-checked, not its proof. A
  tactic script needs both. Route accordingly, because this is where most of the
  saved work is.
- **1.4 Sweep.** A CLI verb that walks the DAG, marks stale nodes, and reports the
  three-way split. Not a re-prove loop yet, just an honest census.

The valuable output is 1.2. "Your theorem is now false or unproved" and "your script
needs a rename" are different facts, and everything today reports them identically.

## Phase 2: refutable alignment

Alignment is the single largest missing piece for connecting corpora, and adopting it
naively would inject an unchecked assertion into the foundation of the dependency
graph. So it gets built inverted.

- **2.1 Propose.** Reimplement the property-pattern matcher: normalize theorems into
  patterns, score constants by shared rare patterns, iteratively substitute the
  top type-compatible pair and re-score. It is reimplementable from the paper and
  outperformed the neural approach on the same task.
- **2.2 Grade, never assert.** No boolean "same". Record the strength actually
  established, modeled on Trocq's parametricity classes and MMT's unidirectional
  annotations. `x/0` is the test case: the honest record is "equal on a stated
  domain", and a schema that cannot express that is the wrong schema.
- **2.3 REFUTE.** The part nobody appears to have built, and the part we are unusually
  well equipped for. For each proposed alignment, auto-generate edge-case probes
  (zero, empty, negative, boundary, the divergence classes the literature already
  documents) plus translated known theorems, and try to DISPROVE the alignment with
  the existing falsifier, witness search, and exact-arithmetic recheck. A refuted
  alignment stores its counterexample witness and is never usable again.
  - An alignment that survives refutation is UNREFUTED, not verified. Say so in the
    vocabulary, the same way the search stages say `formally_verified: false`.
- **2.4 Consume it only where wrong is cheap.** Alignments feed premise retrieval and
  obligation generation. They never license a transfer. Anything crossing a corpus
  boundary is re-proved in the target kernel, which is exactly why HOL(y)Hammer
  tolerated a 6 percent false-positive rate without unsoundness.

Explicitly out of scope: cross-FOUNDATION transfer. Sound transfer exists only within
a foundation, where the correspondence is a proved relation. Cross-foundation efforts
are real but small (Logipedia moved roughly 300 arithmetic lemmas; only 33 concepts
were common to four systems). We should align within a foundation and treat
cross-foundation as retrieval-only.

## Phase 3: the harvester, with attribution

Only after 0 and 1. A harvester on unattributed verdicts manufactures confidence at
volume, which is worse than no harvester.

- **3.1 Verdict attribution.** No accept or reject recorded without the layer that
  decided it. We already have one instance of this: a vacuity fixture that would have
  gone green because the gate rejected it on a `sorry` before reaching the vacuity
  reasoning it existed to test, now tagged `reject_is_underdetermined`. Generalize it.
- **3.2 Enumerate consequences** off the dependency graph, with obligations entering
  unproved (`to_obligations` already enforces this).
- **3.3 Bounded-case search and counterexample generation.** Largely built: the
  falsifier, witness search, and the Wolfram oracle with independent exact recheck.
- **3.4 Measure what it stopped us believing.** Refutations, vacuity catches, and
  mis-formalization detections, tracked as first-class output rather than as failures.
  Until this week every registered corpus was accept-shaped, so we could measure what
  we proved and not what we correctly refused.

## Phase 4: ingestion at paper scale

Deliberately last. Ingesting thousands of papers into a graph whose alignments are
unrefuted and whose greens can go stale produces a large, confident, wrong artifact.

- 4.1 Extract dependencies from existing libraries (`decl_index` dumps declarations
  today but builds no dependency graph from them).
- 4.2 Paper ingestion beyond leanblueprint `content.tex`.
- 4.3 Normalize ingested statements through the existing canonicalization.

## What we are deliberately not building, and why

- **Cross-foundation transfer.** Not soundly possible today. Retrieval only.
- **Automatic repair that rewrites statements.** The dominant failure mode of LLM
  proof repair is weakening the statement until it compiles. Repair may rewrite a
  script; it may never touch the statement.
- **A trained alignment model.** The neural approach lost to the pattern matcher on
  the same task. Revisit only if the matcher's recall proves inadequate.
- **Trusting `Lean.collectAxioms` as a complete closure.** Documented as a limit
  rather than papered over.

## Honest risks

- Phase 2.3 is genuinely novel, which also means unvalidated. If refutation catches
  nothing on real alignment sets, it is expensive theatre; the counter is that its
  probes are cheap and its failure is silence rather than a false green.
- Phase 1.2 assumes statements are re-elaborable in isolation. Where a statement
  depends on local definitions that also moved, the discriminator degrades to
  unknown. Unknown must stay distinct from clean.
- The whole plan raises the cost of a green. That is the intent, and it will make
  throughput look worse before it makes it real.

## Sequencing

0 before everything. 1 next, because it is the cheapest thing with no prior art and
it protects every result already in the store. 2 is the largest and most interesting.
3 depends on 0 and 1. 4 last, and only once the graph it fills is trustworthy.
