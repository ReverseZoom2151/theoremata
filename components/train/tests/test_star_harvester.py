import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.star_harvester import (  # noqa: E402
    content_hash,
    from_graph_export,
    harvest,
    is_verified,
    to_chat_row,
    write_jsonl,
)


def _trace(oid, verified=True, axioms_ok=True, **kw):
    rec = {
        "obligation_id": oid,
        "goal": f"goal-{oid}",
        "formal_statement": f"theorem t{oid} : True",
        "proof": f"proof-{oid}",
        "verified": {"compiled": verified, "axioms_ok": axioms_ok},
    }
    rec.update(kw)
    return rec


# --- verification predicate ------------------------------------------------

def test_is_verified_requires_compile_and_axioms():
    assert is_verified(_trace("a")) is True
    assert is_verified(_trace("a", verified=False)) is False
    assert is_verified(_trace("a", axioms_ok=False)) is False


def test_is_verified_flat_shape_and_axioms_default():
    # flat verified bool + absent axioms_ok defaults to True
    assert is_verified({"verified": True}) is True
    assert is_verified({"verified": True, "axioms_ok": False}) is False
    assert is_verified({"verified": False}) is False


# --- harvest: only verified obligations ------------------------------------

def test_harvest_emits_only_verified():
    traces = [
        _trace("1"),
        _trace("2", verified=False),   # failed compile -> skipped
        _trace("3", axioms_ok=False),  # failed axiom gate -> skipped
    ]
    out = harvest(traces)
    assert out["ok"] is True
    assert out["kept"] == 1
    assert out["skipped_unverified"] == 2
    row = out["rows"][0]
    assert row["prompt"] == "theorem t1 : True"  # formal statement is the prompt
    assert row["completion"] == "proof-1"
    assert row["meta"]["obligation_id"] == "1"


def test_harvest_prompt_falls_back_to_goal_without_formal():
    t = _trace("x")
    t["formal_statement"] = None
    out = harvest([t])
    assert out["rows"][0]["prompt"] == "goal-x"


# --- dedup by content hash -------------------------------------------------

def test_harvest_dedups_by_content_hash():
    a = _trace("1")
    b = _trace("2")  # different obligation id, but same prompt+completion...
    b["formal_statement"] = a["formal_statement"]
    b["proof"] = a["proof"]
    out = harvest([a, b])
    assert out["kept"] == 1
    assert out["dropped"] == 1


def test_content_hash_stable_and_pair_sensitive():
    assert content_hash("p", "c") == content_hash("p", "c")
    assert content_hash("p", "c") != content_hash("p", "c2")
    assert content_hash("ab", "") != content_hash("a", "b")  # separator matters


# --- reformulation adapter step -------------------------------------------

def test_harvest_emits_reformulation_row_when_present():
    t = _trace("r")
    t["reformulation"] = {"from_statement": "informal P", "to_statement": "formal P"}
    out = harvest([t])
    kinds = sorted(r["meta"]["kind"] for r in out["rows"])
    assert kinds == ["proof", "reformulation"]
    reform = next(r for r in out["rows"] if r["meta"]["kind"] == "reformulation")
    assert reform["completion"] == "formal P"
    assert "informal P" in reform["prompt"]


def test_reformulation_not_emitted_from_unverified_trace():
    t = _trace("r", verified=False)
    t["reformulation"] = {"from_statement": "a", "to_statement": "b"}
    out = harvest([t])
    assert out["kept"] == 0


# --- input shapes ----------------------------------------------------------

def test_harvest_accepts_wrapper_dict():
    out = harvest({"traces": [_trace("1")]})
    assert out["kept"] == 1


def test_from_graph_export_extracts_verified_proof():
    export = {
        "project": {"id": "p"},
        "nodes": [
            {
                "id": "obl1",
                "kind": "obligation",
                "status": "formally_verified",
                "statement": "S",
                "formal_statement": "theorem s : True",
            },
            {
                "id": "prf1",
                "kind": "formal_proof",
                "status": "formally_verified",
                "statement": "by trivial",
                "tainted": False,
            },
            {
                "id": "prf2",
                "kind": "formal_proof",
                "status": "proposed",  # not verified
                "statement": "by sorry",
                "tainted": True,
            },
        ],
        "edges": [
            {
                "source_id": "prf1",
                "target_id": "obl1",
                "kind": "verifies",
                "evidence_strength": "lean_checked",
            }
        ],
        "events": [],
    }
    traces = from_graph_export(export)
    assert len(traces) == 2
    out = harvest(export)
    assert out["kept"] == 1
    row = out["rows"][0]
    assert row["completion"] == "by trivial"
    assert row["prompt"] == "theorem s : True"


# --- conversion + io -------------------------------------------------------

def test_to_chat_row_shape():
    out = harvest([_trace("1")])
    chat = to_chat_row(out["rows"][0])
    assert chat["messages"][0]["role"] == "user"
    assert chat["messages"][1]["role"] == "assistant"
    assert chat["messages"][1]["content"] == "proof-1"


def test_write_jsonl_roundtrip(tmp_path):
    import json

    out = harvest([_trace("1")])
    path = tmp_path / "sft.jsonl"
    n = write_jsonl(out["rows"], str(path))
    assert n == 1
    line = path.read_text(encoding="utf-8").splitlines()[0]
    assert json.loads(line) == out["rows"][0]
