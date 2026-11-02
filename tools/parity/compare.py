#!/usr/bin/env python3
"""Diff two canonical dumps and summarise the differences by level, class and field path.

    python tools/parity/compare.py REFERENCE.jsonl[.gz] CANDIDATE.jsonl[.gz] [--levels L0,L1] [--json report.json]

REFERENCE is normally the UnityPy dump and CANDIDATE the unityrs one. A
reference unitypy_dump.py wrote must name the pinned stack; with --oracle (every
gate's comparison), any other reference is refused. Records
are matched by key (file path, then path_id), so a candidate that orders
records differently still compares field by field, and the order itself is
checked: each dump against the spec's order, and the two against each other.
Every line of both dumps, byte-identical pairs and the header and end records
included, is checked against the canonical form. Fields named "note" are free
text and never compared.

The levels the reference holds are compared, and a level the candidate lacks is
a difference. --levels narrows the comparison.

Exit status: 0 when the compared levels agree and every line is canonical, 1
when anything differs, 2 when a dump is unreadable or not a canonical dump, or
when the arguments ask for a comparison the reference cannot give.
"""

from __future__ import annotations

import argparse
import gzip
import itertools
import json
import os
import re
import sys
import zlib
from collections import Counter, defaultdict

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import canonical as C  # noqa: E402

# How two records with one key are diffed; compare.py names it all, as before it moved.
from compare_diff import (  # noqa: E402, F401
    MAX_DIFFS_PER_RECORD,
    MAX_ELEMENT_DIFFS,
    Diff,
    compare_records,
    compare_section,
    compared_text,
    json_diff,
    record_class,
    same,
    short,
    strip_notes,
    value_diff,
    where,
)

_INDEX = re.compile(r"\[\d+\]")


class CompareError(Exception):
    """A comparison that cannot be made: bad --levels, or a reference that is not the pinned oracle."""


# ------------------------------------------------------------------- reading


def open_lines(path):
    opener = gzip.open if path.endswith(".gz") else open
    try:
        with opener(path, "rb") as fh:
            yield from fh
    except (EOFError, zlib.error, gzip.BadGzipFile) as e:
        raise C.FormatError(f"{path}: unreadable stream: {e}") from e


def _no_constant(name):
    raise C.FormatError(f"{name} is not a JSON number")


class _Floats:
    """Set by the decoder whenever a line holds a JSON float (the format has none)."""

    seen = False


def _float(text):
    _Floats.seen = True
    return float(text)


def parse(line: bytes):
    """A line's record, and whether it held a JSON float. Unreadable lines raise FormatError.

    It is read at any depth and integer width the dumps hold (canonical_json.loads). A line longer than
    `C.LONG_LINE` shares its equal small objects (`C.sharing_hook`): no reader of a record may change one.
    """
    _Floats.seen = False
    hook = C.sharing_hook() if len(line) > C.LONG_LINE else None
    try:
        rec = C.loads(line.decode("utf-8"), parse_float=_float, parse_constant=_no_constant, object_pairs_hook=hook)
    except ValueError as e:  # JSONDecodeError, UnicodeDecodeError, NaN/Infinity
        raise C.FormatError(f"not a JSON line: {e}") from e
    if not isinstance(rec, dict) or "rec" not in rec:
        raise C.FormatError("a line is not a record object")
    return rec, _Floats.seen


def canonical_line(rec) -> bytes:
    """The bytes the spec's serialisation gives this record."""
    try:
        return (C.dumps(rec) + "\n").encode()
    except UnicodeEncodeError as e:  # a lone-surrogate escape: valid JSON, not valid Unicode
        raise C.FormatError(f"a string is not valid Unicode: {e}") from e


def _name_key(n):
    """A file-name component as a hashable, type-exact key, or None when it is not a name."""
    if type(n) is str:
        return n
    if type(n) is dict and list(n) == ["t", "hex"] and n["t"] == "str" and type(n["hex"]) is str:
        return ("hex", n["hex"])
    return None


def record_key(rec):
    """A record's identity: ("file", names), ("object", names, path_id) or ("end",)."""
    kind = rec["rec"]
    if kind in ("file", "object"):
        names = rec.get("file")
        keys = [_name_key(n) for n in names] if type(names) is list else []
        if not keys or None in keys:
            raise C.FormatError(f"a {kind} record's file {short(names)} is not a list of names")
        if kind == "file":
            return ("file", tuple(keys))
        path_id = rec.get("path_id")
        if type(path_id) is not int:
            raise C.FormatError(f"path_id {short(path_id)} is not an integer")
        return ("object", tuple(keys), path_id)
    if kind == "end":
        return ("end",)
    if kind == "dump":
        raise C.FormatError("a second dump header")
    raise C.FormatError(f"unknown record kind {short(kind)}")


def _name_bytes(key):
    """A file-name key's bytes (see _name_key), which order inputs; None for malformed hex."""
    if type(key) is str:
        return key.encode("utf-8")
    try:
        return bytes.fromhex(key[1])
    except ValueError:
        return None


class Order:
    """Section 5's order, checked as one dump's file and object records go past.

    Inputs ascend by their names' bytes; each input is written depth first;
    a file's objects follow its record directly, in ascending path_id; and a
    container's entries follow its directory, a repeated path at its first
    position. The directory is known only when the container's l1 is dumped;
    without it, compare_orders still holds the entries to the other dump's order.
    """

    def __init__(self):
        self.last_input = None
        self.open = []  # the current input's file paths that are still open, outermost first
        self.objects_of = self.last_path_id = None
        self.entries = {}  # container path -> ({entry name: directory position}, last entry's position)

    def check(self, rec, key):
        out = []
        if key[0] == "file":
            names = key[1]
            if len(names) == 1:
                raw = _name_bytes(names[0])
                if raw is not None and self.last_input is not None and raw < self.last_input:
                    out.append(("record", "order", "inputs ascend by the bytes of their names"))
                self.last_input = raw if raw is not None else self.last_input
                self.open = []
            else:
                while self.open and self.open[-1] != names[: len(self.open[-1])]:
                    self.open.pop()
                parent = names[:-1]
                if not self.open or self.open[-1] != parent:
                    out.append(("record", "order", "an entry follows its container's records, before the next input"))
                elif parent in self.entries:
                    positions, last = self.entries[parent]
                    pos = positions.get(names[-1])
                    if pos is not None:
                        if last is not None and pos < last:
                            out.append(("record", "order", "entries follow their container's directory order"))
                        self.entries[parent] = (positions, pos)
            self.open.append(names)
            self.objects_of, self.last_path_id = names, None
            l1 = rec.get("l1")
            directory = l1.get("directory") if type(l1) is dict else None
            if type(directory) is list:
                positions = {}
                for row in directory:
                    name = _name_key(row[3]) if type(row) is list and len(row) == 4 else None
                    if name is not None:
                        positions.setdefault(name, len(positions))
                self.entries[names] = (positions, None)
        elif key[0] == "object":
            if key[1] != self.objects_of:
                out.append(("record", "order", "objects follow their file's record directly"))
            else:
                if self.last_path_id is not None and key[2] < self.last_path_id:
                    out.append(("record", "order", "objects ascend by path_id"))
                self.last_path_id = key[2]
        return out


def compare_orders(a, b):
    """The first pair of records the two dumps hold in different orders, or None."""
    seq_a = [k for k in a.sequence if k in b.seen]
    seq_b = [k for k in b.sequence if k in a.seen]
    return next(((x, y) for x, y in zip(seq_a, seq_b, strict=True) if x != y), None)


# -------------------------------------------------------------------- driver


class Report:
    def __init__(self, examples):
        self.examples = examples
        self.counts = Counter()
        self.diffs_by_level = Counter()
        self.records_by_level = defaultdict(set)
        self.records_by_class = defaultdict(set)
        self.paths = defaultdict(lambda: {"records": set(), "fields": 0, "examples": []})
        self.missing_levels = []

    def add(self, diffs):
        for d in diffs:
            self.diffs_by_level[d.level] += 1
            self.records_by_level[d.level].add(d.where)
            self.records_by_class[(d.level, d.cls)].add(d.where)
            group = self.paths[(d.level, d.cls, _INDEX.sub("[]", d.path), d.kind)]
            group["fields"] += 1
            group["records"].add(d.where)
            if len(group["examples"]) < self.examples:
                group["examples"].append(d.as_dict())

    def has_diffs(self):
        return bool(self.diffs_by_level)

    def as_dict(self):
        return {
            "counts": dict(self.counts),
            "missing_levels": self.missing_levels,
            "by_level": {
                lv: {"records": len(r), "fields": self.diffs_by_level[lv]}
                for lv, r in sorted(self.records_by_level.items())
            },
            "by_class": [
                {"level": lv, "class": cls, "records": len(r)}
                for (lv, cls), r in sorted(self.records_by_class.items(), key=lambda i: (i[0][0], -len(i[1]), i[0][1]))
            ],
            "by_path": [
                {
                    "level": lv,
                    "class": cls,
                    "path": p,
                    "kind": k,
                    "records": len(g["records"]),
                    "fields": g["fields"],
                    "examples": g["examples"],
                }
                for (lv, cls, p, k), g in sorted(
                    self.paths.items(), key=lambda i: (i[0][0], -len(i[1]["records"]), i[0][1:])
                )
            ],
        }


class Side:
    """One dump being read: its header, records waiting for a match, and what it has held."""

    def __init__(self, name, path):
        self.name, self.path = name, path
        self.lines = open_lines(path)
        self.pending = {}  # key -> (line, problems), waiting for the other side
        self.seen = set()  # every key read, for duplicates
        self.order = Order()
        self.sequence = []  # file and object keys in the order read, first records only
        self.misordered = False
        self.kinds = Counter()
        self.end = None
        self.header = self.levels = None

    def read_header(self):
        try:
            line = next(self.lines)
        except StopIteration as e:
            raise C.FormatError(f"the {self.name} dump is empty") from e
        h, saw_float = parse(line)
        if h.get("rec") != "dump" or h.get("format") != C.FORMAT:
            raise C.FormatError(f"the {self.name} dump does not start with a {C.FORMAT} header")
        version = h.get("version")
        if type(version) is not int or version != C.VERSION:
            raise C.FormatError(f"the {self.name} dump is format version {short(version)}, this tool reads {C.VERSION}")
        levels = h.get("levels")
        if type(levels) is not list or "L0" not in levels or levels != [lv for lv in C.LEVELS if lv in levels]:
            raise C.FormatError(
                f"the {self.name} header's levels {short(levels)} are not L0 followed by any of L1, L2, L3 in order"
            )
        self.header, self.levels = h, levels
        return line_problems(h, line, saw_float, levels)


def line_problems(rec, line, saw_float, levels):
    """Every canonical-form rule this line breaks on its own, as (path, kind, detail) triples."""
    problems = []
    if canonical_line(rec) != line:
        problems.append(("line", "noncanonical_json", None))
    if saw_float:
        problems.append(("line", "json_float", None))
    problems += C.check_record(rec, levels)
    if len(problems) > MAX_DIFFS_PER_RECORD:
        extra = len(problems) - MAX_DIFFS_PER_RECORD
        problems = [*problems[:MAX_DIFFS_PER_RECORD], ("line", "more", extra)]
    return problems


def reference_problems(rec):
    """What a reference record may hold that vouches for nothing: an `encode` or a `dumper` error.

    The oracle writes one only where its own code failed on that object (its encoder) or input (the rest
    of the dumper), a bug in the oracle, never an outcome of the input (spec 4.2, 4.3); so a comparison
    with such a reference never passes there.
    """
    if rec.get("rec") == "object":
        l2 = rec.get("l2")
        error = l2.get("error") if isinstance(l2, dict) else None
        if isinstance(error, dict) and error.get("kind") == "encode":
            return [("l2.error", "reference_encode", "the oracle's encoder refused the value UnityPy read")]
    elif rec.get("rec") == "file":
        error = rec.get("error")
        if isinstance(error, dict) and error.get("kind") == "dumper":
            return [("error", "reference_dumper", "the oracle's own code failed on this input")]
    return []


def check_reference_producer(header, oracle=False):
    """A dump from the oracle dumper must come from the pinned stack, or every difference it finds is suspect;
    with `oracle` (every gate's comparison), so must the reference. A candidate's `producer` is its own business.
    """
    producer = header.get("producer")
    if not isinstance(producer, dict) or producer.get("name") != C.ORACLE_PRODUCER:
        if oracle:
            raise CompareError(f"the reference's producer is {short(producer)}, not the oracle's ({C.ORACLE_PRODUCER})")
        return
    if producer.get("boost") is not True or producer.get("unitypy") != C.ORACLE_UNITYPY:
        raise CompareError(
            f"the reference was dumped with UnityPy {producer.get('unitypy')} and boost={producer.get('boost')}; "
            f"the oracle is UnityPy {C.ORACLE_UNITYPY} with UnityPyBoost"
        )
    if producer.get("boost_sha256") != C.ORACLE_BOOST_SHA256:
        raise CompareError(
            f"the reference was dumped with the UnityPyBoost binary {producer.get('boost_sha256')}; "
            f"the oracle's is {C.ORACLE_BOOST_SHA256}"
        )
    packages = producer.get("packages")
    pins = C.oracle_pins()
    if not isinstance(packages, dict) or packages != pins:
        problems = C.stack_problems(packages, pins, exact=True) if isinstance(packages, dict) else []
        raise CompareError(
            f"the reference was dumped on another stack than {C.ORACLE_REQUIREMENTS.name}: "
            f"{'; '.join(problems) or f'packages {short(packages)}'}"
        )
    # The record digests say the dumper hashed every pinned wheel's files (oracle_env.py).
    records, pinned = producer.get("records"), C.oracle_records()
    if records != pinned:
        if isinstance(records, dict):
            names = sorted(n for n in pinned.keys() | records.keys() if records.get(n) != pinned.get(n))
            what = f"the record digests of {', '.join(names)} are not the pinned ones"
        else:
            what = f"records {short(records)}"
        raise CompareError(
            f"the reference was dumped from other files than the pinned wheels' ({C.ORACLE_RECORDS.name}): {what}"
        )


def select_levels(held_a, held_b, levels):
    """(levels compared record by record, levels the candidate lacks)."""
    if levels is None:
        wanted = list(held_a)
    else:
        unknown = [lv for lv in levels if lv not in C.LEVELS]
        if unknown:
            raise CompareError(
                f"unknown level(s) {', '.join(map(repr, unknown))}; the levels are {', '.join(C.LEVELS)}"
            )
        wanted = [lv for lv in C.LEVELS if lv in levels]
        absent = [lv for lv in wanted if lv not in held_a]
        if absent:
            raise CompareError(f"the reference holds no {' '.join(absent)}")
    if not wanted:
        raise CompareError("nothing to compare: no levels given")
    return [lv for lv in wanted if lv in held_b], [lv for lv in wanted if lv not in held_b]


def run(path_a, path_b, levels=None, examples=3, oracle=False):
    """Compare two dumps; returns (report, header_a, header_b, levels compared); `oracle`: check_reference_producer.

    An integer of any width a record holds is read, compared and quoted (canonical_json.exact_ints).
    """
    with C.exact_ints():
        return _run(path_a, path_b, levels, examples, oracle)


def _run(path_a, path_b, levels, examples, oracle):
    a, b = Side("reference", path_a), Side("candidate", path_b)
    report = Report(examples)
    header_problems = (a.read_header(), b.read_header())
    check_reference_producer(a.header, oracle)
    compared, missing = select_levels(a.levels, b.levels, levels)
    report.missing_levels = missing
    for side, problems in zip((a, b), header_problems, strict=True):
        report.add([Diff("format", "dump", f"{p} ({side.name})", k, d, None, "(header)") for p, k, d in problems])
    for lv in missing:
        report.add([Diff(lv, "dump", "(header) levels", "missing_level", a.levels, b.levels, "(header)")])

    def line_diffs(problems, side, cls, w):
        return [Diff("format", cls, f"{p} ({side})", k, d, None, w) for p, k, d in problems]

    def pair(la, ra, pa, lb, rb, pb):
        """Two records with one key: la/ra/pa are the reference's line, record (or None) and problems."""
        report.counts["matched"] += 1
        diffs = []
        if la == lb:
            report.counts["identical"] += 1
            if not (pa or pb):
                return
            ra = ra or rb or parse(la)[0]
            rb = ra
        else:
            ra = ra or parse(la)[0]
            rb = rb or parse(lb)[0]
            if compare_records(ra, rb, compared, diffs):
                report.counts["differing"] += 1
            elif not (pa or pb) and compared_text(ra, compared) != compared_text(rb, compared):
                # Equal values in different bytes: one side picked another
                # encoding of the same value.
                diffs.append(Diff("format", record_class(ra), "line", "noncanonical_value", where=where(ra)))
        cls, w = record_class(ra), where(ra)
        diffs += line_diffs(pa, "reference", cls, w) + line_diffs(pb, "candidate", cls, w)
        if diffs:
            report.add(diffs)
            if all(d.level == "format" for d in diffs):
                report.counts["noncanonical"] += 1

    def examine(line, side):
        rec, saw_float = parse(line)
        return rec, record_key(rec), line_problems(rec, line, saw_float, side.levels)

    # Lines wait in a per-side map until the other side yields the same key.
    # Producers that follow the canonical order keep both maps near empty.
    for la, lb in itertools.zip_longest(a.lines, b.lines):
        shared = examine(la, a) if la is not None and la == lb and a.levels == b.levels else None
        fresh = {}
        for side, other, line in ((a, b, la), (b, a, lb)):
            if line is None:
                continue
            rec, key, problems = shared or examine(line, side)
            if side is a:
                problems = [*problems, *reference_problems(rec)]
            kind = rec["rec"]
            side.kinds[kind] += 1
            if side.end is not None:
                problems = [*problems, ("record", "after_end", None)]
            if kind == "end" and side.end is None:
                side.end = rec
            if key in side.seen:
                # A second record with one key: report it, and keep the first
                # one for the comparison.
                report.counts["duplicate"] += 1
                level = {"file": "L0", "object": "L1"}.get(kind, "format")
                cls, w = record_class(rec), where(rec)
                report.add([Diff(level, cls, f"(record, {side.name})", "duplicate", where=w)])
                report.add(line_diffs(problems, side.name, cls, w))
                continue
            side.seen.add(key)
            if kind in ("file", "object"):
                misplaced = side.order.check(rec, key)
                if misplaced:
                    side.misordered = True
                    problems = [*problems, *misplaced]
                side.sequence.append(key)
            if key not in other.pending:
                side.pending[key] = (line, problems)
                fresh[(side.name, key)] = rec
                continue
            oline, oproblems = other.pending.pop(key)
            orec = fresh.get((other.name, key))
            if side is a:
                pair(line, rec, problems, oline, orec, oproblems)
            else:
                pair(oline, orec, oproblems, line, rec, problems)

    # The end record must count what its own dump holds; a mismatch means a
    # truncated or hand-edited stream.
    for side in (a, b):
        if side.end is None:
            report.add([Diff("format", "end", f"(end, {side.name})", "missing_end")])
            continue
        for member, rec_kind in (("files", "file"), ("objects", "object")):
            stated = side.end.get(member)
            if type(stated) is not int or stated != side.kinds[rec_kind]:
                report.add(
                    [Diff("format", "end", f"end.{member} ({side.name})", "end_count", stated, side.kinds[rec_kind])]
                )

    # Each dump can keep the spec's order and still differ from the other where
    # the order rests on what the dump left out (a directory below L1).
    if not (a.misordered or b.misordered):
        first = compare_orders(a, b)
        if first:
            report.add([Diff("format", "dump", "(order)", "order_differs", first[0], first[1], "(dump)")])

    for side, kind in ((a, "missing"), (b, "extra")):
        for key, (line, problems) in side.pending.items():
            rec = parse(line)[0]
            report.counts[kind] += 1
            level = {"file": "L0", "object": "L1"}.get(key[0], "format")
            cls, w = record_class(rec), where(rec)
            report.add([Diff(level, cls, "(record)", kind, where=w), *line_diffs(problems, side.name, cls, w)])
    return report, a.header, b.header, compared


def _printable(producer):
    """A header's producer as JSON, or the start of it where it nests past Python's recursion limit."""
    try:
        return json.dumps(producer)
    except RecursionError:
        return short(producer)


def print_report(report, ha, hb, compared, path_a, path_b, max_paths, out=None):
    out = out or sys.stdout
    p = lambda *a: print(*a, file=out)  # noqa: E731
    p(f"reference: {path_a}  producer={_printable(ha.get('producer'))}")
    p(f"candidate: {path_b}  producer={_printable(hb.get('producer'))}")
    missing = f"  (the candidate lacks {' '.join(report.missing_levels)})" if report.missing_levels else ""
    p(f"levels compared: {' '.join(compared) or '(none)'}{missing}")
    c = report.counts
    p(
        f"records: {c['matched']} matched ({c['identical']} identical lines), {c['differing']} differing, "
        f"{c['missing']} missing from candidate, {c['extra']} extra in candidate, {c['duplicate']} duplicate, "
        f"{c['noncanonical']} noncanonical"
    )
    if not report.has_diffs():
        p("no differences")
        return
    d = report.as_dict()
    p("\nby level:")
    for lv, v in d["by_level"].items():
        p(f"  {lv:7} {v['records']:8} records {v['fields']:9} fields")
    p("\nby level and class:")
    for row in d["by_class"]:
        p(f"  {row['level']:7} {row['class']:40} {row['records']:8} records")
    p(f"\nby field path (top {max_paths}):")
    for row in d["by_path"][:max_paths]:
        counts = f"{row['records']} records, {row['fields']} fields"
        p(f"  {row['level']:7} {row['class']:24} {row['path']}  [{row['kind']}]  {counts}")
        for ex in row["examples"]:
            p(f"      e.g. {ex['where']}  {ex['path']}: A={ex['a']}  B={ex['b']}")
    if len(d["by_path"]) > max_paths:
        p(f"  ... {len(d['by_path']) - max_paths} more paths (see --json)")


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("reference", help="reference dump (normally UnityPy's)")
    ap.add_argument("candidate", help="candidate dump (normally unityrs')")
    ap.add_argument("--levels", help="compare only these levels (comma list, e.g. L0,L1,L2); default: the reference's")
    ap.add_argument("--examples", type=int, default=3, help="examples kept per field path (default 3)")
    ap.add_argument("--max-paths", type=int, default=50, help="field paths printed (default 50)")
    ap.add_argument("--json", help="also write the full report as JSON to this path")
    ap.add_argument("--oracle", action="store_true", help="the reference must be the oracle's dump (every gate's)")
    args = ap.parse_args(argv)
    levels = args.levels.split(",") if args.levels is not None else None
    # The report quotes input names from both dumps, so it stays out of the repository.
    root = args.json and C.inside_repo(args.json)
    if root:
        print(
            f"compare: refusing to write --json into the repository ({root}); pick a path outside it", file=sys.stderr
        )
        return 2
    try:
        report, ha, hb, compared = run(args.reference, args.candidate, levels, args.examples, args.oracle)
    except (C.FormatError, CompareError, OSError) as e:
        print(f"compare: {e}", file=sys.stderr)
        return 2
    with C.exact_ints():
        print_report(report, ha, hb, compared, args.reference, args.candidate, args.max_paths)
    if args.json:
        with open(args.json, "w") as fh:
            json.dump(
                {"reference": args.reference, "candidate": args.candidate, "levels": compared, **report.as_dict()},
                fh,
                indent=1,
            )
    return 1 if report.has_diffs() else 0


if __name__ == "__main__":
    sys.exit(main())
