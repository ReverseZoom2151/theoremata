# Backlog slice: docs/atp-mining, docs/agentic-patterns-mining, docs/formal-systems

Scope: 30 docs (14 ATP, 10 agentic-patterns, 6 formal-systems).
Method: every adopt / build / recommendation / "TOP N gap" / "buildable delta" row
extracted, then each verified by grepping the distinctive token across `components/`
and `app/`. Status is what the grep plus a read of the hit shows, not familiarity.
Rows sorted SOUNDNESS first, then capability/quality, then doc-stated skips.

Row count: 250 backlog rows (sections 1 to 3), plus a 19-row certificate-gap table
in section 4 that is a cross-check, not backlog.
IMPLEMENTED 73 | PARTIAL 64 | NOT-BUILT 78 | SKIPPED 32 | BLOCKED 3.
Where a row carries two statuses (for example "NOT-BUILT (checker); BLOCKED
(generator)") it is counted under the first.

Class column is folded into the section headers: section 1 is SOUNDNESS, section 2
is CAPABILITY/QUALITY, section 3 is SKIPPED/BLOCKED.

---

## 1. SOUNDNESS items

| Item | Source doc | Mechanism (one line, concrete) | Target file(s) | Status | Evidence |
|---|---|---|---|---|---|
| Isabelle oracle audit (`thm_oracles` / `Thm_Deps.all_oracles = []`) | formal-systems/isabelle.md, INTEGRATION-PLAN.md | Query the transitive oracle graph of the proved lemma and require it empty | components/prover/backends/isabelle.rs | **NOT-BUILT** (documented as delivered) | `audit_axioms` at isabelle.rs:386 returns `within_whitelist: true` unconditionally in BOTH mock and live paths; the note defers layer 2a to compile + source scan. An oracle reached through an imported theory is invisible. |
| Agda transitive imported-postulate report | formal-systems/AGDA-1LAB-METAMATH-SPEC.md | "Imported postulates must be reported transitively, not only lexically scanned" | components/prover/backends/external.rs | **NOT-BUILT** | `audit_axioms` (external.rs:1211) returns empty axioms + `within_whitelist: true` for both Agda and Metamath; only the word-boundary token scan runs. `grep transitive components/prover` hits only lean.rs and decl_index_adapter.rs. |
| Metamath independent kernel re-check | AGDA-1LAB-METAMATH-SPEC.md, INTEGRATION-PLAN.md (layer 2b) | Independent replay of the checked database | components/prover/backends/external.rs | **PARTIAL, and it trusts the wrong signal** | `kernel_recheck` (external.rs:1247) sets `rechecked = out.success()`, i.e. the raw exit code, while `compile` correctly uses `compile_success_signal()` precisely because the Metamath reference binary returns 0 on a FAILED `verify proof *`. The recheck layer is therefore vacuous for Metamath. Conjunction with `compile` saves the gate today, so it is not currently exploitable, but layer 2b delivers nothing. |
| Metamath secondary checker must not substitute for the primary | AGDA-1LAB-METAMATH-SPEC.md | "A second checker such as mmverify.py is an optional independent cross-check; it is not silently substituted for the configured primary checker" | components/prover/backends/external.rs | **NOT-BUILT (inverted)** | external.rs:1257 does `rechecked = second.success();` which OVERWRITES the primary verdict with the secondary's. The spec forbids exactly this. |
| Per-result provenance envelope (`source_sha256`, `dependency_sha256`, `limits`, `checker.version`) | AGDA-1LAB-METAMATH-SPEC.md ("Common contract") | Every Agda/Metamath result carries source hash, dependency-closure hash, and resource limits | components/prover/backends/external.rs, components/prover/formal.rs | **NOT-BUILT** | `grep sha256` over external.rs and formal.rs returns nothing. `CompileReport`/`RecheckReport` carry no version or hash fields. |
| Metamath `$a` recorded, not blindly rejected | AGDA-1LAB-METAMATH-SPEC.md ("$a statements ... must be recorded, not blindly rejected"; test: "a database with ordinary $a declarations is not incorrectly rejected") | Record database axioms as context; reject only axioms the generated proof introduces | components/prover/backends/external.rs | **PARTIAL / diverges** | `metamath_source_findings` (external.rs:1350) does `if code.contains("$a")` and flags it, so any ordinary `$a` fails the scan. Nothing records them into `AxiomReport` (which is empty). Over-rejection, opposite of the spec's stated test. |
| Candle statement preservation | INTEGRATION-PLAN.md (gate is universal), rocq.md/lean.md analogues | Compare the canonical proposition against the submitted `let NAME = prove(...)` binding | components/prover/statement_preservation.rs:1228 | **IMPLEMENTED, but the live path can never pass** | `check_candle_signature` fails closed with `CanonicalUnparsable` and the in-code note says today's Candle callers pass a bare entry name or bare term "neither of which asserts anything". Fail-closed is correct; the consequence is that live Candle certification is unreachable until callers are fixed. This is the Candle defect named in the brief, now fail-closed rather than unsound. |
| Rocq statement preservation keyword parser | rocq.md, INTEGRATION-PLAN.md | Parse `Theorem`/`Lemma`/... declarations on word boundaries | components/prover/statement_preservation.rs:812 | **IMPLEMENTED (fixed)** | `ROCQ_DECL_KEYWORDS` is the 8 correctly-capitalized Rocq keywords with `rocq_keyword_at` enforcing both boundaries. The lowercase-only defect named in the brief is not present in the current tree. |
| 3+1-layer gate, source scan MANDATORY | INTEGRATION-PLAN.md | compile AND axioms-in-whitelist AND kernel recheck AND source scan, conjunctive fail-closed | components/prover/formal.rs:575 | **IMPLEMENTED** | `verify_with_gates` conjoins all four plus the suggestion-tactic hatch; `live_poll` fails closed on unavailable toolchain. |
| `FormalSystem` abstraction (enum + `FormalBackend` + `ProofSession`) | INTEGRATION-PLAN.md | One trait, per-system impls for scaffold/compile/audit/recheck/scan | components/prover/formal.rs | **IMPLEMENTED, extended** | 6 systems shipped (Lean, Rocq, Isabelle, Candle, Agda, Metamath) vs 3 in the doc. |
| Mock reports can never certify | INTEGRATION-PLAN.md phase 1 | Mock backends stamped `live: false` so nothing downstream upgrades them | components/prover/formal.rs:653 | **IMPLEMENTED** | `live: !self.is_mock()`. |
| Per-backend `SuccessSignal` (exit code never trusted alone) | AGDA-1LAB-METAMATH-SPEC.md, resource mining | Each backend declares its positive pass signal; Metamath uses a stdout sentinel | components/prover/formal.rs:691 | **IMPLEMENTED** | `SuccessSignal::{NonZeroExitIsHonest, StdoutSentinel}`, required method with no default. See the recheck-layer gap above for where it is not applied. |
| Alias-expanded, word-boundary escape-hatch table | rocq.md, lean.md, isabelle.md | One shared table per system so a rename cannot walk past the ban | components/prover/formal.rs:1017-1111 | **IMPLEMENTED** | Lean/Rocq/Isabelle/Agda tables + `escape_hatch_findings`; test asserts base, alias, and innocent-identifier behavior. |
| Rocq `Print Assumptions` audit against the sentinel | rocq.md, INTEGRATION-PLAN.md | Append `Print Assumptions entry.` at scaffold, check stdout for "Closed under the global context" | components/prover/backends/rocq.rs:293,367 | **IMPLEMENTED** | Live scaffold appends the command; audit parses `Axioms:`/`Variables:` on failure. |
| Rocq escape hatches audit+recheck cannot see (`-type-in-type`, `bypass_check`, `Unset ... Checking`, `Admitted`) | rocq.md | Lexical layer 2c because layer 2a/2b provably miss these | components/prover/formal.rs:1044 | **IMPLEMENTED** | `ROCQ_HATCH_TOKENS` covers all named forms plus `Parameter`/`Conjecture`/`Hypothesis`/`Variable` synonyms. |
| Agda `--safe` mode as the checker boundary | AGDA-1LAB-METAMATH-SPEC.md | Type-check with `agda --safe` plus explicit library config | components/prover/backends/external.rs:893 | **IMPLEMENTED** | `command()` emits `--safe`. |
| Agda interaction output is advisory, never the verdict | AGDA-1LAB-METAMATH-SPEC.md | `--interaction-json` for goals only; batch check is authoritative | components/prover/backends/external.rs:917 | **IMPLEMENTED** | detail carries `authority: advisory_only`, `verdict_source: batch_agda_safe`, `exit_status_trusted: false`. |
| Metamath include-closure / path-traversal guard | AGDA-1LAB-METAMATH-SPEC.md | `$[ file $]` includes must stay inside the workspace | components/prover/backends/external.rs:1345,1386 | **IMPLEMENTED** | `metamath_includes` + absolute/`..` rejection; scaffold copies includes from `cfg.resources` and errors when absent. |
| HOL Light axiom-base auditor (`mk_thm`, `new_axiom`, bad definitions, `CHEAT_TAC`) | atp/hol-light-system.md, hol-light-manuals.md | Flag anything widening the fixed `ETA_AX`/`SELECT_AX`/`INFINITY_AX` base or forging a `thm` | components/prover/axiom_audit.rs | **IMPLEMENTED** | 717 lines; `mk_thm`, `new_axiom`, type-variable-in-definiens, `INST`/`INST_TYPE` warnings; plugs into both `source_scan` and `audit_axioms`. |
| Primitive-inference proof log + independent reference checker (INST/INST_TYPE capture priority) | atp/hol-light-system.md | Persist each kernel primitive and re-derive it from first principles | components/prover/proof_log.rs | **IMPLEMENTED, one honest gap** | 1192 lines; REFL, MK_COMB, ABS, BETA, ASSUME, EQ_MP, DEDUCT_ANTISYM_RULE, INST, INST_TYPE with capture-avoiding substitution. `TRANS` is explicitly out of scope; live emission from a running HOL Light image is toolchain-gated. |
| Candle fixed axiom whitelist | INTEGRATION-PLAN.md, hol-light-system.md | Whitelist is exactly the 3 HOL Light axioms; any other `new_axiom` fails | components/prover/formal.rs:96 | **IMPLEMENTED** | `axiom_whitelist()` returns ETA_AX/SELECT_AX/INFINITY_AX; test asserts len == 3. |
| Candle/Metamath structurally audited, not token-scanned | formal.rs design note | Empty token slice is a decision, not an omission | components/prover/formal.rs:1103 | **IMPLEMENTED** | Test `candle_and_metamath_are_audited_structurally_not_by_token`. |
| Tier-0 hypothesis-discharge audit | (gate extension, INTEGRATION-PLAN lineage) | Catch a theorem silently conditional on an unproved `Prop` in its own signature | components/prover/hypothesis_audit.rs, formal.rs:619 | **PARTIAL by design** | Computed and published unconditionally; conjoined only behind `TierZeroGates::hypothesis_discharge`, default OFF because the `designated_inputs` channel is unpopulated by every shipped backend (`designated_inputs` default returns empty). |
| Tier-0 vacuity guard (contradictory-hypothesis proofs) | (gate extension) | Require a satisfiability witness for the goal's hypothesis bundle | components/prover/vacuity.rs, formal.rs:629 | **PARTIAL by design** | Same shape: `hypothesis_bundle` and `satisfiability_witness` both default to `None`, so the check reports NOT DECLARED. Gate default OFF. |
| `sturm` certificate (exact real-root count) | atp/floating-point-numerics.md, hol-light-reflection-cas.md | Sturm chain by exact pseudo-division, root count = variation(a) - variation(b) | components/verify/python/theoremata_tools/cert_sturm.py | **IMPLEMENTED** | File exists, dispatched at worker.py:393. The doc's claim "grep sturm returns nothing in-repo" is stale. |
| `poly_minimax` / `approx_error` composite | atp/floating-point-numerics.md | taylor_model then sturm then Lipschitz max over endpoints and isolating intervals | components/verify/python/theoremata_tools/minimax_bounds.py, cert_sturm.py | **IMPLEMENTED** | Both files carry `poly_minimax`. |
| Squarefree Bezout witness seam | atp/floating-point-numerics.md | `d = r*p + s*p'` proposed by sympy, re-checked in exact rationals | cert_sturm.py, cert_sos.py | **IMPLEMENTED** | `squarefree` present in both. |
| `nullstellensatz` / Groebner cofactor certificate | analysis-transcendentals.md, hol-light-manuals.md, number-theory-misc.md | Checker expands `sum q_i p_i = 1` as a polynomial identity | components/verify/python/theoremata_tools/cert_nullstellensatz.py | **IMPLEMENTED** | dispatched at worker.py:353. |
| Rabinowitsch normal form | analysis-transcendentals.md | `x != y` becomes `exists z. (x-y)z + 1 = 0` | cert_nullstellensatz.py | **IMPLEMENTED** | token present in module and its test. |
| `pratt_primality` certificate | analysis-transcendentals.md, hol-light-manuals.md | Recursive factor tree checked by modexp identities | cert_pratt.py | **IMPLEMENTED** | worker.py:361. Factor-finding for large p remains the gated generator half, as the doc states. |
| Pocklington primality certificate | flyspeck-kepler.md (named alongside Pratt) | Partial-factorization primality certificate | cert_pocklington.py | **IMPLEMENTED** | worker.py:385. |
| `gcd_bezout` / divisibility certificate | hol-light-reflection-cas.md, hol-light-manuals.md | Emit Bezout cofactors `d = p*r + q*s` instead of a bare gcd, plus a `divides` sub-kind | cert_bezout.py | **IMPLEMENTED** (kind name differs) | `grep gcd_bezout` misses; the module is `cert_bezout.py` with `export_bezout_cert`/`export_divides_cert` and `egcd`. Same mechanism, different token. |
| `herbrand_instances` certificate | hol-light-reflection-cas.md | Finite ground-instance disjunction that is a propositional tautology | cert_herbrand.py | **IMPLEMENTED** | worker.py:381. |
| `wz` (Wilf-Zeilberger) certificate + Gosper generator | number-theory-misc.md | Rational-function certificate R(n,k) verified as one algebraic WZ-pair identity plus base case | cert_wz.py | **IMPLEMENTED** | worker.py:357; `gosper` present in cert_wz.py. The doc's rigorous gamma/limit justification remains out of scope, as the doc says. |
| `bernstein` polynomial-nonnegativity certificate | flyspeck-kepler.md | Largest Bernstein coefficient bounds the polynomial on [0,1] | cert_bernstein.py | **IMPLEMENTED** | worker.py:369. |
| `positivstellensatz` refutation datatype | hol-light-manuals.md | Triple of (equalities, non-strict, strict) combined to `0 > 0`; Farkas is the linear instance | cert_positivstellensatz.py | **IMPLEMENTED** | worker.py:389. |
| LP modified-dual (Farkas) certificate | flyspeck-kepler.md (ranked #1, "most load-bearing") | Rationalize the untrusted float dual, clamp negatives, verify by ONE integer-weighted row sum, no search | cert_flyspeck_lp.py | **IMPLEMENTED** | Module docstring names the modified dual and the repair-is-untrusted split explicitly; worker.py:349. |
| `taylor_model` certificate | analysis-transcendentals.md, floating-point-numerics.md | Carry (T, Delta) and re-verify the remainder bound | cert_taylor_model.py | **IMPLEMENTED** | worker.py:449. |
| `continued_fraction` hardness certificate | analysis-transcendentals.md | Diophantine bound on closeness to a boundary | cert_continued_fraction.py | **IMPLEMENTED** | worker.py:453. |
| `bnb` branch-and-bound certificate | flyspeck-kepler.md (LP case-splitting) | Case-split when the linear relaxation is too weak | cert_bnb.py | **PARTIAL** | `cert_bnb.py` ships and is dispatched (worker.py:377), but `grep case_split` returns nothing; whether it is the doc's `x <= a OR a <= x` re-relaxation was not confirmed from the module name alone. |
| Diophantine / Hensel hard-case generator + exclusion zone | analysis-transcendentals.md, flyspeck-kepler.md, mark10 | Enumerate the finite exceptional set by parity plus Hensel lifting, check the exclusion inequality | components/tools/python/theoremata_tools/falsify_hardcase.py | **IMPLEMENTED** | `hensel`, `exclusion_zone`, `near_root`, `balanced_factorization`, `worst_cases` all present. |
| Untrusted-input screen / guardrails facade | agentic A2, A4 (TOP GAP 3) | Named policy facade over the existing enforcers plus an injection screen for untrusted text | components/reason/critique/guardrails.rs | **IMPLEMENTED** | `Policy` enum with `enforces`/`is_soundness_authority`, `InjectionKind`, `check_untrusted`, `Guardrails::{policy_report, soundness_authority, screen_untrusted}`. |
| Explicit `UntrustedSource` boundary | agentic A4 | Untrusted external text can never influence a certified result, as testable infrastructure | components/reason/critique/guard.rs, guardrails.rs | **IMPLEMENTED** | `wrap_untrusted` present in both. |
| Checkpoint / rollback over validated graph state | agentic A4 | Explicit rollback-to-checkpoint beyond transactional commits | components/graph/db.rs | **PARTIAL** | `checkpoint` hits only in eval/proof_grader.py and its test, not in graph/db.rs. Graph-level rollback not evidenced. |
| Unified system-prompt layer carrying shared invariants | agentic H1 #1, H3 §3.2, A1, A5 | One composable preamble so abstention / no-sorry / untrusted-text rules ride on every call | components/reason/orchestration/context_assembly.rs | **PARTIAL** | `PromptAssembler` exists in context_assembly.rs and is used by agent.rs, but `compose_system` and a versioned `system/shared` registry are absent. |
| Versioned prompt registry (`system/shared/v1`, `system/role/ROLE/v2`) | agentic H3 §3.2, H5 | Named versioned templates owned Rust-side so invariants cannot drift | components/reason/orchestration/ | **NOT-BUILT** | `grep "system/shared"` returns nothing. |
| Token counter / silent-truncation defense | agentic H3 §18.2.1 | Pre-flight token counting with explicit overflow handling | context_assembly.rs | **NOT-BUILT** | `token_budget` and `max_tokens` both absent; only `token_count` in train/progress_sft.py. Silent truncation can still drop a constraint block. |
| GDPO normalize-then-sum multi-reward aggregation | agentic H2 (TOP GAP 2) | Normalize each reward channel before summing to stop advantage collapse | components/train/python/theoremata_tools/grpo_upgrades.py | **IMPLEMENTED** | `GDPO` present in grpo_upgrades.py with a test file. |
| TIS/MIS vLLM logprob-mismatch correction | agentic H2 (TOP GAP 3a) | Importance-sampling correction for the generation-vs-training logprob divergence that biases gradients | components/train/python/theoremata_tools/grpo.py | **NOT-BUILT** | `grep importance_sampling` returns nothing. Doc itself flags this as go-live-gated, not now. |
| Contamination detection for eval claims | agentic H2 (§b pitfalls) | Canary strings / n-gram overlap beyond the `usage_tag == "test"` split | components/eval/python/theoremata_tools/eval_harness.py | **PARTIAL** | `contamination` present in eval_harness.py and grader.py; `canary` absent, so the canary-string half is missing. |
| Abstention as a first-class eval axis | agentic H2 (§b) | Score abstention precision/recall against the formal verdict | eval_harness.py, agent.rs | **PARTIAL** | `abstained` and `abstain_threshold` present; a precision/recall axis was not evidenced. |
| Adversarial audit mode (assume malicious intent) | flyspeck-kepler.md §10 | Re-run scripts, confirm the RIGHT theorem was stated, hunt rogue axioms, guard cross-system translation | axiom_audit.rs, statement_preservation.rs, statement_validation.rs | **PARTIAL** | The three enforcement pieces exist and are strong; there is no single named `audit_mode` entry point, and cross-backend translation guarding is moot because the design forbids translation. |
| Content-addressed theorem fingerprint for distributed reassembly | flyspeck-kepler.md (ranked #2) | Hash of the theorem plus its whole history of constants, types and axioms; import iff the hash matches | blueprint_run.rs, graph/db.rs, proof_log.rs | **NOT-BUILT** | `grep axiom_fingerprint` returns nothing. |
| Automath "book" discipline for cert-log (assumption / definition / primitive lines) | papers-and-foundations.md | Line l+1 admitted only against the checked prefix; `primitive` lines surface oracle-trusted inputs explicitly | cert_log.py, proof_log.rs | **NOT-BUILT** | `cert_log.KINDS` is a flat 5-kind tuple; no line-species discipline. `primitive_notion` hits only formal.rs (Wiedijk foundation profile, unrelated). |
| Oracle / reflection trust-tagging (`TOOL |- phi`, tainted tier) | hol-light-reflection-cas.md (ranked HIGH) | Tag a theorem with the untrusted tool it depended on so downstream inherits the dependence | axiom_audit.rs, graph/evidence.rs, verify.rs | **PARTIAL** | `native_decide` is recognized and left to config policy; `EdgeStrength` exists in the graph model. No `trust_tier` token and no explicit tool-name inheritance chain. |
| Search/check tool contract with trust-tier tagging | hol-light-system.md (P0) | Every helper returns an untrusted certificate re-checked in the kernel, tagged kernel-chained / proof-of-algorithm / extracted-and-run | graph/evidence.rs, verify.rs | **PARTIAL** | The producer/checker split is real throughout `cert_*.py`; the three-tier tag itself is absent (`grep trust_tier` empty). |
| Cross-backend certificate replay | hol-light-system.md (P1), flyspeck-kepler.md | Replay the same certificate in a second kernel with a different bug surface | prover/backends/mod.rs, cert_log.py | **NOT-BUILT** | `grep cross_backend` empty. Portfolio proving races backends on the same statement; it does not replay one certificate in two kernels. |
| Bounded "obvious-step" justifier gate | hol-light-misc-short.md (ranked #2) | A tableau/MESON prover with a hard inference limit decides whether a claimed step is obvious enough to accept | prover/session/verify.rs, critique/critic.rs | **NOT-BUILT** | `inference_budget` and `inference_count` both absent. `model_elimination.rs` exists but is not wired as a bounded step-acceptance gate. |
| Compute-then-validate contract ("never lies / return bottom") | analysis-transcendentals.md | Heuristic guess then prove the bound; on failure return bottom rather than a wrong answer | verify.rs, guardrails.rs | **PARTIAL** | Fail-closed is the pervasive discipline (gate, precheck, witness) but there is no named `compute_then_validate` contract with the eta quality parameter. |
| Decidability-map capability boundary to abstention | euclidean-vectors-geometry.md | Encode "IP with k vars decidable in R^k via RCF; normed/metric undecidable, abstain" | guardrails.rs, meta-gate | **NOT-BUILT** | `dimension_collapse` absent. |
| `parity_scale` reduction generator | floating-point-numerics.md | round(2^n x) = 2^n round(x) reduces the infinite hard-case family to the n=0 representative | falsify_hardcase.py | **NOT-BUILT** | `grep parity_scale` empty. Directly completes the shipped `exclusion_zone`. |
| Upgrade `near_root` with a Sturm completeness certificate | floating-point-numerics.md | Heuristic root-hugging becomes "provably all roots in [a,b]" | falsify_hardcase.py + cert_sturm.py | **NOT-BUILT** | Both halves ship; nothing joins them. `falsify_hardcase.py` does not import cert_sturm. |
| `fp_rounding` certificate | floating-point-numerics.md | ROUND_CONV analog: format, mode, rational input, claimed round and ulp bound, recomputed exactly | cert_fp_rounding.py | **NOT-BUILT** | absent. First IEEE-rounding cert. |
| `fp_error_bound` certificate | floating-point-numerics.md | Serialize fma-chain (magnitude, abs/rel error) triples; checker re-propagates in exact-rational intervals | cert_fp_error_bound.py | **NOT-BUILT** | absent. |
| `fp_exclusion` certificate | floating-point-numerics.md | Straddling floats plus mpmath.iv distance to `f^{-1}(t)` | cert_fp_exclusion.py | **NOT-BUILT (checker); BLOCKED (full quantifier)** | absent. Doc itself gates the "no float exists over the whole format" claim on a verified inverse bound. |
| `analytic_log_linear` width certificate | analysis-transcendentals.md | Four boundary inequalities at H = 99...9 for the minimal multiplier bit-width | cert_log_linear.py | **NOT-BUILT** | absent. |
| `sos_interval` univariate Positivstellensatz | analysis-transcendentals.md (ranked #1 of that doc) | p >= 0 on [a,b] via squarefree + epsilon-perturbation + two-square SOS with the (a+b y^2)/(1+y^2) change of variable | cert_sos.py extension | **NOT-BUILT** | `sos_interval` hits only `components/verify/tests/test_wolfram_cert.py`, i.e. a test string, not a checker. |
| `sos_multivariate` | analysis-transcendentals.md | P + Q + R^2 = 0 refutation; exact LDL^T PSD test emits the `f(u) < 0` witness | cert_sos.py | **NOT-BUILT (checker); BLOCKED (generator)** | absent. Generator needs an external SDP solver per the doc. |
| `factorization` certificate | hol-light-reflection-cas.md | Untrusted CAS returns factors; checker multiplies out | cert_factorization.py | **NOT-BUILT** | `factorization` hits only geometry_ddar2.py and falsify_hardcase.py (`balanced_factorization`), not a cert kind. |
| `antiderivative_by_diff` certificate | hol-light-reflection-cas.md | Check a CAS antiderivative by differentiating it, with a cascaded residual pass | cert_antiderivative.py | **NOT-BUILT** | `antiderivative` hits only a benchmark JSONL. |
| `summation_by_differencing` certificate | hol-light-reflection-cas.md | Check a claimed closed-form sum by differencing / induction | cert_summation.py | **NOT-BUILT** | absent. |
| `cas_numeric_error_bound` certificate | hol-light-reflection-cas.md | Never trust CAS digits; assert `|pi - 3.14159...| < 10^-18` with a checked accuracy bound | cert_numeric.py | **NOT-BUILT** | absent. |
| `cooper` / Presburger QE trace certificate | hol-light-manuals.md | Cooper 1972 QE over Z/N with divisibility; certificate is the QE trace | cert_cooper.py | **NOT-BUILT** | `grep cooper` empty repo-wide. |
| RCF QE sign-matrix certificate format (`interpmat`) | hol-light-manuals.md, hol-light-system.md | Cohen-Hormander sign matrix encoded as one term so a rewrite is one step | verify python | **NOT-BUILT** | `grep interpmat` empty. Doc marks it lowest priority, format-only. |
| `ideal_cofactors` as a first-class emitted object | hol-light-manuals.md (ranked #1 there) | Return the cofactor list `[q1..qn]` with `p = sum p_i q_i` as the certificate object | cert_nullstellensatz.py | **PARTIAL** | The nullstellensatz cert does cofactor checking, but there is no `ideal_cofactors` named object/kind. |
| Superposition / completion replay certificate | course-slides-part2.md | Log each critical-pair overlap (position, mgu), orient decision, rewrite, with ordering side conditions | cert_log.py new kind | **NOT-BUILT** | `grep superposition` empty; `rewriting.rs` computes critical pairs but emits no replayable certificate. |
| `counterexample_norm` model finder | euclidean-vectors-geometry.md, hol-light-system.md | Universal additive normed-space procedure emits a concrete counterexample norm on R^n | tools python | **NOT-BUILT** | absent. Doc marks it weak/gated. |
| Verifier-failure as a negative certificate | hol-light-reflection-cas.md (ranked HIGH) | Record a tactic/verifier failure as structured evidence and route it to falsify plus the critic | critic.rs, falsify router | **PARTIAL** | `error_feedback.rs` and `critic.rs` consume failures, but there is no `negative_certificate` object in the cert-log envelope. |
| BIT0/BIT1 numerals as rewrite rules, not an `mk_thm` schema | hol-light-misc-short.md | Binary numeral arithmetic as rewrite rule sets, replacing an unsound axiom schema | Candle term layer | **SKIPPED (doc reason)** | Doc says reference-only: Candle already supplies the kernel; keep on file only if the harness ever needs its own term layer. `grep BIT0` empty, consistent. |
| Mutable-strings / host-language aliasing trust note | hol-light-system.md | OCaml string mutability weakens abstract-type protection even in a correct kernel | docs/TRUST_BOUNDARIES.md | **NOT-BUILT** | `TRUST_BOUNDARIES` appears only as a doc-comment reference in components/tools/mod.rs; no such document in the repo root docs tree was found by that grep. |
| de Bruijn criterion + four reliability mechanisms as trust documentation | papers-and-foundations.md | Document the gate's tiers against Geuvers' menu and state the residual metatheoretic assumption | trust docs | **NOT-BUILT** | `de_bruijn_criterion` absent. |
| MCP tool safety annotations (readOnly / destructive / idempotent / openWorld) | agentic H4 §21.5.3 | Machine-readable per-tool safety hints so hosts can gate or auto-approve | components/tools/python/theoremata_tools/worker.py | **NOT-BUILT** | `grep readOnlyHint` empty. |
| Docker-default episodic sandbox | agentic H4 §20.3.1 | Container isolation as the default per-episode rather than degrading to not-launched | components/prover/session/exec.rs | **PARTIAL** | `Runner::Docker` exists and is per-system configurable, but Docker is not the default. |
| Golden-route regression test | agentic H5 §25.5.3 | Assert the agent takes the same router route sequence on fixed inputs | reason tests or eval | **NOT-BUILT** | `grep golden_route` empty. |
| General proof plus finite exceptional-set enumeration | flyspeck-kepler.md, mark10 | Prove the general case, isolate a finite exceptional set, discharge each explicitly | guardrails.rs, falsify_hardcase.py | **PARTIAL** | `exclusion_zone` ships in falsify_hardcase.py; the "general theorem plus enumerated exceptions" composition is not a first-class harness pattern. |
| True first-order subsumption (C sigma subset of D) | course-slides-part1.md | Matching modulo substitution so `P(x,f(x))` subsumes `P(b,y)` | components/reason/search/rewriting.rs, subsumption.rs | **IMPLEMENTED** | `rewriting.rs:236 pub fn subsumes` with `matches` at :206 built on `unify`/`apply_subst`. |

---

## 2. CAPABILITY / QUALITY items

| Item | Source doc | Mechanism (one line, concrete) | Target file(s) | Status | Evidence |
|---|---|---|---|---|---|
| First-order unification producing an explicit mgu | course-slides-part1.md ("the one real gap") | Rule-based decompose/delete/orient/eliminate with occur check, solved form to idempotent mgu | components/reason/search/rewriting.rs:163 | **IMPLEMENTED** | `pub fn unify(s,t) -> Option of Subst`. The doc's "real gap" is closed. |
| LPO term ordering | course-slides-part2.md (highest-leverage of that batch) | Lexicographic path ordering from a symbol precedence | rewriting.rs:300 | **IMPLEMENTED** | `pub fn lpo` + `LpoOrdering`. |
| KBO term ordering | course-slides-part2.md | Knuth-Bendix ordering from precedence plus admissible weights | rewriting.rs:423 | **IMPLEMENTED** | `pub fn kbo` + `KboWeights` + `KboOrdering`. |
| Multiset ordering (Dershowitz-Manna) | course-slides-part1/2.md | Lift a base well-founded ordering to multisets | rewriting.rs:477 | **IMPLEMENTED** | `pub fn multiset_compare`. |
| Critical-pair computation | course-slides-part2.md | Overlap rule LHSs at non-variable positions via mgu | rewriting.rs:683 | **IMPLEMENTED** | `pub fn critical_pairs`. |
| Knuth-Bendix (unfailing) completion loop | course-slides-part2.md | Orient / Delete / Deduce / Simplify to a convergent TRS | rewriting.rs:742 | **IMPLEMENTED** (different name) | `pub fn complete(...)`; `grep knuth_bendix` misses because the function is `complete`. |
| Demodulation / rewriting by unit equations | course-slides-part2.md | Rewrite `C[s sigma]` to `C[t sigma]` when `s sigma > t sigma` | rewriting.rs:675 | **IMPLEMENTED** | `pub fn demodulate` plus `normal_form`. |
| Congruence closure | course-slides-part2.md | Union-find over terms deciding ground equality | rewriting.rs:820 | **IMPLEMENTED** | `pub fn congruence_closure`. |
| Ordered resolution with selection | course-slides-part1.md | Restrict inferences to strictly maximal literals under a substitution-stable ordering | components/reason/search/inverse_method.rs | **NOT-BUILT** | `grep ordered_resolution` empty; `inverse_method.rs` exists but the ordering restriction is not evidenced. |
| Subsumption resolution | course-slides-part1.md | Delete the complementary literal from `C or D sigma or (not L) sigma` | subsumption.rs | **NOT-BUILT** | `grep subsumption_resolution` empty. |
| Condensation | course-slides-part2.md | Replace a clause by a proper factor of itself that subsumes it | subsumption.rs | **NOT-BUILT** | absent. |
| Feature-vector subsumption index (Schulz) | course-slides-part2.md (ranked #2) | Monotone integer clause features in a trie as an imperfect filter, then exact recheck | subsumption.rs | **NOT-BUILT** | `grep feature_vector` empty. |
| Discrimination-tree / substitution-tree term index | course-slides-part1/2.md, papers-and-foundations.md | Preorder-string trie with `*` for variables, answering instance/generalization/unifiable queries | reason/search index module | **NOT-BUILT** | both `discrimination_tree` and `substitution_tree` empty. |
| Tseitin definitional CNF | course-slides-part1.md | Linear-size clausification, <= 4 clauses per subformula | reason/search or tools python | **NOT-BUILT** | absent. Doc itself rates it marginal for us. |
| Sign-based bi-implication splitting + NNF/epsilon-Skolemization | hol-light-misc-short.md | Split `p <=> q` by sign into many easy subgoals | reason/search preprocessing | **NOT-BUILT** | `grep biimp_split` empty. |
| Divide-and-conquer inference-bounded search | hol-light-misc-short.md (top of that batch) | With n inferences left and subgoals g1,g2 one has a proof of size <= n/2; try that split first | reason/search MCGS driver | **NOT-BUILT** | `grep divide_and_conquer` empty. |
| Continuation caching / lemmaizing of solved subgoals | hol-light-misc-short.md | Memoize solved subgoals across the search | components/reason/search/goal_cache.rs | **IMPLEMENTED** | `goal_cache` present in reason/mod.rs and graph/db.rs. |
| Declarative proof IR (Mizar-mode skeleton to tactics) | hol-light-misc-short.md | assume->DISCH_TAC, let->GEN_TAC, take->EXISTS_TAC, ... with thesis tracking | reason/proving node schema | **NOT-BUILT** | `grep declarative_ir` empty. |
| Candle/HOL-Light API catalog (holchart transcription) | hol-light-misc-short.md | JSON vocabulary of theorems/rules/conversions/tactics to seed retrieval and guardrail generation | prover/backends resource | **NOT-BUILT** | `grep tactic_catalog` empty. |
| Difficulty-laddering benchmark generator | hol-light-misc-short.md | Ship closed "obvious" subgoals as an ATP set, then excise steps until a target difficulty | eval harness | **PARTIAL** | `difficulty.py` and `lifelong_curriculum.py` exist for training curricula; the excision generator is not evidenced. |
| TPTP-wide benchmark harness (runtime / inference count / proof size) | hol-light-misc-short.md | Per-problem table across multiple search modes | eval harness | **NOT-BUILT** | `inference_count` absent. |
| WLOG frame-normalization tactic (GEOM_ORIGIN chain) | euclidean-vectors-geometry.md (ranked [High]) | Translate a point to origin, rotate another onto an axis, drop the dimension | geometry_algebraic.py, DDAR2 preprocessing | **NOT-BUILT** | `geom_origin` absent. |
| Invariance-lemma registry | euclidean-vectors-geometry.md (ranked [High]), hol-light-system.md | Table tagging each predicate with the transform group it is invariant under; bottom-up rewrite cancels the transform | geometry_algebraic.py | **NOT-BUILT** | `invariant_under` and `invariance_database` both absent. |
| Symmetry / orbit dedup in the proof DAG | euclidean-vectors-geometry.md | Prove one representative of a variable-permutation orbit | reason/search driver, subsumption.rs | **NOT-BUILT** | `orbit_dedup` absent. |
| `QUANTIFY_SURJECTION_THM` quantifier rewriting under a transform | euclidean-vectors-geometry.md | Rewrite forall/exists (and set comprehensions) under a surjective transform in one non-looping pass | Candle backend | **NOT-BUILT / gated** | absent. Doc gates this on Candle maturity and the multivariate Euclidean theory. |
| Higher-order WLOG lift to structured configs | euclidean-vectors-geometry.md | Apply an orthogonal transform as `MAP g` over a vertex list | geometry_synth configs | **NOT-BUILT** | absent. Doc marks [Low/gated]. |
| inside/outside via connected components + Jordan-Triple-Curve shelf | euclidean-vectors-geometry.md | `inside s = {x | x not in s and bounded(connected_component ...)}` plus a crossing-parity lemma | geometry topology library | **SKIPPED (doc reason)** | Doc: thousands of lines (Pick cost 3709), only if area/polygon goals become a target class. `connected_component` absent, consistent. |
| Additivity / inclusion-exclusion set-function lemma | euclidean-vectors-geometry.md | `f(s u t) = f(s) + f(t) - f(s n t)` applied to lattice-count and area | geometry lemma shelf | **NOT-BUILT** | absent; doc marks [Low]. |
| Type-encodes-dimension modelling (`dimindex`, `A^N`) | hol-light-system.md | Encode dimension as a type variable so the type system enforces dimensional constraints | FormalSystem contract | **NOT-BUILT** | `dimindex` absent. |
| VECTOR_ARITH componentwise reducer | hol-light-system.md | Reduce vector identities to componentwise real arithmetic | geometry_algebraic.py | **NOT-BUILT** | `vector_arith` absent. |
| Solovay vector-space procedure (Gram-Schmidt + SOS via CSDP) | hol-light-system.md | Gram-Schmidt reduction then REAL_SOS, translating the SDP certificate into inferences | geometry_algebraic.py | **NOT-BUILT / gated** | `gram_schmidt` absent; CSDP dependency. |
| Polynomial normal-form + exact-rational ground reduction primitives | hol-light-manuals.md | REAL_POLY_CONV canonical form, SEMIRING_NORMALIZERS_CONV, arbitrary-precision rational reduction | cert_log.py `_CheckPoly` | **IMPLEMENTED (equivalent)** | `_CheckPoly` pseudo-division exists in cert_log.py and is reused by cert_sturm.py and cert_nullstellensatz.py; the HOL-named conversions are not ported verbatim. |
| Shadow-function idiom | hol-light-system.md (P1) | Fast unverified explorer alongside the proof-producing version that certifies at commit time | candle.rs, tools python | **PARTIAL** | The producer/checker split is exactly this pattern throughout `cert_*.py`; there is no named `shadow_function` seam. |
| de-Bruijn-factor report + hand-waving detector | hol-light-system.md (P1), number-theory-misc.md | Report informal-to-formal blowup per certificate and flag steps with an outlier factor | proof_log.rs, trace.rs, critic.rs | **PARTIAL** | `handwave` hits components/reason/proving/library.rs; `de_bruijn` absent, so the numeric factor metric is not there. |
| Machine-learned premise selection + ATP/ITP flywheel | flyspeck-kepler.md | Learned premise selection over a growing lemma library with a mutual-improvement loop | graph_rag.rs, library.rs, dense index | **PARTIAL** | `graph_rag.rs` ships and BM25 + cascade retrieval exist; `premise_selection` as a named learned component is absent. |
| Blueprint / formal-abstract design rule | flyspeck-kepler.md | Few reusable concepts, explicit hypotheses, short independent chapters to parallelize | blueprint_generate.rs | **IMPLEMENTED** | blueprint_generate.rs and blueprint_run.rs ship. |
| Provenance cross-linking blueprint text to formal obligations | flyspeck-kepler.md (ranked last) | Stable cross-revision identifiers tying informal text to formal obligations | graph/evidence.rs, trace.rs | **PARTIAL** | `evidence.rs` carries provenance; `blueprint_ref` cross-links absent. |
| Efficiency-lever doctrine (proforma theorems, memoization, lazy checking) | hol-light-reflection-cas.md | Prove a schema once and instantiate; memoize trivialities; batch deferred checking | goal_cache.rs, model.rs | **PARTIAL** | `goal_cache` exists; `proforma` and lazy/batched checking absent. |
| Retrieval over a named-theorem library plus library-coverage gap signal | hol-light-system.md (P1) | Index by name/statement/file; measure coverage gaps as a driver signal | library.rs, graph_rag.rs | **PARTIAL** | `library.rs` and `graph_rag.rs` ship; coverage-gap-as-driver-signal not evidenced. |
| Retrieval seed corpus (Dirichlet / complex analysis / PNT) | number-theory-misc.md | Ingest structured lemma inventories as retrieval content | library.rs, retrieval index | **NOT-BUILT** | `grep dirichlet` empty. |
| Triangle-law norm reasoning + COMPLEX_FIELD algebra tactics | number-theory-misc.md | Under-automated recurring niches Harrison flags | prover tactic layer | **NOT-BUILT** | `triangle_law` absent. Doc marks it conditional. |
| HoTT eval fixtures / agda-unimath corpus | survey-2212.md | Add pi_1(S^1)=Z etc. as eval targets; index agda-unimath for retrieval | eval fixtures, retrieval | **NOT-BUILT** | `agda-unimath` absent. Doc marks both optional/low. |
| 1Lab ingestion with pinned revision and cubical tagging | AGDA-1LAB-METAMATH-SPEC.md | Index module declarations, imports, dependency closure; tag cubical modules | eval benchmarks | **PARTIAL** | `components/eval/python/theoremata_tools/benchmarks/agda_1lab.py` ships; revision/Agda-version pinning and cubical tagging were not confirmed. |
| Lean warm REPL driver (`proofState` per-tactic stepping) | lean.md, INTEGRATION-PLAN.md | leanprover-community/repl over JSON stdin/stdout with env/proofState ids | prover/backends/lean.rs, lean_session.rs | **IMPLEMENTED** | `proofState` in lean.rs and formal.rs; `lean_repl` worker tool. |
| Rocq Petanque / SerAPI driver | rocq.md, INTEGRATION-PLAN.md | coq-lsp Petanque start/run/goals, or sertop Add/Exec/Query Goals | prover/backends/rocq.rs, python/rocq_driver.py | **IMPLEMENTED** | `Petanque`, `SerAPI`, `sertop`, `coq-lsp` all present. |
| Isabelle Server driver (`session_start` / `use_theories`) | isabelle.md, INTEGRATION-PLAN.md | TCP server, submit a whole `Scratch.thy`, parse `use_theories_results` | python/isabelle_driver.py | **IMPLEMENTED** | `session_start` and `use_theories` present. |
| Project scaffolds (lakefile / `_CoqProject` / session ROOT) | lean.md, rocq.md, isabelle.md | Per-system buildable workspace around generated code | lean.rs, rocq.rs, isabelle.rs | **IMPLEMENTED** | `lakefile`, `_CoqProject`, `ROOT` all present. |
| Sledgehammer as an agent-callable hammer | isabelle.md, INTEGRATION-PLAN.md phase 4 | Fire `sledgehammer [provers=..., timeout=t]` headless, parse the `by (metis ...)` reconstruction | prover/python/theoremata_tools/hammer.py | **IMPLEMENTED** | `_build_sledgehammer_theory`, `_parse_sledgehammer`, real and mock tiers. |
| CoqHammer (`sauto`/`hauto`) | rocq.md, INTEGRATION-PLAN.md | Pure tier plus external ATP tier with kernel-checked reconstruction | hammer.py | **PARTIAL** | `sauto`/`hauto`/`CoqHammer` present in hammer.py; INTEGRATION-PLAN itself records it as gated on `opam install coq-hammer-tactics` with mock fallback. |
| `aesop` / Duper as the Lean-side hammer | lean.md, INTEGRATION-PLAN.md | Closest thing to a tactic hammer for Lean | prover backends | **PARTIAL** | `aesop` hits only eval/exposition.py and a test; `duper` hits conjecture_engine.rs and library.rs as text. Not wired as a hammer tool the way Sledgehammer is. |
| `nitpick` / `quickcheck` as falsifiers | isabelle.md | Counterexample search alongside the hammer | hammer.py | **PARTIAL** | Named in hammer.py docstrings and the mock path only. |
| Isabelle `find_theorems` / `find_consts` retrieval | isabelle.md | In-session premise search | components/retrieval/isabelle/find_theorems_template.thy, python/isabelle_retrieval.py | **IMPLEMENTED** | both files present. |
| Find_Facts JSON REST retrieval | isabelle.md | Solr REST search over Main/HOL plus AFP | retrieval | **NOT-BUILT** | `grep Find_Facts` empty. |
| Loogle / LeanSearch / Moogle retrieval | lean.md, MCP_INTEGRATION.md | Structural and semantic mathlib search | retrieval | **NOT-BUILT** | both `Loogle` and `LeanSearch` empty repo-wide. |
| `isabelle mirabelle` batch eval | isabelle.md | Batch-run automation over a session for evaluation | eval | **NOT-BUILT** | `grep mirabelle` empty. |
| Portfolio proving (race all backends, first to certify wins) | INTEGRATION-PLAN.md phase 5 | Per-system formalization, race, no cross-translation | reason/proving/formalize_portfolio.rs | **IMPLEMENTED** | `portfolio_prove` present; `generate_and_verify` in formal_generate.rs. |
| No cross-system translation (design rule) | INTEGRATION-PLAN.md decision 1 | Each backend emits system-native code; portfolio races, never translates | formal.rs, formalize_portfolio.rs | **IMPLEMENTED** | No translation path exists; each backend has its own generator and signature checker. |
| Aristotle MCP client status-machine mapping | MCP_INTEGRATION.md §2c | `map_api_status()` normalizing raw ProjectStatus | prover/python/aristotle_mcp_client.py | **IMPLEMENTED** | `map_api_status` and `ProjectStatus` present with a test file. |
| `#line`-directive source mapping | MCP_INTEGRATION.md §4 | Map generated-file positions back to blueprint source | verify/prover | **NOT-BUILT** | both `line_directive` and `source_map` empty. |
| Verso as blueprint format | MCP_INTEGRATION.md §3 | Literate Lean blueprint authoring | reason/proving | **PARTIAL** | `Verso` appears only in evolve_sketch.rs as text, not as an ingestion path. |
| Context-assembly layer | agentic A1/A5/H1/H3 (most-repeated item in the slice) | One module composing system + memory + tool + retrieval + query blocks under a budget | components/reason/orchestration/context_assembly.rs | **PARTIAL** | Module and `PromptAssembler` exist and are called from agent.rs; the token budget, priority eviction, ToolBlock rendering and TaskContext are not evidenced (`token_budget`, `TaskContext` absent). |
| Meta-tool layer (plan / critique / redecompose / spend as tools) | agentic A2 (TOP GAP 1), A5, H3 §3.3 | Orchestration decisions become tools the model calls and observes | components/reason/orchestration/meta_tools.rs | **IMPLEMENTED** | meta_tools.rs ships and references BlueprintRefiner and refine_ops. |
| RunTrace span tree + failure taxonomy | agentic H5 (top 1), A4, A5, H3 | Named span tree over LLM/tool/route transitions plus a 6-class terminal-failure taxonomy | components/reason/orchestration/trace.rs | **IMPLEMENTED** | `SpanKind`, `SpanStatus`, `Span`, `RunTrace`, `to_forest`, `replay`, `FailureClass`, `Layer`, `ErrorContext`, `FailureTaxonomy::classify`; `span_id` persisted in graph/db.rs. |
| Trajectory scoring track in the eval harness | agentic A5 (TOP GAP 2), H2 | Score the logged action sequence against a gold path | components/eval/python/theoremata_tools/trajectory_eval.py | **IMPLEMENTED** | file ships with a test; `tool_use_accuracy` present there. |
| Tool-Use Accuracy metric | agentic H2 | Grade tool calls for correct tool AND valid args AND right moment | trajectory_eval.py | **IMPLEMENTED** | `tool_use_accuracy` in trajectory_eval.py and its test. |
| Unbiased pass@k estimator | agentic H2 | `1 - C(n-c,k)/C(n,k)` instead of any-of-k boolean | eval_harness.py | **IMPLEMENTED** | `pass_at_k` in eval_harness.py and grader.py. |
| 2-GRPO (G=2) preset | agentic H2 (high-value, cheap) | `num_generations=2` for 4-6x rollout savings | train/grpo.py | **IMPLEMENTED** | `num_generations` in grpo.py and grpo_upgrades.py. |
| Soft-overlong length penalty (DAPO fix 4) | agentic H2 | Smooth length penalty rather than hard truncation mask | train/grpo.py | **NOT-BUILT** | `soft_overlong` absent; only `mask_truncated_completions` (the hard mask) exists. |
| Trained V(s) wired into live MCGS node priority | agentic H2 (TOP GAP 3b) | Feed the learned value head into search node priority | critic_scorer.rs, distance_critic.rs, driver.rs | **PARTIAL** | All three files exist and `process_supervision` is referenced from critic_scorer.rs; consumption of a trained value at search time is GPU-gated per the doc. |
| Dynamic curriculum advancement gate | agentic H2 | Advance difficulty past a success threshold, keep 10-20% easy | train/lifelong_curriculum.py, difficulty.py | **IMPLEMENTED** | both files present. |
| LLM-judge debiasing (position swap, panel, logprob weighting) | agentic H2 | Augment judged pairs and weight by logprobs | eval/proof_grader.py, benchmarks/graders.py | **PARTIAL** | `proof_grader.py` ships; the three debiasing mechanisms were not evidenced by token. |
| Episodic-reflection buffer feeding retries | agentic H2, H3 | Store NL self-critique and inject on the next attempt | critique/memory.rs, proving/refine_ops.rs | **IMPLEMENTED** | `plan_history`, `refine_ops`, `EpisodicMemory`/`MemorySnapshot` all present. |
| Inference-time episodic recall (few-shot from past solved trajectories) | agentic A3 (TOP GAP 2) | Retrieve nearest prior successful trajectories as exemplars at inference | train/trajectory_recycler.py, memory.rs | **PARTIAL** | `trajectory_recycler` ships but feeds training; inference-time recall not evidenced. |
| Dense semantic index over the lemma library | agentic A3/A4/H4 | Embedding index replacing BM25-only access | retrieval, library.rs | **PARTIAL** | `bm25_retriever.py` and `cascade.py` ship; a dense embedding index was not evidenced. |
| GraphRAG retrieval over the lemma dependency DAG | agentic A4 (TOP GAP 1, "highest value") | Retrieve premises by graph proximity in the proof DAG | reason/proving/graph_rag.rs | **IMPLEMENTED** | graph_rag.rs plus graph/db.rs support. |
| Query rewriting / expansion / HyDE | agentic A4 | LM paraphrase plus notation variants before the first retrieval stage | retrieval/query_rewrite.py | **IMPLEMENTED** | `HyDE` and `query_rewrite` present with a test. |
| Pre-prover relevance gate (Self-RAG completion) | agentic A4 | Grade the retrieved set and re-retrieve before an expensive prover call | retrieval, graph_rag.rs | **PARTIAL** | `cascade.py` exists; pre-prover gating not evidenced. |
| Difficulty-aware model-tier router | agentic A4 (TOP GAP 2) | Route to cheap vs expensive model tiers by classified difficulty | provider, ttc.rs | **PARTIAL** | `model_for_role` exists in model_provider.py, i.e. role-based not difficulty-based. |
| Provider fallback ladder | agentic A4 | Primary to cheaper-model chain on throttle/outage | provider | **PARTIAL** | `THEOREMATA_MODEL_FALLBACK` grep was not run to completion; `model_for_role` is the only confirmed seam. |
| Unified budget accountant across TTC + retrieval + provider | agentic A4 §c | One budget all consumers draw from | reason/search/ttc.rs | **IMPLEMENTED** | `global_budget` in ttc.rs and driver.rs. |
| Contextual pruning / history summarization | agentic A4, H3 §18.2.3 | Explicit prompt-token trimming and history summarization | context_assembly.rs | **NOT-BUILT** | no token-budget machinery found. |
| Opt-in concurrent fan-out | agentic A1 (TOP GAP 1) | Run portfolio / best-of-N branches simultaneously behind the budget controller, deterministic default | reason/search/concurrent.rs | **IMPLEMENTED** | `components/reason/search/concurrent.rs` plus `multi_alpha_union`. |
| Semantic / embedding route classifier | agentic A1 | Reuse the dense index as a route classifier | proving/router.rs | **NOT-BUILT** | no dense route classifier evidenced. |
| Learned ML route classifier | agentic A1 | Train the selector/critic head as a discriminative router | train/selector.py, critic_scorer.rs | **PARTIAL** | `critic_scorer.rs` ships; not wired at a routing juncture. |
| Unified conversational-state object | agentic A1 | One accumulating turn-by-turn state over chat/memory/plan_history/goal_cache | orchestration/chat.rs, critique/memory.rs | **PARTIAL** | components exist separately; no unifying facade evidenced. |
| Offline prompt optimizer over the eval harness | agentic A1, H5 | Refine prompt text against eval metrics | eval, train | **NOT-BUILT** | `prompt_optimizer` absent. Doc rates it low priority. |
| Dynamic tool selection from a self-describing manifest | agentic A2 | Emit a tool manifest with schemas so the model selects among the worker tools | tools/worker.py | **NOT-BUILT** | `tool_manifest` absent; worker.py is still an `if tool == ...` ladder. |
| Agent-as-tool wrapper | agentic A2 | Uniform "invoke sub-agent X as a tool" seam | orchestration/team.rs, method_transfer.rs | **PARTIAL** | `method_transfer.rs` ships; no uniform AgentTool seam. |
| A-priori plan critic | agentic A2 (TOP GAP 2) | Score a fresh blueprint for decomposition quality before any proving spend | critique/critic.rs, blueprint_generate.rs | **IMPLEMENTED** | `BlueprintRefiner` in blueprint_generate.rs and meta_tools.rs. |
| HITL plan-approval checkpoint | agentic A2, H3 gap 3 | Surface the blueprint for approve/edit before spend; wire the Proposal table | blueprint_generate.rs, graph/model.rs | **PARTIAL and contested** | `Proposal` exists in graph/db.rs and model.rs but is not wired into the loop. A3 argues against inline HITL for correctness; see SKIPPED. |
| Budget-aware planning | agentic A2, A4 | Blueprint generator takes a compute budget and reallocates by expected payoff | ttc.rs, blueprint_run.rs | **PARTIAL** | `TtcController` and `global_budget` exist; blueprint-level reallocation not evidenced. |
| Procedural memory / self-editing instruction layer | agentic A3 (TOP GAP 1) | Versioned instruction store plus a reflection node proposing edits to its own rules, gated | orchestration prompt registry, graph/db.rs | **NOT-BUILT** | `procedural_memory` absent. |
| Revisable goal monitor with escalate transitions | agentic A3 (TOP GAP 3) | Treat the top objective as revisable, emit escalate rather than only re-decompose | live_plan.rs | **PARTIAL** | `live_plan.rs` ships and `escalate` appears in guard.rs and declaration_lookup.rs; a goal-level revise/escalate transition is not evidenced. |
| Abstention to human-review-queue escalation | agentic A3, H3 | Terminal abstention emits the stuck high-value subgoal to a review queue | certification.rs, agent.rs | **PARTIAL** | `abstain_threshold` exists; no review queue. |
| Human-label ingestion into the flywheel | agentic A3 | Offline batched marking schemes feeding curriculum_synth and reward | train/curriculum_synth.py, reward.py | **PARTIAL** | `curriculum_synth` ships; human-label ingestion path not evidenced. |
| Unified named exception-policy facade | agentic A3 | One place for retry / fallback / degrade / escalate | proving/repair.rs, refine_ops.rs | **PARTIAL** | `refine_ops.rs` ships; no single named policy facade. |
| Richer evolutionary population DB + multi-objective scoring | agentic A3 | OpenEvolve-style population database over evolve_sketch | proving/evolve_sketch.rs | **PARTIAL** | `evolve_sketch.rs` ships and carries `elo_`; a full population DB with multi-objective scoring is not evidenced. |
| Elo / tournament ladder over conjectures | agentic A5 (low priority) | Wrap preference_pairs in an Elo table | conjecture_engine.rs | **IMPLEMENTED** | `Elo` in eval/evolve.py with a test; `elo_` in evolve_sketch.rs. |
| Cross-run learned prioritiser | agentic A5 | Re-rank work using historical outcomes across runs | preference_pairs.rs, fitness.rs | **PARTIAL** | `preference_pairs` ships; cross-run reprioritization not evidenced. |
| Model-driven tool loop replacing the fixed pipeline | agentic A5 (TOP GAP 1) | while-not-done loop where the model picks tools; router becomes advisory | orchestration/agent.rs, router.rs | **PARTIAL** | `AgentLoop` exists in agent.rs; the doc's own caution is to keep the pipeline as default safety rails because the router enforces falsify-before-prove. |
| Live model-editable plan/todo object | agentic A5 | Promote plan_history from a log to a live todo the model rewrites | plan_history.rs, live_plan.rs | **IMPLEMENTED** | `live_plan.rs` ships and is referenced from agent.rs. |
| Role personas + version-controlled prompt library | agentic A5, H5, H4 | Prompt-selectable personas backed by versioned prompt files | orchestration prompt registry, formalize_modes.rs | **PARTIAL** | `role_for` in formal_generate.rs and `model_for_role` in the provider; no versioned prompt files. |
| Multi-turn session evalset schema | agentic A5 | Turn/session record carrying expected tool trajectory | eval/benchmarks/schema.py | **PARTIAL** | benchmarks package ships; a session schema was not evidenced. |
| Latency / token / cost meter per model call | agentic A5 | Persist tokens, latency and dollar cost per provider call | provider, graph/db.rs | **PARTIAL** | `ProofResult.cost` field exists in formal.rs; a per-call meter with aggregation was not evidenced. |
| Online monitoring, drift detection, A/B over agent versions | agentic A5 | Streaming monitor plus drift detection and cohort comparison over the event log | eval, graph/db.rs | **PARTIAL** | `drift` hits eval/benchmarks/adversarial.py and hypothesis_audit.rs; no A/B cohort machinery. |
| Unified eval + telemetry aggregator CLI | agentic A5 | One report folding scores with trajectory | eval, trace.rs | **NOT-BUILT** | not evidenced. |
| Task-contract object + clarify/negotiate step | agentic A5 (TOP GAP 3) | Machine-readable task contract at statement-validation time plus a clarify-or-abstain step | statement_validation.rs | **PARTIAL** | `statement_validation.rs` ships; no contract object or clarify step. |
| Grammar-constrained decoding | agentic H1 (rated highest-value new capability there) | Token-level constrained decoding behind the provider seam | provider/model_provider.py | **PARTIAL** | `response_format` (soft) present; no XGrammar/Outlines constrained decoding. |
| Context budgeting / lost-in-the-middle mitigation | agentic H1 | Place decision-relevant lemmas nearest the generation point, repeat rules at the tail | context_assembly.rs | **NOT-BUILT** | no budget machinery. |
| Per-role temperature knob | agentic H1 | Low temperature for verify roles, higher for sketch branches | provider/model_provider.py | **PARTIAL** | `model_for_role` exists; per-role temperature not evidenced. |
| Native tool-call schema instead of NL cues | agentic H1 | Use the provider's function-calling schema | provider, tools | **NOT-BUILT** | tied to the missing tool manifest. |
| Log-prob / consistency pre-filter before the gate | agentic H1 (doc says LOW priority) | Prioritize which candidates to verify first; the gate stays the authority | search/proof_pool.rs | **PARTIAL** | `proof_pool` exists as the pooling site; no logprob prefilter. |
| Meta-tools served over the MCP server, agent as MCP client | agentic H3 §3.3 | Expose plan/critique/retrieve/recall/remember/spend as MCP tools | meta_tools.rs, tools/mcp_server.py | **PARTIAL** | Both `meta_tools.rs` and `mcp_server.py` ship; the agent loop is not evidenced as an MCP client of its own meta-tools. |
| Tool descriptions injected into the prompt (ToolBlock) | agentic H3 §18.3.4 | Render tools/list descriptors, role-filtered, into the prompt | context_assembly.rs, tools | **NOT-BUILT** | `inputSchema` rendering into prompts not evidenced. |
| Retrieval-augmented tool selection at scale | agentic H3 §18.4.2 | Retrieve top-k relevant tools before prompting | retrieval, tools | **NOT-BUILT** | depends on the missing manifest. |
| Prompt caching of the stable system+tool prefix | agentic H3 §18.8.1 | Cache the system + tool-definition prefix across calls | provider/model_provider.py | **NOT-BUILT** | `prompt_cache` and `cache_control` both absent. |
| Importance-gated memory writes / contradiction resolution | agentic H3 | Explicit write-threshold policy and contradiction handling | critique/memory.rs, search/subsumption.rs | **PARTIAL** | `subsumption` dedup ships; an importance-gated write policy is not evidenced. |
| MemGPT tiered virtual context + self-directed memory ops | agentic H3 | Hot/warm/cold tiers plus model-issued memory_search/memory_write | memory.rs, meta_tools.rs | **NOT-BUILT** | `MemorySnapshot` is flat and deterministic by design. |
| Rust-side lemma-reuse seam (close `LemmaReuse::PythonSide`) | agentic H3 | Give lemma_cache.py a Rust seam | memory.rs, library.rs | **NOT-BUILT** | `LemmaReuse` still present in memory.rs as the honest-gap marker. |
| Aggregate metrics + trace replay tooling | agentic H3, H5 | Metrics aggregation plus re-running a past trace with a modified prompt | trace.rs, observe.rs | **PARTIAL** | `trace::replay` exists (span tree to steps); re-running with a modified prompt is not evidenced. |
| Irreversible-action / over-budget approval gate | agentic H3 (TOP GAP 3) | Escalate iff p below tau OR irreversible OR cost above B | agent.rs, graph/model.rs | **PARTIAL** | the p<tau branch exists (`abstain_threshold`, `LoopGuard`); the other two branches and the Proposal wiring do not. |
| MCP / OpenEnv server exposing the verified prover | agentic H4 (TOP GAP 1) | FastMCP facade over worker + exec + gate | tools/mcp_server.py | **IMPLEMENTED (doc is stale)** | `mcp_server.py` ships with `tools/tests/test_mcp_server.py`. H4 assumes it does not exist; A3 correctly records that it does. |
| Gymnasium-style reset/step environment | agentic H4 §20.4 | Typed StepResult with reset/step/state/close over the prover | prover/session/exec.rs, eval | **NOT-BUILT** | `StepResult` (the env one) absent; the `formal::StateResult` is a different thing. |
| First-class Skills layer (manifest, registry, router, lifecycle) | agentic H4 (TOP GAP 2) | Unify worker tools and library lemmas under one manifest with a dense skill router | library.rs, tools | **NOT-BUILT** | `SkillRegistry`, `skill_router`, `skills_manifest` all absent. |
| Agent Card + A2A task endpoint for EXTERNAL delegation | agentic H4 (TOP GAP 3) | `/.well-known` agent card plus async task lifecycle on the MCP server | tools, app/api.rs | **NOT-BUILT** | `agent_card`/`AgentCard`/`agent.json` all absent. |
| Runtime tool registration / introspection | agentic H4 §21.5.2 | Tools added/removed at runtime with a tools/list refetch | tools | **NOT-BUILT** | worker.py dispatch is static. |
| Correlation / span IDs on delegation chains | agentic H4 §23.5.4 | Workflow and span IDs threaded through delegations | trace.rs | **IMPLEMENTED** | `span_id` in trace.rs and graph/db.rs. Doc deferred it behind A2A; it landed anyway via RunTrace. |
| TUI live-DAG visualization + attempt-level replay | agentic H5 (top 2) | Render the claim DAG with per-node status; re-run one obligation's failing step | app/tui.rs, graph/model.rs | **PARTIAL** | `attempt_run.rs` and graph/db.rs support attempt granularity; the TUI DAG view was not evidenced. |
| Attempt-level checkpoint / time-travel replay | agentic H5 | Per-node snapshots you can rewind | graph/db.rs | **PARTIAL** | `attempt_run` ships; rewind/replay of a single attempt not evidenced. |
| Agent-level eval metric checklist | agentic H5 Ch.28 | steps-to-completion, tool-call accuracy, recovery rate, escalation rate | eval | **PARTIAL** | `trajectory_eval.py` covers tool-call accuracy and step counts; recovery and escalation rates not evidenced. |
| Inline tool-use cards in the TUI | agentic H5 (low priority) | Per-step cards with error highlighting | app/tui.rs | **NOT-BUILT** | folds into the missing TUI trace view. |

---

## 3. SKIPPED and BLOCKED (doc-stated)

| Item | Source doc | Doc's stated reason | Status |
|---|---|---|---|
| Rebuild the MCP server/client | agentic A3 §b | "MCP support is warranted and substantially already built. Do NOT rebuild it." | SKIPPED (confirmed: `mcp_server.py` + `aristotle_mcp_client.py` both ship with tests) |
| MCP dynamic discovery / federation | agentic A3 §b | Do it "only when a concrete second external server justifies the auth surface" | SKIPPED (deferred) |
| Consuming third-party MCP tools | agentic H4 §b | Adds an untrusted prompt-injection surface against a harness whose value is soundness | SKIPPED |
| Inline HITL checkpoints for correctness | agentic A3 §c | The kernel plus axiom allowlist is a deterministic oracle a human cannot out-verify; per-step HITL adds latency for zero correctness gain | SKIPPED (two narrow seams survive: offline curation, abstention escalation) |
| A2A wire protocol for internal agents | agentic A4 §d | "No, not now." One Rust harness with in-process specialists; agent cards and mTLS are a distributed-systems tax | SKIPPED |
| Internal multi-agent runtime (roles, personas, message bus) | agentic H4 §c | "NO, and mostly it should stay that way." Specialization already comes from routing + portfolio + backends | SKIPPED (directly contradicts A2 TOP GAP 3; H4 is later and more argued) |
| Message-passing conversational agents (AutoGen GroupChat) | agentic H5 | "adds nondeterminism we explicitly avoid" | SKIPPED |
| Adopting LangGraph / AutoGen / CrewAI / Semantic Kernel | agentic H5 | Adopting one "would be a regression": trades Rust's compiler-enforced typed contracts for Python glue | SKIPPED |
| Generative UI / streaming web UI / deployment infra | agentic H5 | Terminal-first batch/CLI harness, not a multi-tenant service | SKIPPED (N/A) |
| External moderation API guardrail | agentic A4 | The domain has no toxic-content surface | SKIPPED (N/A) |
| PPO / RLHF four-model loop, learned Bradley-Terry reward models | agentic H2 §a,§c | GRPO is critic-free and the formal verifier replaces the learned RM | SKIPPED by design |
| ELO / Arena / TrueSkill model ranking | agentic H2 §b | We evaluate against a fixed oracle, not by ranking models | SKIPPED (N/A) |
| BLEU / ROUGE / BERTScore | agentic H2 §b | Irrelevant to proofs; symbolic-equivalence EM is correct | SKIPPED |
| GOPO | agentic H2 §a | Applies to RM-based rewards; ours are verifiable | SKIPPED |
| GSPO / Dr.GRPO / SAPO / VESPO / CISPO | agentic H2 | "niche knobs, low priority given our binary verifiable reward" | SKIPPED |
| Listwise / Plackett-Luce critic ordering | agentic H2 §c | Low value under binary verifiable rewards | SKIPPED |
| Full SICA-style source-code self-modification | agentic A3 | "out of scope/risky for a proof kernel" | SKIPPED |
| Temporal-decay / recency-weighted memory reads | agentic H3 | Determinism is a stated invariant; MemorySnapshot ranks by proof-score and id deliberately | SKIPPED by design |
| Decoding zoo (beam/top-k/top-p/min-p/contrastive) and transformer internals | agentic H1 | Delegated to the provider or serving stack; "nothing to build" | SKIPPED |
| Gap-to-external-web-source retrieval | agentic A4 | Our corpus is closed by design | SKIPPED |
| Multi-agent debate / Graph of Debates | agentic A2, A4 | "research-grade"; A4 marks GoD "Later" | SKIPPED (deferred) |
| MESON / METIS / LEANCOP / NANOCOP as certificate sources | hol-light-manuals.md | They reconstruct through the kernel but emit no compact standalone certificate; map to a generic kernel-replay evidence record | SKIPPED (doc reason) |
| Ground-resolution refutation DAG as a cert-log kind | course-slides-part1.md | Duplicates guarantees the proof-assistant backends already give; doc explicitly declines to recommend it | SKIPPED (doc reason) |
| Native superposition / given-clause engine + CDCL(T) / AVATAR | papers-and-foundations.md | Large; defer unless shelling out to E/Vampire proves insufficient | SKIPPED (deferred) |
| Verified reflective checker inside Candle (computational reflection) | hol-light-reflection-cas.md | Harrison's evidence: used seriously once (NQTHM); payoff cases need unverifiable imperative code; hard to combine with a checkable proof log | SKIPPED (frontier) |
| Shulman set theory / Martin-Lof meanings | papers-and-foundations.md | "Foundational-only. No adopt item." | SKIPPED |
| demo.pdf, itj.pdf, ab.pdf, sfm.pdf, super.pdf | hol-light-misc-short.md, number-theory-misc.md | Context/survey only, or irrelevant (HPC paper) | SKIPPED |
| Full HOL Light kernel self-verification | hol-light-system.md | Cite as provenance, do not reproduce | SKIPPED |
| Candle kernel reimplementation | hol-light-system.md | Honor the ~400-line trusted-core boundary; do not rebuild the kernel or its self-verification | SKIPPED (design constraint) |
| handbook-practical-logic.md adopt-list | handbook-practical-logic.md | **BLOCKED**: `atp/book.pdf` is not Harrison's Handbook; it is the UniMath "Symmetry" HoTT textbook. The report refuses to emit an adopt list rather than invent one. Blocker: obtain the real Handbook plus its OCaml companion code (DPLL, Stalmarck, tableaux, resolution, Knuth-Bendix, Cooper, Hormander/CAD, Nelson-Oppen, Groebner) and re-run the pass. | BLOCKED |
| survey-2212.md ATP/ML survey mining | survey-2212.md | **BLOCKED**: `2212.11082v1.pdf` is Rijke's "Introduction to Homotopy Type Theory", not an ATP/ML survey. Expected topics appear zero times. Blocker: source the intended survey. | BLOCKED |
| ESSLLI94.pdf section | hol-light-system.md | **BLOCKED**: pypdf and poppler both fail on its Ghostscript 7.07 font encoding. Blocker: render pages to PNG and OCR with Tesseract. | BLOCKED |
| Univalent / cubical FormalSystem backend | survey-2212.md | "scope flag, not an adopt"; park as a research question | SKIPPED |

---

## 4. Certificate kinds: what we ship vs what the ATP corpus describes

We ship 14 checker modules plus the log itself: `cert_bernstein`, `cert_bezout`,
`cert_bnb`, `cert_continued_fraction`, `cert_flyspeck_lp`, `cert_herbrand`,
`cert_nullstellensatz`, `cert_pocklington`, `cert_positivstellensatz`, `cert_pratt`,
`cert_sos`, `cert_sturm`, `cert_taylor_model`, `cert_wz`, with `cert_log.py` as the
envelope. Note a separate finding: `cert_log.KINDS` is only
`("lp_primal_dual", "lp_farkas", "asymptotic", "wu_geometry", "subsumption")`, so the
proof-log envelope recognizes 5 kinds while 14 standalone checkers exist. Those are
two different registries and nothing reconciles them.

Certificate kinds the corpus describes that we do NOT have:

| Missing kind | Source doc | Why it matters |
|---|---|---|
| `sos_interval` (univariate Positivstellensatz on [a,b]) | analysis-transcendentals.md | Ranked #1 in its doc; checker is pure rational, only the root-find is an injected seam |
| `sos_multivariate` | analysis-transcendentals.md | Checker ungated (exact LDL^T emits the `f(u)<0` witness); generator needs SDP |
| `fp_rounding` | floating-point-numerics.md | Would be our FIRST IEEE-rounding certificate; exact `Fraction`, nothing gated |
| `fp_error_bound` | floating-point-numerics.md | fma-chain error propagation; nothing gated |
| `fp_exclusion` | floating-point-numerics.md | Checker buildable; the full format-wide quantifier is honestly gated |
| `analytic_log_linear` | analysis-transcendentals.md | Realizes cert-log's named "asymptotic log-linear" family concretely |
| `factorization` | hol-light-reflection-cas.md | Trivial checker (multiply out); we have no factorization cert at all |
| `antiderivative_by_diff` | hol-light-reflection-cas.md | Checks a CAS antiderivative by differentiating it |
| `summation_by_differencing` | hol-light-reflection-cas.md | Closed-form sum checked by differencing |
| `cas_numeric_error_bound` | hol-light-reflection-cas.md | Explicit accuracy bound so CAS digits are never trusted |
| `cooper` (Presburger QE trace) | hol-light-manuals.md | No integer-arithmetic QE certificate exists in-repo |
| `rcf_qe` / `interpmat` (Cohen-Hormander sign matrix) | hol-light-manuals.md, hol-light-system.md | Format-only per the doc, lowest priority |
| `superposition` / completion replay | course-slides-part2.md | `rewriting.rs` computes the objects but emits no replayable certificate |
| `ideal_cofactors` as a named object | hol-light-manuals.md | Ranked #1 there; the mechanism is inside cert_nullstellensatz but not a first-class kind |
| `negative_certificate` (verifier failure) | hol-light-reflection-cas.md | Ranked HIGH; failures are consumed but not certified |
| `counterexample_norm` | euclidean-vectors-geometry.md | Gated, narrow, diagnostics-only |
| `axiom_fingerprint` (content-addressed reassembly) | flyspeck-kepler.md | Ranked #2 there; prerequisite for sharding a blueprint-scale run |
| `resolution_refutation` | course-slides-part1.md | Doc explicitly does NOT recommend it |
| `half_ulp`, `parity_scale` (generators) | floating-point-numerics.md | Extend the shipped `exclusion_zone` / `near_root` family |

## 5. Decision procedures named in atp-mining that we have NOT implemented

Unification, LPO, KBO, multiset ordering, critical pairs, Knuth-Bendix completion,
demodulation, congruence closure and subsumption ARE implemented, in
`components/reason/search/rewriting.rs` (the doc set predates that file and calls
several of them gaps).

Not implemented: ordered resolution with selection, hyperresolution, subsumption
resolution, condensation, feature-vector subsumption indexing, discrimination and
substitution tree indexing, Tseitin CNF, DPLL/CDCL, CDCL(T), AVATAR splitting,
Stalmarck, OBDD/BDD, semantic tableaux, Cooper/Presburger QE, RCF QE via
Cohen-Hormander, CAD, complex-field QE, Nelson-Oppen/Shostak combination, Buchberger
as a named engine, Dixon resultant, Solovay's vector-space procedure with Gram-Schmidt
plus CSDP, the universal-additive normed-space procedure, VECTOR_ARITH, ROUND_CONV,
higher-order matching, and Bernstein-basis bounding as a live tactic (we have the
certificate checker but not the procedure).

---

## 6. Highest-value list

Ranked by soundness impact first, then by ratio of value to effort.

1. **Isabelle live oracle audit is a no-op.** `audit_axioms` returns
   `within_whitelist: true` unconditionally. Every doc in `docs/formal-systems`
   presents `thm_oracles` / `Thm_Deps.all_oracles = []` as the Isabelle layer-2a
   guarantee. An oracle reached through an imported theory passes the gate today.
2. **Metamath layer 2b trusts the exit code the codebase knows is a lie.**
   `kernel_recheck` uses `out.success()` while `compile` correctly uses the stdout
   sentinel, for a binary that returns 0 on a failed `verify proof *`. The recheck
   layer delivers nothing for Metamath.
3. **Metamath secondary checker overwrites the primary verdict**, which the spec
   explicitly forbids. One line, one-line fix (`rechecked = rechecked && second.success()`).
4. **Agda/Metamath `audit_axioms` is a stub for both systems**, and the Agda spec's
   transitive imported-postulate requirement is not delivered at all. Only the
   lexical token scan stands between a postulate in an imported module and a pass.
5. **No provenance envelope** (`source_sha256`, `dependency_sha256`, `limits`) on any
   Agda/Metamath result, though the spec calls it the common contract for every result.
6. **`cert_log.KINDS` (5) and the shipped checkers (14) are unreconciled registries.**
   A certificate that a standalone checker validates has no envelope kind, so it
   cannot ride the proof log.
7. **Tier-0 gates are permanently observational** because no shipped backend
   populates `designated_inputs` or `satisfiability_witness`. The mechanisms are
   built and tested; the channels are empty, so the gates can never be turned on
   without failing everything.
8. **`parity_scale` and the `near_root`-plus-Sturm join** are the cheapest real
   soundness wins available: both halves already ship, nothing is gated, and they
   upgrade a heuristic enumeration to a certified one.
9. **`fp_rounding` and `fp_error_bound`** are the two highest-confidence NEW
   certificate kinds in the corpus: exact-rational, offline, deterministic, no gate.
10. **Content-addressed axiom fingerprint** (flyspeck ranked #2 overall) is the
    prerequisite for ever sharding a blueprint-scale run across workers.
11. **Token budget / silent-truncation defense in `context_assembly.rs`.** The
    module exists and is on the hot path for every model call; without a token
    counter a constraint block can be silently dropped, which is a soundness-adjacent
    failure in a layer whose whole purpose is carrying invariants.
12. **Two mining docs are blocked on the wrong source file** (`handbook-practical-logic.md`
    reads the UniMath HoTT book, `survey-2212.md` reads Rijke's HoTT intro). Both
    correctly refused to invent content. Re-sourcing the real Handbook would unblock
    the single densest algorithm catalog in the slice.

## 7. Documented-but-not-delivered guarantees

Each is a guarantee a `docs/formal-systems` document states, that the code does not
deliver.

1. **Isabelle: "`thm_oracles LEMMA` empty/whitelisted (full transitive oracle
   graph)"** (isabelle.md integration recommendation; INTEGRATION-PLAN.md layer 2a
   table). Delivered: nothing. `isabelle.rs:386` returns clean unconditionally and
   defers to compile plus source scan. The source scan only sees the SUBMITTED
   theory, so an oracle in an imported theory is invisible to all four layers.
2. **Agda: "Imported postulates must be reported transitively, not only lexically
   scanned"** (AGDA-1LAB-METAMATH-SPEC.md trust policy). Delivered: only the lexical
   scan. `audit_axioms` is a stub.
3. **Metamath: "A second checker ... is not silently substituted for the configured
   primary checker"** (same spec). Delivered: the opposite. `external.rs:1257`
   assigns the secondary's verdict over the primary's.
4. **Metamath: the batch verification is the authoritative operation** (same spec).
   Delivered for `compile`, NOT for `kernel_recheck`, which trusts an exit code the
   same file documents as unreliable four hundred lines earlier.
5. **"Every result includes `source_sha256`, `dependency_sha256`, `limits`,
   `checker.version`"** (same spec, "Common contract"). Delivered: none of them.
6. **Metamath: "`$a` statements ... must be recorded, not blindly rejected", and the
   stated test "a database with ordinary `$a` declarations is not incorrectly
   rejected"** (same spec). Delivered: `code.contains("$a")` flags every occurrence
   and nothing records them. This one fails safe (over-rejection) but contradicts a
   test the spec asserts should pass.
7. **INTEGRATION-PLAN.md's "BUILD STATUS: all phases shipped" for the axiom-audit
   layer.** Phase 2 is described as "real verify gates ... mapping each layer to the
   concrete commands (table above)". For Isabelle, Agda and Metamath, layer 2a was
   never mapped; only Lean and Rocq have a real axiom audit.
8. **Candle is presented as a live certifying backend** (INTEGRATION-PLAN.md,
   formal.rs doc comments, `backend_for` wires `CandleBackend::live`). In practice
   `check_candle_signature` fails closed on every current caller because Candle
   canonical statements carry no proposition. The gate is honest; the documentation
   implies a capability that cannot fire. This is the Candle defect from the brief,
   confirmed and now fail-closed rather than unsound.
9. **INTEGRATION-PLAN.md's claim that the four-layer default `verify()` enforces all
   four layers** is true syntactically, but for three of the six systems layer 2a is
   a constant `true`, so the conjunction is a three-layer gate wearing a four-layer
   name.
