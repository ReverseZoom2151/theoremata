"""Small SymPy boundary with serialized inputs and outputs."""
from __future__ import annotations


def run(operation: str, expression: str, variable: str | None = None) -> dict:
    try:
        import sympy
    except ImportError as exc:
        return {"available": False, "error": f"SymPy unavailable: {exc}"}

    locals_: dict[str, object] = {}
    if variable:
        locals_[variable] = sympy.Symbol(variable)
    expr = sympy.sympify(expression, locals=locals_)
    operations = {
        "simplify": lambda: sympy.simplify(expr),
        "factor": lambda: sympy.factor(expr),
        "expand": lambda: sympy.expand(expr),
        "solve": lambda: sympy.solve(expr, locals_.get(variable)) if variable else sympy.solve(expr),
        "differentiate": lambda: sympy.diff(expr, locals_[variable]),
        "integrate": lambda: sympy.integrate(expr, locals_[variable]),
    }
    if operation not in operations:
        raise ValueError(f"unsupported symbolic operation: {operation}")
    result = operations[operation]()
    return {
        "available": True,
        "operation": operation,
        "input": str(expr),
        "result": str(result),
        "srepr": sympy.srepr(result),
    }
