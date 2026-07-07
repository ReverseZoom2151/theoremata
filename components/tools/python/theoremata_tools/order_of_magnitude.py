"""A SymPy-based ``Theta`` / order-of-magnitude semiring.

Ported and adapted (SymPy-only, no Z3) from Terence Tao's ``estimates`` project
(``src/estimates/order_of_magnitude.py``). It builds a formal semiring of
"orders of infinity" *inside* SymPy by subclassing :class:`sympy.Expr` and
intercepting arithmetic:

* positive numeric constants collapse to ``Theta(1)``;
* addition / max become :class:`OrderMax` (``+`` is ``max`` of orders);
* multiplication gathers like bases into powers (:class:`OrderMul`);
* exponentiation scales exponents (:class:`OrderPow`).

Operations are *total*: an undefined result (e.g. ``Theta`` of a non-positive
quantity) returns the :class:`Undefined` marker (``⊥``) rather than raising, so
downstream reasoning never crashes on a partial function.

This is the algebra layer. :mod:`log_linarith` turns relations between these
orders into an exact linear program via :mod:`linprog_cert`.
"""
from __future__ import annotations

from sympy import Add, Basic, Eq, Expr, Max, Min, Mul, Pow, S, Symbol, sympify
from sympy.core.relational import Relational


class Undefined(Expr):
    """Marker for an undefined order-of-magnitude result (``⊥``).

    Still an :class:`Expr` so SymPy operations do not choke; returned (instead
    of raising) whenever an order operation is not defined.
    """

    def __str__(self) -> str:  # pragma: no cover - trivial
        return "⊥"

    __repr__ = __str__


class FormalSub(Expr):
    """A purely formal subtraction marker.

    Orders of magnitude have no subtraction; this stops SymPy's simplifier from
    performing illegal cancellation on ``a - b`` between orders.
    """

    name: str

    def __new__(cls, lhs, rhs):
        obj = Expr.__new__(cls, sympify(lhs), sympify(rhs))
        obj.name = f"FormalSub({lhs!r}, {rhs!r})"
        return obj

    def __str__(self) -> str:  # pragma: no cover - trivial
        return self.name

    __repr__ = __str__


class OrderOfMagnitude(Basic):
    """Base class intercepting arithmetic so ``+`` -> max, ``*`` -> mul, etc.

    Subclasses also subclass a concrete SymPy type (``Expr``/``Symbol``).
    """

    def __add__(self, other):
        return OrderMax(self, other).doit()

    def __radd__(self, other):
        return OrderMax(other, self).doit()

    def __sub__(self, other):
        return FormalSub(self, other)

    def __rsub__(self, other):
        return FormalSub(other, self)

    def __neg__(self):
        return FormalSub(0, self)

    def __mul__(self, other):
        return OrderMul(self, other).doit()

    def __rmul__(self, other):
        return OrderMul(other, self).doit()

    def __truediv__(self, other):
        return OrderMul(self, other ** -1).doit()

    def __rtruediv__(self, other):
        return OrderMul(other, self ** -1).doit()

    def __pow__(self, other):
        return OrderPow(self, other).doit()

    def __rpow__(self, other):
        return Undefined()

    # Comparisons build SymPy Relationals, wrapping the "real" side in Theta.
    def __lt__(self, other):
        return Relational(self, Theta(other), "<")

    def __le__(self, other):
        return Relational(self, Theta(other), "<=")

    def __gt__(self, other):
        return Relational(self, Theta(other), ">")

    def __ge__(self, other):
        return Relational(self, Theta(other), ">=")

    def __abs__(self):
        return self

    def as_real_imag(self, deep=True, **hints):
        return (self, S(0))


class Theta(OrderOfMagnitude, Expr):
    """``Theta(expr)`` = the order of magnitude of a positive expression.

    Construction-time normalization: positive constants collapse to
    ``Theta(1)``; ``Theta`` distributes over ``+``/``max`` (-> :class:`OrderMax`),
    ``*`` (-> :class:`OrderMul`) and rational powers (-> :class:`OrderPow`).
    """

    name: str

    def __new__(cls, expr):
        expr = sympify(expr)

        if isinstance(expr, OrderOfMagnitude):
            return expr

        if expr.is_positive is None:
            # Unknown sign: we cannot decide; treat as undefined rather than
            # silently assuming positivity.
            return Undefined()

        if not expr.is_positive:
            return Undefined()

        if expr.is_number:
            obj = Expr.__new__(cls, S.One)
            obj.name = "Theta(1)"
            return obj

        if isinstance(expr, (Add, Max)) and all(a.is_positive for a in expr.args):
            return OrderMax(*[Theta(a) for a in expr.args]).doit()

        if isinstance(expr, Min) and all(a.is_positive for a in expr.args):
            return OrderMin(*[Theta(a) for a in expr.args]).doit()

        if isinstance(expr, Mul) and all(a.is_positive for a in expr.args):
            return OrderMul(*[Theta(a) for a in expr.args]).doit()

        if isinstance(expr, Pow) and (
            expr.args[0].is_positive
            and expr.args[1].is_number
            and expr.args[1].is_rational
        ):
            return OrderPow(Theta(expr.args[0]), expr.args[1]).doit()

        obj = Expr.__new__(cls, expr)
        obj.name = f"Theta({expr!r})"
        return obj

    def __str__(self) -> str:
        return self.name

    __repr__ = __str__

    def _sympystr(self, printer):  # pragma: no cover - printing glue
        return self.name


class OrderSymbol(OrderOfMagnitude, Symbol):
    """An abstract, positive-by-construction formal order of magnitude."""

    def _eval_abs(self):
        return self


class OrderMax(OrderOfMagnitude, Expr):
    """Maximum (hence also sum) of orders of magnitude."""

    name: str

    def __new__(cls, *args):
        newargs = list(dict.fromkeys([Theta(a) for a in args]))
        if len(newargs) == 0:
            return Undefined()
        if len(newargs) == 1:
            return newargs[0]
        obj = Expr.__new__(cls, *newargs)
        obj.name = "Max(" + ", ".join(str(a) for a in newargs) + ")"
        return obj

    def doit(self, **hints):
        newargs = []
        for arg in self.args:
            if isinstance(arg, OrderMax):
                newargs.extend(arg.args)
            else:
                newargs.append(arg)
        return OrderMax(*newargs)

    def __str__(self) -> str:
        return self.name

    __repr__ = __str__

    def _sympystr(self, printer):  # pragma: no cover
        return self.name


class OrderMin(OrderOfMagnitude, Expr):
    """Minimum of orders of magnitude."""

    name: str

    def __new__(cls, *args):
        newargs = list(dict.fromkeys([Theta(a) for a in args]))
        if len(newargs) == 0:
            return Undefined()
        if len(newargs) == 1:
            return newargs[0]
        obj = Expr.__new__(cls, *newargs)
        obj.name = "Min(" + ", ".join(str(a) for a in newargs) + ")"
        return obj

    def doit(self, **hints):
        newargs = []
        for arg in self.args:
            if isinstance(arg, OrderMin):
                newargs.extend(arg.args)
            else:
                newargs.append(arg)
        return OrderMin(*newargs)

    def __str__(self) -> str:
        return self.name

    __repr__ = __str__

    def _sympystr(self, printer):  # pragma: no cover
        return self.name


class OrderMul(OrderOfMagnitude, Expr):
    """Product of orders of magnitude; gathers like bases into powers."""

    name: str

    def __new__(cls, *args):
        newargs = [Theta(a) for a in args]
        if len(newargs) == 0:
            return Theta(1)
        if len(newargs) == 1:
            return newargs[0]
        obj = Expr.__new__(cls, *newargs)
        obj.name = "*".join(str(a) for a in newargs)
        return obj

    def doit(self, **hints):
        newargs = []
        for arg in self.args:
            if isinstance(arg, OrderMul):
                newargs.extend(arg.args)
            elif arg != Theta(1):
                newargs.append(arg)

        terms: dict[Basic, object] = {}
        for arg in newargs:
            if isinstance(arg, OrderPow):
                base, exp = arg.args
                terms[base] = terms.get(base, S(0)) + exp
            elif arg == Theta(1):
                continue
            else:
                terms[arg] = terms.get(arg, S(0)) + 1

        gathered = []
        for term, exp in terms.items():
            if exp == 0:
                continue
            elif exp == 1:
                gathered.append(term)
            else:
                gathered.append(OrderPow(term, exp))

        if len(gathered) == 0:
            return Theta(1)
        if len(gathered) == 1:
            return gathered[0]
        return OrderMul(*gathered)

    def __str__(self) -> str:
        return self.name

    __repr__ = __str__

    def _sympystr(self, printer):  # pragma: no cover
        return self.name


class OrderPow(OrderOfMagnitude, Expr):
    """A power of an order of magnitude by a rational exponent."""

    name: str

    def __new__(cls, *args):
        if len(args) != 2:
            return Undefined()
        base = S(args[0])
        exp = S(args[1])
        if not exp.is_number:
            return Undefined()
        if not isinstance(base, OrderOfMagnitude):
            return Undefined()

        if exp == S(0):
            return Theta(1)
        if exp == S(1):
            return args[0]
        if base == Theta(1):
            return Theta(1)

        obj = Expr.__new__(cls, args[0], exp)
        obj.name = f"{args[0]}**{exp}"
        return obj

    def doit(self, **hints):
        base, exp = self.args
        if exp == S(0):
            return Theta(1)
        if exp == S(1):
            return base
        if isinstance(base, OrderPow):
            return (base.args[0] ** (exp * base.args[1])).doit()
        if isinstance(base, OrderMul):
            return OrderMul(*[a.doit() ** exp for a in base.args]).doit()
        return self

    def __str__(self) -> str:
        return self.name

    __repr__ = __str__

    def _sympystr(self, printer):  # pragma: no cover
        return self.name


# --- Asymptotic-relation sugar (free functions; not monkeypatched onto Expr) ---

def ll(expr1: Expr, expr2: Expr) -> Relational:
    """``expr1`` is asymptotically much less than ``expr2`` (``o``)."""
    return Theta(abs(expr1)) < Theta(expr2)


def lesssim(expr1: Expr, expr2: Expr) -> Relational:
    """``expr1`` is less than or comparable to ``expr2`` (``O`` / ``≲``)."""
    return Theta(abs(expr1)) <= Theta(expr2)


def gg(expr1: Expr, expr2: Expr) -> Relational:
    """``expr1`` is asymptotically much greater than ``expr2``."""
    return Theta(expr1) > Theta(abs(expr2))


def gtrsim(expr1: Expr, expr2: Expr) -> Relational:
    """``expr1`` is greater than or comparable to ``expr2`` (``≳``)."""
    return Theta(expr1) >= Theta(abs(expr2))


def asymp(expr1: Expr, expr2: Expr) -> Relational:
    """``expr1`` is asymptotically equivalent to ``expr2`` (``Θ``)."""
    return Eq(Theta(expr1), Theta(expr2))
