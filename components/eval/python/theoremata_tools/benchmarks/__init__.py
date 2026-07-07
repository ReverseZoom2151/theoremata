"""Tier 4 benchmark ingestion + eval harness for Theoremata.

A unified, pluggable harness: per-corpus *loaders* turn each resource into the
common internal item schema (see :mod:`.schema`), and per-track *graders*
(see :mod:`.graders`) score responses. Public API::

    from theoremata_tools.benchmarks import (
        list_benchmarks, load_benchmark, grade,
    )
"""
from __future__ import annotations

from .graders import grade
from .registry import list_benchmarks, load_benchmark, run
from .schema import AXIOMS_WHITELIST, KINDS, make_item

__all__ = [
    "list_benchmarks",
    "load_benchmark",
    "grade",
    "run",
    "make_item",
    "KINDS",
    "AXIOMS_WHITELIST",
]
