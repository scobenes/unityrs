#!/usr/bin/env python3
"""Diff two canonical dumps and summarise the differences by level, class and field path.

    python tools/parity/compare.py REFERENCE.jsonl[.gz] CANDIDATE.jsonl[.gz] [--json report.json]

REFERENCE is normally the UnityPy dump and CANDIDATE the unityrs one. Records
are matched by key (file path, then path_id), so a candidate that orders
records differently still compares field by field. Byte-identical lines are
skipped unparsed. Fields named "note" are free text and never compared.

Exit status: 0 when the compared levels agree, 1 when they differ, 2 when a
dump is unreadable or not in the canonical format.
"""

from __future__ import annotations

import argparse
import gzip
import itertools
import json
import os
import re
import sys
from collections import Counter, defaultdict

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import canonical as C  # noqa: E402

MAX_DIFFS_PER_RECORD = 20
MAX_ELEMENT_DIFFS = 5
_INDEX = re.compile(r"\[\d+\]")


class Diff:
    __slots__ = ("level", "cls", "path", "kind", "a", "b", "where")

    def __init__(self, level, cls, path, kind, a=None, b=None, where=""):
        self.level, self.cls, self.path, self.kind = level, cls, path, kind
        self.a, self.b, self.where = a, b, where

    def as_dict(self):
        return {
            "level": self.level,
            "class": self.cls,
            "path": self.path,
            "kind": self.kind,
            "where": self.where,
            "a": short(self.a),
            "b": short(self.b),
        }


def short(v, limit=160):
    if v is None:
        return None
    text = repr(v) if not isinstance(v, str) else json.dumps(v, ensure_ascii=False)
    return text if len(text) <= limit else text[: limit - 3] + "..."


# ------------------------------------------------------------------- reading


def open_lines(path):
    opener = gzip.open if path.endswith(".gz") else open
    with opener(path, "rb") as fh:
        yield from fh


def parse(line: bytes):
    try:
        rec = json.loads(line)
    except ValueError as e:
        raise C.FormatError(f"not JSON: {e}") from e
    if not isinstance(rec, dict) or "rec" not in rec:
        raise C.FormatError("a line is not a record object")
    return rec


def file_key(components):
    return tuple(c if isinstance(c, str) else json.dumps(c, sort_keys=True) for c in components)


def record_key(rec):
    kind = rec["rec"]
    if kind == "file":
        return ("file", file_key(rec["file"]))
    if kind == "object":
        return ("object", file_key(rec["file"]), rec["path_id"])
    return (kind,)


_DECODER = json.JSONDecoder()
_REC_KIND = re.compile(rb'\{"rec":"(\w+)"')
_PATH_ID = re.compile(r',"path_id":(-?\d+)[,}]')


def line_key(line: bytes):
    """A record's key read from its canonical prefix, without parsing the line.

    Returns None when the line does not start the way the spec orders members;
    the caller then parses it in full.
    """
    head = line[:4096].decode("utf-8", "replace")
    if head.startswith(('{"rec":"object","file":', '{"rec":"file","file":')):
        start = head.index('"file":') + 7
        try:
            components, end = _DECODER.raw_decode(head, start)
        except ValueError:
            return None
        if head[8] == "f":
            return ("file", file_key(components))
        m = _PATH_ID.match(head, end)
        return ("object", file_key(components), int(m.group(1))) if m else None
    if head.startswith('{"rec":"end"'):
        return ("end",)
    return None


def where(rec):
    parts = [p if isinstance(p, str) else json.dumps(p) for p in rec.get("file", [])]
    w = ":".join(parts)
    if rec["rec"] == "object":
        w += f":path_id={rec['path_id']}"
    return w or rec["rec"]


def record_class(rec):
    if rec["rec"] == "object":
        l2 = rec.get("l2") or {}
        rt = l2.get("root_type")
        if isinstance(rt, str):
            return rt
        return f"class_id={(rec.get('l1') or {}).get('class_id')}"
    if rec["rec"] == "file":
        return "file:" + str((rec.get("l0") or {}).get("parsed_as"))
    return rec["rec"]


# ------------------------------------------------------------------- diffing


def strip_notes(v):
    if isinstance(v, dict):
        return {k: strip_notes(x) for k, x in v.items() if k != "note"}
    if isinstance(v, list):
        return [strip_notes(x) for x in v]
    return v


def json_diff(a, b, path, out):
    """Plain JSON structures (metadata): exact, with paths."""
    if type(a) is not type(b):
        out.append((path, "type", a, b))
    elif isinstance(a, dict):
        for k in [*a, *(k for k in b if k not in a)]:
            if k not in b:
                out.append((f"{path}.{k}", "missing", a[k], None))
            elif k not in a:
                out.append((f"{path}.{k}", "extra", None, b[k]))
            else:
                json_diff(a[k], b[k], f"{path}.{k}", out)
        if list(a) != list(b) and a.keys() == b.keys():
            out.append((path, "key_order", list(a), list(b)))
    elif isinstance(a, list):
        if len(a) != len(b):
            out.append((path, "length", len(a), len(b)))
        shown = 0
        for i, (x, y) in enumerate(zip(a, b, strict=False)):
            if x != y or type(x) is not type(y):
                if shown == MAX_ELEMENT_DIFFS:
                    out.append((f"{path}[...]", "more", None, None))
                    break
                shown += 1
                json_diff(x, y, f"{path}[{i}]", out)
    elif a != b:
        out.append((path, "value", a, b))


def _key_text(k):
    return k if isinstance(k, str) else repr(k)


def value_diff(a, b, path, out):
    """Decoded typetree values (see canonical.decode): type-exact, with paths."""
    ta, tb = type(a), type(b)
    if ta is not tb or (ta is C.Blob and a.kind != b.kind):
        out.append((path, "type", a, b))
        return
    if ta is C.Dict:
        ka, kb = [k for k, _ in a], [k for k, _ in b]
        da, db = dict(a), dict(b)
        if ka != kb:
            if set(ka) == set(kb):
                out.append((path, "key_order", ka, kb))
            for k in ka:
                if k not in db:
                    out.append((f"{path}.{_key_text(k)}", "missing_key", da[k], None))
            for k in kb:
                if k not in da:
                    out.append((f"{path}.{_key_text(k)}", "extra_key", None, db[k]))
        for k in ka:
            if k in db:
                value_diff(da[k], db[k], f"{path}.{_key_text(k)}", out)
    elif ta is list or ta is C.Tuple:
        if len(a) != len(b):
            out.append((path, "length", len(a), len(b)))
        shown = 0
        for i, (x, y) in enumerate(zip(a, b, strict=False)):
            if type(x) is not type(y) or x != y:
                if shown == MAX_ELEMENT_DIFFS:
                    out.append((f"{path}[...]", "more", None, None))
                    break
                shown += 1
                value_diff(x, y, f"{path}[{i}]", out)
    elif a != b:
        kind = "value"
        # CPython widens a float32 through the FPU, which quiets a signalling
        # NaN; UnityPy's dict cannot hold the file's bits (spec: "Floats").
        if ta is C.F32 and (a ^ b) == 0x00400000 and (a & 0x7F800000) == 0x7F800000 and a & 0x00400000:
            kind = "f32_snan_quieted"
        out.append((path, kind, a, b))


def compare_section(level, cls, name, a, b, w, out, value=False):
    raw = []
    if value:
        if (a is None) != (b is None):
            raw.append((name, "type", a, b))
        elif a is not None:
            try:
                da, db = C.decode(a), C.decode(b)
            except C.FormatError as e:
                raw.append((name, "invalid", str(e), None))
            else:
                value_diff(da, db, name, raw)
    else:
        json_diff(strip_notes(a), strip_notes(b), name, raw)
    for path, kind, x, y in raw:
        out.append(Diff(level, cls, path, kind, x, y, w))


def compare_records(ra, rb, levels, out):
    w, cls = where(ra), record_class(ra)
    diffs = []
    if ra["rec"] == "file":
        if "L0" in levels:
            compare_section("L0", cls, "l0", ra.get("l0"), rb.get("l0"), w, diffs)
        if "L1" in levels:
            compare_section("L1", cls, "l1", ra.get("l1"), rb.get("l1"), w, diffs)
        compare_section("L1", cls, "error", ra.get("error"), rb.get("error"), w, diffs)
    elif ra["rec"] == "object":
        if "L1" in levels:
            compare_section("L1", cls, "l1", ra.get("l1"), rb.get("l1"), w, diffs)
        if "L2" in levels:
            l2a, l2b = ra.get("l2") or {}, rb.get("l2") or {}
            meta_a = {k: v for k, v in l2a.items() if k != "value"}
            meta_b = {k: v for k, v in l2b.items() if k != "value"}
            compare_section("L2", cls, "l2", meta_a, meta_b, w, diffs)
            compare_section("L2", cls, "l2.value", l2a.get("value"), l2b.get("value"), w, diffs, value=True)
        if "L3" in levels:
            compare_section("L3", cls, "l3", ra.get("l3"), rb.get("l3"), w, diffs)
    elif ra["rec"] == "end":
        compare_section("format", cls, "end", ra, rb, w, diffs)
    if len(diffs) > MAX_DIFFS_PER_RECORD:
        extra = len(diffs) - MAX_DIFFS_PER_RECORD
        diffs = diffs[:MAX_DIFFS_PER_RECORD]
        diffs.append(Diff(diffs[0].level, cls, "(record)", "more", extra, None, w))
    out.extend(diffs)
    return bool(diffs)


# -------------------------------------------------------------------- driver


class Report:
    def __init__(self, examples):
        self.examples = examples
        self.counts = Counter()
        self.diffs_by_level = Counter()
        self.records_by_level = defaultdict(set)
        self.records_by_class = defaultdict(set)
        self.paths = defaultdict(lambda: {"records": set(), "fields": 0, "examples": []})

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


def run(path_a, path_b, levels=None, examples=3):
    """Compare two dumps; returns (report, header_a, header_b, levels compared)."""
    lines_a, lines_b = open_lines(path_a), open_lines(path_b)
    try:
        ha, hb = parse(next(lines_a)), parse(next(lines_b))
    except StopIteration as e:
        raise C.FormatError("empty dump") from e
    for name, h in (("reference", ha), ("candidate", hb)):
        if h.get("rec") != "dump" or h.get("format") != C.FORMAT:
            raise C.FormatError(f"{name} does not start with a {C.FORMAT} header")
        if h.get("version") != C.VERSION:
            raise C.FormatError(f"{name} is format version {h.get('version')}, this tool reads {C.VERSION}")
    compared = [lv for lv in C.LEVELS if lv in ha.get("levels", []) and lv in hb.get("levels", [])]
    if levels:
        compared = [lv for lv in compared if lv in levels]

    report = Report(examples)
    pending_a, pending_b = {}, {}
    diffs = []

    same_levels = compared == ha.get("levels") == hb.get("levels")

    def pair(ra, rb, la, lb):
        report.counts["matched"] += 1
        diffs.clear()
        if compare_records(ra, rb, compared, diffs):
            report.counts["differing"] += 1
        # Equal values in different bytes mean a producer broke a canonical-form
        # rule: either its line is not the spec's JSON serialisation of itself
        # (whitespace, escapes, number spelling), or it picked another encoding
        # of the same value (an unpacked float list, an inline long blob).
        cls, w = record_class(ra), where(ra)
        for side, rec, line in (("reference", ra, la), ("candidate", rb, lb)):
            if (C.dumps(rec) + "\n").encode() != line:
                diffs.append(Diff("format", cls, f"(line, {side})", "noncanonical_json", where=w))
        if list(ra) != list(rb):
            diffs.append(Diff("format", cls, "(members)", "member_order", list(ra), list(rb), w))
        elif not diffs and same_levels and strip_notes(ra) != strip_notes(rb):
            diffs.append(Diff("format", cls, "(line)", "noncanonical_value", where=w))
        if diffs:
            report.add(diffs)
            if all(d.level == "format" for d in diffs):
                report.counts["noncanonical"] += 1

    # Lines wait, unparsed, in a per-side map until the other side yields the
    # same key; equal lines then match without a parse. Producers that follow
    # the canonical order keep both maps near empty.
    seen = {"a": Counter(), "b": Counter()}
    ends = {}
    for la, lb in itertools.zip_longest(lines_a, lines_b):
        for side, line in (("a", la), ("b", lb)):
            if line is not None:
                m = _REC_KIND.match(line)
                kind = m.group(1).decode() if m else parse(line)["rec"]
                seen[side][kind] += 1
                if kind == "end":
                    ends[side] = parse(line)
        if la is not None and la == lb:
            report.counts["matched"] += 1
            report.counts["identical"] += 1
            continue
        for line, mine, other, side in ((la, pending_a, pending_b, "a"), (lb, pending_b, pending_a, "b")):
            if line is None:
                continue
            key = line_key(line)
            rec = None
            if key is None:
                rec = parse(line)
                key = record_key(rec)
            if key not in other:
                mine[key] = line
                continue
            oline = other.pop(key)
            if oline == line:
                report.counts["matched"] += 1
                report.counts["identical"] += 1
                continue
            rec = rec or parse(line)
            orec = parse(oline)
            if side == "a":
                pair(rec, orec, line, oline)
            else:
                pair(orec, rec, oline, line)

    # The end record must count what its own dump holds; a mismatch means a
    # truncated or hand-edited stream.
    for side, name in (("a", "reference"), ("b", "candidate")):
        end = ends.get(side)
        if end is None:
            report.add([Diff("format", "end", f"(end, {name})", "missing_end")])
            continue
        for member, rec_kind in (("files", "file"), ("objects", "object")):
            if end.get(member) != seen[side][rec_kind]:
                report.add(
                    [
                        Diff(
                            "format",
                            "end",
                            f"end.{member} ({name})",
                            "end_count",
                            end.get(member),
                            seen[side][rec_kind],
                        )
                    ]
                )

    for pending, kind in ((pending_a, "missing"), (pending_b, "extra")):
        for key, line in pending.items():
            rec = parse(line)
            report.counts[kind] += 1
            level = "L0" if key[0] == "file" else "L1" if key[0] == "object" else "format"
            report.add([Diff(level, record_class(rec), "(record)", kind, where=where(rec))])
    return report, ha, hb, compared


def print_report(report, ha, hb, compared, path_a, path_b, max_paths, out=None):
    out = out or sys.stdout
    p = lambda *a: print(*a, file=out)  # noqa: E731
    p(f"reference: {path_a}  producer={json.dumps(ha.get('producer'))}")
    p(f"candidate: {path_b}  producer={json.dumps(hb.get('producer'))}")
    p(f"levels compared: {' '.join(compared) or '(none)'}")
    c = report.counts
    p(
        f"records: {c['matched']} matched ({c['identical']} identical lines), {c['differing']} differing, "
        f"{c['missing']} missing from candidate, {c['extra']} extra in candidate, {c['noncanonical']} noncanonical"
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
    ap.add_argument("--levels", help="restrict to these levels (comma list, e.g. L0,L1,L2)")
    ap.add_argument("--examples", type=int, default=3, help="examples kept per field path (default 3)")
    ap.add_argument("--max-paths", type=int, default=50, help="field paths printed (default 50)")
    ap.add_argument("--json", help="also write the full report as JSON to this path")
    args = ap.parse_args(argv)
    try:
        report, ha, hb, compared = run(
            args.reference, args.candidate, args.levels.split(",") if args.levels else None, args.examples
        )
    except (C.FormatError, OSError) as e:
        print(f"compare: {e}", file=sys.stderr)
        return 2
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
