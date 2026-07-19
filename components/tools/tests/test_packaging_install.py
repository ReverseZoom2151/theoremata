"""The worker namespace must work after installation, not only from the repo."""

from __future__ import annotations

import subprocess
import sys
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]


def test_wheel_installs_split_worker_namespace(tmp_path: Path) -> None:
    """Build then import representative modules from an isolated install target."""
    target = tmp_path / "site"
    subprocess.run(
        [
            sys.executable,
            "-m",
            "pip",
            "install",
            "--no-deps",
            "--no-build-isolation",
            "--target",
            str(target),
            str(ROOT),
        ],
        check=True,
        cwd=tmp_path,
        capture_output=True,
        text=True,
    )
    run_module = (
        "import runpy, sys; "
        f"sys.path.insert(0, {str(target)!r}); "
        "runpy.run_module(sys.argv[1], run_name='__main__')"
    )
    worker = subprocess.run(
        [sys.executable, "-I", "-c", run_module, "theoremata_tools.worker"],
        input=json.dumps({"tool": "not_a_real_tool"}),
        check=False,
        cwd=tmp_path,
        capture_output=True,
        text=True,
    )
    assert worker.returncode == 2
    assert json.loads(worker.stdout)["ok"] is False

    mcp = subprocess.run(
        [sys.executable, "-I", "-c", run_module, "theoremata_tools.mcp_server"],
        input=json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize"}) + "\n",
        check=True,
        cwd=tmp_path,
        capture_output=True,
        text=True,
    )
    assert json.loads(mcp.stdout)["result"]["serverInfo"]["name"] == "theoremata"
    probe = """
import importlib
import importlib.resources
import sys
sys.path.insert(0, {target!r})
for module in (
    'theoremata_tools.worker',
    'theoremata_tools.geometry',
    'theoremata_tools.model_provider',
    'theoremata_tools.stages',
    'theoremata_tools.retrieval',
    'theoremata_tools.cert_log',
    'theoremata_tools.benchmarks.formalizing_100',
):
    importlib.import_module(module)
data = importlib.resources.files('theoremata_tools.benchmarks').joinpath(
    'data', 'formalizing_100.jsonl'
)
assert data.is_file(), data
""".format(target=str(target))
    subprocess.run(
        [sys.executable, "-I", "-c", probe],
        check=True,
        cwd=tmp_path,
        capture_output=True,
        text=True,
    )
