"""JSON-lines worker entrypoint."""
from __future__ import annotations

import json
import sys
from typing import Any

from .estimates_adapter import capability as estimates_capability
from .falsify import search
from .feasibility import feasibility
from .lean_soundness import check as lean_soundness_check
from .mathlib_index import run as mathlib_index_run
from .safe_eval import evaluate
from .symbolic import run as symbolic_run


def dispatch(request: dict[str, Any]) -> dict[str, Any]:
    tool = request.get("tool", "evaluate")
    if tool == "evaluate":
        return {"result": evaluate(request["expression"], request.get("variables"))}
    if tool == "falsify":
        return search(
            variables=request["variables"],
            claim=request["claim"],
            assumptions=request.get("assumptions", "True"),
            max_cases=int(request.get("max_cases", 100_000)),
        )
    if tool == "symbolic":
        return symbolic_run(
            request["operation"], request["expression"], request.get("variable")
        )
    if tool == "estimates_capability":
        return estimates_capability(request.get("resources", "resources"))
    if tool == "lean_soundness":
        return lean_soundness_check(request["text"])
    if tool == "feasibility":
        return feasibility(request["constraints"])
    if tool == "mathlib_index":
        return mathlib_index_run(
            root=request["root"],
            query=request.get("query", "stats"),
            module=request.get("module"),
            substring=request.get("substring"),
            limit=int(request.get("limit", 50)),
            package=request.get("package", "Mathlib"),
        )
    raise ValueError(f"unknown tool: {tool}")


def main() -> None:
    request = json.load(sys.stdin)
    try:
        response = {"ok": True, "output": dispatch(request)}
    except Exception as exc:
        response = {"ok": False, "error": str(exc)}
    print(json.dumps(response, default=repr))
    raise SystemExit(0 if response["ok"] else 2)


if __name__ == "__main__":
    main()
