"""Adversarial expected-verdict fixtures.

These tests must pass with ``resources/`` absent, which is the CI condition, so
everything that touches a corpus is written as "either empty, or well formed".
The invariants that do NOT depend on the corpus (the verdict vocabulary, the
validation that rejects a typo) are asserted unconditionally, because those are
what stop the fixture set from silently asserting nothing.

TWO TIERS, and the split is deliberate. The pure tier below runs everywhere and
starts no Lean process. The live tier actually INVOKES
:mod:`theoremata_tools.statement_triviality` against the reject fixtures whose
reason is ``name_claims_more_than_statement``, because a fixture set that merely
records "a checker exists which would catch this" asserts nothing about the
checker. Every live test skips, never fails, when Lean, Mathlib or the gitignored
corpus is absent, following the gating in
``components/verify/tests/test_statement_triviality.py``.
"""
from pathlib import Path

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
    TAU_CETI_ATTRIBUTION,
    TAU_CETI_LICENSE,
    TAU_CETI_MATHLIB_REV,
    TAU_CETI_TOOLCHAIN,
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
    "tau_ceti",
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


def test_maxwell_equations_no_longer_admits_we_cannot_catch_it():
    items = _items("maxwell_equations")
    if not items:
        pytest.skip("corpus absent")
    (item,) = items
    # This used to assert the admission was True. It stayed True after
    # statement_triviality shipped and demonstrably flagged this very theorem,
    # which is exactly the staleness the live section below now prevents: the
    # flag is asserted against a checker we run rather than against prose.
    assert item["provenance"]["expected_to_fail_today"] is False
    assert item["provenance"]["caught_by"] == ["statement_triviality"]
    assert item["provenance"]["triviality_mutated_def"] == "xFluxJacobianEigenExprs"
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


def test_trivial_existential_probe_no_longer_admits_we_cannot_catch_it():
    # The admission was machine-readable so that retiring it would be a deliberate
    # act once a triviality check existed. It now exists and catches the probe, so
    # both halves read False, and the two are told apart by `caught_by` instead.
    items = {i["provenance"]["role"]: i for i in _items("trivial_existential")}
    assert items["probe"]["provenance"]["expected_to_fail_today"] is False
    assert items["control"]["provenance"]["expected_to_fail_today"] is False
    assert items["probe"]["provenance"]["caught_by"] == ["statement_triviality"]
    # Empty on the control, because naming a checker there would read as an
    # assertion that something objects to it, which nothing may.
    assert items["control"]["provenance"]["caught_by"] == []


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


# --------------------------------------------------------------------------- #
# tau_ceti: third-party, Apache-2.0, and the first ACCEPT corpus we elaborated
# --------------------------------------------------------------------------- #

def test_tau_ceti_items_are_all_plain_accepts():
    items = _items("tau_ceti")
    if not items:
        pytest.skip("corpus absent")
    # The registry was accept-poor (7 accepts against 5 rejects), which is the
    # gap this corpus exists to close. A conditional accept smuggled in here
    # would not close it: expect_accept_conditional demands hypotheses in the
    # report and is a different assertion entirely.
    assert all(i["expected"]["verdict"] == EXPECT_ACCEPT for i in items)
    assert all(i["expected"]["reason"] is None for i in items)


def test_tau_ceti_records_its_licence_and_per_file_attribution():
    items = _items("tau_ceti")
    if not items:
        pytest.skip("corpus absent")
    for item in items:
        # Apache-2.0, not the MIT default the rest of this module inherits. The
        # licence is what permits the excerpt to exist, so it travels with it.
        assert item["provenance"]["license"] == TAU_CETI_LICENSE
        assert item["provenance"]["attribution"] == TAU_CETI_ATTRIBUTION
        assert item["provenance"]["untrusted"] is True
        assert item["provenance"]["third_party"] is True


def test_tau_ceti_accepts_claim_only_an_elaboration_we_performed():
    """The whole reason this corpus is registerable, asserted as data.

    Four accept fixtures in this module once pointed at ``sorry`` stubs, so an
    accept that claims a green nobody reproduced is a known failure mode here.
    Every TauCeti item carries the elaboration claim explicitly, including the
    honest half: the pinned Mathlib revision is NOT the one we ran against.
    """
    items = _items("tau_ceti")
    if not items:
        pytest.skip("corpus absent")
    for item in items:
        prov = item["provenance"]
        assert prov["elaborated_here"] is True
        assert prov["imports"] == "mathlib_only"
        # The toolchain matches exactly, which is why a failure would have been a
        # contradiction rather than something excusable as version drift.
        assert prov["toolchain_pinned"] == TAU_CETI_TOOLCHAIN
        assert prov["toolchain_matches_ours"] is True
        # The Mathlib revision does not, and the item says so rather than
        # implying a match by staying silent.
        assert prov["mathlib_rev_pinned"] == TAU_CETI_MATHLIB_REV
        assert prov["mathlib_rev_matches"] is False


def test_tau_ceti_surfaces_only_lean_sources_under_the_library():
    items = _items("tau_ceti")
    if not items:
        pytest.skip("corpus absent")
    for item in items:
        path = item["provenance"]["path"].replace("\\", "/")
        assert path.endswith(".lean"), path
        assert "/TauCeti/" in path, path
        # TauCeti ships AGENTS.md and COORDINATION.md, which are imperative prose
        # addressed at an agent, plus a README of author claims. None of the three
        # may ever reach an item, and other tests in this tree pin those files.
        for forbidden in ("README", "AGENTS", "CLAUDE", "COORDINATION"):
            assert forbidden not in path, path


def test_tau_ceti_ids_are_unique_and_namespaced():
    items = _items("tau_ceti")
    if not items:
        pytest.skip("corpus absent")
    ids = [i["id"] for i in items]
    assert len(ids) == len(set(ids))
    assert all(i.startswith("tau_ceti:") for i in ids)


def test_adversarial_registry_is_no_longer_accept_poor():
    """The measurement that motivated the corpus, kept as a live number.

    Skipped rather than asserted when the vendored corpora are missing, because
    with ``resources/`` absent the only items that load are our own in-tree pair
    and the ratio would be a statement about nothing.
    """
    if not _items("tau_ceti"):
        pytest.skip("tau_ceti corpus absent, so the ratio measures nothing")
    accepts, rejects = 0, 0
    for name in _NAMES:
        for item in _items(name):
            verdict = item["expected"]["verdict"]
            accepts += verdict in (EXPECT_ACCEPT, EXPECT_ACCEPT_CONDITIONAL)
            rejects += verdict == EXPECT_REJECT
    # A gate that refuses everything must not be able to score well here.
    assert accepts > rejects * 2, (accepts, rejects)


# --------------------------------------------------------------------------- #
# The name/claim reason, actually exercised
#
# Everything above asserts what the fixtures SAY. This section runs the checker
# they are about. The pure half starts no Lean process so CI still exercises
# something real; the live half drives an elaborator and skips when it cannot.
# --------------------------------------------------------------------------- #

#: components/eval/tests/this_file.py -> [0]=tests [1]=eval [2]=components [3]=root
_REPO_ROOT = Path(__file__).resolve().parents[3]
_MATHLIB = _REPO_ROOT / "resources" / "mathlib4-master" / "mathlib4-master"

try:
    from theoremata_tools.statement_triviality import (
        VERDICT_NOT_SHOWN_TRIVIAL,
        VERDICT_TRIVIAL,
        VERDICT_WITHHELD,
        check_statement_triviality,
        lean_available,
        plan_mutation,
    )
except ImportError:  # pragma: no cover - the verify component is a sibling
    _TRIVIALITY_IMPORT_OK = False
else:
    _TRIVIALITY_IMPORT_OK = True

try:
    from theoremata_tools.opaque_statement import (
        VERDICT_OPAQUE,
        check_statement_constants,
    )
except ImportError:  # pragma: no cover
    _OPAQUE_IMPORT_OK = False
else:
    _OPAQUE_IMPORT_OK = True

needs_checker = pytest.mark.skipif(
    not _TRIVIALITY_IMPORT_OK,
    reason="theoremata_tools.statement_triviality is not importable here",
)

# `lean_available` shells out, so it is evaluated once at collection rather than
# once per test. A box with no toolchain skips; it never reds.
needs_lean = pytest.mark.skipif(
    not (_TRIVIALITY_IMPORT_OK and lean_available()),
    reason="no Lean toolchain on PATH",
)

#: Which fixture ids carry ``name_claims_more_than_statement``, and which theorem
#: in each file the reason is about. The theorem name cannot be derived from the
#: item, and guessing it would make a green here meaningless.
_NAME_CLAIM_TARGETS = {
    "trivial_existential:probe": "spectrumIsOrdered",
    "maxwell_equations:hyperbolicity": "xHyperbolicity",
}


def _name_claim_items() -> dict:
    out = {}
    for name in _NAMES:
        for item in _items(name):
            if item["expected"]["reason"] == "name_claims_more_than_statement":
                out[item["id"]] = item
    return out


def _abs_path(item) -> Path:
    raw = Path(item["provenance"]["path"])
    return raw if raw.is_absolute() else (_REPO_ROOT / raw)


def test_every_name_claim_reject_has_a_named_target_theorem():
    # Pure. Guards the table above: a new fixture carrying this reason and no
    # entry would silently stop being exercised by everything below it.
    unknown = set(_name_claim_items()) - set(_NAME_CLAIM_TARGETS)
    assert not unknown, (
        "these fixtures claim name_claims_more_than_statement but name no "
        f"theorem for the checker to run on: {sorted(unknown)}"
    )


@needs_checker
def test_name_claim_rejects_are_inside_the_checkers_covered_class():
    """Pure: planning runs no Lean, so this executes on a bare CI box.

    A ``withheld`` plan would mean the checker declines to look at the very
    fixtures whose reason it is supposed to back, which is a silent hole that no
    amount of green elsewhere would reveal.
    """
    items = _name_claim_items()
    if not items:
        pytest.skip("no name_claims_more_than_statement fixtures loaded")
    for item_id, item in sorted(items.items()):
        path = _abs_path(item)
        if not path.is_file():
            continue
        plan = plan_mutation(
            path.read_text(encoding="utf-8", errors="replace"),
            _NAME_CLAIM_TARGETS[item_id],
        )
        assert plan["verdict"] != VERDICT_WITHHELD, (item_id, plan.get("reason"))
        assert plan["mutated_defs"], item_id


@needs_checker
def test_no_name_claim_fixture_still_claims_we_cannot_catch_it():
    """The staleness this section exists to stop from coming back.

    Both fixtures carried ``expected_to_fail_today: True`` after the checker that
    catches them had already shipped. The flag is only worth having if a stale
    True is loud, so it is pinned here alongside the checker that retired it.
    """
    items = _name_claim_items()
    if not items:
        pytest.skip("no name_claims_more_than_statement fixtures loaded")
    for item_id, item in sorted(items.items()):
        assert item["provenance"]["expected_to_fail_today"] is False, item_id
        assert "statement_triviality" in item["provenance"]["caught_by"], item_id


@needs_lean
def test_probe_is_flagged_trivial_and_control_is_not(tmp_path):
    """The in-tree pair, both halves, with a real elaborator.

    Mathlib-free and import-free, so this costs about a second per half and needs
    no build directory: it runs wherever any Lean 4 exists.

    THE CONTROL IS THE POINT. Flagging the probe alone is satisfied by a heuristic
    that flags every short existential proved by an anonymous constructor, and that
    heuristic fires on honest mathematics. Only the contrast shows the checker is
    reading the statement rather than its shape.
    """
    items = {i["provenance"]["role"]: i for i in _items("trivial_existential")}
    probe, control = items["probe"], items["control"]

    hit = check_statement_triviality(
        str(_abs_path(probe)),
        _NAME_CLAIM_TARGETS[probe["id"]],
        work_dir=str(tmp_path / "probe"),
        timeout=300.0,
    )
    assert hit["verdict"] == VERDICT_TRIVIAL, hit
    assert hit["mutated_defs"] == ["spectrum"]
    # Evidence, not inference: every stage really compiled, so the accusation
    # rests on the proof surviving rather than on a mutant that never ran.
    assert all(s["ok"] for s in hit["stages"]), hit["stages"]

    miss = check_statement_triviality(
        str(_abs_path(control)),
        "spectrumIsOrdered",
        work_dir=str(tmp_path / "control"),
        timeout=300.0,
    )
    assert miss["verdict"] != VERDICT_TRIVIAL, miss
    assert miss["verdict"] == VERDICT_NOT_SHOWN_TRIVIAL, miss
    # And it survived for the right reason: the mutated statement elaborated
    # (stage A ok) and the proof then failed to close it (stage B not ok). A
    # stage-A failure would have been the checker declining to look, not the
    # control passing on its merits.
    stage_a = [s for s in miss["stages"] if s.get("stage") == "A"]
    stage_b = [s for s in miss["stages"] if s.get("stage") == "B"]
    assert stage_a and all(s["ok"] for s in stage_a)
    assert stage_b and not stage_b[-1]["ok"]


@needs_lean
@pytest.mark.skipif(
    not (_MATHLIB / ".lake" / "build" / "lib" / "lean" / "Mathlib.olean").is_file(),
    reason="no BUILT Mathlib at resources/mathlib4-master (gitignored; CI has none)",
)
def test_maxwell_hyperbolicity_is_flagged_trivial(tmp_path):
    """The third-party half, which is the one that establishes anything.

    The clean-room pair can only ever show we catch the shape we ourselves wrote.
    This is a published artifact from someone else's generator, and the same check
    reaches the same verdict on it.

    Minutes, not seconds: the file imports Mathlib and the staged check compiles
    five mutants. Skipped wherever the corpus or the built Mathlib is missing,
    which is every CI box, since ``resources/`` is gitignored.
    """
    items = _items("maxwell_equations")
    if not items:
        pytest.skip("MaxwellEquations corpus absent")
    (item,) = items
    out = check_statement_triviality(
        str(_abs_path(item)),
        _NAME_CLAIM_TARGETS[item["id"]],
        work_dir=str(tmp_path),
        lake_workspace=str(_MATHLIB),
        timeout=1800.0,
    )
    if out["verdict"] == VERDICT_WITHHELD:
        # Failing closed is the checker behaving correctly, and a Mathlib that
        # cannot elaborate the vendored file is not evidence about the fixture.
        pytest.skip(f"triviality check withheld, so it settles nothing: {out['reason']}")
    assert out["verdict"] == VERDICT_TRIVIAL, out
    assert out["mutated_defs"] == [item["provenance"]["triviality_mutated_def"]]
    assert all(s["ok"] for s in out["stages"]), out["stages"]


@needs_lean
@pytest.mark.skipif(
    not _OPAQUE_IMPORT_OK, reason="theoremata_tools.opaque_statement is not importable"
)
def test_opaque_statement_does_not_explain_the_probe():
    """The two checkers are orthogonal, and only one of them owns this reason.

    ``opaque_statement`` accuses when a constant in the STATEMENT carries
    ``sorryAx``. The probe's constants are all real definitions with real bodies,
    so it must report nothing. Running it here is what keeps the credit attached
    to ``statement_triviality``: if the opaque check also fired, the fixture would
    not be demonstrating what its reason says it demonstrates.
    """
    probe = {i["provenance"]["role"]: i for i in _items("trivial_existential")}["probe"]
    source = _abs_path(probe).read_text(encoding="utf-8")
    out = check_statement_constants(source, "spectrumIsOrdered", timeout=300.0)
    # `unknown` is the withheld posture and is a legitimate outcome of a probe we
    # could not run; the only forbidden answer here is an accusation.
    assert out["verdict"] != VERDICT_OPAQUE, out
