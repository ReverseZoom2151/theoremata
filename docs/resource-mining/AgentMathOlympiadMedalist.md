# Resource Mining: AgentMathOlympiadMedalist

Full-pass study of `resources/AgentMathOlympiadMedalist-main/AgentMathOlympiadMedalist-main`
(note the doubly-nested directory: the download unzipped a `-main` inside a `-main`).

Mined for the **Theoremata** project (graph-first agentic math harness). This is the *full* read; a prior
targeted skim is superseded by this pass.

---

## 1) What it is (scope, size, structure)

A small (~800 LOC of Python plus 9 prompt files) open-source reimplementation of the pipeline from the arXiv
paper **"Gemini 2.5 Pro Capable of Winning Gold at IMO 2025"** (Huang & Yang, arXiv:2507.15855). The README
(`README.md:12-16`) states it directly adapts that paper's self-verification loop, which reportedly solved 5/6
IMO 2025 problems. **The "medal" claim is the paper's, not this repo's** — this repo is a faithful but thin
harness around Google Gemini 2.5 Pro; it has no independent benchmark results committed.

Structure (only substantive files listed; `.git`, `uv.lock`, build artifacts skipped):

- `app/imo_client.py` (403 lines) — **THE orchestration loop.** A standalone A2A client that drives the entire
  multi-stage pipeline. This is the heart of the repo.
- `app/agent.py` (175 lines) — the LangGraph ReAct agent wrapper (`MathOlympiadAgent`) + a stray access-test class.
- `app/agent_executor.py` (91 lines) — A2A `AgentExecutor` adapter (streaming task states → artifacts).
- `app/__main__.py` (105 lines) — A2A Starlette server bootstrap (agent card, skills, uvicorn on :10000).
- `prompts/*.md` (9 files) — **the real IP.** Verbatim prompt templates for each pipeline stage.
- `imo_problems/*.md` (7 files) — IMO 2025 problem statements 1–6 + an empty `_test`. Tiny (2–6 lines each).
- `README.md` — architecture overview + mermaid diagram.
- `Containerfile`, `pyproject.toml`, `.python-version` (3.12), `.gitignore`.

**No data/results directory exists.** Nothing to sample beyond the 7 problem statements (which are just LaTeX
problem prose — e.g. `imo_problems/imo_2025_problem_1.md` is the "sunny lines" combinatorics problem, ending with
the injected hint "Solve the problem by induction."). Runtime output goes to timestamped
`imo_client_output_YYYYMMDD_HHMMSS.txt` files (git-ignored), none present.

Deps (`pyproject.toml`): `a2a-sdk==0.2.16`, `langgraph`, `langchain-google-genai`, `langchain-openai`. No Lean,
no MCTS, no retrieval, no vector store, no test suite despite README claiming `uv run pytest`.

---

## 2) Reusable ideas / patterns / code for Theoremata — PRIORITY

The whole value of this repo is the **verify-driven refinement loop** and its **prompt templates**. It maps
cleanly onto Theoremata's "falsify-before-prove → ... → hardening" spine, but as an LLM-only critic loop
(no Lean, no executable check).

### 2a) The agent loop (the core find) — `app/imo_client.py`

Five distinct LLM roles, each a separate prompt + call, orchestrated by a plain Python `while` loop (NOT
LangGraph — LangGraph is only used inside `agent.py` as a trivial single-node ReAct wrapper; the actual control
flow is hand-rolled in the client). Stages:

1. **Initial solution** (`core_instructions` prompt + problem) — `imo_client.py:271-282`
2. **Self-improvement** (one-shot critique+rewrite of the initial solution) — `imo_client.py:285-295`
3. **Verification** (grader produces a detailed bug log) — `imo_client.py:308-322`
4. **Binary gate** (reduce the verification log to a single `0`/`1`) — `imo_client.py:324-336`
5. **Bug-report review** (a critic of the critic — prune false-positive bugs) — `imo_client.py:348-363`
6. **Solution correction** (apply the *pruned* bug list) — `imo_client.py:365-379`

**Acceptance criterion (the key mechanism), `imo_client.py:302-346`:**

```python
i = 0
consecutive_positive_verifications = 0
is_problem_solved = False
while i < 10:
    # ... verification -> verification_binary ...
    if verification_binary_result == '1':
        consecutive_positive_verifications += 1
    else:
        consecutive_positive_verifications = 0
    if (consecutive_positive_verifications == 5):
        is_problem_solved = True
        break
    # ... bug_report_review -> solution_correction (updates solution) ...
    i += 1
```

Require **5 consecutive** clean verifications, max **10** iterations, counter resets on any failure. The README
(`README.md:80`) explains the rationale: this hedges against an *imperfect verifier* — a stochastic critic that
sometimes false-negatives — by demanding a run of agreements rather than a single pass. **This is a directly
portable, cheap robustness trick for Theoremata's axioms/verification gate: treat the LLM verifier as noisy and
require k-consecutive passes rather than one.**

**Note a real bug worth not copying:** at `imo_client.py:367` the correction step feeds `verification_response`
(the *bug log*) as "Current Solution" instead of the actual improved solution — the solution text and its
critique get conflated in the correction prompt. The loop also never re-runs self-improvement per the README's
diagram; and `improved_solution` is never reassigned inside the loop, so verification always re-checks the
*same* post-self-improvement solution while corrections are printed but not fed back. If porting the *idea*,
fix the state threading (thread the corrected solution back into the next verification).

### 2b) The prompt templates (verbatim, high-value) — `prompts/`

These are cleanly separated "start/end" bracketing files concatenated around problem/solution text. The
strongest reusable assets:

- **`core_instructions.md`** — the solver system prompt. Notable design choices to steal:
  - "**Rigor is Paramount**": *"A correct final answer derived from flawed or incomplete reasoning is considered
    a failure."*
  - "**Honesty About Completeness**": forbids guessing; instructs the model to instead report *significant
    partial results* (a proven lemma, a resolved case, a proven bound without achievability). This is a good
    anti-hallucination lever and maps to Theoremata's proof-DAG (partial results = subgoal nodes).
  - Mandated **output schema**: `1. Summary (a. Verdict, b. Method Sketch incl. full precise lemma statements)`
    then `2. Detailed Solution` containing *only* the clean proof (no scratch work / failed attempts). Enforced
    "Self-Correction Instruction" to re-read before finalizing.

- **`verification_start.md`** — the grader prompt. The most reusable prompt in the repo. Key mechanics:
  - "act as a **verifier**, NOT a solver ... Do NOT attempt to correct the errors or fill the gaps."
  - **Two-class taxonomy** with distinct downstream procedures:
    - **Critical Error** (breaks logical chain — logical fallacy or factual/calc error): stop following that
      line, but *scan independent parts* (e.g. other cases) and still verify them.
    - **Justification Gap** (conclusion likely right, argument insufficient): *assume the conclusion true* and
      continue verifying the rest.
  - Required output: a `Summary` (Final Verdict + bulleted `List of Findings`, each with a **Location** = direct
    quote and an **Issue** = description + classification) then a step-by-step `Detailed Verification Log` that
    *quotes* the text it critiques. It even ships a one-shot example of the summary format.
  - **This taxonomy (Critical Error vs Justification Gap, with "assume-and-continue" for gaps) is the single
    most portable idea for Theoremata's critique/self-verification stage** — it lets one verification pass find
    multiple independent issues instead of halting at the first.

- **`verification_binary.md`** — reduces a verbose log to a machine-parseable gate: *"Respond with only a single
  number: 0 if ... contains any Critical Errors or Major Justification Gaps; 1 otherwise."* Clean pattern for
  turning an LLM's prose judgment into a loop-control boolean (Theoremata gate signal).

- **`bug_report_review_start/end.md`** — a **meta-critic** that reviews the verifier's own findings and *removes
  false positives* before they drive corrections, with a brief explanation per removal. This "critique the
  critique" layer is a nice de-noising step directly relevant to reducing wasted rewrite cycles.

- **`solution_correction_start/end.md`** — applies only the *retained* bugs; if the solver disagrees with a bug,
  it must *clarify the step* rather than silently ignore, and list "Disputed Issues" with reasoning. Preserves
  the structured output format.

- **`self_improvement.md`** — a single-pass "review for consistency/completeness/rigor, then rewrite keeping the
  same structure" primer, run once before the verification loop.

### 2c) Model config / provider-agnosticism — `app/agent.py:90-114`

Model-source switch via env var, echoing Theoremata's "model-agnostic LiteLLM provider" goal (here done with
LangChain instead of LiteLLM):

```python
model_source = os.getenv('model_source', 'google')
if model_source == 'google':
    self.model = ChatGoogleGenerativeAI(model="gemini-2.5-pro", max_tokens=65000, temperature=0.1)
else:
    self.model = ChatOpenAI(model=os.getenv('TOOL_LLM_NAME'), openai_api_key=os.getenv('API_KEY','EMPTY'),
                            openai_api_base=os.getenv('TOOL_LLM_URL'), temperature=0)
```

Deliberate low temperature (0.1 / 0), very high `max_tokens` (65k) — README (`README.md:176`) ties this to
"Sections 3.1 & 3.2 of the paper (low temperature, high max tokens)." Long-form rigorous proofs need the token
budget; determinism favored over diversity. (Contrast: Theoremata's best-of-N wants *higher* temp for sampling
diversity — this repo does NOT do best-of-N sampling within an attempt; see §4.)

### 2d) Tool use / search / voting — essentially absent

- **No tools.** `self.tools = []` (`agent.py:106`); README "Limitations" confirms "No external tools"
  (`README.md:199`). No Lean, no calculator, no retrieval.
- **No MCTS, no tree search.** Control flow is a linear refinement loop.
- **No best-of-N / selector / voting.** The README diagram mentions "Restart with New Sample" on max-attempts
  (`README.md:43`), but **that branch is not implemented** — on failure the loop just prints "FAILED" and exits
  (`imo_client.py:386-389`). So the multi-sample selection Theoremata wants is aspirational here, not present.
- The A2A protocol (`__main__.py`, `agent_executor.py`) is scaffolding for *potential* multi-agent setups but is
  used only as a local client↔server transport for a single agent. Overkill for our purposes; not worth porting.

---

## 3) Data / schema / eval format

- **Problem format:** plain Markdown, one problem per file, raw LaTeX-ish prose. No metadata, no answer key, no
  tags/difficulty. E.g. `imo_problems/imo_2025_problem_3.md` is the "bonza function" problem; README says the
  expected answer is `c = 4` (`README.md:184-192`) but no machine-checkable answer field exists.
- **Prompt-assembly "schema":** stages are built by f-string concatenation with literal ASCII-banner delimiters,
  e.g. `imo_client.py:309`:
  `f"{verification_start}\n\n{problem}\n\n===...===\n######### Solution #########\n\n{improved_solution}\n\n{verification_end}"`.
  Delimiters like `######### X #########` and long `===` rules are used consistently to fence sections inside a
  single user turn.
- **Response extraction:** A2A responses are dug out positionally:
  `response['result']['artifacts'][0]['parts'][0]['text']` (`imo_client.py:280,321,335`). Brittle; not a schema
  we'd adopt.
- **Eval signal:** the only "eval" is the binary verifier's `0`/`1` and the 5-consecutive-pass rule. No scoring
  rubric, no dataset harness, no pass@k, despite README claiming a pytest suite (there is none).

---

## 4) What our earlier targeted pass MISSED

- **The acceptance mechanism is the whole game.** 5-consecutive-clean-verifications with reset-on-failure
  (`imo_client.py:339-346`) as an explicit hedge against a noisy verifier (`README.md:80`). Cheap, portable,
  and not something a skim would surface as *the* design principle.
- **The two-tier "critic-of-critic" layer** (`bug_report_review_*`) that prunes false-positive bug reports
  before correction — an extra de-noising stage beyond plain verify→fix.
- **The Critical-Error vs Justification-Gap taxonomy with divergent procedures** ("stop this line but check
  independent parts" vs "assume true and continue") in `verification_start.md` — the most reusable prompt logic.
- **Gap between README/mermaid and actual code:** "Restart with New Sample," per-iteration re-improvement, and
  pytest are advertised but **not implemented**. The loop is simpler (and buggier) than the diagram implies.
- **A latent state-threading bug** (`imo_client.py:367` feeds the bug log as "Current Solution";
  `improved_solution` never updated in-loop). Important to know so we port the *intent*, not the code.
- **It is single-sample, tool-free, Lean-free.** No search, no voting, no formalization — much thinner than the
  "MCTS + best-of-N + Lean gate" framing suggested for this repo. The medal pedigree is entirely the upstream
  paper's Gemini pipeline, not code here.

---

## 5) Test / benchmark value

- **Problem set:** the 6 IMO 2025 statements in `imo_problems/` are a small, clean, ready-to-use eval set (with
  known answers documented in prose, e.g. P1 sunny-lines, P3 `c=4`). Useful as a smoke-test fixture for
  Theoremata's solver+verifier, though we'd want to add machine-checkable answer keys and more problems.
- **No committed results, no test suite, no CI.** Zero regression/benchmark value as-is; `uv run pytest` finds
  nothing.
- **Reusable as a prompt-ablation baseline:** the exact prompt wording is a known-good reference for a
  Gemini-based rigor-first solver+grader, worth A/B-ing against Theoremata's own prompts.

---

## 6) New vs. already-in-our-design

**Already in Theoremata's design (this repo confirms/validates, adds little new code):**
- Falsify/verify-before-accept loop; separating solver from verifier.
- Model-agnostic provider switch (they use a LangChain env-var switch; we use LiteLLM — same idea).
- Structured output contract (Verdict + Method Sketch + Detailed Solution).
- Iterate-to-convergence with a max-iteration cap.

**New / worth adopting:**
1. **k-consecutive-pass acceptance (k=5) with reset-on-failure** as an explicit noisy-verifier hedge — add this
   to Theoremata's Lean/axioms gate *and* to any LLM-critic gate. Cheap, high-leverage.
2. **Critical-Error vs Justification-Gap taxonomy** with "assume-the-gap-and-continue" so one verification pass
   yields *all* independent findings instead of halting at the first — port into the critique stage.
3. **Meta-critic / bug-report-review layer** that prunes false-positive findings before triggering rewrites —
   reduces wasted formalize/rewrite cycles.
4. **Binary-reducer prompt** pattern: a dedicated tiny call that turns a verbose critique into a single `0/1`
   gate token — clean control-flow signal, easy to log/vote on.
5. **"Report significant partial results, never guess"** solver instruction — maps naturally onto emitting
   partial proof-DAG nodes (proven lemmas / resolved cases / one-sided bounds) rather than a fake full proof.

**Explicitly NOT here (Theoremata must supply these itself):** Lean compilation + axioms gate, retrieval,
best-of-N sampling with a selector, MCTS/LLM-prior search, loop-detection guardrails, worktree/DAG state. This
repo is a *linear, LLM-only, single-sample* critic loop — a strong prompt/loop reference, not an architecture to
adopt wholesale.

### Key file references
- Orchestration loop + acceptance rule: `app/imo_client.py:266-389`
- Verifier taxonomy prompt: `prompts/verification_start.md`
- Binary gate prompt: `prompts/verification_binary.md`
- Meta-critic prompt: `prompts/bug_report_review_start.md` / `bug_report_review_end.md`
- Solver rigor/partial-results prompt: `prompts/core_instructions.md`
- Provider switch + model config: `app/agent.py:90-114`
- Source paper: arXiv:2507.15855 (README.md:12-16)
