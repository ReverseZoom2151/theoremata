"""Tests for the Theta / order-of-magnitude semiring."""
from fractions import Fraction

from sympy import Symbol

from theoremata_tools.order_of_magnitude import (
    OrderMax,
    OrderMul,
    OrderPow,
    Theta,
    Undefined,
    asymp,
    lesssim,
    ll,
)
from theoremata_tools.log_linarith import extract_monomials


def _psym(name):
    return Symbol(name, positive=True)


def test_positive_constants_collapse_to_theta_one():
    assert Theta(5) == Theta(1)
    assert Theta(Fraction(3, 2)) == Theta(1)
    assert Theta(1) == Theta(1)


def test_theta_distributes_over_product():
    x, y = _psym("x"), _psym("y")
    assert isinstance(Theta(x * y), OrderMul)
    # constants vanish inside a product
    assert Theta(3 * x) == Theta(x)


def test_addition_is_max_of_orders():
    x, y = _psym("x"), _psym("y")
    s = Theta(x) + Theta(y)
    assert isinstance(s, OrderMax)
    assert set(s.args) == {Theta(x), Theta(y)}


def test_like_bases_gather_into_powers():
    x = _psym("x")
    prod = Theta(x) * Theta(x)
    assert isinstance(prod, OrderPow)
    base, exp = prod.args
    assert base == Theta(x) and exp == 2
    # x * x / x == x
    assert Theta(x) * Theta(x) / Theta(x) == Theta(x)


def test_non_positive_argument_is_undefined_not_exception():
    z = Symbol("z")  # unknown sign
    assert isinstance(Theta(z), Undefined)
    neg = Symbol("n", negative=True)
    assert isinstance(Theta(neg), Undefined)


def test_power_normalization():
    x = _psym("x")
    p = Theta(x ** 4)
    assert isinstance(p, OrderPow)
    assert p.args == (Theta(x), 4)


def test_sugar_builds_theta_relations():
    x, y = _psym("x"), _psym("y")
    rel = lesssim(x, y)
    assert rel.rel_op in ("<=", ">=")
    assert asymp(x, y).lhs == Theta(x) or asymp(x, y).rhs == Theta(x)
    assert ll(x, y).rel_op in ("<", ">")


def test_extract_monomials_reads_exponents():
    x, y, N = _psym("x"), _psym("y"), _psym("N")
    order = Theta(x * y) / Theta(N ** 4)
    mon = {str(b): e for b, e in extract_monomials(order).items()}
    assert mon[str(Theta(x))] == 1
    assert mon[str(Theta(y))] == 1
    assert mon[str(Theta(N))] == -4
