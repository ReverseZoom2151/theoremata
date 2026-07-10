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

from .parsers_extra import (
    extract_lean_headers,
    extract_problem_comment,
    parse_external_provenance,
)
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
    # resources/ is gitignored, so also read a committed fixture beside this
    # loader (inert until a maintainer drops real data/aimeXX.jsonl there).
    committed = Path(__file__).parent / "data" / f"{corpus}.jsonl"
    if committed.exists():
        files = [committed, *files]
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
# Proof-completion track (MiniF2F / Harmonic)
# ===========================================================================

_MINIF2F_SPLITS: dict[str, list[str]] = {
    "train": [
        "datasets-main/**/MiniF2F/train.json",
        "datasets-main/**/minif2f/train.json",
    ],
    "valid": [
        "datasets-main/**/MiniF2F/validation.json",
        "datasets-main/**/MiniF2F/valid.json",
        "datasets-main/**/minif2f/validation.json",
    ],
    "test": [
        "datasets-main/**/MiniF2F/test.json",
        "datasets-main/**/minif2f/test.json",
    ],
}


def _minif2f_lean_name(rec: dict[str, Any]) -> str:
    name = str(rec.get("name") or "").strip()
    if name:
        return name
    return str(rec.get("id") or "MiniF2F.unknown")


def _load_minif2f_split(split: str) -> list[dict[str, Any]]:
    """Harmonic Lean 4 MiniF2F: NL + formal theorem pairs ending in ``by sorry``."""
    patterns = _MINIF2F_SPLITS.get(split)
    if not patterns:
        raise ValueError(f"unknown MiniF2F split: {split!r}")
    files = find_files(*patterns)
    if not files:
        _log_counts(f"minif2f_{split}", 0, 0, "corpus absent")
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
        if not isinstance(data, list):
            skipped += 1
            continue
        for rec in data:
            if not isinstance(rec, dict):
                skipped += 1
                continue
            rid = rec.get("id")
            formal = str(rec.get("formal") or "").strip()
            if rid is None or not formal:
                skipped += 1
                continue
            lean_name = _minif2f_lean_name(rec)
            uid = f"minif2f:{split}:{rid}"
            if uid in seen:
                continue
            seen.add(uid)
            items.append(
                make_item(
                    id=uid,
                    kind="formalization",
                    informal=str(rec.get("natural") or ""),
                    formal=formal,
                    expected={
                        "formal_statement": formal,
                        "lean_name": lean_name,
                        "axioms_whitelist": list(AXIOMS_WHITELIST),
                        "minif2f_id": rid,
                        "minif2f_name": rec.get("name"),
                    },
                    grading={
                        "track": "formalization",
                        "method": "comparator_or_statement",
                        "task": "proof_completion",
                    },
                    provenance={
                        "corpus": "minif2f",
                        "split": split,
                        "minif2f_id": rid,
                        "name": rec.get("name"),
                        "path": rel(path),
                    },
                )
            )
    _log_counts(f"minif2f_{split}", len(items), skipped)
    return items


def load_minif2f_train() -> list[dict[str, Any]]:
    return _load_minif2f_split("train")


def load_minif2f_valid() -> list[dict[str, Any]]:
    return _load_minif2f_split("valid")


def load_minif2f_test() -> list[dict[str, Any]]:
    return _load_minif2f_split("test")


def load_minif2f() -> list[dict[str, Any]]:
    """All MiniF2F splits concatenated (train, then valid, then test)."""
    out: list[dict[str, Any]] = []
    for split in ("train", "valid", "test"):
        out.extend(_load_minif2f_split(split))
    _log_counts("minif2f", len(out), 0, "all splits")
    return out


# ===========================================================================
# Verified programming (BRIDGE-178)
# ===========================================================================

def load_bridge178() -> list[dict[str, Any]]:
    """BRIDGE-178: NL problem + Lean signatures + executable oracle I/O pairs."""
    files = find_files("BRIDGE-main/**/bridge178.jsonl", "BRIDGE-main/**/datasets/bridge178.jsonl")
    if not files:
        _log_counts("bridge178", 0, 0, "corpus absent")
        return []
    items: list[dict[str, Any]] = []
    skipped = 0
    seen: set[str] = set()
    for path in files:
        for rec in _read_records(path):
            task_id = rec.get("task_id") or rec.get("id")
            stmt = rec.get("problem_statement") or rec.get("problem")
            lean_meta = rec.get("lean") or {}
            tests = rec.get("tests") or {}
            if not task_id or not stmt:
                skipped += 1
                continue
            uid = f"bridge178:{task_id}"
            if uid in seen:
                continue
            seen.add(uid)
            # Real dataset key is `function_signature` (singular string); keep
            # `signatures`/`signature` as back-compat fallbacks.
            signatures: list[str] = []
            arguments: list[str] = []
            argument_types: list[str] = []
            function_name = None
            if isinstance(lean_meta, dict):
                sig = (
                    lean_meta.get("function_signature")
                    or lean_meta.get("signatures")
                    or lean_meta.get("signature")
                    or []
                )
                if isinstance(sig, str):
                    signatures = [sig]
                elif isinstance(sig, list):
                    signatures = [s for s in sig if s]
                arguments = lean_meta.get("arguments") or []
                argument_types = lean_meta.get("argument_types") or []
                function_name = lean_meta.get("function_name")
            # `tests.inputs` are named-kwarg dicts (keyed by arg name); an oracle
            # runner must bind by name via `arguments`, not by position.
            items.append(
                make_item(
                    id=uid,
                    kind="verified_programming",
                    informal=str(stmt),
                    formal=None,
                    expected={
                        "lean_signatures": signatures,
                        "function_name": function_name,
                        "arguments": arguments,
                        "argument_types": argument_types,
                        "oracle_tests": {
                            "inputs": tests.get("inputs") or tests.get("input"),
                            "expected_outputs": tests.get("expected_outputs")
                            or tests.get("outputs"),
                            "bind": "kwargs",
                            "arguments": arguments,
                        },
                        "python": rec.get("python"),
                        "prompt_variants": ["direct", "functional", "theorem", "proof"],
                    },
                    grading={
                        "track": "verified_programming",
                        "method": "signature_and_oracle",
                    },
                    provenance={
                        "corpus": "bridge178",
                        "task_id": task_id,
                        "dataset_id": rec.get("dataset_id"),
                        "difficulty": rec.get("difficulty"),
                        "tags": rec.get("tags"),
                        "path": rel(path),
                    },
                )
            )
    _log_counts("bridge178", len(items), skipped)
    return items


# ===========================================================================
# Scientific formalization (QuantumLean-Bench)
# ===========================================================================

def _load_quantumlean(domain: str | None = None) -> list[dict[str, Any]]:
    files = find_files(
        "QuantumLean-Bench-main/**/BenchmarkData/**/*_problems.json",
        "QuantumLean-Bench-main/**/BenchmarkData/**/mitocw*.json",
    )
    if not files:
        label = f"quantumlean_{domain}" if domain else "quantumlean"
        _log_counts(label, 0, 0, "corpus absent")
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
        for rec in records:
            if not isinstance(rec, dict):
                skipped += 1
                continue
            dom = str(rec.get("domain") or path.stem)
            if domain and domain.lower() not in dom.lower():
                continue
            rid = rec.get("id")
            problem = rec.get("problem")
            if rid is None or not problem:
                skipped += 1
                continue
            uid = f"quantumlean:{dom}:{rid}"
            if uid in seen:
                continue
            seen.add(uid)
            # `solution_formal`/`solution_informal` are DICTS keyed by model name
            # (e.g. "gpt5.4_response") — model outputs, NOT a gold reference. There
            # is no gold formal proof anywhere in the corpus, so we do NOT stringify
            # them into `formal`; the problem statement is the item and the human
            # 0-2 `manual_eval` rubric is the intended scoring channel.
            formal_responses = rec.get("solution_formal")
            informal_responses = rec.get("solution_informal")
            formal_responses = (
                formal_responses if isinstance(formal_responses, dict) else {}
            )
            informal_responses = (
                informal_responses if isinstance(informal_responses, dict) else {}
            )
            response_model_keys = sorted(
                set(formal_responses) | set(informal_responses)
            )
            items.append(
                make_item(
                    id=uid,
                    kind="scientific_formalization",
                    informal=str(problem),
                    formal=None,  # no gold formal proof exists in this corpus
                    expected={
                        "mode": "typecheck_only",
                        "gold_present": False,
                        "problem": str(problem),
                        "model_responses_formal": formal_responses,
                        "model_responses_informal": informal_responses,
                        "response_model_keys": response_model_keys,
                        "manual_eval": rec.get("manual_eval"),
                        "type": rec.get("type"),
                        "axioms_whitelist": list(AXIOMS_WHITELIST),
                    },
                    grading={
                        "track": "formalization",
                        "method": "typecheck_only",
                        "task": "scientific_formalization",
                        "domain": dom,
                        "type": rec.get("type"),
                    },
                    provenance={
                        "corpus": "quantumlean",
                        "domain": dom,
                        "source": rec.get("source"),
                        "type": rec.get("type"),
                        "metadata": rec.get("metadata"),
                        "citations": rec.get("citations"),
                        "path": rel(path),
                    },
                )
            )
    label = f"quantumlean_{domain}" if domain else "quantumlean"
    _log_counts(label, len(items), skipped)
    return items


def load_quantumlean() -> list[dict[str, Any]]:
    return _load_quantumlean(None)


def load_quantumlean_physics() -> list[dict[str, Any]]:
    return _load_quantumlean("Physics")


# ===========================================================================
# Statement targets (Millennium Prize)
# ===========================================================================

def load_millennium() -> list[dict[str, Any]]:
    """Clay Millennium statements — definition/statement quality, not proof completion."""
    files = find_files(
        "LeanMillenniumPrizeProblems-main/**/Problems/**/*.lean",
        "LeanMillenniumPrizeProblems-main/**/Millennium/**/*.lean",
    )
    if not files:
        _log_counts("millennium", 0, 0, "corpus absent")
        return []
    items: list[dict[str, Any]] = []
    skipped = 0
    refs = find_files("LeanMillenniumPrizeProblems-main/**/references/**/*.pdf")
    ref_index = {p.stem.lower(): rel(p) for p in refs}
    for path in files:
        src = path.read_text(encoding="utf-8", errors="replace")
        headers = extract_lean_headers(src)
        if not headers:
            skipped += 1
            continue
        primary = headers[-1]
        stem = path.stem.lower()
        uid = f"millennium:{path.parent.name}:{primary['name']}"
        items.append(
            make_item(
                id=uid,
                kind="statement_target",
                informal=extract_problem_comment(src) or path.stem,
                formal=primary["signature"],
                expected={
                    "mode": "statement_quality",
                    "lean_name": primary["name"],
                    "headers": headers,
                    "reference_pdf": ref_index.get(stem),
                    "axioms_whitelist": list(AXIOMS_WHITELIST),
                },
                grading={
                    "track": "statement_target",
                    "method": "statement_preservation",
                },
                provenance={
                    "corpus": "millennium",
                    "problem_area": path.parent.name,
                    "lean_name": primary["name"],
                    "path": rel(path),
                    "reference_pdf": ref_index.get(stem),
                },
            )
        )
    _log_counts("millennium", len(items), skipped)
    return items


# ===========================================================================
# Olympiad formalization (IMO 2025 statement-only)
# ===========================================================================

def load_imo2025() -> list[dict[str, Any]]:
    """Harmonic IMO 2025 `StatementOnly_*` files as proof obligations."""
    files = find_files("IMO2025-main/**/StatementOnly_*.lean")
    if not files:
        _log_counts("imo2025", 0, 0, "corpus absent")
        return []
    items: list[dict[str, Any]] = []
    skipped = 0
    for path in sorted(files):
        src = path.read_text(encoding="utf-8", errors="replace")
        headers = extract_lean_headers(src)
        if not headers:
            skipped += 1
            continue
        primary = headers[-1]
        problem = extract_problem_comment(src)
        ref_glob = path.name.replace("StatementOnly_", "")
        ref_files = find_files(f"IMO2025-main/**/IMO2025{ref_glob}")
        uid = f"imo2025:{path.stem}"
        items.append(
            make_item(
                id=uid,
                kind="formalization",
                informal=problem,
                formal=primary["signature"],
                expected={
                    "formal_statement": primary["signature"],
                    "lean_name": primary["name"],
                    "reference_proof_path": rel(ref_files[0]) if ref_files else None,
                    "axioms_whitelist": list(AXIOMS_WHITELIST),
                },
                grading={
                    "track": "formalization",
                    "method": "comparator_or_statement",
                    "task": "olympiad_formalization",
                },
                provenance={
                    "corpus": "imo2025",
                    "statement_file": rel(path),
                    "reference_proof": rel(ref_files[0]) if ref_files else None,
                },
            )
        )
    _log_counts("imo2025", len(items), skipped)
    return items


# ===========================================================================
# External prover artifacts (Putnam 2025 / Aristotle outputs)
# ===========================================================================

def load_putnam_artifacts() -> list[dict[str, Any]]:
    """Aristotle Putnam 2025 generated Lean + LaTeX inputs as trust-but-verify fixtures."""
    outputs = find_files("aristotle_putnam25-main/**/aristotle_outputs/*.lean")
    if not outputs:
        _log_counts("putnam_artifacts", 0, 0, "corpus absent")
        return []
    inputs = {
        p.stem.replace("aristotle_putnam25_", ""): p
        for p in find_files("aristotle_putnam25-main/**/inputs/*.tex")
    }
    items: list[dict[str, Any]] = []
    for path in sorted(outputs):
        src = path.read_text(encoding="utf-8", errors="replace")
        headers = extract_lean_headers(src)
        primary = headers[-1] if headers else {"name": path.stem, "signature": ""}
        key = path.stem.replace("aristotle_putnam25_", "").replace("aristotle_", "")
        tex = inputs.get(key)
        uid = f"putnam_artifact:{path.stem}"
        items.append(
            make_item(
                id=uid,
                kind="external_artifact",
                informal=tex.read_text(encoding="utf-8", errors="replace")[:4000] if tex else "",
                formal=src,
                expected={
                    "mode": "trust_but_verify",
                    "lean_name": primary["name"],
                    "headers": headers,
                    "provenance": parse_external_provenance(src),
                    "axioms_whitelist": list(AXIOMS_WHITELIST),
                    "input_tex": rel(tex) if tex else None,
                },
                grading={
                    "track": "external_artifact",
                    "method": "structural_and_axiom_gate",
                },
                provenance={
                    "corpus": "putnam_artifacts",
                    "output_lean": rel(path),
                    "input_tex": rel(tex) if tex else None,
                    "external_prover": "aristotle",
                },
            )
        )
    _log_counts("putnam_artifacts", len(items), 0)
    return items


# ===========================================================================
# MILP reformulation (FormulationBench / FLARE)
# ===========================================================================

def load_formulationbench() -> list[dict[str, Any]]:
    """FLARE FormulationBench reformulation pairs from ``dataset/dataset.json``."""
    files = find_files("flare-main/**/dataset/dataset.json")
    if not files:
        _log_counts("formulationbench", 0, 0, "corpus absent")
        return []
    path = files[0]
    try:
        data = json.loads(path.read_text(encoding="utf-8", errors="replace"))
    except json.JSONDecodeError:
        _log_counts("formulationbench", 0, 1, "invalid dataset.json")
        return []
    pairs = data.get("reformulations") or []
    items: list[dict[str, Any]] = []
    skipped = 0
    for idx, rec in enumerate(pairs):
        if not isinstance(rec, dict):
            skipped += 1
            continue
        a = rec.get("a")
        b = rec.get("b")
        if not isinstance(a, dict) or not isinstance(b, dict):
            skipped += 1
            continue
        rid = f"p{a.get('problem')}-{a.get('formulation')}_p{b.get('problem')}-{b.get('formulation')}"
        uid = f"formulationbench:{rid}"
        items.append(
            make_item(
                id=uid,
                kind="reformulation",
                informal=f"Problem {a.get('problem')}: reformulation {a.get('formulation')} vs {b.get('formulation')}",
                formal=None,
                expected={
                    "formulation_a": a,
                    "formulation_b": b,
                    "is_reformulation": bool(rec.get("reformulation", True)),
                    "response_keys": ["response_key"],
                },
                grading={
                    "track": "reformulation",
                    "method": "equivalence_claim",
                },
                provenance={
                    "corpus": "formulationbench",
                    "pair_index": idx,
                    "pair_id": rid,
                    "path": rel(path),
                },
            )
        )
    _log_counts("formulationbench", len(items), skipped)
    return items


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


# ===========================================================================
# Proof-grading / evaluator-calibration track (IMO-ProofBench)
# ===========================================================================

def _as_int(v: Any) -> int | None:
    try:
        return int(v)
    except (TypeError, ValueError):
        return None


def _as_float(v: Any) -> float | None:
    try:
        return float(v)
    except (TypeError, ValueError):
        return None


def load_imo_proofbench() -> list[dict[str, Any]]:
    """IMO-ProofBench (DeepSeek-Math-V2 release): 60 olympiad problems
    (Basic + Advanced) shipping a reference solution, human grading guidelines,
    a model proof, a **gold human rating** (0-7 IMO scale) and the model's own
    **automatic rating** (0-1).

    Ground truth = reference solution + grading guidelines; the (gold human
    grade, model grade) pair makes this double as an EVALUATOR-CALIBRATION set
    for the proof_calibration layer (does an automated grader agree with the
    human?). Kind = ``proof_grading`` (grade the grader, not the proof).
    """
    files = find_files(
        "DeepSeek-Math-V2-main/**/outputs/IMO-ProofBench-*.jsonl",
    )
    if not files:
        _log_counts("imo_proofbench", 0, 0, "corpus absent")
        return []
    items: list[dict[str, Any]] = []
    skipped = 0
    seen: set[str] = set()
    for path in files:
        # "IMO-ProofBench-Basic.jsonl" -> "Basic"
        split = path.stem.replace("IMO-ProofBench-", "") or path.stem
        for rec in _read_records(path):
            pid = rec.get("problem_idx") or rec.get("id")
            question = rec.get("question") or rec.get("problem")
            if not pid or not question:
                skipped += 1
                continue
            uid = f"imo_proofbench:{pid}"
            if uid in seen:
                continue
            seen.add(uid)
            pred = rec.get("model_prediction")
            pred = pred if isinstance(pred, dict) else {}
            human_rating = _as_int(pred.get("human_rating"))
            auto_rating = _as_float(pred.get("average_automatic_rating"))
            items.append(
                make_item(
                    id=uid,
                    kind="proof_grading",
                    informal=str(question),
                    formal=None,
                    expected={
                        "mode": "proof_grading_calibration",
                        "reference_solution": rec.get("solution", ""),
                        "grading_guidelines": rec.get("grading guidelines", ""),
                        # gold human grade (IMO 0-7 points scale)
                        "gold_human_rating": human_rating,
                        "gold_scale": "0-7",
                        # the proof under evaluation + the model's own grade
                        "prediction_proof": pred.get("proof", ""),
                        "model_auto_rating": auto_rating,
                        "model_scale": "0-1",
                        "level": rec.get("level"),
                        "problem_type": rec.get("type"),
                        "source": rec.get("source"),
                    },
                    grading={
                        "track": "proof_grading",
                        "method": "grade_calibration",
                        "split": split,
                    },
                    provenance={
                        "corpus": "imo_proofbench",
                        "problem_idx": pid,
                        "split": split,
                        "level": rec.get("level"),
                        "type": rec.get("type"),
                        "source": rec.get("source"),
                        "path": rel(path),
                    },
                )
            )
    _log_counts("imo_proofbench", len(items), skipped, "gold+model grade pairs")
    return items


# ===========================================================================
# IMO-Bench — AnswerBench (verifiable-answer) + GradingBench (autograder cal.)
# ===========================================================================
#
# The IMO-Bench suite (Luong et al., Google DeepMind, EMNLP 2025;
# https://imobench.github.io) ships three tracks. IMO-ProofBench is loaded
# above (it happens to be re-released inside DeepSeek-Math-V2). The other two —
# IMO-AnswerBench (400 perturbed short-answer problems) and IMO-GradingBench
# (1000 human-graded 0-7 solutions) — are loaded here from a vendored IMO-Bench
# corpus. As with every loader these are ingested purely by glob and degrade to
# ``[]`` when the corpus is absent. All record text is treated as untrusted.

# Robustification / perturbation families used by IMO-AnswerBench (paper §2.2).
_ANSWERBENCH_PERTURBATIONS = (
    "paraphrase",
    "rename",
    "resubstitute",
    "distractor",
    "renumber",
    "reformulate",
    "original",
)

# corpus answer_type -> the grade route understood by grade_answer_match.
_ANSWERBENCH_KIND_ROUTE = {
    "integer": "integer",
    "int": "integer",
    "count": "integer",
    "numeric": "symbolic",
    "number": "symbolic",
    "real": "symbolic",
    "expression": "symbolic",
    "algebraic": "symbolic",
    "symbolic": "symbolic",
    "relation": "relation",
    "set": "set",
    "list": "list",
    "tuple": "list",
    "string": "string",
    "text": "string",
}


def _answerbench_answer_kind(rec: dict[str, Any], answer: str) -> str:
    """Pick the answer-matching route for an AnswerBench record.

    Honour an explicit ``answer_type``/``answer_kind`` field; otherwise infer a
    conservative default (a bare integer grades as ``integer``, anything else as
    ``symbolic`` so numeric/format variation still matches)."""
    declared = str(
        rec.get("answer_type") or rec.get("answer_kind") or rec.get("type") or ""
    ).strip().lower()
    if declared in _ANSWERBENCH_KIND_ROUTE:
        return _ANSWERBENCH_KIND_ROUTE[declared]
    stripped = (answer or "").strip().lstrip("+-")
    if stripped.isdigit():
        return "integer"
    return "symbolic"


def _norm_perturbation(value: Any) -> str | None:
    if value is None:
        return None
    p = str(value).strip().lower()
    return p or None


def load_imo_answerbench() -> list[dict[str, Any]]:
    """IMO-AnswerBench: 400 Olympiad short-answer problems, each expert/LLM
    *perturbed* (paraphrase / rename / resubstitute / distractor, per the paper)
    to defeat memorization. Every problem has a unique verifiable gold answer.

    Kind = ``nl_answer``; grading method = ``answer_match`` so the robust,
    format-resistant answer matcher (numeric / canonical-string / set-list
    equivalence) grades it rather than the plain IneqMath symbolic path.
    """
    files = find_files(
        "IMO-Bench-main/**/*answerbench*.jsonl",
        "IMO-Bench-main/**/*answerbench*.json",
        "IMO-Bench-main/**/*answer_bench*.jsonl",
        "IMO-AnswerBench-main/**/*.jsonl",
        "IMO-AnswerBench-main/**/*.json",
        "imobench-main/**/*answerbench*.jsonl",
        "imobench-main/**/*answerbench*.json",
    )
    if not files:
        _log_counts("imo_answerbench", 0, 0, "corpus absent")
        return []
    items: list[dict[str, Any]] = []
    skipped = 0
    seen: set[str] = set()
    for path in files:
        for rec in _read_records(path):
            if not isinstance(rec, dict):
                skipped += 1
                continue
            problem = rec.get("problem") or rec.get("question")
            answer = rec.get("answer")
            if answer is None:
                answer = rec.get("gold_answer") or rec.get("final_answer")
            if not problem or answer is None:
                skipped += 1
                continue
            rid = str(
                rec.get("id")
                or rec.get("problem_id")
                or rec.get("uid")
                or f"{path.stem}-{len(items)}"
            )
            uid = f"imo_answerbench:{rid}"
            if uid in seen:
                continue
            seen.add(uid)
            answer = str(answer)
            answer_kind = _answerbench_answer_kind(rec, answer)
            perturbation = _norm_perturbation(
                rec.get("perturbation")
                or rec.get("perturbation_type")
                or rec.get("robustification")
            )
            items.append(
                make_item(
                    id=uid,
                    kind="nl_answer",
                    informal=str(problem),
                    expected={
                        "answer": answer,
                        "answer_kind": answer_kind,
                        "choices": rec.get("choices"),
                        "perturbation": perturbation,
                        "original_problem": rec.get("original_problem")
                        or rec.get("original"),
                        "category": rec.get("category"),
                        "difficulty": rec.get("difficulty"),
                    },
                    grading={
                        "track": "answer_match",
                        "method": "answer_match",
                        "answer_kind": answer_kind,
                    },
                    provenance={
                        "corpus": "imo_answerbench",
                        "perturbation": perturbation,
                        "category": rec.get("category"),
                        "difficulty": rec.get("difficulty"),
                        "source": rec.get("source"),
                        "path": rel(path),
                    },
                )
            )
    _log_counts("imo_answerbench", len(items), skipped, "perturbed short-answer")
    return items


# Paper 4-way rubric: Correct=7, Almost=6, Partial=1, Incorrect=0 (humans may
# use any integer 0-7). Used to derive a gold bucket label from the human grade.
_GRADE_CANON_POINTS: tuple[tuple[int, str], ...] = (
    (0, "incorrect"),
    (1, "partial"),
    (6, "almost"),
    (7, "correct"),
)


def _grade_bucket_label(value: float | None) -> str | None:
    """Map a 0-7 grade to the nearest 4-way rubric bucket."""
    if value is None:
        return None
    return min(_GRADE_CANON_POINTS, key=lambda p: abs(p[0] - value))[1]


def load_imo_gradingbench() -> list[dict[str, Any]]:
    """IMO-GradingBench: ~1000 (problem, proposed solution, human grade 0-7)
    triples for training/evaluating auto-graders. Minimal-context by design
    (problem + solution only; usually no reference solution / no guidelines).

    Kind = ``proof_grading`` (grade the grader, not the proof); grading method =
    ``grading_correlation`` so the autograder-vs-human agreement grader scores a
    proposed grade against the gold human grade. Ties into proof_calibration.
    """
    files = find_files(
        "IMO-Bench-main/**/*gradingbench*.jsonl",
        "IMO-Bench-main/**/*gradingbench*.json",
        "IMO-Bench-main/**/*grading_bench*.jsonl",
        "IMO-GradingBench-main/**/*.jsonl",
        "IMO-GradingBench-main/**/*.json",
        "imobench-main/**/*gradingbench*.jsonl",
        "imobench-main/**/*gradingbench*.json",
    )
    if not files:
        _log_counts("imo_gradingbench", 0, 0, "corpus absent")
        return []
    items: list[dict[str, Any]] = []
    skipped = 0
    seen: set[str] = set()
    for path in files:
        for rec in _read_records(path):
            if not isinstance(rec, dict):
                skipped += 1
                continue
            problem = rec.get("problem") or rec.get("question")
            solution = (
                rec.get("solution")
                or rec.get("proposed_solution")
                or rec.get("candidate_solution")
                or rec.get("proof")
            )
            human = _as_int(
                rec.get("human_grade")
                if rec.get("human_grade") is not None
                else rec.get("human_rating")
                if rec.get("human_rating") is not None
                else rec.get("grade")
            )
            if not problem or not solution or human is None:
                skipped += 1
                continue
            rid = str(
                rec.get("id")
                or rec.get("problem_id")
                or rec.get("uid")
                or f"{path.stem}-{len(items)}"
            )
            uid = f"imo_gradingbench:{rid}"
            if uid in seen:
                continue
            seen.add(uid)
            items.append(
                make_item(
                    id=uid,
                    kind="proof_grading",
                    informal=str(problem),
                    formal=None,
                    expected={
                        "mode": "grading_bench_calibration",
                        "proposed_solution": str(solution),
                        # gold human grade (IMO 0-7 points scale) + rubric bucket
                        "gold_human_rating": human,
                        "gold_scale": "0-7",
                        "gold_bucket": _grade_bucket_label(float(human)),
                        # usually absent (minimal-context grading) but kept if shipped
                        "reference_solution": rec.get("reference_solution", ""),
                        "grading_guidelines": rec.get("grading_guidelines")
                        or rec.get("grading guidelines", ""),
                        "category": rec.get("category"),
                        "source": rec.get("source"),
                    },
                    grading={
                        "track": "proof_grading",
                        "method": "grading_correlation",
                        "category": rec.get("category"),
                    },
                    provenance={
                        "corpus": "imo_gradingbench",
                        "category": rec.get("category"),
                        "source": rec.get("source"),
                        "path": rel(path),
                    },
                )
            )
    _log_counts("imo_gradingbench", len(items), skipped, "human-graded solutions")
    return items


# ===========================================================================
# Classic-math proof-completion bench (zero-to-qed)
# ===========================================================================

def load_zero_to_qed() -> list[dict[str, Any]]:
    """Curated classic-math proofs from zero-to-qed (√2 irrational, infinitude
    of primes, pigeonhole, binomial theorem, Euclid's lemma, …) as a Lean
    formalization / proof-completion bench.

    Each ``Proofs/*.lean`` file → one item whose gold is the reference proof
    (full source). The manual-vs-automation pair (``InfinitudePrimes`` vs
    ``InfinitudePrimesGrind``) is preserved via a shared ``theorem_key`` and a
    ``strategy`` tag (manual | automation).
    """
    files = find_files("zero-to-qed-main/**/src/ZeroToQED/Proofs/*.lean")
    if not files:
        _log_counts("zero_to_qed", 0, 0, "corpus absent")
        return []
    items: list[dict[str, Any]] = []
    skipped = 0
    for path in sorted(files):
        src = path.read_text(encoding="utf-8", errors="replace")
        headers = extract_lean_headers(src)
        if not headers:
            skipped += 1
            continue
        primary = headers[-1]  # the file's headline theorem (last decl)
        stem = path.stem
        is_grind = stem.lower().endswith("grind")
        theorem_key = stem[:-5] if is_grind else stem
        strategy = "automation" if is_grind else "manual"
        uid = f"zero_to_qed:{stem}"
        items.append(
            make_item(
                id=uid,
                kind="formalization",
                informal=extract_problem_comment(src) or theorem_key,
                formal=primary["signature"],
                expected={
                    "formal_statement": primary["signature"],
                    "lean_name": primary["name"],
                    "reference_proof": src,
                    "headers": headers,
                    "theorem_key": theorem_key,
                    "strategy": strategy,
                    "axioms_whitelist": list(AXIOMS_WHITELIST),
                },
                grading={
                    "track": "formalization",
                    "method": "comparator_or_statement",
                    "task": "proof_completion",
                    "strategy": strategy,
                },
                provenance={
                    "corpus": "zero_to_qed",
                    "lean_name": primary["name"],
                    "theorem_key": theorem_key,
                    "strategy": strategy,
                    "n_headers": len(headers),
                    "path": rel(path),
                },
            )
        )
    _log_counts("zero_to_qed", len(items), skipped, "classic-proof bench")
    return items


# ===========================================================================
# Lean tactics knowledge base (zero-to-qed appendix_c_tactics.md)
# ===========================================================================

import re as _re  # local alias; keep module imports tidy

_TACTICS_TOC = _re.compile(
    r"^- \[`([^`]+)`\]\(#([^)]+)\)\s*-\s*(.*)$", _re.MULTILINE
)


def _parse_tactics_sections(md: str) -> dict[str, dict[str, str]]:
    """Map the first token of each ``### heading`` to its description + example."""
    sections: dict[str, dict[str, str]] = {}
    parts = _re.split(r"^### (.+)$", md, flags=_re.MULTILINE)
    # parts = [pre, heading1, body1, heading2, body2, ...]
    for i in range(1, len(parts), 2):
        heading = parts[i].strip()
        body = parts[i + 1] if i + 1 < len(parts) else ""
        key = heading.split()[0].lower() if heading.split() else heading.lower()
        # first prose paragraph (skip blank lines)
        desc = ""
        for para in _re.split(r"\n\s*\n", body):
            p = para.strip()
            if p and not p.startswith(("<figure", "```", "!", "|")):
                desc = _re.sub(r"\s+", " ", p)
                break
        m = _re.search(r"```lean\s*([\s\S]*?)```", body)
        example = m.group(1).strip() if m else ""
        sections.setdefault(key, {"description": desc, "example": example})
    return sections


def load_lean_tactics_kb() -> list[dict[str, Any]]:
    """Parse ``appendix_c_tactics.md`` (~60-80 Lean 4 / Mathlib tactics) into a
    structured tactic reference usable as a retrieval / knowledge-base corpus.

    Each entry: ``{tactic, purpose, example, description}``. The canonical tactic
    list + one-line purpose come from the document's table of contents; the
    longer description + a worked ``lean`` example are pulled from each tactic's
    section where present.
    """
    files = find_files("zero-to-qed-main/**/docs/src/appendix_c_tactics.md")
    if not files:
        _log_counts("lean_tactics_kb", 0, 0, "corpus absent")
        return []
    items: list[dict[str, Any]] = []
    skipped = 0
    seen: set[str] = set()
    for path in files:
        md = path.read_text(encoding="utf-8", errors="replace")
        sections = _parse_tactics_sections(md)
        for tactic, anchor, purpose in _TACTICS_TOC.findall(md):
            tactic = tactic.strip()
            key = tactic.split()[0].lower() if tactic.split() else tactic.lower()
            uid = f"lean_tactic:{tactic}"
            if not tactic or uid in seen:
                skipped += 1
                continue
            seen.add(uid)
            sec = sections.get(key, {})
            items.append(
                make_item(
                    id=uid,
                    kind="tactic_reference",
                    informal=purpose.strip(),
                    formal=None,
                    expected={
                        "tactic": tactic,
                        "purpose": purpose.strip(),
                        "description": sec.get("description", ""),
                        "example": sec.get("example", ""),
                        "anchor": anchor,
                    },
                    grading={
                        "track": "tactic_reference",
                        "method": "retrieval_reference",
                    },
                    provenance={
                        "corpus": "lean_tactics_kb",
                        "tactic": tactic,
                        "path": rel(path),
                    },
                )
            )
    _log_counts("lean_tactics_kb", len(items), skipped, "tactic KB entries")
    return items


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
    "minif2f": load_minif2f,
    "minif2f_train": load_minif2f_train,
    "minif2f_valid": load_minif2f_valid,
    "minif2f_test": load_minif2f_test,
    "bridge178": load_bridge178,
    "quantumlean": load_quantumlean,
    "quantumlean_physics": load_quantumlean_physics,
    "millennium": load_millennium,
    "imo2025": load_imo2025,
    "putnam_artifacts": load_putnam_artifacts,
    "formulationbench": load_formulationbench,
    "imo_proofbench": load_imo_proofbench,
    "imo_answerbench": load_imo_answerbench,
    "imo_gradingbench": load_imo_gradingbench,
    "zero_to_qed": load_zero_to_qed,
    "lean_tactics_kb": load_lean_tactics_kb,
}
