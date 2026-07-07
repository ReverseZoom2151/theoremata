"""Tests for the compilable lemma cache."""
from __future__ import annotations

from theoremata_tools.lemma_cache import (
    LemmaCache,
    make_entry,
    normalize_key,
    parse_file,
    render_block,
    render_file,
)


def test_normalize_key_collapses_whitespace():
    assert normalize_key("(n : Nat)  :   n + 0 = n") == "(n : Nat) : n + 0 = n"
    assert normalize_key("  : Nat.Prime 7 :=  ") == ": Nat.Prime 7"


def test_make_entry_rejects_bad_identifier():
    import pytest

    with pytest.raises(ValueError):
        make_entry("9bad", ": True", "trivial")
    with pytest.raises(ValueError):
        make_entry("ok", "", "trivial")


def test_render_block_is_delimited_and_compilable_shape():
    entry = make_entry(
        "seven_prime",
        ": Nat.Prime 7",
        "by decide",
        source="problem:xyz",
    )
    block = render_block(entry)
    assert block.startswith("-- @stored-theorem seven_prime")
    assert block.rstrip().endswith("-- @end-stored-theorem")
    assert "theorem seven_prime : Nat.Prime 7 :=" in block
    assert "  by decide" in block
    assert "-- Key:      : Nat.Prime 7" in block


def test_add_lookup_roundtrip(tmp_path):
    path = str(tmp_path / "Stored.lean")
    cache = LemmaCache(path, namespace="TheoremataLib")

    entry = cache.append_lemma(
        "add_zero_nat",
        "(n : Nat) : n + 0 = n",
        "by simp",
        source="unit-test",
    )
    assert entry["name"] == "add_zero_nat"

    # Lookup by normalized key, raw statement, and by name all resolve.
    got = cache.get("(n : Nat) : n + 0 = n")
    assert got is not None
    assert got["name"] == "add_zero_nat"
    assert got["proof"] == "by simp"
    assert cache.get(normalize_key("(n : Nat)   :  n + 0 = n")) is not None
    assert cache.get("add_zero_nat") is not None
    assert cache.get("nonexistent") is None

    # Reload from disk: the round-trip preserves the entry contract.
    reloaded = LemmaCache(path).list()
    assert len(reloaded) == 1
    r = reloaded[0]
    for field in ("name", "statement", "proof", "source", "proved_at", "key"):
        assert field in r
    assert r["statement"] == "(n : Nat) : n + 0 = n"
    assert r["proof"] == "by simp"
    assert r["source"] == "unit-test"

    # Usage hint uses the namespace.
    assert cache.usage_hint(got).startswith("exact TheoremataLib.add_zero_nat")


def test_append_dedup_and_overwrite(tmp_path):
    path = str(tmp_path / "Stored.lean")
    cache = LemmaCache(path)

    assert cache.append_lemma("foo", ": True", "trivial")["name"] == "foo"
    # Same key -> idempotent skip.
    changed = cache.append(make_entry("foo2", ": True", "trivial"))
    assert changed is False
    assert len(cache.list()) == 1

    # overwrite replaces the existing entry sharing the key.
    changed = cache.append(
        make_entry("foo", ": True", "by exact True.intro"), overwrite=True
    )
    assert changed is True
    entries = cache.list()
    assert len(entries) == 1
    assert entries[0]["proof"] == "by exact True.intro"


def test_render_file_roundtrips_multiple_entries():
    entries = [
        make_entry("a", ": True", "trivial"),
        make_entry("b", "(n : Nat) : n = n", "by rfl"),
    ]
    text = render_file(entries, namespace="Lib")
    assert "namespace Lib" in text and "end Lib" in text

    parsed = parse_file(text)
    assert [e["name"] for e in parsed] == ["a", "b"]
    assert parsed[1]["statement"] == "(n : Nat) : n = n"
    assert parsed[1]["proof"] == "by rfl"


def test_multiline_proof_indentation_roundtrip(tmp_path):
    path = str(tmp_path / "Stored.lean")
    cache = LemmaCache(path)
    proof = "by\nintro n\nsimp"
    cache.append_lemma("ml", "(n : Nat) : n + 0 = n", proof)
    got = LemmaCache(path).get("ml")
    assert got is not None
    # Proof body survives (modulo surrounding whitespace) the render/parse trip.
    assert "intro n" in got["proof"]
    assert "simp" in got["proof"]
