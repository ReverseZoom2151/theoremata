"""MCTS-Q process supervision: train the critic half of the flywheel.

Ports AlphaMath *Process Supervision without Process*
(``docs/paper-mining/alphamath-almost-zero.md``) and the Super_MARIO mine
(``docs/resource-mining/new/super-mario.md``) into offline, CPU-runnable
scaffolding.

The gap this closes
-------------------
Our outcome-only flywheel (``theoremata_tools.flywheel``) trains a *policy* on
whole-proof labels: the formal 3+1 gate (Lean/Rocq/Isabelle compile + ``#print
axioms`` closure + kernel typecheck + soundness scan) passes (``+1``) or fails
(``-1``). It never trains a *value/critic head* because it has no per-step
signal. AlphaMath manufactures that signal for free: run search over the proof
tree, back-propagate each terminal ``±1`` verdict, and read every intermediate
node's Monte-Carlo ``Q = value_sum / visits`` -- the mean terminal reward of the
simulations through it. Those ``Q``s are dense per-step regression targets for a
value head. (The Q-backup + target extraction also live, tested, in the pure Rust
core ``components/reason/search/process_reward.rs``; this module mirrors the
labeling in Python so training data can be produced offline, and adds the
value-head trainer + step-beam.)

Joint policy + value objective (AlphaMath Eq. 6)
------------------------------------------------
``argmin_{pi,V}  -log pi(x+ | q)  +  beta * [ sum_{t in x+} (V(s_t) - Qhat)^2
                                            + sum_{t in x-} (V(s_t) - Qhat)^2 ]``

i.e. next-token NLL on **verified** proofs only, plus value-MSE on **both**
verified and refuted proofs (positives train both heads, negatives train only the
critic). This module focuses on the value/critic head: the loss is
``MSE(tanh(v), Q)`` -- ``tanh`` bounds the head to ``[-1, 1]`` to match the ``±1``
reward scale, exactly AlphaMath's ``Linear -> tanh`` value head.

Backends (honestly gated, like our other train modules)
-------------------------------------------------------
* ``fallback`` (default; no torch, deterministic) -- a closed-form ridge
  least-squares fit of ``Q`` on the state features. A cheap linear critic that
  proves the data + objective are internally consistent with no GPU.
* ``torch`` (optional) -- one real value head (``Linear -> tanh``) trained a few
  steps on ``MSE(tanh(v), Q)``. The fitted weights are stored as plain lists, so a
  model can be *queried* (:func:`predict_value`) with no torch at all.

The **formal gate stays the sole hard oracle**: everything here is soft per-step
shaping riding on top of the ``±1`` terminal verdict, never a replacement for it.
Offline / dry-run only; torch is gated behind ``importorskip`` in tests.
"""
from __future__ import annotations

import json
import sys
from typing import Any, Optional, Sequence

# Terminal rewards, identical to the Rust core's REWARD_PASS / REWARD_FAIL.
REWARD_PASS = 1.0
REWARD_FAIL = -1.0

# AlphaMath value-loss weight beta (Table 6); documented for callers wiring the
# joint objective. Unused by the value-only fit here (which optimizes the value
# term alone) but kept as the single source of truth for the blend.
VALUE_LOSS_WEIGHT_BETA = 0.1


# ---------------------------------------------------------------------------
# Tree representation + Q-backup (mirrors process_reward.rs)
# ---------------------------------------------------------------------------
#
# A tree dict is ``{"nodes": [node, ...]}`` where each node is::
#
#   {"id": int,
#    "parent": int | None,          # None only for the root
#    "terminal": float | None,      # +1/-1 gate verdict on a leaf (or "passed": bool)
#    "step_final": bool,            # a step boundary -> emit a Q target here
#    "value_estimate": float,       # value head's direct prediction (for step-beam)
#    "features": [float, ...]}      # state features for value-head training
#
# Only ``id`` and ``parent`` are required; the rest default sensibly.


def _terminal_reward(node: dict[str, Any]) -> Optional[float]:
    """The ``±1`` gate reward of a leaf, or ``None`` for an internal node.
    Accepts either an explicit ``terminal`` float or a ``passed`` bool."""
    if node.get("terminal") is not None:
        return float(node["terminal"])
    if "passed" in node:
        return REWARD_PASS if node["passed"] else REWARD_FAIL
    return None


def backup_q(tree: dict[str, Any]) -> dict[int, dict[str, float]]:
    """Monte-Carlo Q-backup: propagate every terminal ``±1`` up to the root.

    Each terminal leaf is one simulation whose reward is its gate verdict; walking
    leaf -> root, every ancestor gets ``visits += 1`` and ``value_sum += reward``
    (AlphaMath ``update_recursive``). Returns ``{node_id: {"visits", "value_sum",
    "q"}}`` where ``q = value_sum / visits`` -- so a node on two winning and one
    losing path scores ``q = 1/3``. Pure and deterministic (nodes indexed by id;
    sum order is irrelevant)."""
    nodes = {int(n["id"]): n for n in tree.get("nodes", [])}
    acc: dict[int, dict[str, float]] = {
        nid: {"visits": 0.0, "value_sum": 0.0, "q": 0.0} for nid in nodes
    }
    for nid, node in nodes.items():
        reward = _terminal_reward(node)
        if reward is None:
            continue
        cur: Optional[int] = nid
        # Walk to the root via parent links, guarding against malformed cycles.
        seen: set[int] = set()
        while cur is not None and cur in acc and cur not in seen:
            seen.add(cur)
            acc[cur]["visits"] += 1.0
            acc[cur]["value_sum"] += reward
            parent = nodes[cur].get("parent")
            cur = int(parent) if parent is not None else None
    for nid, a in acc.items():
        a["q"] = a["value_sum"] / a["visits"] if a["visits"] > 0 else 0.0
    return acc


def value_targets_from_tree(tree: dict[str, Any]) -> dict[str, Any]:
    """Extract per-node value-head regression targets from a tree dict.

    Runs :func:`backup_q`, then emits a ``(node_id, q)`` target at every
    **step-final**, visited node (mirrors ``process_reward::q_targets``). The
    node's ``features`` ride along as the training input. Returns
    ``{"q_by_node", "visits", "targets"}`` where ``targets`` is a list of
    ``{"node_id", "q", "features"}`` in ascending id order (deterministic)."""
    acc = backup_q(tree)
    nodes = {int(n["id"]): n for n in tree.get("nodes", [])}
    targets: list[dict[str, Any]] = []
    for nid in sorted(nodes):
        node = nodes[nid]
        if not node.get("step_final"):
            continue
        if acc[nid]["visits"] <= 0:
            continue
        targets.append(
            {
                "node_id": nid,
                "q": acc[nid]["q"],
                "features": list(node.get("features", [])),
            }
        )
    return {
        "q_by_node": {nid: a["q"] for nid, a in acc.items()},
        "visits": {nid: int(a["visits"]) for nid, a in acc.items()},
        "targets": targets,
    }


# ---------------------------------------------------------------------------
# Step-level beam search (AlphaMath SBS, backup-free inference)
# ---------------------------------------------------------------------------

def step_beam_select(
    candidates: Sequence[dict[str, Any]], beam_width: int
) -> list[int]:
    """AlphaMath Step-level Beam Search selection, backup-free.

    Rank the frontier ``candidates`` (each a node dict with ``id`` and
    ``value_estimate``) by their value head's **direct** estimate ``V(s)``
    descending, keep the top ``beam_width``. The production-friendly MCTS
    approximation: no tree, no backup. Deterministic -- ties break toward the
    smaller id (mirrors ``process_reward::step_beam_select``)."""
    ranked = sorted(
        candidates,
        key=lambda n: (-float(n.get("value_estimate", 0.0)), int(n["id"])),
    )
    return [int(n["id"]) for n in ranked[: max(0, beam_width)]]


# ---------------------------------------------------------------------------
# Value-head training (critic): torch head OR closed-form least-squares fallback
# ---------------------------------------------------------------------------

def _detect_backend() -> str:
    try:
        import torch  # noqa: F401
    except Exception:  # noqa: BLE001 - any import failure => fallback
        return "fallback"
    return "torch"


def _solve_spd(a: list[list[float]], b: list[float]) -> list[float]:
    """Solve ``A x = b`` for a small symmetric system by Gaussian elimination with
    partial pivoting. Pure Python (no numpy) so the fallback is dependency-free."""
    n = len(b)
    # Augmented matrix.
    m = [row[:] + [b[i]] for i, row in enumerate(a)]
    for col in range(n):
        # Partial pivot: largest magnitude in this column.
        pivot = max(range(col, n), key=lambda r: abs(m[r][col]))
        if abs(m[pivot][col]) < 1e-12:
            continue  # singular column; leave x[col] at 0
        m[col], m[pivot] = m[pivot], m[col]
        piv = m[col][col]
        for r in range(n):
            if r == col:
                continue
            factor = m[r][col] / piv
            for c in range(col, n + 1):
                m[r][c] -= factor * m[col][c]
    return [m[i][n] / m[i][i] if abs(m[i][i]) > 1e-12 else 0.0 for i in range(n)]


def _fit_fallback(
    features: list[list[float]], q: list[float], ridge: float = 1e-6
) -> dict[str, Any]:
    """Closed-form ridge least-squares critic: fit ``Q ~ w.x + b`` via the normal
    equations ``(X^T X + lambda I) w = X^T y`` (a bias column appended). Linear,
    deterministic, no GPU -- a cheap stand-in that still separates high-Q from
    low-Q states monotonically. Prediction clips to ``[-1, 1]``."""
    dim = len(features[0]) if features else 0
    # Design matrix with a trailing bias feature of 1.0.
    xs = [row + [1.0] for row in features]
    p = dim + 1
    # Normal equations.
    ata = [[0.0] * p for _ in range(p)]
    aty = [0.0] * p
    for row, target in zip(xs, q):
        for i in range(p):
            aty[i] += row[i] * target
            for j in range(p):
                ata[i][j] += row[i] * row[j]
    for i in range(p):
        ata[i][i] += ridge
    theta = _solve_spd(ata, aty)
    return {
        "backend": "fallback",
        "dim": dim,
        "w": theta[:dim],
        "b": theta[dim] if dim < len(theta) else 0.0,
        "squash": False,  # linear head; predict clips to [-1, 1]
        "n_examples": len(q),
    }


def _fit_torch(
    features: list[list[float]], q: list[float], *, seed: int, steps: int
) -> dict[str, Any]:
    """One real value head (``Linear -> tanh``) trained on ``MSE(tanh(v), Q)`` --
    AlphaMath's value loss. Torch-only; the fitted weights are returned as plain
    lists so :func:`predict_value` needs no torch to query the head."""
    import torch

    torch.manual_seed(int(seed))
    dim = len(features[0]) if features else 0
    x = torch.tensor(features or [[0.0]], dtype=torch.float32)
    y = torch.tensor(q or [0.0], dtype=torch.float32).unsqueeze(1)
    head = torch.nn.Linear(dim, 1)
    optim = torch.optim.Adam(head.parameters(), lr=5e-2)
    for _ in range(max(1, steps)):
        optim.zero_grad()
        v = torch.tanh(head(x))  # tanh-bounded value, matches the ±1 reward scale
        loss = torch.nn.functional.mse_loss(v, y)
        loss.backward()
        optim.step()
    return {
        "backend": "torch",
        "dim": dim,
        "w": head.weight.detach().reshape(-1).tolist(),
        "b": float(head.bias.detach().reshape(-1)[0]),
        "squash": True,  # tanh head
        "n_examples": len(q),
        "final_loss": float(loss.detach()),
    }


def train_value_head(
    examples: Sequence[dict[str, Any]],
    *,
    backend: str = "auto",
    seed: int = 0,
    steps: int = 200,
) -> dict[str, Any]:
    """Fit a value/critic head on ``(state_features, q)`` pairs.

    ``examples`` are ``{"features": [float, ...], "q": float}`` (e.g. the
    ``targets`` of :func:`value_targets_from_tree`). ``backend="auto"`` uses the
    ``torch`` head when torch imports, else the closed-form ``fallback``. Both
    return a JSON-serializable model ``{backend, dim, w, b, squash, ...}`` that
    :func:`predict_value` can evaluate with no torch. The loss mirrors AlphaMath:
    value-MSE on ``tanh(v)`` vs ``Q`` (torch path); the fallback fits ``Q`` by
    ridge least squares. The empty-example case returns a zero model."""
    rows = [
        (list(map(float, e.get("features", []))), float(e["q"]))
        for e in examples
        if e.get("features") is not None and "q" in e
    ]
    if not rows:
        return {"backend": "empty", "dim": 0, "w": [], "b": 0.0, "squash": False, "n_examples": 0}
    features = [f for f, _ in rows]
    q = [t for _, t in rows]
    if backend == "auto":
        backend = _detect_backend()
    if backend == "torch":
        return _fit_torch(features, q, seed=seed, steps=steps)
    return _fit_fallback(features, q)


def predict_value(model: dict[str, Any], features: Sequence[float]) -> float:
    """Evaluate a trained value head on a state's ``features`` -> scalar in
    ``[-1, 1]``. No torch needed (weights are plain lists). Applies ``tanh`` for a
    torch head (``squash``), else clips the linear score into ``[-1, 1]``."""
    w = model.get("w", [])
    b = float(model.get("b", 0.0))
    score = b + sum(float(wi) * float(xi) for wi, xi in zip(w, features))
    if model.get("squash"):
        return math_tanh(score)
    return -1.0 if score < -1.0 else 1.0 if score > 1.0 else score


def math_tanh(x: float) -> float:
    """``tanh`` without importing the whole ``math`` namespace at call sites."""
    import math

    return math.tanh(x)


# ---------------------------------------------------------------------------
# Additive hook: per-node Q-targets feed R_score / R_meta (formal gate = oracle)
# ---------------------------------------------------------------------------

def graded_revolution_with_process(
    trees: Sequence[dict[str, Any]],
    *,
    value_model: Optional[dict[str, Any]] = None,
    r_format: float = 1.0,
) -> dict[str, Any]:
    """Additive variant/hook: turn per-node MCTS ``Q``-targets into the soft
    ``R_score`` / ``R_meta`` factors of our graded reward ``R = R_format . R_score
    . R_meta`` (``theoremata_tools.reward.graded_verifier_reward``).

    For every step-final node across ``trees`` (each labeled by :func:`backup_q`):

    * ``R_meta`` = the Monte-Carlo **confidence** that this step lies on a proof
      path, ``(q + 1) / 2`` in ``[0, 1]`` -- the backed-up win-rate. This is the
      dense, annotation-free per-step signal our outcome-only flywheel lacked;
      today ``R_meta`` comes only from a terminal outcome, here it comes from the
      step's own Q.
    * ``R_score`` = the value head's **faithfulness** to that Q,
      ``1 - |V(s) - q| / 2`` in ``[0, 1]``, where ``V(s)`` is the model's
      prediction (:func:`predict_value`) or the node's stored ``value_estimate``.
      This rewards the critic for scoring like the search-derived target -- the
      soft-reward analog of the value-MSE training loss.

    The composite ``reward = graded_verifier_reward(R_format, R_score, R_meta)``.

    Crucially this is **additive shaping only**: the formal 3+1 gate's ``±1``
    terminal verdict remains the sole hard oracle (it is what produced every Q in
    the first place); these per-step rewards never enter the hard SFT/positive set
    -- exactly the discipline of ``flywheel.graded_revolution``. Returns
    ``{ok, n_trees, n_nodes, rows}`` with a row per step-final node."""
    from theoremata_tools.reward import graded_verifier_reward

    rows: list[dict[str, Any]] = []
    for ti, tree in enumerate(trees):
        vt = value_targets_from_tree(tree)
        nodes = {int(n["id"]): n for n in tree.get("nodes", [])}
        for tgt in vt["targets"]:
            nid = tgt["node_id"]
            q = tgt["q"]
            node = nodes[nid]
            if value_model is not None and tgt["features"]:
                predicted = predict_value(value_model, tgt["features"])
            else:
                predicted = float(node.get("value_estimate", 0.0))
            r_meta = (q + 1.0) / 2.0  # MC win-rate confidence, in [0,1]
            r_score = 1.0 - abs(predicted - q) / 2.0  # value-head faithfulness, in [0,1]
            reward = graded_verifier_reward(r_format, r_score, r_meta)
            rows.append(
                {
                    "tree": ti,
                    "node_id": nid,
                    "q": q,
                    "predicted_value": predicted,
                    "r_format": float(r_format),
                    "r_score": r_score,
                    "r_meta": r_meta,
                    "reward": reward,
                    "note": "soft per-step shaping; formal ±1 gate remains the hard oracle",
                }
            )
    return {
        "ok": True,
        "n_trees": len(trees),
        "n_nodes": len(rows),
        "rows": rows,
    }


# ---------------------------------------------------------------------------
# Worker dispatch. Wire in components/tools/python/theoremata_tools/worker.py as
# tool ``"process_supervision"`` -> :func:`run` (report-only; worker.py unedited).
# ---------------------------------------------------------------------------

def run(request: dict[str, Any]) -> dict[str, Any]:
    op = request.get("op", "value_targets")
    if op == "value_targets":
        return value_targets_from_tree(request["tree"])
    if op == "backup_q":
        acc = backup_q(request["tree"])
        return {"q_by_node": {nid: a["q"] for nid, a in acc.items()}}
    if op == "step_beam":
        return {
            "selected": step_beam_select(
                request.get("candidates", []), int(request.get("beam_width", 1))
            )
        }
    if op == "train_value_head":
        return train_value_head(
            request.get("examples", []),
            backend=request.get("backend", "auto"),
            seed=int(request.get("seed", 0)),
            steps=int(request.get("steps", 200)),
        )
    if op == "graded_process":
        return graded_revolution_with_process(
            request.get("trees", []),
            value_model=request.get("value_model"),
            r_format=float(request.get("r_format", 1.0)),
        )
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
