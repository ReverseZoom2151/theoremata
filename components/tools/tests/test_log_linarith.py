"""Tests for the LogLinarith asymptotic prove/refute engine."""
from theoremata_tools.log_linarith import evaluate


def test_prove_classic_loglinarith_exercise():
    # x <= 2N^2, y < 3kN, k bounded, N positive integer  =>  xy <~ N^4
    req = {
        "op": "prove",
        "vars": {"N": "pos_int", "x": "pos_real", "y": "pos_real", "k": "pos_real"},
        "hypotheses": ["x <= 2*N**2", "y < 3*k*N"],
        "bounded": ["k"],
        "goal": "lesssim(x*y, N**4)",
    }
    res = evaluate(req)
    assert res["status"] == "ok"
    assert res["verdict"] == "proved"
    assert isinstance(res["certificate"], list) and len(res["certificate"]) >= 1
    # every branch certificate is a Farkas certificate
    for branch in res["certificate"]:
        assert branch["certificate"]["type"] == "farkas"


def test_refute_false_asymptotic_goal_returns_counterexample():
    # No relation between x and y, so x <~ y is NOT provable: must be refuted
    # with a concrete asymptotic counterexample.
    req = {
        "op": "prove",
        "vars": {"x": "pos_real", "y": "pos_real"},
        "hypotheses": [],
        "goal": "lesssim(x, y)",
    }
    res = evaluate(req)
    assert res["status"] == "ok"
    assert res["verdict"] == "refuted"
    counter = res["counterexample"]
    assert "assignment" in counter
    # the counterexample should push Theta(x) above Theta(y)
    values = {a["order"]: a["exponent"] for a in counter["assignment"]}
    assert values  # non-empty witness


def test_bounded_makes_variable_vanish():
    # With k bounded, k*N <~ N should hold (k contributes nothing).
    req = {
        "op": "prove",
        "vars": {"N": "pos_int", "k": "pos_real"},
        "hypotheses": [],
        "bounded": ["k"],
        "goal": "lesssim(k*N, N)",
    }
    res = evaluate(req)
    assert res["verdict"] == "proved"


def test_without_bounded_the_same_goal_is_refuted():
    # Same goal but k is now unbounded: k*N <~ N is false.
    req = {
        "op": "prove",
        "vars": {"N": "pos_int", "k": "pos_real"},
        "hypotheses": [],
        "goal": "lesssim(k*N, N)",
    }
    res = evaluate(req)
    assert res["verdict"] == "refuted"


def test_integrality_gap_used():
    # For positive integer N, 1 <~ N holds only via the integrality gap N >~ 1.
    req = {
        "op": "prove",
        "vars": {"N": "pos_int"},
        "hypotheses": [],
        "goal": "lesssim(1, N)",
    }
    res = evaluate(req)
    assert res["verdict"] == "proved"


def test_transitivity_chain():
    # x <~ y, y <~ z  =>  x <~ z
    req = {
        "op": "prove",
        "vars": {"x": "pos_real", "y": "pos_real", "z": "pos_real"},
        "hypotheses": ["lesssim(x, y)", "lesssim(y, z)"],
        "goal": "lesssim(x, z)",
    }
    res = evaluate(req)
    assert res["verdict"] == "proved"


def test_asymp_goal_needs_both_directions():
    # x ~ y from x <~ y and y <~ x
    req = {
        "op": "prove",
        "vars": {"x": "pos_real", "y": "pos_real"},
        "hypotheses": ["lesssim(x, y)", "lesssim(y, x)"],
        "goal": "asymp(x, y)",
    }
    res = evaluate(req)
    assert res["verdict"] == "proved"


def test_max_disjunction_expansion():
    # max(x, y) >~ x  is always true (needs the max-split).
    req = {
        "op": "prove",
        "vars": {"x": "pos_real", "y": "pos_real"},
        "hypotheses": [],
        "goal": "gtrsim(Max(x, y), x)",
    }
    res = evaluate(req)
    assert res["verdict"] == "proved"


def test_normalize_op():
    res = evaluate({
        "op": "normalize",
        "vars": {"x": "pos_real", "y": "pos_real"},
        "expression": "3*x**2*y",
    })
    assert res["status"] == "ok"
    assert res["verdict"] == "normalized"
    mon = res["details"]["monomials"]
    # 3 collapses; x^2 * y
    assert mon[str(__import__("theoremata_tools.order_of_magnitude",
                              fromlist=["Theta"]).Theta(
        __import__("sympy").Symbol("x", positive=True)))] == "2"


def test_feasibility_op_delegates():
    res = evaluate({
        "op": "feasibility",
        "constraints": [
            {"coeffs": {"a": 1}, "sense": "leq", "rhs": 1},
            {"coeffs": {"a": 1}, "sense": "geq", "rhs": 2},
        ],
    })
    assert res["verdict"] == "infeasible"


def test_unknown_op_errors_gracefully():
    res = evaluate({"op": "nonsense"})
    assert res["status"] == "error"
