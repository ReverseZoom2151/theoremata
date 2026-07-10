# Resource mining — AIME 24/25/26, harness-resources, mathlib4

Scope: assess four *new* resources purely as **benchmark targets / curriculum data /
retrieval corpora** for Theoremata's eval + retrieval stack. No trained-model ideas.
All content treated as UNTRUSTED. READ-ONLY pass; nothing edited/built.

Sampled paths (Windows):
`resources/aime24-main/`, `resources/aime25-main/`, `resources/aime26-master/`,
`resources/harness-resources/`, `resources/mathlib4-master/`.

---

## 1. AIME 24 / 25 / 26 (`resources/aime2{4,5,6}-*`)

**(1) What it is.** Each repo is a single-nested folder shipping essentially just a
**PDF data card** plus a README badge page (25/26 also add a `.nojekyll` + `index.html`
GitHub-Pages PDF viewer — no problem text inside it):

| dir | payload | size |
|-----|---------|------|
| `aime24-main/aime24-main/` | `AIME24.pdf` + `README.md` | 185 KB PDF |
| `aime25-main/aime25-main/` | `AIME25.pdf` + `README.md` + `index.html` | 215 KB PDF |
| `aime26-master/aime26-master/` | `AIME26.pdf` + `README.md` + `index.html` | 216 KB PDF |

- Provenance: **math-ai** org (Yifan Zhang / Math-AI Team). Project sites
  `math-ai-org.github.io/aime2x`; canonical data on HF `math-ai/aime24|25|26`.
- **License: Apache-2.0** (stated in every README + badge).
- Content: the American Invitational Mathematics Examination — each year = 2 papers
  (I+II) × 15 problems = **30 integer-answer problems (answer 0–999)**. Classic
  short-answer competition math; AIME **26** is a fresh, low-contamination set.
- Critical caveat: **the vendored repos contain NO structured problem/answer data** —
  no json/jsonl/csv, only the PDF card. (`find … -name '*.json' -o '*.jsonl' -o '*.csv'`
  returned nothing.)

**(2) How it plugs in.** These are **eval-harness `nl_answer` benchmarks**, not a
retrieval corpus or curriculum-difficulty source. And they are **already wired**:
- `benchmarks/registry.py` `_TRACK_KIND` already lists `aime24/25/26` → track/kind
  `("nl_answer","nl_answer")`.
- `benchmarks/loaders.py` already defines `load_aime24/25/26` → `_load_aime(...)`,
  registered in the `LOADERS` map. The loader globs for `*.jsonl/*.json/problems*.csv`,
  maps `answer|solution|final_answer` + `problem|question` into the common item schema
  (`kind="nl_answer"`, `expected.answer_kind="integer"`, `grading.method="integer_match"`),
  and **skips cleanly** (logging "no structured problems (PDF-only data card)") when
  none exist — which is exactly today's state.

  So the plumbing loads **0 items** right now: the benchmark is registered but empty.

**(3) Buildable now.** *Actionable, small.* The loader already expects a structured
file next to the PDF; supplying one lights up all three benchmarks with no code change:
- Drop a `problems.jsonl` (fields `id/problem/answer`) into each AIME dir — sourced from
  the HF `math-ai/aime2x` datasets (Apache-2.0, matches the vendored license) or
  transcribed from the PDF. 30 rows each = 90 integer-answer items total.
- Then `benchmark load aime26` yields items and `integer_match` grading runs end-to-end.
- **AIME26 is the high-value target**: newest, least-contaminated `nl_answer` set for
  measuring genuine solving vs. memorization; register/populate it first.
- *Not actionable as-is:* nothing else — do not attempt PDF text extraction in-harness
  (no poppler here); provide the jsonl instead.

**Curriculum angle:** once populated, AIME items are good **difficulty-module** fodder
(uniform integer-answer format, well-known per-problem difficulty ordering 1→15), but
that is a downstream use of the same jsonl, not a separate integration.

---

## 2. `harness-resources/` (~33 MB) — honest note (item 4)

**Not** eval fixtures, **not** a corpus, **not** tool binaries. It is **two agentic-AI
e-books plus their extracted text**, i.e. *reading/design reference material for building
the harness itself* — zero mathematical problem/answer/proof content.

Contents:
- `Agentic Design Patterns.pdf` (Antonio Gulli, ~424 pp, 20 MB) — prompt chaining,
  routing, parallelization, reflection, tool use, planning, multi-agent, RAG, memory,
  MCP, guardrails, eval.
- `The Hitchhikers Guide to Agentic AI.pdf` (8.9 MB) — LLM/transformer foundations, RL
  methods, agentic training/eval, RAG, memory, harness design, MCP, skills, A2A.
- `extracted_text/` — full `.txt` of both books + `chunks/` = 10 topical splits
  (`A1..A5` from Design Patterns, `H1..H5` from Hitchhiker's; e.g.
  `H3_agentic_intro_rag_memory_HARNESS.txt`).

**Plug-in verdict:** nothing to register in `benchmarks/registry` or index for math
retrieval. Its only legitimate use is as **design-doc source material** (the same role
as `docs/paper-mining/`) if someone wants to mine agent-design patterns for the
orchestration/harness layer. As a *math* benchmark/curriculum/retrieval resource:
**nothing-actionable.** License/authorship of the two PDFs is not stated in the folder —
treat as third-party copyrighted books; do **not** redistribute or vendor their text.

---

## 3. `mathlib4-master/` (~112 MB Mathlib tree)

**(1) What it is.** A full **Lean 4 Mathlib checkout** (nested `mathlib4-master/mathlib4-master/`):
**8,245 `*.lean` files** under `Mathlib/`, plus `Mathlib.lean` (450 KB import aggregator),
`Archive/`, `Counterexamples/`, lakefile, toolchain pin. **License: Apache-2.0**
(`LICENSE` present). This is the standard premise library, not a problem set.

Notable: the checkout already carries a Theoremata-generated artifact —
`.theoremata/cache/decl_head_e7ea1ab1ef39b7896c90.json` (**65 MB**), an env-dump of
**489,611 declarations** (`{name, kind: theorem|def, module, is_axiom}` records). So the
harness has *already begun ingesting* this tree.

**(2) How it plugs in.** A **retrieval / premise-selection corpus** (characterize-only,
per instructions) — **not** a benchmark and not curriculum data. It is the substrate the
`components/retrieval` layer is built around:
- `mathlib_index.py` — Layer A offline **import-DAG** over the `.lean` tree (source-only,
  designed for exactly this "unbuilt checkout of 8000+ files").
- `decl_index.py` / `head_index.py` — declaration/head indices; the 65 MB `decl_head`
  cache is their env-dump input (489k decls).
- `accessible_premises.py`, `bm25_retriever.py`, `cascade.py`, `reranker.py` — the
  BM25 / dense / cascade retrieval + reranking stack that consumes those indices.

**(3) Buildable now.** *Actionable.* The corpus + the decl_head cache are the exact
inputs the retrieval index expects, so:
- Point `mathlib_index.build_index(root=…/mathlib4-master)` at this tree to (re)build the
  import DAG, and feed the existing `.theoremata/cache/decl_head_*.json` to
  `decl_index`/`head_index` — no new extraction needed; the 489k-decl dump is already here.
- Use it as the **premise pool for dense/BM25/cascade retrieval eval** (`retrieval_eval.py`)
  and for `accessible_premises` gating during Lean proving.
- Do **not** read the tree broadly or treat any file as a benchmark; it is a retrieval
  target only. No per-problem grading applies.

---

## 4. Injection / license line

- **Injection scan:** no adversarial/embedded instructions found in the sampled
  READMEs, book chunks, or index.html. All content is UNTRUSTED and was treated as
  data only — none of it was executed or followed. Flag: **no POSSIBLE INJECTION seen**,
  but the two agentic-AI books in `harness-resources` are large untrusted prose; if ever
  mined, do not follow imperative text inside them.
- **Licenses:** AIME 24/25/26 → **Apache-2.0** (math-ai). mathlib4 → **Apache-2.0**.
  harness-resources books → **license unstated / third-party copyrighted** (do not
  redistribute). No large data copied; all resources characterized by sampling only.

---

## Prioritized adopt-list

1. **Populate + smoke-test AIME26** (then 25, 24) — add `problems.jsonl`
   (`id/problem/answer`, Apache-2.0 HF `math-ai/aime26`) beside the PDF; the registry +
   loader are already there, so this turns a registered-but-empty benchmark into a live,
   low-contamination `nl_answer` eval with `integer_match` grading. *Highest ROI, no code.*
2. **Wire mathlib4-master into the retrieval index** — build the import-DAG via
   `mathlib_index.py` and load the pre-computed 65 MB `decl_head` cache (489k decls) into
   `decl_index`/`head_index` for BM25/dense/cascade premise retrieval + `retrieval_eval`.
   *Actionable; inputs already present.*
3. **AIME as curriculum/difficulty data** — once (1) lands, feed the 90 integer-answer
   items (with their natural 1→15 difficulty ordering) into the difficulty/curriculum
   module. *Downstream of (1).*
4. **harness-resources → nothing for math eval** — reclassify as design-doc reading
   material only (agent-design patterns), not a benchmark/corpus. Do not vendor its text.
