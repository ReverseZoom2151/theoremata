import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))

from theoremata_tools.lean_corpus import (  # noqa: E402
    SFT_SCHEMA,
    canonicalize_statement,
    decl_to_record,
    extract_corpus,
    parse_decls,
    parse_imports,
    run,
    split_tactics,
    strip_comments_and_strings,
)


# --- fixture builder -------------------------------------------------------

FILE_A = """\
import Mathlib.Data.Nat.Basic
import MyProj.Util

-- a proper tactic proof
theorem add_comm_nat (a b : Nat) : a + b = b + a := by
  rw [Nat.add_comm]

/- this theorem lives in a block comment and must be ignored
theorem ghost (z : Nat) : z = z := by rfl
-/

lemma zero_add_x (n : Nat) : 0 + n = n := by
  simp
"""

# Alpha-equivalent to add_comm_nat (only bound names differ) -> should dedup.
FILE_B = """\
import MyProj.Util

theorem add_comm_dup (x y : Nat) : x + y = y + x := by
  rw [Nat.add_comm]

-- term-mode proof (no `by`) -> skipped when tactic_only
theorem term_mode : True := trivial
"""

FILE_C = """\
import MyProj.Util

example (p : Prop) (h : p) : p := by
  exact h
"""


def _write_fixtures(tmp_path: Path) -> Path:
    root = tmp_path / "repo"
    (root / "MyProj").mkdir(parents=True)
    (root / "MyProj" / "A.lean").write_text(FILE_A, encoding="utf-8")
    (root / "MyProj" / "B.lean").write_text(FILE_B, encoding="utf-8")
    (root / "MyProj" / "C.lean").write_text(FILE_C, encoding="utf-8")
    return root


# --- comment / string scrubbing --------------------------------------------

def test_strip_comments_ignores_ghost_theorem():
    scrubbed = strip_comments_and_strings(FILE_A)
    assert "ghost" not in scrubbed
    assert "add_comm_nat" in scrubbed


def test_strip_handles_string_with_theorem_word():
    src = 'def s := "theorem not real := by sorry"\n'
    scrubbed = strip_comments_and_strings(src)
    assert "theorem" not in scrubbed


# --- decl + tactic parsing -------------------------------------------------

def test_parse_decls_finds_statement_and_tactics():
    decls = parse_decls(FILE_A)
    names = {d.name for d in decls}
    assert "add_comm_nat" in names
    assert "zero_add_x" in names
    assert "ghost" not in names  # was in a comment
    d = next(x for x in decls if x.name == "add_comm_nat")
    assert d.kind == "theorem"
    assert d.statement == "(a b : Nat) : a + b = b + a"
    assert d.is_tactic
    assert d.tactics == ["rw [Nat.add_comm]"]
    assert d.first_tactic == "rw [Nat.add_comm]"


def test_split_tactics_multi_step():
    steps = split_tactics("by\n  intro h\n  simp [a, b]\n  exact h")
    assert steps == ["intro h", "simp [a, b]", "exact h"]
    # combinator split, bracket-aware
    steps2 = split_tactics("by constructor <;> simp")
    assert steps2 == ["constructor", "simp"]


def test_example_decl_parsed():
    decls = parse_decls(FILE_C)
    assert len(decls) == 1
    assert decls[0].kind == "example"
    assert decls[0].tactics == ["exact h"]


# --- imports ---------------------------------------------------------------

def test_parse_imports():
    imps = parse_imports(FILE_A)
    assert imps == ["Mathlib.Data.Nat.Basic", "MyProj.Util"]


# --- canonicalization / dedup ----------------------------------------------

def test_canonicalize_alpha_equivalent():
    a = canonicalize_statement("(a b : Nat) : a + b = b + a")
    b = canonicalize_statement("(x y : Nat) : x + y = y + x")
    assert a == b


def test_canonicalize_distinguishes_real_difference():
    a = canonicalize_statement("(a b : Nat) : a + b = b + a")
    c = canonicalize_statement("(a b : Nat) : a * b = b * a")
    assert a != c


# --- record schema ---------------------------------------------------------

def test_decl_to_record_matches_chat_sft_schema():
    d = parse_decls(FILE_A)[0]
    d.module = "MyProj.A"
    rec = decl_to_record(d)
    assert set(rec.keys()) == {"messages", "meta"}
    msgs = rec["messages"]
    assert [m["role"] for m in msgs] == ["user", "assistant"]
    assert msgs[0]["content"] == d.statement
    assert msgs[1]["content"] == d.body
    assert rec["meta"]["schema"] == SFT_SCHEMA
    assert rec["meta"]["source"] == "lean_corpus"


def test_first_tactic_variant():
    d = parse_decls(FILE_A)[0]
    rec = decl_to_record(d, variant="first_tactic")
    assert rec["messages"][1]["content"] == d.first_tactic
    assert rec["meta"]["variant"] == "first_tactic"


# --- end-to-end extract ----------------------------------------------------

def test_extract_corpus_end_to_end(tmp_path):
    root = _write_fixtures(tmp_path)
    res = extract_corpus(str(root))
    stats = res["stats"]

    assert res["ok"] and res["schema"] == SFT_SCHEMA
    assert stats["n_files"] == 3
    # add_comm_nat, zero_add_x, add_comm_dup, term_mode, example = 5 decls
    assert stats["n_decls"] == 5
    # add_comm_dup is alpha-equal to add_comm_nat -> 1 dedup
    assert stats["n_deduped"] == 1

    # import graph captures MyProj.A -> MyProj.Util edge
    graph = res["import_graph"]
    assert "MyProj.Util" in graph["MyProj.A"]
    assert "Mathlib.Data.Nat.Basic" in graph["MyProj.A"]
    assert stats["import_edges"] >= 1

    # records: tactic decls minus the alpha-dup and minus the term-mode proof
    #   kept: add_comm_nat, zero_add_x, example  (add_comm_dup deduped,
    #   term_mode skipped as non-tactic)
    assert stats["n_records"] == 3
    for rec in res["records"]:
        assert rec["meta"]["schema"] == SFT_SCHEMA
        assert len(rec["messages"]) == 2


def test_extract_deterministic(tmp_path):
    root = _write_fixtures(tmp_path)
    r1 = extract_corpus(str(root))
    r2 = extract_corpus(str(root))
    assert json.dumps(r1, sort_keys=True) == json.dumps(r2, sort_keys=True)


# --- worker ops ------------------------------------------------------------

def test_run_extract_op(tmp_path):
    root = _write_fixtures(tmp_path)
    res = run({"op": "extract", "root": str(root)})
    assert res["ok"]
    assert res["stats"]["n_records"] == 3


def test_run_emit_jsonl_op(tmp_path):
    root = _write_fixtures(tmp_path)
    out = tmp_path / "corpus.jsonl"
    res = run({"op": "emit_jsonl", "root": str(root), "path": str(out)})
    assert res["ok"] and res["written"] == 3
    lines = out.read_text(encoding="utf-8").splitlines()
    assert len(lines) == 3
    for line in lines:
        obj = json.loads(line)
        assert obj["meta"]["schema"] == SFT_SCHEMA
        assert [m["role"] for m in obj["messages"]] == ["user", "assistant"]


def test_run_unknown_op_raises(tmp_path):
    try:
        run({"op": "nope"})
    except ValueError as e:
        assert "unknown op" in str(e)
    else:
        raise AssertionError("expected ValueError")
