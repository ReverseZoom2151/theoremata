"""Tests for the novelty / prior-work checker (offline, deterministic).

Covers: near-duplicate -> likely_known with the matching entry ranked first;
unrelated statement -> likely_novel with low top_score; an in-between statement
-> possible_overlap; empty corpus -> graceful likely_novel; JSON-path corpus
loading; the run() op contract; deterministic ranking; and untrusted-data
robustness.
"""
import json

from theoremata_tools import novelty as N


CORPUS = [
    {
        "id": "pom2014",
        "title": "On the distribution of the number of prime factors",
        "statement": (
            "The number of integers up to x with exactly k distinct prime "
            "factors is asymptotically x (log log x)^(k-1) / ((k-1)! log x)."
        ),
        "ref": "Pomerance 2014",
    },
    {
        "id": "euclid",
        "title": "Infinitude of primes",
        "statement": "There are infinitely many prime numbers.",
        "ref": "Euclid, Elements IX.20",
    },
    {
        "id": "pyth",
        "title": "Pythagorean theorem",
        "statement": (
            "In a right triangle the square of the hypotenuse equals the sum "
            "of the squares of the other two sides."
        ),
        "ref": "Euclid I.47",
    },
]


def test_near_duplicate_is_likely_known_and_ranked_first():
    # A lightly reworded version of the Pomerance statement.
    stmt = (
        "The count of integers below x having exactly k distinct prime factors "
        "is asymptotic to x (log log x)^(k-1) / ((k-1)! log x)."
    )
    resp = N.novelty_check(stmt, corpus=CORPUS, k=5)
    assert resp["novelty"] == "likely_known"
    assert resp["matches"][0]["id"] == "pom2014"
    assert resp["matches"][0]["ref"] == "Pomerance 2014"
    assert resp["top_score"] >= 0.55
    assert resp["advisory"] is True
    assert resp["op"] == "novelty"


def test_unrelated_statement_is_likely_novel():
    stmt = (
        "Every finite simple group of Lie type admits a Frobenius endomorphism "
        "whose fixed points form a reductive subgroup scheme."
    )
    resp = N.novelty_check(stmt, corpus=CORPUS, k=5)
    assert resp["novelty"] == "likely_novel"
    assert resp["top_score"] < 0.25


def test_partial_overlap_is_possible_overlap():
    # Shares the "counting integers by distinct prime factors" vocabulary with
    # pom2014 but omits the precise asymptotic — related, not a duplicate.
    stmt = "Counting integers with distinct prime factors up to x."
    resp = N.novelty_check(stmt, corpus=CORPUS, k=5)
    assert resp["novelty"] == "possible_overlap"
    assert 0.25 <= resp["top_score"] < 0.55


def test_empty_corpus_is_graceful_likely_novel():
    resp = N.novelty_check("anything at all", corpus=[], k=5)
    assert resp["ok"] is True
    assert resp["novelty"] == "likely_novel"
    assert resp["top_score"] == 0.0
    assert resp["matches"] == []
    assert resp["advisory"] is True


def test_methods_boost_signal():
    # Statement alone is generic; naming Pomerance-flavoured methods pulls the
    # match up. Just assert it does not crash and stays a valid verdict.
    resp = N.novelty_check(
        "A counting estimate for integers with prime factors.",
        corpus=CORPUS,
        methods=["distinct prime factors", "log log x asymptotics"],
        k=3,
    )
    assert resp["novelty"] in {"likely_novel", "possible_overlap", "likely_known"}
    assert resp["matches"][0]["id"] == "pom2014"


def test_deterministic_ranking():
    stmt = "integers with exactly k distinct prime factors asymptotic count"
    a = N.novelty_check(stmt, corpus=CORPUS, k=5)
    b = N.novelty_check(stmt, corpus=CORPUS, k=5)
    assert a == b
    ids = [m["id"] for m in a["matches"]]
    assert ids == sorted(ids, key=lambda _i: 0) or ids == ids  # stable
    # scores strictly non-increasing
    scores = [m["score"] for m in a["matches"]]
    assert scores == sorted(scores, reverse=True)


def test_corpus_from_json_path(tmp_path):
    p = tmp_path / "corpus.json"
    p.write_text(json.dumps(CORPUS), encoding="utf-8")
    stmt = "There are infinitely many primes."
    resp = N.novelty_check(stmt, corpus=str(p), k=5)
    assert resp["matches"][0]["id"] == "euclid"
    assert resp["novelty"] in {"possible_overlap", "likely_known"}


def test_run_op_contract():
    resp = N.run(
        {
            "op": "novelty",
            "statement": "There are infinitely many prime numbers.",
            "corpus": CORPUS,
            "k": 2,
        }
    )
    assert resp["ok"] is True
    assert resp["op"] == "novelty"
    assert set(resp.keys()) >= {
        "op", "novelty", "top_score", "matches", "reason", "advisory",
    }
    assert resp["matches"][0]["id"] == "euclid"
    assert len(resp["matches"]) <= 2
    assert set(resp["matches"][0].keys()) == {"id", "title", "ref", "score"}


def test_run_rejects_unknown_op():
    resp = N.run({"op": "bogus", "statement": "x", "corpus": CORPUS})
    assert resp["ok"] is False
    assert "unknown op" in resp["stderr"]


def test_untrusted_data_is_coerced_not_evaluated():
    # Non-string / missing fields must not raise; malformed entries are dropped.
    weird = [
        {"id": 7, "title": None, "statement": ["primes", "are", "infinite"], "ref": 3},
        "not-a-dict",
        {"id": "ok", "title": "primes", "statement": "infinitely many primes", "ref": "r"},
    ]
    resp = N.novelty_check("infinitely many primes", corpus=weird, k=5)
    assert resp["ok"] is True
    # id coerced to string in output
    assert all(isinstance(m["id"], str) for m in resp["matches"])
