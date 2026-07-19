"""Wolfram|Alpha Full Results / LLM API client, treated as an UNTRUSTED oracle.

This is the low-friction sibling of :mod:`.wolfram_link`. Where that module shells
out to a locally installed Wolfram Engine (exact symbolic computation, but a
heavyweight licence-gated install), this one is an HTTP call against
``api.wolframalpha.com`` needing only an AppID, with a free non-commercial tier.

The two are NOT interchangeable, and the difference is a soundness difference:

* The Engine evaluates Wolfram Language you wrote. What you asked is what it
  computes.
* Alpha evaluates a NATURAL LANGUAGE query it first has to interpret. It tells
  you so: every response can carry an ``assumptions`` element recording the
  disambiguation it chose ("x is a variable, not the unit", "log is base 10").

That interpretation layer is the hazard. A silently reinterpreted question is
exactly the statement-drift failure ``components/prover/statement_preservation.rs``
exists to catch: an answer to a neighbouring question, returned as if it were an
answer to yours. So this module NEVER returns a result without also returning the
assumptions Alpha made, and callers must treat an assumption-bearing result as an
answer to a possibly different question until they have checked it.

Nothing here can verify anything. Alpha output is a conjecture generator and a
lookup, upstream of the gate, never a step in it. The ``trusted: False`` marker is
attached at every exit for that reason.
"""
from __future__ import annotations

import json
import os
import urllib.error
import urllib.parse
import urllib.request
from typing import Any

#: AppID from https://developer.wolframalpha.com . Absent means unavailable.
APPID_ENV = "THEOREMATA_WOLFRAM_APPID"

#: Opt-in switch. This makes a NETWORK CALL to a third party carrying the query
#: text, so it is off unless an operator turns it on. A proof goal can be
#: sensitive or unpublished; sending it off-machine must be deliberate.
ENABLED_ENV = "THEOREMATA_WOLFRAM_ALPHA_ENABLED"

FULL_RESULTS_ENDPOINT = "https://api.wolframalpha.com/v2/query"
LLM_ENDPOINT = "https://www.wolframalpha.com/api/v1/llm-api"

DEFAULT_TIMEOUT_SECONDS = 20.0


def _appid() -> str | None:
    value = os.environ.get(APPID_ENV, "").strip()
    return value or None


def _enabled() -> bool:
    return os.environ.get(ENABLED_ENV, "").strip().lower() in {"1", "true", "yes", "on"}


def available() -> bool:
    """True only with an explicit opt-in AND an AppID.

    Both are required: the opt-in because this leaves the machine, the AppID
    because there is no anonymous access.
    """
    return _enabled() and _appid() is not None


def _describe_unavailable() -> str:
    if not _enabled():
        return f"Wolfram|Alpha is off; set {ENABLED_ENV}=1 to enable it"
    return f"no AppID; set {APPID_ENV} (get one at developer.wolframalpha.com)"


def unavailable_response(**extra: Any) -> dict:
    """Canonical no-access response. Keeps "we could not ask" distinct from
    "we asked and got nothing", which are different facts."""
    response = {
        "ok": False,
        "unavailable": True,
        "reason": _describe_unavailable(),
        "trusted": False,
    }
    response.update(extra)
    return response


def _get(url: str, timeout: float) -> tuple[int, str] | None:
    """Fetch a URL, returning (status, body) or None on a transport failure."""
    try:
        request = urllib.request.Request(url, headers={"User-Agent": "theoremata"})
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.status, response.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as exc:
        # An HTTP error still carries a body worth reporting (Alpha explains
        # invalid AppIDs and rate limits there).
        try:
            return exc.code, exc.read().decode("utf-8", "replace")
        except Exception:
            return exc.code, ""
    except (urllib.error.URLError, TimeoutError, OSError):
        return None


def _extract_assumptions(payload: dict) -> list[str]:
    """Pull every disambiguation Alpha applied out of a Full Results payload.

    This is the load-bearing extraction in the module. An assumption means Alpha
    answered a specific reading of an ambiguous query, and the caller has to know
    which reading before it can use the answer for anything.
    """
    out: list[str] = []
    assumptions = payload.get("queryresult", {}).get("assumptions")
    if not assumptions:
        return out
    # Alpha returns either a dict or a list here depending on cardinality.
    if isinstance(assumptions, dict):
        assumptions = assumptions.get("assumption", assumptions)
    if isinstance(assumptions, dict):
        assumptions = [assumptions]
    if not isinstance(assumptions, list):
        return out
    for item in assumptions:
        if not isinstance(item, dict):
            continue
        kind = item.get("type", "assumption")
        values = item.get("values", [])
        if isinstance(values, dict):
            values = [values]
        chosen = None
        alternatives: list[str] = []
        for value in values if isinstance(values, list) else []:
            if not isinstance(value, dict):
                continue
            desc = value.get("desc") or value.get("name") or ""
            # Alpha marks the reading it used; the rest are the roads not taken,
            # and they are what tell a caller the query was ambiguous at all.
            if chosen is None:
                chosen = desc
            else:
                alternatives.append(desc)
        if chosen:
            note = f"{kind}: interpreted as {chosen}"
            if alternatives:
                note += f" (not: {', '.join(a for a in alternatives if a)})"
            out.append(note)
    return out


def query(
    text: str,
    *,
    podids: list[str] | None = None,
    timeout: float = DEFAULT_TIMEOUT_SECONDS,
) -> dict:
    """Ask the Full Results API, in JSON, and report what it assumed.

    Returns ``{ok, unavailable, interpretation, assumptions, pods, wolfram_input,
    trusted}``. ``wolfram_input`` is Alpha's own Wolfram Language rendering of the
    query (the ``minput`` format), which is the most useful field for us: it says
    in code what Alpha thought the question was, so drift is inspectable rather
    than hidden in prose.
    """
    if not available():
        return unavailable_response()

    params = {
        "appid": _appid(),
        "input": text,
        "output": "json",
        # minput/moutput give the Wolfram Language form, which is far more
        # checkable than the rendered prose.
        "format": "plaintext,minput,moutput",
    }
    if podids:
        # Selecting by pod id is the documented-robust approach; titles vary.
        params["includepodid"] = ",".join(podids)

    url = f"{FULL_RESULTS_ENDPOINT}?{urllib.parse.urlencode(params)}"
    fetched = _get(url, timeout)
    if fetched is None:
        return {
            "ok": False,
            "unavailable": False,
            "error": "network failure reaching Wolfram|Alpha",
            "trusted": False,
        }
    status, body = fetched
    if status != 200:
        return {
            "ok": False,
            "unavailable": False,
            "error": f"Wolfram|Alpha returned HTTP {status}",
            "detail": body[:500],
            "trusted": False,
        }
    try:
        payload = json.loads(body)
    except json.JSONDecodeError:
        return {
            "ok": False,
            "unavailable": False,
            "error": "Wolfram|Alpha returned unparseable JSON",
            "trusted": False,
        }

    result = payload.get("queryresult", {})
    # `success` is Alpha's own flag for "I understood the query". A false here
    # with no error is the "did not understand" case, which is not a failure of
    # ours and must not read as an empty answer.
    understood = bool(result.get("success"))
    pods: list[dict] = []
    for pod in result.get("pods", []) or []:
        if not isinstance(pod, dict):
            continue
        texts = []
        for sub in pod.get("subpods", []) or []:
            if isinstance(sub, dict) and sub.get("plaintext"):
                texts.append(sub["plaintext"])
        pods.append({"id": pod.get("id"), "title": pod.get("title"), "text": texts})

    wolfram_input = None
    for pod in result.get("pods", []) or []:
        for sub in (pod.get("subpods", []) or []) if isinstance(pod, dict) else []:
            if isinstance(sub, dict) and sub.get("minput"):
                wolfram_input = sub["minput"]
                break
        if wolfram_input:
            break

    return {
        "ok": understood and not result.get("error"),
        "unavailable": False,
        "understood": understood,
        "interpretation": pods[0]["text"][0] if pods and pods[0]["text"] else None,
        # Always present, even when empty, so a caller cannot forget to look.
        "assumptions": _extract_assumptions(payload),
        "wolfram_input": wolfram_input,
        "pods": pods,
        # Restated at every exit: this is a lookup, not a verification.
        "trusted": False,
        "caveat": (
            "Wolfram|Alpha interprets natural language before computing. This is an "
            "unproved lookup, not a checked result, and any listed assumption means "
            "it may have answered a different question than the one intended."
        ),
    }


def run(request: dict) -> dict:
    """Worker entry point. Ops: ``available``, ``query``."""
    op = request.get("op", "available")
    if op == "available":
        return {
            "ok": True,
            "available": available(),
            "reason": None if available() else _describe_unavailable(),
        }
    if op == "query":
        text = request.get("input") or request.get("text")
        if not isinstance(text, str) or not text.strip():
            return {"ok": False, "error": "query requires a non-empty `input` string"}
        podids = request.get("podids")
        timeout = request.get("timeout", DEFAULT_TIMEOUT_SECONDS)
        try:
            timeout = float(timeout)
        except (TypeError, ValueError):
            timeout = DEFAULT_TIMEOUT_SECONDS
        return query(
            text,
            podids=podids if isinstance(podids, list) else None,
            timeout=timeout,
        )
    return {"ok": False, "error": f"unknown wolfram_alpha op: {op}"}
