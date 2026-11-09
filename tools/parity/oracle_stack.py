"""The oracle stack a reference dump must come from (docs/CANONICAL_DUMP.md): its pins, and checks against them.

canonical.py names everything public here too; the tools and tests reach it there, as `C.oracle_pins()`.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

# The oracle stack a reference dump must come from (docs/CANONICAL_DUMP.md):
# every package requirements-oracle.txt pins, installed from its pinned wheel
# (its version, and its files as oracle_records.json pins them), and the
# UnityPyBoost binary the pinned UnityPy wheel holds.
ORACLE_REQUIREMENTS = Path(__file__).resolve().with_name("requirements-oracle.txt")
ORACLE_PRODUCER = "unitypy_dump.py"
# The sha256 of UnityPy/UnityPyBoost.cpython-311-x86_64-linux-gnu.so in the
# pinned unitypy wheel, as its RECORD lists it (in RECORD's urlsafe base64,
# QO5PYLPXAPOONPCtIF72pivuoxH0TmH0_Lwv7s6W4SQ). test_unitypy_boost.py checks it
# against the installed RECORD and the loaded module; re-pin it with UnityPy.
ORACLE_BOOST_SHA256 = "40ee4f60b3d700f38e34f0ad205ef6a62beea311f44e61f4fcbc2feece96e124"
# Each pinned wheel's files, as the digest of its RECORD (oracle_env.py writes and checks it).
ORACLE_RECORDS = ORACLE_REQUIREMENTS.with_name("oracle_records.json")
# pip leaves these out of `pip freeze` on Python 3.11: they install the stack and are no part of it.
INSTALLERS = frozenset(("pip", "setuptools", "wheel", "distribute"))

_PIN = re.compile(r"([A-Za-z0-9][A-Za-z0-9._-]*)==([^\s\\;]+)")


def normalise_package(name: str) -> str:
    """A distribution name as PEP 503 compares it: lower case, each run of -_. one hyphen."""
    return re.sub(r"[-_.]+", "-", name).lower()


def oracle_pins(path: Path = ORACLE_REQUIREMENTS) -> dict[str, str]:
    """{normalised name: version} for every `name==version` line of requirements-oracle.txt."""
    pins = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        m = _PIN.match(line)
        if m:
            pins[normalise_package(m.group(1))] = m.group(2)
    return pins


def oracle_records(path: Path = ORACLE_RECORDS) -> dict[str, str]:
    """{normalised name: record digest} of every pinned wheel, from oracle_records.json."""
    records = json.loads(path.read_text(encoding="utf-8"))["records"]
    return {name: records[name]["record_sha256"] for name in sorted(records)}


def installed_packages(path: list[str] | None = None) -> dict[str, str]:
    """{normalised name: version} of the distributions on `path` (default sys.path), the first of each name."""
    from importlib import metadata

    out = {}
    for dist in metadata.distributions(**({} if path is None else {"path": path})):
        name = dist.metadata["Name"]
        if name:
            out.setdefault(normalise_package(name), dist.version)
    return out


def stack_problems(installed: dict[str, str], pins: dict[str, str], exact: bool = False) -> list[str]:
    """How `installed` differs from the pins: a pin missing or at another version; with `exact`, any extra too."""
    problems = [
        f"{name} is {installed[name]}, not the pinned {version}" if name in installed else f"{name} is not installed"
        for name, version in sorted(pins.items())
        if installed.get(name) != version
    ]
    if exact:
        problems += [
            f"{name} {version} is installed but not pinned"
            for name, version in sorted(installed.items())
            if name not in pins and name not in INSTALLERS
        ]
    return problems


ORACLE_UNITYPY = oracle_pins()["unitypy"]
