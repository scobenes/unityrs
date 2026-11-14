"""In-memory generated cases; never read batch engine output or generated pins."""

import copy
import json
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
CASES = {}


def load_case(name):
    return copy.deepcopy(CASES[name][0])


def write_case(case, *, ensure_ascii=True):
    CASES[case["name"]] = (copy.deepcopy(case), ensure_ascii)


def encoded(case, ensure_ascii):
    return (json.dumps(case, ensure_ascii=ensure_ascii, indent=2) + "\n").encode()
