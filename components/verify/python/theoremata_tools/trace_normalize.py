"""FLARE-style agent trace normalization (tool/time/cost aggregates).

Reads JSONL spans (one JSON object per line) and produces summary statistics
without calling models or executing tools.
"""
from __future__ import annotations

import json
import sys
from collections import Counter, defaultdict
from typing import Any


def _span_cost(span: dict[str, Any]) -> float:
    for key in ("cost_usd", "cost", "price"):
        val = span.get(key)
        if isinstance(val, (int, float)):
            return float(val)
    return 0.0


def _span_duration_ms(span: dict[str, Any]) -> float:
    for key in ("duration_ms", "elapsed_ms", "duration"):
        val = span.get(key)
        if isinstance(val, (int, float)):
            return float(val)
    return 0.0


def normalize_trace(spans: list[dict[str, Any]]) -> dict[str, Any]:
    """Aggregate a list of trace span dicts into FLARE-style telemetry."""
    tool_counts: Counter[str] = Counter()
    tool_ms: defaultdict[str, float] = defaultdict(float)
    tool_cost: defaultdict[str, float] = defaultdict(float)
    actors: Counter[str] = Counter()
    total_ms = 0.0
    total_cost = 0.0
    errors = 0

    for span in spans:
        tool = str(span.get("tool") or span.get("name") or span.get("op") or "unknown")
        actor = str(span.get("actor") or span.get("role") or "agent")
        ms = _span_duration_ms(span)
        cost = _span_cost(span)
        tool_counts[tool] += 1
        tool_ms[tool] += ms
        tool_cost[tool] += cost
        actors[actor] += 1
        total_ms += ms
        total_cost += cost
        if span.get("error") or span.get("ok") is False:
            errors += 1

    per_tool = [
        {
            "tool": name,
            "calls": tool_counts[name],
            "duration_ms": tool_ms[name],
            "cost_usd": tool_cost[name],
        }
        for name in sorted(tool_counts)
    ]
    return {
        "spans": len(spans),
        "errors": errors,
        "total_duration_ms": total_ms,
        "total_cost_usd": total_cost,
        "actors": dict(actors),
        "per_tool": per_tool,
    }


def run(request: dict[str, Any]) -> dict[str, Any]:
    op = request.get("op", "normalize")
    if op == "normalize":
        spans = request.get("spans")
        if spans is None and request.get("path"):
            path = request["path"]
            spans = []
            with open(path, encoding="utf-8") as fh:
                for line in fh:
                    line = line.strip()
                    if line:
                        spans.append(json.loads(line))
        if not isinstance(spans, list):
            raise ValueError("spans must be a list or provide path to JSONL")
        return {"op": "normalize", **normalize_trace(spans)}
    raise ValueError(f"unknown op: {op}")


def main() -> None:
    request = json.load(sys.stdin)
    print(json.dumps(run(request), indent=2))
    raise SystemExit(0)


if __name__ == "__main__":
    main()