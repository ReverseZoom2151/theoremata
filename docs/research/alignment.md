# Aligning definitions and concepts across formal libraries: what is actually known

Research brief. Date: 2026-07-20. Scope: the ITP "concept alignment" literature, the
MMT/OMDoc theory-morphism line, proof-transfer inside a single system, and the recent
LLM-era statement-matching work. Focus on soundness, since alignment is the operation
that can silently corrupt a downstream transfer.

Every claim below carries a URL. Where I could only read an abstract or a secondary
description rather than the full text, I say so explicitly.

---

## 0. The one-paragraph answer

The literature has good, reproducible techniques for *proposing* alignments and
essentially no technique for *certifying* them. The best-established approach
(Gauthier and Kaliszyk) is explicitly statistical: it produces ranked candidate pairs,
and correctness is established by a human reading the list. The one place where the
literature genuinely handles wrongness well is not in the alignment step at all, it is
in the *consumption* step: HOL(y)Hammer uses cross-library alignments only to select
premises and to conjecture, then re-proves everything in the target system, so a wrong
alignment costs a failed proof attempt rather than a false theorem. That pattern,
alignment as a heuristic that feeds a checked step, is the only soundness story in the
field that actually holds up, and it is directly adoptable.

---

## 1. Gauthier and Kaliszyk: property-based concept matching

### 1.1 The technique

The core paper is "Matching Concepts across HOL Libraries" (CICM 2014),
https://arxiv.org/pdf/1405.3906 (also
https://link.springer.com/chapter/10.1007/978-3-319-08434-3_20). The method is
concrete enough to reimplement:

1. Export all theorems of each library to a common term representation.
2. Normalize each theorem into a *pattern*: abstract the constant of interest into a
   hole, and normalize the rest. Three normalization levels are reported: `norm_0`
   (identity, no abstraction), `norm_1` (abstract over the standard logical operators
   forall, exists, and, or, implies, not, equals, plus rewrites: remove implication,
   push negation inward, pull universal quantifiers out, distribute or over and, and
   associative-commutative normalization), and `norm_2` (`norm_1` plus normalization
   of a predefined list of application-specific AC constants).
3. A constant is then characterized by the *set of patterns it participates in*. Two
   constants in different libraries are scored by their shared patterns. Reported
   weightings: `w_0` uniform weight 1 per pattern; `w_1` weights a pattern by
   `1/card(C_set(lib,p))`, i.e. rare patterns count more. Scores: `score_0` and
   `score_1` sum the respective weights over common patterns; `score_2` divides
   `score_1` by `log(2 + n_1 * n_2)` to penalize very common constants.
4. Iterate: pick the top-scoring type-compatible pair, substitute both constants by a
   shared fresh symbol throughout both libraries (so downstream patterns now agree),
   re-score, repeat. This bootstrapping is the actual engine; it starts from the
   logical constants alone.

The later "Sharing HOL4 and HOL Light Proof Knowledge" (LPAR 2015,
https://arxiv.org/abs/1509.03527, full text
https://ar5iv.labs.arxiv.org/html/1509.03527) describes the same iterative scheme in
terms of shared properties (associativity, commutativity, nilpotence and so on),
weighted by rarity and by prevalence among already-matched constants, and reports an
engineering improvement: runtime dropped from about 1 hour to about 2 minutes.

The journal extension is "Aligning concepts across proof assistant libraries", J.
Symbolic Computation 90 (2019) 89-123,
https://www.sciencedirect.com/science/article/pii/S0747717118300348, which per its
abstract adds "evaluating match quality from environmental similarity" and a
"classification process with a disambiguation mechanism", and evaluates on six proof
assistant libraries with different logical foundations. I could not obtain the full
text (ScienceDirect returned 403 and the author-hosted PDF host no longer resolves),
so I have not verified its internal numbers and I do not report any.

### 1.2 What was evaluated, with numbers

From the CICM 2014 paper (https://arxiv.org/pdf/1405.3906):

- Corpora: HOL Light 11,501 theorems / 871 constants; HOL4 10,847 theorems / 1,962
  constants; Isabelle/HOL 18,914 theorems / 2,214 constants.
- Cross-library results, reported as (manually verified correct / total proposed):
  HOL Light to HOL4 177/203 constants and 16/22 types; HOL4 to Isabelle/HOL 109/131
  constants and 11/17 types; HOL Light to Isabelle/HOL 78/98 constants and 7/13 types.
  The widely quoted headline "398 pairs of isomorphic constants and types"
  (https://link.springer.com/chapter/10.1007/978-3-319-08434-3_20) is the sum of the
  verified numbers.
- Precision is therefore roughly 80 to 87 percent on constants and notably worse on
  types (7/13 in the hardest pair). That is the real number to keep in mind: about one
  in six proposed alignments was wrong.
- Ranking quality: the paper reports the rank at which the *first* incorrect match
  appears. The best configuration (`score_2` with iteration) reached rank 113 on the
  HOL Light / HOL4 pair before its first error. So the top of the list is clean and
  the tail is not.

From the LPAR 2015 paper (https://ar5iv.labs.arxiv.org/html/1509.03527): the improved
matcher gave "220 correct matches instead of the 178 previously obtained and 15 false
positives" instead of 32. Still a false-positive rate of roughly 6 percent, on the
easiest possible pair of libraries (two systems sharing the same logic).

Downstream use, and this is the important part: HOL(y)Hammer reproving the standard
libraries. HOL4 improved from 30 percent to 40 percent of problems solved when HOL
Light knowledge was added; HOL Light improved from 30.92 percent to 35.56 percent with
HOL4 knowledge. Per-theory the effect is very uneven: HOL Light's `real` theory reached
72.16 percent reproving from HOL4 dependencies, while HOL4's `real` theory got only
30.91 percent.

### 1.3 Failure modes and limitations (their own words and mine)

- **Structural rigidity.** The matcher "can only match objects that have the same
  structure". Concretely it cannot align HOL Light's primitive `complex` type with
  HOL4's representation as pairs of reals
  (https://ar5iv.labs.arxiv.org/html/1509.03527). This is exactly the class of case we
  care about: same mathematics, different encoding.
- **False negatives never measured.** The authors state that manually evaluating false
  negatives "is a very hard task and requires knowledge of the whole libraries" and did
  not attempt it (https://arxiv.org/pdf/1405.3906). So recall is unknown.
- **Verification was partial even for the reported positives.** Matches were "inspected
  manually", and they stopped verifying "at a point where a previously found error
  propagates". Type checking filters some false positives after the fact but is not a
  correctness check.
- **Weighting is ad hoc.** The authors note their weight functions ignore pattern size
  and the number of defined vs undefined constants.
- **Foundation-bound in the 2014 work.** "The approach has been tested on three provers
  based on higher-order logic ... it remains to be seen how smoothly does the approach
  extend to provers based on different logics." The 2019 journal version claims to
  extend to six libraries with different foundations; I could not verify how well.
- **Bootstrapping error propagation.** Because the iterative algorithm substitutes
  matched constants into both libraries, an early wrong match contaminates every
  subsequent score. This is my own reading of the algorithm as described, not a stated
  limitation, but it follows directly from step 4 above and is consistent with the
  authors' observation about errors propagating during manual inspection.

### 1.4 Requirements

Needs neither a trained model nor a shared foundation *in principle*, but it does need
a common term export format and, in the evaluated cases, a shared logic. It needs a
sizeable body of theorems per library, since the whole signal is statistical
co-occurrence of properties. It needs a human to read the ranked list.

---

## 2. The soundness-relevant design in HOL(y)Hammer: alignment as conjecture, not as fact

This is the most useful finding in this brief.

In "Sharing HOL4 and HOL Light Proof Knowledge"
(https://ar5iv.labs.arxiv.org/html/1509.03527) the cross-library knowledge is consumed
in scenarios where "the final predicted lemmas come from the initial library". That is,
the alignment is used to decide *which internal lemmas to try*, and the resulting proof
is reconstructed by ATP inside the target system against the target system's own
definitions. A wrong alignment yields a bad premise selection, which yields a failed
proof, which is a benign outcome.

The paper states this explicitly for the risk case: "If a constant contained in these
lemmas is matched inconsistently then each method would fail to reprove the lemmas,
preserving the coherence of the internal library". They also flag the unchecked
variant, where external theorems with no internal counterpart are suggested to the user
as an additional hypothesis to be proved separately, and note that the recursive
mechanism and the import functionality were not implemented.

So: the field does have a pattern where a wrong alignment is *caught*, and the catching
mechanism is not a check on the alignment itself. It is that the alignment is only ever
used to generate a claim that must independently pass the target system's kernel.

---

## 3. The MMT / OMDoc line: alignments as declared, human-curated data

### 3.1 The technique

"Alignment-based Translations Across Formal Systems Using Interface Theories" (Müller,
Rothgang, Liu, Rabe; PxTP 2017, EPTCS 262, pp. 77-93),
https://arxiv.org/abs/1712.01489, full text
https://ar5iv.labs.arxiv.org/html/1712.01489. The design: write *interface theories*
that specify a mathematical concept in a foundation-neutral way, then record
*alignments* saying how each library realizes that concept. Translation is then symbol
substitution through the interface plus dialect fixes (for instance, PVS `member(a,A)`
to HOL Light `IN`, plus translating function application conventions). Where a
foundational transformation is needed, MMT theory morphisms connect a detailed
implementation theory to a more abstract one.

There is an accompanying taxonomy paper, "Classification of Alignments Between Concepts
of Formal Mathematical Systems" (Müller, Gauthier, Kaliszyk, Kohlhase, Rabe, CICM 2017,
pp. 83-98), and "A Standard for Aligning Mathematical Concepts" (Kaliszyk, Kohlhase,
Müller, Rabe, CICM 2016 WiP, pp. 229-244), PDF at
https://kwarc.info/people/mkohlhase/papers/cicm17-alignments.pdf. Both exist and are
cited under those titles (https://dblp.org/db/conf/mkm/cicm2017.html,
https://kwarc.github.io/bibs/dmueller/). I could not extract the taxonomy's actual
categories: the kwarc PDF would not yield text through my fetch tool. Treat the
existence of a classification of alignment *kinds* (exact vs partial vs
directional) as established, the specific category names as unverified.

### 3.2 What was evaluated

- Alignments were produced **manually**: roughly 900 alignment declarations across HOL
  Light, PVS, Mizar and Coq, produced by two undergraduates in about 230 hours with
  minimal supervision. Reported per-system counts (bidirectional / unidirectional):
  HOL Light 177/23, PVS 159/48, Mizar 173/22, with a total in the region of 1,400
  alignments of which about 400 were usable for translation.
- Coverage is the binding constraint: only 33 concepts existed in all four systems,
  while 510 concepts appeared in exactly one system and therefore cannot be translated
  at all.
- The implementation is a prototype in MMT that translated a handful of expressions
  (set membership, function application, list filtering). It returns *partial*
  translations when no full translation exists, which is used as a discovery mechanism
  for missing alignments.

### 3.3 Failure modes

- **No correctness checking whatsoever.** Human verification of the alignments; no
  automated validation reported.
- **Directionality from partiality and subtyping.** Their own example: PVS requires the
  divisor to be nonzero, while HOL Light and Mizar define `x/0 = 0`, so the alignment
  of division is only usable in one direction. This is precisely the "different edge
  case at zero" failure the task asks about, and the literature's answer is to
  hand-annotate the alignment as unidirectional.
- **"Morally the same" is the actual standard.** The paper concedes that two aligned
  symbols "may use entirely different (possibly not even logically equivalent)
  definitions" and still be aligned. That is a deliberate design decision, not an
  oversight: alignments are a knowledge-management relation, not a semantic guarantee.

### 3.4 Requirements

No trained model. No shared foundation, that is the point of interface theories. But it
needs a large manual curation effort and an MMT-style export of every library involved
(see "Experiences from Exporting Major Proof Assistant Libraries",
https://arxiv.org/pdf/2005.03089 and
https://link.springer.com/article/10.1007/s10817-021-09604-0).

### 3.5 Theory morphism finding

"Automatically Finding Theory Morphisms for Knowledge Management" (Müller, Kohlhase,
Rabe, CICM 2018), https://link.springer.com/chapter/10.1007/978-3-319-96812-4_18,
searches for morphisms between theories, within and across libraries built on different
foundations, motivated by the fact that a morphism induces new theorems in the target
for free. This is structurally the most promising *checkable* form of alignment, since
a theory morphism is a mathematical object whose proof obligations can in principle be
discharged. I was blocked by Springer's paywall and could not verify its evaluation
numbers or whether the found morphisms are actually validated. Flagging this as the
single most relevant gap in my coverage.

---

## 4. Neural / embedding alignment

"JEFL: Joint Embedding of Formal Proof Libraries" (Wang and Kaliszyk, FroCoS 2021),
https://arxiv.org/abs/2107.10188, full text
https://ar5iv.labs.arxiv.org/html/2107.10188.

- Technique: parse terms into trees, emit token sequences by preorder / inorder /
  postorder traversal with configurable weights (defaults 0.33 each), feed to fastText
  (CBOW or skip-gram, hierarchical softmax or negative sampling), then align constants
  by nearest neighbour in the joint embedding space.
- Data: 18,723 HOL4 items plus 16,874 HOL Light items, 35,597 s-expressions total.
  Ground truth: 1,000 manually verified constant pairs from Gauthier's work.
- Results are weak in absolute terms. Best reported configuration (hierarchical
  softmax): Top-1 78, Top-3 161, Top-10 304, Top-20 419, out of the 1,000-pair ground
  truth. So under 8 percent Top-1 accuracy and about 42 percent Top-20. Tree-dump beat
  leaf-dump substantially (Top-10 204 vs 96), skip-gram beat CBOW modestly.
- Limitations: not integrated into any proof assistant, only two of the six intended
  libraries evaluated, constant-level only rather than term-level, and no verification
  of proposed matches at all. The paper's stated advantage over the symbolic matcher is
  online-servability and customizability, not accuracy.

Read honestly: as of this work, the embedding approach was worse than the 2014 symbolic
matcher at the same task. It is a retrieval front-end, not an aligner.

---

## 5. Proof transfer *within* one system: the part that is actually rigorous

Everything above is about *guessing* correspondences. There is a separate and much more
mature literature about *using* a correspondence once you have proved it, and that
literature is fully sound because the correspondence is a proved theorem, not an
assertion.

- **Isabelle Lifting and Transfer** (Huffman and Kunčar, CPP 2013),
  https://www21.in.tum.de/~kuncar/documents/huffman-kuncar-cpp2013.pdf. The user
  supplies a transfer relation between a raw type and an abstract type and proves
  transfer rules; the package then mechanically moves theorems across. Everything ends
  up kernel-checked. Related later work: "Transport via Partial Galois Connections and
  Equivalences", https://link.springer.com/chapter/10.1007/978-981-99-8311-7_11.
- **Trocq** (Cohen, Crance, Mahboubi), https://arxiv.org/abs/2310.14022, full text
  https://arxiv.org/html/2310.14022v2, journal version
  https://dl.acm.org/doi/10.1145/3737283, implementation
  https://github.com/rocq-community/trocq. A modular parametricity plugin for Rocq/Coq
  with a hierarchy of six parametricity classes running from a bare relation up to full
  equivalence, so the transfer only demands as much structure as the goal actually
  needs, and only invokes univalence when structurally required. The transferred proof
  is machine-checked: the abstraction theorems guarantee the translated term typechecks.
  Limitations as stated: single system (Rocq), the user must declare relational
  instances for library constants or the translation gets stuck, and it cannot infer
  relations between arbitrary type pairs.

The lesson for us: the notion of "these two definitions are the same" that is actually
usable for sound transfer is a *proved relation between them*, with a specific
strength (bare relation, function, equivalence). Trocq's class hierarchy is a good
model for how to represent partial alignment honestly.

---

## 6. Cross-system transfer that is sound by construction

- **OpenTheory** (Hurd), https://www.gilith.com/papers/stdlib.pdf,
  https://www.gilith.com/opentheory/. A proof-package format for the HOL family, with
  interfaces to HOL Light, HOL4 and ProofPower. Soundness comes from shipping the
  actual proof, replayed by the target kernel. Alignment is handled by a standard
  library acting as an agreed contract, i.e. the alignment problem is solved socially
  by everyone agreeing on one set of names, not algorithmically.
- **HOL Light into Isabelle/HOL** (Obua and Skalberg; Kaliszyk and Krauss; recently
  rebooted, https://members.loria.fr/STourret/papers/isabelle24translation.pdf, code in
  the Isabelle repository under `src/HOL/Import`). Proofs are recorded in the source
  and replayed in Isabelle's kernel, which the literature describes as "completely
  safe". Crucially, alignment enters through user-declared `type_maps` and `const_maps`
  that say "HOL Light's `X` is Isabelle's `Y`", and thereafter occurrences are
  substituted during import. The example the description gives is that HOL Light's
  `num` and Isabelle's `nat` have different binary representations. My reading, stated
  as inference rather than as a quote: proof replay is sound with respect to the
  *source* constants; the moment a `const_map` is used, the imported theorem is a
  statement about the *target* constants, and its truth rests on the unproved claim
  that the two constants agree. The replay does not check that. This is the exact
  soundness hole the task is asking about, sitting inside a system that is otherwise
  advertised as completely safe.
- **Logipedia / Dedukti** (Dowek, Thiré), https://arxiv.org/abs/2305.00064, full text
  https://ar5iv.labs.arxiv.org/html/2305.00064. Encode proofs in the lambda-Pi calculus
  modulo theory, then export to whichever system can accept them. Combines a logical
  framework with reverse mathematics, the idea being to find the weakest theory in which
  each proof lives so it can be exported to as many systems as possible. Demonstrated
  scope is modest and should be quoted honestly: the Matita arithmetic library up to
  Fermat's little theorem, around 300 lemmas, exported to HOL Light, Isabelle/HOL, Coq,
  Lean, PVS and Matita. Only a subset of proofs survives the trip to a weaker system.
  Note this is proof transport, not concept alignment: it moves the definitions along
  with the proofs rather than matching them to native ones.

---

## 7. LLM-era work

- **FormalAlign** (ICLR 2025), https://arxiv.org/abs/2410.10135,
  https://openreview.net/forum?id=B5RrIFMqbe. A trained cross-modal model scoring
  alignment between an informal statement and its formalization, using a dual loss over
  generation plus representational alignment. Reported Alignment-Selection Score 99.21
  vs GPT-4's 88.91 on FormL4-Basic, and 66.39 vs 64.34 on MiniF2F-Valid. The MiniF2F
  number is the honest one: on anything resembling real problems it is a coin flip plus
  16 points. It is a filter, not a decision procedure, and it needs a trained model.
- **ASSESS** (https://arxiv.org/abs/2509.22246): converts formal statements into
  operator trees and scores a "Transformation Tree Edit Distance", explicitly to get a
  *graded* similarity where proof-based checking gives nothing when the proof fails.
  Validated on EPLA, 1,247 expert-annotated formal statement pairs from miniF2F and
  ProofNet. Useful as a ranking signal; it makes no correctness claim.
- **Retrieval systems** such as Lean Finder (https://arxiv.org/html/2510.15940v1) and
  the mathlib4 semantic search engine (https://arxiv.org/pdf/2403.13310) solve the
  "find the candidate" half and say nothing about the "is it really the same" half.
- **The checkable idea in this line, and the one worth stealing.** The autoformalization
  evaluation community converged on: you cannot directly verify that a formal statement
  matches a natural-language one, but *you can verify that two formal statements are
  logically equivalent by stating the biconditional and proving it in the kernel*. This
  reduces alignment to a theorem. The same community also reports Lean equivalence
  checkers built on definitional equality (`isDefEq`) and congruence closure (`grind`)
  as cheaper decision procedures. Community discussion of the practicalities is at
  https://leanprover-community.github.io/archive/stream/270676-lean4/topic/Checking.20theorem.20equivalence.html.
  Caveat I want to be explicit about: I found this technique described consistently
  across several 2025 autoformalization papers via search, but I read it in summarized
  form rather than lifting it from one canonical paper, so treat the specific tactic
  names as indicative.

---

## 8. The soundness question, answered directly

**Does the literature handle the risk of a wrong alignment?**

Mostly no, and it is fairly open about this. The three dominant approaches handle it in
three different unsatisfying ways:

1. *Gauthier and Kaliszyk*: rank candidates, have a human read the list. Measured
   precision was around 80 to 87 percent in 2014 and improved to roughly 94 percent by
   2015, on the easiest possible library pair. Nothing in the pipeline detects a wrong
   match; a wrong match is found by a person or not at all.
2. *MMT interface theories*: alignments are hand-written declarations. The paper
   explicitly permits aligning symbols whose definitions are "not even logically
   equivalent" when they are "morally the same". Wrongness is not a failure mode of the
   system, it is outside the system's contract.
3. *Import via proof replay* (Isabelle's HOL Light importer, OpenTheory): the proof is
   genuinely rechecked, so the *proof* cannot be wrong. But the `const_map` layer is an
   unchecked assertion, and it is exactly where a subtle definitional disagreement would
   enter.

**Is alignment ever treated as a checked claim rather than a heuristic assertion?**

Within one foundation, yes, and this is the mature part of the field: Isabelle's
Transfer/Lifting and Trocq both require that the correspondence be a *proved* relation
before anything moves across it, and Trocq goes further by grading how strong that
relation needs to be. That is exactly "alignment as a checked claim". Across systems,
essentially no. The closest is the theory-morphism line, where a morphism carries proof
obligations by construction, but I could not verify that the automatic morphism finder
discharges them rather than merely proposing candidates.

**What would make an alignment refutable?**

Three mechanisms, in increasing strength, all of which exist in pieces in the
literature but are not assembled anywhere I found:

1. **Consequence testing.** From an alignment `f_A ~ f_B`, take known theorems about
   `f_A`, translate them, and attempt to prove or to *refute* them in library B. A
   counterexample finder (Nitpick, quickcheck, or plain numeric evaluation on the edge
   cases: zero, empty set, negative arguments, boundary of a branch cut) refutes the
   alignment cheaply. This is the natural inversion of the HOL(y)Hammer setup, which
   only uses the positive direction. I did not find a paper that does this deliberately
   as an alignment validator, which is a genuine gap and possibly a small contribution.
2. **Biconditional proof.** Where both libraries live in one system or one has been
   imported, state `forall x, f_A x = f_B x` (or the iff for predicates) and prove it in
   the kernel. This turns the alignment into a theorem and is the strongest form. It is
   what the autoformalization evaluation community already does for statement pairs.
3. **Graded alignment with an explicit strength.** Do not record a boolean "same".
   Record what is actually proved: equal everywhere, equal on a stated domain, related
   by a stated function, or merely correlated. Trocq's six-class hierarchy and the
   MMT unidirectional-alignment annotation are both instances of this idea. The
   division-by-zero example is the canonical case: the honest record is not "same" but
   "equal when the divisor is nonzero".

**The honest summary.** The literature does not solve the soundness problem. It
sidesteps it by only using alignments in places where being wrong is cheap. Where
transfer is genuinely relied upon (Isabelle's importer, OpenTheory), the trust is
relocated to a small hand-written mapping table that nobody checks mechanically, and
that table is trusted precisely because it is small and human-reviewed. That is a
reasonable engineering answer for a mapping of 200 entries maintained by experts. It
does not scale, and it is not what an automated harness should imitate.

---

## 9. Different foundations: is transfer meaningful at all?

Briefly, and the answer is nuanced.

- **Proof transport across foundations is real but limited.** Logipedia demonstrates it
  concretely: about 300 arithmetic lemmas moved from Matita's Calculus of Constructions
  into HOL Light, Isabelle/HOL, Coq, Lean, PVS and Matita
  (https://ar5iv.labs.arxiv.org/html/2305.00064). The mechanism is to express the proof
  in the weakest sufficient theory, so it only works for the fragment that does not use
  the source's extra strength. Only a subset of the proofs survives the trip.
- **Conservativity results tell you when it is even possible in principle.** Dependent
  type theory without universes is conservative over Heyting Arithmetic, and with one
  universe level over higher-order Heyting Arithmetic, but proof-relevant
  interpretations prove strictly more second-order arithmetic formulas while agreeing on
  first-order ones (https://arxiv.org/abs/2308.15288). Practical reading: for
  first-order arithmetic content, cross-foundation transfer is meaningful; for
  higher-order and proof-relevant content, the foundations genuinely disagree about what
  is provable and "the same theorem" is not well defined.
- **Alignment specifically degrades across foundations.** The 2014 matcher's precision
  was already worse on the HOL Light / Isabelle/HOL pair (78/98) than on the HOL Light /
  HOL4 pair (177/203), and those share a logic. The MMT effort, which explicitly spans
  HOL Light, PVS, Mizar and Coq, found only 33 concepts common to all four
  (https://ar5iv.labs.arxiv.org/html/1712.01489). Set-theoretic Mizar and type-theoretic
  Coq mostly do not talk about the same objects in a way that a symbol-level alignment
  can capture.
- **Practical conclusion:** cross-foundation alignment is useful for *retrieval and
  conjecturing* (what does the other library know about this) and mostly not usable for
  *transfer* (import the theorem and rely on it). Within a foundation, and especially
  within one system, transfer can be made fully rigorous, as Trocq and Isabelle's
  Transfer show.

---

## 10. What a soundness-first harness could realistically adopt

Ordered by ratio of value to risk.

1. **Adopt the HOL(y)Hammer consumption pattern as an invariant, not a feature.** Any
   cross-library alignment we compute may influence *premise selection, retrieval,
   conjecture generation and search ordering*, and may never be used to introduce a
   fact. Concretely: an aligned foreign theorem enters as a *goal to reprove locally*
   or as a hint, never as a hypothesis. Under this rule a wrong alignment costs compute,
   not correctness. Evidence that this works: 30 to 40 percent improvement on HOL4
   reproving with a matcher that had a 6 percent false-positive rate
   (https://ar5iv.labs.arxiv.org/html/1509.03527).
2. **Represent alignments as graded, refutable claims with an evidence field.** Never a
   boolean. Record: proposed-by (matcher, embedding, LLM, human), strength (proved
   equal, equal on domain D, related by r, unverified), and refutation status. Model the
   strength lattice on Trocq's parametricity classes
   (https://arxiv.org/html/2310.14022v2) and on the MMT bidirectional /
   unidirectional distinction (https://ar5iv.labs.arxiv.org/html/1712.01489).
3. **Build the refutation loop that the literature is missing.** For every proposed
   alignment, auto-generate edge-case probes (zero, one, empty, negative, boundary,
   undefined-at) plus a batch of translated known theorems, and try to *disprove* the
   alignment with counterexample search and cheap evaluation. An alignment that survives
   N probes gets a higher grade; one that fails a probe is marked refuted and the
   refuting instance is stored. This is cheap, it is the natural complement to the
   existing positive-direction work, and I found no paper doing it as a validator.
4. **Where both sides live in one system, promote alignments to theorems.** State the
   equality or the biconditional and discharge it with the existing prover stack; on
   success the alignment is kernel-backed and can be used for real transfer. This is
   the autoformalization community's equivalence-checking idea applied to library
   alignment.
5. **Use the property-pattern matcher, not embeddings, as the proposer.** The 2014
   symbolic method is reimplementable in a few hundred lines given a term export, has
   published precision numbers, and outperformed the neural approach on the same task
   (https://arxiv.org/pdf/1405.3906 vs
   https://ar5iv.labs.arxiv.org/html/2107.10188). Use an LLM or an embedding index only
   as an additional candidate generator feeding the same refutation loop, never as an
   adjudicator. FormalAlign's 66 percent on MiniF2F-Valid
   (https://arxiv.org/abs/2410.10135) is a reasonable estimate of what to expect from a
   trained alignment scorer on non-toy inputs.
6. **Do not attempt cross-foundation transfer.** Treat cross-foundation alignment as a
   retrieval and inspiration channel only. The coverage numbers (33 concepts common to
   four systems) and the conservativity results say the payoff is small and the risk
   is high.

## 11. Open questions I could not close

- The JSC 2019 journal version's disambiguation mechanism and its six-library numbers.
  Paywalled, author copy dead. This is the most current symbolic aligner and I could
  only read its abstract (https://www.sciencedirect.com/science/article/pii/S0747717118300348).
- Whether the automatic theory-morphism finder discharges morphism proof obligations or
  only proposes candidates (https://link.springer.com/chapter/10.1007/978-3-319-96812-4_18).
  If it discharges them, it is the closest thing in the literature to checked
  cross-library alignment and deserves a follow-up.
- The exact taxonomy in the alignment classification papers
  (https://kwarc.info/people/mkohlhase/papers/cicm17-alignments.pdf). Worth recovering,
  since we would otherwise reinvent their category names.
- Whether anyone has published the refutation loop in item 3 above. My searches did not
  find it, but absence of evidence here is weak evidence.
