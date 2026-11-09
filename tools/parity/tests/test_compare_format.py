"""compare.py on hand-built dumps: records missing, extra, out of order or repeated, and the format findings."""

import copy
import json
import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import canonical as C  # noqa: E402
from compare_support import (  # noqa: E402
    LEVELS,
    SIDES,
    CompareCase,
    S,
    base_records,
    bundle_rec,
    end,
    f32,
    file_rec,
    header,
    obj_rec,
    one_object,
    type_rec,
)


class CompareTests(CompareCase):
    def test_fixtures_are_canonical(self):
        for rec in [*base_records(), bundle_rec(), header()]:
            self.assertEqual(C.check_record(rec, LEVELS), [], rec["rec"])

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

        def raw(*names):
            return {
                "rec": "file",
                "file": list(names),
                "l0": {"type": "ResourceFile", "parsed_as": "raw", "size": 10, "sha256": "22" * 32},
                "l1": None,
                "error": None,
            }

        bundle = bundle_rec()
        bundle["l1"]["directory"] = [[0, 10, 4, "y"], [10, 10, 4, "x"], [20, 10, 4, "y"]]
        f, o1, o2, _ = base_records()
        y, x, c = raw("b.bundle", "y"), raw("b.bundle", "x"), raw("c")
        ref = [f, o1, o2, bundle, y, x, c, end(5, 2)]
        cases = {
            "objects descending": [f, o2, o1, bundle, y, x, c, end(5, 2)],
            "objects before their file": [o1, o2, f, bundle, y, x, c, end(5, 2)],
            "inputs descending": [bundle, y, x, f, o1, o2, c, end(5, 2)],
            "entries against the directory": [f, o1, o2, bundle, x, y, c, end(5, 2)],
            "an entry after the next input": [f, o1, o2, bundle, y, c, x, end(5, 2)],
        }
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
        f["l0"]["type"], f["l0"]["parsed_as"] = "BundleFile", "BundleFile"
        self.assertEqual(kinds(f), [("l1.kind", "kind_mismatch")])
        f["l0"]["type"], f["l0"]["parsed_as"] = "AssetsFile", "raw"  # an AssetsFile the name rule keeps raw
        self.assertEqual(kinds(f), [("l1.kind", "kind_mismatch")])
        f["l1"] = None
        self.assertEqual(kinds(f), [])
        f["l0"]["parsed_as"] = "error"
        self.assertEqual(kinds(f), [("error", "error_mismatch")])
        f["error"] = {"kind": "parse"}
        self.assertEqual(kinds(f), [])
        f["l0"]["type"], f["l0"]["parsed_as"] = "ZIP", "zip"
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


if __name__ == "__main__":
    unittest.main()
