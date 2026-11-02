#!/usr/bin/env python3
"""G3b's verdict (docs/plans/core.md section 6): every mutated input the two dumps differ on, and why.

    python tools/parity/g3b.py ORACLE.jsonl.gz UNITYRS.jsonl.gz --inputs DIR --unityrs BINARY \\
        [--tpk TPK] [-j N] [--rerun-cap BYTES] [--rerun-rss BYTES] [--list OUT.json]

Runs in the oracle image: the re-runs below load UnityPy and run BINARY, the
unityrs CLI that wrote UNITYRS (with `--tpk TPK` when it was given one).

First both dumps must pass compare.py's checks of their form: a format-1 header
holding L0-L3, canonical lines in the canonical order, no record twice, an `end`
record that counts what precedes it. The oracle's header must name the pinned
stack (compare.py's `--oracle`: a dump any other producer wrote, or one with
no producer, is refused as the reference), and the unityrs dump must not be one
made with limits lifted (its producer's `unbounded`). Any failure there fails
G3b.

Two inputs agree when every record of theirs does, notes left out (compare.py's
rule). An input that differs is admitted only for a reason checked on that
input. The reference is the oracle's records, or, where the oracle's process
died, UnityPy's records from a re-run (below). Classes:

- `crash_non_utf8`: the oracle's process died of a UnityPyBoost signal. The
  re-run gives each typetree node member that is not UTF-8 a stand-in of valid
  UTF-8 before UnityPyBoost's constructor sees it (the member's valid prefix,
  then a private-use tag), and maps the stand-ins back to the original bytes
  wherever the dump writes a name, so UnityPy reads on as it would had the
  bytes been valid. The first such member must give the signal the oracle died
  of (`TypeTreeHelper.cpp:1112`, a type: SIGSEGV; `:1218`, a name: SIGABRT),
  and unityrs's records must equal the re-run's: `docs/CANONICAL_DUMP.md`
  section 7's crash divergence.
- `memory_bound`: the oracle's process ran out of memory under its
  `--memory-cap` (a `MemoryError`, or SIGKILL), and unityrs's records equal a
  re-run's under `--rerun-cap`, with an old-format tree's children read as they
  come instead of into a list of `children_count` placeholders (the one
  allocation UnityPy touches before it reads; an LZMA dictionary and
  UnityPyBoost's list for an array's count are reserved, not touched, so a
  larger cap lets them by). Given the memory, UnityPy reads on and fails where
  the bytes run out, as unityrs does without allocating: `PARITY.md` section
  19.3, "a count UnityPy allocates before it reads".
- `limit`: unityrs's records differ from the reference and carry limit
  refusals (`limit <Kind>: <value> > <max>` in a note, each with value above
  max). Each record no refusal covers must be the reference's, in the same
  place: a refusal covers its own record and, on a file record, the records of
  that file and of every file inside it, so only what a refusal stopped may
  differ. BINARY, run on the input alone, must reproduce the dump's records,
  and run again with those limits lifted (`unityrs dump --unbounded`, adding
  each limit that trips once another is lifted) must give the reference's
  records: so the refusals are the whole difference, and each trips on this
  input. `PARITY.md` section 19.3's limit rows.
- `beyond_memory`: UnityPy has no answer within the re-run's bounds either
  (its re-run ran out of memory, or passed `--rerun-rss`), after the oracle's
  process ran out of memory (or died of the signal of a member that is not
  UTF-8, as above). What still checks unityrs on such an input: its dump holds
  its records, well-formed and canonical (the checks above), and each limit
  refusal among them trips (value above max) and is reproduced by BINARY. What
  the records hold is compared with nothing: the plan's G3b row states this
  gap.
- `unityrs_crash`: unityrs wrote a crash record for the input (its process
  ran out of memory: the allocator or its memory budget refused). No admit
  covers it, whatever the oracle wrote: unityrs's memory is bounded by its
  limits, and records that equal an oracle crash record, notes left out,
  say nothing about why either side died.
- `other`: anything else, a bug until shown otherwise; also an input under
  `--inputs` that either dump lacks (each must hold the input's own file
  record), and an input either dump holds that is no file under `--inputs`.

The inputs are the files under `--inputs`, found as `unitypy_dump.py` finds
them in a folder (every file below it, subfolders in name order, named by its
path relative to the folder), so an input both dumps left out fails G3b.

Each re-run runs in a process of its own under the address-space limit
`--rerun-cap` (default 32 GiB), killed when its resident set passes
`--rerun-rss` (default 1 GiB): a reservation is free, memory touched is not.
A re-run SIGKILL ends from outside (the OOM killer, as no re-run holds more
than `--rerun-rss` for long) is run again, up to `child.KILLED_RUNS` runs,
and one killed on each of them stops G3b (exit 1), saying the system killed
it (review round 9: it was read as out of memory, so the input was admitted
as `beyond_memory` with nothing compared).
`--list` writes every differing input with its class. Exit status: 0 when both
dumps are well-formed, each holds every input under `--inputs` and nothing
else, and every differing input is admitted (`limit`, `crash_non_utf8`,
`memory_bound`, `beyond_memory`), 1 otherwise, 2 when a dump cannot be read,
the oracle's is not the pinned stack's, or a usage error.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import os
import re
import resource
import signal
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import canonical as C  # noqa: E402
import child  # noqa: E402
import compare  # noqa: E402
import scratch  # noqa: E402

ADMITTED = ("limit", "crash_non_utf8", "memory_bound", "beyond_memory")
# A unityrs limit refusal (unityrs_core::LimitError's text).
LIMIT = re.compile(r"\blimit (\w+): (\d+) > (\d+)")
# The limits `unityrs dump --unbounded` lifts (unityrs_dump::LIFTABLE).
LIFTABLE = (
    "InputMemory",
    "InputBytes",
    "BlockOutput",
    "DecodeWork",
    "Inflate",
    "ContainerDepth",
    "TreeDepth",
    "ValueDepth",
    "ValuesPerObject",
    "ZeroSizeElements",
)
# The signal UnityPyBoost dies of when a node's member is not UTF-8.
SIGNAL_OF = {"m_Type": "SIGSEGV", "m_Name": "SIGABRT"}
# The oracle's notes for a process that ran out of memory.
OUT_OF_MEMORY = ("(MemoryError)", "died of SIGKILL")
# The crash of a crash record with no note.
CRASH = "a crash record with no note"
EXAMPLES = 20
RERUN_CAP = 32 << 30
RERUN_RSS = 1 << 30


# ----------------------------------------------------------------- the dumps


def stripped(rec):
    """compare.strip_notes without recursion, so any depth the JSON parser takes: (rec without notes, the notes).
    An object or a list that a long line's parse shares (compare.parse) is copied once, and its copy shared."""
    notes, root = [], [None]
    copies = {}
    stack = [(rec, root, 0)]
    while stack:
        v, parent, at = stack.pop()
        if type(v) in (dict, list) and id(v) in copies:
            parent[at] = copies[id(v)]
            continue
        if isinstance(v, dict):
            out = copies[id(v)] = {}
            for k, x in v.items():
                if k == "note":
                    if isinstance(x, str):
                        notes.append(x)
                    continue
                out[k] = None
                stack.append((x, out, k))
            parent[at] = out
        elif isinstance(v, list):
            out = copies[id(v)] = [None] * len(v)
            stack.extend((x, out, i) for i, x in enumerate(v))
            parent[at] = out
        else:
            parent[at] = v
    return root[0], notes


def digest(rec):
    """A record's content without its notes, as a digest, and its limit refusals as (kind, value, max)."""
    body, notes = stripped(rec)
    text = json.dumps(body, ensure_ascii=False, separators=(",", ":"))
    limits = [(m[1], int(m[2]), int(m[3])) for n in notes for m in LIMIT.finditer(n)]
    return hashlib.blake2b(text.encode("utf-8", "surrogatepass"), digest_size=16).digest(), limits


class Input:
    """One input's records in a dump: [(record key, digest)] in dump order, their limit refusals, the keys of
    the records that carry one, whether its own file record is among them, and its crash: "memory", the
    signal's name, or else the note (`CRASH` when it has none) when its record is a crash record (the only
    record of its input, CANONICAL_DUMP.md section 4.2), else None."""

    def __init__(self):
        self.records, self.limits, self.refused, self.crash, self.own = [], [], set(), None, False

    def add(self, rec):
        d, limits = digest(rec)
        key = compare.record_key(rec)
        self.records.append((key, d))
        self.limits.extend(limits)
        if limits:
            self.refused.add(key)
        if rec["rec"] == "file" and len(rec["file"]) == 1:
            self.own = True
        error = rec.get("error") if rec["rec"] == "file" else None
        if isinstance(error, dict) and error.get("kind") == "crash":
            note = error.get("note") or ""
            m = re.search(r"died of (SIG\w+)", note)
            self.crash = "memory" if any(s in note for s in OUT_OF_MEMORY) else (m[1] if m else note or CRASH)


def unrefused_difference(inp, reference):
    """The first record of `inp`'s or of `reference` (both [(key, digest)]) that no limit refusal of `inp`
    covers and that the other lacks, holds otherwise, or has in another place among the uncovered records;
    None when there is none. A refusal covers its own record and, on a file record, every record of that
    file and of the files inside it (an object's, an entry's): those are what the refusal stopped."""
    files = [k[1] for k in inp.refused if k[0] == "file"]

    def covered(key):
        return key in inp.refused or any(key[1][: len(f)] == f for f in files)

    def named(key):
        names = "/".join(n if isinstance(n, str) else f"<hex {n[1]}>" for n in key[1])
        return f"object {key[2]} of {names}" if key[0] == "object" else f"the file {names}"

    ours = [r for r in inp.records if not covered(r[0])]
    theirs = [r for r in reference if not covered(r[0])]
    for a, b in zip(ours, theirs, strict=False):
        if a != b:
            return named(a[0]) if a[0] == b[0] else f"{named(a[0])} where the reference has {named(b[0])}"
    if len(ours) != len(theirs):
        extra = ours[len(theirs)] if len(ours) > len(theirs) else theirs[len(ours)]
        return f"{named(extra[0])}, in {'unityrs' if len(ours) > len(theirs) else 'the reference'} only"
    return None


def lines_of(data: bytes):
    """A dump's lines: split at "\n" alone, as a JSON line may hold U+2028 or U+0085 unescaped."""
    return [line for line in data.split(b"\n") if line]


def inputs_of(lines, keep=None):
    """{input: Input} for the file and object records among `lines`, keyed by the input's name as written."""
    inputs = {}
    for line in lines:
        rec = compare.parse(line)[0]
        if rec.get("rec") not in ("file", "object"):
            continue
        key = json.dumps(rec["file"][0])
        if keep is None or key == keep:
            inputs.setdefault(key, Input()).add(rec)
    return inputs


def load(path):
    """{input: Input} for a dump (its header and end skipped)."""
    opener = gzip.open if str(path).endswith(".gz") else open
    with opener(path, "rb") as f:
        next(f, None)
        return inputs_of(f)


def inputs_under(root) -> list[str]:
    """The input keys (`inputs_of`'s: the dump's name, as JSON) of the files under the folder `root`, as
    `unitypy_dump.py` `collect_inputs` finds them, and a unityrs dump of the folder with it
    (`crates/unityrs-dump/tests/fixtures/dump/select.json`): every file below it that is one or links to one
    (a linked folder is not entered), named by its path relative to `root` as bytes."""
    given = os.fsencode(root)
    keys = []
    for dirpath, dirnames, filenames in os.walk(given):
        dirnames.sort()
        for name in filenames:
            path = os.path.join(dirpath, name)
            if os.path.isfile(path):
                rel = os.path.relpath(path, given).decode("utf-8", "surrogateescape")
                keys.append(json.dumps(C.enc_name(rel)))
    return sorted(keys)


def form_problems(oracle, unityrs):
    """compare.py's findings on the dumps' form, as {problem: count}; raises C.FormatError or CompareError, the
    latter when `oracle` is not the oracle's dump on the pinned stack."""
    report, ha, hb, _ = compare.run(oracle, unityrs, oracle=True)
    problems = Counter()
    for (level, cls, path, kind), group in report.paths.items():
        # The two end records' counts differ wherever the records do: a
        # difference, not a fault of either dump's form (end_count is).
        if cls == "end" and path.startswith("end.") and " (" not in path:
            continue
        if level == "format" or kind == "duplicate":
            problems[f"{kind}: {path}"] += max(1, len(group["records"]))
    for side, h in (("oracle", ha), ("unityrs", hb)):
        if h.get("levels") != list(C.LEVELS):
            problems[f"the {side} dump holds {h.get('levels')}, not L0-L3"] += 1
    producer = hb.get("producer") if isinstance(hb.get("producer"), dict) else {}
    if producer.get("unbounded"):
        problems[f"the unityrs dump was made with limits lifted: {producer['unbounded']}"] += 1
    return problems, hb


# ------------------------------------------------------------------ re-runs


PAGE = os.sysconf("SC_PAGE_SIZE")


def resident(pid):
    try:
        with open(f"/proc/{pid}/statm") as f:
            return int(f.read().split()[1]) * PAGE
    except (OSError, IndexError, ValueError):
        return 0


def run_all(jobs, workers, cap, rss):
    """Runs each (key, argv, out) with stdout to `out`, at most `workers` at once, each under RLIMIT_AS `cap`
    and killed once its resident set passes `rss`, and run again when SIGKILL from outside ends it
    (`child.run_all`): {key: None when it exited 0, else why not}."""

    hard = resource.getrlimit(resource.RLIMIT_AS)[1]
    soft = cap if hard == resource.RLIM_INFINITY else min(cap, hard)

    def limit():
        resource.setrlimit(resource.RLIMIT_AS, (soft, hard))

    results = {}
    ends = child.run_all(jobs, workers, stop=lambda proc: resident(proc.pid) > rss, preexec_fn=limit)
    for key, end in ends.items():
        code = end.returncode
        if end.stopped:
            results[key] = f"passed the resident bound of {rss} bytes"
        elif code < 0:
            try:
                results[key] = f"died of {signal.Signals(-code).name}"
            except ValueError:
                results[key] = f"died of signal {-code}"
        else:
            results[key] = None if code == 0 else f"exited {code}"
    return results


def input_path(root: Path, name) -> tuple[Path, bytes]:
    """The input's file under `root`, and its name's bytes."""
    raw = bytes.fromhex(name["hex"]) if isinstance(name, dict) else name.encode("utf-8")
    return root / os.fsdecode(raw), raw


def glob_exact(raw: bytes) -> bytes:
    """An fnmatch pattern that matches `raw` alone."""
    special = {ord("["): b"[[]", ord("*"): b"[*]", ord("?"): b"[?]"}
    return b"".join(special.get(b, bytes([b])) for b in raw)


# The re-run of UnityPy, in a process of its own (`g3b.py --probe PATH NAME_HEX`).


def probe_main(path, name_hex):
    """Dumps one input as the oracle does, with UTF-8 stand-ins and an old-format tree read as it comes;
    prints {"outcome", "first", "text"}: "ok" or why not, the first node member that was not UTF-8, the lines."""
    import unitypy_dump

    unitypy_dump.pin_none()
    node = sys.modules["UnityPy.helpers.TypeTreeNode"].TypeTreeNode
    init, stand_in, original, first = node.__init__, {}, {}, []

    def valid(s):
        if not isinstance(s, str):
            return s, False
        try:
            s.encode("utf-8")
            return s, False
        except UnicodeEncodeError:
            pass
        if s not in stand_in:
            cut = next(i for i, ch in enumerate(s) if "\ud800" <= ch <= "\udfff")
            stand_in[s] = f"{s[:cut]}\ue000g3b{len(stand_in)}\ue001"
            original[stand_in[s]] = s
        return stand_in[s], True

    def checked(self, *args, **kwargs):
        # TypeTreeNode_init's order: m_Type (get_node_data_type), then m_Name.
        args = list(args)
        for i, key in ((1, "m_Type"), (2, "m_Name")):
            if len(args) > i:
                args[i], bad = valid(args[i])
            elif key in kwargs:
                kwargs[key], bad = valid(kwargs[key])
            else:
                bad = False
            if bad and not first:
                first.append(key)
        init(self, *args, **kwargs)

    def parse(cls, reader, version):
        # helpers/TypeTreeNode.py parse, but each child appended as it is read
        # instead of `[dummy_node] * children_count` first: the same nodes,
        # the same reads and the same error, without the up-front list.
        dummy_root = cls(-1, "", "", 0, 0, [])
        stack = [(dummy_root, 1)]
        while stack:
            parent, count = stack[-1]
            if count == 1:
                stack.pop()
            else:
                stack[-1] = (parent, count - 1)
            child = cls(
                m_Level=parent.m_Level + 1,
                m_Type=reader.read_string_to_null(),
                m_Name=reader.read_string_to_null(),
                m_ByteSize=reader.read_int(),
                m_VariableCount=reader.read_int() if version == 2 else None,
                m_Index=reader.read_int() if version != 3 else None,
                m_TypeFlags=reader.read_int(),
                m_Version=reader.read_int(),
                m_MetaFlag=reader.read_int() if version != 3 else None,
            )
            parent.m_Children.append(child)
            children_count = reader.read_int()
            if children_count > 0:
                child.m_Children = []
                stack.append((child, children_count))
        return dummy_root.m_Children[0]

    enc_name = C.enc_name
    node.__init__ = checked
    node.parse = classmethod(parse)
    C.enc_name = lambda s: enc_name(original.get(s, s) if isinstance(s, str) else s)
    raw = bytes.fromhex(name_hex)
    try:
        _, text, _, _ = unitypy_dump.dump_input((path, unitypy_dump.name_of(raw), C.LEVELS))
        result = {"outcome": "ok", "text": text}
    except MemoryError:
        result = {"outcome": "out of memory", "text": None}
    except BaseException as e:  # noqa: BLE001 - reported, not judged
        result = {"outcome": f"raised {type(e).__name__}", "text": None}
    result["first"] = first[0] if first else None
    sys.stdout.write(json.dumps(result))
    sys.stdout.flush()
    return 0


# --------------------------------------------------------------- the verdict


class Verdict:
    """Classifies each differing input; `rows` maps it to (class, detail)."""

    def __init__(self, oracle, unityrs, args, version, scratch, expected):
        self.oracle, self.unityrs, self.root = oracle, unityrs, Path(args.inputs)
        self.args, self.version, self.scratch = args, version, scratch
        self.expected = set(expected)
        self.workers = max(1, args.jobs)
        self.rows = {}

    def run(self, jobs):
        return run_all(jobs, self.workers, self.args.rerun_cap, self.args.rerun_rss)

    def probes(self, keys):
        """{input: (outcome, first member not UTF-8, Input or None)} from UnityPy's re-runs."""
        jobs, files = [], {}
        for i, key in enumerate(keys):
            path, raw = input_path(self.root, json.loads(key))
            files[key] = self.scratch / f"p{i}.json"
            argv = [sys.executable, str(Path(__file__).resolve()), "--probe", str(path), raw.hex()]
            jobs.append((key, argv, files[key]))
        out = {}
        for key, why in self.run(jobs).items():
            if why is not None:
                memory = why.startswith("passed the resident bound")
                out[key] = (f"out of memory ({why})" if memory else why, None, None)
                continue
            r = json.loads(files[key].read_bytes())
            inp = inputs_of(lines_of(r["text"].encode("utf-8")), key).get(key, Input()) if r["text"] else None
            out[key] = (r["outcome"], r["first"], inp)
        return out

    def unityrs_runs(self, wanted):
        """{input: Input, or why not} from BINARY run on each input alone with the limits `wanted` names lifted."""
        jobs, files = [], {}
        for i, (key, kinds) in enumerate(wanted):
            _, raw = input_path(self.root, json.loads(key))
            files[key] = self.scratch / f"u{i}.jsonl"
            argv = [self.args.binary, "dump", str(self.root), "--name", glob_exact(raw), "-o", str(files[key])]
            argv += ["-q", "-j", "1"] + (["--tpk", self.args.tpk] if self.args.tpk else [])
            argv += ["--unbounded", ",".join(kinds)] if kinds else []
            jobs.append((key, argv, self.scratch / f"u{i}.out"))
        results = {}
        lifted = dict(wanted)
        for key, why in self.run(jobs).items():
            label = f"unityrs with {'+'.join(lifted[key])} lifted" if lifted[key] else "unityrs"
            if why is not None:
                results[key] = f"{label} {why}"
                continue
            lines = lines_of(files[key].read_bytes())
            head = json.loads(lines[0]) if lines else {}
            producer = head.get("producer") if isinstance(head.get("producer"), dict) else {}
            if producer.get("unityrs") != self.version or tuple(producer.get("unbounded", ())) != lifted[key]:
                results[key] = f"{label} wrote the producer {producer}, not the dump's unityrs {self.version}"
            elif json.loads(lines[-1]).get("rec") != "end":
                results[key] = f"{label} wrote no end record"
            else:
                results[key] = inputs_of(lines[1:], key).get(key, Input())
        return results

    def classify(self):
        keys = sorted(self.expected | set(self.oracle) | set(self.unityrs))
        reference, reason, probe, beyond = {}, {}, [], []
        for key in keys:
            a, b = self.oracle.get(key), self.unityrs.get(key)
            sides = (("unityrs", b), ("oracle", a))
            missing = [side for side, x in sides if x is None or not x.own]
            if key not in self.expected:
                found = " and the ".join(side for side, x in sides if x is not None)
                self.rows[key] = ("other", f"in the {found} dump, but no file under --inputs")
            elif missing:
                self.rows[key] = ("other", f"missing from the {' and the '.join(missing)} dump")
            elif b.crash is not None:
                # Before the records are compared: notes left out, any two crash records of an input are equal.
                self.rows[key] = ("unityrs_crash", f"unityrs: {b.crash}; the oracle: {a.crash or 'no crash'}")
            elif a.crash is None and a.records == b.records:
                continue
            elif a.crash is not None:
                probe.append(key)
            else:
                reference[key], reason[key] = a.records, None
        for key, (outcome, first, inp) in self.probes(probe).items():
            crash = self.oracle[key].crash
            if crash == "memory":
                why = ("memory_bound", "re-run under the larger cap")
            elif first is not None and SIGNAL_OF[first] == crash:
                why = ("crash_non_utf8", f"{first}: {crash}")
            else:
                self.rows[key] = ("other", f"oracle {crash}; the re-run: {outcome}, first member not UTF-8: {first}")
                continue
            if inp is None and outcome.startswith("out of memory"):
                beyond.append((key, why[0]))
            elif inp is None:
                self.rows[key] = ("other", f"after {why[0]}: the re-run {outcome}")
            elif inp.records == self.unityrs[key].records:
                self.rows[key] = why
            else:
                reference[key], reason[key] = inp.records, why[0]
        self.beyond(beyond)
        self.limits(reference, reason)
        return keys

    def beyond(self, inputs):
        """`beyond_memory`: no reference; a limit refusal of unityrs's must still trip, as BINARY reproduces it."""
        check = []
        for key, after in inputs:
            found = self.unityrs[key].limits
            bad = [f"{k}: {v} > {m}" for k, v, m in found if v <= m]
            if bad:
                self.rows[key] = ("other", f"after {after}, out of memory: a limit note that does not trip, {bad[0]}")
            elif found:
                check.append(key)
            else:
                self.rows[key] = ("beyond_memory", f"after {after}")
        for key, got in self.unityrs_runs([(k, ()) for k in check]).items():
            after = dict(inputs)[key]
            why = self.not_reproduced(key, got)
            if why:
                self.rows[key] = ("other", f"after {after}, out of memory: {why}")
            else:
                kinds = "+".join(k for k in LIFTABLE if any(f[0] == k for f in self.unityrs[key].limits))
                self.rows[key] = ("beyond_memory", f"after {after}; unityrs refuses at {kinds}")

    def not_reproduced(self, key, got):
        """Why BINARY's bounded re-run `got` is not the dump's records of `key` and their refusals, or None."""
        if isinstance(got, str):
            why = got
        elif got.records != self.unityrs[key].records:
            why = "other records"
        elif sorted(got.limits) != sorted(self.unityrs[key].limits):
            why = f"other limit refusals {sorted(got.limits)}"
        else:
            return None
        return f"the unityrs binary does not reproduce the dump: {why}"

    def limits(self, reference, reason):
        """The `limit` check for each input whose unityrs records differ from its reference."""
        kinds = {}
        for key in reference:
            ctx = f" (reference: UnityPy's re-run, {reason[key]})" if reason[key] else ""
            found = self.unityrs[key].limits
            bad = [f"{k}: {v} > {m}" for k, v, m in found if k not in LIFTABLE or v <= m]
            differs = unrefused_difference(self.unityrs[key], reference[key])
            if not found:
                self.rows[key] = ("other", "records differ" + ctx)
            elif bad:
                self.rows[key] = ("other", f"a limit note that does not trip or cannot be lifted, {bad[0]}{ctx}")
            elif differs:
                self.rows[key] = ("other", f"a record no limit refusal covers differs, {differs}{ctx}")
            else:
                kinds[key] = tuple(k for k in LIFTABLE if any(f[0] == k for f in found))
        # BINARY must reproduce the dump's records before its lifted runs mean anything.
        for key, got in self.unityrs_runs([(k, ()) for k in kinds]).items():
            why = self.not_reproduced(key, got)
            if why:
                self.rows[key] = ("other", why)
                del kinds[key]
        for _ in LIFTABLE:
            more = {}
            for key, got in self.unityrs_runs(list(kinds.items())).items():
                lifted = "+".join(kinds[key])
                ctx = f" (reference: UnityPy's re-run, {reason[key]})" if reason[key] else ""
                if isinstance(got, str):
                    self.rows[key] = ("other", got + ctx)
                elif got.records == reference[key]:
                    self.rows[key] = ("limit", lifted + (f", after {reason[key]}" if reason[key] else ""))
                else:
                    new = {k for k, v, m in got.limits if k in LIFTABLE and v > m} - set(kinds[key])
                    if new:
                        more[key] = tuple(k for k in LIFTABLE if k in new or k in kinds[key])
                    else:
                        self.rows[key] = ("other", f"records differ with {lifted} lifted{ctx}")
            kinds = more
        for key in kinds:  # unreachable: each round lifts one more of the ten
            self.rows[key] = ("other", "limits still trip with every limit lifted")


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if argv[:1] == ["--probe"] and len(argv) == 3:
        return probe_main(argv[1], argv[2])
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("oracle")
    ap.add_argument("unityrs")
    ap.add_argument("--inputs", required=True, help="the folder both dumps read")
    ap.add_argument("--unityrs", dest="binary", required=True, help="the unityrs CLI that wrote UNITYRS")
    ap.add_argument("--tpk", help="the --tpk UNITYRS was written with")
    ap.add_argument("--rerun-cap", type=int, default=RERUN_CAP, help="each re-run's address-space limit")
    ap.add_argument("--rerun-rss", type=int, default=RERUN_RSS, help="each re-run's resident-set bound")
    ap.add_argument("-j", "--jobs", type=int, default=1)
    ap.add_argument("--list", help="write every differing input and its class to this JSON file")
    args = ap.parse_args(argv)
    if not Path(args.inputs).is_dir():
        ap.error(f"--inputs {args.inputs}: not a folder")
    try:
        problems, hb = form_problems(args.oracle, args.unityrs)
        oracle, unityrs = load(args.oracle), load(args.unityrs)
    except (C.FormatError, compare.CompareError, OSError, ValueError) as e:
        print(f"g3b: {e}", file=sys.stderr)
        return 2
    version = (hb.get("producer") or {}).get("unityrs")
    expected = inputs_under(args.inputs)
    try:
        with scratch.folder("g3b-") as folder:
            v = Verdict(oracle, unityrs, args, version, folder, expected)
            keys = v.classify()
    except child.SystemKilled as e:
        print(f"g3b: {e}", file=sys.stderr)
        return 1
    rows = v.rows
    counts = Counter(cls for cls, _ in rows.values())
    counts["agree"] = len(keys) - len(rows)
    kinds = Counter(f"{cls}: {detail}" for cls, detail in rows.values() if cls in ADMITTED)
    unadmitted = {k: r for k, r in rows.items() if r[0] not in ADMITTED}
    report = {
        "inputs": len(keys),
        "form": dict(sorted(problems.items())),
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
    return 1 if unadmitted or problems else 0


if __name__ == "__main__":
    sys.exit(main())
