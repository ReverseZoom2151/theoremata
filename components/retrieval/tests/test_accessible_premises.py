"""Tests for import-DAG accessibility masking (ReProver get_accessible_premises).

Exercises the transitive import-closure filter over a small synthetic import
graph, the same-file forward-reference rule, and the lexical fallback path.
"""
from theoremata_tools import accessible_premises as A


# Synthetic import DAG (module -> direct imports):
#
#   A  imports  B
#   B  imports  C
#   C  imports  (nothing)
#   D  imports  (nothing)      # a disconnected island
#
# So from A the transitive closure is {A, B, C}; D is NOT accessible.
DAG = {
    "imports": {
        "A": ["B"],
        "B": ["C"],
        "C": [],
        "D": [],
    }
}

DECLS = [
    {"name": "a_lemma", "module": "A"},
    {"name": "b_lemma", "module": "B"},
    {"name": "c_lemma", "module": "C"},
    {"name": "d_lemma", "module": "D"},
]


def _modules(decls):
    return {d["module"] for d in decls}


# --- transitive closure ------------------------------------------------------


def test_import_closure_is_transitive():
    assert A.import_closure(DAG, ["A"]) == {"A", "B", "C"}
    assert A.import_closure(DAG, ["B"]) == {"B", "C"}
    assert A.import_closure(DAG, ["C"]) == {"C"}
    assert A.import_closure(DAG, ["D"]) == {"D"}


def test_import_closure_can_exclude_roots():
    assert A.import_closure(DAG, ["A"], include_roots=False) == {"B", "C"}


def test_accepts_bare_adjacency_mapping():
    bare = {"A": ["B"], "B": ["C"], "C": []}
    assert A.import_closure(bare, ["A"]) == {"A", "B", "C"}


# --- DAG-based filtering -----------------------------------------------------


def test_disconnected_module_is_inaccessible():
    # A file that imports B sees B and C, but never the disconnected D.
    out = A.filter_accessible(DECLS, imports=["B"], dag=DAG)
    assert _modules(out) == {"B", "C"}


def test_full_closure_from_root():
    out = A.filter_accessible(DECLS, imports=["A"], dag=DAG)
    assert _modules(out) == {"A", "B", "C"}
    assert "d_lemma" not in {d["name"] for d in out}


def test_file_module_is_visible_even_if_not_imported():
    # The query file's own module (D) is visible for its earlier declarations
    # even though nothing imports it.
    out = A.filter_accessible(DECLS, imports=["C"], dag=DAG, file_module="D")
    assert _modules(out) == {"C", "D"}


# --- same-file forward-reference rule ---------------------------------------


def test_same_file_forward_reference_is_filtered():
    decls = [
        {"name": "earlier", "module": "F", "file": "F.lean", "line_start": 10},
        {"name": "at_theorem", "module": "F", "file": "F.lean", "line_start": 20},
        {"name": "later", "module": "F", "file": "F.lean", "line_start": 30},
    ]
    dag = {"imports": {"F": []}}
    out = A.filter_accessible(
        decls, imports=["F"], dag=dag, file_module="F", theorem_line=20
    )
    names = {d["name"] for d in out}
    # `later` (line 30 > 20) is a forward reference and must be dropped.
    assert names == {"earlier", "at_theorem"}


# --- lexical fallback (no DAG) ----------------------------------------------


def test_lexical_fallback_keeps_imported_prefixes():
    decls = [
        {"name": "n", "module": "Init.Data.Nat"},
        {"name": "l", "module": "Init.Data.List"},
    ]
    out = A.filter_accessible(decls, imports=["Init.Data.Nat"])
    assert _modules(out) == {"Init.Data.Nat"}


def test_lexical_fallback_allows_mathlib_when_imported():
    decls = [
        {"name": "x", "module": "Mathlib.Algebra.Group.Defs"},
        {"name": "y", "module": "SomeOther.Pkg"},
    ]
    out = A.filter_accessible(decls, imports=["Mathlib"])
    assert _modules(out) == {"Mathlib.Algebra.Group.Defs"}
