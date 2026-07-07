import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from theoremata_tools.grader import (  # noqa: E402
    extract_answer,
    grade_answer,
    grade_samples,
    six_axis,
)


# --- extract_answer -------------------------------------------------------

def test_extract_marker_final_answer_is():
    assert extract_answer("blah blah. The final answer is 42.") == "42"


def test_extract_marker_answer_is():
    assert extract_answer("So the answer is (B)") == "(B)"


def test_extract_marker_final_answer():
    assert extract_answer("Final answer 17") == "17"


def test_extract_boxed_balanced():
    assert extract_answer(r"we conclude \boxed{\frac{1}{2}}") == r"\frac{1}{2}"


def test_extract_uses_last_marker():
    assert extract_answer("answer is 1 ... the final answer is 2") == "2"


def test_extract_none():
    assert extract_answer("no answer here") is None


# --- integer --------------------------------------------------------------

def test_integer_exact():
    assert grade_answer("42", "42", "integer")["correct"] is True


def test_integer_float_whole():
    assert grade_answer("42", "42.0", "integer")["correct"] is True


def test_integer_non_integer_incorrect():
    assert grade_answer("42", "42.5", "integer")["correct"] is False


def test_integer_wrong():
    assert grade_answer("42", "7", "integer")["correct"] is False


# --- symbolic -------------------------------------------------------------

def test_symbolic_expansion_equal():
    assert grade_answer("(x+1)**2", "x**2+2*x+1", "symbolic")["correct"] is True


def test_symbolic_decimal_not_exact():
    assert grade_answer("2*pi", "6.28", "symbolic")["correct"] is False


def test_symbolic_half_equals_point_five():
    assert grade_answer("1/2", "0.5", "symbolic")["correct"] is True


def test_symbolic_caret_notation():
    assert grade_answer("x^2 - 1", "(x-1)*(x+1)", "symbolic")["correct"] is True


# --- relation -------------------------------------------------------------

def test_relation_geq_variants():
    assert grade_answer(r"\geq", "(B) $\\geq$", "relation")["correct"] is True


def test_relation_mismatch():
    assert grade_answer("<=", ">=", "relation")["correct"] is False


# --- grade_samples --------------------------------------------------------

def test_grade_samples_metrics():
    res = grade_samples("42", ["42", "7", "42", "42"], "integer")
    assert res["k"] == 4
    assert res["pass_at_k"] is True
    assert res["majority_at_k"] is True  # 42 is the mode
    assert abs(res["averaged_at_k"] - 0.75) < 1e-9
    assert res["stderr"] > 0
    assert [s["correct"] for s in res["per_sample"]] == [True, False, True, True]


def test_grade_samples_majority_wrong():
    res = grade_samples("42", ["7", "7", "42"], "integer")
    assert res["pass_at_k"] is True  # one sample is 42
    assert res["majority_at_k"] is False  # mode is 7


# --- six_axis -------------------------------------------------------------

def test_six_axis_separate_axes_with_nulls():
    out = six_axis({"solved": True, "pass_k": True, "compiles": True, "no_sorry": True})
    assert out["discovery"] == {"solved": True, "pass_k": True, "majority_k": None}
    assert out["formal"] == {"compiles": True, "no_sorry": True}
    # axes with no inputs are null, not zero
    assert out["informal"] is None
    assert out["soundness"] is None
    assert out["efficiency"] is None
    assert out["novelty"] is None


def test_six_axis_informal_overall_is_and():
    good = six_axis({"answer_correct": True, "ntc": True, "nlg": True, "nae": True, "nce": True})
    assert good["informal"]["overall"] is True
    bad = six_axis({"answer_correct": True, "ntc": True, "nlg": False})
    assert bad["informal"]["overall"] is False


def test_six_axis_no_scalar_collapse():
    out = six_axis({"solved": True})
    # the result is a dict of axes, never a single number
    assert set(out) == {"discovery", "informal", "formal", "soundness", "efficiency", "novelty"}
