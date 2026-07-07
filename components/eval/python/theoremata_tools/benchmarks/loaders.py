"""Per-corpus loaders → uniform benchmark items (Tier 4).

Every loader:

* globs its corpus at runtime under ``resources/`` and returns ``[]`` (logging a
  skip) when the corpus is absent — no crashes, no hardcoded problems;
* logs the number of items *loaded* and *skipped* (no silent truncation);
* emits :func:`schema.make_item` dicts in the common internal schema.

Loaders are grouped by track:

* formalization: FormalQualBench, Sphere-Packing, ZkLinalg, strongpnt, Kakeya,
  RiemannHypothesisCurves, FrontierMath-Hypergraphs, Erdos1196;
* nl_answer: IneqMath, AIME 24/25/26;
* falsification: brokenmath, goldbach-collatz.
"""
from __future__ import annotations

import csv
import json
import logging
from pathlib import Path
from typing import Any, Callable

from .parsing import (
    extract_sorry_obligations,
    parse_blueprint_nodes,
    parse_fqb_main,
)
from .resources import find_dir, find_files, rel
from .schema import AXIOMS_WHITELIST, make_item

log = logging.getLogger("theoremata.benchmarks")


def _log_counts(name: str, loaded: int, skipped: int, note: str = "") -> None:
    log.info(
        "benchmark %-24s loaded=%d skipped=%d%s",
        name,
        loaded,
        skipped,
        f" ({note})" if note else "",
    )


# ===========================================================================
# Formalization track
# ===========================================================================

def _formal_expected(formal: str, lean_name: str | None) -> dict[str, Any]:
    return {
        "formal_statement": formal,
        "lean_name": lean_name,
        "axioms_whitelist": list(AXIOMS_WHITELIST),
    }


def load_formalqualbench() -> list[dict[str, Any]]:
    """23 ``MainTheorem`` stubs; id = ``<Namespace>.MainTheorem``."""
    files = find_files("FormalQualBench-main/**/FormalQualBench/*/Main.lean")
    items: list[dict[str, Any]] = []
    skipped = 0
    for path in files:
        parsed = parse_fqb_main(path.read_text(encoding="utf-8", errors="replace"))
        if not parsed:
            skipped += 1
            continue
        items.append(
            make_item(
                id=parsed["id"],
                kind="formalization",
                informal=parsed["docstring"],
                formal=parsed["formal"],
                expected=_formal_expected(parsed["formal"], parsed["id"]),
                grading={"track": "formalization", "method": "comparator_or_statement"},
                provenance={
                    "corpus": "formalqualbench",
                    "namespace": parsed["namespace"],
                    "path": rel(path),
                },
            )
        )
    _log_counts("formalqualbench", len(items), skipped)
    return items


def _load_blueprint_corpus(
    corpus: str,
    glob_patterns: list[str],
) -> list[dict[str, Any]]:
    """Generic leanblueprint loader: each labeled node with a ``\\lean`` binding
    (i.e. an actual Lean obligation) becomes a formalization item."""
    files = find_files(*glob_patterns)
    items: list[dict[str, Any]] = []
    skipped = 0
    seen: set[str] = set()
    for path in files:
        nodes = parse_blueprint_nodes(path.read_text(encoding="utf-8", errors="replace"))
        for node in nodes:
            if not node["lean_names"]:
                skipped += 1  # a prose/eqn node with no Lean binding
                continue
            lean_name = node["lean_names"][0]
            node_id = f"{corpus}:{node['label']}"
            if node_id in seen:
                continue
            seen.add(node_id)
            items.append(
                make_item(
                    id=node_id,
                    kind="formalization",
                    informal=node["statement"],
                    formal=lean_name,
                    expected=_formal_expected(lean_name, lean_name),
                    grading={
                        "track": "formalization",
                        "method": "comparator_or_statement",
                    },
                    provenance={
                        "corpus": corpus,
                        "blueprint_label": node["label"],
                        "env": node["env"],
                        "lean_names": node["lean_names"],
                        "uses": node["uses"],
                        "leanok": node["leanok"],
                        "path": rel(path),
                    },
                )
            )
    _log_counts(corpus, len(items), skipped, "blueprint nodes without \\lean skipped")
    return items


def load_zklinalg() -> list[dict[str, Any]]:
    return _load_blueprint_corpus(
        "zklinalg", ["ZkLinalg-main/**/blueprint/src/**/*.tex"]
    )


def load_strongpnt() -> list[dict[str, Any]]:
    return _load_blueprint_corpus(
        "strongpnt", ["strongpnt-main/**/blueprint/src/**/*.tex"]
    )


def load_kakeya() -> list[dict[str, Any]]:
    return _load_blueprint_corpus(
        "kakeya", ["KakeyaFiniteFields-main/**/blueprint/src/**/*.tex"]
    )


def load_riemann_hypothesis_curves() -> list[dict[str, Any]]:
    return _load_blueprint_corpus(
        "riemann_hypothesis_curves",
        ["RiemannHypothesisCurves-main/**/blueprint/src/**/*.tex"],
    )


def load_frontiermath_hypergraphs() -> list[dict[str, Any]]:
    return _load_blueprint_corpus(
        "frontiermath_hypergraphs",
        ["FrontierMathOpen-Hypergraphs-main/**/blueprint/src/**/*.tex"],
    )


def load_erdos1196() -> list[dict[str, Any]]:
    return _load_blueprint_corpus(
        "erdos1196", ["Erdos1196-main/**/blueprint/src/**/*.tex"]
    )


def load_sphere_packing() -> list[dict[str, Any]]:
    """Live ``sorry`` obligations. Each sorry-bearing Lean decl → a formalization
    item; if the decl name is bound to a blueprint node, attach its prose."""
    lean_files = find_files(
        "Sphere-Packing-Lean-main/**/SpherePacking/**/*.lean"
    )
    if not lean_files:
        _log_counts("sphere_packing", 0, 0, "corpus absent")
        return []

    # blueprint index: lean-decl-name -> statement prose
    blueprint_prose: dict[str, str] = {}
    for tex in find_files("Sphere-Packing-Lean-main/**/blueprint/src/**/*.tex"):
        for node in parse_blueprint_nodes(
            tex.read_text(encoding="utf-8", errors="replace")
        ):
            for name in node["lean_names"]:
                blueprint_prose.setdefault(name, node["statement"])

    items: list[dict[str, Any]] = []
    files_with_sorry = 0
    for path in lean_files:
        obligations = extract_sorry_obligations(
            path.read_text(encoding="utf-8", errors="replace")
        )
        if obligations:
            files_with_sorry += 1
        for ob in obligations:
            name = ob["name"]
            items.append(
                make_item(
                    id=f"sphere_packing:{name}@{path.stem}:{ob['line']}",
                    kind="formalization",
                    informal=blueprint_prose.get(name, ""),
                    formal=ob["signature"],
                    expected=_formal_expected(ob["signature"], name),
                    grading={
                        "track": "formalization",
                        "method": "comparator_or_statement",
                    },
                    provenance={
                        "corpus": "sphere_packing",
                        "lean_name": name,
                        "kind": ob["kind"],
                        "line": ob["line"],
                        "has_blueprint": name in blueprint_prose,
                        "path": rel(path),
                    },
                )
            )
    _log_counts(
        "sphere_packing",
        len(items),
        0,
        f"{files_with_sorry} files with sorry",
    )
    return items


# ===========================================================================
# NL / answer track
# ===========================================================================

def _ineqmath_answer_kind(rec: dict[str, Any]) -> str:
    return "relation" if rec.get("type") == "relation" else "bound"


def load_ineqmath() -> list[dict[str, Any]]:
    """IneqMath bound + relation problems (dev/train, whatever JSON is vendored).

    ``bound`` answers grade by deterministic symbolic equivalence; ``relation``
    by canonical option match — both via the existing grader.
    """
    files = find_files(
        "ineqmath-main/**/data/json/training_data_sampled_200.json",
        "ineqmath-main/**/data/json/dev.json",
        "ineqmath-main/**/data/json/test.json",
        "ineqmath-main/**/data/json/train.json",
    )
    if not files:
        _log_counts("ineqmath", 0, 0, "corpus absent")
        return []
    items: list[dict[str, Any]] = []
    skipped = 0
    seen: set[str] = set()
    for path in files:
        try:
            data = json.loads(path.read_text(encoding="utf-8", errors="replace"))
        except json.JSONDecodeError:
            skipped += 1
            continue
        records = data if isinstance(data, list) else data.get("data", [])
        split = "dev" if "dev" in path.name else "test" if "test" in path.name else "train"
        for rec in records:
            rid = str(rec.get("data_id") or rec.get("annot_id") or rec.get("id") or "")
            if not rid:
                skipped += 1
                continue
            uid = f"ineqmath:{split}:{rid}"
            if uid in seen:
                continue
            seen.add(uid)
            atype = rec.get("type", "bound")
            answer_kind = _ineqmath_answer_kind(rec)
            items.append(
                make_item(
                    id=uid,
                    kind="nl_answer",
                    informal=rec.get("problem", ""),
                    expected={
                        "answer": rec.get("answer", ""),
                        "answer_kind": answer_kind,
                        "type": atype,
                        "choices": rec.get("choices"),
                    },
                    grading={
                        "track": "nl_answer",
                        "method": "deterministic_symbolic",
                        "answer_kind": answer_kind,
                    },
                    provenance={
                        "corpus": "ineqmath",
                        "type": atype,
                        "data_split": rec.get("data_split", split),
                        "path": rel(path),
                    },
                )
            )
    _log_counts("ineqmath", len(items), skipped)
    return items


def _load_aime(corpus: str, glob_prefix: str) -> list[dict[str, Any]]:
    """AIME integer-answer problems. The vendored repos ship only a PDF *data
    card* (no problem/answer table), so this loads structured problem files
    (json/jsonl/csv) if any are present and otherwise skips cleanly."""
    files = find_files(
        f"{glob_prefix}/**/*.jsonl",
        f"{glob_prefix}/**/*.json",
        f"{glob_prefix}/**/problems*.csv",
    )
    items: list[dict[str, Any]] = []
    skipped = 0
    seen: set[str] = set()
    for path in files:
        for rec in _read_records(path):
            answer = rec.get("answer") or rec.get("solution") or rec.get("final_answer")
            problem = rec.get("problem") or rec.get("question")
            if answer is None or problem is None:
                skipped += 1
                continue
            rid = str(rec.get("id") or rec.get("problem_id") or f"{path.stem}-{len(items)}")
            uid = f"{corpus}:{rid}"
            if uid in seen:
                continue
            seen.add(uid)
            items.append(
                make_item(
                    id=uid,
                    kind="nl_answer",
                    informal=str(problem),
                    expected={"answer": str(answer), "answer_kind": "integer"},
                    grading={
                        "track": "nl_answer",
                        "method": "integer_match",
                        "answer_kind": "integer",
                    },
                    provenance={"corpus": corpus, "path": rel(path)},
                )
            )
    note = "" if items else "no structured problems (PDF-only data card)"
    _log_counts(corpus, len(items), skipped, note)
    return items


def _read_records(path: Path) -> list[dict[str, Any]]:
    text = path.read_text(encoding="utf-8", errors="replace")
    if path.suffix == ".jsonl":
        return [json.loads(ln) for ln in text.splitlines() if ln.strip()]
    if path.suffix == ".csv":
        return list(csv.DictReader(text.splitlines()))
    try:
        data = json.loads(text)
    except json.JSONDecodeError:
        return []
    if isinstance(data, list):
        return [r for r in data if isinstance(r, dict)]
    if isinstance(data, dict):
        return data.get("data") or data.get("records") or []
    return []


def load_aime24() -> list[dict[str, Any]]:
    return _load_aime("aime24", "aime24-main")


def load_aime25() -> list[dict[str, Any]]:
    return _load_aime("aime25", "aime25-main")


def load_aime26() -> list[dict[str, Any]]:
    return _load_aime("aime26", "aime26-master")


# ===========================================================================
# Falsification / critic track
# ===========================================================================

def load_brokenmath() -> list[dict[str, Any]]:
    """10 adversarially-corrupted competition problems. Grade = did the response
    detect the flaw (counterexample / "this is false") rather than "prove" it."""
    files = find_files(
        "alethfeld-legacy/**/examples/brokenmath/brokenmath_selected_10.json"
    )
    if not files:
        _log_counts("brokenmath", 0, 0, "corpus absent")
        return []
    items: list[dict[str, Any]] = []
    skipped = 0
    for path in files:
        try:
            data = json.loads(path.read_text(encoding="utf-8", errors="replace"))
        except json.JSONDecodeError:
            skipped += 1
            continue
        for rec in data:
            pid = rec.get("problem_id")
            if not pid:
                skipped += 1
                continue
            corrupted = rec.get("problem", "")
            original = rec.get("original_problem", "")
            items.append(
                make_item(
                    id=f"brokenmath:{pid}",
                    kind="falsification",
                    informal=corrupted,
                    expected={
                        "mode": "detect_flaw",
                        "is_adversarial": bool(rec.get("is_adversarial", True)),
                        "original_problem": original,
                        "solution": rec.get("solution", ""),
                        "gold_answer": rec.get("gold_answer"),
                    },
                    grading={"track": "falsification", "method": "flaw_detection"},
                    provenance={
                        "corpus": "brokenmath",
                        "problem_id": pid,
                        "question_type": rec.get("question_type"),
                        "path": rel(path),
                    },
                )
            )
    _log_counts("brokenmath", len(items), skipped, "baseline 5/10")
    return items


def load_goldbach_collatz() -> list[dict[str, Any]]:
    """Negative fixture: a crank "proof" the pipeline MUST reject (nothing
    compiles). One item; correctness = the response rejects it."""
    d = find_dir("goldbach-collatz-proof-main/**", "goldbach-collatz-proof-main")
    main = find_files(
        "goldbach-collatz-proof-main/**/main.tex",
        "goldbach-collatz-proof-main/**/README.md",
    )
    if not main:
        _log_counts("goldbach_collatz", 0, 0, "corpus absent")
        return []
    path = main[0]
    excerpt = path.read_text(encoding="utf-8", errors="replace")[:4000]
    item = make_item(
        id="goldbach_collatz:crank",
        kind="falsification",
        informal=(
            "A submitted manuscript claiming a simultaneous elementary proof of "
            "both the Goldbach and Collatz conjectures. Determine whether it is a "
            "valid, machine-checkable proof."
        ),
        formal=None,
        expected={
            "mode": "reject",
            "verdict": "reject",
            "reason": "crank artifact; nothing compiles / not a valid proof",
            "excerpt": excerpt,
        },
        grading={"track": "falsification", "method": "must_reject"},
        provenance={
            "corpus": "goldbach_collatz",
            "path": rel(path),
            "dir": rel(d) if d else None,
        },
    )
    _log_counts("goldbach_collatz", 1, 0, "negative fixture")
    return [item]


# Registry name -> loader callable.
LOADERS: dict[str, Callable[[], list[dict[str, Any]]]] = {
    "formalqualbench": load_formalqualbench,
    "sphere_packing": load_sphere_packing,
    "zklinalg": load_zklinalg,
    "strongpnt": load_strongpnt,
    "kakeya": load_kakeya,
    "riemann_hypothesis_curves": load_riemann_hypothesis_curves,
    "frontiermath_hypergraphs": load_frontiermath_hypergraphs,
    "erdos1196": load_erdos1196,
    "ineqmath": load_ineqmath,
    "aime24": load_aime24,
    "aime25": load_aime25,
    "aime26": load_aime26,
    "brokenmath": load_brokenmath,
    "goldbach_collatz": load_goldbach_collatz,
}
