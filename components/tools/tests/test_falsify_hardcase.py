"""Tests for the exclusion-zone enumerator and worst-case generators.

sympy is required by the enumerator/factoriser; the sympy-dependent tests are
guarded with ``pytest.importorskip("sympy")``.
"""
from __future__ import annotations

import pytest

from theoremata_tools.falsify_hardcase import exclusion_zone, run, worst_cases


# --- #9 exclusion-zone enumeration -----------------------------------------


def test_exclusion_zone_finds_known_hard_case_set():
    """N*m = k**2 - 17 with N = 2**6 has exactly the hard cases
    (k, m) in {(9,1),(23,8),(41,26),(55,47)} for m in [1, 50]."""
    pytest.importorskip("sympy")
    out = exclusion_zone({
        "modulus": {"base": 2, "exp": 6},
        "d": -17,
        "m_range": {"start": 1, "stop": 50},
    })
    got = {(c["k"], c["m"]) for c in out["hard_cases"]}
    assert got == {(9, 1), (23, 8), (41, 26), (55, 47)}
    assert out["count"] == 4
    # Every enumerated case satisfies the equation N*m = k**2 + d exactly.
    N, d = out["modulus"], out["d"]
    for c in out["hard_cases"]:
        assert N * c["m"] == c["k"] ** 2 + d
        assert c["value"] == c["k"] ** 2 + d
    # Residues are the sqrt_mod roots of 17 mod 64.
    assert set(out["residues"]) == {9, 23, 41, 55}


def test_exclusion_zone_bounded_and_complete_outside_window():
    """The next residue-cycle solution (k=73, m=83) is correctly excluded from
    the m in [1,50] window -- the set is finite and complete."""
    pytest.importorskip("sympy")
    out = exclusion_zone({
        "modulus": 64,
        "d": -17,
        "m_range": {"start": 1, "stop": 50},
    })
    assert all(c["k"] <= 55 for c in out["hard_cases"])
    # Widen the window: k=73 (m=83) now appears.
    wide = exclusion_zone({
        "modulus": 64,
        "d": -17,
        "m_range": {"start": 1, "stop": 90},
    })
    assert (73, 83) in {(c["k"], c["m"]) for c in wide["hard_cases"]}
    assert 64 * 83 == 73 ** 2 - 17


def test_exclusion_zone_no_solutions_when_congruence_unsolvable():
    """k**2 ≡ 3 (mod 8) has no solution -> empty hard-case set."""
    pytest.importorskip("sympy")
    out = exclusion_zone({
        "modulus": 8,
        "d": -3,  # k**2 ≡ 3 (mod 8), impossible
        "m_range": {"start": 0, "stop": 100},
    })
    assert out["count"] == 0
    assert out["hard_cases"] == []
    assert out["residues"] == []


def test_exclusion_zone_k_range_form():
    """k_range selects the window directly and yields the same equation facts."""
    pytest.importorskip("sympy")
    out = exclusion_zone({
        "modulus": 64,
        "d": -17,
        "k_range": {"start": 0, "stop": 55},
    })
    assert {(c["k"], c["m"]) for c in out["hard_cases"]} == {
        (9, 1), (23, 8), (41, 26), (55, 47)
    }


def test_exclusion_zone_bound_predicate_flags_only_uncovered():
    """A bound_predicate that holds for k < 30 leaves only k in {41,55} flagged
    as potential counterexamples."""
    pytest.importorskip("sympy")
    out = exclusion_zone({
        "modulus": 64,
        "d": -17,
        "m_range": {"start": 1, "stop": 50},
        "bound_predicate": "k < 30",  # analytic bound covers small k
    })
    flagged = {c["k"] for c in out["potential_counterexamples"]}
    assert flagged == {41, 55}
    # Cases where the predicate holds are not flagged.
    for c in out["hard_cases"]:
        assert c["potential_counterexample"] == (c["k"] >= 30)


def test_exclusion_zone_default_flags_all():
    pytest.importorskip("sympy")
    out = exclusion_zone({
        "modulus": 64, "d": -17, "m_range": {"start": 1, "stop": 50},
    })
    assert all(c["potential_counterexample"] for c in out["hard_cases"])
    assert len(out["potential_counterexamples"]) == out["count"]


def test_exclusion_zone_requires_exactly_one_window():
    pytest.importorskip("sympy")
    with pytest.raises(ValueError):
        exclusion_zone({"modulus": 64, "d": -17})
    with pytest.raises(ValueError):
        exclusion_zone({
            "modulus": 64, "d": -17,
            "m_range": {"start": 1, "stop": 50},
            "k_range": {"start": 0, "stop": 55},
        })


# --- #10 balanced factorization --------------------------------------------


def test_worst_cases_balanced_factorization_p10():
    """2**20 + 3 = 1048579 = 7 * 163 * 919; the most balanced divisor pair is
    (919, 1141) -- the near-square split closest to sqrt(N)."""
    pytest.importorskip("sympy")
    out = worst_cases({
        "kind": "balanced_factorization", "p": 10, "d": 3, "limit": 5,
    })
    assert out["n"] == 2 ** 20 + 3
    top = out["candidates"][0]
    assert (top["m"], top["b"]) == (919, 1141)
    # Every candidate multiplies back to N exactly.
    for c in out["candidates"]:
        assert c["m"] * c["b"] == out["n"]
        assert c["balance"] == c["b"] - c["m"]
    # Ordered by increasing balance (most balanced first).
    balances = [c["balance"] for c in out["candidates"]]
    assert balances == sorted(balances)


def test_worst_cases_balanced_factorization_explicit_n():
    pytest.importorskip("sympy")
    out = worst_cases({"kind": "balanced_factorization", "n": 36, "limit": 10})
    # Divisor pairs of 36 with m <= b, most balanced first: (6,6) leads.
    assert (out["candidates"][0]["m"], out["candidates"][0]["b"]) == (6, 6)
    for c in out["candidates"]:
        assert c["m"] * c["b"] == 36


def test_worst_cases_balanced_factorization_gated_on_bits():
    pytest.importorskip("sympy")
    with pytest.raises(ValueError):
        worst_cases({
            "kind": "balanced_factorization", "n": 2 ** 200 + 1, "max_bits": 80,
        })


# --- #10 near-root generators ----------------------------------------------


def test_worst_cases_near_root_explicit_roots():
    """Immediate integer bracket of sqrt(2) ~ 1.414 is {1, 2}; both within 1."""
    out = worst_cases({"kind": "near_root", "roots": [2 ** 0.5], "step": 1})
    xs = [c["x"] for c in out["candidates"]]
    assert xs == [1, 2]
    for c in out["candidates"]:
        assert c["distance"] < 1  # every candidate hugs the root
        assert abs(c["x"] - c["nearest_root"]) == pytest.approx(c["distance"])


def test_worst_cases_near_root_from_polynomial():
    """Real roots of x**2 - 2 are +-sqrt(2); lattice bracket is {-2,-1,1,2}."""
    pytest.importorskip("sympy")
    out = worst_cases({"kind": "near_root", "poly": "x**2 - 2", "var": "x"})
    xs = sorted(c["x"] for c in out["candidates"])
    assert xs == [-2, -1, 1, 2]
    for c in out["candidates"]:
        assert c["distance"] < 1


def test_worst_cases_near_root_radius_widens():
    out = worst_cases({"kind": "near_root", "roots": [1.5], "step": 1, "radius": 2})
    xs = sorted(c["x"] for c in out["candidates"])
    # floor/ceil are 1/2; radius 2 adds 0 and 3.
    assert xs == [0, 1, 2, 3]


def test_worst_cases_near_root_on_lattice_point():
    """A root exactly on a lattice point still yields a two-sided bracket."""
    out = worst_cases({"kind": "near_root", "roots": [3.0], "step": 1})
    xs = sorted(c["x"] for c in out["candidates"])
    # Two-sided bracket plus the exact root point (distance 0).
    assert xs == [2, 3, 4]
    assert min(c["distance"] for c in out["candidates"]) == 0.0


# --- #10 hensel worst cases (wrapper over exclusion_zone) -------------------


def test_worst_cases_hensel_matches_exclusion_zone():
    pytest.importorskip("sympy")
    req = {"kind": "hensel", "modulus": 64, "d": -17,
           "m_range": {"start": 1, "stop": 50}}
    out = worst_cases(req)
    got = {(c["k"], c["m"]) for c in out["candidates"]}
    assert got == {(9, 1), (23, 8), (41, 26), (55, 47)}
    for c in out["candidates"]:
        assert 64 * c["m"] == c["k"] ** 2 - 17


# --- run() adapter + determinism -------------------------------------------


def test_run_dispatches_ops():
    pytest.importorskip("sympy")
    ez = run({"op": "exclusion_zone", "modulus": 64, "d": -17,
              "m_range": {"start": 1, "stop": 50}})
    assert ez["op"] == "exclusion_zone" and ez["count"] == 4
    wc = run({"op": "worst_cases", "kind": "near_root", "roots": [1.4142]})
    assert wc["op"] == "worst_cases" and wc["kind"] == "near_root"


def test_run_unknown_op_raises():
    with pytest.raises(ValueError):
        run({"op": "nope"})


def test_determinism_repeated_calls_identical():
    """No wall-clock / RNG: repeated calls are byte-identical."""
    pytest.importorskip("sympy")
    spec = {"op": "exclusion_zone", "modulus": {"base": 2, "exp": 6},
            "d": -17, "m_range": {"start": 1, "stop": 90}}
    assert run(dict(spec)) == run(dict(spec))
    fac = {"kind": "balanced_factorization", "p": 10, "d": 3, "limit": 5}
    assert worst_cases(dict(fac)) == worst_cases(dict(fac))
