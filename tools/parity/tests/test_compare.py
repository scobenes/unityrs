"""compare.py on hand-built dumps: what it reports, at which level, and what it ignores."""

import copy
import gzip
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import canonical as C  # noqa: E402
import compare  # noqa: E402

LEVELS = ["L0", "L1", "L2", "L3"]


def header(levels=LEVELS, version=C.VERSION):
    return {"rec": "dump", "format": C.FORMAT, "version": version, "levels": levels, "producer": {"name": "test"}}


def file_rec(name="a.bundle"):
    return {
        "rec": "file",
        "file": [name],
        "l0": {"type": "AssetsFile", "parsed_as": "SerializedFile", "size": 10, "sha256": "00" * 32},
        "l1": {"kind": "serialized", "header": {"version": 22, "endian": "<"}},
        "error": None,
    }


def obj_rec(path_id, value, name="a.bundle", root="Transform"):
    return {
        "rec": "object",
        "file": [name],
        "path_id": path_id,
        "l1": {"type_id": 0, "class_id": 4, "byte_start": 0, "byte_size": 8, "is_destroyed": None, "is_stripped": None},
        "l2": {"root_type": root, "source": "embedded", "error": None, "value": value},
        "l3": {
            "raw_size": 8,
            "raw_sha256": "11" * 32,
            "resave": "equal",
            "resave_size": None,
            "resave_sha256": None,
            "first_diff": None,
        },
    }


def f32(bits):
    return {"t": "f32", "v": f"{bits:08x}"}


def base_records():
    v = {"t": "dict", "v": [["x", f32(0x3F800000)], ["list", [1, 2]], ["fs", {"t": "f32s", "v": "3f80000040000000"}]]}
    return [file_rec(), obj_rec(-5, v), obj_rec(7, copy.deepcopy(v)), {"rec": "end", "files": 1, "objects": 2}]


class CompareTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def write(self, name, records, head=None, lines=None):
        path = Path(self.tmp.name) / name
        body = lines if lines is not None else [C.dumps(r) for r in records]
        text = "\n".join([C.dumps(head or header()), *body]) + "\n"
        if name.endswith(".gz"):
            with gzip.open(path, "wb") as fh:
                fh.write(text.encode())
        else:
            path.write_text(text, encoding="utf-8")
        return str(path)

    def run_compare(self, a, b, **kw):
        report, *_ = compare.run(a, b, **kw)
        return report, report.as_dict()

    def paths(self, d):
        return {(r["level"], r["path"], r["kind"]) for r in d["by_path"]}

    def test_identical_dumps_agree_without_parsing(self):
        recs = base_records()
        report, d = self.run_compare(self.write("a.jsonl", recs), self.write("b.jsonl.gz", recs))
        self.assertFalse(report.has_diffs())
        self.assertEqual(report.counts["identical"], 4)

    def test_value_type_and_order_differences(self):
        a, b = base_records(), base_records()
        v = b[1]["l2"]["value"]["v"]
        v[0][1] = f32(0x3F800001)  # value
        v[1][1] = {"t": "tuple", "v": [1, 2]}  # list became tuple
        b[2]["l2"]["value"]["v"].reverse()  # key order
        _, d = self.run_compare(self.write("a.jsonl", a), self.write("b.jsonl", b))
        self.assertEqual(
            self.paths(d),
            {("L2", "l2.value.x", "value"), ("L2", "l2.value.list", "type"), ("L2", "l2.value", "key_order")},
        )
        self.assertEqual(d["by_level"]["L2"]["records"], 2)
        ex = next(r for r in d["by_path"] if r["kind"] == "value")["examples"][0]
        self.assertEqual((ex["a"], ex["b"]), ("f32:3f800000", "f32:3f800001"))

    def test_kinds_never_compare_equal_across_types(self):
        a, b = base_records(), base_records()
        a[1]["l2"]["value"]["v"].append(["flag", 1])
        b[1]["l2"]["value"]["v"].append(["flag", True])
        a[2]["l2"]["value"]["v"][0][1] = {"t": "f64", "v": "3ff0000000000000"}
        _, d = self.run_compare(self.write("a.jsonl", a), self.write("b.jsonl", b))
        self.assertEqual(self.paths(d), {("L2", "l2.value.flag", "type"), ("L2", "l2.value.x", "type")})

    def test_quieted_snan_is_labelled(self):
        a, b = base_records(), base_records()
        a[1]["l2"]["value"]["v"][0][1] = f32(0x7FC00001)  # what UnityPy's dict holds
        b[1]["l2"]["value"]["v"][0][1] = f32(0x7F800001)  # what the file holds
        _, d = self.run_compare(self.write("a.jsonl", a), self.write("b.jsonl", b))
        self.assertEqual(self.paths(d), {("L2", "l2.value.x", "f32_snan_quieted")})

    def test_missing_extra_and_reordered_records(self):
        a, b = base_records(), base_records()
        b[1], b[2] = b[2], b[1]  # reordered: still matched by key
        del b[0]  # file record missing
        b.insert(1, obj_rec(99, {"t": "dict", "v": []}))
        b[-1]["objects"] = 3
        report, d = self.run_compare(self.write("a.jsonl", a), self.write("b.jsonl", b))
        self.assertEqual(report.counts["missing"], 1)
        self.assertEqual(report.counts["extra"], 1)
        self.assertEqual(
            self.paths(d),
            {
                ("L0", "(record)", "missing"),
                ("L1", "(record)", "extra"),
                ("format", "end.objects", "value"),
                ("format", "end.files (candidate)", "end_count"),
            },
        )

    def test_noncanonical_lines_are_format_findings(self):
        a = base_records()
        lines = [C.dumps(r) for r in a]
        lines[1] = json.dumps(a[1], ensure_ascii=False, separators=(", ", ": "))  # whitespace
        b2 = copy.deepcopy(a[2])
        b2["l2"]["value"]["v"][2][1] = [f32(0x3F800000), f32(0x40000000)]  # unpacked f32s
        lines[2] = C.dumps(b2)
        report, d = self.run_compare(self.write("a.jsonl", a), self.write("b.jsonl", [], lines=lines))
        self.assertEqual(
            self.paths(d),
            {("format", "(line, candidate)", "noncanonical_json"), ("format", "(line)", "noncanonical_value")},
        )
        self.assertEqual(report.counts["noncanonical"], 2)
        self.assertEqual(report.counts["differing"], 0)

    def test_notes_and_producer_are_ignored(self):
        a, b = base_records(), base_records()
        a[1]["l2"]["error"] = {"kind": "check_read", "bytes_read": 4, "note": "ValueError: Expected ..."}
        b[1]["l2"]["error"] = {"kind": "check_read", "bytes_read": 4, "note": "rust: short read"}
        hb = header()
        hb["producer"] = {"name": "unityrs"}
        report, _ = self.run_compare(self.write("a.jsonl", a), self.write("b.jsonl", b, head=hb))
        self.assertFalse(report.has_diffs())

    def test_only_shared_levels_are_compared(self):
        a, b = base_records(), base_records()
        for r in b[1:3]:
            r["l3"] = None
        report, _, _, compared = compare.run(
            self.write("a.jsonl", a), self.write("b.jsonl", b, head=header(LEVELS[:3]))
        )
        self.assertEqual(compared, ["L0", "L1", "L2"])
        self.assertFalse(report.has_diffs())
        report, _, _, compared = compare.run(self.write("a.jsonl", a), self.write("c.jsonl", b), levels=["L0", "L1"])
        self.assertEqual(compared, ["L0", "L1"])

    def test_classes_without_l2_fall_back_to_class_id(self):
        a, b = base_records(), base_records()
        for recs in (a, b):
            for r in recs[1:3]:
                r["l2"] = r["l3"] = None
        b[1]["l1"]["byte_size"] = 9
        hl = header(["L0", "L1"])
        _, d = self.run_compare(self.write("a.jsonl", a, head=hl), self.write("b.jsonl", b, head=hl))
        self.assertEqual([(r["class"], r["path"]) for r in d["by_path"]], [("class_id=4", "l1.byte_size")])

    def test_payload_summary_equals_inline(self):
        raw = bytes(range(70))
        a, b = base_records(), base_records()
        a[1]["l2"]["value"]["v"].append(["blob", C.enc_raw("bytes", raw)])
        b[1]["l2"]["value"]["v"].append(["blob", C.enc_raw("bytes", raw[:-1] + b"\0")])
        _, d = self.run_compare(self.write("a.jsonl", a), self.write("b.jsonl", b))
        self.assertEqual(self.paths(d), {("L2", "l2.value.blob", "value")})

    def test_truncated_dump_is_a_format_finding(self):
        recs = base_records()
        _, d = self.run_compare(self.write("a.jsonl", recs), self.write("b.jsonl", recs[:-1]))
        self.assertIn(("format", "(end, candidate)", "missing_end"), self.paths(d))

    def test_bad_inputs(self):
        recs = base_records()
        with self.assertRaises(C.FormatError):
            compare.run(self.write("a.jsonl", recs), self.write("b.jsonl", recs, head=header(version=99)))
        with self.assertRaises(C.FormatError):
            compare.run(self.write("a.jsonl", recs), self.write("b.jsonl", [], lines=["not json"]))

    def test_cli_exit_codes_and_report(self):
        a, b = base_records(), base_records()
        b[1]["l3"]["resave"] = "differs"
        pa, pb = self.write("a.jsonl", a), self.write("b.jsonl", b)
        out = io.StringIO()
        stdout, sys.stdout = sys.stdout, out
        try:
            self.assertEqual(compare.main([pa, pa]), 0)
            self.assertEqual(compare.main([pa, pb, "--json", str(Path(self.tmp.name) / "r.json")]), 1)
        finally:
            sys.stdout = stdout
        self.assertIn("L3      Transform", out.getvalue())
        report = json.loads((Path(self.tmp.name) / "r.json").read_text())
        self.assertEqual(report["by_path"][0]["path"], "l3.resave")


if __name__ == "__main__":
    unittest.main()
