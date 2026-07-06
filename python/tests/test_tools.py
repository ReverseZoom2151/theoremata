import pytest

from theoremata_tools.falsify import search
from theoremata_tools.safe_eval import evaluate


def test_safe_evaluation():
    assert evaluate("sum(n*n for n in range(5))") == 30


def test_blocks_unsafe_syntax():
    with pytest.raises(ValueError):
        evaluate("__import__('os').system('true')")


def test_finds_counterexample():
    result = search(
        {"n": {"start": -5, "stop": 6}},
        claim="n * n < 10",
    )
    assert result["verdict"] == "counterexample"


def test_supports_assumptions():
    result = search(
        {"n": {"start": -100, "stop": 101}},
        assumptions="n % 2 == 0",
        claim="(n * n) % 2 == 0",
    )
    assert result["verdict"] == "no_counterexample_in_domain"
