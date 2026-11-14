"""In-memory generated cases; never read batch engine output or generated pins."""

import copy
import json
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
CASES = {}


def load_case(name):
    return copy.deepcopy(CASES[name])


def write_case(case):
    CASES[case["name"]] = copy.deepcopy(case)


def encoded(case):
    """UTF-8 with non-ASCII characters written literally, as the canonical JSON does."""
    return (json.dumps(case, ensure_ascii=False, indent=2) + "\n").encode()
