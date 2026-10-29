#!/usr/bin/env python3
"""Check that an interpreter's packages are the oracle's pinned wheels (requirements-oracle.txt).

    python tools/parity/oracle_env.py                    # each pin installed from its pinned wheel
    python tools/parity/oracle_env.py --exact            # ... and nothing else installed
    python tools/parity/oracle_env.py --pin-records DIR  # re-pin oracle_records.json from the wheels in DIR

The pins are the whole oracle (docs/DESIGN.md section 3): requirements-oracle.txt
explains why numpy, cffi and zstandard, which the stack imports when it can,
are among them. A pin is satisfied by the files of its pinned wheel, not by a
version string, so for each pin the check (also what unitypy_dump.py runs
before it dumps) requires:
- the distribution installed at the pinned version;
- its RECORD's hashed entries, without those the installer writes (INSTALLER,
  REQUESTED, direct_url.json, and the launchers outside site-packages), to
  be the pinned wheel's: their digest (record_digest) must be the one
  oracle_records.json holds. Another build of the same version, or an edited
  RECORD, differs here;
- every one of those files to hash on disk as its RECORD entry says, so an
  edited file is refused;
- each top-level module to import from those files, so a copy earlier on
  sys.path (PYTHONPATH, the script's folder) cannot stand in for it;
- no file in its packages' folders but those (`__pycache__` aside), so a
  module planted beside one, such as an extension that Python would import in
  place of a .py file of the same name, is refused too;
- no symlink among those files or anywhere in those folders, since no wheel
  installs one. The walk follows none, so a planted link to a folder (which
  Python would import as a package in place of a module of the same name)
  is refused rather than skipped.
A process that runs the oracle also checks what it loads (LoadedModules):
every module it imports, at import time or while it dumps, must come from
the standard library, the parity tools, or a pinned wheel's files. So a
package a pin imports only when it is installed (requirements-oracle.txt
names those the image holds) cannot change the dump unseen.
It guards against drift (an edited file, another build, a stray copy), not
against someone set on deceiving it: code run at start-up from outside the
pins (a .pth file, sitecustomize) and a compiled file planted in
`__pycache__` with its source's timestamp are not looked for.
Without options the interpreter running this script is checked: the oracle
image holds other packages too. `--exact` also refuses any distribution that
is not pinned, apart from the installers `pip freeze` leaves out (pip,
setuptools, wheel); CI runs it in the fresh venv it installs the pins into.

`--pin-records DIR` writes oracle_records.json from DIR, which must hold the
pinned wheels and nothing else (each one's sha256 as requirements-oracle.txt
pins it), as a re-pin takes them from PyPI (tools/parity/README.md).

Exit status: 0 when the check passes, 1 when it fails (each difference on its
own line on stderr), 2 for a usage error. Standard library only.
"""

from __future__ import annotations

import argparse
import base64
import csv
import functools
import hashlib
import importlib.util
import io
import json
import os
import re
import sys
import sysconfig
import zipfile
from importlib import metadata
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import canonical as C  # noqa: E402

# Files an installer writes into a dist-info folder, which no wheel holds (PEP 376, PEP 610).
INSTALLER_FILES = frozenset(("INSTALLER", "REQUESTED", "direct_url.json"))
_WHEEL_PIN = re.compile(r"^([A-Za-z0-9][A-Za-z0-9._-]*)==(\S+) \\\n\s+--hash=sha256:([0-9a-f]{64})", re.M)


def parse_record(text: str) -> dict[str, str]:
    """{path: hash} for each RECORD row that carries a hash (RECORD itself, and .pyc files, carry none)."""
    return {row[0]: row[1] for row in csv.reader(io.StringIO(text)) if len(row) >= 2 and row[1]}


def from_wheel(entries: dict[str, str]) -> dict[str, str]:
    """The entries without what an installer writes: INSTALLER, REQUESTED, direct_url.json, the launchers (../)."""
    installer = tuple(f".dist-info/{name}" for name in INSTALLER_FILES)
    return {
        path: digest
        for path, digest in entries.items()
        if not path.startswith("../") and not (path.count("/") == 1 and path.endswith(installer))
    }


def record_digest(entries: dict[str, str]) -> str:
    """The sha256 that oracle_records.json pins: over the entries, sorted by path, as compact JSON."""
    text = json.dumps(sorted(entries.items()), separators=(",", ":"), ensure_ascii=False)
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def wheel_entries(wheel: Path) -> tuple[str, str, dict[str, str]]:
    """(normalised name, version, {path: hash}) from a wheel's own RECORD.

    The pins' wheels install every file at its own path; a wheel with a
    .data folder (files placed elsewhere, scripts whose first line the
    installer rewrites) would need more care, so it is refused.
    """
    with zipfile.ZipFile(wheel) as zf:
        records = [n for n in zf.namelist() if n.count("/") == 1 and n.endswith(".dist-info/RECORD")]
        if len(records) != 1:
            raise ValueError(f"{wheel.name}: {len(records)} RECORD files")
        info = records[0].split("/")[0]
        entries = from_wheel(parse_record(zf.read(records[0]).decode("utf-8")))
        meta = zf.read(f"{info}/METADATA").decode("utf-8")
    data = sorted(p for p in entries if p.split("/")[0].endswith(".data"))
    if data:
        raise ValueError(f"{wheel.name}: a .data entry ({data[0]}) is not supported")
    fields = dict(line.split(": ", 1) for line in meta.split("\n\n")[0].splitlines() if ": " in line)
    return C.normalise_package(fields["Name"]), fields["Version"], entries


def installed_entries(dist) -> dict[str, str]:
    """A distribution's RECORD entries that come from its wheel: those with a hash, minus what the installer wrote."""
    return from_wheel(parse_record(dist.read_text("RECORD") or ""))


def _record_hash(data: bytes) -> str:
    return "sha256=" + base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode("ascii")


def file_problems(name: str, dist, entries: dict[str, str]) -> list[str]:
    """Each file of `entries` that is missing, or does not hash as its RECORD entry says."""
    problems = []
    for path, digest in sorted(entries.items()):
        if not digest.startswith("sha256="):
            problems.append(f"{name}: {path} has a {digest.split('=')[0]} hash in RECORD; only sha256 is checked")
            continue
        try:
            data = Path(dist.locate_file(path)).read_bytes()
        except OSError as e:
            problems.append(f"{name}: {path} cannot be read ({e.strerror or e})")
            continue
        found = _record_hash(data)
        if found != digest:
            problems.append(f"{name}: {path} hashes as {found}, not as its RECORD's {digest}")
    return problems


def _tops(entries: dict[str, str]) -> tuple[list[str], list[str]]:
    """(the top-level folders the entries fill, the top-level module names they provide)."""
    folders, modules = set(), set()
    for path in entries:
        first, sep, _ = path.partition("/")
        if first.endswith((".dist-info", ".data")):
            continue
        if sep:
            folders.add(first)
            if first.isidentifier():
                modules.add(first)
        elif first.endswith((".py", ".so")) and first.split(".")[0].isidentifier():
            modules.add(first.split(".")[0])
    return sorted(folders), sorted(modules)


def import_problems(name: str, dist, entries: dict[str, str]) -> list[str]:
    """Each top-level module of the distribution that would import from anything but its own files."""
    site = Path(dist.locate_file("")).resolve()
    files = {(site / path).resolve() for path in entries}
    problems = []
    for module in _tops(entries)[1]:
        try:
            spec = importlib.util.find_spec(module)
        except (ImportError, ValueError) as e:
            problems.append(f"{name}: module {module} cannot be found ({e})")
            continue
        if spec is None:
            problems.append(f"{name}: module {module} cannot be found")
            continue
        origin = Path(spec.origin).resolve() if spec.has_location and spec.origin else None
        places = [Path(p).resolve() for p in spec.submodule_search_locations or ()]
        if (origin is not None and origin not in files) or any(p != site / module for p in places):
            where = origin or (places[0] if places else spec.origin)
            problems.append(f"{name}: module {module} imports from {where}, not from {site}")
    return problems


def stray_problems(name: str, dist, entries: dict[str, str]) -> list[str]:
    """Each symlink among the distribution's files and in its top-level folders, and each file in those folders
    that its RECORD does not list (`__pycache__` aside).

    No pinned wheel installs a symlink. One to a folder would also hide what
    it holds from the walk, which follows none, so every symlink is refused,
    `__pycache__` included.
    """
    site = Path(dist.locate_file(""))
    links = {path for path in entries if (site / path).is_symlink()}
    strays = []
    for folder in _tops(entries)[0]:
        if (site / folder).is_symlink():
            links.add(folder)
            continue
        for dirpath, dirnames, filenames in os.walk(site / folder):  # followlinks=False
            for entry in dirnames + filenames:
                if Path(dirpath, entry).is_symlink():
                    links.add(Path(dirpath, entry).relative_to(site).as_posix())
            for filename in filenames:
                path = Path(dirpath, filename).relative_to(site).as_posix()
                if path not in entries and path not in links and "__pycache__" not in path.split("/"):
                    strays.append(path)
    problems = [f"{name}: {path} is a symlink, which no pinned wheel installs" for path in sorted(links)]
    return problems + [f"{name}: {path} is not one of its wheel's files" for path in sorted(strays)]


def _distribution(name: str, path: list[str] | None):
    """The first distribution named `name` on `path` (default sys.path), as Python's import system finds it."""
    for dist in metadata.distributions(**({} if path is None else {"path": path})):
        if dist.metadata["Name"] and C.normalise_package(dist.metadata["Name"]) == name:
            return dist
    return None


def wheel_problems(records: dict[str, str], path: list[str] | None = None) -> tuple[list[str], dict[str, str]]:
    """(why the pins on `path` are not their pinned wheels' files, {name: record digest as installed}).

    `records` is {normalised name: record digest}, as oracle_records() gives it.
    """
    problems, digests = [], {}
    for name in sorted(records):
        dist = _distribution(name, path)
        if dist is None:
            problems.append(f"{name} is not installed")
            continue
        entries = installed_entries(dist)
        digests[name] = record_digest(entries)
        if digests[name] != records[name]:
            problems.append(
                f"{name} {dist.version}: its RECORD is not the pinned wheel's (record digest {digests[name]}, "
                f"pinned {records[name]}): another build of the version, or an edited RECORD"
            )
            continue
        problems += file_problems(name, dist, entries)
        problems += import_problems(name, dist, entries)
        problems += stray_problems(name, dist, entries)
    return problems, digests


def pinned_files(records: dict[str, str], path: list[str] | None = None) -> set[Path]:
    """The resolved path of each file the RECORDs of the pinned distributions on `path` list (default sys.path)."""
    files = set()
    for name in records:
        dist = _distribution(name, path)
        if dist is not None:
            site = Path(dist.locate_file(""))
            files.update((site / entry).resolve() for entry in installed_entries(dist))
    return files


class LoadedModules:
    """Which loaded modules come from anywhere but the standard library, the `own` folders, or the pinned `files`.

    Modules in `before`, loaded before the oracle's script imported
    anything, are the interpreter's start-up (site, .pth files), which this
    check does not look at; neither are modules without a file (built in,
    frozen without a source, namespace packages). A module under the standard
    library's folder but in a site-packages or dist-packages folder is not
    the standard library's. Each module's verdict is kept, so a process
    forked after a check re-checks only what it loads itself.
    """

    def __init__(self, files, own, before):
        self.files = frozenset(files)
        self.own = [Path(p).resolve() for p in own]
        self.stdlib = sorted({Path(sysconfig.get_paths()[key]).resolve() for key in ("stdlib", "platstdlib")})
        self.before = {id(module): module for module in before}  # held, so no id is reused
        self.verdicts = {}

    def allowed(self, path: Path) -> bool:
        if path in self.files or any(path.is_relative_to(own) for own in self.own):
            return True
        for stdlib in self.stdlib:
            if path.is_relative_to(stdlib):
                return not {"site-packages", "dist-packages"} & set(path.relative_to(stdlib).parts)
        return False

    def problems(self, modules: dict | None = None) -> list[str]:
        """'module NAME is loaded from PATH, ...' for each module of `modules` (default sys.modules) not allowed."""
        problems = []
        for name, module in sorted(list((sys.modules if modules is None else modules).items()), key=lambda kv: kv[0]):
            file = getattr(module, "__file__", None)
            if id(module) in self.before or not isinstance(file, str):
                continue
            if (name, file) not in self.verdicts:
                path = Path(file).resolve()
                self.verdicts[name, file] = None if self.allowed(path) else path
            if self.verdicts[name, file] is not None:
                problems.append(
                    f"module {name} is loaded from {self.verdicts[name, file]}, "
                    "which is neither the standard library nor a pinned wheel's file"
                )
        return problems


@functools.cache
def installed_wheel_problems() -> tuple[tuple[str, ...], dict[str, str]]:
    """wheel_problems for this interpreter against oracle_records.json, computed once per process."""
    problems, digests = wheel_problems(C.oracle_records())
    return tuple(problems), digests


def pinned_wheels(path: Path = C.ORACLE_REQUIREMENTS) -> dict[str, tuple[str, str]]:
    """{normalised name: (version, wheel sha256)} from requirements-oracle.txt."""
    return {
        C.normalise_package(m.group(1)): (m.group(2), m.group(3))
        for m in _WHEEL_PIN.finditer(path.read_text(encoding="utf-8"))
    }


def pin_records(folder: Path) -> dict:
    """oracle_records.json's content from the pinned wheels in `folder`; ValueError unless it holds exactly them."""
    wheels = pinned_wheels()
    if set(wheels) != set(C.oracle_pins()):
        raise ValueError(f"{C.ORACLE_REQUIREMENTS.name}: a pin without exactly one --hash line")
    by_hash = {sha: name for name, (_, sha) in wheels.items()}
    records = {}
    for entry in sorted(folder.iterdir()):
        sha = hashlib.sha256(entry.read_bytes()).hexdigest() if entry.is_file() else None
        if sha not in by_hash:
            raise ValueError(f"{entry.name} is not a pinned wheel")
        name, version, entries = wheel_entries(entry)
        if (name, version) != (by_hash[sha], wheels[by_hash[sha]][0]):
            raise ValueError(f"{entry.name} holds {name} {version}, not what its hash pins")
        records[name] = {
            "version": version,
            "wheel_sha256": sha,
            "files": len(entries),
            "record_sha256": record_digest(entries),
        }
    missing = sorted(set(wheels) - set(records))
    if missing:
        raise ValueError(f"no wheel for {', '.join(missing)}")
    return {
        "about": (
            "Written by `python tools/parity/oracle_env.py --pin-records DIR` from the wheels "
            "requirements-oracle.txt pins: for each, the number of its RECORD's hashed entries (`files`) and "
            "their digest (`record_sha256`, oracle_env.record_digest). The oracle's installed files must be these."
        ),
        "records": dict(sorted(records.items())),
    }


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description="Check that this interpreter's packages are the oracle's pinned wheels.")
    group = ap.add_mutually_exclusive_group()
    group.add_argument("--exact", action="store_true", help="also refuse any package that is not pinned")
    group.add_argument("--pin-records", metavar="DIR", help="write oracle_records.json from the pinned wheels in DIR")
    args = ap.parse_args(argv)
    pins = C.oracle_pins()
    if args.pin_records:
        if not Path(args.pin_records).is_dir():
            ap.error(f"{args.pin_records} is not a folder")
        try:
            content = pin_records(Path(args.pin_records))
        except ValueError as e:
            print(f"oracle_env: {e}", file=sys.stderr)
            return 1
        C.ORACLE_RECORDS.write_text(json.dumps(content, indent=1) + "\n", encoding="utf-8")
        print(f"oracle_env: wrote {C.ORACLE_RECORDS.name}: {len(content['records'])} wheels")
        return 0
    problems = C.stack_problems(C.installed_packages(), pins, exact=args.exact)
    if not problems:
        problems = list(wheel_problems(C.oracle_records())[0])
    for p in problems:
        print(f"oracle_env: {p}", file=sys.stderr)
    if problems:
        print(f"oracle_env: this is not the oracle stack of {C.ORACLE_REQUIREMENTS.name}", file=sys.stderr)
        return 1
    said = (
        f"oracle_env: each of the {len(pins)} pins in {C.ORACLE_REQUIREMENTS.name} is installed from its pinned "
        f"wheel ({C.ORACLE_RECORDS.name}): the version, every file's hash, and where it imports from"
    )
    if args.exact:
        said += ", and nothing else is installed"
    print(said)
    return 0


if __name__ == "__main__":
    sys.exit(main())
