"""Tests for the axiom-allowlist lint + #print axioms audit (formal_lint).

The BugZoo adversarial corpus is the load-bearing regression fixture: every
known-bad snippet MUST be flagged, and the paired clean snippet MUST pass.
"""
import pytest

from theoremata_tools.formal_lint import (
    BUGZOO,
    CLEAN_FIXTURES,
    DEFAULT_ALLOW,
    lint,
    lint_bugzoo_entry,
    parse_audit,
    run,
)


# --------------------------------------------------------------------------- #
# BugZoo corpus: the regression contract.
# --------------------------------------------------------------------------- #

_BUGZOO_IDS = [
    (system, entry["name"])
    for system, entries in BUGZOO.items()
    for entry in entries
]


@pytest.mark.parametrize(
    "system,entry",
    [(s, e) for s, entries in BUGZOO.items() for e in entries],
    ids=[f"{s}:{n}" for s, n in _BUGZOO_IDS],
)
def test_bugzoo_every_bad_snippet_is_flagged(system, entry):
    report = lint_bugzoo_entry(system, entry)
    assert report["clean"] is False, (
        f"{system}:{entry['name']} ({entry['why']}) was NOT flagged"
    )
    # At least one critical violation must be present.
    assert any(v["severity"] == "critical" for v in report["violations"])


@pytest.mark.parametrize("system", list(CLEAN_FIXTURES))
def test_clean_fixture_passes(system):
    report = lint(system, **CLEAN_FIXTURES[system])
    assert report["clean"] is True, report["violations"]
    assert report["violations"] == []


def test_run_bugzoo_op_all_ok():
    result = run({"op": "bugzoo"})
    assert result["ok"] is True
    # Every non-CLEAN entry flagged; every CLEAN entry passed.
    for r in result["results"]:
        if r["name"] == "CLEAN":
            assert r["passed"] is True
        else:
            assert r["flagged"] is True


# --------------------------------------------------------------------------- #
# Axiom-allowlist enforcement (audit channel).
# --------------------------------------------------------------------------- #

def test_lean_allowlisted_axioms_pass():
    audit = ("'good' depends on axioms: "
             "[propext, Classical.choice, Quot.sound]")
    report = lint("lean", audit=audit)
    assert report["clean"] is True
    assert set(report["axioms"]) == set(DEFAULT_ALLOW["lean"])


def test_lean_extra_axiom_rejected():
    audit = "'thm' depends on axioms: [propext, myAxiom]"
    report = lint("lean", audit=audit)
    assert report["clean"] is False
    bad = [v for v in report["violations"]
           if v.get("axiom") == "myAxiom"]
    assert bad and bad[0]["kind"] == "axiom_not_in_allowlist"


def test_lean_sorryAx_never_allowed_even_if_whitelisted():
    audit = "'thm' depends on axioms: [sorryAx]"
    # Caller foolishly tries to allow sorryAx; it must still be rejected.
    report = lint("lean", audit=audit, allow=["sorryAx", "propext"])
    assert report["clean"] is False
    assert any(v.get("axiom") == "sorryAx"
               and v["kind"] == "forbidden_axiom"
               for v in report["violations"])


def test_lean_no_axioms_line_is_clean():
    report = lint("lean", audit="'triv' does not depend on any axioms")
    assert report["clean"] is True
    assert report["audit"]["closed"] is True


def test_axioms_passed_directly():
    report = lint("lean", axioms=["propext", "Quot.sound"])
    assert report["clean"] is True
    report2 = lint("lean", axioms=["propext", "weird.axiom"])
    assert report2["clean"] is False


def test_rocq_closed_context_is_clean():
    report = lint("rocq", audit="Closed under the global context")
    assert report["clean"] is True
    assert report["audit"]["closed"] is True


def test_rocq_axiom_section_rejected():
    audit = "Axioms:\nclassic : forall P : Prop, P \\/ ~ P\n"
    report = lint("rocq", audit=audit)
    assert report["clean"] is False
    assert any(v.get("axiom") == "classic" for v in report["violations"])


# --------------------------------------------------------------------------- #
# Source channel (reuses formal_source_scan).
# --------------------------------------------------------------------------- #

def test_source_channel_flags_sorry():
    report = lint("lean", source="theorem t : False := by sorry")
    assert report["clean"] is False
    assert report["source_scan"] is not None
    assert any(v.get("pattern") == "sorry" for v in report["violations"])


def test_clean_source_passes():
    report = lint("lean", source="theorem t : 1 = 1 := rfl")
    assert report["clean"] is True


def test_custom_allowlist_permits_extra_axiom():
    audit = "'thm' depends on axioms: [propext, funext]"
    # By default funext is not allowed; widen the allowlist.
    strict = lint("lean", audit=audit)
    assert strict["clean"] is False
    widened = lint("lean", audit=audit,
                   allow=["propext", "Classical.choice", "Quot.sound",
                          "funext"])
    assert widened["clean"] is True


# --------------------------------------------------------------------------- #
# Parsers + error handling.
# --------------------------------------------------------------------------- #

def test_parse_audit_isabelle_oracles():
    info = parse_audit("isabelle", "oracles: {Pure.skip_proof, other}")
    assert set(info["axioms"]) == {"Pure.skip_proof", "other"}


def test_unknown_system_raises():
    with pytest.raises(ValueError):
        lint("agda", source="foo")


def test_alias_systems():
    # coq -> rocq, lean4 -> lean.
    assert lint("coq", audit="Closed under the global context")["system"] == "rocq"
    assert lint("lean4", axioms=["propext"])["system"] == "lean"
