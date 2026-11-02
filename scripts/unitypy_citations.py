#!/usr/bin/env python3
"""Every citation of UnityPy 1.25.2's source in the repository names lines that exist.

    python3 scripts/unitypy_citations.py [ROOT] [--unitypy DIR] [--boost DIR]

A citation is `<path>:<line>` or `<path>:<first>-<last>`, where the path ends
in .py, .pyi, .cpp or .hpp, in a text file of the repository (vendored
UnityPy files, build output and caches aside). It is UnityPy's when its path
names a file of UnityPy 1.25.2's package (the installed one, which must be
1.25.2, or `--unitypy DIR`) or of UnityPyBoost's C++ sources: a path from the
package's root (`environment.py`, `files/BundleFile.py`, with or without a
leading `UnityPy/`; `UnityPyBoost/TypeTreeHelper.cpp`), or the end of exactly
one file's path (`TypeTreeHelper.py`, `TypeTreeHelper.cpp`). Any other path
(CPython's `Lib/gzip.py`, a file of this repository) is not checked. It fails:
- on a citation whose path ends more than one UnityPy file's (`BundleFile.py`:
  files/ and enums/), which could be either: write the path from the root;
- on a citation of a line past its file's end, of line 0, or of a range that
  runs backwards;
- on a file under a crate's src/ (test modules aside: `tests.rs`, `*_tests.rs`
  and `tests/` folders) that cites UnityPy and has no
  `// Ported from UnityPy 1.25.2 (MIT): ...` header line, which about.hbs
  says every such file has.
A line that names another UnityPy revision (`HEAD`, or a commit such as
`417d595`) is skipped whole: its citations may be that revision's. A bare
`:<line>` after a citation, which names no file, is not checked.

UnityPyBoost's sources are in UnityPy's sdist, not in the wheel the oracle
installs, so their line counts are pinned below (BOOST) with each file's
sha256; `--boost DIR` checks a copy of the sources against the pins and reads
it instead.

This checks that a cited line exists, not that it holds what the comment says
(review round 11 found nine citations off by a few lines). Reading the name a
comment gives with its citation, and requiring the cited range to fall within
that name's definition in UnityPy, flagged 20 of the 87 citations it could
read on 2026-11-01, and only 2 of those 20 were wrong (the rest cite a
caller, an alias or a module table), so it is not a check.

Exit status: 0 when every citation passes (a summary on stdout), 1 when one
does not (each problem on its own line on stderr), 2 for a usage error or a
UnityPy that is not 1.25.2. Standard library only.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import importlib.util
import os
import re
import sys
from pathlib import Path
from typing import NamedTuple

sys.path.insert(0, str(Path(__file__).resolve().parent))

from third_party_licenses_check import port_headers  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
VERSION = "1.25.2"
SOURCE = "UnityPy 1.25.2"
# UnityPyBoost's sources in UnityPy 1.25.2's sdist: (lines, sha256).
BOOST = {
    "UnityPyBoost/ArchiveStorageDecryptor.cpp": (
        90,
        "f387d5093e7f504ec5735d11e67f447da3ec881b70206c54aba10954bb4899ae",
    ),
    "UnityPyBoost/ArchiveStorageDecryptor.hpp": (5, "4b669249b28603ec64130fce258a35ced7f7adaf16ae7509ee47fe9daa5f301e"),
    "UnityPyBoost/Mesh.cpp": (210, "ee9542139cb02c1ab8a417192227784c6ee5de3867b9d5761daebf427ad65c8d"),
    "UnityPyBoost/Mesh.hpp": (5, "7e081fef7e5e9b95881cc7bba8c284aa1d13f025167a3f3b6a4b30bdbe18ec12"),
    "UnityPyBoost/TypeTreeHelper.cpp": (1286, "9749bd26f2983ef73a5138c04513317eaeb7b4f2a3631545d23ebc10039db17e"),
    "UnityPyBoost/TypeTreeHelper.hpp": (55, "7ca69cd47ce2a71870bc82c3aae7600ad573d76acbd862e3053d664f05ec7543"),
    "UnityPyBoost/UnityPyBoost.cpp": (47, "80f005e91ccb786c6705a9e50c19c3bfaf19444f40a65d24b82094c0f0417253"),
    "UnityPyBoost/swap.hpp": (78, "48d0ef17909c5c6b07d6b3c50750b3828b9472c2d872f51c4ec36f5699bdf719"),
}
CITATION = re.compile(r"(?<![\w./-])(?P<path>(?:[\w-]+/)*[\w-]+\.(?:py|pyi|cpp|hpp)):(?P<first>\d+)(?:-(?P<last>\d+))?")
# A line about another revision: HEAD, or a short commit hash (7 to 12 hex digits, a letter and a digit
# among them, as in `417d595` or `head_417d595`), not a longer run of hex data.
OTHER_REVISION = re.compile(
    r"\bHEAD\b|(?<![0-9A-Za-z])(?=[0-9a-f]*[a-f])(?=[0-9a-f]*[0-9])[0-9a-f]{7,12}(?![0-9A-Za-z])"
)
TEXT_SUFFIXES = {".rs", ".py", ".pyi", ".md", ".hbs", ".toml", ".yml", ".yaml", ".sh", ".txt", ""}
SKIP_DIRS = {".git", "target", "__pycache__", ".wt", "node_modules", ".venv"}
# Copies of UnityPy files of another revision, which cite nothing of 1.25.2's.
SKIP_PATHS = ("tools/parity/tests/head_417d595/",)


class Report(NamedTuple):
    problems: list[str]
    checked: int
    boost: int
    files: int
    skipped_lines: int
    ported: int


def unitypy_lines(unitypy: Path, boost: Path | None) -> dict[str, int]:
    """The number of lines of each UnityPy 1.25.2 source, by its path from the package's root."""
    lines = {}
    for path in sorted(unitypy.rglob("*")):
        if path.suffix in (".py", ".pyi") and "__pycache__" not in path.parts:
            lines[path.relative_to(unitypy).as_posix()] = len(path.read_text(encoding="utf-8").splitlines())
    for name, (count, sha) in BOOST.items():
        if boost is not None:
            data = (boost / Path(name).name).read_bytes()
            if hashlib.sha256(data).hexdigest() != sha:
                raise ValueError(f"{boost / Path(name).name} is not UnityPy {VERSION}'s {name} (sha256 differs)")
            count = len(data.decode("utf-8").splitlines())
        lines[name] = count
    return lines


def resolve(cited: str, lines: dict[str, int]) -> list[str]:
    """The UnityPy files a cited path can name: the one at that path from the root, or each whose path ends with it."""
    path = cited.removeprefix("UnityPy/") if not cited.startswith("UnityPyBoost/") else cited
    if path in lines:
        return [path]
    return [k for k in lines if k.endswith("/" + path)]


def is_test_module(rel: str) -> bool:
    parts = rel.split("/")
    return parts[-1] == "tests.rs" or parts[-1].endswith("_tests.rs") or "tests" in parts[3:-1]


def text_files(root: Path):
    for top, dirs, names in os.walk(root):
        dirs[:] = sorted(d for d in dirs if d not in SKIP_DIRS)
        for name in sorted(names):
            path = Path(top) / name
            rel = path.relative_to(root).as_posix()
            if path.suffix in TEXT_SUFFIXES and not rel.startswith(SKIP_PATHS) and path.is_file():
                try:
                    yield rel, path.read_text(encoding="utf-8")
                except UnicodeDecodeError:
                    continue


def check(root: Path, lines: dict[str, int]) -> Report:
    problems, checked, boost, files, skipped, ported = [], 0, 0, 0, 0, 0
    for rel, text in text_files(root):
        cites = 0
        for n, line in enumerate(text.splitlines(), 1):
            found = [m for m in CITATION.finditer(line) if resolve(m["path"], lines)]
            if found and OTHER_REVISION.search(line):
                skipped += 1
                continue
            for m in found:
                cites += 1
                where = f"{rel}:{n}: {m[0]}"
                named = resolve(m["path"], lines)
                if len(named) > 1:
                    problems.append(f"{where} could be any of {', '.join(sorted(named))}; write the path from the root")
                    continue
                first, last = int(m["first"]), int(m["last"] or m["first"])
                if not 1 <= first <= last <= lines[named[0]]:
                    problems.append(f"{where} is not a range of {named[0]}'s {lines[named[0]]} lines")
                checked += 1
                boost += named[0].startswith("UnityPyBoost/")
        files += cites > 0
        if cites and re.fullmatch(r"crates/[^/]+/src/.+\.rs", rel) and not is_test_module(rel):
            if SOURCE in {source for source, _ in port_headers(text)}:
                ported += 1
            else:
                problems.append(
                    f"{rel}: cites UnityPy, but has no `// Ported from {SOURCE} (MIT): ...` header line at its top"
                )
    return Report(problems, checked, boost, files, skipped, ported)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("root", nargs="?", type=Path, default=ROOT)
    parser.add_argument("--unitypy", type=Path, help="UnityPy 1.25.2's package folder (default: the installed one)")
    parser.add_argument("--boost", type=Path, help="a folder of UnityPyBoost's sources from UnityPy 1.25.2's sdist")
    args = parser.parse_args(argv)
    unitypy = args.unitypy
    if unitypy is None:
        spec = importlib.util.find_spec("UnityPy")
        if spec is None or not spec.submodule_search_locations:
            print("unitypy-citations: UnityPy is not installed; pass --unitypy DIR", file=sys.stderr)
            return 2
        if (installed := importlib.metadata.version("UnityPy")) != VERSION:
            print(f"unitypy-citations: UnityPy {installed} is installed, not {VERSION}", file=sys.stderr)
            return 2
        unitypy = Path(spec.submodule_search_locations[0])
    try:
        lines = unitypy_lines(unitypy, args.boost)
    except (OSError, ValueError) as e:
        print(f"unitypy-citations: {e}", file=sys.stderr)
        return 2
    report = check(args.root, lines)
    for p in report.problems:
        print(f"unitypy-citations: {p}", file=sys.stderr)
    if report.problems:
        return 1
    print(
        f"unitypy-citations: {report.checked} citations of UnityPy {VERSION} in {report.files} files name lines "
        f"that exist ({report.boost} of them UnityPyBoost's); {report.skipped_lines} lines naming another revision "
        f"skipped; {report.ported} files under crates' src/ cite it, each with its header"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
