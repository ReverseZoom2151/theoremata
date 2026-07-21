<div align="center">

# Theoremata

### an AI mathematician

An autonomous agent that takes a mathematical conjecture, proves it inside a formal proof
assistant, and returns a machine-checkable certificate of the result.

![Rust](https://img.shields.io/badge/Rust-2021-000000?style=flat-square&logo=rust&logoColor=white)
![Python](https://img.shields.io/badge/Python-3.12-3776AB?style=flat-square&logo=python&logoColor=white)
![Lean](https://img.shields.io/badge/Lean-4-4B0082?style=flat-square)
![Rocq](https://img.shields.io/badge/Rocq-Coq-D4A017?style=flat-square)
![Isabelle](https://img.shields.io/badge/Isabelle-HOL-990000?style=flat-square)
![Candle](https://img.shields.io/badge/Candle-CakeML-8B0000?style=flat-square)
![Agda](https://img.shields.io/badge/Agda-2-2C6BAA?style=flat-square)
![Metamath](https://img.shields.io/badge/Metamath-set.mm-444444?style=flat-square)
![License: MIT](https://img.shields.io/badge/license-MIT-blue?style=flat-square)

[Watch it run](#watch-it-run) • [The gate](#the-gate) • [Proof it bites](#proof-it-bites) • [Certificates](#certificates) • [Install](#install) • [CLI](#cli)

<!-- Demo GIF: run `vhs demo/theoremata.tape` to generate assets/demo.gif, then
     uncomment the next line. Kept commented so the hero never shows a broken image. -->
<!-- ![Theoremata falsify-then-prove demo](assets/demo.gif) -->

</div>

---

## What it does

**It attacks your conjecture before it believes it.**
Every claim goes to an executable counter-search first. If a counterexample exists you get
it in seconds, instead of a week of proof attempts that were never going to close. Surviving
that is how a conjecture earns real effort.

**It drafts, then makes it rigorous.**
It writes an informal outline the way a person would, marks the steps that are still
hand-waving, and turns each one into a formal obligation it has to discharge. Nothing gets
waved through because the surrounding prose sounded convincing.

**It builds its own toolkit as it works.**
Between attempts it invents auxiliary lemmas, tries to prove or disprove each, and keeps
the survivors in a growing library. The pool it draws on gets richer as it goes, so the
tenth theorem stands on everything it learned from the first nine.

**It scales past one theorem.**
Point it at a paper's blueprint and it works the whole dependency graph, proving lemmas
before the results that lean on them.

**It hands you a receipt, not a claim.**
A proof an independent kernel re-checked, and for many results a certificate a small
separate program re-verifies from the raw numbers in exact arithmetic. You do not have to
trust the model. You do not have to trust us either.

The last one is the whole point. Anything can generate convincing mathematics; the useful
question is whether you can check it without taking anyone's word for it.

## Watch it run

Break it first. A bounded counter-search runs offline in milliseconds, and a false
conjecture never reaches the prover. Give it the variables with a search box and the claim
as an expression:

```console
$ theoremata falsify '{"x":{"start":0,"stop":6},"y":{"start":0,"stop":6}}' "x**2 + y**2 >= 3*x*y"
{ "verdict": "counterexample", "assignment": { "x": 1, "y": 1 }, "checked": 8 }
```

`x=1, y=1` gives `2 >= 3`, which is false. Rejected before a single proof attempt. The true
form survives the same search and earns its shot at a proof:

```console
$ theoremata falsify '{"x":{"start":0,"stop":6},"y":{"start":0,"stop":6}}' "x**2 + y**2 >= 2*x*y"
{ "verdict": "no_counterexample_in_domain", "checked": 36 }
```

Then `theoremata formal-prove <system> "<statement>"` formalizes the survivor with your
configured model and runs it through the gate, returning a report whose fields are the
verdict: `lexically_verified`, `axioms_clean`, `statement_preserved`, and `live`.

> [!NOTE]
> `live: true` is the field that matters. A backend without its toolchain still runs, but
> in mock mode, where `live` is `false` and the result is at most *informal*. A mock check
> can never mark a result formally verified. Only a live prover run can.

## The gate

Here is why the receipt is worth something. A model can emit fluent Lean that leans on
`sorry`, admits an axiom it should not, or type-checks a statement that quietly drifted
from the theorem you asked about. All three compile. So compiling is not the bar.

Six proof assistants, one gate (`components/prover/formal.rs`). Which prover you use is a
config choice, not a rewrite.

```
proof text
    |
    +--  1. compile .......... elaborates and type-checks in the target system
    +--  2. axiom audit ...... every axiom and oracle inside a per-system whitelist
    +--  3. kernel recheck ... an independent kernel re-validates the term
    +--  4. source scan ...... escape hatches, and the statement is the one you asked for
    |
    v
FormallyVerified        or it fails closed, and says which layer said no
```

Layer 4 is not optional. Several soundness escapes evade both the axiom audit and the
kernel re-check, so the source is always scanned and the statement the proof actually
proves is compared against the statement that was requested.

Each backend declares how it signals success, an honestly non-zero exit or a required
stdout sentinel plus a forbidden one, because a checker that reports success by exit
status alone can be made to pass a proof it never accepted.

| System | Foundation | Check | Rejected escape hatches |
|---|---|---|---|
| **Lean 4** | CIC | `lean`, recheck `leanchecker` | `sorry`, disallowed axioms, `native_decide` trust holes |
| **Rocq** | CIC | `coqc`, recheck `coqchk` | `Admitted`, `-type-in-type`, `Unset Universe Checking` |
| **Isabelle/HOL** | Church HOL | `isabelle build`, oracle audit | non-empty oracle set, `sorry` |
| **Candle** | HOL Light on CakeML | `candle`, the kernel is the proven check | `mk_thm`, `new_axiom`, `CHEAT_TAC` |
| **Agda** | Martin-Lof type theory | `agda --safe` | `postulate`, unsolved metas, foreign pragmas |
| **Metamath** | set.mm substitution kernel | `verify proof *` | generated `$a`, `?` placeholder steps |

Candle is the outlier: its kernel's soundness is machine-proven in HOL4 down to the
CakeML-compiled binary, so its layer-3 recheck carries a stronger guarantee than a
smaller-trusted-checker argument.

## Proof it bites

The interesting cases are the ones that pass everything else. Each row below is a real
artifact found in third-party Lean, and each is pinned by a fixture in the test suite.

| What slipped through | What caught it |
|---|---|
| An `.olean` asserting `False` with a body of `True.intro`. A consumer importing it proves `False` at exit 0, with no `sorry`, and `#print axioms` reporting that it depends on **no axioms at all**. | Environment replay. The axiom audit calls this clean; only re-running every declaration through the kernel rejects it. |
| `theorem xHyperbolicity : ∃ r, r = (bigExpr).lambda1`, trivially true for any expression whatsoever, named for a substantive PDE property. Sorry-free, axiom-free, statement genuinely preserved. | Statement triviality. Replace the definitions with unrelated constants, re-run the unchanged proof; if it still closes, the statement never constrained them. |
| A theorem named for a published result whose every substantive symbol is a `sorry`-defined constant. The proof is complete. | Opaque-constant attribution. The axiom audit sees `sorryAx` but reports it identically to an honest unfinished proof; this names the guilty constants. |

The adversarial registry carries accept and reject fixtures both, because a gate that
rejects everything is as broken as one that accepts everything, and a reject for the wrong
reason is a coincidence rather than a passing test.

By the numbers:

- **6** proof assistants behind **1** gate
- **22** independently re-checkable certificate kinds
- **0 of 7,365** ordinary Mathlib statements trip the triviality detector; it fires only on
  the shapes it was built for, never on real mathematics
- accusing detectors **only ever accuse**: surviving a check is recorded as "not shown to
  be wrong", never as sound

## Certificates

Beyond the kernel gate, results that admit a certificate emit a self-describing proof-log
(`theoremata.cert-log.v1`) that a small independent checker re-verifies offline in exact
rational arithmetic. **22 kinds** ship, including:

- linear and nonlinear bounds
- Positivstellensatz and sums-of-squares
- Pratt and Pocklington primality
- Nullstellensatz ideal membership
- Sturm real-root counts
- WZ hypergeometric identities
- Bezout coefficients
- continued fractions
- Taylor models
- floating-point rounding and error bounds

Certificates do not rot the way tactic scripts do. A self-contained rational certificate
replays forever; a proof term rots only if a constant it names changes. That asymmetry is
why the staleness sweep routes them differently.

## Install

```bash
git clone https://github.com/ReverseZoom2151/theoremata && cd theoremata
cargo build --release

./target/release/theoremata init
./target/release/theoremata doctor    # probes the six backends, Python, model provider
```

The Python workers need no install step: the namespace package resolves from the source
tree. `doctor` tells you what is actually live on your machine, and whatever is missing is
skipped rather than fatal.

Formal backends are optional and independent. Install only the ones you want; a backend
without its toolchain runs mock-only and can never reach `FormallyVerified`.

> [!TIP]
> Bring your own model. The provider is model-agnostic through LiteLLM, so any hosted API
> or a local model via [Ollama](docs/ollama.md) works by setting one environment variable.

## CLI

```bash
theoremata new pyth "for all a b c : Nat, a*a + b*b = c*c -> ..."
theoremata falsify "x,y" "x^2 + y^2 >= 2*x*y"     # break it before proving it
theoremata formal-prove lean "1 + 1 = 2"          # straight through the live gate
theoremata hammer-prove isabelle "1 + 1 = (2::nat)"
theoremata agent pyth                             # the autonomous loop on a project
theoremata sweep --project pyth                   # staleness census over stored greens
theoremata alpha-sweep "..." --critic-weight 1.0  # critic-guided frontier search
theoremata blueprint-run pyth content.tex         # paper-scale, dependency-ordered
```

Every Python worker is reachable directly (`theoremata tool '{"tool":"..."}'`), including
all the certificate checkers. Search stages emit candidates branded unverified; nothing
becomes a proof without passing the live gate.

## Honest limits

Stated plainly, because a harness that overstates itself is the thing this project exists
to prevent.

- **Live in CI:** the full four-layer gate for all six provers, portfolio and
  hammer-assisted proving, the sketch and blueprint pipelines, falsification, retrieval,
  grading, the proof-DAG, and every certificate checker.
- **Toolchain-gated:** a genuinely certified pass needs that backend's toolchain present.
  Without it the backend is mock-only. Certificate checkers run offline regardless.
- **GPU / model-gated:** real weight training and a live end-to-end model run. The
  flywheel, reward models, and selectors run offline in deterministic mode.
- **Corpus-gated:** benchmark loaders without a vendored corpus return empty rather than
  fail. 42 benchmarks are registered.

The harness supplies the environment, the verification, the certificates, and the
orchestration. A trained model and real mileage on hard theorems are the two things it
does not supply on its own.

## Non-goals

- **Cross-foundation transfer.** Not soundly possible today. Alignments steer retrieval
  and may never license a transfer; anything crossing a corpus boundary is re-proved in
  the target kernel.
- **Repair that rewrites statements.** The dominant failure of LLM proof repair is
  weakening the statement until it compiles. Repair may rewrite a script. It may never
  touch the statement.
- **Trusting a coverage number.** Detectors here can accuse and never bless. Surviving a
  check is recorded as "not shown to be wrong", never as sound.

## Architecture

```
app/            CLI, TUI, config, the versioned JSON API
components/
  graph/        typed proof-DAG, evidence trail, citations
  prover/       the gate, six backends, statement preservation, vacuity
  reason/       orchestration, search, proving, critique
  retrieval/    premise selection, indices, semantic memory
  verify/       certificate checkers, kernel replay, statement quality
  eval/         benchmarks, graders, adversarial fixtures
  train/        flywheel, reward models, selectors
```

Rust owns orchestration, the proof-DAG, the gate, and the search primitives. Python owns
the mechanical and numerical work: falsification, retrieval, grading, training, and exact
certificate checking.

## Credits and licence

Theoremata draws on open-source and published theorem-proving work, including:

- **Neural proving:** LeanDojo and ReProver, AlphaProof, DeepSeekMath-V2, Goedel-Prover,
  Kimina-Prover, DeepSeek-Prover-V2, Seed-Prover, BFS-Prover, InternLM-StepProver
- **Sketch, library, and repair:** Aristotle, LEGO-Prover, ImProver
- **Grading and geometry:** PROOFGRADER, AlphaGeometry
- **Soundness and certificates:** LeanParanoia, SafeVerify, the Flyspeck project, the
  CakeML and HOL Light ecosystems, and John Harrison's certificate work

Every adopted idea is traced to its source, mechanism by mechanism, in:

- [`docs/resource-mining/`](docs/resource-mining/) (repositories)
- [`docs/paper-mining/`](docs/paper-mining/) (papers)
- [`docs/atp-mining/`](docs/atp-mining/) (the HOL Light and ATP corpus)

Vendored sources are reused clean-room where their licence requires it.

Our own code is MIT. Vendored resources and the proof assistants keep their own licences.
The Lean, Rocq, and ratatui logos in `assets/logos/` belong to their respective projects.
