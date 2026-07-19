"""Build the split ``theoremata_tools`` source tree into one wheel.

Runtime code remains owned by its component directories. The custom build step
only assembles those files into the distribution build directory, avoiding a
source-tree-only ``sys.path`` contract.
"""

from __future__ import annotations

from pathlib import Path

from setuptools import setup
from setuptools.command.build_py import build_py


ROOT = Path(__file__).parent
COMPONENT_PYTHON_ROOTS = (
    "components/eval/python",
    "components/prover/python",
    "components/provider/python",
    "components/reason/python",
    "components/retrieval/python",
    "components/tools/python",
    "components/train/python",
    "components/verify/python",
)
PACKAGE_FILES = {".py", ".json", ".jsonl"}


class BuildSplitNamespace(build_py):
    """Copy each component's namespace fragment into the wheel staging area."""

    def run(self) -> None:
        super().run()
        destination = Path(self.build_lib) / "theoremata_tools"
        for root in COMPONENT_PYTHON_ROOTS:
            source = ROOT / root / "theoremata_tools"
            for path in source.rglob("*"):
                if (
                    not path.is_file()
                    or "__pycache__" in path.parts
                    or path.suffix not in PACKAGE_FILES
                ):
                    continue
                target = destination / path.relative_to(source)
                self.mkpath(str(target.parent))
                self.copy_file(str(path), str(target))


setup(cmdclass={"build_py": BuildSplitNamespace})
