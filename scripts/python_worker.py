#!/usr/bin/env python3
"""Restricted deterministic math worker. Reads one JSON request from stdin."""
import ast
import json
import math
import statistics
import sys

ALLOWED_NAMES = {
    "abs": abs, "all": all, "any": any, "divmod": divmod, "enumerate": enumerate,
    "float": float, "int": int, "len": len, "list": list, "max": max, "min": min,
    "pow": pow, "range": range, "round": round, "set": set, "sorted": sorted,
    "sum": sum, "tuple": tuple, "zip": zip, "math": math, "statistics": statistics,
}
ALLOWED_NODES = (
    ast.Expression, ast.Constant, ast.List, ast.Tuple, ast.Set, ast.Dict,
    ast.BinOp, ast.UnaryOp, ast.BoolOp, ast.Compare, ast.IfExp,
    ast.Add, ast.Sub, ast.Mult, ast.Div, ast.FloorDiv, ast.Mod, ast.Pow,
    ast.USub, ast.UAdd, ast.Not, ast.And, ast.Or, ast.Eq, ast.NotEq, ast.Lt,
    ast.LtE, ast.Gt, ast.GtE, ast.In, ast.NotIn, ast.Name, ast.Load,
    ast.Call, ast.keyword, ast.Attribute, ast.Subscript, ast.Slice,
    ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp, ast.comprehension,
)

def validate(tree):
    for node in ast.walk(tree):
        if not isinstance(node, ALLOWED_NODES):
            raise ValueError(f"syntax not allowed: {type(node).__name__}")
        if isinstance(node, ast.Name) and node.id not in ALLOWED_NAMES and not isinstance(node.ctx, ast.Store):
            # Comprehension variables are handled by the compiler after this conservative check.
            targets = {n.id for n in ast.walk(tree)
                       if isinstance(n, ast.comprehension)
                       for n in ast.walk(n.target) if isinstance(n, ast.Name)}
            if node.id not in targets:
                raise ValueError(f"name not allowed: {node.id}")
        if isinstance(node, ast.Attribute):
            if not isinstance(node.value, ast.Name) or node.value.id not in {"math", "statistics"}:
                raise ValueError("attribute access is restricted")

def main():
    request = json.load(sys.stdin)
    expr = request.get("expression")
    if not isinstance(expr, str):
        raise ValueError("expression must be a string")
    tree = ast.parse(expr, mode="eval")
    validate(tree)
    result = eval(compile(tree, "<theoremata>", "eval"), {"__builtins__": {}}, ALLOWED_NAMES)
    print(json.dumps({"result": result}, default=repr))

if __name__ == "__main__":
    try:
        main()
    except Exception as exc:
        print(json.dumps({"error": str(exc)}))
        raise SystemExit(2)
