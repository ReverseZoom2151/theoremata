<div align="center">

# Theoremata

**A graph-first agentic harness for mathematics: falsify before proving, and never believe a proof the kernel didn't check.**

![Rust](https://img.shields.io/badge/Rust-2021-000000?style=flat-square&logo=rust&logoColor=white)
![Python](https://img.shields.io/badge/Python-3.12-3776AB?style=flat-square&logo=python&logoColor=white)
![Lean](https://img.shields.io/badge/Lean-4-4B0082?style=flat-square)
![Rocq](https://img.shields.io/badge/Rocq-Coq-D4A017?style=flat-square)
![Isabelle](https://img.shields.io/badge/Isabelle-HOL-990000?style=flat-square)
![License: MIT](https://img.shields.io/badge/license-MIT-blue?style=flat-square)
![Tests](https://img.shields.io/badge/tests-250%20Rust%20%2B%20876%20Python-2ea44f?style=flat-square)

</div>

Theoremata is an environment for doing mathematics with a language model in the
loop, where the model is never the authority — a formal proof assistant is. A
Rust core drives a typed proof-DAG and orchestration loop; a set of deterministic
Python workers do the mechanical work (falsification, retrieval, grading,
training); and **Lean 4, Rocq (Coq), and Isabelle/HOL each verify proofs live,
on-box, through the same layered gate**. Every claim the system makes is a claim
some kernel re-checked.

It began as a single Lean-only vertical slice and grew, feature by feature, into
a harness that spans three proof assistants, an informal-sketch-to-formal-proof
pipeline, a growing verified-lemma library, and a self-improvement flywheel —
each piece adopted deliberately from the open-source theorem-proving literature
(~50 repositories and 20 papers, mined in `docs/`) and wired around one spine:
**the verifier is ground truth**.

## About

Most of the difficulty in machine mathematics is not generating a proof — it is
*trusting* one. A model that emits fluent Lean can still emit a proof that uses
`sorry`, admits an axiom it shouldn't, or type-checks a statement that quietly
drifted from the theorem you asked about. Theoremata is built around that
problem. Generation is treated as cheap and untrusted; **verification is the
authoritative, non-negotiable step**, and everything else — search, memory,
retrieval, learning, evaluation — hangs off it.

Concretely, that means two commitments run through the whole system:

- **Falsify before you prove.** Before a conjecture is worth proving, the system
  tries to *break* it (an executable counter-search / numeric falsifier). A
  claim that survives falsification is worth formal effort; one that doesn't is
  rejected with a concrete counterexample.
- **A proof is only as good as its kernel re-check.** Compiling is not enough.
  Every accepted proof passes a **3 + 1-layer gate**: compile → audit the axioms
  and oracles it used against a whitelist → re-check it through the kernel →
  scan the source for the escape hatches the first three layers can't see.

The result is a system that is deliberately conservative: it would rather abstain
or report "unverifiable" than certify something it cannot stand behind.

## The verification gate

Every formal system plugs into one uniform gate (`components/prover/formal.rs`),
so "which prover" is a configuration choice, not a rewrite:

| Layer | What it does | Lean | Rocq | Isabelle |
|-------|--------------|------|------|----------|
| **1. Compile** | The proof elaborates and type-checks | `lean` | `coqc` | `isabelle build` |
| **2. Axiom/oracle audit** | No disallowed axioms or oracles were used | `#print axioms` | `Print Assumptions` | `thm_oracles` |
| **3. Kernel re-check** | An independent kernel re-validates the term | `leanchecker` | `coqchk` | clean `isabelle build` |
| **4. Source scan** | No `sorry` / `Admitted` / `-type-in-type` / `Unset Universe Checking` / `bypass_check` — the escape hatches layers 1–3 can miss | ✓ | ✓ | ✓ |

Layer 4 is not optional: several soundness escapes evade both the axiom audit
*and* the kernel re-check, so the source is always scanned. See
[`docs/TRUST_BOUNDARIES.md`](docs/TRUST_BOUNDARIES.md) for the full trust model.

## Features

**Formal verification (live, three systems)**

- Lean 4, Rocq (Coq), and Isabelle/HOL behind one `FormalSystem` /
  `FormalBackend` / `ProofSession` abstraction, each running the full 3 + 1 gate.
- A runner-agnostic exec bridge: any system can run **native, in WSL, or in
  Docker** by flipping a config key (`formal_runners`) — no code change.
- **Portfolio proving**: attempt a conjecture across all three and take whichever
  certifies first.
- **Hammer-assisted proving**: Sledgehammer (Isabelle), CoqHammer (Rocq), and
  aesop (Lean) can *find* a tactic, which is then assembled into a native proof
  and verified through the same gate — no model in the loop.

**Search and proving**

- A Monte-Carlo *Graph* Search driver (transposition table collapses equivalent
  subgoals into one node) with an orchestrated test-time-compute controller that
  spends search budget as a function of goal difficulty.
- Negation-augmented search (a disproof competes for the same budget), AND/OR
  minimax selection, and empirical sampled-action priors.
- An **informal-sketch → autoformalize-holes → splice** pipeline: decompose a
  proof sketch into a sub-DAG of obligations, prove each hole, reassemble — and
  refuse assembly unless every hole closes.
- **Blueprint / paper-scale runs**: drive a whole multi-lemma `leanblueprint`
  `content.tex` end to end, proving dependencies before dependents.

**Memory and reuse**

- A typed proof-DAG (SQLite/WAL) with three-valued taint provenance
  (clean / tainted / self-admitted).
- A **growing verified-lemma library + evolver**: solved sub-lemmas become
  reusable skills, generalized along four axes and admitted only through the gate.
- A **global goal cache** that reuses proofs across runs via subsumption
  (α-renaming + ordered-literal canonicalization), so a proof of a more-general
  goal discharges a more-specific one.

**Retrieval**

- Premise retrieval over each system's corpus (Lean `Loogle`-style, Rocq
  `Search`, Isabelle `find_theorems`) behind one contract.
- A BM25 → dense → LM-reranker cascade.

**Learning (self-improvement flywheel)**

- An expert-iteration loop: generate → verify (the formal gate is the hard-label
  oracle) → collect verified pairs → retrain → repeat.
- A graded meta-verifier reward (`R = R_format · R_score · R_meta`) with
  majority-vote auto-labeling — a *verifier* trained to score rigor, kept
  strictly beneath the formal gate.
- A learned backend selector, a difficulty curriculum + H0 pre-filter, a
  ReProver-style retriever trainer, and a LEAN-GitHub-style corpus pipeline.

**Evaluation**

- A benchmark harness with formalization, natural-language-answer, proof-grading,
  and tactic-reference tracks (IMO-ProofBench / AnswerBench / GradingBench,
  BRIDGE, QuantumLean, and more).
- A marking-scheme-conditioned proof grader (0–7 scale, median-of-N) with full
  calibration metrics (MAE / RMSE / bias / Kendall-τb) — an *advisory* signal
  that never overrides a passed formal check.

**Geometry**

- A sound Euclidean vertical: a numeric falsifier (with concrete
  counterexamples) plus a deductive-closure forward-chainer.
- An algebraic prover via **Wu's method** (characteristic sets, exact rational
  arithmetic) for the ratio/concurrency facts the rule-based chainer can't reach.

**Abstention**

- The verifier can *decline* rather than bluff: a low-confidence result is a
  first-class `Abstained` terminal state, and conditional accuracy excludes
  abstentions — reliability over coverage.

## Verified end to end

Nothing here is a mock-only claim. On a configured machine:

- Each of Lean, Rocq, and Isabelle **certifies a trivial proof and rejects the
  `sorry` / `Admitted` variant** through the live gate.
- `portfolio-prove "True"` races all three and returns the winner.
- Sledgehammer **found `by simp` for `1 + 1 = (2::nat)`**, which was assembled and
  verified live through Isabelle — no model involved.
- `blueprint-run` drove a three-item `base → middle → top` blueprint to **3/3
  proved** in dependency order, each item certified by the live Lean gate.

The Rust core carries **250 tests** and the Python workers **876**; both suites
are green.

## Building

Theoremata is a Rust binary plus a Python worker package. The Rust side builds
and tests with no external services; the formal backends are optional and detected
at runtime.

```sh
# Rust core
cargo build --release
cargo test                     # 250 tests, no provers required (mock-backed)

# Python workers (editable install of the namespace package)
pip install -e .
python -m pytest components     # 876 tests, offline/deterministic
```

Then initialize a workspace and check what's available on your machine:

```sh
./target/release/theoremata init
./target/release/theoremata doctor    # probes Lean/Rocq/Isabelle, Python, model provider
```

`doctor` is honest about your environment: it reports which formal backends are
live, whether the Python worker is reachable, and whether a model provider is
configured. Everything that isn't available degrades gracefully rather than
failing.

### Formal backends (optional)

Each system is resolved through a configurable runner (native / WSL / Docker).
Point the binaries at your install via config or environment
(`THEOREMATA_LEAN`, `THEOREMATA_COQC`, `THEOREMATA_COQCHK`, `THEOREMATA_ISABELLE`)
— the defaults are the bare names on `PATH`. A backend that isn't present is
simply skipped by the portfolio; it never blocks a run.

### Model provider (optional)

The model seam is provider-agnostic (LiteLLM under the hood), so any chat model
works. Without one, the system runs in a deterministic mock mode — useful for
tests and for exercising the whole pipeline offline.

```sh
source scripts/use-model.sh    # sets THEOREMATA_MODEL_COMMAND for real-model runs
```

## The CLI

`theoremata <command>`. The surface is broad; the commands you'll reach for
first:

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

# Falsify-before-prove, and the agent loop
theoremata falsify <project> "the conjecture"
theoremata agent <project>                 # the autonomous loop

# Any Python worker tool, directly
theoremata tool '{"tool":"benchmark","request":{"op":"list"}}'
```

Run `theoremata help` (or `theoremata <command> --help`) for the full list —
projects, graph editing, retrieval, evaluation, training exports, proof-job
management, and the interactive chat/TUI are all there.

## Architecture

The layout is **component-first**: each component owns its Rust, Python, and
formal-system query templates together, and the Rust crate mounts them into a flat
namespace. The physical structure is by component; the code namespace is flat.

```
theoremata/
├── app/                     # the binary: CLI, config, TUI, main loop
├── components/
│   ├── graph/               # proof-DAG store (SQLite/WAL), typed nodes/edges, scheduler
│   ├── reason/              # the reasoning core
│   │   ├── orchestration/   #   agent loop, certification gate, blueprint-run
│   │   ├── search/          #   MCGS driver, TTC, subsumption, goal cache, fitness
│   │   ├── proving/         #   sketch pipeline, lemma library + evolver, optimizer, portfolio
│   │   └── critique/        #   critic + meta-verification, taint, plan history, memory facade
│   ├── prover/              # FormalSystem abstraction + Lean/Rocq/Isabelle backends + exec bridge
│   ├── provider/            # model provider seam (LiteLLM), mock mode
│   ├── verify/              # the 3+1 gate internals, hardening, per-system templates
│   ├── retrieval/           # BM25/dense/reranker cascade + per-system query templates
│   ├── eval/                # benchmark harness, graders, ProofGrader + calibration
│   ├── train/               # flywheel, meta-verifier reward, selectors, corpus pipeline
│   └── tools/               # the Python worker dispatch (58 tools) + math tools
├── docs/
│   ├── formal-systems/      # Lean/Rocq/Isabelle integration references + trust boundaries
│   ├── resource-mining/     # per-repo mining reports (~50 repos) + adopt list
│   ├── paper-mining/        # per-paper mining reports (20 papers) + synthesis
│   └── PLAN.md
├── scripts/use-model.sh     # wire a real model provider
├── Cargo.toml · pyproject.toml · conftest.py
```

The Rust core talks to the Python workers over a simple JSON-lines protocol
(`components/tools/python/theoremata_tools/worker.py`), so the deterministic math
— falsification, symbolic work, retrieval, grading, training — stays in Python
while orchestration, the proof-DAG, and verification stay in Rust.

## What's live vs. scaffolded

In the spirit of not overclaiming, here is the honest line:

- **Live and verified on-box:** the full 3 + 1 gate for all three provers;
  portfolio proving; Sledgehammer/CoqHammer-assisted proving; the sketch and
  blueprint-run pipelines; falsification; retrieval; grading; the proof-DAG and
  memory; the whole test suite.
- **Runnable but gated / scaffolded:** GPU training runs (the flywheel, reward
  models, and selectors run offline in deterministic mode; real weight training
  needs a GPU); live goal-state extraction for chain-of-states retries; a live
  real-model end-to-end run (needs your API key); `aesop`-live (needs a Mathlib
  Lake project); and the benchmark loaders that ship without vendored corpora
  (they degrade gracefully to empty).

A harness can supply the environment, the verification, and the orchestration; it
cannot supply a trained model or real mileage on hard theorems. Those are the two
honest frontiers that remain.

## Credits and licence

Theoremata is inspired by a large body of open-source theorem-proving work —
LeanDojo/ReProver, LeanParanoia, Aristotle, AlphaProof/Nexus, DeepSeekMath-V2,
LEGO-Prover, ImProver, PROOFGRADER, AlphaGeometry, and many others. Each adopted
idea is traced to its source in
[`docs/resource-mining/`](docs/resource-mining/) (repositories) and
[`docs/paper-mining/`](docs/paper-mining/) (papers), where the specific mechanism
and how it was used here are documented paper by paper.

The project's own code is under the MIT licence. Vendored resources and the
formal-proof assistants keep their own licences.
