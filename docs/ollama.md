# Running Theoremata against local models via Ollama

Theoremata talks to models through a single model-agnostic adapter
(`components/provider/python/theoremata_tools/model_provider.py`) backed by
[LiteLLM]. LiteLLM speaks to [Ollama] natively, so pointing Theoremata at a
local open-weights model (for example a Qwen 3 chat model) needs no code change
and no new HTTP client: it is purely install plus environment configuration.

## Setup

1. Install Ollama (https://ollama.com/download) and start its daemon:

   ```
   ollama serve
   ```

   The daemon listens on `http://localhost:11434` by default. Override with the
   `OLLAMA_API_BASE` environment variable if it runs elsewhere.

2. Pull a model. Use whatever tag `ollama pull` actually offers; browse
   https://ollama.com/library to find the exact name and size. For a Qwen 3
   chat model this looks like:

   ```
   ollama pull qwen3
   ```

   The precise Qwen tag (the user mentioned "3.6") must be whatever the library
   currently publishes; do not assume a version string that may not exist.
   Confirm what you pulled with `ollama list`.

3. Install LiteLLM into the environment Theoremata's Python adapter runs in:

   ```
   pip install litellm
   ```

   (The adapter imports LiteLLM lazily, so it is only needed for real calls, not
   for mock mode.)

4. Select the model by environment variable. Use the `ollama_chat/` prefix for
   chat models: it routes to Ollama's `/api/chat` endpoint, which is correct for
   instruction/chat models. (`ollama/<model>` instead hits `/api/generate` and is
   for non-chat completion models.)

   ```
   THEOREMATA_MODEL=ollama_chat/qwen3
   ```

   Substitute the exact tag from step 2. Per-role overrides use
   `THEOREMATA_MODEL_<ROLE>` (role upper-cased, non-alphanumerics to `_`), e.g.
   `THEOREMATA_MODEL_PROOF_DECOMPOSER`.

5. Point Theoremata at the adapter. The Rust `CommandProvider` runs the command
   in `THEOREMATA_MODEL_COMMAND`, sending one `ModelRequest` JSON on stdin and
   reading `ModelStreamEvent` lines from stdout. Set it to invoke this adapter,
   for example:

   ```
   THEOREMATA_MODEL_COMMAND="python -m theoremata_tools.model_provider"
   ```

   (run from `components/provider/python`, or with that directory on
   `PYTHONPATH`).

6. Run a Theoremata command as usual. It will now route model calls to your
   local Ollama model.

## A worked example

```
ollama serve &
ollama pull qwen3
pip install litellm
export THEOREMATA_MODEL=ollama_chat/qwen3
export THEOREMATA_MODEL_COMMAND="python -m theoremata_tools.model_provider"
# then run your normal theoremata invocation
```

## Reasoning-model caveat

Qwen 3 and most current open-weights models are reasoning models: they emit a
leading chain-of-thought block (a `think` span) or a prose preamble before the
actual answer, and their JSON is often wrapped in a markdown code fence. The
adapter's JSON extraction is hardened for this: it strips a leading reasoning
block (including a truncated one with no closing tag) and any code fences, then
locates the first balanced top-level JSON object with a string-aware brace scan.

This only makes the JSON findable; it never fabricates or coerces fields, and
schema validation is unchanged, so a reply that does not conform still fails and
the built-in retry/fallback path runs. Structured-output reliability still varies
by model; smaller models may need the fallback chain
(`THEOREMATA_MODEL_FALLBACK`) or a larger tag.

## Verifying without a model

Mock mode returns canned, schema-shaped JSON and imports neither LiteLLM nor
Ollama:

```
THEOREMATA_MODEL_MOCK=1
```

Use it to exercise the pipeline before a real model is pulled.

[LiteLLM]: https://github.com/BerriAI/litellm
[Ollama]: https://ollama.com
