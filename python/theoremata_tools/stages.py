"""Research-to-formal stage catalog.

Encodes the 12 domain-neutral stage templates and the typed claim-DAG that
Theoremata's research->formal spine follows (docs/PLAN.md section 10), derived
from the MathResearchPrompts workflow. This is pure scaffolding: the templates,
the pipeline topology, the node/evidence typing, and validation helpers. It runs
a *no* model and touches *no* network -- a model runner sits above it later.

Hard rule encoded throughout: numeric checks SCREEN a claim, they never PROVE
it. A numeric pass only unblocks a formalization node.
"""
from __future__ import annotations

import json
import string
import sys
from enum import IntEnum
from typing import Any

# --- Typed claim-DAG -------------------------------------------------------

# Node types of the research claim-DAG:
#   Setting -> CandidateClaim{type, status} -> Proof/Disproof + Verdict
#           -> FormalizationTarget -> LeanTheorem
NODE_TYPES = [
    "Setting",
    "CandidateClaim",
    "ProofBranch",
    "DisproofBranch",
    "Verdict",
    "FormalizationTarget",
    "LeanTheorem",
]

# Status a CandidateClaim can hold after screening.
CLAIM_STATUSES = ["pass", "fail", "inconclusive"]

# Candidate-claim type labels observed in the reference workflow.
CLAIM_TYPE_LABELS = [
    "Invariant",
    "NormIdentity",
    "ScalarRecursion",
    "Spectral",
    "Convergence",
    "Stability",
    "NormalForm",
]


class EvidenceStrength(IntEnum):
    """Ordered evidence strength carried on claim-DAG edges.

    numeric_screen < prose_proof < lean_checked. A numeric screen is the
    weakest: it can only unblock formalization, never certify a claim.
    """

    numeric_screen = 1
    prose_proof = 2
    lean_checked = 3


def stronger_than(a: str, b: str) -> bool:
    """True iff evidence level ``a`` is strictly stronger than ``b``."""
    return EvidenceStrength[a] > EvidenceStrength[b]


# Two-tolerance falsifier epistemics: an exact algebraic identity is checked at
# machine-zero; a finite-difference approximation is checked loosely.
FALSIFIER_TOLERANCES = {
    "exact_identity": 1e-12,
    "finite_difference": 5e-3,
}


# --- The 12 stages ---------------------------------------------------------

STAGES: dict[str, dict[str, Any]] = {
    "scope_ideate": {
        "key": "scope_ideate",
        "title": "Scope & Ideate",
        "purpose": (
            "Frame the problem and feasibility; call out obvious degenerate or "
            "trivial regimes to watch for."
        ),
        "produces": "Setting",
        "requires": [],
        "prompt_template": (
            "Scope the following problem and assess feasibility. List the "
            "mathematical objects in play and any degenerate/trivial regimes to "
            "watch out for.\n\nProblem:\n{problem}"
        ),
    },
    "object_identification": {
        "key": "object_identification",
        "title": "Sharpen Objects",
        "purpose": "Name the precise objects on both sides of the problem.",
        "produces": "Setting",
        "requires": ["scope_ideate"],
        "prompt_template": (
            "Identify precisely every object involved: domains, spaces, maps, "
            "and the objective. State each with exact notation.\n\nProblem:\n"
            "{problem}"
        ),
    },
    "claim_sharpening": {
        "key": "claim_sharpening",
        "title": "Sharpen Claims + Validation Plan",
        "purpose": (
            "Turn intuition into explicit claims, add an artifact/intrinsic-vs-"
            "coordinate check, and TRIAGE: which claims target rigorous proof vs "
            "are presented as conjectures supported by numerics."
        ),
        "produces": "CandidateClaim",
        "requires": ["object_identification"],
        "prompt_template": (
            "Given the objects {objects}, state sharp claims. For each, add "
            "checks that the observed structure is intrinsic rather than a "
            "parameterization artifact, then triage each claim into "
            "PROVE (target rigorous proof) or CONJECTURE (numeric support only)."
            "\n\nClaim under study:\n{claim}"
        ),
    },
    "direct_proof": {
        "key": "direct_proof",
        "title": "Direct Proof",
        "purpose": (
            "Prove line-by-line from restated assumptions; mark every extra "
            "assumption or external theorem as a proof obligation; never "
            "fabricate references."
        ),
        "produces": "ProofBranch",
        "requires": ["claim_sharpening"],
        "prompt_template": (
            "Restate all assumptions and domains, then prove the claim with "
            "numbered, individually justified steps. Cite standard results by "
            "their usual names; never invent references. Mark any step needing "
            "an extra assumption or external theorem as a PROOF OBLIGATION. End "
            "with an itemized list of adversarial checks (degenerate/boundary "
            "cases).\n\nClaim:\n{claim}\n\nAssumptions:\n{assumptions}"
        ),
    },
    "prove_or_disprove": {
        "key": "prove_or_disprove",
        "title": "Prove-or-Disprove",
        "purpose": (
            "Run a proof branch and a disproof/counterexample-hunt branch in "
            "parallel, then merge into a single verdict."
        ),
        "produces": "Verdict",
        "requires": ["claim_sharpening"],
        "prompt_template": (
            "Pursue two branches for the claim: (A) a proof, (B) a disproof with "
            "an explicit counterexample hunt. Then give a final verdict merging "
            "both branches.\n\nClaim:\n{claim}\n\nAssumptions:\n{assumptions}"
        ),
    },
    "candidate_discovery": {
        "key": "candidate_discovery",
        "title": "Candidate Discovery",
        "purpose": (
            "Emit N typed candidate claims each with a type label and a "
            "pass/fail/inconclusive status, then select a coherent subset."
        ),
        "produces": "CandidateClaim",
        "requires": ["object_identification"],
        "prompt_template": (
            "From the objects {objects}, propose candidate claims. Give each a "
            "type label (one of Invariant, NormIdentity, ScalarRecursion, "
            "Spectral, Convergence, Stability, NormalForm) and a status "
            "(pass/fail/inconclusive). Select a coherent 3-5 subset into one "
            "composite proposition."
        ),
    },
    "transfer_schema": {
        "key": "transfer_schema",
        "title": "Transfer Schema",
        "purpose": (
            "Extract a reusable schema (invariant subspace, progress "
            "coordinate) and instantiate it in a new setting."
        ),
        "produces": "Setting",
        "requires": ["candidate_discovery"],
        "prompt_template": (
            "Extract the reusable schema behind the established structure over "
            "objects {objects}, then instantiate it in a new setting and note "
            "the analogy edges.\n\nAnchor claim:\n{claim}"
        ),
    },
    "property_constrained_synthesis": {
        "key": "property_constrained_synthesis",
        "title": "Property-Constrained Synthesis",
        "purpose": (
            "Emit an EXECUTABLE falsifier for the claim. Numeric tests are "
            "screening evidence ONLY -- a symbolic proof or a clearly stated "
            "proof obligation is required before acceptance."
        ),
        "produces": "falsifier",
        "requires": ["claim_sharpening"],
        "prompt_template": (
            "Write concrete, executable code (standard-library numerics) that "
            "tries to FALSIFY the claim over deterministic and random "
            "adversarial seeds. Check exact algebraic identities at ~1e-12 and "
            "finite-difference approximations at ~5e-3. Remember: a numeric "
            "pass SCREENS the claim, it never PROVES it -- it only unblocks the "
            "formalization node.\n\nClaim:\n{claim}\n\nAssumptions:\n"
            "{assumptions}"
        ),
    },
    "constant_stress_test": {
        "key": "constant_stress_test",
        "title": "Constant Stress-Test",
        "purpose": "Stress the constants/edge parameters of the screened claim.",
        "produces": "numeric_screen",
        "requires": ["property_constrained_synthesis"],
        "prompt_template": (
            "Stress-test the constants and boundary parameters of the claim "
            "numerically; report which regimes hold and which break.\n\nClaim:\n"
            "{claim}"
        ),
    },
    "rate_refinement": {
        "key": "rate_refinement",
        "title": "Rate Refinement",
        "purpose": "Refine quantitative rates (e.g. sublinear -> linear).",
        "produces": "CandidateClaim",
        "requires": ["property_constrained_synthesis"],
        "prompt_template": (
            "Refine the quantitative rate in the claim to the tightest form the "
            "evidence supports, and state the remaining obligation to prove "
            "it.\n\nClaim:\n{claim}"
        ),
    },
    "prompt_variation": {
        "key": "prompt_variation",
        "title": "Prompt-Variation / Failure-Mode Sensitivity",
        "purpose": (
            "Vary the prompt/assumptions to probe robustness and surface "
            "failure modes of the proof."
        ),
        "produces": "sensitivity_report",
        "requires": ["direct_proof"],
        "prompt_template": (
            "Vary the phrasing and marginal assumptions of the proof of the "
            "claim; report sensitivity and any failure modes uncovered.\n\n"
            "Claim:\n{claim}"
        ),
    },
    "environment_log": {
        "key": "environment_log",
        "title": "Environment Log",
        "purpose": (
            "Record per-node provenance: prompt+version, model snapshot, "
            "temperature, tool versions, accepted/rejected outputs, and the "
            "unresolved-proof-obligations list (the DAG frontier)."
        ),
        "produces": "provenance",
        "requires": [],
        "prompt_template": (
            "Record the computational environment and provenance for this run: "
            "prompt and version, model snapshot, temperature, tool versions, "
            "accepted/rejected outputs, and the current unresolved-proof-"
            "obligations list."
        ),
    },
}


# --- Catalog helpers -------------------------------------------------------


def stage(key: str) -> dict[str, Any]:
    if key not in STAGES:
        raise KeyError(f"unknown stage: {key}")
    return STAGES[key]


def sequence() -> list[str]:
    """A valid topological order of the stages honoring ``requires``.

    Declaration order is preserved as the tiebreak within each dependency
    layer, so the ideation-first spine reads naturally.
    """
    ordered: list[str] = []
    remaining = list(STAGES.keys())
    while remaining:
        progressed = False
        for key in list(remaining):
            if all(dep in ordered for dep in STAGES[key]["requires"]):
                ordered.append(key)
                remaining.remove(key)
                progressed = True
        if not progressed:
            raise ValueError(f"cycle in stage requirements: {remaining}")
    return ordered


def _template_slots(template: str) -> set[str]:
    return {
        field_name
        for _, field_name, _, _ in string.Formatter().parse(template)
        if field_name
    }


def render(key: str, **slots: Any) -> str:
    """Fill a stage's prompt template; raise on any missing slot."""
    template = stage(key)["prompt_template"]
    required = _template_slots(template)
    missing = required - set(slots)
    if missing:
        raise ValueError(f"missing slots for stage {key!r}: {sorted(missing)}")
    return template.format(**slots)


def next_stages(done: list[str]) -> list[str]:
    """Stages whose ``requires`` are all satisfied and not yet done."""
    done_set = set(done)
    return [
        key
        for key, spec in STAGES.items()
        if key not in done_set and all(dep in done_set for dep in spec["requires"])
    ]


def formalization_target(
    informal_statement: str, symbol_dictionary: dict[str, str]
) -> dict[str, Any]:
    """Build the first-class FormalizationTarget artifact.

    Carries a Lean signature stub, the paper-symbol -> Lean-def dictionary, and
    a default decomposition into Lean-sized sub-targets (algebraic-identity,
    structure/invariance, scalar-analysis, bridge/join).
    """
    stub = "theorem target : <TODO: formalize> := by sorry"
    sub_targets = [
        {"key": "algebraic_identity", "role": "algebraic-identity node"},
        {"key": "structure_invariance", "role": "structure/invariance node"},
        {"key": "scalar_analysis", "role": "scalar-analysis node"},
        {"key": "bridge_join", "role": "bridge/join node"},
    ]
    return {
        "informal_statement": informal_statement,
        "lean_signature_stub": stub,
        "symbol_dictionary": dict(symbol_dictionary),
        "sub_targets": sub_targets,
    }


# --- Worker entrypoint -----------------------------------------------------


def run(request: dict[str, Any]) -> Any:
    op = request.get("op", "sequence")
    if op == "sequence":
        return {"op": op, "sequence": sequence()}
    if op == "stage":
        return {"op": op, "stage": stage(request["key"])}
    if op == "render":
        slots = request.get("slots", {})
        return {"op": op, "key": request["key"], "prompt": render(request["key"], **slots)}
    if op == "next_stages":
        return {"op": op, "next": next_stages(request.get("done", []))}
    if op == "formalization_target":
        return {
            "op": op,
            "target": formalization_target(
                request["informal_statement"],
                request.get("symbol_dictionary", {}),
            ),
        }
    raise ValueError(f"unknown op: {op}")


def main() -> None:
    if len(sys.argv) >= 2:
        with open(sys.argv[1], encoding="utf-8") as fh:
            request = json.load(fh)
    else:
        request = json.load(sys.stdin)
    print(json.dumps(run(request), indent=2))
    raise SystemExit(0)


if __name__ == "__main__":
    main()
