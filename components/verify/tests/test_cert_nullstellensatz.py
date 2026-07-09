"""Tests for the Nullstellensatz / Gröbner cofactor certificate.

Offline, deterministic, exact.  Exports genuine cofactor certificates
(``Σ q_i p_i = 1`` weak Nullstellensatz and ``g = Σ q_i p_i`` ideal membership)
to the ``theoremata.cert-log.v1`` format, and confirms the self-contained
reference checker (a) validates a genuine cert and (b) REJECTS a tampered
cofactor (the soundness boundary).  Rabinowitsch encoding round-trips.
"""
import copy
import json
import sys
from pathlib import Path

import pytest

pytest.importorskip("sympy")

# Verify tools live under one component root; make the namespace pkg resolve.
_ROOT = Path(__file__).resolve().parents[3]
for rel in ("components/verify/python",):
    p = str(_ROOT / rel)
    if p not in sys.path:
        sys.path.insert(0, p)

import sympy  # noqa: E402
from sympy import symbols, expand  # noqa: E402

from theoremata_tools.cert_nullstellensatz import (  # noqa: E402
    FORMAT,
    KIND,
    check,
    export_nullstellensatz_cert,
    rabinowitsch,
    run,
    _deserialize_poly,
    _serialize_poly,
)


x, y, z = symbols("x y z")


def _roundtrip(log):
    """JSON dump/load (proves the log is plain, transport-neutral JSON)."""
    return json.loads(json.dumps(log))


# --------------------------------------------------------------------------- #
# Weak Nullstellensatz: 1 = Sum q_i p_i  (no common zero).
# --------------------------------------------------------------------------- #

def test_weak_nullstellensatz_exports_and_validates():
    # <x, x-1> has no common zero -> 1 is in the ideal.
    log = export_nullstellensatz_cert([x, x - 1], gens=[x])
    assert log["format"] == FORMAT
    assert log["kind"] == KIND
    res = check(log)
    assert res["valid"] is True, res
    assert res["checked_steps"] == len(log["steps"])


def test_weak_nullstellensatz_multivariate():
    # <x, y, x+y-1>: no common zero (x=y=0 fails x+y-1).
    log = export_nullstellensatz_cert([x, y, x + y - 1], gens=[x, y])
    assert check(log)["valid"] is True


def test_weak_nullstellensatz_roundtrips_through_json():
    log = export_nullstellensatz_cert([x**2 + 1, x], gens=[x])
    assert check(_roundtrip(log))["valid"] is True


# --------------------------------------------------------------------------- #
# Ideal membership: g = Sum q_i p_i.
# --------------------------------------------------------------------------- #

def test_membership_exports_and_validates():
    # g = x*y + x is in <x, y>.
    log = export_nullstellensatz_cert([x, y], gens=[x, y], target=x * y + x)
    assert log["kind"] == KIND
    steps = {s["op"]: s for s in log["steps"]}
    assert steps["target"]["mode"] == "membership"
    res = check(log)
    assert res["valid"] is True, res


def test_membership_nontrivial_roundtrips():
    # x**2 + 1 is in <x-1, x+1> since (x-1)(x+1) = x**2-1, plus 2 = ...; use a
    # clean membership: x**3 in <x**2>.
    log = export_nullstellensatz_cert([x**2], gens=[x], target=x**3)
    assert check(_roundtrip(log))["valid"] is True


# --------------------------------------------------------------------------- #
# Tamper rejection (soundness boundary).
# --------------------------------------------------------------------------- #

def test_tampered_cofactor_rejected():
    log = export_nullstellensatz_cert([x, x - 1], gens=[x])
    bad = copy.deepcopy(log)
    # Corrupt a cofactor coefficient: Sum q_i p_i no longer equals the target.
    for step in bad["steps"]:
        if step["op"] == "cofactors":
            step["polys"][0]["terms"][0][1] = "999"
    res = check(bad)
    assert res["valid"] is False
    assert "cofactor identity" in res["reason"].lower() or "fails" in res["reason"].lower()


def test_tampered_generator_rejected():
    log = export_nullstellensatz_cert([x, y, x + y - 1], gens=[x, y])
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "generators":
            step["polys"][0]["terms"][0][1] = "7"
    assert check(bad)["valid"] is False


def test_tampered_target_rejected():
    log = export_nullstellensatz_cert([x, y], gens=[x, y], target=x * y + x)
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "target":
            step["poly"]["terms"].append([[0, 0], "5"])  # add a spurious constant
    assert check(bad)["valid"] is False


def test_nullstellensatz_mode_requires_unit_target():
    log = export_nullstellensatz_cert([x, x - 1], gens=[x])
    bad = copy.deepcopy(log)
    # Keep the identity satisfiable-looking but flip the target off the unit 1 in
    # a way the mode guard must catch: zero out the cofactors AND the target.
    for step in bad["steps"]:
        if step["op"] == "target":
            step["poly"] = {"terms": []}  # target 0, still nullstellensatz mode
        if step["op"] == "cofactors":
            step["polys"] = [{"terms": []}, {"terms": []}]  # all-zero cofactors
    res = check(bad)
    assert res["valid"] is False  # 0 == Sum 0*p_i holds, but mode demands unit 1
    assert "nullstellensatz" in res["reason"].lower() or "unit" in res["reason"].lower() \
        or "constant 1" in res["reason"].lower()


# --------------------------------------------------------------------------- #
# Rabinowitsch encoding round-trip.
# --------------------------------------------------------------------------- #

def test_rabinowitsch_roundtrips():
    poly = rabinowitsch(x, y, z)
    assert expand(poly - ((x - y) * z + 1)) == 0
    # Serialize / deserialize through the poly-dict form and back.
    gens = [x, y, z]
    d = _serialize_poly(poly, gens)
    back = _deserialize_poly(d, gens).as_expr()
    assert expand(back - poly) == 0
    # The encoding certifies x != y: adjoined to <x-y>, the ideal becomes <1>.
    log = export_nullstellensatz_cert([x - y, poly], gens=[x, y, z])
    assert check(log)["valid"] is True


# --------------------------------------------------------------------------- #
# Determinism + run() dispatch + structural rejection.
# --------------------------------------------------------------------------- #

def test_determinism_export_and_check_are_stable():
    log1 = export_nullstellensatz_cert([x, y, x + y - 1], gens=[x, y])
    log2 = export_nullstellensatz_cert([x, y, x + y - 1], gens=[x, y])
    assert json.dumps(log1, sort_keys=True) == json.dumps(log2, sort_keys=True)
    r1 = check(log1)
    r2 = check(_roundtrip(log1))
    assert r1["valid"] == r2["valid"] is True
    assert r1["checked_steps"] == r2["checked_steps"]


def test_run_export_then_check_roundtrip():
    exported = run({"op": "export", "polys": [x, x - 1], "gens": [x]})
    assert "log" in exported
    checked = run({"op": "check", "log": exported["log"]})
    assert checked["valid"] is True


def test_run_check_rejects_tampered():
    log = export_nullstellensatz_cert([x, y], gens=[x, y], target=x * y + x)
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "cofactors":
            step["polys"][0]["terms"][0][1] = "42"
    assert run({"op": "check", "log": bad})["valid"] is False


def test_run_unknown_op_raises():
    with pytest.raises(ValueError):
        run({"op": "nope"})


def test_unknown_format_and_kind_rejected():
    assert check({"format": "bogus.v9", "kind": KIND, "steps": []})["valid"] is False
    assert check({"format": FORMAT, "kind": "wu_geometry", "steps": []})["valid"] is False


def test_not_in_ideal_raises_on_export():
    # x**2*y is NOT in <x*y-1, x-y>; the producer must refuse to certify.
    with pytest.raises(ValueError):
        export_nullstellensatz_cert([x * y - 1, x - y], gens=[x, y], target=x**2 * y)
