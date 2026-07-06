"""Conservative expression evaluator for bounded mathematical experiments."""
from __future__ import annotations

import ast
import math
import statistics
from typing import Any

ALLOWED_NAMES: dict[str, Any] = {
    "abs": abs,
    "all": all,
    "any": any,
    "divmod": divmod,
    "enumerate": enumerate,
    "float": float,
    "int": int,
    "len": len,
    "list": list,
    "max": max,
    "min": min,
    "pow": pow,
    "range": range,
    "round": round,
    "set": set,
    "sorted": sorted,
    "sum": sum,
    "tuple": tuple,
    "zip": zip,
    "math": math,
    "statistics": statistics,
}

ALLOWED_NODES = (
    ast.Expression, ast.Constant, ast.List, ast.Tuple, ast.Set, ast.Dict,
    ast.BinOp, ast.UnaryOp, ast.BoolOp, ast.Compare, ast.IfExp,
    ast.Add, ast.Sub, ast.Mult, ast.Div, ast.FloorDiv, ast.Mod, ast.Pow,
    ast.USub, ast.UAdd, ast.Not, ast.And, ast.Or, ast.Eq, ast.NotEq,
    ast.Lt, ast.LtE, ast.Gt, ast.GtE, ast.In, ast.NotIn,
    ast.Name, ast.Load, ast.Store, ast.Call, ast.keyword, ast.Attribute, ast.Subscript,
    ast.Slice, ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp,
    ast.comprehension,
)


def compile_expression(expression: str, variables: set[str] | None = None):
    tree = ast.parse(expression, mode="eval")
    variables = variables or set()
    comprehension_targets = {
        node.id
        for comp in ast.walk(tree)
        if isinstance(comp, ast.comprehension)
        for node in ast.walk(comp.target)
        if isinstance(node, ast.Name)
    }
    names = set(ALLOWED_NAMES) | variables | comprehension_targets
    for node in ast.walk(tree):
        if not isinstance(node, ALLOWED_NODES):
            raise ValueError(f"syntax not allowed: {type(node).__name__}")
        if isinstance(node, ast.Name) and node.id not in names:
            raise ValueError(f"name not allowed: {node.id}")
        if isinstance(node, ast.Attribute):
            if not isinstance(node.value, ast.Name) or node.value.id not in {
                "math", "statistics"
            }:
                raise ValueError("attribute access is restricted")
            if node.attr.startswith("_"):
                raise ValueError("private attributes are restricted")
    return compile(tree, "<theoremata>", "eval")


def evaluate(expression: str, variables: dict[str, Any] | None = None) -> Any:
    variables = variables or {}
    code = compile_expression(expression, set(variables))
    return eval(code, {"__builtins__": {}}, {**ALLOWED_NAMES, **variables})
