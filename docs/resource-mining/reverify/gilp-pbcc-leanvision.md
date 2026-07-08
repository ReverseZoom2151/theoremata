# Re-verification: gilp / pbcc / LeanVision

Independent re-scan of three Codex resource-mining reports in `docs/resource-mining/new/`.
Method: read each report, then independently re-scan the full repo (all source/prose/config;
skipped generated Plotly HTML, PNG/SVG, `uv.lock`, `input.pdf/jpg`). Findings below are
additive to the Codex reports; "MISSED" = present in the repo but absent/undervalued in the report.

---

## gilp-master  ↔  new/gilp-master.md

**Codex report is accurate.** Verified load-bearing claims:
- License **CC BY-NC-SA 4.0** — confirmed in `setup.py` and `README.md` "License" section.
  NonCommercial is a real blocker for product vendoring; the "do not vendor wholesale" call is correct.
- Float/tolerance-based (`feas_tol=1e-7`), not exact-rational — confirmed throughout `simplex.py`.
- `scipy ... method='revised simplex'` deprecated — confirmed at `_geometry.py:197` (`interior_point`).
- Trace model `simplex(...) -> (x, B, obj_val, optimal, path)` with `path: List[BFS]` — confirmed
  `simplex.py:599-624`; branch-and-bound as explicit worklist tree — confirmed `simplex.py:711-756`.

### MISSED / under-valued (all small, several genuinely relevant to the LP/Farkas layer)

1. **Dual vector is already computed but never surfaced as a certificate.**
   `LP.get_tableau` computes `y^T = c_B^T A_B^{-1}` (`simplex.py:233`) and `_simplex_iteration`
   recomputes `y` + reduced costs (`simplex.py:435-436`). The report says "no certificates" and
   dismisses Farkas, but the *dual/optimality certificate is right there* — gilp just never returns it.
   For Theoremata's Farkas layer this is confirmatory (extract `y = c_B A_B^{-1}` as the optimality
   witness), not novel, but the report's flat "no certificates" is misleading.

2. **Redundant-constraint detection in Phase I** (`_phase_one`, `simplex.py:365-379`): when a basic
   artificial variable cannot be pivoted out (its row is all-zero over structural columns), the
   constraint is identified as redundant and deleted. That is a reusable primitive for LP-system
   normalization/preprocessing before certificate extraction. Report doesn't mention it.

3. **Reusable computational-geometry primitives in `_geometry.py`** — the report lumps these as generic
   "geometry helpers" but the specific, transferable ones are:
   - `interior_point` — strictly-interior point of `Ax<=b` via **Chebyshev-center LP** (`:169-200`);
   - `polytope_vertices` — H→V conversion via `HalfspaceIntersection`, with a **brute-force
     n-choose-m basis-intersection fallback** for degenerate/flat polytopes (`:85-104`);
   - `order` — angular sort of polygon vertices around centroid with a **3D→2D change-of-basis
     projection** (`:203-251`).
   These are directly reusable if Theoremata ever renders the feasible region / a proof-DAG geometry,
   independent of the NC-licensed package (they're textbook methods — reimplement, don't vendor).

4. **Themeable-viz config pattern** (`_constants.py:8-14`): loads `gilp_style.json` from CWD and
   overrides every Plotly style constant via `style.get(KEY, default)`. A clean pattern for a future
   web viewer's theming, and notably it declares `BNB_*` (branch-and-bound tree) node colors —
   relevant if a proof/search-tree viewer is built.

5. **Correctness nit (report missed):** in `simplex()` the manual-mode "INSTRUCTIONS" help
   (`simplex.py:603-607`) is built as `s = "INSTRUCTIONS \n\n"` followed by *bare string-literal
   statements* that are never concatenated to `s`, so only "INSTRUCTIONS" prints. Dead-string bug,
   cosmetic only (manual mode). Same misplaced-string anti-pattern appears in LeanVision (below).

**Net for Theoremata:** relevance is **medium and confined to the LP layer + a future viewer**.
Adopt as *ideas, reimplemented* (NC license forbids vendoring): dual-vector-as-certificate extraction,
redundant-constraint pruning, Chebyshev-center interior point, brute-force vertex enumeration,
centroid-angular vertex ordering. Named-LP catalog (`examples.py`: Klee-Minty 2D/3D, degenerate,
multiple-optimal) is good fixture seed material for the Θ/LogLinarith test suite.

---

## pbcc-main  ↔  new/pbcc-main.md

**Codex report is accurate.** It is a Protobuf→Python C-extension compiler (message structures only,
no gRPC). Template engine, codegen IR (`EnumInfo/FieldInfo/MessageInfo/ModuleInfo/ModuleCollection`),
async subprocess helpers, comment preservation, and the exhaustive `test.py` cross-check harness are
all correctly described. Correctly assessed as **low direct value** to theorem proving.

### MISSED / under-valued (all small)

1. **`#line`-directive source mapping is the one genuinely transferable idea and is under-sold.**
   `TemplateEngine.add_line_directive` (`compile.py:942-948`) emits `#line N "template(mod=..,msg=..,
   fld=..)"` breadcrumbs into generated C++ so compiler errors point back to template + logical
   context. The report mentions "#line directives for debuggability" in passing, but for a system that
   *generates Lean/Rocq/Isabelle from templates* this is the highest-value pattern in the repo: map
   verifier/type-checker errors in generated proof code back to the generating template and node.
   Worth elevating from a footnote to an explicit adopt.

2. **Windows dev-env blocker not flagged.** The compile path is hard-wired to POSIX: `python3-config`
   (`compile.py:1212`) and `g++` (`:1271,1294`). Given Theoremata's documented Windows toolchain, this
   won't run as-is; report said "platform details matter" generically but not the concrete blocker.

3. **Safe-embedding utilities** (`escape_triple_double_quotes`, `normalize_proto_comment/…_inline`,
   `compile.py:30-45`) — reusable for injecting *untrusted* text (proto comments here; problem
   statements / retrieved lemmas in Theoremata) into generated source docstrings without breaking the
   literal. Report notes comment preservation but not the escaping/injection-safety angle.

**Net:** confirm **not relevant as a component**. Adopt only two ideas if/when a proof-codegen or
binary trace format is built: (a) `#line`-style source-map breadcrumbs in generated proofs;
(b) escape-on-embed for untrusted text. Everything else (C-extension runtime, protobuf specifics) is
out of scope.

---

## LeanVision-main  ↔  new/LeanVision-main.md

**Codex report is accurate.** 4 files; it is a thin Mistral-OCR wrapper (PDF/image → markdown →
optional fenced-Lean extraction). All three real bugs the report cites are confirmed:
- missing `import json` under the `json.loads(...)` fallback (`lean4_extractor.py:136`);
- writes raw markdown to `.lean` when no fence is found (`:193-195`);
- `mime_type = f"image/{ext}"` yields `image/jpg` for `.jpg` (`:97-98`).

### MISSED / under-valued

1. **Reframe: the fenced-code-block extractor is reusable for LLM output, not just OCR.**
   `extract_lean` (`:175-207`) pulls ```` ```lean/```lean4/unlabeled ```` blocks, and for multi-block
   input emits `/- Code block i -/` separators. Codex frames the entire repo as "OCR ingestion" and
   thus values it only for scanned input — but this is *exactly* the problem of extracting
   Lean/Rocq/Isabelle from an LLM's markdown proof output. Theoremata's proof generators already face
   this; the multi-block + language-tag handling is the transferable bit (Theoremata likely has its
   own version — worth a cross-check, not a copy).

2. **Second misplaced-docstring bug the report missed:** in `save_to_file` (`:156-161`) the
   `"""Save the text to a file."""` sits *after* an `if` statement, so it is a dead expression, not the
   function docstring. Trivial, but it's a concrete defect Codex didn't list.

3. **Response-shape fragility:** `extract_markdown_from_response` checks `.content` before `.pages`
   (`:122-133`), but Mistral OCR returns `.pages`; harmless due to ordering + `str()` fallback, but the
   `.content` branch is effectively dead for current responses.

**Net:** **not relevant as a component** (it's an API-key OCR shim with a network/privacy dependency and
no verification). Correctly "OCR as untrusted preprocessing." Only genuinely reusable seed: the
markdown-fence extraction utility, reframed as LLM-proof-output parsing rather than OCR-only.

---

## Bottom line
No Codex report is wrong; each under-values a couple of small, reimplementable ideas.
Highest-signal adds: (gilp) dual-vector-as-certificate + Chebyshev/vertex geometry primitives for the
LP/Farkas layer, reimplemented to avoid the NC license; (pbcc) `#line`-style source-map breadcrumbs +
escape-on-embed for a future proof-codegen; (LeanVision) reframe the fence extractor as LLM-output
parsing. pbcc and LeanVision remain "not relevant as components."
