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
    MAXWELL_EQUATIONS_ATTRIBUTION,
    MAXWELL_EQUATIONS_TOTAL_THEOREMS,
    MAXWELL_EQUATIONS_TRIVIAL_THEOREMS,
    RAMANUJAN_TAU_HYPOTHESES,
    REJECT_REASONS,
    TRIVIAL_EXISTENTIAL_DIR,
    TRIVIAL_EXISTENTIAL_PAIR,
    make_adversarial_item,
)

#: Corpora globbed out of the gitignored ``resources/`` tree. Absence is normal
#: for these, so every assertion below is written as "either empty, or well formed".
_VENDORED_NAMES = (
    "borwein_vacuity",
    "partition_elliptic",
    "higher_dyson",
    "erdos_public",
    "ramanujan_tau",
    "maxwell_equations",
)

#: Corpora we wrote, committed under ``components/eval/fixtures/``. These are ALWAYS
#: present, so they are held to stricter assertions: no skip branch, and no
#: absent-corpus test, because for them absence is a failure and not a condition.
_IN_TREE_NAMES = ("trivial_existential",)

_NAMES = _VENDORED_NAMES + _IN_TREE_NAMES


def _items(name):
    items = load_benchmark(name)
    assert isinstance(items, list)
    return items


def _strip_lean_comments(src: str) -> str:
    """Drop Lean block ``/- ... -/`` and line ``--`` comments.

    Used so a code-only assertion is not tripped by explanatory prose that names
    the very tokens (``sorry``, ``axiom``) the code is asserted to avoid. Handles
    the nested block comments Lean allows; not a full lexer, but the fixtures are
    ours and contain no string literals carrying comment openers.
    """
    out: list[str] = []
    i, depth, n = 0, 0, len(src)
    while i < n:
        two = src[i : i + 2]
        if two == "/-":
            depth += 1
            i += 2
        elif two == "-/" and depth:
            depth -= 1
            i += 2
        elif depth:
            i += 1
        elif two == "--":
            j = src.find("\n", i)
            i = n if j == -1 else j
        else:
            out.append(src[i])
            i += 1
    return "".join(out)


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


@pytest.mark.parametrize("name", _VENDORED_NAMES)
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
        "name_claims_more_than_statement",
    }


@pytest.mark.parametrize(
    "bogus",
    [
        # Near-misses on the fourth reason. The vocabulary is only worth having if
        # a plausible mistyping is refused as loudly as an obvious one.
        "name_claims_more_than_the_statement",
        "name_claims_more_than_statment",
        "NAME_CLAIMS_MORE_THAN_STATEMENT",
        "trivial_conclusion",
        "name_claim_drift",
    ],
)
def test_a_typo_cannot_become_a_reject_reason(bogus, tmp_path):
    path = tmp_path / "x.lean"
    path.write_text("theorem t : True := trivial", encoding="utf-8")
    with pytest.raises(ValueError):
        make_adversarial_item(
            id="t:6",
            verdict=EXPECT_REJECT,
            reason=bogus,
            informal="i",
            corpus="t",
            path=path,
        )


def test_the_fourth_reason_is_accepted(tmp_path):
    path = tmp_path / "x.lean"
    path.write_text("theorem t : True := trivial", encoding="utf-8")
    item = make_adversarial_item(
        id="t:7",
        verdict=EXPECT_REJECT,
        reason="name_claims_more_than_statement",
        informal="i",
        corpus="t",
        path=path,
    )
    assert item["expected"]["reason"] == "name_claims_more_than_statement"
    assert item["grading"]["reject_reason"] == "name_claims_more_than_statement"


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
def test_corpus_excerpts_are_fenced(name):
    # The fence is unconditional, including on our own files: a uniform fence is
    # cheaper to reason about downstream than a per-item exception.
    for item in _items(name):
        excerpt = item["expected"]["excerpt"]
        assert excerpt.startswith("BEGIN UNTRUSTED CORPUS EXCERPT")
        assert excerpt.rstrip().endswith("END UNTRUSTED CORPUS EXCERPT")


@pytest.mark.parametrize("name", _VENDORED_NAMES)
def test_vendored_items_are_flagged_untrusted(name):
    for item in _items(name):
        assert item["provenance"]["untrusted"] is True


@pytest.mark.parametrize("name", _IN_TREE_NAMES)
def test_in_tree_items_are_not_flagged_untrusted(name):
    # Flagging our own committed source as third-party data would make the flag
    # useless for the thing it exists to mark.
    for item in _items(name):
        assert item["provenance"]["untrusted"] is False
        assert item["provenance"]["license"] == "first_party"


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
    # Batch3 only. Batch2 is deliberately unregistered: its solution.lean carries
    # a sorry, so it is an unfinished proof rather than a clean control, and
    # asserting the gate must accept it would assert the gate is broken.
    assert sorted(i["provenance"]["batch"] for i in by_role["control"]) == ["Batch3"]
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


# --------------------------------------------------------------------------- #
# maxwell_equations: third-party, MIT, vendored under the gitignored resources/
# --------------------------------------------------------------------------- #

def test_maxwell_equations_is_a_name_claim_reject():
    items = _items("maxwell_equations")
    if not items:
        pytest.skip("corpus absent")
    (item,) = items
    assert item["expected"]["verdict"] == EXPECT_REJECT
    # The point of registering it: the fourth reason was backed only by our own
    # clean-room probe, which can confirm nothing except that we catch what we wrote.
    assert item["expected"]["reason"] == "name_claims_more_than_statement"
    assert item["provenance"]["third_party"] is True


def test_maxwell_equations_records_its_licence_and_attribution():
    items = _items("maxwell_equations")
    if not items:
        pytest.skip("corpus absent")
    (item,) = items
    # MIT is what permits the excerpt to exist at all, and the notice has to travel
    # with it, so both facts live in the item rather than only in the mining doc.
    assert item["provenance"]["license"] == "MIT"
    assert item["provenance"]["attribution"] == MAXWELL_EQUATIONS_ATTRIBUTION
    assert item["provenance"]["untrusted"] is True


def test_maxwell_equations_admits_we_cannot_catch_it_yet():
    items = _items("maxwell_equations")
    if not items:
        pytest.skip("corpus absent")
    (item,) = items
    # Same admission the clean-room probe carries, and for the same reason: the file
    # is sorry-free, axiom-clean and statement-preserving, so no gate we ship objects.
    assert item["provenance"]["expected_to_fail_today"] is True
    assert item["provenance"]["occurrences_in_corpus"] == (
        MAXWELL_EQUATIONS_TRIVIAL_THEOREMS
    )
    assert item["provenance"]["theorems_in_corpus"] == MAXWELL_EQUATIONS_TOTAL_THEOREMS


def test_maxwell_equations_excerpt_reaches_the_offending_theorem():
    items = _items("maxwell_equations")
    if not items:
        pytest.skip("corpus absent")
    (item,) = items
    excerpt = item["expected"]["excerpt"]
    # The theorem starts past the default excerpt budget. An excerpt that stops short
    # of it would make the item unreviewable without opening the vendored file, which
    # is exactly what the excerpt exists to avoid.
    assert "theorem xHyperbolicity" in excerpt
    assert "= (xFluxJacobianEigenExprs C P U).lambda1" in excerpt


def test_maxwell_equations_surfaces_only_lean_sources():
    items = _items("maxwell_equations")
    if not items:
        pytest.skip("corpus absent")
    for item in items:
        path = item["provenance"]["path"]
        assert path.endswith(".lean"), path
        # README.md is the author's claim about the artifact and must never reach an
        # item; the mining report replaces it with measurements.
        assert "README" not in path
        assert "Highlights" not in item["expected"]["excerpt"]


# --------------------------------------------------------------------------- #
# trivial_existential: ours, in-tree, never absent
# --------------------------------------------------------------------------- #

def test_trivial_existential_fixture_files_exist():
    # No skip branch anywhere in this section. These files are committed, so a
    # missing one is a broken checkout and must be red rather than green-by-skip.
    assert TRIVIAL_EXISTENTIAL_DIR.is_dir()
    for name in ("probe.lean", "control.lean"):
        path = TRIVIAL_EXISTENTIAL_DIR / name
        assert path.is_file(), path
        assert path.read_text(encoding="utf-8").strip()


def test_trivial_existential_ignores_the_resources_override(monkeypatch):
    # Every other loader resolves through THEOREMATA_RESOURCES. This one must not:
    # our own tree is not a vendored corpus, and pointing the override at nothing
    # must not make the pair vanish.
    monkeypatch.setenv("THEOREMATA_RESOURCES", "/nonexistent-theoremata-resources")
    assert len(ADVERSARIAL_LOADERS["trivial_existential"]()) == 2


def test_trivial_existential_pair_is_probe_and_control():
    items = {i["provenance"]["role"]: i for i in _items("trivial_existential")}
    assert sorted(items) == ["control", "probe"]

    probe, control = items["probe"], items["control"]
    for item in (probe, control):
        assert item["provenance"]["pair"] == TRIVIAL_EXISTENTIAL_PAIR
        assert item["provenance"]["clean_room"] is True
        assert item["provenance"]["in_tree"] is True

    # The two halves must be distinguishable in the only way that matters.
    assert probe["expected"]["verdict"] == EXPECT_REJECT
    assert probe["expected"]["reason"] == "name_claims_more_than_statement"
    assert control["expected"]["verdict"] == EXPECT_ACCEPT
    assert control["expected"]["reason"] is None
    assert probe["id"] != control["id"]


def test_trivial_existential_probe_admits_we_cannot_catch_it_yet():
    # The admission is machine-readable so a run can report "0 caught, 1 expected
    # to fail" instead of a bare red, and so removing the flag is a deliberate act
    # once a triviality check exists.
    items = {i["provenance"]["role"]: i for i in _items("trivial_existential")}
    assert items["probe"]["provenance"]["expected_to_fail_today"] is True
    assert items["control"]["provenance"]["expected_to_fail_today"] is False


def test_trivial_existential_probe_is_trivial_and_control_is_not():
    # Reading the actual Lean, because the whole claim of the pair is a property of
    # these two files and not of the metadata we wrote about them.
    probe = (TRIVIAL_EXISTENTIAL_DIR / "probe.lean").read_text(encoding="utf-8")
    control = (TRIVIAL_EXISTENTIAL_DIR / "control.lean").read_text(encoding="utf-8")

    # Same theorem name in both halves; that identity is what makes the pair a pair.
    assert "theorem spectrumIsOrdered" in probe
    assert "theorem spectrumIsOrdered" in control

    # The probe states an existential equation and proves it by reflexivity; the
    # control states the ordering the name claims.
    assert "∃ x : Int, x = (spectrum p).lo" in probe
    assert "rfl" in probe
    assert "(spectrum p).lo ≤ (spectrum p).hi" in control

    # Nothing crude objects to the probe. If any of these ever appear in CODE the
    # fixture has stopped testing the layer it was built for. The comments discuss
    # these words on purpose (they explain what the probe evades), so strip comments
    # before checking rather than banning the words from the prose too.
    probe_code = _strip_lean_comments(probe)
    control_code = _strip_lean_comments(control)
    for banned in ("sorry", "admit", "native_decide", "axiom "):
        assert banned not in probe_code, banned

    # Mathlib-free, so the pair runs on every commit rather than nightly.
    assert "import" not in probe_code
    assert "import" not in control_code
