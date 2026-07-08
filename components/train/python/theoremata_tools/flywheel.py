"""Auto-labeling flywheel: zero-human hard labels for proof-correctness data.

Ports DeepSeek-Math-V2's self-improving data engine
(``docs/resource-mining/new/DeepSeek-Math-V2.md`` s2.5, PDF p.6). Given generated
proofs, it runs each proof through ``n`` verifications and, for the ones that
report an issue, ``m`` meta-verifications; a majority of confirming
meta-assessments validates an analysis. The proof is labeled with the **lowest
score confirmed by >= k valid analyses**, else labeled ``1.0``. In DeepSeek's
last two RL iterations this fully replaced human annotation.

Pluggable ORACLE
----------------
The verifier is a *pluggable oracle* -- any callable ``(problem, proof) -> score``
(a float, a ``{"score","analysis","reports_issue"}`` dict, or a
:class:`Verification`). Two oracle families matter:

* **NL self-verifier** -- an LLM that emits ``{issue-summary, score in {0,.5,1}}``.
  It can *lie* (score a flawed proof while hallucinating fake issues), so its
  analyses must pass the meta-verifier before they count.
* **Formal oracle (STRONGER)** -- our Lean/Rocq/Isabelle ``verify`` (compile +
  ``#print axioms`` closure + kernel typecheck + soundness scan). A clean verdict
  is a **hard label 1.0**, a failure a **hard label 0.0**, with *no self-report to
  be faithful about and no meta-verification needed*. Pass ``formal=True`` (or a
  :func:`formal_oracle`-wrapped verdict fn) to take this path. This is a strictly
  stronger hard-label source than DeepSeek's all-LLM verifier for the
  autoformalizable subset -- exactly the hybrid the mining report recommends.

Output is a labeled dataset with full provenance, convertible to SFT rows
(label-1 proofs become positive ``messages`` targets) or GRPO rows
(``{gold, proof, label, verdict}``) that feed the faithful-verifier reward in
``reward.py``.

Offline / dry-run only: no model, no GPU. Tests drive it with mock oracles.
"""
from __future__ import annotations

import json
import math
import sys
from dataclasses import dataclass, field
from typing import Any, Callable, Optional, Sequence

# valid DeepSeek scores: fatal / minor-gap / complete
SCORE_SET = (0.0, 0.5, 1.0)


@dataclass
class Verification:
    """One verifier pass over a proof."""

    score: float
    analysis: str = ""
    reports_issue: Optional[bool] = None  # None -> derived from score < 1.0

    def issues(self) -> bool:
        if self.reports_issue is not None:
            return bool(self.reports_issue)
        return self.score < 1.0


# A verify oracle: (problem, proof) -> score | dict | Verification
VerifyOracle = Callable[[str, str], Any]
# A meta oracle: (problem, proof, verification) -> bool | {"valid": bool}
MetaOracle = Callable[[str, str, Verification], Any]


def _snap(score: Any) -> float:
    """Snap an arbitrary numeric to the nearest value in {0, 0.5, 1}."""
    try:
        s = float(score)
    except (TypeError, ValueError):
        return 1.0
    return min(SCORE_SET, key=lambda t: abs(t - s))


def coerce_verification(x: Any) -> Verification:
    """Accept a float, a dict, or a :class:`Verification`; normalize the score
    into {0, 0.5, 1}."""
    if isinstance(x, Verification):
        return Verification(_snap(x.score), x.analysis, x.reports_issue)
    if isinstance(x, dict):
        return Verification(
            _snap(x.get("score", 1.0)),
            str(x.get("analysis", "")),
            x.get("reports_issue"),
        )
    return Verification(_snap(x))


def _coerce_bool(x: Any) -> bool:
    if isinstance(x, dict):
        return bool(x.get("valid", x.get("confirmed", x.get("ok"))))
    return bool(x)


def majority_confirm(votes: Sequence[Any]) -> bool:
    """A DeepSeek meta majority: strictly more confirming than denying votes.
    An empty vote list is *not* confirmed."""
    bools = [_coerce_bool(v) for v in votes]
    return bools.count(True) > bools.count(False)


def majority_meta_confirm(passes: Sequence[Any]) -> float:
    """Auto-label one proof from ``N`` verifier passes by *verification-compute
    scaling* (DeepSeek-Math-V2 s2.5): the confirmed label flips away from the
    default ``1.0`` only when **>= ceil(N/2)** of the passes agree that the proof
    has an issue (score < 1.0). When that majority threshold is met the label is
    the **lowest** issue score reported (fatal beats minor-gap); otherwise the
    proof is confirmed correct (``1.0``).

    Each element of ``passes`` may be a float, a dict, or a :class:`Verification`
    (see :func:`coerce_verification`). An empty list is trivially ``1.0``. This is
    a SOFT auto-label from the trained/graded verifier -- the formal 3+1 gate
    remains the ground-truth oracle."""
    verifs = [coerce_verification(p) for p in passes]
    n = len(verifs)
    if n == 0:
        return 1.0
    need = math.ceil(n / 2)
    issue_scores = [v.score for v in verifs if v.issues()]
    if len(issue_scores) >= need:
        return min(issue_scores)
    return 1.0


def _meta_agreement(passes: Sequence[Verification], label: float) -> float:
    """Fraction of passes whose score matches the confirmed ``label`` (the meta
    ``R_meta`` term: how consistently the compute-scaled passes back the label)."""
    if not passes:
        return 0.0
    return sum(1 for v in passes if v.score == label) / len(passes)


def formal_oracle(verdict_fn: Callable[[str, str], Any]) -> VerifyOracle:
    """Wrap a formal verdict function ``(problem, proof) -> verdict`` into a
    verify oracle emitting a *hard* score. ``verdict`` may be a bool or a
    ``{compiled, axioms_ok}`` dict; a clean pass -> 1.0, anything else -> 0.0."""

    def _oracle(problem: str, proof: str) -> Verification:
        v = verdict_fn(problem, proof)
        if isinstance(v, dict):
            ok = bool(v.get("compiled")) and bool(v.get("axioms_ok", True))
        else:
            ok = bool(v)
        return Verification(1.0 if ok else 0.0, analysis="formal-gate", reports_issue=not ok)

    return _oracle


def auto_label(
    problem: str,
    proof: str,
    verify_oracle: VerifyOracle,
    *,
    meta_oracle: Optional[MetaOracle] = None,
    n: int = 4,
    m: int = 3,
    k: int = 1,
    formal: bool = False,
) -> dict[str, Any]:
    """Auto-label one proof (DeepSeek recipe).

    * ``formal=True``: run the oracle once; its score is a HARD label (formal
      ground truth). No meta-verification.
    * otherwise: run ``verify_oracle`` ``n`` times. For each verification that
      reports an issue, run ``meta_oracle`` ``m`` times and keep the analysis
      only if a majority confirms it (a *valid analysis*). If there are ``>= k``
      valid analyses, label the proof with the **lowest** valid score; else
      label ``1.0``.

    Returns ``{label, hard, confirmed, provenance}`` with the full audit trail.
    """
    if formal:
        v = coerce_verification(verify_oracle(problem, proof))
        return {
            "label": v.score,
            "hard": True,
            "confirmed": True,
            "provenance": {
                "oracle": "formal",
                "n_verify": 1,
                "scores": [v.score],
                "valid_analyses": 0,
                "note": "formal gate: clean verdict == hard label",
            },
        }

    verifications = [coerce_verification(verify_oracle(problem, proof)) for _ in range(max(n, 0))]
    scores = [v.score for v in verifications]
    valid_scores: list[float] = []
    meta_log: list[dict[str, Any]] = []
    for v in verifications:
        if not v.issues():
            continue
        if meta_oracle is None:
            valid = True
            votes: list[bool] = []
        else:
            votes = [_coerce_bool(meta_oracle(problem, proof, v)) for _ in range(max(m, 0))]
            valid = majority_confirm(votes)
        meta_log.append({"score": v.score, "valid": valid, "votes": votes})
        if valid:
            valid_scores.append(v.score)

    if len(valid_scores) >= max(k, 1):
        label = min(valid_scores)
        confirmed = True
    else:
        label = 1.0
        confirmed = False

    return {
        "label": label,
        "hard": False,
        "confirmed": confirmed,
        "provenance": {
            "oracle": "nl_verifier",
            "n_verify": n,
            "m_meta": m,
            "k": k,
            "scores": scores,
            "valid_analyses": len(valid_scores),
            "meta": meta_log,
        },
    }


def label_dataset(
    items: Sequence[dict[str, Any]],
    verify_oracle: VerifyOracle,
    *,
    meta_oracle: Optional[MetaOracle] = None,
    n: int = 4,
    m: int = 3,
    k: int = 1,
    formal: bool = False,
) -> dict[str, Any]:
    """Auto-label a batch of ``{problem, proof}`` items.

    Returns ``{ok, labeled, hard, confirmed, distribution, rows}`` where each row
    is ``{problem, proof, label, hard, confirmed, provenance}``. ``distribution``
    tallies rows per score in {0, 0.5, 1}.
    """
    rows: list[dict[str, Any]] = []
    distribution = {0.0: 0, 0.5: 0, 1.0: 0}
    hard = 0
    confirmed = 0
    for item in items:
        problem = str(item.get("problem", item.get("goal", "")))
        proof = str(item.get("proof", ""))
        res = auto_label(
            problem,
            proof,
            verify_oracle,
            meta_oracle=meta_oracle,
            n=n,
            m=m,
            k=k,
            formal=formal,
        )
        distribution[res["label"]] = distribution.get(res["label"], 0) + 1
        hard += 1 if res["hard"] else 0
        confirmed += 1 if res["confirmed"] else 0
        rows.append(
            {
                "problem": problem,
                "proof": proof,
                "label": res["label"],
                "hard": res["hard"],
                "confirmed": res["confirmed"],
                "provenance": res["provenance"],
            }
        )
    return {
        "ok": True,
        "labeled": len(rows),
        "hard": hard,
        "confirmed": confirmed,
        "distribution": {str(kk): vv for kk, vv in distribution.items()},
        "rows": rows,
    }


# ---------------------------------------------------------------------------
# Dataset conversion (SFT / GRPO-ready, with provenance)
# ---------------------------------------------------------------------------

def to_sft_rows(rows: Sequence[dict[str, Any]], *, threshold: float = 1.0) -> list[dict[str, Any]]:
    """Emit chat-SFT rows for proofs whose auto-label is ``>= threshold`` (only
    fully-confirmed correct proofs by default). Positives only -- the flywheel
    does not teach the model on flawed proofs."""
    out: list[dict[str, Any]] = []
    for r in rows:
        if float(r["label"]) >= threshold:
            out.append(
                {
                    "messages": [
                        {"role": "user", "content": r["problem"]},
                        {"role": "assistant", "content": r["proof"]},
                    ],
                    "meta": {"label": r["label"], "hard": r["hard"], "provenance": r["provenance"]},
                }
            )
    return out


def to_grpo_rows(rows: Sequence[dict[str, Any]]) -> list[dict[str, Any]]:
    """Emit GRPO rows ``{gold, proof, label, gold_score, verdict}``. ``verdict``
    maps a hard label onto the binary ``{compiled, axioms_ok}`` verdict the
    existing reward consumes (pass iff label == 1.0); ``gold_score`` feeds the
    new faithful-verifier reward directly."""
    out: list[dict[str, Any]] = []
    for r in rows:
        label = float(r["label"])
        out.append(
            {
                "gold": r["problem"],
                "proof": r["proof"],
                "label": label,
                "gold_score": label,
                "verdict": {"compiled": label >= 1.0, "axioms_ok": True},
                "provenance": r["provenance"],
            }
        )
    return out


# ---------------------------------------------------------------------------
# Shared chat-SFT JSONL schema.
#
# The flywheel PRODUCES this shape (``to_sft_rows`` / ``revolution``) and
# ``progress_sft.sft_finetune`` CONSUMES it. One JSON object per line::
#
#   {"messages": [{"role": "user", "content": <statement>},
#                 {"role": "assistant", "content": <proof>}],
#    "meta": {"label": <float>, "hard": <bool>, "provenance": {...}}}
#
# This is the single source of truth for the training data contract; keep the
# producer (here) and the consumer (progress_sft) in lockstep with it.
# ---------------------------------------------------------------------------
SFT_SCHEMA = "theoremata.chat-sft.v1"


# ---------------------------------------------------------------------------
# One expert-iteration revolution (offline, CPU-only).
#
#   generate candidate proofs -> verify each via a pluggable oracle -> collect
#   the verified (statement, proof) pairs -> emit SFT-ready JSONL -> report
#   round metrics (n_generated, n_verified, yield).
#
# The generator and the oracle are INJECTABLE SEAMS. The defaults are a
# deterministic mock generator and a trivial pattern oracle so the whole loop
# runs -- and is tested -- offline with no model and no GPU. In production the
# generator is the policy LLM and the oracle is the live Lean/Rocq/Isabelle 3+1
# verification gate (pass ``formal=True`` with a :func:`formal_oracle`).
# ---------------------------------------------------------------------------

# A candidate generator: statement -> sequence of candidate proof strings.
Generator = Callable[[str], Sequence[str]]


def canonical_proof(statement: str) -> str:
    """The single 'known-good' proof string for a statement (mock ground truth).
    Deterministic and content-addressed by the statement text -- the pattern the
    default oracle accepts."""
    return f"by simp -- proves: {statement}"


def mock_generator(statement: str, *, n: int = 4) -> list[str]:
    """Deterministic mock proof generator -- NO model, NO GPU.

    Emits ``n`` candidate proof strings for ``statement``: exactly one is the
    canonical (known-good) proof, the rest are reproducible distractors the
    oracle rejects. Fully deterministic given ``(statement, n)`` so the loop's
    yield is stable across runs. This is the injectable stand-in for the policy
    LLM's sampled completions."""
    n = max(n, 1)
    cands = [canonical_proof(statement)]
    for i in range(1, n):
        cands.append(f"sorry -- candidate {i} for: {statement}")
    return cands[:n]


def pattern_oracle(
    canonical: Callable[[str], str] = canonical_proof,
) -> VerifyOracle:
    """A trivial pure-Python verify oracle: score ``1.0`` iff the proof matches
    the statement's known-good pattern, else ``0.0``. Makes the flywheel testable
    offline; the real oracle is the live formal 3+1 gate (see module docstring)."""

    def _oracle(problem: str, proof: str) -> float:
        return 1.0 if proof.strip() == canonical(problem).strip() else 0.0

    return _oracle


def revolution(
    problems: Sequence[dict[str, Any]],
    *,
    generator: Optional[Generator] = None,
    verify_oracle: Optional[VerifyOracle] = None,
    meta_oracle: Optional[MetaOracle] = None,
    n_candidates: int = 4,
    n: int = 1,
    m: int = 1,
    k: int = 1,
    formal: bool = False,
    threshold: float = 1.0,
    jsonl_path: Optional[str] = None,
    round_index: int = 0,
) -> dict[str, Any]:
    """Turn ONE full expert-iteration revolution, entirely offline.

    For each problem in ``problems`` (``{statement}`` -- ``problem``/``goal`` also
    accepted), ``generator`` proposes ``n_candidates`` candidate proofs. Every
    candidate is auto-labeled by ``verify_oracle`` via the DeepSeek recipe
    (:func:`label_dataset`); candidates whose label is ``>= threshold`` become
    verified SFT positives, emitted in the shared :data:`SFT_SCHEMA` chat shape
    (optionally written to ``jsonl_path``).

    With the defaults (:func:`mock_generator` + :func:`pattern_oracle`) the loop
    is a real closed cycle that needs no model and no GPU. Returns
    ``{ok, schema, round, n_problems, n_generated, n_verified, yield,
    distribution, sft_rows, jsonl_path, written}``.
    """
    gen: Generator = generator or (lambda s: mock_generator(s, n=n_candidates))
    oracle: VerifyOracle = verify_oracle or pattern_oracle()

    items: list[dict[str, Any]] = []
    for prob in problems:
        statement = str(
            prob.get("statement", prob.get("problem", prob.get("goal", "")))
        )
        for cand in gen(statement):
            items.append({"problem": statement, "proof": str(cand)})

    n_generated = len(items)
    labeled = label_dataset(
        items,
        oracle,
        meta_oracle=meta_oracle,
        n=n,
        m=m,
        k=k,
        formal=formal,
    )
    sft_rows = to_sft_rows(labeled["rows"], threshold=threshold)
    n_verified = len(sft_rows)

    written: Optional[int] = None
    if jsonl_path is not None:
        written = write_jsonl(sft_rows, jsonl_path)

    return {
        "ok": True,
        "schema": SFT_SCHEMA,
        "round": round_index,
        "n_problems": len(problems),
        "n_generated": n_generated,
        "n_verified": n_verified,
        "yield": (n_verified / n_generated) if n_generated else 0.0,
        "distribution": labeled["distribution"],
        "sft_rows": sft_rows,
        "jsonl_path": jsonl_path,
        "written": written,
    }


def graded_revolution(
    problems: Sequence[dict[str, Any]],
    *,
    generator: Optional[Generator] = None,
    verify_oracle: Optional[VerifyOracle] = None,
    graded_verifier: Optional[VerifyOracle] = None,
    n_candidates: int = 4,
    n: int = 1,
    m: int = 1,
    k: int = 1,
    formal: bool = False,
    threshold: float = 1.0,
    n_graded: int = 4,
    r_format: float = 1.0,
    jsonl_path: Optional[str] = None,
    round_index: int = 0,
) -> dict[str, Any]:
    """A :func:`revolution` variant that scales *verification compute* to
    auto-label the HARD proofs the ground-truth oracle did not verify.

    The ground-truth path is unchanged: ``verify_oracle`` (the formal 3+1 gate)
    hard-labels every candidate and candidates with label ``>= threshold`` become
    verified SFT positives (identical to :func:`revolution`). ON TOP of that, each
    candidate the ground-truth oracle did NOT verify is run through the
    trained/graded ``graded_verifier`` ``n_graded`` times; :func:`majority_meta_confirm`
    turns those passes into a SOFT auto-label, and a graded soft reward
    ``R = R_format . R_score . R_meta`` (:func:`reward.graded_verifier_reward`) is
    attached. These soft labels never enter the hard SFT set -- the formal gate
    stays the sole ground-truth oracle; the graded verifier is a soft reward.

    ``graded_verifier`` defaults to ``verify_oracle`` (so with no soft verifier the
    hard proofs simply re-confirm as flawed). Returns the :func:`revolution` fields
    plus ``n_auto_labeled`` and ``auto_labeled`` (each row
    ``{problem, proof, soft_label, confirmed, reward, provenance}``).
    """
    from theoremata_tools.reward import faithfulness_reward, graded_verifier_reward

    gen: Generator = generator or (lambda s: mock_generator(s, n=n_candidates))
    gt_oracle: VerifyOracle = verify_oracle or pattern_oracle()
    soft_oracle: VerifyOracle = graded_verifier or gt_oracle

    items: list[dict[str, Any]] = []
    for prob in problems:
        statement = str(
            prob.get("statement", prob.get("problem", prob.get("goal", "")))
        )
        for cand in gen(statement):
            items.append({"problem": statement, "proof": str(cand)})

    n_generated = len(items)
    labeled = label_dataset(items, gt_oracle, n=n, m=m, k=k, formal=formal)
    sft_rows = to_sft_rows(labeled["rows"], threshold=threshold)
    n_verified = len(sft_rows)

    auto_labeled: list[dict[str, Any]] = []
    for row in labeled["rows"]:
        if float(row["label"]) >= threshold:
            continue  # already a hard-verified positive; nothing to auto-label
        passes = [
            coerce_verification(soft_oracle(row["problem"], row["proof"]))
            for _ in range(max(n_graded, 1))
        ]
        soft_label = majority_meta_confirm(passes)
        r_meta = _meta_agreement(passes, soft_label)
        agreements = [faithfulness_reward(v.score, soft_label) or 0.0 for v in passes]
        r_score = sum(agreements) / len(agreements) if agreements else 0.0
        reward = graded_verifier_reward(r_format, r_score, r_meta)
        need = math.ceil(len(passes) / 2)
        confirmed = sum(1 for v in passes if v.score == soft_label) >= need
        auto_labeled.append(
            {
                "problem": row["problem"],
                "proof": row["proof"],
                "soft_label": soft_label,
                "confirmed": confirmed,
                "reward": reward,
                "provenance": {
                    "oracle": "graded_soft",
                    "n_graded": n_graded,
                    "scores": [v.score for v in passes],
                    "r_score": r_score,
                    "r_meta": r_meta,
                    "note": "soft auto-label; formal gate remains ground truth",
                },
            }
        )

    written: Optional[int] = None
    if jsonl_path is not None:
        written = write_jsonl(sft_rows, jsonl_path)

    return {
        "ok": True,
        "schema": SFT_SCHEMA,
        "round": round_index,
        "n_problems": len(problems),
        "n_generated": n_generated,
        "n_verified": n_verified,
        "n_auto_labeled": len(auto_labeled),
        "yield": (n_verified / n_generated) if n_generated else 0.0,
        "distribution": labeled["distribution"],
        "sft_rows": sft_rows,
        "auto_labeled": auto_labeled,
        "jsonl_path": jsonl_path,
        "written": written,
    }


def dry_run(
    items: Sequence[dict[str, Any]],
    verify_oracle: VerifyOracle,
    *,
    meta_oracle: Optional[MetaOracle] = None,
    **kwargs: Any,
) -> dict[str, Any]:
    """Validate the flywheel end-to-end offline: label the batch, then confirm
    both conversions produce well-formed rows. No GPU, no trainer."""
    result = label_dataset(items, verify_oracle, meta_oracle=meta_oracle, **kwargs)
    sft = to_sft_rows(result["rows"])
    grpo = to_grpo_rows(result["rows"])
    return {
        "ok": True,
        "dry_run": True,
        "labeled": result["labeled"],
        "hard": result["hard"],
        "confirmed": result["confirmed"],
        "distribution": result["distribution"],
        "sft_rows": len(sft),
        "grpo_rows": len(grpo),
    }


def write_jsonl(rows, path: str) -> int:
    count = 0
    with open(path, "w", encoding="utf-8") as fh:
        for row in rows:
            fh.write(json.dumps(row, ensure_ascii=False))
            fh.write("\n")
            count += 1
    return count


# ---------------------------------------------------------------------------
# Worker dispatch. NOTE: a real oracle cannot cross the JSON boundary, so the
# JSON ``run`` supports only the "already produced verifications" mode: each
# item carries ``verifications: [score,...]`` (and optional ``meta`` votes),
# and a table oracle replays them. For live oracles call the Python API.
# ---------------------------------------------------------------------------

@dataclass
class _ReplayOracle:
    """Replays pre-computed verifications per problem id (for JSON/offline use)."""

    table: dict[str, list[Any]]
    _cursor: dict[str, int] = field(default_factory=dict)

    def __call__(self, problem: str, proof: str) -> Any:
        seq = self.table.get(problem, [1.0])
        i = self._cursor.get(problem, 0)
        val = seq[min(i, len(seq) - 1)]
        self._cursor[problem] = i + 1
        return val


def run(request: dict[str, Any]) -> dict[str, Any]:
    op = request.get("op", "label")
    items = request.get("items", [])
    # Build a replay oracle from each item's inlined verifications.
    table = {str(it.get("problem", it.get("goal", ""))): it.get("verifications", [1.0]) for it in items}
    oracle = _ReplayOracle(table)
    kwargs = {
        "n": int(request.get("n", 4)),
        "m": int(request.get("m", 3)),
        "k": int(request.get("k", 1)),
        "formal": bool(request.get("formal", False)),
    }
    if op in ("label", "dry_run"):
        result = (dry_run if op == "dry_run" else label_dataset)(items, oracle, **kwargs)
        if op == "label" and request.get("emit") == "sft":
            result = dict(result)
            result["sft_rows"] = to_sft_rows(result["rows"])
        elif op == "label" and request.get("emit") == "grpo":
            result = dict(result)
            result["grpo_rows"] = to_grpo_rows(result["rows"])
        return result
    if op == "revolution":
        # Offline mock loop: the default generator + pattern oracle cannot cross
        # the JSON boundary, so this replays the deterministic mock seams.
        res = revolution(
            request.get("problems", request.get("items", [])),
            n_candidates=int(request.get("n_candidates", 4)),
            n=int(request.get("n", 1)),
            m=int(request.get("m", 1)),
            k=int(request.get("k", 1)),
            formal=bool(request.get("formal", False)),
            threshold=float(request.get("threshold", 1.0)),
            jsonl_path=request.get("jsonl_path"),
            round_index=int(request.get("round", 0)),
        )
        if not request.get("with_rows"):
            res = {kk: vv for kk, vv in res.items() if kk != "sft_rows"}
        return res
    if op == "graded_revolution":
        # Offline mock loop: defaults (mock_generator + pattern_oracle) cannot
        # cross the JSON boundary, so this replays the deterministic mock seams.
        res = graded_revolution(
            request.get("problems", request.get("items", [])),
            n_candidates=int(request.get("n_candidates", 4)),
            n=int(request.get("n", 1)),
            m=int(request.get("m", 1)),
            k=int(request.get("k", 1)),
            formal=bool(request.get("formal", False)),
            threshold=float(request.get("threshold", 1.0)),
            n_graded=int(request.get("n_graded", 4)),
            jsonl_path=request.get("jsonl_path"),
            round_index=int(request.get("round", 0)),
        )
        if not request.get("with_rows"):
            res = {kk: vv for kk, vv in res.items() if kk != "sft_rows"}
        return res
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
