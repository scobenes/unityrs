#!/usr/bin/env python3
"""Checks unityrs-imageio's vendored zlib against zlib 1.2.13's release tarball.

    python3 scripts/zlib_vendor.py verify TARBALL

crates/unityrs-imageio/vendor/zlib-1.2.13.sha256 lists the tarball's sha256 (its line for `zlib-1.2.13.tar.gz`) and,
for each file vendored in crates/unityrs-imageio/vendor/zlib-1.2.13/, the sha256 of the tarball's member of that name
(`zlib-1.2.13/<file>`). `verify` checks a downloaded tarball (https://zlib.net/fossils/zlib-1.2.13.tar.gz, or the
GitHub release's asset of the same name) against all of it: the tarball's own sha256, each listed member's, and each
vendored file's bytes, which must be its member's. The crate's tests/zlib.rs checks, without the tarball, that the
folder holds exactly the listed files with the listed hashes.

The tarball is read in memory (tarfile's `extractfile`), never extracted. Exit status: 0 when it passes, 1 when it
fails (each problem on its own line on stderr), 2 for a usage error. Standard library only.
"""

from __future__ import annotations

import hashlib
import io
import sys
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
VENDOR = Path("crates/unityrs-imageio/vendor")
MANIFEST = "zlib-1.2.13.sha256"
TARBALL = "zlib-1.2.13.tar.gz"
PREFIX = "zlib-1.2.13/"


def manifest(text: str) -> tuple[str | None, dict[str, str]]:
    """The tarball's sha256 and each listed member's (its name in the tarball -> its sha256)."""
    tarball, members = None, {}
    for line in text.splitlines():
        if not line or line.startswith("#"):
            continue
        sha, name = line.split("  ", 1)
        if name == TARBALL:
            tarball = sha
        else:
            members[name] = sha
    return tarball, members


def problems(tarball: bytes, root: Path = ROOT) -> list[str]:
    vendor = root / VENDOR
    want_tarball, members = manifest((vendor / MANIFEST).read_text(encoding="utf-8"))
    out = []
    if want_tarball is None:
        out.append(f"{MANIFEST} lists no sha256 of {TARBALL}")
    elif hashlib.sha256(tarball).hexdigest() != want_tarball:
        out.append(f"the tarball's sha256 is {hashlib.sha256(tarball).hexdigest()}, not {MANIFEST}'s {want_tarball}")
    with tarfile.open(fileobj=io.BytesIO(tarball), mode="r:gz") as tar:
        for name, sha in sorted(members.items()):
            if not name.startswith(PREFIX):
                out.append(f"{MANIFEST} lists {name}, which is not under {PREFIX}")
                continue
            try:
                member = tar.getmember(name)
            except KeyError:
                out.append(f"the tarball has no {name}")
                continue
            reader = tar.extractfile(member) if member.isfile() else None
            if reader is None:
                out.append(f"the tarball's {name} is not a file")
                continue
            data = reader.read()
            if hashlib.sha256(data).hexdigest() != sha:
                out.append(f"the tarball's {name} has sha256 {hashlib.sha256(data).hexdigest()}, not {sha}")
            vendored = vendor / name
            if not vendored.is_file():
                out.append(f"{VENDOR / name} is missing")
            elif vendored.read_bytes() != data:
                out.append(f"{VENDOR / name} is not the tarball's {name}")
    return out


def main(argv: list[str] | None = None) -> int:
    args = sys.argv[1:] if argv is None else argv
    if len(args) != 2 or args[0] != "verify":
        print(__doc__.split("\n\n")[1], file=sys.stderr)
        return 2
    found = problems(Path(args[1]).read_bytes())
    for problem in found:
        print(f"zlib_vendor: {problem}", file=sys.stderr)
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
