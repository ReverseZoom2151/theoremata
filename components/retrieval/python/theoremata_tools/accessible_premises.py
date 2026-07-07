"""ReProver-style accessible-premise filtering for Mathlib retrieval.

A premise is *accessible* from a theorem context when both hold (ReProver's
``Corpus.get_accessible_premises``):

1. **Import reachability** — its defining module is in the transitive import
   closure of the query file (the modules ``import``-reachable from the file's
   direct imports), or is the query file's own module.
2. **Same-file order** — if it is defined in the query file itself, it must be
   defined *before* the theorem position (no forward references).

Two backends
------------
* **Import-DAG closure** (preferred, exact): given the module import graph built
  by :func:`theoremata_tools.mathlib_index.build_index`, compute the real
  transitive closure. This matches ReProver: a premise from module ``M`` is
  eligible iff ``M`` is reachable in the import DAG from the query file.
* **Lexical fallback** (no DAG available): a conservative prefix/heuristic filter
  over the declared ``imports`` list. Used when the caller has no import graph
  (e.g. a bare decl dump with no source tree). Kept for backward compatibility.

Standard-library only.
"""
from __future__ import annotations

from typing import Any, Iterable


# --------------------------------------------------------------------------- #
# Import-DAG transitive closure (the real ReProver-style filter).
# --------------------------------------------------------------------------- #
def _dag_edges(dag: dict[str, Any]) -> dict[str, list[str]]:
    """Adjacency ``module -> [directly imported modules]`` from an index dict.

    Accepts the :func:`mathlib_index.build_index` shape (``{"imports": {...}}``)
    or a bare ``{module: [imports]}`` mapping.
    """
    if isinstance(dag.get("imports"), dict):
        return dag["imports"]
    return {k: list(v) for k, v in dag.items() if isinstance(v, (list, tuple))}


def import_closure(
    dag: dict[str, Any],
    roots: Iterable[str],
    *,
    include_roots: bool = True,
) -> set[str]:
    """Transitive set of modules import-reachable from ``roots`` in the DAG.

    A depth-first walk over the ``imports`` adjacency: every module ``import``ed
    (directly or transitively) by any root is reachable, since Lean ``import``
    is transitive. ``roots`` themselves are included when ``include_roots``.
    """
    edges = _dag_edges(dag)
    seen: set[str] = set()
    stack = list(roots)
    while stack:
        node = stack.pop()
        if node in seen:
            continue
        seen.add(node)
        stack.extend(edges.get(node, ()))
    if not include_roots:
        seen -= set(roots)
    return seen


def accessible_modules(
    dag: dict[str, Any],
    imports: Iterable[str],
    file_module: str | None = None,
) -> set[str]:
    """Modules whose declarations are visible from a file with these imports.

    = transitive import closure of ``imports`` (what the file pulls in) plus the
    file's own module ``file_module`` (its earlier same-file declarations).
    """
    closure = import_closure(dag, imports, include_roots=True)
    if file_module:
        closure.add(file_module)
    return closure


# --------------------------------------------------------------------------- #
# Lexical fallback (no import DAG).
# --------------------------------------------------------------------------- #
def _module_prefixes(module: str) -> set[str]:
    parts = (module or "").split(".")
    return {".".join(parts[: i + 1]) for i in range(len(parts))}


def _lexical_closure(imports: Iterable[str]) -> set[str]:
    closure: set[str] = set()
    for imp in imports or ["Init"]:
        closure |= _module_prefixes(imp)
        closure.add(imp)
    return closure


# --------------------------------------------------------------------------- #
# Public filter.
# --------------------------------------------------------------------------- #
def filter_accessible(
    decls: list[dict[str, Any]],
    *,
    imports: list[str],
    file_path: str | None = None,
    theorem_line: int | None = None,
    dag: dict[str, Any] | None = None,
    file_module: str | None = None,
) -> list[dict[str, Any]]:
    """Return the subset of ``decls`` accessible from the theorem context.

    Parameters
    ----------
    decls : list of ``{name, module, file?, line_start?}``
        Candidate premises to filter.
    imports : list[str]
        Modules ``import``ed at the top of the query file.
    file_path : str, optional
        Path/identifier of the query file, matched against ``decl["file"]`` for
        same-file forward-reference filtering.
    theorem_line : int, optional
        Line of the theorem; same-file decls defined *after* it are dropped.
    dag : dict, optional
        Import graph (``mathlib_index.build_index`` output or ``{mod: [imps]}``).
        When present the **transitive import closure** decides reachability
        (ReProver-exact); otherwise a lexical prefix fallback is used.
    file_module : str, optional
        The query file's own module name, so its earlier declarations stay
        visible under the DAG backend.

    Returns
    -------
    list[dict]
        ``decls`` filtered to the accessible premises, order preserved.
    """
    if dag is not None:
        allowed = accessible_modules(dag, imports or ["Init"], file_module=file_module)
        use_dag = True
    else:
        allowed = _lexical_closure(imports)
        use_dag = False
        has_mathlib = any(imp.startswith("Mathlib") for imp in allowed)

    out: list[dict[str, Any]] = []
    for d in decls:
        module = d.get("module") or ""

        if use_dag:
            if module and module not in allowed:
                continue
        else:
            if module and module not in allowed:
                # Backward-compatible Mathlib allowance: when Mathlib is imported,
                # treat Mathlib.*/Init decls as reachable (the lexical filter can
                # not enumerate Mathlib's ~4k transitive modules).
                if not (has_mathlib and (module.startswith("Mathlib") or module == "Init")):
                    continue

        # Same-file forward-reference filter (both backends).
        same_file = (file_path is not None and d.get("file") == file_path) or (
            file_module is not None and module == file_module
        )
        if same_file:
            start = d.get("line_start")
            if (
                theorem_line is not None
                and isinstance(start, int)
                and start > theorem_line
            ):
                continue

        out.append(d)
    return out
