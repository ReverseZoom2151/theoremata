"""Flywheel data-curation: curate, don't just accumulate.

A convergent finding across DeepSeek-Prover-V2, BFS-Prover, Hunyuan-Prover,
Goedel-Prover and Kimina is that raw self-generated proof data must be
*curated* -- densified into a subgoal curriculum, mined for cold-start
positives, consistency-checked, and self-filtered to shed already-easy problems
-- before it feeds SFT/RL. Accumulating everything plateaus; curation keeps the
accumulated corpus trending harder and denser. This module implements those four
offline curation ops as a sibling to :mod:`flywheel` (auto-labeling) and
:mod:`star_harvester` (trace -> SFT).

The four ops
------------
* :func:`subgoal_to_conjectures` -- the DeepSeek-Prover-V2 recursive-subgoal
  recipe: turn every ``have``-subgoal of a (whole or sketch) proof into
  standalone training theorems, in TWO forms -- (a) the subgoal alone, and
  (b) the subgoal WITH its preceding sibling subgoals as premises. This
  *densifies* one proof into many gradient-bearing conjectures.
* :func:`mine_cold_start_positives` -- keep the "whole-proof failed but every
  subgoal was solved" attempts as SFT positives (the cold-start seed: the model
  could not close the theorem in one shot, but the *decomposition* is sound and
  worth learning).
* :func:`consistency_check` -- the DeepSeek/Goedel consistency reward: reject a
  final proof that silently DROPS a ``have``-lemma it declared. A decomposition
  the final proof abandons was never really used.
* :func:`beam_self_filter` -- the BFS/Hunyuan easy-data self-filter: drop
  problems the policy already solves *consistently* so the accumulated corpus
  trends harder. Fully deterministic (order-preserving, seeded by index; no
  wall-clock, no randomness).

Data-row contract
-----------------
A *theorem* / *conjecture* row carries a ``statement`` key (the rendered goal),
so it drops straight into :func:`flywheel.revolution` /
:func:`flywheel.label_dataset` as a problem (which accept ``statement`` /
``problem`` / ``goal``). SFT positive rows use the flat
``{prompt, completion, meta}`` shape of :mod:`star_harvester` (convert with
``star_harvester.to_chat_row`` for the chat-SFT schema).

Offline / pure-Python: no model, no GPU, no live-core coupling. This is the
*curation* layer -- it shapes what the GPU-gated SFT/RL later trains on.
"""
from __future__ import annotations

import json
import sys
from typing import Any, Optional, Sequence

__all__ = [
    "parse_haves",
    "subgoal_to_conjectures",
    "mine_cold_start_positives",
    "consistency_check",
    "beam_self_filter",
    "run",
]


# ---------------------------------------------------------------------------
# have-subgoal parsing
# ---------------------------------------------------------------------------

def parse_haves(proof: str) -> list[dict[str, Any]]:
    """Parse ``have``-subgoals out of a Lean(-ish) proof body.

    Recognizes ``have <name> : <statement> := <body>`` and the anonymous
    ``have : <statement> := <body>`` (the ``:= <body>`` may be absent for a
    goal-style ``have``). Returns an ordered list of
    ``{"name": str|None, "statement": str}`` in source order. Best-effort and
    line-oriented -- it is a curation heuristic, not a Lean parser.
    """
    haves: list[dict[str, Any]] = []
    for raw in proof.splitlines():
        line = raw.strip()
        if not (line == "have" or line.startswith("have ") or line.startswith("have:")):
            continue
        rest = line[len("have"):].strip()
        # Drop the proof body: everything from the first ':=' onward.
        head = rest.split(":=", 1)[0].strip()
        if ":" not in head:
            continue
        name_part, stmt = head.split(":", 1)
        name = name_part.strip() or None
        stmt = stmt.strip()
        if stmt:
            haves.append({"name": name, "statement": stmt})
    return haves


def _render(premises: Sequence[str], statement: str) -> str:
    """Render a conjecture goal: preceding subgoals become hypotheses via ``->``
    (Lean's implication arrow), matching how a standalone theorem would carry
    them as premises."""
    if not premises:
        return statement
    return " -> ".join([*premises, statement])


# ---------------------------------------------------------------------------
# Op 1: subgoal -> conjecture curriculum (DeepSeek-Prover-V2)
# ---------------------------------------------------------------------------

def subgoal_to_conjectures(proof_with_haves: Any) -> list[dict[str, Any]]:
    """Densify one proof into standalone training theorems, one pair per
    ``have``-subgoal.

    ``proof_with_haves`` is either a proof string or a dict carrying ``proof``
    (and optional ``goal`` / ``statement`` / ``problem`` for provenance). Each
    ``have``-subgoal yields TWO conjecture rows:

    * **form ``"alone"``** -- the subgoal proved from scratch, ``premises=[]``.
    * **form ``"with_premises"``** -- the subgoal proved with all PRECEDING
      sibling subgoals available as premises (the recursive-subgoal curriculum:
      later subgoals may legitimately depend on earlier ones).

    Each row is ``{statement, subgoal, premises, form, name, source, index}``
    where ``statement`` is the rendered goal (so the row is a ready flywheel
    problem). The first subgoal's two forms are identical (no preceding
    premises); both are still emitted so downstream dedup (by content hash) --
    not this densifier -- decides.
    """
    if isinstance(proof_with_haves, dict):
        proof = str(proof_with_haves.get("proof", ""))
        source = str(
            proof_with_haves.get("goal")
            or proof_with_haves.get("statement")
            or proof_with_haves.get("problem")
            or ""
        )
    else:
        proof = str(proof_with_haves)
        source = ""

    haves = parse_haves(proof)
    out: list[dict[str, Any]] = []
    preceding: list[str] = []
    for i, hv in enumerate(haves):
        stmt = hv["statement"]
        name = hv["name"] or f"have_{i}"
        premises = list(preceding)
        # (a) the subgoal alone
        out.append(
            {
                "statement": _render([], stmt),
                "subgoal": stmt,
                "premises": [],
                "form": "alone",
                "name": name,
                "source": source,
                "index": i,
            }
        )
        # (b) the subgoal WITH preceding subgoals as premises
        out.append(
            {
                "statement": _render(premises, stmt),
                "subgoal": stmt,
                "premises": premises,
                "form": "with_premises",
                "name": name,
                "source": source,
                "index": i,
            }
        )
        preceding.append(stmt)
    return out


# ---------------------------------------------------------------------------
# Op 2: cold-start positive mining
# ---------------------------------------------------------------------------

def _whole_ok(attempt: dict[str, Any]) -> bool:
    """Did the whole-proof attempt itself verify? Tolerates ``whole_verified`` /
    ``verified`` (bool or ``{compiled, axioms_ok}``)."""
    v = attempt.get("whole_verified", attempt.get("verified"))
    if isinstance(v, dict):
        return bool(v.get("compiled")) and bool(v.get("axioms_ok", True))
    return bool(v)


def _subgoal_solved(sg: dict[str, Any]) -> bool:
    v = sg.get("solved", sg.get("verified"))
    if isinstance(v, dict):
        return bool(v.get("compiled")) and bool(v.get("axioms_ok", True))
    return bool(v)


def mine_cold_start_positives(attempts: Sequence[dict[str, Any]]) -> list[dict[str, Any]]:
    """Keep the cold-start SFT positives: attempts where the WHOLE proof FAILED
    but EVERY declared subgoal was solved.

    Each attempt is ``{problem|goal, subgoals: [{statement, solved, proof}], ...,
    whole_verified: bool}``. The sound decomposition (all subgoals discharged) is
    a positive worth teaching even though the one-shot whole proof did not close.
    Emits :mod:`star_harvester`-shaped ``{prompt, completion, meta}`` rows; the
    completion is the attempt's ``assembled_proof`` if provided, else a
    ``have``-block reassembled from the solved subgoals. Attempts with no
    subgoals, an unsolved subgoal, or a whole proof that already verified are
    skipped.
    """
    rows: list[dict[str, Any]] = []
    for attempt in attempts:
        subgoals = list(attempt.get("subgoals", []))
        if not subgoals:
            continue
        if _whole_ok(attempt):
            continue  # already a whole-proof positive; not a cold-start case
        if not all(_subgoal_solved(sg) for sg in subgoals):
            continue  # decomposition is incomplete -> not a positive

        prompt = str(attempt.get("problem") or attempt.get("goal") or attempt.get("statement") or "")
        assembled = attempt.get("assembled_proof")
        if assembled is None:
            lines = []
            for i, sg in enumerate(subgoals):
                name = sg.get("name") or f"h{i}"
                body = str(sg.get("proof", "")).strip() or "sorry"
                lines.append(f"have {name} : {sg.get('statement', '')} := {body}")
            assembled = "\n".join(lines)
        rows.append(
            {
                "prompt": prompt,
                "completion": str(assembled),
                "meta": {
                    "kind": "cold_start_positive",
                    "n_subgoals": len(subgoals),
                    "source": "subgoal_mining",
                },
            }
        )
    return rows


# ---------------------------------------------------------------------------
# Op 3: consistency check (declared have-lemma must survive)
# ---------------------------------------------------------------------------

def _have_token(item: Any) -> str:
    """The token that must appear in the final proof for a declared ``have`` to
    count as kept: its name when we have one, else its statement text."""
    if isinstance(item, dict):
        return str(item.get("name") or item.get("statement") or "")
    return str(item)


def consistency_check(declared_haves: Sequence[Any], final_proof: str) -> bool:
    """The DeepSeek/Goedel consistency reward: return ``False`` if the final
    proof DROPS any ``have``-lemma it declared, else ``True``.

    ``declared_haves`` is a list of names, statements, or
    ``{name, statement}`` dicts (e.g. from :func:`parse_haves`). A declared have
    is "kept" iff its name (preferred) or statement text still appears in
    ``final_proof``. An empty declaration list is trivially consistent.
    """
    for item in declared_haves:
        token = _have_token(item)
        if not token:
            continue
        if token not in final_proof:
            return False
    return True


def _dropped_haves(declared_haves: Sequence[Any], final_proof: str) -> list[str]:
    return [
        _have_token(it)
        for it in declared_haves
        if _have_token(it) and _have_token(it) not in final_proof
    ]


# ---------------------------------------------------------------------------
# Op 4: beam self-filter (drop consistently-solved easy problems)
# ---------------------------------------------------------------------------

def _consistently_solved(flag: Any) -> bool:
    """A problem is EASY (consistently solved) when every recorded attempt
    succeeded. ``flag`` is a single bool, or a sequence of per-beam-sample bools;
    a bare truthy value or an all-True non-empty sequence counts as consistent.
    An empty sequence means "never solved" -> not easy."""
    if isinstance(flag, (list, tuple)):
        return len(flag) > 0 and all(bool(x) for x in flag)
    return bool(flag)


def beam_self_filter(
    problems: Sequence[Any],
    solved_flags: Sequence[Any],
    *,
    keep_hard: bool = True,
) -> list[Any]:
    """Self-filter accumulated problems by beam-solve difficulty so the corpus
    trends harder (BFS-Prover / Hunyuan).

    ``solved_flags`` runs parallel to ``problems``; each entry is a bool or a
    list of per-beam-sample bools. A problem is EASY when consistently solved
    (every sample succeeded). With ``keep_hard=True`` (default) the easy problems
    are dropped and the hard/unsolved ones kept; with ``keep_hard=False`` the
    reverse. Deterministic: input order is preserved and the decision is a pure
    function of ``(index, flag)`` -- no wall-clock, no randomness. Problems past
    the end of ``solved_flags`` are treated as unsolved (hard).
    """
    out: list[Any] = []
    for i, problem in enumerate(problems):
        flag = solved_flags[i] if i < len(solved_flags) else False
        easy = _consistently_solved(flag)
        keep = (not easy) if keep_hard else easy
        if keep:
            out.append(problem)
    return out


# ---------------------------------------------------------------------------
# Worker dispatch
# ---------------------------------------------------------------------------

def run(request: dict[str, Any]) -> dict[str, Any]:
    """Dispatch a curation op over a JSON request.

    Ops:

    * ``subgoal_to_conjectures`` -- ``{proof|goal|statement}`` ->
      ``{ok, theorems, n}``.
    * ``mine_cold_start_positives`` -- ``{attempts:[...]}`` ->
      ``{ok, rows, kept}``.
    * ``consistency_check`` -- ``{declared_haves:[...], final_proof}`` ->
      ``{ok, consistent, dropped}``.
    * ``beam_self_filter`` -- ``{problems:[...], solved_flags:[...],
      keep_hard?}`` -> ``{ok, kept, dropped, problems}``.
    """
    op = request.get("op", "subgoal_to_conjectures")
    if op == "subgoal_to_conjectures":
        proof_in = request.get("proof", request.get("proof_with_haves", request))
        theorems = subgoal_to_conjectures(proof_in)
        return {"ok": True, "theorems": theorems, "n": len(theorems)}
    if op == "mine_cold_start_positives":
        rows = mine_cold_start_positives(request.get("attempts", []))
        return {"ok": True, "rows": rows, "kept": len(rows)}
    if op == "consistency_check":
        declared = request.get("declared_haves", [])
        final_proof = str(request.get("final_proof", ""))
        consistent = consistency_check(declared, final_proof)
        return {
            "ok": True,
            "consistent": consistent,
            "dropped": _dropped_haves(declared, final_proof),
        }
    if op == "beam_self_filter":
        problems = request.get("problems", [])
        solved_flags = request.get("solved_flags", [])
        keep_hard = bool(request.get("keep_hard", True))
        kept = beam_self_filter(problems, solved_flags, keep_hard=keep_hard)
        return {
            "ok": True,
            "kept": len(kept),
            "dropped": len(problems) - len(kept),
            "problems": kept,
        }
    raise ValueError(f"unknown op: {op}")


def main() -> None:
    if len(sys.argv) >= 2:
        with open(sys.argv[1], encoding="utf-8") as fh:
            request = json.load(fh)
    else:
        request = json.load(sys.stdin)
    print(json.dumps(run(request), indent=2, default=str))
    raise SystemExit(0)


if __name__ == "__main__":
    main()
