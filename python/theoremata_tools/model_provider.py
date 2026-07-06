"""Model-agnostic provider adapter for Theoremata (Phase 1 / plan §3).

This is a small, model-agnostic script that satisfies the Rust
``CommandProvider`` protocol in ``src/provider.rs``. It reads a single
``ModelRequest`` JSON object from stdin and writes ``ModelStreamEvent`` JSON
lines to stdout:

    {"type": "started", "provider": "litellm"}
    {"type": "delta", "text": "..."}            # optional, may repeat
    {"type": "completed", "response": {"content": {...}, "model": "...",
                                       "provider": "litellm"}}

The ``completed`` event's ``response`` becomes the Rust ``ModelResponse``; its
``content`` is a JSON object conforming to the request's ``output_schema``.

Backed by **LiteLLM** so a single script can route to any provider
(Anthropic, OpenAI, Google, local, ...). LiteLLM is imported lazily inside the
call path, so this module imports fine (and mock mode + unit tests run) with
LiteLLM NOT installed.

litellm pin: tested against **litellm==1.91.0**. litellm is an OPTIONAL
dependency -- mock mode (``THEOREMATA_MODEL_MOCK=1``) needs no install at all.

Environment variables
----------------------
- ``THEOREMATA_MODEL_<ROLE_UPPER>``  per-role model override (role upper-cased,
  non-alphanumeric chars -> ``_``). e.g. ``THEOREMATA_MODEL_PROOF_DECOMPOSER``.
- ``THEOREMATA_MODEL``               default model for all roles.
- (built-in default)                 ``anthropic/claude-sonnet-5``.
- ``THEOREMATA_MODEL_FALLBACK``      comma-separated fallback model chain.
- ``THEOREMATA_TEMPERATURE``         sampling temperature (default ``0.2``).
- ``THEOREMATA_MODEL_RETRIES``       per-model attempts (default ``3``).
- ``THEOREMATA_MODEL_MOCK``          ``1`` -> return canned JSON, no litellm.

Provider API keys are read by litellm from the usual env vars, e.g.
``ANTHROPIC_API_KEY`` / ``OPENAI_API_KEY`` / ``GEMINI_API_KEY``.
"""
from __future__ import annotations

import json
import os
import re
import sys
import time
from typing import Any, Optional

DEFAULT_MODEL = "anthropic/claude-sonnet-5"
DEFAULT_TEMPERATURE = 0.2
DEFAULT_RETRIES = 3
PROVIDER_NAME = "litellm"


# --------------------------------------------------------------------------- #
# Pure helpers (no litellm, no network) -- unit-tested directly.
# --------------------------------------------------------------------------- #
def _strip_code_fences(text: str) -> str:
    """Remove surrounding ```/```json markdown fences from a model reply."""
    stripped = text.strip()
    if not stripped.startswith("```"):
        return stripped
    # Drop the opening fence line (``` or ```json etc.) and a trailing fence.
    lines = stripped.splitlines()
    if lines and lines[0].startswith("```"):
        lines = lines[1:]
    if lines and lines[-1].strip().startswith("```"):
        lines = lines[:-1]
    return "\n".join(lines).strip()


def _find_balanced_object(text: str) -> Optional[str]:
    """Return the first balanced ``{...}`` JSON object substring, string-aware."""
    start = text.find("{")
    if start == -1:
        return None
    depth = 0
    in_string = False
    escape = False
    for i in range(start, len(text)):
        ch = text[i]
        if in_string:
            if escape:
                escape = False
            elif ch == "\\":
                escape = True
            elif ch == '"':
                in_string = False
            continue
        if ch == '"':
            in_string = True
        elif ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return text[start : i + 1]
    return None


def extract_json_object(text: str) -> dict[str, Any]:
    """Robustly extract a single JSON object from model text.

    Strips markdown code fences, tries a direct ``json.loads``, and finally
    falls back to the first balanced ``{...}`` object embedded in prose.
    Raises ``ValueError`` if no JSON object can be recovered.
    """
    if text is None:
        raise ValueError("no text to parse")
    candidate = _strip_code_fences(text)
    # Fast path: the whole thing is a JSON object.
    try:
        parsed = json.loads(candidate)
        if isinstance(parsed, dict):
            return parsed
    except (json.JSONDecodeError, ValueError):
        pass
    # Fallback: locate the first balanced object and parse it.
    balanced = _find_balanced_object(candidate)
    if balanced is None:
        raise ValueError("no JSON object found in model output")
    parsed = json.loads(balanced)
    if not isinstance(parsed, dict):
        raise ValueError("extracted JSON is not an object")
    return parsed


def missing_required_keys(obj: dict[str, Any], schema: Any) -> list[str]:
    """Return required schema keys absent from ``obj`` (lightweight validation).

    Only enforces top-level ``required`` when ``schema.type == "object"``.
    """
    if not isinstance(schema, dict):
        return []
    if schema.get("type") not in (None, "object"):
        return []
    required = schema.get("required")
    if not isinstance(required, list):
        return []
    return [key for key in required if key not in obj]


def validate_against_schema(obj: dict[str, Any], schema: Any) -> None:
    """Raise ``ValueError`` if required keys from ``schema`` are missing."""
    missing = missing_required_keys(obj, schema)
    if missing:
        raise ValueError(f"missing required keys: {', '.join(missing)}")


# --------------------------------------------------------------------------- #
# Role -> model routing.
# --------------------------------------------------------------------------- #
def _role_env_suffix(role: str) -> str:
    """Upper-case a role and replace non-alphanumeric chars with ``_``."""
    return re.sub(r"[^0-9A-Za-z]+", "_", (role or "")).strip("_").upper()


def model_for_role(role: str, env: Optional[dict[str, str]] = None) -> str:
    """Resolve the model id for a role: per-role env -> global env -> default."""
    env = os.environ if env is None else env
    suffix = _role_env_suffix(role)
    if suffix:
        specific = env.get(f"THEOREMATA_MODEL_{suffix}")
        if specific:
            return specific
    return env.get("THEOREMATA_MODEL") or DEFAULT_MODEL


def fallback_models(env: Optional[dict[str, str]] = None) -> list[str]:
    """Parse the comma-separated ``THEOREMATA_MODEL_FALLBACK`` chain."""
    env = os.environ if env is None else env
    raw = env.get("THEOREMATA_MODEL_FALLBACK", "")
    return [m.strip() for m in raw.split(",") if m.strip()]


def _temperature(env: Optional[dict[str, str]] = None) -> float:
    env = os.environ if env is None else env
    try:
        return float(env.get("THEOREMATA_TEMPERATURE", DEFAULT_TEMPERATURE))
    except (TypeError, ValueError):
        return DEFAULT_TEMPERATURE


def _retries(env: Optional[dict[str, str]] = None) -> int:
    env = os.environ if env is None else env
    try:
        return max(1, int(env.get("THEOREMATA_MODEL_RETRIES", DEFAULT_RETRIES)))
    except (TypeError, ValueError):
        return DEFAULT_RETRIES


# --------------------------------------------------------------------------- #
# Prompt construction.
# --------------------------------------------------------------------------- #
def build_messages(request: dict[str, Any]) -> list[dict[str, str]]:
    """Build the system + user chat messages for a ``ModelRequest``."""
    role = request.get("role", "assistant")
    task = request.get("task", "")
    context = request.get("context")
    schema = request.get("output_schema")

    system = (
        f"You are the '{role}' component of the Theoremata mathematical "
        "research system. Your task:\n"
        f"{task}\n\n"
        "Respond ONLY with a single JSON object conforming to the provided "
        "JSON schema. Do not include any prose, explanation, or markdown "
        "code fences -- output the raw JSON object and nothing else."
    )

    user_parts = [
        "Context (JSON):",
        json.dumps(context, indent=2, default=str),
    ]
    if schema is not None:
        user_parts += [
            "",
            "Output JSON schema:",
            json.dumps(schema, indent=2, default=str),
        ]
    user_parts += [
        "",
        "Return the single JSON object now.",
    ]

    return [
        {"role": "system", "content": system},
        {"role": "user", "content": "\n".join(user_parts)},
    ]


# --------------------------------------------------------------------------- #
# Mock mode -- role-appropriate canned JSON, no litellm import.
# --------------------------------------------------------------------------- #
def _mock_content(role: str, schema: Any) -> dict[str, Any]:
    """Return canned JSON for a role, respecting the schema's required keys."""
    if role == "proof_decomposer":
        content: dict[str, Any] = {
            "obligations": [{"title": "Mock", "statement": "Mock step."}]
        }
    elif role == "lean_formalizer":
        content = {"lean": "import Init\n\ntheorem theoremata_mock : True := trivial\n"}
    else:
        content = {"reply": "[mock] ok", "mutations": []}

    # Ensure every required key from the schema is present, with a minimal
    # schema-shaped (non-empty) value so downstream consumers get usable data.
    props = schema.get("properties", {}) if isinstance(schema, dict) else {}
    for key in missing_required_keys(content, schema):
        content[key] = _sample_value(props.get(key, {}))
    return content


def _sample_value(prop: Any) -> Any:
    """A minimal, non-empty value that satisfies a JSON-Schema property."""
    if not isinstance(prop, dict):
        return "sample"
    kind = prop.get("type")
    if kind == "array":
        return [_sample_value(prop.get("items", {"type": "object"}))]
    if kind == "object":
        return {
            key: _sample_value(prop.get("properties", {}).get(key, {}))
            for key in prop.get("required", [])
        }
    if kind in ("integer", "number"):
        return 0
    if kind == "boolean":
        return False
    return "sample"


# --------------------------------------------------------------------------- #
# litellm-backed generation.
# --------------------------------------------------------------------------- #
def _import_litellm():
    """Lazy-import litellm and configure per-provider param dropping."""
    import litellm  # noqa: WPS433 -- intentional lazy import

    # Drop provider-unsupported params (e.g. response_format on providers that
    # don't accept it) instead of erroring.
    litellm.drop_params = True
    return litellm


def _completion_kwargs(
    model: str,
    messages: list[dict[str, str]],
    schema: Any,
    temperature: float,
) -> dict[str, Any]:
    kwargs: dict[str, Any] = {
        "model": model,
        "messages": messages,
        "temperature": temperature,
    }
    if schema is not None:
        kwargs["response_format"] = {
            "type": "json_schema",
            "json_schema": {
                "name": "response",
                "schema": schema,
                "strict": False,
            },
        }
    return kwargs


def _extract_text(response: Any) -> str:
    """Pull the assistant text out of a litellm ModelResponse."""
    try:
        return response.choices[0].message.content or ""
    except (AttributeError, IndexError, KeyError, TypeError):
        # Dict-like fallback.
        try:
            return response["choices"][0]["message"]["content"] or ""
        except (KeyError, IndexError, TypeError):
            return ""


def _call_model(
    litellm,
    model: str,
    messages: list[dict[str, str]],
    schema: Any,
    temperature: float,
    retries: int,
    on_delta=None,
) -> dict[str, Any]:
    """Attempt one model with in-model retries + corrective turns.

    Raises on exhaustion so the caller can move to the next fallback model.
    """
    convo = list(messages)
    last_error: Optional[Exception] = None
    for attempt in range(retries):
        try:
            response = litellm.completion(
                **_completion_kwargs(model, convo, schema, temperature)
            )
        except Exception as exc:  # noqa: BLE001 -- API/transport errors
            last_error = exc
            # Exponential backoff on API errors.
            time.sleep(min(2**attempt, 8))
            continue

        text = _extract_text(response)
        if on_delta is not None and text:
            on_delta(text)
        try:
            content = extract_json_object(text)
            validate_against_schema(content, schema)
            return content
        except ValueError as exc:
            last_error = exc
            # Append a corrective user turn and retry the same model.
            convo = convo + [
                {"role": "assistant", "content": text},
                {
                    "role": "user",
                    "content": (
                        f"That response was not valid ({exc}). Reply with ONLY "
                        "a single JSON object conforming to the schema, with no "
                        "prose or markdown."
                    ),
                },
            ]
    raise RuntimeError(
        f"model '{model}' failed after {retries} attempts: {last_error}"
    )


def generate(
    request: dict[str, Any],
    on_delta=None,
    env: Optional[dict[str, str]] = None,
) -> tuple[dict[str, Any], str]:
    """Generate schema-conforming content for a ``ModelRequest``.

    Returns ``(content_obj, model_str)``. In mock mode
    (``THEOREMATA_MODEL_MOCK == "1"``) returns canned JSON WITHOUT importing
    litellm.
    """
    env = os.environ if env is None else env
    role = request.get("role", "assistant")
    schema = request.get("output_schema")

    if env.get("THEOREMATA_MODEL_MOCK") == "1":
        return _mock_content(role, schema), "mock"

    litellm = _import_litellm()
    messages = build_messages(request)
    temperature = _temperature(env)
    retries = _retries(env)

    candidates = [model_for_role(role, env)] + fallback_models(env)
    seen: set[str] = set()
    ordered = [m for m in candidates if not (m in seen or seen.add(m))]

    last_error: Optional[Exception] = None
    for model in ordered:
        try:
            content = _call_model(
                litellm,
                model,
                messages,
                schema,
                temperature,
                retries,
                on_delta=on_delta,
            )
            return content, model
        except Exception as exc:  # noqa: BLE001 -- try next fallback
            last_error = exc
            continue
    raise RuntimeError(f"all models failed: {last_error}")


# --------------------------------------------------------------------------- #
# Stream-event emission / CLI entrypoint.
# --------------------------------------------------------------------------- #
def _emit(event: dict[str, Any]) -> None:
    sys.stdout.write(json.dumps(event) + "\n")
    sys.stdout.flush()


def main() -> None:
    """Read a ModelRequest from stdin, emit ModelStreamEvent lines to stdout."""
    try:
        request = json.load(sys.stdin)
    except Exception as exc:  # noqa: BLE001
        sys.stderr.write(f"invalid ModelRequest JSON on stdin: {exc}\n")
        raise SystemExit(1)

    _emit({"type": "started", "provider": PROVIDER_NAME})

    def on_delta(text: str) -> None:
        _emit({"type": "delta", "text": text})

    try:
        content, model = generate(request, on_delta=on_delta)
    except Exception as exc:  # noqa: BLE001
        sys.stderr.write(f"model provider failed: {exc}\n")
        raise SystemExit(1)

    _emit(
        {
            "type": "completed",
            "response": {
                "content": content,
                "model": model,
                "provider": PROVIDER_NAME,
            },
        }
    )


if __name__ == "__main__":
    main()
