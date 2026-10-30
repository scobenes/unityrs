"""compare.py on hand-built dumps: how values compare (types, kinds, lengths, floats, NaNs and nulls)."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import canonical as C  # noqa: E402
import compare  # noqa: E402
from compare_support import CompareCase, S, base_records, bundle_rec, end, f32, file_rec, header, one_object  # noqa: E402


class CompareTests(CompareCase):
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

    def test_u8s_compares_as_bytes_of_its_own_kind(self):
        """A packed UInt8 vector is raw bytes: not a list of ints, not TypelessData (spec section 3)."""
        u8s = {"t": "u8s", "hex": "000102"}
        for other, kind in (([0, 1, 2], "type"), ({"t": "bytes", "hex": "000102"}, "value")):
            with self.subTest(other=other):
                a = self.write("a.jsonl", one_object(S(["u8v", u8s])))
                b = self.write("b.jsonl", one_object(S(["u8v", other])))
                _, d = self.run_compare(a, b)
                self.assertEqual(self.paths(d), {("L2", "l2.value.u8v", kind)})
        summary = {"t": "u8s", "len": 65, "sha256": C.enc_raw("u8s", bytes(65))["sha256"]}
        _, d = self.run_compare(
            self.write("a.jsonl", one_object(S(["u8v", summary]))),
            self.write("b.jsonl", one_object(S(["u8v", [0] * 65]))),
        )
        self.assertEqual(self.paths(d), {("L2", "l2.value.u8v", "type")})

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
        # A float is no size either: section 4.2 makes it an integer.
        self.assertEqual(
            self.paths(d),
            {
                ("L0", "l0.size", "type"),
                ("format", "line (candidate)", "json_float"),
                ("format", "l0.size (candidate)", "bad_value"),
            },
        )

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


if __name__ == "__main__":
    unittest.main()
