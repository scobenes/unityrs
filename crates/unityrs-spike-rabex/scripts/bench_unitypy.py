#!/usr/bin/env python3
"""UnityPyBoost's side of the rabex spike benchmark (docs/spikes/rabex.md).

    python bench_unitypy.py INDEX.json [-j 8] [--build B] [--limit N] [--phase serial|parallel|both]

The same work as `rabex-dump bench`: load each input from disk
(`Environment.load_file`), then `parse_as_dict()` every object of every
SerializedFile. Serial in this process, then a fork pool of N workers.
`--phase` runs one side only, so `mem_peak.py` sees that side's peak alone.
The per-worker `ru_maxrss` figures count copy-on-write pages shared with the
parent once per worker; use `mem_peak.py` for memory.
"""

import argparse
import json
import os
import resource
import sys
import time
from multiprocessing import get_context
from pathlib import Path

import UnityPy
from UnityPy.files import BundleFile, SerializedFile, WebFile
from UnityPy.helpers import TypeTreeHelper

assert TypeTreeHelper.read_typetree_boost is not None, "UnityPyBoost is not active"


def serialized_files(f):
    if isinstance(f, SerializedFile):
        yield f
    elif isinstance(f, (BundleFile, WebFile)):
        for child in f.files.values():
            yield from serialized_files(child)


def work(path):
    t0 = time.perf_counter()
    try:
        f = UnityPy.Environment().load_file(path)
    except Exception:  # noqa: BLE001
        return 0, 0, 1, time.perf_counter() - t0, 0.0, os.getpid(), peak()
    t1 = time.perf_counter()
    n = errors = 0
    for sf in serialized_files(f):
        for obj in sf.objects.values():
            n += 1
            try:
                obj.parse_as_dict()
            except Exception:  # noqa: BLE001
                errors += 1
    return n, errors, 0, t1 - t0, time.perf_counter() - t1, os.getpid(), peak()


def peak():
    return resource.getrusage(resource.RUSAGE_SELF).ru_maxrss * 1024


def summarise(results, wall, size):
    objects = sum(r[0] for r in results)
    return {
        "wall_s": wall,
        "objects_per_s": objects / wall if wall else None,
        "mb_per_s": size / 1e6 / wall if wall else None,
        "objects": objects,
        "value_errors": sum(r[1] for r in results),
        "file_errors": sum(r[2] for r in results),
        "cpu_s": {"load": sum(r[3] for r in results), "values": sum(r[4] for r in results)},
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("index")
    ap.add_argument("-j", "--jobs", type=int, default=8)
    ap.add_argument("--build")
    ap.add_argument("--limit", type=int)
    ap.add_argument("--phase", choices=("serial", "parallel", "both"), default="both")
    args = ap.parse_args()
    index = json.loads(Path(args.index).read_text())
    root = Path(args.index).parent
    paths = sorted(
        (
            str(root / e["path"])
            for e in index["files"]
            if e.get("kind") in ("bundle", "unitypy-sample") and (not args.build or str(e.get("build")) == args.build)
        ),
        key=lambda p: os.fsencode(os.path.relpath(p, root)),
    )[: args.limit]
    size = 0
    for p in paths:  # warm the page cache, as the Rust side does
        size += len(Path(p).read_bytes())

    serial, serial_wall, serial_rss = [], 0.0, 0
    if args.phase != "parallel":
        t0 = time.perf_counter()
        serial = [work(p) for p in paths]
        serial_wall = time.perf_counter() - t0
        serial_rss = peak()

    parallel = None
    if args.phase != "serial":
        t0 = time.perf_counter()
        with get_context("fork").Pool(args.jobs) as pool:
            par = pool.map(work, paths, chunksize=1)
        par_wall = time.perf_counter() - t0
        worker_peaks = {}
        for r in par:
            worker_peaks[r[5]] = max(worker_peaks.get(r[5], 0), r[6])
        parallel = {
            **summarise(par, par_wall, size),
            "jobs": args.jobs,
            "worker_peak_rss_sum": sum(worker_peaks.values()),
            "worker_peak_rss_max": max(worker_peaks.values()),
        }

    out = {
        "inputs": len(paths),
        "input_bytes": size,
        "unitypy": UnityPy.__version__,
        "serial": {**summarise(serial, serial_wall, size), "peak_rss": serial_rss} if serial else None,
        "parallel": parallel,
    }
    json.dump(out, sys.stdout, indent=1)
    print()


if __name__ == "__main__":
    main()
