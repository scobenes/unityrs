"""compare.py on hand-built dumps: what it reports, at which level, and what it ignores."""

import contextlib
import copy
import gzip
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import canonical as C  # noqa: E402
import compare  # noqa: E402

LEVELS = ["L0", "L1", "L2", "L3"]
SIDES = ("reference", "candidate")


def header(levels=LEVELS, version=C.VERSION, producer=None):
    return {
        "rec": "dump",
        "format": C.FORMAT,
        "version": version,
        "levels": levels,
        "producer": producer or {"name": "test"},
    }


def type_rec():
    return {
        "class_id": 4,
        "is_stripped_type": False,
        "script_type_index": -1,
        "script_id": None,
        "old_type_hash": "00" * 16,
        "nodes": [[0, "Transform", "Base", -1, 1, 0, 0, 0, 0, None]],
        "type_dependencies": None,
        "class_name": None,
        "namespace": None,
        "assembly": None,
    }


def file_rec(name="a.bundle"):
    return {
        "rec": "file",
        "file": [name],
        "l0": {"type": "AssetsFile", "parsed_as": "SerializedFile", "size": 10, "sha256": "00" * 32},
        "l1": {
            "kind": "serialized",
            "header": {
                "metadata_size": 1,
                "file_size": 10,
                "version": 22,
                "data_offset": 2,
                "endian": "<",
                "reserved": "000000",
                "unknown": 0,
            },
            "unity_version": "2021.3.2f1",
            "version_packed": 0x7E5000300020301,
            "target_platform": 19,
            "enable_type_tree": True,
            "big_id_enabled": 0,
            "types": [type_rec()],
            "object_order": [-5, 7],
            "script_types": [],
            "externals": [],
            "ref_types": [],
            "user_information": "",
            "container": [],
        },
        "error": None,
    }


def bundle_rec(name="b.bundle"):
    return {
        "rec": "file",
        "file": [name],
        "l0": {"type": "BundleFile", "parsed_as": "BundleFile", "size": 10, "sha256": "11" * 32},
        "l1": {
            "kind": "bundle",
            "signature": "UnityFS",
            "format": 6,
            "version_player": "5.x.x",
            "version_engine": "2018.4.11f1",
            "engine_version_packed": 0x7E20004000B0301,
            "flags": 67,
            "flags_class": "ArchiveFlagsOld",
            "block_alignment": False,
            "encrypted": False,
            "hash": None,
            "crc": None,
            "blocks_info": [153, 88, 67],
            "blocks": [[93300, 76633, 3]],
            "directory": [[0, 10, 4, "CAB-x"]],
        },
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


def S(*items):
    """A typetree struct: a dict value from (name, value) pairs."""
    return {"t": "dict", "v": [list(i) for i in items]}


def end(files, objects):
    return {"rec": "end", "files": files, "objects": objects}


def base_records():
    v = S(["x", f32(0x3F800000)], ["list", [1, 2]], ["fs", {"t": "f32s", "v": "3f80000040000000"}])
    return [file_rec(), obj_rec(-5, v), obj_rec(7, copy.deepcopy(v)), end(1, 2)]


def one_object(value):
    return [file_rec(), obj_rec(1, value), end(1, 1)]


class CompareTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def write(self, name, records, head=None, lines=None, raw=None):
        path = Path(self.tmp.name) / name
        if raw is None:
            body = lines if lines is not None else [C.dumps(r) for r in records]
            raw = ("\n".join([C.dumps(head or header()), *body]) + "\n").encode()
        if name.endswith(".gz"):
            raw = gzip.compress(raw, mtime=0)
        path.write_bytes(raw)
        return str(path)

    def run_compare(self, a, b, **kw):
        report, *_ = compare.run(a, b, **kw)
        return report, report.as_dict()

    def paths(self, d):
        return {(r["level"], r["path"], r["kind"]) for r in d["by_path"]}

    def cli(self, *argv):
        """compare.main's exit status; its stdout and stderr land in self.output and self.errors."""
        with contextlib.redirect_stdout(io.StringIO()) as out, contextlib.redirect_stderr(io.StringIO()) as err:
            rc = compare.main([*map(str, argv)])
        self.output, self.errors = out.getvalue(), err.getvalue()
        return rc

    def test_fixtures_are_canonical(self):
        for rec in [*base_records(), bundle_rec(), header()]:
            self.assertEqual(C.check_record(rec, LEVELS), [], rec["rec"])

    def test_identical_dumps_agree(self):
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

    def test_a_longer_list_with_an_equal_prefix_is_a_length_difference(self):
        a, b = base_records(), base_records()
        b[1]["l2"]["value"]["v"][1][1] = [1, 2, 3]
        _, d = self.run_compare(self.write("a.jsonl", a), self.write("b.jsonl", b))
        self.assertEqual(self.paths(d), {("L2", "l2.value.list", "length")})

    def test_kinds_never_compare_equal_across_types(self):
        a, b = base_records(), base_records()
        a[1]["l2"]["value"]["v"].append(["flag", 1])
        b[1]["l2"]["value"]["v"].append(["flag", True])
        a[2]["l2"]["value"]["v"][0][1] = {"t": "f64", "v": "3ff0000000000000"}
        _, d = self.run_compare(self.write("a.jsonl", a), self.write("b.jsonl", b))
        self.assertEqual(self.paths(d), {("L2", "l2.value.flag", "type"), ("L2", "l2.value.x", "type")})

    def test_nested_type_mismatches_in_l2(self):
        """Types match exactly at any depth: in a struct inside an array, and in an array of map pairs."""
        cases = {
            "bool vs int": (
                S(["arr", [S(["m_Enabled", True])]]),
                S(["arr", [S(["m_Enabled", 1])]]),
                "l2.value.arr[].m_Enabled",
            ),
            "f32 vs raw int": (
                S(["keys", [S(["time", f32(0)], ["value", f32(0x3F800000)])]]),
                S(["keys", [S(["time", 0], ["value", 1065353216])]]),
                "l2.value.keys[].time",
            ),
            "f32 vs f64 zero": (
                S(["keys", [S(["time", f32(0)])]]),
                S(["keys", [S(["time", {"t": "f64", "v": "0" * 16}])]]),
                "l2.value.keys[].time",
            ),
            "tuple vs list": (
                S(["arr", [S(["m", [{"t": "tuple", "v": ["k", 1]}]])]]),
                S(["arr", [S(["m", [["k", 1]]])]]),
                "l2.value.arr[].m[]",
            ),
            "bool vs int in a pair": (
                S(["m", [{"t": "tuple", "v": ["k", True]}]]),
                S(["m", [{"t": "tuple", "v": ["k", 1]}]]),
                "l2.value.m[][]",
            ),
        }
        for label, (ref, cand, path) in cases.items():
            pa = self.write("a.jsonl", one_object(ref))
            pb = self.write("b.jsonl", one_object(cand))
            for levels in (None, ["L2"]):
                with self.subTest(label, levels=levels):
                    report, d = self.run_compare(pa, pb, levels=levels)
                    self.assertIn(("L2", path, "type"), self.paths(d))
                    self.assertEqual((report.counts["differing"], report.counts["noncanonical"]), (1, 0))
                    self.assertEqual(self.cli(pa, pb, *(["--levels", "L2"] if levels else [])), 1)

    def test_nested_type_mismatches_in_l1(self):
        fa, fb = file_rec(), file_rec()
        fa["l1"]["types"][0]["is_stripped_type"] = True
        fb["l1"]["types"][0]["is_stripped_type"] = 1
        ba, bb = bundle_rec(), bundle_rec()
        ba["l1"]["directory"][0][2] = 1
        bb["l1"]["directory"][0][2] = True
        pa = self.write("a.jsonl", [fa, ba, end(2, 0)])
        pb = self.write("b.jsonl", [fb, bb, end(2, 0)])
        for levels in (None, ["L1"]):
            with self.subTest(levels=levels):
                _, d = self.run_compare(pa, pb, levels=levels)
                self.assertEqual(
                    self.paths(d),
                    {("L1", "l1.types[].is_stripped_type", "type"), ("L1", "l1.directory[][]", "type")},
                )

    def test_json_floats_are_type_differences_and_format_findings(self):
        a, b = base_records(), base_records()
        lines = [C.dumps(r) for r in b]
        lines[0] = lines[0].replace('"size":10', '"size":10.0')
        _, d = self.run_compare(self.write("a.jsonl", a), self.write("b.jsonl", [], lines=lines))
        self.assertEqual(self.paths(d), {("L0", "l0.size", "type"), ("format", "line (candidate)", "json_float")})

    def test_path_id_must_be_an_integer(self):
        recs = one_object(S(["x", 1]))
        lines = [C.dumps(r) for r in recs]
        lines[1] = lines[1].replace('"path_id":1,', '"path_id":1.0,')
        pa, pb = self.write("a.jsonl", recs), self.write("b.jsonl", [], lines=lines)
        with self.assertRaisesRegex(C.FormatError, "path_id"):
            compare.run(pa, pb)
        self.assertEqual(self.cli(pa, pb), 2)
        self.assertIn("path_id", self.errors)
        lines[1] = lines[1].replace('"path_id":1.0,', '"path_id":true,')
        self.assertEqual(self.cli(pa, self.write("c.jsonl", [], lines=lines)), 2)

    def test_quieted_snan_is_labelled(self):
        a, b = base_records(), base_records()
        a[1]["l2"]["value"]["v"][0][1] = f32(0x7FC00001)  # what UnityPy's dict holds
        b[1]["l2"]["value"]["v"][0][1] = f32(0x7F800001)  # what the file holds
        _, d = self.run_compare(self.write("a.jsonl", a), self.write("b.jsonl", b))
        self.assertEqual(self.paths(d), {("L2", "l2.value.x", "f32_snan_quieted")})

    def test_nan_against_infinity_is_a_value_difference(self):
        """A qNaN and an infinity also differ only in the quiet bit; only a signalling NaN is the oracle's loss."""
        for ref, cand in ((0x7FC00000, 0x7F800000), (0xFFC00000, 0xFF800000)):
            with self.subTest(ref=hex(ref)):
                a, b = base_records(), base_records()
                a[1]["l2"]["value"]["v"][0][1] = f32(ref)
                b[1]["l2"]["value"]["v"][0][1] = f32(cand)
                _, d = self.run_compare(self.write("a.jsonl", a), self.write("b.jsonl", b))
                self.assertEqual(self.paths(d), {("L2", "l2.value.x", "value")})

    def test_missing_and_extra_records(self):
        a, b = base_records(), base_records()
        del b[1]  # object -5 missing
        b.insert(2, obj_rec(99, S()))
        report, d = self.run_compare(self.write("a.jsonl", a), self.write("b.jsonl", b))
        self.assertEqual((report.counts["missing"], report.counts["extra"]), (1, 1))
        self.assertEqual(self.paths(d), {("L1", "(record)", "missing"), ("L1", "(record)", "extra")})
        # Without its file record, a file's objects are out of place too.
        del b[0]
        b[-1]["files"] = 0
        report, d = self.run_compare(self.write("a.jsonl", a), self.write("b.jsonl", b))
        self.assertEqual(
            self.paths(d),
            {
                ("L0", "(record)", "missing"),
                ("L1", "(record)", "missing"),
                ("L1", "(record)", "extra"),
                ("format", "record (candidate)", "order"),
                ("format", "end.files", "value"),
            },
        )

    def test_records_out_of_order_are_format_findings(self):
        """Section 5's order is part of the content: a reordered dump is matched by key, and reported."""

        def entry(name):
            return {
                "rec": "file",
                "file": ["b.bundle", name],
                "l0": {"type": "ResourceFile", "parsed_as": "raw", "size": 10, "sha256": "22" * 32},
                "l1": None,
                "error": None,
            }

        bundle = bundle_rec()
        bundle["l1"]["directory"] = [[0, 10, 4, "y"], [10, 10, 4, "x"], [20, 10, 4, "y"]]
        f, o1, o2, e = base_records()
        ref = [f, o1, o2, bundle, entry("y"), entry("x"), end(4, 2)]
        cases = {
            "objects descending": [f, o2, o1, bundle, entry("y"), entry("x"), end(4, 2)],
            "objects before their file": [o1, o2, f, bundle, entry("y"), entry("x"), end(4, 2)],
            "inputs descending": [bundle, entry("y"), entry("x"), f, o1, o2, end(4, 2)],
            "entries against the directory": [f, o1, o2, bundle, entry("x"), entry("y"), end(4, 2)],
            "an entry outside its container": [f, o1, o2, bundle, entry("y"), end(4, 2)],
        }
        cases["an entry outside its container"].insert(1, entry("x"))
        a = self.write("ref.jsonl", ref)
        self.assertEqual(self.cli(a, a), 0, self.output)
        for name, recs in cases.items():
            with self.subTest(name):
                b = self.write("b.jsonl", recs)
                report, d = self.run_compare(a, b)
                self.assertEqual(self.paths(d), {("format", "record (candidate)", "order")})
                self.assertEqual(report.counts["differing"], 0)
                _, d = self.run_compare(b, b)
                self.assertEqual(
                    self.paths(d),
                    {("format", "record (reference)", "order"), ("format", "record (candidate)", "order")},
                )

        # Without L1 no directory is dumped, so only the other dump can show the entries' order.
        def l0_only(recs):
            recs = copy.deepcopy(recs)
            for r in recs:
                if r["rec"] in ("file", "object"):
                    r["l1"] = None
                if r["rec"] == "object":
                    r["l2"] = r["l3"] = None
            return recs

        head = header(["L0"])
        a = self.write("ref0.jsonl", l0_only(ref), head=head)
        b = self.write("b0.jsonl", l0_only(cases["entries against the directory"]), head=head)
        self.assertEqual(self.cli(b, b), 0, self.output)
        _, d = self.run_compare(a, b)
        self.assertEqual(self.paths(d), {("format", "(order)", "order_differs")})
        self.assertEqual(self.cli(a, b), 1)

    def test_duplicate_records_are_reported_and_the_first_is_compared(self):
        """The candidate writes a bad copy, then a good copy, of obj 1 before the reference reaches it."""
        good, bad = S(["x", f32(0x3F800000)]), S(["x", f32(0xDEADBEEF)])
        a = [file_rec(), obj_rec(1, good), obj_rec(2, good), end(1, 2)]
        b = [obj_rec(1, bad), obj_rec(1, good), file_rec(), obj_rec(2, good), end(1, 3)]
        report, d = self.run_compare(self.write("a.jsonl", a), self.write("b.jsonl", b))
        self.assertEqual(report.counts["duplicate"], 1)
        self.assertEqual(
            self.paths(d),
            {
                ("L1", "(record, candidate)", "duplicate"),
                ("L2", "l2.value.x", "value"),
                ("format", "end.objects", "value"),
                ("format", "record (candidate)", "order"),
            },
        )
        # A copy that arrives after its key was matched is a duplicate too, not an extra record.
        b = [file_rec(), obj_rec(1, good), obj_rec(2, good), obj_rec(2, good), end(1, 3)]
        report, _ = self.run_compare(self.write("a.jsonl", a), self.write("c.jsonl", b))
        self.assertEqual((report.counts["duplicate"], report.counts["extra"]), (1, 0))

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
            {("format", "line (candidate)", "noncanonical_json"), ("format", "l2.value.fs (candidate)", "unpacked")},
        )
        self.assertEqual(report.counts["noncanonical"], 2)
        self.assertEqual(report.counts["differing"], 0)

    def test_identical_noncanonical_lines_are_format_findings(self):
        """Both dumps break the format the same way: equal bytes are no excuse, header and end included."""
        recs = base_records()
        recs[2]["l2"]["value"]["v"][2][1] = [f32(0x3F800000)]  # unpacked f32s
        lines = [
            json.dumps(header(), separators=(", ", ":")),
            C.dumps(recs[0]),
            json.dumps(recs[1], ensure_ascii=False, separators=(", ", ": ")),
            C.dumps(recs[2]),
            '{"rec":"end", "files":1,"objects":2}',
        ]
        raw = ("\n".join(lines) + "\n").encode()
        pa, pb = self.write("a.jsonl", [], raw=raw), self.write("b.jsonl.gz", [], raw=raw)
        report, d = self.run_compare(pa, pb)
        self.assertEqual(report.counts["identical"], 4)
        expected = set()
        for side in ("reference", "candidate"):
            expected |= {
                ("format", f"line ({side})", "noncanonical_json"),
                ("format", f"l2.value.fs ({side})", "unpacked"),
            }
        self.assertEqual(self.paths(d), expected)
        self.assertEqual(d["by_level"]["format"]["records"], 4, "the header, two objects and the end record")
        self.assertEqual(self.cli(pa, pb), 1)

    def test_member_order_and_undeclared_levels_are_format_findings(self):
        recs = base_records()
        reordered = {"rec": "object", "path_id": -5, "file": ["a.bundle"], **recs[1]}
        raw = ("\n".join(map(C.dumps, [header(), recs[0], reordered, *recs[2:]])) + "\n").encode()
        _, d = self.run_compare(self.write("a.jsonl", [], raw=raw), self.write("b.jsonl", [], raw=raw))
        self.assertIn(("format", "record (candidate)", "members"), self.paths(d))
        # A dump that declares only L0 and L1 carries no l2 or l3.
        hl = header(["L0", "L1"])
        _, d = self.run_compare(self.write("c.jsonl", base_records(), head=hl), self.write("d.jsonl", base_records()))
        self.assertIn(("format", "l2 (reference)", "undeclared_level"), self.paths(d))

    def test_noncanonical_encodings_are_found_whatever_the_levels(self):
        """An unpacked f32 vector and an inline 70-byte blob are format findings under --levels too."""
        packed = S(["fs", {"t": "f32s", "v": "3f80000040000000"}], ["blob", C.enc_raw("bytes", bytes(70))])
        unpacked = S(["fs", [f32(0x3F800000), f32(0x40000000)]], ["blob", {"t": "bytes", "hex": bytes(70).hex()}])
        pa, pb = self.write("a.jsonl", one_object(packed)), self.write("b.jsonl", one_object(unpacked))
        for levels in (None, ["L2"], ["L0", "L1"]):
            with self.subTest(levels=levels):
                report, d = self.run_compare(pa, pb, levels=levels)
                self.assertEqual(
                    self.paths(d),
                    {
                        ("format", "l2.value.fs (candidate)", "unpacked"),
                        ("format", "l2.value.blob (candidate)", "inline_too_long"),
                    },
                )
                self.assertEqual(report.counts["noncanonical"], 1)

    def test_cross_dump_net_does_not_depend_on_levels(self):
        """Equal values in other bytes are a format finding even where no single-line rule names the cause."""
        packed = S(["fs", {"t": "f32s", "v": "3f80000040000000"}])
        unpacked = S(["fs", [f32(0x3F800000), f32(0x40000000)]])
        pa, pb = self.write("a.jsonl", one_object(packed)), self.write("b.jsonl", one_object(unpacked))
        with mock.patch.object(C, "check_record", return_value=[]):
            for levels in (None, ["L2"], ["L0", "L1", "L2"]):
                with self.subTest(levels=levels):
                    report, d = self.run_compare(pa, pb, levels=levels)
                    self.assertEqual(self.paths(d), {("format", "line", "noncanonical_value")})
                    self.assertEqual(report.counts["noncanonical"], 1)

    def test_notes_and_producer_are_ignored(self):
        a, b = base_records(), base_records()
        a[1]["l2"]["error"] = {"kind": "check_read", "bytes_read": 4, "note": "ValueError: Expected ..."}
        b[1]["l2"]["error"] = {"kind": "check_read", "bytes_read": 4, "note": "rust: short read"}
        a[2]["l3"]["note"] = "TypeError: ..."
        report, _ = self.run_compare(
            self.write("a.jsonl", a), self.write("b.jsonl", b, head=header(producer={"name": "unityrs"}))
        )
        self.assertFalse(report.has_diffs())

    def test_candidate_missing_levels_is_a_difference(self):
        a = base_records()
        for levels in (LEVELS[:3], ["L0"], ["L0", "L2"]):
            with self.subTest(levels=levels):
                b = base_records()
                for r in b[1:3]:
                    for lv, name in (("L1", "l1"), ("L2", "l2"), ("L3", "l3")):
                        if lv not in levels:
                            r[name] = None
                if "L1" not in levels:
                    b[0]["l1"] = None
                pa, pb = self.write("a.jsonl", a), self.write("b.jsonl", b, head=header(levels))
                report, _, _, compared = compare.run(pa, pb)
                self.assertEqual(compared, levels)
                self.assertEqual(report.missing_levels, [lv for lv in LEVELS if lv not in levels])
                self.assertEqual(
                    self.paths(report.as_dict()),
                    {(lv, "(header) levels", "missing_level") for lv in LEVELS if lv not in levels},
                )
                self.assertEqual(self.cli(pa, pb), 1)
                self.assertIn("the candidate lacks", self.output)
        # Narrowed to the levels the candidate holds, the rest agree.
        b = base_records()
        for r in b[1:3]:
            r["l3"] = None
        pa, pb = self.write("a.jsonl", a), self.write("b.jsonl", b, head=header(LEVELS[:3]))
        self.assertEqual(self.cli(pa, pb, "--levels", "L0,L1,L2"), 0)

    def test_levels_narrow_the_comparison(self):
        a, b = base_records(), base_records()
        b[1]["l2"]["value"]["v"][0][1] = f32(0x40000000)
        pa, pb = self.write("a.jsonl", a), self.write("b.jsonl", b)
        report, _, _, compared = compare.run(pa, pb, levels=["L1", "L0"])
        self.assertEqual(compared, ["L0", "L1"])
        self.assertFalse(report.has_diffs())
        self.assertEqual(self.cli(pa, pb, "--levels", "L2"), 1)

    def test_bad_levels_are_refused(self):
        recs = base_records()
        pa, pb = self.write("a.jsonl", recs), self.write("b.jsonl", recs)
        for arg in ("l2", "L2,L9", "", "L0,,L1"):
            with self.subTest(levels=arg):
                self.assertEqual(self.cli(pa, pb, "--levels", arg), 2)
                self.assertIn("unknown level", self.errors)
        no_l3 = [r if r["rec"] != "object" else dict(r, l3=None) for r in recs]
        self.assertEqual(self.cli(self.write("r.jsonl", no_l3, head=header(LEVELS[:3])), pb, "--levels", "L3"), 2)
        self.assertIn("holds no L3", self.errors)
        with self.assertRaisesRegex(compare.CompareError, "nothing to compare"):
            compare.run(pa, pb, levels=[])

    def test_reference_must_come_from_the_pinned_oracle(self):
        recs = base_records()
        pb = self.write("b.jsonl", recs)
        oracle = {"name": C.ORACLE_PRODUCER, "unitypy": C.ORACLE_UNITYPY, "boost": True, "python": "3.11"}
        self.assertEqual(self.cli(self.write("a.jsonl", recs, head=header(producer=oracle)), pb), 0)
        for change in ({"boost": False}, {"unitypy": "1.24.0"}, {"boost": 1}):
            with self.subTest(change):
                pa = self.write("a.jsonl", recs, head=header(producer={**oracle, **change}))
                self.assertEqual(self.cli(pa, pb), 2)
                self.assertIn("the oracle is UnityPy", self.errors)
                self.assertEqual(self.cli(pb, pa), 0, "a candidate's producer is never checked")

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

    def test_null_against_a_value_differs_both_ways(self):
        a, b = base_records(), base_records()
        a[1]["l2"]["value"] = None
        b[2]["l2"]["value"] = None
        _, d = self.run_compare(self.write("a.jsonl", a), self.write("b.jsonl", b))
        # A parse without an error has a value, so each null is also a format finding.
        self.assertEqual(
            self.paths(d),
            {
                ("L2", "l2.value", "type"),
                ("format", "l2.value (reference)", "value_mismatch"),
                ("format", "l2.value (candidate)", "value_mismatch"),
            },
        )
        self.assertEqual(d["by_level"]["L2"]["records"], 2)

    def test_null_sections_under_declared_levels_fail_both_dumps(self):
        """Sections a header declares must be there: two dumps that both leave them null do not agree."""
        recs = base_records()
        recs[0]["l1"] = None
        for r in recs[1:3]:
            r["l1"] = r["l2"] = r["l3"] = None
        path = self.write("a.jsonl", recs)
        report, d = self.run_compare(path, path)
        self.assertEqual(
            self.paths(d),
            {("format", f"{s} ({side})", "missing_section") for s in ("l1", "l2", "l3") for side in SIDES},
        )
        self.assertEqual(self.cli(path, path), 1)
        # Narrowed to L0, the same dumps break no rule: the header declares what each line must hold.
        recs = base_records()
        for r in recs[:3]:
            r["l1"] = None
        for r in recs[1:3]:
            r["l2"] = r["l3"] = None
        path = self.write("l0.jsonl", recs, head=header(["L0"]))
        self.assertEqual(self.cli(path, path), 0, self.output)

    def test_names_break_the_metadata_string_rule_in_both_dumps(self):
        recs = base_records()
        a_bundle = {"t": "str", "hex": b"a.bundle".hex()}
        for r in recs[:3]:
            r["file"] = [a_bundle]
        recs[0]["l1"]["unity_version"] = {"t": "str", "hex": b"2021".hex()}
        recs[0]["l1"]["types"][0]["nodes"][0][1] = {"t": "str", "hex": "5A"}
        recs[1]["l2"]["root_type"] = 5
        path = self.write("a.jsonl", recs)
        _, d = self.run_compare(path, path)
        self.assertEqual(
            self.paths(d),
            {
                (lv, f"{p} ({side})", k)
                for lv, p, k in (
                    ("format", "file[]", "hex_name_is_utf8"),
                    ("format", "l1.unity_version", "hex_name_is_utf8"),
                    ("format", "l1.types[].nodes[][]", "bad_name"),
                    ("format", "l2.root_type", "bad_name"),
                )
                for side in SIDES
            },
        )

    def test_check_record_sections_and_names(self):
        def kinds(rec, levels=LEVELS):
            return sorted((p, k) for p, k, _ in C.check_record(rec, levels))

        good = [file_rec(), bundle_rec(), obj_rec(1, S(["x", 1]))]
        for rec in good:
            self.assertEqual(kinds(rec), [])
        obj = obj_rec(1, S(["x", 1]))
        for level, name in (("L1", "l1"), ("L2", "l2"), ("L3", "l3")):
            rec = copy.deepcopy(obj)
            rec[name] = None
            self.assertEqual(kinds(rec), [(name, "missing_section")])
            self.assertEqual(kinds(rec, [lv for lv in LEVELS if lv != level]), [])
        # l2's value follows its error: none for a failed read, one for check_read.
        rec = copy.deepcopy(obj)
        rec["l2"]["error"] = {"kind": "out_of_bounds", "bytes_read": None}
        self.assertEqual(kinds(rec), [("l2.value", "value_mismatch")])
        rec["l2"]["value"] = None
        self.assertEqual(kinds(rec), [])
        rec["l2"]["error"] = {"kind": "check_read", "bytes_read": 9}
        self.assertEqual(kinds(rec), [("l2.value", "value_mismatch")])

        # A file's l1 is there exactly when L1 is declared and it parsed as a container, of the matching kind.
        f = file_rec()
        f["l1"] = None
        self.assertEqual(kinds(f), [("l1", "missing_section")])
        self.assertEqual(kinds(f, ["L0"]), [])
        f = file_rec()
        f["l0"]["parsed_as"] = "BundleFile"
        self.assertEqual(kinds(f), [("l1.kind", "kind_mismatch")])
        f["l0"]["parsed_as"] = "raw"
        self.assertEqual(kinds(f), [("l1.kind", "kind_mismatch")])
        f["l1"] = None
        self.assertEqual(kinds(f), [])
        f["l0"]["parsed_as"] = "error"
        self.assertEqual(kinds(f), [("error", "error_mismatch")])
        f["error"] = {"kind": "parse"}
        self.assertEqual(kinds(f), [])
        f["l0"]["parsed_as"] = "zip"
        self.assertEqual(kinds(f), [("error", "error_mismatch")])

        # Every name position: a valid-UTF-8 hex name, then a malformed one.
        utf8, upper = {"t": "str", "hex": "61"}, {"t": "str", "hex": "FF"}
        b = bundle_rec()
        s = file_rec()
        s["l1"]["externals"] = [["", "00" * 16, 0, "lib"]]
        s["l1"]["container"] = [["assets/x", 0, 0, 0, 1]]
        s["l1"]["ref_types"] = [{**type_rec(), "class_name": "C", "namespace": "N", "assembly": "A"}]
        positions = [
            (b, ("file", 0), "file[0]"),
            (b, ("l1", "signature"), "l1.signature"),
            (b, ("l1", "version_player"), "l1.version_player"),
            (b, ("l1", "version_engine"), "l1.version_engine"),
            (b, ("l1", "directory", 0, 3), "l1.directory[0][3]"),
            (s, ("l1", "unity_version"), "l1.unity_version"),
            (s, ("l1", "user_information"), "l1.user_information"),
            (s, ("l1", "externals", 0, 0), "l1.externals[0][0]"),
            (s, ("l1", "externals", 0, 3), "l1.externals[0][3]"),
            (s, ("l1", "container", 0, 0), "l1.container[0][0]"),
            (s, ("l1", "types", 0, "nodes", 0, 1), "l1.types[0].nodes[0][1]"),
            (s, ("l1", "types", 0, "nodes", 0, 2), "l1.types[0].nodes[0][2]"),
            (s, ("l1", "ref_types", 0, "class_name"), "l1.ref_types[0].class_name"),
            (s, ("l1", "ref_types", 0, "namespace"), "l1.ref_types[0].namespace"),
            (s, ("l1", "ref_types", 0, "assembly"), "l1.ref_types[0].assembly"),
            (obj, ("file", 0), "file[0]"),
            (obj, ("l2", "root_type"), "l2.root_type"),
        ]
        self.assertEqual(kinds(s), [])
        for base, keys, path in positions:
            for value, kind in ((utf8, "hex_name_is_utf8"), (upper, "bad_name"), (["x"], "bad_name")):
                with self.subTest(path=path, value=value):
                    rec = copy.deepcopy(base)
                    target = rec
                    for k in keys[:-1]:
                        target = target[k]
                    target[keys[-1]] = value
                    self.assertEqual(kinds(rec), [(path, kind)])
        rec = copy.deepcopy(b)
        rec["l1"]["directory"] = [[0, 1, "x"]]
        self.assertEqual(kinds(rec), [("l1.directory[0]", "malformed")])

    def test_end_counts_are_integers(self):
        recs = base_records()
        recs[3]["files"] = True  # one file record, but true is not 1
        report, d = self.run_compare(self.write("a.jsonl", recs), self.write("b.jsonl", recs))
        self.assertEqual(
            self.paths(d),
            {("format", "end.files (reference)", "end_count"), ("format", "end.files (candidate)", "end_count")},
        )

    def test_truncated_dump_is_a_format_finding(self):
        recs = base_records()
        _, d = self.run_compare(self.write("a.jsonl", recs), self.write("b.jsonl", recs[:-1]))
        self.assertIn(("format", "(end, candidate)", "missing_end"), self.paths(d))
        _, d = self.run_compare(self.write("a.jsonl", recs), self.write("b.jsonl", [*recs, file_rec("z")]))
        self.assertIn(("format", "record (candidate)", "after_end"), self.paths(d))

    def test_bad_inputs(self):
        recs = base_records()
        pa = self.write("a.jsonl", recs)
        for head in (
            header(version=99),
            header(version=True),
            header(["L1", "L0"]),
            header(["L0", "L0"]),
            header(["L1", "L2"]),  # every dump holds L0
            header([]),
        ):
            with self.subTest(head=head), self.assertRaises(C.FormatError):
                compare.run(pa, self.write("b.jsonl", recs, head=head))
        with self.assertRaises(C.FormatError):
            compare.run(pa, self.write("b.jsonl", [], lines=["not json"]))

    def test_unreadable_dumps_exit_2(self):
        """Malformed input is a one-line error and exit 2, never a traceback."""
        recs = base_records()
        pa = self.write("a.jsonl", recs)
        good = ("\n".join(C.dumps(r) for r in [header(), *recs]) + "\n").encode()
        packed = gzip.compress(good, mtime=0)
        bad = {
            "truncated.jsonl.gz": packed[: len(packed) // 2],
            "corrupt.jsonl.gz": packed[:12] + b"\xff" * 40,
            "nan.jsonl": good.replace(b'"size":10,', b'"size":NaN,'),
            "infinity.jsonl": good.replace(b'"size":10,', b'"size":-Infinity,'),
            "surrogate.jsonl": good.replace(b'"root_type":"Transform"', b'"root_type":"\\ud800"', 1),
            "not_utf8.jsonl": good.replace(b'"Transform"', b'"Tr\xffnsform"', 1),
            "blank_line.jsonl": good.replace(b"\n", b"\n\n", 1),
            "second_header.jsonl": good + good.split(b"\n")[0] + b"\n",
            "bad_name.jsonl": good.replace(b'"file":["a.bundle"]', b'"file":[7]'),
            "empty.jsonl": b"",
        }
        for name, raw in bad.items():
            with self.subTest(name):
                path = Path(self.tmp.name) / name
                path.write_bytes(raw)
                self.assertEqual(self.cli(pa, path), 2)
                self.assertTrue(self.errors.startswith("compare: "), self.errors)

    def test_json_report_stays_out_of_the_repository(self):
        """The report quotes input names, so --json is refused inside the repository, through symlinks too."""
        a = self.write("a.jsonl", base_records())
        here = Path(__file__).resolve().parents[1]
        link = Path(self.tmp.name) / "link"
        link.symlink_to(here)
        for target in (here / "report.json", link / "report.json"):
            with self.subTest(target=target):
                try:
                    self.assertEqual(self.cli(a, a, "--json", target), 2)
                    self.assertIn("refusing to write --json into the repository", self.errors)
                    self.assertFalse((here / "report.json").exists())
                finally:
                    (here / "report.json").unlink(missing_ok=True)
        out = Path(self.tmp.name) / "report.json"
        self.assertEqual(self.cli(a, a, "--json", out), 0)
        self.assertEqual(json.loads(out.read_text())["counts"]["identical"], 4)

    def test_cli_exit_codes_and_report(self):
        a, b = base_records(), base_records()
        b[1]["l3"]["resave"] = "differs"
        pa, pb = self.write("a.jsonl", a), self.write("b.jsonl", b)
        self.assertEqual(self.cli(pa, pa), 0)
        self.assertEqual(self.cli(pa, pb, "--json", Path(self.tmp.name) / "r.json"), 1)
        self.assertIn("L3      Transform", self.output)
        report = json.loads((Path(self.tmp.name) / "r.json").read_text())
        self.assertEqual(report["by_path"][0]["path"], "l3.resave")


if __name__ == "__main__":
    unittest.main()
