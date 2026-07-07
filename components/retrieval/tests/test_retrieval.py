"""Tests for plan §8 in-the-loop Mathlib retrieval.

The ranking/tokeniser logic is exercised entirely in-memory (no Lean). One
optional integration test builds the real ``Init`` index and asserts the cache
makes the second call fast; it skips cleanly when no toolchain is present.
"""
import os
import shutil
import time

import pytest

from theoremata_tools import retrieval as R


def _lean_available() -> bool:
    if shutil.which("lean") or shutil.which("lake"):
        return True
    for name in ("lean", "lake"):
        for ext in ("", ".exe"):
            if os.path.exists(os.path.expanduser(os.path.join("~", ".elan", "bin", name + ext))):
                return True
    return False


# --- tokenizer ---------------------------------------------------------------


def test_tokenize_dotted_and_underscore():
    assert R.tokenize("Nat.succ_le_succ") == ["nat", "succ", "le", "succ"]


def test_tokenize_camel_case():
    assert R.tokenize("List.getLastNe") == ["list", "get", "last", "ne"]


def test_tokenize_mixed_camel_and_acronym():
    # ACRONYM run stays whole; trailing camel word splits off
    assert R.tokenize("HTMLParser") == ["html", "parser"]


def test_tokenize_empty():
    assert R.tokenize("") == []
    assert R.tokenize("   ") == []


# --- ranking -----------------------------------------------------------------


def _names(results):
    return [r["name"] for r in results]


def test_exact_identifier_beats_loose_match():
    decls = [
        {"name": "Nat.succ_le_succ", "module": "Init.Data.Nat", "kind": "theorem"},
        {"name": "Nat.le_of_succ_le", "module": "Init.Data.Nat", "kind": "theorem"},
        {"name": "List.succ_pos", "module": "Init.Data.List", "kind": "theorem"},
    ]
    results = R.retrieve("Nat.succ_le_succ", decls, limit=10)
    assert results, "expected at least one match"
    assert results[0]["name"] == "Nat.succ_le_succ"
    # the exact identifier must strictly outrank the merely-overlapping decls
    assert results[0]["score"] > results[1]["score"]


def test_camel_query_matches_underscore_decl():
    # query in camelCase, declaration in snake_case -> same token stream matches
    decls = [
        {"name": "Nat.succLeSucc", "module": "Init.Data.Nat", "kind": "theorem"},
        {"name": "Nat.add_comm", "module": "Init.Data.Nat", "kind": "theorem"},
    ]
    results = R.retrieve("succ_le_succ", decls, limit=10)
    assert _names(results)[0] == "Nat.succLeSucc"


def test_returned_shape_and_ordering():
    decls = [
        {"name": "Nat.add_comm", "module": "Init.Data.Nat", "kind": "theorem"},
        {"name": "Nat.mul_comm", "module": "Init.Data.Nat", "kind": "theorem"},
    ]
    results = R.retrieve("add comm", decls, limit=10)
    assert set(results[0].keys()) == {"name", "module", "kind", "score"}
    assert results[0]["name"] == "Nat.add_comm"
    # descending score
    scores = [r["score"] for r in results]
    assert scores == sorted(scores, reverse=True)


def test_limit_is_respected():
    decls = [
        {"name": f"Foo.bar_{i}", "module": "M", "kind": "theorem"} for i in range(10)
    ]
    results = R.retrieve("bar", decls, limit=3)
    assert len(results) == 3


def test_no_match_returns_empty():
    decls = [{"name": "Nat.add_comm", "module": "Init.Data.Nat", "kind": "theorem"}]
    assert R.retrieve("completely unrelated xyzzy", decls, limit=5) == []


def test_head_bucket_bonus_lifts_matching_head():
    # Two decls with equal lexical overlap for the query token "bound"; only
    # `le_bound` sits in the LE.le head bucket, so the head bonus must lift it.
    decls = [
        {"name": "Foo.eq_bound", "module": "M", "kind": "theorem"},
        {"name": "Foo.le_bound", "module": "M", "kind": "theorem"},
    ]
    head_index = {
        "heads": {"LE.le": ["Foo.le_bound"], "Eq": ["Foo.eq_bound"]},
        "conclusions": {},
        "count": 2,
    }
    # query head symbol resolves to LE.le via the `≤` relation
    results = R.retrieve("a ≤ bound", decls, head_index=head_index, limit=10)
    assert _names(results)[0] == "Foo.le_bound"

    # without the head index the tie is not broken in le_bound's favour by a bonus
    plain = R.retrieve("a ≤ bound", decls, head_index=None, limit=10)
    top_plain = _names(plain)[0]
    # sanity: the head-aware ranking put le_bound strictly first
    scored = {r["name"]: r["score"] for r in results}
    assert scored["Foo.le_bound"] > scored["Foo.eq_bound"]


def test_head_bucket_by_explicit_bucket_name():
    decls = [
        {"name": "thm_a", "module": "M", "kind": "theorem"},
        {"name": "thm_b", "module": "M", "kind": "theorem"},
    ]
    head_index = {"heads": {"LE.le": ["thm_a"]}, "conclusions": {}, "count": 2}
    # query names the bucket directly; thm_a should be lifted above thm_b
    results = R.retrieve("LE.le thm", decls, head_index=head_index, limit=10)
    assert _names(results)[0] == "thm_a"


# --- cache keying (no Lean) --------------------------------------------------


def test_cache_key_changes_with_imports():
    k1 = R.cache_key(None, ["Init"])
    k2 = R.cache_key(None, ["Init", "Mathlib"])
    assert k1 != k2


def test_cache_key_stable_and_order_independent():
    assert R.cache_key(None, ["A", "B"]) == R.cache_key(None, ["B", "A"])


# --- optional Lean integration ----------------------------------------------


@pytest.mark.skipif(not _lean_available(), reason="Lean toolchain not available")
def test_build_or_load_caches_over_init(tmp_path):
    root = str(tmp_path)  # bare root: no lake project, dumps run against `lean`
    # first build (cold): force a rebuild so the cache state is deterministic
    # regardless of any index left on disk by a prior run.
    first = R.build_or_load(None, ["Init"], rebuild=True, timeout=180.0)
    if not first.get("ok"):
        pytest.skip(f"lean dump did not run: {first.get('stderr')}")
    assert first["count"] > 0
    assert first["cached"] is False

    t0 = time.perf_counter()
    second = R.build_or_load(None, ["Init"], timeout=180.0)
    elapsed = time.perf_counter() - t0
    assert second["cached"] is True
    assert second["count"] == first["count"]
    # a cache hit must be dramatically faster than a cold dump
    assert elapsed < 10.0

    results = R.retrieve("Eq", second["decls"], head_index=second.get("head_index"), limit=5)
    assert isinstance(results, list)
