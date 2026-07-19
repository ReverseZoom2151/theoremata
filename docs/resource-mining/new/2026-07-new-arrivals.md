# Resource mining: 2026-07 new arrivals (34 repos)

Scope: every repo under `resources/` with no mention in any existing mining report,
determined by grepping each repo name against the full text of `docs/resource-mining/`
(826 KB) rather than matching filenames -- several existing reports cover many repos
each. 39 flagged, ~5 of which were Higher Order Co leftovers already covered by
`higher-order-co.md`, leaving **34 genuinely new**. All read fully, read-only, with
vendored content treated as untrusted data.

## Headline: two confirmed gaps in our own gate

The most valuable output of this pass is not an idea to adopt, it is two ways a proof
can pass **every layer we currently have** while assuming real mathematics. Both were
verified against our code, not merely asserted.

### Gap 1: no unaccounted-hypothesis audit

A theorem can be `sorry`-free, `axiom`-free, kernel-clean, and statement-preserved,
while being **conditional on unproved mathematics carried in its own signature**.

Two mechanisms, both observed at scale in this corpus:

- **`Prop`-valued hypothesis arguments.** `andrews_dhar`'s headline result is
  `theorem phi3_bijOn (hGlaisher : Glaisher3) (N : ℕ)`, where `Glaisher3` is a
  `def ... : Prop` that is stated and never proved. 22,475 lines, zero live sorries,
  zero axioms.
- **Uninhabited assumption-bundling `structure`s.** `ramanujan-tau` defines
  `structure RamanujanTau` bundling five properties of τ and **never constructs an
  instance** anywhere in the repo. The main theorem is vacuous if that bundle is
  inconsistent. 5,601 lines, 487 lemmas, clean on every check we perform.

Both are *faithful to their stated tasks* -- this is not fraud, it is normal practice.
But an assumption bundle is semantically an axiom set that is **invisible to
`#print axioms` and invisible to a `sorry` grep**.

What we have and why it does not cover this: `statement_preservation.rs` has a
`BindersChanged` verdict, but that detects **drift of the submitted binders away from
the canonical statement**. If the canonical statement itself carries the hypothesis, or
the conditionality is introduced upstream, `BindersChanged` is silent. It is a
drift check, not a discharge check.

**Proposed layer.** Enumerate every free hypothesis in the delivered theorem's
signature (including fields of any assumption-bundling structure it takes) and
classify each as: (a) discharged elsewhere in the submission, (b) explicitly
allowlisted as a designated input, or (c) **unaccounted** -- and fail closed on (c).
`ramanujan-tau` is a ready-made regression fixture: it must be reported as
*conditional*, never as certified.

### Gap 2: vacuous success is not in the failure taxonomy

An agent can discharge a goal by making its hypotheses **contradictory**. The result
passes the kernel, passes the axiom audit, passes statement preservation, and is
worthless. `trace.rs`'s `FailureClass` is `{Planning, ToolingExec, VerificationReject,
TimeoutResource, ModelFormat, Unknown}` -- there is no vacuity class, and nothing
anywhere checks that a hypothesis bundle is inhabited.

The defense is cheap and comes from `LatentError/task.md`, which mandates it as a
pre-search gate:

> "The same pass must also confirm the bundle is SATISFIABLE by exhibiting at least
> one numerical instance meeting every field (an unsatisfiable bundle would make all
> four theorems vacuously true -- **a hollow success that must be treated as a spec
> failure**)."

**Proposed:** add `VacuousSuccess` to `FailureClass`, and require a satisfiability
witness (one concrete instance meeting every hypothesis field) before proof search
starts on a generated statement. A numerical witness costs almost nothing relative to
what it catches.

### Gap 3 (unconfirmed, worth checking): axiom injection via imports

`llm-driven-proof-search` documents a live playtest failure where the import list
accepted arbitrary Lean, so `"Mathlib\naxiom cheat : False"` **baked a false axiom into
every subsequent proof for that problem** -- certifying anything, with no `sorry` and
nothing for an axiom audit downstream to see, because the axiom is in the environment
rather than the submission. Our `formal.rs` has `default_imports`, but I did not
establish whether a model-supplied import list is compile-validated and pinned. Worth
an explicit check.

## What these repos actually are

**24 of the 34 are one corpus.** Repos named `gdm-formal-conjectures`, `erdos-public`,
`IMO2026`, `Putnam2025`, and 20 others are all **Axiom Math** (axiommath.ai) public
output artifacts from their **AxiomProver** system -- identical `logo.svg`, identical
MIT license text, identical `input/task.md -> problem.lean -> solution.lean` layout,
overlapping maintainers (Evan Chen, Kenny Lau, Ken Ono, Jujian Zhang).

Two corrections that matter:

- **`gdm-formal-conjectures` is not Google DeepMind's `formal-conjectures`.** It is
  Axiom's solutions to *two problems drawn from* it. The real corpus (~496 Lean
  statements, 616 open problems) is **absent from `resources/`** and should be fetched
  separately if open conjectures are wanted as targets.
- **`LatentError` is not error detection.** It is "estimation **error** in **latent**
  factor models" -- a statistics formalization. There is no detection method here.

`lambda-eval` is unrelated to everything (a 2023 528-line lambda-calculus reducer by a
HigherOrderCO dev, abandoned, with a capture-unsafe `subst`). Ignore.

## Tier 1: act on these

1. **Unaccounted-hypothesis audit** (Gap 1). New gate layer. Fixture: `ramanujan-tau`.
2. **`VacuousSuccess` + satisfiability witness** (Gap 2). Taxonomy entry + pre-search check.
3. **Verify the import-manifest vector** (Gap 3), and hash the validated manifest into
   the checker-cache key. A cached pass is only meaningful relative to an environment.
   (`checker_identity` and `policy_fingerprint` already cover toolchain and policy; the
   per-problem import closure is the open question.)
4. **Goal-state-at-error-position feedback** (Kimina, `create_tool_message`). Walks the
   Lean **infotree**, finds the smallest syntax node containing the error span (scored
   `10*Δline + Δcolumn` on both endpoints), extracts `goalsBefore`/`goalsAfter`, and
   leads the feedback with the actual hypothesis context at the failure. Requires the
   REPL be run with `infotree_type: "original"`. We have `goal_state.rs`; if it is only
   feeding search and not failure feedback, this is the highest-value port available.
5. **Structured error rendering** (Goedel, `get_error_str`). Pure function, ~100 lines:
   4 lines of leading context, the failing span wrapped in `<error>...</error>`, middle
   elided past 6 lines with `... --[Truncated]-- ...`, capped at 8 errors with an
   explicit omitted-count. Strictly better than dumping raw compiler output.
6. **Never expose a fail-open verification primitive beside a fail-closed one.** AXLE's
   `check` returns `okay: true` for code full of `sorry` (offenders land in
   `failed_declarations`); `verify_proof` folds the same conditions into errors. Their
   docs fight this constantly. Either do not expose the cheap one, or name it
   `compiles_only`.
7. **Deterministic failures are non-retryable.** AXLE split out `LeanTimeout` /
   `LeanResourceExceeded` precisely because they are reproducible for a given
   statement+budget: record as an outcome, never retry. Retrying burns budget and
   pollutes traces.

## Tier 2: design patterns worth adopting

8. **Statement-validity filter stack** (Kimina §C.2.2) -- the most sophisticated
   anti-reward-hacking machinery in the batch, and it maps onto what we already have:
   *unanimous multi-sample* LLM judging (not one call); **negation-proving as a
   curation filter** (prove `¬statement`; success means the formalization is broken);
   **triviality detection** (if a short generated proof closes it, the statement is
   degenerate); and post-hoc checking of whether a proof exploited a formalization bug.
   Their own conclusion is worth heeding: an LLM judge alone is *not* a sufficient
   anti-hacking gate. FormaRL converged on the same finding independently.
9. **Conjunctive, zero-partial-credit reward.** Every shipped FormaRL config sets
   `beta_1 = beta_2 = 0.0, beta_3 = 4.0` -- no credit at all for passing the cheap gate
   alone. They have the scar tissue to justify it: their policy learned to emit the
   empty string, comment-only output, and to copy the `1+1=2` example out of its own
   prompt. The fixes shipped in `eval.py` are literal string blacklists. If we ever
   reward on gate outcomes, weight compile-only at zero.
10. **Two-file spec/proof split**, `problem.lean` (statement with `sorry` as the hole)
    and `solution.lean` (restates and proves), with an explicit "solution must match
    problem" check. Clean, gradeable I/O contract; used uniformly across 24 repos.
11. **`SafeVerify.lean`** (Putnam2025, adapted from `lean4checker`): replays the
    submission environment via `Environment.replay` **to defend against environment
    manipulation**, matches theorems on name + type + `levelParams` + `all`, compares
    definition *values* only when the target's own axioms lack `sorryAx`, rejects
    `unsafe`/`partial`, and enforces `AllowedAxioms` transitively via `CollectAxioms`.
    A stronger statement-preservation primitive than string comparison.
12. **Spec / computable-mirror / bridge-theorem triple** as the certificate shape
    (`quadratic-dinv`). A `Set` spec, a `Finset` mirror obtained by filtering an
    explicit bounding box, and a **proved** `mem_gapFinset_iff` linking them. The
    certificate is a checked theorem, so no trusted numeric layer is needed -- notably
    chosen *over* `native_decide`, which would pull the compiler into the trusted base.
13. **Decomposition admission checks** (llm-driven-proof-search): reject unless every
    child elaborates, **no child hash equals the parent or a sibling** (kills
    restate-the-goal-as-its-own-lemma), the graph stays acyclic, **the parent proof is
    required to reference every child**, and **no child is admitted as proved on the
    decomposition model's assertion**. Plus: *syntax and elaboration errors trigger
    repair before decomposition -- they do not by themselves justify changing the
    mathematics.*
14. **Four-valued declaration lookup.** `found` / `not_in_current_import_scope` /
    `unknown_declaration` / `environment_error`, where the last is *evidence of nothing
    either way*. Prevents "environmental scope collapse" -- the model reading a
    scope miss as "Mathlib doesn't have this" and abandoning a provable branch. Their
    phrasing is good: *"not inventing a capability, but confidently inventing a
    limitation."* All six of our backends have this pathology.
15. **`training_eligible` firewall.** Dev-bypass and synthetic-negative data must be
    structurally unable to enter the DPO/preference corpus, enforced in the data model
    rather than by convention. Pair with an organic/synthetic provenance stamp.
16. **Falsifiers return structured witnesses, not verdicts.** The conflict set is what
    routes repair. (axplorer returns the offending tuples, not a boolean.)
17. **Verification ladder: `fast_refute -> cheap_decide -> expensive_kernel`.** Unsound
    numeric probes are legitimate *pre-filters* -- refute false claims cheaply before
    paying for a backend. Generalizes our falsify-before-prove into a cost hierarchy.
18. **Ternary validity**: correct / incorrect / **undecodable**. A candidate that failed
    to parse is categorically different from one the checker rejected; they route
    differently (repair vs prune).
19. **`file_uri` + MCP roots enforcement** on content-bearing tools, with three-valued
    semantics (`None` = unconstrained, `[]` = deny-all, list = allowlist), blocked in
    HTTP mode. Large proof sources should not be paid for in context tokens.
20. **Audit our MCP schemas for `oneOf`/`anyOf`/`allOf`** -- the Anthropic Messages API
    and Vertex-via-OpenRouter reject them in tool `input_schema`. Enforce mutual
    exclusion in handler code instead. Live compatibility bug if we use them.
21. **`sorry2lemma` from *error locations*, not just explicit `sorry`s.** A partial proof
    that errors out still yields well-formed subgoals to enqueue as DAG nodes --
    subgoal extraction from failure, which is strictly more useful than from success.
22. **Sanctioned abstention.** "Report NOT CLOSED -- never a `sorry`, never a smuggled
    hypothesis; a sorry-free file of the rest is worth more." Exactly the behavior our
    meta-gate should reward.
23. **The validator greps comments too.** HigherDyson's spec records that *a commented
    `sorry` tripped the validator before*. Our source scan should not exempt comments.
24. **Banned-hypothesis denylist** (HigherDyson `task_batch3.md`): an explicit list of
    17 hypotheses, annotated *"every one previously smuggled and rejected."* The
    productionized form of the Gap-1 audit, learned from observed prover cheating.

## Eval targets

Ranked. All MIT except where noted.

| Repo | Why | Caveat |
|---|---|---|
| `zeta-h123` | 4 unconditional tasks, sharply varied difficulty (H3 solution 311 LOC vs H1 2,313 from similar-looking statements), shared context | wire first |
| `PartitionPolynomial` | 6 unconditional, uniform difficulty, sorry-free, one-line prompts, published baseline | best ready-made mini-benchmark |
| `RogersRamanujan-artifacts` | 6 self-contained pairs, **including a refutation** (`prescribed-open`, answered negatively) | rare negative-result eval |
| `Biswal` | ships "prove Thm 1, then feed its solution forward as input to Thms 2-3" | ready-made library-growth / `method_transfer.rs` test |
| `ramanujan-tau` | **the Gap-1 regression fixture** | must be reported conditional, never certified |
| `andrews_dhar` | brutal (16,199-line single solution file) | must preserve conditionality on `Glaisher3` |

**Do not vendor `RogersRamanujan-main`: it has no LICENSE file** (all rights reserved).
It is also the most instructive repo in the corpus -- 9,859 lines of which ~75% is
*missing Mathlib substrate* (non-archimedean topology, `MvLaurentSeries`) rather than
q-series. The lesson: at research level, the theorem is small and the substrate is the
cost.

**Corpus-wide caveat.** These are one company's *published successes*, curated by the
four mathematicians who wrote the source papers, with human-authored LaTeX proofs
supplied as input and multi-round judge rejection. Success on them measures fit to
AxiomProver's output distribution, not general research-formalization ability. Also:
toolchain pins span Lean 4.21-4.31, so this corpus cannot be run against one Mathlib.

## Empirical claims: do not cite these

- **Goedel-Prover-V2**: README and its own plotting notebook disagree (8B Pass@32
  **84.6** vs **83.3**; 32B **88.0** vs **88.11**), the olympiad benchmark is named
  differently in each, and the compute-adjusted plot rescales the competitor's x-axis by
  `37/8` in one cell and `67/8` in the next. Use the raw Pass@k table only.
- **Kimina**: numbers are paper-only; no training or eval code ships. Genuine credit for
  decontamination and for **identifying 8 unsolvable/mis-formalized miniF2F-test
  problems by name** -- if we report miniF2F, we should exclude or correct those.
- **FormaRL**: reports **zero benchmark numbers**, and has three real bugs including
  divergent pass criteria between its train and eval paths.
- **llm-driven-proof-search**: 8/12 on a hand-picked easy Putnam sample, where the
  `certified` results rest on **fidelity reviews by the same LLM that wrote the
  proofs**. Zero Erdős problems solved; the repo says so plainly and repeatedly. Its
  most credible content is *negative* -- five playtest reports documenting real
  soundness holes caught before shipping, which is a useful threat model.

## Licensing

MIT: all 24 Axiom Math repos, `axle-mcp-server`, `axiom-lean-engine`, `FormaRL`,
`lambda-eval`. Apache-2.0: `axolver`, `axplorer`. **No GPL/AGPL anywhere in the 34.**

Unlicensed or unconfirmed, do not copy code:
- `RogersRamanujan-main` -- no LICENSE file.
- `llm-driven-proof-search` -- MIT *badge* linking to a 404, no LICENSE file, no
  copyright holder, no `license` field in `Cargo.toml`. Treat as unlicensed; take ideas
  only.
- `Goedel-Prover-V2` -- Apache badge, no LICENSE file in the vendored copy. Verify
  upstream before copying literal code.
- `Kimina-Prover-Preview` -- no LICENSE, no license statement. Port algorithms, not code.

Supply-chain note: Goedel and Kimina both vendor **git submodules pointing at
third-party mathlib4 forks** (Goedel at `github.com/xinhjBrant/mathlib4`, not upstream).
Do not `git submodule update` these.

## Untrusted-content flags

No malicious injection found in any of the 34. But this corpus is **structurally full of
AI-directed imperative text**, because `input/task.md` files *are prompts to a prover*.
Flagged for operational awareness, all treated as data:

- Every Axiom Math `input/*/task.md` and `requirement.md` -- imperative instructions to
  an automated prover, including output constraints ("must be fully sorry-free. Do not
  add axioms").
- `axplorer/program.md` -- an agent playbook with prescribed `python` commands and
  "STOP HERE. Present the implementation to the user."
- `llm-driven-proof-search`: root `CLAUDE.md` instructing `cargo build/test/fmt` before
  reporting completion, and `docs/playtests/handoff-*.md` written as second-person
  instructions to an AI host.
- **`TanArctan/output/solution.lean:530-565`** -- the sharpest case: an agent scratchpad
  with self-directed planning and Mathlib search instructions survived **into a shipped
  `.lean` file**, i.e. imperative text living inside source a parser would otherwise
  treat as trusted proof content. If we ingest this corpus, `task.md` and inline
  scratchpads will land in context as instructions. Fence as data.

## Coverage

34 repos read across 7 parallel readers. Nothing was built, compiled, or executed; no
git operations. The `AgreeToDisagree` / `partial-regularity` / `lattice-triangle` /
`parity-differential` / `kaprekar4` / `record-compositions` cluster is pending and will
be appended.
