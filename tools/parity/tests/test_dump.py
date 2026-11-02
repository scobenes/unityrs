"""unitypy_dump.py end to end: UnityPy's samples, determinism, and the records it writes.

L0 detection and the name rules, bundle layouts and child failures, the object
table, version strings, type records, node rows and blob trees, externals, the
resave, and input names. The crash records, the pinned-stack refusal and the
command line's rules are test_dump_crashes.py's, test_dump_oracle.py's and
test_dump_cli.py's; dump_harness.py holds what they share, the samples included.
Needs the oracle stack; skipped elsewhere.
"""

import gzip
import hashlib
import io
import json
import os
import struct
import sys
import unittest
import zipfile
from collections import Counter
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

import canonical as C  # noqa: E402
import compare  # noqa: E402
import synth as S  # noqa: E402
from dump_harness import (  # noqa: E402
    SAMPLES,
    DumpCase,
    assets,
    dump,
    dump_process,
    ints_value,
    records,
    unitypy_dump,
    unityraw,
)


@unittest.skipIf(unitypy_dump is None, "needs UnityPy (the oracle image)")
class DumpTests(DumpCase):
    def test_samples(self):
        out = self.tmp / "s.jsonl"
        self.assertEqual(dump(*self.sample_args(), "-o", out), 0)
        recs = records(out)
        head, end = recs[0], recs[-1]
        self.assertEqual(
            (head["format"], head["version"], head["levels"]), ("unityrs-canonical-dump", 1, ["L0", "L1", "L2", "L3"])
        )
        self.assertTrue(head["producer"]["boost"])
        # The header names the stack: the Boost binary's hash and every pinned package's version.
        self.assertEqual(head["producer"]["boost_sha256"], C.ORACLE_BOOST_SHA256)
        self.assertEqual(head["producer"]["packages"], C.oracle_pins())
        self.assertIn("numpy", head["producer"]["packages"])
        # ... and each one's RECORD digest, whose files the dumper hashed: the pinned wheels'.
        self.assertEqual(head["producer"]["records"], C.oracle_records())
        self.assertEqual(list(head["producer"])[-2:], ["records", "python"])
        self.assertEqual(end, {"rec": "end", "files": 13, "objects": 53})
        self.assert_canonical(out)

        files = [r for r in recs if r["rec"] == "file"]
        objects = [r for r in recs if r["rec"] == "object"]
        self.assertEqual([r["file"][0] for r in files if len(r["file"]) == 1], sorted(SAMPLES))
        for r in objects:
            self.assertIsNone(r["l2"]["error"], r["path_id"])
            self.assertEqual(r["l2"]["source"], "embedded")
            self.assertEqual(r["l3"]["resave"], "equal")
            self.assertEqual(r["l3"]["raw_size"], r["l1"]["byte_size"])
        classes = {name: Counter(r["l2"]["root_type"] for r in objects if r["file"][0] == name) for name in SAMPLES}
        self.assertEqual(classes["atlas_test"], {"Sprite": 7, "SpriteAtlas": 1, "Texture2D": 1, "AssetBundle": 1})
        self.assertEqual(classes["char_118_yuki.ab"], {"AudioClip": 35, "AssetBundle": 1})
        self.assertEqual(classes["xinzexi_2_n_tex"], {"Mesh": 1, "Sprite": 1, "Texture2D": 1, "AssetBundle": 1})

        by_path = {tuple(r["file"]): r for r in files}
        atlas = by_path[("atlas_test",)]["l1"]
        self.assertEqual(
            {
                k: atlas[k]
                for k in ("signature", "format", "version_engine", "flags", "flags_class", "blocks_info", "blocks")
            },
            {
                "signature": "UnityFS",
                "format": 6,
                "version_engine": "2018.4.11f1",
                "flags": 67,
                "flags_class": "ArchiveFlagsOld",
                "blocks_info": [153, 88, 67],
                "blocks": [[93300, 76633, 3]],
            },
        )
        self.assertEqual(
            [row[3] for row in atlas["directory"]],
            ["CAB-41a198e3c6c156112514cb656a4a5a12", "CAB-41a198e3c6c156112514cb656a4a5a12.resS"],
        )
        self.assertEqual(by_path[("xinzexi_2_n_tex",)]["l1"]["blocks"], [[2076384, 634146, 65]], "one LZMA block")
        cab = by_path[("atlas_test", "CAB-41a198e3c6c156112514cb656a4a5a12")]
        self.assertEqual(cab["l0"]["parsed_as"], "SerializedFile")
        self.assertEqual(cab["l1"]["header"]["version"], 17)
        self.assertEqual(cab["l1"]["target_platform"], 13)
        self.assertEqual(sorted(cab["l1"]["object_order"]), [r["path_id"] for r in objects if r["file"] == cab["file"]])
        res = by_path[("char_118_yuki.ab", "CAB-8579bc75d50073df38987733a7cb3193.resource")]
        self.assertEqual((res["l0"]["type"], res["l0"]["parsed_as"]), ("ResourceFile", "raw"))
        self.assertEqual(by_path[("xinzexi_2_n_tex_mesh",)]["l0"]["parsed_as"], "raw")

    def test_deterministic_across_runs_jobs_and_gzip(self):
        args = self.sample_args()
        a, b, c = self.tmp / "a.jsonl", self.tmp / "b.jsonl", self.tmp / "c.jsonl.gz"
        dump(*args, "-o", a)
        dump(*args, "-o", b, "-j", "2")
        dump(*args, "-o", c)
        self.assertEqual(a.read_bytes(), b.read_bytes())
        self.assertEqual(a.read_bytes(), gzip.decompress(c.read_bytes()))
        d = self.tmp / "d.jsonl.gz"
        dump(*args, "-o", d)
        self.assertEqual(c.read_bytes(), d.read_bytes(), "gzip output is byte-stable (mtime 0)")
        # The spec's example header: level 9 (XFL 02), mtime 0, OS 255.
        self.assertEqual(c.read_bytes()[:10].hex(), "1f8b08000000000002ff")

    def test_compare_finds_an_injected_difference(self):
        a = self.tmp / "a.jsonl"
        dump(*self.sample_args(), "-o", a)
        lines = a.read_text(encoding="utf-8").splitlines()
        for i, line in enumerate(lines):
            rec = json.loads(line)
            if rec["rec"] == "object" and rec["l2"]["root_type"] == "Sprite":
                rect = dict(rec["l2"]["value"]["v"])["m_Rect"]
                rect["v"][0][1]["v"] = "3f800001"
                lines[i] = json.dumps(rec, ensure_ascii=False, separators=(",", ":"))
                break
        b = self.tmp / "b.jsonl"
        b.write_text("\n".join(lines) + "\n", encoding="utf-8")
        report, *_ = compare.run(str(a), str(a))
        self.assertFalse(report.has_diffs())
        report, *_ = compare.run(str(a), str(b))
        rows = report.as_dict()["by_path"]
        self.assertEqual(
            [(r["level"], r["class"], r["path"], r["kind"]) for r in rows],
            [("L2", "Sprite", "l2.value.m_Rect.x", "value")],
        )

    def test_l0_detection_and_name_rules(self):
        d = self.tmp / "in"
        d.mkdir()
        assets_header = struct.pack(">IIII", 100, 200, 17, 150) + b"\0" * 184
        zbuf = io.BytesIO()
        with zipfile.ZipFile(zbuf, "w") as z:
            z.writestr("x.txt", "x" * 200)
        noise = b"".join(hashlib.sha256(bytes([i])).digest() for i in range(10))
        web = gzip.compress(b"UnityWebData1.0\0" + noise, mtime=0)
        files = {
            "gz_garbage": b"\x1f\x8b" + bytes(range(198)),
            "gz_web": web,
            "gz_web_bad_crc": web[:-8] + bytes(4) + web[-4:],
            "gz_web_junk_tail": web + b"junkjunk",
            "gz_web_truncated": web[:-20],
            "gz_web_zero_padded": web + bytes(40),
            "gz_12_bytes": gzip.compress(b"UnityWebData", mtime=0).ljust(200, b"\0"),  # last byte never read
            "gz_13_bytes": gzip.compress(b"UnityWebData1", mtime=0).ljust(200, b"\0"),
            "tiny": b"A" * 19,
            "twenty": b"A" * 20,
            "fa_sig": b"\xfa" * 8 + b"\0" * 200,
            "u_sig": "ú".encode() * 8 + b"\0" * 200,
            "nul.zip": b"PK\x03\x04\0" + b"\0" * 200,
            "plain.zip": zbuf.getvalue(),
            "fake.resS": assets_header,
            "fake.RESS": assets_header,  # the name rule is case-sensitive
            "fake.assets": assets_header,
        }
        for name, data in files.items():
            (d / name).write_bytes(data)
        out = self.tmp / "l0.jsonl"
        dump(d, "-o", out)
        recs = [r for r in records(out) if r["rec"] == "file"]
        self.assertEqual([r["file"][0] for r in recs], sorted(files, key=os.fsencode))
        got = {r["file"][0]: (r["l0"]["type"], r["l0"]["parsed_as"], (r["error"] or {}).get("kind")) for r in recs}
        self.assertEqual(
            got,
            {
                # The whole gzip stream must decompress; the signature is read from it.
                "gz_garbage": ("error", "error", "detect"),
                "gz_web": ("WebFile", "error", "parse"),
                "gz_web_bad_crc": ("error", "error", "detect"),
                "gz_web_junk_tail": ("error", "error", "detect"),
                "gz_web_truncated": ("error", "error", "detect"),
                "gz_web_zero_padded": ("WebFile", "error", "parse"),
                "gz_12_bytes": ("ResourceFile", "raw", None),
                "gz_13_bytes": ("WebFile", "error", "parse"),
                "tiny": ("ResourceFile", "raw", None),
                "twenty": ("error", "error", "detect"),
                "fa_sig": ("ResourceFile", "raw", None),  # bytes FA never equal U+00FA
                "u_sig": ("BundleFile", "error", "parse"),  # C3 BA x8 does
                "nul.zip": ("ZIP", "zip", None),
                "plain.zip": ("ResourceFile", "raw", None),  # its 5th byte is 0x14, not NUL
                "fake.resS": ("AssetsFile", "raw", None),  # the name rule
                "fake.RESS": ("AssetsFile", "error", "parse"),
                "fake.assets": ("AssetsFile", "error", "parse"),
            },
        )
        self.assert_canonical(out)

    def test_bundle_layouts_names_and_child_failures(self):
        d = self.tmp / "in"
        d.mkdir()
        broken_assets = struct.pack(">IIII", 100, 200, 17, 150) + bytes(4) + b"\xff" * 180
        files = {
            "raw5": unityraw(5, [(b"one", b"tiny"), (b"tw\xffo", b"abc")], player=b"5.x\xff"),
            "raw6": unityraw(6, [(b"one", b"tiny"), (b"two", b"abc")]),
            "raw5_bad_detect": unityraw(5, [(b"one", b"tiny"), (b"bad", b"A" * 20)]),
            "raw5_bad_parse": unityraw(5, [(b"one", b"tiny"), (b"bad.assets", broken_assets)]),
        }
        for name, data in files.items():
            (d / name).write_bytes(data)
        out = self.tmp / "raw.jsonl"
        self.assertEqual(dump(d, "-o", out), 0)
        self.assert_canonical(out)
        recs = {json.dumps(r["file"]): r for r in records(out) if r["rec"] == "file"}
        tw = {"t": "str", "hex": b"tw\xffo".hex()}

        raw5 = recs[json.dumps(["raw5"])]["l1"]
        packed_567f1 = 5 << 48 | 6 << 32 | 7 << 16 | 3 << 8 | 1
        self.assertEqual(raw5["engine_version_packed"], packed_567f1)
        self.assertEqual(recs[json.dumps(["raw6"])]["l1"]["engine_version_packed"], packed_567f1)
        self.assertEqual((raw5["hash"], raw5["crc"]), (bytes(range(16)).hex(), 0xC0FFEE))
        self.assertEqual((raw5["flags"], raw5["flags_class"], raw5["blocks_info"], raw5["blocks"]), (None,) * 4)
        self.assertEqual([row[2:] for row in raw5["directory"]], [[None, "one"], [None, tw]])
        self.assertEqual(raw5["version_player"], {"t": "str", "hex": b"5.x\xff".hex()}, "metadata strings use enc_name")
        self.assertIn(json.dumps(["raw5", tw]), recs, "entry names use enc_name")

        raw6 = recs[json.dumps(["raw6"])]["l1"]
        self.assertEqual((raw6["hash"], raw6["crc"]), (None, None), "format 6 takes the UnityFS layout")
        self.assertEqual((raw6["flags"], raw6["flags_class"], raw6["blocks"]), (0x40, "ArchiveFlagsOld", [[7, 7, 0]]))
        self.assertEqual(raw6["blocks_info"], [82, 82, 0x40])
        self.assertEqual([row[2] for row in raw6["directory"]], [4, 4])

        # A child that cannot be detected or parsed fails the whole input.
        for name in ("raw5_bad_detect", "raw5_bad_parse"):
            self.assertEqual([k for k in recs if json.loads(k)[0] == name], [json.dumps([name])])
            top = recs[json.dumps([name])]
            self.assertEqual((top["l0"]["parsed_as"], top["l1"], top["error"]["kind"]), ("error", None, "parse"))
        self.assertTrue(all(r["error"] is None for k, r in recs.items() if len(json.loads(k)) > 1))

    def test_object_records_follow_the_table_and_the_parse(self):
        """The object table's dict order, a check_read value, a resave that differs, a TPK tree."""
        d = self.tmp / "in"
        d.mkdir()
        tree = S.Node("MyThing", "Base", [S.Node("int", "i"), S.Node("bool", "b", meta=S.ALIGN)])
        rows = [
            S.Obj(5, 0, struct.pack("<i", 1) + b"\1\0\0\0"),
            S.Obj(-3, 0, struct.pack("<i", 7) + b"\1\0\0\0", byte_size=5),  # the read ends at 8
            S.Obj(9, 0, struct.pack("<i", 7) + b"\2\0\0\0"),  # a bool byte of 2 resaves as 1
            S.Obj(5, 0, struct.pack("<i", 2) + b"\1\0\0\0"),  # a repeated path_id: the last entry wins
        ]
        (d / "obj.assets").write_bytes(S.serialized_file([S.Type(114, tree)], rows))
        textasset = S.s32("name") + S.s32("text")
        (d / "tpk.assets").write_bytes(
            S.serialized_file([S.Type(49)], [S.Obj(1, 0, textasset)], enable_type_tree=False)
        )
        out = self.tmp / "objects.jsonl"
        self.assertEqual(dump(d, "-o", out), 0)
        self.assert_canonical(out)
        recs = records(out)
        files = {r["file"][0]: r for r in recs if r["rec"] == "file"}
        objects = {(r["file"][0], r["path_id"]): r for r in recs if r["rec"] == "object"}
        self.assertEqual(files["obj.assets"]["l1"]["object_order"], [5, -3, 9])
        self.assertEqual([k for k in objects if k[0] == "obj.assets"], [("obj.assets", p) for p in (-3, 5, 9)])

        def struct_value(i, b):
            return {"t": "dict", "v": [["i", i], ["b", b]]}

        short = objects[("obj.assets", -3)]
        self.assertEqual(short["l1"]["byte_size"], 5)
        self.assertEqual(
            {k: v for k, v in short["l2"].items() if k != "error"},
            {"root_type": "MyThing", "source": "embedded", "value": struct_value(7, True)},
        )
        self.assertEqual((short["l2"]["error"]["kind"], short["l2"]["error"]["bytes_read"]), ("check_read", 8))
        self.assertEqual((short["l3"]["raw_size"], short["l3"]["resave"]), (5, "skipped"))

        last = objects[("obj.assets", 5)]
        self.assertEqual(last["l2"]["value"], struct_value(2, True))
        self.assertGreater(last["l1"]["byte_start"], objects[("obj.assets", 9)]["l1"]["byte_start"])

        bool2 = objects[("obj.assets", 9)]["l3"]
        self.assertEqual(
            (bool2["resave"], bool2["resave_size"], bool2["first_diff"]), ("differs", 8, 4), "the bool's byte"
        )

        self.assertIsNone(files["tpk.assets"]["l1"]["types"][0]["nodes"])
        tpk = objects[("tpk.assets", 1)]["l2"]
        self.assertEqual(
            tpk,
            {
                "root_type": "TextAsset",
                "source": "tpk",
                "error": None,
                "value": {"t": "dict", "v": [["m_Name", "name"], ["m_Script", "text"]]},
            },
        )

    def test_a_kind_comes_from_the_whole_message_not_the_files_strings(self):
        """A ReferencedObject whose class the file does not hold is `ref_type` (spec 4.3), whatever the class is
        named. The pure-Python reader's message names it (TypeTreeHelper.py:109), so review round 9 found the
        oracle reading a class named like another kind's message as that kind, and stopping the whole run on one
        named like the check_read message (it then reads the object again with check_read off)."""
        d = self.tmp / "in"
        d.mkdir()
        named = ["out of bounds", "Negative length read from TypeTree"]
        named += ["Expected to read 3 bytes, but only read 99 bytes", "read_u8 out of bounds", "Nothing"]
        known = S.Type(114, S.Node("Other", "Base", [S.Node("int", "x")]), ref_names=[b"Other", b"NS", b"Asm"])
        tree = S.Node("MyRefs", "Base", [S.ref_object("ref")])
        body = [struct.pack("<q", 1) + S.s32(cls) + S.s32("NS") + S.s32("Asm") for cls in named]
        # byte_size 0 is the pure-Python reader's, whose message names the class; the rest UnityPyBoost's.
        rows = [S.Obj(i + 1, 0, b, byte_size=0) for i, b in enumerate(body)]
        rows += [S.Obj(i + 11, 0, b) for i, b in enumerate(body)]
        (d / "refs.assets").write_bytes(S.serialized_file([S.Type(114, tree)], rows, ref_types=[known]))
        out = self.tmp / "refs.jsonl"
        self.assertEqual(dump(d, "-o", out), 0, "the run stops on none of them")
        self.assert_canonical(out)
        errors = {r["path_id"]: r["l2"]["error"] for r in records(out) if r["rec"] == "object"}
        self.assertEqual(sorted(errors), [*range(1, 6), *range(11, 16)])
        for path_id, error in errors.items():
            with self.subTest(path_id=path_id, note=error["note"]):
                self.assertEqual((error["kind"], error["bytes_read"]), ("ref_type", None))
        self.assertEqual(errors[3]["note"], f"ValueError: Referenced type not found: {named[2]} NS Asm")
        self.assertEqual(errors[13]["note"], "ValueError: Failed to get ref type node")

    def test_version_strings_pack_as_unitypy_packs_them(self):
        """Spec 4.2's version-packing rule and its failures, for SerializedFiles and bundles."""
        d = self.tmp / "in"
        d.mkdir()
        tree = S.Node("MyThing", "Base", [S.Node("int", "i")])

        def assets_with(version):
            return assets(tree, struct.pack("<i", 7), unity_version=version)

        files = {
            "v_major0.assets": assets_with(b"0.5.1f1"),  # packs as it is
            "v_wide.assets": assets_with(b"133091.1.1f1"),  # wider than u64 once packed
            "v_nomatch.assets": assets_with(b"abc"),  # fails whatever the fallback
            "v_empty.assets": assets_with(b""),  # wants a parent bundle, else the fallback
            "b6_wide": unityraw(6, [(b"one", b"tiny")], engine=b"133091.1.1f1"),  # compares as 2019
            "b6_spill": unityraw(6, [(b"one", b"tiny")], engine=b"2020.3.34f1024"),  # type byte 7
            "b6_major64k": unityraw(6, [(b"one", b"tiny")], engine=b"65536.1.1f1"),  # major 0 once masked
            "b5_major64k": unityraw(5, [(b"one", b"tiny")], engine=b"65536.1.1f1"),  # never parsed to load
            "b7": unityraw(7, [(b"one", b"tiny")]),  # the web/raw layout: no alignment
            # A 4,300-digit major: past 4,300 digits once packed, which the oracle once could not write (its
            # run stopped, review round 10).
            "v_4300.assets": assets_with(b"9" * 4300 + b".1.1f1"),
            "b6_4300": unityraw(6, [(b"one", b"tiny")], engine=b"9" * 4300 + b".1.1f1"),
        }
        for name, data in files.items():
            (d / name).write_bytes(data)
        wide = 133091 << 48 | 1 << 32 | 1 << 16 | 3 << 8 | 1
        self.assertGreater(wide, 2**64)
        widest = (10**4300 - 1) << 48 | 1 << 32 | 1 << 16 | 3 << 8 | 1
        for fallback in (None, "2021.3.2f1"):
            out = self.tmp / f"versions{fallback}.jsonl"
            with mock.patch.object(unitypy_dump.UnityPy.config, "FALLBACK_UNITY_VERSION", fallback):
                self.assertEqual(dump(d, "-o", out), 0)
            recs = {r["file"][0]: r for r in records(out) if r["rec"] == "file" and len(r["file"]) == 1}
            with self.subTest(fallback=fallback):
                self.assertEqual(recs["v_major0.assets"]["l1"]["version_packed"], 5 << 32 | 1 << 16 | 3 << 8 | 1)
                self.assertEqual(recs["v_wide.assets"]["l1"]["version_packed"], wide)
                self.assertEqual(recs["v_4300.assets"]["l1"]["version_packed"], widest)
                self.assertEqual(recs["b6_4300"]["l1"]["engine_version_packed"], widest)
                self.assertEqual(recs["v_nomatch.assets"]["error"]["note"], "ValueError: Invalid version string: abc")
                b6 = recs["b6_wide"]["l1"]
                self.assertEqual((b6["engine_version_packed"], b6["flags_class"]), (wide, "ArchiveFlagsOld"))
                self.assertEqual(recs["b6_spill"]["error"]["note"], "ValueError: 7 is not a valid UnityVersionType")
                self.assertEqual((recs["b7"]["l1"]["block_alignment"], recs["b7"]["l1"]["blocks"]), (False, None))
                self.assertEqual(recs["b5_major64k"]["l0"]["parsed_as"], "BundleFile")
                self.assert_canonical(out)
        # The oracle leaves the fallback unset: where UnityPy wants it, the input fails or the field is null.
        recs = {
            r["file"][0]: r
            for r in records(self.tmp / "versionsNone.jsonl")
            if r["rec"] == "file" and len(r["file"]) == 1
        }
        self.assertTrue(recs["v_empty.assets"]["error"]["note"].startswith("UnityVersionFallbackError"))
        self.assertTrue(recs["b6_major64k"]["error"]["note"].startswith("UnityVersionFallbackError"))
        self.assertIsNone(recs["b5_major64k"]["l1"]["engine_version_packed"])

    def test_tables_nodes_and_entries_follow_unitypy(self):
        """Spec 4.2-4.3: node rows, type-record defaults by format, object l1, truncated blocks, zip entries."""
        d = self.tmp / "in"
        d.mkdir()
        tree = S.Node("MyThing", "Base", [S.Node("int", "i")])
        nodes = S.Node(0x80000000, "Base", [S.Node(0x8000_0000 | 0x7FFF_FFF0, "x", version=-1, ref_hash=2**64 - 1)])
        files = {
            "nodes.assets": S.serialized_file([S.Type(114, nodes)], [S.Obj(1, 0, b"")]),
            "v15.assets": S.serialized_file(
                [S.Type(114, tree)], [S.Obj(1, 114, struct.pack("<i", 7), is_stripped=1)], version=15
            ),
            "v16.assets": S.serialized_file(
                [S.Type(114, tree)],
                [S.Obj(1, 0, struct.pack("<i", 7), script_type_index=3, is_stripped=-1)],
                version=16,
            ),
            "deps.assets": S.serialized_file([S.Type(114, tree, deps=[1, 2])], [S.Obj(1, 0, struct.pack("<i", 7))]),
            "nodeps.assets": S.serialized_file(
                [S.Type(49, deps=[1, 2])], [S.Obj(1, 0, S.s32("n") + S.s32("t"))], enable_type_tree=False
            ),
            "b6_truncated": unityraw(6, [(b"one", b"tiny"), (b"two", b"abc")])[:-2],
            "b5_zip": unityraw(5, [(b"z", b"PK\x03\x04\0" + bytes(200))]),
        }
        for name, data in files.items():
            (d / name).write_bytes(data)
        out = self.tmp / "tables.jsonl"
        self.assertEqual(dump(d, "-o", out), 0)
        self.assert_canonical(out)
        recs = records(out)
        files = {tuple(r["file"]): r for r in recs if r["rec"] == "file"}
        objects = {r["file"][0]: r for r in recs if r["rec"] == "object"}

        # Rows unpack as hBBIIiii(Q); a common-string offset UnityPy does not know is its decimal string.
        self.assertEqual(
            files[("nodes.assets",)]["l1"]["types"][0]["nodes"],
            [[0, "AABB", "Base", -1, 1, 0, None, 0, 0, 0], [1, "2147483632", "x", -1, -1, 0, None, 1, 0, 2**64 - 1]],
        )
        type_fields = ("is_stripped_type", "script_type_index", "type_dependencies")
        self.assertEqual([files[("v15.assets",)]["l1"]["types"][0][k] for k in type_fields], [None, -1, None])
        self.assertEqual([files[("v16.assets",)]["l1"]["types"][0][k] for k in type_fields], [False, 3, None])
        self.assertEqual(files[("deps.assets",)]["l1"]["types"][0]["type_dependencies"], [1, 2])
        self.assertIsNone(files[("nodeps.assets",)]["l1"]["types"][0]["type_dependencies"])
        self.assertEqual(
            (objects["v15.assets"]["l1"]["is_stripped"], objects["v16.assets"]["l1"]["is_stripped"]), (1, -1)
        )

        # A block reports the bytes actually read; a ZIP entry is kept raw.
        self.assertEqual(files[("b6_truncated",)]["l1"]["blocks"], [[7, 5, 0]])
        self.assertEqual(files[("b6_truncated", "two")]["l0"]["size"], 1)
        self.assertEqual(
            (files[("b5_zip", "z")]["l0"]["type"], files[("b5_zip", "z")]["l0"]["parsed_as"]), ("ZIP", "raw")
        )

    def test_externals_hold_only_the_members_their_format_reads(self):
        """Spec 4.2: below format 6 UnityPy reads no temp_empty, below 5 no guid or type; each is null, not a stop."""

        def old_format(version):
            # Formats below 9: the header, 128 bytes of data (detection needs
            # 128), then the metadata at the end: no types, no objects, one
            # external with the members its format reads.
            ext = b""
            if version >= 6:
                ext += b"tmp\0"
            if version >= 5:
                ext += bytes(range(16)) + struct.pack("<i", 2)
            ext += b"lib/x\0"
            meta = b"\0" + struct.pack("<ii", 0, 0) + struct.pack("<i", 1) + ext
            if version >= 5:
                meta += b"\0"  # userInformation
            size = 16 + 128 + len(meta)
            return struct.pack(">IIII", len(meta), size, version, 16) + bytes(128) + meta

        d = self.tmp / "in"
        d.mkdir()
        for version in (1, 4, 5, 6):
            (d / f"f{version}").write_bytes(old_format(version))
        out = self.tmp / "old.jsonl"
        # The whole run is dumped: no input stops it.
        self.assertEqual(dump(d, "-o", out), 0)
        self.assert_canonical(out)
        files = {r["file"][0]: r for r in records(out) if r["rec"] == "file"}
        guid = bytes(range(16)).hex()
        self.assertEqual(
            {name: (r["l0"]["parsed_as"], r["l1"]["externals"]) for name, r in files.items()},
            {
                "f1": ("SerializedFile", [[None, None, None, "lib/x"]]),
                "f4": ("SerializedFile", [[None, None, None, "lib/x"]]),
                "f5": ("SerializedFile", [[None, guid, 2, "lib/x"]]),
                "f6": ("SerializedFile", [["tmp", guid, 2, "lib/x"]]),
            },
        )

    def test_resave_aligns_after_every_aligned_array_element(self):
        """Spec 7: UnityPy's writer aligns after each aligned element; UnityPyBoost reads runs and aligns once."""
        d = self.tmp / "in"
        d.mkdir()

        def array(elem, array_meta=0):
            return S.Node(
                "vector", "arr", [S.Node("Array", "Array", [S.Node("int", "size"), elem], meta=array_meta, flags=1)]
            )

        after = S.Node("int", "after")
        i42 = struct.pack("<i", 42)
        u8s = struct.pack("<i", 3) + b"\1\2\3\0" + i42  # three bytes, then one alignment
        pair = S.Node("pair", "data", [S.Node("UInt8", "first"), S.Node("UInt8", "second")], meta=S.ALIGN)
        cases = {
            "u8_elements_aligned": ([array(S.Node("UInt8", "data", meta=S.ALIGN)), after], u8s),
            "s16_elements_aligned": (
                [array(S.Node("SInt16", "data", meta=S.ALIGN)), after],
                struct.pack("<ihh", 2, 1, 2) + i42,
            ),
            "pairs_aligned": ([array(pair), after], struct.pack("<i", 2) + b"\1\2\3\4" + i42),
            # Four-byte elements after an unaligned field: each ends off a boundary.
            "u32_elements_unaligned_start": (
                [S.Node("UInt8", "pre"), array(S.Node("UInt32", "data", meta=S.ALIGN)), after],
                b"\7" + struct.pack("<iII", 2, 1, 2) + b"\0\0\0" + i42,
            ),
            "u8_array_aligned": ([array(S.Node("UInt8", "data"), array_meta=S.ALIGN), after], u8s),
            "u8_one_element": (
                [array(S.Node("UInt8", "data", meta=S.ALIGN)), after],
                struct.pack("<i", 1) + b"\1\0\0\0" + i42,
            ),
            "strings_aligned": (
                [array(S.Node("string", "data", meta=S.ALIGN)), after],
                struct.pack("<i", 2) + S.s32("a") + S.s32("bc") + i42,
            ),
        }
        for name, (fields, data) in cases.items():
            (d / f"{name}.assets").write_bytes(assets(S.Node("MyThing", "Base", fields), data))
        out = self.tmp / "align.jsonl"
        self.assertEqual(dump(d, "-o", out), 0)
        self.assert_canonical(out)
        objects = {r["file"][0][: -len(".assets")]: r for r in records(out) if r["rec"] == "object"}
        values = {name: dict(r["l2"]["value"]["v"]) for name, r in objects.items()}
        self.assertEqual(values["u8_elements_aligned"]["arr"], {"t": "u8s", "hex": "010203"})
        self.assertEqual(values["u32_elements_unaligned_start"]["after"], 42)
        l3 = {
            name: [r["l3"][k] for k in ("raw_size", "resave", "resave_size", "first_diff")]
            for name, r in objects.items()
        }
        self.assertEqual(
            l3,
            {
                # 03000000 010203 00 -> 03000000 01000000 02000000 03000000
                "u8_elements_aligned": [12, "differs", 20, 5],
                "s16_elements_aligned": [12, "differs", 16, 6],
                "pairs_aligned": [12, "differs", 16, 6],
                # Same length: the padding moves from after the array to after its first element.
                "u32_elements_unaligned_start": [20, "differs", 20, 9],
                # The flag on the Array node: one alignment after the array on both sides.
                "u8_array_aligned": [12, "equal", None, None],
                "u8_one_element": [12, "equal", None, None],
                # UnityPyBoost reads strings element by element, as the writer writes them.
                "strings_aligned": [24, "equal", None, None],
            },
        )

    def test_type_indices_and_blob_trees_follow_python(self):
        """Spec 4.2-4.3: a negative type_id indexes from the end; blob rows become a tree as parse_blob builds it."""
        d = self.tmp / "in"
        d.mkdir()
        thing_a = S.Type(114, S.Node("ThingA", "Base", [S.Node("int", "a")]))
        thing_b = S.Type(115, S.Node("ThingB", "Base", [S.Node("int", "b")]))
        two_ints = struct.pack("<ii", 1, 2)
        files = {
            "negative.assets": S.serialized_file([thing_a, thing_b], [S.Obj(1, -1, struct.pack("<i", 7))]),
            "too_negative.assets": S.serialized_file([thing_a], [S.Obj(1, -2, struct.pack("<i", 7))]),
            "two_roots.assets": S.serialized_file(
                [S.Type(114, [(0, "Base1", "Base"), (1, "int", "a"), (0, "Other", "Base"), (1, "int", "z")])],
                [S.Obj(1, 0, two_ints)],
            ),
            "jump.assets": S.serialized_file(
                [S.Type(114, [(0, "Base", "Base"), (2, "int", "deep"), (1, "int", "mid")])], [S.Obj(1, 0, two_ints)]
            ),
        }
        for name, data in files.items():
            (d / name).write_bytes(data)
        out = self.tmp / "trees.jsonl"
        self.assertEqual(dump(d, "-o", out), 0)
        self.assert_canonical(out)
        recs = records(out)
        files = {r["file"][0]: r for r in recs if r["rec"] == "file"}
        objects = {r["file"][0]: r for r in recs if r["rec"] == "object"}

        neg = objects["negative.assets"]
        self.assertEqual((neg["l1"]["type_id"], neg["l1"]["class_id"], neg["l2"]["root_type"]), (-1, 115, "ThingB"))
        self.assertEqual(neg["l2"]["value"], {"t": "dict", "v": [["b", 7]]})
        self.assertEqual(files["too_negative.assets"]["error"]["note"], "IndexError: list index out of range")

        # Only the first root's subtree is kept, so the value reads 4 of the 8 bytes.
        two = objects["two_roots.assets"]["l2"]
        self.assertEqual(
            [row[:3] for row in files["two_roots.assets"]["l1"]["types"][0]["nodes"]],
            [[0, "Base1", "Base"], [1, "int", "a"]],
        )
        self.assertEqual(
            (two["value"], two["error"]["kind"], two["error"]["bytes_read"]), (ints_value(a=1), "check_read", 4)
        )

        # A rise of two levels nests one level down; the fall to 1 stays under the root. Rows keep their levels.
        self.assertEqual([row[0] for row in files["jump.assets"]["l1"]["types"][0]["nodes"]], [0, 2, 1])
        self.assertEqual(objects["jump.assets"]["l2"]["value"], ints_value(deep=1, mid=2))

    def test_old_formats_record_what_they_lack_as_null(self):
        """Spec 4.2: below formats 8, 7, 6 and 5 the members a format lacks are null, externals' unset slots too."""
        d = self.tmp / "in"
        d.mkdir()
        tree = S.Node("TextAsset", "Base", [S.Node("int", "a", byte_size=4)], byte_size=4)
        for version in (4, 5, 6, 7, 8):
            data = S.serialized_file(
                [S.Type(49, tree)],
                [S.Obj(7, 49, struct.pack("<i", 5))],
                version=version,
                unity_version=b"4.7.2f1",
                externals=[S.External(b"library/x", b"tmp", bytes(range(16)), 2)],
                user_information=b"user",
            )
            (d / f"v{version}.assets").write_bytes(data)
        out = self.tmp / "old.jsonl"
        self.assertEqual(dump(d, "-o", out), 0, "the dumper reads every old format")
        self.assert_canonical(out)
        files = {r["file"][0]: r["l1"] for r in records(out) if r["rec"] == "file"}
        guid = bytes(range(16)).hex()
        self.assertEqual(
            {
                name: (l1["externals"], l1["user_information"], l1["version_packed"] is None)
                for name, l1 in files.items()
            },
            {
                "v4.assets": ([[None, None, None, "library/x"]], None, True),
                "v5.assets": ([[None, guid, 2, "library/x"]], "user", True),
                "v6.assets": ([["tmp", guid, 2, "library/x"]], "user", True),
                "v7.assets": ([["tmp", guid, 2, "library/x"]], "user", False),
                "v8.assets": ([["tmp", guid, 2, "library/x"]], "user", False),
            },
        )
        self.assertEqual([files[f"v{v}.assets"]["target_platform"] for v in (7, 8)], [None, 19])
        self.assertEqual({files[f"v{v}.assets"]["unity_version"] for v in (4, 5, 6)}, {"2.5.0f5"})

    def test_input_names_come_from_the_bytes_as_given(self):
        """Names do not depend on the locale, and a symlink is named after itself, in either input mode."""
        d = self.tmp / "in"
        d.mkdir()
        for raw in ("café".encode(), b"bad\xffname", b"b"):
            with open(os.path.join(os.fsencode(d), raw), "wb") as fh:
                fh.write(b"tiny")
        c_locale = {"PYTHONUTF8": "0", "PYTHONCOERCECLOCALE": "0", "LC_ALL": "C"}
        outs = []
        for env in ({}, c_locale):
            out = self.tmp / f"names{len(outs)}.jsonl"
            proc = dump_process(d, "-o", out, env=env)
            self.assertEqual(proc.returncode, 0, proc.stderr[-2000:])
            outs.append(out.read_bytes())
        self.assertEqual(outs[0], outs[1], "the C locale names the inputs the same")
        self.assertEqual(
            [r["file"] for r in records(self.tmp / "names0.jsonl") if r["rec"] == "file"],
            [["b"], [{"t": "str", "hex": b"bad\xffname".hex()}], ["café"]],
        )

        # The name rule follows the link's name, not its target's: thing.resS stays raw.
        (self.tmp / "store").mkdir()
        (self.tmp / "view").mkdir()
        tree = S.Node("MyThing", "Base", [S.Node("int", "i")])
        (self.tmp / "store" / "0a1b2c").write_bytes(assets(tree, struct.pack("<i", 7)))
        (self.tmp / "view" / "thing.resS").symlink_to("../store/0a1b2c")
        single, folder = self.tmp / "single.jsonl", self.tmp / "folder.jsonl"
        self.assertEqual(dump(self.tmp / "view" / "thing.resS", "-o", single), 0)
        self.assertEqual(dump(self.tmp / "view", "-o", folder), 0)
        self.assertEqual(single.read_bytes(), folder.read_bytes())
        rec = records(single)[1]
        self.assertEqual(
            (rec["file"], rec["l0"]["type"], rec["l0"]["parsed_as"]), (["thing.resS"], "AssetsFile", "raw")
        )

    def test_input_names_are_hex_tagged_when_not_utf8(self):
        d = self.tmp / "in"
        d.mkdir()
        with open(os.path.join(os.fsencode(d), b"bad\xffname"), "wb") as fh:
            fh.write(b"tiny")
        (d / "ok").write_bytes(b"tiny")
        out = self.tmp / "names.jsonl"
        self.assertEqual(dump(d, "-o", out), 0)
        self.assertEqual(
            [r["file"] for r in records(out) if r["rec"] == "file"],
            [[{"t": "str", "hex": b"bad\xffname".hex()}], ["ok"]],
        )
        self.assert_canonical(out)


if __name__ == "__main__":
    unittest.main()
