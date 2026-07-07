"""LeanCopilot-style external-model tactic server for Theoremata.

This module exposes Theoremata's model provider behind the LeanCopilot
"external model" protocol so a Lean ``ExternalGenerator`` / ``ExternalEncoder``
can drive tactic suggestion and premise encoding over plain HTTP, with no
FastAPI / CTranslate2 / native-FFI dependency.

Protocol (from ``docs/resource-mining/new/LeanCopilot.md``)
-----------------------------------------------------------
LeanCopilot's external server speaks two POST endpoints::

    POST /generate   GeneratorRequest{name, input, prefix}
                     -> GeneratorResponse{outputs: [{output, score}, ...]}

    POST /encode     EncoderRequest{name, input}
                     -> EncoderResponse{outputs: [float, ...]}

and the Lean side wires those to the ``TextToText`` / ``TextToVec``
typeclasses::

    class TextToText (τ) where
      generate : τ -> String -> String -> IO (Array (String × Float))
    class TextToVec  (τ) where
      encode   : τ -> String -> IO FloatArray

How a future Lean ``ExternalGenerator`` consumes this server
------------------------------------------------------------
On the Lean side (next step, not implemented here) one registers this server
as an external model and hands it to Aesop, mirroring LeanCopilot::

    -- point Lean at this Python server
    def theoremata : ExternalGenerator := {
      name := "theoremata-tactic",
      host := "localhost",
      port := 23337,
    }

    -- register it as the LLM tactic source for Aesop and run proof search
    #configure_llm_aesop theoremata
    example (a b : Nat) : a + b = b + a := by
      search_proof            -- Aesop calls POST /generate for each MVarId

    -- or ask for editor hints only
    example : True := by
      suggest_tactics         -- POST /generate, ranked candidates as hints

Lean's ``Aesop.TacGen : MVarId -> MetaM (Array (String × Float))`` is satisfied
by POSTing the pretty-printed goal as ``input`` to ``/generate`` and mapping the
``outputs`` array to ``(output, score)`` pairs. Premise selection
(``select_premises``) POSTs to ``/encode`` and does nearest-neighbour lookup
against precomputed premise vectors.

Design notes
------------
* Generation is backed by ``theoremata_tools.model_provider.generate`` and
  therefore honours ``THEOREMATA_MODEL_MOCK=1`` -- so this whole server (and its
  tests) runs fully offline.
* Scores come from per-candidate log-probs when the provider supplies them
  (``exp(logprob)``), else an explicit ``score`` field, else ``1.0``.
* Candidates are de-duplicated keeping the maximum score, mirroring
  LeanCopilot's ``choices_dedup``.
* ``rank_candidates`` is the pure selection/formatting layer that mirrors
  LeanCopilot's Frontend ``hint`` best-of-N intent: self-reference filtering
  (drop a tactic that just cites the goal's own theorem name) and bare-``aesop``
  filtering. It cannot run Lean here; the warm REPL drives the actual checks.

Worker wiring (report only -- this module does NOT edit worker.py)
------------------------------------------------------------------
A future dispatch line in ``components/tools/.../worker.py`` would be::

    if tool == "tactic_generate":
        from theoremata_tools.tactic_server import run as tactic_run
        return tactic_run(request)

with ``request = {"tool": "tactic_generate", "op": "generate"|"encode"|"rank",
...}``. See ``run`` below.
"""
from __future__ import annotations

import hashlib
import json
import math
import re
import struct
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Callable, Optional

from theoremata_tools import model_provider

# A provider callable maps a ModelRequest dict to a (content, model) tuple,
# exactly like ``model_provider.generate``. Tests inject their own to get
# deterministic candidates without touching the network.
Provider = Callable[[dict[str, Any]], "tuple[dict[str, Any], str]"]

DEFAULT_SCORE = 1.0
EMBED_DIM = 16
GENERATE_ROLE = "tactic_generator"
ENCODE_ROLE = "premise_encoder"


# --------------------------------------------------------------------------- #
# ModelRequest construction.
# --------------------------------------------------------------------------- #
def _generate_request(name: str, input_state: str, prefix: str) -> dict[str, Any]:
    """Build a Theoremata ``ModelRequest`` asking for candidate tactics."""
    return {
        "role": GENERATE_ROLE,
        "task": (
            "Given a Lean proof state, propose candidate next tactics that make "
            "progress on the goal. Return a JSON object with a 'tactics' array; "
            "each item has a 'tactic' string and an optional 'logprob' (natural "
            "log probability) or 'score' (0..1)."
        ),
        "context": {"model": name, "state": input_state, "prefix": prefix},
        "output_schema": {
            "type": "object",
            "required": ["tactics"],
            "properties": {
                "tactics": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["tactic"],
                        "properties": {
                            "tactic": {"type": "string"},
                            "logprob": {"type": "number"},
                            "score": {"type": "number"},
                        },
                    },
                }
            },
        },
    }


def _encode_request(name: str, input_text: str) -> dict[str, Any]:
    """Build a Theoremata ``ModelRequest`` asking for an embedding vector."""
    return {
        "role": ENCODE_ROLE,
        "task": (
            "Encode the given Lean text as a dense embedding vector for premise "
            "retrieval. Return a JSON object with an 'embedding' array of floats."
        ),
        "context": {"model": name, "text": input_text},
        "output_schema": {
            "type": "object",
            "required": ["embedding"],
            "properties": {
                "embedding": {"type": "array", "items": {"type": "number"}}
            },
        },
    }


def _default_provider(request: dict[str, Any]) -> "tuple[dict[str, Any], str]":
    """Default provider: our model_provider (honours THEOREMATA_MODEL_MOCK)."""
    return model_provider.generate(request)


# --------------------------------------------------------------------------- #
# Score extraction + de-duplication (LeanCopilot choices_dedup).
# --------------------------------------------------------------------------- #
def _score_of(item: Any) -> float:
    """Score a raw candidate: exp(logprob) > explicit score > 1.0."""
    if not isinstance(item, dict):
        return DEFAULT_SCORE
    logprob = item.get("logprob")
    if isinstance(logprob, (int, float)):
        try:
            return math.exp(float(logprob))
        except (OverflowError, ValueError):
            return DEFAULT_SCORE
    score = item.get("score")
    if isinstance(score, (int, float)):
        return float(score)
    return DEFAULT_SCORE


def _tactic_of(item: Any) -> Optional[str]:
    """Pull the tactic string out of a raw candidate (dict or bare string)."""
    if isinstance(item, str):
        text = item
    elif isinstance(item, dict):
        text = item.get("tactic") or item.get("output") or ""
    else:
        return None
    text = text.strip()
    return text or None


def dedup_candidates(candidates: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """De-duplicate ``[{output, score}]`` keeping the max score per output.

    Mirrors LeanCopilot's ``choices_dedup``. Output order is by descending
    score (stable for ties on first-seen order).
    """
    best: dict[str, float] = {}
    order: list[str] = []
    for cand in candidates:
        output = cand.get("output")
        if not isinstance(output, str):
            continue
        score = float(cand.get("score", DEFAULT_SCORE))
        if output not in best:
            best[output] = score
            order.append(output)
        elif score > best[output]:
            best[output] = score
    ranked = sorted(order, key=lambda o: (-best[o], order.index(o)))
    return [{"output": o, "score": best[o]} for o in ranked]


def _outputs_from_content(content: dict[str, Any]) -> list[dict[str, Any]]:
    """Turn a provider content dict into raw ``[{output, score}]`` candidates."""
    raw = content.get("tactics")
    if raw is None:
        raw = content.get("outputs", [])
    if not isinstance(raw, list):
        return []
    out: list[dict[str, Any]] = []
    for item in raw:
        tactic = _tactic_of(item)
        if tactic is None:
            continue
        out.append({"output": tactic, "score": _score_of(item)})
    return out


# --------------------------------------------------------------------------- #
# Public generate / encode API.
# --------------------------------------------------------------------------- #
def generate(
    name: str,
    input: str,  # noqa: A002 -- match LeanCopilot protocol field name
    prefix: str = "",
    *,
    provider: Optional[Provider] = None,
) -> list[dict[str, Any]]:
    """LeanCopilot ``/generate``: proof state -> ``[{output, score}]``.

    ``name`` is the (registry) model name, ``input`` the pretty-printed proof
    state, ``prefix`` an optional tactic prefix to continue. De-duplicates
    keeping max score. Runs offline under ``THEOREMATA_MODEL_MOCK=1``.
    """
    provider = provider or _default_provider
    request = _generate_request(name, input, prefix)
    content, _model = provider(request)
    return dedup_candidates(_outputs_from_content(content))


def _deterministic_vector(text: str, dim: int = EMBED_DIM) -> list[float]:
    """Stable unit-norm pseudo-embedding from a text hash (offline stand-in)."""
    vec: list[float] = []
    counter = 0
    while len(vec) < dim:
        digest = hashlib.sha256(f"{counter}:{text}".encode("utf-8")).digest()
        for i in range(0, len(digest), 4):
            if len(vec) >= dim:
                break
            (u,) = struct.unpack(">I", digest[i : i + 4])
            vec.append((u / 0xFFFFFFFF) * 2.0 - 1.0)
        counter += 1
    norm = math.sqrt(sum(x * x for x in vec)) or 1.0
    return [x / norm for x in vec]


def _coerce_vector(value: Any, text: str) -> list[float]:
    """Accept a provider embedding if it is a usable numeric vector, else fall
    back to a deterministic per-text vector (mock provider can't embed)."""
    if isinstance(value, list) and len(value) >= 2:
        nums = [float(x) for x in value if isinstance(x, (int, float))]
        if len(nums) == len(value) and any(abs(x) > 0.0 for x in nums):
            return nums
    return _deterministic_vector(text)


def encode(
    name: str,
    input: str,  # noqa: A002 -- match LeanCopilot protocol field name
    *,
    provider: Optional[Provider] = None,
) -> list[float]:
    """LeanCopilot ``/encode``: text -> embedding ``[float]``.

    Backed by the model provider when it yields a real vector; otherwise a
    deterministic, stable per-text vector so premise retrieval works offline.
    """
    provider = provider or _default_provider
    request = _encode_request(name, input)
    try:
        content, _model = provider(request)
    except Exception:  # noqa: BLE001 -- degrade to deterministic offline vector
        return _deterministic_vector(input)
    return _coerce_vector(content.get("embedding"), input)


# --------------------------------------------------------------------------- #
# hint-style best-of-N selection (pure). Mirrors LeanCopilot Frontend intent.
# --------------------------------------------------------------------------- #
def _theorem_name(state: Any) -> str:
    """Extract the current goal's theorem name from a proof-state dict."""
    if not isinstance(state, dict):
        return ""
    for key in ("theorem_name", "full_name", "name", "theorem"):
        val = state.get(key)
        if isinstance(val, str) and val.strip():
            return val.strip()
        if isinstance(val, dict):
            fn = val.get("full_name") or val.get("name")
            if isinstance(fn, str) and fn.strip():
                return fn.strip()
    return ""


def is_bare_aesop(tactic: str) -> bool:
    """True if the tactic is a bare ``aesop`` invocation (LeanCopilot drops it)."""
    return tactic.strip().rstrip(";").strip().lower() == "aesop"


def is_self_reference(tactic: str, theorem_name: str) -> bool:
    """True if ``tactic`` cites the goal's own theorem (its last name component).

    Heuristic string match, like LeanCopilot: guards against a suggestion that
    "proves" the goal by referring to itself (e.g. ``exact my_thm``).
    """
    short = (theorem_name or "").rsplit(".", 1)[-1].strip()
    if not short:
        return False
    return re.search(rf"(?<!\w){re.escape(short)}(?!\w)", tactic) is not None


def rank_candidates(
    state: Any, candidates: list[Any]
) -> list[dict[str, Any]]:
    """Pure best-of-N selection: normalise, filter, de-dup, order by score.

    * ``state``: proof-state dict; its theorem name drives self-reference
      filtering (accepts ``theorem_name`` / ``full_name`` / ``name`` /
      ``theorem`` keys).
    * ``candidates``: model tactic strings or ``{output/tactic, score/logprob}``
      dicts.

    Drops bare ``aesop`` and self-referencing tactics, then de-duplicates
    keeping the max score and orders by descending score. This is the
    formatting layer our warm Lean REPL will drive to actually check tactics.
    """
    name = _theorem_name(state)
    normalised: list[dict[str, Any]] = []
    for item in candidates:
        tactic = _tactic_of(item)
        if tactic is None:
            continue
        if is_bare_aesop(tactic):
            continue
        if is_self_reference(tactic, name):
            continue
        normalised.append({"output": tactic, "score": _score_of(item)})
    return dedup_candidates(normalised)


# --------------------------------------------------------------------------- #
# Worker-style dispatch (report-only wiring: tool == "tactic_generate").
# --------------------------------------------------------------------------- #
def run(request: dict[str, Any]) -> dict[str, Any]:
    """Dispatch a worker-style request. ``op`` in {generate, encode, rank}."""
    op = request.get("op", "generate")
    if op == "generate":
        return {
            "outputs": generate(
                request.get("name", ""),
                request.get("input", ""),
                request.get("prefix", ""),
            )
        }
    if op == "encode":
        return {"outputs": encode(request.get("name", ""), request.get("input", ""))}
    if op == "rank":
        return {
            "outputs": rank_candidates(
                request.get("state", {}), request.get("candidates", [])
            )
        }
    raise ValueError(f"unknown op: {op}")


# --------------------------------------------------------------------------- #
# stdlib http.server shim (no FastAPI). POST /generate and POST /encode.
# --------------------------------------------------------------------------- #
def _make_handler(provider: Optional[Provider]) -> "type[BaseHTTPRequestHandler]":
    class _Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_args: Any) -> None:  # silence stderr logging
            pass

        def _read_json(self) -> dict[str, Any]:
            length = int(self.headers.get("Content-Length", 0) or 0)
            body = self.rfile.read(length) if length else b""
            if not body:
                return {}
            return json.loads(body.decode("utf-8"))

        def _send_json(self, code: int, payload: dict[str, Any]) -> None:
            data = json.dumps(payload).encode("utf-8")
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_POST(self) -> None:  # noqa: N802 -- http.server API name
            try:
                req = self._read_json()
            except (ValueError, json.JSONDecodeError) as exc:
                self._send_json(400, {"error": f"invalid JSON: {exc}"})
                return
            try:
                if self.path.rstrip("/") == "/generate":
                    outputs = generate(
                        req.get("name", ""),
                        req.get("input", ""),
                        req.get("prefix", ""),
                        provider=provider,
                    )
                    self._send_json(200, {"outputs": outputs})
                elif self.path.rstrip("/") == "/encode":
                    outputs = encode(
                        req.get("name", ""),
                        req.get("input", ""),
                        provider=provider,
                    )
                    self._send_json(200, {"outputs": outputs})
                else:
                    self._send_json(404, {"error": f"unknown path: {self.path}"})
            except Exception as exc:  # noqa: BLE001 -- report as 500 JSON
                self._send_json(500, {"error": str(exc)})

    return _Handler


def make_server(
    host: str = "localhost",
    port: int = 23337,
    *,
    provider: Optional[Provider] = None,
) -> ThreadingHTTPServer:
    """Build (but do not start) the tactic HTTP server. ``port=0`` -> ephemeral."""
    return ThreadingHTTPServer((host, port), _make_handler(provider))


def serve(host: str = "localhost", port: int = 23337) -> None:
    """Run the tactic server forever (CLI entrypoint)."""
    server = make_server(host, port)
    host_p, port_p = server.server_address[:2]
    print(f"theoremata tactic server on http://{host_p}:{port_p}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


def main() -> None:
    import argparse

    parser = argparse.ArgumentParser(description="Theoremata tactic server")
    parser.add_argument("--host", default="localhost")
    parser.add_argument("--port", type=int, default=23337)
    args = parser.parse_args()
    serve(args.host, args.port)


if __name__ == "__main__":
    main()
