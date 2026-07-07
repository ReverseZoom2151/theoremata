# LeanCopilot — Full Resource-Mining Pass

Source: `resources/LeanCopilot-main/LeanCopilot-main/`
Pass type: full over README, Lean source, tests, Python server/runners, build scripts, C++ FFI glue; vendored `cpp/json.hpp` catalogued as third-party nlohmann JSON and not line-read.

---

## 1) What it is

LeanCopilot is an in-Lean AI assistant for tactic suggestion, premise selection, and proof search. Unlike LeanDojo/ReProver, it is not primarily a training pipeline. Its main value is the interactive Lean integration layer:

- `suggest_tactics` asks a model for candidate tactics and shows them as Lean editor hints.
- `select_premises` retrieves relevant premises for the current tactic state.
- `search_proof` plugs model-generated tactics into Aesop as a tactic generator.
- Models can be native CTranslate2 FFI models, external HTTP models, or arbitrary Lean-defined generic models.

This repo is valuable for Theoremata’s user-facing/Lean-facing integration, not as the core proving/training backend.

## 2) Core architecture

Main files inspected:

- `LeanCopilot/Tactics.lean` — `pp_state`, `suggest_tactics`, `select_premises`, premise annotation.
- `LeanCopilot/LlmAesop.lean` — model-backed Aesop tactic generator and `search_proof`.
- `LeanCopilot/Options.lean` — Lean options for model choice, checking, verbosity, premise count.
- `LeanCopilot/Models/Interface.lean` — `TextToText` and `TextToVec` typeclasses.
- `LeanCopilot/Models/{Generic,Native,External,FFI,Registry,Builtin,ByT5}.lean` — model abstractions, registry, native and external runners.
- `cpp/ct2.cpp`, `cpp/npy.hpp` — CTranslate2 Lean FFI and premise-embedding retrieval.
- `python/server.py`, `python/models.py`, `python/external_models/*` — FastAPI external model server and runners for HF/vLLM/OpenAI/Claude/Gemini-style backends.
- `LeanCopilotTests/*.lean` — usage examples for suggestions, premise selection, proof search, and model APIs.
- `lakefile.lean` — complex platform/CUDA/release build logic for CTranslate2.

## 3) Reusable ideas and code patterns

**Small model typeclasses.** The core interface is excellent:

```lean
class TextToText (τ) where
  generate : τ -> String -> String -> IO (Array (String × Float))

class TextToVec (τ) where
  encode : τ -> String -> IO FloatArray
```

This is enough to support tactic generators and premise encoders without coupling Lean tactics to one model runtime.

**Model registry inside Lean.** `ModelRegistry` maps names to `Generator` and `Encoder` variants. Lean options choose the active model:

- `LeanCopilot.suggest_tactics.model`
- `LeanCopilot.suggest_tactics.check`
- `LeanCopilot.select_premises.k`
- `LeanCopilot.verbose`

Theoremata should adopt this kind of named model indirection for editor/prover integration.

**Tactic suggestions as Lean hints.** `suggest_tactics` pretty-prints the current tactic state, generates candidates, filters obvious self-references and raw `aesop`, optionally checks tactics, and emits Lean hints. This is a good UX pattern for human-in-the-loop proving.

**LLM as an Aesop tactic generator.** `LlmAesop.lean` defines `tacGen : Aesop.TacGen`, registers it with `#configure_llm_aesop`, and exposes `search_proof`. The key idea is compositional: let a traditional search framework call the model as one rule source, instead of letting the model drive everything.

**External model server contract.** The FastAPI server exposes `/generate` and `/encode`, matching the Lean-side `ExternalGenerator`/`ExternalEncoder`. This cleanly separates Lean from Python/HF/vLLM/OpenAI runtime complexity.

**Low-latency native path.** The CTranslate2 FFI caches translators/encoders in process and supports beam-search generation and embedding retrieval. This is heavy, but it shows how to make in-editor model calls fast when local inference matters.

## 4) Benchmark and evaluation value

LeanCopilot’s tests are mostly executable usage examples rather than a formal benchmark:

- tactic suggestion examples over simple arithmetic;
- premise selection examples with configurable `k`;
- model API `#eval` examples for native, generic, and external models;
- Aesop integration examples.

For Theoremata, the value is smoke-testing an editor-facing integration:

- can Lean call the model server?
- can the model server return candidates?
- can tactic suggestions be checked?
- can model-backed Aesop finish simple examples?

## 5) Gaps and risks

- LeanDojo docs explicitly note that FFI-heavy repos like LeanCopilot cannot be processed by LeanDojo tracing.
- The native build is complex: CTranslate2, CUDA/platform detection, dynamic libraries, release archives, and vendored C++.
- `External.lean` shells out to `curl` for every request; this is simple but not ideal for robust long-running integration.
- Model registry and native model caches are global process state.
- The self-reference filter is heuristic string matching on the current theorem’s last name component.
- Premise retrieval uses precomputed global embeddings and does not obviously enforce LeanDojo/ReProver-style import accessibility in the inspected Lean tactic path.
- The C++ retrieval path forces premise embeddings to CPU because of a noted CUDA crash.
- The tests demonstrate behavior but do not assert robust pass/fail criteria.
- This repo is an assistant/plugin layer, not a dataset/trainer/lifelong agent.

## 6) Adopt list for Theoremata

P0:

- Adopt the **model interface shape**: `generate(state, prefix) -> [(tactic, score)]` and `encode(text) -> vector`.
- Expose Theoremata models through an **external `/generate` and `/encode` server** that Lean-side tools can call.
- Add an editor-facing `suggest_tactics` equivalent for human-in-the-loop proof development.

P1:

- Integrate the model as a tactic source inside a symbolic/search tactic framework, following the `LlmAesop` pattern.
- Use named model registries/options so a proof script or session can switch among backends without code changes.
- Return candidate tactics with scores and optional check results; do not just print raw text.

P2:

- Do not initially port the CTranslate2 FFI. Use HTTP/server integration first; add native inference only if latency becomes the bottleneck.
- If premise selection is exposed inside Lean, enforce import/accessibility constraints rather than global nearest-neighbor retrieval alone.

