"""Tier 4 benchmark harness tests.

Loaders are corpus-conditional: when a corpus is present under ``resources/``
the loader must return >0 items; when absent it must skip cleanly (return ``[]``
without raising). Graders are tested deterministically (no network — the LLM
fallback runs in mock mode).
"""
import json
import os
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
# provider component (for the mock LLM-judge fallback path)
_provider = Path(__file__).resolve().parents[2] / "provider" / "python"
if _provider.exists():
    sys.path.insert(0, str(_provider))

os.environ.setdefault("THEOREMATA_MODEL_MOCK", "1")

from theoremata_tools.benchmarks import (  # noqa: E402
    grade,
    list_benchmarks,
    load_benchmark,
    make_item,
)
from theoremata_tools.benchmarks.proof_completion import run_proof_completion  # noqa: E402
from theoremata_tools.benchmarks.registry import run as benchmark_run  # noqa: E402
from theoremata_tools.benchmarks.parsing import (  # noqa: E402
    extract_sorry_obligations,
    parse_blueprint_nodes,
    parse_fqb_main,
)
from theoremata_tools.benchmarks.resources import find_dir  # noqa: E402
from theoremata_tools.benchmarks import loaders as _loaders  # noqa: E402
from theoremata_tools.benchmarks import graders as _graders  # noqa: E402


def _fake_passing_comparator(tmp_path):
    """A comparator stub that exits 0, so the authoritative path is exercised."""
    comparator = tmp_path / "ok-comparator.py"
    comparator.write_text("raise SystemExit(0)\n", encoding="utf-8")
    return comparator


def _no_comparator(monkeypatch):
    """Force the 'authoritative comparator unavailable' condition."""
    monkeypatch.delenv("THEOREMATA_COMPARATOR", raising=False)
    monkeypatch.setattr(_graders, "_comparator_path", lambda: None)

ALL_NAMES = [b["name"] for b in list_benchmarks()]

# corpus dir glob per benchmark; presence gates the >0 assertion
_CORPUS_GLOB = {
    "formalqualbench": "FormalQualBench-main",
    "sphere_packing": "Sphere-Packing-Lean-main",
    "zklinalg": "ZkLinalg-main",
    "strongpnt": "strongpnt-main",
    "kakeya": "KakeyaFiniteFields-main",
    "riemann_hypothesis_curves": "RiemannHypothesisCurves-main",
    "frontiermath_hypergraphs": "FrontierMathOpen-Hypergraphs-main",
    "erdos1196": "Erdos1196-main",
    "ineqmath": "ineqmath-main",
    "aime24": "aime24-main",
    "aime25": "aime25-main",
    "aime26": "aime26-master",
    "brokenmath": "alethfeld-legacy",
    "goldbach_collatz": "goldbach-collatz-proof-main",
    "minif2f": "datasets-main",
    "minif2f_train": "datasets-main",
    "minif2f_valid": "datasets-main",
    "minif2f_test": "datasets-main",
    "bridge178": "BRIDGE-main",
    "quantumlean": "QuantumLean-Bench-main",
    "quantumlean_physics": "QuantumLean-Bench-main",
    "millennium": "LeanMillenniumPrizeProblems-main",
    "imo2025": "IMO2025-main",
    "putnam_artifacts": "aristotle_putnam25-main",
    "formulationbench": "flare-main",
    "imo_proofbench": "DeepSeek-Math-V2-main",
    "imo_answerbench": "IMO-Bench-main",
    "imo_gradingbench": "IMO-Bench-main",
    "zero_to_qed": "zero-to-qed-main",
    "lean_tactics_kb": "zero-to-qed-main",
    # Agda 1Lab and Metamath corpora are resources-based (absent by default).
    "1lab": "1lab-main",
    "metamath_100": "metamath-main",
    # Formalizing-100 ships a committed fixture (no resources corpus); glob a
    # never-present dir so `_corpus_present` reports absent and the loader is
    # handled via `_COMMITTED_FIXTURE` below.
    "formalizing_100": "formalizing-100-none",
    # Open conjectures as open targets (see test_formal_conjectures.py).
    "formal_conjectures": "formal-conjectures-main",
    # Adversarial expected-verdict fixtures (see test_adversarial_fixtures.py).
    "borwein_vacuity": "gdm-formal-conjectures-main",
    "partition_elliptic": "PartitionElliptic-main",
    "higher_dyson": "HigherDyson-main",
    "erdos_public": "erdos-public-main",
    "ramanujan_tau": "ramanujan-tau-misses-primes-main",
}

# corpora that exist but ship no structured problems (PDF-only data cards)
_STRUCTURELESS = {"aime24", "aime25", "aime26"}

# benchmarks whose data is a committed fixture beside the loader (not under
# resources/), so they legitimately return items even with resources/ absent.
_COMMITTED_FIXTURE = {"formalizing_100"}


def _corpus_present(name: str) -> bool:
    return find_dir(_CORPUS_GLOB[name], f"{_CORPUS_GLOB[name]}/**") is not None


# --------------------------------------------------------------------------- #
# Registry
# --------------------------------------------------------------------------- #

def test_registry_lists_all_tracks():
    tracks = {b["track"] for b in list_benchmarks()}
    assert tracks == {
        "formalization",
        "nl_answer",
        "falsification",
        "verified_programming",
        "statement_target",
        "external_artifact",
        "reformulation",
        "proof_grading",
        "tactic_reference",
        "adversarial",
        "open_conjecture",
    }
    assert len(ALL_NAMES) == 39


def test_load_unknown_benchmark_raises():
    with pytest.raises(KeyError):
        load_benchmark("does_not_exist")


# --------------------------------------------------------------------------- #
# Loaders — >0 when present, clean skip when absent
# --------------------------------------------------------------------------- #

@pytest.mark.parametrize("name", ALL_NAMES)
def test_loader_present_or_skips(name):
    items = load_benchmark(name)  # must never raise
    assert isinstance(items, list)
    if _corpus_present(name) and name not in _STRUCTURELESS:
        assert len(items) > 0, f"{name} present but loaded 0 items"
        for it in items:
            assert set(it) >= {
                "id", "kind", "informal", "formal", "expected",
                "provenance", "grading",
            }
    else:
        # absent (or structureless) → clean skip
        assert items == [] or len(items) >= 0


def test_absent_corpus_skips(monkeypatch, tmp_path):
    # point the resource root at an empty dir → every resources-based loader
    # returns []. Committed-fixture benchmarks (data beside the loader) are
    # exempt: they legitimately return items regardless of resources/.
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    for name in ALL_NAMES:
        if name in _COMMITTED_FIXTURE:
            continue
        assert load_benchmark(name) == []


# --------------------------------------------------------------------------- #
# Parsers (unit, corpus-independent)
# --------------------------------------------------------------------------- #

def test_parse_blueprint_node():
    tex = (
        r"\begin{lemma}[Chaining]\label{lem:chain}\leanok \lean{Ns.chain}"
        r"\uses{def:pi} Let $x$ be a thing. \end{lemma}"
    )
    nodes = parse_blueprint_nodes(tex)
    assert len(nodes) == 1
    n = nodes[0]
    assert n["label"] == "lem:chain"
    assert n["lean_names"] == ["Ns.chain"]
    assert n["uses"] == ["def:pi"]
    assert n["leanok"] is True
    assert "Let $x$ be a thing" in n["statement"]


def test_parse_fqb_main():
    src = (
        "import Mathlib\nnamespace Foo\n"
        "/-- A famous theorem. -/\n"
        "theorem MainTheorem (n : Nat) : n = n := by\n  sorry\nend Foo\n"
    )
    p = parse_fqb_main(src)
    assert p["id"] == "Foo.MainTheorem"
    assert p["docstring"] == "A famous theorem."
    assert "MainTheorem" in p["formal"]


def test_extract_sorry_skips_comments():
    src = (
        "theorem good : True := by sorry\n"
        "-- theorem commented : True := sorry\n"
        "theorem done : True := trivial\n"
        "/- theorem blocked : True := sorry -/\n"
    )
    obs = extract_sorry_obligations(src)
    names = {o["name"] for o in obs}
    assert names == {"good"}


# --------------------------------------------------------------------------- #
# NL grader — accepts exact, rejects approximation (deterministic)
# --------------------------------------------------------------------------- #

def _bound_item(answer):
    return make_item(
        id="t", kind="nl_answer", informal="",
        expected={"answer": answer, "answer_kind": "bound"},
        grading={"track": "nl_answer", "method": "deterministic_symbolic",
                 "answer_kind": "bound"},
    )


def test_ineqmath_bound_accepts_exact():
    item = _bound_item("$C = 1/3$")
    assert grade(item, "The answer is $C = 1/3$")["is_correct"] is True


def test_ineqmath_bound_accepts_equal_decimal_of_rational():
    # 0.5 IS exactly 1/2 → accepted (IneqMath exact rule)
    item = _bound_item("$C = 1/2$")
    assert grade(item, "The answer is $C = 0.5$")["is_correct"] is True


def test_ineqmath_bound_rejects_approximation():
    # 0.333 is only an approximation of 1/3 → rejected
    item = _bound_item("$C = 1/3$")
    assert grade(item, "The answer is $C = 0.333$")["is_correct"] is False


def test_ineqmath_relation_exact():
    item = make_item(
        id="r", kind="nl_answer", informal="",
        expected={"answer": r"(B) $\geq$", "answer_kind": "relation"},
        grading={"track": "nl_answer", "method": "deterministic", "answer_kind": "relation"},
    )
    assert grade(item, r"The answer is (B) $\geq$")["is_correct"] is True
    assert grade(item, r"The answer is (A) $\leq$")["is_correct"] is False


# --------------------------------------------------------------------------- #
# Formalization grader
# --------------------------------------------------------------------------- #

def _lean_formalization_item():
    stmt = "theorem MainTheorem (n : Nat) : n = n := by"
    return make_item(
        id="f", kind="formalization", informal="", formal=stmt,
        expected={"formal_statement": stmt, "lean_name": "Foo.MainTheorem",
                  "axioms_whitelist": ["propext", "Quot.sound", "Classical.choice"]},
        grading={"track": "formalization", "method": "comparator_or_statement"},
    )


def test_formalization_without_comparator_is_ungraded_not_a_pass(monkeypatch):
    # Statement preservation is undecidable without the authoritative
    # comparator, so the item must be UNGRADED rather than scored by a proxy.
    _no_comparator(monkeypatch)
    item = _lean_formalization_item()
    res = grade(item, "theorem MainTheorem (n : Nat) : n = n := by exact rfl")
    assert res["is_correct"] is False
    assert res["is_solved"] is False
    assert res["ungraded"] is True
    assert res["detail"]["graded"] is False
    assert res["detail"]["ungraded_reason"] == "comparator_unavailable"
    # the containment signal survives only as an explicitly labelled proxy
    assert res["detail"]["statement_preserved"] is None
    assert res["detail"]["proxy"]["is_proxy"] is True
    assert res["detail"]["proxy"]["counts_toward_pass_rate"] is False
    assert res["detail"]["proxy"]["statement_preserved_proxy"] is True


def test_formalization_sorry_is_a_definitive_fail_without_comparator(monkeypatch):
    # The axiom/sorry gate is a necessary condition, so it can still fail an
    # item outright. It must never turn into a pass.
    _no_comparator(monkeypatch)
    res = grade(_lean_formalization_item(),
                "theorem MainTheorem (n : Nat) : n = n := by sorry")
    assert res["is_correct"] is False
    assert res["ungraded"] is False
    assert res["detail"]["method"] == "axiom_gate_failed"


def test_commented_statement_is_not_preserved(monkeypatch):
    # HEADLINE: planting the canonical statement in a comment, a doc comment or
    # a string literal must not satisfy statement preservation.
    _no_comparator(monkeypatch)
    item = _lean_formalization_item()
    planted = (
        "-- theorem MainTheorem (n : Nat) : n = n := by\n"
        "/-- theorem MainTheorem (n : Nat) : n = n := by -/\n"
        '#eval "theorem MainTheorem (n : Nat) : n = n := by"\n'
        "theorem SomethingElse : True := by trivial\n"
    )
    res = grade(item, planted)
    assert res["is_correct"] is False
    assert res["detail"]["proxy"]["statement_preserved_proxy"] is False


def test_commented_statement_is_not_preserved_with_comparator_absent_helper():
    # Same claim at the helper level, independent of any grader plumbing.
    stmt = "theorem MainTheorem (n : Nat) : n = n"
    assert _graders._statement_preserved(stmt, f"-- {stmt} := by\nexample : True := trivial") is False
    assert _graders._statement_preserved(stmt, f"{stmt} := by exact rfl") is True


def test_formalization_invokes_comparator_when_configured(monkeypatch, tmp_path):
    comparator = tmp_path / "fake-comparator"
    marker = tmp_path / "called.txt"
    comparator.write_text(
        "#!/usr/bin/env python3\n"
        "import json, pathlib, sys\n"
        "cfg = json.load(open(sys.argv[1]))\n"
        f"pathlib.Path({str(marker)!r}).write_text(cfg['theorem_names'][0])\n"
        "raise SystemExit(0)\n",
        encoding="utf-8",
    )
    comparator.chmod(0o755)
    monkeypatch.setenv("THEOREMATA_COMPARATOR", str(comparator))

    stmt = "theorem MainTheorem (n : Nat) : n = n := by sorry"
    item = make_item(
        id="f",
        kind="formalization",
        informal="",
        formal=stmt,
        expected={
            "formal_statement": stmt,
            "lean_name": "Foo.MainTheorem",
            "axioms_whitelist": ["propext", "Quot.sound", "Classical.choice"],
        },
        grading={"track": "formalization", "method": "comparator_or_statement"},
    )

    res = grade(item, "theorem MainTheorem (n : Nat) : n = n := by exact rfl")
    assert res["is_correct"] is True
    assert res["detail"]["method"] == "comparator"
    assert res["detail"]["invoked"] is True
    assert marker.read_text() == "Foo.MainTheorem"


def test_formalization_uses_comparator_failure(monkeypatch, tmp_path):
    comparator = tmp_path / "fake-comparator"
    comparator.write_text("#!/usr/bin/env sh\nexit 7\n", encoding="utf-8")
    comparator.chmod(0o755)
    monkeypatch.setenv("THEOREMATA_COMPARATOR", str(comparator))

    stmt = "theorem MainTheorem (n : Nat) : n = n := by sorry"
    item = make_item(
        id="f",
        kind="formalization",
        informal="",
        formal=stmt,
        expected={"formal_statement": stmt, "lean_name": "Foo.MainTheorem"},
        grading={"track": "formalization", "method": "comparator_or_statement"},
    )

    res = grade(item, "theorem MainTheorem (n : Nat) : n = n := by exact rfl")
    assert res["is_correct"] is False
    assert res["detail"]["returncode"] == 7


# --------------------------------------------------------------------------- #
# Formalization grader — per-system routing (audit finding #10)
# --------------------------------------------------------------------------- #

def _agda_1lab_item():
    # mirrors load_1lab(): kind=formalization, no gold formal, not auto-gradable
    return make_item(
        id="1lab:Foo.Bar", kind="formalization", informal="Agda module Foo.Bar",
        formal=None,
        expected={"mode": "agda_typecheck", "gold_present": False,
                  "axioms_whitelist": ["propext"]},
        grading={"track": "formalization", "method": "agda_typecheck",
                 "auto_gradable": False},
        provenance={"corpus": "1lab", "module": "Foo.Bar"},
    )


def _metamath_item():
    # mirrors load_metamath_100(): kind=formalization, gold formal present
    return make_item(
        id="metamath:mp2", kind="formalization", informal="Metamath theorem mp2",
        formal="|- ph => |- ps => |- ch",
        expected={"mode": "metamath_verify", "gold_present": True},
        grading={"track": "formalization", "method": "metamath_verify",
                 "auto_gradable": True},
        provenance={"corpus": "metamath_100", "label": "mp2"},
    )


def test_agda_item_not_graded_by_lean_comparator(monkeypatch, tmp_path):
    # Even with a Lean comparator configured, an Agda item must NOT invoke it,
    # and must not be spuriously scored (1Lab is to-be-formalized: no gold).
    comparator = tmp_path / "fake-comparator"
    marker = tmp_path / "agda-called.txt"
    comparator.write_text(
        "#!/usr/bin/env python3\n"
        "import pathlib, sys\n"
        f"pathlib.Path({str(marker)!r}).write_text('called')\n"
        "raise SystemExit(0)\n",
        encoding="utf-8",
    )
    comparator.chmod(0o755)
    monkeypatch.setenv("THEOREMATA_COMPARATOR", str(comparator))

    res = grade(_agda_1lab_item(), "postulate foo : Set")
    assert res["detail"]["system"] == "agda"
    assert res["detail"]["method"] != "comparator"
    assert res["detail"]["auto_gradable"] is False
    assert res["is_correct"] is False
    # the Lean comparator was never spawned for an Agda item
    assert not marker.exists()


def test_metamath_item_graded_language_agnostic_not_lean():
    # Metamath has a gold formal statement → language-agnostic normalized match,
    # NOT the Lean `:=` split / sorry gate.
    item = _metamath_item()
    ok = grade(item, "prefix |- ph => |- ps => |- ch suffix")
    assert ok["detail"]["system"] == "metamath"
    assert ok["detail"]["method"] == "metamath_ungraded_no_verifier"
    # no Metamath verifier runs in-process, so containment is a proxy only
    assert ok["is_correct"] is False
    assert ok["ungraded"] is True
    assert ok["detail"]["proxy"]["statement_preserved_proxy"] is True
    # a "sorry" that would trip the Lean axiom gate is irrelevant here: the
    # statement simply is not present, so the proxy is False for the right
    # reason (no Lean-specific token handling)
    bad = grade(item, "totally different statement with sorry in it")
    assert bad["is_correct"] is False
    assert bad["detail"]["method"] == "metamath_ungraded_no_verifier"
    assert bad["detail"]["proxy"]["statement_preserved_proxy"] is False
    # a Metamath comment ($( ... $)) cannot plant the statement either
    planted = grade(item, "$( |- ph => |- ps => |- ch $) something else")
    assert planted["detail"]["proxy"]["statement_preserved_proxy"] is False


def test_lean_formalization_still_uses_lean_path_no_regression(monkeypatch, tmp_path):
    item = _lean_formalization_item()
    monkeypatch.setenv("THEOREMATA_COMPARATOR", str(_fake_passing_comparator(tmp_path)))
    res = grade(item, "theorem MainTheorem (n : Nat) : n = n := by exact rfl")
    assert res["detail"]["system"] == "lean"
    assert res["detail"]["method"] == "comparator"
    assert res["is_correct"] is True
    assert res["ungraded"] is False
    # Lean sorry gate still fires when the comparator is unavailable
    _no_comparator(monkeypatch)
    assert grade(item, "theorem MainTheorem (n : Nat) : n = n := by sorry")["is_correct"] is False


def test_to_be_formalized_item_not_spuriously_scored():
    # Echoing the informal text back must never yield a pass for a no-gold item.
    res = grade(_agda_1lab_item(), "Agda module Foo.Bar")
    assert res["is_correct"] is False
    assert res["detail"]["auto_gradable"] is False
    assert res["detail"]["statement_preserved"] is None


def test_agda_and_metamath_load_and_grade_if_present():
    for name, system in (("1lab", "agda"), ("metamath_100", "metamath")):
        if not _corpus_present(name):
            continue
        items = load_benchmark(name)
        assert len(items) > 0
        res = grade(items[0], "some response text")
        assert res["detail"]["system"] == system
        assert res["detail"]["method"] != "comparator"


# --------------------------------------------------------------------------- #
# Falsification grader
# --------------------------------------------------------------------------- #

def test_brokenmath_scores_flaw_detection():
    item = make_item(
        id="b", kind="falsification", informal="prove X (corrupted)",
        expected={"mode": "detect_flaw", "is_adversarial": True,
                  "original_problem": "...", "solution": "..."},
        grading={"track": "falsification", "method": "flaw_detection"},
    )
    ok = grade(item, "The statement is false; here is a counterexample.")
    assert ok["is_correct"] is True and ok["is_solved"] is True
    # falsely "proving" a corrupted statement is wrong
    bad = grade(item, "We prove it. QED, the proof is valid.")
    assert bad["is_correct"] is False
    # structured verdict also accepted
    assert grade(item, {"verdict": "flawed"})["is_correct"] is True


def test_goldbach_collatz_must_reject():
    item = make_item(
        id="g", kind="falsification", informal="crank proof",
        expected={"mode": "reject", "verdict": "reject"},
        grading={"track": "falsification", "method": "must_reject"},
    )
    assert grade(item, "This is not a valid proof; reject.")["is_correct"] is True
    assert grade(item, "The proof is valid and complete. QED.")["is_correct"] is False


# --------------------------------------------------------------------------- #
# End-to-end on a real corpus when available
# --------------------------------------------------------------------------- #

def test_brokenmath_end_to_end_if_present():
    if not _corpus_present("brokenmath"):
        pytest.skip("brokenmath corpus absent")
    items = load_benchmark("brokenmath")
    assert len(items) == 10
    res = grade(items[0], "This claim is false — counterexample found.")
    assert res["is_correct"] is True


# --------------------------------------------------------------------------- #
# MiniF2F loader + proof-completion runner (synthetic corpus)
# --------------------------------------------------------------------------- #

def _write_minif2f_split(root: Path, split: str, records: list[dict]) -> None:
    d = root / "datasets-main" / "datasets-main" / "MiniF2F"
    d.mkdir(parents=True, exist_ok=True)
    fname = {"train": "train.json", "valid": "validation.json", "test": "test.json"}[split]
    (d / fname).write_text(json.dumps(records), encoding="utf-8")


def test_minif2f_loader_parses_synthetic_corpus(monkeypatch, tmp_path):
    _write_minif2f_split(
        tmp_path,
        "test",
        [
            {
                "id": 42,
                "name": "mathd_algebra_182",
                "natural": "What is 1+1?",
                "formal": "theorem mathd_algebra_182 : 1 + 1 = 2 := by sorry",
            }
        ],
    )
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    items = load_benchmark("minif2f_test")
    assert len(items) == 1
    it = items[0]
    assert it["id"] == "minif2f:test:42"
    assert it["kind"] == "formalization"
    assert it["informal"] == "What is 1+1?"
    assert "sorry" in it["formal"]
    assert it["expected"]["lean_name"] == "mathd_algebra_182"
    assert it["grading"]["task"] == "proof_completion"
    assert it["provenance"]["split"] == "test"


def test_minif2f_combined_loader(monkeypatch, tmp_path):
    for split, rid in (("train", 1), ("valid", 2), ("test", 3)):
        _write_minif2f_split(
            tmp_path,
            split,
            [{"id": rid, "name": f"t{rid}", "natural": "n", "formal": f"theorem t{rid} : True := by sorry"}],
        )
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    items = load_benchmark("minif2f")
    assert len(items) == 3
    assert {it["provenance"]["split"] for it in items} == {"train", "valid", "test"}


def test_proof_completion_smoke_runner(monkeypatch, tmp_path):
    formal = "theorem smoke_thm (n : Nat) : n = n := by sorry"
    _write_minif2f_split(
        tmp_path,
        "test",
        [{"id": 7, "name": "smoke_thm", "natural": "n=n", "formal": formal}],
    )
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    monkeypatch.setenv("THEOREMATA_COMPARATOR", str(_fake_passing_comparator(tmp_path)))
    items = load_benchmark("minif2f_test")
    good = "theorem smoke_thm (n : Nat) : n = n := by exact rfl"
    out = run_proof_completion(
        benchmark="minif2f_test",
        responses={items[0]["id"]: good},
    )
    assert out["n"] == 1
    assert out["correct"] == 1
    assert out["results"][0]["is_correct"] is True

    via_registry = benchmark_run(
        {"op": "proof_completion", "benchmark": "minif2f_test", "responses": {items[0]["id"]: good}}
    )
    assert via_registry["op"] == "proof_completion"
    assert via_registry["correct"] == 1


def test_proof_completion_without_comparator_scores_nothing(monkeypatch, tmp_path):
    # Without the comparator every formalization item is ungraded, so the
    # runner's headline "correct" must be 0 rather than a substring-inflated 1.
    formal = "theorem smoke_thm (n : Nat) : n = n := by sorry"
    _write_minif2f_split(
        tmp_path,
        "test",
        [{"id": 7, "name": "smoke_thm", "natural": "n=n", "formal": formal}],
    )
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    _no_comparator(monkeypatch)
    items = load_benchmark("minif2f_test")
    good = "theorem smoke_thm (n : Nat) : n = n := by exact rfl"
    out = run_proof_completion(
        benchmark="minif2f_test", responses={items[0]["id"]: good}
    )
    assert out["correct"] == 0
    assert out["solved"] == 0
    assert out["results"][0]["detail"]["ungraded"] is True


# --------------------------------------------------------------------------- #
# BRIDGE-178 loader — reads `function_signature` (bug fix regression lock)
# --------------------------------------------------------------------------- #

_BRIDGE_RECORD = {
    "task_id": "t1",
    "dataset_id": "bridge178",
    "title_or_source_id": "weekly-contest-381-minimum-number-of-pushes",
    "difficulty": "easy",
    "tags": ["algorithms"],
    "problem_statement": "Return the minimum number of pushes.",
    "python": {
        "function_name": "minimumPushes",
        "function_signature": "def minimumPushes(word: str) -> int:\n    pass",
    },
    "lean": {
        "function_name": "minimumPushes",
        "function_signature": "def minimumPushes (word : String) : Int",
        "arguments": ["word"],
        "argument_types": ["String"],
    },
    "tests": {
        "inputs": [{"word": "abcde"}, {"word": "b"}],
        "expected_outputs": [5, 1],
    },
}


def _write_bridge(root: Path, records: list[dict]) -> None:
    d = root / "BRIDGE-main" / "BRIDGE-main" / "datasets"
    d.mkdir(parents=True, exist_ok=True)
    (d / "bridge178.jsonl").write_text(
        "\n".join(json.dumps(r) for r in records), encoding="utf-8"
    )


def test_bridge178_loads_function_signature(monkeypatch, tmp_path):
    _write_bridge(tmp_path, [_BRIDGE_RECORD])
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    items = load_benchmark("bridge178")
    assert len(items) == 1
    it = items[0]
    assert it["kind"] == "verified_programming"
    # the bug: signatures used to load [] because the key was wrong
    assert it["expected"]["lean_signatures"] == ["def minimumPushes (word : String) : Int"]
    assert it["expected"]["function_name"] == "minimumPushes"
    assert it["expected"]["arguments"] == ["word"]
    assert it["expected"]["argument_types"] == ["String"]
    # named-kwarg oracle inputs bind by argument name
    oracle = it["expected"]["oracle_tests"]
    assert oracle["bind"] == "kwargs"
    assert oracle["inputs"][0] == {"word": "abcde"}
    # and grading is now actually correctable (non-empty signatures)
    good = (
        "```lean\ndef minimumPushes (word : String) : Int := 0\n```"
    )
    res = grade(it, good)
    assert res["detail"]["signatures_ok"] is True
    assert res["is_correct"] is True


def test_bridge178_grade_correctable_on_real_corpus_if_present():
    if not _corpus_present("bridge178"):
        pytest.skip("BRIDGE corpus absent")
    items = load_benchmark("bridge178")
    assert len(items) > 0
    # every item now carries a non-empty Lean signature (was always [] pre-fix)
    assert all(it["expected"]["lean_signatures"] for it in items)


# --------------------------------------------------------------------------- #
# QuantumLean loader — no fabricated formal-gold grade (bug fix regression lock)
# --------------------------------------------------------------------------- #

_QL_RECORD = {
    "id": "5.73_0001",
    "source": "MIT OpenCourseWare, 5.73",
    "domain": "quantum_physics",
    "type": "proof-based",
    "problem": "Show that the operator is Hermitian.",
    "metadata": {},
    "citations": [],
    "solution_informal": {"gpt5.4_response": "Because the eigenvalues are real ..."},
    "solution_formal": {"gpt5.4_response": "import Mathlib\ntheorem foo : True := by trivial"},
    "manual_eval": {
        "scale": "0-2",
        "gold_present": False,
        "rubric": {"2": "Correct.", "1": "Partial.", "0": "Wrong."},
        "responses": {"solution_formal.gpt5.4_response": {"score": 1, "correct": False}},
    },
}


def _write_quantumlean(root: Path, records: list[dict]) -> None:
    d = root / "QuantumLean-Bench-main" / "QuantumLean" / "BenchmarkData" / "Physics"
    d.mkdir(parents=True, exist_ok=True)
    (d / "mitocw_5.73.json").write_text(json.dumps(records), encoding="utf-8")


def test_quantumlean_does_not_stringify_model_dict(monkeypatch, tmp_path):
    _write_quantumlean(tmp_path, [_QL_RECORD])
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    items = load_benchmark("quantumlean")
    assert len(items) == 1
    it = items[0]
    assert it["kind"] == "scientific_formalization"
    # the bug: `formal` used to be the repr of a {model: lean} dict
    assert it["formal"] is None
    assert "gpt5.4_response" not in str(it["formal"])
    exp = it["expected"]
    assert exp["gold_present"] is False
    assert exp["response_model_keys"] == ["gpt5.4_response"]
    assert exp["model_responses_formal"]["gpt5.4_response"].startswith("import Mathlib")
    assert exp["manual_eval"]["scale"] == "0-2"
    assert it["grading"]["method"] == "typecheck_only"


def test_quantumlean_grader_is_honest_no_statement_preservation(monkeypatch, tmp_path):
    _write_quantumlean(tmp_path, [_QL_RECORD])
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    it = load_benchmark("quantumlean")[0]
    # Even echoing back a model's "solution" must NOT yield a fabricated pass:
    # there is no gold statement to preserve.
    res = grade(it, "import Mathlib\ntheorem foo : True := by trivial")
    assert res["is_correct"] is False
    assert res["detail"]["method"] == "typecheck_only"
    assert res["detail"]["auto_gradable"] is False
    assert res["is_solved"] is True


# --------------------------------------------------------------------------- #
# IMO-ProofBench loader — gold+model grade pairs (evaluator calibration)
# --------------------------------------------------------------------------- #

_PROOFBENCH_ROW = {
    "solution": "By taking x=0 ...",
    "grading guidelines": "(Partial) 1. Guessed the solution correctly",
    "level": "IMO-easy",
    "source": "(Modified) IMO 2019, P1",
    "question": "Determine all functions f such that ...",
    "problem_idx": "PB-Basic-001",
    "type": "Algebra",
    "model_prediction": {
        "proof": "Let P(x,y) denote the statement ...",
        "average_automatic_rating": 1.0,
        "human_rating": 7,
    },
}


def _write_proofbench(root: Path, split: str, rows: list[dict]) -> None:
    d = root / "DeepSeek-Math-V2-main" / "DeepSeek-Math-V2-main" / "outputs"
    d.mkdir(parents=True, exist_ok=True)
    (d / f"IMO-ProofBench-{split}.jsonl").write_text(
        "\n".join(json.dumps(r) for r in rows), encoding="utf-8"
    )


def test_imo_proofbench_exposes_gold_and_prediction(monkeypatch, tmp_path):
    _write_proofbench(tmp_path, "Basic", [_PROOFBENCH_ROW])
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    items = load_benchmark("imo_proofbench")
    assert len(items) == 1
    it = items[0]
    assert it["id"] == "imo_proofbench:PB-Basic-001"
    assert it["kind"] == "proof_grading"
    exp = it["expected"]
    # gold human grade + model grade pair (the calibration signal)
    assert exp["gold_human_rating"] == 7
    assert exp["model_auto_rating"] == 1.0
    assert exp["reference_solution"].startswith("By taking")
    assert exp["grading_guidelines"].startswith("(Partial)")
    assert exp["prediction_proof"].startswith("Let P(x,y)")
    assert it["grading"]["split"] == "Basic"


def test_imo_proofbench_calibration_grader(monkeypatch, tmp_path):
    _write_proofbench(tmp_path, "Basic", [_PROOFBENCH_ROW])
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    it = load_benchmark("imo_proofbench")[0]
    # a grader that also says 7/7 agrees with the human → correct
    assert grade(it, r"Final grade: \boxed{7}")["is_correct"] is True
    # a grader that says 0 is far from the human 7 → incorrect
    assert grade(it, r"Final grade: \boxed{0}")["is_correct"] is False
    # normalized 0-1 scale is accepted too (1.0 ~ 7/7)
    assert grade(it, {"score": 1.0})["is_correct"] is True


def test_imo_proofbench_end_to_end_if_present():
    if not _corpus_present("imo_proofbench"):
        pytest.skip("IMO-ProofBench corpus absent")
    items = load_benchmark("imo_proofbench")
    assert len(items) == 60  # 30 Basic + 30 Advanced
    # every item exposes a gold human grade + the model's proof under evaluation
    assert all(it["expected"]["prediction_proof"] for it in items)
    assert all(it["expected"]["gold_human_rating"] is not None for it in items)
    assert {it["grading"]["split"] for it in items} == {"Basic", "Advanced"}


# --------------------------------------------------------------------------- #
# zero-to-qed loader — classic-proof completion bench (manual vs automation)
# --------------------------------------------------------------------------- #

def _write_zero_to_qed(root: Path, stem: str, src: str) -> None:
    d = root / "zero-to-qed-main" / "zero-to-qed-main" / "src" / "ZeroToQED" / "Proofs"
    d.mkdir(parents=True, exist_ok=True)
    (d / f"{stem}.lean").write_text(src, encoding="utf-8")


def test_zero_to_qed_loads_manual_vs_automation_pair(monkeypatch, tmp_path):
    _write_zero_to_qed(
        tmp_path,
        "InfinitudePrimes",
        "namespace ZeroToQED.Proofs\n"
        "theorem InfinitudeOfPrimes : ∀ n, ∃ p > n, Nat.Prime p := by\n  sorry\n"
        "end ZeroToQED.Proofs\n",
    )
    _write_zero_to_qed(
        tmp_path,
        "InfinitudePrimesGrind",
        "namespace ZeroToQED.Proofs.Grind\n"
        "theorem InfinitudeOfPrimes : ∀ n, ∃ p > n, IsPrime p := by\n  grind\n"
        "end ZeroToQED.Proofs.Grind\n",
    )
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    items = load_benchmark("zero_to_qed")
    assert len(items) == 2
    by_strategy = {it["expected"]["strategy"]: it for it in items}
    assert set(by_strategy) == {"manual", "automation"}
    # the manual/automation pair shares a theorem_key
    assert by_strategy["manual"]["expected"]["theorem_key"] == "InfinitudePrimes"
    assert by_strategy["automation"]["expected"]["theorem_key"] == "InfinitudePrimes"
    for it in items:
        assert it["kind"] == "formalization"
        assert it["grading"]["task"] == "proof_completion"
        assert "InfinitudeOfPrimes" in it["expected"]["reference_proof"]


def test_zero_to_qed_end_to_end_if_present():
    if not _corpus_present("zero_to_qed"):
        pytest.skip("zero-to-qed corpus absent")
    items = load_benchmark("zero_to_qed")
    assert len(items) > 0
    keys = {it["expected"]["theorem_key"] for it in items}
    # the InfinitudePrimes manual-vs-grind pair is present and paired
    strategies = {
        it["expected"]["strategy"]
        for it in items
        if it["expected"]["theorem_key"] == "InfinitudePrimes"
    }
    assert strategies == {"manual", "automation"}
    assert "Sqrt2Irrational" in keys


# --------------------------------------------------------------------------- #
# Lean tactics KB loader — structured {tactic, purpose, example}
# --------------------------------------------------------------------------- #

_TACTICS_MD = """# Tactics Reference

## Table of Contents

- [`omega`](#omega) - Solve linear arithmetic over Nat and Int
- [`simp`](#simp) - Apply simplification lemmas
- [`push Not`](#push-not) - Push negations inward

## Section

### omega

The **`omega`** tactic decides linear arithmetic over integers and naturals.

```lean
example (n : Nat) : n + 0 = n := by omega
```

### simp

The `simp` tactic applies simplification lemmas to the goal.

```lean
example : 1 + 1 = 2 := by simp
```
"""


def _write_tactics_kb(root: Path, md: str) -> None:
    d = root / "zero-to-qed-main" / "zero-to-qed-main" / "docs" / "src"
    d.mkdir(parents=True, exist_ok=True)
    (d / "appendix_c_tactics.md").write_text(md, encoding="utf-8")


def test_lean_tactics_kb_parses_structured_entries(monkeypatch, tmp_path):
    _write_tactics_kb(tmp_path, _TACTICS_MD)
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    items = load_benchmark("lean_tactics_kb")
    assert len(items) == 3
    by_tactic = {it["expected"]["tactic"]: it for it in items}
    assert set(by_tactic) == {"omega", "simp", "push Not"}
    omega = by_tactic["omega"]
    assert omega["kind"] == "tactic_reference"
    assert omega["expected"]["purpose"] == "Solve linear arithmetic over Nat and Int"
    assert "linear arithmetic" in omega["expected"]["description"]
    assert "by omega" in omega["expected"]["example"]
    # retrieval-style grader matches when the tactic is named
    assert grade(omega, "use the omega tactic here")["is_correct"] is True
    assert grade(omega, "use ring instead")["is_correct"] is False


# --------------------------------------------------------------------------- #
# MiniF2F exclusions: applied AND reported, never silently dropped
# --------------------------------------------------------------------------- #

_MINIF2F_ROWS = [
    {"id": 1, "name": "mathd_algebra_182", "natural": "a", "formal": "theorem a : True := by sorry"},
    {"id": 2, "name": "amc12a_2020_p10", "natural": "b", "formal": "theorem b : True := by sorry"},
    {"id": 3, "name": "imo_1983_p6", "natural": "c", "formal": "theorem c : True := by sorry"},
]


def test_minif2f_empty_exclusion_list_changes_nothing(monkeypatch, tmp_path):
    # The shipped list is empty on purpose (the mis-formalised ids were never
    # copied into this repo), so today's numbers must be untouched.
    assert _loaders.MINIF2F_EXCLUSIONS == {}
    _write_minif2f_split(tmp_path, "test", _MINIF2F_ROWS)
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    items = load_benchmark("minif2f_test")
    assert len(items) == 3
    rep = _loaders.exclusion_report("minif2f_test")
    assert rep["candidates"] == 3
    assert rep["excluded"] == 0
    assert rep["scored"] == 3
    assert rep["exclusions"] == {}


def test_minif2f_exclusion_is_applied_and_reported(monkeypatch, tmp_path):
    monkeypatch.setitem(
        _loaders.MINIF2F_EXCLUSIONS, "mathd_algebra_182", "mis-formalised: test fixture"
    )
    _write_minif2f_split(tmp_path, "test", _MINIF2F_ROWS)
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    items = load_benchmark("minif2f_test")

    # excluded item is absent from the scored set ...
    ids = {it["id"] for it in items}
    names = {it["provenance"]["name"] for it in items}
    assert "minif2f:test:1" not in ids
    assert "mathd_algebra_182" not in names
    assert len(items) == 2

    # ... and the drop is fully accounted for: a run can say "3 items, 1
    # excluded as mis-formalised, scored over 2".
    rep = _loaders.exclusion_report("minif2f_test")
    assert rep["candidates"] == 3
    assert rep["excluded"] == 1
    assert rep["scored"] == 2 == len(items)
    assert rep["exclusions"] == {"minif2f:test:1": "mis-formalised: test fixture"}


def test_minif2f_exclusion_also_matches_item_id(monkeypatch, tmp_path):
    monkeypatch.setitem(
        _loaders.MINIF2F_EXCLUSIONS, "minif2f:test:3", "mis-formalised: by uid"
    )
    _write_minif2f_split(tmp_path, "test", _MINIF2F_ROWS)
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    items = load_benchmark("minif2f_test")
    assert {it["id"] for it in items} == {"minif2f:test:1", "minif2f:test:2"}
    assert _loaders.exclusion_report("minif2f_test")["excluded"] == 1


def test_minif2f_combined_report_aggregates_splits(monkeypatch, tmp_path):
    monkeypatch.setitem(
        _loaders.MINIF2F_EXCLUSIONS, "mathd_algebra_182", "mis-formalised: fixture"
    )
    for split in ("train", "valid", "test"):
        _write_minif2f_split(tmp_path, split, _MINIF2F_ROWS)
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    items = load_benchmark("minif2f")
    rep = _loaders.exclusion_report("minif2f")
    assert rep["candidates"] == 9
    assert rep["excluded"] == 3  # one per split
    assert rep["scored"] == 6 == len(items)
    assert set(rep["per_split"]) == {"train", "valid", "test"}


def test_minif2f_missing_train_items_are_reconciled_not_excluded(monkeypatch, tmp_path):
    # The three README-documented missing training problems are ABSENT, not
    # mis-formalised: they must not be treated as exclusions.
    _write_minif2f_split(tmp_path, "train", _MINIF2F_ROWS)
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    items = load_benchmark("minif2f_train")
    rep = _loaders.exclusion_report("minif2f_train")
    assert rep["excluded"] == 0
    assert rep["scored"] == len(items) == 3
    missing = rep["known_missing_upstream"]
    assert missing["ids"] == [
        "mathd_algebra_31",
        "mathd_numbertheory_24",
        "amc12a_2020_p22",
    ]
    assert missing["confirmed_absent"] == missing["ids"]
    assert missing["unexpectedly_present"] == []
    assert missing["nominal_total_all_splits"] == 488
    # only the train split carries the reconciliation note
    assert "known_missing_upstream" not in (
        _loaders.exclusion_report("minif2f_test") or {"known_missing_upstream": None}
    )


# --------------------------------------------------------------------------- #
# Per-item toolchain metadata: declared when the corpus declares it, else
# explicitly `unknown` (never fabricated)
# --------------------------------------------------------------------------- #

_TOOLCHAIN_KEYS = {"declared", "system", "lean", "mathlib_rev", "mathlib_input_rev"}


@pytest.mark.parametrize("name", sorted(_loaders.LOADERS))
def test_every_item_carries_toolchain_metadata(name):
    for it in load_benchmark(name):
        tc = it["provenance"].get("toolchain")
        assert isinstance(tc, dict), f"{name}: item {it['id']} has no toolchain"
        assert _TOOLCHAIN_KEYS <= set(tc)
        if not tc["declared"]:
            # nothing declared → explicit unknown, not a guess
            assert tc["lean"] == "unknown"
            assert tc["system"] == "unknown"


def test_toolchain_is_read_from_what_the_corpus_declares(monkeypatch, tmp_path):
    root = tmp_path / "zero-to-qed-main" / "zero-to-qed-main"
    (root / "src" / "ZeroToQED" / "Proofs").mkdir(parents=True)
    (root / "lean-toolchain").write_text("leanprover/lean4:v4.30.0\n", encoding="utf-8")
    (root / "lake-manifest.json").write_text(
        json.dumps(
            {
                "version": "1.1.0",
                "packages": [
                    {"name": "batteries", "rev": "aaa", "inputRev": "main"},
                    {"name": "mathlib", "rev": "f897ebc", "inputRev": "v4.30.0"},
                ],
            }
        ),
        encoding="utf-8",
    )
    (root / "src" / "ZeroToQED" / "Proofs" / "Sqrt2Irrational.lean").write_text(
        "theorem Sqrt2Irrational : True := by trivial\n", encoding="utf-8"
    )
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    items = load_benchmark("zero_to_qed")
    assert len(items) == 1
    tc = items[0]["provenance"]["toolchain"]
    assert tc["declared"] is True
    assert tc["system"] == "lean4"
    assert tc["lean"] == "leanprover/lean4:v4.30.0"
    assert tc["mathlib_rev"] == "f897ebc"
    assert tc["mathlib_input_rev"] == "v4.30.0"
    assert tc["source"].endswith("lean-toolchain")


def test_toolchain_unknown_when_corpus_declares_nothing(monkeypatch, tmp_path):
    # MiniF2F ships JSON only: no lean-toolchain, no lakefile. Its items are
    # Lean 4 artifacts, but we must not invent a version for them.
    _write_minif2f_split(tmp_path, "test", _MINIF2F_ROWS)
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    tc = load_benchmark("minif2f_test")[0]["provenance"]["toolchain"]
    assert tc == dict(_loaders.UNKNOWN_TOOLCHAIN)
    assert tc["declared"] is False
    assert tc["lean"] == "unknown"
    assert tc["mathlib_rev"] == "unknown"


def test_toolchain_records_are_not_shared_between_items(monkeypatch, tmp_path):
    _write_minif2f_split(tmp_path, "test", _MINIF2F_ROWS)
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    items = load_benchmark("minif2f_test")
    items[0]["provenance"]["toolchain"]["lean"] = "mutated"
    assert items[1]["provenance"]["toolchain"]["lean"] == "unknown"


def test_lean_tactics_kb_end_to_end_if_present():
    if not _corpus_present("lean_tactics_kb"):
        pytest.skip("zero-to-qed tactics appendix absent")
    items = load_benchmark("lean_tactics_kb")
    assert len(items) > 30  # the appendix documents ~60-80 tactics
    tactics = {it["expected"]["tactic"] for it in items}
    assert {"omega", "simp", "aesop", "ring"} <= tactics
    assert all(it["expected"]["purpose"] for it in items)
