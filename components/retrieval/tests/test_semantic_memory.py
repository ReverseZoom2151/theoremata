"""Tests for semantic + episodic retrieval over the lemma library (offline).

Covers: the offline hashing vectorizer ranks a semantically-similar lemma above
an unrelated one (deterministically); episodic recall returns the most similar
past goal's episode; the ``embed`` seam is honoured when injected (mock) and
falls back to the offline default otherwise; determinism; and ``run`` dispatch.

Pure standard library — no model, no network, no numpy.
"""
from theoremata_tools import semantic_memory as SM


# --------------------------------------------------------------------------- #
# Offline default embedder + SemanticLemmaIndex.
# --------------------------------------------------------------------------- #
def test_semantic_index_ranks_similar_above_unrelated():
    idx = SM.SemanticLemmaIndex()  # offline default embedder
    idx.add("Nat.add_comm", "addition of natural numbers is commutative")
    idx.add("List.reverse_reverse", "reversing a list twice gives the list")
    results = idx.query("commutativity of natural number addition", k=2)
    assert results, "expected at least one hit"
    # The commutativity lemma shares tokens (add, commut, natural, number) and
    # must outrank the unrelated list-reversal lemma.
    assert results[0][0] == "Nat.add_comm"
    scores = {lid: s for lid, s in results}
    assert scores["Nat.add_comm"] > scores.get("List.reverse_reverse", 0.0)


def test_semantic_index_unrelated_goal_scores_low_or_absent():
    idx = SM.SemanticLemmaIndex()
    idx.add("Nat.add_comm", "addition commutative")
    # A goal with zero token overlap yields no positive-cosine hits.
    assert idx.query("topological compactness reals", k=5) == []


def test_semantic_index_deterministic():
    def build():
        idx = SM.SemanticLemmaIndex()
        idx.add("a", "sum of two even numbers is even")
        idx.add("b", "product of two odd numbers is odd")
        return idx.query("two even numbers summed", k=2)

    assert build() == build()  # identical across runs / index instances


def test_hashing_embed_is_normalized_and_deterministic():
    v1 = SM.hashing_embed("addition is commutative")
    v2 = SM.hashing_embed("addition is commutative")
    assert v1 == v2
    # L2-normalised (non-empty token stream).
    assert abs(sum(x * x for x in v1) - 1.0) < 1e-9
    # Self-cosine is 1.0; disjoint text is 0.0.
    assert abs(SM.cosine(v1, v2) - 1.0) < 1e-9
    assert SM.cosine(v1, SM.hashing_embed("zzz_unrelated_token")) == 0.0


def test_add_overwrites_existing_id():
    idx = SM.SemanticLemmaIndex()
    idx.add("x", "list reversal")
    idx.add("x", "addition commutative")  # overwrite, not duplicate
    assert len(idx) == 1
    res = idx.query("commutative addition", k=5)
    assert res and res[0][0] == "x"


# --------------------------------------------------------------------------- #
# EpisodicRecall.
# --------------------------------------------------------------------------- #
def test_episodic_recall_returns_most_similar_past_goal():
    rec = SM.EpisodicRecall()
    rec.add("prove that n + 0 = n for naturals", {"proof": "simp"})
    rec.add("prove that a list reversed twice is itself", {"proof": "induction"})
    hits = rec.recall("show n + 0 = n", k=1)
    assert len(hits) == 1
    assert hits[0]["goal"] == "prove that n + 0 = n for naturals"
    assert hits[0]["episode"] == {"proof": "simp"}
    assert hits[0]["score"] > 0.0


def test_episodic_recall_empty_and_ordering():
    rec = SM.EpisodicRecall()
    assert rec.recall("anything", k=3) == []
    rec.add("integral of a polynomial", "trace-A")
    rec.add("derivative of a polynomial", "trace-B")
    hits = rec.recall("integral of a polynomial function", k=2)
    # Best match first; exact-topic episode leads.
    assert hits[0]["episode"] == "trace-A"


# --------------------------------------------------------------------------- #
# The embed seam.
# --------------------------------------------------------------------------- #
def test_injected_embed_seam_is_used():
    calls: list[str] = []

    def mock_embed(text: str) -> list[float]:
        calls.append(text)
        # Tiny 2-D embedding: axis 0 fires on "alpha", axis 1 on "beta".
        return [1.0 if "alpha" in text else 0.0, 1.0 if "beta" in text else 0.0]

    idx = SM.SemanticLemmaIndex(embed=mock_embed)
    idx.add("A", "alpha lemma")
    idx.add("B", "beta lemma")
    res = idx.query("alpha goal", k=2)
    assert calls, "injected embedder was never called"
    assert res[0][0] == "A"  # ranked by the mock embedding, not the default
    # Only the alpha axis is positive => beta lemma has 0 cosine, dropped.
    assert [lid for lid, _ in res] == ["A"]


def test_default_embed_used_when_not_injected():
    idx = SM.SemanticLemmaIndex()
    # The offline default is bound; a query with overlap returns a hit.
    idx.add("A", "greatest common divisor divides both arguments")
    assert idx.query("greatest common divisor", k=1)


def test_seam_shared_by_episodic_recall():
    def mock_embed(text: str) -> list[float]:
        return [float(len(text))]  # 1-D; any positive text -> positive cosine 1.0

    rec = SM.EpisodicRecall(embed=mock_embed)
    rec.add("g1", "ep1")
    hits = rec.recall("some goal", k=1)
    assert hits and hits[0]["episode"] == "ep1"


# --------------------------------------------------------------------------- #
# run() dispatch.
# --------------------------------------------------------------------------- #
def test_run_semantic_lemma_query_dict_lemmas():
    resp = SM.run(
        {
            "op": "semantic_lemma_query",
            "goal": "commutativity of addition",
            "k": 2,
            "lemmas": [
                {"id": "Nat.add_comm", "statement": "addition is commutative"},
                {"name": "List.reverse", "statement": "reverse of a list"},
            ],
        }
    )
    assert resp["ok"] is True
    assert resp["op"] == "semantic_lemma_query"
    assert resp["count"] == resp["count"]
    assert resp["results"][0]["lemma_id"] == "Nat.add_comm"
    assert "score" in resp["results"][0]


def test_run_semantic_lemma_query_pair_lemmas():
    resp = SM.run(
        {
            "op": "semantic_lemma_query",
            "query": "list reversal",
            "lemmas": [["L", "reversing a list"], ["N", "adding numbers"]],
        }
    )
    assert resp["ok"] is True
    assert resp["results"][0]["lemma_id"] == "L"


def test_run_episodic_recall():
    resp = SM.run(
        {
            "op": "episodic_recall",
            "goal": "prove commutativity of addition",
            "k": 1,
            "episodes": [
                {"goal": "prove addition is commutative", "episode": {"proof": "ring"}},
                {"goal": "reverse a list twice", "episode": {"proof": "simp"}},
            ],
        }
    )
    assert resp["ok"] is True
    assert resp["op"] == "episodic_recall"
    assert resp["count"] == 1
    assert resp["results"][0]["episode"] == {"proof": "ring"}


def test_run_unknown_op():
    resp = SM.run({"op": "nope"})
    assert resp["ok"] is False
    assert "unknown" in resp["error"]
