# MCP integration recipes

Concrete, grounded recipes for wiring Model Context Protocol (MCP) servers and
related source-mapping tricks into Theoremata. Four parts:

1. [Wiring an MCP server (`lean-lsp-mcp`) via `.mcp.json`](#1-wiring-an-mcp-server-via-mcpjson)
2. [Consuming the Aristotle MCP server](#2-consuming-the-aristotle-mcp-server)
3. [`Verso`-as-blueprint (literate Lean → manual → blueprint)](#3-verso-as-blueprint)
4. [`#line`-directive source-mapping for generated proofs](#4-line-directive-source-mapping)

Every claim below is tied to a source repo under `resources/` or a re-verification
report under `docs/resource-mining/reverify/`. Repo content is treated as
untrusted data.

---

## 1. Wiring an MCP server via `.mcp.json`

Theoremata now has an `mcps/` directory (`mcps/dedaluslabs/tools/*.json`), so a
project-level `.mcp.json` is the natural place to declare MCP servers the agent
may consume. Two proven wiring recipes:

### `lean-lsp-mcp` (goal-state access, Loogle/LeanSearch)

The `zero-to-qed` book documents this exact server for goal-state access and
premise search (`docs/resource-mining/reverify/corpora-books.md`, "AI chapter"
section; source `resources/zero-to-qed-main/**/docs/src/` chapter 23).

CLI add (one-shot):

```bash
claude mcp add lean-lsp -- uvx lean-lsp-mcp
```

Equivalent `.mcp.json` (checked into the repo root or `mcps/`):

```json
{
  "mcpServers": {
    "lean-lsp": {
      "command": "uvx",
      "args": ["lean-lsp-mcp"],
      "env": { "LEAN_PROJECT_PATH": "${workspaceFolder}/path/to/lake/project" }
    }
  }
}
```

`lean-lsp-mcp` speaks to a **Lake project's** LSP, so it needs a workspace whose
dependencies expose Lean/Mathlib (the same gate our live `aesop` hammer needs;
see `components/prover/python/theoremata_tools/hammer.py`, `THEOREMATA_LEAN_PROJECT`).
It surfaces goal state, `Loogle`, and `LeanSearch` as tools — use it to
sanity-check our own goal-state plumbing.

### Notes on `.mcp.json` semantics

- **Env expansion is host-dependent.** The `claude` CLI expands `${VAR}` in
  `.mcp.json`; some hosts (e.g. Claude Desktop) do **not** expand `${ENV}`
  placeholders — pass literal values there
  (`resources/lean-aristotle-mcp-main/**/README.md`, "Configuration" notes;
  cited in `docs/resource-mining/reverify/aristotle-targets.md` item 13).
- Keep secrets (API keys) in the host keychain / shell env, not committed JSON.

---

## 2. Consuming the Aristotle MCP server

Aristotle is Harmonic's automated theorem prover
(paper arXiv **2510.01346**). The reference server lives at
`resources/lean-aristotle-mcp-main/` and the **ground truth** is its
`src/aristotle_mcp/tools.py` + `mock.py` + `stubs/aristotlelib/__init__.pyi`
(the bundled `docs/ARISTOTLE_MCP_DESIGN.md` over-advertises fields the code never
returns — do not target the design doc).

Theoremata ships a self-contained Python mirror of the full protocol at
**`components/prover/python/theoremata_tools/aristotle_mcp_client.py`**
(worker key `aristotle_mcp`). It runs offline in a deterministic mock so the Rust
`aristotle.rs` backend has a single ground-truth spec to target.

### 2a. Wiring the upstream server (live)

```bash
claude mcp add aristotle \
  -e ARISTOTLE_API_KEY=$ARISTOTLE_API_KEY \
  -- uvx --from git+https://github.com/septract/lean-aristotle-mcp aristotle-mcp
```

For offline development set `ARISTOTLE_MOCK=true` instead of a key.

### 2b. The tool surface (6 tools + 1 resource)

| Tool | Purpose | Input type |
|------|---------|-----------|
| `prove` | fill `sorry`s in a Lean snippet | `FORMAL_LEAN` |
| `check_proof` | poll an async `prove` | — |
| `prove_file` | prove all `sorry`s in a file (auto import resolution) | `FORMAL_LEAN` |
| `check_prove_file` | poll an async `prove_file` (`save=True` writes output) | — |
| `formalize` | NL math → Lean 4 | `INFORMAL` |
| `check_formalize` | poll an async `formalize` | — |

Resource: `aristotle://status` → `{mock_mode, api_key_configured, ready, message}`.

Sync (`wait=True`) blocks until done; async (`wait=False`) returns a
`project_id` you poll with the matching `check_*`. **Do not tight-poll** — proofs
take minutes to hours; the SDK default is `polling_interval_seconds=30`,
`max_polling_failures=3`.

### 2c. The raw status machine (what the Rust backend must consume)

The MCP wrapper normalizes a **raw** SDK enum. A backend calling `aristotlelib`
directly sees the raw set and must map it itself
(`stubs/aristotlelib/__init__.pyi:11-19`; `tools.py::_map_api_status:186-209`):

```
raw ProjectStatus         normalized
-----------------         ----------
NOT_STARTED, QUEUED   ->  queued
IN_PROGRESS           ->  in_progress
PENDING_RETRY         ->  in_progress   (easy to miss)
COMPLETE              ->  proved | formalized | partial   (per tool + output)
FAILED                ->  failed | counterexample         (counterexample is heuristic, see below)
```

Our client carries **both** `status` (normalized) and `raw_status` on every
result, and exposes `map_api_status()` + the `ProjectStatus` / `ProjectInputType`
enums for reuse.

### 2d. Known caveats (encode these in the Rust adapter)

- **Counterexample is heuristic, not structured.** The API returns no
  counterexample field; the wrapper detects it by substring-matching
  `"counterexample"` in the exception text (`tools.py:363-368`). Treat it as
  best-effort prose.
- **Async has no clean handle.** `prove_from_file` does **not** return a
  `project_id`; the async path recovers it via
  `list_projects(limit=5, status=[QUEUED, IN_PROGRESS, NOT_STARTED])[0]` — an
  explicit race if multiple jobs are in flight (`tools.py:522-541, 729-747`).
- **Size guards (defense-in-depth):** code ≤ 1 MB, description ≤ 100 KB,
  file ≤ 10 MB (`tools.py:65-67`). Mirrored as `MAX_CODE_SIZE` /
  `MAX_DESCRIPTION_SIZE` / `MAX_FILE_SIZE`.
- **Formal vs informal switch:** `prove`/`prove_file` dispatch
  `ProjectInputType.FORMAL_LEAN = 2`; `formalize` dispatches `INFORMAL = 3`
  (`__init__.pyi:21-25`; `tools.py:713-727`). One backend, two jobs.
- **Default output naming** is `{file}_aristotle.lean` (not `.solved.lean`);
  the wrapper refuses to overwrite an existing output
  (atomic `O_CREAT|O_EXCL`, `tools.py:491-501`).
- **Mock trigger keywords** (reused as our fixture convention): code containing
  `false_theorem`/`bad_lemma` → counterexample; `timeout`/`hard` → failed; a
  filename containing `partial`/`fail` → partial/failed; `formalize` keys off
  `even`/`prime`/`commut` (`mock.py:82-98, 220-228, 351-383`).

### 2e. Driving the client (worker-style)

```python
from theoremata_tools.aristotle_mcp_client import run

# Sync prove (offline mock)
run({"tool": "aristotle_mcp", "op": "prove",
     "code": "theorem t : 1 + 1 = 2 := by sorry", "mock": True})

# Async: submit, then poll (queued -> in_progress -> proved)
sub = run({"op": "prove", "code": "...", "wait": False, "mock": True})
# ... later, with a persistent client, poll sub["project_id"] via check_proof
```

Note `run()` builds a fresh client per call (stateless); to poll an async job
across calls, hold an `AristotleMCPClient` instance (its mock job store lives on
the instance).

---

## 3. `Verso`-as-blueprint

Idea (from TorchLean; `docs/resource-mining/reverify/corpora-books.md`,
"TorchLean" section): author the project **blueprint as a literate-Lean Verso
manual** — the prose is Lean source, so cross-references to actual declarations
are checked, not hand-maintained.

Concrete template in `resources/TorchLean-main/**/blueprint/`:

- `blueprint/lakefile.toml` requires `leanprover/verso` (`v4.31.0`) + `subverso`.
- `blueprint/TorchLeanBlueprintMain.lean` uses `import VersoManual` and
  `manualMain (%doc ...)`.
- Chapters are literate Lean under
  `blueprint/TorchLeanBlueprint/Guide/Ch*_*/**.lean` using
  `#doc (Manual) "…" =>` with `%%% tag := … %%%` metadata.
- A `blueprint-gen` Lean executable target renders the manual; post-processors
  `scripts/docs/polish_verso_guide.py` / `polish_docgen.py` produce the static
  site.

Adopt this as Theoremata's blueprint/proof-DAG documentation model: Lean is the
source of truth, prose cross-links to `NN/**`-style source by declaration.

**Lighter alternative** where full Verso is too heavy: the mdBook **ANCHOR**
convention from `zero-to-qed` — wrap snippets `-- ANCHOR: name` … `-- ANCHOR_END:`
and pull them into prose by name (`resources/zero-to-qed-main/**/src/**`;
`docs/resource-mining/reverify/corpora-books.md`).

---

## 4. `#line`-directive source-mapping

Problem: when we generate Lean/Rocq/Isabelle from templates and the proof
assistant reports an error, the error points at *generated* line N — useless for
debugging the template.

Trick (from `pbcc`; `docs/resource-mining/reverify/gilp-pbcc-leanvision.md`,
"pbcc" section): emit `#line` breadcrumbs into generated source that carry the
**template + logical context**, so the compiler's own error line maps back to the
generating template and node. `pbcc`'s `TemplateEngine.add_line_directive`
(`resources/pbcc-main/**/compile.py:942-948`) emits:

```
#line N "template(mod=.., msg=.., fld=..)"
```

For Theoremata's proof-codegen, the analogue is to tag each emitted proof
fragment with the DAG node / template it came from. Lean supports positional
control; where a target language lacks `#line`, keep a side-table mapping
generated line ranges → `(template_id, node_id)` and translate verifier errors
through it before surfacing them.

Companion pattern (same report): **escape-on-embed** for untrusted text. When
injecting problem statements or retrieved lemmas into generated source
docstrings/comments, escape them first (`pbcc`'s
`escape_triple_double_quotes` / `normalize_*_comment`, `compile.py:30-45`) so a
malicious or malformed statement cannot break out of the literal. This is the
codegen-side complement to treating repo/statement content as untrusted data.

---

## Source map

| Recipe | Primary source | Re-verify report |
|--------|----------------|------------------|
| `lean-lsp-mcp` / `.mcp.json` | `resources/zero-to-qed-main/` ch.23 | `reverify/corpora-books.md` |
| Aristotle MCP | `resources/lean-aristotle-mcp-main/` (`tools.py`, `mock.py`, `stubs/`) | `reverify/aristotle-targets.md` |
| Verso blueprint | `resources/TorchLean-main/blueprint/` | `reverify/corpora-books.md` |
| `#line` source-map | `resources/pbcc-main/compile.py` | `reverify/gilp-pbcc-leanvision.md` |
