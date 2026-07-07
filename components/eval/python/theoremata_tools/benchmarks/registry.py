"""Public benchmark registry + top-level API (Tier 4).

Three entry points, plus a JSON ``run`` op so ``worker.py`` can dispatch a
``benchmark`` tool:

* :func:`list_benchmarks` -> the registered corpus names (with track/kind);
* :func:`load_benchmark(name)` -> ``list[item]`` in the common schema;
* :func:`grade(item, response)` -> ``{is_solved, is_correct, detail}``.
"""
from __future__ import annotations

import json
import sys
from typing import Any

from . import graders
from .loaders import LOADERS
from .proof_completion import run_proof_completion
from .verified_programming import run_verified_programming

# name -> (track, kind) for a self-documenting catalogue
_TRACK_KIND = {
    "formalqualbench": ("formalization", "formalization"),
    "sphere_packing": ("formalization", "formalization"),
    "zklinalg": ("formalization", "formalization"),
    "strongpnt": ("formalization", "formalization"),
    "kakeya": ("formalization", "formalization"),
    "riemann_hypothesis_curves": ("formalization", "formalization"),
    "frontiermath_hypergraphs": ("formalization", "formalization"),
    "erdos1196": ("formalization", "formalization"),
    "ineqmath": ("nl_answer", "nl_answer"),
    "aime24": ("nl_answer", "nl_answer"),
    "aime25": ("nl_answer", "nl_answer"),
    "aime26": ("nl_answer", "nl_answer"),
    "brokenmath": ("falsification", "falsification"),
    "goldbach_collatz": ("falsification", "falsification"),
    "minif2f": ("formalization", "formalization"),
    "minif2f_train": ("formalization", "formalization"),
    "minif2f_valid": ("formalization", "formalization"),
    "minif2f_test": ("formalization", "formalization"),
    "bridge178": ("verified_programming", "verified_programming"),
    "quantumlean": ("formalization", "formalization"),
    "quantumlean_physics": ("formalization", "formalization"),
    "millennium": ("statement_target", "statement_target"),
    "imo2025": ("formalization", "formalization"),
    "putnam_artifacts": ("external_artifact", "external_artifact"),
    "formulationbench": ("reformulation", "reformulation"),
}


def list_benchmarks() -> list[dict[str, str]]:
    """Registered benchmark names with their track + item kind."""
    out = []
    for name in LOADERS:
        track, kind = _TRACK_KIND.get(name, ("unknown", "unknown"))
        out.append({"name": name, "track": track, "kind": kind})
    return out


def load_benchmark(name: str) -> list[dict[str, Any]]:
    """Load one benchmark by registry name into the common item schema.

    Returns ``[]`` (logging a skip) when the corpus is absent — never raises for
    a missing corpus. Raises ``KeyError`` for an unknown name.
    """
    if name not in LOADERS:
        raise KeyError(
            f"unknown benchmark {name!r}; known: {sorted(LOADERS)}"
        )
    return LOADERS[name]()


def grade(item: dict[str, Any], response: Any, **kw: Any) -> dict[str, Any]:
    """Grade a single response against an item (routes by ``item['kind']``)."""
    return graders.grade(item, response, **kw)


# --------------------------------------------------------------------------- #
# JSON dispatch (worker.py hook) + CLI
# --------------------------------------------------------------------------- #

def run(request: dict[str, Any]) -> dict[str, Any]:
    op = request.get("op", "list")
    if op == "list":
        return {"op": "list", "benchmarks": list_benchmarks()}
    if op == "load":
        name = request["name"]
        items = load_benchmark(name)
        limit = request.get("limit")
        if isinstance(limit, int) and limit >= 0:
            items = items[:limit]
        return {"op": "load", "name": name, "n": len(items), "items": items}
    if op == "grade":
        return grade(request["item"], request.get("response", ""))
    if op == "grade_batch":
        item = request["item"]
        results = [grade(item, r) for r in request.get("responses", [])]
        return {"op": "grade_batch", "results": results}
    if op == "proof_completion":
        return {
            "op": "proof_completion",
            **run_proof_completion(
                benchmark=request.get("benchmark", "minif2f_test"),
                responses=request.get("responses"),
                limit=request.get("limit"),
            ),
        }
    if op == "verified_programming":
        return {
            "op": "verified_programming",
            **run_verified_programming(
                benchmark=request.get("benchmark", "bridge178"),
                responses=request.get("responses"),
                limit=request.get("limit"),
            ),
        }
    raise ValueError(f"unknown op: {op}")


def main() -> None:
    if len(sys.argv) >= 2:
        with open(sys.argv[1], encoding="utf-8") as fh:
            request = json.load(fh)
    else:
        request = json.load(sys.stdin)
    print(json.dumps(run(request), indent=2, default=str))
    raise SystemExit(0)


if __name__ == "__main__":
    main()
