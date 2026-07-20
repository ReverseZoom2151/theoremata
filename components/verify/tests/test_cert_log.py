"""Tests for the cert-log exporter + reference checker.

Offline, deterministic, exact.  Drives the REAL certificate producers
(linprog_cert, lp_geometry, log_linarith, geometry_algebraic), exports their
certs to the ``theoremata.cert-log.v1`` format, and confirms the self-contained
reference checker (a) validates a genuine cert and (b) REJECTS every tampered
one (the soundness boundary).
"""
import copy
import json
import sys
from pathlib import Path

import pytest

# Producers live under two component roots; the verify tools under a third.
_ROOT = Path(__file__).resolve().parents[3]
for rel in ("components/verify/python", "components/tools/python",
            "components/prover/python"):
    sys.path.insert(0, str(_ROOT / rel))

from theoremata_tools import cert_log as cl  # noqa: E402  (registry drift guard)
from theoremata_tools.cert_log import (  # noqa: E402
    FORMAT,
    check_cert_log,
    export_asymptotic_cert,
    export_geometry_cert,
    export_lp_cert,
    export_subsumption_cert,
    run,
)
from theoremata_tools import linprog_cert as lc  # noqa: E402
from theoremata_tools import lp_geometry as lg  # noqa: E402
from theoremata_tools import log_linarith as ll  # noqa: E402
from theoremata_tools import geometry_algebraic as ga  # noqa: E402


# --------------------------------------------------------------------------- #
# Fixtures: real producer certificates.
# --------------------------------------------------------------------------- #

def _lp_primal_dual_cert():
    constraints = [
        {"coeffs": {"x": 1, "y": 1}, "sense": "leq", "rhs": 4},
        {"coeffs": {"x": 1}, "sense": "leq", "rhs": 3},
        {"coeffs": {"y": 1}, "sense": "leq", "rhs": 3},
    ]
    objective = {"x": 1, "y": 1}
    cert = lg.primal_dual(objective, constraints, sense="max")
    return cert, constraints, objective


def _farkas_cert():
    constraints = [
        {"coeffs": {"x": 1}, "sense": "geq", "rhs": 1},
        {"coeffs": {"x": 1}, "sense": "leq", "rhs": 0},
    ]
    cert = lc.feasibility(constraints)
    assert cert["feasible"] is False
    return cert, constraints


def _asymptotic_cert():
    ns, _ = ll._build_namespace({"x": "pos_real", "y": "pos_real", "z": "pos_real"})
    P = lambda s: ll.sympify(s, locals=ns)  # noqa: E731
    cert = ll.log_linarith(
        hypotheses=[P("Theta(x) <= Theta(y)"), P("Theta(y) <= Theta(z)")],
        goal=P("Theta(x) <= Theta(z)"),
    )
    assert cert["proved"] is True
    return cert


def _geometry_args():
    points = {"A": [0, 0], "B": ["u1", 0], "C": ["u2", "u3"],
              "D": ["x1", "x2"], "M": ["x3", "x4"]}
    hyps = [
        {"pred": "parallel", "points": ["A", "B", "D", "C"]},
        {"pred": "parallel", "points": ["A", "D", "B", "C"]},
        {"pred": "midpoint", "points": ["M", "A", "C"]},
    ]
    goal = {"pred": "midpoint", "points": ["M", "B", "D"]}
    var_order = ["u1", "u2", "u3", "x1", "x2", "x3", "x4"]
    cert = ga.prove(points, hyps, goal, seed=12345, var_order=var_order)
    assert cert["proved"] is True
    return cert, points, hyps, goal, var_order


def _roundtrip(log):
    """JSON dump/load a log (proves it is plain, transport-neutral JSON)."""
    return json.loads(json.dumps(log))


# --------------------------------------------------------------------------- #
# LP primal/dual: export + validate + tamper rejection.
# --------------------------------------------------------------------------- #

def test_lp_primal_dual_exports_and_validates():
    cert, constraints, objective = _lp_primal_dual_cert()
    log = export_lp_cert(cert, constraints=constraints, objective=objective, sense="max")
    assert log["format"] == FORMAT
    assert log["kind"] == "lp_primal_dual"
    res = check_cert_log(log)
    assert res["valid"] is True, res
    assert res["checked_steps"] == len(log["steps"])


def test_free_text_claim_is_never_reported_as_verified():
    # SOUNDNESS: a valid certificate can carry an arbitrary, unrelated claim; the
    # checker must NOT present that free text as proven.
    cert, constraints, objective = _lp_primal_dual_cert()
    log = export_lp_cert(cert, constraints=constraints, objective=objective, sense="max")
    log["claim"] = "the Riemann hypothesis is true"
    res = check_cert_log(log)
    assert res["valid"] is True  # the LP math still checks
    assert res["claim_verified"] is False  # ...but the claim is NOT verified
    assert "does NOT assert" in res["verified_statement"]
    # A rejected certificate also reports claim_verified False.
    bad = dict(log)
    bad["kind"] = "not_a_real_kind"
    assert check_cert_log(bad)["claim_verified"] is False


def test_lp_primal_dual_roundtrips_through_json():
    cert, constraints, objective = _lp_primal_dual_cert()
    log = export_lp_cert(cert, constraints=constraints, objective=objective, sense="max")
    assert check_cert_log(_roundtrip(log))["valid"] is True


def test_lp_tampered_negative_dual_entry_rejected():
    cert, constraints, objective = _lp_primal_dual_cert()
    log = export_lp_cert(cert, constraints=constraints, objective=objective, sense="max")
    bad = copy.deepcopy(log)
    # Flip a dual entry negative: y >= 0 must fail (soundness).
    for step in bad["steps"]:
        if step["op"] == "dual_vector":
            step["y"][0] = "-1"
    res = check_cert_log(bad)
    assert res["valid"] is False
    assert "negative" in res["reason"] or "y >= 0" in res["reason"]


def test_lp_tampered_objective_breaks_dual_feasibility():
    cert, constraints, objective = _lp_primal_dual_cert()
    log = export_lp_cert(cert, constraints=constraints, objective=objective, sense="max")
    bad = copy.deepcopy(log)
    # Inflate c so that G^T y >= c is violated: dual no longer feasible.
    for step in bad["steps"]:
        if step["op"] == "lp_problem":
            step["c"] = ["99", "99"]
    res = check_cert_log(bad)
    assert res["valid"] is False


def test_lp_tampered_bound_rejected():
    cert, constraints, objective = _lp_primal_dual_cert()
    log = export_lp_cert(cert, constraints=constraints, objective=objective, sense="max")
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "assert_bound":
            step["bound"] = "0"  # true bound is 4
    res = check_cert_log(bad)
    assert res["valid"] is False
    assert "bound" in res["reason"]


# --------------------------------------------------------------------------- #
# LP Farkas infeasibility: export + validate + tamper rejection.
# --------------------------------------------------------------------------- #

def test_farkas_exports_and_validates():
    cert, constraints = _farkas_cert()
    log = export_lp_cert(cert, constraints=constraints)
    assert log["kind"] == "lp_farkas"
    res = check_cert_log(log)
    assert res["valid"] is True, res


def test_farkas_roundtrips_through_json():
    cert, constraints = _farkas_cert()
    log = export_lp_cert(cert, constraints=constraints)
    assert check_cert_log(_roundtrip(log))["valid"] is True


def test_farkas_tampered_negative_multiplier_rejected():
    cert, constraints = _farkas_cert()
    log = export_lp_cert(cert, constraints=constraints)
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "farkas_multipliers":
            step["m"][0] = "-1"
    res = check_cert_log(bad)
    assert res["valid"] is False
    assert "multiplier" in res["reason"]


def test_farkas_tampered_row_breaks_combination():
    cert, constraints = _farkas_cert()
    log = export_lp_cert(cert, constraints=constraints)
    bad = copy.deepcopy(log)
    # Perturb a row coefficient so sum m_k a_k is no longer the zero row.
    for step in bad["steps"]:
        if step["op"] == "farkas_system":
            row = step["rows"][0]
            var = next(iter(row["a"]))
            row["a"][var] = "5"
    res = check_cert_log(bad)
    assert res["valid"] is False
    assert "combination" in res["reason"]


def test_farkas_tampered_rhs_removes_contradiction():
    cert, constraints = _farkas_cert()
    log = export_lp_cert(cert, constraints=constraints)
    bad = copy.deepcopy(log)
    # Make every rhs non-negative so no contradiction can be derived.
    for step in bad["steps"]:
        if step["op"] == "farkas_system":
            for row in step["rows"]:
                row["b"] = "10"
    res = check_cert_log(bad)
    assert res["valid"] is False


# --------------------------------------------------------------------------- #
# Asymptotic: export + validate + tamper rejection.
# --------------------------------------------------------------------------- #

def test_asymptotic_exports_and_validates():
    cert = _asymptotic_cert()
    log = export_asymptotic_cert(cert)
    assert log["kind"] == "asymptotic"
    res = check_cert_log(log)
    assert res["valid"] is True, res


def test_asymptotic_roundtrips_through_json():
    cert = _asymptotic_cert()
    log = export_asymptotic_cert(cert)
    assert check_cert_log(_roundtrip(log))["valid"] is True


def test_asymptotic_tampered_multiplier_rejected():
    cert = _asymptotic_cert()
    log = export_asymptotic_cert(cert)
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "branch_farkas":
            step["m"][0] = "-3"
    assert check_cert_log(bad)["valid"] is False


def test_asymptotic_tampered_row_rejected():
    cert = _asymptotic_cert()
    log = export_asymptotic_cert(cert)
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "branch_farkas":
            row = step["rows"][0]
            var = next(iter(row["a"]))
            row["a"][var] = str(_frac_bump(row["a"][var]))
    assert check_cert_log(bad)["valid"] is False


def _frac_bump(s):
    from fractions import Fraction
    return Fraction(s) + 7


# --------------------------------------------------------------------------- #
# Wu geometry: export + validate + tamper rejection.
# --------------------------------------------------------------------------- #

def test_geometry_exports_and_validates():
    cert, points, hyps, goal, var_order = _geometry_args()
    log = export_geometry_cert(cert, points=points, hypotheses=hyps, goal=goal,
                               var_order=var_order)
    assert log["kind"] == "wu_geometry"
    res = check_cert_log(log)
    assert res["valid"] is True, res


def test_geometry_roundtrips_through_json():
    cert, points, hyps, goal, var_order = _geometry_args()
    log = export_geometry_cert(cert, points=points, hypotheses=hyps, goal=goal,
                               var_order=var_order)
    assert check_cert_log(_roundtrip(log))["valid"] is True


def test_geometry_tampered_goal_yields_nonzero_remainder():
    cert, points, hyps, goal, var_order = _geometry_args()
    log = export_geometry_cert(cert, points=points, hypotheses=hyps, goal=goal,
                               var_order=var_order)
    bad = copy.deepcopy(log)
    # Corrupt a goal-polynomial coefficient: the pseudo-remainder is now nonzero.
    for step in bad["steps"]:
        if step["op"] == "goal_polynomials":
            step["polys"][0]["terms"][0][1] = "999"
    res = check_cert_log(bad)
    assert res["valid"] is False
    assert "NONZERO" in res["reason"] or "remainder" in res["reason"].lower()


def test_geometry_tampered_characteristic_set_rejected():
    cert, points, hyps, goal, var_order = _geometry_args()
    log = export_geometry_cert(cert, points=points, hypotheses=hyps, goal=goal,
                               var_order=var_order)
    bad = copy.deepcopy(log)
    # Corrupt a chain polynomial: reduction no longer cancels the goal.
    for step in bad["steps"]:
        if step["op"] == "characteristic_set":
            step["polys"][0]["terms"][0][1] = "123"
    res = check_cert_log(bad)
    assert res["valid"] is False


# --------------------------------------------------------------------------- #
# Subsumption (optional).
# --------------------------------------------------------------------------- #

def test_subsumption_valid_and_tampered():
    log = export_subsumption_cert(
        subsumer=["P(x)", "Q(x)"],
        subsumed=["P(a)", "Q(a)", "R(a)"],
        substitution={"x": "a"},
    )
    assert check_cert_log(log)["valid"] is True
    bad = copy.deepcopy(log)
    # Remove a literal the subsumer maps onto: subsumption must fail.
    for step in bad["steps"]:
        if step["op"] == "subsumption_relation":
            step["subsumed"] = ["P(a)", "R(a)"]  # Q(a) gone
    assert check_cert_log(bad)["valid"] is False


# --------------------------------------------------------------------------- #
# Format / structural rejection + determinism + run() dispatch.
# --------------------------------------------------------------------------- #

def test_unknown_format_rejected():
    res = check_cert_log({"format": "bogus.v9", "kind": "lp_farkas", "steps": []})
    assert res["valid"] is False


def test_step_from_wrong_kind_rejected():
    cert, constraints = _farkas_cert()
    log = export_lp_cert(cert, constraints=constraints)
    bad = copy.deepcopy(log)
    bad["steps"].append({"op": "assert_pseudo_remainders_zero"})  # wu op in farkas log
    assert check_cert_log(bad)["valid"] is False


def test_no_conclusion_rejected():
    cert, constraints = _farkas_cert()
    log = export_lp_cert(cert, constraints=constraints)
    bad = copy.deepcopy(log)
    bad["steps"] = [s for s in bad["steps"] if s["op"] != "assert_contradiction"]
    assert check_cert_log(bad)["valid"] is False


def test_determinism_export_and_check_are_stable():
    cert, constraints, objective = _lp_primal_dual_cert()
    log1 = export_lp_cert(cert, constraints=constraints, objective=objective, sense="max")
    log2 = export_lp_cert(cert, constraints=constraints, objective=objective, sense="max")
    assert json.dumps(log1, sort_keys=True) == json.dumps(log2, sort_keys=True)
    r1 = check_cert_log(log1)
    r2 = check_cert_log(_roundtrip(log1))
    assert r1["valid"] == r2["valid"] is True
    assert r1["checked_steps"] == r2["checked_steps"]


def test_run_export_then_check_roundtrip():
    cert, constraints = _farkas_cert()
    exported = run({"op": "export", "kind": "lp_farkas", "cert": cert,
                    "constraints": constraints})
    assert "log" in exported
    checked = run({"op": "check", "log": exported["log"]})
    assert checked["valid"] is True


def test_run_check_rejects_tampered():
    cert = _asymptotic_cert()
    log = export_asymptotic_cert(cert)
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "branch_farkas":
            step["m"] = ["-1" for _ in step["m"]]
    assert run({"op": "check", "log": bad})["valid"] is False


def test_run_unknown_op_raises():
    with pytest.raises(ValueError):
        run({"op": "nope"})


# --------------------------------------------------------------------------- #
# Registry drift guard: cert_log's notion of "which kinds exist" versus the
# checker modules that actually ship.  These two drifted apart once (5 kinds
# registered, 20 shipped); these tests keep them married.
# --------------------------------------------------------------------------- #

_TOOLS_DIR = _ROOT / "components/verify/python/theoremata_tools"


def _shipped_checker_modules():
    """Every sibling ``cert_*.py`` checker module (cert_log itself excluded)."""
    import importlib
    mods = {}
    for path in sorted(_TOOLS_DIR.glob("cert_*.py")):
        if path.stem == "cert_log":
            continue
        mods[path.stem] = importlib.import_module(f"theoremata_tools.{path.stem}")
    return mods


def _exported_kinds(mod):
    """Kinds a checker module exports.  Some modules export MORE than one, so
    the guard counts KINDS, never files."""
    kinds = getattr(mod, "KINDS", None)
    if kinds is None:
        single = getattr(mod, "KIND", None)
        assert single is not None, (
            f"{mod.__name__} declares neither KIND nor KINDS; the drift guard "
            "cannot see which certificate kinds it checks"
        )
        kinds = (single,)
    return tuple(str(k) for k in kinds)


def test_every_shipped_checker_kind_is_registered():
    """Adding a cert_*.py without registering its kind must FAIL here."""
    mods = _shipped_checker_modules()
    assert len(mods) >= 14, f"expected the full checker family, saw {sorted(mods)}"
    known = set(cl.KINDS) | set(cl.FOREIGN_KIND_OWNERS)
    missing = {}
    for name, mod in mods.items():
        for kind in _exported_kinds(mod):
            if kind not in known:
                missing.setdefault(name, []).append(kind)
    assert not missing, (
        f"unregistered certificate kinds: {missing}; "
        "add them to cert_log.FOREIGN_KIND_OWNERS"
    )


def test_foreign_owners_point_at_the_module_that_exports_the_kind():
    """The registry must not merely be non-empty; it must be accurate, and it
    must carry no phantom entry for a kind nothing ships."""
    mods = _shipped_checker_modules()
    for name, mod in mods.items():
        for kind in _exported_kinds(mod):
            assert cl.FOREIGN_KIND_OWNERS.get(kind) == mod.__name__, (
                f"{kind!r} (exported by {name}) is missing or misattributed"
            )
    all_shipped = {k for m in mods.values() for k in _exported_kinds(m)}
    assert set(cl.FOREIGN_KIND_OWNERS) == all_shipped


def test_true_kind_count_is_kinds_not_files():
    """cert_sturm ships two kinds, so kinds outnumber modules."""
    from theoremata_tools import cert_sturm
    assert set(_exported_kinds(cert_sturm)) == {"sturm", "poly_minimax"}
    for kind in ("sturm", "poly_minimax"):
        assert cl.FOREIGN_KIND_OWNERS[kind] == "theoremata_tools.cert_sturm"
    mods = _shipped_checker_modules()
    all_shipped = {k for m in mods.values() for k in _exported_kinds(m)}
    assert len(all_shipped) > len(mods)


def test_foreign_and_own_kinds_are_disjoint():
    """No kind may be claimed both by cert_log and by a sibling checker."""
    assert not set(cl.KINDS) & set(cl.FOREIGN_KIND_OWNERS)


def test_kinds_matches_the_dispatch_table():
    """cert_log's own two lists cannot drift from each other either."""
    assert set(cl.KINDS) == set(cl._KIND_OPS)


# --------------------------------------------------------------------------- #
# Unknown kinds: never treated as checked, and a sibling's kind is not refuted.
# --------------------------------------------------------------------------- #

def test_sibling_kind_is_unsupported_not_validated():
    cert, constraints = _farkas_cert()
    log = export_lp_cert(cert, constraints=constraints)
    doc = copy.deepcopy(log)
    doc["kind"] = "sturm"  # a real kind, owned by cert_sturm
    res = check_cert_log(doc)
    assert res["valid"] is False  # unknown here means UNCHECKED, never valid
    assert res["status"] == "unsupported_kind"
    assert res["checker"] == "theoremata_tools.cert_sturm"
    assert res["claim_verified"] is False
    assert "NOTHING was verified" in res["verified_statement"]


def test_every_foreign_kind_fails_closed():
    """Every sibling-owned kind: valid=False, and steps are never executed."""
    cert, constraints = _farkas_cert()
    log = export_lp_cert(cert, constraints=constraints)
    for kind, owner in cl.FOREIGN_KIND_OWNERS.items():
        doc = copy.deepcopy(log)
        doc["kind"] = kind
        res = check_cert_log(doc)
        assert res["valid"] is False, kind
        assert res["status"] == "unsupported_kind", kind
        assert res["checker"] == owner, kind
        assert res["checked_steps"] == 0, kind


def test_nonsense_kind_is_rejected_outright():
    cert, constraints = _farkas_cert()
    log = export_lp_cert(cert, constraints=constraints)
    for kind in ("not_a_real_kind", "", None, 17, ("sturm",)):
        doc = copy.deepcopy(log)
        doc["kind"] = kind
        res = check_cert_log(doc)
        assert res["valid"] is False
        assert res["status"] == "rejected"
        assert "checker" not in res


def test_status_verified_only_ever_accompanies_valid_true():
    cert, constraints = _farkas_cert()
    log = export_lp_cert(cert, constraints=constraints)
    good = check_cert_log(log)
    assert good["valid"] is True and good["status"] == "verified"
    bad = copy.deepcopy(log)
    for step in bad["steps"]:
        if step["op"] == "farkas_multipliers":
            step["m"] = ["-1" for _ in step["m"]]
    res = check_cert_log(bad)
    assert res["valid"] is False and res["status"] == "rejected"


def test_worker_run_surfaces_the_unsupported_status():
    cert, constraints = _farkas_cert()
    log = export_lp_cert(cert, constraints=constraints)
    doc = copy.deepcopy(log)
    doc["kind"] = "pratt_primality"
    res = run({"op": "check", "log": _roundtrip(doc)})
    assert res["valid"] is False
    assert res["status"] == "unsupported_kind"
    assert res["checker"] == "theoremata_tools.cert_pratt"
