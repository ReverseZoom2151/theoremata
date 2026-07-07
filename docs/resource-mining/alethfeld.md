# Resource Mining: `alethfeld-legacy`

**Mined:** 2026-07-07
**Location:** `C:\Users\adria\Downloads\math-agent\resources\alethfeld-legacy\alethfeld-legacy\` (note the doubled directory)
**Scope of pass:** FULL. Read in full: `README.md`, both stable/experimental orchestrator prompts, all `cli/src/alethfeld/schema/*.clj`, `cli/src/alethfeld/graph.clj`, `cli/src/alethfeld/validators.clj`, `cli/src/alethfeld/ops/update_status.clj`, `docs/architecture.md`, `docs/proof-format.md`, `AGENTS.md`, `.claude/commands/verify-proof.md`, `examples/README.md`, `examples/brokenmath/README.md`, `examples/brokenmath/hmmt_feb_2025_7/REPORT.md`, the full `aime_2025_20/graph.edn` + `Counterexample.lean`, and `determinant-rank1-perturbation-v4.edn`. The large `examples/` tree was additionally catalogued by a sub-agent (§3 draws on both). Skipped: `.olean/.ilean/.trace/.hash/.c/.ir` build artifacts, `.git`, PDFs, and `cli/target/classes` (compiled duplicates of `cli/src`).

---

## 1. What it is (scope, size, structure)

**Alethfeld** is an **archived (Nov 2025 – Jan 2026) experiment** in producing machine-checkable math proofs by coordinating **adversarial LLM agents**. It has been superseded by **Vibefeld** (a Go rewrite; event-sourced ledger, natural-language math, automatic hierarchical numbering, built-in taint propagation, filesystem multi-agent concurrency). Alethfeld itself is *not code that runs the agents* — it is **(a) a set of orchestrator prompt files** that an off-the-shelf coding CLI (Claude Code / Gemini / Codex) executes, plus **(b) a Clojure/EDN CLI (`alethfeld`)** that is the *only* sanctioned way to mutate the proof graph, plus **(c) a Lean 4 library** of formalized results, plus **(d) a large worked-example corpus**.

The origin story matters for us: the design came from asking Claude *"what would help you prove theorems more reliably?"* → the answer was **structured (Lamport) notation, adversarial verification, explicit citations (no "well-known"), and proof obligations**. Everything in the harness derives from LLM-identified failure modes.

**Size:** 461 non-artifact files. Component layout:
- `cli/` — Clojure CLI (Babashka/Clojure + Malli schemas). `src/alethfeld/{schema,ops,commands}/`, plus a substantial `test/` tree (property, mutation, concurrency, integration tests).
- `lean/` — `AlethfeldLean` Mathlib-based library: Quantum (~3800 LOC), QBF/Rank1, Reconstruction, Computability, plus `Examples/BrokenMath/`.
- `examples/` — ~15 top-level worked proofs + `brokenmath/` (10 adversarial problems). Each usually has `.edn` (canonical graph), `.tex`/`.pdf` (Lamport LaTeX), `.lean` (skeleton), and often `REPORT.md`/`HANDOFF.md`/`session-transcript.md`.
- `orchestrator-prompt-v5.1-*.md` (Claude/Gemini/Codex) and `orchestrator-prompt-v5_2-claude.md` — the actual "agent brains."
- `docs/`, `scripts/` (ansi-viz graph renderer, validate-graph standalone, deprecated prompts), `.claude/commands/` (slash commands: verify-proof, extract-lemma, fix-sorries, etc.), `.beads/` (bd issue tracker).

**Roles (7 agents), all defined as prose inside one orchestrator prompt:** Orchestrator (state machine), Adviser (strategy/theorem-audit), Prover (emits graph deltas in EDN), Verifier (adversarial), Lemma-Decomposer, Reference-Checker (web-search citations), Formalizer (→ Lean skeleton), LaTeX-er. The **Prover↔Verifier adversarial loop** (max 7 rounds/step, 50 total) is the core.

---

## 2. Reusable ideas / patterns / code for Theoremata — THE priority

### 2.1 The proof-DAG core (directly maps to Theoremata's "proof-DAG core")

The **semantic graph is the single source of truth; EDN is only serialization** (Design Principle 1). This is exactly Theoremata's graph-first stance. Concrete pieces worth porting:

**Node schema** (`cli/src/alethfeld/schema/node.clj`, and the prose spec in `orchestrator-prompt-v5_2-claude.md` §II.1). Every node:
```clojure
{:id :<depth>-<6hex>        ; e.g. :1-a3f2b1  — PERMANENT, never renumbered/reused
 :type   #{:assumption :local-assume :local-discharge :definition
           :claim :lemma-ref :external-ref :qed}
 :statement "LaTeX"
 :content-hash "<sha256>"    ; computed by CLI
 :dependencies #{:<node-id> ...}   ; the DAG edges
 :scope        #{:<local-assume-id> ...}
 :justification <one of 25 fixed keywords>
 :status  #{:proposed :verified :admitted :rejected}
 :taint   #{:clean :tainted :self-admitted}
 :depth :int :parent :<id>|nil :display-order :int
 :provenance {:created-at ISO8601 :created-by #{:prover :orchestrator :extraction}
              :round :int :revision-of :<id>|nil}}
```
Key policies (all reusable design decisions):
- **Stable UUID-ish IDs; revisions get NEW ids and archive the old, linked via `:revision-of`** (§II.2). Never mutate-in-place a claim.
- **Fixed justification vocabulary** (25 keywords, `schema/enums.clj` `Justification`): `:modus-ponens :universal-elim :existential-intro :case-split :induction-base/step :contradiction :substitution :algebraic-rewrite :lemma-application :external-application :admitted :qed` … Constraining the Prover to a closed set makes the Verifier's job tractable ("constraints the search space and makes checking easier", `docs/proof-format.md`).
- **A step is proven EITHER by an atomic justification OR by a complete set of substeps ending in QED** — never both, never hand-wave.

**Taint propagation** (`cli/src/alethfeld/graph.clj` `compute-taint`, `recompute-all-taints`; `ops/update_status.clj`). This is the executable version of "theorems depending on `sorry` are tainted":
```clojure
(defn compute-taint [graph node-id]
  (cond
    (= :admitted status) :self-admitted
    (= :rejected status) :tainted
    (= :lemma-ref type)  (if (#{:tainted :self-admitted} (lemma-taint)) :tainted :clean)
    :else (if (some #{:tainted :self-admitted} dep-taints) :tainted :clean)))
```
`update-status` recomputes taint for the node **and all transitive descendants in topological order** (`recompute-taints-from` → `get-descendants` → `topological-sort`), and auto-adds/removes a proof **obligation** when a node enters/leaves `:admitted`. Three-valued taint (`:clean` / `:tainted`=propagated / `:self-admitted`=the gap itself) is cleaner than a boolean and worth adopting.

**Graph algorithms already written** (`graph.clj`, pure/stateless, ~400 LOC — portable logic even if we don't reuse Clojure): `get-ancestors`/`get-descendants` (DFS closure), `topological-sort` (Kahn, with a partial "targets-only closure" arity for efficiency), `compute-valid-scope`/`compute-all-scopes` (single O(n) topo pass computing which `local-assume`s are live minus discharged), `would-create-cycle?`, reverse-dep cache in metadata, and **token estimation** (`estimate-graph-tokens`: per-node chars × tokens-per-char + lemma/ref overhead — used for context-budget gating).

**Semantic validators** (`cli/src/alethfeld/validators.clj`) — a ready checklist for a graph linter: referential integrity (deps/parent/lemma/external/symbol/lemma-root all resolve), **acyclicity via DFS 3-coloring with cycle-path reconstruction** (`find-cycle`), **scope validity** (no node references an out-of-scope or discharged assumption), **discharge validity** (a `:local-discharge` must target an in-scope ancestor), and **taint correctness** (stored taint == recomputed taint). `assert-valid-graph!` runs these as postconditions inside every mutation — an invariant-at-the-source pattern we should copy.

**Lemma extraction / decomposition** (`graph.clj` `check-independence`; orchestrator §III.6, §VII.4). A node set S rooted at R is an extractable independent lemma iff: R∈S; all nodes verified/admitted; **only R is depended-on from outside S**; internal deps satisfied (external deps must be assumptions/verified/lemma-refs); **scope balanced** (every `local-assume` in S has a matching `local-discharge` in S). The Decomposer scores candidates: `benefit = 0.3·size_reduction + 0.3·isolation + 0.2·reusability + 0.2·depth_reduction`, only proposing if >0.4. This is the "decompose theorems into obligations" primitive with an actual acceptance metric.

### 2.2 The orchestration state machine (maps to Theoremata's single orchestrator loop)

`orchestrator-prompt-v5_2-claude.md` §VI is a **fully explicit state machine** — 30+ transitions each written as `{:from :condition :action :next}`. States: `init → theorem-audit → strategy → skeleton → skeleton-review → decomposition → expand-verify-loop → reference-check → finalization → complete` with `escalated` as the other terminal. The `expand-verify-loop` is the heart: pop from `pending-expansions`, Prover expands, add to `pending-verifications`, Verifier returns `:accept`/`:challenge`/`:reject`, on reject the node is `update-status … rejected` then Prover revises. **Iteration limits are first-class** (§V.1): `strategy-attempts 2, skeleton-revisions 5, decomposition-rounds 3, expansion-per-step 5, verification-per-step 7, total-verification-rounds 50, adviser-diagnoses 3`; hitting a limit → **escalate to human with full context** rather than loop forever. Directly reusable as the skeleton of our agent loop.

**Subagent dispatch protocol** (§VI.3): input is constructed as explicit EDN matching the subagent's schema, orchestrator must *not* assume the subagent's role, must wait for response, parse it, and announce the state transition. This maps onto our LiteLLM-provider sub-agent calls.

### 2.3 Falsify-before-prove / critic gold (the `brokenmath` design)

This is the highest-value transferable content for our **"falsify-before-prove → … → critic"** pipeline. It is not just data; it is *encoded anti-sycophancy prompt engineering derived from measured benchmark failures* (v5.1 was literally created from the BrokenMath post-mortem).

**Design Principle 6:** *"Detection over sycophancy: Finding an error is MORE VALUABLE than producing a flawed proof."* Promoted to a terminal invariant.

**Theorem-Audit phase (Adviser, `:request :theorem-audit`)** runs *before* any proving when source is unknown/competition:
```clojure
{:theorem-audit {:plausibility #{:high :medium :low :suspicious}
                 :concerns [...] :suggested-sanity-checks ["compute X directly" ...]
                 :recommendation #{:proceed :verify-first :refuse}}}
```
`:suspicious`/`:refuse` → escalate immediately. Prompt heuristics (VII.1): order-of-magnitude check on numeric claims, "could the inequality direction be wrong?", "does the problem *smell* adversarial?", plus **Domain Traps** (log negative for 0<x<1; sqrt sign; unbounded optimization) and **Counting Traps** (labeled vs unlabeled, ordered vs unordered, with/without replacement).

**Verifier Anti-Sycophancy Protocol (VII.3)** — reusable almost verbatim as our critic prompt. "Your primary failure mode is ACCEPTING FALSE THEOREMS." Before accepting any claim it must ask: (1) could the theorem itself be false? (2) is the prover *explaining away* a contradiction? — red-flag phrases: *"We interpret the problem as asking for…", "Up to equivalence…", "The natural reading suggests…"*; (3) did the prover find ONE solution or ALL solutions? On any hit it must emit `:verdict :challenge :reason "POSSIBLE FALSE THEOREM: …" :suggested-check "…"`.

**Prover forbidden-list (VII.2)** encodes the same failure modes at generation time (all → INVALID): hidden quantifiers, uncited external theorems (must use `:admitted` instead), "well known"/"standard", **implicit domain restriction** (`x²≥c` must yield both `x≥√c` and `x≤−√c`), **incomplete case enumeration** (min/max/unique ⇒ all branches), **unwarranted equivalence** ("WLOG"/"up to symmetry" needs proof). Plus an explicit **Optimization Protocol**: enumerate ALL critical points, evaluate objective at each, check boundaries, rule out the rest — with a required `:justification :exhaustive-case-analysis` node.

**How a detected flaw is actually encoded in the graph** (real example, `examples/brokenmath/aime_2025_20/graph.edn`): there is no special `:error` field. Falsification is expressed *in the graph's own vocabulary*:
- the QED node `:1-c00007` has `:status :rejected, :taint :tainted`;
- intermediate wrong calcs (`:1-c00005`, `:1-c00006`) are `:status :rejected :taint :tainted`;
- a **new verified claim node whose statement begins `"COUNTEREXAMPLE: …"`** carries the refutation: `:1-c00010 … "COUNTEREXAMPLE: The claimed identity is FALSE. … = 336° ≠ 300°" :status :verified :taint :clean`.
So "the proof is broken" = *rejected QED + a verified COUNTEREXAMPLE claim*, and taint automatically marks the poisoned subtree. `hmmt_feb_2025_7/REPORT.md` corroborates: graph v59, 29 nodes, 25 verified / 4 rejected ("the main claim and QED, plus incorrect intermediate calculations").

### 2.4 Formalizer → Lean, and the (informal) axioms gate

Formalizer (VII.6) emits a **compiling Lean skeleton** with `sorry` for hard steps, mapping taint → Lean comment: `:self-admitted → sorry -- ADMITTED`, `:tainted → sorry -- TAINTED: <reason>`. For **detected-false** theorems it emits a genuine **counterexample proof** discharged by `native_decide` — see `aime_2025_20/Counterexample.lean`: `theorem claimed_identity_false : weighted_sum arc_DE arc_HJ arc_FG ≠ 300 := by native_decide`. **`native_decide`/`decide` on concrete arithmetic is the cheap, reliable falsification back-end** and is worth wiring into our Lean-compile stage.

**Axioms gate:** Alethfeld tracks sorry-count and axiom-count but only **documentarily** — prose tables in `lean/API.md` (e.g. "Quantum Entropy Increase … ⚠️ 2 axioms remaining (`spectral_entropy_transform_axiom`, …)"; Halting "0 sorries, 0 axioms"). There is **no automated `#print axioms` gate**. The `.claude/commands/verify-proof.md` slash command tells the agent to "Check … Axiom usage (especially `Classical.*`)" and to count/locate `sorry`, but this is manual. → An *executable* axioms gate is a genuine gap we should close (see §6).

### 2.5 Other portable prompt/infra bits
- **Reference-Checker (VII.5):** web-search DOI/arXiv verification returning `:verified/:mismatch/:not-found/:metadata-only`, red-flagging preprints-cited-as-published and withdrawn papers. Directly reusable for our retrieve/citation stage.
- **Context management (§IV):** compressed graph view (collapse verified subtrees), delta reporting (`v23 → v24: + :2-… [proposed]; Δ :2-… proposed→verified; − :1-… archived`), token budget gating.
- **`.claude/commands/verify-proof.md`** is a compact, ready adversarial-verify prompt with a STRUCTURAL / SEMANTIC / MATHEMATICAL checklist and a fixed report format.
- **LaTeX-er (VII.7)** uses a fixed external `latex-template.tex` with `%%MARKER%%` placeholders — no free-form doc structure.
- **Run-artifact formats worth copying** (from examples): `gjwh-generalization/ORCHESTRATION_REPORT.md` (a structured run report: theorem, a "Subagents Spawned" table [Adviser-Audit, Adviser-Strategy, Prover, Verifier×6, Diagnosis, Counterexample…], graph stats, per-step verdict table, final `ESCALATED (potential false theorem)`); `gjwh-generalization/session-transcript.md` (a 9-phase narrated end-to-end trace: Init→Theorem-Audit→Strategy→Skeleton→Parallel-Verification→Gap-Diagnosis→Counterexample-Construction→Counterexample-Verification→Summary — the single best artifact for seeing the protocol in action); `qbf-rank1/lemmas/00-index.md` (Lemma-Decomposer output table with per-lemma **benefit scores**); `qbf-rank1/lemmas/L*/protocol-log.md` (per-lemma phase logs with the clojure `:request` inputs). These are ready templates for our own run reports/telemetry.
- **CLI hardening ethos (§IX):** exhaustively documents *non-existent* flags (`-o`, `--force`, `--dry-run`, `--json`) to stop the LLM inventing them; documents exit codes and common-error→solution table. Good pattern for any agent-facing CLI (cf. our CLI/TUI plans).

---

## 3. The per-example DATA SCHEMA (esp. brokenmath)

**The BrokenMath source problems** (`brokenmath/brokenmath_selected_10.json`) are a JSON array of 10 objects with schema: `original_problem` (the correct competition statement), **`problem`** (the *adversarially corrupted* statement fed to the system — diff the two to see the injected error), `problem_id` (e.g. `"matharena_hmmt/hmmt_feb_2025_7"`), `question_type: "proof"`, `is_adversarial: true`, `solution` (full official LaTeX solution with the true answer), `gold_answer: null`. This is our ground-truth key for scoring.

**Three EDN shapes coexist; do not conflate them:**

**(A) Persisted graph** (`graph.edn`, `*.edn`, `*-v4.edn`) — the canonical schema of §2.1: a top-level map `{:graph-id :version :theorem :nodes :symbols :external-refs :lemmas :obligations :archived-nodes :metadata}`, where `:nodes` is `{:<id> {…node…}}` keyed by id, edges via **`:dependencies` (a set)**. Confirmed in the wild in `aime_2025_20/graph.edn` and `determinant-rank1-perturbation-v4.edn`. Assumption nodes may carry an `:assumption-label :A2` and be referenced by short ids like `:A2`, `:D3`. `:metadata` carries `:proof-mode`, `:iteration-counts`, and `:context-budget {:max-tokens 100000 :current-estimate …}`.

**(B) Prover I/O delta / older "v2.0" flat schema** (orchestrator §VII.2 as transient Prover output; also *persisted* in older examples like `dobinski-formula.edn`): `{:meta {:orchestrator-version "2.0" :status :proven …} :theorem "…" :symbols […] :assumptions [{:id :A1 …}] :definitions [{:id :D1 …}] :steps [{:id :<1>1 :claim "LaTeX" :using [:D3 :D5] :justification :status :substeps [{:id :<2>1 …}]}]}`. Here edges are **`:using`**, hierarchy is **inline `:substeps`**, step ids use Lamport `:<1>1`/`:<2>7` notation, and there is **no `:taint`/`:provenance`/`:content-hash`**. The CLI translates this into shape (A). Our tooling should expect `:using`+`:substeps` from a model but store `:dependencies`+`:parent`.

**(C) Verification-log EDN** (e.g. `qbf-rank1/lemmas/L2/L2-verification-log.edn`) — a Verifier audit trail, not a graph: `{:verification-session … :rigor-setting :strictest :total-rounds 52 :final-status :all-verified :verification-results [{:node-id :1-step1c1 :verdict :accept :reason "…" :round 6 :strict-check {:quantifiers :explicit :indices :specified :dependencies :complete}} …] :summary {:total-nodes 43 :verified 43 :challenged-then-resolved 5 :admitted 0 :taint-status :clean}}`. A reusable structured record of the adversarial loop's outcome per node (matches `schema/verification.clj` `VerificationLog`). QBF graph nodes additionally carry a **`:lean4-ref {:module … :theorem … :api-ref …}`** cross-link tying an EDN node to its formalized Lean theorem — a nice EDN↔Lean traceability field.

**Example-tree conventions** (`examples/README.md` gives a full annotated index with node counts):
- Single-file proofs: `<name>.edn` + `<name>.tex` + `<name>.pdf` + `<name>.lean` (e.g. `dobinski-formula`, `determinant-rank1-perturbation`, often a `-v4.edn` schema variant).
- Multi-lemma proofs use **per-node directories**: `quantum-entropy-increase/lemma{1..6}.edn` + `lemmaN_nodes/nodeK.edn`, `theorem1_nodes/node1..13.edn`; `qbf-rank1/lemmas/L{1..5}/…` with `-expanded.edn`, `-graph.edn`, `-v4.edn`, `-verification-log.edn`, `protocol-log.md`.
- Node counts (from README verification table): QBF Rank-1 = 195 nodes/0 admitted; Halting = 18/0 (0 axioms); Quantum Entropy Increase 50+; Deligne 39 (4 verification rounds); Fib 45; Prop-infinite-Z 16; HMMT-2025-3 31 (1 admitted, theorem false).

**`brokenmath/` specifics:**
- `brokenmath_selected_10.json` — the source problems (subset of the INSAIT BrokenMath benchmark).
- Per-problem dir: always `graph.edn`; plus some of `proof.tex/.pdf`, `node-*.edn` (expansion nodes: `brumo_2025_5/node-2-001.edn…node-2-013.edn`, `node-claim2.edn`, `node-qed.edn`; `cmimc_2025_18/node-a1…d2, c3-sub1…`), `REPORT.md` (hmmt_feb_2025_7), `Counterexample.lean` (aime_2025_20), `lean/…lean` or `<Name>.lean`.
- **Flaw-encoding taxonomy** (5 distinct in-graph mechanisms, per-problem table below):
  - **(a) Rejected-QED + verified `COUNTEREXAMPLE:` node** — `aime_2025_20` (`:1-c00010` "COUNTEREXAMPLE … 336≠300"), `hmmt_feb_2025_7` ("…a+b+c<2", + REPORT.md).
  - **(b) Verified narrative verdict nodes** — `imosl_2025_6`: `"STRUCTURAL ISSUE IDENTIFIED: …counterexamples… a_n=n+1 … a_n=2^n"` and `"FINAL VERDICT: The theorem as originally stated is FALSE… A CORRECTED statement would be…"` (both `:status :verified`).
  - **(c) Admitted load-bearing false step** — `cmimc_2025_18`: `:2-c30003 {… "+220 additional forced rectangles … R≥2460", :justification :admitted :status :admitted :taint :self-admitted}`, re-listed in the graph's `:obligations`; QED verified-but-`:taint :tainted`.
  - **(d) Latent internal contradiction (numbers don't match)** — `brumo_2025_5`: skeleton `:1-claim3`="45" and QED `:1-qed001`="45×8=360" contradict the worked substep `:2-seq013`="90−60=30" (⇒240). Every node still `:status :proposed` — the flaw is latent in the arithmetic for the Verifier to catch, not pre-marked.
  - **(e) Verified-but-false (false positive)** — `hmmt_feb_2025_3`: ALL nodes `:verified` incl. QED "minimum is 576"; the missed `s<0` branch was caught only in Phase-2 Lean (`HMMT2025_3.lean`, 1 intentional `sorry`).

| Problem | Flaw location / mechanism |
|---|---|
| aime_2025_20 | node `COUNTEREXAMPLE 336≠300` (verified) + rejected QED; `Counterexample.lean` `native_decide` |
| hmmt_feb_2025_7 | 4 rejected nodes + `COUNTEREXAMPLE a+b+c<2`; `REPORT.md` |
| brumo_2025_5 | latent contradiction 45/360 (claim3/qed) vs 30 (seq013 substep) |
| brumo_2025_22 | 5 rejected nodes; "reinterpreted" 72→36 transpose pairs |
| cmimc_2025_18 | admitted/self-admitted `:2-c30003` (+220); in `:obligations` |
| imosl_2025_6 | verified `STRUCTURAL ISSUE` + `FINAL VERDICT … FALSE` nodes |
| imosl_2025_8 | many rejected construction attempts; existence admitted |
| hmmt_feb_2025_3 | all verified (false-positive); caught only in Lean |
| chinatst_2025_8 | admitted determinant-factorization step |
| rmm_2025_2 | admitted; impossibility not proven |
- **`brokenmath/README.md` is itself a mini-benchmark report**: a results table (problem, competition, error type, result, detection method) and a per-problem post-mortem. Error-type taxonomy: *negated conclusion, wrong inequality, wrong numerical value, existence-vs-uniqueness, possibility-vs-impossibility, wrong count, wrong minimum, reciprocal error*. Detection methods: *numerical computation, direct calculation, internal contradiction, explicit counterexample, Lean formalization (Phase 2)*.

---

## 4. What our earlier TARGETED pass MISSED

1. **The full v5.2 state machine (§VI)** — 30+ explicit `{:from :condition :action :next}` transitions incl. `theorem-audit` and `escalated`. Earlier we knew "there's an orchestrator loop"; we did not have the concrete transition table, iteration-limit budget, or escalation semantics.
2. **The executable graph core in Clojure** — `graph.clj` (taint, scope, topo-sort, independence, token-estimate) and `validators.clj` (acyclicity 3-coloring, scope/discharge/taint validators, `assert-valid-graph!` postconditions). This is real algorithmic content, not just a schema.
3. **Three-valued taint** (`:clean`/`:tainted`/`:self-admitted`) and **auto-obligation add/remove on status change** in `ops/update_status.clj`.
4. **The concrete flaw-encoding convention**: falsification lives in ordinary `:rejected`/`:verified` nodes + `"COUNTEREXAMPLE:"` statements — there is no dedicated error field. (Previously we may have assumed a structured error object.)
5. **The two distinct EDN shapes** (persisted `:dependencies` graph vs. Prover `:using`/`:substeps` delta) and the `-v4` schema-migration variants littered through examples.
6. **Anti-sycophancy is measured, not aspirational**: v5.1's Verifier/Prover/Adviser rules are each traceable to a specific BrokenMath failure (README results table maps problem → error-type → why detection succeeded/failed).
7. **The axioms gate is only documentary** (prose in `API.md`), not an automated check — a gap, not a feature.
8. **`native_decide`/`decide` as the falsification back-end** in Lean counterexample files.
9. **Breadth of the verified corpus**: QBF Rank-1 (195 nodes, 0 sorries), Quantum Entropy Increase (~3800 LOC Lean, 0 sorries), Halting (0 axioms), Kelly's Lemma, Dobinski, Divisor-Sum-9! — all fully machine-verified, usable as regression fixtures.
10. **Supporting infra**: `scripts/ansi-viz.clj` (terminal DAG renderer), standalone `scripts/validate-graph/`, `.claude/commands/*` slash commands, `.beads` issue tracking, and a real Clojure `test/` suite (property/mutation/concurrency) around the graph ops.

---

## 5. Test / benchmark value

**`brokenmath/` is a ready-made falsification/critic benchmark for Theoremata.** 10 curated competition problems, each a *subtly corrupted* true problem, with:
- ground-truth error type and the correct answer (in `README.md` + `brokenmath_selected_10.json`);
- a reference outcome (Alethfeld got 5/10 detected, 5/10 admitted, 0/10 falsely-verified after Phase-2 Lean) — a concrete **baseline to beat**;
- worked graphs showing *how* a detection is represented, usable to score our critic (did we produce a rejected-QED + counterexample? did we escalate?).

Suggested use: run our falsify-before-prove stage on these 10, scoring three outcomes {detected-false / admitted / falsely-verified} against the table, and specifically test the hard failure modes Alethfeld missed — impossibility proofs (imosl_2025_8, rmm_2025_2), plausible-adjustment errors (cmimc_2025_18), and **reinterpretation/rationalization** (brumo_2025_22, where the system found 72 but "explained away" the mismatch to match 36 — a critic-integrity test). The `native_decide` counterexamples (aime, divisor-sum, hmmt-3) are clean pass/fail Lean fixtures.

**Beyond brokenmath:** the 6 fully-verified proofs (0 sorries) are regression fixtures for a formalize→Lean-compile→axioms-gate pipeline. The `cantor-set-trick-question` and the divisor-sum "105→66" case are additional pre-flight-rejection tests. The Clojure `test/` suite (property/mutation tests on add-node/update-status/extract-lemma/taint) is a model for our own graph-op test coverage.

---

## 6. New vs. already-in-our-design

**Already central to Theoremata's design (Alethfeld = independent corroboration + concrete reference impl):**
- Graph-first / proof-DAG core, EDN-ish node schema, dependency edges → our proof-DAG core.
- Falsify-before-prove + adversarial critic → their theorem-audit + Verifier anti-sycophancy.
- Formalize → Lean compile → sorry/obligation tracking → their Formalizer + taint.
- Decompose into obligations → their lemma-decomposer with independence check + benefit score.
- Model-agnostic providers → they ship per-model prompt variants (Claude/Gemini/Codex) (coarser than our LiteLLM approach).
- Best-of-N formalize: **NOT** present in Alethfeld (single-shot Formalizer). Our best-of-N is a genuine advance.

**New / worth lifting into Theoremata:**
1. **Three-valued taint + auto-obligation lifecycle** on status change (`ops/update_status.clj`) — cleaner than boolean tainted.
2. **`compute-all-scopes` single-pass scope algebra** and **acyclicity-with-cycle-path** validator — ready algorithms.
3. **Explicit iteration-limit budget + escalate-to-human-with-context** as first-class loop control (avoids infinite spin; we should adopt the numeric budget + escalation payload).
4. **Theorem-Audit as a distinct pre-proof phase** with `:plausibility/:recommendation` and domain/counting-trap checklists — richer than a generic "sanity check."
5. **Anti-sycophancy prompt library** (Verifier's "explaining-away" red-flag phrases; Prover forbidden-list; Optimization Protocol requiring `:exhaustive-case-analysis`) — port near-verbatim into our critic/prover prompts.
6. **Counterexample-as-verified-node convention** + **`native_decide` Lean counterexample** back-end — a concrete, checkable falsification artifact.
7. **`benefit = 0.3·size + 0.3·isolation + 0.2·reuse + 0.2·depth` decomposition metric** — a decision rule for *when* to extract a lemma.
8. **Agent-facing CLI hardening** (document non-existent flags, exit-code table, error→solution table) — for our CLI/TUI.
9. **`brokenmath` benchmark harness** (§5) — adopt as a scored eval.

**Our design already improves on Alethfeld in:** best-of-N formalization, an *executable* axioms gate (Alethfeld's is prose-only — close this gap), true model-agnostic provider (LiteLLM) vs. hand-forked prompts, and (per README) the successor Vibefeld's event-sourced ledger / auto-numbering / built-in taint — several of which we can leapfrog directly rather than re-deriving.

---

### Key file references
- Harness brains: `orchestrator-prompt-v5_2-claude.md` (state machine §VI, subagent prompts §VII, CLI ref §IX), `orchestrator-prompt-v5.1-claude.md` (stable).
- Schema: `cli/src/alethfeld/schema/{node,graph,enums,verification,primitives}.clj`.
- Core logic: `cli/src/alethfeld/graph.clj` (taint/scope/topo/independence/tokens), `cli/src/alethfeld/validators.clj`, `cli/src/alethfeld/ops/update_status.clj`.
- Docs: `docs/architecture.md`, `docs/proof-format.md`, `.claude/commands/verify-proof.md`.
- Falsification corpus: `examples/brokenmath/README.md`, `examples/brokenmath/aime_2025_20/{graph.edn,Counterexample.lean}`, `examples/brokenmath/hmmt_feb_2025_7/REPORT.md`, `examples/brokenmath/brokenmath_selected_10.json`.
- Verified fixtures: `examples/README.md` (annotated index + status table), `lean/API.md`, `lean/AlethfeldLean/**`.
