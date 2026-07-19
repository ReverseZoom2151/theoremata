"""Tests for the MCP stdio server, driving ``handle`` directly (no subprocess)."""
from __future__ import annotations

import json
from types import SimpleNamespace

from theoremata_tools import worker
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


def test_explicit_null_optional_arguments_fall_back_to_defaults():
    # Models routinely emit `"max_cases": null` / `"assumptions": null` to mean
    # "leave this optional parameter unset". Before nulls were stripped, the
    # null reached `int(request.get("max_cases", 100_000))` and raised
    # TypeError, surfacing as a tool error that read like a model failure.
    base = {
        "name": "falsify",
        "arguments": {"variables": {"x": {"start": 0, "stop": 3}}, "claim": "x >= 0"},
    }
    with_nulls = {
        "name": "falsify",
        "arguments": {
            "variables": {"x": {"start": 0, "stop": 3}},
            "claim": "x >= 0",
            "assumptions": None,
            "max_cases": None,
        },
    }
    omitted = handle(_rpc("tools/call", base))["result"]
    explicit_null = handle(_rpc("tools/call", with_nulls))["result"]
    assert omitted["isError"] is False
    assert explicit_null["isError"] is False, explicit_null["content"][0]["text"]
    # An absent key and a key present with null must behave identically.
    assert _parse_tool_text(explicit_null) == _parse_tool_text(omitted)


def test_explicit_null_is_stripped_before_the_worker_sees_it():
    from theoremata_tools.mcp_server import _build_request

    request = _build_request(
        "falsify",
        {"variables": {"x": {"start": 0, "stop": 1}}, "claim": "x >= 0", "max_cases": None},
    )
    assert "max_cases" not in request
    assert request["tool"] == "falsify"
    assert request["claim"] == "x >= 0"


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


def test_rust_meta_bridge_uses_explicit_context_and_never_invents_accepted(monkeypatch):
    calls = []

    def fake_run(command, **kwargs):
        calls.append((command, kwargs))
        request = json.loads(command[-1])
        if request["op"] == "list_meta_tools":
            response = {
                "version": "1",
                "result": "meta_tools",
                "tools": [
                    {
                        "name": "plan",
                        "description": "Plan a proof.",
                        "inputSchema": {"type": "object", "properties": {}},
                    }
                ],
            }
        else:
            assert request == {
                "op": "invoke_meta_tool",
                "tool": "plan",
                "arguments": {"statement": "P"},
            }
            response = {
                "version": "1",
                "result": "meta_tool_invoked",
                "tool": "plan",
                "output": {"accepted": True, "worker_op": "meta_plan"},
            }
        return SimpleNamespace(returncode=0, stdout=json.dumps(response), stderr="")

    monkeypatch.setenv("THEOREMATA_MCP_API_COMMAND", "/opt/theoremata")
    monkeypatch.setenv("THEOREMATA_MCP_DATABASE", "/tmp/theoremata.db")
    monkeypatch.setenv("THEOREMATA_MCP_CONFIG", "/tmp/theoremata.toml")
    monkeypatch.setattr(worker.subprocess, "run", fake_run)

    descriptors = worker.meta_tool_descriptors()
    assert descriptors == [
        {
            "name": "meta_plan",
            "description": "Store-backed Rust orchestration API. Certification is forbidden. Plan a proof.",
            "inputSchema": {"type": "object", "properties": {}},
        }
    ]
    output = worker.dispatch({"tool": "meta_plan", "statement": "P"})
    assert output["result"] == "meta_tool_invoked"
    assert output["tool"] == "meta_plan"
    assert output["output"] == {"worker_op": "meta_plan"}
    assert output["bridge"] == {
        "backend": "rust_api",
        "store_backed": True,
        "certification": "forbidden",
    }
    assert all(
        command[:6]
        == [
            "/opt/theoremata",
            "--config",
            "/tmp/theoremata.toml",
            "api",
            "--database",
            "/tmp/theoremata.db",
        ]
        for command, _ in calls
    )
    assert all(kwargs.get("shell", False) is False for _, kwargs in calls)


def test_meta_bridge_rejects_certification_before_spawning(monkeypatch):
    monkeypatch.setenv("THEOREMATA_MCP_API_COMMAND", "/opt/theoremata")
    monkeypatch.setenv("THEOREMATA_MCP_DATABASE", "/tmp/theoremata.db")
    monkeypatch.setattr(
        worker.subprocess,
        "run",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(AssertionError("must not spawn")),
    )

    try:
        worker.dispatch(
            {"tool": "meta_self_review", "candidate": {"status": "formally_verified"}}
        )
    except ValueError as exc:
        assert "certification requires proof evidence" in str(exc)
    else:
        raise AssertionError("certification-shaped meta input must be rejected")


def test_mcp_meta_api_error_is_a_tool_error_not_success(monkeypatch):
    descriptor = {
        "name": "meta_plan",
        "description": "Store-backed Rust orchestration API.",
        "inputSchema": {"type": "object", "properties": {}},
    }
    monkeypatch.setattr(worker, "meta_tool_descriptors", lambda: [descriptor])
    monkeypatch.setattr(
        worker,
        "dispatch",
        lambda request: {
            "version": "1",
            "result": "error",
            "code": "forbidden",
            "message": "certification requires proof evidence",
        },
    )

    response = handle(_rpc("tools/call", {"name": "meta_plan", "arguments": {}}))
    assert response["result"]["isError"] is True
    payload = _parse_tool_text(response["result"])
    assert payload["code"] == "forbidden"
