"""Capability boundary for Terence Tao's lightweight Estimates assistant."""
from __future__ import annotations

import importlib
import sys
from pathlib import Path


def capability(resources: str = "resources") -> dict:
    source = (
        Path(resources)
        / "estimates-main"
        / "estimates-main"
        / "src"
    )
    if not source.exists():
        return {"available": False, "reason": "Estimates source not found"}
    sys.path.insert(0, str(source.resolve()))
    try:
        module = importlib.import_module("estimates")
        return {
            "available": True,
            "source": str(source),
            "module": getattr(module, "__file__", None),
            "role": "domain-specific evidence; not a foundational certificate",
        }
    except Exception as exc:
        return {
            "available": False,
            "source": str(source),
            "reason": f"import failed: {exc}",
        }
