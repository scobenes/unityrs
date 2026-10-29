#!/usr/bin/env python3
"""Check an interpreter's packages against the oracle's pins (requirements-oracle.txt).

    python tools/parity/oracle_env.py                  # each pin installed at its version
    python tools/parity/oracle_env.py --exact          # ... and nothing else installed
    python tools/parity/oracle_env.py --records SITE   # each pin's files hash as they do under SITE

The pins are the whole oracle (docs/DESIGN.md section 3): requirements-oracle.txt
explains why numpy, which UnityPy imports when it can, is among them.
- Without options it checks the interpreter running it, as unitypy_dump.py
  does before it dumps: the oracle image holds other packages too.
- `--exact` also refuses any distribution that is not pinned, apart from the
  installers `pip freeze` leaves out (pip, setuptools, wheel). CI runs it in
  the fresh venv it installs the pins into, so CI's oracle is the pinned set.
- `--records SITE` compares each pinned package's RECORD with the one in the
  site-packages folder SITE (the oracle image's, when re-pinning): every file
  must hash the same, except the console-script launchers, which name their
  interpreter, and INSTALLER, which names the installer.

Exit status: 0 when the check passes, 1 when it fails (each difference on its
own line on stderr), 2 for a usage error. Standard library only.
"""

from __future__ import annotations

import argparse
import sys
from importlib import metadata
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import canonical as C  # noqa: E402


def _record(dist) -> dict[str, str]:
    """{path: hash} from a distribution's RECORD, without the entries that carry no hash (RECORD, .pyc)."""
    out = {}
    for line in (dist.read_text("RECORD") or "").splitlines():
        path, digest, _size = line.rsplit(",", 2)
        if digest:
            out[path] = digest
    return out


def _distribution(name: str, path: list[str] | None):
    for dist in metadata.distributions(**({} if path is None else {"path": path})):
        if dist.metadata["Name"] and C.normalise_package(dist.metadata["Name"]) == name:
            return dist
    return None


def record_problems(pins: dict[str, str], site: str, here_path: list[str] | None = None) -> list[str]:
    """Each pinned file whose RECORD hash differs between `here_path` (default sys.path) and the folder `site`."""
    problems = []
    for name in sorted(pins):
        here, there = _distribution(name, here_path), _distribution(name, [site])
        if here is None or there is None:
            problems.append(f"{name}: not installed {'here' if here is None else 'in ' + site}")
            continue
        a, b = _record(here), _record(there)
        for path in sorted(set(a) | set(b)):
            launcher = path.startswith("../") and "/bin/" in path
            if a.get(path) != b.get(path) and not launcher and not path.endswith(".dist-info/INSTALLER"):
                problems.append(f"{name}: {path} is {a.get(path)} here and {b.get(path)} in {site}")
    return problems


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description="Check this interpreter's packages against requirements-oracle.txt.")
    group = ap.add_mutually_exclusive_group()
    group.add_argument("--exact", action="store_true", help="also refuse any package that is not pinned")
    group.add_argument("--records", metavar="SITE", help="compare each pin's RECORD with SITE's")
    args = ap.parse_args(argv)
    pins = C.oracle_pins()
    if args.records:
        if not Path(args.records).is_dir():
            ap.error(f"{args.records} is not a folder")
        problems = record_problems(pins, args.records)
    else:
        problems = C.stack_problems(C.installed_packages(), pins, exact=args.exact)
    for p in problems:
        print(f"oracle_env: {p}", file=sys.stderr)
    if problems:
        print(f"oracle_env: this is not the oracle stack of {C.ORACLE_REQUIREMENTS.name}", file=sys.stderr)
        return 1
    said = f"oracle_env: each of the {len(pins)} pins in {C.ORACLE_REQUIREMENTS.name} is installed at its version"
    if args.exact:
        said += ", and nothing else is"
    elif args.records:
        said += f", and its files hash as they do in {args.records}"
    print(said)
    return 0


if __name__ == "__main__":
    sys.exit(main())
