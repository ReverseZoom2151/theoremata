"""Put every component's ``python/`` dir on ``sys.path`` so the split
``theoremata_tools`` namespace package resolves during tests."""
import glob
import os
import sys

_root = os.path.dirname(os.path.abspath(__file__))
for _p in sorted(glob.glob(os.path.join(_root, "components", "*", "python"))):
    if _p not in sys.path:
        sys.path.insert(0, _p)
