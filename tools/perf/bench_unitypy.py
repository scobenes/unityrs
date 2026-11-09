#!/usr/bin/env python3
"""UnityPyBoost's side of the perf bench (docs/plans/core.md section 10).

    python tools/perf/bench_unitypy.py INDEX.json [--phase serial|parallel] [-j 8] [--kind K ...] [--sample N]

The same work as unityrs's side (`cargo run --release -p unityrs-dump --example bench`): the inputs the dump
selects (`unitypy_dump.py`'s own selection: the index's bundle and UnityPy-sample entries, ordered by name), each
read once first to warm the page cache, then each loaded from disk with `UnityPy.Environment().load_file(path)`
and every object of every SerializedFile (each bundle's `files` dict, in order, nested bundles included) read
with `obj.parse_as_dict()`. `serial` runs in this process; `parallel` in a fork pool of N workers that take the
inputs in order, one at a time. It refuses to run on anything but the pinned oracle stack (UnityPy 1.25.2 with
the pinned UnityPyBoost), and prints one JSON object: throughput, each input's load and value CPU summed
(`time.process_time` in the process that read it), and the peak RSS of this process (serial) or of the largest
worker (parallel). Memory proper is `mem_peak.py`'s summed PSS, taken around this process (perf.py).
"""

from __future__ import annotations

import argparse
import json
import os
import resource
import sys
import time
from multiprocessing import get_context
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "parity"))

import unitypy_dump  # noqa: E402  (imports UnityPy, with UnityPyBoost when it is installed)

import UnityPy  # noqa: E402
from UnityPy.files import BundleFile, SerializedFile, WebFile  # noqa: E402


def stack_problem() -> str | None:
    """Why this is not UnityPy 1.25.2 with the pinned UnityPyBoost, or None."""
    if not unitypy_dump.HAVE_BOOST:
        return "UnityPyBoost is not loaded"
    if UnityPy.__version__ != unitypy_dump.C.ORACLE_UNITYPY:
        return f"UnityPy is {UnityPy.__version__}, not {unitypy_dump.C.ORACLE_UNITYPY}"
    if unitypy_dump.BOOST_SHA256 != unitypy_dump.C.ORACLE_BOOST_SHA256:
        return f"UnityPyBoost has sha256 {unitypy_dump.BOOST_SHA256}, not the pinned wheel's"
    return None


def inputs(index: str, kinds=None, sample=None) -> list[str]:
    """The paths `unitypy_dump.py --index INDEX [--kind ...] [--sample N]` dumps, in its order."""
    args = argparse.Namespace(
        input=index,
        index=True,
        kind=list(kinds or unitypy_dump.INDEX_KINDS),
        build=None,
        name=None,
        sample=sample,
        limit=None,
    )
    return [os.fsdecode(path) for path, _name, _sha in unitypy_dump.collect_inputs(args)]


def serialized_files(f):
    if isinstance(f, SerializedFile):
        yield f
    elif isinstance(f, (BundleFile, WebFile)):
        for entry in f.files.values():
            yield from serialized_files(entry)


def work(path: str) -> tuple[int, int, int, float, float]:
    """(objects, value errors, file errors, load CPU s, values CPU s) of one input."""
    t0 = time.process_time()
    try:
        f = UnityPy.Environment().load_file(path)
    except Exception:  # noqa: BLE001  (a file UnityPy fails is counted, as the other side counts it)
        return 0, 0, 1, time.process_time() - t0, 0.0
    t1 = time.process_time()
    n = errors = 0
    for sf in serialized_files(f):
        for obj in sf.objects.values():
            n += 1
            try:
                obj.parse_as_dict()
            except Exception:  # noqa: BLE001
                errors += 1
    return n, errors, 0, t1 - t0, time.process_time() - t1


def run(paths: list[str], phase: str, jobs: int) -> dict:
    """One pass over `paths`: the JSON the module docs describe (without `input_bytes`)."""
    t0 = time.perf_counter()
    if phase == "serial":
        results = [work(p) for p in paths]
    else:
        with get_context("fork").Pool(jobs) as pool:
            results = pool.map(work, paths, chunksize=1)
    wall = time.perf_counter() - t0
    objects = sum(r[0] for r in results)
    own = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss * 1024
    kids = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss * 1024
    return {
        "bench": "unitypy",
        "unitypy": UnityPy.__version__,
        "inputs": len(paths),
        "jobs": 1 if phase == "serial" else jobs,
        "phase": phase,
        "wall_s": wall,
        "objects": objects,
        "objects_per_s": objects / wall if wall else None,
        "value_errors": sum(r[1] for r in results),
        "file_errors": sum(r[2] for r in results),
        "cpu_s": {"load": sum(r[3] for r in results), "values": sum(r[4] for r in results)},
        "peak_rss_bytes": own if phase == "serial" else kids,
    }


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("index")
    ap.add_argument("--phase", choices=("serial", "parallel"), default="serial")
    ap.add_argument("-j", "--jobs", type=int, default=8)
    ap.add_argument("--kind", action="append")
    ap.add_argument("--sample", type=int)
    args = ap.parse_args(argv)
    problem = stack_problem()
    if problem:
        print(f"bench_unitypy: not the pinned oracle: {problem}", file=sys.stderr)
        return 2
    paths = inputs(args.index, args.kind, args.sample)
    size = 0
    for p in paths:  # warm the page cache, as the other side does
        size += len(Path(p).read_bytes())
    out = run(paths, args.phase, args.jobs)
    out["input_bytes"] = size
    out["mb_per_s"] = size / 1e6 / out["wall_s"] if out["wall_s"] else None
    json.dump(out, sys.stdout)
    print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
