#!/usr/bin/env python3
"""Peak memory of a command and every process it starts (docs/spikes/rabex.md).

    python mem_peak.py [--interval S] [--out PATH] -- CMD [ARGS...]

Runs CMD and samples its whole process tree every S seconds (default 0.05):
the sum of each process's PSS from /proc/<pid>/smaps_rollup. PSS charges a
page shared by n processes 1/n to each, so a fork pool's copy-on-write pages
inherited from the parent count once, not once per worker, and one process
with many threads is measured the same way as many forked processes. The
report (JSON, on stderr or to PATH) also gives the summed RSS for contrast,
which counts every shared page once per process. CMD's stdout passes through.
"""

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path


def _children():
    """ppid -> [pid] for every process visible in /proc."""
    kids = {}
    for d in os.listdir("/proc"):
        if not d.isdigit():
            continue
        try:
            stat = Path(f"/proc/{d}/stat").read_text()
        except OSError:
            continue
        # The command name is in parentheses and may hold spaces.
        ppid = int(stat[stat.rindex(")") + 2 :].split()[1])
        kids.setdefault(ppid, []).append(int(d))
    return kids


def tree_memory(root):
    """(summed PSS bytes, summed RSS bytes, processes) of root and its descendants."""
    kids = _children()
    pss = rss = n = 0
    stack = [root]
    while stack:
        pid = stack.pop()
        stack.extend(kids.get(pid, ()))
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


def run(cmd, interval=0.05):
    """Runs cmd to completion; returns (exit code, report)."""
    t0 = time.perf_counter()
    proc = subprocess.Popen(cmd)
    peak = {"pss": 0, "rss": 0, "processes": 0}
    samples = 0
    while proc.poll() is None:
        pss, rss, n = tree_memory(proc.pid)
        samples += 1
        peak["pss"] = max(peak["pss"], pss)
        peak["rss"] = max(peak["rss"], rss)
        peak["processes"] = max(peak["processes"], n)
        time.sleep(interval)
    report = {
        "command": cmd,
        "exit": proc.returncode,
        "wall_s": time.perf_counter() - t0,
        "interval_s": interval,
        "samples": samples,
        "peak_pss_sum_bytes": peak["pss"],
        "peak_rss_sum_bytes": peak["rss"],
        "max_processes": peak["processes"],
    }
    return proc.returncode, report


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--interval", type=float, default=0.05)
    ap.add_argument("--out", help="write the report here instead of stderr")
    ap.add_argument("cmd", nargs=argparse.REMAINDER)
    args = ap.parse_args()
    cmd = args.cmd[1:] if args.cmd[:1] == ["--"] else args.cmd
    if not cmd:
        ap.error("no command")
    code, report = run(cmd, args.interval)
    text = json.dumps(report, indent=1)
    if args.out:
        Path(args.out).write_text(text + "\n")
    else:
        print(text, file=sys.stderr)
    sys.exit(code)


if __name__ == "__main__":
    main()
