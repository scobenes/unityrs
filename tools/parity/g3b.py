#!/usr/bin/env python3
"""G3b's verdict (docs/plans/core.md section 6): the mutated inputs the two dumps differ on, each with its reason.

    python tools/parity/g3b.py ORACLE.jsonl.gz UNITYRS.jsonl.gz --inputs DIR \\
        [--rerun ORACLE2.jsonl.gz] [--memory-cap BYTES] [-j N] [--list OUT.json]

Runs in the oracle image (the crash check below loads UnityPy). Two inputs agree
when every record of theirs does, notes stripped (compare.py's rule; compare.py
itself still has to pass the dumps' format). Every input that differs gets one
class:

- `limit`: a unityrs record of the input carries a limit refusal (`limit <Kind>:`
  or a memory budget refusal in its note), a `PARITY.md` safety divergence
  (section 19.3 and the limits table).
- `crash_non_utf8`: the oracle's record is the crash record of a UnityPyBoost
  signal, and that input, dumped again with each typetree node's type and name
  checked as UTF-8 before UnityPyBoost's constructor sees them, stops at a node
  that is not, with the signal that member gives (`TypeTreeHelper.cpp:1112`, a
  type: SIGSEGV; `:1218`, a name: SIGABRT): `docs/CANONICAL_DUMP.md` section
  7's crash divergence. unityrs reads that node, hex-tagged, and records what
  follows: the whole file, or the error it meets further on.
- `memory_bound`: the oracle's crash record of a `MemoryError` under
  `unitypy_dump.py --memory-cap` (the machine's limit, not UnityPy's answer),
  where that input, dumped again under the same cap, meets the `MemoryError` at
  one of `ALLOCATION_SITES`: an allocation UnityPy makes from a count or a
  header before reading what it counts. Given the memory, UnityPy reads on and
  fails where the bytes run out, as unityrs does without allocating: the
  `PARITY.md` section 19.3 row "a count UnityPy allocates before it reads".
- `out_of_memory`: such a crash record at any other site. `--rerun` takes a
  dump of some inputs made under a larger cap and uses its records instead.
  With unityrs at a limit the input is `limit`.
- `other`: anything else, a bug until shown otherwise.

`--list` writes every differing input with its class. Exit status: 0 when every
differing input is `limit`, `crash_non_utf8` or `memory_bound`, 1 otherwise, 2
for a usage error.
"""

from __future__ import annotations

import argparse
import gzip
import json
import os
import re
import resource
import signal
import sys
import traceback
from collections import Counter, defaultdict
from multiprocessing import get_context
from multiprocessing.connection import wait
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import canonical as C  # noqa: E402
import compare  # noqa: E402

LIMIT = re.compile(r"\blimit (\w+):|memory budget refused")
ADMITTED = ("limit", "crash_non_utf8", "memory_bound")
# Where UnityPy allocates from a count or a header before reading what it counts
# (the innermost UnityPy frame of the MemoryError), as `path:line`.
ALLOCATION_SITES = {
    # An old-format tree's child count: `node.m_Children = [dummy_node] * children_count`.
    "helpers/TypeTreeNode.py:107": "an old-format node's children_count",
    # An LZMA block's dictionary size, from its 5-byte header: `lzma.LZMADecompressor(...)`.
    "helpers/CompressionHelper.py:31": "an LZMA block's dictionary size",
    # UnityPyBoost's list for an array's count (`PyList_New`) under `read_typetree_boost`.
    "helpers/TypeTreeHelper.py:148": "an array's count, in UnityPyBoost",
}
# The signal UnityPyBoost dies of when a node's member is not UTF-8.
SIGNAL_OF = {"m_Type": "SIGSEGV", "m_Name": "SIGABRT"}
EXAMPLES = 20


def load(path):
    """Each input's file and object records, in dump order, keyed by the input's name as dumped."""
    by_input = defaultdict(list)
    opener = gzip.open if str(path).endswith(".gz") else open
    with opener(path, "rb") as f:
        for line in f:
            rec = json.loads(line)
            if rec.get("rec") in ("file", "object"):
                by_input[json.dumps(rec["file"][0])].append(rec)
    return by_input


def notes(v, out):
    if isinstance(v, dict):
        for k, x in v.items():
            if k == "note" and isinstance(x, str):
                out.append(x)
            else:
                notes(x, out)
    elif isinstance(v, list):
        for x in v:
            notes(x, out)
    return out


def limit_of(records):
    """The first limit a unityrs record of the input names, or None."""
    for r in records:
        for n in notes(r, []):
            m = LIMIT.search(n)
            if m:
                return m.group(1) or "Budget"
    return None


def crash_of(records):
    """("signal", NAME) or ("memory", None) when the oracle's only record is a crash record, else None."""
    if len(records) != 1 or (records[0].get("error") or {}).get("kind") != "crash":
        return None
    note = records[0]["error"].get("note", "")
    if "MemoryError" in note:
        return ("memory", None)
    m = re.search(r"died of (SIG\w+)", note)
    return ("signal", m.group(1) if m else note)


def input_path(root: Path, name) -> tuple[Path, str]:
    """The input's file under `root`, and its name as the oracle's dump_input takes it."""
    import unitypy_dump

    raw = bytes.fromhex(name["hex"]) if isinstance(name, dict) else name.encode("utf-8")
    return root / os.fsdecode(raw), unitypy_dump.name_of(raw)


class NonUtf8Node(BaseException):
    """Raised where UnityPyBoost's node constructor would crash: not an Exception, so no handler records it."""


def memory_site(tb) -> str:
    """The innermost UnityPy frame of a traceback, as `path:line` below the package, or "no UnityPy frame"."""
    for frame in reversed(traceback.extract_tb(tb)):
        path = frame.filename.replace(os.sep, "/")
        if "/UnityPy/" in path:
            return f"{path.rsplit('/UnityPy/', 1)[1]}:{frame.lineno}"
    return "no UnityPy frame"


def _probe(conn, path, rel, cap, mode):
    """Dump one input again in this process, under `cap`, and send what `mode` asks.

    "utf8": with UTF-8-checked typetree nodes, the member that is not UTF-8.
    "memory": as the oracle ran it, the site of its MemoryError.
    Else what happened.
    """
    import unitypy_dump

    unitypy_dump.pin_none()
    if cap:
        resource.setrlimit(resource.RLIMIT_AS, (cap, cap))
    if mode == "memory":
        try:
            unitypy_dump.dump_input((str(path), rel, C.LEVELS))
            result = "no MemoryError"
        except MemoryError:
            result = "site " + memory_site(sys.exc_info()[2])
        except BaseException as e:  # noqa: BLE001 - reported, not judged
            result = f"raised {type(e).__name__}"
        conn.send(result)
        conn.close()
        return
    node = sys.modules["UnityPy.helpers.TypeTreeNode"].TypeTreeNode
    init = node.__init__

    def checked(self, *args, **kwargs):
        # TypeTreeNode_init's order: m_Type (get_node_data_type), then m_Name.
        for i, key in ((1, "m_Type"), (2, "m_Name")):
            v = args[i] if len(args) > i else kwargs.get(key)
            if isinstance(v, str):
                try:
                    v.encode("utf-8")
                except UnicodeEncodeError:
                    raise NonUtf8Node(key) from None
        init(self, *args, **kwargs)

    node.__init__ = checked
    try:
        unitypy_dump.dump_input((str(path), rel, C.LEVELS))
        result = "no node that is not UTF-8"
    except NonUtf8Node as e:
        result = e.args[0]
    except BaseException as e:  # noqa: BLE001 - reported, not judged
        result = f"raised {type(e).__name__}"
    conn.send(result)
    conn.close()


def probe_all(jobs, workers, cap):
    """{input: _probe's result, or the signal its process died of}, at most `workers` at a time."""
    ctx = get_context("fork")
    results, running, todo = {}, {}, list(jobs)
    while todo or running:
        while todo and len(running) < workers:
            key, path, rel, mode = todo.pop()
            recv, send = ctx.Pipe(duplex=False)
            proc = ctx.Process(target=_probe, args=(send, path, rel, cap, mode), daemon=True)
            proc.start()
            send.close()
            running[key] = (proc, recv)
        ready = wait([recv for _, recv in running.values()])
        for key, (proc, recv) in list(running.items()):
            if recv not in ready:
                continue
            try:
                results[key] = recv.recv()
            except (EOFError, OSError):
                results[key] = None
            recv.close()
            proc.join()
            if results[key] is None:
                code = proc.exitcode or 0
                results[key] = f"died of {signal.Signals(-code).name}" if code < 0 else f"exited {code}"
            del running[key]
    return results


def classify(oracle, unityrs, root, workers=1, cap=None):
    """(counts, kinds, rows): each differing input's class and its detail."""
    inputs = sorted(set(oracle) | set(unityrs))
    rows, probes = {}, []
    for key in inputs:
        a, b = oracle.get(key, []), unityrs.get(key, [])
        if [compare.strip_notes(r) for r in a] == [compare.strip_notes(r) for r in b]:
            continue
        crash, limit = crash_of(a), limit_of(b)
        if crash and crash[0] == "signal":
            probes.append((key, *input_path(root, json.loads(key)), "utf8"))
            rows[key] = ("pending", crash[1])
        elif limit:
            rows[key] = ("limit", limit)
        elif crash:
            probes.append((key, *input_path(root, json.loads(key)), "memory"))
            rows[key] = ("pending", "MemoryError")
        else:
            rows[key] = ("other", "records differ")
    for key, result in probe_all(probes, workers, cap).items():
        sig = rows[key][1]
        if sig == "MemoryError":
            site = result.removeprefix("site ")
            if site in ALLOCATION_SITES:
                rows[key] = ("memory_bound", ALLOCATION_SITES[site])
            else:
                rows[key] = ("out_of_memory", f"re-run: {result}")
        elif result in SIGNAL_OF and SIGNAL_OF[result] == sig:
            rows[key] = ("crash_non_utf8", f"{result}: {sig}")
        else:
            rows[key] = ("other", f"oracle {sig}; re-run: {result}")
    counts = Counter(cls for cls, _ in rows.values())
    counts["agree"] = len(inputs) - len(rows)
    kinds = Counter(f"{cls}: {detail}" for cls, detail in rows.values() if cls != "other")
    return counts, kinds, rows


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("oracle")
    ap.add_argument("unityrs")
    ap.add_argument("--inputs", required=True, help="the folder both dumps read")
    ap.add_argument("--rerun", help="an oracle dump of some inputs whose records replace the first dump's")
    ap.add_argument("--memory-cap", type=int, help="the address-space limit of each re-run (as the oracle's)")
    ap.add_argument("-j", "--jobs", type=int, default=1)
    ap.add_argument("--list", help="write every differing input and its class to this JSON file")
    args = ap.parse_args(argv)
    root = Path(args.inputs)
    if not root.is_dir():
        ap.error(f"--inputs {root}: not a folder")
    oracle = load(args.oracle)
    if args.rerun:
        oracle.update(load(args.rerun))
    counts, kinds, rows = classify(oracle, load(args.unityrs), root, max(1, args.jobs), args.memory_cap)
    unadmitted = {k: v for k, v in rows.items() if v[0] not in ADMITTED}
    report = {
        "inputs": sum(counts.values()),
        "classes": dict(sorted(counts.items())),
        "kinds": dict(sorted(kinds.items())),
        "unadmitted": [{"input": json.loads(k), "class": c, "detail": d} for k, (c, d) in sorted(unadmitted.items())][
            :EXAMPLES
        ],
    }
    print(json.dumps(report, indent=1))
    if args.list:
        with open(args.list, "w") as f:
            json.dump([{"input": json.loads(k), "class": c, "detail": d} for k, (c, d) in sorted(rows.items())], f)
    return 1 if unadmitted else 0


if __name__ == "__main__":
    sys.exit(main())
