"""Wolfram|Alpha Fast Query Recognizer, used ONLY as a cost triage step.

The Recognizer is a ~2-3ms classifier that answers one question: "is this string
the kind of thing Alpha can probably answer?". It is the cheap gate in front of
the expensive :mod:`.wolfram_alpha` call, so we pay for a full query only when a
sub-10ms classifier says it is likely to return something.

READ THIS BEFORE USING ANY FIELD HERE.

This module produces a ROUTING HINT. It produces nothing else. In particular:

* ``accepted: false`` means "Alpha probably cannot answer this". It says NOTHING
  about whether the statement is true, false, provable, refutable, interesting,
  or well formed. Most real theorem statements are unanswerable by Alpha, which
  is a fact about Alpha's coverage, not about mathematics. A recognizer verdict
  must never be recorded as evidence on a node, never gate a proof attempt, and
  never surface as a verdict.
* ``resultsignificancescore`` (0-100) is Alpha's confidence that its OWN answer
  would be relevant to the query. It is not a probability that a theorem is true,
  and it must never be used as one or compared against a confidence threshold
  that means anything mathematical.
* ``domain`` is a content category ("Math", "Physics", ...), a routing label.

To make misreading hard rather than merely discouraged, the response vocabulary
is deliberately cost-flavoured, not truth-flavoured: the keys are ``routing_hint``,
``worth_querying`` and ``expensive_path_skipped``, there is no ``verdict`` /
``proved`` / ``verified`` / ``refuted`` / ``confidence`` key anywhere, and every
exit carries ``trusted: False``. A caller looking for a mathematical answer in
this payload will not find a key that plausibly holds one.

The three outcomes are kept distinct on purpose, because they are different facts:

1. unavailable  -> we could not ask (no opt-in, or no AppID).
2. not accepted -> we asked the cheap classifier, it said do not bother, so the
   expensive call was SKIPPED. We did not look and find nothing; we did not look.
3. accepted     -> the expensive call is worth making. It still may find nothing.
"""
from __future__ import annotations

import json
import urllib.parse
from typing import Any

# Imported, never redefined, so the opt-in switch and AppID for the cheap gate
# and the expensive call behind it can never drift apart: enabling one enables
# both, and there is no configuration in which triage runs against a service the
# operator has not consented to.
from .wolfram_alpha import (  # noqa: F401  (re-exported for callers and tests)
    APPID_ENV,
    ENABLED_ENV,
    _appid,
    _describe_unavailable,
    _get,
    available,
    unavailable_response,
)
from . import wolfram_alpha

RECOGNIZER_ENDPOINT = "https://www.wolframalpha.com/queryrecognizer/query.jsp"

#: The whole point of triage is that it is fast. A recognizer call that takes
#: longer than a couple of seconds has already lost its reason to exist, so the
#: timeout is short and a timeout is treated as "no hint", not as "not accepted".
DEFAULT_TIMEOUT_SECONDS = 5.0

#: Documented modes. "Default" is the text mode; "Voice" tunes for speech input.
MODES = ("Default", "Voice")

_ROUTING_ONLY = (
    "Routing hint only. This is Wolfram's guess about whether ITS OWN service can "
    "answer the string, and it carries no information about whether the statement "
    "is true, false, provable, or interesting. Never record it as evidence and "
    "never let it gate a proof attempt."
)


def _score(raw: Any) -> float | None:
    """Parse resultsignificancescore, which arrives as a string in the XML form."""
    if raw is None:
        return None
    try:
        return float(raw)
    except (TypeError, ValueError):
        return None


def _as_bool(raw: Any) -> bool:
    # JSON gives a real bool; the XML-ish path gives the string "true".
    if isinstance(raw, bool):
        return raw
    return str(raw).strip().lower() == "true"


def recognize(text: str, *, mode: str = "Default", timeout: float = DEFAULT_TIMEOUT_SECONDS) -> dict:
    """Ask the cheap classifier whether Alpha is likely to answer ``text``.

    Returns ``{ok, unavailable, worth_querying, routing_hint, domain,
    relevance_score_0_100, recognizer_timing_ms, summarybox, trusted, caveat}``.

    ``worth_querying`` is a COST decision. See the module docstring: it is not a
    statement about the mathematics, and this function cannot produce one.
    """
    if not available():
        # "We could not ask" is deliberately not collapsed into "do not bother":
        # a caller that wants to run the expensive path anyway must be able to
        # tell an absent classifier from a negative one.
        return unavailable_response(worth_querying=None, routing_hint="unavailable")

    if mode not in MODES:
        mode = "Default"

    params = {
        "appid": _appid(),
        "mode": mode,
        "i": text,
        "output": "json",
    }
    url = f"{RECOGNIZER_ENDPOINT}?{urllib.parse.urlencode(params)}"
    fetched = _get(url, timeout)
    if fetched is None:
        return {
            "ok": False,
            "unavailable": False,
            "error": "network failure reaching the Wolfram query recognizer",
            # No hint at all. Not a negative hint: a dead network says nothing
            # about the query, so the caller decides whether to pay anyway.
            "worth_querying": None,
            "routing_hint": "no_hint",
            "trusted": False,
        }
    status, body = fetched
    if status != 200:
        return {
            "ok": False,
            "unavailable": False,
            "error": f"query recognizer returned HTTP {status}",
            "detail": body[:500],
            "worth_querying": None,
            "routing_hint": "no_hint",
            "trusted": False,
        }
    try:
        payload = json.loads(body)
    except json.JSONDecodeError:
        return {
            "ok": False,
            "unavailable": False,
            "error": "query recognizer returned unparseable JSON",
            "worth_querying": None,
            "routing_hint": "no_hint",
            "trusted": False,
        }

    # The documented payload nests under `query`; tolerate a flat body too, since
    # a shape change must degrade to "no hint" rather than to a false negative.
    node = payload.get("query", payload)
    if isinstance(node, list):
        node = node[0] if node else {}
    if not isinstance(node, dict):
        node = {}

    accepted = _as_bool(node.get("accepted"))
    return {
        "ok": True,
        "unavailable": False,
        # The cost decision, and the only actionable output of this module.
        "worth_querying": accepted,
        "routing_hint": "likely_answerable" if accepted else "likely_unanswerable",
        # Content category, e.g. "Math". A label for routing, not a judgement.
        "domain": node.get("domain"),
        # Named for what it actually is so it cannot be mistaken for P(true).
        "relevance_score_0_100": _score(node.get("resultsignificancescore")),
        "recognizer_timing_ms": _score(node.get("timing")),
        "summarybox": node.get("summarybox"),
        "mode": mode,
        "trusted": False,
        "caveat": _ROUTING_ONLY,
    }


def triage_then_query(
    text: str,
    *,
    mode: str = "Default",
    podids: list[str] | None = None,
    timeout: float = wolfram_alpha.DEFAULT_TIMEOUT_SECONDS,
    query_when_no_hint: bool = True,
) -> dict:
    """Run the cheap classifier first, and only pay for Alpha if it says to.

    The returned dict always carries ``triage`` (the recognizer response) and
    ``expensive_path_skipped``. When the expensive path is skipped there is no
    ``result`` key at all, because an absent result and an empty result are
    different facts and giving them the same shape is how they get confused.
    """
    triage = recognize(text, mode=mode)

    if triage.get("unavailable"):
        # Cannot ask the cheap gate; cannot ask the expensive one either, since
        # they share the same opt-in and AppID.
        return {
            "ok": False,
            "unavailable": True,
            "reason": triage.get("reason"),
            "triage": triage,
            "expensive_path_skipped": True,
            "skip_reason": "wolfram access is unavailable, so nothing was asked",
            "trusted": False,
        }

    worth = triage.get("worth_querying")
    if worth is False:
        return {
            "ok": True,
            "unavailable": False,
            "triage": triage,
            "expensive_path_skipped": True,
            # Stated in cost terms and in the negative-of-the-negative, so this
            # can never be paraphrased as "Wolfram found nothing".
            "skip_reason": (
                "the fast recognizer judged Alpha unlikely to answer this, so the "
                "expensive query was never sent. Nothing was computed and nothing "
                "was looked up: this is a spend decision about Wolfram's coverage, "
                "not a finding about the statement."
            ),
            "trusted": False,
            "caveat": _ROUTING_ONLY,
        }

    if worth is None and not query_when_no_hint:
        return {
            "ok": True,
            "unavailable": False,
            "triage": triage,
            "expensive_path_skipped": True,
            "skip_reason": "the recognizer gave no hint and the caller opted not to guess",
            "trusted": False,
            "caveat": _ROUTING_ONLY,
        }

    result = wolfram_alpha.query(text, podids=podids, timeout=timeout)
    return {
        "ok": bool(result.get("ok")),
        "unavailable": bool(result.get("unavailable")),
        "triage": triage,
        "expensive_path_skipped": False,
        "result": result,
        "trusted": False,
    }


def run(request: dict) -> dict:
    """Worker entry point. Ops: ``available``, ``recognize``, ``triage_then_query``."""
    op = request.get("op", "available")
    if op == "available":
        return {
            "ok": True,
            "available": available(),
            "reason": None if available() else _describe_unavailable(),
        }

    if op in {"recognize", "triage_then_query"}:
        text = request.get("input") or request.get("text")
        if not isinstance(text, str) or not text.strip():
            return {"ok": False, "error": f"{op} requires a non-empty `input` string"}
        mode = request.get("mode", "Default")
        if not isinstance(mode, str):
            mode = "Default"
        if op == "recognize":
            return recognize(text, mode=mode)
        podids = request.get("podids")
        return triage_then_query(
            text,
            mode=mode,
            podids=podids if isinstance(podids, list) else None,
            query_when_no_hint=bool(request.get("query_when_no_hint", True)),
        )

    return {"ok": False, "error": f"unknown wolfram_recognizer op: {op}"}
