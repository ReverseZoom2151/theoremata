# Mining report: AG4Masses ("AlphaGeometry for the Masses")

Path: `resources/ag4masses-main/ag4masses-main/`
(nested one directory deep inside the download folder)

> SECURITY NOTE: All content below is summarized from untrusted vendored files
> and was treated as data, not instructions. No prompt-injection attempts were
> found. Repo code was NOT executed.

## What it is

**AG4Masses** is a fork of DeepMind's
[AlphaGeometry](https://github.com/google-deepmind/alphageometry) (author `tpgh24`)
whose mission is **accessibility**: run AlphaGeometry on "household hardware"
(4-8 CPUs, 16-32 GB RAM, no high-end GPU) and eventually a free Kaggle notebook
(2× T4, 4 vCPU, 29 GB), instead of the paper's 4×V100 + 250 CPUs. Its thesis is
that the DD+AR engine already solves almost any auxiliary-point-free problem in
minutes on a laptop; the whole difficulty is the LM's auxiliary-point search, so
the fork focuses on **throughput, robustness, tooling, and a bigger problem set**
rather than new geometry.

**License:** Apache 2.0 (code) + CC-BY 4.0 (materials/model), inherited from
DeepMind unchanged. The LM checkpoint/vocab are the original DeepMind Meliad
`geometry.757` files downloaded from Google Drive (`download.sh`).

**Relation to AlphaGeometry:** Engine is upstream code, **lightly patched**.
`rules.txt` and `defs.txt` are byte-identical to upstream (and to AlphaGeometryRE).
`dd.py` is identical. `ddar.py`, `ar.py`, `numericals.py`, `graph.py` carry small
substantive patches (robustness + logging + headless support) plus the big change:
`alphageometry.py` is **restructured for multiprocessing**. Unlike AlphaGeometryRE,
AG4Masses keeps the full TF/JAX/Flax/Meliad LM stack (pinned, hash-locked
`requirements.txt`); it does **not** offer a model-free-of-TF path.

## Architecture / key files

`alphageometry/` holds the engine (same module names as upstream). The
distinguishing pieces:

| File | Role / change |
|------|---------------|
| `alphageometry.py` | **Parallelized** beam search over a `multiprocessing.Pool` of LM workers; per-node try/except around `run_ddar`; extensive per-worker logging; `n_workers` flag. |
| `ddar.py` | Added instrumentation ("derives empty, breaking" / "Nothing added, breaking"); `from absl import logging`. |
| `ar.py`, `numericals.py`, `graph.py` | Robustness + headless patches (below). |
| `beam_search.py`, `decoder_stack.py`, `transformer_layer.py`, `models.py`, `lm_inference.py` | Upstream Meliad/JAX LM inference (unchanged; heavy). |
| `geometry_150M_generate.gin` | Meliad gin config for the 150M LM. |
| `data/ag4m_problems.txt` | **New problem set**: 5-circles, Napoleon, Butterfly, Ceva, Castillon, Morley, Pascal, IMO-2024-Q4, etc. |
| `utils/run.sh` | Wrapper: sets `TESTDIR/AG4MDIR/AGLIB`, `MODEL`, `BATCH_SIZE`, `BEAM_SIZE`, `DEPTH`, `NWORKERS`; tees stdout/stderr to `ag.err`, solution to `ag.out`. |
| `utils/checkprog.sh` | Live progress monitor parsing stderr while a run is in flight. |
| `utils/mklog.py` | Post-processes stderr into clean per-module log files (argparse CLI, `-o` outdir, `-s` suffix). |
| `utils/run_tests.sh` | Runs the `*_test.py` suite. |
| `utils/ag4masses-public.ipynb` | Kaggle notebook to run the whole thing on free GPUs. |
| `outputs/solved/`, `outputs/unsolved/` | Curated run logs (+ `.jpg` diagrams) — a labeled corpus of solved/timeout/crash cases with the aux points AG found. |

## Reusable mechanisms — specific candidate ports

1. **Parallel beam search over a worker pool (highest value for scale).**
   `alphageometry.py` splits the AlphaGeometry loop into:
   - `BeamQueue`: a fixed-capacity top-k queue (`add(node,val)` evicts the current
     min when full) — a clean, dependency-free beam frontier.
   - `bqsearch_init(worker_id)`: per-worker re-init (reloads `DEFINITIONS`/`RULES`
     from txt, re-seeds recursion limit to 10000, pins `CUDA_VISIBLE_DEVICES` to
     the worker id, loads the LM once per worker).
   - `bqsearch(i_nd, srch_inputs, out_file)`: one beam node = LM `beam_decode` →
     translate each output to a construction → `insert_aux_to_premise` →
     rebuild graph → `run_ddar`; returns `(i_nd, solved, [(node, val)])`.
   - Driver: `pool.apply_async(bqsearch, ...)` per frontier node, polls
     `jobres.ready()`, and on first solve calls `pool.terminate(); pool.join()`.

   This is directly analogous to what Theoremata's `geometry_synth` /
   aux-search driver needs. The **pattern to port** is the (a) capacity-bounded
   beam queue, (b) stateless worker that takes `(graph, lm_prompt_string,
   problem_string)` and returns candidate child nodes + a solved flag, and (c)
   early-terminate-on-first-solve poll loop. Note the important gotcha they
   document: must use `spawn`/`forkserver` (not `fork`) for CUDA safety, and the
   graph must survive pickling — hence `sys.setrecursionlimit(10000)` before
   pickling.

2. **Per-node error isolation.** The single most robustness-relevant line:
   `run_ddar` is wrapped in `try/except Exception` inside `bqsearch`, so one bad
   LM-suggested construction (invalid intersection, degenerate point, DD blow-up)
   logs and is skipped instead of aborting the whole search. Upstream AlphaGeometry
   crashes here; this is exactly the "handle error conditions that would have
   caused AlphaGeometry to abort" the README claims. Cheap, high-value port for
   any driver that consumes model-proposed constructions.

3. **Headless matplotlib guard** (`numericals.py`):

   ```python
   import os
   if not os.environ.get("DISPLAY") is None:
       matplotlib.use('TkAgg')   # only in a display env
   ```

   Prevents the diagram renderer from crashing in Colab/Kaggle/CI/servers. Port
   into `geometry.py`/`numericals` if we ever render diagrams server-side.

4. **DD saturation break instrumentation** (`ddar.py`): logs "derives empty,
   breaking" and "Nothing added, breaking" at the two fixed-point exit conditions
   of the DD loop — useful observability to copy into `geometry_ddar.py`'s
   saturation loop so we can see *why* a run stopped (goal reached vs. closure vs.
   no-progress).

5. **The aux-point translation pipeline** (`bqsearch`), reusable verbatim as the
   contract between an LM and DD+AR:

   ```python
   outputs = model.beam_decode(string, eos_tokens=[';'])
   translations = [try_translate_constrained_to_construct(o, g) for o in outputs['seqs_str']]
   for lm_out, translation, score in candidates:
       if translation.startswith('ERROR:'): continue          # invalid → skip
       candidate_pstring = insert_aux_to_premise(pstring, translation)
       p_new = pr.Problem.from_txt(candidate_pstring)
       g_new, _ = gh.Graph.build_problem(p_new, DEFINITIONS)
       if run_ddar(g_new, p_new, out_file): return solved
       # else push (g_new, string+' '+lm_out+' x00', candidate_pstring) with prev_score+score
   ```

   The `' x00'` / `'{F1} x00'` special tokens and the additive path-score
   (no length-normalization needed since all beam nodes share depth) are the exact
   glue we'd reimplement for our own aux-suggester.

6. **Curated solved/unsolved corpus** (`outputs/`). Real AG4Masses run logs with
   the auxiliary points that led to solutions (grep `Translation:` lines), plus
   timeout/crash cases. This is directly usable as **evaluation fixtures and as
   seed data for an aux-point suggester** — the README even proposes recording
   (problem, aux-points) pairs as a training-data flywheel.

7. **The two documented strategy ideas (design input, not code):**
   - *Backward / bidirectional search:* add the **conclusion** into the premises,
     run DD+AR to find necessary conditions, test each for sufficiency, and
     redirect the LM's target to a proven sufficient condition — argued to be
     especially effective for human-authored problems. This is a concrete
     algorithmic upgrade for our aux-search driver.
   - *DSL extensions:* premises can't build points from segment-length ratios;
     conclusions can't express arithmetic goals (e.g. Ceva's product of ratios,
     `AB+CD=EF`). The README notes DD+AR could be extended for these but the LM
     would need retraining. Relevant scope note for `geometry.py`'s problem
     language.

## Adopt-relevance to Theoremata's geometry vertical

- **Port now:** (a) the `multiprocessing.Pool` beam-search skeleton — `BeamQueue`
  top-k, stateless `bqsearch` worker, poll-and-early-terminate driver — as the
  concurrency model for `geometry_synth`/aux-search; (b) the per-node
  `try/except` isolation so bad model constructions never kill a run; (c) the
  headless-matplotlib guard; (d) the DD break-condition logging into
  `geometry_ddar.py`.
- **Adopt as data:** `data/ag4m_problems.txt` (classic problems in the DSL) and
  `outputs/solved/*.log` (problem + winning aux points) as eval fixtures / seed
  set for a suggester.
- **Adopt as design:** the backward-search-from-conclusion strategy and the
  DSL-extension wishlist — both feed our algorithm/roadmap, not code.
- **Already have / upstream:** rules, defs, DD/AR/DDAR engine (identical to
  upstream; same as our starting point).
- **Needs a model / scale:** the whole point of this fork is scaling the DeepMind
  Meliad LM across CPUs/GPUs. It keeps the heavy TF/JAX stack and the original
  checkpoint — so unlike AlphaGeometryRE it gives us **no** model-free-of-TF path;
  its contribution is the *parallel driver*, which is model-agnostic and portable.

## Verbatim-worthy details

**`BeamQueue` (capacity-bounded top-k frontier), `alphageometry.py`:**

```python
class BeamQueue:
    def __init__(self, max_size=512):
        self.queue = []; self.max_size = max_size
    def add(self, node, val):
        if len(self.queue) < self.max_size:
            self.queue.append((val, node)); return
        min_idx, (min_val, _) = min(enumerate(self.queue), key=lambda x: x[1])
        if val > min_val:
            self.queue[min_idx] = (val, node)
```

**Parallel driver core (early terminate on first solve):**

```python
multiprocessing.set_start_method('spawn')          # 'fork' unsafe with CUDA
pool = multiprocessing.Pool(_N_WORKSERS.value)
pool.map(bqsearch_init, range(_N_WORKSERS.value))  # per-worker LM + defs/rules load
...
jobs = [pool.apply_async(bqsearch, (i, si, out_file)) for i, si in enumerate(beam_queue)]
while n_done < len(beam_queue):
    for i, jr in enumerate(jobs):
        if jr and jr.ready():
            n_done += 1; jobs[i] = None
            _, solved, res = jr.get()
            if solved: pool.terminate(); pool.join(); return True
            for node, val in res: new_queue.add(node, val)
```

**Problem DSL (from README) — conclusion predicates supported:** `coll`, `cong`,
`contri`, `cyclic`, `eqangle` (directed/signed!), `eqratio`, `midp`, `para`,
`perp`, `simtri`. Problem = 2 lines (name; definition). Clauses separated by
` ; `, actions within a clause by ` , `, premises/goal by ` ? `; **whitespace- and
trailing-space-sensitive**. Actions live in `defs.txt` (5 lines each, last line =
Python numeric-check hooks). Tip from README: to syntax-check a problem, set a
trivial goal like `cong a b a b` — it proves instantly and emits the diagram.

**Headless guard (`numericals.py`):**
```python
import os
if not os.environ.get("DISPLAY") is None:
    matplotlib.use('TkAgg')
```

**Run knobs (`utils/run.sh`):** `MODEL` (`ddar` | `alphageometry`), `BATCH_SIZE`
(#LM outputs/query), `BEAM_SIZE` (BFS queue size), `DEPTH` (#aux points),
`NWORKERS` (rule of thumb: 128 GB / 16 CPU → `NWORKERS=8, BATCH_SIZE=24`; larger
values risk OOM).
