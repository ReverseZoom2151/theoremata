"""Conjecture-discovery / pattern-mining pipeline (DeepMind "AI-guided pure
mathematics" — *exploring the beauty of pure mathematics in novel ways*;
``docs/paper-mining/deepmind-articles.md`` §5).

This is the **upstream** of proving. The rest of the harness starts from a
*given* conjecture (falsify -> novelty -> prove); this module **generates** one.
It follows the DeepMind recipe that produced the knot-theory "natural slope" and
the ~40-year combinatorial-invariance results:

1. **Generate `(object, invariant)` data** over math objects
   (:func:`build_dataset`) — sample objects, compute a feature vector per object
   and a target invariant. The object sampler / feature map / invariant map are
   **injected** (deterministic mocks in tests; integers, graphs, knots-as-feature
   -vectors, Bruhat intervals in real use).
2. **Detect whether a relationship EXISTS** (:func:`detect_relationship`) —
   supervised-learn ``features -> invariant`` and check the held-out fit beats a
   **permuted-label baseline** (does the invariant depend on the features beyond
   chance?).
3. **Attribution / saliency** (:func:`attribute`) — model-agnostic **permutation
   importance** (works for every backend incl. the fallback) plus coefficient
   saliency when the model is linear, to surface **which** features drive the
   invariant.
4. **Emit a candidate conjecture** (:func:`propose_conjecture`) — a natural-
   language relationship statement plus a machine-usable ``form`` that feeds the
   novelty checker (:mod:`theoremata_tools.novelty`) and the falsifier.

Backends follow the house **injectable-model + deterministic-fallback** pattern
(cf. :mod:`theoremata_tools.difficulty`, :mod:`theoremata_tools.retriever_train`):
scikit-learn (Ridge / LogisticRegression) or torch (a real GD-fit linear model)
are used **when importable**, otherwise a dependency-free **closed-form ridge /
logistic** fit runs fully offline. The chosen backend is always reported, never
silent. Everything is deterministic (seeds threaded through sampling, the
train/test split, the permuted baseline, and permutation importance).

Security
--------
All numeric inputs are coerced to ``float`` and never evaluated; injected
callables are the caller's own code (in-process), and any data read from
``resources/`` must be treated as **untrusted** by the caller before it reaches
:func:`run` (this module only does arithmetic on numbers). No network, stdlib
only on the fallback path.

Worker wiring: expose in ``worker.py`` as tool ``'conjecture_discovery'`` ->
:func:`run` (this module does NOT edit worker.py).
"""
from __future__ import annotations

import json
import math
import random
import sys
from typing import Any, Callable, Optional, Sequence

MODEL_SCHEMA = "theoremata.conjecture-model.v1"
CONJECTURE_SCHEMA = "theoremata.conjecture.v1"

# A relationship is "worth conjecturing" only when the held-out fit is this
# strong (R^2 for regression, accuracy for classification). Below this we emit a
# no-relationship verdict rather than a false conjecture.
STRENGTH_THRESHOLD = 0.3
# A feature is a "key driver" when its permutation importance is at least this
# fraction of the top feature's importance (and strictly positive).
KEY_FEATURE_FRACTION = 0.15


# ===========================================================================
# 1. Data generation: sample objects -> (feature rows X, invariant y)
# ===========================================================================

def build_dataset(
    object_sampler: Callable[[random.Random], Any],
    feature_fn: Callable[[Any], Any],
    invariant_fn: Callable[[Any], Any],
    *,
    seed: int = 0,
    n: int = 100,
    feature_names: Optional[Sequence[str]] = None,
) -> dict[str, Any]:
    """Sample ``n`` math objects and compute their feature rows + invariant.

    ``object_sampler(rng)`` returns one object using the seeded RNG (so sampling
    is deterministic); ``feature_fn(object)`` returns its feature vector (a
    ``dict`` of named features or a plain sequence); ``invariant_fn(object)``
    returns the target invariant (a number). These three are **injected** — in
    tests they are deterministic mocks (integers / graphs / knot feature
    vectors); in real use they wrap a math library.

    Returns ``{X, y, feature_names}`` where ``X`` is a list of equal-length float
    rows, ``y`` the list of invariants, and ``feature_names`` the column labels
    (``dict`` keys sorted for stability, else ``x1..xk`` when unnamed).
    """
    rng = random.Random(seed)
    names: Optional[list[str]] = list(feature_names) if feature_names else None
    X: list[list[float]] = []
    y: list[float] = []
    for _ in range(max(0, int(n))):
        obj = object_sampler(rng)
        feats = feature_fn(obj)
        if isinstance(feats, dict):
            if names is None:
                names = sorted(str(k) for k in feats.keys())
            row = [float(feats.get(k, feats.get(_maybe_num(k), 0.0))) for k in names]
        else:
            row = [float(v) for v in feats]
            if names is None:
                names = [f"x{i + 1}" for i in range(len(row))]
        X.append(row)
        y.append(float(invariant_fn(obj)))
    return {"X": X, "y": y, "feature_names": names or []}


def _maybe_num(key: str) -> Any:
    return key


# ===========================================================================
# Small pure-Python linear algebra (no numpy dependency on the fallback path)
# ===========================================================================

def _sigmoid(x: float) -> float:
    if x >= 0:
        z = math.exp(-x)
        return 1.0 / (1.0 + z)
    z = math.exp(x)
    return z / (1.0 + z)


def _mean(xs: Sequence[float]) -> float:
    return sum(xs) / len(xs) if xs else 0.0


def _standardize(X: Sequence[Sequence[float]]) -> tuple[list[list[float]], list[float], list[float]]:
    """Column-standardize ``X`` to zero mean / unit std (std 0 -> 1). Standardized
    features make ridge well-conditioned and make coefficient magnitudes
    comparable across features (so coefficient saliency is meaningful)."""
    n = len(X)
    m = len(X[0]) if n else 0
    means = [sum(X[i][j] for i in range(n)) / n for j in range(m)] if n else []
    stds: list[float] = []
    for j in range(m):
        var = sum((X[i][j] - means[j]) ** 2 for i in range(n)) / n if n else 0.0
        stds.append(math.sqrt(var) or 1.0)
    Z = [[(X[i][j] - means[j]) / stds[j] for j in range(m)] for i in range(n)]
    return Z, means, stds


def _solve(A: list[list[float]], b: list[float]) -> list[float]:
    """Solve ``A w = b`` by Gauss-Jordan with partial pivoting (A is small and,
    after ridge regularization, well-conditioned)."""
    n = len(A)
    M = [A[i][:] + [b[i]] for i in range(n)]
    for col in range(n):
        piv = max(range(col, n), key=lambda r: abs(M[r][col]))
        if abs(M[piv][col]) < 1e-12:
            continue
        M[col], M[piv] = M[piv], M[col]
        pv = M[col][col]
        for j in range(col, n + 1):
            M[col][j] /= pv
        for r in range(n):
            if r != col and M[r][col] != 0.0:
                f = M[r][col]
                for j in range(col, n + 1):
                    M[r][j] -= f * M[col][j]
    return [M[i][n] for i in range(n)]


def _fit_ridge(Z: Sequence[Sequence[float]], y: Sequence[float], lam: float) -> tuple[list[float], float]:
    """Closed-form ridge over standardized ``Z``: solve
    ``(Z^T Z + lam I) w = Z^T (y - mean_y)``; intercept is ``mean_y``."""
    m = len(Z[0]) if Z else 0
    mean_y = _mean(y)
    yc = [v - mean_y for v in y]
    A = [[0.0] * m for _ in range(m)]
    b = [0.0] * m
    for i in range(len(Z)):
        zi = Z[i]
        for a in range(m):
            b[a] += zi[a] * yc[i]
            for c in range(m):
                A[a][c] += zi[a] * zi[c]
    for a in range(m):
        A[a][a] += lam
    coef = _solve(A, b) if m else []
    return coef, mean_y


def _fit_logistic(
    Z: Sequence[Sequence[float]], y: Sequence[float], *, iters: int = 500, lr: float = 0.3
) -> tuple[list[float], float]:
    """Closed-form-ish logistic fit: deterministic full-batch gradient descent
    from a zero init (no RNG needed -> reproducible) over standardized ``Z``."""
    n = len(Z)
    m = len(Z[0]) if n else 0
    w = [0.0] * m
    b = 0.0
    if n == 0:
        return w, b
    for _ in range(iters):
        gw = [0.0] * m
        gb = 0.0
        for i in range(n):
            zi = Z[i]
            p = _sigmoid(b + sum(w[j] * zi[j] for j in range(m)))
            err = p - y[i]
            gb += err
            for j in range(m):
                gw[j] += err * zi[j]
        b -= lr * gb / n
        for j in range(m):
            w[j] -= lr * gw[j] / n
    return w, b


# ===========================================================================
# Backend-gated fitting. sklearn / torch when importable, else closed-form.
# Every fitted model is a plain dict carrying its own ``backend`` label; the
# fallback / torch models are JSON-serializable (queryable with no ML stack).
# ===========================================================================

def _resolve_backend(backend: str) -> str:
    if backend != "auto":
        return backend
    try:
        import sklearn  # noqa: F401
        return "sklearn"
    except Exception:  # noqa: BLE001
        pass
    try:
        import torch  # noqa: F401
        return "torch"
    except Exception:  # noqa: BLE001
        pass
    return "fallback"


def _infer_task(y: Sequence[float]) -> str:
    """Classification when ``y`` is a small set of integer labels, else
    regression. Continuous invariants (the knot/rep-theory case, and our
    noise-plus-signal tests) are regression."""
    uniq = set(y)
    if len(uniq) < 2:
        return "regression"
    if all(float(v).is_integer() for v in y) and len(uniq) <= 10 and len(uniq) * 2 <= len(y):
        return "classification"
    return "regression"


def _fit_fallback(X, y, task, seed) -> dict[str, Any]:
    Z, means, stds = _standardize(X)
    if task == "classification":
        coef, intercept = _fit_logistic(Z, y)
    else:
        coef, intercept = _fit_ridge(Z, y, lam=1.0)
    return {
        "schema": MODEL_SCHEMA, "backend": "fallback", "task": task,
        "means": means, "stds": stds, "coef": coef, "intercept": intercept,
    }


def _fit_sklearn(X, y, task, seed) -> dict[str, Any]:
    if task == "classification":
        from sklearn.linear_model import LogisticRegression
        yi = [int(round(v)) for v in y]
        est = LogisticRegression(max_iter=500, random_state=int(seed)).fit(X, yi)
        coef = [float(c) for c in est.coef_[0]]
        intercept = float(est.intercept_[0])
    else:
        from sklearn.linear_model import Ridge
        est = Ridge(alpha=1.0).fit(X, y)
        coef = [float(c) for c in est.coef_]
        intercept = float(est.intercept_)
    return {
        "schema": MODEL_SCHEMA, "backend": "sklearn", "task": task,
        "estimator": est, "coef": coef, "intercept": intercept,
    }


def _fit_torch(X, y, task, seed) -> dict[str, Any]:
    """Fit a linear model with REAL torch gradient descent, then store the
    weights as plain lists so prediction needs no torch (mirrors
    ``retriever_train._train_projection``)."""
    import torch
    import torch.nn.functional as F

    torch.manual_seed(int(seed))
    Z, means, stds = _standardize(X)
    m = len(means)
    Zt = torch.tensor(Z, dtype=torch.float32) if Z else torch.zeros((0, m))
    yt = torch.tensor(list(y), dtype=torch.float32)
    lin = torch.nn.Linear(m, 1)
    opt = torch.optim.Adam(lin.parameters(), lr=0.05)
    for _ in range(500):
        opt.zero_grad()
        out = lin(Zt).squeeze(1)
        if task == "classification":
            loss = F.binary_cross_entropy_with_logits(out, yt)
        else:
            loss = F.mse_loss(out, yt)
        loss.backward()
        opt.step()
    coef = [float(c) for c in lin.weight.detach().tolist()[0]]
    intercept = float(lin.bias.item())
    return {
        "schema": MODEL_SCHEMA, "backend": "torch", "task": task,
        "means": means, "stds": stds, "coef": coef, "intercept": intercept,
    }


def _fit(backend: str, X, y, task, seed) -> dict[str, Any]:
    """Fit with the resolved backend; degrade to the closed-form fallback if the
    optional stack raises (e.g. single-class / solver issue)."""
    if backend == "sklearn":
        try:
            return _fit_sklearn(X, y, task, seed)
        except Exception:  # noqa: BLE001
            pass
    if backend == "torch":
        try:
            return _fit_torch(X, y, task, seed)
        except Exception:  # noqa: BLE001
            pass
    return _fit_fallback(X, y, task, seed)


def _predict(model: dict[str, Any], X: Sequence[Sequence[float]]) -> list[float]:
    """Predict a continuous score per row (regression value, or class-1 score /
    probability for classification). Dispatches on the model's backend; the
    linear path needs no ML stack."""
    est = model.get("estimator")
    if est is not None:
        if model["task"] == "classification":
            return [float(p[1]) for p in est.predict_proba(X)]
        return [float(v) for v in est.predict(X)]
    means = model["means"]
    stds = model["stds"]
    coef = model["coef"]
    b = model["intercept"]
    out: list[float] = []
    for row in X:
        z = b + sum(coef[j] * ((row[j] - means[j]) / stds[j]) for j in range(len(coef)))
        out.append(_sigmoid(z) if model["task"] == "classification" else z)
    return out


def _score(y: Sequence[float], pred: Sequence[float], task: str) -> float:
    """Held-out score: accuracy (classification) or R^2 (regression; may be
    negative when the fit is worse than predicting the mean)."""
    if not y:
        return 0.0
    if task == "classification":
        correct = sum(1 for a, p in zip(y, pred) if int(p >= 0.5) == int(round(a)))
        return correct / len(y)
    mean_y = _mean(y)
    ss_tot = sum((a - mean_y) ** 2 for a in y) or 1e-12
    ss_res = sum((a - p) ** 2 for a, p in zip(y, pred))
    return 1.0 - ss_res / ss_tot


# ===========================================================================
# 2. Detect whether a relationship exists (held-out fit vs permuted baseline)
# ===========================================================================

def detect_relationship(
    X: Sequence[Sequence[float]],
    y: Sequence[float],
    *,
    backend: str = "auto",
    seed: int = 0,
    holdout: float = 0.3,
    n_permute: int = 5,
) -> dict[str, Any]:
    """Fit ``features -> invariant`` and decide whether a relationship EXISTS.

    Fits on a seeded train split and scores on the held-out split (R^2 /
    accuracy = ``strength``), then repeats with **permuted training labels**
    (``n_permute`` times, seeded) to get a chance ``baseline``. A relationship is
    reported when ``strength`` clears :data:`STRENGTH_THRESHOLD` **and** beats the
    permuted baseline — i.e. the invariant depends on the features beyond chance.

    The returned ``model`` is refit on **all** the data (best estimate for
    attribution / the conjecture). ``backend`` is one of ``sklearn`` / ``torch``
    (when importable) / ``fallback``, always reported.

    Returns ``{exists, strength, baseline, gap, task, backend, model, n}``.
    """
    X = [[float(v) for v in row] for row in X]
    y = [float(v) for v in y]
    n = len(X)
    task = _infer_task(y)
    backend = _resolve_backend(backend)

    if n < 2:
        model = _fit(backend, X, y, task, seed)
        return {
            "exists": False, "strength": 0.0, "baseline": 0.0, "gap": 0.0,
            "task": task, "backend": model["backend"], "model": model, "n": n,
        }

    idx = list(range(n))
    random.Random(seed).shuffle(idx)
    ntest = min(max(1, int(round(n * holdout))), n - 1)
    test_idx = idx[:ntest]
    train_idx = idx[ntest:] or idx
    Xtr = [X[i] for i in train_idx]
    ytr = [y[i] for i in train_idx]
    Xte = [X[i] for i in test_idx]
    yte = [y[i] for i in test_idx]

    fit_model = _fit(backend, Xtr, ytr, task, seed)
    strength = _score(yte, _predict(fit_model, Xte), task)

    baselines: list[float] = []
    for r in range(max(1, n_permute)):
        yp = ytr[:]
        random.Random(seed + 1 + r).shuffle(yp)
        pm = _fit(backend, Xtr, yp, task, seed)
        baselines.append(_score(yte, _predict(pm, Xte), task))
    baseline = _mean(baselines)
    gap = strength - baseline

    if task == "classification":
        exists = strength >= max(STRENGTH_THRESHOLD, 0.6) and gap > 0.1
    else:
        exists = strength >= STRENGTH_THRESHOLD and gap > 0.1

    final_model = _fit(backend, X, y, task, seed)
    return {
        "exists": bool(exists),
        "strength": round(float(strength), 6),
        "baseline": round(float(baseline), 6),
        "gap": round(float(gap), 6),
        "task": task,
        "backend": final_model["backend"],
        "model": final_model,
        "n": n,
    }


# ===========================================================================
# 3. Attribution / saliency: which features drive the invariant?
# ===========================================================================

def attribute(
    model: dict[str, Any],
    X: Sequence[Sequence[float]],
    y: Sequence[float],
    feature_names: Sequence[str],
    *,
    seed: int = 0,
    n_repeats: int = 5,
) -> dict[str, Any]:
    """Rank features by **permutation importance** (model-agnostic; works for the
    fallback, sklearn, and torch models alike).

    For each feature we shuffle its column ``n_repeats`` times (each seeded per
    feature -> fully deterministic) and measure the mean drop in the model's
    score. A large drop means the model relies on that feature. When the model is
    linear we also return ``coef_saliency`` (``|standardized coefficient|``) as a
    corroborating gradient-style signal.

    Returns ``{task, base_score, importances, coef_saliency}`` where
    ``importances`` is a list of ``(feature_name, importance)`` sorted descending
    (ties broken by name).
    """
    X = [[float(v) for v in row] for row in X]
    y = [float(v) for v in y]
    task = model["task"]
    n = len(X)
    m = len(feature_names)
    base = _score(y, _predict(model, X), task)

    importances: list[tuple[str, float]] = []
    for j in range(m):
        col = [X[i][j] for i in range(n)]
        frng = random.Random(seed * 100003 + j)
        drops: list[float] = []
        for _ in range(max(1, n_repeats)):
            perm = list(range(n))
            frng.shuffle(perm)
            Xp = [X[i][:] for i in range(n)]
            for i in range(n):
                Xp[i][j] = col[perm[i]]
            drops.append(base - _score(y, _predict(model, Xp), task))
        importances.append((str(feature_names[j]), _mean(drops)))

    importances.sort(key=lambda t: (-t[1], t[0]))

    coef_saliency: Optional[list[tuple[str, float]]] = None
    coef = model.get("coef")
    if coef is not None and len(coef) == m:
        coef_saliency = sorted(
            ((str(feature_names[j]), abs(float(coef[j]))) for j in range(m)),
            key=lambda t: (-t[1], t[0]),
        )

    return {
        "task": task,
        "base_score": round(float(base), 6),
        "importances": importances,
        "coef_saliency": coef_saliency,
    }


# ===========================================================================
# 4. Emit a candidate conjecture (machine-usable form feeds novelty / falsify)
# ===========================================================================

def _normalize_importances(importances: Any) -> list[tuple[str, float]]:
    out: list[tuple[str, float]] = []
    for item in importances or []:
        if isinstance(item, dict):
            out.append((str(item.get("feature")), float(item.get("importance", 0.0))))
        else:
            name, val = item
            out.append((str(name), float(val)))
    return out


def propose_conjecture(
    feature_names: Sequence[str],
    importances: Any,
    strength: float,
    *,
    target: str = "invariant",
    exists: Optional[bool] = None,
    max_features: Optional[int] = None,
) -> dict[str, Any]:
    """Turn ranked importances + held-out strength into a candidate conjecture.

    Selects the **key driver** features (importance strictly positive and at
    least :data:`KEY_FEATURE_FRACTION` of the top feature's importance), then, if
    the relationship is strong enough (``strength >= STRENGTH_THRESHOLD`` and
    ``exists`` is not ``False``), emits a natural-language ``conjecture`` and a
    machine-usable ``form``. Otherwise emits a **no-relationship** verdict (no
    false conjecture) with low confidence.

    The ``form`` is what feeds the rest of the pipeline: ``form["statement"]`` is
    a plain-text claim for :func:`theoremata_tools.novelty.novelty_check` (as its
    ``statement``, with ``key_features`` as ``methods``), and ``key_features`` +
    ``target`` give the falsifier the dependence to search for a counterexample
    against.

    Returns ``{conjecture, key_features, confidence, form, relationship}``.
    """
    ranked = _normalize_importances(importances)
    positive = [(nm, v) for nm, v in ranked if v > 0.0]
    key: list[str] = []
    if positive:
        top = positive[0][1]
        key = [nm for nm, v in positive if v >= top * KEY_FEATURE_FRACTION]
    if max_features is not None:
        key = key[: max(0, int(max_features))]

    strength = float(strength)
    confidence = round(max(0.0, min(1.0, strength)), 6)
    relationship = bool(key) and strength >= STRENGTH_THRESHOLD and exists is not False

    if relationship:
        feats = ", ".join(key)
        conjecture = (
            f"The {target} is determined primarily by feature(s) {{{feats}}} "
            f"(held-out strength {strength:.3f}); other measured features appear "
            f"not to drive it."
        )
        form = {
            "schema": CONJECTURE_SCHEMA,
            "kind": "functional_dependence",
            "target": target,
            "relation": "determined_by",
            "key_features": key,
            "all_features": [str(f) for f in feature_names],
            "strength": round(strength, 6),
            "statement": conjecture,
        }
    else:
        conjecture = (
            f"No strong dependence of the {target} on the measured features was "
            f"detected (held-out strength {strength:.3f}); no conjecture emitted."
        )
        key = []
        form = {
            "schema": CONJECTURE_SCHEMA,
            "kind": "no_relationship",
            "target": target,
            "relation": None,
            "key_features": [],
            "all_features": [str(f) for f in feature_names],
            "strength": round(strength, 6),
            "statement": conjecture,
        }

    return {
        "conjecture": conjecture,
        "key_features": key,
        "confidence": confidence,
        "form": form,
        "relationship": relationship,
    }


# ===========================================================================
# Orchestration + worker dispatch. Wire in worker.py as:
#   tool 'conjecture_discovery' -> conjecture_discovery.run
# ===========================================================================

def discover(
    X: Sequence[Sequence[float]],
    y: Sequence[float],
    feature_names: Sequence[str],
    *,
    backend: str = "auto",
    seed: int = 0,
    holdout: float = 0.3,
    n_repeats: int = 5,
    target: str = "invariant",
) -> dict[str, Any]:
    """Run stages 2-4 end to end on a prepared dataset and return a plain
    (JSON-serializable) result. The live ``model`` is used internally for
    attribution but not returned (it may hold a non-serializable estimator)."""
    det = detect_relationship(X, y, backend=backend, seed=seed, holdout=holdout)
    attr = attribute(det["model"], X, y, feature_names, seed=seed, n_repeats=n_repeats)
    prop = propose_conjecture(
        feature_names, attr["importances"], det["strength"],
        target=target, exists=det["exists"],
    )
    return {
        "op": "conjecture_discovery",
        "exists": det["exists"],
        "strength": det["strength"],
        "baseline": det["baseline"],
        "gap": det["gap"],
        "task": det["task"],
        "backend": det["backend"],
        "base_score": attr["base_score"],
        "importances": [
            {"feature": nm, "importance": round(v, 6)} for nm, v in attr["importances"]
        ],
        "coef_saliency": (
            [{"feature": nm, "saliency": round(v, 6)} for nm, v in attr["coef_saliency"]]
            if attr["coef_saliency"] is not None else None
        ),
        "conjecture": prop["conjecture"],
        "key_features": prop["key_features"],
        "confidence": prop["confidence"],
        "relationship": prop["relationship"],
        "form": prop["form"],
    }


def run(request: dict[str, Any]) -> dict[str, Any]:
    """Answer one ``conjecture_discovery`` request.

    Two input modes:

    * **Prepared data** (the JSON/worker path): ``{"op":"conjecture_discovery",
      "X":[[...],...], "y":[...], "feature_names":[...]}``.
    * **Injected samplers** (in-process): pass ``object_sampler`` / ``feature_fn``
      / ``invariant_fn`` callables (+ ``n``) and the dataset is built first via
      :func:`build_dataset`.

    Optional: ``backend`` (``auto``/``fallback``/``sklearn``/``torch``), ``seed``,
    ``holdout``, ``n_repeats``, ``target``.
    """
    op = request.get("op", "conjecture_discovery")
    if op != "conjecture_discovery":
        raise ValueError(f"unknown op: {op}")

    seed = int(request.get("seed", 0))
    backend = request.get("backend", "auto")
    target = request.get("target", "invariant")

    if request.get("X") is not None and request.get("y") is not None:
        X = [[float(v) for v in row] for row in request["X"]]
        y = [float(v) for v in request["y"]]
        names = request.get("feature_names") or [
            f"x{i + 1}" for i in range(len(X[0]) if X else 0)
        ]
    elif callable(request.get("object_sampler")):
        ds = build_dataset(
            request["object_sampler"],
            request["feature_fn"],
            request["invariant_fn"],
            seed=seed,
            n=int(request.get("n", 100)),
            feature_names=request.get("feature_names"),
        )
        X, y, names = ds["X"], ds["y"], ds["feature_names"]
    else:
        raise ValueError(
            "conjecture_discovery requires either X+y or "
            "object_sampler/feature_fn/invariant_fn callables"
        )

    return discover(
        X, y, names,
        backend=backend, seed=seed,
        holdout=float(request.get("holdout", 0.3)),
        n_repeats=int(request.get("n_repeats", 5)),
        target=target,
    )


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
