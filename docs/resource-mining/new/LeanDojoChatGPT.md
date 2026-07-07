# LeanDojoChatGPT — Full Resource-Mining Pass

Source: `resources/LeanDojoChatGPT-main/LeanDojoChatGPT-main/`
Pass type: full; the repo is small enough that all prose and implementation files were read.

---

## 1) What it is

LeanDojoChatGPT is an old ChatGPT plugin wrapper around LeanDojo. It exposes two HTTP operations:

- initialize proof search for a theorem;
- run a tactic on a previously returned proof state.

The README explicitly says the repo is outdated due to OpenAI API changes. Its technical value is therefore not the plugin packaging; it is the minimal tool contract showing how an LLM can interact with Lean through LeanDojo.

## 2) Files and implementation

Files inspected:

- `README.md` — launch instructions, plugin usage prompt, outdated warning.
- `main.py` — Quart backend and LeanDojo session wrapper.
- `openapi.yaml` — operation schema and model-facing instructions.
- `manifest.json` — old ChatGPT plugin manifest.
- `images/logo.jpg` — non-semantic logo asset.

`main.py` keeps global variables:

- `repo`
- `theorem`
- `dojo`
- `states`

`/initialize_proof_search` accepts `theorem_file_path` and `theorem_name`, constructs a `Theorem(repo, path, name)`, enters `Dojo(theorem)`, stores the initial `TacticState` by ID, and returns:

```json
{ "state_id": 0, "state": "..." }
```

`/run_tactic` accepts `state_id` and `tactic`, calls `dojo.run_tac(...)`, and returns one of:

- next `state_id`, next `state`, `proof_finished: false`;
- `error`, `proof_finished: false`;
- `proof_finished: true`.

The OpenAPI description tells ChatGPT to explain tactic choices, run one tactic step, inspect errors, and backtrack by using previous state IDs.

## 3) Reusable ideas

**The two-operation proof tool is right.** The core API should remain small:

1. `initialize(theorem_ref) -> state_id, pretty_state`
2. `run_tactic(session_id, state_id, tactic) -> next_state | error | proof_finished`

That is enough for an LLM agent to plan, try tactics, recover from errors, and build a proof transcript.

**State IDs are an LLM-friendly abstraction.** The model does not need to hold Lean kernel objects. It only needs stable IDs and pretty-printed states.

**The OpenAPI text captures useful behavioral guidance.** It tells the model to:

- explain tactic choices before acting;
- treat errors as feedback;
- backtrack to previous states;
- stop when no goals remain.

Those instructions should be converted into Theoremata’s tool policy / system prompt for tactic search.

**The server is a thin adapter, not a theorem prover.** This is a useful architectural boundary: theorem proving control can live in the agent, while LeanDojo/Lean/Pantograph provides transition semantics.

## 4) Gaps and risks

This repo should not be used directly:

- It targets the deprecated ChatGPT plugin system.
- It has no authentication, no per-user/session isolation, and permissive CORS.
- `repo`, `dojo`, and `states` are process-global, so concurrent proof searches collide.
- It never clearly calls the `Dojo` context manager’s exit/cleanup path after proof completion or abandonment.
- It assumes a single cached/traced repo passed at server startup.
- It returns `json.dumps(str(res))` in `/run_tactic`, which serializes a Python string representation rather than a proper JSON object.
- It does not validate theorem paths/names, state IDs, tactic size, session ownership, or resource budgets.
- The OpenAPI prompt suggests decrementing `state_id` to backtrack, but state IDs are implementation artifacts and need not form a semantic parent chain.

## 5) Adopt list for Theoremata

P0:

- Use the **two-tool contract**: initialize a theorem and run one tactic on one saved state.
- Return structured JSON with typed outcomes: `state`, `error`, `timeout`, `given_up`, `proof_finished`.
- Add a true `session_id`; never use global state for multi-agent or multi-user proof search.

P1:

- Add explicit state-parent metadata rather than asking the model to “decrement” state IDs.
- Persist a proof transcript: `(state_id, tactic, response, parent_state_id, timestamp, model)` for replay and training.
- Give the LLM the same behavioral loop as the plugin prompt, but enforce it with controller logic: retry caps, backtracking policy, tactic deduplication, and proof validation.

P2:

- Treat this repo as an API-design seed only. The implementation is too old and unsafe for direct reuse.

