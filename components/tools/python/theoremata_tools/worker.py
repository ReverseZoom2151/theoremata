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
