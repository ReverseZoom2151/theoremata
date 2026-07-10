"""Tests for the branch-and-bound nonlinear-inequality certificate (cert_bnb).

Offline, deterministic, fixed-precision.  Builds ``bnb_inequality`` result trees
in the ``theoremata.cert-log.v1`` format and confirms the self-contained reference
checker (a) validates genuine trees, (b) REJECTS unsound/bogus ones, and (c)
reports a genuine counterexample as a refutation — the soundness boundary.

``mpmath`` ships with sympy; guard anyway so the suite skips cleanly if absent.
"""
import copy
import sys
from pathlib import Path

import pytest

pytest.importorskip("mpmath")

# Put the verify component's python/ dir on the path (namespace package).
_ROOT = Path(__file__).resolve().parents[3]
for rel in ("components/verify/python",):
    p = str(_ROOT / rel)
    if p not in sys.path:
        sys.path.insert(0, p)

from theoremata_tools.cert_bnb import (  # noqa: E402
    FORMAT,
    KINDS,
    check,
    export_bnb_cert,
    leaf_false,
    leaf_pass,
    node_mono,
    node_split,
    run,
)


# --------------------------------------------------------------------------- #
# Expression-AST shorthands.
# --------------------------------------------------------------------------- #

def _v(name="x"):
    return ["var", name]


def _c(x):
    return ["const", str(x)]


def _pow(e, k):
    return ["^", e, str(k)]


# f(x) = x^2 + 1  (strictly positive everywhere)
X2_PLUS_1 = ["+", _pow(_v(), 2), _c(1)]
# f(x) = x^2 - x + 1  (min 3/4; interval eval of the whole box dips < 0)
X2_MINUS_X_PLUS_1 = ["+", _pow(_v(), 2), ["neg", _v()], _c(1)]
# f(x) = x^2 - 1  (negative near 0)
X2_MINUS_1 = ["-", _pow(_v(), 2), _c(1)]


# --------------------------------------------------------------------------- #
# Valid certificates check True.
# --------------------------------------------------------------------------- #

def test_single_pass_leaf_valid():
    """x^2 + 1 > 0 on [-2, 2] closes as a single pass leaf."""
    tree = leaf_pass([("-2", "2")])
    log = export_bnb_cert(X2_PLUS_1, ["x"], [("-2", "2")], tree)
    assert log["format"] == FORMAT and log["kind"] == "bnb_inequality"
    assert log["kind"] in KINDS
    res = check(log)
    assert res["valid"] is True, res["reason"]
    assert res["checked_nodes"] == 1


def test_needs_one_split_valid():
    """x^2 - x + 1 >= 0 on [-2, 2]: the whole-box enclosure fails, one split fixes it."""
    # A single pass leaf would be rejected (enclosure lower bound < 0)...
    bad = export_bnb_cert(X2_MINUS_X_PLUS_1, ["x"], [("-2", "2")],
                          leaf_pass([("-2", "2")]))
    assert check(bad)["valid"] is False
    # ...but a split into three flush sub-boxes discharges each piece.
    tree = node_split([("-2", "2")], 0, [
        leaf_pass([("-2", "0")]),
        leaf_pass([("0", "1")]),
        leaf_pass([("1", "2")]),
    ])
    log = export_bnb_cert(X2_MINUS_X_PLUS_1, ["x"], [("-2", "2")], tree)
    res = check(log)
    assert res["valid"] is True, res["reason"]
    assert res["checked_nodes"] == 4  # split + 3 leaves


def test_monotone_reduction_valid():
    """exp(x) - 1 >= 0 on [0, 1] via monotone (inc) reduction to the face x = 0."""
    f = ["-", ["exp", _v()], _c(1)]
    tree = node_mono([("0", "1")], 0, "inc", leaf_pass([("0", "0")]))
    log = export_bnb_cert(f, ["x"], [("0", "1")], tree)
    res = check(log)
    assert res["valid"] is True, res["reason"]


def test_two_variable_split_valid():
    """x^2 + y^2 >= 0 on [-1,1]x[-1,1] as a single pass leaf (multivariate)."""
    f = ["+", _pow(_v("x"), 2), _pow(_v("y"), 2)]
    box = [("-1", "1"), ("-1", "1")]
    log = export_bnb_cert(f, ["x", "y"], box, leaf_pass(box))
    res = check(log)
    assert res["valid"] is True, res["reason"]


# --------------------------------------------------------------------------- #
# Unsound / bogus certificates are REJECTED (the soundness boundary).
# --------------------------------------------------------------------------- #

def test_reject_unsound_pass_leaf():
    """A pass leaf over a box where f actually dips below 0 is rejected."""
    tree = leaf_pass([("-2", "2")])   # x^2 - 1 = -1 at x=0
    log = export_bnb_cert(X2_MINUS_1, ["x"], [("-2", "2")], tree)
    res = check(log)
    assert res["valid"] is False
    assert "unsound pass" in res["reason"]
    assert not res.get("refuted")


def test_reject_split_not_covering_parent():
    """A split whose children leave a gap (do not reach the parent bound) is rejected."""
    tree = node_split([("-2", "2")], 0, [
        leaf_pass([("-2", "0")]),
        leaf_pass([("0", "1")]),      # missing [1, 2]
    ])
    log = export_bnb_cert(X2_PLUS_1, ["x"], [("-2", "2")], tree)
    res = check(log)
    assert res["valid"] is False
    assert "parent" in res["reason"] or "cover" in res["reason"]


def test_reject_split_with_overlap():
    """A split with overlapping children (not an exact tiling) is rejected."""
    tree = node_split([("-2", "2")], 0, [
        leaf_pass([("-2", "1")]),
        leaf_pass([("0", "2")]),      # overlaps [-2,1] on [0,1]
    ])
    log = export_bnb_cert(X2_PLUS_1, ["x"], [("-2", "2")], tree)
    res = check(log)
    assert res["valid"] is False
    assert "gap or overlap" in res["reason"]


def test_reject_wrong_monotone_face():
    """A mono node whose child is the wrong face is rejected."""
    f = ["-", ["exp", _v()], _c(1)]
    # inc => minimum on the LOWER face x=0; pointing the child at x=1 is wrong.
    tree = node_mono([("0", "1")], 0, "inc", leaf_pass([("1", "1")]))
    log = export_bnb_cert(f, ["x"], [("0", "1")], tree)
    res = check(log)
    assert res["valid"] is False
    assert "face" in res["reason"]


def test_reject_bogus_counterexample():
    """A false leaf whose witness does NOT give f < 0 is rejected (not a refutation)."""
    tree = leaf_false([("-2", "2")], ["2"])   # x^2 - 1 = 3 at x=2, not < 0
    log = export_bnb_cert(X2_MINUS_1, ["x"], [("-2", "2")], tree)
    res = check(log)
    assert res["valid"] is False
    assert not res.get("refuted")
    assert "witness" in res["reason"] or "not < 0" in res["reason"]


# --------------------------------------------------------------------------- #
# A genuine counterexample refutes the inequality.
# --------------------------------------------------------------------------- #

def test_false_leaf_refutes():
    """x^2 - 1 >= 0 is FALSE on [-2, 2]: x=0 gives f=-1 < 0, a real counterexample."""
    tree = leaf_false([("-2", "2")], ["0"])
    log = export_bnb_cert(X2_MINUS_1, ["x"], [("-2", "2")], tree)
    res = check(log)
    assert res["valid"] is False
    assert res.get("refuted") is True
    assert res["witness"] == {"x": "0"}


# --------------------------------------------------------------------------- #
# Structural rejections.
# --------------------------------------------------------------------------- #

def test_reject_bad_format_and_kind():
    log = export_bnb_cert(X2_PLUS_1, ["x"], [("-2", "2")], leaf_pass([("-2", "2")]))
    bad = copy.deepcopy(log); bad["format"] = "nope"
    assert check(bad)["valid"] is False
    bad2 = copy.deepcopy(log); bad2["kind"] = "taylor_model"
    assert check(bad2)["valid"] is False


def test_reject_root_box_mismatch():
    """A tree whose root box is not the declared domain is rejected."""
    log = export_bnb_cert(X2_PLUS_1, ["x"], [("-2", "2")], leaf_pass([("-2", "2")]))
    tampered = copy.deepcopy(log)
    tampered["steps"][1]["root"]["box"] = [["-1", "1"]]
    res = check(tampered)
    assert res["valid"] is False
    assert "domain" in res["reason"]


# --------------------------------------------------------------------------- #
# Determinism + worker dispatch.
# --------------------------------------------------------------------------- #

def test_determinism():
    """Re-checking the same log yields byte-identical results (fixed precision)."""
    log = export_bnb_cert(X2_MINUS_X_PLUS_1, ["x"], [("-2", "2")], node_split(
        [("-2", "2")], 0,
        [leaf_pass([("-2", "0")]), leaf_pass([("0", "1")]), leaf_pass([("1", "2")])],
    ))
    r1 = check(log)
    r2 = check(log)
    assert r1 == r2 and r1["valid"] is True


def test_worker_run_export_then_check():
    out = run({
        "op": "export",
        "expr": X2_PLUS_1,
        "vars": ["x"],
        "domain": [("-2", "2")],
        "tree": leaf_pass([("-2", "2")]),
    })
    assert "log" in out
    res = run({"op": "check", "log": out["log"]})
    assert res["valid"] is True, res["reason"]
