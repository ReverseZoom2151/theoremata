"""Elastic Weight Consolidation (EWC) penalty -- portable, torch-free.

Ports LeanAgent / LeanDojo-v2's continual-learning regularizer
(``docs/resource-mining/reverify/LeanCopilot-LeanAgent-LeanProgress.md``;
``retrieval/model.py:95-120``): when training a retriever / value model across
many repos or formal systems in sequence, anchor each new task to the previous
one so it does not catastrophically forget. The penalty added to the training
loss is::

    ewc_loss = lambda * sum_p  fisher[p] * (theta[p] - theta_prev[p])**2

where ``fisher[p]`` (diagonal Fisher information) measures how important each
parameter was to the previous task -- computed as accumulated squared gradients
over a frozen pass, normalized by the dataset size.

This module implements the *math* only, over plain nested-list / float
"parameter trees" (a stand-in for a state_dict). No torch, no GPU: the real
trainer supplies real tensors, but the penalty formula and Fisher accumulation
are validated here offline. Gated off by default (``lambda = 0.0``), matching
LeanAgent's default which only sets ``lambda=0.1`` under progressive training.
"""
from __future__ import annotations

import json
import sys
from dataclasses import dataclass, field
from typing import Any

# A parameter tree: a nested structure of floats / lists of floats, keyed by name.
ParamTree = dict[str, Any]

DEFAULT_LAMBDA = 0.0  # EWC off unless progressive/continual training turns it on


def _sq_diff_dot(fisher: Any, theta: Any, theta_prev: Any) -> float:
    """Recursively sum ``fisher * (theta - theta_prev)**2`` over a param subtree.
    Scalars and equally-shaped nested lists are supported."""
    if isinstance(theta, (list, tuple)):
        if not (isinstance(theta_prev, (list, tuple)) and isinstance(fisher, (list, tuple))):
            raise ValueError("mismatched param structure (list vs scalar)")
        if not (len(theta) == len(theta_prev) == len(fisher)):
            raise ValueError("mismatched param lengths")
        return sum(_sq_diff_dot(f, t, p) for f, t, p in zip(fisher, theta, theta_prev))
    f = float(fisher)
    d = float(theta) - float(theta_prev)
    return f * d * d


def ewc_penalty(
    params: ParamTree,
    prev_params: ParamTree,
    fisher: ParamTree,
    lam: float = DEFAULT_LAMBDA,
) -> float:
    """The EWC penalty ``lambda * sum fisher*(theta-theta_prev)**2``.

    Returns ``0.0`` when ``lam == 0`` (gated off) or ``fisher`` is empty (no
    previous task recorded) -- exactly LeanAgent's short-circuit. Only names
    present in *all three* trees contribute (a fresh parameter with no Fisher /
    no previous value is unconstrained)."""
    if not lam or not fisher:
        return 0.0
    total = 0.0
    for name, f in fisher.items():
        if name in params and name in prev_params:
            total += _sq_diff_dot(f, params[name], prev_params[name])
    return float(lam) * total


def _accumulate(acc: Any, grad: Any) -> Any:
    """Recursively add squared grads into an accumulator subtree."""
    if isinstance(grad, (list, tuple)):
        if acc is None:
            acc = [None] * len(grad)
        return [_accumulate(a, g) for a, g in zip(acc, grad)]
    return (0.0 if acc is None else float(acc)) + float(grad) * float(grad)


def _scale(acc: Any, denom: float) -> Any:
    if isinstance(acc, (list, tuple)):
        return [_scale(a, denom) for a in acc]
    return float(acc) / denom


def compute_fisher(grad_samples: list[ParamTree], dataset_size: int | None = None) -> ParamTree:
    """Diagonal Fisher = mean of squared per-sample gradients (LeanAgent's
    frozen-pass estimate). ``grad_samples`` is a list of gradient trees (one per
    example); accumulate ``grad**2`` per parameter and normalize by
    ``dataset_size`` (defaults to ``len(grad_samples)``). Empty input -> ``{}``."""
    if not grad_samples:
        return {}
    denom = float(dataset_size if dataset_size else len(grad_samples))
    acc: ParamTree = {}
    for grads in grad_samples:
        for name, g in grads.items():
            acc[name] = _accumulate(acc.get(name), g)
    return {name: _scale(a, denom) for name, a in acc.items()}


@dataclass
class EWCState:
    """Snapshot anchoring a new task to the previous one: the frozen previous
    parameters and their Fisher importances. ``penalty(params)`` applies the
    regularizer against the current parameters."""

    prev_params: ParamTree = field(default_factory=dict)
    fisher: ParamTree = field(default_factory=dict)
    lam: float = DEFAULT_LAMBDA

    @classmethod
    def from_task(
        cls,
        params: ParamTree,
        grad_samples: list[ParamTree],
        *,
        lam: float = DEFAULT_LAMBDA,
        dataset_size: int | None = None,
    ) -> "EWCState":
        """Snapshot after finishing a task: freeze ``params`` and estimate Fisher
        from that task's gradient samples."""
        return cls(
            prev_params={k: v for k, v in params.items()},
            fisher=compute_fisher(grad_samples, dataset_size),
            lam=lam,
        )

    def penalty(self, params: ParamTree) -> float:
        return ewc_penalty(params, self.prev_params, self.fisher, self.lam)


def run(request: dict[str, Any]) -> dict[str, Any]:
    op = request.get("op", "penalty")
    if op == "penalty":
        return {
            "op": "penalty",
            "penalty": ewc_penalty(
                request["params"],
                request["prev_params"],
                request["fisher"],
                float(request.get("lam", DEFAULT_LAMBDA)),
            ),
        }
    if op == "compute_fisher":
        return {
            "op": "compute_fisher",
            "fisher": compute_fisher(
                request["grad_samples"], request.get("dataset_size")
            ),
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
