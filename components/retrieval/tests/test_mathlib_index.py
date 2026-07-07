from theoremata_tools import mathlib_index as mi


def _make_tree(tmp_path):
    """A small Lean-ish source tree exercising imports, public import,
    external imports, and imports hidden inside comments."""
    base = tmp_path / "Mathlib"
    (base / "Algebra").mkdir(parents=True)
    (base / "Topology").mkdir(parents=True)
    (base / "Analysis").mkdir(parents=True)

    (base / "Algebra" / "Basic.lean").write_text(
        "-- foundational, no imports\ndef x := 1\n", encoding="utf-8"
    )
    (base / "Algebra" / "Group.lean").write_text(
        "import Mathlib.Algebra.Basic\nimport Init.Core\n\ndef g := 2\n",
        encoding="utf-8",
    )
    (base / "Topology" / "Basic.lean").write_text(
        "import Mathlib.Algebra.Group\n", encoding="utf-8"
    )
    (base / "Analysis" / "Calc.lean").write_text(
        "public import Mathlib.Algebra.Group\n", encoding="utf-8"
    )
    # A commented-out import (line comment) must NOT be counted.
    (base / "Topology" / "Empty.lean").write_text(
        "-- import Mathlib.Algebra.Basic\n\ndef y := 3\n", encoding="utf-8"
    )
    # A block-commented import must NOT be counted; the real one below must be.
    (base / "Topology" / "Block.lean").write_text(
        "/-\nimport Mathlib.Algebra.Basic\n-/\nimport Mathlib.Algebra.Group\n",
        encoding="utf-8",
    )
    return str(tmp_path)


def test_module_set(tmp_path):
    idx = mi.build_index(_make_tree(tmp_path))
    assert set(idx["modules"]) == {
        "Mathlib.Algebra.Basic",
        "Mathlib.Algebra.Group",
        "Mathlib.Topology.Basic",
        "Mathlib.Analysis.Calc",
        "Mathlib.Topology.Empty",
        "Mathlib.Topology.Block",
    }


def test_direct_imports_and_external(tmp_path):
    idx = mi.build_index(_make_tree(tmp_path))
    assert idx["imports"]["Mathlib.Algebra.Group"] == ["Mathlib.Algebra.Basic"]
    # Init.Core is not in-tree: excluded from the DAG, recorded as external.
    assert "Init.Core" not in idx["imports"]["Mathlib.Algebra.Group"]
    assert idx["external"]["Mathlib.Algebra.Group"] == ["Init.Core"]


def test_public_import(tmp_path):
    idx = mi.build_index(_make_tree(tmp_path))
    assert idx["imports"]["Mathlib.Analysis.Calc"] == ["Mathlib.Algebra.Group"]


def test_transitive_imports(tmp_path):
    idx = mi.build_index(_make_tree(tmp_path))
    assert set(mi.transitive_imports(idx, "Mathlib.Topology.Basic")) == {
        "Mathlib.Algebra.Group",
        "Mathlib.Algebra.Basic",
    }


def test_importers_and_transitive(tmp_path):
    idx = mi.build_index(_make_tree(tmp_path))
    assert mi.importers(idx, "Mathlib.Algebra.Basic") == ["Mathlib.Algebra.Group"]
    assert "Mathlib.Topology.Basic" in mi.transitive_importers(
        idx, "Mathlib.Algebra.Basic"
    )


def test_line_comment_import_ignored(tmp_path):
    idx = mi.build_index(_make_tree(tmp_path))
    assert mi.direct_imports(idx, "Mathlib.Topology.Empty") == []


def test_block_comment_import_ignored(tmp_path):
    idx = mi.build_index(_make_tree(tmp_path))
    # The block-commented `import ... Basic` is skipped; only Group is real.
    assert idx["imports"]["Mathlib.Topology.Block"] == ["Mathlib.Algebra.Group"]


def test_search(tmp_path):
    idx = mi.build_index(_make_tree(tmp_path))
    assert set(mi.search(idx, "algebra")) == {
        "Mathlib.Algebra.Basic",
        "Mathlib.Algebra.Group",
    }
    assert mi.search(idx, "calc") == ["Mathlib.Analysis.Calc"]


def test_run_stats(tmp_path):
    result = mi.run(_make_tree(tmp_path), "stats")
    assert result["modules"] == 6
    # edges: Group->Basic, Topology.Basic->Group, Calc->Group, Block->Group
    assert result["edges"] == 4
    assert result["most_imported"][0]["module"] == "Mathlib.Algebra.Group"


def test_run_search_and_module_query(tmp_path):
    root = _make_tree(tmp_path)
    assert "Mathlib.Analysis.Calc" in mi.run(root, "search", substring="calc")["matches"]
    res = mi.run(root, "transitive_imports", module="Mathlib.Topology.Basic")
    assert set(res["result"]) == {"Mathlib.Algebra.Group", "Mathlib.Algebra.Basic"}
