# Running the agent locally against a local model

This machine is already set up to drive Theoremata with a local Ollama model. This is the
copy-paste reference.

## Prerequisites (all currently satisfied here)

- `target/release/theoremata.exe` is built (`cargo build --release` if not).
- Ollama is serving on `http://localhost:11434` with a model pulled (`qwen3.6:35b`,
  `qwen3.6:27b`, `ornith:9b`, `ornith:35b` are present here). Start it with `ollama serve`
  if the API is down.
- `litellm` is installed for the Windows `python` (the model adapter uses it).
- `.theoremata/config.json` has `model_command` pointing at the adapter:
  `PYTHONPATH=components/provider/python python -m theoremata_tools.model_provider`.

Live formal backends here: **Lean** (native), **Rocq** and **Isabelle** (via WSL Ubuntu).
Candle, Agda, Metamath are not installed.

## Which terminal: PowerShell, not WSL

Run it in **Windows Terminal / PowerShell**. The binary is a native Windows console app,
so the full-screen TUI renders correctly there. Do **not** launch it from a WSL shell: that
runs it as a Windows process through a Linux pty and the TUI breaks.

One catch: the model call runs via `bash -lc`, and in a plain PowerShell `bash` resolves to
the WSL launcher, which is the wrong `python` (no litellm). So put **Git's bin first on
PATH** so `bash` means Git Bash, whose `python` is the Windows one with litellm. The setup
line below does that.

## One-time shell setup (PowerShell, from the repo root)

Run from the repo root, because `model_command` uses a relative `PYTHONPATH`.

```powershell
cd C:\Users\adria\Downloads\math-agent
# Git's bin FIRST so `bash` is Git Bash (Windows python + litellm), not the WSL launcher.
$env:PATH = "C:\Program Files\Git\bin;$PWD\target\release;$env:PATH"
$env:THEOREMATA_MODEL = "ollama_chat/qwen3.6:35b"
```

Prefer Git Bash instead? It already resolves `bash` correctly, so you only need:

```bash
cd /c/Users/adria/Downloads/math-agent
export PATH="$PWD/target/release:$PATH"
export THEOREMATA_MODEL=ollama_chat/qwen3.6:35b
```

Swap the model any time: `ollama_chat/qwen3.6:27b` (smaller, faster) or
`ollama_chat/ornith:9b` (fastest). Use `ollama_chat/`, not `ollama/`, for chat models.

## Just run it (the agent chat)

This is the main way in. Type the name, nothing else:

```bash
theoremata
```

A bare `theoremata` boots straight into the agent chat, opening your most recent project
or creating a `scratch` workspace on a first run. (The old `theoremata chat <project>`
still works if you want to open a specific one.)

It is a full-screen TUI with three panes: **CHAT**, **PROOF GRAPH**, **TRAJECTORY**.
Tab switches panes, Esc clears the input, Ctrl-C exits.

You drive it like any CLI agent: type natural language, and it can **act**, not just talk.

```text
> /model qwen3.6:35b                       switch the active model, live
> /new fermat | for all a b : Nat, (a+b)^2 = a^2 + 2*a*b + b^2   create a goal, switch to it
> prove the main theorem                   plain English; the agent runs the real gate
```

- **Plain text talks to the agent.** It reasons over the project's whole proof-DAG and the
  conversation, and can invoke real actions (prove, falsify, hammer, sweep), see the
  results, and react, up to a few rounds per turn. Esc or Ctrl-C interrupts between rounds.
- **It can restructure the problem**, proposing graph changes (add a lemma, set a formal
  statement). Those are proposals you review: `/proposals`, `/approve <id>`,
  `/reject <id> [reason]`. The graph never mutates without your approval.
- **It can never fake a verdict.** A model reply is text, not a status; only the gate marks
  a node verified. Ask it to prove something and it runs the real pipeline; if the proof
  does not hold, it says so.

Commands inside the chat:

- `/model [name]`: list local models, or switch the active one live
- `/project [name]`: list projects, or switch to one
- `/new <name> | <thm>`: create a project and switch to it
- `/prove [sys] <target>`: formalize, prove, and gate a node, index, or statement
- `/hammer <sys> <goal>`: hammer-assisted native proof plus gate
- `/falsify <json> <claim>`: numeric counterexample search
- `/sweep`: staleness census for this project
- `/agent`: run the autonomous loop on this project
- `/graph` `/obligations` `/attempts` `/events` `/verify` `/status` `/proposals`: inspect state
- `/help`: the full reference

`theoremata send <project> "<message>"` is the non-interactive one-shot version, useful for
scripting or piping.

The CLI verbs below are the same machinery the chat drives; reach for them when scripting.

## Prove a single statement

`formal-prove` asks the model to formalize your statement into the target system, prove it,
and run it through the full gate.

```bash
theoremata formal-prove lean "1 + 1 = 2"
theoremata formal-prove lean "for all n : Nat, n + 0 = n"
theoremata formal-prove rocq "forall n : nat, n + 0 = n"
theoremata formal-prove isabelle "(a::nat) + b = b + a"
```

Each call takes minutes against a 35B model (a single cold call was ~3 min here). The
output is a JSON report; the fields ARE the verdict:

- `code` -- the Lean/Rocq/Isabelle the model actually wrote.
- `compiled` (in `report.detail.compile`) -- did the target system accept it.
- `axioms_clean` -- axioms used are within the whitelist.
- `live: true` -- a real toolchain ran, not a mock. This is the field that matters.
- `statement_preserved` / `lexically_verified` -- see the caveat below.

## Hammer-assisted proving

Ask Sledgehammer / CoqHammer / aesop to find a tactic and assemble a proof around it, then
verify it through the gate. Often closes goals the raw model cannot.

```bash
theoremata hammer-prove isabelle "1 + 1 = (2::nat)"
theoremata hammer-prove lean "1 + 1 = 2"
```

## The full autonomous loop

`agent` runs the real loop on a project: falsify, formalize, prove, verify, across the
proof-DAG. Create a project first, then run it.

```bash
theoremata new pyth "for all a b : Nat, (a + b) * (a + b) = a*a + 2*a*b + b*b"
theoremata agent pyth
theoremata graph pyth          # inspect the resulting proof-DAG
theoremata status pyth
```

## Falsify first (no model, instant)

The counter-search is deterministic and offline. Variables are a JSON object of integer
search boxes; the claim is a PYTHON expression (so `**`, not `^`).

```bash
theoremata falsify '{"x":{"start":0,"stop":6},"y":{"start":0,"stop":6}}' "x**2 + y**2 >= 3*x*y"
```

## Per-role models

Different steps can use different models. Roles are upper-cased with non-alphanumerics as
`_`. A cheap model for routing, a strong one for proving:

```bash
export THEOREMATA_MODEL=ollama_chat/ornith:9b                 # default for all roles
export THEOREMATA_MODEL_PROOF_GENERATOR=ollama_chat/qwen3.6:35b   # override one role
```

## Two honest caveats

1. **Open models are weak at formal proof.** qwen3.6 handles trivial goals (it wrote a
   correct `1 + 1 = 2 := by rfl`), but complex proofs will often fail or abstain, and the
   gate will say so rather than pretend. That is the system working as intended.
2. **A natural-language statement cannot be fully verified.** When you pass prose,
   `formal-prove` formalizes it but has no independent formal statement to check the proof
   against, so `statement_preserved` is `false` with verdict `canonical_unparsable`, even
   when the proof compiles. `live: true` and `compiled: true` are the real signals there.
   To get a verifiable statement-preservation result, the pipeline needs a canonical formal
   statement, not prose.
