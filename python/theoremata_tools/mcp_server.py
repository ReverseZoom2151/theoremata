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

import json
import sys
from typing import Any, Optional, TextIO

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
            "Cheap lexical Lean soundness pre-gate. Flags residual sorry/admit "
            "placeholders and axiom/constant/postulate declarations while "
            "ignoring comments and string literals. Necessary-not-sufficient: it "
            "must precede, never replace, real Lean compilation and kernel replay."
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

_TOOLS_BY_NAME = {t["name"]: t for t in _TOOLS}


def _build_request(name: str, arguments: dict[str, Any]) -> dict[str, Any]:
    """Turn an MCP ``tools/call`` (name, arguments) into a worker request dict.

    The request always carries ``tool`` = the tool name; every remaining field
    is taken verbatim from ``arguments`` (the schema names them to match the
    fields ``dispatch`` reads). Unknown tool names raise ``KeyError`` so the
    caller can surface an isError result.
    """
    if name not in _TOOLS_BY_NAME:
        raise KeyError(f"unknown tool: {name}")
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
    if name not in _TOOLS_BY_NAME:
        # An unknown tool is a tool-level error, not a protocol error, so the
        # model can recover rather than the call crashing.
        return _ok(msg_id, _tool_error(f"unknown tool: {name}"))
    try:
        request = _build_request(name, arguments)
        output = worker.dispatch(request)
    except Exception as exc:  # noqa: BLE001 - report any tool failure as isError.
        return _ok(msg_id, _tool_error(f"{type(exc).__name__}: {exc}"))
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
        return _ok(msg_id, {"tools": _TOOLS})
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
        if response is not None:
            stdout.write(json.dumps(response, default=repr) + "\n")
            stdout.flush()


def main() -> None:
    serve(sys.stdin, sys.stdout)


if __name__ == "__main__":
    main()
