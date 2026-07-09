"""Triviality / degenerate-solution detector for formalized statements.

Motivation (Erdős #728): a problem was mis-formulated and its naive
formalization admitted *trivial* solutions, so a short "proof" of a supposedly
hard problem actually meant the statement was broken. This module extends the
spirit of :mod:`theoremata_tools.falsify` -- which actively hunts for a
*counterexample* to a conjecture -- to actively hunt for a *degenerate witness*
or *vacuity* in the STATEMENT itself, before anyone tries to prove it.

Given a structured statement spec (variables with integer domains, a conjunction
of constraint predicates, and a goal predicate), it seeds a search that asks:

* **Degenerate witness** -- is there a witness satisfying ``constraints ∧ goal``
  that is "trivial" by a documented notion (a variable pinned to a domain
  boundary / zero / one / minus-one, i.e. a boundary value, an unexpectedly
  large extremal value, or a variable that escapes every constraint)?
* **Vacuity** -- are the constraints unsatisfiable over the searched domain (so a
  ``for all`` statement is vacuously true / an ``exists`` statement is
  mis-specified), or is the goal a tautology (trivially implied regardless of
  the constraints)?
* **Short certificate** -- when either of the above fires, we return a concrete,
  numerically re-verified witness (or a vacuity reason) as the certificate.

Design reuse from ``falsify.py``:

* the ``{start, stop, step}`` integer-domain shape and the 100k-value cap
  (:func:`_domain`, mirrors ``falsify._domain``);
* ``compile_expression`` + ``ALLOWED_NAMES`` to evaluate untrusted, model-emitted
  predicate strings in a restricted, builtin-free scope (same pattern as
  ``falsify._search_impl``'s ``eval(..., {"__builtins__": {}}, scope)``);
* the :class:`~theoremata_tools.sandbox.StepBudget` governor to decouple "how
  many candidates" from "how much total work" and terminate gracefully;
* the :func:`~theoremata_tools.sandbox.run_in_subprocess` hard-kill wrapper so an
  untrusted expression (e.g. ``10 ** 10 ** 8``) is force-killed rather than
  hanging the worker -- exactly as ``falsify.search`` does;
* the "re-verify the reported witness numerically before returning it" soundness
  stance (``falsify`` re-checks its counterexample; we re-check our witness).

SOUNDNESS: a reported trivial witness is re-evaluated against the compiled
constraints and goal and only returned if it genuinely satisfies them, so
``trivial: true`` with a ``degenerate_witness`` is a hard fact about the spec.
Everything else is ADVISORY: ``trivial: false`` / ``kind: "none"`` means only
that the *bounded, sampled* search found nothing trivial -- it is NOT a proof
that the statement is non-degenerate (the true domain may be larger than what we
sampled, and a degenerate witness could live outside it). Vacuity findings are
likewise advisory: "no admissible point sampled" and "goal held on every sampled
point" are bounded observations, not decision procedures.

The statement text is treated as UNTRUSTED DATA throughout: predicates are only
ever compiled through the safe-eval allow-list and executed in a hard-kill child
process. Pure standard library; any randomness is seeded via the caller.
"""
from __future__ import annotations

import random
from typing import Any

from .safe_eval import ALLOWED_NAMES, compile_expression
from .sandbox import DEFAULT_TIMEOUT_SECONDS, StepBudget, run_in_subprocess

OP = "triviality"

#: Cap on how many predicate evaluations one check may perform. Mirrors the
#: ``falsify`` step-budget idea: decouples "how many candidates" from "how much
#: total work" so a pathological product terminates gracefully.
DEFAULT_MAX_EVALS = 200_000

#: How many structured "corner" candidates (products of boundary/zero/one values)
#: to try before falling back to seeded random sampling.
DEFAULT_CORNER_CAP = 4096

#: How many seeded random candidates to sample for admissibility / tautology.
DEFAULT_RANDOM_SAMPLES = 2000

#: Minimum number of successfully-evaluated raw points before we are willing to
#: call a goal a "tautology" (holds regardless of constraints). Advisory only.
_TAUTOLOGY_MIN_POINTS = 12


# --- domain handling (mirrors falsify._domain) -----------------------------


def _domain(spec: dict[str, Any]) -> range:
    """Build an integer domain ``range`` from a ``{start, stop, step}`` dict.

    Mirrors ``falsify._domain`` including the 100,000-value hard cap; ``stop`` is
    exclusive (Python ``range`` semantics).
    """
    start = int(spec.get("start", -20))
    stop = int(spec.get("stop", 21))
    step = int(spec.get("step", 1))
    if step == 0:
        raise ValueError("domain step cannot be zero")
    values = range(start, stop, step)
    if len(values) == 0:
        raise ValueError("domain is empty")
    if len(values) > 100_000:
        raise ValueError("domain exceeds 100,000 values")
    return values


# --- spec parsing ----------------------------------------------------------


def _parse_spec(
    statement_spec: dict[str, Any],
) -> tuple[str, list[str], list[range], list[str], str]:
    """Validate the spec structurally (cheap, no eval) and extract its parts.

    Returns ``(quantifier, names, domains, constraints, goal)``. Raises
    ``ValueError``/``KeyError`` on a malformed spec so the worker surfaces it as
    ``{"ok": false, ...}`` -- structural errors are reported eagerly, before any
    untrusted expression is compiled or executed.
    """
    if not isinstance(statement_spec, dict):
        raise ValueError("statement_spec must be an object")

    quantifier = statement_spec.get("quantifier", "exists")
    if quantifier not in ("exists", "forall"):
        raise ValueError("quantifier must be 'exists' or 'forall'")

    raw_vars = statement_spec.get("variables", [])
    if not isinstance(raw_vars, list):
        raise ValueError("variables must be a list")

    names: list[str] = []
    domains: list[range] = []
    for entry in raw_vars:
        if not isinstance(entry, dict):
            raise ValueError("each variable must be an object with name/domain")
        name = entry["name"]
        if not isinstance(name, str) or not name.isidentifier():
            raise ValueError(f"invalid variable name: {name!r}")
        if name in names:
            raise ValueError(f"duplicate variable name: {name!r}")
        names.append(name)
        domains.append(_domain(entry.get("domain", {})))
    if not names:
        raise ValueError("spec must declare at least one variable")

    raw_constraints = statement_spec.get("constraints", [])
    if isinstance(raw_constraints, str):
        raw_constraints = [raw_constraints]
    if not isinstance(raw_constraints, list) or not all(
        isinstance(c, str) for c in raw_constraints
    ):
        raise ValueError("constraints must be a string or list of strings")

    goal = statement_spec.get("goal", "True")
    if not isinstance(goal, str):
        raise ValueError("goal must be a string predicate")

    return quantifier, names, list(domains), list(raw_constraints), goal


# --- degeneracy classification ---------------------------------------------


def _value_markers(value: int, dom: range) -> list[str]:
    """Documented "trivial value" markers for a single assigned value.

    A witness is considered a *degenerate witness* iff at least one of its
    variables carries one of these value markers.
    """
    markers: list[str] = []
    lo, hi = dom[0], dom[-1]
    if value == lo:
        markers.append("min_boundary")
    if value == hi:
        # The extremal / "unexpectedly large" value the domain permits.
        markers.append("max_boundary")
    if value == 0:
        markers.append("zero")
    if value == 1:
        markers.append("one")
    if value == -1:
        markers.append("neg_one")
    return markers


def _classify(env: dict[str, int], domains_by_name: dict[str, range],
              unconstrained: set[str]) -> dict[str, list[str]]:
    """Per-variable degeneracy markers for a full assignment (value markers plus,
    as a supplementary annotation, whether the variable escapes all constraints).
    """
    out: dict[str, list[str]] = {}
    for name, value in env.items():
        markers = _value_markers(value, domains_by_name[name])
        if name in unconstrained:
            markers = markers + ["unconstrained"]
        if markers:
            out[name] = markers
    return out


def _is_degenerate(marks: dict[str, list[str]]) -> bool:
    """Degenerate iff some variable carries a *value* marker. ``unconstrained``
    alone (a variable free of constraints but at a non-extremal value) is only an
    annotation -- it does not, by itself, make a witness degenerate, so a spec
    with no constraints at all is not blanket-flagged."""
    value_markers = {"min_boundary", "max_boundary", "zero", "one", "neg_one"}
    return any(value_markers.intersection(m) for m in marks.values())


# --- candidate generation --------------------------------------------------


def _corner_values(dom: range) -> list[int]:
    """The distinct "trivial" probe values for one domain: its two boundaries
    plus zero/one/minus-one when present, in a stable order."""
    seen: list[int] = []
    for v in (dom[0], dom[-1], 0, 1, -1):
        if v in dom and v not in seen:
            seen.append(v)
    return seen


def _corner_candidates(names: list[str], domains: list[range],
                       rng: random.Random, cap: int) -> list[dict[str, int]]:
    """Structured degenerate candidates: assignments drawn from each variable's
    corner values. Enumerated fully and seed-shuffled when small; seed-sampled
    when the product would exceed ``cap``."""
    per_var = [_corner_values(d) for d in domains]
    total = 1
    for vals in per_var:
        total *= len(vals)

    candidates: list[dict[str, int]] = []
    if total <= cap:
        # Full enumeration, then a seeded shuffle so witness selection is
        # deterministic-per-seed but not biased to the first lexicographic corner.
        import itertools

        for combo in itertools.product(*per_var):
            candidates.append(dict(zip(names, combo)))
        rng.shuffle(candidates)
    else:
        seen: set[tuple[int, ...]] = set()
        attempts = 0
        while len(candidates) < cap and attempts < cap * 4:
            attempts += 1
            combo = tuple(rng.choice(vals) for vals in per_var)
            if combo in seen:
                continue
            seen.add(combo)
            candidates.append(dict(zip(names, combo)))
    return candidates


# --- core search (runs inside the hard-kill sandbox) -----------------------


def _eval_bool(code: Any, env: dict[str, int]) -> bool | None:
    """Evaluate a compiled predicate in a restricted, builtin-free scope (same
    pattern as ``falsify._search_impl``). Returns ``True``/``False`` or ``None``
    when the expression errors on this point (e.g. a math-domain error), so a
    partial predicate simply excludes that point instead of crashing the search.
    """
    scope = {**ALLOWED_NAMES, **env}
    try:
        return bool(eval(code, {"__builtins__": {}}, scope))  # noqa: S307
    except Exception:  # noqa: BLE001 - untrusted expr may raise on some points
        return None


def _constraints_hold(codes: list[Any], env: dict[str, int],
                      budget: StepBudget) -> bool | None:
    """All-constraints conjunction. ``None`` if the budget is exhausted."""
    for code in codes:
        if not budget.spend(1):
            return None
        if _eval_bool(code, env) is not True:
            return False
    return True


def _triviality_impl(
    names: list[str],
    domains: list[range],
    constraints: list[str],
    goal: str,
    quantifier: str,
    unconstrained: list[str],
    seed: int,
    corner_cap: int,
    random_samples: int,
    max_evals: int,
) -> dict[str, Any]:
    """Seeded degenerate-witness / vacuity search. Runs in the sandbox child.

    Compiles the untrusted predicates through the safe-eval allow-list, probes
    structured "corner" candidates for a degenerate witness, then seed-samples
    the domain to gauge admissibility (is any constraint-satisfying point
    reachable?) and whether the goal is a tautology.
    """
    constraint_codes = [compile_expression(c, set(names)) for c in constraints]
    goal_code = compile_expression(goal, set(names))
    domains_by_name = dict(zip(names, domains))
    unconstrained_set = set(unconstrained)
    budget = StepBudget(total=max(1, int(max_evals)))
    rng = random.Random(seed)

    def verify(env: dict[str, int]) -> bool:
        """Independent re-verification of a witness (soundness gate)."""
        for code in constraint_codes:
            if _eval_bool(code, env) is not True:
                return False
        return _eval_bool(goal_code, env) is True

    any_admissible = False
    admissible_example: dict[str, int] | None = None
    goal_true_raw = 0
    goal_eval_raw = 0

    def observe_raw(env: dict[str, int]) -> None:
        """Track admissibility + tautology signal for one raw point."""
        nonlocal any_admissible, admissible_example, goal_true_raw, goal_eval_raw
        g = _eval_bool(goal_code, env)
        if g is not None:
            goal_eval_raw += 1
            if g:
                goal_true_raw += 1
        hold = _constraints_hold(constraint_codes, env, budget)
        if hold:
            any_admissible = True
            if admissible_example is None:
                admissible_example = dict(env)

    # 1. Structured degenerate probe: the first re-verified degenerate witness of
    #    ``constraints ∧ goal`` wins. Candidate order is seed-shuffled.
    for env in _corner_candidates(names, domains, rng, corner_cap):
        if budget.exhausted:
            break
        observe_raw(env)
        hold = _constraints_hold(constraint_codes, env, budget)
        if hold and _eval_bool(goal_code, env) is True:
            marks = _classify(env, domains_by_name, unconstrained_set)
            if _is_degenerate(marks) and verify(env):
                return _degenerate_result(env, marks, quantifier)

    # 2. Seeded random probe: gauge admissibility and tautology over the domain.
    for _ in range(max(0, int(random_samples))):
        if budget.exhausted:
            break
        env = {n: d[rng.randrange(len(d))] for n, d in zip(names, domains)}
        observe_raw(env)

    # 3. Decide, in priority order.
    if not any_admissible:
        reason = (
            "no constraint-satisfying assignment found in the seeded search; "
            + (
                "the 'for all' statement is vacuously true"
                if quantifier == "forall"
                else "the 'exists' statement has no witness (likely mis-specified)"
            )
            + " (advisory: bounded search, not a proof of unsatisfiability)"
        )
        return {"op": OP, "trivial": True, "kind": "vacuous", "witness": None,
                "reason": reason, "advisory": True}

    if goal_eval_raw >= _TAUTOLOGY_MIN_POINTS and goal_true_raw == goal_eval_raw:
        witness = admissible_example
        reason = (
            f"goal held on all {goal_eval_raw} sampled assignments (including "
            "constraint-violating ones): the goal is trivially implied / a "
            "tautology, so the statement carries no real content "
            "(advisory: sampled, not a decision procedure)"
        )
        return {"op": OP, "trivial": True, "kind": "vacuous", "witness": witness,
                "reason": reason, "advisory": True}

    return {
        "op": OP,
        "trivial": False,
        "kind": "none",
        "witness": None,
        "reason": (
            "no trivial/degenerate witness and no vacuity found in the seeded "
            "search; the constraints are satisfiable and the goal is non-trivial "
            "on the sampled domain. ADVISORY ONLY: 'none found' is not a "
            "guarantee of non-triviality -- a degenerate witness may exist "
            "outside the searched domain."
        ),
        "advisory": True,
    }


def _degenerate_result(env: dict[str, int], marks: dict[str, list[str]],
                       quantifier: str) -> dict[str, Any]:
    detail = ", ".join(
        f"{name}={env[name]} ({'/'.join(m)})" for name, m in sorted(marks.items())
    )
    reason = (
        f"degenerate witness satisfying constraints and goal: {detail}. "
        "Such a boundary/zero/one/extremal solution usually signals a "
        "mis-formulated statement whose 'hard' content is trivially satisfiable "
        "(verified numerically)."
    )
    return {
        "op": OP,
        "trivial": True,
        "kind": "degenerate_witness",
        "witness": dict(env),
        "markers": marks,
        "reason": reason,
        "advisory": True,
    }


# --- public API ------------------------------------------------------------


def triviality_check(
    statement_spec: dict[str, Any],
    *,
    seed: int,
    corner_cap: int = DEFAULT_CORNER_CAP,
    random_samples: int = DEFAULT_RANDOM_SAMPLES,
    max_evals: int = DEFAULT_MAX_EVALS,
    hard_kill: bool = True,
    timeout_seconds: float = DEFAULT_TIMEOUT_SECONDS,
) -> dict[str, Any]:
    """Actively check a formalized statement for triviality / degeneracy.

    ``statement_spec`` schema (number-theory / arithmetic first-order claims)::

        {
          "quantifier": "exists" | "forall",   # default "exists"
          "variables": [
            {"name": "x", "domain": {"start": -20, "stop": 21, "step": 1}},
            ...                                  # integer domain, stop exclusive
          ],
          "constraints": ["x > 0", "y > 0"],     # str | list[str]; conjunction
          "goal": "x*x + y*y == z*z"             # predicate over the variables
        }

    * ``quantifier: "exists"`` -- the claim is "there exist values in the domains
      satisfying ``constraints`` and ``goal``". It is *trivial* if that witness
      is degenerate, or *vacuous* if no admissible point exists / the goal is a
      tautology.
    * ``quantifier: "forall"`` -- the claim is "for all values satisfying
      ``constraints``, ``goal`` holds". It is *vacuously true* if the constraints
      are unsatisfiable, or *trivial* if the goal is a tautology; a degenerate
      admissible point that already satisfies the goal is also flagged.

    Returns::

        {"op": "triviality",
         "trivial": bool,
         "kind": "degenerate_witness" | "vacuous" | "none",
         "witness": {var: int} | None,
         "reason": str,
         "advisory": true}          # (plus "markers" for a degenerate witness)

    A reported degenerate ``witness`` is numerically re-verified before return
    (sound). ``kind: "none"`` is advisory: it means the bounded, seeded search
    found nothing trivial, not that the statement is provably non-degenerate.

    By default the search runs in a hard-kill child process (see
    :func:`~theoremata_tools.sandbox.run_in_subprocess`) so an untrusted,
    runaway predicate is force-killed and reported as ``inconclusive`` rather
    than hanging the worker.
    """
    quantifier, names, domains, constraints, goal = _parse_spec(statement_spec)
    unconstrained = _unconstrained_vars(names, constraints)

    impl_args = (
        names, domains, constraints, goal, quantifier, unconstrained,
        int(seed), int(corner_cap), int(random_samples), int(max_evals),
    )
    if not hard_kill:
        return _triviality_impl(*impl_args)

    result = run_in_subprocess(
        _triviality_impl, args=impl_args, timeout=timeout_seconds
    )
    if result.timed_out:
        return {
            "op": OP,
            "trivial": False,
            "kind": "none",
            "witness": None,
            "reason": f"triviality search timed out after {timeout_seconds}s "
                      "(inconclusive; no trivial witness confirmed)",
            "advisory": True,
        }
    # ok -> the result dict; compile/spec error -> raise (worker -> ok:false).
    return result.unwrap()


def _unconstrained_vars(names: list[str], constraints: list[str]) -> list[str]:
    """Variable names that do not appear (textually, as an identifier) in any
    constraint. A cheap, safe heuristic (no eval) used only to annotate a witness
    -- a variable free of every constraint is a classic "vacuous constraint"
    smell. ``ast`` would be exact but the plain-substring test is intentionally
    conservative (it over-reports 'constrained', never under-reports it)."""
    import ast

    referenced: set[str] = set()
    for expr in constraints:
        try:
            tree = ast.parse(expr, mode="eval")
        except SyntaxError:
            # Can't parse -> assume it might reference anything; be conservative.
            return []
        for node in ast.walk(tree):
            if isinstance(node, ast.Name):
                referenced.add(node.id)
    return [n for n in names if n not in referenced]


def run(request: dict[str, Any]) -> dict[str, Any]:
    """JSON-worker adapter. Accepts ``{"statement_spec": {...}, "seed": int}``
    (or the spec fields at the top level) and returns the triviality report.

    Suggested worker op name: ``"triviality"`` (wire in ``worker.dispatch``).
    """
    spec = request.get("statement_spec") or request.get("spec")
    if spec is None:
        # Allow the spec fields to be passed at the top level.
        spec = {
            k: request[k]
            for k in ("quantifier", "variables", "constraints", "goal")
            if k in request
        }
    seed = int(request.get("seed", 0))
    kwargs: dict[str, Any] = {}
    for key in ("corner_cap", "random_samples", "max_evals"):
        if key in request:
            kwargs[key] = int(request[key])
    if "hard_kill" in request:
        kwargs["hard_kill"] = bool(request["hard_kill"])
    if "timeout_seconds" in request:
        kwargs["timeout_seconds"] = float(request["timeout_seconds"])
    return triviality_check(spec, seed=seed, **kwargs)
