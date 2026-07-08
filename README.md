<div align="center">

# Theoremata

**A graph-first agentic harness for mathematics. It falsifies conjectures before proving them and re-checks every proof through a kernel.**

![Rust](https://img.shields.io/badge/Rust-2021-000000?style=flat-square&logo=rust&logoColor=white)
![Python](https://img.shields.io/badge/Python-3.12-3776AB?style=flat-square&logo=python&logoColor=white)
![Lean](https://img.shields.io/badge/Lean-4-4B0082?style=flat-square)
![Rocq](https://img.shields.io/badge/Rocq-Coq-D4A017?style=flat-square)
![Isabelle](https://img.shields.io/badge/Isabelle-HOL-990000?style=flat-square)
![License: MIT](https://img.shields.io/badge/license-MIT-blue?style=flat-square)
![Tests](https://img.shields.io/badge/tests-250%20Rust%20%2B%20876%20Python-2ea44f?style=flat-square)

</div>

Theoremata is an environment for doing mathematics with a language model in the
loop. The model proposes; a formal proof assistant decides. A Rust core drives a
typed proof-DAG and an orchestration loop. A set of deterministic Python workers
do the mechanical work: falsification, retrieval, grading, training. Lean 4, Rocq
(Coq), and Isabelle/HOL each verify proofs on-box through the same layered gate.
Every claim the system accepts is one a kernel re-checked.

It started as a Lean-only vertical slice and grew into a harness that spans three
proof assistants, an informal-sketch-to-formal-proof pipeline, a growing
verified-lemma library, and a self-improvement flywheel. Each piece was adopted
from the open-source theorem-proving literature (about 50 repositories and 20
papers, mined under `docs/`) and wired around one rule: the verifier is ground
truth.

## About

The hard part of machine mathematics is not writing a proof. It is trusting one.
A model that emits fluent Lean can still emit a proof that uses `sorry`, admits an
axiom it should not, or type-checks a statement that drifted from the theorem you
asked about. Theoremata treats generation as cheap and untrusted, and makes
verification the authoritative step. Search, memory, retrieval, learning, and
evaluation all hang off that.

Two rules run through the whole system:

- **Falsify before you prove.** Before a conjecture is worth proving, the system
  tries to break it with an executable counter-search or numeric falsifier. A
  claim that survives is worth formal effort. One that does not is rejected with a
  concrete counterexample.
- **A proof is only as good as its kernel re-check.** Compiling is not enough.
  Every accepted proof passes a 3+1-layer gate: compile, audit the axioms and
  oracles it used against a whitelist, re-check it through the kernel, then scan
  the source for the escape hatches the first three layers cannot see.

The system prefers to abstain or report "unverifiable" over certifying something
it cannot stand behind.

## The verification gate

Every formal system plugs into one gate (`components/prover/formal.rs`), so which
prover you use is a configuration choice rather than a rewrite:

| Layer | What it does | Lean | Rocq | Isabelle |
|-------|--------------|------|------|----------|
| **1. Compile** | The proof elaborates and type-checks | `lean` | `coqc` | `isabelle build` |
| **2. Axiom/oracle audit** | No disallowed axioms or oracles were used | `#print axioms` | `Print Assumptions` | `thm_oracles` |
| **3. Kernel re-check** | An independent kernel re-validates the term | `leanchecker` | `coqchk` | clean `isabelle build` |
| **4. Source scan** | No `sorry`, `Admitted`, `-type-in-type`, `Unset Universe Checking`, or `bypass_check` | yes | yes | yes |

Layer 4 is not optional. Several soundness escapes evade both the axiom audit and
the kernel re-check, so the source is always scanned. See
[`docs/TRUST_BOUNDARIES.md`](docs/TRUST_BOUNDARIES.md) for the full trust model.

## Features

**Formal verification (live, three systems)**

- Lean 4, Rocq (Coq), and Isabelle/HOL behind one `FormalSystem` /
  `FormalBackend` / `ProofSession` abstraction, each running the full 3+1 gate.
- A runner-agnostic exec bridge. Any system can run native, in WSL, or in Docker
  by setting a config key (`formal_runners`), with no code change.
- Portfolio proving: attempt a conjecture across all three and take whichever
  certifies first.
- Hammer-assisted proving: Sledgehammer (Isabelle), CoqHammer (Rocq), and aesop
  (Lean) can find a tactic, which is then assembled into a native proof and
  verified through the same gate, with no model in the loop.

**Search and proving**

- A Monte-Carlo graph search driver whose transposition table collapses
  equivalent subgoals into one node, plus a test-time-compute controller that
  spends search budget as a function of goal difficulty.
- Negation-augmented search where a disproof competes for the same budget, AND/OR
  minimax selection, and empirical sampled-action priors.
- An informal-sketch to autoformalize-holes to splice pipeline: decompose a proof
  sketch into a sub-DAG of obligations, prove each hole, reassemble, and refuse
  assembly unless every hole closes.
- Blueprint and paper-scale runs: drive a whole multi-lemma `leanblueprint`
  `content.tex` end to end, proving dependencies before dependents.

**Memory and reuse**

- A typed proof-DAG (SQLite/WAL) with three-valued taint provenance: clean,
  tainted, self-admitted.
- A growing verified-lemma library with an evolver. Solved sub-lemmas become
  reusable skills, generalized along four axes and admitted only through the gate.
- A global goal cache that reuses proofs across runs via subsumption (α-renaming
  plus ordered-literal canonicalization), so a proof of a more-general goal
  discharges a more-specific one.

**Retrieval**

- Premise retrieval over each system's corpus (Lean Loogle-style, Rocq `Search`,
  Isabelle `find_theorems`) behind one contract.
- A BM25, dense, and LM-reranker cascade.

**Learning (self-improvement flywheel)**

- An expert-iteration loop: generate, verify (the formal gate is the hard-label
  oracle), collect verified pairs, retrain, repeat.
- A graded meta-verifier reward (`R = R_format · R_score · R_meta`) with
  majority-vote auto-labeling. The verifier is trained to score rigor and sits
  strictly beneath the formal gate.
- A learned backend selector, a difficulty curriculum with an H0 pre-filter, a
  ReProver-style retriever trainer, and a LEAN-GitHub-style corpus pipeline.

**Evaluation**

- A benchmark harness with formalization, natural-language-answer, proof-grading,
  and tactic-reference tracks (IMO-ProofBench, AnswerBench, GradingBench, BRIDGE,
  QuantumLean, and others).
- A marking-scheme-conditioned proof grader (0-7 scale, median-of-N) with
  calibration metrics (MAE, RMSE, bias, Kendall-τb). This grade is advisory and
  never overrides a passed formal check.

**Geometry**

- A sound Euclidean vertical: a numeric falsifier with concrete counterexamples,
  plus a deductive-closure forward-chainer.
- An algebraic prover using Wu's method (characteristic sets, exact rational
  arithmetic) for the ratio and concurrency facts the rule-based chainer cannot
  reach.

**Abstention**

- The verifier can decline rather than guess. A low-confidence result is a
  first-class `Abstained` terminal state, and conditional accuracy excludes
  abstentions, so declining is not scored as a wrong answer.

## Verified on-box

These are things that ran, not mock-only claims. On a configured machine:

- Each of Lean, Rocq, and Isabelle certifies a trivial proof and rejects the
  `sorry` or `Admitted` variant through the live gate.
- `portfolio-prove "True"` races all three and returns the winner.
- Sledgehammer found `by simp` for `1 + 1 = (2::nat)`, which was assembled and
  verified live through Isabelle, with no model involved.
- `blueprint-run` drove a three-item `base -> middle -> top` blueprint to 3/3
  proved in dependency order, each item certified by the live Lean gate.

The Rust core carries 250 tests and the Python workers 876. Both suites pass.

## Tech stack & integrations

<div align="center">
<img src="https://cdn.simpleicons.org/rust/000000" height="34" alt="Rust">&nbsp;&nbsp;&nbsp;&nbsp;
<img src="https://cdn.simpleicons.org/python/3776AB" height="34" alt="Python">&nbsp;&nbsp;&nbsp;&nbsp;
<img src="assets/logos/lean.png" height="34" alt="Lean 4">&nbsp;&nbsp;&nbsp;&nbsp;
<img src="assets/logos/rocq.svg" height="26" alt="Rocq (Coq)">&nbsp;&nbsp;&nbsp;&nbsp;
<img src="https://cdn.simpleicons.org/sqlite/003B57" height="30" alt="SQLite">&nbsp;&nbsp;&nbsp;&nbsp;
<img src="assets/logos/ratatui.svg" height="32" alt="ratatui">&nbsp;&nbsp;&nbsp;&nbsp;
<img src="https://cdn.simpleicons.org/docker/2496ED" height="30" alt="Docker">&nbsp;&nbsp;&nbsp;&nbsp;
<img src="https://cdn.simpleicons.org/ubuntu/E95420" height="30" alt="WSL / Ubuntu">&nbsp;&nbsp;&nbsp;&nbsp;
<img src="https://cdn.simpleicons.org/sympy/3B5526" height="30" alt="SymPy">&nbsp;&nbsp;&nbsp;&nbsp;
<img src="https://cdn.simpleicons.org/scikitlearn/F7931E" height="30" alt="scikit-learn">&nbsp;&nbsp;&nbsp;&nbsp;
<img src="https://cdn.simpleicons.org/pytorch/EE4C2C" height="30" alt="PyTorch">&nbsp;&nbsp;&nbsp;&nbsp;
<img src="https://cdn.simpleicons.org/huggingface/FFD21E" height="30" alt="Hugging Face">
</div>

The rule for external tools is: detect at runtime, degrade gracefully. The only
hard requirements are a Rust toolchain and Python. Every proof assistant, hammer,
model provider, and optional library is probed. If it is present it is used; if it
is absent it is skipped, and `theoremata doctor` reports which is which. Nothing
external blocks a build or a run.

| | Integration | Role | How it's resolved |
|-|-------------|------|-------------------|
| <img src="https://cdn.simpleicons.org/rust/000000" height="16"> | [Rust](https://www.rust-lang.org) 2021 | Core binary: proof-DAG, orchestration, verification gate | Required (`cargo`) |
| <img src="https://cdn.simpleicons.org/sqlite/003B57" height="16"> | [SQLite](https://www.sqlite.org) via [`rusqlite`](https://docs.rs/rusqlite) | The proof-DAG store (WAL) | Bundled, nothing to install |
| <img src="assets/logos/ratatui.svg" height="16"> | [`clap`](https://docs.rs/clap), [`ratatui`](https://ratatui.rs), [`crossterm`](https://docs.rs/crossterm) | CLI and interactive TUI | `cargo` |
| <img src="https://cdn.simpleicons.org/python/3776AB" height="16"> | [Python](https://www.python.org) 3.11+ | Deterministic workers (falsify, retrieval, grading, training) | Required for the tool layer (`pip install -e .`) |
| <img src="assets/logos/lean.png" height="18"> | [Lean 4](https://lean-lang.org) with [Mathlib](https://github.com/leanprover-community/mathlib4) | Formal backend and premise corpus | Detected at runtime, optional |
| <img src="assets/logos/rocq.svg" height="14"> | [Rocq (Coq)](https://rocq-prover.org) | Formal backend | Detected at runtime, optional |
| <img src="https://img.shields.io/badge/Isabelle-HOL-990000?style=flat-square" height="16"> | [Isabelle/HOL](https://isabelle.in.tum.de) | Formal backend | Detected at runtime, optional |
| | Sledgehammer, [CoqHammer](https://coqhammer.github.io), [aesop](https://github.com/leanprover-community/aesop) | Hammers: find a tactic, verify it through the gate | Ships with its prover, optional |
| <img src="https://cdn.simpleicons.org/docker/2496ED" height="16"> <img src="https://cdn.simpleicons.org/ubuntu/E95420" height="16"> | Docker and WSL | Runners: run any backend native, in WSL, or in a container | Config (`formal_runners`), optional |
| | [LiteLLM](https://github.com/BerriAI/litellm) | Model provider seam (any chat model) | Optional, a deterministic mock runs without it |
| <img src="https://cdn.simpleicons.org/sympy/3B5526" height="16"> | [SymPy](https://www.sympy.org) | Symbolic math and Wu's-method geometry | Core dependency, with stdlib fallbacks in the hot paths |
| | [`rank_bm25`](https://pypi.org/project/rank-bm25/) | BM25 premise retrieval | Optional, a stdlib BM25 backend runs without it |
| <img src="https://cdn.simpleicons.org/scikitlearn/F7931E" height="16"> | [scikit-learn](https://scikit-learn.org) | Learned backend and difficulty selectors | Optional, deterministic fallbacks otherwise |
| <img src="https://cdn.simpleicons.org/huggingface/FFD21E" height="16"> <img src="https://cdn.simpleicons.org/pytorch/EE4C2C" height="16"> | [PyTorch](https://pytorch.org), [Transformers](https://github.com/huggingface/transformers), [TRL](https://github.com/huggingface/trl) | Retriever, reward, and SFT/GRPO training | Optional, GPU only for real training runs |

## Building

Theoremata is a Rust binary plus a Python worker package. The Rust side builds and
tests with no external services. The formal backends are optional and detected at
runtime.

```sh
# Rust core
cargo build --release
cargo test                     # 250 tests, no provers required (mock-backed)

# Python workers (editable install of the namespace package)
pip install -e .
python -m pytest components     # 876 tests, offline and deterministic
```

Then initialize a workspace and check what is available on your machine:

```sh
./target/release/theoremata init
./target/release/theoremata doctor    # probes Lean/Rocq/Isabelle, Python, model provider
```

`doctor` reports your environment: which formal backends are live, whether the
Python worker is reachable, and whether a model provider is configured. Whatever
is not available is skipped rather than fatal.

### Formal backends (optional)

Each system is resolved through a configurable runner (native, WSL, or Docker).
Point the binaries at your install via config or environment (`THEOREMATA_LEAN`,
`THEOREMATA_COQC`, `THEOREMATA_COQCHK`, `THEOREMATA_ISABELLE`). The defaults are
the bare names on `PATH`. A backend that is not present is skipped by the
portfolio and never blocks a run.

### Model provider (optional)

The model seam is provider-agnostic (LiteLLM underneath), so any chat model works.
Without one, the system runs in a deterministic mock mode, which is useful for
tests and for exercising the whole pipeline offline.

```sh
source scripts/use-model.sh    # sets THEOREMATA_MODEL_COMMAND for real-model runs
```

## The CLI

`theoremata <command>`. The surface is broad. The commands to reach for first:

```sh
# Projects and the proof-DAG
theoremata new "my-project" "the theorem statement"
theoremata graph <project>                 # inspect the DAG
theoremata status <project>

# Formal proving
theoremata formal-prove lean "1 + 1 = 2"           # model-driven, gate-verified
theoremata hammer-prove isabelle "1 + 1 = (2::nat)" # hammer finds the tactic
theoremata portfolio-prove "True"                   # race Lean/Rocq/Isabelle

# Blueprint (paper-scale)
theoremata blueprint-import <project> content.tex
theoremata blueprint-run <project> content.tex --systems lean

# Falsify before proving, and the agent loop
theoremata falsify <project> "the conjecture"
theoremata agent <project>                 # the autonomous loop

# Any Python worker tool, directly
theoremata tool '{"tool":"benchmark","request":{"op":"list"}}'
```

Run `theoremata help` (or `theoremata <command> --help`) for the full list.
Projects, graph editing, retrieval, evaluation, training exports, proof-job
management, and the interactive chat/TUI are all there.

## Architecture

The layout is component-first. Each component owns its Rust, Python, and
formal-system query templates together, and the Rust crate mounts them into a flat
namespace. The physical structure is by component; the code namespace is flat.

```
theoremata/
├── app/                     # the binary: CLI, config, TUI, main loop
├── components/
│   ├── graph/               # proof-DAG store (SQLite/WAL), typed nodes/edges, scheduler
│   ├── reason/              # the reasoning core
│   │   ├── orchestration/   #   agent loop, certification gate, blueprint-run
│   │   ├── search/          #   graph search, TTC, subsumption, goal cache, fitness
│   │   ├── proving/         #   sketch pipeline, lemma library + evolver, optimizer, portfolio
│   │   └── critique/        #   critic + meta-verification, taint, plan history, memory facade
│   ├── prover/              # FormalSystem abstraction + Lean/Rocq/Isabelle backends + exec bridge
│   ├── provider/            # model provider seam (LiteLLM), mock mode
│   ├── verify/              # the 3+1 gate internals, hardening, per-system templates
│   ├── retrieval/           # BM25/dense/reranker cascade + per-system query templates
│   ├── eval/                # benchmark harness, graders, proof grader + calibration
│   ├── train/               # flywheel, meta-verifier reward, selectors, corpus pipeline
│   └── tools/               # the Python worker dispatch (58 tools) + math tools
├── docs/
│   ├── formal-systems/      # Lean/Rocq/Isabelle integration references + trust boundaries
│   ├── resource-mining/     # per-repo mining reports (about 50 repos) + adopt list
│   ├── paper-mining/        # per-paper mining reports (20 papers) + synthesis
│   └── PLAN.md
├── scripts/use-model.sh     # wire a real model provider
├── Cargo.toml · pyproject.toml · conftest.py
```

The Rust core talks to the Python workers over a JSON-lines protocol
(`components/tools/python/theoremata_tools/worker.py`). The deterministic math,
namely falsification, symbolic work, retrieval, grading, and training, stays in
Python. Orchestration, the proof-DAG, and verification stay in Rust.

## What is live and what is scaffolded

- Live and verified on-box: the full 3+1 gate for all three provers, portfolio
  proving, Sledgehammer and CoqHammer-assisted proving, the sketch and
  blueprint-run pipelines, falsification, retrieval, grading, the proof-DAG and
  memory, and the whole test suite.
- Runnable but gated or scaffolded: GPU training runs (the flywheel, reward
  models, and selectors run offline in deterministic mode; real weight training
  needs a GPU), live goal-state extraction for chain-of-states retries, a live
  real-model end-to-end run (needs an API key), aesop-live (needs a Mathlib Lake
  project), and the benchmark loaders that ship without vendored corpora (they
  return empty rather than fail).

The harness supplies the environment, the verification, and the orchestration. A
trained model and real mileage on hard theorems are the two pieces it does not
supply on its own.

## Credits and licence

Theoremata draws on open-source theorem-proving work, including LeanDojo and
ReProver, LeanParanoia, Aristotle, AlphaProof and Nexus, DeepSeekMath-V2,
LEGO-Prover, ImProver, PROOFGRADER, and AlphaGeometry, among others. Each adopted
idea is traced to its source in
[`docs/resource-mining/`](docs/resource-mining/) (repositories) and
[`docs/paper-mining/`](docs/paper-mining/) (papers), where the mechanism and its
use here are documented paper by paper.

The project's own code is under the MIT licence. Vendored resources and the formal
proof assistants keep their own licences. The Lean, Rocq, and ratatui logos in
`assets/logos/` are the property of their respective projects and are used here to
identify and link those projects.
