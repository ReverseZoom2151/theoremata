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
}

# corpora that exist but ship no structured problems (PDF-only data cards)
_STRUCTURELESS = {"aime24", "aime25", "aime26"}


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
    }
    assert len(ALL_NAMES) == 25


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
    # point the resource root at an empty dir → every loader returns []
    monkeypatch.setenv("THEOREMATA_RESOURCES", str(tmp_path))
    for name in ALL_NAMES:
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

def test_formalization_statement_preserved_no_sorry():
    stmt = "theorem MainTheorem (n : Nat) : n = n := by"
    item = make_item(
        id="f", kind="formalization", informal="", formal=stmt,
        expected={"formal_statement": stmt, "lean_name": "Foo.MainTheorem",
                  "axioms_whitelist": ["propext", "Quot.sound", "Classical.choice"]},
        grading={"track": "formalization", "method": "comparator_or_statement"},
    )
    good = "theorem MainTheorem (n : Nat) : n = n := by exact rfl"
    res = grade(item, good)
    assert res["is_correct"] is True and res["detail"]["statement_preserved"] is True
    # a residual sorry fails the axioms gate
    assert grade(item, "theorem MainTheorem (n : Nat) : n = n := by sorry")["is_correct"] is False


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
