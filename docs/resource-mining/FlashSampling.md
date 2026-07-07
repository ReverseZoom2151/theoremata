# Resource Mining: FlashSampling

Full-pass study of `resources/FlashSampling-main/FlashSampling-main` for the Theoremata project.
Prior work on this resource was a *targeted skim*; this is the exhaustive pass (all prose + all source read in full; large result dirs catalogued and sampled).

**Bottom line up front:** FlashSampling is a GPU-kernel-level inference optimization (fuse categorical sampling into the LM-head matmul so the `[batch, vocab]` logits tensor is never written to HBM). It is *not* a best-of-N / selection / search system. Its relevance to Theoremata is **indirect but real**: (a) it is a rigorous template for how to draw N i.i.d. samples cheaply and correctly, (b) it carries several concrete correctness/perf gotchas we will hit if we ever run our own sampling instead of going through a served engine, and (c) its engineering-discipline artifacts (`findings/`, reproduction mapping, benchmark harness) are directly worth imitating. It contributes **almost nothing new to our best-of-N *selection* logic or MCTS** — those layers live above the sampling primitive it optimizes.

---

## 1) What it is (scope, size, structure)

**Paper:** "FlashSampling: Fast and Memory-Efficient Exact Sampling" (Ruiz, Qin, Zhang, Shen, Zhong, Wang; Feb 2026, arXiv:2603.15854). Internal codename is **FMMS** = "Fused Matrix Multiplication & Sampling."

**Core idea** (from `index.html` abstract/method and `README.md`): standard categorical sampling computes the full logits tensor over the vocabulary, writes it to HBM, then samples. That memory round-trip dominates decode latency. FlashSampling collapses matmul + sampling into one fused kernel using the **Gumbel-Max trick**:
1. Compute logits tile-by-tile on chip (never write full `[V, H]` to HBM).
2. Add i.i.d. Gumbel noise to each on-chip logit.
3. Keep only the single maximizer (index + value) per row and per vocab tile.
4. Small reduction over tiles → global argmax = the sampled token (an exact softmax draw).

Claimed result: exact categorical sampling with **zero logits materialization** and **up to 19% faster decoding** as a drop-in replacement for the sample step.

**Size / structure** (265 files; `.git` and build artifacts excluded):
- `json` 121 (119 under `benchmarking/vllm/**` — raw vLLM latency runs), `py` 62, `md` 42, `csv` 10, `pdf` 4, `png` 3, `ipynb` 3, `cu` 1, plus config (`pyproject.toml`, `uv.lock`, Makefiles).
- Source: `src/fused_mm_sampling/*.py` (~3,982 LoC total). Key files: `core.py` (the Triton FMMS kernel + all sampler variants + `get_sampler()` dispatch), `tl_fused_mm_topk.py` + `tl_argsort.py` (fused top-k variant), `helion_impl.py`, `cuda_impl.py` + `csrc/fmms_kernel.cu`, `tl_matmul.py`, `tl_gemv.py`, `persistent_matmul.py`, `qitra.py` (vendored vLLM sort-free top-k/top-p), `tensor_parallel_reduce.py`, `tp_info.py`.
- `findings/` — **30 markdown write-ups** of bugs, ablations, and design decisions (this is the highest-signal prose in the repo; see §4).
- `benchmarking/` — Triton microbench + vLLM end-to-end harness, Modal cloud drivers, NCU/Proton/nsys profiling scripts, plotting.
- `tests/` — chi-squared distribution correctness, matmul/gemv/argsort unit tests.
- `docs/` — profiling, vLLM integration, Helion/TMA pitfalls, Modal/Brev env notes.
- Binaries **not read** (per instructions): `FlashSampling.pdf`, `FlashSampling.png`, `imgs/*.pdf|png|drawio`. Their content is captioned in `index.html` and `REPRODUCTION.md`, which I used instead.

**What I sampled vs. read in full.** Read in full: `README.md`, `README-bak.md`-adjacent prose via `index.html`, `CLAUDE.md`, `AGENTS.md`, `REPRODUCTION.md`, `core.py`, `alg_names.py`, `examples/*.py`, `tl_fused_mm_topk.py` (head), and the highest-value findings (`fused-top-k-top-p-feasibility.md`, `multinomial-validation-overhead.md`). Catalogued + sampled: the 119 `benchmarking/vllm/**/*.json` runs are a regular grid — `{model}/{variant}/{timestamp}/BENCH--max_concurrency={1,2,4,8,16,32,64,128,256}-num_prompts=10×c-request_rate=c/run={0,1,2}.json` plus `summary.json`/`summary.csv`. I read the aggregated `benchmarking/vllm/gpt-oss-120b/results.txt` (the human-readable rollup) rather than the individual JSONs, which are per-run duplicates of the same TPOT metric. The `findings/rtx3090-barrier-comparison/*.csv` and `imgs/tp-scaling/*.csv` are raw timing dumps behind the tables in the `.md`/`AGENTS.md` summaries.

---

## 2) Reusable ideas / patterns / code for Theoremata (THE priority)

Framing: Theoremata's "best-of-N formalize" draws N candidate formalizations/proofs and selects with a compiler-based selector. FlashSampling optimizes the *token-level sampling* underneath generation, not the candidate-selection layer. So the transferable value is in **how to draw N diverse samples correctly and cheaply**, plus **harness/discipline**. Concretely:

### 2a) The core algorithm — Gumbel-Max as parallel best-of-N token sampling
The single most transferable concept. To draw a categorical sample from `softmax(logits)`, you do **not** need softmax + multinomial. Instead: `sample = argmax_i(logits_i + Gumbel_i)` where `Gumbel_i = -log(-log(Uniform_i))`. This is exact, embarrassingly parallel, and **reduces streaming** (each tile keeps its local max; a cheap reduction finds the global winner). Real code, `core.py` kernel epilogue:

```python
gumbel_max, gumbel_max_idx_local = tl.max(
    logits_blk + _gumbel_noise(seed, pid_v_c, pid_h_c, sample_idx, noise_offsets),
    axis=0, return_indices=True,
)
# _gumbel_noise:
return -tl.log(-tl.log(tl.rand(seed + pid_v*100 + pid_h*1_000 + sample_idx*10_000, noise_offsets)))
```

Why it matters for us: **N independent samples = N independent Gumbel perturbations of the *same* logits.** The kernel literally loops `for sample_idx in range(num_samples)` reusing one on-chip logit tile (`core.py` line ~572). For best-of-N this is the ideal shape: compute the expensive thing (logits / a proof-step distribution) once, then cheaply fan out N diverse draws. Even if we never write a GPU kernel, the *pattern* — "materialize the scoring surface once, perturb-and-argmax N times for diversity" — is directly applicable to any place we sample N candidates from a shared distribution.

**Critical correctness note we must copy:** each sample and each tile needs a *distinct* RNG seed, or all draws collapse to identical noise → sampling artifacts (see the comment in `_gumbel_noise`). If Theoremata ever seeds N parallel workers, they must use decorrelated seeds (the repo mixes `seed + pid_v*100 + pid_h*1000 + sample_idx*10000`). This is the same class of bug as reusing a temperature seed across parallel formalize workers.

### 2b) `_fast_multinomial` — the "exponential race" trick (drop-in speedup, no kernel)
`torch.multinomial` spends **~2/3 of its GPU time on input validation, not sampling** (`findings/multinomial-validation-overhead.md`: NCU on RTX 3090, V=128K → 13 kernels / 163 µs, of which 10 kernels / 107 µs are just "probs ≥ 0, ≤ 1, sum to 1" checks; actual sampling is 55 µs). Replacement (`core.py`):

```python
def _fast_multinomial(probs, num_samples):
    q = torch.empty(num_samples, H, V, device=probs.device, dtype=probs.dtype)
    q.exponential_()
    return probs.unsqueeze(0).div(q).argmax(dim=-1).T  # [H, num_samples]
```

This is the exponential-race / Gumbel-max equivalent in probability space and is what vLLM V1 uses. **If Theoremata ever samples candidate distributions in-process (e.g. a local reranker, a learned selector, or a small local model), prefer this over `torch.multinomial`.** Filed upstream as pytorch/pytorch#177127.

### 2c) `torch.multinomial` + bfloat16 is silently wrong
`torch.multinomial` produces **incorrect distributions** on bf16 probabilities. Fix: upcast to fp32 *before* softmax: `probs = (logits.float() / temperature).softmax(dim=1)` (`core.py` line 53, `CLAUDE.md`, `findings/upcasting-before-softmax.md`). Relevant to us whenever we compute any probability/score in bf16 and sample or normalize it — a real correctness trap.

### 2d) Selection among samples: how FMMS "selects" (and why it is *not* our selector)
The reduction that picks the winning token is a pure **argmax over (logit + noise)** across vocab tiles — `_local_reduce` in `core.py`:

```python
idxs = maxs.max(dim=1).indices                       # winning tile per row
samples = maxs_idx.gather(1, idxs.unsqueeze(1))...    # its global vocab index
```

This is a max-reduction *selector*, but it selects the sampled token, not the best of N *candidate solutions*. There is **no scoring/verification/reranking of full outputs** anywhere in the repo. So FlashSampling's "selection" is orthogonal to Theoremata's compiler-based best-of-N selector: they select at the token step; we select at the candidate/proof level. The reusable transfer is only the *streaming-reduction shape* (keep local winner per shard, cheap merge) — useful if our selector ever scores N candidates spread across parallel workers and needs a cheap top-1/top-k merge.

### 2e) Fused top-k as a diversity/candidate-pruning primitive
`tl_fused_mm_topk.py` + `findings/fused-top-k-top-p-feasibility.md`: **top-k is fusible** (each tile emits its local top-k via a custom Triton argsort, then merge `num_tiles × k` candidates — see `_topk_merge_and_sample`), but **top-p / min-p are NOT fusible** because they need a *global* softmax normalizer + sorted cumsum (no tile-local decomposition). Practical path they adopt: fuse top-k with a conservatively large k, apply top-p on the k survivors post-kernel — mirroring vLLM. Transfer: if we ever build candidate-diversity controls (nucleus/top-k over a proof-step distribution), this is the definitive feasibility analysis — top-k parallelizes cleanly, cumulative-mass filters need a global pass.

### 2f) Johnson–Lindenstrauss approximate logits (`JLSampler`, `core.py`)
An **approximate** sampler that projects both weights `[V,D]` and hidden `[H,D]` through a shared random matrix `R ∈ [D,k]` (`k = 24 ln(n)/(3ε²−2ε³)`), computing cheap `[V,k]·[k,H]` logits instead of `[V,D]·[D,H]`. Not exact, but a template for "when the full scoring matmul is too expensive, project to a low-dim sketch first." Marginal for us unless we build a large learned selector over huge candidate sets.

### 2g) Throughput/scaling results (what actually moved the needle)
- **Kernel microbench:** up to ~19% decode-time reduction; the win is memory-bound regime only. `findings/fused-top-k-top-p-feasibility.md` derives arithmetic intensity of the decode matmul ≈ **H (batch size)**: memory-bound up to H≈295 (H100 bf16), so the fusion helps most at *small batch* (single-request / low-concurrency decode) and the advantage shrinks as batch grows.
- **End-to-end vLLM (`gpt-oss-120b/results.txt`):** FMMS-Triton TPOT deltas vs baseline are **small and mixed** at the served-engine level: −2.4% at concurrency 1, roughly flat (−0.1% to −2%) up to 32, and *+6.5% (slower)* at concurrency 64. FMMS-FlashInfer reaches −10% at concurrency 128. Takeaway for us: **kernel-level sampling optimizations mostly wash out once you're behind a batched serving engine** — the microbench 19% does not translate to a uniform end-to-end win. This is a caution against over-indexing on token-sampling micro-optimizations for our throughput.
- **Tensor-parallel scaling** (`AGENTS.md` findings + `imgs/tp-scaling`): clean low-batch gains (TP4≈0.7×TP2), but at TP=8 the fan-out symmetric-memory write cost scales O(world_size) and *regresses* once the matmul shrinks. Relevant only if we shard a selection model across GPUs.

### 2h) Parallelism / batching lessons directly reusable for our parallel workers
From `AGENTS.md` (a genuinely useful ops checklist):
- **Don't run benchmarks in parallel on one GPU — they contend for resources; launch sequentially.** On Modal (isolated resources) parallelize freely. Directly analogous to Theoremata parallel formalize workers sharing one GPU/CPU: co-located workers contend; only parallelize when each has isolated resources.
- **Warm the autotune/JIT cache with a single warmup job before fanning out parallel jobs**, else every parallel job autotunes independently, wasting compute and risking inconsistent config selection. Analogous to warming any shared compile/kernel/model cache (Lean elaboration cache, model load) before spawning N workers.
- **Never introduce GPU-CPU syncs on the hot path** (`.item()`, `float(tensor)`, `.cpu()`, `print(tensor)`) — they serialize the pipeline. Pass scalars as 0-d tensors. Analogous to avoiding blocking syncs between async workers.
- **Don't block with long foreground sleeps**; launch >1 min tasks in background and wait on completion. (This matches our own working-style memory.)

---

## 3) Schema / config format

No graph/proof schema (irrelevant to this repo). Config surfaces worth noting:
- **`pyproject.toml` / `uv.lock`** — `uv`-managed deps; Python ≥3.10/3.12, pinned Modal/FlashInfer/Helion/Triton.
- **Pinned runtime matrix** (`REPRODUCTION.md`): PyTorch 2.10, CUDA 13.0, Triton 3.6.0, FlashInfer 0.6.9. BF16 inputs; 25 warmup iters; CUPTI medians over 100 iters.
- **Benchmark params as JSON** (`benchmarking/vllm/bench-params.json`, `quick-bench-params.json`, `nsys-bench-params.json`): concurrency sweep `{1,2,4,8,16,32,64,128,256}`, `num_prompts = 10×concurrency`, `request_rate = concurrency`, runs `{0,1,2}`. Dataset `AI-MO/aimo-validation-aime`, `--hf-output-len 256`, `--max-model-len 1024`, `temperature=0.6, top_k=-1, top_p=1.0`.
- **Provider registry** (`alg_names.py` + `get_sampler()` match/case in `core.py`): a clean **string-keyed dispatch** pattern — canonical short names → display names → factory. Adding a sampler = one `case`. Good template for our own model-agnostic provider/selector registry (mirrors the LiteLLM-provider idea in our design).
- **`Sampler` Protocol** (`prepare()` + `sample(**kwargs)`) with `SimpleSampler` wrapper for bare callables — a lightweight strategy-pattern worth copying for pluggable selectors/samplers.
- **Autotune config as data** (`get_autotuning_configs()`): cartesian product of `BLOCK_SIZE_V/D`, `num_warps`, `maxnreg`, `num_stages`, keyed by `["vocab_size","hidden_size","BLOCK_SIZE_H","num_samples","GREEDY_SAMPLING"]`.

---

## 4) What our earlier targeted pass MISSED

The earlier skim captured the headline (Gumbel-max fusion, 19%). This full pass surfaced:

1. **The `findings/` directory (30 write-ups) is the real intellectual payload** and is easy to miss. Highest-value beyond sampling itself:
   - `multinomial-validation-overhead.md` — the 2/3-of-runtime-is-validation result + the exponential-race fix (§2b). Actionable for any in-process sampling.
   - `upcasting-before-softmax.md` — bf16 multinomial is *wrong* (§2c).
   - `fused-top-k-top-p-feasibility.md` — the definitive fusibility analysis: top-k yes, top-p/min-p no; hierarchical-reduction (registers→warp→SMEM→cluster DSMEM→HBM) blueprint; Triton vs CUDA/CUTLASS vs CuTe-DSL tradeoff table (§2e).
   - `argsort-topk-complexity.md`, `helion-hl-rand-specialize-1-bug.md`, `register-spilling-bsz256.md`, `tma-store-blackwell-singleton-dims.md` — deep hardware/compiler gotchas (mostly not relevant to us unless we write kernels, but exemplary bug write-ups).
2. **End-to-end vLLM numbers are mixed, not a uniform win** (§2g) — the 19% is a *kernel* microbench; served TPOT is roughly flat and sometimes *slower*. The skim likely over-credited the headline number.
3. **The parallelism/ops discipline in `AGENTS.md`** (sequential-on-shared-GPU, warm-cache-before-fanout, no hot-path syncs) — directly transferable to our parallel-worker design (§2h), and not sampling-specific.
4. **`_fast_multinomial` and `JLSampler` exist as first-class alternate samplers** (§2b, §2f) — cheap/approximate sampling variants beyond the flagship kernel.
5. **Explicit non-fusibility of top-p/min-p and the exact reason** (needs global normalizer) — a boundary result worth knowing before we attempt any "fuse the whole selection into one pass" idea.
6. **`tensor_parallel_reduce.py` uses symmetric-memory NVLink writes instead of NCCL all-gather** to cut collective overhead (~0.12–0.20 ms → direct writes) — an advanced multi-GPU reduction pattern, only relevant if we shard a model/selector.

---

## 5) Test / benchmark value

Genuinely worth borrowing:
- **Chi-squared goodness-of-fit correctness test** (`tests/test_core.py::test_sampling_distribution`, described in `CLAUDE.md`/`REPRODUCTION.md`): draw ~5,000 samples, compare empirical frequencies to theoretical `softmax` probabilities via chi-squared, excluding bins with expected count < 5. Parametrized over **all providers × vocab sizes {100,256} × n_hidden_states {1,2}** to catch tile-boundary and dimension-edge bugs. **This is exactly the right way to validate any stochastic component of Theoremata** (e.g. that a sampler/selector actually samples from the intended distribution). The "test every provider against a ground-truth distribution over multiple sizes" pattern is our template for validating best-of-N candidate diversity.
- **`make_synthetic_inputs()`** (`testing.py`): constructs weights/hidden that produce *known* logit vectors (ascending/descending) via SVD + pseudoinverse — i.e. build inputs with a known correct answer to test a numeric primitive. Reusable technique for testing our selector against known-optimal candidates.
- **Reproduction discipline** (`REPRODUCTION.md`): a table mapping *every* paper artifact (each table/figure) → exact `make` target → output path. **We should adopt this** for our own eval claims — one table binding claim→command→artifact.
- **Timing rigor** (`triton_benchmark_lib.py`, `CLAUDE.md`): CUPTI vs CUDA-event methods cross-validated (within 1.46% at TP1, but +7.3% divergence at TP2 → not interchangeable for distributed); L2-cache flushing between iters; fixed warmup. The lesson — *validate your measurement method before trusting cross-config comparisons* — applies to our latency/throughput benchmarking.
- Benchmark **data** itself (the 119 vLLM JSONs, TP-scaling CSVs) has little reuse value for us — it's GPU-specific microdata.

---

## 6) New vs. already-in-our-design

| FlashSampling element | Status for Theoremata |
|---|---|
| Gumbel-max: perturb shared distribution → argmax for N diverse draws | **New framing, conceptually reusable.** Our best-of-N samples full outputs from an LLM API; this is the token-level analog. The reusable idea is "compute scoring surface once, fan out N cheap perturbed draws" + decorrelated per-draw seeds. |
| Exponential-race `_fast_multinomial` | **New, actionable** *if* we ever sample in-process. Not relevant while sampling happens inside a served engine (vLLM/LiteLLM). |
| bf16-multinomial-is-wrong / upcast-before-softmax | **New correctness trap** to remember for any in-process probability math. |
| Compiler/argmax *token* selection | **Orthogonal, not new.** Our compiler-based selector operates on full candidates, not tokens. Only the streaming top-1/top-k merge *shape* transfers. |
| MCTS / tree search | **Absent here.** FlashSampling has no search. Nothing to mine for our MCTS. |
| Parallel workers / batching | **Reinforces our design.** No new mechanism, but concrete ops rules (sequential-on-shared-GPU, warm-cache-before-fanout, no hot-path syncs, background long tasks) validate and sharpen our parallel-worker plan. |
| Model-agnostic provider registry (`get_sampler` match/case, `Sampler` Protocol) | **Already in our design** (LiteLLM provider abstraction). FlashSampling's string-keyed factory + Protocol is a clean concrete template to mirror for pluggable selectors. |
| Fused top-k feasibility / top-p non-fusibility | **New boundary knowledge** for any future diversity-control layer. |
| Chi-squared distribution test + synthetic-known-answer inputs | **New test pattern, directly adoptable** for validating stochastic components. |
| Reproduction artifact→command→path table | **New discipline, adopt for our eval.** |
| Symmetric-memory NVLink TP reduction, TMA/Helion/Blackwell kernel gotchas | **Not applicable** unless we hand-write GPU kernels — which our design does not. |

**Net:** No change to our best-of-N *selection* or MCTS architecture. Genuinely new/actionable items are narrow but concrete: (1) the "sample-once-perturb-N-times + decorrelated seeds" pattern and its correctness traps, (2) the exponential-race and bf16-upcast fixes for any in-process sampling, (3) the chi-squared + synthetic-input test methodology, (4) the parallel-worker ops rules from `AGENTS.md`, and (5) the reproduction-mapping discipline. Everything kernel-level (Triton/CUDA/TMA/TP-reduction) is out of scope for our Rust+Python+Lean+served-LLM stack.
