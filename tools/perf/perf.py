#!/usr/bin/env python3
"""The perf report's command (docs/plans/core.md sections 6, 8 and 10): every number of it, from one run.

    python tools/perf/perf.py --out DIR [--index /corpus/index.json] [--repeat 3] [-j 8] [--sample N] [--no-build]

Run in the dev container from any folder (`scripts/dev python tools/perf/perf.py ...` from the repository root).
It builds unityrs's bench (`cargo build --release -p unityrs-dump --example bench`, at -j 2) twice: as the
workspace builds it, and with `--cfg unityrs_no_counters`, which compiles the core's counters out
(`BundleFile::counts`), into `target/perf-nocounters`. Then it runs every pass as a process of its own under
`mem_peak.py`'s sampler, which takes the pass's peak summed PSS and the load beside it (/proc/loadavg before and
after, the CPU the container's other processes used, the machine's busy CPUs):

- timed passes, each run `--repeat` times, round by round so drift in the load spreads over every pass: unityrs
  serially and on `-j` threads with the `Unbounded` budget (R4), the same serially with the counters compiled
  out (their cost), serially and on `-j` threads with one shared `Counting` budget (R10), and UnityPyBoost
  serially and in a fork pool of `-j` workers (`bench_unitypy.py`, under `ulimit -v` of 4,000,000 KB a process);
- once each, for counts: unityrs eagerly with a tracked budget serially (each input's peak `Meter::used()` against
  its decoded size, peak RSS against the peak of live charges, the cached runs' peak per bundle, the version
  sources: R3, R5, R10) and on `-j` threads (peak RSS against live charges), eagerly keeping every value (what
  values pin once their handles drop, and what one value holding a slice pins: R3; a pass apart, since kept
  values raise each input's peak), lazily over the R2 sample (the 20 bundles `unitypy_dump.py --kind bundle
  --sample 20` selects: spread evenly over the index's bundles in name order) reading every directory row in
  order, and lazily over the whole selection (R10 under `Lazy`).

DIR (outside the repository; made, or empty) gets `runs.jsonl` (a line per pass as it ends), `per-input-*.jsonl`
(a line per input by position, never a name), `report.json` (every range and count) and `report.md` (the same as
tables). Both name no input, no corpus path and no build of the corpus but by its number. `--sample N` runs every
pass on N inputs spread over the selection, for a trial. Standard library only.
"""

from __future__ import annotations

import argparse
import json
import os
import resource
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(ROOT / "tools" / "parity"))

import child  # noqa: E402
import mem_peak  # noqa: E402
import report  # noqa: E402

UNITYPY_AS_KB = 4_000_000
NOCOUNTERS = "target/perf-nocounters"
BENCH = "release/examples/bench"


def bench_binary(nocounters: bool = False) -> Path:
    return ROOT / (NOCOUNTERS if nocounters else "target") / BENCH


def build_commands() -> list[tuple[list[str], dict]]:
    """The two cargo builds, each (argv, extra environment)."""
    base = ["cargo", "build", "--release", "--locked", "-j", "2", "-p", "unityrs-dump", "--example", "bench"]
    nocounters = {"RUSTFLAGS": "--cfg unityrs_no_counters"}
    return [(base, {}), ([*base, "--target-dir", NOCOUNTERS], nocounters)]


def plan(index: str, jobs: int, sample: int | None, out: Path) -> tuple[list[dict], list[dict]]:
    """(timed passes, once passes): each {name, argv, unitypy}."""
    sel = ["--sample", str(sample)] if sample else []
    urs = [str(bench_binary()), index, *sel]
    nc = [str(bench_binary(True)), index, *sel]
    upy = [sys.executable, str(HERE / "bench_unitypy.py"), index, *sel]
    j = str(jobs)
    timed = [
        {"name": "unityrs-serial", "argv": urs},
        {"name": "unityrs-parallel", "argv": [*urs, "-j", j]},
        {"name": "unityrs-serial-nocounters", "argv": nc},
        {"name": "unityrs-serial-counting", "argv": [*urs, "--budget", "counting"]},
        {"name": "unityrs-parallel-counting", "argv": [*urs, "-j", j, "--budget", "counting"]},
        {"name": "unitypy-serial", "argv": [*upy, "--phase", "serial"], "unitypy": True},
        {"name": "unitypy-parallel", "argv": [*upy, "--phase", "parallel", "-j", j], "unitypy": True},
    ]
    r2 = [str(bench_binary()), index, "--kind", "bundle", "--sample", "20"]  # R2's rule, whatever --sample
    once = [
        {
            "name": "unityrs-tracked-serial",
            "argv": [*urs, "--budget", "tracked", "--per-input", str(out / "per-input-tracked.jsonl")],
        },
        {"name": "unityrs-tracked-parallel", "argv": [*urs, "-j", j, "--budget", "tracked"]},
        # Apart from the tracked passes: keeping every value would raise each input's peak.
        {"name": "unityrs-pin", "argv": [*urs, "--pin", "--per-input", str(out / "per-input-pin.jsonl")]},
        {
            "name": "unityrs-lazy-r2-sample",
            "argv": [*r2, "--load", "lazy", "--budget", "tracked", "--per-input", str(out / "per-input-r2.jsonl")],
        },
        {
            "name": "unityrs-lazy",
            "argv": [*urs, "--load", "lazy", "--budget", "tracked", "--per-input", str(out / "per-input-lazy.jsonl")],
        },
    ]
    for p in timed + once:
        p.setdefault("unitypy", False)
    return timed, once


def limit_address_space() -> None:
    """`ulimit -v` for a UnityPy pass: every process it forks inherits it."""
    resource.setrlimit(resource.RLIMIT_AS, (UNITYPY_AS_KB * 1024, UNITYPY_AS_KB * 1024))


def run_pass(p: dict, round_: int | None, runs_file: Path) -> dict:
    """Runs one pass under the sampler; appends its record to `runs_file` and returns it."""
    kwargs = {"capture_output": True, "text": True}
    if p["unitypy"]:
        kwargs["preexec_fn"] = limit_address_space
    proc, mem = mem_peak.measure(p["argv"], **kwargs)
    if proc.returncode != 0:
        raise SystemExit(f"perf: {p['name']} exited {proc.returncode}:\n{proc.stderr[-4000:]}")
    record = {"phase": p["name"], "round": round_, "bench": json.loads(proc.stdout), "mem": mem}
    with open(runs_file, "a", encoding="utf-8") as fh:
        fh.write(json.dumps(record) + "\n")
    load = mem["load"]
    print(
        f"perf: {p['name']}{f' (round {round_})' if round_ else ''}: {record['bench']['wall_s']:.2f} s, "
        f"peak PSS {mem['peak_pss_sum_bytes'] / 1e6:.0f} MB, loadavg {load['loadavg_before'][0]:.1f}, "
        f"others {load['others_cores']:.1f} cores, machine {load['box_busy_cores']:.1f} busy",
        file=sys.stderr,
        flush=True,
    )
    return record


def check_out(out: Path) -> None:
    full = out.resolve()
    if full == ROOT or ROOT in full.parents:
        raise SystemExit(f"perf: {out} is inside the repository")
    if full.exists() and any(full.iterdir()):
        raise SystemExit(f"perf: {out} is not empty")
    full.mkdir(parents=True, exist_ok=True)


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--index", default="/corpus/index.json")
    ap.add_argument("--repeat", type=int, default=3, help="rounds of the timed passes (0: the counts only)")
    ap.add_argument("-j", "--jobs", type=int, default=8)
    ap.add_argument("--sample", type=int, help="every pass on this many inputs (a trial)")
    ap.add_argument("--no-build", action="store_true", help="use the bench binaries already built")
    args = ap.parse_args(argv)
    check_out(args.out)
    started = time.time()
    builds = []
    if not args.no_build:
        for argv_, env in build_commands():
            t0 = time.perf_counter()
            child.run(argv_, check=True, cwd=ROOT, env={**os.environ, **env})
            builds.append({"argv": argv_, "env": env, "wall_s": time.perf_counter() - t0})
    timed, once = plan(args.index, args.jobs, args.sample, args.out)
    runs_file = args.out / "runs.jsonl"
    records = []
    for r in range(1, args.repeat + 1):
        for p in timed:
            records.append(run_pass(p, r, runs_file))
    for p in once:
        records.append(run_pass(p, None, runs_file))
    meta = {
        "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(started)),
        "took_s": time.time() - started,
        "repeat": args.repeat,
        "jobs": args.jobs,
        "sample": args.sample,
        "builds": builds,
    }
    summary = report.summarise(records, meta)
    (args.out / "report.json").write_text(json.dumps(summary, indent=1) + "\n")
    (args.out / "report.md").write_text(report.markdown(summary))
    print(f"perf: wrote {args.out / 'report.md'} in {meta['took_s']:.0f} s", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
