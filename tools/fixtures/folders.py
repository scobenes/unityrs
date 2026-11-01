"""Oracle fixtures for package E: how unitypy_dump.py walks an input folder (docs/plans/core.md 7.6).

Runs in the oracle image and writes crates/unityrs-dump/tests/fixtures/dump/folders.json, which
crates/unityrs-cli/tests/folder_runs.rs replays without Python. The oracle walks a folder with Python
3.11's `os.walk` (`collect_inputs`), which recurses one generator frame a level and closes each
folder before it walks the ones below it. Recorded here, from the oracle's whole run
(`unitypy_dump.py FOLDER`, a subprocess, so its call stack is the real one):

- "depth": the deepest folder the walk lists before Python's recursion limit stops it
  (`RecursionError`: exit 1, no dump), found by bisection on a chain of folders, and the oracle's run
  at that depth and one past it for each shape of the deepest level: a folder holding a file, an
  empty folder, a symlink to a folder (which the walk lists but never enters) and a folder it cannot
  read;
- "nofile": its run on a chain of folders under a low limit on open files (`RLIMIT_NOFILE`, set in
  the child), which it walks to the end.

A run is `{"shape", "levels", "status", "names"}`: `status` the exit status, `names` the dump's file
records' names (null when it wrote no dump). Every tree is built here in code, under a temporary
folder; no other file is read.

A tree is the input folder `in` holding the file `top` (`t`) and a chain of `levels` folders named
`d`, each in the one before (`in/d/d/.../d`); its deepest level by shape:
- "file": the deepest folder holds the file `f` (`f`);
- "empty": the deepest folder holds nothing;
- "link": the chain has `levels - 1` folders, and the deepest of them holds `d`, a symlink to `in`;
- "unreadable": the deepest folder's mode is 000 (root reads it all the same, so it is then an empty
  folder, whose runs are the same).

    scripts/dev python tools/fixtures/folders.py           # write the fixture
    scripts/dev python tools/fixtures/folders.py --check   # regenerate and compare
"""

from __future__ import annotations

import argparse
import json
import os
import resource
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-dump/tests/fixtures/dump/folders.json"
ORACLE = ROOT / "tools/parity/unitypy_dump.py"
sys.path.insert(0, str(ROOT / "tools/parity"))

import UnityPy  # noqa: E402

import child  # noqa: E402
import scratch  # noqa: E402

SHAPES = ("file", "empty", "link", "unreadable")
NOFILE = {"shape": "file", "levels": 100, "limit": 32}


def build(base: str, shape: str, levels: int) -> str:
    """The tree of `shape` and `levels` (module docs) in a new folder under `base`; its input folder.

    Built with relative paths, one level at a time, as a deep path may pass the system's limit on a
    path's length."""
    top = os.path.join(base, "in")
    os.mkdir(top)
    with open(os.path.join(top, "top"), "wb") as fh:
        fh.write(b"t")
    here = os.getcwd()
    try:
        os.chdir(top)
        for _ in range(levels - 1 if shape == "link" else levels):
            os.mkdir("d")
            os.chdir("d")
        if shape == "file":
            with open("f", "wb") as fh:
                fh.write(b"f")
        elif shape == "link":
            os.symlink(top, "d")
        elif shape == "unreadable":
            os.chmod(".", 0)
    finally:
        os.chdir(here)
    return top


def run_oracle(shape: str, levels: int, nofile: int | None = None) -> dict:
    """The oracle's run on a fresh tree of `shape` and `levels`, under `nofile` open files."""
    base = str(scratch.make("unityrs-fixtures-"))  # removed with its deep chain and its closed folder
    try:
        top = build(base, shape, levels)
        out = os.path.join(base, "out.jsonl")

        def limit() -> None:
            if nofile is not None:
                resource.setrlimit(resource.RLIMIT_NOFILE, (nofile, nofile))

        proc = child.run([sys.executable, str(ORACLE), top, "-q", "-o", out], stdin=child.DEVNULL,
                              capture_output=True, check=False, preexec_fn=limit)
        names = None
        if os.path.exists(out):
            with open(out) as fh:
                recs = [json.loads(line) for line in fh]
            names = [r["file"][0] for r in recs if r.get("rec") == "file"]
        if proc.returncode == 1 and b"RecursionError" not in proc.stderr:
            raise AssertionError(f"{shape} {levels}: exit 1: {proc.stderr.decode(errors='replace')}")
        return {"shape": shape, "levels": levels, "status": proc.returncode, "names": names}
    finally:
        scratch.remove(base)


def depth_cases() -> dict:
    # The oracle dumps a chain it walks (status 0) and fails one it cannot (1).
    lo, hi = 2, 1500
    if (run_oracle("file", lo)["status"], run_oracle("file", hi)["status"]) != (0, 1):
        raise AssertionError("the oracle's depth bounds moved")
    while hi - lo > 1:
        mid = (lo + hi) // 2
        if run_oracle("file", mid)["status"] == 0:
            lo = mid
        else:
            hi = mid
    return {"max": lo, "runs": [run_oracle(shape, levels) for shape in SHAPES for levels in (lo, lo + 1)]}


def fixture() -> dict:
    nofile = run_oracle(NOFILE["shape"], NOFILE["levels"], NOFILE["limit"])
    return {
        "unitypy": UnityPy.__version__,
        "python": sys.version.split()[0],
        "depth": depth_cases(),
        "nofile": {"limit": NOFILE["limit"], **nofile},
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true", help="regenerate and compare, write nothing")
    args = ap.parse_args()
    if UnityPy.__version__ != "1.25.2":
        print(f"needs UnityPy 1.25.2, found {UnityPy.__version__}", file=sys.stderr)
        return 2
    text = json.dumps(fixture(), indent=1, sort_keys=True) + "\n"
    if args.check:
        if not OUT.exists() or OUT.read_text() != text:
            print(f"stale fixture: {OUT.name}", file=sys.stderr)
            return 1
        return 0
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
