"""LM-as-scorer retrieval reranker (Tier 3, item 15).

Second-stage reranker for the Mathlib retrieval stack. The first stage
(``retrieval.retrieve`` / ``retrieval.run``) produces a lexically-ranked
candidate list of declarations for a proof obligation. This module reorders
those candidates by a **model relevance score** using the *LM-as-scorer* /
*zero-shot generative classifier* method from AutoMathText (arXiv 2402.07625,
ACL 2025 Findings).

The method
----------
Instead of a fine-tuned classifier, we prompt a base model with a yes/no
question -- "Is lemma L relevant/useful for proving goal G?" -- and turn the
model's affirmative signal into a scalar in ``[0, 1]``:

* **logprobs** (preferred, when the provider exposes them): read the token
  probabilities for the affirmative vs. negative answer and normalise
  ``P(yes) / (P(yes) + P(no))`` -- see ``affirmative_probability_from_logprobs``.
* **robust yes/no parse** (fallback): parse the model's structured/yes-no reply
  into a boolean; optionally take several samples and use the affirmative
  fraction as a self-consistency probability.

Our provider protocol (``theoremata_tools.model_provider``) returns parsed,
schema-conforming JSON rather than raw logprobs, so the default provider-backed
scorer uses the parse + self-consistency path; a logprobs-based scorer can be
injected via the ``scorer`` argument for providers that expose logits.

Reusability note
----------------
The *same* LM-as-scorer primitive (affirmative-token probability of a "is this
worth keeping" question) is directly reusable for **SFT-data curation** -- score
generated proof traces / retrieved snippets and filter the low-scoring ones
before they enter the SFT set (per ``docs/resource-mining``). This module
deliberately implements **only the retrieval reranker**; the scoring helpers
(``score_candidate``, ``affirmative_probability_from_logprobs``, ``parse_yes_no``)
are factored so a curation path can reuse them later.

Design contract
---------------
* Drop-in second stage: ``rerank(query, candidates, k) -> {..., "results": [...]}``.
* Model-agnostic via the provider protocol, with a **MOCK mode**
  (``THEOREMATA_MODEL_MOCK=1``) so tests run fully offline.
* Scores are **cached** by ``(query, candidate)`` hash to avoid rescoring.
* **Degrades to identity** (input order preserved) when the model is
  unavailable, and says so (``degraded=True`` + a ``reason``).

Standard-library only; imports fine with neither litellm nor a model present.
"""
from __future__ import annotations

import hashlib
import json
import math
import os
from typing import Any, Callable, Optional

from . import retrieval as _retrieval

# A scorer maps (query, candidate_record) -> a ScoreResult dict:
#   {"affirmative_prob": float in [0,1], "relevant": bool,
#    "method": str, "model": str, "samples": int}
ScoreResult = dict[str, Any]
Scorer = Callable[[str, dict[str, Any]], ScoreResult]


# --------------------------------------------------------------------------- #
# Yes/no parsing + logprob -> probability helpers (reusable primitives).
# --------------------------------------------------------------------------- #
_AFFIRMATIVE = {"yes", "y", "true", "relevant", "useful", "1", "affirmative"}
_NEGATIVE = {"no", "n", "false", "irrelevant", "unrelated", "useless", "0"}


def parse_yes_no(text: Any) -> Optional[bool]:
    """Best-effort boolean from a model reply.

    Accepts real booleans, ``"yes"``/``"no"`` style strings (leading token wins),
    or JSON like ``{"relevant": true}``. Returns ``None`` when undecidable.
    """
    if isinstance(text, bool):
        return text
    if text is None:
        return None
    if isinstance(text, dict):
        for key in ("relevant", "answer", "yes", "verdict", "useful"):
            if key in text:
                return parse_yes_no(text[key])
        return None
    s = str(text).strip().lower()
    if not s:
        return None
    # Whole-string fast path.
    if s in _AFFIRMATIVE:
        return True
    if s in _NEGATIVE:
        return False
    # Leading-token scan (handles "Yes, because ..." / "no - the lemma ...").
    for tok in _tokenize_words(s):
        if tok in _AFFIRMATIVE:
            return True
        if tok in _NEGATIVE:
            return False
    return None


def _tokenize_words(s: str) -> list[str]:
    out: list[str] = []
    cur: list[str] = []
    for ch in s:
        if ch.isalnum():
            cur.append(ch)
        elif cur:
            out.append("".join(cur))
            cur = []
    if cur:
        out.append("".join(cur))
    return out


def affirmative_probability_from_logprobs(
    logprobs: dict[str, float] | list[tuple[str, float]],
) -> Optional[float]:
    """Normalise ``P(yes) / (P(yes) + P(no))`` from a token->logprob mapping.

    ``logprobs`` maps candidate first-token strings to their log-probabilities
    (natural log), e.g. ``{"Yes": -0.11, "No": -2.3}``. Tokens are matched
    case-insensitively against the affirmative/negative vocabularies and their
    probabilities summed. Returns ``None`` if neither side is represented.
    """
    items = logprobs.items() if isinstance(logprobs, dict) else logprobs
    p_yes = 0.0
    p_no = 0.0
    for token, lp in items:
        tok = str(token).strip().lower()
        prob = math.exp(lp)
        if tok in _AFFIRMATIVE:
            p_yes += prob
        elif tok in _NEGATIVE:
            p_no += prob
    total = p_yes + p_no
    if total <= 0.0:
        return None
    return p_yes / total


# --------------------------------------------------------------------------- #
# Candidate text + cache keying.
# --------------------------------------------------------------------------- #
def _candidate_record(cand: Any) -> dict[str, Any]:
    """Coerce a candidate into a dict record (tolerating bare strings)."""
    if isinstance(cand, dict):
        return cand
    return {"name": str(cand)}


def candidate_text(cand: dict[str, Any]) -> str:
    """Human/model-readable text for a candidate declaration.

    Uses whatever fields are present: a pretty statement/type/signature if the
    retrieval record carries one, otherwise the declaration name (+ module)."""
    parts: list[str] = []
    name = cand.get("name")
    if name:
        parts.append(str(name))
    for key in ("statement", "type", "signature", "conclusion"):
        val = cand.get(key)
        if val:
            parts.append(str(val))
    module = cand.get("module")
    if module and not any(cand.get(k) for k in ("statement", "type", "signature")):
        parts.append(f"(in {module})")
    return "  ".join(parts) if parts else ""


def _candidate_id(cand: dict[str, Any]) -> str:
    """Stable identity for caching: name if present, else the full text."""
    return str(cand.get("name") or candidate_text(cand))


def cache_key(query: str, cand: dict[str, Any]) -> str:
    """SHA-256 over ``(query, candidate identity + text)``."""
    payload = json.dumps(
        [query, _candidate_id(cand), candidate_text(cand)], sort_keys=True
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


class ScoreCache:
    """Tiny in-memory ``(query, candidate)`` -> ScoreResult cache."""

    def __init__(self) -> None:
        self._store: dict[str, ScoreResult] = {}

    def get(self, key: str) -> Optional[ScoreResult]:
        return self._store.get(key)

    def set(self, key: str, value: ScoreResult) -> None:
        self._store[key] = value

    def __len__(self) -> int:  # pragma: no cover - trivial
        return len(self._store)


_DEFAULT_CACHE = ScoreCache()


# --------------------------------------------------------------------------- #
# Scorers.
# --------------------------------------------------------------------------- #
def mock_scorer(query: str, cand: dict[str, Any]) -> ScoreResult:
    """Offline, deterministic stand-in for the LM-as-scorer.

    Produces an affirmative probability from lexical overlap between the query
    and the candidate (reusing the retrieval tokeniser). No network, no model:
    a clearly-relevant lemma (shared tokens) scores high, an unrelated one low,
    which is exactly what the reranker's ``THEOREMATA_MODEL_MOCK`` path needs.
    """
    q_tokens = set(_retrieval.tokenize(query))
    c_tokens = set(_retrieval.tokenize(candidate_text(cand)))
    if not q_tokens:
        prob = 0.5
    else:
        overlap = len(q_tokens & c_tokens) / len(q_tokens)
        # 0 overlap -> 0.1 (not relevant); full overlap -> 0.95.
        prob = 0.1 + 0.85 * overlap
    return {
        "affirmative_prob": round(prob, 6),
        "relevant": prob >= 0.5,
        "method": "mock",
        "model": "mock",
        "samples": 1,
    }


_RERANK_SCHEMA = {
    "type": "object",
    "required": ["relevant"],
    "properties": {
        "relevant": {"type": "boolean"},
        "confidence": {"type": "number"},
    },
}

_RERANK_TASK = (
    "You are a retrieval relevance judge. Given a proof GOAL and a candidate "
    "LEMMA, decide whether the lemma is plausibly relevant or useful for proving "
    "the goal. Answer with relevant=true or relevant=false, and an optional "
    "confidence in [0,1]."
)


def make_provider_scorer(
    samples: int = 1, env: Optional[dict[str, str]] = None
) -> Scorer:
    """Build a scorer backed by ``model_provider.generate`` (generative classifier).

    Asks the model the yes/no relevance question with a small structured schema
    and parses the boolean (self-consistency over ``samples`` calls when
    ``samples > 1``). Raises on the first call if the provider is unavailable so
    ``rerank`` can degrade to identity.
    """
    samples = max(1, int(samples))

    def _score(query: str, cand: dict[str, Any]) -> ScoreResult:
        from theoremata_tools import model_provider  # lazy: keeps import light

        request = {
            "role": "retrieval_reranker",
            "task": _RERANK_TASK,
            "context": {"goal": query, "lemma": candidate_text(cand)},
            "output_schema": _RERANK_SCHEMA,
        }
        affirmatives = 0
        conf_sum = 0.0
        conf_n = 0
        model_used = "unknown"
        for _ in range(samples):
            content, model_used = model_provider.generate(request, env=env)
            verdict = parse_yes_no(content)
            if verdict is True:
                affirmatives += 1
            conf = content.get("confidence") if isinstance(content, dict) else None
            if isinstance(conf, (int, float)):
                conf_sum += float(conf)
                conf_n += 1
        prob = affirmatives / samples
        # If the model volunteered calibrated confidence, blend it in lightly.
        if conf_n:
            prob = 0.5 * prob + 0.5 * (conf_sum / conf_n)
        return {
            "affirmative_prob": round(prob, 6),
            "relevant": prob >= 0.5,
            "method": "self_consistency" if samples > 1 else "parse",
            "model": model_used,
            "samples": samples,
        }

    return _score


def score_candidate(
    query: str,
    cand: Any,
    *,
    scorer: Optional[Scorer] = None,
    env: Optional[dict[str, str]] = None,
) -> ScoreResult:
    """Score a single candidate (convenience wrapper; reusable for curation)."""
    record = _candidate_record(cand)
    if scorer is None:
        scorer = _default_scorer(env)[0]
    if scorer is None:
        raise RuntimeError("no scorer available")
    return scorer(query, record)


def _default_scorer(
    env: Optional[dict[str, str]], samples: int = 1
) -> tuple[Optional[Scorer], str]:
    """Resolve the default scorer + a label. ``(None, reason)`` => degrade."""
    env = os.environ if env is None else env
    if env.get("THEOREMATA_MODEL_MOCK") == "1":
        return mock_scorer, "mock"
    return make_provider_scorer(samples=samples, env=env), "provider"


# --------------------------------------------------------------------------- #
# Public reranker.
# --------------------------------------------------------------------------- #
def _identity_results(candidates: list[dict[str, Any]]) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for i, cand in enumerate(candidates):
        rec = dict(cand)
        rec["base_rank"] = i
        rec["base_score"] = cand.get("score")
        rec["rank"] = i
        out.append(rec)
    return out


def _identity_response(
    query: str,
    candidates: list[dict[str, Any]],
    k: Optional[int],
    *,
    degraded: bool,
    reason: str,
    model: Optional[str] = None,
) -> dict[str, Any]:
    results = _identity_results(candidates)
    if k is not None:
        results = results[: max(0, k)]
    return {
        "ok": True,
        "op": "rerank",
        "method": "identity",
        "query": query,
        "model": model,
        "degraded": degraded,
        "reason": reason,
        "count": len(results),
        "results": results,
    }


def rerank(
    query: str,
    candidates: Any,
    k: Optional[int] = None,
    *,
    scorer: Optional[Scorer] = None,
    samples: int = 1,
    cache: Optional[ScoreCache] = None,
    env: Optional[dict[str, str]] = None,
) -> dict[str, Any]:
    """Reorder ``candidates`` for ``query`` by LM relevance score.

    Parameters
    ----------
    query : str
        The goal / obligation statement to retrieve lemmas for.
    candidates : list
        Initial candidates from lexical/decl retrieval. Each is a dict
        (``{name, module, kind, score, ...}``); bare strings are tolerated.
    k : int, optional
        Keep only the top ``k`` reranked candidates (all if ``None``).
    scorer : callable, optional
        ``(query, candidate) -> ScoreResult``. Defaults to the mock scorer under
        ``THEOREMATA_MODEL_MOCK=1``, else a provider-backed generative classifier.
        Inject a logprobs-based scorer here for providers that expose logits.
    samples : int
        Self-consistency samples for the default provider scorer.
    cache : ScoreCache, optional
        ``(query, candidate)`` score cache (module-level default if ``None``).
    env : dict, optional
        Environment override (defaults to ``os.environ``).

    Returns
    -------
    dict
        Stable JSON contract::

            {
              "ok": true,
              "op": "rerank",
              "method": "generative_classifier" | "identity",
              "query": "...",
              "model": "mock" | "<model-id>" | null,
              "degraded": false,
              "reason": "" ,                     # why it degraded, if it did
              "count": <n returned>,
              "results": [
                {   # original candidate fields preserved, plus:
                  "name": ..., "module": ..., "kind": ..., "score": <base>,
                  "base_rank": <int>, "base_score": <float|null>,
                  "affirmative_prob": <float 0..1>, "relevant": <bool>,
                  "score_method": "mock" | "parse" | "self_consistency" | ...,
                  "rank": <int, 0-based post-rerank>
                }, ...
              ]
            }

        On ``degraded=True`` the ``results`` are the input order (identity) with
        ``method="identity"``; callers get a well-formed response either way.
    """
    cand_list = [_candidate_record(c) for c in (candidates or [])]
    if not query or not cand_list:
        return _identity_response(
            query, cand_list, k, degraded=False, reason="empty query or candidates"
        )

    if cache is None:
        cache = _DEFAULT_CACHE

    if scorer is None:
        resolved, label = _default_scorer(env, samples=samples)
        scorer = resolved
        if scorer is None:
            return _identity_response(
                query, cand_list, k, degraded=True, reason=f"no scorer: {label}"
            )

    scored: list[tuple[float, int, dict[str, Any], ScoreResult]] = []
    try:
        for i, cand in enumerate(cand_list):
            key = cache_key(query, cand)
            res = cache.get(key)
            if res is None:
                res = scorer(query, cand)
                cache.set(key, res)
            prob = float(res.get("affirmative_prob", 0.0))
            scored.append((prob, i, cand, res))
    except Exception as exc:  # noqa: BLE001 -- degrade closed, never raise
        return _identity_response(
            query,
            cand_list,
            k,
            degraded=True,
            reason=f"scorer failed, returning input order: {exc}",
        )

    # Highest affirmative probability first; ties broken by original rank (stable).
    scored.sort(key=lambda t: (-t[0], t[1]))

    model = scored[0][3].get("model") if scored else None
    results: list[dict[str, Any]] = []
    for new_rank, (prob, base_rank, cand, res) in enumerate(scored):
        rec = dict(cand)
        rec["base_rank"] = base_rank
        rec["base_score"] = cand.get("score")
        rec["affirmative_prob"] = round(prob, 6)
        rec["relevant"] = bool(res.get("relevant", prob >= 0.5))
        rec["score_method"] = res.get("method", "unknown")
        rec["rank"] = new_rank
        results.append(rec)

    if k is not None:
        results = results[: max(0, k)]

    return {
        "ok": True,
        "op": "rerank",
        "method": "generative_classifier",
        "query": query,
        "model": model,
        "degraded": False,
        "reason": "",
        "count": len(results),
        "results": results,
    }


def run(
    query: str = "",
    candidates: Optional[list[dict[str, Any]]] = None,
    k: Optional[int] = None,
    *,
    samples: int = 1,
    env: Optional[dict[str, str]] = None,
) -> dict[str, Any]:
    """Thin JSON-in/JSON-out entry point (worker dispatch friendly).

    Mirrors ``retrieval.run``'s shape so a ``tool == "rerank"`` branch can call
    it directly: ``run(query, candidates, k)`` -> the ``rerank`` contract dict.
    """
    return rerank(query, candidates or [], k, samples=samples, env=env)
