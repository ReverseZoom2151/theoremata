# Resource Mining: MathCode (`resources/mathcode-main`)

Full-pass study for the Theoremata project. Every text file in the repo was read
in full; the single binary (`Demo.png`, 2.4 MB) was skipped as an image. This
supersedes the earlier targeted skim.

**Bottom line up front:** This repo is NOT the "MathCode" code-augmented math
reasoning research codebase we assumed. It is a **release/bootstrap wrapper** for
a *closed-source binary product* ("MathCode: A Frontier Mathematical Coding
Agent"), whose actual engine (`./mathcode`, `./mathcode-webui`) is downloaded
from GitHub Releases by `setup.sh` and is **not present** in the checkout. The
proving pipeline is explicitly "based on the AUTOLEAN project"
(`README.md:975`, `index.html:252`) — i.e. a Lean-4 *formalize-then-prove*
agent, essentially the same shape as Theoremata, **not** a code-execution
reasoning loop. The premise "model writes+runs code to reason" does not describe
this artifact. What is genuinely minable is small but real: a clean
tool/plugin interface, a robust Lean source-masking utility, an axiom/sorry
gate, and a fully-documented multi-stage pipeline architecture (in `.env.example`
and `README.md`) that closely parallels — and in places extends — our own design.

---

## 1) What it is (scope, size, structure)

- **Repo type:** GitHub Pages bootstrap repo (`.nojekyll`, `index.html` landing
  page) + release installer. `RELEASE_REPO="math-ai-org/mathcode"`,
  `RELEASE_TAG="v0.2.0"` (`setup.sh:1572-1573`).
- **Size:** 22 files, ~2.5 MB (2.4 MB is `Demo.png`). All source is a few small
  shell scripts, five Python tools, README/env prose, and a stub Lean workspace.
- **The engine is absent.** `setup.sh` downloads
  `mathcode-vX.Y.Z-<os>-<arch>.tar.gz` and restores `./mathcode`,
  `./mathcode-webui`, `vendor/ripgrep/` (`setup.sh:2560-2686`). The product is a
  compiled TypeScript/Node CLI (a Claude-Code-style terminal agent; the README's
  Session/Compaction/Tasks prose at `README.md:625-685` is lifted from a
  Claude-Code-like harness). We only see its *extension points* and *config*.
- **Platform:** macOS arm64 / Linux x86_64 only (`README.md:532`). Bundles its
  own Lean via a bundle-local `.local/elan` (`setup.sh:2720-2742`) and a Mathlib
  cache (~8 GB, `setup.sh:1577`).
- **Directory map:**
  - `tools/` — five Python analysis tools + a shared helper (the real code).
  - `skills/`, `plugins/` — empty (`.gitkeep`) extension dirs with READMEs
    describing the plugin/skill contract.
  - `lean-workspace/` — a stub Lake project: `import Mathlib`, one trivial def
    (`MathCodeLean.lean:1-7`), a `lakefile.toml` requiring mathlib
    (`lakefile.toml:320-347`), toolchain `leanprover/lean4:v4.30.0-rc2`.
  - `bin/lean-check` — a shell shim that compile-checks one `.lean` file.
  - `run`, `setup.sh` — launcher + installer.
  - `.env.example` — 216 lines; the single richest architecture document here.

---

## 2) Reusable ideas / patterns / code for Theoremata (THE priority)

**Important caveat:** the code-generation-interleaved-with-reasoning loop, the
execution/sandbox, and the prover/planner *prompt templates* live in the closed
binary and are **not in this repo**. There are zero LLM prompt strings, zero
tool-calling agent loop code, and zero sandbox code in the checkout. What we can
mine is the *tool interface contract*, a few genuinely reusable utilities, and
the *documented* pipeline shape. Below, everything I quote is real code/config
present in the repo.

### 2a) The tool-plugin interface (directly portable pattern)

MathCode auto-discovers Python tools dropped in `tools/` via **YAML frontmatter
embedded in a `#`-comment header**, and each tool speaks **JSON on stdout**.
This is exactly the "Python tool-calling for symbolic/numeric checks" surface
Theoremata wants. Example header (`tools/axiom_checker.py:2-11`):

```python
# ---
# name: axiom-checker
# description: Check Lean 4 files for forbidden axiom/constant/postulate declarations
# input:
#   path:
#     type: string
#     description: Path to a .lean file or directory to check
#     required: true
# output: json
# ---
```

The convention across all five tools:
- **Self-describing:** name/description/typed inputs/`output: json` in
  frontmatter → the agent can render a tool schema without extra registration.
- **Stdin/argv in, JSON-on-stdout out, exit code = verdict.** `axiom_checker`
  returns `0` if clean, `1` if a critical issue is found
  (`tools/axiom_checker.py:134,148`) — so the agent can gate on both the exit
  code *and* the structured payload.
- **Machine-readable structured findings.** Each issue is
  `{"line", "severity", "type", "name", "message"}`
  (`tools/axiom_checker.py:81-87`) with severities `critical` / `info`. This is
  the right shape for feeding a repair loop.

**Takeaway for Theoremata:** adopt this exact "frontmatter-typed, JSON-out,
exit-code-as-verdict" contract for our sympy/z3 checkers. It gives model-agnostic
tool-calling with near-zero glue, and the `severity`+`line` payload plugs
straight into a falsify/repair loop.

### 2b) `axiom_checker.py` — a static `#print axioms` pre-filter (mirrors our gate)

This is the closest thing in the repo to our "`#print axioms` gate + LeanParanoia
hardening." It is a **purely static regex scan** (no Lean needed) that catches
proofs which cheat by introducing axioms or leaving placeholders
(`tools/axiom_checker.py:34-46`):

```python
_FORBIDDEN_RE = re.compile(
    rf"^\s*{_ATTR_FRAGMENT}(?:(?:private|protected|noncomputable|local|unsafe|partial)\s+)*"
    rf"(?:axiom|constant|postulate)\s+{_DECL_NAME}",
    re.MULTILINE,
)
_SORRY_RE = re.compile(r"(?<![\w'])(sorry|admit)(?![\w'])")
```

- Flags `axiom`/`constant`/`postulate` decls as `critical`
  ("proof must not introduce axioms", `:86`), surviving `sorry`/`admit` as
  `critical`, and `noncomputable def/instance` as `info` (`:99-107`).
- It handles Lean attribute prefixes (`@[...]`) and modifier keywords in the
  regex — a subtlety worth copying so `@[simp] axiom foo` is still caught.

**Value:** a **fast, dependency-free first-line hardening pass** to run *before*
the expensive Lean `#print axioms` check — reject obviously-cheating candidates
in microseconds. It does NOT replace the real kernel-level axiom check (a proof
can pull axioms transitively through a lemma without a local `axiom` keyword), so
in our design it is a cheap pre-filter, not the gate itself. Note the honesty
gap: the README elsewhere leans on real `#print axioms`; this tool is only the
syntactic guard.

### 2c) `_lean_masking.py` — robust Lean comment/string masker (copy verbatim)

The single most directly reusable *code* asset. Any regex scan of Lean source
(axiom check, sorry count, tactic stats) must not match inside comments/strings.
`mask_lean_comments_and_strings` (`tools/_lean_masking.py:4-75`) replaces
comments and string bodies with spaces **while preserving line/column structure**
(so line/col reported to the model stays accurate). It correctly handles:
- nested block comments `/- ... /- ... -/ ... -/` via `block_depth`
  (`:33-49`),
- line comments `--` (`:51-58`),
- string literals with backslash escapes (`:17-31`),
- and never rewrites newlines, so downstream `count("\n")` line math is exact.

Every other tool imports it (`from _lean_masking import
mask_lean_comments_and_strings`). We should lift this file wholesale into
Theoremata's Lean-scanning utilities — it is a solved, fiddly problem.

### 2d) `proof_stats.py` — tactic-frequency + proof-shape analyzer

`tools/proof_stats.py` reports, per Lean file: theorem/def/import lists, a
tactic-frequency histogram over a curated tactic vocabulary
(`_TACTIC_RE`, `:44-51`: `simp|rfl|ring|omega|linarith|nlinarith|norm_num|aesop|
decide|...|simpa|rwa`), `has_sorry`/`has_admit`, and a `status: proven|unproven`
verdict (`:243-257`). The non-trivial part is `_find_tactic_proof_start`
(`:121-192`), a hand-written scanner that isolates the `:= by ...` tactic block
(skipping term-mode `let/have/suffices` bodies) so tactic counts come only from
real proof text.

**Value:** (i) a ready-made **proof-complexity / telemetry signal** for ranking
best-of-N candidates or logging DAG nodes; (ii) the curated tactic list is a
useful default vocabulary; (iii) `status: proven|unproven` as a JSON verdict is a
clean gate signal. The scanner is somewhat over-engineered — for our best-of-N
ranking a simpler heuristic likely suffices, but the tactic taxonomy is worth
keeping.

### 2e) `sorry_analyzer.py` — placeholder localizer with owning-theorem attribution

`tools/sorry_analyzer.py` finds every `sorry`/`admit`, and for each reports
`line`, `column`, `token`, the **enclosing theorem name** (by walking backward
through declaration positions, `:91-97`), and 2 lines of surrounding context
(`:98-101`). Directory mode aggregates counts across a tree (`:137-158`).

**Value:** this is precisely the "what remains to be proved" signal a
decomposition/tree-prove loop needs — it maps each open goal back to its parent
theorem, which is the join key for a proof-DAG. Directly relevant to our DAG
core: a `sorry` node → the subgoal that must be discharged.

### 2f) `lib_search.py` — keyword search over a persisted theorem library

`tools/lib_search.py` searches a per-vault `TheoremLib/Stored.lean` for
previously-proved theorems, so the agent can **reuse instead of re-derive**. Two
transferable pieces:

1. **A concrete on-disk schema for stored theorems** — a delimited comment block
   (`STORED_THEOREM_BLOCK_RE`, `:37-45`):
   ```
   -- @stored-theorem <Name>
   -- Original: <original name>
   -- Source:   <where it came from>
   -- Proved:   <timestamp>
   theorem <Name> <type-and-body>
   -- @end-stored-theorem
   ```
2. **`split_header_and_body`** (`:80-99`) — splits a theorem's signature from its
   proof at the top-level `:=` using paren-depth tracking, and normalizes a
   term-mode body into `exact (<term>)`. Emits a ready-to-paste
   `usage: "exact <Namespace>.<Name> <args>"` string (`:181`).

**Value:** a minimal, greppable **lemma-cache format** and reuse hint we can
adopt for Theoremata's retrieval layer — no DB required, the store is just an
annotated `.lean` file that also compiles. Naive substring search is weak
(no embeddings), but the *format* and the header/body split are the useful bits.

### 2g) The pipeline architecture, as documented (not as code)

`.env.example` is effectively the product's architecture spec. The proving
pipeline decomposes into **named stages, each independently routable to a
different backend/model** (`.env.example:35-74`):

```
formalize_plan → formalize → formalize_eval → prove_plan → prove
```
- Each `AUTOLEAN_<STAGE>_BACKEND ∈ {codex, cli, openrouter}` and
  `AUTOLEAN_<STAGE>_MODEL` — i.e. per-stage model routing (e.g. keep planning on
  a strong model, route bulk Lean generation to a cheap fast model). This is a
  concrete, minable idea for our loop: **planner and prover need not share a
  model.**
- Model-agnostic provider matrix (`.env.example`): OpenAI/Codex OAuth (default),
  Anthropic API, OpenRouter/Atlas (OpenAI-compatible Responses), Bedrock, Vertex,
  Azure Foundry, MiniMax. This is the "model-agnostic LiteLLM provider" idea,
  realized as env-var routing rather than a library.
- **Iteration/replan knobs** (`.env.example:132-172`) that map onto our loop:
  - `MATHCODE_MAX_FORMALIZE_ITERS=6` — formalization compile-repair iterations.
  - `MATHCODE_ATTEMPTS_BEFORE_REPLAN=5` — proof attempts before re-planning.
  - `MATHCODE_MAX_PLAN_ROUNDS=2` — replanning rounds.
  - `MATHCODE_NUM_PLANNERS` — parallel planners, "prover sees all plans and picks
    the best" (best-of-N at the *plan* level, not just the proof level).
  - **Tree-of-subgoals (DSP-V2-style):** planner emits a Lean skeleton of
    `have ... := by sorry` steps, each leaf proved independently in parallel,
    stitched back and re-compiled, "falls back to flat proving on any failure"
    (`.env.example:162-172`, README `:779-792`). Each proved subgoal becomes a
    first-class lemma-cache entry + a graph node. This is the concrete
    proof-DAG-decomposition recipe, described operationally.
  - **Agent-mode proving:** an interactive session with a persistent Lean REPL,
    ≤10 compiles/session (`MATHCODE_AGENT_MAX_COMPILES=10`,
    `.env.example:156-159`).
- **Feedback mechanics named (design cues):** Lean **LSP** gives structured
  diagnostics (line/col/severity) instead of raw stderr, extracts the **proof
  goal at the error location** for targeted repair, and searches
  **leansearch.net + Loogle** for verified Mathlib lemma names before planning
  (`README.md:739-745`). Persistent Lean REPL warms Mathlib once (~90s) then
  ~0.4s/check (`README.md:688-700`) — the same latency argument behind our
  compile-gate design. Optional **Kimina Lean Server** as an external long-lived
  compiler over HTTP `/verify` (`.env.example:141-151`).

These are architecture *claims/config*, not runnable logic — treat as design
corroboration and a knob checklist, not as code to port.

---

## 3) Data / schema formats

1. **Tool manifest** — YAML-in-comment frontmatter (§2a); typed `input:` map +
   `output: json`. The discovery contract for `tools/*.py`.
2. **Tool output** — JSON on stdout. Issue record:
   `{line, severity(critical|info), type, name, message}`
   (`axiom_checker.py:81-107`); stats record
   (`proof_stats.py:243-257`); sorry location
   `{line, column, token, theorem, context}` (`sorry_analyzer.py:103-109`).
3. **Stored-theorem block** — the delimited `-- @stored-theorem ... --
   @end-stored-theorem` comment schema in `TheoremLib/Stored.lean`
   (`lib_search.py:37-45`). Doubles as compilable Lean + metadata.
4. **Vault namespace derivation** — `basename(vault)` → PascalCase +
   `TheoremLib` suffix, with `MATHCODE_VAULT_NAME` override
   (`lib_search.py:53-77`). Mirrors a TS `theoremLib.ts` (closed).
5. **Plugin manifest** — `.mathcode-plugin/plugin.json`
   (`{name, version, description}`), plus optional `commands/`, `skills/`,
   `agents/`, `hooks/hooks.json`, `.mcp.json` (`plugins/README.md`). This is the
   Claude-Code plugin model rebranded.
6. **Skill format** — a `.md` file with optional frontmatter
   (`description`, `when_to_use`, `allowed-tools`, `model`); filename = command
   name (`skills/README.md`). Built-in skills named but compiled-in:
   `compilation-errors`, `group-theory`, `number-theory`, `parity-proofs`,
   `proof-golfing`, `tactic-cascade`, `type-coercion-patterns`
   (`skills/README.md:2944-2952`) — a useful *taxonomy* of domain skill areas.
7. **Release metadata** — `.mathcode-release` KV file
   (`release_tag`, `mathcode_sha256`, `mathcode_webui_sha256`,
   `setup.sh:2475-2492`) with SHA-256 verification.

---

## 4) What our earlier targeted pass MISSED

- **The core misidentification.** This is not the code-augmented-reasoning
  "MathCode." It is a **closed-binary Lean formalize-and-prove agent built on
  AUTOLEAN** (`README.md:975`), architecturally a sibling of Theoremata, not a
  code-execution reasoner. Any expectation of mining a "write+run code to reason"
  loop from here is unfounded — that loop is not in the checkout, and the product
  itself is Lean-proof-centric, not Python-reasoning-centric.
- **The engine source is entirely absent** — downloaded as a binary by
  `setup.sh`. There are **no prompt templates, no agent loop, no sandbox code**
  to quote. Earlier "tool-calling ideas" attributed to MathCode actually come
  only from the five Python tools' *interface convention*, not from any reasoning
  engine.
- **`.env.example` is the real spec** and was under-weighted: per-stage backend
  routing, the full replan/iteration knob set, tree-of-subgoals, multi-planner,
  Kimina server, cache-policy diagnostics — the entire pipeline shape is
  documented there (§2g).
- **`_lean_masking.py` exists and is genuinely reusable** — easy to overlook as a
  helper, but it is the highest-quality single code asset here.
- **`axiom_checker`'s attribute-aware regex** and `proof_stats`'s
  `:= by` proof-block isolation are non-trivial parsing details worth reusing.
- **The stored-theorem on-disk schema** (compilable Lean doubling as a lemma
  cache) is a concrete retrieval-store design we hadn't noted.
- **Provenance:** the harness prose (compaction, tasks, `/loop`, plugins, skills,
  agents) is Claude-Code-derived; MathCode is a Claude-Code-style shell wrapping
  an AUTOLEAN proving core. Worth knowing so we don't over-attribute novelty.

---

## 5) Test / benchmark value

- **No tests, no benchmarks, no datasets.** No unit tests, no problem sets, no
  eval harness, no sample proofs in the repo. `LeanFormalizations/` (the output
  dir) is `.gitignore`d and absent.
- The five tools are **usable as-is on Windows** (pure Python 3.12+, stdlib
  only). They run against any `.lean` file — so they can serve as **immediate CI
  checks** over Theoremata's generated proofs (axiom/sorry gate, proof stats)
  without adopting anything else from MathCode.
- `bin/lean-check` is a portable pattern for a "compile one file" shim
  (prefers a bundled binary, falls back to `lake env lean`,
  `bin/lean-check:290-311`) — minor reference value for our compile wrapper.
- No benchmark provenance to borrow; if we want comparable numbers, look upstream
  at **AUTOLEAN** (`github.com/T3S1AMAX/autolean`), not here.

---

## 6) New vs. already-in-our-design

**Already in our design (corroboration, not new):**
- Falsify/repair-before-accept loop; Lean compile gate; `#print axioms` honesty
  gate (our real gate; their `axiom_checker` is only a static pre-filter).
- Proof-DAG decomposition (their "tree-of-subgoals" = our DAG core).
- Best-of-N formalization/proof (their multi-planner + attempts-before-replan).
- Model-agnostic provider layer (their env-var provider matrix ≈ our LiteLLM).
- Persistent Lean REPL for fast compile checks; LSP structured diagnostics;
  retrieval before proving (leansearch/Loogle ≈ our retrieve step).
- Graph-first view of theorems (their Obsidian vault ≈ our proof-DAG, though
  theirs is a visualization export, not the execution substrate).

**New / worth adopting:**
1. **The tool contract** (frontmatter-typed inputs + `output: json` +
   exit-code-as-verdict, §2a) — clean, model-agnostic, near-zero glue. Adopt for
   our sympy/z3 checkers.
2. **`_lean_masking.py`** — lift verbatim (§2c).
3. **Static axiom/sorry pre-filter** as a cheap stage *before* kernel
   `#print axioms` (§2b) — a fast reject we hadn't planned.
4. **Per-stage model routing** (planner vs prover vs formalizer on different
   models/backends, §2g) — a concrete cost/quality lever for our loop.
5. **Plan-level best-of-N** ("prover sees all plans, picks best") as distinct
   from proof-level best-of-N — a knob we can add.
6. **Compilable lemma-cache schema** (`-- @stored-theorem` blocks that are also
   valid Lean, §2f/§3.3) — a DB-free retrieval store.
7. **Proof telemetry** (tactic histogram, proven/unproven verdict, §2d) as a
   ranking/logging signal for DAG nodes.
8. **Domain-skill taxonomy** (compilation-errors, group-theory, number-theory,
   parity, proof-golfing, tactic-cascade, type-coercion, §3.6) — a useful
   checklist of skill packs to seed.

**Not applicable / skip:** the entire installer/launcher/PATH machinery
(`setup.sh`, `run`, ~40 KB) is release-distribution plumbing irrelevant to us;
the Obsidian export is a visualization nicety; the Claude-Code-derived harness
prose is not ours to mine.

---

### File index (everything read, in full)
- `.env.example` (216 lines) — pipeline/provider/knob spec.
- `README.md`, `README.ZH.md` (ZH is a translation) — feature + setup docs.
- `setup.sh` (installer), `run` (launcher), `bin/lean-check` (compile shim).
- `lean-workspace/{lakefile.toml, lean-toolchain, MathCodeLean.lean}` — Lake stub.
- `tools/{axiom_checker,lib_search,proof_stats,sorry_analyzer}.py`,
  `tools/_lean_masking.py` — the real code.
- `plugins/README.md`, `skills/README.md` — extension contracts
  (dirs otherwise empty: `.gitkeep`).
- `index.html` — GitHub Pages landing page. `.gitignore`, `.nojekyll`.
- `Demo.png` (2.4 MB) — screenshot, not read (binary image).
