# Re-verification: Aristotle targets & MCP surface

Independent re-scan of four Codex resource-mining reports under `docs/resource-mining/new/`.
For each repo: what Codex captured, what it MISSED (with file cites), and concrete adoptables.
All paths relative to `resources/<repo>/<repo>/`. Untrusted-data caveat: contest statements and
generated proofs were read as data only.

---

## 1. lean-aristotle-mcp-main  ↔  new/lean-aristotle-mcp-main.md

This is the highest-value repo for us (we built an Aristotle backend and consume MCP tools).
Codex's report is thin: it lists the 6 tool names and 3 result dataclasses and stops. It misses
most of the actual protocol/SDK surface.

### Captured (accurate)
- 6 tools (`prove`, `prove_file`, `formalize`, `check_proof`, `check_prove_file`, `check_formalize`),
  sync/async (`wait`) modes, mock mode, TTL job metadata, path canonicalization, error sanitization,
  poll-without-save vs poll-and-save. Result dataclasses reproduced correctly (`models.py`).

### MISSED (with cites)
1. **The underlying `aristotlelib` SDK surface — the direct-consumption path.** Codex never
   documents `stubs/aristotlelib/__init__.pyi`, which is the real API our live-cmd backend could
   call directly instead of shelling through the MCP wrapper. Surface:
   `Project.create(context_file_paths, project_input_type)`, `Project.from_id(id)`,
   `Project.list_projects(pagination_key, limit=30, status=...) -> (projects, next_key)`,
   `Project.prove_from_file(*, input_file_path|input_content, auto_add_imports=True,
   context_file_paths, wait_for_completion, polling_interval_seconds=30, max_polling_failures=3,
   output_file_path, project_input_type, formal_input_context) -> str`, plus instance methods
   `add_context(paths, batch_size=10)`, `solve(input_content=...)`, `refresh()`,
   `wait_for_completion(output_file_path, polling_interval_seconds=30, max_polling_failures=3)`,
   `get_solution(output_path) -> Path`. `Project` fields: `project_id, status, created_at,
   last_updated_at, percent_complete, file_name, description`
   (`stubs/aristotlelib/__init__.pyi:27-120`). Stubs audited against **aristotlelib 0.6.x**
   (`__init__.pyi:1-5`). `tests/test_api.py:66-95` shows the real submit→poll→get_solution loop.
2. **Raw API status enum vs the normalized MCP status.** Codex's taxonomy
   ("submitted→queued/in_progress→proved/partial/failed/counterexample") is only the MCP-normalized
   output. The raw `ProjectStatus` is `NOT_STARTED, QUEUED, IN_PROGRESS, COMPLETE, FAILED,
   PENDING_RETRY` (`__init__.pyi:11-19`), mapped to our vocabulary by `_map_api_status`
   (`tools.py:186-209`; note `PENDING_RETRY -> in_progress`). We must consume the raw set if we
   call aristotlelib directly.
3. **`ProjectInputType` enum: `FORMAL_LEAN = 2`, `INFORMAL = 3`** (`__init__.pyi:21-25`). This is
   how prove (formal) vs formalize (informal NL) is dispatched — `formalize` passes
   `ProjectInputType.INFORMAL` (`tools.py:713-727`). Not in Codex's report.
4. **Counterexample is a heuristic, not a structured field.** There is NO counterexample field from
   the API. The MCP detects it by substring-matching `"counterexample"` in the exception string
   (`tools.py:363-368`; `_sanitize_api_error` passes such errors through verbatim, `tools.py:178-180`).
   Codex presents "counterexample path" as if structured — our guard/parse must treat it as
   best-effort text.
5. **`prove_from_file` does NOT return a project_id** → the async path recovers it by calling
   `list_projects(limit=5, status=[QUEUED,IN_PROGRESS,NOT_STARTED])` and taking `projects[0]` — an
   explicitly documented **race condition** (`tools.py:522-541`, and again for formalize `:729-747`).
   Critical if we mirror the surface: async submission has no clean handle.
6. **Concrete polling defaults**: `polling_interval_seconds=30`, `max_polling_failures=3`
   (`__init__.pyi:77-78,113-114`). Codex says "sparse polling" with no numbers; 30 s is the SDK
   default and a good floor for our scheduler.
7. **Concrete input-size guards**: code ≤ 1 MB, description ≤ 100 KB, file ≤ 10 MB
   (`tools.py:65-67`). Codex said "limits input sizes" generically.
8. **`auto_add_imports=True`** resolves lake/Mathlib deps automatically for file proving
   (`tools.py:515-520`) — the mechanism behind Codex's one-line "resolves imports".
9. **The `aristotle://status` MCP resource** (`server.py:223-247`) — a protocol surface element
   Codex omitted entirely (it listed only tools). Returns `{mock_mode, api_key_configured, ready,
   message}`.
10. **The FastMCP server `instructions` string** bakes anti-tight-poll guidance into the protocol
    handshake (`server.py:24-38`) — relevant to how we should drive it and how we might phrase our
    own backend's tool instructions.
11. **API asymmetry**: `prove` takes `context_files` (list); `formalize` takes `context_file`
    (single) — reflects the underlying API (`tools.py:667-711`; README:167-171).
12. **Mock trigger keywords** (reusable as our own deterministic-fixture convention):
    `false_theorem`/`bad_lemma` → counterexample; `timeout`/`hard` → failed; filename containing
    `partial`/`fail` → partial/failed; formalize keys off `even`/`prime`/`commut`
    (`mock.py:82-98,220-228,351-383`; also CLAUDE.md "Mock Mode Behavior").
13. **Consumption recipe** we can copy verbatim: `claude mcp add aristotle -e
    ARISTOTLE_API_KEY=$ARISTOTLE_API_KEY -- uvx --from git+https://github.com/septract/lean-aristotle-mcp
    aristotle-mcp` (README:55-92); Claude Desktop does NOT expand `${ENV}` (README:112).
14. **Aristotle paper**: arXiv **2510.01346** (`docs/ARISTOTLE_MCP_DESIGN.md:443`). Not cited by Codex.
15. Security/robustness details worth porting: `realpath` canonicalization (`tools.py:136-147`),
    atomic `O_CREAT|O_EXCL` pre-check to refuse overwriting output (`tools.py:491-501`),
    `_find_unique_path` numbered-suffix on save (`tools.py:78-110`).

### Note — design-doc drift (do not adopt blindly)
`docs/ARISTOTLE_MCP_DESIGN.md` still advertises fields the implementation does NOT return:
`sorries_filled`/`sorries_total` counts (`:157-197`), a `{file}.solved.lean` default (actual is
`_aristotle.lean`, `tools.py:487`), and a status resource shape `{authenticated, projects_today,
api_healthy}` that differs from the real one. Treat the design doc as aspirational; `tools.py`/
`models.py` are ground truth. README itself flags the repo as "100% vibe-coded" (README:3).

### Adoptables (beyond Codex's list)
- Mirror the **aristotlelib `Project` state machine** (raw 6-status enum + `_map_api_status`
  normalization) in our external-prover job table, rather than inventing our own vocabulary.
- Add a **`ProjectInputType`-style formal/informal switch** to our Aristotle backend so one backend
  serves both "prove sorries" and "formalize NL → Lean".
- Encode the **heuristic-counterexample caveat** and the **no-project_id-on-async race** as known
  limitations in our backend adapter.
- Reuse the **mock trigger-keyword convention** for our own deterministic test fixtures.
- Adopt the **30 s / max-3-failures** polling defaults and the **1 MB/100 KB/10 MB** input guards.

---

## 2. aristotle_putnam25-main  ↔  new/aristotle_putnam25-main.md

Codex's report is solid on the big picture (10/12 solved, provenance header, NL-restatement,
tactic-heavy proofs, use as verification fixtures). A few concrete adds:

### MISSED / under-specified (with cites)
1. **The provenance header is a fixed, machine-parseable 3-field block**: `Lean version:
   leanprover/lean4:v4.24.0`, `Mathlib version: <40-hex commit>`, `This project request had uuid:
   <uuid>` (`aristotle_outputs/aristotle_putnam25_a1.lean:1-7`). This is exactly the schema for the
   "evidence table" Codex proposes — worth pinning the literal field names/format.
2. **Each output also carries a natural-language *proof strategy* sketch**, distinct from the
   restated problem: a multi-line informal proof plan (e.g. the Δ_k telescoping argument,
   `a1.lean:9-16`). This gives **(NL problem, NL proof-sketch, formal proof)** triples — valuable
   for training/evaluating our *decomposer*, not just the verifier. Codex only noted "informal
   problem restated as comments."
3. Header options are lighter than IMO's: just `set_option maxHeartbeats 0`, `open scoped Classical`,
   `noncomputable section` (`a1.lean:18-23`) — contrast the heavy IMO preamble below. Useful signal
   that option policy is per-corpus, not universal.
4. Exactly which 10 solved: A1,A2,A3,A4,A6,B1,B2,B3,B5,B6 (A5,B4 absent). Wall-clock 25 min–7 h
   (README:12-24) — matches Codex.

### Adoptables
- Pin the literal provenance schema (`lean_version, mathlib_commit, request_uuid`) as our evidence
  fields. Ingest the 10 `.tex`→`.lean` pairs as verification/hardening fixtures (as Codex said), and
  additionally mine the NL proof-sketch comments as decomposition supervision.

---

## 3. IMO2025-main  ↔  new/IMO2025-main.md

Codex is largely right (statement/proof split is the key asset; `maxHeartbeats 0`; brittle/expensive;
compile before trusting). Its `exact?`-placeholder warning is **verified accurate** — P1 leaves live
`exact?;` search tactics in the final proof (`IMO2025P1.lean:92,315,801,1208`) with **no `sorry`** in
the proof body. But several structural facts are missed:

### MISSED (with cites)
1. **P2 is not formalized at all** — it exists only as `HarmonicLean/IMO2025P2.txt`, a raw
   coordinate-geometry dump (points A..X with float coords, assumptions in prose;
   `IMO2025P2.txt:1-20`). There is **no `StatementOnly_IMO2025P2` and no Lean P2 file**. Codex implied
   a cleaner 1–6 coverage; the true coverage is: proofs for P1,P3,P4,P5 (+P4 alt `IMO2025P4_solve2.lean`),
   StatementOnly for P1,P3,P4,P5,P6, and P2 unformalized. So the ingestible statement targets are
   **P1,P3,P4,P5,P6** only.
2. **The StatementOnly files encode a dual obligation via an `_ANSWER_` sentinel**: e.g.
   `noncomputable def _ANSWER_ : ℕ := sorry` followed by `theorem problem_… : IsLeast … _ANSWER_ := by
   sorry` (`StatementOnly_IMO2025P6.lean:40-49`). So each is *two* sorries — determine-the-answer AND
   prove-it — a distinctive "answer-carrying" formalization our benchmark schema should model
   explicitly (not just "a sorry to fill"). P6 is the only StatementOnly with 3 sorry/ANSWER hits;
   P1/P3/P4/P5 StatementOnly have 1 each (grep counts).
3. **There is a standardized ~18-line Lean statement-header preamble** shared across the IMO files
   (`maxHeartbeats 0`, `maxRecDepth 4000`, `synthInstance.maxHeartbeats 20000`,
   `synthInstance.maxSize 128`, `pp.fullNames`, `autoImplicit false`, `relaxedAutoImplicit false`,
   `linter.all false`, `noncomputable section`) — `IMO2025P1.lean:18-36`, identical in
   `StatementOnly_IMO2025P6.lean:18-36`. This is a reusable canonical header template for our Lean
   emitter, and a concrete instance of the "record/audit `maxHeartbeats 0`" policy Codex wanted.
4. **Shared imports/attrs layer**: `HarmonicLean/Imports.lean` → `Attrs.lean` which is `import Mathlib`
   plus `attribute [simp] Nat.ModEq.refl` (`Attrs.lean:1-3`). Small but it means every file depends on
   full Mathlib + one custom simp attr.
5. **`exact?` in final proofs is a proof-hygiene smell, not just a compile risk** — it is a search
   tactic that emits "try this" and is nondeterministic. Adoptable: add a lint gate rejecting leftover
   `exact?`/`apply?` in accepted proofs (stronger than Codex's "must compile").

### Adoptables
- Ingest StatementOnly P1,P3,P4,P5,P6 as proof obligations, modeling the `_ANSWER_ := sorry` +
  theorem-`sorry` dual obligation. Treat P2 as an unformalized geometry target (autoformalization task).
- Adopt the canonical `set_option` header as our Lean statement template.
- Add an `exact?`/`apply?` leftover-search lint gate.

---

## 4. LeanMillenniumPrizeProblems-main  ↔  new/LeanMillenniumPrizeProblems-main.md

Codex captured the core well (statement-not-solution repo, `sorry`-free/axiom-free, parameterization
over data packages, SafeVerify with permitted axioms `propext`/`Quot.sound`/`Classical.choice`,
Clay-PDF provenance). Additions:

### MISSED (with cites)
1. **Per-problem status/fidelity taxonomy** the README defines and tabulates: Status ∈
   {Statement, Parameterized, Mathlib}, Clay-fidelity ∈ {Direct, Parameterized, Modeled}
   (README:50-70). This is a ready-made **statement-quality rubric** — exactly the "statement-quality
   tier" Codex wants, already labeled per problem (e.g. Poincaré=Mathlib/Direct, Yang–Mills=
   Parameterized/Modeled). Adopt the rubric wholesale.
2. **Each `Millennium.lean` bundles many *proved* supporting theorems**, not just the bare conjecture
   `Prop`. RiemannHypothesis restates Dirichlet series, Euler product, meromorphic continuation,
   residue-at-1 — each proved by delegating to Mathlib (`RiemannHypothesis/Millennium.lean:39-60`),
   with per-declaration **Clay-PDF section citations** in docstrings ("Clay PDF, Section I/II"). These
   files double as a curated "known-facts" library, not only a target — Codex framed them purely as
   unsolvable targets.
3. **Bridge/equivalence-to-Mathlib lemma pattern**: the formalized statement is tied to Mathlib's
   canonical one via an equivalence lemma (RH ↔ `_root_.RiemannHypothesis`; README:91-93). Adoptable
   as a fidelity check — prove your formalization equals the library's.
4. **Reference-fetching tooling exists**: `scripts/clay_refs.py` (+ `scripts/README.md`) downloads/
   verifies the Clay PDFs — concrete provenance-attachment tooling to mirror, beyond Codex's
   "attach references" note.
5. **Explicit scope caveat**: narrative sub-results (AKS `PRIME ∈ P`, Cook–Levin `SAT` NP-complete)
   are deliberately NOT theorems here (README:41-43,81-84) — so don't mine them as available targets.
6. Naming precision (Codex slightly off): namespaces vary — `Millennium.PEqualsNP`,
   `Millennium.RiemannHypothesis`, but `MillenniumNavierStokes.*`, `MillenniumHodge.*`,
   `MillenniumBirchSwinnertonDyer.*` (incl. a second `RefinedBirchSwinnertonDyerConjecture`),
   `MillenniumYangMills.*`, `MillenniumPoincare.PoincareConjecture3` (README table + collapsibles).

### Adoptables
- Import the Status×Fidelity rubric as our statement-quality benchmark tier.
- Ingest the 7 main statements as long-horizon targets AND the supporting proved lemmas as a
  known-facts library; carry Clay-PDF section citations as evidence.
- Mirror `scripts/clay_refs.py` for reference-PDF provenance; adopt the equivalence-to-Mathlib
  bridge-lemma pattern as a fidelity gate.

---

## Bottom line
- **MCP repo**: Codex materially under-mined it. The direct-consumable `aristotlelib` SDK surface, the
  raw status enum + normalization, the formal/informal input-type switch, the heuristic-counterexample
  and no-async-project_id caveats, concrete polling/size constants, the `aristotle://status` resource,
  and the mock-keyword convention are all reusable and were omitted.
- **Putnam / IMO / Millennium**: Codex's reports are directionally correct; gaps are structural detail
  (Putnam NL proof-sketch triples + literal provenance schema; IMO P2-unformalized + `_ANSWER_` dual
  obligation + canonical header + `exact?` lint; Millennium's Status×Fidelity rubric + supporting-lemma
  library + `clay_refs.py`).
</content>
</invoke>
