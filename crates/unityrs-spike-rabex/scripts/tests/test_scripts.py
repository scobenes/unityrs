"""Tests for the spike's comparison scripts.

    python -m unittest discover -s crates/unityrs-spike-rabex/scripts/tests

Runs in the dev container (UnityPy 1.25.2 installed); no corpus needed.
"""

import json
import subprocess
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace as NS

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))

import classid_compare  # noqa: E402
import mem_peak  # noqa: E402
import rabex_loc  # noqa: E402
import save_compare  # noqa: E402
import value_cases  # noqa: E402


def fake_sf(**over):
    """A parsed-SerializedFile stand-in with the attributes save_compare reads."""
    node = NS(m_Level=0, m_Type="Foo", m_Name="Base", m_ByteSize=-1, m_Version=1, m_TypeFlags=0, m_MetaFlag=0)
    node.m_Children = []
    typ = NS(class_id=114, is_stripped_type=False, script_type_index=0, script_id=b"s" * 16, old_type_hash=b"h" * 16)
    typ.node = node
    offsets = over.pop("offsets", (0, 16))

    def obj(pid, off):
        raw = bytes([pid]) * 8
        return NS(
            path_id=pid,
            type_id=0,
            class_id=114,
            byte_start=100 + off,
            byte_size=8,
            is_destroyed=None,
            is_stripped=None,
            get_raw_data=lambda: raw,
        )

    sf = NS(
        header=NS(version=22, endian="<", reserved=b"\0\0\0", metadata_size=52, data_offset=100, file_size=132),
        unknown=0,
        unity_version="6000.3.17f1",
        target_platform=19,
        _enable_type_tree=True,
        big_id_enabled=0,
        userInformation="",
        types=[typ],
        ref_types=[],
        script_types=[NS(local_serialized_file_index=0, local_identifier_in_file=11)],
        externals=[NS(path="archive:/CAB-x/CAB-x", guid=b"\0" * 16, type=0)],
        objects={pid: obj(pid, off) for pid, off in zip((1, 2), offsets, strict=True)},
    )
    for k, v in over.items():
        setattr(sf, k, v)
    return sf


class SaveCompareTests(unittest.TestCase):
    def test_identical_files_do_not_differ(self):
        self.assertEqual(save_compare.sf_diffs(fake_sf(), fake_sf()), [])

    def test_moved_objects_are_a_layout_difference(self):
        # Object data rewritten in another order: raw bytes and classes are
        # equal, which is all the old check compared, but the table moved.
        a, b = fake_sf(), fake_sf(offsets=(16, 0))
        self.assertEqual(save_compare.sf_diffs(a, b), ["object_offsets"])
        self.assertEqual(save_compare.moved_objects(a, b), (2, 2))

    def test_metadata_differences_are_seen(self):
        cases = {
            "header": {"unity_version": "2018.4.4f1"},
            "externals": {"externals": [NS(path="archive:/CAB-y/CAB-y", guid=b"\0" * 16, type=0)]},
            "script_types": {"script_types": []},
            "layout": {
                "header": NS(version=22, endian="<", reserved=b"\0\0\0", metadata_size=60, data_offset=112, file_size=144)
            },
        }
        for field, over in cases.items():
            with self.subTest(field=field):
                self.assertIn(field, save_compare.sf_diffs(fake_sf(), fake_sf(**over)))

    def test_fs_flags_reads_the_header_flags(self):
        head = b"UnityFS\0" + (6).to_bytes(4, "big") + b"5.x.x\0" + b"2018.4.4f1\0"
        head += (100).to_bytes(8, "big") + (40).to_bytes(4, "big") + (40).to_bytes(4, "big") + (0x240).to_bytes(4, "big")
        self.assertEqual(save_compare.fs_flags(head), 0x240)

    def test_reserved_header_bytes_are_compared(self):
        # rabex's version-22 writer zeroes the 3 reserved bytes it read.
        a = fake_sf()
        a.header.reserved = b"\x01\0\0"
        self.assertEqual(save_compare.sf_diffs(a, fake_sf()), ["header"])
        b = fake_sf(unknown=5)
        self.assertEqual(save_compare.sf_diffs(fake_sf(), b), ["header"])

    def test_type_trees_are_compared(self):
        b = fake_sf()
        b.types[0].node.m_Name = "Other"
        self.assertEqual(save_compare.sf_diffs(fake_sf(), b), ["types"])

    def test_type_dependencies_and_ref_type_names_are_compared(self):
        # Version 21+: a type's dependency list, and a ref type's three names.
        a, b = fake_sf(), fake_sf()
        a.types[0].type_dependencies, b.types[0].type_dependencies = (1, 2), (1, 3)
        self.assertEqual(save_compare.sf_diffs(a, b), ["types"])
        a, b = fake_sf(), fake_sf()
        for sf, name in ((a, "Foo"), (b, "Bar")):
            ref = fake_sf().types[0]
            ref.m_ClassName, ref.m_NameSpace, ref.m_AssemblyName = name, "", "Assembly-CSharp.dll"
            sf.ref_types = [ref]
        self.assertEqual(save_compare.sf_diffs(a, b), ["ref_types"])

    def test_node_index_and_ref_type_hash_are_compared(self):
        # rabex writes each node's pre-order position, not the m_Index it read.
        for attr in ("m_Index", "m_RefTypeHash", "m_VariableCount"):
            with self.subTest(attr=attr):
                a, b = fake_sf(), fake_sf()
                setattr(a.types[0].node, attr, 0)
                setattr(b.types[0].node, attr, 7)
                self.assertEqual(save_compare.sf_diffs(a, b), ["types"])


class ValueCasesTests(unittest.TestCase):
    def test_l2_compares_bytes_read(self):
        v = value_cases.l2_verdict
        value = '{"t":"dict","v":[]}'

        def side(kind, n, val=value):
            return {"kind": kind, "bytes_read": n, "value": val}

        self.assertEqual(v(side("check_read", 4), side("check_read", 4)), "match")
        # The same value, but the reads ended at different bytes: compare.py calls this an L2 difference.
        self.assertEqual(v(side("check_read", 4), side("check_read", 1)), "differs")
        self.assertEqual(v(side(None, None, "7fc00001"), side(None, None, "7f800001")), "f32_snan_quieted")
        self.assertEqual(v(side(None, None, "7fc00001"), side("check_read", 1, "7f800001")), "differs")

    def test_empty_struct_align_last_reads_four_bytes_in_unitypy(self):
        case = next(c for c in value_cases.CASES if c["name"] == "empty_struct_align_last")
        o = value_cases.oracle(case)
        self.assertEqual((o["kind"], o["bytes_read"]), ("check_read", 4))

    def test_l3_needs_both_resaves(self):
        v = value_cases.l3_verdict
        data = b"\x02\0\0\0"
        self.assertEqual(v(None, None, data), "not_resaved")
        self.assertEqual(v(data, None, data), "one_side")
        self.assertEqual(v(None, data, data), "one_side")

    def test_l3_compares_the_resaved_bytes(self):
        v = value_cases.l3_verdict
        data = b"\x03\0\0\0a\xffb\x01\x07\0\0\0"
        clean = b"\x03\0\0\0a\xffb\0\x07\0\0\0"
        unaligned = b"\x03\0\0\0a\xffb\x07\0\0\0"
        self.assertEqual(v(data, data, data), "match")
        # Both re-saves differ from the input, and from each other: not a match.
        self.assertEqual(v(clean, unaligned, data), "differs")
        # Both differ from the input the same way: agreement, not a round trip.
        self.assertEqual(v(clean, clean, data), "same_bytes")
        self.assertEqual(v(data, clean, data), "differs")

    def test_summary_counts_l3_over_resaved_cases(self):
        table = [
            {"case": "a", "l2": "match", "l3": "match"},
            {"case": "b", "l2": "differs", "l3": "not_resaved"},
            {"case": "c", "l2": "match", "l3": "differs"},
            {"case": "d", "l2": "differs", "l3": "one_side"},
            {"case": "e", "l2": "match", "l3": "same_bytes"},
        ]
        s = value_cases.summary(table)
        self.assertEqual(s["l3_resaved_by_both"], 3)
        self.assertEqual(s["l3_same_bytes"], ["e"])
        self.assertEqual(s["l3_differs"], ["c"])
        self.assertEqual(s["l3_not_resaved"], ["b"])
        self.assertEqual(s["l3_one_side"], ["d"])

    def test_truncation_cases_are_present(self):
        names = {c["name"] for c in value_cases.CASES}
        self.assertLessEqual({"vector_u8_truncated", "vector_float_truncated", "vector_int_truncated"}, names)


class MemPeakTests(unittest.TestCase):
    def test_forked_workers_share_the_parents_pages(self):
        # A parent touches 200 MB, then forks 4 workers that only sleep. Every
        # worker's RSS includes the parent's pages (the old summed ru_maxrss
        # counted them 5 times); PSS counts them once.
        prog = (
            "import os, time\n"
            "b = bytearray(200 * 1024 * 1024)\n"
            "for i in range(0, len(b), 4096): b[i] = 1\n"
            "pids = []\n"
            "for _ in range(4):\n"
            "    p = os.fork()\n"
            "    if p == 0:\n"
            "        time.sleep(1.5); os._exit(0)\n"
            "    pids.append(p)\n"
            "for p in pids: os.waitpid(p, 0)\n"
        )
        code, r = mem_peak.run([sys.executable, "-c", prog], interval=0.05)
        self.assertEqual(code, 0)
        mb = 1024 * 1024
        self.assertEqual(r["max_processes"], 5)
        self.assertGreater(r["peak_rss_sum_bytes"], 4 * 200 * mb)
        self.assertGreater(r["peak_pss_sum_bytes"], 200 * mb)
        self.assertLess(r["peak_pss_sum_bytes"], 300 * mb)


class RabexLocTests(unittest.TestCase):
    def test_counts_code_comments_and_tests(self):
        text = (
            "//! doc\n"
            "use a::b;\n"
            "\n"
            "/* block\n"
            "   comment */\n"
            "fn f() {\n"
            "    // note\n"
            "    let x = format!(\"{}\", 1);\n"
            "}\n"
            "#[cfg(test)]\n"
            "mod tests {\n"
            "    #[test]\n"
            "    fn t() {}\n"
            "}\n"
            "fn g() {}\n"
        )
        self.assertEqual(rabex_loc.count(text), (5, 5))

    def test_cfg_all_test_items_are_tests(self):
        text = (
            "fn f() {}\n"
            "#[cfg(all(test, feature = \"embed-tpk\"))]\n"
            "mod tests {\n"
            "    fn t() {}\n"
            "}\n"
        )
        self.assertEqual(rabex_loc.count(text), (1, 4))


class ClassIdCompareTests(unittest.TestCase):
    def test_reads_the_constants(self):
        text = (
            "    pub const UnknownType: ClassId = ClassId(-1);\n"
            "    // Renamed from: \"Scene\", \"SceneSettings\"\n"
            "    pub const OcclusionCullingSettings: ClassId = ClassId(29);\n"
        )
        self.assertEqual(classid_compare.rabex_ids(text), {-1: "UnknownType", 29: "OcclusionCullingSettings"})


class MemPeakCliTests(unittest.TestCase):
    def test_report_goes_to_out_and_exit_code_passes_through(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "r.json"
            p = subprocess.run(
                [sys.executable, str(SCRIPTS / "mem_peak.py"), "--out", str(out), "--", sys.executable, "-c", "exit(3)"],
                check=False,
            )
            self.assertEqual(p.returncode, 3)
            self.assertEqual(json.loads(out.read_text())["exit"], 3)


if __name__ == "__main__":
    unittest.main()
