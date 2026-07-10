"""Tests for error-identifier-keyed retrieval (offline, deterministic).

Covers: identifier extraction from representative Lean errors (unknown
identifier, unknown constant, unsolved goals naming a lemma, type mismatch);
deterministic + de-duplicated query lists; empty/garbage errors; and the run()
op dispatch pairing the extractor with a mock retriever.
"""
from theoremata_tools import error_keyed_retrieval as EKR


# --------------------------------------------------------------------------- #
# error_keyed_query — extraction.
# --------------------------------------------------------------------------- #
def test_unknown_identifier_is_extracted():
    err = "error: unknown identifier 'foo_bar'"
    assert EKR.error_keyed_query(err) == ["foo_bar"]


def test_unknown_constant_qualified_name_is_extracted():
    err = "error: unknown constant 'Nat.succ_le_succ'"
    qs = EKR.error_keyed_query(err)
    assert "Nat.succ_le_succ" in qs
    # The quoted-unknown tier surfaces the full qualified name first.
    assert qs[0] == "Nat.succ_le_succ"


def test_unsolved_goals_named_lemma_is_extracted():
    err = (
        "unsolved goals\n"
        "case succ\n"
        "n : Nat\n"
        "⊢ Finset.sum_range_succ f n"
    )
    qs = EKR.error_keyed_query(err)
    # The named lemma in the goal dump is mined even without an 'unknown' prefix.
    assert "Finset.sum_range_succ" in qs


def test_type_mismatch_terms_are_extracted():
    err = (
        "type mismatch\n"
        "  Nat.add_comm a b\n"
        "has type\n"
        "  a + b = b + a\n"
        "but is expected to have type\n"
        "  b + a = a + b"
    )
    qs = EKR.error_keyed_query(err)
    assert "Nat.add_comm" in qs
    # Single-letter variables are not treated as lemma names.
    assert "a" not in qs and "b" not in qs


def test_bare_snake_case_lemma_extracted_but_not_dotted_tail_duplicated():
    err = "unsolved goals\n⊢ apply add_comm then Nat.mul_comm"
    qs = EKR.error_keyed_query(err)
    assert "add_comm" in qs           # bare snake_case lemma
    assert "Nat.mul_comm" in qs       # qualified name
    # 'mul_comm' as the tail of Nat.mul_comm must NOT be added as a separate query.
    assert "mul_comm" not in qs


def test_query_list_is_deduplicated_and_deterministic():
    err = (
        "unknown identifier 'foo'\n"
        "unknown identifier 'foo'\n"
        "⊢ Nat.add_comm x, Nat.add_comm y"
    )
    a = EKR.error_keyed_query(err)
    b = EKR.error_keyed_query(err)
    assert a == b                     # deterministic
    assert a.count("foo") == 1        # de-duplicated
    assert a.count("Nat.add_comm") == 1


def test_empty_and_nonidentifier_errors_yield_no_queries():
    assert EKR.error_keyed_query("") == []
    assert EKR.error_keyed_query("   ") == []
    assert EKR.error_keyed_query(None) == []  # type: ignore[arg-type]
    assert EKR.error_keyed_query("goals accomplished!") == []


# --------------------------------------------------------------------------- #
# run — op dispatch with an injected (mock) retriever.
# --------------------------------------------------------------------------- #
def _mock_retriever(calls: list[str]):
    """A deterministic mock: records queries, returns one hit per query keyed on
    the query so the merge/tagging can be asserted."""

    def _retrieve(query: str) -> list:
        calls.append(query)
        return [{"name": f"lemma_for::{query}", "score": 1.0}]

    return _retrieve


def test_run_dispatches_keyed_queries_through_the_mock_retriever():
    calls: list[str] = []
    req = {
        "op": "error_keyed_retrieval",
        "error": "unknown constant 'Nat.succ_le_succ'\nunknown identifier 'helper_lemma'",
    }
    out = EKR.run(req, retrieve=_mock_retriever(calls))

    assert out["ok"] is True
    assert out["op"] == "error_keyed_retrieval"
    # Both identifiers became queries and were dispatched to the retriever.
    assert out["queries"] == ["Nat.succ_le_succ", "helper_lemma"]
    assert calls == ["Nat.succ_le_succ", "helper_lemma"]
    # Merged results are keyed back to the query that found them.
    names = [r["name"] for r in out["results"]]
    assert names == ["lemma_for::Nat.succ_le_succ", "lemma_for::helper_lemma"]
    assert all("query" in r for r in out["results"])
    assert out["results"][0]["query"] == "Nat.succ_le_succ"
    assert out["count"] == 2
    assert set(out["per_query"]) == {"Nat.succ_le_succ", "helper_lemma"}


def test_run_dedups_results_shared_across_queries():
    # A retriever that returns the SAME record for every query — the merged list
    # must contain it once.
    def _same(query: str) -> list:
        return [{"name": "shared_lemma", "score": 0.5}]

    req = {"error": "unknown identifier 'a_lemma'\n⊢ Nat.foo_bar"}
    out = EKR.run(req, retrieve=_same)
    assert len(out["queries"]) == 2
    assert out["count"] == 1
    assert out["results"][0]["name"] == "shared_lemma"


def test_run_with_no_extractable_ids_skips_retrieval():
    called = []
    out = EKR.run(
        {"error": "goals accomplished!"},
        retrieve=lambda q: called.append(q) or [],
    )
    assert out["ok"] is True
    assert out["queries"] == []
    assert out["results"] == []
    assert out["count"] == 0
    assert called == []  # retriever never invoked


def test_run_rejects_unknown_op():
    out = EKR.run({"op": "bogus", "error": "unknown identifier 'x'"})
    assert out["ok"] is False
    assert "unknown op" in out["stderr"]


def test_run_rejects_non_dict_request():
    out = EKR.run(["not", "a", "dict"])  # type: ignore[arg-type]
    assert out["ok"] is False


def test_run_is_deterministic_with_a_fixed_retriever():
    req = {"error": "unknown constant 'A.b_c'\ntype mismatch\n  D.e_f x"}
    a = EKR.run(req, retrieve=lambda q: [{"name": q}])
    b = EKR.run(req, retrieve=lambda q: [{"name": q}])
    assert a == b
