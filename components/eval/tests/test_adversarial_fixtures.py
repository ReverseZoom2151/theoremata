"""Adversarial expected-verdict fixtures.

These tests must pass with ``resources/`` absent, which is the CI condition, so
everything that touches a corpus is written as "either empty, or well formed".
The invariants that do NOT depend on the corpus (the verdict vocabulary, the
validation that rejects a typo) are asserted unconditionally, because those are
what stop the fixture set from silently asserting nothing.
"""
import pytest

from theoremata_tools.benchmarks import list_benchmarks, load_benchmark
from theoremata_tools.benchmarks.adversarial import (
    ADVERSARIAL_LOADERS,
    EXPECT_ACCEPT,
    EXPECT_ACCEPT_CONDITIONAL,
    EXPECT_REJECT,
    EXPECTED_VERDICTS,
    HIGHER_DYSON_PAIR,
    RAMANUJAN_TAU_HYPOTHESES,
    REJECT_REASONS,
    make_adversarial_item,
)

_NAMES = (
    "borwein_vacuity",
    "partition_elliptic",
    "higher_dyson",
    "erdos_public",
    "ramanujan_tau",
)


def _items(name):
    items = load_benchmark(name)
    assert isinstance(items, list)
    return items


# --------------------------------------------------------------------------- #
# Registration
# --------------------------------------------------------------------------- #

def test_adversarial_corpora_are_registered():
    entries = {e["name"]: e for e in list_benchmarks()}
    assert set(_NAMES) <= set(entries)
    for name in _NAMES:
        assert entries[name]["track"] == "adversarial"
        assert entries[name]["kind"] == "expected_verdict"


def test_registration_does_not_shadow_existing_corpora():
    # A name collision would silently replace a real benchmark with a fixture.
    names = [e["name"] for e in list_benchmarks()]
    assert len(names) == len(set(names))
    assert "goldbach_collatz" in names and "erdos1196" in names


@pytest.mark.parametrize("name", _NAMES)
def test_loader_degrades_cleanly_when_corpus_absent(name, monkeypatch):
    # resources/ is gitignored, so point the resolver at a directory that cannot
    # contain anything and assert we get an empty list rather than an exception.
    monkeypatch.setenv("THEOREMATA_RESOURCES", "/nonexistent-theoremata-resources")
    assert ADVERSARIAL_LOADERS[name]() == []


# --------------------------------------------------------------------------- #
# The verdict vocabulary
# --------------------------------------------------------------------------- #

def test_verdict_vocabulary_is_exactly_three_values():
    assert EXPECTED_VERDICTS == {
        "expect_accept",
        "expect_reject",
        "expect_accept_conditional",
    }
    assert REJECT_REASONS == {
        "vacuous_hypothesis",
        "unencoded_side_condition",
        "missing_witness",
    }


@pytest.mark.parametrize(
    "bogus",
    ["accept", "Accept", "EXPECT_ACCEPT", "expect_acept", "pass", "", "reject"],
)
def test_a_typo_cannot_become_a_verdict(bogus, tmp_path):
    path = tmp_path / "x.lean"
    path.write_text("theorem t : True := trivial", encoding="utf-8")
    with pytest.raises(ValueError):
        make_adversarial_item(
            id="t:1", verdict=bogus, informal="i", corpus="t", path=path
        )


def test_reject_requires_a_known_reason(tmp_path):
    path = tmp_path / "x.lean"
    path.write_text("theorem t : True := trivial", encoding="utf-8")
    with pytest.raises(ValueError):
        make_adversarial_item(
            id="t:2",
            verdict=EXPECT_REJECT,
            informal="i",
            corpus="t",
            path=path,
        )
    with pytest.raises(ValueError):
        make_adversarial_item(
            id="t:3",
            verdict=EXPECT_REJECT,
            reason="looks_wrong",
            informal="i",
            corpus="t",
            path=path,
        )


def test_conditional_accept_cannot_collapse_to_a_plain_accept(tmp_path):
    path = tmp_path / "x.lean"
    path.write_text("theorem t : True := trivial", encoding="utf-8")
    with pytest.raises(ValueError):
        make_adversarial_item(
            id="t:4",
            verdict=EXPECT_ACCEPT_CONDITIONAL,
            informal="i",
            corpus="t",
            path=path,
        )
    ok = make_adversarial_item(
        id="t:5",
        verdict=EXPECT_ACCEPT_CONDITIONAL,
        informal="i",
        corpus="t",
        path=path,
        hypotheses=["ABC"],
        required_report_fields=["hypotheses"],
    )
    assert ok["expected"]["verdict"] == EXPECT_ACCEPT_CONDITIONAL
    assert ok["expected"]["hypotheses"] == ["ABC"]
    # It must not be readable as an unconditional accept.
    assert ok["expected"]["verdict"] != EXPECT_ACCEPT


# --------------------------------------------------------------------------- #
# Item shape (skipped when the corpus is absent)
# --------------------------------------------------------------------------- #

@pytest.mark.parametrize("name", _NAMES)
def test_every_item_carries_a_valid_verdict(name):
    for item in _items(name):
        verdict = item["expected"]["verdict"]
        assert verdict in EXPECTED_VERDICTS
        assert item["grading"]["expected_verdict"] == verdict
        assert item["grading"]["track"] == "adversarial"
        if verdict == EXPECT_REJECT:
            assert item["expected"]["reason"] in REJECT_REASONS
        else:
            assert item["expected"]["reason"] is None
        if verdict == EXPECT_ACCEPT_CONDITIONAL:
            assert item["expected"]["hypotheses"]


@pytest.mark.parametrize("name", _NAMES)
def test_corpus_excerpts_are_fenced_as_untrusted(name):
    for item in _items(name):
        assert item["provenance"]["untrusted"] is True
        excerpt = item["expected"]["excerpt"]
        assert excerpt.startswith("BEGIN UNTRUSTED CORPUS EXCERPT")
        assert excerpt.rstrip().endswith("END UNTRUSTED CORPUS EXCERPT")


def test_borwein_pair_differs_only_in_verdict():
    items = {i["id"]: i for i in _items("borwein_vacuity")}
    if not items:
        pytest.skip("corpus absent")
    probe = items["borwein_vacuity:false_premise"]
    assert probe["expected"]["verdict"] == EXPECT_REJECT
    assert probe["expected"]["reason"] == "vacuous_hypothesis"
    assert "7.6063" in probe["expected"]["excerpt"]
    control = items.get("borwein_vacuity:corrected_control")
    if control is not None:
        assert control["expected"]["verdict"] == EXPECT_ACCEPT
        assert "7.10321" in control["expected"]["excerpt"]


def test_higher_dyson_batches_are_paired():
    items = _items("higher_dyson")
    if not items:
        pytest.skip("corpus absent")
    by_role = {}
    for item in items:
        by_role.setdefault(item["provenance"]["role"], []).append(item)
        assert item["provenance"]["pair"] == HIGHER_DYSON_PAIR
    # Batch4 is the probe; batches 2 and 3 are the control with no Summable
    # hypotheses at all. Without both halves the contrast is not assertable.
    assert [i["provenance"]["batch"] for i in by_role["probe"]] == ["Batch4"]
    assert sorted(i["provenance"]["batch"] for i in by_role["control"]) == [
        "Batch2",
        "Batch3",
    ]
    assert all(i["expected"]["verdict"] == EXPECT_REJECT for i in by_role["probe"])
    assert all(
        i["expected"]["reason"] == "missing_witness" for i in by_role["probe"]
    )
    assert all(
        i["expected"]["verdict"] == EXPECT_ACCEPT for i in by_role["control"]
    )


def test_partition_elliptic_is_an_unencoded_side_condition_reject():
    items = _items("partition_elliptic")
    if not items:
        pytest.skip("corpus absent")
    (item,) = items
    assert item["expected"]["verdict"] == EXPECT_REJECT
    assert item["expected"]["reason"] == "unencoded_side_condition"


def test_erdos_public_includes_three_refutations():
    items = _items("erdos_public")
    if not items:
        pytest.skip("corpus absent")
    assert all(i["expected"]["verdict"] == EXPECT_ACCEPT for i in items)
    refutations = sorted(
        i["provenance"]["problem"]
        for i in items
        if i["provenance"]["shape"] == "refutation"
    )
    assert refutations == ["Erdos231", "Erdos328", "Erdos441"]


def test_ramanujan_tau_is_conditional_and_carries_its_hypotheses():
    items = _items("ramanujan_tau")
    if not items:
        pytest.skip("corpus absent")
    (item,) = items
    assert item["expected"]["verdict"] == EXPECT_ACCEPT_CONDITIONAL
    assert item["expected"]["hypotheses"] == RAMANUJAN_TAU_HYPOTHESES
    assert "hypotheses" in item["expected"]["required_report_fields"]
    assert "warnings" in item["expected"]["required_report_fields"]
