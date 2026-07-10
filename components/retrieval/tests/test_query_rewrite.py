"""Tests for query rewriting / expansion (offline, deterministic).

Covers: rule-based synonym/notation variants for representative math queries;
conjunctive-goal decomposition into sub-queries; de-duplication; the injected
HyDE / llm_rewrite seams (invoked when provided, skipped otherwise);
determinism; and the run() op dispatch (pure-rewrite pass + multi-query
retrieval-then-merge through a mock retriever).
"""
from theoremata_tools import query_rewrite as QR


# --------------------------------------------------------------------------- #
# expand_query — synonym / notation variants.
# --------------------------------------------------------------------------- #
def test_iff_synonym_expansion():
    variants = QR.expand_query("a iff b")
    assert "a if and only if b" in variants
    # The original is not echoed back among the expansions.
    assert "a iff b" not in variants


def test_if_and_only_if_expands_back_to_iff():
    variants = QR.expand_query("a if and only if b")
    assert "a iff b" in variants


def test_nat_notation_and_synonym_expansion():
    variants = QR.expand_query("n is a nat")
    assert "n is a natural number" in variants


def test_mathlib_shorthand_variants():
    # add -> addition/sum ; comm -> commutative
    add_variants = QR.expand_query("add is comm")
    assert "addition is comm" in add_variants
    assert "sum is comm" in add_variants
    assert "add is commutative" in add_variants


def test_unicode_notation_expansion():
    # ↔ spells out to iff ; ℕ to natural number ; ≤ to <=
    assert "a iff b" in QR.expand_query("a ↔ b")
    assert "x in natural number" in QR.expand_query("x in ℕ")
    assert "a <= b" in QR.expand_query("a ≤ b")


def test_symbolic_roundtrip_ascii_to_unicode():
    # "<=" is recognised and offered back as its Unicode form.
    assert "a ≤ b" in QR.expand_query("a <= b")


# --------------------------------------------------------------------------- #
# expand_query — conjunctive decomposition.
# --------------------------------------------------------------------------- #
def test_conjunctive_goal_decomposes_into_subqueries():
    variants = QR.expand_query("f is continuous and g is injective")
    assert "f is continuous" in variants
    assert "g is injective" in variants


def test_conjunction_via_wedge_and_comma_and_semicolon():
    assert "p" in QR.expand_query("p ∧ q") and "q" in QR.expand_query("p ∧ q")
    assert "p" in QR.expand_query("p, q") and "q" in QR.expand_query("p, q")
    assert "p" in QR.expand_query("p ; q") and "q" in QR.expand_query("p ; q")


def test_non_conjunctive_query_yields_no_subquery_split():
    # A plain query must not be split into pieces (no spurious conjuncts).
    variants = QR.expand_query("commutativity of addition")
    assert "commutativity of addition" not in variants  # not echoed
    # None of the expansions is a bare fragment shorter than the whole phrase
    # produced by a (nonexistent) conjunction split.
    assert all(" of " in v or "comm" in v.lower() or "add" in v.lower() for v in variants)


# --------------------------------------------------------------------------- #
# Dedup + determinism.
# --------------------------------------------------------------------------- #
def test_expansions_are_deduplicated():
    variants = QR.expand_query("add and add")
    # "add" -> addition/sum on each occurrence would collide; the result is
    # de-duplicated (no repeated surface form, case/space-insensitive).
    lowered = [v.lower() for v in variants]
    assert len(lowered) == len(set(lowered))


def test_expand_query_is_deterministic():
    q = "f is continuous and n is a nat, a iff b"
    assert QR.expand_query(q) == QR.expand_query(q)


def test_empty_query_yields_nothing():
    assert QR.expand_query("") == []
    assert QR.expand_query("   ") == []
    assert QR.expand_query(None) == []  # type: ignore[arg-type]
    assert QR.rewrite_for_retrieval("") == []


# --------------------------------------------------------------------------- #
# rewrite_for_retrieval — original + expansions + HyDE seam.
# --------------------------------------------------------------------------- #
def test_rewrite_prepends_original_then_expansions():
    multi = QR.rewrite_for_retrieval("a iff b")
    assert multi[0] == "a iff b"  # original first
    assert "a if and only if b" in multi


def test_rewrite_can_disable_expansions():
    multi = QR.rewrite_for_retrieval("a iff b", expansions=False)
    assert multi == ["a iff b"]


def test_hyde_seam_invoked_when_provided():
    calls: list[str] = []

    def _hyde(q: str) -> str:
        calls.append(q)
        return "hypothetical: a holds exactly when b holds"

    multi = QR.rewrite_for_retrieval("a iff b", hyde=_hyde)
    assert calls == ["a iff b"]  # HyDE seam was called once, with the query
    assert "hypothetical: a holds exactly when b holds" in multi


def test_hyde_seam_skipped_when_absent():
    # No hyde => no hypothetical doc, and nothing beyond original + rule variants.
    multi = QR.rewrite_for_retrieval("a iff b")
    assert not any("hypothetical" in m for m in multi)


def test_hyde_seam_accepts_multiple_docs():
    multi = QR.rewrite_for_retrieval("x", hyde=lambda q: ["doc one", "doc two"])
    assert "doc one" in multi and "doc two" in multi


def test_llm_rewrite_seam_invoked_when_provided_and_skipped_otherwise():
    calls: list[str] = []

    def _llm(q: str) -> list[str]:
        calls.append(q)
        return ["a is equivalent to b"]

    with_llm = QR.expand_query("a iff b", llm_rewrite=_llm)
    assert calls == ["a iff b"]
    assert "a is equivalent to b" in with_llm

    # Absent => never called, fully rule-based.
    calls.clear()
    QR.expand_query("a iff b")
    assert calls == []


def test_rewrite_is_deduplicated():
    # Original that also appears as an expansion surface must not duplicate.
    multi = QR.rewrite_for_retrieval("add", hyde=lambda q: "add")
    lowered = [m.lower() for m in multi]
    assert len(lowered) == len(set(lowered))


# --------------------------------------------------------------------------- #
# run — op dispatch.
# --------------------------------------------------------------------------- #
def test_run_pure_rewrite_pass_returns_queries_without_retrieving():
    out = QR.run({"op": "query_rewrite", "query": "a iff b"})
    assert out["ok"] is True
    assert out["op"] == "query_rewrite"
    assert out["query"] == "a iff b"
    assert "a iff b" in out["queries"]
    assert "a if and only if b" in out["queries"]
    # expansions field excludes the original.
    assert "a iff b" not in out["expansions"]
    assert "a if and only if b" in out["expansions"]
    # No retriever asked for => no retrieval performed.
    assert out["count"] == 0
    assert out["results"] == []


def _mock_retriever(calls: list[str]):
    def _retrieve(query: str) -> list:
        calls.append(query)
        return [{"name": f"lemma_for::{query}", "score": 1.0}]

    return _retrieve


def test_run_multi_query_retrieval_then_merge():
    calls: list[str] = []
    out = QR.run({"op": "query_rewrite", "query": "a iff b"}, retrieve=_mock_retriever(calls))
    assert out["ok"] is True
    # Every rewritten sub-query was dispatched to the retriever.
    assert calls == out["queries"]
    assert len(calls) >= 2  # original + at least the iff-expansion
    # Merged hits are tagged with the query that found them.
    assert all("query" in r for r in out["results"])
    assert out["count"] == len(out["results"])
    assert set(out["per_query"]) == set(out["queries"])


def test_run_merge_dedups_shared_results():
    def _same(query: str) -> list:
        return [{"name": "shared_lemma", "score": 0.5}]

    out = QR.run({"op": "query_rewrite", "query": "a iff b"}, retrieve=_same)
    assert out["count"] == 1
    assert out["results"][0]["name"] == "shared_lemma"


def test_run_retrieve_flag_triggers_default_path_guarded_by_mock():
    # request["retrieve"]=True with an injected mock exercises the merge branch
    # without touching the cascade/Lean index.
    out = QR.run({"query": "x", "retrieve": True}, retrieve=lambda q: [{"name": q}])
    assert out["ok"] is True
    assert out["count"] >= 1


def test_run_is_deterministic():
    req = {"query": "f is continuous and a iff b"}
    a = QR.run(req, retrieve=lambda q: [{"name": q}])
    b = QR.run(req, retrieve=lambda q: [{"name": q}])
    assert a == b


def test_run_rejects_unknown_op():
    out = QR.run({"op": "bogus", "query": "x"})
    assert out["ok"] is False
    assert "unknown op" in out["stderr"]


def test_run_rejects_non_dict_request():
    out = QR.run(["not", "a", "dict"])  # type: ignore[arg-type]
    assert out["ok"] is False
