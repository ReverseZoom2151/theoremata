"""Tests for the lexical Lean soundness pre-gate."""
from __future__ import annotations

from theoremata_tools.lean_soundness import check, mask_comments_and_strings


def _critical(result):
    return [i for i in result["issues"] if i["severity"] == "critical"]


def _kinds(result):
    return {i["kind"] for i in result["issues"]}


def test_mask_preserves_length_and_newlines():
    src = 'a -- sorry\nb /- x -/ c\n"str"\n'
    masked = mask_comments_and_strings(src)
    assert len(masked) == len(src)
    assert masked.count("\n") == src.count("\n")
    # Newlines must survive at the same positions.
    assert [i for i, c in enumerate(src) if c == "\n"] == [
        i for i, c in enumerate(masked) if c == "\n"
    ]


def test_sorry_in_line_comment_not_flagged():
    src = "theorem t : True := by\n  -- sorry left as a note\n  trivial\n"
    result = check(src)
    assert result["clean"] is True
    assert result["issues"] == []


def test_sorry_in_block_comment_not_flagged():
    src = "theorem t : True := by\n  /- todo: sorry -/\n  trivial\n"
    assert check(src)["clean"] is True


def test_sorry_in_nested_block_comment_not_flagged():
    src = "/- outer /- inner sorry -/ still sorry -/\ntheorem t : True := trivial\n"
    assert check(src)["clean"] is True


def test_sorry_in_string_not_flagged():
    src = 'def msg : String := "this proof has no sorry inside"\n'
    assert check(src)["clean"] is True


def test_escaped_quote_in_string_not_flagged():
    src = 'def msg : String := "a \\" sorry still inside"\n'
    assert check(src)["clean"] is True


def test_real_sorry_is_flagged_with_location():
    src = "theorem t : True := by\n  sorry\n"
    result = check(src)
    assert result["clean"] is False
    crit = _critical(result)
    assert len(crit) == 1
    issue = crit[0]
    assert issue["kind"] == "placeholder"
    assert issue["token"] == "sorry"
    assert issue["line"] == 2
    assert issue["column"] == 2


def test_admit_is_flagged():
    src = "theorem t : True := by\n  admit\n"
    result = check(src)
    assert result["clean"] is False
    assert _critical(result)[0]["token"] == "admit"


def test_sorry_prime_and_prefixed_not_flagged():
    src = (
        "def sorry' : Nat := 0\n"
        "def my_sorry : Nat := 1\n"
        "def sorryish : Nat := 2\n"
        "theorem t : True := trivial\n"
    )
    assert check(src)["clean"] is True


def test_axiom_declaration_flagged():
    src = "axiom foo : True\n"
    result = check(src)
    assert result["clean"] is False
    assert _critical(result)[0]["kind"] == "forbidden_declaration"
    assert _critical(result)[0]["name"] == "foo"


def test_axiom_behind_attribute_flagged():
    src = "@[simp] axiom bar : True\n"
    result = check(src)
    assert result["clean"] is False
    assert _critical(result)[0]["name"] == "bar"


def test_noncomputable_axiom_flagged():
    src = "noncomputable axiom baz : True\n"
    result = check(src)
    assert result["clean"] is False
    assert "forbidden_declaration" in _kinds(result)


def test_constant_and_postulate_flagged():
    assert check("constant c : Nat\n")["clean"] is False
    assert check("postulate p : True\n")["clean"] is False


def test_noncomputable_def_is_info_only():
    src = "noncomputable def f : Nat := 0\n"
    result = check(src)
    assert result["clean"] is True
    assert _critical(result) == []
    assert "noncomputable" in _kinds(result)


def test_fully_clean_proof():
    src = (
        "import Mathlib\n\n"
        "theorem even_sq (n : Nat) (h : Even n) : Even (n * n) := by\n"
        "  obtain ⟨k, rfl⟩ := h\n"
        "  exact ⟨2 * k * k, by ring⟩\n"
    )
    result = check(src)
    assert result["clean"] is True
    assert result["issues"] == []
