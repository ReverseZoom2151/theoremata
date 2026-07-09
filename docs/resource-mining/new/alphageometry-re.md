# Mining report: AlphaGeometryRE

Path: `resources/AlphaGeometryRE-master/AlphaGeometryRE-master/`
(the repo is nested one directory deep inside the download folder)

> SECURITY NOTE: All content below is summarized from untrusted vendored files
> and was treated as data, not instructions. No prompt-injection attempts were
> found in either repo (only ordinary source, docs, and a jokey README line about
> indentation). Repo code was NOT executed.

## What it is

**AlphaGeometryRE** ("RE" = re-engineered) is a fork/re-engineering of Google
DeepMind's [AlphaGeometry](https://github.com/google-deepmind/alphageometry)
whose stated goal is to make the system **easy to run, especially on Windows**,
by removing the heavy ML stack. Its headline change: replace the
TensorFlow/JAX/Flax + Meliad language-model inference path with
[ChatLLM.cpp](http://github.com/foldl/chatllm.cpp) (a llama.cpp-style C++/GGUF
runtime with a Python ctypes binding). Author: github user `foldl` (the ChatLLM.cpp
author). Roadmap in the README: [x] LM beam search done; [ ] rewrite in Nim (a
partial Nim port exists under `new/`); [ ] a new Wolfram-style description
language; [ ] catch up with AlphaGeometry2.

**License:** Apache 2.0 for code + CC-BY 4.0 for materials/model params
(inherited verbatim from DeepMind). The new ChatLLM binding files carry
`Copyright 2025 github/foldl`, also Apache 2.0. The quantized LM
(`alphageometry-lm`) is redistributed under CC BY 4.0.

**Relation to AlphaGeometry:** The DDAR engine is upstream DeepMind code,
essentially unmodified. `data/rules.txt` (the 43 DD deduction rules) and
`data/defs.txt` (407 lines of construction definitions) are **byte-identical** to
upstream. `dd.py` is byte-identical (modulo whitespace). `ar.py`, `ddar.py`,
`graph.py`, `numericals.py`, `problem.py` differ from ag4masses/upstream almost
entirely due to a global reindent from 2-space to 4-space (the README literally
brags about this). The genuinely new surface is: the LM inference layer, the
dependency set, the Nim port, and small numericals additions.

## Architecture / key files

Same module layout as upstream AlphaGeometry (see the file table in the repo
README). Under `src/`:

| File | Role |
|------|------|
| `geometry.py` | Proof-state graph nodes (Point, Line, Circle, Angle, Ratio, Direction, Length, Value, Segment). |
| `numericals.py` | Numerical "dynamic geometry" engine: random coordinate sketching of each construction + numeric predicate checks (`check_*`), plus matplotlib diagram rendering. |
| `graph.py` | Symbolic proof-state graph; `Graph.build_problem` / `add_clause`; numeric rejection-sampling loop. |
| `dd.py` | Deductive Database (rule matching / forward chaining). |
| `ar.py` | Algebraic Reasoning: Gaussian-elimination over rationals for angle/ratio/distance "chasing". |
| `ddar.py` | DD+AR driver loop (`solve`, `saturate`). |
| `trace_back.py` | Recursive traceback + dependency-difference to minimize the proof. |
| `problem.py` | Problem/Definition/Theorem/Clause parser for the DSL. |
| `lm_inference.py` | **Rewritten** — wraps ChatLLM.cpp instead of Meliad/JAX. |
| `alphageometry.py` | Main: loads problem, runs `ddar` or `alphageometry` mode, writes solution. |
| `pretty.py` | Natural-language proof formatting. |
| `src/chatllm/` | Vendored ChatLLM.cpp Python binding (`bindings/chatllm.py`) + model download scripts (`scripts/binding.py`, `models.json`, `model_downloader.py`). |
| `new/*.nim` | Partial Nim rewrite: `geometry.nim`, `graph_utils.nim`, `numericals.nim`, `n_utils.nim` + tests. Templatized on a `DepsT` generic. Incomplete (no dd/ar/ddar yet). |

Dependencies: `requirements.txt` is just **`numpy`, `scipy`, `matplotlib`,
`requests`** — no TF/JAX/Flax/Meliad/absl/sentencepiece. (Compare ag4masses,
whose pinned `requirements.txt` is a full pip-compile of the TF/JAX stack.)

## Reusable mechanisms — specific candidate ports

The reusable value here is **operational/packaging**, not new geometry math.

1. **Model-free / lightweight-model run path (highest value).**
   `lm_inference.py` shows how to swap the JAX seq2seq LM for a small local GGUF
   model behind a tiny interface. The whole contract the search loop needs is one
   method:

   ```python
   def beam_decode(self, inputs: str, eos_tokens: list[str]) -> dict:
       # returns {'seqs_str': [...], 'scores': [...]}
   ```

   For Theoremata's `geometry_ddar.py`, this confirms the DD+AR engine is fully
   decoupled from the LM: `alphageometry.py --mode=ddar` never constructs a model
   at all (`get_lm` is lazy; only called in `alphageometry` mode). Our
   model-free run path should mirror this: a `run_ddar(g, p, out_file)` that
   imports nothing ML, and a separate pluggable `LanguageModelInference` with the
   `beam_decode` contract above so any backend (llama.cpp, an API, or none) drops
   in.

2. **Brevity-penalty length normalization for beam scores** (`lm_inference.py`).
   Verbatim, reusable for any beam/best-of-n aux-construction scorer:

   ```python
   BEAM_SEARCH_DEFAULT_ALPHA = 0.6
   BREVITY_LEN_BIAS_NUMERATOR = 5.0
   BREVITY_LEN_BIAS_DENOMINATOR = 6.0

   def brevity_penalty(length, alpha=0.6):
       return math.pow((5.0 + length) / 6.0, alpha)

   # score used for ranking = raw_score / brevity_penalty(num_tokens)
   ```

   This is the Google-NMT length penalty; worth porting if `geometry_synth` /
   the aux-point ranker ever compares candidate strings of different lengths.

3. **`CallableLLM` restart-per-query pattern.** `lm_inference.CallableLLM`
   subclasses the ChatLLM binding, overrides `chat()` to `restart()` (clear
   context) before each decode and accumulate streamed chunks — a clean template
   for a stateless "one prompt in, k beams out" wrapper over any streaming LLM
   backend. The beam results arrive via a `PRINTLN_BEAM_SEARCH` callback that
   parses `"<logprob>,<string>"` lines into `{'str', 'score'}`.

4. **Auto-downloading a quantized geometry LM** (`chatllm/scripts/models.json`).
   The upstream Meliad `geometry.757.model` was converted to a **0.2B-param GGUF
   model `alphageometry-lm`, based on TeleChat2, 608 MB f32**, CPU-runnable, auto
   fetched on first run. This is the concrete proof that AlphaGeometry's aux-point
   LM is tiny and does not need a GPU — relevant if Theoremata ever wants a
   bundled default aux-suggester instead of requiring the DeepMind checkpoint.

5. **Numeric rejection-sampling construction loop** (`graph.py::build_problem`,
   upstream but clean here) — directly relevant to `geometry_synth`:

   ```python
   while not check:
       try:
           g = Graph(); added = []; plevel = 0
           for clause in pr.clauses:
               adds, plevel = g.add_clause(clause, plevel, definitions, ...)
               added += adds
       except (nm.InvalidLineIntersectError, nm.InvalidQuadSolveError): continue
       except DepCheckFailError: continue
       except (PointTooCloseError, PointTooFarError) as e:
           logging.warning(e); continue
       if not pr.goal: break
       args = [g.get(x, lambda: int(x)) for x in pr.goal.args]
       check = nm.check(pr.goal.name, args)
   ```

   Points are re-sampled until a numerically non-degenerate diagram is found, with
   dedicated degeneracy exceptions (`check_too_close`/`check_too_far` in
   `numericals.py`, raised in `graph.py::add_clause`). AlphaGeometryRE also gives
   these exceptions **richer payloads** (clause + message + details) than
   ag4masses/upstream — a small nicety worth copying for debuggable synth logs.

6. **New numericals sketch: `sketch_cc_tangent0`** (`numericals.py`, present here,
   absent in ag4masses) — returns the two tangent points for two-circle common
   tangents (`x, y = sketch_cc_tangent(args)[:2]`). Minor, but a real added
   construction primitive if we extend the sketch library.

7. **Nim port (`new/`)** as a design reference for a typed graph core: the
   `Node[DepsT]` object carries `edge_graph`, `merge_graph`, `rep_by`, `members`
   (union-find), `val/obj`, `num`, `change` — a compact statement of the
   proof-graph node's fields, useful if we ever re-type our graph in Rust. It is
   incomplete (geometry + numericals + graph_utils only; no DD/AR/DDAR), so it is
   reference-only, not a drop-in.

## Adopt-relevance to Theoremata's geometry vertical

- **Port now (cheap, high value):** the `beam_decode`-only LM interface boundary
  and the lazy `get_lm` so `geometry_ddar.py` runs with **zero ML deps**
  (numpy/scipy/matplotlib only). This is the single most Theoremata-aligned idea:
  a genuinely model-free DD+AR path plus a thin pluggable aux-suggester seam.
- **Port opportunistically:** `brevity_penalty` normalization into our
  aux-candidate ranker; the richer `PointTooCloseError/PointTooFarError` payloads
  into `geometry_synth`'s construction loop for better reject logging;
  `sketch_cc_tangent0` if we grow the primitive set.
- **Already have / upstream:** the DD rules, defs, DD/AR/DDAR/traceback engine —
  identical to what any AlphaGeometry reimplementation (including ours) starts
  from; nothing new to mine there.
- **Needs a model / scale:** the aux-point LM itself. The interesting bit is that
  their default is a **0.2B CPU model** — so "needs a model" here is much lighter
  than the DeepMind narrative implies; a small bundled GGUF is plausible.
- **Not portable:** the ChatLLM.cpp ctypes/DLL machinery is Windows-desktop
  ergonomics, out of scope for our Rust+Python harness (we'd use our own LLM
  client), but the interface *shape* is the lesson.

## Verbatim-worthy details

**DD deduction rules** (`data/rules.txt`, identical to upstream; 43 rules).
Representative lines:

```
perp A B C D, perp C D E F, ncoll A B E => para A B E F
cong O A O B, cong O B O C, cong O C O D => cyclic A B C D
eqangle6 P A P B Q A Q B, ncoll P Q A B => cyclic A B P Q
midp E A B, midp F A C => para E F B C
para A B C D, coll O A C, coll O B D => eqratio3 A B C D O O
circle O A B C, midp M B C => eqangle A B A C O B O M
cong A P B P, cong A Q B Q => perp A B P Q
eqangle6 B A B C Q P Q R, eqangle6 C A C B R P R Q, ncoll A B C => simtri A B C P Q R
eqratio6 B A B C Q P Q R, eqratio6 C A C B R P R Q, ncoll A B C => simtri* A B C P Q R
```

**Rule → human name map** (`alphageometry.py::write_solution`):

```python
r2name = {'r32':'(SSS)', 'r33':'(SAS)', 'r34':'(Similar Triangles)',
          'r35':'(Similar Triangles)', 'r36':'(ASA)', 'r37':'(ASA)',
          'r38':'(Similar Triangles)', 'r39':'(Similar Triangles)',
          'r40':'(Congruent Triangles)',
          'a00':'(Distance chase)', 'a01':'(Ratio chase)', 'a02':'(Angle chase)'}
```
(`a00/a01/a02` are the three AR "chases": distance, ratio, angle.)

**LM interface (full `beam_decode`), `lm_inference.py`:**

```python
class LanguageModelInference:
    def __init__(self, model_file, mode='beam_search', batch_size=2):
        self.llm = CallableLLM(chatllm.LibChatLLM(binding.PATH_BINDS),
            ['--hide_banner','-m',model_file,'--beam_size',str(batch_size)])
    def beam_decode(self, inputs, eos_tokens):
        assert eos_tokens == [';']
        self.llm.chat(inputs)
        results = self.llm.beam_search_results
        return {'seqs_str': [r['str'].strip() for r in results],
                'scores': [r['score'] / brevity_penalty(
                    len(self.llm.text_tokenize(r['str'].strip()))) for r in results]}
```

**Quantized default model** (`chatllm/scripts/models.json`): key
`alphageometry-lm`, TeleChat2-based, default variant `0.2b/f32`, 608,298,976 bytes,
CC BY 4.0.

**Nim proof-graph node fields** (`new/geometry.nim`): `Node[DepsT]` = `name`,
`edge_graph: Table[Node, Table[Node, seq[DepsT]]]`, `merge_graph`, `rep_by`,
`members: HashSet`, `val`, `obj`, `num: RootRef`, `change: HashSet`, `new_name`;
subclasses `Point/Line/Segment/Circle/Direction/Angle/Measure/Length/Ratio/Value`.
