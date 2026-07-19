# Backlog slice: docs/paper-mining/ (31 files)

Scope: every `.md` in `docs/paper-mining/` (30 per-paper reports + `README.md`, which
acts as the cross-paper synthesis). Each adopt / build / recommendation item in those
docs is one row. Status was checked by grepping the tree, not inferred from the docs.
Several docs contain their own "BUILT" claims (notably `deepmind-articles.md`); those
claims were re-verified rather than trusted.

Legend:

- IMPLEMENTED: the mechanism exists in `components/` or `app/`, cited by file:line.
- PARTIAL: something related exists, but not this mechanism. The gap is named.
- NOT-BUILT: no trace found.
- SKIPPED: the mining doc itself decided against it, reason given.
- BLOCKED: cannot be meaningfully built without a trained model, GPU, or a corpus we
  do not have. The offline scaffold may still exist; that is called out.

Strictness rule applied throughout: a scaffold that has only ever run against a mock
generator, a mock oracle, or a heuristic stand-in is PARTIAL or BLOCKED, never
IMPLEMENTED, when the paper's mechanism is the trained/served component. Rust search
and verification logic that is deterministic and unit-tested counts as IMPLEMENTED,
because for those the offline artifact IS the mechanism.

Axis column: S = soundness (can it let a wrong thing through, or wrongly reject),
Q = capability or quality (how much it proves, how fast, how well ranked).

---

## A. SOUNDNESS items

| Item | Source paper/doc | Mechanism (one line, concrete) | Target file(s) in our tree | Status | Evidence |
|---|---|---|---|---|---|
| SafeVerify-style axiom-injection gate | alphaproof-nexus | Reject any proof whose axiom set includes `sorryAx` or an injected axiom | `components/prover/axiom_audit.rs` | IMPLEMENTED | `components/prover/axiom_audit.rs` exists as a dedicated audit stage; `components/prover/formal.rs` wires the verdict |
| Statement-preservation anti-cheat (`replace_statement_in_proof`) | goedel-prover | Splice the model's proof body onto the canonical statement so the goal cannot be silently weakened | `components/prover/statement_preservation.rs` | IMPLEMENTED | `statement_preservation.rs:173` `check_statement_preserved`, `:2357` `splice_canonical_statement`, `:68` `PreservationVerdict` |
| Ban interactive search tactics (`apply?`, `exact?`) in submitted proofs | goedel-prover | Blocklist search tactics that make a proof unreproducible | `components/prover/statement_preservation.rs`, `components/verify/hardening.rs` | PARTIAL | `statement_preservation.rs` covers signature and context integrity; no explicit tactic blocklist found for `apply?`/`exact?` |
| Context-integrity check (proof may not mutate hypotheses/locals) | alphaproof-nexus, goedel-prover | Compare pre/post local context and reject silent hypothesis edits | `components/prover/statement_preservation.rs` | IMPLEMENTED | `statement_preservation.rs:1904` `check_context_preserved`, `:1845` `ContextIntegrityReport` |
| Vacuity / trivially-true statement guard | kimina-prover, goedel-prover | Reject statements provable because hypotheses are contradictory or the goal is trivial | `components/prover/vacuity.rs`, `components/tools/python/theoremata_tools/triviality.py` | IMPLEMENTED | `components/prover/vacuity.rs`; `triviality.py` present with a worker op |
| Meta-verification as a multiplicative reward term `R_V = R_format * R_score * R_meta` | deepseekmath-v2 | Penalise a verifier that hallucinates issues, by multiplying its reward by a meta-verifier score | `components/train/python/theoremata_tools/reward.py` | PARTIAL | `reward.py:245` `graded_verifier_reward` and `:272` `graded_generator_reward` implement the formula exactly, but no trained verifier or meta-verifier exists; the scores come from mocks. Formula built, signal not |
| Majority-vote meta-confirmation for auto-labeling | deepseekmath-v2 | n verifications, m meta-verifications each, keep an issue only if a majority confirms; label with lowest confirmed score | `components/train/python/theoremata_tools/flywheel.py` | PARTIAL | `flywheel.py:96` `majority_confirm`, `:103` `majority_meta_confirm`, `:150` `auto_label` implement the rule; runs only against `mock_generator` (`:369`) and `pattern_oracle` (`:384`) |
| Abstention as a first-class terminal state | aletheia | Verifier may return "cannot verify" instead of a claim; report conditional accuracy | `components/reason/orchestration/certification.rs` | IMPLEMENTED | `certification.rs:101` `CertifyOutcome::Abstained`, `:95` doc comment states the decline-not-bluff rationale; `:536` test asserts terminal state |
| Abstain rung verdict inside the portfolio ladder | aletheia, aristotle | A cheap rung that finds nothing returns Abstain, not Refuted | `components/reason/proving/portfolio.rs`, `components/reason/search/verification_ladder.rs` | IMPLEMENTED | `portfolio.rs:212` and `:215` return `RungVerdict::Abstain`; `verification_ladder.rs:166` `RungVerdict` |
| Thinking-token decoupling (verifier sees only the final claimed proof) | aletheia | Strip generator chain-of-thought before critique so it cannot inflate a wrong claim | `components/reason/critique/critic.rs` | NOT-BUILT | No CoT-stripping step found in `critique/`; `critic.rs` is a structural gap/circularity audit |
| Do not let the soft LLM check veto a passed formal gate | robust-mathematical-reasoning, proofgrader | Advisory-only NL grading; formal gate stays authority | `components/reason/orchestration/certification.rs`, `components/reason/critique/` | IMPLEMENTED | `certification.rs` treats grader output as advisory alongside the formal outcome; `reason/critique/validity_seams.rs` keeps validity checks separated from the gate |
| Statement roundtrip / consistency check with a divergence taxonomy (CC) | formarl, goedel-prover, kimina-prover | Back-translate formal to English, classify quantifier-flip / dropped-hypothesis / relation-flip, emit a faithfulness score | `components/reason/orchestration/statement_validation.rs`, `components/tools/python/theoremata_tools/roundtrip_audit.py` | PARTIAL | Both files exist and produce a faithfulness verdict, but the model-backed judge is mocked offline; the lexical backend is a stand-in for the CC judge |
| Composite `SC and CC` formalization reward, with anti-hack filters | formarl | AND-gate compile-check with consistency-check; reject empty, comment-only, NL-echo and trivial-stub statements | `components/train/python/theoremata_tools/formalization_reward.py` | IMPLEMENTED | `formalization_reward.py:259` `formalization_reward`, `:195` `is_trivial_statement`, `:235` `is_nl_echo`, `:103` `default_syntax_check` |
| SC gates sampling selection, CC scores only the winner | formarl | Because CC specificity collapses under pass@k, pick the first SC-clean candidate and run CC once | `components/train/python/theoremata_tools/formalization_reward.py`, `components/reason/proving/formalize_portfolio.rs` | IMPLEMENTED | `formalization_reward.py:336` `selection_policy` |
| Roundtrip recall/specificity audit before trusting CC as reward | formarl | Perturb gold Lean (drop hypothesis, flip relation, swap quantifier), measure specificity | `components/tools/python/theoremata_tools/roundtrip_audit.py` | PARTIAL | `roundtrip_audit.py` exists; no recall/specificity report or perturbation generator surfaced by grep |
| Two-gate statement validation (CC compile + FC faithfulness N-vote) | goedel-prover | Statement must compile with a `sorry` body AND pass an N-vote LLM faithfulness judge above 0.5 | `components/reason/orchestration/statement_validation.rs` | PARTIAL | The gate stage exists; the N-vote judge is model-gated and currently mocked |
| Test-lemma misformalization guard (prove first terms before the conjecture) | alphaproof-nexus | Cheap pre-check that a formalization is not vacuous or wrong before spending search | `components/prover/vacuity.rs`, `components/reason/proving/falsification.rs` | PARTIAL | Vacuity and falsification checks exist; the specific "prove the first N sequence terms first" guard for OEIS-style conjectures is not present |
| Negation-augmented search state (disprove inside the same budget) | aristotle, internlm-stepprover | Every single-goal state gets a transition to the goal's negation; search can refute | `components/reason/search/driver.rs` | IMPLEMENTED | `driver.rs` exposes `with_negator` and a `refuted` outcome (`DriverResult` at `driver.rs:102`); noted as already-covered in internlm-stepprover mining |
| Negation augmentation at dataset time (both polarities as training tasks) | goedel-prover, internlm-stepprover | Add the negation of every extracted subgoal so the model learns to reject false props | `components/train/python/theoremata_tools/curriculum_synth.py` | PARTIAL | `curriculum_synth.py:103` `subgoal_to_conjectures` mints subgoal tasks; no negation-polarity augmentation found |
| Falsify-before-prove gate | aristotle, alphageometry, quantum-atp | Search for a counterexample before committing search budget to a proof | `components/reason/proving/falsification.rs`, `components/tools/python/theoremata_tools/falsify.py` | IMPLEMENTED | `reason/proving/falsification.rs`; `tools/python/theoremata_tools/falsify.py` and `falsify_hardcase.py` |
| Solution filtering: drop "all steps errored but answer correct" hallucinations | alphamath-almost-zero | Discard traces where every step failed yet the goal was marked closed | `components/train/python/theoremata_tools/star_harvester.py` | PARTIAL | `star_harvester.py:97` `is_verified` and `:77` `_verdict_of` gate on the formal verdict, which subsumes the answer-only case; the explicit all-steps-errored filter is not separately implemented |
| Timeout scored as failure, not as unknown | goedel-prover | A prover timeout must not be rewarded | `components/train/python/theoremata_tools/reward.py` | PARTIAL | `reward.py:88` `correctness_reward` returns None or 0 on non-pass verdicts; no explicit timeout-specific clause found |
| Eager well-typedness check on tactic-produced proof terms | aristotle | Catch ill-typed terms before the kernel does, to fail fast and loudly | `components/prover/` | NOT-BUILT | No eager pre-kernel typecheck found; the gate relies on the kernel pass |
| Kernel-order alpha-rename canonical node hash | lean-github | Rename hypotheses by kernel storage order so alpha-variant states hash identically | `components/reason/search/goal_cache.rs`, `components/reason/search/symmetry_dedup.rs` | IMPLEMENTED | `goal_cache.rs:33` `GoalCache`; `symmetry_dedup.rs:156` `canonical_key`, `:166` `OrbitDedup` |
| Subsumption-based redundancy elimination instead of hash equality | automated-theorem-proving-survey, README convergence 2 | A node subsumes another if weaker hypotheses and stronger conclusion; drop the subsumed one | `components/reason/search/subsumption.rs` | IMPLEMENTED | `subsumption.rs:42` `CanonicalGoal`, `:105` `subsumes`, `:117` `subsumes_str` |
| Backward subsumption as lemma-cache eviction | automated-theorem-proving-survey | A newly derived stronger node retires stored weaker ones | `components/reason/proving/library.rs`, `components/reason/search/subsumption.rs` | PARTIAL | The subsumption predicate exists and `LemmaLibrary` accepts an injected deduper, but `library.rs:139` `with_exact_dedup` defaults to exact string equality and no eviction pass was found |
| Signed-subformula polarity pre-filter on candidate premises | automated-theorem-proving-survey | Restrict candidate premises to those polarity-compatible with the goal | `components/retrieval/python/theoremata_tools/` | NOT-BUILT | Retrieval cascade is BM25 + dense + reranker (`cascade.py`, `reranker.py`); no polarity filter |
| Failed tactic is a no-op self-edge (never spawns a node) | leandojo-v2 | Erroring tactics leave state unchanged, so they must not enter the graph | `components/reason/search/tactic_outcome.rs` | IMPLEMENTED | `components/reason/search/tactic_outcome.rs` classifies error edges as Discard (referenced in bfs-prover mining) |
| Soundness-preserving lossy abstraction discipline | atp-for-prolog-verification | Under-approximate provability, never make a false thing provable | `components/reason/proving/decomposition_admission.rs` | IMPLEMENTED | `reason/proving/decomposition_admission.rs` governs which decompositions may be admitted |
| Citation-faithfulness check (does the cited source state the claim) | aletheia | Retrieval must verify the claim is actually in the cited premise, not just that it retrieved something | `components/reason/orchestration/method_transfer.rs`, `components/reason/critique/critic.rs` | PARTIAL | Both files mention citation handling; no source-verifies-claim check found. The docs flag this as the residual hallucination mode |
| Reject sketches that hide the whole difficulty in one restated `sorry` lemma | alphaproof-nexus | Detect a helper lemma whose statement just restates the target | `components/reason/proving/decomposition_admission.rs` | PARTIAL | Decomposition admission exists; no self-restatement detector found by grep |
| Portfolio redundancy / cross-checking as fault tolerance | von-neumann | Do not trust a single prover; run several and cross-check | `components/reason/proving/portfolio.rs`, `components/reason/search/verification_ladder.rs` | IMPLEMENTED | `portfolio.rs`; `verification_ladder.rs:439` `VerificationLadder` |
| Generic (not per-bug) error coverage | von-neumann | Gate on classes of failure: `sorry`, admits, timeouts, type errors | `components/verify/hardening.rs`, `components/verify/paranoia_corpus.rs` | IMPLEMENTED | `components/verify/hardening.rs`, `components/verify/paranoia_corpus.rs` |
| Grader must not be the same model family as the generator | proofgrader | Within-generator bias causes self-over-scoring | `components/eval/python/theoremata_tools/proof_grader.py` | PARTIAL | `proof_calibration.py:324` `evaluator_disagreement` measures cross-evaluator spread, which is the detector; no enforced family separation in the grader call path |
| Contamination / decontamination flag on ingested benchmark problems | kimina-prover, robust-mathematical-reasoning | n-gram overlap check against training corpora plus a freshness tier | `components/eval/python/theoremata_tools/eval_harness.py` | IMPLEMENTED | `eval_harness.py:205` `_ngrams`, `:212` `contamination_flag`, `:66` `freshness_tier`, `:240` `recalled_answer_smell` |
| Verify-solve gap (grader over-scores problems it cannot solve) | proofgrader | Measure over-scoring as a function of the grader's own ability on the problem | `components/eval/python/theoremata_tools/proof_calibration.py` | IMPLEMENTED | `proof_calibration.py:383` `verify_solve_gap` |
| Herbrand-ground bounded refutation as a falsifier | quantum-automated-theorem-proving | Bounded ground-instance search for a contradiction before full proof | `components/reason/search/model_elimination.rs` | IMPLEMENTED | `model_elimination.rs:462` `prove`, `:474` `refute`, `:628` `refute_clauses` (bounded depth) |
| Pure-past temporal monitors over agent run traces | first-order-automata-for-foltl | Deterministic safety monitors on harness invariants such as "no `sorry` reintroduced" | none | SKIPPED | The doc explicitly says "park it"; only reach for it if a runtime-monitoring layer appears. No proving-core adoption recommended |
| Quantum ATP backend | quantum-automated-theorem-proving | Grover-style speedups for resolution and PIT | none | SKIPPED / BLOCKED | Doc concludes "no core-harness architecture to port"; also requires fault-tolerant quantum hardware |

---

## B. CAPABILITY / QUALITY items

| Item | Source paper/doc | Mechanism (one line, concrete) | Target file(s) in our tree | Status | Evidence |
|---|---|---|---|---|---|
| Trained meta-verifier reward as the flywheel signal | deepseekmath-v2 | Train a verifier on rubric-scored proofs, then a meta-verifier on the verifier's analyses, and use both as RL reward | `components/train/python/` | BLOCKED | The whole reward algebra is coded (`reward.py:245`, `:272`, `:286`) and the labeling loop exists (`flywheel.py:150`), but there is no trained verifier. Needs GPU plus an expert-scored cold-start set. This is the calibration example "trained meta-verifier reward -> components/train/"; the file is there, the trained model is not |
| Generator self-analysis reward split alpha 0.76 / beta 0.24 | deepseekmath-v2 | Reward faithful self-assessment alongside proof quality | `components/train/python/theoremata_tools/reward.py` | IMPLEMENTED | `reward.py:286` `generator_self_verify_reward`, `:209` `faithfulness_reward` |
| Proof pool with top-N-by-average-verification refinement scheduler | deepseekmath-v2 | Keep a candidate pool per problem, refine the top candidates against issue-reporting analyses, stop when one passes all verifications | `components/reason/search/proof_pool.rs` | PARTIAL | `proof_pool.rs:123` `ProofPool`, `:37` `PoolVerdict`, `:211` `ProofPoolStore` implement the pool and stop condition; the "pair with 8 issue-prioritised analyses" scheduler needs a real verifier to produce analyses |
| Sequential refinement with self-verification wrapper on the sketch stage | deepseekmath-v2 | Generate proof plus self-analysis, re-prompt to fix flagged issues, stop at self-score 1 | `components/reason/proving/repair.rs`, `components/reason/proving/retry.rs` | IMPLEMENTED | `repair.rs:254` `repair_proof` with `RepairConfig` rounds; `retry.rs:20` `RetryLimits`, `:196` `Escalation` |
| Growing verified-lemma library with the four evolve directions | lego-prover | Verified lemmas as a store; an evolver generalises them along parameterize / key-concepts / scale / extend-dimensions | `components/reason/proving/library.rs` | IMPLEMENTED | `library.rs:49` `EvolveDirection` with all four variants, `:81` `Evolver` trait, `:119` `LemmaLibrary`, `:108` `EvolveSummary`. Matches the calibration example |
| Least-`update_count` round-robin evolve scheduler | lego-prover | Always evolve the least-evolved lemma so the library spreads rather than over-mining favourites | `components/reason/proving/library.rs`, `components/graph/db.rs` | IMPLEMENTED | `update_count` present in `library.rs` and `components/graph/db.rs` |
| Dedup admission gate on new skills (similarity below 0.85) | lego-prover | Admit only verified, non-duplicate lemmas | `components/reason/proving/library.rs` | PARTIAL | The dedup seam exists (`DedupFn`) but `library.rs:139` `with_exact_dedup` defaults to string equality; no similarity-ratio or embedding-cosine deduper wired in |
| Request store: unsolved holes become a worklist and a retrieval query | lego-prover | Failed prover lemmas and decomposer subgoals become "requests" that a request-solver worker attacks | `components/reason/proving/library.rs` | NOT-BUILT | `solve_request` exists on the `Evolver` trait (`library.rs:88`) as a seam, but grep finds no request store, no request queue, and no request-solver worker |
| Problem store to bias evolution toward pending targets | lego-prover | Third vector store of open target statements steering the evolver | `components/retrieval/python/theoremata_tools/semantic_memory.py` | NOT-BUILT | `semantic_memory.py` exists but is not a problem-store steering signal for the evolver |
| Retrieve self-grown lemmas during hole-proving | lego-prover | Embed the hole statement and k-NN the lemma store, feed results as spliceable premises | `components/reason/proving/graph_rag.rs`, `components/retrieval/python/theoremata_tools/cascade.py` | PARTIAL | `graph_rag.rs:67` `GraphView` retrieves over the proof graph, and `library.rs:371` `embedding_key` exists; the lemma store is not wired in as a retrieval corpus alongside BM25/dense |
| Marking-scheme-conditioned proof grading | proofgrader | Generate a per-problem rubric (checkpoints / zero-credit / deductions) from a reference solution, then grade against it | `components/eval/python/theoremata_tools/proof_grader.py` | IMPLEMENTED | `proof_grader.py:709` `_template_marking_scheme`, `:861` `generate_marking_scheme`, `:973` `grade_with_marking_scheme`. Matches the calibration example |
| Fine-grained 0 to 7 scale with four bands | proofgrader, robust-mathematical-reasoning | Replace binary pass/fail with an ordinal score and band mapping | `components/eval/python/theoremata_tools/proof_grader.py` | IMPLEMENTED | `proof_grader.py:662` `score_band`, `:214` `_to_ordinal`, `IMO_SCALE_MAX` constant |
| Ensembling (median of N grading runs) | proofgrader | Variance reduction over independent grader runs | `components/eval/python/theoremata_tools/proof_grader.py` | IMPLEMENTED | `proof_grader.py:189` `_median`, `:205` `aggregate_scores` |
| Calibration metrics: MAE, RMSE, bias, within-1, Kendall tau-b | proofgrader | Report grader agreement against a held-out expert-graded set | `components/eval/python/theoremata_tools/proof_calibration.py` | IMPLEMENTED | `proof_calibration.py:88` `mae`, `:95` `rmse`, `:102` `bias`, `:109` `within_tolerance`, `:159` `kendall_tau_b` |
| Bootstrap confidence intervals on calibration metrics | proofgrader | Report uncertainty, not a point estimate | `components/eval/python/theoremata_tools/proof_calibration.py` | IMPLEMENTED | `proof_calibration.py:267` `bootstrap_ci` |
| Score the marking-scheme generator itself | proofgrader | Measure whether generated schemes are high quality | `components/eval/python/theoremata_tools/proof_calibration.py` | IMPLEMENTED | `proof_calibration.py:471` `score_marking_scheme_grader` |
| Best-of-n selection by fine-grained score, O(n) not O(n squared) | proofgrader, improver | Rank candidates by score rather than pairwise tournament | `components/reason/search/proof_pool.rs`, `components/reason/proving/optimize.rs` | IMPLEMENTED | `proof_pool.rs:64` `ProofCandidate` ranking; `optimize.rs:284` `optimize` uses a correctness-first-then-metric selector |
| Failure-mode taxonomy baked into grader prompts | proofgrader | Explicit checks for over-crediting and under-crediting patterns | `components/eval/python/theoremata_tools/proof_grader.py` | IMPLEMENTED | `proof_grader.py:290` `classify_step`, `:417` `_taxonomy_counts`, `:269` `_bad_arithmetic` |
| MCTS-Q process supervision (dense value targets from one terminal bit) | alphamath-almost-zero | Back up the terminal gate verdict through the search tree; the converged Q becomes the value-head regression target | `components/reason/search/process_reward.rs` | IMPLEMENTED | `process_reward.rs:216` `backup_q`, `:243` `q_targets`, `:108` `QTarget`, `:45` `gate_reward`. Matches the calibration example |
| Step-level beam search (backup-free value-ranked frontier) | alphamath-almost-zero | Rank children by direct V, keep top B1, no tree, no backup | `components/reason/search/process_reward.rs` | IMPLEMENTED | `process_reward.rs:266` `step_beam_select`; Python mirror at `process_supervision.py:155` |
| PUCT prior from averaged token log-prob of the step | alphamath-almost-zero | Cheap prior with no extra forward pass | `components/reason/search/mcts.rs` | PARTIAL | `mcts.rs:43` `PriorMode` exists, but the driver deliberately prefers `PriorMode::EmpiricalSampled` (Aristotle's recommendation) over log-prob priors. This is a conscious divergence, not a gap |
| Value head sharing the generator backbone (tanh head at step end) | alphamath-almost-zero | One backbone, two heads, trained by NLL plus value MSE | `components/train/python/theoremata_tools/process_supervision.py` | BLOCKED | `process_supervision.py:269` `train_value_head` and `:238` `_fit_torch` exist with a closed-form ridge fallback; a real head needs GPU and real trajectories |
| Trained critic score wired into node priority | internlm-stepprover | Replace the heuristic progress term with a learned V(s) in PUCT selection | `components/reason/search/driver.rs`, `components/reason/search/critic_scorer.rs` | PARTIAL | The seam is fully built: `critic_scorer.rs:100` `CriticScorer`, `:148` `blend_priority`; `driver.rs:277` `with_critic`, `:415` blend, `:140` critic field. But only `HeuristicCritic` (`critic_scorer.rs:112`) and `ConstantCritic` (`:123`) exist; grep finds no production `CriticScorer` impl backed by `predict_value`, and only a doc-comment example calls `with_critic` |
| Bradley-Terry preference pairs (path pairs and sibling pairs) | internlm-stepprover | Mine ordered preference pairs from a finished search tree to train a critic | `components/reason/search/preference_pairs.rs` | IMPLEMENTED | `preference_pairs.rs:202` `path_pairs`, `:232` `sibling_pairs`, `:268` `extract_preference_pairs`, `:313` `mine_critic_pairs` |
| Project the live driver DAG into the process-reward SearchTree | internlm-stepprover | Make `q_targets` / `step_beam_select` run on real runs, not mocks | `components/reason/search/dag_projection.rs` | IMPLEMENTED | `dag_projection.rs:357` `project_search_dag`, `:225` `project_dag_to_tree`; called from `app/lib.rs:1434` |
| Critic-driven curriculum (re-score unproven, focus top 50 percent) | internlm-stepprover | After each flywheel round, rank unsolved statements by critic score | `components/train/python/theoremata_tools/difficulty.py` | PARTIAL | `difficulty.py:298` `estimate_difficulty`, `:331` `triage`, `:125` `bucket_by_percentile` provide the bucketing; the critic is not the ranker |
| Hybrid best-first plus critic-guided budget split | internlm-stepprover | Split budget between two rankers because they explore disjoint proof space | `components/reason/search/hybrid_search.rs` | IMPLEMENTED | `hybrid_search.rs:318` `split_budget`, `:337` `route`, `:408` `run_split`, `:299` `HybridPlan` |
| Prior tactics in the expander prompt (`PROOF_BEFORE`) | internlm-stepprover | Feed the tactic path so far into the tactic prompt for deep-tree coherence | `components/reason/search/sampler.rs` | PARTIAL | `sampler.rs:41` `ModelSampler` builds the prompt; no explicit prior-tactic history field found |
| Pairwise held-out critic accuracy metric | internlm-stepprover | Cheap offline critic eval on a labeled pair set | `components/reason/search/preference_pairs.rs`, `components/eval/` | NOT-BUILT | Pairs are extractable but no pairwise-accuracy evaluator found |
| Length-normalized best-first search score, sum log p over L to the alpha | bfs-prover | Priority-queue frontier ordered by cumulative path log-prob divided by length to the alpha | `components/reason/search/best_first.rs` | IMPLEMENTED | `best_first.rs:419` `best_first_search`, `:169` `BestFirstConfig`, `:241` `QueueItem` ordering |
| Multi-alpha accumulative union as a TTC portfolio config | bfs-prover | Sweep alpha in {0, 0.5, 1} and union the solved sets | `components/reason/search/hybrid_search.rs`, `components/reason/search/ttc.rs` | IMPLEMENTED | `hybrid_search.rs:128` `multi_alpha_union`, `:77` `AlphaRun`, `:657` `run_alpha_sweep_search`; `ttc.rs:153` `TtcController` |
| DPO preference pairs harvested from discarded error edges | bfs-prover | For each state on a winning path, pair the winning tactic with an erroring sibling | `components/reason/search/best_first.rs` | IMPLEMENTED | `best_first.rs:520` `dpo_pairs`, `:277` `DpoPair` |
| Beam-search self-filter of easy problems | bfs-prover, hunyuanprover | Deterministic beam pass classifies a statement easy, then withholds its data so the corpus trends harder | `components/train/python/theoremata_tools/curriculum_synth.py` | IMPLEMENTED | `curriculum_synth.py:287` `beam_self_filter` |
| Proof-length and tactic-diversity telemetry across rounds | bfs-prover | Mode-collapse guard: watch the length and token-length distributions per round | `components/reason/search/search_telemetry.rs` | PARTIAL | `search_telemetry.rs` exists for search stats; no cross-round flywheel distribution report found |
| Actual DPO policy-refinement round | bfs-prover | Run DPO with beta on the harvested pairs | `components/train/python/` | BLOCKED | Pairs are produced offline; the training run needs a 7B policy and GPUs |
| Distance critic: remaining-steps label encoded as a balanced-binary-tree path | hunyuanprover | Predict a coarse-to-fine path over 1..64 instead of a raw integer | `components/reason/search/distance_critic.rs` | IMPLEMENTED | `distance_critic.rs:89` `encode_distance`, `:113` `decode_distance`, `:133` `distance_score`, `:67` `tree_depth` |
| eta-MCTS importance-scaled adaptive expansion budget | hunyuanprover | Give more expansion budget to nodes with a large value gap to descendants | `components/reason/search/distance_critic.rs`, `components/reason/search/driver.rs` | IMPLEMENTED | `distance_critic.rs:188` `expansion_budget`, `:212` `importance_from_value_gap`; consumed at `driver.rs:474` |
| UCB-with-critic-score selection mode | hunyuanprover | Exploit the critic score, explore under-visited nodes | `components/reason/search/critic_scorer.rs`, `components/reason/search/mcts.rs` | IMPLEMENTED | `critic_scorer.rs:164` `blend_priority_with_cfg`; `mcts.rs:27` `SelectionMode` |
| Failed-trajectory to new-statement recycler | hunyuanprover | Turn the last state of an unfinished proof into a fresh, well-formed goal | `components/train/python/theoremata_tools/trajectory_recycler.py` | IMPLEMENTED | `trajectory_recycler.py:134` `recycle_failed_trajectory`, `:195` `recycle_batch`, `:123` `_render_premise` |
| Easy-data curation: evict early-solved examples before retrain | hunyuanprover | Removing early-easy data raised their accuracy after v12 | `components/train/python/theoremata_tools/curriculum_synth.py` | PARTIAL | `beam_self_filter` withholds easy solves at collection time; no first-solved-iteration tracking or retroactive eviction across accumulated rounds |
| Temperature-diversified N-sample plus filter statement synthesis | hunyuanprover, goedel-prover | Sample 8 outputs at varied temperatures, dedup, filter by grammar and roundtrip | `components/reason/proving/formalize_portfolio.rs` | PARTIAL | The N-candidate portfolio and screen exist; multi-temperature sampling is model-gated and mocked offline |
| Trained PRM and DC value heads | hunyuanprover | Fit the scalar / num-token heads on the labels we already manufacture | `components/train/python/theoremata_tools/process_supervision.py` | BLOCKED | Targets are produced offline (`q_targets`, `encode_distance`); fitting needs GPU |
| Verifier-guided self-correction loop with span-marked error rendering | goedel-prover, kimina-prover | Feed capped, context-windowed, span-marked compiler errors back into a repair round | `components/prover/error_feedback.rs`, `components/reason/proving/repair.rs` | IMPLEMENTED | `error_feedback.rs:166` `render_feedback`, `:101` `Diagnostic`, `:210` `parse_diagnostics`, `:55` `FeedbackConfig` (cap and context lines) |
| Infotree tightest-node goal-state-before-error attachment | kimina-prover, improver | Walk the Lean infotree to the tightest enclosing node and attach the goal state before the error | `components/prover/infotree.rs`, `components/verify/lean_session.rs` | IMPLEMENTED | `components/prover/infotree.rs`; `components/verify/python/theoremata_tools/lean_repl.py` exposes infotrees; `components/prover/session/goal_state.rs` |
| Chain-of-States: per-tactic goal states injected as generator context | improver | Extract the proof state before each tactic and feed it back as comments | `components/reason/proving/sketch.rs`, `components/prover/session/goal_state.rs` | IMPLEMENTED | `sketch.rs` `run_with_goal_states` (referenced in kimina mining); `components/prover/session/goal_state.rs` |
| `extract_goal` subgoal harvesting into the proof DAG | goedel-prover | On failure, harvest open subgoals with preconditions as new well-formed statements | `components/prover/subgoal_extract.rs` | IMPLEMENTED | `components/prover/subgoal_extract.rs`; consumed by `components/reason/proving/formal_generate.rs` |
| Difficulty-scaffolded synthesis (simpler if unsolved, harder if solved) | goedel-prover | Generate variants at the model's current difficulty frontier | `components/train/python/theoremata_tools/curriculum_synth.py`, `components/train/python/theoremata_tools/difficulty.py` | PARTIAL | The difficulty estimator and cold-start mining exist (`curriculum_synth.py:189` `mine_cold_start_positives`); the LLM harder/simpler variant generator is model-gated |
| Expert-iteration driver with cumulative solved set and per-round bookkeeping | goedel-prover, hunyuanprover, kimina-prover | Prove, verify, keep, retrain from base, repeat, tracking per-iteration deltas | `components/train/python/theoremata_tools/flywheel.py`, `star_harvester.py` | PARTIAL | `flywheel.py:397` `revolution` and `:469` `graded_revolution` implement the loop; it has only ever run against `mock_generator` and `pattern_oracle`. `flywheel.py:577` `dry_run` is the exercised path |
| Model averaging (model soups) against diversity collapse | goedel-prover | Interpolate base and finetuned weights after SFT and after RL | `components/train/python/` | BLOCKED | Only meaningful once we own weights; no implementation and none expected |
| Per-backend inference-handler adapter pattern | goedel-prover | One interface, per-model prompt and extraction formats behind it | `components/prover/backends/` | IMPLEMENTED | `components/prover/backends/{lean,rocq,isabelle,candle,leandojo,reprover,aristotle,external}.rs` behind `backends/mod.rs` |
| Format-collapse filters: at least one tactic block, 60 percent coverage, drop negatives | kimina-prover | Reject RL samples whose reasoning snippets do not appear in the final proof | `components/train/python/theoremata_tools/format_filters.py` | IMPLEMENTED | `format_filters.py:181` `has_tactic_block`, `:205` `snippet_coverage`, `:221` `passes_coverage`, `:248` `drop_negative_gradient`, `:282` `format_filter` |
| Whole-proof reasoning mode as a TTC alternative to tree search | kimina-prover | Let the budget controller pick wide whole-proof sampling versus deep search by difficulty | `components/reason/search/hybrid_search.rs`, `components/reason/search/ttc.rs` | IMPLEMENTED | `hybrid_search.rs:337` `route` with `GoalFeatures` at `:285`; `ttc.rs:153` `TtcController` |
| Lean server throughput: import-header LRU env cache plus multi-REPL pool | kimina-prover | Cache Lean environments keyed on import header, pool REPL processes | `components/verify/lean_session.rs`, `components/reason/orchestration/team.rs` | PARTIAL | `lean_session.rs` and `reason/orchestration/team.rs` manage sessions and concurrent workers; no import-header-keyed LRU env cache surfaced |
| Distilled open prover weights as a drop-in generator | kimina-prover | Point the model command at a vLLM endpoint serving an open prover | `components/provider/python/theoremata_tools/model_provider.py` | BLOCKED | The provider seam exists and honours `THEOREMATA_MODEL_MOCK`; no weights or endpoint configured. Needs a served model |
| Full RL training run (GRPO, no-KL, group-normalized) | kimina-prover, formarl, goedel-prover | Sample groups, verify, group-normalise advantage, no KL term | `components/train/python/theoremata_tools/grpo.py` | BLOCKED | `grpo.py:204` `train` defaults to `dry_run=True` and returns the config; `:138` `dry_run_grpo` is the exercised path. Needs GPU and TRL |
| SKEST cross-tree shared-fact ensemble | alphageometry2 | Heterogeneous parallel trees sharing a filtered pool of problem-relevant proved facts | `components/reason/search/skest.rs` | IMPLEMENTED | `skest.rs:89` `SharedFacts`, `:267` `TreeSearch`, `:473` `SkestConfig`, `:536` `skest_search`, `:696` `run_skest_search`. Matches the calibration example |
| Unified AR coefficient matrix over angles, ratios, distances, constants | alphageometry, alphageometry2 | One linear system plus Gaussian elimination for all chasing | `components/prover/python/theoremata_tools/geometry_ddar2.py` | IMPLEMENTED | `geometry_ddar2.py:451` `UnifiedAR`, `:161` `ElimCore` (exact Fraction elimination), `:343` `encode` |
| Hash-based detection for similar triangles and cyclic quads | alphageometry2 | Hash the triangle shape and the angle value, flag on recurrence, instead of O(N^8) search | `components/prover/python/theoremata_tools/geometry_ddar2.py` | NOT-BUILT | `geometry_ddar2.py` folds rules into the AR normal form (`_dd_closure` at `:508`) but no shape-hash or angle-value-hash detector was found |
| Double-point handling (`overlap`, `cyclic_with_center`) | alphageometry2 | Two differently named points with identical coordinates, to unlock reformulation proofs | `components/prover/python/theoremata_tools/geometry_ddar2.py` | NOT-BUILT | No `overlap` or `cyclic_with_center` predicate in the geometry modules |
| Locus and movement predicates plus linear-equation predicates | alphageometry2 | `distmeq`, `distseq`, `angeq`, `acompute`, `rcompute`, the fixed-point placeholder, and m() movement tracking | `components/prover/python/theoremata_tools/geometry_ddar2.py` | NOT-BUILT | grep for `locus`, `distmeq`, `angeq`, `acompute` returns nothing |
| Traceback with minimal premise support | alphageometry, alphageometry2 | Recover the minimal dependency subgraph for each derived fact | `components/prover/python/theoremata_tools/geometry_synth2.py` | IMPLEMENTED | `geometry_synth2.py:208` `traceback`, `:157` `minimal_support` (exact linear-algebra minimal support, the AG1 MILP analog), `:289` `_footprints` |
| Greedy reverse-topological `prune_points` | alphageometry2 | Linear-count minimality check replacing exponential subset removal | `components/prover/python/theoremata_tools/geometry_synth2.py` | PARTIAL | `minimal_support` gives minimal algebraic support; no reverse-topological point-prune loop and no `check_provable` monotonic prune found |
| Rebalance synthetic data to roughly 50:50 with and without auxiliary points | alphageometry2 | Fix AG1's 9:91 aux imbalance | `components/prover/python/theoremata_tools/geometry_synth.py` | PARTIAL | `geometry_synth.py:338` `_select_goal(candidates, prefer)` supports a `prefer="aux"` bias; there is no explicit 50:50 balancing policy |
| Analysis string: feed S1, S2 minus S1, S3 minus S2 to the generator | alphageometry2 | Give the model the symbolic engine's provable / provable-if-goal / numerically-true sets before it proposes a step | `components/reason/orchestration/context_assembly.rs` | NOT-BUILT | No analysis-string assembly found; grep for the S1/S2/S3 or numerically-true set returns nothing |
| Squared-length AR table for Pythagoras, optional law of sines | aristotle | Extra AR tables for perpendicularity and sine ratios | `components/prover/python/theoremata_tools/geometry_ddar2.py` | NOT-BUILT | No `squared_length`, `sqlen`, or `law_of_sines` symbols |
| Automated diagram generation (Adam plus Gauss-Newton on constraint losses) | alphageometry2 | Three-stage numeric diagram solver for non-constructive statements | `components/prover/python/theoremata_tools/` | NOT-BUILT | Only a `_numeric_screen` / `_numeric_ok` sampler exists (`geometry_ddar2.py:572`, `geometry_synth.py:314`), not a constraint-loss diagram solver |
| Integrate Yuclid (Apache 2.0) directly as the geometry closure engine | aristotle | Reuse the 500x C++ DD/AR engine rather than reimplementing | `components/prover/backends/` | NOT-BUILT | We reimplemented in Python (`geometry_ddar.py`, `geometry_ddar2.py`); no Yuclid backend |
| MCGS state and action equivalence collapsing hypertree to hypergraph | aristotle | Equal goals, context and var names collapse to one node; actions equal iff transitions equal | `components/reason/search/goal_cache.rs`, `components/reason/search/driver.rs` | IMPLEMENTED | `goal_cache.rs:33` `GoalCache` transposition; `driver.rs` dedup key path |
| PUCT prior from the empirical sampled-action distribution, not sequence log-probs | aristotle | Avoid penalising tactics with multiple textual spellings | `components/reason/search/mcts.rs` | IMPLEMENTED | `mcts.rs:43` `PriorMode` with `EmpiricalSampled` (confirmed in the internlm-stepprover mapping table) |
| AND/OR minimax with bottleneck-first child selection | aristotle | Highest-UCB action, then its lowest-LCB resulting state | `components/reason/search/driver.rs` | IMPLEMENTED | `driver.rs` PUCT/AND-OR selection (documented at `driver.rs:415` blend site and the AND/OR notes in the mining reports) |
| Progressive widening over the tactic action space | aristotle | Widen the action set as visit count grows | `components/reason/search/driver.rs`, `components/reason/search/distance_critic.rs` | PARTIAL | `distance_critic.rs:188` `expansion_budget` gives an adaptive per-node budget, which is a close cousin; no visit-count-driven progressive widening formula |
| Failure-mode-aware revision prompt (unworkable versus not granular enough) | aristotle | Distinguish "strategy wrong" from "steps too coarse for search" when revising a lemma list | `components/reason/orchestration/blueprint_generate.rs` | PARTIAL | `blueprint_generate.rs` has a `BlueprintRefiner` that decomposes Failed lemmas and preserves Proved, which covers the "not granular enough" branch; the two-mode prompt distinction is not explicit |
| Faithfulness judge filtering formal proofs that drifted from the informal proof | aristotle | Reject a formal proof that no longer follows the informal argument | `components/reason/critique/statement_validity.rs` | PARTIAL | Faithfulness machinery exists for statements (`statement_validity.rs`, `validity_seams.rs`); no informal-to-formal proof drift judge |
| Hindsight experience replay: re-root every solved subgoal as a standalone example | aristotle | Multiply training signal by treating each closed subgoal as its own theorem | `components/train/python/theoremata_tools/star_harvester.py` | NOT-BUILT | `star_harvester.py:123` `_rows_for_trace` and `:175` `from_graph_export` harvest verified traces, but no re-rooting of non-root nodes as standalone statements |
| Test-time training on the current attempt's own search traces | aristotle | Retrain mid-attempt on traces from failed lemma attempts | `components/train/python/` | BLOCKED | grep for test-time training finds nothing; requires GPU and a live model. `ttc.rs` is budget control, a different thing |
| Ralph-loop episode structure with budget caps and lessons-learned carryover | alphaproof-nexus | Multi-turn edit-and-compile episodes, cap hammer calls and edits, write a lessons comment, restart from the current sketch | `components/reason/proving/retry.rs`, `components/reason/orchestration/agent.rs` | PARTIAL | `retry.rs:20` `RetryLimits` and `:196` `Escalation` cover budget caps; no lessons-learned carryover artifact between episodes |
| Global deep-hash goal cache shared across the whole population | alphaproof-nexus | Hash the exact goal state, reuse a proof anywhere it recurs | `components/reason/search/goal_cache.rs`, `components/reason/proving/checker_cache.rs` | IMPLEMENTED | `goal_cache.rs:33` `GoalCache`, `checker_cache.rs`, `components/graph/db.rs` persistence |
| Elo fitness over a binary landscape plus P-UCB over an elite top-N | alphaproof-nexus | Rate incomplete sketches by LLM relative ranking, convert to Elo, select by P-UCB with c=0.2 | `components/reason/search/fitness.rs` | IMPLEMENTED | `fitness.rs:34` `EloRanker`, `:133` `p_ucb`; consumed by `components/reason/proving/evolve_sketch.rs` |
| Plackett-Luce / Gibbs posterior and Thompson matchmaking | alphaproof-nexus | Full Bayesian rating rather than incremental Elo | `components/reason/search/fitness.rs` | PARTIAL | `EloRanker` is a simpler incremental ranker; no Plackett-Luce likelihood, Gibbs sampler, or Thompson matchmaking |
| EVOLVE-BLOCK and EVOLVE-VALUE joint parameter-plus-proof search | alphaproof-nexus | Mark editable regions and searchable numeric parameters so the agent co-discovers object and proof | `components/reason/proving/evolve_sketch.rs` | IMPLEMENTED | `evolve_sketch.rs:48` `EvolveBlock`, `:58` `EvolveValue`, `:78` `EvolveRegion`, `:91` `EditableSketch`, `:589` `evolve` |
| Cost-per-solved-problem as the headline efficiency metric | alphaproof-nexus | Measure additions against a plain compiler-in-the-loop baseline on cost | `components/eval/python/theoremata_tools/eval_harness.py` | NOT-BUILT | The harness reports rates and axes (`eval_harness.py:426` `_aggregate_axes`); no cost-per-solve accounting |
| FunSearch / AlphaEvolve program search with island populations and MAP-Elites | deepmind-articles | Evolve programs against an injected evaluator, keep only passing, bias toward short programs | `components/tools/python/theoremata_tools/funsearch.py` | IMPLEMENTED | `components/tools/python/theoremata_tools/funsearch.py` with a `funsearch` worker op in `tools/python/theoremata_tools/worker.py`; the doc's own BUILT claim verified |
| Conjecture-discovery pipeline (dataset, relationship detection, attribution, propose) | deepmind-articles | Fit a model over (object, invariant) pairs versus a permuted baseline, use permutation importance to surface structure | `components/train/python/theoremata_tools/conjecture_discovery.py` | IMPLEMENTED | `components/train/python/theoremata_tools/conjecture_discovery.py` plus a worker op; doc claim verified |
| AlphaTensor-style game reformulation with a certificate | deepmind-articles | Cast discovery as a single-player game whose terminal carries a soundness certificate | `components/reason/search/discovery_game.rs` | IMPLEMENTED | `discovery_game.rs:52` `Certificate`, `:72` `DiscoveryGame`, `:212` `search_discovery`, `:397` `confirm_reachable_via_mcgs`, `:437` `ResidualReductionGame` |
| Blueprint generation and refinement (informal to acyclic uses-DAG) | deepmind-articles planning build | Generate a blueprint, drive it, refine failed lemmas, preserve proved ones | `components/reason/orchestration/blueprint_generate.rs`, `blueprint_run.rs` | IMPLEMENTED | Both files present; the generate-drive-refine loop is described in `deepmind-articles.md` build status and the files exist |
| Test-Time RL (generate and learn from problem variants at inference) | deepmind-articles 2026 notes | Problem-specific adaptation at inference time | `components/train/python/` | BLOCKED | Explicitly noted GPU-gated in the doc; no variant-generation scaffold found either |
| Forward saturation / inverse method as a portfolio worker | automated-theorem-proving-survey | Derive all consequences until the goal appears or a fixpoint is reached | `components/reason/search/inverse_method.rs` | IMPLEMENTED | `inverse_method.rs:431` `saturate`, `:657` `saturate_spec`, `:139` `SaturationConfig`, `:178` `SaturationResult` |
| Focusing: chain invertible steps atomically, branch only at synchronous choices | automated-theorem-proving-survey | Collapse micro-inferences into macro-steps to cut the branching factor | `components/reason/search/inverse_method.rs` | IMPLEMENTED | `inverse_method.rs:211` `focus` |
| Ordered-literal indexing for cheap subsumption tests | automated-theorem-proving-survey | Total order on atoms, antecedents as ordered lists, so union and subset are cheap | `components/reason/search/inverse_method.rs`, `components/reason/search/subsumption.rs` | IMPLEMENTED | `inverse_method.rs:49` `Clause` with ordered literal handling; `subsumption.rs:42` `CanonicalGoal` |
| Unification by residuation with incremental early-failure constraint solving | automated-theorem-proving-survey | Postpone metavariable instantiation, accumulate constraints, fail fast | `components/reason/search/rewriting.rs` | IMPLEMENTED | `rewriting.rs:163` `unify`, `:206` `matches`, `:113` `apply_subst`, `:236` `subsumes` |
| Keep cached lemmas by reference, expand only at final validation | automated-theorem-proving-survey | Avoid re-traversing replicated proof subterms during splice | `components/reason/proving/library.rs`, `components/reason/orchestration/proof_import.rs` | PARTIAL | `proof_import.rs` is content-addressed, which gives the referencing discipline; the expand-only-at-validation policy is not explicit |
| Equality handling beyond the notes: KBO/LPO term orderings and completion | automated-theorem-proving-survey (flagged as absent from that text) | Ordered rewriting, critical pairs, Knuth-Bendix completion, congruence closure | `components/reason/search/rewriting.rs` | IMPLEMENTED | `rewriting.rs:300` `lpo`, `:423` `kbo`, `:683` `critical_pairs`, `:742` `complete`, `:820` `congruence_closure`. We are ahead of what this doc asked for |
| Shortest-path proof recovery over the search DAG | leandojo-v2 | Extract the minimal tactic sequence from a graph that may contain redundant paths | `components/reason/search/minimize.rs` | IMPLEMENTED | `minimize.rs:45` `minimal_proof`, `:238` `minimize_proof_checked`, `:190` `MinimizeOutcome` |
| Curriculum retriever training bucketed at the 33rd and 67th percentiles | leandojo-v2 | Bucket theorems easy/medium/hard by proof-step-count percentile, train easiest-first | `components/train/python/theoremata_tools/retriever_train.py`, `difficulty.py` | PARTIAL | `difficulty.py:125` `bucket_by_percentile` provides the bucketing; `retriever_train.py` exists but the easiest-first repo-by-repo schedule was not found |
| Evaluate Pantograph as an alternative Lean backend | leandojo-v2 | Benchmark Pantograph against the current Lean driver for throughput | `components/prover/backends/` | NOT-BUILT | Backends are lean, rocq, isabelle, candle, leandojo, reprover, aristotle, external; no Pantograph adapter or benchmark |
| Tri-state tactic categorization (closes / advances with new subgoals / errors) | lean-copilot | Advancing tactics are valid DAG edges, not failures | `components/reason/search/tactic_outcome.rs` | IMPLEMENTED | `components/reason/search/tactic_outcome.rs` classifies outcomes; noted in the bfs-prover mapping |
| Model-generated goal-conditioned rules injected into an aesop-style rule set | lean-copilot | Expand the action set per node with LLM-proposed rules rather than a fixed vocabulary | `components/reason/search/sampler.rs`, `components/prover/python/theoremata_tools/tactic_server.py` | PARTIAL | `sampler.rs:26` `TacticSampler` and `:41` `ModelSampler` are exactly this seam; `tactic_server.py` is the serving side. Model-gated, so it has only run against mocks |
| Precomputed premise embedding matrix with in-scope / out-of-scope annotation | lean-copilot | Matvec goal against premise matrix, annotate results with import or source | `components/retrieval/python/theoremata_tools/accessible_premises.py`, `decl_index.py` | IMPLEMENTED | `accessible_premises.py`, `decl_index.py`, `head_index.py`, `cascade.py` in `components/retrieval/python/theoremata_tools/` |
| Native in-Lean FFI inference | lean-copilot | Run the model inside Lean via C++ FFI | none | SKIPPED | Doc states this is an implementation detail we do not need; our core is Rust plus Python orchestration |
| GitHub-scale Lean corpus extraction (Lake bypass, isolated files, import graph) | lean-github | Enumerate repos, repair a global import graph, call the compiler directly, extract DECL/GOAL/PROOFSTEP | `components/train/python/theoremata_tools/lean_corpus.py` | PARTIAL | `lean_corpus.py` mines (theorem, tactic) records by parsing, explicitly without `leanc` on this box. The Lake-bypass parallel compile pipeline is not built and is corpus-and-toolchain gated |
| Mix human-diverse plus Mathlib plus synthetic data, weighted for OOD | lean-github | Diversity, not volume, drives out-of-distribution gains | `components/train/python/theoremata_tools/` | NOT-BUILT | No corpus-mixture weighting policy found |
| Learned relative-margin backend/heuristic selector | ml-and-automated-theorem-proving | Train per-backend classifiers, select by the most positive margin across them | `components/train/python/theoremata_tools/selector.py` | IMPLEMENTED | `selector.py:171` `train`, `:226` `select`, `:105` `_fit_margin_fallback`, `:262` `evaluate` (margin-based, with sklearn/torch/closed-form backends) |
| H0 difficulty pre-filter for triage | ml-and-automated-theorem-proving | Cheap classifier routes hopeless goals away from expensive search | `components/train/python/theoremata_tools/difficulty.py` | IMPLEMENTED | `difficulty.py:257` `train_h0`, `:203` `_h0_label`, `:331` `triage` |
| Dynamic in-search features as MCTS priors | ml-and-automated-theorem-proving | Features of the partial proof state, not just the initial goal | `components/reason/search/progress.rs`, `components/reason/search/hybrid_search.rs` | IMPLEMENTED | `progress.rs:48` `ProgressFeatures`; `hybrid_search.rs:285` `GoalFeatures` |
| Proof-optimization metrics: length, declarativity, mixed, completion | improver | Rewrite a correct proof to optimise a user metric, correctness first | `components/reason/proving/optimize.rs` | IMPLEMENTED | `optimize.rs:48` `Metric`, `:82` `Length`, `:99` `Readability`, `:160` `Modularity`, `:284` `optimize` |
| Declarativity optimization as flywheel data curation | improver | Convert successful proofs into `have`-structured proofs for better training data and splice targets | `components/reason/proving/optimize.rs`, `components/train/python/theoremata_tools/star_harvester.py` | PARTIAL | The optimizer exists but no pipeline runs it over harvested traces before export |
| MMR retrieval of examples plus docs keyed on current error messages | improver | Retrieve syntax docs against theorem plus errors, lemmas against the theorem | `components/retrieval/python/theoremata_tools/error_keyed_retrieval.py` | IMPLEMENTED | `components/retrieval/python/theoremata_tools/error_keyed_retrieval.py`, `query_rewrite.py` |
| Compound sampling schedule: refinement over best-of-n | improver | Nest best-of-n inside refinement rounds with a correctness-first selector | `components/reason/proving/portfolio.rs`, `components/reason/proving/repair.rs` | PARTIAL | Both primitives exist; no nested compound scheduler with the m times n accounting |
| Structural render of the proof as an extra verification signal | theorem-explain-agent | Force the model to render the dependency structure and check consistency | `components/eval/python/theoremata_tools/exposition.py` | IMPLEMENTED | `exposition.py:437` `_render_structural`, `:423` `_dependency_section`, `:241` `extract` |
| Geometric-mean aggregation across independent judged dimensions | theorem-explain-agent | Do not blend into one score; take the geometric mean of independent dimensions | `components/eval/python/theoremata_tools/proof_grader.py` | IMPLEMENTED | `proof_grader.py:171` `geometric_mean`, `:205` `aggregate_scores` |
| Bounded retry with explicit root-cause diagnosis | theorem-explain-agent | Diagnose the cause before emitting a fix, cap at N attempts | `components/reason/proving/repair.rs` | IMPLEMENTED | `repair.rs:182` `localize_failing_step`, `:143` `RepairConfig`, `:197` `RepairRound` |
| Staged agentic RAG router with cached, stage-scoped queries | theorem-explain-agent | Classify the stage, then issue stage-specific retrieval queries, cache them | `components/retrieval/python/theoremata_tools/query_rewrite.py`, `components/reason/orchestration/context_assembly.rs` | PARTIAL | Query rewriting and context assembly exist; no explicit stage router with a query cache |
| Anti-memorization robustification of ingested NL problems | robust-mathematical-reasoning | Paraphrase, rename objects, reparametrize, change constants, add distractors | `components/eval/python/theoremata_tools/benchmarks/adversarial.py` | IMPLEMENTED | `components/eval/python/theoremata_tools/benchmarks/adversarial.py` plus `test_adversarial_fixtures.py` |
| IMO-Bench style tracks in the eval harness | robust-mathematical-reasoning | Answer / proof / grading tracks with the 7-6-1-0 mapping | `components/eval/tests/test_imo_bench_tracks.py`, `benchmarks/schema.py` | IMPLEMENTED | `components/eval/tests/test_imo_bench_tracks.py`; `benchmarks/schema.py`, `benchmarks/graders.py` |
| Adopt MathOlympiadBench and a miniF2F statement-quality lint | goedel-prover | Ingest a hard eval set and flag weaker-than-informal statements | `components/eval/python/theoremata_tools/benchmarks/` | PARTIAL | The benchmark registry and loaders exist (`benchmarks/registry.py`, `loaders.py`, `formal_conjectures.py`); no MathOlympiadBench loader or miniF2F statement lint found |
| Strong PNT blueprint parsed into a proof-DAG eval fixture | strong-pnt-paper | Convert the 621-item dependency chain into a JSON DAG fixture for blueprint and import tests | `components/eval/python/theoremata_tools/benchmarks/data/strongpnt_dag.json` | IMPLEMENTED | `components/eval/python/theoremata_tools/benchmarks/data/strongpnt_dag.json` exists |
| Informal-lemma to Mathlib-name retrieval benchmark from the same blueprint | strong-pnt-paper | Labeled retrieval pairs for library-reuse hit-rate | `components/retrieval/python/theoremata_tools/retrieval_eval.py` | PARTIAL | `retrieval_eval.py` exists as the harness; no evidence the Strong PNT pairs are loaded as its dataset |
| Atomization granularity plus short semantic name tags as blueprint style | strong-pnt-paper | Target style for generated blueprints, doubles as a dedup signal | `components/reason/orchestration/blueprint_generate.rs` | PARTIAL | The generator exists; no explicit atomization or naming convention enforcement |
| Port Strong PNT mathematics | strong-pnt-paper | Classical analytic number theory content | none | SKIPPED | Doc says explicitly "no mathematical technique or prover mechanism to port" |
| Autonomy times significance taxonomy and HAI interaction cards | aletheia | Tag each result with autonomy level and significance, emit a machine-readable interaction card | `components/reason/orchestration/trace.rs`, `certification.rs` | NOT-BUILT | grep for autonomy level or HAI card metadata finds only an unrelated `wolfram_recognizer.py` hit |
| Conjecturing loop: solve, generalize, weaken hypotheses, prove, find optimality examples | aletheia | Explicit generalize-a-solved-problem workflow feeding falsify | `components/reason/proving/conjecture_engine.rs` | PARTIAL | `conjecture_engine.rs` exists and uses subsumption and abstention; the five-stage generalize-and-weaken loop with optimality-example search was not found as a named pipeline |
| Four-way outcome reporting (fundamentally flawed / technically correct / meaningfully correct / ambiguous) | aletheia | Report the research-grade failure taxonomy, not a single solve rate | `components/eval/python/theoremata_tools/eval_harness.py` | PARTIAL | `eval_harness.py:426` `_aggregate_axes` reports multi-axis outcomes; not this specific taxonomy |
| Novelty checker over discovered objects | aletheia, deepmind-articles | Is the found object or result already known | `components/retrieval/python/theoremata_tools/novelty.py` | IMPLEMENTED | `components/retrieval/python/theoremata_tools/novelty.py` plus `test_novelty.py` |
| Framing only, nothing to build | on-mathematical-superintelligence, von-neumann | Positioning, epoch taxonomy, cost-aware-search and redundancy north-stars | none | SKIPPED | Both docs state explicitly that there is no engineering gap and no code to adopt |
| TPTP FOF as a normalized interchange format for pluggable first-order backends | atp-for-prolog-verification | One normalized problem format so any first-order prover is pluggable | `components/reason/search/model_elimination.rs`, `components/prover/backends/external.rs` | PARTIAL | `model_elimination.rs:504` `parse_clause` gives a clause format and `backends/external.rs` is the external-prover seam; no TPTP FOF emitter |
| Portfolio-of-provers with per-timeout success curves in the eval harness | atp-for-prolog-verification | Report 1s / 10s / 60s success per backend, not a single number | `components/eval/python/theoremata_tools/eval_harness.py` | NOT-BUILT | No per-backend per-timeout success curve reporting found |
| Induction-axiom synthesis from the conjecture shape | atp-for-prolog-verification | Statically instantiate an induction principle when handing a goal to a first-order backend | `components/reason/proving/` | NOT-BUILT | No induction-axiom synthesis found |

---

## Counts

Total rows: 166 (39 soundness, 127 capability/quality).

| Status | Count |
|---|---|
| IMPLEMENTED | 85 |
| PARTIAL | 47 |
| NOT-BUILT | 20 |
| BLOCKED | 9 |
| SKIPPED | 5 |

One row (quantum ATP backend) carries a combined SKIPPED / BLOCKED status and is
counted once under SKIPPED above.

Note on the boundary: 6 of the 9 BLOCKED items also have a working offline scaffold in
the tree. They are BLOCKED rather than PARTIAL because the paper's mechanism IS the
trained artifact, and the scaffold produces no signal without it.

---

## Disagreements with the calibration examples given in the brief

Five of the seven calibration examples check out exactly as stated. Two do not.

1. **"trained meta-verifier reward (DeepSeekMath-V2) -> components/train/"** is only half
   right. `components/train/python/theoremata_tools/reward.py` implements the reward
   algebra faithfully (`graded_verifier_reward` at `:245`, `graded_generator_reward` at
   `:272`), and `flywheel.py` implements the auto-labeling rule. But there is no trained
   verifier and no trained meta-verifier anywhere in the tree. `grpo.py:204` defaults to
   `dry_run=True`. Calling this IMPLEMENTED would be the single most misleading claim in
   this backlog, so I marked the reward formula IMPLEMENTED and the trained meta-verifier
   BLOCKED, separately.
2. **"MCTS-Q process supervision -> components/reason/search/process_reward.rs"** is
   correct and is genuinely exercised: `dag_projection.rs:357` `project_search_dag` is
   called from `app/lib.rs:1434`, so Q targets run over real driver DAGs, not only mocks.
   That one is stronger than the brief implies.

The other five confirm: `library.rs` has all four LEGO evolve directions at `:49`;
`proof_grader.py` has the full marking-scheme path at `:861` and `:973`;
`subsumption.rs:105` implements the subsumption predicate; `Abstained` is a real terminal
state at `certification.rs:101`; `skest.rs:89` has the `SharedFacts` cross-tree pool.

---

## Highest-value list

Ranked by soundness first, then by how much capability the item unlocks per unit of work.

1. **Feed a real `CriticScorer` into the driver.** The entire seam is built and unused
   (`driver.rs:277`, `critic_scorer.rs:100`), and InternLM's headline result is 59.4 to
   65.9 on miniF2F from exactly this wire. Even the fallback ridge critic from
   `process_supervision.py:269` would be a first real signal. This is the largest
   ratio of value to remaining work in the whole slice.
2. **Replace `with_exact_dedup` with the subsumption deduper in the lemma library.**
   `subsumption.rs:105` already exists; `library.rs:139` defaults to string equality.
   This is a one-line injection change that turns the growing library from a
   near-duplicate accumulator into a real one, and doubles as cache eviction.
3. **Build the LEGO request store.** The `Evolver::solve_request` seam exists
   (`library.rs:88`) with no store, no queue, and no worker behind it. Roughly 89 percent
   of LEGO's library came from the evolver's request-solver and directional transformer;
   we have the transformer half and none of the request half.
4. **Thinking-token decoupling in the critique step.** Aletheia's cheapest and highest
   reliability lever, and NOT-BUILT. Strip generator chain-of-thought before the critic
   reads the claimed proof.
5. **Tactic blocklist for `apply?` and `exact?`.** Trivially cheap soundness hardening
   that `statement_preservation.rs` does not currently cover.
6. **Citation-faithfulness check.** Both Aletheia and AlphaProof Nexus independently
   report that the residual hallucination mode is a real citation with a misquoted
   result, and that top sketches lean on `sorry` lemmas claimed to be known literature.
   We retrieve premises but never verify the premise states the claim.
7. **Eager well-typedness check on tactic-produced terms.** Aristotle calls this out as
   an add most systems lack; it catches errors before the kernel and shortens the loop.
8. **Hash-based similar-triangle and cyclic-quad detection in the geometry engine.**
   AG2's 300x speedup came mostly from this plus the C++ elimination. Our `UnifiedAR`
   (`geometry_ddar2.py:451`) already has the normal form the hash needs.
9. **Per-backend, per-timeout success curves in the eval harness.** Two separate docs
   (Prolog-ATP and Bridge 2010) argue portfolio decisions cannot be made without them,
   and we currently cannot ablate whether MCGS overhead pays for itself.
10. **Analysis string in the generator prompt.** Feeding the symbolic engine's
    provable / provable-if-goal / numerically-true sets is a prompt-assembly change to
    `context_assembly.rs` with no model dependency, and AG2 attributes real solve-rate
    gains to it.

---

## Built but never actually exercised

This is the class the brief asked to surface: code that exists, is unit-tested, and is
invisible in a normal status check because nothing about it looks broken, but which has
never run against a real model, a real corpus, or a live caller.

**Built, tested, and dead: no production caller.**

- `components/reason/search/critic_scorer.rs`. Only `HeuristicCritic` (`:112`) and
  `ConstantCritic` (`:123`) implement the trait. The single reference to `with_critic`
  outside the module is a doc-comment example at `hybrid_search.rs:395`.
- `components/reason/search/preference_pairs.rs`. `path_pairs` (`:202`),
  `sibling_pairs` (`:232`) and `mine_critic_pairs` (`:313`) produce Bradley-Terry
  training rows that nothing consumes, because no Bradley-Terry trainer exists.
- `components/reason/search/distance_critic.rs`. `encode_distance` (`:89`) manufactures
  the coarse-to-fine label; the head that would learn it is GPU-gated.
  `expansion_budget` (`:188`) IS consumed at `driver.rs:474`, so that half is live.
- `components/reason/search/best_first.rs:520` `dpo_pairs`. Correct DPO tuples with no
  DPO trainer downstream.
- `components/reason/proving/optimize.rs`. The metric framework and `optimize` (`:284`)
  exist; nothing runs proof optimization over harvested traces before export, which was
  ImProver's main proposed use for us.
- `components/reason/search/proof_pool.rs`. The pool and its stop condition are built,
  but the refinement scheduler needs verification analyses that only a trained verifier
  produces.

**Built, but only ever run against mocks.**

- `components/train/python/theoremata_tools/flywheel.py`. `revolution` (`:397`) and
  `graded_revolution` (`:469`) have `mock_generator` (`:369`) and `pattern_oracle`
  (`:384`) as their exercised inputs, and `dry_run` (`:577`) as the entry point.
- `components/train/python/theoremata_tools/grpo.py`. `train` (`:204`) defaults to
  `dry_run=True` and returns the config unexecuted.
- `components/train/python/theoremata_tools/reward.py` graded reward family. The algebra
  is correct; the scores fed into it are mock scores.
- `components/train/python/theoremata_tools/process_supervision.py:269`
  `train_value_head`. The closed-form ridge fallback is what actually runs.
- `components/train/python/theoremata_tools/selector.py`. The learned backend selector
  trains on proof logs we have not generated at scale from real runs.
- `components/reason/search/sampler.rs` `ModelSampler` and
  `components/prover/python/theoremata_tools/tactic_server.py`. The live tactic-serving
  path, exercised only under `THEOREMATA_MODEL_MOCK`.
- `components/reason/orchestration/statement_validation.rs` and
  `components/tools/python/theoremata_tools/roundtrip_audit.py`. The CC judge is the
  lexical stand-in offline; the divergence taxonomy has never been scored by a model.
- `components/eval/python/theoremata_tools/proof_grader.py` marking-scheme path. The
  scheme generator falls back to `_template_marking_scheme` (`:709`) and
  `_default_scheme_model` (`:829`) without a model; PROOFGRADER's whole result was that
  scheme quality drives grader accuracy, so the offline path proves the plumbing and
  none of the claim.

**Built, but the corpus it needs does not exist here.**

- `components/train/python/theoremata_tools/lean_corpus.py`. Parses without `leanc`,
  which is not installed on this machine, so it has never ingested a real Lean repo.
- `components/retrieval/python/theoremata_tools/retrieval_eval.py`. A harness with no
  labeled dataset loaded; the Strong PNT informal-to-Mathlib pairs would be the obvious
  first one and are not wired in.

The honest summary of this class: our search-side and verification-side Rust is real and
exercised. Almost everything that would need a trained model to mean anything is a
correct, well-tested shell. That is a defensible position, but it means the
IMPLEMENTED count above overstates delivered capability by roughly the size of this
list, which is why these rows are marked PARTIAL or BLOCKED rather than IMPLEMENTED.
