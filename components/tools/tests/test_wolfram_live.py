"""OPT-IN live validation harness for the four Wolfram modules.

This file is NOT part of the normal suite in any meaningful sense: with no
credentials every test in it is SKIPPED, and that is the expected state in CI and
on any developer machine without a licence. It exists to convert the guesses that
are documented in the Wolfram modules into observed facts, by running them once
against a real kernel and a real AppID.

Everything else in the tree tests these modules against MOCKED responses, so the
parsers have never seen a byte that Wolfram actually emitted. The open questions
this harness answers, each one owned by a test below:

* wolfram_cert.py: does the default ``FindInstance`` SOS ansatz (square count and
  degree defaults) actually solve a trivial SOS, and does our EXISTING checker
  accept what a real kernel hands back? Same question for the PolynomialReduce
  cofactors and the CountRoots interval convention.
* wolfram_falsify.py: what does ``InputForm`` of FindInstance / Reduce / NSolve
  really print, does our rule-list parser see it, and how does
  ``FindIntegerRelation`` represent failure?
* wolfram_recognizer.py: does the JSON envelope nest under a ``query`` key, and is
  ``accepted`` a real bool or the string "true"?
* wolfram_link.py: does a real kernel timeout report a string containing
  "timeout", and what are the exact cloud elision markers?

Assertions carry the RAW response in their message wherever the shape is the
thing under test, so a failure teaches the reader the actual shape rather than
just reporting a mismatch.

HOW TO RUN
----------
Engine-backed tests (wolfram_link, wolfram_falsify, wolfram_cert) need::

    export THEOREMATA_WOLFRAM_ENABLED=1
    # then EITHER a local kernel, found on PATH or pointed at explicitly:
    export THEOREMATA_WOLFRAM=/usr/local/bin/wolframscript
    # OR the CAG cloud endpoint, which needs no local install:
    export THEOREMATA_WOLFRAM_CLOUD_KEY=...

Alpha-backed tests (wolfram_alpha, wolfram_recognizer) need::

    export THEOREMATA_WOLFRAM_ALPHA_ENABLED=1
    export THEOREMATA_WOLFRAM_APPID=...          # developer.wolframalpha.com

On Windows PowerShell the same thing is ``$env:THEOREMATA_WOLFRAM_ENABLED = "1"``
and so on. Then::

    python -m pytest components/tools/tests/test_wolfram_live.py -q -s

``-s`` is worth passing: several tests print the raw response they observed, which
is the point of the exercise.

BUDGET
------
The Alpha free tier is 2000 calls a month and an engine call costs a kernel
launch, so every query here is deliberately tiny and every test makes at most one
or two calls with a short timeout.
"""
from __future__ import annotations

import json
import urllib.parse

import pytest

from theoremata_tools import wolfram_alpha, wolfram_cert, wolfram_falsify
from theoremata_tools import wolfram_link, wolfram_recognizer

# Probes are read ONCE at import, which is also when pytest evaluates skipif
# conditions. Keying the skips on the modules' own ``available()`` rather than on
# a private reading of the environment means this file skips under exactly the
# same conditions the production code degrades under: if the module would return
# an ``unavailable`` response, the test that would have exercised it is skipped.
ENGINE = wolfram_link.available()
ALPHA = wolfram_alpha.available()
TRANSPORT = wolfram_link.transport()

needs_engine = pytest.mark.skipif(
    not ENGINE,
    reason=(
        "no Wolfram Engine transport: set THEOREMATA_WOLFRAM_ENABLED=1 plus "
        "either THEOREMATA_WOLFRAM (local binary) or "
        "THEOREMATA_WOLFRAM_CLOUD_KEY"
    ),
)
needs_local_engine = pytest.mark.skipif(
    TRANSPORT != "local",
    reason="needs a LOCAL wolframscript kernel (subprocess timeout path)",
)
needs_cloud_engine = pytest.mark.skipif(
    TRANSPORT != "cloud",
    reason="needs the CAG cloud transport (THEOREMATA_WOLFRAM_CLOUD_KEY)",
)
needs_alpha = pytest.mark.skipif(
    not ALPHA,
    reason=(
        "no Wolfram|Alpha access: set THEOREMATA_WOLFRAM_ALPHA_ENABLED=1 and "
        "THEOREMATA_WOLFRAM_APPID"
    ),
)

# Short on purpose. These inputs are all trivial, so anything slower than this is
# a sign the query was mis-phrased rather than a sign it needs longer.
QUICK = 20.0


# --------------------------------------------------------------------------- #
# Group 1: wolfram_link transport behaviour.
# Open questions: does a real timeout say "timeout"? what does elision look like?
# --------------------------------------------------------------------------- #


@needs_engine
def test_link_roundtrips_a_trivial_expression():
    """Baseline. Everything below is meaningless if this does not hold.

    Also pins the two facts the rest of the file assumes about a live result: it
    is a STRING, and the transport is reported so a certificate is attributable.
    """
    response = wolfram_link.evaluate("ToString[InputForm[1 + 1]]", timeout=QUICK)
    assert response["ok"] is True, f"live evaluate failed: {response!r}"
    assert response["result"].strip() == "2", (
        f"unexpected print shape for 1+1: {response!r}"
    )
    assert response["transport"] in {"local", "cloud"}, response


@needs_local_engine
def test_local_kernel_timeout_reports_the_word_timeout():
    """RESOLVES: does a real kernel timeout produce a string containing "timeout"?

    This matters beyond cosmetics. ``wolfram_falsify._timeout_hit`` decides whether
    a run is reported as deadline-limited by substring-matching "timeout" in the
    error, and a kernel that phrases it differently would silently turn every
    timeout into an ordinary inconclusive with ``timeout_hit=False``.
    """
    # Pause[30] cannot finish inside a 2s deadline on any machine, and it costs
    # only a kernel launch.
    response = wolfram_link.evaluate("Pause[30]", timeout=2.0)
    assert response["ok"] is False, f"Pause[30] somehow returned: {response!r}"
    assert response["unavailable"] is False, response
    error = response.get("error") or ""
    assert "timeout" in error.lower(), (
        "wolfram_falsify._timeout_hit substring-matches 'timeout'; this kernel "
        f"reported a deadline as: {error!r} (full response {response!r})"
    )


@needs_cloud_engine
def test_cloud_elision_markers_are_detected():
    """RESOLVES: are the cloud endpoint's elision markers really `<<` and `>>`?

    A truncated expression re-parses as a valid SHORTER expression, so a missed
    elision is a silent-corruption bug rather than a display bug. Ask for output
    that cannot fit in CLOUD_MAX_CHARS and require the refusal to fire.
    """
    response = wolfram_link.evaluate(
        "ToString[InputForm[Range[20000]]]", timeout=QUICK
    )
    raw = response.get("result") or ""
    assert response["ok"] is False, (
        "oversized output was accepted, so either the endpoint did not elide or "
        f"the markers changed. Raw prefix: {raw[:400]!r}"
    )
    assert "elided" in (response.get("error") or ""), (
        f"expected the elision refusal, got: {response!r}"
    )
    print("observed cloud elision tail:", raw[-200:])


# --------------------------------------------------------------------------- #
# Group 2: wolfram_falsify parsers against real InputForm output.
# --------------------------------------------------------------------------- #


@needs_engine
def test_findinstance_witness_parses_and_survives_our_recheck():
    """RESOLVES: the real `InputForm[FindInstance[...]]` print shape.

    The whole untrusted-oracle argument for this module is that a reported
    counterexample was confirmed by OUR exact recheck of the ORIGINAL Python
    claim, so the assertion is on ``independently_verified``, not on the oracle
    having answered.

    The claim ``x*x != 2*x`` is false at x = 2, which the kernel finds instantly.
    """
    out = wolfram_falsify.falsify(
        {
            "variables": ["x"],
            "claim": "x*x != 2*x",
            "assumptions": "x > 1",
            "domain": "Integers",
            "method": "FindInstance",
            "max_instances": 1,
            "timeout_seconds": QUICK,
        }
    )
    raw = out.get("wolfram_result")
    assert out["verdict"] == wolfram_falsify.VERDICT_COUNTEREXAMPLE, (
        "the rule-list parser did not recover a witness from this kernel's "
        f"InputForm output. Raw result was: {raw!r} (full response {out!r})"
    )
    assert out["refuted"] is True and out["independently_verified"] is True, out
    # Our own exact recheck, not Wolfram, is what makes this admissible.
    assert out["assignment"] == {"x": "2"}, (
        f"unexpected witness {out.get('assignment')!r} from raw {raw!r}"
    )
    assert out["trusted"] is False, out
    print("observed FindInstance InputForm shape:", raw)


@needs_engine
def test_no_counterexample_yields_no_positive_verdict():
    """RESOLVES: what a genuinely unfalsifiable claim returns.

    ``x*x >= 0`` over the integers has no counterexample, so FindInstance returns
    the empty list. The requirement is negative: the module must NOT emit anything
    that reads as a pass. ``no_counterexample_found`` and ``inconclusive`` are both
    acceptable; ``refuted`` and any positive claim are not.
    """
    out = wolfram_falsify.falsify(
        {
            "variables": ["x"],
            "claim": "x*x >= 0",
            "domain": "Integers",
            "method": "FindInstance",
            "max_instances": 1,
            "timeout_seconds": QUICK,
        }
    )
    assert out["verdict"] in {
        wolfram_falsify.VERDICT_NONE_FOUND,
        wolfram_falsify.VERDICT_INCONCLUSIVE,
    }, f"unexpected verdict for an unfalsifiable claim: {out!r}"
    assert out["refuted"] is False, out
    assert out["proved"] is False, out
    assert out.get("proves") is None, out
    # A bounded heuristic search can never justify a universal claim, whatever the
    # kernel prints.
    assert out["search_exhausted"] is False, out
    print("observed empty-FindInstance shape:", out.get("wolfram_result"))


@needs_engine
def test_reduce_solution_set_never_produces_an_unverified_refutation():
    """RESOLVES: the untested `Reduce` path.

    ``Reduce`` returns a SOLUTION SET (``x == 2``), not a list of rules, so
    ``parse_witnesses`` most likely recovers nothing from it. Either outcome is
    sound, and exactly one is unacceptable: a refutation that our recheck never
    confirmed. Assert that invariant and print the real shape so the parser can be
    taught to read it if we decide the recall is worth having.
    """
    out = wolfram_falsify.falsify(
        {
            "variables": ["x"],
            "claim": "x*x != 2*x",
            "assumptions": "x > 1",
            "domain": "Integers",
            "method": "Reduce",
            "timeout_seconds": QUICK,
        }
    )
    raw = out.get("wolfram_result")
    if out["refuted"]:
        assert out["independently_verified"] is True, (
            "Reduce output produced a refutation that our exact recheck never "
            f"confirmed. Raw: {raw!r} (full response {out!r})"
        )
    else:
        assert out.get("proves") is None, out
    print("observed Reduce InputForm shape:", raw)


@needs_engine
def test_nsolve_shape_is_recorded():
    """RESOLVES: the `NSolve` print shape and whether it is exact enough to use.

    NSolve returns machine-precision numbers, which ``parse_exact`` rejects on
    sight (a backtick or ``*^`` is an immediate InexactError). So the expected
    outcome is that nothing survives, and the value of this test is the printed
    raw string telling us whether that is because of the backticks or because of
    the surrounding shape.
    """
    out = wolfram_falsify.falsify(
        {
            "variables": ["x"],
            "claim": "x*x != 2*x",
            "assumptions": "x > 1",
            "domain": "Reals",
            "method": "NSolve",
            "timeout_seconds": QUICK,
        }
    )
    raw = out.get("wolfram_result")
    if out["refuted"]:
        assert out["independently_verified"] is True, (
            f"NSolve refutation was not exactly rechecked. Raw: {raw!r}"
        )
    print("observed NSolve InputForm shape:", raw)


@needs_engine
def test_integer_relation_parses_a_real_pslq_hit():
    """RESOLVES: the coefficient parser against real FindIntegerRelation output.

    Sqrt[2] and Sqrt[8] satisfy 2*a - b == 0 exactly, so PSLQ finds it at once.
    The result must still come back as an UNPROVED conjecture: PSLQ works at
    finite precision and a relation it reports is a numerical coincidence until
    somebody proves it.
    """
    out = wolfram_falsify.integer_relation(
        {"constants": ["Sqrt[2]", "Sqrt[8]"], "precision": 30,
         "timeout_seconds": QUICK}
    )
    raw = out.get("wolfram_result")
    assert out["verdict"] == wolfram_falsify.VERDICT_CANDIDATE_RELATION, (
        "the coefficient parser did not read this kernel's FindIntegerRelation "
        f"output. Raw: {raw!r} (full response {out!r})"
    )
    assert len(out["coefficients"]) == 2, out
    assert out["proved"] is False and out["status"] == "unproved_conjecture", out
    print("observed FindIntegerRelation hit shape:", raw)


@needs_engine
def test_integer_relation_failure_representation():
    """RESOLVES: how FindIntegerRelation represents "no relation".

    Documentation says ``$Failed``, but ``$failed`` is also one of
    ``wolfram_link._FAILURE_MARKERS``, so a literal ``$Failed`` never reaches the
    coefficient parser at all: the link layer classifies it as ``ok=False`` and
    the verdict becomes ``inconclusive`` rather than ``no_relation_found``. Both
    are non-positive and therefore sound; this test records which one actually
    happens so the distinction stops being a guess.
    """
    out = wolfram_falsify.integer_relation(
        {"constants": ["Pi", "Sqrt[2]"], "precision": 30, "timeout_seconds": QUICK}
    )
    assert out["verdict"] in {
        wolfram_falsify.VERDICT_NO_RELATION,
        wolfram_falsify.VERDICT_INCONCLUSIVE,
    }, f"unexpected verdict where no relation exists: {out!r}"
    assert out.get("coefficients") in (None, []), out
    assert out["proved"] is False, out
    print(
        "observed FindIntegerRelation failure:",
        out["verdict"],
        "| raw:", out.get("wolfram_result"),
        "| reason:", out.get("reason"),
    )


# --------------------------------------------------------------------------- #
# Group 3: wolfram_cert end to end. This is the strongest test in the file:
# a real oracle produces a certificate and OUR existing checker adjudicates it.
# --------------------------------------------------------------------------- #


@needs_engine
def test_sos_default_ansatz_produces_a_certificate_our_checker_accepts():
    """RESOLVES: is the default SOS ansatz (square count / degree) usable?

    ``p = x^2 - 2x + 2 = (x - 1)^2 + 1`` is SOS over the RATIONALS with two
    degree-1 squares, which is exactly what the defaults set up
    (``num_squares=2``, ``sq_degree=max(deg//2, 1)=1``). If the defaults are
    mis-sized, FindInstance returns the empty list and this fails, which is the
    signal we want.

    The assertion is on ``check.valid``, not on Wolfram having answered. That is
    the real prize: the untrusted-oracle design validated against a live oracle,
    with ``cert_sos.check`` as the sole trust boundary.
    """
    out = wolfram_cert.run(
        {"op": "sos", "p": "x**2 - 2*x + 2", "x": "x", "timeout": QUICK}
    )
    assert out["unavailable"] is False, out
    assert out["ok"] is True, (
        "the default SOS ansatz did not survive cert_sos.check against a live "
        f"kernel: {out.get('reason')!r} (full response {out!r})"
    )
    assert out["cert"] is not None, out
    assert out["check"]["valid"] is True, out
    assert out["checked"] is True and out["trusted_without_check"] is False, out


@needs_engine
def test_nullstellensatz_cofactors_survive_our_checker():
    """RESOLVES: the PolynomialReduce cofactor shape and its exactness.

    ``x^2 - 1 = (x + 1)(x - 1)`` is ideal membership with a single cofactor, so
    the kernel returns a one-element list and our checker has to accept it. A
    float sneaking into the cofactor would be rejected by ``_require_exact``
    before the checker ever sees it, so a pass here also confirms the generated
    code really does ask for exact rationals.
    """
    out = wolfram_cert.run(
        {
            "op": "nullstellensatz",
            "gens": ["x"],
            "polys": ["x - 1"],
            "target": "x**2 - 1",
            "timeout": QUICK,
        }
    )
    assert out["unavailable"] is False, out
    assert out["ok"] is True, (
        "live PolynomialReduce cofactors were rejected: "
        f"{out.get('reason')!r} (full response {out!r})"
    )
    assert out["cert"] is not None and out["check"]["valid"] is True, out


@needs_engine
def test_sturm_count_from_countroots_matches_our_convention():
    """RESOLVES: whether CountRoots and our Sturm certificate agree in practice.

    They use genuinely different conventions (CountRoots: with multiplicity on the
    CLOSED [a, b]; our certificate: distinct roots on the half-open (a, b]), so
    the interval is chosen to make them coincide. ``x^2 - 1`` on [0, 2] has the
    single simple root 1 strictly inside, where both conventions count 1.

    A rejection here would be the module working as designed, not a bug, so the
    failure message says so.
    """
    out = wolfram_cert.run(
        {
            "op": "sturm",
            "coeffs": [-1, 0, 1],
            "interval": [0, 2],
            "var": "x",
            "timeout": QUICK,
        }
    )
    assert out["unavailable"] is False, out
    assert out["ok"] is True, (
        "cert_sturm.check rejected the live CountRoots count. If the reason is a "
        "count of 2 rather than 1 the two interval conventions have diverged on "
        f"this input, not our arithmetic: {out.get('reason')!r} (full {out!r})"
    )
    assert out["cert"]["steps"][0]["root_count"] == 1, out
    assert out["check"]["valid"] is True, out


# --------------------------------------------------------------------------- #
# Group 4: Wolfram|Alpha and the fast recognizer.
# --------------------------------------------------------------------------- #


@needs_alpha
def test_recognizer_envelope_shape_is_what_we_assumed():
    """RESOLVES: does the payload nest under `query`, and is `accepted` a bool?

    ``wolfram_recognizer`` guesses both: it does ``payload.get("query", payload)``
    and coerces ``accepted`` through ``_as_bool`` because the XML-ish form gives
    the string "true". This test fetches the endpoint directly, bypassing our
    parser, so the assertion message can state the ACTUAL envelope.
    """
    params = {
        "appid": wolfram_alpha._appid(),
        "mode": "Default",
        "i": "2+2",
        "output": "json",
    }
    url = (
        f"{wolfram_recognizer.RECOGNIZER_ENDPOINT}?"
        f"{urllib.parse.urlencode(params)}"
    )
    fetched = wolfram_alpha._get(url, wolfram_recognizer.DEFAULT_TIMEOUT_SECONDS)
    assert fetched is not None, "network failure reaching the query recognizer"
    status, body = fetched
    assert status == 200, f"recognizer returned HTTP {status}: {body[:400]!r}"
    payload = json.loads(body)

    assert "query" in payload, (
        "the recognizer envelope does NOT nest under 'query'; top-level keys are "
        f"{sorted(payload)!r} in {body[:400]!r}"
    )
    node = payload["query"]
    if isinstance(node, list):
        node = node[0]
    accepted = node.get("accepted")
    assert isinstance(accepted, (bool, str)), (
        f"'accepted' arrived as {type(accepted).__name__}: {node!r}"
    )
    print(
        "observed recognizer envelope: accepted =", repr(accepted),
        "| type =", type(accepted).__name__,
        "| keys =", sorted(node),
    )
    # Whichever of the two it is, our coercion has to land on a real bool.
    assert wolfram_recognizer._as_bool(accepted) is True, (
        f"_as_bool mis-read {accepted!r} for the answerable query '2+2': {node!r}"
    )


@needs_alpha
def test_recognizer_parser_returns_a_routing_hint_not_a_verdict():
    """RESOLVES: does our parser produce usable fields from the live envelope?

    Complements the raw-shape test above by going through ``recognize()``. Also
    pins the vocabulary rule: this module must never grow a truth-flavoured key,
    because a routing hint recorded as evidence is the misreading the whole module
    is written to prevent.
    """
    out = wolfram_recognizer.recognize("2+2", timeout=5.0)
    assert out["ok"] is True, f"live recognize() failed: {out!r}"
    assert isinstance(out["worth_querying"], bool), (
        f"worth_querying is not a bool, so the envelope parse degraded: {out!r}"
    )
    assert out["routing_hint"] in {"likely_answerable", "likely_unanswerable"}, out
    assert out["trusted"] is False, out
    for forbidden in ("verdict", "proved", "verified", "refuted", "confidence"):
        assert forbidden not in out, (
            f"a truth-flavoured key {forbidden!r} appeared in a routing payload: {out!r}"
        )
    print(
        "observed recognize():",
        {k: out[k] for k in ("worth_querying", "domain", "relevance_score_0_100")},
    )


@needs_alpha
def test_alpha_query_reports_pods_and_an_assumptions_list():
    """RESOLVES: does the Full Results parser survive a real payload?

    One tiny query. The load-bearing field is ``assumptions``: it must always be
    present as a list even when empty, because a caller that forgets to look at it
    can be handed an answer to a neighbouring question.
    """
    out = wolfram_alpha.query("2+2", timeout=QUICK)
    assert out["unavailable"] is False, out
    assert out["ok"] is True, f"live Alpha query failed: {out!r}"
    assert out["understood"] is True, out
    assert isinstance(out["assumptions"], list), (
        f"'assumptions' must always be a list, got {out['assumptions']!r}"
    )
    assert out["pods"], f"no pods parsed out of the live payload: {out!r}"
    assert out["interpretation"], (
        f"the first pod carried no plaintext, so the pod parse changed: {out['pods']!r}"
    )
    assert out["trusted"] is False, out
    print(
        "observed Alpha pods:", [p["id"] for p in out["pods"]],
        "| wolfram_input =", out["wolfram_input"],
    )
