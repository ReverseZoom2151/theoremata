"""Minimal MCP (Model Context Protocol) stdio server for Theoremata's tools.

This exposes the Python tool workers in :mod:`theoremata_tools.worker` to any
MCP-capable client, without depending on the ``mcp`` pip package. It speaks
JSON-RPC 2.0 over stdio using **newline-delimited JSON framing**: each request
and each response is a single JSON object on its own line (``\\n``-terminated),
read from stdin and written to stdout. (This is a deliberately simple
alternative to LSP-style ``Content-Length`` framing; a compliant client that
sends one JSON message per line interoperates directly.)

Methods handled:

* ``initialize``  -> server info + capabilities.
* ``tools/list``  -> catalog of Theoremata tools as MCP tool descriptors
  ``{name, description, inputSchema}``.
* ``tools/call``  -> ``{name, arguments}`` is turned into a worker ``request``
  dict, passed to :func:`theoremata_tools.worker.dispatch`, and the output is
  wrapped as an MCP tool result ``{"content": [...], "isError": bool}``.

Tools with a content-bearing (textarea-shaped) field additionally accept
``file_uri``, so a large proof source is read from disk by the server instead of
being paid for in context tokens. Exactly one of the content field or
``file_uri`` must be given; that rule is enforced in handler code rather than as
a schema ``oneOf``, because the Anthropic Messages API and Vertex-via-OpenRouter
reject ``oneOf``/``anyOf``/``allOf`` in a tool ``input_schema``. Reads are
confined to the client's declared MCP roots and refused outright over HTTP.

Notifications (JSON-RPC messages with no ``id``, e.g. ``notifications/*``) get
no response. Unknown methods return error ``-32601``; malformed JSON yields
``-32700``; malformed request objects yield ``-32600``. A tool raising an
exception is reported as an ``isError: true`` tool result, never a crash.

The pure function :func:`handle` maps a single decoded JSON-RPC message to its
response object (or ``None`` for a notification), so it is unit-testable without
touching real stdio. :func:`serve` runs the read/handle/write loop over any two
file objects, and :func:`main` wires it to the process stdin/stdout.
"""
from __future__ import annotations

import copy
import json
import pathlib
import sys
import urllib.parse
import urllib.request
from dataclasses import dataclass, field
from typing import Any, Iterable, Optional, TextIO

from . import worker

PROTOCOL_VERSION = "2024-11-05"
SERVER_NAME = "theoremata"
SERVER_VERSION = "0.1.0"

# JSON-RPC 2.0 standard error codes.
PARSE_ERROR = -32700
INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INVALID_PARAMS = -32602
INTERNAL_ERROR = -32603


# --------------------------------------------------------------------------- #
# Tool catalog                                                                #
# --------------------------------------------------------------------------- #
# Each descriptor is an MCP tool: {name, description, inputSchema}. ``inputSchema``
# is a JSON Schema for the tool's ``arguments``; ``_build_request`` maps those
# arguments to the worker ``request`` dict (which carries the ``tool`` key and
# the fields ``dispatch`` reads). Keeping the mapping explicit lets an argument
# name differ from a worker field when that reads better for a client.

_NUMBER_OR_STRING = {"type": ["number", "string"]}

_TOOLS: list[dict[str, Any]] = [
    {
        "name": "falsify",
        "description": (
            "Bounded counterexample search: enumerate integer assignments over "
            "explicit per-variable domains and report the first case (if any) "
            "that satisfies the assumptions but violates the claim. A heuristic "
            "oracle - a found counterexample is a real refutation; no "
            "counterexample within the bound is not a proof."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "variables": {
                    "type": "object",
                    "description": (
                        "Map of variable name -> integer domain "
                        "{start, stop, step} (Python range semantics; stop "
                        "exclusive). Defaults: start=-20, stop=21, step=1."
                    ),
                    "additionalProperties": {
                        "type": "object",
                        "properties": {
                            "start": {"type": "integer"},
                            "stop": {"type": "integer"},
                            "step": {"type": "integer"},
                        },
                    },
                },
                "claim": {
                    "type": "string",
                    "description": "Boolean expression that should hold for all admissible cases.",
                },
                "assumptions": {
                    "type": "string",
                    "description": "Boolean guard selecting admissible cases (default 'True').",
                    "default": "True",
                },
                "max_cases": {
                    "type": "integer",
                    "description": "Maximum number of cases to check (default 100000).",
                    "default": 100000,
                },
            },
            "required": ["variables", "claim"],
        },
    },
    {
        "name": "symbolic",
        "description": (
            "SymPy-backed symbolic computation. Apply an operation "
            "(simplify, expand, factor, diff, integrate, ...) to an expression, "
            "optionally with respect to a variable. Heuristic oracle - results "
            "must be re-verified in Lean before being trusted."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "description": "Operation name, e.g. simplify, expand, factor, diff, integrate.",
                },
                "expression": {"type": "string", "description": "The expression to operate on."},
                "variable": {
                    "type": "string",
                    "description": "Variable for operations that need one (diff/integrate).",
                },
            },
            "required": ["operation", "expression"],
        },
    },
    {
        "name": "feasibility",
        "description": (
            "Exact rational linear-arithmetic feasibility via Fourier-Motzkin "
            "elimination. Decides whether a system of linear inequalities is "
            "satisfiable over the rationals; returns a satisfying model when "
            "feasible, or a Farkas certificate (re-checkable) when infeasible."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "constraints": {
                    "type": "array",
                    "description": "List of linear constraints.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "coeffs": {
                                "type": "object",
                                "description": "Map of variable name -> coefficient (number or 'p/q' string).",
                                "additionalProperties": _NUMBER_OR_STRING,
                            },
                            "sense": {
                                "type": "string",
                                "enum": ["<=", "<", "=", ">=", ">"],
                            },
                            "rhs": _NUMBER_OR_STRING,
                        },
                        "required": ["coeffs", "sense"],
                    },
                }
            },
            "required": ["constraints"],
        },
    },
    {
        "name": "asymptotic_feasibility",
        "description": (
            "Feasibility of a system of asymptotic (log-linear) constraints, "
            "the log-space analogue of the linear feasibility kernel."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "constraints": {
                    "type": "array",
                    "description": "List of asymptotic constraints.",
                    "items": {"type": "object"},
                }
            },
            "required": ["constraints"],
        },
    },
    {
        "name": "prove_asymptotic",
        "description": (
            "Attempt to prove an asymptotic goal from a set of asymptotic "
            "hypotheses, returning a proof/refutation status with certificate."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "hypotheses": {
                    "type": "array",
                    "description": "List of asymptotic hypotheses.",
                    "items": {"type": "object"},
                },
                "goal": {
                    "type": "object",
                    "description": "The asymptotic goal to establish.",
                },
            },
            "required": ["hypotheses", "goal"],
        },
    },
    {
        "name": "lean_soundness",
        "description": (
            "Cheap lexical Lean soundness PRE-GATE. Flags residual sorry/admit "
            "placeholders and axiom/constant/postulate declarations while "
            "ignoring comments and string literals. Returns "
            "`pregate_clean` (not `clean`) plus a constant `sufficient: false`: "
            "the scan is purely lexical, so a sorry synthesised by a macro or "
            "inherited through an import still yields `pregate_clean: true`. "
            "Necessary-not-sufficient - it must precede, never replace, real "
            "Lean compilation, `check_axioms`, and kernel replay. Use "
            "`check_axioms` for an authoritative verdict."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "Lean source text to scan."}
            },
            "required": ["text"],
        },
    },
    {
        "name": "check_axioms",
        "description": (
            "Run `#print axioms` over a theorem in a Lean project and check the "
            "transitive axiom closure against an allowlist. Requires a working "
            "Lean/Lake toolchain."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "source": {"type": "string", "description": "Lean source text or file path."},
                "theorem": {"type": "string", "description": "Fully-qualified theorem name to audit."},
                "root": {"type": "string", "description": "Lake project root directory."},
                "allowed": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Allowlisted axiom names (default the standard three).",
                },
                "timeout": {"type": "number", "description": "Seconds before aborting (default 300)."},
            },
            "required": ["source", "theorem"],
        },
    },
    {
        "name": "grader",
        "description": (
            "Two-pipeline answer/proof grader routed by grade_kind. Extracts and "
            "verifies a final answer (answer-style) or checks statement identity "
            "and axiom closure (proof-style)."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "request": {
                    "type": "object",
                    "description": "The grader request payload (problem record + submission).",
                }
            },
            "required": ["request"],
        },
    },
    {
        "name": "stages",
        "description": (
            "Research->formal stage engine. Runs one of the domain-neutral stage "
            "templates over a payload and returns the stage's typed output."
        ),
        "inputSchema": {
            "type": "object",
            "description": "Stage request payload (passed through to the stages worker).",
            "properties": {
                "stage": {"type": "string", "description": "Stage name / template id."}
            },
            "additionalProperties": True,
        },
    },
    {
        "name": "mathlib_index",
        "description": (
            "Mathlib retrieval Layer A: source-only import DAG index over a "
            "Mathlib checkout. Query module stats, dependencies, or search "
            "declarations by module/substring."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "root": {"type": "string", "description": "Path to the Mathlib checkout root."},
                "query": {
                    "type": "string",
                    "description": "Query kind (e.g. stats, deps, search). Default 'stats'.",
                    "default": "stats",
                },
                "module": {"type": "string", "description": "Module name for module-scoped queries."},
                "substring": {"type": "string", "description": "Substring filter for searches."},
                "limit": {"type": "integer", "description": "Max results (default 50).", "default": 50},
                "package": {"type": "string", "description": "Package name (default 'Mathlib').", "default": "Mathlib"},
            },
            "required": ["root"],
        },
    },
    {
        "name": "decl_index",
        "description": (
            "Mathlib retrieval Layer B: per-declaration env-dump index "
            "(FQ name, namespace, signature, defining module, const-dep edges). "
            "Requires a Lean toolchain to build the dump."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "root": {"type": "string", "description": "Lake project root directory."},
                "imports": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Modules to import for the environment dump.",
                },
                "query": {"type": "string", "description": "Query kind (default 'dump').", "default": "dump"},
                "kind": {"type": "string", "description": "Declaration kind filter."},
                "substring": {"type": "string", "description": "Substring filter."},
                "limit": {"type": "integer", "description": "Max results (default 50).", "default": 50},
                "lean_bin": {"type": "string", "description": "Path to the lean binary."},
                "timeout": {"type": "number", "description": "Seconds before aborting (default 300)."},
            },
        },
    },
    {
        "name": "head_index",
        "description": (
            "Mathlib retrieval Layer C: #find-style head-symbol bucket index over "
            "an imported environment, for type-aware premise lookup. Requires a "
            "Lean toolchain."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "root": {"type": "string", "description": "Lake project root directory."},
                "imports": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Modules to import for the environment dump.",
                },
                "query": {"type": "string", "description": "Query kind (default 'stats').", "default": "stats"},
                "head": {"type": "string", "description": "Head symbol to look up."},
                "pattern": {"type": "string", "description": "Pattern filter."},
                "limit": {"type": "integer", "description": "Max results (default 50).", "default": 50},
                "lean_bin": {"type": "string", "description": "Path to the lean binary."},
                "timeout": {"type": "number", "description": "Seconds before aborting (default 300)."},
            },
        },
    },
]

# --------------------------------------------------------------------------- #
# file_uri: pay for large proof sources in file I/O, not context tokens        #
# --------------------------------------------------------------------------- #
# A "content-bearing" field is one an agent would fill with a whole Lean file
# rather than a short scalar - the textarea-shaped inputs. For those, the client
# may pass `file_uri` instead and the server reads the file itself, so a 5k-line
# proof never crosses the model's context window.
#
# Only genuinely textarea-shaped fields are listed. `symbolic.expression` is a
# one-line formula and `grader.request` / `stages` are structured objects, so
# neither gains anything from a file indirection.
_CONTENT_FIELDS: dict[str, str] = {
    "lean_soundness": "text",
    "check_axioms": "source",
}


@dataclass
class _ServerConfig:
    """Mutable per-process transport/security state.

    ``roots`` is deliberately three-valued and mirrors MCP's own semantics:

    * ``None``  - the client declared no ``roots`` capability, so it has no
      opinion about the filesystem: unconstrained.
    * ``[]``    - the client declared ``roots`` but exposes none: deny all. This
      is NOT the same as "no opinion", and collapsing the two would turn a
      client that deliberately shares nothing into a client that shares
      everything.
    * ``[...]`` - allowlist; a ``file_uri`` must resolve inside one of them.
    """

    http_mode: bool = False
    roots: Optional[list[pathlib.Path]] = None
    roots_declared: bool = False
    _next_request_id: int = field(default=0, repr=False)


_CONFIG = _ServerConfig()


def reset_config() -> None:
    """Restore the default (stdio, unconstrained) state. For tests/embedders."""
    global _CONFIG
    _CONFIG = _ServerConfig()


def set_http_mode(enabled: bool) -> None:
    """Mark the transport as HTTP, which disables ``file_uri`` entirely.

    Over stdio the server is a child process of the client and shares its
    filesystem, so reading a local path is exactly what the client asked for.
    Over HTTP the caller is remote and the paths are the *server's*, so honoring
    a ``file_uri`` would be an arbitrary-file-read primitive. There is no safe
    root set for that case, so it is refused outright rather than filtered.
    """
    _CONFIG.http_mode = bool(enabled)


def set_client_roots(uris: Optional[Iterable[str]]) -> None:
    """Record the client's declared roots (``None`` = no roots capability)."""
    if uris is None:
        _CONFIG.roots = None
        return
    _CONFIG.roots = [_uri_to_path(u) for u in uris]


def _uri_to_path(uri: str) -> pathlib.Path:
    """Resolve a ``file://`` URI (or a bare path) to an absolute real path.

    ``resolve()`` collapses ``..`` and follows symlinks, which is what makes the
    root containment check below meaningful rather than textual.
    """
    parsed = urllib.parse.urlparse(uri)
    if parsed.scheme == "file":
        raw = urllib.request.url2pathname(parsed.path)
    elif parsed.scheme == "" or len(parsed.scheme) == 1:
        # A one-character "scheme" is a Windows drive letter (`C:\proofs\a.lean`
        # parses as scheme='c'), not a URI scheme. Treat it as a bare path.
        raw = uri
    else:
        raise ValueError(f"unsupported URI scheme for file_uri: {uri!r}")
    return pathlib.Path(raw).expanduser().resolve()


def _inject_file_uri(tool: dict[str, Any]) -> dict[str, Any]:
    """Return a copy of ``tool`` whose schema accepts ``file_uri``.

    The content field is dropped from ``required`` because ``file_uri`` can now
    satisfy the call instead. The "exactly one of content or file_uri" rule is
    then enforced in :func:`_resolve_file_uri` - i.e. in handler code, NOT in
    the schema. That is not a shortcut: the Anthropic Messages API and
    Vertex-via-OpenRouter both reject ``oneOf``/``anyOf``/``allOf`` inside a
    tool ``input_schema``, so expressing the constraint declaratively would make
    the whole catalog unusable by the clients we actually serve.
    """
    name = tool["name"]
    content_field = _CONTENT_FIELDS.get(name)
    if content_field is None:
        return tool
    tool = copy.deepcopy(tool)
    schema = tool["inputSchema"]
    properties = schema.setdefault("properties", {})
    if content_field not in properties:
        return tool
    properties["file_uri"] = {
        "type": "string",
        "description": (
            f"Absolute path or file:// URI to read {content_field!r} from, as an "
            f"alternative to passing it inline. Provide exactly one of "
            f"{content_field!r} or 'file_uri' - never both, never neither. Use "
            "this for large sources so they are not paid for in context tokens. "
            "Must resolve inside the client's declared MCP roots. Stdio only."
        ),
    }
    required = schema.get("required")
    if isinstance(required, list) and content_field in required:
        schema["required"] = [r for r in required if r != content_field]
    return tool


def _resolve_file_uri(name: str, arguments: dict[str, Any]) -> dict[str, Any]:
    """Return ``arguments`` with ``file_uri`` replaced by the file's content.

    Enforces, in handler code, the constraint the schema cannot express:
    exactly one of the content field or ``file_uri``.
    """
    content_field = _CONTENT_FIELDS.get(name)
    uri = arguments.get("file_uri")
    if content_field is None:
        if uri is not None:
            raise ValueError(f"file_uri is not accepted by tool {name!r}")
        return arguments

    has_content = arguments.get(content_field) is not None
    if uri is None:
        if not has_content:
            raise ValueError(
                f"provide exactly one of {content_field!r} or 'file_uri'"
            )
        return arguments
    if has_content:
        raise ValueError(
            f"provide exactly one of {content_field!r} or 'file_uri', not both"
        )
    if _CONFIG.http_mode:
        raise ValueError("file_uri is only supported over stdio, not HTTP")
    if not isinstance(uri, str) or not uri.strip():
        raise ValueError("file_uri must be a non-empty string")

    path = _uri_to_path(uri)
    roots = _CONFIG.roots
    if roots is not None and not any(
        path == root or root in path.parents for root in roots
    ):
        # Checked on the *resolved* path, so `..` traversal and symlinks out of
        # a root are both caught. An empty root list reaches here and matches
        # nothing, which is the intended deny-all.
        raise ValueError(
            f"file_uri is outside the client's declared MCP roots: {uri}"
        )
    if not path.is_file():
        raise ValueError(f"file_uri does not point to a regular file: {uri}")

    resolved = dict(arguments)
    resolved.pop("file_uri", None)
    resolved[content_field] = path.read_text(encoding="utf-8")
    return resolved


def _tool_catalog() -> list[dict[str, Any]]:
    """Return Python tools plus Rust-backed meta-tools when their bridge is live.

    Discovery is intentionally fail-closed: a server launched outside
    ``theoremata mcp`` gets the historical Python-only catalog rather than a
    callable-looking meta-tool that would return a fabricated acknowledgement.
    """
    return [
        *(_inject_file_uri(tool) for tool in _TOOLS),
        *worker.meta_tool_descriptors(),
    ]


def _tools_by_name() -> dict[str, dict[str, Any]]:
    return {tool["name"]: tool for tool in _tool_catalog()}


def _build_request(name: str, arguments: dict[str, Any]) -> dict[str, Any]:
    """Turn an MCP ``tools/call`` (name, arguments) into a worker request dict.

    The request always carries ``tool`` = the tool name; every remaining field
    is taken verbatim from ``arguments`` (the schema names them to match the
    fields ``dispatch`` reads). Unknown tool names raise ``KeyError`` so the
    caller can surface an isError result.

    Keys whose value is an explicit JSON ``null`` are dropped: models routinely
    emit ``{"timeout": null}`` for an optional parameter they do not want to
    set, and consumers read those fields with ``request.get(key, default)``,
    which returns the ``None`` rather than the default whenever the key is
    present. Treating an explicit null as absent makes the two forms behave
    identically, which is what the schema's "optional" already promises.
    """
    if name not in _tools_by_name():
        raise KeyError(f"unknown tool: {name}")
    # Strip nulls first so `{"file_uri": null}` reads as "absent", matching the
    # treatment every other optional argument already gets.
    arguments = {k: v for k, v in arguments.items() if v is not None}
    arguments = _resolve_file_uri(name, arguments)
    request: dict[str, Any] = {"tool": name}
    request.update(arguments)
    return request


# --------------------------------------------------------------------------- #
# JSON-RPC helpers                                                             #
# --------------------------------------------------------------------------- #

def _ok(msg_id: Any, result: Any) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": msg_id, "result": result}


def _err(msg_id: Any, code: int, message: str, data: Any = None) -> dict[str, Any]:
    error: dict[str, Any] = {"code": code, "message": message}
    if data is not None:
        error["data"] = data
    return {"jsonrpc": "2.0", "id": msg_id, "error": error}


def _tool_result(output: Any) -> dict[str, Any]:
    return {
        "content": [{"type": "text", "text": json.dumps(output, default=repr)}],
        "isError": False,
    }


def _tool_error(message: str) -> dict[str, Any]:
    return {
        "content": [{"type": "text", "text": message}],
        "isError": True,
    }


# --------------------------------------------------------------------------- #
# Method dispatch                                                             #
# --------------------------------------------------------------------------- #

def _handle_tools_call(msg_id: Any, params: dict[str, Any]) -> dict[str, Any]:
    name = params.get("name")
    if not isinstance(name, str):
        return _err(msg_id, INVALID_PARAMS, "tools/call requires a string 'name'")
    arguments = params.get("arguments", {})
    if arguments is None:
        arguments = {}
    if not isinstance(arguments, dict):
        return _err(msg_id, INVALID_PARAMS, "tools/call 'arguments' must be an object")
    if name not in _tools_by_name():
        # An unknown tool is a tool-level error, not a protocol error, so the
        # model can recover rather than the call crashing.
        return _ok(msg_id, _tool_error(f"unknown tool: {name}"))
    try:
        request = _build_request(name, arguments)
        output = worker.dispatch(request)
    except Exception as exc:  # noqa: BLE001 - report any tool failure as isError.
        return _ok(msg_id, _tool_error(f"{type(exc).__name__}: {exc}"))
    # Rust API failures are valid JSON envelopes, but at the MCP boundary they
    # remain tool failures. Returning them as a successful content result would
    # let a caller mistake an unavailable/forbidden action for completion.
    if isinstance(output, dict) and output.get("result") == "error":
        return _ok(msg_id, _tool_error(json.dumps(output, default=repr)))
    return _ok(msg_id, _tool_result(output))


def handle(message: dict[str, Any]) -> Optional[dict[str, Any]]:
    """Map one decoded JSON-RPC message to its response (``None`` = notification).

    Pure and stdio-free, so it is directly unit-testable. Malformed request
    objects yield an ``INVALID_REQUEST`` error; a request with no ``id`` is
    treated as a notification and returns ``None``.
    """
    if not isinstance(message, dict):
        return _err(None, INVALID_REQUEST, "request must be a JSON object")

    method = message.get("method")
    msg_id = message.get("id")
    is_notification = "id" not in message

    if not isinstance(method, str):
        if is_notification:
            return None
        return _err(msg_id, INVALID_REQUEST, "request missing string 'method'")

    params = message.get("params", {})
    if params is None:
        params = {}
    if not isinstance(params, dict):
        if is_notification:
            return None
        return _err(msg_id, INVALID_PARAMS, "'params' must be an object")

    # Notifications never get a response, whatever the method.
    if is_notification:
        return None

    if method == "initialize":
        # Three-valued roots state starts here. A client that declares the
        # `roots` capability is treated as deny-all until its actual root list
        # arrives (see `_request_client_roots`), so a `file_uri` racing the
        # handshake fails closed rather than reading an unvetted path.
        capabilities = params.get("capabilities")
        declared = isinstance(capabilities, dict) and "roots" in capabilities
        _CONFIG.roots_declared = declared
        _CONFIG.roots = [] if declared else None
        return _ok(
            msg_id,
            {
                "protocolVersion": PROTOCOL_VERSION,
                "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
                "capabilities": {"tools": {}},
            },
        )
    if method == "ping":
        return _ok(msg_id, {})
    if method == "tools/list":
        return _ok(msg_id, {"tools": _tool_catalog()})
    if method == "tools/call":
        return _handle_tools_call(msg_id, params)

    return _err(msg_id, METHOD_NOT_FOUND, f"unknown method: {method}")


# --------------------------------------------------------------------------- #
# stdio loop                                                                  #
# --------------------------------------------------------------------------- #

def serve(stdin: TextIO, stdout: TextIO) -> None:
    """Run the newline-delimited JSON-RPC loop over two text streams.

    Reads one JSON message per line; writes at most one JSON response line per
    request. Blank lines are skipped. A line that is not valid JSON produces a
    ``PARSE_ERROR`` response. The loop ends at EOF.
    """
    for line in stdin:
        line = line.strip()
        if not line:
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError as exc:
            response: Optional[dict[str, Any]] = _err(
                None, PARSE_ERROR, f"parse error: {exc}"
            )
        else:
            response = handle(message)
            if (
                isinstance(message, dict)
                and message.get("method") == "notifications/initialized"
            ):
                # The handshake is complete, so the server may now send its own
                # requests. This is the only point at which the client's roots
                # can be learned.
                _request_client_roots(stdin, stdout)
        if response is not None:
            stdout.write(json.dumps(response, default=repr) + "\n")
            stdout.flush()


def _request_client_roots(stdin: TextIO, stdout: TextIO) -> None:
    """Issue a ``roots/list`` request and apply the reply, or stay deny-all.

    Our transport is a synchronous reactive loop, so the round-trip is done
    inline: write the request, then keep pumping lines - answering any client
    traffic that arrives in the meantime - until our own ``id`` comes back.

    Every failure path (client error, EOF, malformed reply) deliberately leaves
    the deny-all state set by ``initialize`` in place. A server that could not
    learn its roots must not act as if it had none.
    """
    if not _CONFIG.roots_declared:
        return
    _CONFIG._next_request_id += 1
    request_id = f"theoremata-roots-{_CONFIG._next_request_id}"
    stdout.write(
        json.dumps({"jsonrpc": "2.0", "id": request_id, "method": "roots/list"}) + "\n"
    )
    stdout.flush()

    for line in stdin:
        line = line.strip()
        if not line:
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(message, dict) and message.get("id") == request_id:
            result = message.get("result")
            if isinstance(result, dict) and isinstance(result.get("roots"), list):
                uris = [
                    r["uri"]
                    for r in result["roots"]
                    if isinstance(r, dict) and isinstance(r.get("uri"), str)
                ]
                try:
                    set_client_roots(uris)
                except ValueError:
                    # An unparseable root is not permission to read anything.
                    _CONFIG.roots = []
            return
        # Unrelated client traffic during the round-trip is served normally.
        response = handle(message) if isinstance(message, dict) else None
        if response is not None:
            stdout.write(json.dumps(response, default=repr) + "\n")
            stdout.flush()


def main() -> None:
    serve(sys.stdin, sys.stdout)


if __name__ == "__main__":
    main()
