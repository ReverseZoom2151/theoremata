# Consolidated adopt backlog

Every adopt / build / recommendation item recorded across all 138 mining documents,
extracted and status-verified against the codebase. **1,020 rows.**

Per-slice detail lives in [`docs/backlog/`](backlog/):

| Slice | Source | Rows |
|-------|--------|-----:|
| [`backlog-rm-new.md`](backlog/backlog-rm-new.md) | `docs/resource-mining/new/` (50 docs) | 454 |
| [`backlog-atp.md`](backlog/backlog-atp.md) | `atp-mining`, `agentic-patterns-mining`, `formal-systems` (30) | 250 |
| [`backlog-papers.md`](backlog/backlog-papers.md) | `docs/paper-mining/` (31) | 166 |
| [`backlog-rm-top.md`](backlog/backlog-rm-top.md) | `docs/resource-mining/` top level (20) | 150 |

## Why this file exists

Mining outran building by roughly eight to one, and nothing tracked the gap. The
failure mode that prompted this: the `peteroupc` notes were skipped on the reasoning
that their error bounds would matter *if* the minimax path ever became real work. That
condition was already true when the skip was written, because `poly_minimax` was
already a shipping certificate kind. Nobody noticed, because nothing was watching.

A recorded revisit trigger with no owner is not a plan. This file is the register.

## Status distribution

| Status | Count | Meaning |
|--------|------:|---------|
| IMPLEMENTED | 406 | Verified present, with a cited file reference |
| NOT-BUILT | 238 | No trace after grepping at least two plausible names |
| PARTIAL | 230 | Related machinery exists; the specific mechanism does not |
| SKIPPED | 102 | The mining doc decided against it, with a reason |
| UNCERTAIN | 27 | Verification inconclusive; what was checked is recorded |
| BLOCKED | 16 | Needs a GPU, a trained model, a corpus, or a licence we lack |

Roughly 1 in 5 rows is a soundness item. Those rank above everything else here,
because a capability gap costs us a feature and a soundness gap costs us the only
claim the system makes.

## Tier 0: the gate does not deliver what the docs promise

These are the highest-priority items in the entire backlog. Each is a documented
guarantee the code does not keep, found by checking `docs/formal-systems/` against
the implementation. The pattern was established earlier the same day, when live Rocq
turned out never to have been able to pass statement preservation.

1. **Isabelle live oracle audit is a no-op.** Returns `within_whitelist: true`
   unconditionally. The oracle set is the entire point of the Isabelle layer-2 gate.
2. **Metamath `audit_axioms` is a fail-open stub**, also returning
   `within_whitelist: true` unconditionally (`external.rs`). The backend presents as
   having an axiom gate and has none.
3. **Metamath `kernel_recheck` trusts the exit code** of a binary that the same file
   documents as returning 0 on a failed verify. `compile` correctly uses the stdout
   sentinel; the recheck does not.
4. **The Metamath secondary checker overwrites the primary verdict**, which the spec
   explicitly forbids. Recorded as a one-line fix.
5. **Agda and Metamath emit no provenance** (`source_sha256`, `dependency_sha256`,
   `limits`) despite the spec naming it the common contract.
6. **The 3+1-layer gate is really a 3-layer gate for three of six systems.**
7. **Tier-0 hypothesis and vacuity gates are permanently observational**: no shipped
   backend populates `designated_inputs` or `satisfiability_witness`.
8. **`verify_lean_output` stubs LeanParanoia hardening**, so external-prover Lean is
   accepted on compile, axioms, and statement alone.
9. **Candle statement preservation can never pass** with current callers, because no
   Candle canonical statement carries a proposition. Fail-closed, so not unsound, but
   the docs present Candle as a live certifying backend.
10. **Three advertised evidence types are never emitted**
    (`external_prover_artifact`, `external_producer_checked`, `repair_loop`), so the
    audit trail `TRUST_BOUNDARIES.md` describes does not exist in the database.

## Tier 1: fired revisit triggers

Twelve of 62 recorded triggers have fired and are unmet. The two that change what is
possible today:

- **A Lean toolchain is installed.** `lean 4.32.0`, `lake`, and `leanchecker` resolve
  at `~/.elan/bin/`. Every ACCEPT fixture was gated on "must build under its pinned
  toolchain", blocked on "no Lean on this box". That blocker is gone, and four
  adversarial corpora currently assert gate behaviour on artifacts nobody has
  compiled. The dev-environment note claiming otherwise is stale.
- **`poly_minimax` is live**, which is what made the `peteroupc` bounds worth mining.
  Addressed; recorded here as the worked example of the failure this file prevents.

Others: miniF2F is registered across all four splits with no exclusion list for the
eight known mis-formalised test problems; per-item toolchain metadata is missing from
`loaders.py`, blocking honest scoring for three loaders; and three vendored repos
still carry their `AGENTS.md` / `CLAUDE.md` intact, one of which ships a skill that
runs `gh issue close` against a third party.

## Tier 2: built but never exercised

Machinery that exists, is tested, and has no production caller. Invisible to a normal
status check, which is why it is called out separately.

- **No production caller at all:** `critic_scorer.rs`, `preference_pairs.rs`,
  `best_first.rs::dpo_pairs`, `optimize.rs`, `proof_pool.rs`, half of
  `distance_critic.rs`.
- **Mock-only:** the flywheel, GRPO, the graded reward family, the learned selector,
  the round-trip CC judge, and the marking-scheme generator. That last one matters
  more than it looks: PROOFGRADER's entire result was that scheme quality drives
  grader accuracy, and the offline path exercises the plumbing while proving none of
  the claim.
- **Corpus-missing:** `lean_corpus.py`, `retrieval_eval.py`.

The honest summary: the search-side and verification-side Rust is real and exercised.
Nearly everything requiring a trained model is a correct, well-tested shell.

## Tier 3: highest-value capability work

1. Feed a real `CriticScorer` into the driver. The seam is built with zero callers,
   and InternLM attributes 59.4 to 65.9 miniF2F to exactly this wire.
2. Swap the lemma library's exact dedup for the existing subsumption deduper. A
   one-line injection that turns a near-duplicate accumulator into a real library.
3. Port SafeVerify's `Environment.replay`. MIT with attribution already upstream, so
   a port rather than a rebuild, and the strongest anti-tamper mechanism found in any
   vendored source.
4. An MCP client. We ship a server; consuming any external MCP tool is blocked.
5. `cert_log.KINDS` recognizes 5 kinds while 14 checkers ship: two unreconciled
   registries.
6. `fp_rounding` and `fp_error_bound`, the highest-confidence new certificate kinds
   in the ATP corpus.

## Licensing exposure

Nineteen mined sources carry **no licence at all**, which grants fewer rights than
GPL. Four are load-bearing for shipped code (`decomposition_admission.rs`,
`declaration_lookup.rs`, `infotree.rs`, and five Python modules). All were
reimplemented rather than copied, which is the correct posture, but the provenance
belongs in those module headers before any release. Separately, fifteen mined repos
have no licence recorded in their mining doc, and nine of those are already in the
benchmark registry.

## How to keep this honest

Every row cites the source doc and, where implemented, a file reference. A status
here is a claim that was grep-verified once, at extraction time, and will rot. When
closing an item, update its row rather than deleting it, so the register records what
was decided and not merely what remains.
