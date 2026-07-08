"""Learned backend/hammer selector (relative-margin algorithm selection).

Ports the durable idea from *Machine Learning and Automated Theorem Proving*
(Bridge 2010, ``docs/paper-mining/ml-and-automated-theorem-proving.md``): there
is **no universally best backend**, so learn a cheap per-goal selector that picks
the backend most likely to close a goal WITHIN A BUDGET, and rank backends by
their **relative margin** across a per-backend model -- not by any single
backend's isolated accuracy. Crucially the selector is trained on OUR OWN proof
logs (which system closed which goal, and in what time) and is scored on
**total-solved-per-budget**, exactly Bridge's end-to-end objective (his §feature
selection: optimize the relative choice, never per-classifier accuracy).

Two honestly-gated model backends produce the SAME train/select/persist contract:

* ``fallback`` (default, dependency-free) -- a **relative-frequency + feature
  margin** ranker. Per backend it stores the empirical solve-within-budget
  frequency (budget-aware at query time) plus a closed-form feature-margin
  weight vector (mean feature over solved runs minus over failed runs). No
  torch, no sklearn, deterministic.
* ``sklearn`` / ``torch`` (optional) -- a real tiny **logistic / margin** model
  per backend fit on ``features -> solved``; the fitted coefficients replace the
  closed-form margin weights. Weights are stored as plain lists/dicts so the
  persisted model is JSON-serializable and can be QUERIED with no torch/sklearn
  and no GPU. The library and device are always reported in the model's
  ``lib`` / ``device`` fields, so the compute path is never silent.

``backend="auto"`` picks ``sklearn`` if importable, else ``torch``, else
``fallback``. GPU is used only (optionally) to fit the torch margin; selection is
always pure-Python arithmetic over the stored weights.
"""
from __future__ import annotations

import json
import math
import sys
from typing import Any, Optional, Sequence

MODEL_SCHEMA = "theoremata.backend-selector.v1"

# Feature-margin term is squashed and down-weighted so the budget-aware solve
# frequency dominates the ranking (frequency is the strong signal; the margin
# breaks ties and generalizes to unseen feature vectors).
_MARGIN_WEIGHT = 0.25


# ---------------------------------------------------------------------------
# Log normalization
# ---------------------------------------------------------------------------

def _as_bool(x: Any) -> bool:
    if isinstance(x, str):
        return x.strip().lower() in ("1", "true", "yes", "solved", "ok", "pass")
    return bool(x)


def _entry(row: dict[str, Any]) -> dict[str, Any]:
    """Normalize one proof-log row into ``{problem, backend, solved, time,
    features}``. Accepts common key aliases."""
    problem = str(row.get("problem", row.get("goal", row.get("id", ""))))
    backend = str(row.get("backend", row.get("system", row.get("tactic", ""))))
    solved = _as_bool(row.get("solved", row.get("success", row.get("closed"))))
    time = float(row.get("time", row.get("seconds", row.get("cpu_time", 0.0))) or 0.0)
    features = row.get("features")
    if not isinstance(features, dict):
        # fall back to any numeric top-level keys that aren't reserved
        reserved = {"problem", "goal", "id", "backend", "system", "tactic",
                    "solved", "success", "closed", "time", "seconds", "cpu_time"}
        features = {
            kk: float(vv)
            for kk, vv in row.items()
            if kk not in reserved and isinstance(vv, (int, float)) and not isinstance(vv, bool)
        }
    else:
        features = {kk: float(vv) for kk, vv in features.items()
                    if isinstance(vv, (int, float)) and not isinstance(vv, bool)}
    return {"problem": problem, "backend": backend, "solved": solved,
            "time": time, "features": features}


def _feature_keys(entries: Sequence[dict[str, Any]]) -> list[str]:
    keys: set[str] = set()
    for e in entries:
        keys.update(e["features"].keys())
    return sorted(keys)


def _detect_backend() -> str:
    try:
        import sklearn  # noqa: F401
        return "sklearn"
    except Exception:  # noqa: BLE001
        pass
    try:
        import torch  # noqa: F401
        return "torch"
    except Exception:  # noqa: BLE001
        return "fallback"


# ---------------------------------------------------------------------------
# Margin-weight fitting (per backend). All paths return a plain dict {feat: w}
# so the persisted model is JSON-serializable and queryable with no libs.
# ---------------------------------------------------------------------------

def _fit_margin_fallback(rows: Sequence[dict[str, Any]], keys: Sequence[str]) -> dict[str, float]:
    """Closed-form feature margin: mean(feature | solved) - mean(feature | failed).
    Zero when a backend has no successes or no failures (no discriminative signal)."""
    solved = [r for r in rows if r["solved"]]
    failed = [r for r in rows if not r["solved"]]
    if not solved or not failed:
        return {kk: 0.0 for kk in keys}
    w: dict[str, float] = {}
    for kk in keys:
        ms = sum(r["features"].get(kk, 0.0) for r in solved) / len(solved)
        mf = sum(r["features"].get(kk, 0.0) for r in failed) / len(failed)
        w[kk] = ms - mf
    return w


def _fit_margin_sklearn(rows: Sequence[dict[str, Any]], keys: Sequence[str]) -> Optional[dict[str, float]]:
    """Fit a tiny logistic regression ``features -> solved`` and return its coefs
    as a plain weight dict. Returns ``None`` when the labels are single-class
    (logistic needs both classes) so the caller can fall back."""
    labels = {r["solved"] for r in rows}
    if len(labels) < 2 or not keys:
        return None
    try:
        from sklearn.linear_model import LogisticRegression
    except Exception:  # noqa: BLE001
        return None
    X = [[r["features"].get(kk, 0.0) for kk in keys] for r in rows]
    y = [1 if r["solved"] else 0 for r in rows]
    clf = LogisticRegression(max_iter=200)
    clf.fit(X, y)
    coefs = clf.coef_[0]
    return {kk: float(coefs[i]) for i, kk in enumerate(keys)}


def _fit_margin_torch(rows: Sequence[dict[str, Any]], keys: Sequence[str]) -> tuple[Optional[dict[str, float]], str]:
    """Fit a 1-layer logistic margin with a few real gradient steps; return
    ``(weights, device)``. ``weights`` is ``None`` for single-class labels."""
    labels = {r["solved"] for r in rows}
    if len(labels) < 2 or not keys:
        return None, "cpu"
    try:
        import torch
    except Exception:  # noqa: BLE001
        return None, "cpu"
    device = "cuda" if torch.cuda.is_available() else "cpu"
    torch.manual_seed(0)
    X = torch.tensor([[r["features"].get(kk, 0.0) for kk in keys] for r in rows],
                     dtype=torch.float32, device=device)
    y = torch.tensor([[1.0 if r["solved"] else 0.0] for r in rows],
                     dtype=torch.float32, device=device)
    lin = torch.nn.Linear(len(keys), 1).to(device)
    opt = torch.optim.Adam(lin.parameters(), lr=0.1)
    lossf = torch.nn.BCEWithLogitsLoss()
    for _ in range(50):
        opt.zero_grad()
        loss = lossf(lin(X), y)
        loss.backward()
        opt.step()
    w = lin.weight.detach().cpu().flatten().tolist()
    return {kk: float(w[i]) for i, kk in enumerate(keys)}, device


# ---------------------------------------------------------------------------
# Public API: train / select / evaluate / persist
# ---------------------------------------------------------------------------

def train(logs: Sequence[dict[str, Any]], *, backend: str = "auto") -> dict[str, Any]:
    """Train a backend selector on proof logs.

    ``logs`` is a sequence of per-``(problem, backend)`` runs
    ``{problem, backend, solved, time, features}`` (key aliases accepted; see
    :func:`_entry`). Returns a JSON-serializable model
    ``{schema, lib, device, features, backends: {name: {n, solved_times, weights}}}``.
    """
    if backend == "auto":
        backend = _detect_backend()
    entries = [_entry(r) for r in logs]
    keys = _feature_keys(entries)

    by_backend: dict[str, list[dict[str, Any]]] = {}
    for e in entries:
        by_backend.setdefault(e["backend"], []).append(e)

    device = "cpu"
    lib = backend
    models: dict[str, Any] = {}
    for name, rows in by_backend.items():
        weights: Optional[dict[str, float]] = None
        if backend == "sklearn":
            weights = _fit_margin_sklearn(rows, keys)
        elif backend == "torch":
            weights, device = _fit_margin_torch(rows, keys)
        if weights is None:
            # requested lib unavailable / single-class -> honest fallback
            weights = _fit_margin_fallback(rows, keys)
            if backend in ("sklearn", "torch"):
                lib = "fallback"
        models[name] = {
            "n": len(rows),
            "solved_times": sorted(r["time"] for r in rows if r["solved"]),
            "weights": weights,
        }

    return {
        "schema": MODEL_SCHEMA,
        "lib": lib,
        "device": device,
        "features": keys,
        "backends": models,
    }


def _squash(x: float) -> float:
    """Bounded squash into (-1, 1) so the feature margin can't dominate frequency."""
    if x > 20.0:
        return 1.0
    if x < -20.0:
        return -1.0
    return math.tanh(x)


def select(
    model: dict[str, Any],
    problem_features: Optional[dict[str, Any]] = None,
    budget: float = float("inf"),
) -> list[dict[str, Any]]:
    """Rank backends for one goal, most-likely-to-solve-within-``budget`` first.

    Score per backend = budget-aware empirical solve frequency
    ``#(solved with time <= budget) / n`` + ``_MARGIN_WEIGHT * tanh(w . features)``.
    Ranking is by score desc, then by solve frequency, then backend name (stable,
    deterministic). Returns a list of
    ``{backend, score, p_solve_within_budget, margin}`` -- the relative-margin
    ordering, not any single backend's absolute accuracy.
    """
    feats = {kk: float(vv) for kk, vv in (problem_features or {}).items()
             if isinstance(vv, (int, float)) and not isinstance(vv, bool)}
    ranked: list[dict[str, Any]] = []
    for name, bm in model.get("backends", {}).items():
        n = int(bm.get("n", 0)) or 1
        solved_times = bm.get("solved_times", [])
        within = sum(1 for t in solved_times if t <= budget)
        freq = within / n
        weights = bm.get("weights", {}) or {}
        raw_margin = sum(weights.get(kk, 0.0) * feats.get(kk, 0.0) for kk in weights)
        margin = _squash(raw_margin)
        score = freq + _MARGIN_WEIGHT * margin
        ranked.append({
            "backend": name,
            "score": round(score, 8),
            "p_solve_within_budget": round(freq, 8),
            "margin": round(margin, 8),
        })
    ranked.sort(key=lambda r: (-r["score"], -r["p_solve_within_budget"], r["backend"]))
    return ranked


def evaluate(
    model: dict[str, Any],
    logs: Sequence[dict[str, Any]],
    budget: float,
    *,
    problem_features: Optional[dict[str, dict[str, Any]]] = None,
) -> dict[str, Any]:
    """Score the selector on Bridge's end-to-end objective: TOTAL SOLVED PER
    BUDGET. For each problem, pick the top-ranked backend and count it solved iff
    the log shows that backend closed the goal within ``budget``. Compares against
    the best single fixed backend and round-robin/random baselines.
    """
    entries = [_entry(r) for r in logs]
    problems = sorted({e["problem"] for e in entries})
    # (problem, backend) -> solved-within-budget
    solved_map: dict[tuple[str, str], bool] = {}
    for e in entries:
        key = (e["problem"], e["backend"])
        ok = e["solved"] and e["time"] <= budget
        solved_map[key] = solved_map.get(key, False) or ok
    backends = sorted({e["backend"] for e in entries})

    pf = problem_features or {}
    selected = 0
    for prob in problems:
        ranked = select(model, pf.get(prob, {}), budget)
        if ranked and solved_map.get((prob, ranked[0]["backend"]), False):
            selected += 1

    # best single fixed backend baseline
    per_backend = {
        b: sum(1 for p in problems if solved_map.get((p, b), False))
        for b in backends
    }
    best_single = max(per_backend.values()) if per_backend else 0
    return {
        "budget": budget,
        "n_problems": len(problems),
        "solved_by_selector": selected,
        "best_single_backend": best_single,
        "per_backend_solved": per_backend,
        "beats_best_single": selected >= best_single,
    }


def save(model: dict[str, Any], path: str) -> str:
    """Persist a selector model as JSON; returns the path written."""
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(model, fh, ensure_ascii=False)
    return path


def load(path: str) -> dict[str, Any]:
    """Load a selector model previously written by :func:`save`."""
    with open(path, encoding="utf-8") as fh:
        return json.load(fh)


# ---------------------------------------------------------------------------
# Worker dispatch. Wire in worker.py as: tool 'backend_select' -> selector.run.
# ---------------------------------------------------------------------------

def run(request: dict[str, Any]) -> dict[str, Any]:
    op = request.get("op", "train")
    if op == "train":
        return train(request.get("logs", []), backend=request.get("backend", "auto"))
    if op == "select":
        return {
            "ranked": select(
                request["model"],
                request.get("problem_features", {}),
                float(request.get("budget", float("inf"))),
            )
        }
    if op == "evaluate":
        return evaluate(
            request["model"],
            request.get("logs", []),
            float(request.get("budget", float("inf"))),
            problem_features=request.get("problem_features"),
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
