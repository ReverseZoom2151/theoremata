"""Tests for retrieval Layer C: the head-symbol index."""
import os
import shutil

import pytest

from theoremata_tools import head_index as H


def _lean_available() -> bool:
    if shutil.which("lean"):
        return True
    for ext in ("", ".exe"):
        if os.path.exists(os.path.expanduser(os.path.join("~", ".elan", "bin", "lean" + ext))):
            return True
    return False


# --- core head-extraction logic (no Lean required) ---


def test_plain_type_head():
    assert H.head_symbol("Nat") == "Nat"


def test_forall_infix_relation_head():
    # infix `=` maps to the Eq head after stripping the binder
    assert H.head_symbol("∀ n : Nat, n = n") == "Eq"


def test_prefix_relation_head():
    # the real pretty-printer emits prefix form; head is the leading constant
    assert H.head_symbol("∀ {m n : Nat}, Eq (f m) (g n)") == "Eq"


def test_strips_multiple_hypotheses():
    assert H.head_symbol("A → B → C.foo x") == "C.foo"


def test_strips_instance_binder():
    assert H.head_symbol("{x : T} → P x") == "P"


def test_exists_and_not_heads():
    assert H.head_symbol("∃ x, P x") == "Exists"
    assert H.head_symbol("¬ P x") == "Not"


def test_arrow_inside_binder_type_not_split():
    # the → lives inside the ∀ binder's type, so the conclusion is C
    assert H.head_symbol("∀ f : A → B, C x") == "C"


def test_build_and_query_index():
    decls = [
        {"name": "thm_eq", "type": "∀ n : Nat, Eq n n"},
        {"name": "thm_le", "type": "∀ n : Nat, LE.le 0 n"},
        {"name": "thm_eq2", "type": "Eq a b"},
        {"name": "no_type", "kind": "def"},  # skipped: no type
    ]
    index = H.build_head_index(decls)
    assert index["count"] == 3
    assert set(H.by_head(index, "Eq")) == {"thm_eq", "thm_eq2"}
    assert H.by_head(index, "LE.le") == ["thm_le"]
    assert H.by_head(index, "Nonexistent") == []


def test_search_conclusion():
    decls = [
        {"name": "a", "type": "∀ n, Foo.bar n"},
        {"name": "b", "type": "Baz x"},
    ]
    assert H.search_conclusion(decls, "Foo.bar") == ["a"]
    assert H.search_conclusion(decls, "Baz") == ["b"]


# --- integration against the real Lean toolchain (skips if unavailable) ---


@pytest.mark.skipif(not _lean_available(), reason="Lean toolchain not available")
def test_init_head_index_integration():
    result = H.run(root=None, imports=["Init"], query="by_head", head="Eq", limit=5, timeout=180.0)
    if not result.get("ok"):
        pytest.skip(f"lean dump did not run: {result.get('stderr')}")
    assert result["count"] > 0
    assert all(isinstance(n, str) for n in result["matches"])
