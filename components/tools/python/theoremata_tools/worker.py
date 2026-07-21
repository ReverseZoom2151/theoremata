"""JSON-lines worker entrypoint."""
from __future__ import annotations

import json
import os
import subprocess
import sys
from typing import Any

from .asymptotics import asymptotic_feasibility, prove_asymptotic
from .axioms import check_axioms
from .decl_index import run as decl_index_run
from .eval_harness import run as eval_run
from .head_index import run as head_index_run
from .lean_repl import run as lean_repl_run
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


# Rust owns the graph Store and the stable JSON API.  These meta-tool aliases
# are deliberately disjoint from the Python workers: Python forwards the
# request to the configured `theoremata api` subprocess and never opens SQLite.
_META_TOOL_OPS = {
    "meta_plan": "plan",
    "meta_update_plan": "update_plan",
    "meta_critique": "critique",
    "meta_redecompose": "redecompose",
    "meta_recall": "recall",
    "meta_spend": "spend",
    "meta_budget": "budget",
    "meta_self_review": "self_review",
    "meta_abstain": "abstain",
}
_META_BRIDGE_TIMEOUT_SECONDS = 30.0
_MAX_META_BRIDGE_BYTES = 256 * 1024


def is_meta_tool_op(tool: str) -> bool:
    """Whether ``tool`` is a Rust-owned orchestration meta-tool alias."""
    return tool in _META_TOOL_OPS


def _contains_formally_verified(value: Any) -> bool:
    if isinstance(value, str):
        return value.casefold() == "formally_verified"
    if isinstance(value, dict):
        return any(_contains_formally_verified(item) for item in value.values())
    if isinstance(value, (list, tuple)):
        return any(_contains_formally_verified(item) for item in value)
    return False


def _without_accepted_markers(value: Any) -> Any:
    """Remove registry-dispatch acknowledgements from a meta-tool response."""
    if isinstance(value, dict):
        return {
            key: _without_accepted_markers(item)
            for key, item in value.items()
            if key != "accepted"
        }
    if isinstance(value, list):
        return [_without_accepted_markers(item) for item in value]
    if isinstance(value, tuple):
        return tuple(_without_accepted_markers(item) for item in value)
    return value


def _meta_bridge_command() -> list[str]:
    """Build the exact Store-backed Rust API command from MCP-owned context."""
    executable = os.environ.get("THEOREMATA_MCP_API_COMMAND", "").strip()
    database = os.environ.get("THEOREMATA_MCP_DATABASE", "").strip()
    if not executable or not database:
        raise RuntimeError(
            "meta-tools require the theoremata MCP bridge context; "
            "start this server with `theoremata mcp`"
        )
    command = [executable]
    config = os.environ.get("THEOREMATA_MCP_CONFIG", "").strip()
    if config:
        command.extend(["--config", config])
    command.extend(["api", "--database", database])
    return command


def _meta_bridge_timeout() -> float:
    raw = os.environ.get("THEOREMATA_MCP_BRIDGE_TIMEOUT_SECONDS", "")
    if not raw:
        return _META_BRIDGE_TIMEOUT_SECONDS
    try:
        return min(60.0, max(1.0, float(raw)))
    except ValueError:
        return _META_BRIDGE_TIMEOUT_SECONDS


def _call_meta_api(request: dict[str, Any]) -> dict[str, Any]:
    """Call the versioned Rust API and fail closed on malformed responses."""
    payload = json.dumps(request, separators=(",", ":"))
    if len(payload.encode("utf-8")) > _MAX_META_BRIDGE_BYTES:
        raise ValueError("meta-tool request exceeds the Rust API size limit")
    try:
        completed = subprocess.run(
            [*_meta_bridge_command(), payload],
            check=False,
            capture_output=True,
            text=True,
            timeout=_meta_bridge_timeout(),
        )
    except subprocess.TimeoutExpired as exc:
        raise RuntimeError("Rust meta-tool API timed out") from exc
    except OSError as exc:
        raise RuntimeError(f"could not start Rust meta-tool API: {exc}") from exc
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise RuntimeError(f"Rust meta-tool API exited {completed.returncode}: {detail[:512]}")
    stdout = completed.stdout.strip()
    if len(stdout.encode("utf-8")) > _MAX_META_BRIDGE_BYTES:
        raise RuntimeError("Rust meta-tool API response exceeds the size limit")
    try:
        response = json.loads(stdout)
    except json.JSONDecodeError as exc:
        raise RuntimeError("Rust meta-tool API returned invalid JSON") from exc
    if not isinstance(response, dict) or response.get("version") != "1":
        raise RuntimeError("Rust meta-tool API returned an unsupported response envelope")
    return response


def meta_tool_descriptors() -> list[dict[str, Any]]:
    """Return MCP descriptors from Rust, or none when the bridge is unavailable.

    The MCP server only advertises these tools after its parent `theoremata mcp`
    process supplied an executable and database. A failed discovery is treated
    as unavailable rather than advertising a callable-looking stub.
    """
    try:
        response = _call_meta_api({"op": "list_meta_tools"})
    except (RuntimeError, ValueError):
        return []
    if response.get("result") != "meta_tools":
        return []
    tools = response.get("tools")
    if not isinstance(tools, list):
        return []
    descriptors: list[dict[str, Any]] = []
    for descriptor in tools:
        if not isinstance(descriptor, dict):
            return []
        name = descriptor.get("name")
        description = descriptor.get("description")
        schema = descriptor.get("inputSchema")
        if not isinstance(name, str) or name not in _META_TOOL_OPS.values():
            return []
        if not isinstance(description, str) or not isinstance(schema, dict):
            return []
        descriptors.append(
            {
                "name": f"meta_{name}",
                "description": (
                    "Store-backed Rust orchestration API. Certification is forbidden. "
                    + description
                ),
                "inputSchema": schema,
            }
        )
    return descriptors


def _dispatch_meta_tool(request: dict[str, Any]) -> dict[str, Any]:
    tool = request.get("tool")
    if not isinstance(tool, str) or tool not in _META_TOOL_OPS:
        raise ValueError("unknown Rust meta-tool")
    arguments = {key: value for key, value in request.items() if key != "tool"}
    if _contains_formally_verified(arguments):
        raise ValueError(
            "meta-tools cannot assign formally_verified; certification requires proof evidence"
        )
    response = _call_meta_api(
        {
            "op": "invoke_meta_tool",
            "tool": _META_TOOL_OPS[tool],
            "arguments": arguments,
        }
    )
    if response.get("result") == "error":
        return response
    if response.get("result") != "meta_tool_invoked" or response.get("tool") != _META_TOOL_OPS[tool]:
        raise RuntimeError("Rust meta-tool API returned an unexpected invocation response")
    output = response.get("output")
    if _contains_formally_verified(output):
        raise RuntimeError("Rust meta-tool API attempted to return a certification result")
    # The current Rust API's registry exposes a dispatch description, not a
    # completed orchestration action. Do not turn that description into a fake
    # acknowledgement for MCP clients.
    output = _without_accepted_markers(output)
    return {
        "version": response["version"],
        "result": "meta_tool_invoked",
        "tool": tool,
        "output": output,
        "bridge": {"backend": "rust_api", "store_backed": True, "certification": "forbidden"},
    }


def dispatch(request: dict[str, Any]) -> dict[str, Any]:
    tool = request.get("tool", "evaluate")
    if isinstance(tool, str) and is_meta_tool_op(tool):
        return _dispatch_meta_tool(request)
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
    if tool == "triviality":
        from theoremata_tools.triviality import run as triviality_run

        return triviality_run(request)
    if tool == "asymptotic_feasibility":
        return asymptotic_feasibility(request["constraints"])
    if tool == "prove_asymptotic":
        return prove_asymptotic(request["hypotheses"], request["goal"])
    if tool == "grader":
        return grader_run(request["request"])
    if tool == "eval":
        return eval_run(request["request"])
    if tool == "eval_execution":
        from theoremata_tools.eval_execution import run as eval_execution_run

        return eval_execution_run(request)
    if tool == "evolve":
        from .evolve import run as evolve_run

        return evolve_run(request["request"])
    if tool == "sft_export":
        from .sft_export import run as sft_export_run

        return sft_export_run(request["request"])
    if tool == "grpo":
        from .grpo import run as grpo_run

        return grpo_run(request["request"])
    if tool == "leandojo":
        from theoremata_tools.leandojo_adapter import run as leandojo_run

        payload = request.get("request", request)
        return leandojo_run(payload if isinstance(payload, dict) else request)
    if tool == "retrieve":
        return retrieval_run(
            root=request.get("root"),
            imports=request.get("imports"),
            query=request["query"],
            limit=int(request.get("limit", 20)),
            op=request.get("op", "retrieve"),
            theorem_module=request.get("theorem_module"),
            theorem_line=request.get("theorem_line"),
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
    if tool == "lean_warm":
        return lean_repl_run(request)
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
    if tool == "rerank":
        from .reranker import run as reranker_run

        return reranker_run(
            query=request["query"],
            candidates=request.get("candidates", []),
            k=request.get("k"),
            samples=int(request.get("samples", 1)),
        )
    if tool == "loglinarith":
        from .log_linarith import evaluate as loglinarith_evaluate

        return loglinarith_evaluate(request)
    if tool == "linprog_cert":
        from .linprog_cert import evaluate as linprog_cert_evaluate

        return linprog_cert_evaluate(request)
    if tool == "cert_log":
        from theoremata_tools.cert_log import run as cert_log_run

        return cert_log_run(request)
    if tool == "cert_flyspeck_lp":
        from theoremata_tools.cert_flyspeck_lp import run as cert_flyspeck_lp_run

        return cert_flyspeck_lp_run(request)
    if tool == "cert_nullstellensatz":
        from theoremata_tools.cert_nullstellensatz import run as cert_nullstellensatz_run

        return cert_nullstellensatz_run(request)
    if tool == "cert_wz":
        from theoremata_tools.cert_wz import run as cert_wz_run

        return cert_wz_run(request)
    if tool == "cert_pratt":
        from theoremata_tools.cert_pratt import run as cert_pratt_run

        return cert_pratt_run(request)
    if tool == "cert_sos":
        from theoremata_tools.cert_sos import run as cert_sos_run

        return cert_sos_run(request)
    if tool == "cert_bernstein":
        from theoremata_tools.cert_bernstein import run as cert_bernstein_run

        return cert_bernstein_run(request)
    if tool == "cert_bezout":
        from theoremata_tools.cert_bezout import run as cert_bezout_run

        return cert_bezout_run(request)
    if tool == "cert_bnb":
        from theoremata_tools.cert_bnb import run as cert_bnb_run

        return cert_bnb_run(request)
    if tool == "cert_herbrand":
        from theoremata_tools.cert_herbrand import run as cert_herbrand_run

        return cert_herbrand_run(request)
    if tool == "cert_pocklington":
        from theoremata_tools.cert_pocklington import run as cert_pocklington_run

        return cert_pocklington_run(request)
    if tool == "cert_positivstellensatz":
        from theoremata_tools.cert_positivstellensatz import run as cert_positivstellensatz_run

        return cert_positivstellensatz_run(request)
    if tool == "cert_sturm":
        from theoremata_tools.cert_sturm import run as cert_sturm_run

        return cert_sturm_run(request)
    if tool == "formalizing_100":
        from theoremata_tools.benchmarks.formalizing_100 import load_formalizing_100

        return {"op": "load", "name": "formalizing_100", "items": load_formalizing_100()}
    if tool == "formalization_reward":
        from theoremata_tools.formalization_reward import run as formalization_reward_run

        return formalization_reward_run(request)
    if tool == "curriculum_synth":
        from theoremata_tools.curriculum_synth import run as curriculum_synth_run

        return curriculum_synth_run(request)
    if tool == "error_keyed_retrieval":
        from theoremata_tools.error_keyed_retrieval import run as error_keyed_retrieval_run

        return error_keyed_retrieval_run(request)
    if tool == "format_filters":
        from theoremata_tools.format_filters import run as format_filters_run

        return format_filters_run(request)
    if tool == "trajectory_recycler":
        from theoremata_tools.trajectory_recycler import run as trajectory_recycler_run

        return trajectory_recycler_run(request)
    if tool == "roundtrip_audit":
        from theoremata_tools.roundtrip_audit import run as roundtrip_audit_run

        return roundtrip_audit_run(request)
    if tool == "trajectory_eval":
        from theoremata_tools.trajectory_eval import run as trajectory_eval_run

        return trajectory_eval_run(request)
    if tool == "eval_integrity":
        from theoremata_tools.eval_integrity import run as eval_integrity_run

        return eval_integrity_run(request)
    if tool == "formalization_meta":
        from theoremata_tools.formalization_meta import run as formalization_meta_run

        return formalization_meta_run(request)
    if tool == "query_rewrite":
        from theoremata_tools.query_rewrite import run as query_rewrite_run

        return query_rewrite_run(request)
    if tool == "semantic_memory":
        from theoremata_tools.semantic_memory import run as semantic_memory_run

        return semantic_memory_run(request)
    if tool == "grpo_upgrades":
        from theoremata_tools.grpo_upgrades import run as grpo_upgrades_run

        return grpo_upgrades_run(request)
    if tool == "cert_taylor_model":
        from theoremata_tools.cert_taylor_model import run as cert_taylor_model_run

        return cert_taylor_model_run(request)
    if tool == "cert_continued_fraction":
        from theoremata_tools.cert_continued_fraction import run as cert_continued_fraction_run

        return cert_continued_fraction_run(request)
    if tool == "falsify_hardcase":
        from theoremata_tools.falsify_hardcase import run as falsify_hardcase_run

        return falsify_hardcase_run(request)
    if tool == "geometry_wlog":
        from theoremata_tools.geometry_wlog import run as geometry_wlog_run

        return geometry_wlog_run(request)
    if tool == "lemma_cache":
        from .lemma_cache import run as lemma_cache_run

        return lemma_cache_run(request)
    if tool == "proof_telemetry":
        from theoremata_tools.proof_telemetry import run as proof_telemetry_run

        return proof_telemetry_run(request)
    if tool == "trace_normalize":
        from theoremata_tools.trace_normalize import run as trace_normalize_run

        return trace_normalize_run(request.get("request", request))
    if tool == "benchmark":
        from theoremata_tools.benchmarks.registry import run as benchmark_run

        return benchmark_run(request["request"])
    if tool == "star_harvest":
        from .star_harvester import run as star_harvest_run

        return star_harvest_run(request["request"])
    if tool == "bm25_retrieve":
        from theoremata_tools.bm25_retriever import run as bm25_run

        return bm25_run(
            root=request.get("root"),
            imports=request.get("imports"),
            query=request["query"],
            limit=int(request.get("limit", 20)),
            op=request.get("op", "retrieve"),
            theorem_module=request.get("theorem_module"),
            theorem_line=request.get("theorem_line"),
            file_module=request.get("file_module"),
        )
    if tool == "retrieval_eval":
        from theoremata_tools.retrieval_eval import run as retrieval_eval_run

        return retrieval_eval_run(
            examples=request.get("examples"),
            predictions=request.get("predictions"),
            gold=request.get("gold"),
            ks=request.get("ks"),
        )
    if tool == "novelty":
        from theoremata_tools.novelty import run as novelty_run

        return novelty_run(request)
    if tool == "conjecture_discovery":
        from theoremata_tools.conjecture_discovery import run as conjecture_discovery_run

        return conjecture_discovery_run(request)
    if tool == "funsearch":
        from theoremata_tools.funsearch import run as funsearch_run

        return funsearch_run(request)
    if tool == "cascade":
        from theoremata_tools.cascade import run as cascade_run

        return cascade_run(
            root=request.get("root"),
            imports=request.get("imports"),
            query=request["query"],
            first_stage=request.get("first_stage", "bm25"),
            first_k=int(request.get("first_k", 50)),
            k=int(request.get("k", 10)),
            theorem_module=request.get("theorem_module"),
            theorem_line=request.get("theorem_line"),
            file_module=request.get("file_module"),
            mask=request.get("mask", True),
            samples=int(request.get("samples", 1)),
        )
    if tool == "tactic_generate":
        from theoremata_tools.tactic_server import run as tactic_run

        return tactic_run(request)
    if tool == "proof_grader":
        from theoremata_tools.proof_grader import run as proof_grader_run

        return proof_grader_run(request)
    if tool == "exposition":
        from theoremata_tools.exposition import run as exposition_run

        return exposition_run(request)
    if tool == "curriculum":
        from theoremata_tools.curriculum import run as curriculum_run

        return curriculum_run(request)
    if tool == "source_scan":
        from theoremata_tools.formal_source_scan import run as source_scan_run

        return source_scan_run(request)
    if tool == "hammer":
        from theoremata_tools.hammer import run as hammer_run

        return hammer_run(request)
    if tool == "rocq_session":
        from theoremata_tools.rocq_driver import run as rocq_session_run

        return rocq_session_run(request)
    if tool == "isabelle_session":
        from theoremata_tools.isabelle_driver import run as isabelle_session_run

        return isabelle_session_run(request)
    if tool == "rocq_retrieve":
        from theoremata_tools.rocq_retrieval import run as rocq_retrieve_run

        return rocq_retrieve_run(request)
    if tool == "isabelle_retrieve":
        from theoremata_tools.isabelle_retrieval import run as isabelle_retrieve_run

        return isabelle_retrieve_run(request)
    if tool == "proof_calibration":
        from theoremata_tools.proof_calibration import run as proof_calibration_run

        return proof_calibration_run(request)
    if tool == "aristotle_mcp":
        from theoremata_tools.aristotle_mcp_client import run as aristotle_mcp_run

        return aristotle_mcp_run(request)
    if tool == "formal_lint":
        from theoremata_tools.formal_lint import run as formal_lint_run

        return formal_lint_run(request)
    if tool == "statement_roundtrip":
        from theoremata_tools.statement_roundtrip import run as statement_roundtrip_run

        return statement_roundtrip_run(request)
    if tool == "statement_triviality":
        from theoremata_tools.statement_triviality import run as statement_triviality_run

        # Consumers must branch ONLY on verdict == "trivial". Both
        # "not_shown_trivial" and "withheld" mean no signal, never approval:
        # surviving mutation does not make a statement meaningful.
        return statement_triviality_run(request)
    if tool == "opaque_statement":
        from theoremata_tools.opaque_statement import run as opaque_statement_run

        # Gate ONLY on verdict == "opaque_constant_found". Both
        # "no_opaque_constant_found" and "unknown" mean no signal, never approval.
        # This does not replace the layer-2 axiom audit: that audit already reports
        # sorryAx here. What this adds is ATTRIBUTION, naming which statement
        # constants caused it, which the flat closure cannot express.
        return opaque_statement_run(request)
    if tool == "lp_geometry":
        from theoremata_tools.lp_geometry import run as lp_geometry_run

        return lp_geometry_run(request)
    if tool == "geometry":
        from theoremata_tools.geometry import run as geometry_run

        return geometry_run(request)
    if tool == "graph_viewer":
        from theoremata_tools.graph_viewer import run as graph_viewer_run

        return graph_viewer_run(request)
    if tool == "geometry_synth":
        from theoremata_tools.geometry_synth import run as geometry_synth_run

        return geometry_synth_run(request)
    if tool == "geometry_synth2":
        from theoremata_tools.geometry_synth2 import run as geometry_synth2_run

        return geometry_synth2_run(request)
    if tool == "geometry_algebraic":
        from theoremata_tools.geometry_algebraic import run as geometry_algebraic_run

        return geometry_algebraic_run(request)
    if tool == "geometry_ddar":
        from theoremata_tools.geometry_ddar import run as geometry_ddar_run

        return geometry_ddar_run(request)
    if tool == "geometry_ddar2":
        from theoremata_tools.geometry_ddar2 import run as geometry_ddar2_run

        return geometry_ddar2_run(request)
    if tool == "flywheel":
        from theoremata_tools.flywheel import run as flywheel_run

        return flywheel_run(request["request"])
    if tool == "backend_select":
        from theoremata_tools.selector import run as backend_select_run

        return backend_select_run(request["request"])
    if tool == "process_supervision":
        from theoremata_tools.process_supervision import run as process_supervision_run

        return process_supervision_run(request["request"])
    if tool == "lean_corpus":
        from theoremata_tools.lean_corpus import run as lean_corpus_run

        return lean_corpus_run(request["request"])
    if tool == "ewc":
        from theoremata_tools.ewc import run as ewc_run

        return ewc_run(request["request"])
    if tool == "progress_sft":
        from theoremata_tools.progress_sft import run as progress_sft_run

        return progress_sft_run(request["request"])
    if tool == "retriever_train":
        from theoremata_tools.retriever_train import run as retriever_train_run

        return retriever_train_run(request["request"])
    if tool == "train_curriculum":
        from theoremata_tools.lifelong_curriculum import run as train_curriculum_run

        return train_curriculum_run(request["request"])
    if tool == "difficulty":
        from theoremata_tools.difficulty import run as difficulty_run

        return difficulty_run(request["request"])
    # Wolfram tools are UNTRUSTED ORACLES, kept in their own block and
    # deliberately away from the `cert_*` arms above so no reader mistakes them
    # for a certification path. Nothing here certifies: wolfram_link/wolfram_alpha
    # return raw oracle output marked untrusted, wolfram_falsify only ever emits a
    # refutation it rechecked itself, and wolfram_cert emits a `cert` only when one
    # of the existing independent checkers accepted the document on that run.
    # Imports stay lazy like the surrounding arms so a machine without sympy or
    # without the wolfram modules still loads the worker.
    if tool == "wolfram_link":
        from theoremata_tools.wolfram_link import run as wolfram_link_run

        return wolfram_link_run(request)
    if tool == "wolfram_alpha":
        from theoremata_tools.wolfram_alpha import run as wolfram_alpha_run

        return wolfram_alpha_run(request)
    if tool == "wolfram_recognizer":
        # Cheap triage in front of `wolfram_alpha`. Its output is a ROUTING hint
        # about Alpha's coverage, never a statement about the mathematics, so it
        # sits beside the oracles rather than anywhere near the checkers.
        from theoremata_tools.wolfram_recognizer import run as wolfram_recognizer_run

        return wolfram_recognizer_run(request)
    if tool == "wolfram_falsify":
        from theoremata_tools.wolfram_falsify import run as wolfram_falsify_run

        return wolfram_falsify_run(request)
    if tool == "wolfram_cert":
        # Lives under components/verify/python, resolved through the shared
        # `theoremata_tools` namespace package rather than a relative import.
        from theoremata_tools.wolfram_cert import run as wolfram_cert_run

        return wolfram_cert_run(request)
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
