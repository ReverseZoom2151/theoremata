# Re-verification: open-atp / proofgrader / python-memtools

Independent re-scan of three repos against the prior Codex reports in
`docs/resource-mining/new/`. Method: read the existing report, re-scan every
authored source/prose/config file (build artifacts, `.git`, vendored SHAs
sampled at entry points; large data catalogued), then diff against what
Theoremata has actually built since (the `components/prover/` FormalBackend/
ProofSession abstraction, `components/eval/python/theoremata_tools/proof_grader.py`,
`components/reason/critique/plan_history.rs`, `components/verify/python/theoremata_tools/lemma_cache.py`).

Bottom line: **open-atp report = solid** (one extra adoptable + one nuance).
**proofgrader report = has real gaps** — our `proof_grader.py` captured the
taxonomy + two of four workflows but missed the *entire* calibration/metrics
layer, the 0-7 ordinal scale, and marking-scheme rubrics. **memtools report =
solid** — and correctly a non-adoption; the "memory" in its name is CPython heap
debugging, not an agent memory architecture.

---

## open-atp-main

### Captured (report is accurate)
- The `LeanProject`/`ProofTask` → `ComputeBackend`/`ComputeSession` → `Harness`
  → `AutomatedProver`/`ProofResult` → `run_benchmark` split. Theoremata's
  `components/prover/formal.rs` (`FormalBackend`, `ProofSession`, `Workspace`,
  `VerificationReport`) mirrors this cleanly and generalizes it to Lean/Rocq/
  Isabelle — a genuine superset of open-atp's Lean-only object model.
- The warm-session reuse (generate + verify in one hot sandbox): `verify.py`
  `Verifier.verify(project, session=...)` runs the compile in an already-hot
  `ComputeSession` (`src/open_atp/verify.py:229`, `:293`). Theoremata's
  `ProofSession` covers this.
- The axiom-injection bug the report flagged is **real and still present**:
  `Verifier._compile_script` only runs `lake env lean "<file>"`
  (`verify.py:308-325`); `_parse_axioms` (`:343`) merely scrapes
  `depends on axioms: [...]` if it happens to appear in the log — nothing injects
  `#print axioms`. Theoremata's `audit_axioms` layer is the right fix and already
  exists.
- Numina statement-change guard: report captured the concept; Theoremata ported
  it (the vendored `numina_tracker.py` `StatementTracker` maps to Theoremata's
  guard layer).

### MISSED (with file cites)
- **Reject-on-mismatch pin gate, *before* spending compute.**
  `Verifier.check_compatible` (`verify.py:164-227`) rejects a project whose
  `lean-toolchain` / locked `mathlib_rev` differs from the sandbox image's pin
  (`ToolchainMismatch` / `MathlibRevMismatch`) *up front*, and `prove` calls it
  before any generation (`provers/base.py:209`). Theoremata's `formal.rs`
  `verify()` has no toolchain/corpus-revision compatibility precheck — it
  scaffolds and compiles unconditionally. Adoptable: a cheap pin-compatibility
  gate on `FormalProject` that fails fast rather than deep in a build, per
  system (`lean-toolchain`+Mathlib rev; `_CoqProject`+opam; Isabelle heap
  session id).
- **Per-file compile status + failure-isolating build script.**
  `_compile_script` runs each target with `;` (not `&&`) so one file's failure
  doesn't mask the rest, brackets each with `=== FILE / === EXIT $rc ===`
  markers, and `_parse_per_file` reconstructs a `{path: bool}` map
  (`verify.py:307-341`). Theoremata's `CompileReport { compiled: bool, errors }`
  (`prover/formal.rs:133`) is whole-project binary. Adoptable for multi-file /
  multi-lemma theories: per-unit pass/fail so a partially-good artifact is
  visible instead of collapsing to one boolean.
- **`ProofResult.to_dict` inlines completed source + omits the (large)
  compile_log** (`provers/base.py:119`, `verify.py:77`) — a deliberate
  "downloaded logs dir stands on its own" artifact convention. Minor, but the
  self-describing `result.json` layout (`{wd, logs}/`) is a clean pattern for
  Theoremata's `artifacts_dir`.
- **Statement-guard *restore*, not just detect.** `numina_tracker.py`
  `StatementTracker.restore_initial_statements` (`:350`) rewrites a
  modified/removed theorem back to its snapshot and re-appends a deleted one as
  `<stmt> := by sorry`. If Theoremata's guard only *detects* weakening, the
  restore-to-snapshot behavior is the adoptable half.

### Nuance
- The report calls verification "project-level binary success/failure, not
  proof-DAG/node-level." True for open-atp, but Theoremata already adds the
  3+1-layer gate (compile → axioms⊆whitelist → kernel re-check → source scan) in
  `formal.rs:202` — a strictly stronger gate than open-atp's compile/sorry/axiom
  triple, and it is *fail-closed* on an unavailable toolchain (`live_poll`,
  `formal.rs:485`). No gap; worth recording that ours already exceeds theirs.

---

## proofgrader-main

### Captured (report is accurate)
- Generate/evaluate separation, JSONL schemas, composite `<problem_id>::<generator>`
  IDs, lenient XML/JSON parser, and the workflow-plugin idea. Theoremata's
  `proof_grader.py` ported the taxonomy + `decompose_then_judge` (`mode="step_wise"`)
  and `single` (`mode="holistic"`), with a deterministic path + mock-capable
  LLM judge. Good, faithful subset.
- The "LLM grade is a triage signal, not a soundness certificate" framing — our
  `grade_proof_item` docstring repeats it and keeps the Lean gate as the real
  acceptance. Correct.
- LFS-pointer warning is **still true in this checkout**: `problems.jsonl`,
  `model_solutions.jsonl`, `expert_gradings.jsonl` are 129–132-byte
  `git-lfs`/spec pointer stubs (verified by size + header), not data.

### MISSED (with file cites) — these are the real gaps
1. **The entire evaluator-calibration metrics layer is absent from Theoremata.**
   `proofgrader/metrics/compute_evaluator_distances.py` is ~1200 lines computing,
   per evaluator × generator × source × true-score-bin: MAE, RMSE, bias, Pearson,
   Spearman, **Kendall tau-b** (`:96`), within-tolerance accuracy (`:220`),
   **pairwise order-preservation accuracy** (`compute_pairwise_order_stats`, `:260`),
   problem-normalized (÷ per-problem std) metrics (`:336`), macro-by-problem
   averaging (`:315`), **bootstrap percentile CIs** (`:374`), cross-evaluator
   **disagreement-per-item** (`:1037`), and a **verify-vs-solve** analysis
   (`:1169`). Plus `compute_evaluator_binary_metrics.py`. Theoremata's
   `proof_grader.py` emits a single `{score, verdict, per_step}` and has *no*
   machinery to calibrate a critic against expert/formal labels. This is the
   single highest-value miss: it is exactly what the memory notes say we want
   (calibrate LLM critics), and it is pure-stdlib, portable code.
2. **Ordinal 0–7 scale vs. our binary.** `templates/evaluation.yaml:41-47`
   defines an integer 0–7 rubric (0 = nonsense … 7 = fully correct, with
   partial-credit bands). Our grader is binary correct/flawed + a
   fraction-correct in [0,1] (`proof_grader.py:285`, `:378`). The metrics above
   assume a graded ordinal; without it we can't measure order-preservation or
   correlation with expert scores. Adopting the metrics layer implies adopting
   the 0–7 output.
3. **Marking-scheme rubrics** (`guides/MARKING_SCHEMES_GUIDE.md`,
   `scripts/generate_marking_schemes.py`): per-problem, model-generated
   checkpoint rubrics ("[2 pts] Establish…", zero-credit items, deductions,
   summing to 7) attached as a `marking_scheme` field and consumed by
   `with_marking_scheme_and_reference` templates. The guide reports this *raises
   correlation with human scores*. Theoremata has no reference-solution- or
   rubric-conditioned grading path at all — our judge sees only (problem, steps).
4. **Two workflows not ported.** `reflect_and_revise.py` is a 3-stage
   initial-report → self-critique → final-verdict pipeline with *per-stage
   models* (`:33-36`, stages A/B/C) — close to Theoremata's meta-critic but with
   an explicit staged-artifact contract we don't have. `repeat_and_aggregate.py`
   adds aggregation modes our grader lacks: `mean`, discrete-median (`middle`),
   `min`, `max`, and **majority-vote `consistency`** (`:35-56`), including a
   binary-label aggregation (all/any/majority). Our `repeat_and_aggregate` is
   only named in a docstring, not implemented.
5. **`data_validation.py` pre-flight** validates the dataset before any spend —
   a cheap contract check we should mirror on our eval inputs.

### New adoptables
- Port `compute_evaluator_distances.py`'s metric functions wholesale (stdlib
  only) as a `critic_calibration` module: feed it (predicted rubric score, label)
  pairs where the label is Theoremata's *formal* verdict (compile/sorry/axiom/
  node-completed) — turning proofgrader's "vs expert human" calibration into
  "critic vs formal ground truth," which is stronger than proofgrader's own
  human labels.
- Add the 0–7 ordinal + optional marking-scheme conditioning to `grade_proof`
  as an alternate output mode, keeping the current binary path for the gate.
- Implement the `consistency`/discrete-median aggregation for our
  repeat-sampling critic.

---

## python-memtools-main

### Captured (report is accurate and correctly recommends NON-adoption)
- Snapshot/analyze split, `MappedPtr<T>`, region index, type-object discovery
  heuristic, `analysis-data.json` cache, `ShellCommand` registry, depth/cycle-
  guarded `Traversal`, `direct_referents` graph API, async-task-graph cycle
  detection. All present as described across `src/AnalysisShell.{cc,hh}`,
  `src/MemoryReader.*`, `src/Types/*`.

### MISSED
- Nothing material. The report is complete and its non-port recommendation is
  right.

### Clarification on the task's "real memory architecture" question
- python-memtools is **not** an agent-memory system — it is a `/proc/<pid>/mem`
  CPython heap inspector (reconstructs `PyObject`s from raw memory to debug
  leaks/await-stalls). It offers **no** plan-history/lemma-cache/retrieval design
  to adopt. Theoremata's memory layer is correctly sourced elsewhere:
  `plan_history.rs` (event-sourced "Do NOT retry" strategy log, from QED) and
  `lemma_cache.py` (compilable `-- @stored-theorem` block store, from mathcode).
  Nothing in memtools competes with or improves those.
- The one transferable idea (already in the report): its cycle-guarded
  `Traversal` and `async-task-graph` cycle detection map to a Python-native
  watchdog for hung/looping proof runs (`asyncio.all_tasks()` + our existing
  `LoopGuard` in `components/reason/critique/guard.rs`). Reliability inspiration,
  not a memory architecture.
