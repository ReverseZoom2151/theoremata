# LLM-prover arrivals: TheoremLlama, llm-driven-proof-search, Newclid_Transformer, AutoMathText-2.5, FormaRL

Status: **one repo of the five is worth reading, and it yields five small,
concrete mechanisms rather than one big one.** Two of the five
(`Newclid_Transformer`, `FormaRL`) are already fully mined in existing docs and
were re-verified rather than re-read. Two (`TheoremLlama`, `AutoMathText-2.5`)
are shells that ship no mechanism at all. The remaining one,
`llm-driven-proof-search`, is a 52k-line Rust harness that overlaps our
architecture heavily; the overlap is itself the finding, and the non-overlapping
parts are narrow but real.

Scope of the read. Full source read: `TheoremLlama-main` (5 files, all of them),
`AutoMathText-2.5-main` (4 files, all of them),
`llm-driven-proof-search-develop/crates/proofsearch-core/src/` (targeted full
reads of `repair_chain.rs`, `mutations.rs`, `policy.rs`, `analyzer.rs`,
`mcip.rs`, `literature_lineage.rs`, `publication_review.rs`,
`dependency_manifest.rs`, `hashing.rs`, `orchestrator/budget.rs`,
`orchestrator/scheduler.rs`, `orchestrator/module_closure.rs`,
`orchestrator/context.rs`, `models/reward.rs`, `models/action.rs`, plus the
benchmark ladder README). Verification-only pass:
`Newclid_Transformer-main` and `FormaRL-main` against the existing docs. On our
side: `components/reason/orchestration/context_assembly.rs`,
`components/reason/search/best_first.rs`, `components/graph/scheduler.rs`,
`components/train/python/theoremata_tools/reward.py` and `format_filters.py`,
`components/verify/python/theoremata_tools/proof_telemetry.py` and
`formal_lint.py`.

Nothing was built, installed, or executed. No git, no cargo.

## Already covered: do not re-mine

**`Newclid_Transformer-main`** is documented in full at
`docs/resource-mining/new/newclid.md` (141 lines), down to the
`translate.py MAP_SYMBOL` token table, the degeneracy collapses inside
`translate_constrained_to_constructive`, the `check_valid_args()` pre-filter, and
`priority_beam_search` + `brevity_penalty`. That doc's conclusions hold on
re-read. Two things to add to the record and nothing more:

- The `brevity_penalty(length) = ((length+5)/6)**0.6` scoring in
  `src/alphageo/inference.py` is **the same length normalization we already
  run**, and ours is strictly more general:
  `components/reason/search/best_first.rs` implements
  `length_normalized_score(cum_logprob, depth, alpha) = Σ log p / L^alpha` with a
  configurable exponent, and `hybrid_search.rs::multi_alpha_union` sweeps alpha
  rather than pinning it at 0.6. Nothing to port.
- Latent bug in `src/alphageo/alphageometry.py`, `BeamQueue::add`:
  `min(enumerate(self.queue), key=lambda x: x[1])` compares `(val, node)` tuples,
  so any tie on `val` falls through to comparing `node`, whose first element is a
  solver proof-state object. That raises `TypeError` rather than evicting. Ours
  keys the frontier on score with an explicit id tiebreak; worth keeping that
  way.

**`FormaRL-main`** is covered in `docs/resource-mining/new/2026-07-new-arrivals.md`:
adopt item 9 (conjunctive zero-partial-credit reward, `beta_1 = beta_2 = 0.0,
beta_3 = 4.0`, with the reward-hacking scar tissue that justifies it), the
"reports zero benchmark numbers, three real bugs" entry under empirical claims,
and the MIT licence line. Skipped, as instructed. We already carry the FormaRL
SC/CC autoformalization reward rule.

## TheoremLlama-main

**What it is.** Not the paper. The paper (arXiv 2407.03203) is about NL-FL
bootstrapping, block training, and curriculum sorting; **none of that is in this
repo**. What ships is three Python files totalling 462 lines that do batched
sampling from a HuggingFace checkpoint against MiniF2F, plus two JSON eval sets.
The README says so honestly: "The code will be available soon" (last update
10 Oct 2024).

**Licence, size, activity.** No `LICENSE` file, no licence statement anywhere.
Treat as unlicensed: ideas only, no code copying. 9 files. Last activity Oct 2024
per the README's own changelog. Abandoned.

**Mechanisms, such as they are.**

- `Prove_writer.py::_formulate_prompt_llama3Instruct` builds a 14-shot prompt by
  `random.sample` over an example pool whose `FL` field is the
  `Commented_proof` variant, i.e. the Lean proof with natural-language comments
  interleaved. That interleaving is the paper's "NL-FL bootstrapping" and it is
  the only real idea visible in the code. We do the equivalent and more:
  `components/reason/orchestration/context_assembly.rs` composes
  `Concat(System, Memory, Tools, Retrieval, Query)` under a token budget with
  deterministic relevance ordering, rather than uniform random sampling from a
  fixed pool.
- `generate_proof_singleThm(variable_tempreature=0.6, temperature=0.9)` draws a
  fresh `random.uniform(0.6, 0.9)` temperature per batch instead of a fixed one.
  A cheap diversity trick for pass@k sampling. We do not do exactly this, but we
  get diversity from `multi_alpha_union` and the portfolio
  (`components/reason/proving/portfolio.rs`, `formalize_portfolio.rs`), which are
  principled rather than a jittered sampler. Not worth adding.

**Real defects, for the record.** `eval_MiniF2F.py` is named an evaluation script
and **contains no verifier**: it generates 32 proofs per theorem and writes JSON.
Nothing ever calls Lean. Any "MiniF2F result" produced by running this file as
shipped is a generation count, not a pass rate. Separately, line 56 writes to
`f"{SAVE_PATH}/MiniF2F_{data_ls_to_test}_Lean4_proof.json"`, interpolating the
entire list of test records into the filename; and the prompt template emits
`<|start_header_id>` (missing pipe) for the user header, so the user turn is
malformed for every sample.

**Verdict: SKIP.** The repo does not contain the method the paper is cited for,
the one prompt idea in it (comment-interleaved FL examples) is a weaker version
of what our context assembler already does, and the eval script does not
evaluate. Its checkpoints and the OBT dataset live on HuggingFace and could be
mined separately if we ever want an NL-annotated Lean corpus, but that is a
dataset question, not a code question.

## AutoMathText-2.5-main

**What it is.** Four files: a README, a GitHub Pages landing page, a `.nojekyll`
marker, and a licence. No pipeline, no scoring code, no prompts, no data.

**Licence.** A custom "AutoMathText Data Agreement for Model Training". Not
open source; a restrictive data EULA. Read it before touching the HuggingFace
dataset.

**Verdict: SKIP, and already documented.**
`docs/resource-mining/AutoMathText-and-goldbach-collatz.md` covers this repo in
full, including the correct conclusion that the reusable IP (the zero-shot
generative-classifier LM-Score) is in arXiv 2402.07625, not here. Re-reading the
landing page in July 2026 adds only that the 2.5 release claims 2T+ tokens /
7.11 TB / 50+ sources and a four-stage pipeline
(`Deduplicate -> Detect Contamination -> Clean Text -> Quality Score`) described
in prose only. No new mechanism.

## llm-driven-proof-search-develop

**What it is.** A Rust workspace (`proofsearch-core` ~20.5k lines +
`proofsearch-mcp`) implementing an MCP-served, SQLite-backed proof-search
orchestrator over Lean: obligations in a DAG, episodes, budgeted attempts,
verified-module assembly, and a heavy provenance/export layer. It is
architecturally the closest thing to us in the whole `resources/` tree, which is
why most of it is redundant. It is also partially mined already: adopt items 13
(decomposition admission checks) and 14 (four-valued declaration lookup) in
`2026-07-new-arrivals.md` come from this repo, as does the import-manifest attack
vector at line 72 and the empirical-claims warning at line 236. This document
covers what that pass did not reach.

**Licence: unlicensed. Clean-room constraint applies.** The README carries an
MIT badge whose link 404s, there is no `LICENSE` file, no copyright holder, and
no `license` field in `Cargo.toml`. This was already recorded in
`2026-07-new-arrivals.md` and is re-confirmed. **We may read this repo for ideas
and must not copy code from it.** Every mechanism below is described as a design,
not a diff. Not GPL, but the practical constraint is the same or stronger.

**POSSIBLE INJECTION.** `CLAUDE.md` at the repo root is 100% text addressed to an
AI agent ("Operating Doctrine", "Trust the model to propose", instructions to run
build/test/lint before committing). Its content is benign engineering advice and
none of it was followed. It is still agent-directed instruction text sitting
inside untrusted vendored data: if any part of this repo is ever ingested into a
retrieval index or handed to a model, strip `CLAUDE.md`,
`docs/llm_native_design_practices.md`, and the `docs/kits/` tree first. Flagged
previously at `2026-07-new-arrivals.md` line 271; restated here because the file
is still present.

### What we already have (the bulk of it)

- **Proof-profile analysis.** `analyzer.rs::analyze_proof` tokenizes Lean source
  against a ~70-entry `KNOWN_TACTICS` list, derives `uses_induction` /
  `uses_contradiction` / `constructs_witness` / `has_intermediate_claims`,
  counts branches, and assigns an `automation_level` over
  heavy/moderate/light tiers. We have this in
  `components/verify/python/theoremata_tools/proof_telemetry.py`
  (`tactic_histogram`, `analyze`, `_ranking_score`), and **ours is better on the
  one point that matters**: `mask_lean(text, mask_strings=True)` blanks comments
  and string literals before tokenizing, so a tactic name inside a comment cannot
  inflate the profile. `analyzer.rs::tokens` scans raw source and will count
  `-- just use nlinarith here` as heavy automation.
- **Banning `native_decide` and friends.** Their `lean/module.rs`
  `PROHIBITED_ANYWHERE_TOKENS` is a deterministic pre-filter. We have
  `formal_lint.py` (which bans `native_decide` explicitly, with the reason
  "native_decide trusts the compiler") plus `formal_source_scan.py` and the axiom
  audit. Ours is a gate layer; theirs is a policy pre-filter. No gap.
- **Repair chains.** `repair_chain.rs::assemble_repair_chain` reconstructs an
  ordered repair trajectory from recorded attempts, refusing to emit one unless
  some attempt before the last actually failed. We have
  `components/reason/proving/repair.rs` and `retry.rs`, error-keyed retrieval,
  and DPO pair mining that already consumes failure-then-success sequences.
  Nothing new, though the "a first-try success is not a repair chain" guard is a
  sensible invariant to check we hold.
- **Restriction profiles as a non-kernel gate.** `policy.rs::check_restrictions`
  scores a submitted proof against a declared `RestrictionProfile` (forbidden /
  allowed tactics, dependency allowlists, `max_tactic_steps`,
  `automation_ceiling`, `required_proof_format`, `require_intermediate_claims`,
  `allowed_imports`) and returns structured `PolicyViolation`s. We have the
  pieces scattered (lint, source scan, `optimize.rs` minimization, router
  permissions) but not assembled into one versioned per-problem contract. See
  adopt item 3 for the one piece of this worth taking.
- **Topologically ordered, verified-only module closure.**
  `orchestrator/module_closure.rs::order_closure` gathers a BFS closure, requires
  a positive-polarity verified-lemma row for every node, checks name/statement-
  hash interface collisions, checks environment agreement, and Kahn-sorts with a
  `BTreeMap<(name, id)>` ready set for determinism. We have
  `components/graph/scheduler.rs` (topological levels + centrality) and the
  blueprint/splice pipeline. The verified-only admission and the determinism
  discipline match what we do.
- **Synthetic negatives from verified proofs.** `mutations.rs::generate_mutations`
  applies six deterministic corruptions (`WrongTheoremName`, `WrongCoefficient`,
  `FlippedRewriteDirection`, `MissingHypothesis`, `MalformedSyntax`,
  `IncompleteGoal`) each stamped `provenance = "synthetic_mutation"` with an
  `expected_outcome` label. Our `training_eligible` firewall requirement (adopt
  item 15 in `2026-07-new-arrivals.md`) already anticipates this; the corruption
  catalogue itself is unremarkable and our meta-verifier training does not
  currently need it.

### The five things that are actually new

**1. Truncated context is referenced, never dropped.**
`orchestrator/context.rs` is the one module here with no counterpart on our side.
Its `CompactContextBuilder` splits the observation into an always-inlined core
(obligation signature, obligation statement hash, per-dependency
name+status+statement-hash `DependencySummary`, and a `DIAGNOSTIC_HEAD_BYTES =
512` head of the latest diagnostic **even when the budget is already exhausted**)
and a trimmable remainder. Anything trimmed emits a `ContentReference` carrying
`{field, content_hash = SHA-256 of the FULL field, total_bytes, included_bytes,
next_offset}`, and `expand_observation_field` pages the rest back in on demand.
The content hash is over the whole field, so a client can detect that the
underlying material changed between pages.

Our `PromptAssembler` counts tokens and trims by `KEEP_PRIORITY`, records which
sections were trimmed and their cost, and places the most relevant retrieved item
nearest the query. What it does not do is **leave a retrievable handle on what it
dropped**. A model that is told "3 lemmas were trimmed" cannot ask for them; a
model handed `ContentReference{field: DependencySignatures, total_bytes: 9000,
included_bytes: 1200, next_offset: 1200}` can. This composes directly with our
existing `Section` accounting and our meta-tools surface. It is also the correct
shape for our `unknown_declaration` vs `not_in_current_import_scope` discipline:
omitted is not absent.

Also worth stealing from the same file: byte accounting rather than token
accounting for the budget check, on the argument that bytes >= tokens so byte
counting never under-counts. Our `CharsPerToken` estimator is a divisor and can
under-count on dense Lean; their `DEFAULT_BYTES_PER_TOKEN = 4` is used as a
conservative ceiling in the other direction.

**2. "Not instrumented" is a distinct value from "empty".**
`dependency_manifest.rs::DependencySet` is `{instrumented: bool, names: Vec<String>}`
with two constructors: `known(names)` sets `instrumented: true`, `unknown()` sets
`instrumented: false` with an empty vec. `assemble_manifest` will not derive
`retrieved_but_unused` at all unless `retrieved_candidates.instrumented` is true.
The same discipline appears in `mcip.rs::rl_transition_to_mcip`, which on missing
reward/terminal data writes `Value::Null` plus a `missing_field_reasons` entry
("legacy episode predates reward persistence") rather than substituting `0.0` or
`false`, and in `dependency_manifest_to_mcip`, which omits fields for
uninstrumented categories rather than emitting misleading empty arrays.

This is the same failure mode as their four-valued declaration lookup (adopt
item 14) applied to telemetry instead of to the environment: an empty array read
as a measurement is a fabricated negative. Our evidence and trace records
(`components/graph/evidence.rs`, `components/reason/orchestration/trace.rs`)
should carry the same distinction wherever a count or a list can be absent for
reasons other than being genuinely zero. Cheap to add, and it prevents a class of
silently wrong analytics.

**3. Alias expansion on tactic bans.**
`policy.rs::alias_expand` hardcodes synonym groups so that forbidding `ring` also
catches `ring_nf`, forbidding `simp` catches `simp_all` / `simpa` / `dsimp`,
`decide` catches `native_decide`, and `norm_num` catches `norm_cast` / `push_cast`.
Without it, every tactic ban is trivially evadable by a one-character rename, and
a policy that is evadable is worse than no policy because it produces a false
clean signal.

We ban `native_decide` by name in `formal_lint.py`. That specific ban is
adequate today because `decide` is also gated. But any future ban we write should
expand through an alias table by construction. Small, mechanical, and it closes a
whole category of silent bypass.

**4. In-flight reservations count against the budget.**
`orchestrator/budget.rs::get_spent_cost` sums
`CASE WHEN state='committed' THEN COALESCE(actual_cost, reserved_cost)
WHEN state='reserved' THEN reserved_cost ELSE 0` over the ledger. So a
reserve/commit/release lifecycle admits work against *committed plus outstanding*
spend, not just settled spend, and `release` (for abandoned attempts) is a third
state rather than a delete. We have per-call budgets in several places
(`proof_job.rs`, `session/exec.rs`, `decompose.rs`) and a token budget in the
assembler, but no single spend ledger, and nothing that counts concurrent
in-flight attempts against a cap. With any parallel dispatch, a cap that only
counts settled spend overshoots by the width of the parallelism.

Caveat: their implementation is sloppy in ways we should not copy even
conceptually. `commit` and `release` never check rows-affected, so an unmatched
`reservation_id` silently no-ops and the reservation stays counted forever.

**5. Kernel-verified and fidelity-verified are separately rewarded.**
`models/reward.rs::RewardPolicy::default_policy` awards `root_kernel_verified:
+2.0` on **every** kernel-verified termination, whether or not statement fidelity
is verified, and reserves `terminal_success: +10.0` for the composite
kernel-verified-AND-fidelity-verified case. The comment states the reasoning
plainly: the prover proved exactly the formal statement it was given, which is
real work regardless of whether that statement matches the source problem. There
is also a `truncation_penalty: -5.0`, larger in magnitude than `kernel_fail`, for
generations that ran out of tokens.

Our `reward.py` has `correctness_reward`, `format_reward`, `verifier_reward`,
`graded_verifier_reward`, and `generator_self_verify_reward`, but no tier that
separates "proved the formal statement" from "proved the right statement". Our
architecture already makes the distinction (statement validity screening,
`statement_preservation.rs`, `certification.rs`), so the reward shaping is the
part that lags. Flagging as a design point rather than an adopt: note the direct
tension with FormaRL adopt item 9, which argues for zero partial credit. The two
are reconcilable only because kernel-verified-without-fidelity is not a cheap
gate a policy can game the way compile-only is; if that stops being true, the
FormaRL rule wins.

### Two further design points, no action

- **`publication_review.rs::publication_status`** is a deterministic state
  machine over six review layers (`kernel_or_certificate`, `statement_fidelity`,
  `literature_completeness`, `citation_lineage`, `novelty_claim`,
  `exposition_disclosure`) in which `PublicationReady` is structurally
  unreachable without `citation_lineage` complete, and revoking that layer drops
  the status while `kernel_verified()` stays true. `literature_lineage.rs`
  enforces by test that the provenance record's serialized JSON contains none of
  `"proved"`, `"verified"`, `"kernel"`, so the evidence type can never claim proof
  status. Both are good separation-of-concerns discipline for the day we publish
  results. Not a proving mechanism; not on the critical path.
- **`orchestrator/scheduler.rs::next_ready`** scores ready obligations as
  `(0.30*C + 0.20*D + 0.30*I + 0.20*(1-H)) / (1+K)` where C is
  log-normalized paths-to-root centrality, D is proximity to root, I is
  unblocking value (immediate parents unblocked plus a quarter-weighted count of
  open ancestors), H is a difficulty heuristic, and K is a cost term. We have C
  (`graph/scheduler.rs::centrality`) and the topological levels but not I or the
  cost divisor. I would leave this alone: their H is a bag of substring counts
  over the statement text with a hardcoded zero term, and their K divides a
  hardcoded `0.05` "expected next cost" by a `remaining_budget` in unspecified
  units while `budget.rs` works in micros. The weights are unvalidated and the
  units are inconsistent. The *idea* of an unblocking-value term is sound; the
  implementation is not evidence for any particular formula.

### Empirical claims from this repo: still do not cite

The warning in `2026-07-new-arrivals.md` stands and is reinforced by the
`benchmarks/serious_math_ladder/README.md`, which states outright that
`source_fidelity_status` is `synthetic_plumbing` for **every** rung: the seven
ladder proofs exercise the reducer and module-assembly path but their
mathematical validity is only checked by an optional gated real-Lean test. The
ladder is a plumbing fixture, not a capability benchmark. Credit where due: the
README says this itself, in a section headed "Trust boundary", and it correctly
insists that rung metadata is descriptive only and never grants
`kernel_verified` or `certified`. That is the right instinct and matches ours.

## Ranked adopt list

1. **Content-referenced truncation with pagination.** Extend
   `context_assembly.rs` so every trimmed section emits
   `{field, content_hash of the full field, total_bytes, included_bytes,
   next_offset}` and add a meta-tool that pages it back. Guarantee an
   always-inlined head of the latest diagnostic regardless of budget. Highest
   value of anything here: it converts a silent drop into a retrievable handle.
2. **"Not instrumented" as a first-class value in evidence and manifests.** A
   two-field `{instrumented, values}` shape wherever a count or list can be
   absent for reasons other than genuinely being zero, plus explicit
   `missing_field_reasons` on exported records instead of substituted defaults.
3. **Alias expansion for any tactic ban.** A synonym table applied by
   construction (`ring` -> `ring_nf`, `simp` -> `simp_all`/`simpa`/`dsimp`,
   `decide` -> `native_decide`, `norm_num` -> `norm_cast`/`push_cast`) so no ban
   is evadable by rename. Small and mechanical.
4. **Count in-flight reservations against any spend cap.** If we ever add a
   monetary or wall-clock cap across parallel dispatch, the admission check must
   sum committed plus reserved, with an explicit released state. Check
   rows-affected on commit and release, which they do not.
5. **Separate the kernel-verified reward tier from the fidelity-verified tier**,
   with fidelity as the composite terminal. Design point only; revisit against
   the FormaRL zero-partial-credit rule before shipping any weight.

Also worth one line in the cycle-error path: `module_closure.rs::reconstruct_cycle`
walks the remaining nodes and returns the **actual cycle as a list of theorem
names**, not "a cycle exists". If our DAG code reports cycles without naming
them, name them.

## Skip list

- **`TheoremLlama-main`**: does not contain the method it is cited for; the eval
  script has no verifier; unlicensed. Its one prompt idea is subsumed by our
  context assembler.
- **`AutoMathText-2.5-main`**: a landing page. Already documented in
  `docs/resource-mining/AutoMathText-and-goldbach-collatz.md`. The method is in
  the paper, not the repo. Restrictive non-OSS data agreement.
- **`Newclid_Transformer-main`**: already documented in full at
  `docs/resource-mining/new/newclid.md`. Its `brevity_penalty` is a fixed-alpha
  special case of our `length_normalized_score`.
- **`FormaRL-main`**: already documented in
  `docs/resource-mining/new/2026-07-new-arrivals.md` (adopt item 9, empirical-claims
  entry, MIT licence). Skipped per instruction.
- From `llm-driven-proof-search`: the analyzer, repair chain, mutation catalogue,
  module closure, MCIP export layer, and scheduler formula. We have equivalents,
  or the implementation is not evidence for the design.

## Licensing summary

| Repo | Licence | Constraint |
|---|---|---|
| `TheoremLlama-main` | none present | Unlicensed. Ideas only, no code. |
| `llm-driven-proof-search-develop` | MIT badge with 404 link, no LICENSE file, no `license` field in `Cargo.toml` | **Treat as unlicensed. Clean-room: ideas only, no code copied.** |
| `Newclid_Transformer-main` | Apache-2.0 (software), CC-BY-4.0 (weights/vocab) | Portable with attribution. |
| `AutoMathText-2.5-main` | custom "AutoMathText Data Agreement for Model Training" | Not OSS. A data EULA; read before using the HuggingFace dataset. |
| `FormaRL-main` | MIT | Portable. |

**No GPL or AGPL in any of the five.** The binding constraint is the *absence* of
a licence in two of them, which is more restrictive than GPL, not less: an
unlicensed repo grants no rights at all.

## Untrusted-content flags

- **POSSIBLE INJECTION**:
  `resources/llm-driven-proof-search-develop/llm-driven-proof-search-develop/CLAUDE.md`.
  Agent-directed operating instructions ("Operating Doctrine", propose/validate/commit
  loop, run tests and lint before committing). Content is benign; none of it was
  followed. Strip before any ingestion. Adjacent agent-directed prose lives in
  `docs/llm_native_design_practices.md` and `docs/kits/`.
- No injection attempts found in `TheoremLlama-main`, `AutoMathText-2.5-main`,
  `Newclid_Transformer-main`, or `FormaRL-main`. The `AutoMathText-2.5` LICENSE is
  a legal agreement, not instructions to a model.
- Supply-chain note: `llm-driven-proof-search` ships `elan-init.ps1`,
  `restore-database.bat`, and a `scripts/` tree. None were run. Do not run them.

## Risks / revisit triggers

- **Adopt item 1 has a footgun.** If a `ContentReference` is emitted but no
  expansion tool is wired, we have added a promise the model cannot cash, which
  is worse than an honest drop. Ship the reference and the pager together or
  neither.
- **Adopt item 2 changes serialized shapes.** Turning a bare `Vec<String>` into
  `{instrumented, names}` breaks any consumer that reads the field positionally.
  Do it behind a version bump on the record schema, not in place.
- **Adopt item 5 is in tension with a Tier-2 item we already accepted.** FormaRL
  adopt item 9 says weight cheap-gate-only outcomes at zero. Kernel-verified
  without fidelity is not a cheap gate today. If a policy ever learns to farm
  kernel-verified-but-unfaithful statements (for instance by steering
  formalization toward statements it can already prove), item 5 becomes a
  reward-hacking surface and must be reverted to zero.
- **Re-read trigger for `TheoremLlama`**: if the training code the README has
  promised since Oct 2024 ever lands, the block-training and curriculum-sorting
  recipes become minable. Until then there is nothing there.
- **Re-read trigger for `llm-driven-proof-search`**: if it ever acquires a real
  LICENSE file, the clean-room constraint lifts and `context.rs` becomes
  directly portable rather than reimplementable. Also worth re-checking
  `docs/playtests/` and `docs/fix_plan_playtest_0{1..5}.md`, which the earlier
  pass identified as the repo's most credible content (documented soundness holes
  caught before shipping) and which this pass did not re-read.
- **`Newclid`**: the earlier doc's strategic note stands. The real deduction
  engine is the external `newclid >= 2.0` pip package, not the vendored
  `Newclid_Transformer`. If the geometry vertical becomes a priority, that
  package is the thing to mine.
