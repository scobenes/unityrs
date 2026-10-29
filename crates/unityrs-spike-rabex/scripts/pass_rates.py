#!/usr/bin/env python3
"""Per-level pass rates of a candidate dump against the oracle (docs/spikes/rabex.md).

    python pass_rates.py ORACLE.jsonl[.gz] CANDIDATE.jsonl[.gz]

A record passes a level when the candidate has the record and the level's
members are equal, `note` members aside: L0 is a file record's `l0`; L1 is a
file record's `l1` and `error`, and an object record's `l1`; L2 and L3 are an
object record's `l2` and `l3`. Rates are split into the corpus game's
bundles and UnityPy's samples (the `unitypy-samples/` folder).

Both dumps are in the canonical order (spec section 5), so they are read in
lockstep: a candidate record is a missing one when its key runs ahead.
Identical lines pass every level unparsed, which keeps memory flat.
"""

import gzip
import json
import sys
from collections import defaultdict


def records(path):
    opener = gzip.open if path.endswith(".gz") else open
    with opener(path, "rt", encoding="utf-8") as f:
        for line in f:
            r = json.loads(line)
            if r["rec"] in ("file", "object"):
                yield key(r), line, r


def key(r):
    return (r["rec"], json.dumps(r["file"]), r.get("path_id"))


def strip(v):
    if isinstance(v, dict):
        return {k: strip(x) for k, x in v.items() if k != "note"}
    if isinstance(v, list):
        return [strip(x) for x in v]
    return v


def levels(r):
    if r["rec"] == "file":
        return {"L0": r["l0"], "L1": [r["l1"], r["error"]]}
    return {"L1": r["l1"], "L2": r["l2"], "L3": r["l3"]}


def main():
    cand = records(sys.argv[2])
    current = next(cand, None)
    tally = defaultdict(lambda: [0, 0])
    for k, line, r in records(sys.argv[1]):
        part = "unitypy-samples" if r["file"][0].startswith("unitypy-samples/") else "corpus game"
        c = None
        if current is not None and current[0] == k:
            c = current
            current = next(cand, None)
        for level, value in levels(r).items():
            ok = c is not None and (c[1] == line or strip(levels(c[2]).get(level)) == strip(value))
            for p in (part, "all"):
                tally[(p, level)][0] += ok
                tally[(p, level)][1] += 1
    if current is not None:
        print("warning: candidate records out of order or extra; rates are a lower bound", file=sys.stderr)
    for (part, level), (ok, n) in sorted(tally.items()):
        print(f"{part:16} {level}  {ok:>7}/{n:<7} {100 * ok / n:8.3f}%")


if __name__ == "__main__":
    main()
