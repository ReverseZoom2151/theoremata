"""Herbrand certificate: **validity / unsatisfiability of a first-order formula
via a finite set of ground instances**, with a self-contained REFERENCE CHECKER.

The math (clean-room from Harrison's *Handbook of Practical Logic and Automated
Reasoning*, ch. on Herbrand's theorem)
--------------------------------------------------------------------------------
Herbrand's theorem reduces first-order (un)satisfiability to *propositional*
(un)satisfiability over ground instances:

* A universal formula ``forall x. M(x)`` is **unsatisfiable** iff some FINITE
  conjunction of ground instances ``M(t_1) and ... and M(t_k)`` is
  *propositionally* unsatisfiable (a contradiction over the ground atoms).
* Dually, a formula is **valid** iff its negation is unsatisfiable; for an
  existential body ``exists x. M(x)`` this means some finite DISJUNCTION of
  ground instances ``M(t_1) or ... or M(t_k)`` is a propositional TAUTOLOGY.

The expensive, heuristic part is *finding* the instances (an untrusted search:
Herbrand-universe enumeration, unification, resolution, ...).  The
**trust-critical** part is tiny and decidable: once the ground instances are
fixed, distinct ground atoms become propositional variables and the combined
formula's validity/unsatisfiability is settled by an EXACT, finite,
deterministic method (here an exhaustive truth-table over the ground atoms).

Soundness boundary
------------------
:func:`check` is the sound boundary.  It never trusts the producer's claim:  it
re-applies every ground substitution, confirms every instance is genuinely
ground, identifies distinct ground atoms as propositional variables, and DECIDES
the propositional combination itself.  An *insufficient* instance set (one whose
conjunction is still propositionally satisfiable, or whose disjunction is not a
tautology) is REJECTED, as is any tampered substitution, malformed AST, or wrong
``claim``.  Everything is pure standard library, offline and deterministic.

Scope / honest note
-------------------
Only the **ground / quantifier-free-after-instantiation** fragment is in scope:
the certificate carries the quantifier-free *matrix* plus explicit ground
substitutions, and the kernel decides the resulting propositional formula.
FINDING an adequate instance set (and any Skolemization/prenexing that precedes
it) is the untrusted search seam and is deliberately outside the kernel.

Worker dispatch key: ``cert_herbrand`` (see :func:`run`).
"""
from __future__ import annotations

import itertools
import json
from typing import Any, Optional

FORMAT = "theoremata.cert-log.v1"
KINDS = ("herbrand",)

# Truth-table decision is exact but exponential in the number of DISTINCT ground
# atoms; refuse (reject) rather than blow up past this bound so the kernel always
# terminates deterministically.  2**24 rows is the hard ceiling.
_MAX_ATOMS = 24


# --------------------------------------------------------------------------- #
# Term / formula builders (a small first-order AST, mirroring the reason-layer
# Term model: a term is a Var or an App(symbol, args); a constant is App(_, [])).
# The AST is plain JSON so a certificate round-trips through ``json``.
# --------------------------------------------------------------------------- #

def V(name: str) -> dict:
    """A first-order variable term ``name``."""
    return {"var": str(name)}


def T(fn: str, *args: dict) -> dict:
    """A function application ``fn(args...)``; ``T("a")`` is the constant ``a``."""
    return {"fn": str(fn), "args": list(args)}


Const = T  # a nullary application is a constant


def Atom(pred: str, *args: dict) -> dict:
    """An atomic formula ``pred(args...)`` (a predicate applied to terms)."""
    return {"atom": {"pred": str(pred), "args": list(args)}}


def Not(f: dict) -> dict:
    return {"not": f}


def And(*fs: dict) -> dict:
    return {"and": list(fs)}


def Or(*fs: dict) -> dict:
    return {"or": list(fs)}


def Imp(a: dict, b: dict) -> dict:
    return {"imp": [a, b]}


# --------------------------------------------------------------------------- #
# Rejection plumbing (same shape as the other cert modules).
# --------------------------------------------------------------------------- #

class _Reject(Exception):
    """Raised to reject a certificate with a human-readable reason."""


def _need(cond: bool, reason: str) -> None:
    if not cond:
        raise _Reject(reason)


# --------------------------------------------------------------------------- #
# Defensive validation / normalization of the untrusted AST.
# --------------------------------------------------------------------------- #

def _coerce_term(t: Any) -> dict:
    _need(isinstance(t, dict), "term must be an object")
    if "var" in t:
        _need(len(t) == 1, "variable term has extra keys")
        name = t["var"]
        _need(isinstance(name, str) and name != "", "variable name must be a non-empty string")
        return {"var": name}
    _need("fn" in t and "args" in t, "term must be a variable or an application")
    _need(len(t) == 2, "application term has extra keys")
    fn = t["fn"]
    _need(isinstance(fn, str) and fn != "", "function symbol must be a non-empty string")
    args = t["args"]
    _need(isinstance(args, list), "application args must be a list")
    return {"fn": fn, "args": [_coerce_term(a) for a in args]}


def _coerce_formula(f: Any) -> dict:
    _need(isinstance(f, dict), "formula must be an object")
    _need(len(f) == 1, "formula must carry exactly one connective/atom tag")
    (tag, body), = f.items()
    if tag == "atom":
        _need(isinstance(body, dict) and set(body) == {"pred", "args"},
              "atom must have 'pred' and 'args'")
        pred = body["pred"]
        _need(isinstance(pred, str) and pred != "", "predicate must be a non-empty string")
        _need(isinstance(body["args"], list), "atom args must be a list")
        return {"atom": {"pred": pred, "args": [_coerce_term(a) for a in body["args"]]}}
    if tag == "not":
        return {"not": _coerce_formula(body)}
    if tag in ("and", "or"):
        _need(isinstance(body, list) and body, f"'{tag}' needs a non-empty list of formulas")
        return {tag: [_coerce_formula(g) for g in body]}
    if tag == "imp":
        _need(isinstance(body, list) and len(body) == 2, "'imp' needs exactly two formulas")
        return {"imp": [_coerce_formula(body[0]), _coerce_formula(body[1])]}
    raise _Reject(f"unknown formula tag {tag!r}")


# --------------------------------------------------------------------------- #
# Free variables / substitution / groundness.
# --------------------------------------------------------------------------- #

def _term_vars(t: dict, out: set) -> None:
    if "var" in t:
        out.add(t["var"])
    else:
        for a in t["args"]:
            _term_vars(a, out)


def _formula_vars(f: dict, out: set) -> None:
    (tag, body), = f.items()
    if tag == "atom":
        for a in body["args"]:
            _term_vars(a, out)
    elif tag == "not":
        _formula_vars(body, out)
    elif tag in ("and", "or"):
        for g in body:
            _formula_vars(g, out)
    else:  # imp
        _formula_vars(body[0], out)
        _formula_vars(body[1], out)


def _subst_term(t: dict, sigma: dict) -> dict:
    if "var" in t:
        # Substitution values are ground terms; a variable not covered is left in
        # place and will fail the downstream groundness check.
        return sigma.get(t["var"], t)
    return {"fn": t["fn"], "args": [_subst_term(a, sigma) for a in t["args"]]}


def _subst_formula(f: dict, sigma: dict) -> dict:
    (tag, body), = f.items()
    if tag == "atom":
        return {"atom": {"pred": body["pred"],
                         "args": [_subst_term(a, sigma) for a in body["args"]]}}
    if tag == "not":
        return {"not": _subst_formula(body, sigma)}
    if tag in ("and", "or"):
        return {tag: [_subst_formula(g, sigma) for g in body]}
    return {"imp": [_subst_formula(body[0], sigma), _subst_formula(body[1], sigma)]}


def _term_is_ground(t: dict) -> bool:
    if "var" in t:
        return False
    return all(_term_is_ground(a) for a in t["args"])


def _formula_is_ground(f: dict) -> bool:
    (tag, body), = f.items()
    if tag == "atom":
        return all(_term_is_ground(a) for a in body["args"])
    if tag == "not":
        return _formula_is_ground(body)
    if tag in ("and", "or"):
        return all(_formula_is_ground(g) for g in body)
    return _formula_is_ground(body[0]) and _formula_is_ground(body[1])


# --------------------------------------------------------------------------- #
# Propositional identification + exact truth-table decision.
# --------------------------------------------------------------------------- #

def _term_key(t: dict) -> str:
    if "var" in t:
        return "?" + t["var"]
    return t["fn"] + "(" + ",".join(_term_key(a) for a in t["args"]) + ")"


def _atom_key(atom: dict) -> str:
    """Canonical key for a ground atom: distinct ground atoms -> distinct props."""
    return atom["pred"] + "[" + ",".join(_term_key(a) for a in atom["args"]) + "]"


def _collect_atoms(f: dict, into: set) -> None:
    (tag, body), = f.items()
    if tag == "atom":
        into.add(_atom_key(body))
    elif tag == "not":
        _collect_atoms(body, into)
    elif tag in ("and", "or"):
        for g in body:
            _collect_atoms(g, into)
    else:
        _collect_atoms(body[0], into)
        _collect_atoms(body[1], into)


def _eval(f: dict, assign: dict) -> bool:
    (tag, body), = f.items()
    if tag == "atom":
        return assign[_atom_key(body)]
    if tag == "not":
        return not _eval(body, assign)
    if tag == "and":
        return all(_eval(g, assign) for g in body)
    if tag == "or":
        return any(_eval(g, assign) for g in body)
    return (not _eval(body[0], assign)) or _eval(body[1], assign)  # imp


def _decide(f: dict, want: str) -> None:
    """Exactly DECIDE a ground propositional formula by exhaustive truth table.

    ``want == "unsat"``  -> reject unless ``f`` is false under EVERY assignment.
    ``want == "taut"``   -> reject unless ``f`` is true under EVERY assignment.
    """
    atoms: set = set()
    _collect_atoms(f, atoms)
    order = sorted(atoms)
    _need(len(order) <= _MAX_ATOMS,
          f"too many distinct ground atoms ({len(order)}) to decide by truth table")
    for bits in itertools.product((False, True), repeat=len(order)):
        assign = dict(zip(order, bits))
        val = _eval(f, assign)
        if want == "unsat" and val:
            raise _Reject("ground instances are propositionally SATISFIABLE "
                          f"(model {_model_str(order, bits)}); not a Herbrand refutation")
        if want == "taut" and not val:
            raise _Reject("ground-instance disjunction is NOT a propositional "
                          f"tautology (falsified by {_model_str(order, bits)}); "
                          "instance set insufficient for validity")


def _model_str(order: list, bits: tuple) -> str:
    return "{" + ", ".join(f"{a}={'T' if b else 'F'}"
                           for a, b in zip(order, bits)) + "}"


# --------------------------------------------------------------------------- #
# Exporter.
# --------------------------------------------------------------------------- #

def export_herbrand_cert(matrix: dict, substitutions: list, claim: str, *,
                         claim_text: Optional[str] = None) -> dict:
    """Serialize a Herbrand certificate to a cert-log document.

    ``matrix`` is the quantifier-free (prenex) body as the small AST above (its
    free variables are the quantified variables).  ``substitutions`` is the list
    of ground substitutions (each a ``{var_name: ground_term}`` dict) the
    untrusted search supplied.  ``claim`` is ``"unsat"`` (the universal closure
    of ``matrix`` is unsatisfiable — the conjunction of instances is a
    propositional contradiction) or ``"valid"`` (the existential closure is
    valid — the disjunction of instances is a propositional tautology).
    """
    _need(claim in ("unsat", "valid"), f"claim must be 'unsat' or 'valid', got {claim!r}")
    matrix = _coerce_formula(matrix)
    _need(isinstance(substitutions, list) and substitutions,
          "need a non-empty list of ground substitutions")
    subs = []
    for sigma in substitutions:
        _need(isinstance(sigma, dict), "each substitution must be an object")
        subs.append({str(k): _coerce_term(v) for k, v in sigma.items()})
    free: set = set()
    _formula_vars(matrix, free)

    steps = [
        {"op": "herbrand_matrix", "matrix": matrix, "decision": claim,
         "variables": sorted(free),
         "note": "quantifier-free body; free vars are the quantified variables"},
        {"op": "ground_instances", "substitutions": subs},
        {"op": "assert_ground_instances"},
        {"op": "assert_propositional"},
    ]
    default = ("universal closure is unsatisfiable" if claim == "unsat"
               else "formula is valid")
    return {
        "format": FORMAT,
        "kind": "herbrand",
        "claim": claim_text or f"Herbrand certificate: {default}",
        "steps": steps,
        "meta": {"producer": "herbrand_search",
                 "decision_method": "exhaustive propositional truth table",
                 "note": "kernel decides the propositional combination; finding "
                         "the ground instances is the untrusted search seam"},
    }


# --------------------------------------------------------------------------- #
# Step handlers for the reference checker.
# --------------------------------------------------------------------------- #

def _h_herbrand_matrix(step, ctx):
    ctx["matrix"] = _coerce_formula(step["matrix"])
    decision = step["decision"]
    _need(decision in ("unsat", "valid"), f"bad decision {decision!r}")
    ctx["decision"] = decision
    free: set = set()
    _formula_vars(ctx["matrix"], free)
    ctx["free_vars"] = free


def _h_ground_instances(step, ctx):
    subs = step["substitutions"]
    _need(isinstance(subs, list) and subs,
          "ground_instances: need a non-empty list of substitutions")
    parsed = []
    for sigma in subs:
        _need(isinstance(sigma, dict), "each substitution must be an object")
        s = {}
        for k, v in sigma.items():
            _need(isinstance(k, str) and k != "", "substituted variable must be a name")
            term = _coerce_term(v)
            _need(_term_is_ground(term),
                  f"substitution for {k!r} is not a ground term")
            s[k] = term
        parsed.append(s)
    ctx["substitutions"] = parsed


def _h_assert_ground_instances(step, ctx):
    _need("matrix" in ctx and "substitutions" in ctx,
          "assert_ground_instances before matrix/substitutions")
    free = ctx["free_vars"]
    instances = []
    for sigma in ctx["substitutions"]:
        missing = free - set(sigma)
        _need(not missing,
              f"substitution does not cover free variables {sorted(missing)}")
        inst = _subst_formula(ctx["matrix"], sigma)
        _need(_formula_is_ground(inst),
              "instantiated matrix is not ground (unbound variable remains)")
        instances.append(inst)
    ctx["instances"] = instances


def _h_assert_propositional(step, ctx):
    instances = ctx.get("instances")
    _need(instances, "assert_propositional before instances were built")
    if ctx["decision"] == "unsat":
        # Universal closure unsatisfiable <-> conjunction of instances is a
        # propositional contradiction.
        combined = {"and": instances}
        _decide(combined, "unsat")
    else:
        # Formula valid <-> disjunction of instances is a propositional tautology.
        combined = {"or": instances}
        _decide(combined, "taut")
    ctx["concluded"] = True


_HANDLERS = {
    "herbrand_matrix": _h_herbrand_matrix,
    "ground_instances": _h_ground_instances,
    "assert_ground_instances": _h_assert_ground_instances,
    "assert_propositional": _h_assert_propositional,
}


# --------------------------------------------------------------------------- #
# REFERENCE CHECKER.
# --------------------------------------------------------------------------- #

def check(log: Any) -> dict:
    """Independently RE-VERIFY a Herbrand cert-log document.

    Returns ``{valid, reason, checked_steps, kind, claim}``.  Re-applies every
    ground substitution, confirms each instance is ground, identifies distinct
    ground atoms as propositional variables, and DECIDES the propositional
    combination by an exhaustive truth table; it never trusts the producer's
    claim.  An insufficient instance set (conjunction still satisfiable, or
    disjunction not a tautology), a tampered substitution, or any malformed step
    yields ``valid=False`` with a ``reason`` — the sound boundary.
    """
    checked = 0
    try:
        _need(isinstance(log, dict), "log is not a JSON object")
        _need(log.get("format") == FORMAT, f"unknown format: {log.get('format')!r}")
        kind = log.get("kind")
        _need(kind in KINDS, f"unknown kind: {kind!r}")
        steps = log.get("steps")
        _need(isinstance(steps, list) and steps, "steps must be a non-empty list")
        _need(isinstance(log.get("claim", ""), str), "claim must be a string")

        ctx: dict[str, Any] = {"concluded": False}
        for i, step in enumerate(steps):
            _need(isinstance(step, dict), f"step {i} is not an object")
            op = step.get("op")
            _need(op in _HANDLERS, f"step {i}: unknown op {op!r}")
            try:
                _HANDLERS[op](step, ctx)
            except _Reject:
                raise
            except (KeyError, IndexError, TypeError, ValueError) as exc:
                raise _Reject(f"step {i} ({op}): malformed data ({exc})")
            checked += 1

        _need(ctx.get("concluded"), "log reached no verified conclusion step")
        return {"valid": True, "reason": "all steps independently re-verified",
                "checked_steps": checked, "kind": kind, "claim": log.get("claim")}
    except _Reject as exc:
        return {"valid": False, "reason": str(exc), "checked_steps": checked,
                "kind": log.get("kind") if isinstance(log, dict) else None,
                "claim": log.get("claim") if isinstance(log, dict) else None}


# --------------------------------------------------------------------------- #
# Worker dispatch.
# --------------------------------------------------------------------------- #

def run(request: dict) -> dict:
    """Worker entrypoint.  ``request["op"]`` is ``export`` or ``check``.

    * ``export`` -> ``{"log": <document>}`` from ``request["matrix"]``,
      ``request["substitutions"]`` and ``request["claim"]`` (``"unsat"`` or
      ``"valid"``), optional ``claim_text``.
    * ``check`` -> :func:`check` on ``request["log"]``.
    """
    op = request.get("op", "check")
    if op == "check":
        return check(request["log"])
    if op == "export":
        log = export_herbrand_cert(
            request["matrix"],
            request["substitutions"],
            request["claim"],
            claim_text=request.get("claim_text"),
        )
        return {"log": log}
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
