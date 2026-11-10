#!/usr/bin/env python3
"""Peak memory of a command and every process it starts, and the load beside it (docs/plans/core.md section 10).

    python tools/perf/mem_peak.py [--interval S] [--out PATH] -- CMD [ARGS...]

Runs CMD (through tools/parity/child.py) and, every S seconds (default 0.05), samples every process this one
started, with all their descendants: the sum of each one's PSS from /proc/<pid>/smaps_rollup, the method of
docs/spikes/rabex.md section 5. PSS charges a page shared by n processes 1/n to each, so a fork pool's
copy-on-write pages count once, and one process of many threads is measured as many forked processes are. The
summed RSS is given for contrast; it counts a shared page once per process.

Each sample also reads every other process the system shows (`others`): the CPU each used while the command ran
(utime + stime, from its first sample to its last; one first seen after the first sample counts from zero, as it
started during the run), so a timing can be read beside what else was running in the container. The report adds
/proc/loadavg before and after, and the whole machine's busy CPUs over the run from /proc/stat (in a container
that is the host kernel's count, every container's and the host's work included).

The report is JSON, on stderr or to PATH; CMD's stdout passes through. Standard library only.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "parity"))

import child  # noqa: E402

TICK = os.sysconf("SC_CLK_TCK")


def processes() -> dict[int, tuple[int, int]]:
    """pid -> (ppid, utime + stime in clock ticks) of every process /proc shows."""
    out = {}
    for d in os.listdir("/proc"):
        if not d.isdigit():
            continue
        try:
            stat = Path(f"/proc/{d}/stat").read_text()
        except OSError:
            continue
        # The command name is in parentheses and may hold spaces and parentheses.
        fields = stat[stat.rindex(")") + 2 :].split()
        out[int(d)] = (int(fields[1]), int(fields[11]) + int(fields[12]))
    return out


def descendants(root: int, procs) -> set[int]:
    """Every process under `root` (not `root` itself), by the ppid links of `procs`."""
    kids = {}
    for pid, (ppid, _) in procs.items():
        kids.setdefault(ppid, []).append(pid)
    out, stack = set(), list(kids.get(root, ()))
    while stack:
        pid = stack.pop()
        out.add(pid)
        stack.extend(kids.get(pid, ()))
    return out


def memory(pids) -> tuple[int, int, int]:
    """(summed PSS bytes, summed RSS bytes, processes read) of `pids`."""
    pss = rss = n = 0
    for pid in pids:
        try:
            text = Path(f"/proc/{pid}/smaps_rollup").read_text()
        except OSError:
            continue
        n += 1
        for line in text.splitlines():
            if line.startswith("Pss:"):
                pss += int(line.split()[1]) * 1024
            elif line.startswith("Rss:"):
                rss += int(line.split()[1]) * 1024
    return pss, rss, n


def busy_ticks() -> tuple[int, int]:
    """(busy, total) clock ticks of all CPUs from /proc/stat's `cpu` line (busy: all but idle and iowait)."""
    fields = [int(x) for x in Path("/proc/stat").read_text().splitlines()[0].split()[1:]]
    total = sum(fields[:8])
    idle = fields[3] + (fields[4] if len(fields) > 4 else 0)
    return total - idle, total


def loadavg() -> list[float]:
    return [float(x) for x in Path("/proc/loadavg").read_text().split()[:3]]


class Others:
    """The CPU every process outside `ours` used between its first sample and its last."""

    def __init__(self):
        self.first, self.last, self.samples = {}, {}, 0

    def sample(self, procs, ours) -> None:
        for pid, (_, cpu) in procs.items():
            if pid in ours:
                continue
            if pid not in self.first:
                self.first[pid] = cpu if self.samples == 0 else 0
            self.last[pid] = cpu
        self.samples += 1

    def cpu_s(self) -> float:
        return sum(max(0, self.last[p] - self.first[p]) for p in self.last) / TICK


class Sampler(threading.Thread):
    """Samples this process's descendants and every other process until stopped."""

    def __init__(self, interval: float):
        super().__init__(daemon=True)
        self.interval, self.done = interval, threading.Event()
        self.peak = {"pss": 0, "rss": 0, "processes": 0}
        self.samples, self.others = 0, Others()
        self.me = os.getpid()

    def once(self) -> None:
        procs = processes()
        ours = descendants(self.me, procs)
        pss, rss, n = memory(ours)
        self.peak["pss"] = max(self.peak["pss"], pss)
        self.peak["rss"] = max(self.peak["rss"], rss)
        self.peak["processes"] = max(self.peak["processes"], n)
        self.others.sample(procs, ours | {self.me})
        self.samples += 1

    def run(self) -> None:
        while not self.done.is_set():
            self.once()
            self.done.wait(self.interval)


def measure(cmd, interval: float = 0.05, **kwargs):
    """Runs `cmd` with `child.run(cmd, **kwargs)` while sampling: (CompletedProcess, report)."""
    load0, (busy0, total0) = loadavg(), busy_ticks()
    t0 = time.perf_counter()
    sampler = Sampler(interval)
    sampler.once()
    sampler.start()
    try:
        proc = child.run(cmd, **kwargs)
    finally:
        sampler.done.set()
        sampler.join()
    wall = time.perf_counter() - t0
    busy1, total1 = busy_ticks()
    cpus = os.cpu_count() or 1
    box = (busy1 - busy0) / max(1, total1 - total0) * cpus
    others = sampler.others.cpu_s()
    report = {
        "command": [os.fsdecode(c) for c in cmd],
        "exit": proc.returncode,
        "wall_s": wall,
        "interval_s": interval,
        "samples": sampler.samples,
        "peak_pss_sum_bytes": sampler.peak["pss"],
        "peak_rss_sum_bytes": sampler.peak["rss"],
        "max_processes": sampler.peak["processes"],
        "load": {
            "loadavg_before": load0,
            "loadavg_after": loadavg(),
            "cpus": cpus,
            "others_cpu_s": others,
            "others_cores": others / wall if wall else 0.0,
            "box_busy_cores": box,
        },
    }
    return proc, report


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--interval", type=float, default=0.05)
    ap.add_argument("--out", help="write the report here instead of stderr")
    ap.add_argument("cmd", nargs=argparse.REMAINDER)
    args = ap.parse_args(argv)
    cmd = args.cmd[1:] if args.cmd[:1] == ["--"] else args.cmd
    if not cmd:
        ap.error("no command")
    proc, report = measure(cmd, args.interval)
    text = json.dumps(report, indent=1)
    if args.out:
        Path(args.out).write_text(text + "\n")
    else:
        print(text, file=sys.stderr)
    return proc.returncode


if __name__ == "__main__":
    sys.exit(main())
