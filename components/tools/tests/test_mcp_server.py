"""Tests for the MCP stdio server, driving ``handle`` directly (no subprocess)."""
from __future__ import annotations

import json

from theoremata_tools.mcp_server import (
    METHOD_NOT_FOUND,
    handle,
    serve,
)


def _rpc(method, params=None, msg_id=1):
    msg = {"jsonrpc": "2.0", "id": msg_id, "method": method}
    if params is not None:
        msg["params"] = params
    return msg


def _parse_tool_text(result):
    """Extract and JSON-parse the single text content block of a tool result."""
    content = result["content"]
    assert isinstance(content, list) and content, "content must be a non-empty list"
    block = content[0]
    assert block["type"] == "text"
    return json.loads(block["text"])


def test_initialize_returns_server_info():
    resp = handle(_rpc("initialize"))
    assert resp["jsonrpc"] == "2.0"
    assert resp["id"] == 1
    result = resp["result"]
    assert result["serverInfo"]["name"] == "theoremata"
    assert result["serverInfo"]["version"]
    assert "protocolVersion" in result
    assert "tools" in result["capabilities"]


def test_tools_list_is_nonempty_and_well_formed():
    resp = handle(_rpc("tools/list"))
    tools = resp["result"]["tools"]
    assert isinstance(tools, list) and tools, "tool catalog must be non-empty"
    names = set()
    for tool in tools:
        assert isinstance(tool["name"], str) and tool["name"]
        assert isinstance(tool["description"], str) and tool["description"]
        schema = tool["inputSchema"]
        assert isinstance(schema, dict)
        assert schema.get("type") == "object"
        assert "properties" in schema
        names.add(tool["name"])
    # The main tools called out in the spec must all be present.
    for expected in (
        "falsify",
        "symbolic",
        "feasibility",
        "prove_asymptotic",
        "lean_soundness",
        "check_axioms",
        "grader",
        "stages",
        "mathlib_index",
        "decl_index",
        "head_index",
    ):
        assert expected in names, f"missing tool descriptor: {expected}"


def test_tools_call_feasibility_satisfiable():
    params = {
        "name": "feasibility",
        "arguments": {
            "constraints": [
                {"coeffs": {"x": 1}, "sense": ">=", "rhs": 0},
                {"coeffs": {"x": 1}, "sense": "<=", "rhs": 1},
            ]
        },
    }
    resp = handle(_rpc("tools/call", params))
    result = resp["result"]
    assert result["isError"] is False
    payload = _parse_tool_text(result)
    assert payload["feasible"] is True


def test_tools_call_lean_soundness_flags_sorry():
    params = {
        "name": "lean_soundness",
        "arguments": {"text": "theorem foo : 1 = 1 := by sorry"},
    }
    resp = handle(_rpc("tools/call", params))
    result = resp["result"]
    assert result["isError"] is False
    payload = _parse_tool_text(result)
    assert payload["clean"] is False
    assert any(issue.get("token") == "sorry" for issue in payload["issues"])


def test_tools_call_unknown_tool_is_error_result():
    params = {"name": "does_not_exist", "arguments": {}}
    resp = handle(_rpc("tools/call", params))
    # Unknown tool is a tool-level error, not a JSON-RPC protocol error.
    result = resp["result"]
    assert result["isError"] is True


def test_tools_call_tool_exception_becomes_error_result():
    # feasibility with no 'constraints' argument -> dispatch raises KeyError.
    params = {"name": "feasibility", "arguments": {}}
    resp = handle(_rpc("tools/call", params))
    assert "result" in resp
    assert resp["result"]["isError"] is True


def test_unknown_method_returns_minus_32601():
    resp = handle(_rpc("no/such/method"))
    assert "error" in resp
    assert resp["error"]["code"] == METHOD_NOT_FOUND


def test_notification_returns_none():
    # No 'id' -> notification -> no response.
    assert handle({"jsonrpc": "2.0", "method": "notifications/initialized"}) is None


def test_serve_loop_roundtrip(tmp_path):
    import io

    stdin = io.StringIO(
        json.dumps(_rpc("initialize"))
        + "\n"
        + "\n"  # blank line is skipped
        + json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"})
        + "\n"
        + json.dumps(_rpc("tools/list", msg_id=2))
        + "\n"
    )
    stdout = io.StringIO()
    serve(stdin, stdout)
    lines = [ln for ln in stdout.getvalue().splitlines() if ln]
    # Two responses: initialize + tools/list (the notification produced none).
    assert len(lines) == 2
    first = json.loads(lines[0])
    assert first["result"]["serverInfo"]["name"] == "theoremata"
    second = json.loads(lines[1])
    assert second["id"] == 2
    assert second["result"]["tools"]


def test_serve_malformed_line_returns_parse_error():
    import io

    stdin = io.StringIO("{not valid json}\n")
    stdout = io.StringIO()
    serve(stdin, stdout)
    resp = json.loads(stdout.getvalue().strip())
    assert resp["error"]["code"] == -32700
