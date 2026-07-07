# QED — Resource Mining (Full Pass)

Repo studied: `resources/QED-main/QED-main/` (note the doubled nesting).
Source: **QED — An Open-Source Multi-Agent System for Generating Mathematical Proofs on Open Problems** (An, Ye, Pan, Zhang; arXiv:2604.24021, repo `proofQED/QED`). This is the paper/repo our three-tier `REVISE_PROOF < REVISE_PLAN < REWRITE` retry design was taken from. This pass reads **all** code and prose in full (prior pass was a targeted skim).

---

## 1) What it is (scope, size, structure)

A LaTeX-problem-in → natural-language-proof-out multi-agent pipeline. ~19k lines total; the substance is ~4.3k lines of Python + ~2.5k lines of prompt markdown. **No agent framework** — every model call is a raw CLI subprocess (`claude -p`, `codex exec`, `gemini -p`) wrapped in `asyncio.run_in_executor`. Model-agnostic per-agent provider selection (not LiteLLM — direct CLI shell-out). **There is no Lean / formal component at all** — QED is entirely natural-language proof + LLM-judge verification. Its notable output: 5 expert-verified original research results (PDE, algebraic geometry, probability ×2, inverse problems).

Key files (all paths relative to `resources/QED-main/QED-main/`):

- `code/pipeline.py` (914 ln) — Stage 0/1/2 orchestrator, `TokenTracker`, `PipelineLogger`, provider option building, easy short-circuit.
- `code/decomposition_prover.py` (1991 ln) — **the retry state machine**; `DecompositionState`, resume detection, the six agent runners, the main loop with escalation. THE priority file.
- `code/model_runner.py` (751 ln) — Claude/Codex/Gemini async CLI wrappers, `ModelRunnerError` taxonomy, retry-with-backoff (Claude only), per-agent provider override resolution.
- `code/smoke_test.py` (596 ln) — pre-flight validation (prompt files exist, template placeholders render, CLI connectivity).
- `prompts/decomposition-prover/*.md` — decomposition, single_prover, proof_verify_structural, proof_verify_detailed, regulator, verdict_proof (+ `archive/` old step-wise prompts).
- `prompts/literature_survey.md`, `prompts/proof_effort_summary.md` — Stage 0 and Stage 2.
- `skill/super_math_skill.md` (286 ln) — a 38-principle proof-methodology system prompt.
- `verify/verify.py` (598 ln) + 4 prompts — **standalone** difficulty-adaptive verifier (separable product).
- `human_help/` — two (empty by default) live-editable steering files read every round.
- `config.yaml` — per-agent provider/model/effort matrix.
- `proved_statements/` — 5 case studies (problems, correct proofs, expert comments, cited theorems).
- `ui/` — Streamlit monitor (config panel, process manager, progress monitor) — low reuse value for us.

Output artifact tree (the on-disk DAG of the whole run) — `<output>/decomposition/attempt_N/revision_M/proof_K/` each holding `proof.md`, `structural_verification.md`, `detailed_verification.md`, `regulator_decision.md`; plus `decomposition/plan_history.md`, `decomposition/failure_analysis.md`, `decomposition/STATUS.md`, `TOKEN_USAGE.md` / `token_usage.json`.

---

## 2) Reusable ideas / patterns / code for Theoremata — THE priority

### 2.1 The three-tier retry state machine (verified, and deeper than we had)

The hierarchy is a **nested directory counter**, not just conceptual: `attempt` (REWRITE) → `revision` (REVISE_PLAN) → `proof` (REVISE_PROOF). `DecompositionState` (`decomposition_prover.py:284`) owns three integer counters + `attempt_history`, and the directory path *is* the state:

```python
def get_proof_dir(self):     # attempt_N/revision_M/proof_K
    return os.path.join(self.get_revision_dir(), f"proof_{self.proof}")
def new_proof(self):    self.proof += 1                         # REVISE_PROOF
def new_revision(self): self.revision += 1; self.proof = 1      # REVISE_PLAN
def new_attempt(self):  self.attempt += 1; self.revision = 1; self.proof = 1  # REWRITE
```

Limits live in `config.yaml` under `decomposition:` — **note the three different default sets** (a real inconsistency worth being aware of): `config.yaml` ships `max_proof_attempts: 8, max_revisions: 4, max_decompositions: 4`; `decomposition_prover.py:34 DEFAULT_CONFIG` = `3/2/3`; the README table claims `4/4/4`. The runtime value is whatever `config.yaml` says.

**Loop control** (`decomposition_prover.py:1635–1931`), quoted structure:
- Outer `while state.attempt <= max_decompositions:` → decompose (CREATE/REVISE/REWRITE).
- Inner `while state.proof <= max_proof_attempts:` → prove → structural verify → structural verdict → (if CONTINUE) regulator; (if DONE) detailed verify → final verdict → (if CONTINUE) regulator.
- Regulator decision routes with `continue` (REVISE_PROOF, stay in inner loop) vs `break` (REVISE_PLAN/REWRITE, back to outer loop).

**Key insight we MISSED — automatic escalation independent of the regulator.** When the inner loop exhausts `max_proof_attempts` without the regulator having chosen to escalate, the orchestrator escalates *on its own* and synthesizes guidance (not a stale REVISE_PROOF message) — `decomposition_prover.py:1885`:

```python
if state.proof > max_proof_attempts:
    if state.revision < max_revisions:
        state.new_revision(); resume_point = "decompose_revise"
        regulator_guidance = ("# Automatic Escalation to Plan Revision\n\n"
            f"All {max_proof_attempts} proof attempts ... failed verification. "
            "The repeated failures suggest structural issues with the decomposition plan ...")
    else:
        state.new_attempt(); resume_point = "decompose"
        regulator_guidance = ("# Automatic Escalation to Complete Rewrite\n\n"
            f"All {max_revisions} plan revisions have been exhausted ...")
```
So there are **two** escalation drivers: (a) the regulator's semantic decision, and (b) a mechanical budget-exhaustion escalation. Our design only modeled (a).

### 2.2 The Regulator prompt (`prompts/decomposition-prover/regulator.md`) — the selector, in full detail

This is the crown jewel to port. Concrete features beyond "pick one of three":

- **A `verification_phase` prior injected by the harness** (`run_regulator(..., verification_phase=)`): the prompt tells the regulator that a *structural* failure biases toward REVISE_PLAN and a *detailed* failure biases toward REVISE_PROOF — "The phase bias is a **prior, not a verdict**. The verification report is the authoritative source — if it tells a different story, follow the evidence." (`regulator.md:20–31`).
- **Explicit decision criteria** per tier with cost ordering: "REVISE_PROOF is cheapest: Try this first ... REWRITE is last resort" (`regulator.md:362–364`).
- **`plan_history.md` — a curated cross-attempt memory the regulator is the SOLE writer of.** On REVISE_PLAN/REWRITE the regulator MUST append a structured entry (strategy in one sentence, key step statements *verbatim*, diagnosis, "Do NOT try again", "May still be reusable", advisory next suggestion). The decomposer MUST read it before proposing a new plan. This is the mechanism that stops the system re-trying dead strategies — `regulator.md:135–172`, plumbed via `DecompositionState.ensure_plan_history()` / `get_plan_history_file()` (`decomposition_prover.py:329–358`). **We do not have this; it is the single most valuable idea to steal.**
- Four fully worked decision examples (REVISE_PROOF, two REVISE_PLAN, REWRITE) embedded in the prompt as few-shot.
- A **FINAL mode** that produces a `failure_analysis.md` for human review (blockers, strategies tried, what was NOT tried, recommendations, "Possible Issues with Problem Statement", "Literature Gaps").
- Decision parsing is defensive: `parse_regulator_decision()` scans for `DECISION: REVISE_PROOF|REVISE_PLAN|REWRITE`, maps legacy `REVISE`→`REVISE_PROOF`, defaults to the cheapest (`decomposition_prover.py:261`).

### 2.3 The Decomposer prompt (`decomposition.md`) — DAG-as-YAML plan schema (see §3)

Reusable specifics: CREATE/REVISE/REWRITE modes; a hard rule that **every step must be a quantitative statement** with BAD/GOOD examples ("The random variable X has a thin tail" → "For all t>0, E[e^{tX}] ≤ e^{t²σ²/2}"); mandatory `self_critique` block (plausibility, contradiction-vs-literature, difficulty, completeness); `is_key_step` flagging; a quality checklist ("No step is harder than the original problem", "proof_order is a valid topological sort").

### 2.4 The Single-Prover prompt (`single_prover.md`) — anti-hand-wave engineering

The whole prompt is built to stop the model dodging the hard step. Worth lifting verbatim for our prover: a "CRITICAL: Do NOT Shy Away from Difficulty" section enumerating avoidance patterns ("Writing 'clearly, X holds' ... **Prove it.**"), a "Do NOT Alter the Problem Statement" section (no added assumptions/weakened conclusion/changed quantifiers, copy problem verbatim), a personal `scratchpad.md` that "is never read by any downstream agent or verifier", and freeform shell/CAS tool use with a 3-minute kill rule and "write large results to files, print only a summary".

### 2.5 Two structured tags the verifier enforces (portable to any NL-proof system)

- **`<cite>type=…; label=…; title=…; authors=…; source_url=…; verifier_locator=…; statement_match=exact; statement=…; usage=…</cite>`** — every external result; the structural verifier independently fetches the URL and compares the statement word-by-word (Phase 3). "Citations are the #1 source of hallucinations. Check every single one."
- **`<key-original-step>…</key-original-step><heuristics>…</heuristics>`** — every nontrivial original step wrapped, followed by a heuristics blurb; the verifier flags *untagged* nontrivial steps (hiding weak arguments) AND *inflated* tags.

### 2.6 Model runner patterns (`model_runner.py`) — directly reusable engineering

- **`ModelRunnerError` taxonomy**: `subprocess_error | non_zero_exit | json_parse_error | empty_response`, each with `.full_details()` markdown for logging.
- **Claude gets retry-with-backoff** (`MAX_RETRIES=3`, `RETRY_BACKOFF=[30,60,120]s`) on subprocess error / non-zero exit / empty response; Codex and Gemini do not.
- **Provider cross-contamination guard**: before a Claude call it strips `CLAUDE_CODE_USE_BEDROCK, ANTHROPIC_API_KEY, AWS_PROFILE, ANTHROPIC_MODEL` from env then re-adds only configured ones (`model_runner.py:117`).
- **Codex non-zero-exit is treated as a warning if a response parsed** (Codex exits 1 on a stdin warning even on success) — `model_runner.py:365`.
- **Per-agent override resolution** (`resolve_agent_provider_config`): each agent role is `{provider, model?, reasoning_effort?/thinking_level?}`; the provider name selects the global section, other keys overlay it. For Claude the per-agent `model` slots into whichever auth block (`subscription|api_key|bedrock`) `claude.provider` selects, while auth mode stays global. This is a clean pattern for our own model-agnostic layer.
- CLI invocation strings (useful reference): Claude `claude -p --output-format json --dangerously-skip-permissions --model M [--append-system-prompt S] PROMPT`; Codex `codex --search -m M -c model_reasoning_effort="xhigh" exec --json --dangerously-bypass-approvals-and-sandbox -C DIR PROMPT`; Gemini `gemini -m M --approval-mode yolo -o json -p PROMPT` (thinking config injected via a temp `GEMINI_CLI_HOME/.gemini/settings.json`).

### 2.7 Robustness/orchestration patterns worth copying

- **Tool-write-or-fallback**: agents are told to write output files via tool calls; the harness reads the file, and if empty falls back to the returned text (`_fallback_save_response` in pipeline.py; `if not read_file(output_file): write_file(output_file, response)` throughout). Plus `_check_expected_files()` raises a FATAL if a required artifact is missing.
- **Every agent gets an `error_<name>.md`** it must always create (empty if no error) — makes missing-artifact detection unambiguous.
- **`TokenTracker`** (pipeline.py:274) persists `TOKEN_USAGE.md` + `token_usage.json` after *every* call, with per-provider subtotals — good template for our cost telemetry.
- **`human_help/` live steering**: `additional_prove_human_help_global.md` (hints, treated as hard requirements by decomposer/prover) and `additional_verify_rule_global.md` (hard rules enforced as Phase 5). Copied into the output dir by `run.sh` with `cp -n` so a UI edit mid-run isn't clobbered.

---

## 3) Blueprint / DAG / schema / eval format

**The proof plan IS a DAG, serialized as YAML** (`decomposition.md:166–235`). Nodes = `sources` (literature leaves) + `steps` (intermediate claims) + one `target` (GOAL); edges = each node's `inputs:` list; `proof_order:` is the topological sort. Schema:

```yaml
metadata: {problem_id, mode, attempt, revision, timestamp}
sources:
  - id: S1
    type: literature
    statement: |  ...
    citation: |  <cite>type=theorem; label=...; source_url=...; statement_match=exact; ...</cite>
steps:
  - id: STEP2
    statement: |  [precise QUANTITATIVE statement]
    inputs: [S1, STEP1]          # DAG edges
    difficulty: hard             # easy|medium|hard
    is_key_step: true
    rationale: |  ...
    strategy_hint: |  ...
    hueristics: |  [why this key step could work]   # (sic — misspelled in the template)
target:
  id: GOAL
  statement: |  [copy problem EXACTLY]
  inputs: [STEP2, STEP3]
proof_order: [STEP1, STEP2, STEP3, GOAL]
key_steps: [STEP2]
self_critique:
  plausibility_issues: []
  contradiction_checks: [...]
  refinements_made: [...]
  difficulty_assessment: |  ...
```

The DAG is parsed leniently: `parse_decomposition()` strips ```` ```yaml ```` fences then `yaml.safe_load`, with fallback to parsing the raw response if the agent didn't write the file.

**Eval / verification format** — a two-phase, gated LLM-judge (no Lean, no `#print axioms` — this is the divergence from Theoremata's design):
- **Structural (Phases 1–5)** (`proof_verify_structural.md`): (1) Problem-Statement Integrity (word-by-word vs original — catches weakened quantifiers/added hypotheses/special-casing); (2) Completeness & Originality (all sub-questions addressed; genuine proof work not just reference-listing; **any acknowledged hole = FAIL**); (3) Citation Verification (fetch URL, match statement verbatim, any FAIL ⇒ phase FAIL); (4) Decomposition Adherence (coverage table per STEP; key-step rigor; declared vs undeclared deviations; **4f "Refuted Plan Steps"** — the one place the structural verifier IS required to do math: independently judge a prover's counterexample/no-go claim against a plan step, verdict VERIFIED/REFUTED/UNSUPPORTED/UNCERTAIN, to feed the regulator's REVISE_PLAN-vs-REWRITE choice); (5) Additional Rules from `human_help`.
- **Detailed (Phase 6)** (`proof_verify_detailed.md`) runs ONLY if structural passed: 6a step-by-step (with mandated computational cross-check via SymPy/Z3), 6b key-step analysis (tag presence + rigor + flag untagged nontrivial steps), 6c dependency-chain, 6d coverage, 6e assembly coherence.
- **Verdict agent** (`verdict_proof.md`): a cheap separate agent that reads the report(s) and emits exactly one word `DONE` / `CONTINUE`. "Be strict and conservative — when in doubt, reply CONTINUE." The orchestrator keys off `"OVERALL VERDICT: PASS"` in the report text and the verdict word.

Verifier persona instruction, reused across both: *"Your personality is very mean and critical."* + "A proof that is 'almost right' is still FAIL."

---

## 4) What our earlier targeted pass MISSED

1. **`plan_history.md`** — the regulator-owned, append-only cross-attempt strategy memory (see §2.2). Central to avoiding repeated dead-ends; we had no equivalent.
2. **Automatic (mechanical) escalation** on budget exhaustion, separate from the regulator's semantic decision, with *synthetic* escalation guidance (§2.1, `decomposition_prover.py:1885–1927`).
3. **`verification_phase` prior** fed into the regulator (structural→bias REVISE_PLAN, detailed→bias REVISE_PROOF) as a "prior not a verdict" (§2.2).
4. **Phase 4f "Refuted Plan Steps"** meta-check — the structural verifier independently adjudicates prover complaints that a plan step is false/impossible/circular/too-weak, and this is what lets the regulator choose REVISE_PLAN vs REWRITE intelligently.
5. **Full resume/checkpoint state machine** (`detect_decomposition_resume`, `decomposition_prover.py:521`): scans the attempt/revision/proof tree, reads each proof dir's report/regulator files, and computes a precise `resume_point` ∈ {fresh, decompose, decompose_revise, prove, verify_structural, verify_detailed, regulator, done}. The whole pipeline is crash-resumable purely from on-disk artifacts — no external state store.
6. **The `archive/` superseded architecture** — QED *used to be* per-step: `step_prover.md` (prove one step, "NEVER give up"), `step_verifier.md` (verify one step), `proof_aggregator.md` (stitch step proofs into one document). This was replaced by the current **single monolithic prover** that writes the whole proof from the plan. Signal for us: they tried granular per-node prove/verify and abandoned it for a whole-proof prover + whole-proof verifier. Worth weighing against our proof-DAG-node design.
7. **`skill/super_math_skill.md`** — a 38-principle proof-methodology system prompt (embrace difficulty; concrete-before-abstract; work backward; unfold definitions; epsilon management; witness construction; "if a hard problem solves itself effortlessly, suspect an error"; compute-before-you-prove with CAS). Applied as `--append-system-prompt` to the literature-survey agent (and reusable for any prover). High-quality, directly liftable.
8. **Easy short-circuit** in Stage 0: the literature-survey agent classifies Easy/Medium/Hard and, if *Easy*, writes `proof.md` directly and the pipeline exits (skips Stages 1–2). Difficulty is parsed from `## Classification: …` in `difficulty_evaluation.md`.
9. **Standalone verifier** (`verify/verify.py`) as a **separable difficulty-adaptive product**: judge → (Easy: 1-agent full report) or (Hard: structural gate → detailed), plus a `--problem-only` mode that reviews a problem statement for well-definedness (well-defined/consistent/clear/complete/sound). Cleaner, dependency-light reference than the in-pipeline verifier.
10. **`smoke_test.py`** — pre-flight that renders every prompt template with dummy values to catch `str.format` placeholder breakage before spending money, and checks CLI connectivity.
11. **Config default inconsistency** across README/`config.yaml`/`DEFAULT_CONFIG` (§2.1) — a trap if we copy numbers.
12. **Provider env-scrubbing** and **Codex-warning-exit tolerance** engineering details (§2.6).
13. **`hueristics` misspelling** is baked into the decomposer template and mirrored (correctly spelled `<heuristics>`) in the verifier — if we port the schema verbatim, keep the keys consistent.

---

## 5) Test / benchmark value

- **`proved_statements/` — 5 expert-verified case studies** with problem statements, full "correct" proofs, and expert commentary. Directly usable as regression/eval targets for Theoremata:
  - `analysis-May-19-2026/` — advection-diffusion lower bounds (3 sub-problems), arXiv:2605.20623; includes `cited-theorems/` (3 files, the external results the proof leaned on — good for testing citation-faithfulness checks).
  - `prob-May-15-2026/` — lamplighter return-probability and total-variation asymptotics (2 problems, each with a 700–1073-line correct proof). Sample problem-1 (verbatim): return-probability asymptotic on $\mathbb{Z}_2\wr T_d$ with an explicit instruction "do not cite the paper 'The Anderson model on the Bethe lattice…'" — i.e. embeds a `human_help`-style constraint into the problem.
  - `algebraicgeometry-May-17-2026/` — integral invariant cycle theorem for $H^1$; has `original-expert-comment/` (`.tex` + `.bib`) — an expert's line-by-line critique of the AI proof, useful as a gold-standard verifier calibration set.
  - `analysis-Apr-24-2026/` — 4 problems, 2 with correct proofs.
  - `pde-Mar-23-2026/` — README only (Carleman weight construction; proof withheld pending arXiv).
- **`standalone_verifier/problem.txt` + `proof.txt`** — a tiny default problem/proof pair for exercising the verifier.
- The correct-proof `.md` files show the enforced output structure (`### STEPn`, **Claim/Proof/Dependencies**, `<cite>`, `<key-original-step>`) applied to real research proofs — good fixtures for testing our own structural verifier.

These are natural-language (LaTeX) proofs, not Lean — so they test our *NL* layer and our falsify/retrieve/verify judgment, not the Lean-compile gate.

---

## 6) New vs. already-in-our-design

**Already in our design (QED corroborates):**
- Three-tier retry `REVISE_PROOF < REVISE_PLAN < REWRITE` — confirmed; QED is the origin, and our conceptual model matches. The cost-ordering "try cheapest first, escalate" is exactly ours.
- Proof-DAG core — QED's YAML plan is a sources→steps→GOAL DAG with `inputs` edges + topological `proof_order`; validates our proof-DAG-first stance.
- Falsify-before-prove — QED's decomposer `self_critique` (plausibility + contradiction-vs-literature) and the prover's "try to disprove it" (skill principle 11) are a lightweight version; our dedicated executable falsifier is stronger.
- Retrieve → formalize → verify staging — QED's Stage 0 survey → decompose → prove → verify mirrors ours minus the formal layer.
- Best-of-N — partially: QED does sequential retries, not parallel N; it has no explicit best-of-N selector.
- Model-agnostic providers — QED does per-agent provider/model via CLI shell-out; we do LiteLLM. Same intent, different transport.

**New / not yet in our design (candidate adoptions, priority order):**
1. **`plan_history.md` regulator-owned strategy memory** — highest value; prevents dead-strategy loops. Adopt.
2. **`verification_phase` prior into the selector** — cheap, improves REVISE_PLAN vs REVISE_PROOF routing.
3. **Dual escalation (semantic regulator + mechanical budget-exhaustion)** with synthetic guidance.
4. **Phase-4f refuted-plan-step meta-check** feeding the selector — a principled REVISE_PLAN↔REWRITE discriminator.
5. **Structured `<cite>` schema with independent URL/statement re-verification** and **`<key-original-step>`/untagged-hard-step flagging** — portable to our NL layer regardless of Lean.
6. **Two-phase gated verify (cheap structural gate before expensive step-by-step)** + a one-word `DONE/CONTINUE` verdict agent — a cost-saving pattern; complements (not replaces) our Lean `#print axioms` gate.
7. **Crash-resume purely from on-disk artifact tree** — resume_point inference; good for our long-running loops.
8. **`super_math_skill.md`** as a reusable prover system prompt.
9. **Difficulty triage + Easy short-circuit** to avoid running the full loop on trivial problems.
10. **`human_help/` live-editable steering files** read each round (prover hints + hard verifier rules).
11. **Standalone difficulty-adaptive verifier + problem-only well-definedness review** as a separable tool.

**Divergences to keep in mind:** QED has **no formal/Lean component and no MCTS/evolution loop** — verification is entirely LLM-judge (with mandated CAS spot-checks), and search is sequential retry, not tree search. Our Lean-compile + `#print axioms` + LeanParanoia gate and our MCTS/evolution loop are strict supersets on the rigor and search axes; QED's value to us is concentrated in the **selector/regulator, plan-history memory, plan-DAG schema, structured tags, gated LLM verifier, and prompt craft**, not its search or verification backbone.
