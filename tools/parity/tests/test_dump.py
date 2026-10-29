"""unitypy_dump.py end to end: UnityPy's samples, determinism, L0 detection, the CLI rules.

The samples are read at run time from $UNITYRS_UNITYPY_SAMPLES (UnityPy's
tests/samples at commit 417d595), else from wherever the corpus index
($UNITYRS_CORPUS_INDEX, default /corpus/index.json) lists its unitypy-sample
entries. They are game bundles and are never copied into this repository.
Needs the oracle stack; skipped elsewhere.
"""

import contextlib
import gzip
import hashlib
import io
import json
import os
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import unittest
import zipfile
from collections import Counter
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

try:
    import unitypy_dump
except ImportError:  # pragma: no cover - outside the oracle image
    unitypy_dump = None

import compare  # noqa: E402
import synth as S  # noqa: E402

SAMPLES = {
    "atlas_test": "61ee5afb5f5c64f0a4f65f039d09b2509198363e713423289c895067ac215177",
    "banner_1": "6bd0c3bc36c97e5f1cd2de71bb58477b6c32ac2c254a5705884306c56b94417f",
    "char_118_yuki.ab": "6198cb71c7a820256208332bbd375d21b18ed1a36a12ad04be1eef2111b014a1",
    "xinzexi_2_n_tex": "a9f6e6bba7110cce647ef752d66655d13ce5850b013a60d80d4e8b08953e02ea",
    "xinzexi_2_n_tex_mesh": "8bb2082b9586b3f8f3bad2d22d33346ec0492053f2a76323aa8b4b4047368ca8",
}


def samples_dir():
    """The pinned samples' folder: $UNITYRS_UNITYPY_SAMPLES, else the one the corpus index names."""
    candidates = [os.environ.get("UNITYRS_UNITYPY_SAMPLES")]
    index = Path(os.environ.get("UNITYRS_CORPUS_INDEX", "/corpus/index.json"))
    try:
        entries = json.loads(index.read_text())["files"]
        candidates += sorted(
            {str((index.parent / e["path"]).parent) for e in entries if e.get("kind") == "unitypy-sample"}
        )
    except (OSError, ValueError, KeyError, TypeError):
        pass
    for candidate in candidates:
        if candidate and all((Path(candidate) / name).is_file() for name in SAMPLES):
            return Path(candidate)
    return None


def unityraw(fmt, entries, player=b"5.x.x", engine=b"5.6.7f1"):
    """A synthetic UnityRaw bundle holding uncompressed entries [(name bytes, data)].

    Format 6 takes UnityPy's UnityFS layout (read_fs, plus one byte after the
    flags); every other format the web/raw layout, with a hash and crc from 4.
    """
    head = b"UnityRaw\0" + struct.pack(">I", fmt) + player + b"\0" + engine + b"\0"
    data = b"".join(p for _, p in entries)
    if fmt == 6:
        rows, off = b"", 0
        for name, payload in entries:
            rows += struct.pack(">qqI", off, len(payload), 4) + name + b"\0"
            off += len(payload)
        info = bytes(16) + struct.pack(">iIIHi", 1, len(data), len(data), 0, len(entries)) + rows
        return head + struct.pack(">qIII", 0, len(info), len(info), 0x40) + b"\0" + info + data
    rows, off = b"", 4 + sum(len(n) + 9 for n, _ in entries)
    for name, payload in entries:
        rows += name + b"\0" + struct.pack(">II", off, len(payload))
        off += len(payload)
    block = struct.pack(">i", len(entries)) + rows + data
    size = len(head) + 52
    fields = struct.pack(">IIIIiIIII", 0xC0FFEE, 0, size, 1, 1, len(block), len(block), size + len(block), 0)
    return head + bytes(range(16)) + fields + block


def records(path):
    opener = gzip.open if str(path).endswith(".gz") else open
    with opener(path, "rt", encoding="utf-8") as fh:
        return [json.loads(line) for line in fh]


def dump(*argv):
    """Run the dumper in-process, quietly (UnityPy prints load errors); returns its exit status."""
    with contextlib.redirect_stderr(io.StringIO()), contextlib.redirect_stdout(io.StringIO()):
        return unitypy_dump.main(["-q", *map(str, argv)])


def dump_process(*argv, env=None):
    """Run the dumper as its own process, for inputs that could take the test runner down with it."""
    return subprocess.run(
        [sys.executable, "-B", str(HERE / "unitypy_dump.py"), "-q", *map(str, argv)],
        capture_output=True,
        env={**os.environ, **(env or {})},
        timeout=300,
    )


def assets(tree, data, **kw):
    """A format-22 SerializedFile with one MonoBehaviour-class object (path_id 1) typed by `tree`."""
    return S.serialized_file([S.Type(114, tree)], [S.Obj(1, 0, data)], **kw)


@unittest.skipIf(unitypy_dump is None, "needs UnityPy (the oracle image)")
class DumpTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-parity-"))
        self.addCleanup(shutil.rmtree, self.tmp)

    def assert_canonical(self, path):
        """Every line the oracle wrote is in the canonical form, and the dump compares clean with itself."""
        report, *_ = compare.run(str(path), str(path))
        self.assertEqual(report.as_dict()["by_path"], [])
        self.assertEqual(report.counts["identical"], len(records(path)) - 1)

    def sample_args(self):
        src = samples_dir()
        if src is None:
            self.skipTest("UnityPy samples not found; set UNITYRS_UNITYPY_SAMPLES")
        for name, sha in SAMPLES.items():
            self.assertEqual(
                hashlib.sha256((src / name).read_bytes()).hexdigest(), sha, f"{name} is not the pinned sample"
            )
        args = [src]
        for name in SAMPLES:
            args += ["--name", name]
        return args

    def test_samples(self):
        out = self.tmp / "s.jsonl"
        self.assertEqual(dump(*self.sample_args(), "-o", out), 0)
        recs = records(out)
        head, end = recs[0], recs[-1]
        self.assertEqual(
            (head["format"], head["version"], head["levels"]), ("unityrs-canonical-dump", 1, ["L0", "L1", "L2", "L3"])
        )
        self.assertTrue(head["producer"]["boost"])
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
        }
        for name, data in files.items():
            (d / name).write_bytes(data)
        wide = 133091 << 48 | 1 << 32 | 1 << 16 | 3 << 8 | 1
        self.assertGreater(wide, 2**64)
        for fallback in (None, "2021.3.2f1"):
            out = self.tmp / f"versions{fallback}.jsonl"
            with mock.patch.object(unitypy_dump.UnityPy.config, "FALLBACK_UNITY_VERSION", fallback):
                self.assertEqual(dump(d, "-o", out), 0)
            recs = {r["file"][0]: r for r in records(out) if r["rec"] == "file" and len(r["file"]) == 1}
            with self.subTest(fallback=fallback):
                self.assertEqual(recs["v_major0.assets"]["l1"]["version_packed"], 5 << 32 | 1 << 16 | 3 << 8 | 1)
                self.assertEqual(recs["v_wide.assets"]["l1"]["version_packed"], wide)
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

    def test_a_dead_or_raising_input_process_is_one_crash_record(self):
        """An input whose process is killed (the OOM killer) or raises is recorded, and the dump goes on."""
        d = self.tmp / "in"
        d.mkdir()
        for name in "abcd":
            (d / name).write_bytes(b"tiny")
        real = unitypy_dump.dump_input

        def failing(job):
            if job[1] == "b":
                os.kill(os.getpid(), signal.SIGKILL)
            if job[1] == "c":
                raise RuntimeError("boom \udcff")
            return real(job)

        outs = []
        for jobs in ("1", "3"):
            out = self.tmp / f"dead{jobs}.jsonl"
            with mock.patch.object(unitypy_dump, "dump_input", failing):
                self.assertEqual(dump(d, "-o", out, "-j", jobs), 0)
            outs.append(out.read_bytes())
            self.assert_canonical(out)
            recs = {r["file"][0]: r for r in records(out) if r["rec"] == "file"}
            self.assertEqual(sorted(recs), ["a", "b", "c", "d"])
            for name, why in (("b", "died of SIGKILL"), ("c", "raised RuntimeError: boom \\udcff")):
                rec = recs[name]
                self.assertEqual(
                    (rec["l0"]["type"], rec["l0"]["parsed_as"], rec["l1"]), ("ResourceFile", "error", None)
                )
                self.assertEqual(rec["error"]["kind"], "crash")
                self.assertIn(why, rec["error"]["note"])
            self.assertEqual((recs["a"]["error"], recs["d"]["error"]), (None, None))
        self.assertEqual(outs[0], outs[1])
        self.assertEqual(sorted(p.name for p in self.tmp.iterdir()), ["dead1.jsonl", "dead3.jsonl", "in"])

    def test_a_unitypyboost_crash_costs_only_its_input(self):
        """A typetree node name or type that is not UTF-8 crashes UnityPyBoost while the file loads (spec 7)."""
        d = self.tmp / "in"
        d.mkdir()
        good = assets(S.Node("MyThing", "Base", [S.Node("int", "i")]), struct.pack("<i", 7))
        (d / "a.assets").write_bytes(good)
        (d / "b_name.assets").write_bytes(assets(S.Node("MyThing", "Base", [S.Node("int", b"a\xffb")]), bytes(4)))
        (d / "c_type.assets").write_bytes(assets(S.Node("MyThing", "Base", [S.Node(b"in\xfft", "i")]), bytes(4)))
        (d / "d.assets").write_bytes(good)
        outs = []
        for jobs in ("1", "2"):
            out = self.tmp / f"boost{jobs}.jsonl"
            proc = dump_process(d, "-o", out, "-j", jobs)
            self.assertEqual(proc.returncode, 0, proc.stderr[-2000:])
            outs.append(out.read_bytes())
            recs = {r["file"][0]: r for r in records(out) if r["rec"] == "file"}
            for name, signame in (("b_name.assets", "SIGABRT"), ("c_type.assets", "SIGSEGV")):
                self.assertEqual(recs[name]["l0"]["parsed_as"], "error")
                self.assertEqual(recs[name]["error"]["kind"], "crash")
                self.assertIn(f"died of {signame}", recs[name]["error"]["note"])
            objects = [r for r in records(out) if r["rec"] == "object"]
            self.assertEqual(
                [(r["file"], r["l2"]["value"]) for r in objects],
                [
                    (["a.assets"], {"t": "dict", "v": [["i", 7]]}),
                    (["d.assets"], {"t": "dict", "v": [["i", 7]]}),
                ],
            )
            self.assert_canonical(out)
        self.assertEqual(outs[0], outs[1])

    def test_a_load_error_quoting_bad_bytes_is_a_parse_record_in_any_environment(self):
        """UnityPy's message quotes the bytes raw and its print of it can raise; neither may cost the dump."""
        d = self.tmp / "in"
        d.mkdir()
        (d / "bad_version.assets").write_bytes(
            assets(S.Node("MyThing", "Base", [S.Node("int", "i")]), bytes(4), unity_version=b"\xff\xfe")
        )
        (d / "ok").write_bytes(b"tiny")
        outs = []
        for env in ({}, {"PYTHONIOENCODING": "utf-8:strict"}):
            out = self.tmp / f"v{len(outs)}.jsonl"
            proc = dump_process(d, "-o", out, env=env)
            self.assertEqual(proc.returncode, 0, proc.stderr[-2000:])
            self.assertEqual(proc.stdout, b"")
            outs.append(out.read_bytes())
            rec = records(out)[1]
            self.assertEqual((rec["l0"]["parsed_as"], rec["error"]["kind"]), ("error", "parse"))
            self.assertEqual(rec["error"]["note"], "ValueError: Invalid version string: \\udcff\\udcfe")
            self.assert_canonical(out)
        self.assertEqual(outs[0], outs[1])

    def test_empty_class_references_never_abort_the_oracle(self):
        """UnityPyBoost drops a reference to None per empty-class ReferencedObject (spec 7)."""
        n = 2 * sys.getrefcount(None)  # more than one process holds
        empty = struct.pack("<q", 0) + S.s32("") * 3
        tree = S.Node("MyRefs", "Base", [S.vec("refs", S.ref_object())])
        (self.tmp / "refs.assets").write_bytes(assets(tree, struct.pack("<i", n) + empty * n, ref_types=[]))
        out = self.tmp / "refs.jsonl"
        proc = dump_process(self.tmp / "refs.assets", "-o", out)
        self.assertEqual(proc.returncode, 0, proc.stderr[-2000:])
        obj = records(out)[2]
        self.assertIsNone(obj["l2"]["error"])
        refs = dict(obj["l2"]["value"]["v"])["refs"]
        self.assertEqual(len(refs), n)
        self.assertEqual(
            refs[0],
            {"t": "dict", "v": [["rid", 0], ["type", {"t": "dict", "v": [["class", ""], ["ns", ""], ["asm", ""]]}]]},
        )
        # UnityPy's writer wants the data the reader left out (spec 7).
        self.assertEqual(
            (obj["l3"]["resave"], obj["l3"]["note"]), ("error", "ValueError: SerializedFile has no ref_types")
        )

    def test_refuses_without_the_pinned_oracle(self):
        tiny = self.tmp / "tiny"
        tiny.write_bytes(b"tiny")
        out = self.tmp / "x.jsonl"
        for patch in (
            mock.patch.object(unitypy_dump, "HAVE_BOOST", False),
            mock.patch.object(unitypy_dump.UnityPy, "__version__", "1.24.0"),
        ):
            with self.subTest(patch.attribute):
                with patch, contextlib.redirect_stderr(io.StringIO()) as err, self.assertRaises(SystemExit):
                    unitypy_dump.main([str(tiny), "-o", str(out), "-q"])
                self.assertIn("not the pinned oracle", err.getvalue())
                self.assertFalse(out.exists())

    def test_index_mode_and_sha_check(self):
        src = samples_dir()
        if src is None:
            self.skipTest("UnityPy samples not found; set UNITYRS_UNITYPY_SAMPLES")
        root = self.tmp / "corpus"
        (root / "s").mkdir(parents=True)
        os.symlink(src / "banner_1", root / "s" / "banner_1")
        os.symlink(src / "atlas_test", root / "s" / "atlas_test")
        index = {
            "files": [
                {"path": "s/banner_1", "kind": "unitypy-sample", "sha256": SAMPLES["banner_1"]},
                {"path": "s/atlas_test", "kind": "unitypy-sample", "sha256": "0" * 64},
                {"path": "m.json", "kind": "manifest"},
            ]
        }
        (root / "index.json").write_text(json.dumps(index))
        out, stats = self.tmp / "i.jsonl", self.tmp / "i.stats.json"
        dump("--index", root / "index.json", "-o", out, "--stats", stats)
        tops = [r["file"] for r in records(out) if r["rec"] == "file" and len(r["file"]) == 1]
        self.assertEqual(tops, [["s/atlas_test"], ["s/banner_1"]])
        self.assertEqual(json.loads(stats.read_text())["summary"]["index_sha256_mismatch"], 1)

    def test_levels_leave_the_rest_null(self):
        args = self.sample_args()
        out = self.tmp / "l01.jsonl"
        dump(*args, "-o", out, "--levels", "L0,L1")
        recs = records(out)
        self.assertEqual(recs[0]["levels"], ["L0", "L1"])
        objects = [r for r in recs if r["rec"] == "object"]
        self.assertTrue(objects and all(r["l1"] and r["l2"] is None and r["l3"] is None for r in objects))
        out = self.tmp / "l0.jsonl"
        dump(*args, "-o", out, "--levels", "L0")
        recs = records(out)
        self.assertTrue(all(r.get("l1") is None for r in recs if r["rec"] in ("file", "object")))
        for bad in ("L0,L3", "l2", "L0,L9", "L1,L2", ""):
            with self.subTest(levels=bad), contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                unitypy_dump.main([*map(str, args), "-o", str(self.tmp / "x.jsonl"), "--levels", bad])
            self.assertFalse((self.tmp / "x.jsonl").exists())

    def test_refuses_output_inside_the_repository(self):
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            unitypy_dump.main([str(HERE), "-o", str(HERE / "dump.jsonl")])
        self.assertFalse((HERE / "dump.jsonl").exists())

    def test_refuses_stats_inside_the_repository(self):
        """The stats list every input path, so they are kept out of the repository like the dump."""
        tiny, stats = self.tmp / "tiny", HERE / "stats.json"
        tiny.write_bytes(b"tiny")
        try:
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                unitypy_dump.main([str(tiny), "-o", str(self.tmp / "x.jsonl"), "--stats", str(stats)])
            self.assertFalse(stats.exists())
            self.assertFalse((self.tmp / "x.jsonl").exists())
        finally:
            stats.unlink(missing_ok=True)


if __name__ == "__main__":
    unittest.main()
