"""Pratt (Lucas--Lehmer/Pocklington-style) **primality certificate** exporter +
self-contained **reference checker**, in the ``theoremata.cert-log.v1`` spirit.

A *Pratt certificate* proves that ``n`` is prime by exhibiting a witness ``a``
(a primitive root mod ``n``) together with the complete prime factorization of
``n - 1``, and *recursively* a Pratt certificate for every prime ``q`` dividing
``n - 1``.  Concretely ``n`` is prime iff there is an ``a`` with

* ``a^(n-1) ≡ 1 (mod n)``  and
* for every prime ``q | (n - 1)``:  ``a^((n-1)/q) ≢ 1 (mod n)``,

which forces the multiplicative order of ``a`` to be exactly ``n - 1``; a group
of that order mod ``n`` means ``n`` is prime.  The recursion bottoms out at the
base prime ``2`` (a leaf carrying no witness).

Two halves, mirroring ``cert_log.py``:

* **Generator** (``build_pratt`` / ``export_pratt_cert``) may use
  :func:`sympy.factorint` to factor ``n - 1`` and search a witness.  It is the
  *trusted producer*.
* **Checker** (``check``) is the *sound boundary*: it re-verifies EVERY modular
  exponentiation identity with the standard library's :func:`pow` (fast modular
  exponentiation), recursively validates each child certificate, and confirms
  the factorization of ``n - 1`` is complete (the prime powers multiply back to
  ``n - 1``).  It imports **nothing** from the generator and uses **no** sympy —
  pure integer arithmetic, deterministic, offline.  A tampered witness, an
  incomplete factorization, or a bad child certificate is REJECTED.

Worker dispatch key: ``cert_pratt`` (see :func:`run`).
"""
from __future__ import annotations

import json
from math import gcd
from typing import Any, Optional

# Same transport-neutral proof-log envelope as cert_log.py.
FORMAT = "theoremata.cert-log.v1"

# This module's proof-log KIND (independent of cert_log.py's KINDS tuple).
KINDS = ("pratt_primality",)


# --------------------------------------------------------------------------- #
# Generator (trusted producer; may use sympy to factor n-1).
# --------------------------------------------------------------------------- #

def _factor(n_minus_1: int) -> list[tuple[int, int]]:
    """Factor ``n - 1`` into sorted ``(prime, exponent)`` pairs via sympy."""
    import sympy  # generator-only import; the checker never needs it

    return sorted((int(p), int(e)) for p, e in sympy.factorint(n_minus_1).items())


def _find_witness(n: int, primes: list[int]) -> int:
    """Smallest ``a`` in ``[2, n)`` whose order mod ``n`` is exactly ``n - 1``.

    Returns ``0`` if none exists (``n`` is then not prime — no Pratt witness).
    """
    for a in range(2, n):
        if pow(a, n - 1, n) != 1:
            continue
        if all(pow(a, (n - 1) // q, n) != 1 for q in primes):
            return a
    return 0


def build_pratt(n: int) -> dict:
    """Build a recursive Pratt certificate *node* proving ``n`` is prime.

    Raises ``ValueError`` if ``n`` is not a prime that admits a witness (so a
    composite is rejected here).  A node is::

        {"n": n, "a": a, "factors": [[q, e], ...], "children": [node, ...]}

    with a child node per distinct prime ``q > 2`` dividing ``n - 1``; ``q == 2``
    is a leaf and needs no child.  The base case ``n == 2`` is ``{"n": 2}``.
    """
    n = int(n)
    if n < 2:
        raise ValueError(f"{n} is not prime (n < 2)")
    if n == 2:
        return {"n": 2}
    if n % 2 == 0:
        raise ValueError(f"{n} is even and > 2: not prime")

    factors = _factor(n - 1)
    primes = [q for q, _e in factors]
    a = _find_witness(n, primes)
    if a == 0:
        raise ValueError(f"{n} has no Pratt witness: not prime")

    children = [build_pratt(q) for q in primes if q != 2]
    return {"n": n, "a": a, "factors": [[q, e] for q, e in factors],
            "children": children}


def export_pratt_cert(n: int, *, claim: Optional[str] = None) -> dict:
    """Serialize a Pratt certificate for ``n`` to a cert-log ``v1`` document.

    Kind ``pratt_primality``; the single ``pratt_witness`` step carries the
    recursive witness tree from :func:`build_pratt`.
    """
    root = build_pratt(int(n))
    return {
        "format": FORMAT,
        "kind": "pratt_primality",
        "claim": claim or f"{int(n)} is prime (Pratt certificate)",
        "steps": [
            {"op": "pratt_witness", "root": root,
             "note": "order of a mod n is n-1 => n prime; recurse on primes of n-1"},
            {"op": "assert_prime"},
        ],
        "meta": {"producer": "cert_pratt.build_pratt", "n": int(n)},
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
    """Recursively re-verify a Pratt node; return the certified prime ``n``.

    Verifies with stdlib :func:`pow` that (1) the factorization of ``n - 1`` is
    COMPLETE, (2) every prime factor is itself certified (base case ``2`` or a
    recursively-valid child), (3) ``a^(n-1) ≡ 1 (mod n)`` and (4) for each prime
    ``q | (n - 1)``, ``a^((n-1)/q) ≢ 1 (mod n)``.
    """
    _need(depth < 4096, "certificate nesting too deep")
    _need(isinstance(node, dict), "node must be an object")
    n = _as_int(node.get("n"), "node.n")
    _need(n >= 2, f"node.n = {n} is not >= 2")

    if n == 2:  # base case: 2 is prime, a leaf carries no witness
        return 2
    _need(n % 2 == 1, f"n = {n} is even and > 2: not prime")

    factors_raw = node.get("factors")
    _need(isinstance(factors_raw, list) and factors_raw, "node.factors must be a non-empty list")
    factors: list[tuple[int, int]] = []
    for entry in factors_raw:
        _need(isinstance(entry, (list, tuple)) and len(entry) == 2, "factor must be [q, e]")
        q = _as_int(entry[0], "factor prime")
        e = _as_int(entry[1], "factor exponent")
        _need(q >= 2 and e >= 1, "factor requires q >= 2, e >= 1")
        factors.append((q, e))

    # (1) factorization of n-1 is COMPLETE: prime powers multiply back exactly.
    prod = 1
    for q, e in factors:
        prod *= q ** e
    _need(prod == n - 1, f"factorization incomplete: prod = {prod} != n-1 = {n - 1}")

    # (2) every distinct prime factor q must itself be certified prime.
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
              f"prime factor {q} of n-1 lacks a valid child certificate")

    # (3)/(4) the witness a certifies order(a) mod n == n - 1.
    a = _as_int(node.get("a"), "node.a")
    _need(1 < a < n, f"witness a = {a} must satisfy 1 < a < n")
    _need(gcd(a, n) == 1, f"witness a = {a} not coprime to n")
    _need(pow(a, n - 1, n) == 1, f"Fermat check fails: a^(n-1) != 1 (mod {n})")
    for q in distinct_primes:
        _need(pow(a, (n - 1) // q, n) != 1,
              f"order check fails: a^((n-1)/{q}) == 1 (mod {n}); order < n-1")
    return n


def check(log: Any) -> dict:
    """Independently RE-VERIFY a Pratt cert-log document.

    Returns ``{valid, reason, checked_nodes, kind, claim, n}``.  Recomputes every
    modular-exponentiation identity and the recursive factorization with exact
    integer arithmetic and stdlib :func:`pow`; never trusts the generator.  Any
    malformed, tampered, or unsatisfied part yields ``valid=False``.
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
            if op == "pratt_witness":
                root = step.get("root")
                _need(root is not None, "pratt_witness: missing root node")
            elif op == "assert_prime":
                _need(root is not None, "assert_prime before pratt_witness")
                n_certified = _check_node(root)
                concluded = True
            else:
                raise _Reject(f"step {i}: unknown op {op!r}")

        _need(concluded, "log reached no verified conclusion step")
        return {"valid": True, "reason": "Pratt certificate independently re-verified",
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
    """Worker entrypoint (dispatch key ``cert_pratt``).

    * ``export`` -> ``{"log": export_pratt_cert(request["n"])}``.
    * ``check``  -> ``check(request["log"])``.
    """
    op = request.get("op", "check")
    if op == "check":
        return check(request["log"])
    if op == "export":
        return {"log": export_pratt_cert(int(request["n"]), claim=request.get("claim"))}
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
