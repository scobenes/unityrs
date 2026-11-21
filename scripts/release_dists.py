"""Check the release workflow's Python distributions and gather them for upload.

    python3 scripts/release_dists.py --version 1.0.0 ARTIFACTS DIST

ARTIFACTS holds one folder per artifact (actions/download-artifact without
`merge-multiple`). The files in them must be exactly one sdist and one wheel
per platform target of `.github/workflows/release.yml`'s matrix (TARGETS
below, by platform tag), with no file name twice, every one of them at
`--version` by its name and by its own metadata (a wheel's METADATA, the
sdist's PKG-INFO). Then each is copied into DIST, which must not exist yet,
for `twine check` and the upload.

Exit status: 0 when the set is right, 1 when it is not (each problem on its
own line on stderr), 2 for a usage error. Standard library only.
"""

from __future__ import annotations

import argparse
import re
import shutil
import sys
import tarfile
import zipfile
from email.parser import HeaderParser
from pathlib import Path

NAME = "unityrs"
# The abi3 wheel's Python and ABI tags (pyo3's abi3-py310).
ABI = "cp310-abi3"
# One wheel per row of the workflow's wheel matrix: its label, and the
# platform tag (one of a wheel's dot-separated set) that names it.
TARGETS = {
    "linux x86_64": re.compile(r"manylinux_2_28_x86_64"),
    "linux aarch64": re.compile(r"manylinux_2_28_aarch64"),
    "macOS arm64": re.compile(r"macosx_\d+_\d+_arm64"),
    "macOS x86_64": re.compile(r"macosx_\d+_\d+_x86_64"),
}
WHEEL = re.compile(rf"{NAME}-(?P<version>[^-]+)-(?P<abi>[^-]+-[^-]+)-(?P<platform>[^-]+)\.whl")
SDIST = re.compile(rf"{NAME}-(?P<version>[^-]+)\.tar\.gz")


def _metadata_version(path: Path, version: str) -> str | None:
    """The `Version` a distribution's own metadata gives, or None when it has none to read."""
    try:
        if path.suffix == ".whl":
            with zipfile.ZipFile(path) as wheel:
                text = wheel.read(f"{NAME}-{version}.dist-info/METADATA").decode("utf-8")
        else:
            with tarfile.open(path) as sdist:
                member = sdist.extractfile(f"{NAME}-{version}/PKG-INFO")
                text = member.read().decode("utf-8") if member else ""
    except (KeyError, OSError, zipfile.BadZipFile, tarfile.TarError, UnicodeDecodeError):
        return None
    return HeaderParser().parsestr(text).get("Version")


def problems(files: list[Path], version: str) -> list[str]:
    """Why `files` (every file of every artifact) is not the release's set at `version`; empty when it is."""
    out = []
    names = [f.name for f in files]
    for name in sorted({n for n in names if names.count(n) > 1}):
        out.append(f"{name}: in more than one artifact")
    found = {label: [] for label in TARGETS}
    sdists = []
    for path in sorted(set(files), key=lambda p: p.name):
        if wheel := WHEEL.fullmatch(path.name):
            if wheel["abi"] != ABI:
                out.append(f"{path.name}: tagged {wheel['abi']}, not {ABI}")
            labels = [lb for lb, tag in TARGETS.items() if any(tag.fullmatch(p) for p in wheel["platform"].split("."))]
            if len(labels) != 1:
                out.append(f"{path.name}: its platform tag {wheel['platform']} names no single target")
            for label in labels:
                found[label].append(path.name)
            given = wheel["version"]
        elif sdist := SDIST.fullmatch(path.name):
            sdists.append(path.name)
            given = sdist["version"]
        else:
            out.append(f"{path.name}: neither a {NAME} wheel nor its sdist")
            continue
        if given != version:
            out.append(f"{path.name}: version {given}, not {version}")
        elif (meta := _metadata_version(path, version)) != version:
            out.append(f"{path.name}: its metadata gives version {meta}, not {version}")
    for label, wheels in found.items():
        if len(wheels) != 1:
            out.append(f"{label}: {len(wheels)} wheels ({', '.join(wheels) or 'none'}), not one")
    if len(sdists) != 1:
        out.append(f"sdist: {len(sdists)} ({', '.join(sdists) or 'none'}), not one")
    return out


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--version", required=True)
    parser.add_argument("artifacts", type=Path)
    parser.add_argument("dist", type=Path)
    args = parser.parse_args(argv)
    if not args.artifacts.is_dir() or args.dist.exists():
        print("release-dists: ARTIFACTS must be a folder and DIST must not exist", file=sys.stderr)
        return 2
    files = sorted(p for p in args.artifacts.rglob("*") if p.is_file())
    if found := problems(files, args.version):
        for problem in found:
            print(f"release-dists: {problem}", file=sys.stderr)
        return 1
    args.dist.mkdir(parents=True)
    for path in files:
        shutil.copy2(path, args.dist / path.name)
    print(f"release-dists: {len(files)} distributions of {NAME} {args.version} in {args.dist}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
