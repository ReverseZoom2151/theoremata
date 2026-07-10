"""Pocklington **primality certificate** exporter + self-contained **reference
checker**, in the ``theoremata.cert-log.v1`` spirit and a companion to
``cert_pratt.py``.

Where a Pratt certificate demands the *complete* factorization of ``n - 1`` (and
recursively of every prime factor), **Pocklington's theorem** needs only a
*partial* factorization: it suffices to factor a divisor ``F`` of ``n - 1`` that
exceeds ``sqrt(n)`` and leave the cofactor ``R`` untouched.  Concretely, write

    n - 1 = F * R,   gcd(F, R) = 1,   F fully factored,   F > sqrt(n),

then ``n`` is prime iff there is a witness ``a`` with

* ``a^(n-1) ≡ 1 (mod n)``  and
* for every prime ``q | F``:  ``gcd(a^((n-1)/q) - 1, n) = 1``.

(The gcd condition forces the multiplicative order of ``a`` to be divisible by
``q^{v_q(F)}`` in every prime factor of ``n``; with ``F > sqrt(n)`` that pins
down enough of the order structure to force ``n`` prime.)  Each prime factor of
``F`` must itself be certified prime, so the certificate is recursive on the
primes dividing ``F`` only — never on the unfactored cofactor ``R``.  The
recursion bottoms out at the base prime ``2`` (a leaf carrying no witness).

Two halves, mirroring ``cert_pratt.py``:

* **Generator** (``build_pocklington`` / ``export_pocklington_cert``) may use
  :func:`sympy.factorint` to factor *just enough* of ``n - 1`` to exceed
  ``sqrt(n)`` and to search a witness.  It is the *trusted producer*.
* **Checker** (``check``) is the *sound boundary*: it re-verifies with the
  standard library's :func:`pow` and :func:`math.gcd` that ``F * R == n - 1``,
  ``gcd(F, R) == 1``, ``F^2 > n``, the factorization of ``F`` is complete, every
  prime factor of ``F`` is itself certified, ``a^(n-1) ≡ 1 (mod n)`` and each
  gcd condition holds; it recursively validates each child certificate.  It
  imports **nothing** from the generator and uses **no** sympy — pure integer
  arithmetic, deterministic, offline.  A tampered witness, an unfactored or
  incomplete ``F``, an ``F`` that is too small, or a bad gcd condition is
  REJECTED.

Clean-room from the theorem (not transcribed from any BSD-2 source).

Worker dispatch key: ``cert_pocklington`` (see :func:`run`).
"""
from __future__ import annotations

import json
from math import gcd
from typing import Any, Optional

# Same transport-neutral proof-log envelope as cert_log.py / cert_pratt.py.
FORMAT = "theoremata.cert-log.v1"

# This module's proof-log KIND (independent of cert_log.py's KINDS tuple).
KINDS = ("pocklington_primality",)


# --------------------------------------------------------------------------- #
# Generator (trusted producer; may use sympy to partially factor n-1).
# --------------------------------------------------------------------------- #

def _partial_factor(n: int) -> list[tuple[int, int]]:
    """Factor *just enough* of ``n - 1`` to build an ``F > sqrt(n)``.

    Uses :func:`sympy.factorint` on ``n - 1`` (the generator is allowed sympy),
    then greedily accumulates whole prime powers ``q^e`` — largest first, so the
    threshold is reached quickly — until their product ``F`` satisfies
    ``F^2 > n``.  Taking each chosen prime *power in full* guarantees
    ``gcd(F, R) = 1`` for the cofactor ``R = (n-1)/F``.  Returns the chosen
    ``(prime, exponent)`` pairs sorted by prime.
    """
    import sympy  # generator-only import; the checker never needs it

    full = sorted((int(p), int(e)) for p, e in sympy.factorint(n - 1).items())
    chosen: list[tuple[int, int]] = []
    F = 1
    # Largest prime-power first: reach F > sqrt(n) with as few factors as we can.
    for q, e in sorted(full, key=lambda pe: pe[0] ** pe[1], reverse=True):
        chosen.append((q, e))
        F *= q ** e
        if F * F > n:
            break
    return sorted(chosen)


def _find_witness(n: int, primes: list[int]) -> int:
    """Smallest ``a`` in ``[2, n)`` satisfying the Pocklington conditions for ``F``.

    Requires ``a^(n-1) ≡ 1 (mod n)`` and, for each prime ``q | F`` (given in
    ``primes``), ``gcd(a^((n-1)/q) - 1, n) = 1``.  Returns ``0`` if none exists.
    """
    for a in range(2, n):
        if pow(a, n - 1, n) != 1:
            continue
        if all(gcd(pow(a, (n - 1) // q, n) - 1, n) == 1 for q in primes):
            return a
    return 0


def build_pocklington(n: int) -> dict:
    """Build a recursive Pocklington certificate *node* proving ``n`` is prime.

    Raises ``ValueError`` if ``n`` is not a prime that admits a certificate (so a
    composite is rejected here).  A node is::

        {"n": n, "a": a, "F": F, "R": R,
         "factors": [[q, e], ...], "children": [node, ...]}

    where ``factors`` is the complete factorization of ``F`` (a divisor of
    ``n - 1`` with ``F > sqrt(n)``), ``R = (n-1)/F`` is the *unfactored* cofactor,
    and there is a child node per distinct prime ``q > 2`` dividing ``F``; ``q ==
    2`` is a leaf and needs no child.  The base case ``n == 2`` is ``{"n": 2}``.
    """
    n = int(n)
    if n < 2:
        raise ValueError(f"{n} is not prime (n < 2)")
    if n == 2:
        return {"n": 2}
    if n % 2 == 0:
        raise ValueError(f"{n} is even and > 2: not prime")

    factors = _partial_factor(n)
    F = 1
    for q, e in factors:
        F *= q ** e
    R = (n - 1) // F
    if F * R != n - 1 or gcd(F, R) != 1 or F * F <= n:
        raise ValueError(f"{n}: could not form a valid Pocklington F")
    primes = [q for q, _e in factors]
    a = _find_witness(n, primes)
    if a == 0:
        raise ValueError(f"{n} has no Pocklington witness: not prime")

    children = [build_pocklington(q) for q in primes if q != 2]
    return {"n": n, "a": a, "F": F, "R": R,
            "factors": [[q, e] for q, e in factors], "children": children}


def export_pocklington_cert(n: int, *, claim: Optional[str] = None) -> dict:
    """Serialize a Pocklington certificate for ``n`` to a cert-log ``v1`` document.

    Kind ``pocklington_primality``; the single ``pocklington_witness`` step
    carries the recursive witness tree from :func:`build_pocklington`.
    """
    root = build_pocklington(int(n))
    return {
        "format": FORMAT,
        "kind": "pocklington_primality",
        "claim": claim or f"{int(n)} is prime (Pocklington certificate)",
        "steps": [
            {"op": "pocklington_witness", "root": root,
             "note": "n-1 = F*R, gcd(F,R)=1, F>sqrt(n) fully factored; "
                     "witness a with a^(n-1)=1 and gcd(a^((n-1)/q)-1,n)=1 for q|F "
                     "=> n prime; recurse on primes of F only"},
            {"op": "assert_prime"},
        ],
        "meta": {"producer": "cert_pocklington.build_pocklington", "n": int(n)},
    }


# --------------------------------------------------------------------------- #
# REFERENCE CHECKER (pure standard library; independent of the generator).
# --------------------------------------------------------------------------- #

class _Reject(Exception):
    """Raised to reject a certificate with a human-readable reason."""


def _need(cond: bool, reason: str) -> None:
    if not cond:
        raise _Reject(reason)


def _as_int(x: Any, what: str) -> int:
    _need(isinstance(x, int) and not isinstance(x, bool), f"{what} must be an int")
    return x


def _check_node(node: Any, depth: int = 0) -> int:
    """Recursively re-verify a Pocklington node; return the certified prime ``n``.

    Verifies with stdlib :func:`pow` / :func:`math.gcd` that (1) ``F * R == n - 1``
    and ``gcd(F, R) == 1``, (2) ``F^2 > n`` (i.e. ``F > sqrt(n)``), (3) the
    factorization of ``F`` is COMPLETE (its prime powers multiply back to ``F``),
    (4) every prime factor of ``F`` is itself certified (base case ``2`` or a
    recursively-valid child), (5) ``a^(n-1) ≡ 1 (mod n)`` and (6) for each prime
    ``q | F``, ``gcd(a^((n-1)/q) - 1, n) == 1``.  Note only ``F`` is checked for
    completeness — the cofactor ``R`` is deliberately left unfactored.
    """
    _need(depth < 4096, "certificate nesting too deep")
    _need(isinstance(node, dict), "node must be an object")
    n = _as_int(node.get("n"), "node.n")
    _need(n >= 2, f"node.n = {n} is not >= 2")

    if n == 2:  # base case: 2 is prime, a leaf carries no witness
        return 2
    _need(n % 2 == 1, f"n = {n} is even and > 2: not prime")

    # (1) n-1 = F*R with F, R coprime.
    F = _as_int(node.get("F"), "node.F")
    R = _as_int(node.get("R"), "node.R")
    _need(F >= 1 and R >= 1, "F and R must be >= 1")
    _need(F * R == n - 1, f"F*R = {F * R} != n-1 = {n - 1}")
    _need(gcd(F, R) == 1, f"gcd(F, R) = {gcd(F, R)} != 1 (F, R not coprime)")

    # (2) F > sqrt(n): the factored part must dominate. F^2 > n is exact.
    _need(F * F > n, f"F = {F} too small: F^2 = {F * F} <= n = {n} (need F > sqrt(n))")

    # (3) factorization of F is COMPLETE: its prime powers multiply back exactly.
    factors_raw = node.get("factors")
    _need(isinstance(factors_raw, list) and factors_raw,
          "node.factors must be a non-empty list")
    factors: list[tuple[int, int]] = []
    for entry in factors_raw:
        _need(isinstance(entry, (list, tuple)) and len(entry) == 2, "factor must be [q, e]")
        q = _as_int(entry[0], "factor prime")
        e = _as_int(entry[1], "factor exponent")
        _need(q >= 2 and e >= 1, "factor requires q >= 2, e >= 1")
        factors.append((q, e))
    prod = 1
    for q, e in factors:
        prod *= q ** e
    _need(prod == F, f"factorization of F incomplete: prod = {prod} != F = {F}")

    # (4) every distinct prime factor q of F must itself be certified prime.
    children = node.get("children", [])
    _need(isinstance(children, list), "node.children must be a list")
    certified: dict[int, bool] = {}
    child_ns = [_check_node(c, depth + 1) for c in children]  # recurse (validates each)
    for cn in child_ns:
        certified[cn] = True
    distinct_primes = sorted({q for q, _e in factors})
    for q in distinct_primes:
        if q == 2:
            continue  # 2 is prime by the base case; no child required
        _need(certified.get(q, False),
              f"prime factor {q} of F lacks a valid child certificate")

    # (5)/(6) the witness a certifies primality via the Pocklington gcd conditions.
    a = _as_int(node.get("a"), "node.a")
    _need(1 < a < n, f"witness a = {a} must satisfy 1 < a < n")
    _need(pow(a, n - 1, n) == 1, f"Fermat check fails: a^(n-1) != 1 (mod {n})")
    for q in distinct_primes:
        g = gcd(pow(a, (n - 1) // q, n) - 1, n)
        _need(g == 1,
              f"gcd condition fails for prime q = {q}: "
              f"gcd(a^((n-1)/q) - 1, n) = {g} != 1")
    return n


def check(log: Any) -> dict:
    """Independently RE-VERIFY a Pocklington cert-log document.

    Returns ``{valid, reason, checked_nodes, kind, claim, n}``.  Recomputes every
    modular-exponentiation and gcd identity and the recursive partial
    factorization with exact integer arithmetic, stdlib :func:`pow` and
    :func:`math.gcd`; never trusts the generator.  Any malformed, tampered, or
    unsatisfied part yields ``valid=False``.
    """
    try:
        _need(isinstance(log, dict), "log is not a JSON object")
        _need(log.get("format") == FORMAT, f"unknown format: {log.get('format')!r}")
        kind = log.get("kind")
        _need(kind in KINDS, f"unknown kind: {kind!r}")
        _need(isinstance(log.get("claim", ""), str), "claim must be a string")
        steps = log.get("steps")
        _need(isinstance(steps, list) and steps, "steps must be a non-empty list")

        root = None
        concluded = False
        n_certified = None
        for i, step in enumerate(steps):
            _need(isinstance(step, dict), f"step {i} is not an object")
            op = step.get("op")
            if op == "pocklington_witness":
                root = step.get("root")
                _need(root is not None, "pocklington_witness: missing root node")
            elif op == "assert_prime":
                _need(root is not None, "assert_prime before pocklington_witness")
                n_certified = _check_node(root)
                concluded = True
            else:
                raise _Reject(f"step {i}: unknown op {op!r}")

        _need(concluded, "log reached no verified conclusion step")
        return {"valid": True,
                "reason": "Pocklington certificate independently re-verified",
                "checked_nodes": _count_nodes(root), "kind": kind,
                "claim": log.get("claim"), "n": n_certified}
    except _Reject as exc:
        return {"valid": False, "reason": str(exc), "checked_nodes": 0,
                "kind": log.get("kind") if isinstance(log, dict) else None,
                "claim": log.get("claim") if isinstance(log, dict) else None,
                "n": None}


def _count_nodes(node: Any) -> int:
    if not isinstance(node, dict):
        return 0
    return 1 + sum(_count_nodes(c) for c in node.get("children", []) or [])


# --------------------------------------------------------------------------- #
# Worker dispatch.
# --------------------------------------------------------------------------- #

def run(request: dict) -> dict:
    """Worker entrypoint (dispatch key ``cert_pocklington``).

    * ``export`` -> ``{"log": export_pocklington_cert(request["n"])}``.
    * ``check``  -> ``check(request["log"])``.
    """
    op = request.get("op", "check")
    if op == "check":
        return check(request["log"])
    if op == "export":
        return {"log": export_pocklington_cert(int(request["n"]),
                                               claim=request.get("claim"))}
    raise ValueError(f"unknown op: {op!r}")


def main() -> None:
    import sys

    if len(sys.argv) >= 2:
        with open(sys.argv[1], encoding="utf-8") as fh:
            request = json.load(fh)
    else:
        request = json.load(sys.stdin)
    print(json.dumps(run(request), indent=2, default=str))
    raise SystemExit(0)


if __name__ == "__main__":
    main()
