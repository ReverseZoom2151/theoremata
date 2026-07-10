from theoremata_tools.benchmarks import list_benchmarks, load_benchmark


def test_external_formal_corpora_are_registered_and_degrade_cleanly():
    names = {entry["name"] for entry in list_benchmarks()}
    assert {"1lab", "metamath_100"} <= names
    assert isinstance(load_benchmark("1lab"), list)
    assert isinstance(load_benchmark("metamath_100"), list)
