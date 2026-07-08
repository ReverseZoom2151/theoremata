# Benchmark repos — re-verification of Codex resource-mining reports

Scope: independently re-scanned four BENCHMARK repos and cross-checked the Codex
reports under `docs/resource-mining/new/` against (a) the actual vendored data
schema/splits/counts and (b) our live loaders+graders in
`components/eval/python/theoremata_tools/benchmarks/` (`loaders.py`,
`graders.py`, `registry.py`, `schema.py`). Verdict per repo below, with concrete
data-backed citations. Two loaders have real field-name bugs that make their
benchmark ungradable; details in each section. Nothing committed.

Contamination note applies across all four: MiniF2F test, BRIDGE (LeetCode), and
QuantumLean (MITOCW + qiskit-katas + qm7, with model outputs baked in) are all
public/derivative corpora likely present in modern LLM pretraining.

---

## 1. datasets-main (Harmonic MiniF2F) — REPORT SOLID

Report: `docs/resource-mining/new/datasets-main.md`.

### Captured (confirmed against data)
- Fields exactly `['formal','id','name','natural']` on every record (verified all
  three splits). Report's `id/name/natural/formal` is correct.
- Counts exact: train **389**, validation **48**, test **48**, total **485**.
  Report's "nominal 488 / 3 missing train" is right (classic MiniF2F is
  244 valid + 244 test = 488; Harmonic re-split to 389/48/48).
- `formal` bodies end in `by sorry` — verified **48/48** in test; suited to
  proof-completion + statement-preservation. Report correct.
- `id` values unique within split (48/48 unique in test).

### Our loader/grader status
- `load_minif2f_{train,valid,test}` + `load_minif2f` present and correct. Maps
  `id/natural/formal/name`, keeps `by sorry`, carries `minif2f_id`/`name` for
  stable identity, emits `formalization` kind with `task=proof_completion`.
- `grade_formalization` statement-preservation + axiom gate is the right rubric.
  No gaps.

### MISSED / adoptables (minor)
- The Harmonic README also flags **"3 missing train problems"** and that this is
  a *random* split (not the historical valid/test split) — worth recording in
  provenance so cross-paper number comparisons aren't apples-to-oranges. Report
  mentions the 3-missing but not the random-split caveat's effect on
  comparability.
- No per-split `name`-collision check across splits (train vs test can share a
  theorem `name`); use the split-scoped `minif2f:{split}:{id}` uid (we do) as the
  join key, never bare `name`.

---

## 2. BRIDGE-main (BRIDGE-178) — GAPS: loader loads ZERO signatures → ungradable

Report: `docs/resource-mining/new/BRIDGE-main.md`.

### Captured (confirmed against data)
- `datasets/bridge178.jsonl` = **178** rows; `manifest.json` `num_tasks: 178`.
- Top-level keys exactly `['dataset_id','difficulty','lean','problem_statement',
  'python','tags','task_id','title_or_source_id']` + `tests`. Report's row shape
  is right.
- `tests` = `{'inputs','expected_outputs'}`; `difficulty ∈ {easy,medium,hard}`.
- Pipeline (`render_prompts.py` → `openai_compatible_generate.py` →
  `extract_fenced_code.py` → external checker → `error_correction_loop.py`) and
  prompt conditions (direct/functional/imperative/spec/theorem/proof) all confirmed.

### MISSED — with citations
1. **Lean metadata key is `function_signature` (singular string), NOT
   `signatures`/`signature`.** The `lean` object is
   `{'function_name','function_signature','arguments','argument_types'}` (e.g.
   `{"function_name":"minimumPushes","function_signature":"def minimumPushes (word : String) : Int", ...}`).
   Our `load_bridge178` reads `lean_meta.get("signatures") or lean_meta.get("signature")`
   → **always `[]`**. Downstream, `grade_verified_programming` gates on
   `signatures_ok = bool(signatures) and len(sig_hits)==len(signatures)`; with an
   empty list `bool([])` is False → **`is_correct` is always False for all 178
   tasks.** BRIDGE is currently unscorable. Report did not catch this (it only
   said "lean metadata including function signatures" generically).
   Fix: `signatures = [lean_meta["function_signature"]]` (+ carry
   `function_name`, `arguments`, `argument_types`).
2. **`tests.inputs` is a list of *named-kwarg dicts*, not positional lists** —
   e.g. `[{"word":"abcde"}, {"word":"b"}, ...]` keyed by the Lean/Python arg name.
   Any oracle runner must bind by argument name (join with `arguments`), not by
   position. Report says only "inputs/expected_outputs".
3. **`python` metadata is `{'function_name','function_signature'}`** (a real
   Python signature to cross-check against the Lean one) — we drop it entirely.
4. **Provenance = LeetCode.** `title_or_source_id` values are LeetCode
   weekly/biweekly contest slugs (`weekly-contest-381-minimum-number-of-pushes-...`).
   `tags` is uniformly `['algorithms']` (178/178); `dataset_id` uniformly
   `bridge178`. Strong **contamination** signal (public LeetCode problems) — must
   be flagged when reporting pass@k. Report omitted the LeetCode origin.
5. `external_benchmarks/` are **docs/adapters, not vendored data**: VERINA
   (187 tasks, HF `sunblaze-ucb/verina`), CLEVER (161 tasks, HF `amitayusht/clever`,
   separate prover repo), DafnyBench (782 tasks, Dafny verifier pass@5). These are
   the paper's *comparison axis*, not ingestible corpora here. Report lumped them
   as "external benchmark adapters" without the counts/metrics.

### Ingestion adoptables
- P0 (bug): fix the `function_signature` key so BRIDGE is gradable at all.
- P1: bind oracle `inputs` by argument name; store `arguments`/`argument_types`.
- P2: keep `python.function_signature` for Lean↔Python signature consistency.
- P2: tag items `source=leetcode` in provenance for contamination filtering.

---

## 3. QuantumLean-Bench-main — GAPS: "solutions" are model-output dicts, not gold; loader mis-stringifies

Report: `docs/resource-mining/new/QuantumLean-Bench-main.md`.

### Captured (confirmed against data)
- **931** records across **11** JSON files. Domain counts match report exactly:
  chemistry **306**, computing **220**, cryptography **4**, info-theory **86**,
  physics **215**, QML **100** (domain field values are `quantum_chemistry` etc.).
- Union of record keys = `['citations','domain','id','manual_eval','metadata',
  'problem','solution_formal','solution_informal','source','type']` — matches the
  report's field list. `id` present on all 931.
- Two eval modes (`run_informal.py`, `run_formal.py`+`lean_verify.py`) and the
  per-domain `Common.lean` + domain scaffold confirmed.

### MISSED — with citations
1. **`solution_informal` and `solution_formal` are DICTS keyed by model name,
   not gold solutions.** Sampled Physics `5.73_0001`:
   `solution_formal = {'gpt5.4_response': 'import Mathlib\n...'}`,
   `solution_informal = {'gpt5.4_response': '...'}`. There is **no gold
   reference** anywhere in the corpus — the dataset ships *model outputs* plus a
   human score. The repo's own `core/schema.py` says as much ("old solutions may
   be strings; newer model outputs live in maps"). Report only softly noted "some
   solution_informal fields already contain model outputs" — it's actually **all
   931, and there is no gold at all.**
2. **`manual_eval` is the grading signal**: a 0–2 human rubric
   (`{'scale':'0-2','rubric':{'2':'Correct or substantially correct...','1':
   'Partially correct...','0':...},'evaluated_at':...}`) attached to a model's
   response. Faithful ingestion = generation + Lean **typecheck-only** (no
   statement-preservation is possible without gold), optionally scored against the
   manual-eval rubric. Report noted "typecheck alone may reward shallow snippets"
   but didn't identify manual_eval as the actual metric.
3. **Our loader corrupts this**: `load_quantumlean` does
   `formal = rec.get("solution_formal"); ... str(formal) if formal else None`,
   so `item['formal']` becomes the **Python repr of a dict**
   (`"{'gpt5.4_response': 'import Mathlib...'}"`), and it routes to
   `grade_formalization` (`method=comparator_or_statement`) which then does
   statement-preservation against that repr string. This is meaningless —
   QuantumLean has no gold statement to preserve. Loader + track assignment are
   both wrong for this corpus.
4. **`type` drives prompt routing and is richer than reported**: values are
   `numerical 372, algorithmic 283, proof-based 244, derivation 16,
   computational 4, conceptual 3, open-ended 9`. Only the **244 proof-based** map
   to the formal Lean track; the rest are informal. Prompt templates are per
   `(mode × type)`: `formal_theorem/formal_numerical/formal_algorithmic` and
   `informal_*`. Our loader ignores `type`-based routing entirely.
5. **Sources are public/derivative**: `source` values like "MIT OpenCourseWare,
   5.73"; files include `qiskit-quantum-katas_problems.json`, `qm7_problems.json`
   → contamination + licensing to track. Report didn't enumerate.

### Ingestion adoptables
- P0 (bug): stop `str()`-ing the model-output dict into `formal`; QuantumLean has
  no gold — model responses live under `solution_formal[<model_key>]`.
- P1: re-track QuantumLean as **typecheck-only formalization** (or a new
  `scientific_formalization` grading method), not comparator/statement-preservation.
- P1: ingest `manual_eval` (0–2 rubric) as the scoring channel; keep multi-model
  response-key map intact (the report's response-key idea is right — but the keys
  are real model names, not a literal `"response_key"`).
- P2: filter/route by `type` (only proof-based → formal track).

---

## 4. flare-main (FormulationBench / FLARE) — REPORT SOLID (loader has one resolution gap)

Report: `docs/resource-mining/new/flare-main.md`.

### Captured (confirmed against data)
- `dataset/dataset.json` = `{'problems':[1..20], 'reformulations':[96]}`.
  Counts exact: **20** problems, **116** `formulation.json` files on disk,
  **96** reformulation pairs, **70 positive / 26 negative**
  (`sum(reformulation==true)`). Report's 20/116/96/70/26 all correct.
- Reformulation record shape `{'a':{'problem','formulation'},'b':{...},
  'reformulation':bool}` confirmed. `ReformulationVerifier.start/cancel/result`
  interface and per-attempt artifact dirs (Markdown / `solve.py` / Lean) confirmed.

### Our loader status
- `load_formulationbench` reads `reformulations`, keys off `reformulation`
  boolean, emits `reformulation` kind → `grade_reformulation` (equivalence-claim
  keyword grader). Counts and pos/neg orientation are correct.

### MISSED / adoptables
1. **Loader stores only the pointer, not the artifact.** `expected.formulation_a/b`
   are just `{problem:1, formulation:'a'}` references; the actual obligation lives
   in `dataset/problems/p{n}/formulations/{x}/Formulation.lean` (+ `formulation.json`
   params, `description.md`). To grade a real machine-checked equivalence you must
   resolve the pointer to those files. Report didn't note the loader wouldn't have
   the Lean bodies. Adoptable: resolve pointers → inline the two `Formulation.lean`
   texts + params into the item.
2. `grade_reformulation` is keyword-only (`lean_proof_checked:false`) — matches
   FLARE's *claim* but not its *proof* obligation; the whole point of FLARE is a
   checked Lean equivalence proof. Note as a known stub, not a faithful grader.
3. Ops caveats confirmed: needs Docker + harness creds; Gurobi license for the
   `solve.py` execution baseline. Report captured these.

---

## Cross-repo summary of the two hard loader bugs

| Corpus | Bug | Effect |
|---|---|---|
| BRIDGE-178 | reads `lean.signatures`; real key is `lean.function_signature` | `lean_signatures=[]` → `grade_verified_programming` `is_correct` always False (0/178 gradable) |
| QuantumLean | `str(solution_formal)` where value is a `{model: lean}` dict; routed to comparator/statement-preservation with no gold | `formal` = repr of a dict; grading meaningless; wrong track |

MiniF2F and FLARE loaders are faithful (FLARE needs pointer→artifact resolution
for real proof grading). Contamination to flag on ingest: MiniF2F-test (standard,
public), BRIDGE (LeetCode contests), QuantumLean (MITOCW/qiskit-katas/qm7 + model
outputs embedded).
