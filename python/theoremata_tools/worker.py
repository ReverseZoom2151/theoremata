"""JSON-lines worker entrypoint."""
from __future__ import annotations

import json
import sys
from typing import Any

from .asymptotics import asymptotic_feasibility, prove_asymptotic
from .axioms import check_axioms
from .decl_index import run as decl_index_run
from .eval_harness import run as eval_run
from .head_index import run as head_index_run
from .lean_workspace import place_proof as lean_workspace_place
from .lean_workspace import scaffold as lean_workspace_scaffold
from .estimates_adapter import capability as estimates_capability
from .falsify import search
from .feasibility import feasibility
from .grader import run as grader_run
from .lean_soundness import check as lean_soundness_check
from .mathlib_index import run as mathlib_index_run
from .retrieval import run as retrieval_run
from .safe_eval import evaluate
from .stages import run as stages_run
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
    if tool == "check_axioms":
        return check_axioms(
            source=request["source"],
            theorem=request["theorem"],
            root=request.get("root"),
            allowed=request.get("allowed"),
            timeout=float(request.get("timeout", 300.0)),
        )
    if tool == "stages":
        return stages_run(request)
    if tool == "feasibility":
        return feasibility(request["constraints"])
    if tool == "asymptotic_feasibility":
        return asymptotic_feasibility(request["constraints"])
    if tool == "prove_asymptotic":
        return prove_asymptotic(request["hypotheses"], request["goal"])
    if tool == "grader":
        return grader_run(request["request"])
    if tool == "eval":
        return eval_run(request["request"])
    if tool == "retrieve":
        return retrieval_run(
            root=request.get("root"),
            imports=request.get("imports"),
            query=request["query"],
            limit=int(request.get("limit", 20)),
            op=request.get("op", "retrieve"),
        )
    if tool == "mathlib_index":
        return mathlib_index_run(
            root=request["root"],
            query=request.get("query", "stats"),
            module=request.get("module"),
            substring=request.get("substring"),
            limit=int(request.get("limit", 50)),
            package=request.get("package", "Mathlib"),
        )
    if tool == "decl_index":
        return decl_index_run(
            root=request.get("root"),
            imports=request.get("imports"),
            query=request.get("query", "dump"),
            kind=request.get("kind"),
            substring=request.get("substring"),
            limit=int(request.get("limit", 50)),
            lean_bin=request.get("lean_bin"),
            timeout=float(request.get("timeout", 300.0)),
        )
    if tool == "lean_workspace_scaffold":
        return lean_workspace_scaffold(request["target_dir"], request["mathlib_root"])
    if tool == "lean_workspace_place":
        return lean_workspace_place(
            request["workspace_dir"], request["module_name"], request["source"]
        )
    if tool == "head_index":
        return head_index_run(
            root=request.get("root"),
            imports=request.get("imports"),
            query=request.get("query", "stats"),
            head=request.get("head"),
            pattern=request.get("pattern"),
            limit=int(request.get("limit", 50)),
            lean_bin=request.get("lean_bin"),
            timeout=float(request.get("timeout", 300.0)),
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
