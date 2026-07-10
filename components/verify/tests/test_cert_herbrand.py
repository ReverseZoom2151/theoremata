"""Tests for the Herbrand cert-log module (validity / unsatisfiability of a
first-order formula via a finite set of ground instances).

Offline, deterministic, exact, pure standard library.  Exercises: a genuine
UNSAT refutation and a genuine VALIDITY certificate that check ``True``; an
INSUFFICIENT instance set (still propositionally satisfiable / not a tautology)
that is REJECTED; a tampered substitution that is rejected; malformed-AST
rejection; determinism; and JSON round-trip.  The trust-critical checker decides
the propositional combination itself (exhaustive truth table) — it never trusts
the producer's claim.
"""
import copy
import json
import sys
from pathlib import Path

# The verify tools live under components/verify/python.
_ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(_ROOT / "components" / "verify" / "python"))

from theoremata_tools.cert_herbrand import (  # noqa: E402
    FORMAT,
    Atom,
    And,
    Imp,
    Not,
    Or,
    T,
    V,
    check,
    export_herbrand_cert,
    run,
)


def _roundtrip(log):
    """JSON dump/load a log (proves it is plain, transport-neutral JSON)."""
    return json.loads(json.dumps(log))


# --------------------------------------------------------------------------- #
# Ground constants / helpers used across the cases.
# --------------------------------------------------------------------------- #

A = T("a")            # constant a
B = T("b")            # constant b
X = V("x")            # variable x


def _P(t):
    return Atom("P", t)


def _Q(t):
    return Atom("Q", t)


# --------------------------------------------------------------------------- #
# UNSAT: a genuinely unsatisfiable universal set with a correct ground instance.
# --------------------------------------------------------------------------- #

def _modus_ponens_unsat_matrix():
    # forall x. (P(x) -> Q(x)) and P(a) and not Q(a)   -- unsatisfiable.
    return And(Imp(_P(X), _Q(X)), _P(A), Not(_Q(A)))


def test_unsat_with_correct_instance_checks_true():
    log = export_herbrand_cert(_modus_ponens_unsat_matrix(),
                               [{"x": A}], "unsat")
    res = check(log)
    assert res["valid"] is True, res
    assert res["kind"] == "herbrand"
    assert res["checked_steps"] == 4


def test_trivial_clause_set_p_and_not_p_unsat():
    # The ground clause set {P(a), not P(a)} with the identity (empty) subst.
    matrix = And(_P(A), Not(_P(A)))
    log = export_herbrand_cert(matrix, [{}], "unsat")
    assert check(log)["valid"] is True


# --------------------------------------------------------------------------- #
# VALID: a genuine validity certificate (disjunction of instances is a taut).
# --------------------------------------------------------------------------- #

def _drinker_like_valid_matrix():
    # exists x. (P(a) -> P(x)) is valid; instance x = a gives P(a) -> P(a).
    return Imp(_P(A), _P(X))


def test_valid_with_correct_instance_checks_true():
    log = export_herbrand_cert(_drinker_like_valid_matrix(), [{"x": A}], "valid")
    res = check(log)
    assert res["valid"] is True, res
    assert res["kind"] == "herbrand"


# --------------------------------------------------------------------------- #
# INSUFFICIENT instance set: propositionally satisfiable / not a tautology.
# --------------------------------------------------------------------------- #

def test_insufficient_unsat_instance_is_rejected():
    # Instantiate at x = b instead of x = a: the modus-ponens contradiction never
    # fires, so the conjunction of instances is still SATISFIABLE -> rejected.
    log = export_herbrand_cert(_modus_ponens_unsat_matrix(), [{"x": B}], "unsat")
    res = check(log)
    assert res["valid"] is False
    assert "SATISFIABLE" in res["reason"]


def test_insufficient_valid_instance_is_rejected():
    # Disjunction P(a) -> P(b) is not a tautology (P(a)=T, P(b)=F falsifies it).
    log = export_herbrand_cert(_drinker_like_valid_matrix(), [{"x": B}], "valid")
    res = check(log)
    assert res["valid"] is False
    assert "tautolog" in res["reason"].lower()


# --------------------------------------------------------------------------- #
# Tamper rejection: mutate a good certificate.
# --------------------------------------------------------------------------- #

def test_tampered_substitution_is_rejected():
    log = export_herbrand_cert(_modus_ponens_unsat_matrix(), [{"x": A}], "unsat")
    assert check(log)["valid"] is True
    tampered = copy.deepcopy(log)
    # Rewrite the winning substitution x=a to x=b -> no longer a refutation.
    tampered["steps"][1]["substitutions"][0]["x"] = B
    assert check(tampered)["valid"] is False


def test_tampered_matrix_atom_is_rejected():
    log = export_herbrand_cert(_modus_ponens_unsat_matrix(), [{"x": A}], "unsat")
    tampered = copy.deepcopy(log)
    # Flip not Q(a) into Q(a): the set becomes satisfiable.
    tampered["steps"][0]["matrix"]["and"][2] = _Q(A)
    assert check(tampered)["valid"] is False


def test_flipped_decision_is_rejected():
    # A genuinely-unsat matrix relabeled as a validity claim: its disjunction of
    # instances is not a tautology, so the flipped claim is rejected.
    log = export_herbrand_cert(_modus_ponens_unsat_matrix(), [{"x": A}], "unsat")
    tampered = copy.deepcopy(log)
    tampered["steps"][0]["decision"] = "valid"
    assert check(tampered)["valid"] is False


def test_non_ground_substitution_is_rejected():
    log = export_herbrand_cert(_modus_ponens_unsat_matrix(), [{"x": A}], "unsat")
    tampered = copy.deepcopy(log)
    tampered["steps"][1]["substitutions"][0]["x"] = V("y")  # not ground
    assert check(tampered)["valid"] is False


def test_missing_variable_coverage_is_rejected():
    # An empty substitution leaves x free -> instance is not ground -> rejected.
    log = export_herbrand_cert(_modus_ponens_unsat_matrix(), [{"x": A}], "unsat")
    tampered = copy.deepcopy(log)
    tampered["steps"][1]["substitutions"][0] = {}
    assert check(tampered)["valid"] is False


# --------------------------------------------------------------------------- #
# Malformed / structural rejection.
# --------------------------------------------------------------------------- #

def test_bad_format_and_kind_rejected():
    log = export_herbrand_cert(_modus_ponens_unsat_matrix(), [{"x": A}], "unsat")
    bad = copy.deepcopy(log)
    bad["format"] = "nope"
    assert check(bad)["valid"] is False
    bad2 = copy.deepcopy(log)
    bad2["kind"] = "mystery"
    assert check(bad2)["valid"] is False


def test_malformed_matrix_rejected():
    log = export_herbrand_cert(_modus_ponens_unsat_matrix(), [{"x": A}], "unsat")
    bad = copy.deepcopy(log)
    bad["steps"][0]["matrix"] = {"and": [], "or": []}  # two tags, empty
    assert check(bad)["valid"] is False


# --------------------------------------------------------------------------- #
# Determinism + round-trip + worker dispatch.
# --------------------------------------------------------------------------- #

def test_determinism():
    m = _modus_ponens_unsat_matrix()
    log1 = export_herbrand_cert(m, [{"x": A}], "unsat")
    log2 = export_herbrand_cert(m, [{"x": A}], "unsat")
    assert json.dumps(log1, sort_keys=True) == json.dumps(log2, sort_keys=True)
    assert check(log1) == check(log2)


def test_json_roundtrip_preserves_verdict():
    log = export_herbrand_cert(_modus_ponens_unsat_matrix(), [{"x": A}], "unsat")
    assert check(_roundtrip(log))["valid"] is True


def test_worker_run_export_then_check():
    exported = run({
        "op": "export",
        "matrix": _drinker_like_valid_matrix(),
        "substitutions": [{"x": A}],
        "claim": "valid",
    })
    assert exported["log"]["format"] == FORMAT
    assert run({"op": "check", "log": exported["log"]})["valid"] is True


def test_worker_check_rejects_insufficient():
    exported = run({
        "op": "export",
        "matrix": _modus_ponens_unsat_matrix(),
        "substitutions": [{"x": B}],
        "claim": "unsat",
    })
    assert run({"op": "check", "log": exported["log"]})["valid"] is False
