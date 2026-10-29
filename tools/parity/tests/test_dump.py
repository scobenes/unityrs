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
import pickle
import re
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import time
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

import canonical as C  # noqa: E402
import compare  # noqa: E402
import oracle_env  # noqa: E402
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


def ints_value(**fields):
    """The encoded value of a struct of ints."""
    return {"t": "dict", "v": [[k, v] for k, v in fields.items()]}


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

    def test_a_killed_input_process_is_one_crash_record(self):
        """An input whose process is killed (the OOM killer), before or while it sends its result, is recorded."""
        d = self.tmp / "in"
        d.mkdir()
        for name in "abcd":
            (d / name).write_bytes(b"tiny")
        with open(os.path.join(os.fsencode(d), b"e\xff"), "wb") as fh:
            fh.write(b"tiny")
        real_dump, real_send = unitypy_dump.dump_input, unitypy_dump._send

        def killed(job):
            if job[1] in ("b", "e\udcff"):
                os.kill(os.getpid(), signal.SIGKILL)
            return real_dump(job)

        def killed_mid_send(conn, result):
            """c: the message header and half the payload reach the pipe, then SIGKILL."""
            if result[0] == "ok" and result[1][0] == "c":
                payload = pickle.dumps(result)
                os.write(conn.fileno(), struct.pack("!i", len(payload)) + payload[: len(payload) // 2])
                os.kill(os.getpid(), signal.SIGKILL)
            real_send(conn, result)

        tiny = {"type": "ResourceFile", "parsed_as": "error", "size": 4, "sha256": hashlib.sha256(b"tiny").hexdigest()}
        outs = []
        for jobs in ("1", "3"):
            out = self.tmp / f"dead{jobs}.jsonl"
            with (
                mock.patch.object(unitypy_dump, "dump_input", killed),
                mock.patch.object(unitypy_dump, "_send", killed_mid_send),
            ):
                self.assertEqual(dump(d, "-o", out, "-j", jobs), 0)
            outs.append(out.read_bytes())
            self.assert_canonical(out)
            recs = [r for r in records(out) if r["rec"] == "file"]
            e_name = {"t": "str", "hex": b"e\xff".hex()}
            self.assertEqual([r["file"] for r in recs], [["a"], ["b"], ["c"], ["d"], [e_name]])
            for rec in (recs[1], recs[2], recs[4]):
                self.assertEqual((rec["l0"], rec["l1"], rec["error"]["kind"]), (tiny, None, "crash"))
                self.assertIn("died of SIGKILL", rec["error"]["note"])
            self.assertEqual((recs[0]["error"], recs[3]["error"]), (None, None))
        self.assertEqual(outs[0], outs[1])
        self.assertEqual(sorted(p.name for p in self.tmp.iterdir()), ["dead1.jsonl", "dead3.jsonl", "in"])

    def test_finished_results_wait_in_bounded_number(self):
        """While the first input is slow, at most 2 x -j inputs start, so held results cannot grow without limit."""
        d = self.tmp / "in"
        d.mkdir()
        for name in "abcdefgh":
            (d / name).write_bytes(b"tiny")
        log, seen = self.tmp / "started", self.tmp / "seen"
        real = unitypy_dump.dump_input

        def slow_first(job):
            with open(log, "a") as fh:
                fh.write(job[1] + "\n")
            if job[1] == "a":
                time.sleep(2)
                seen.write_text(log.read_text())
            return real(job)

        with mock.patch.object(unitypy_dump, "dump_input", slow_first):
            self.assertEqual(dump(d, "-o", self.tmp / "o.jsonl", "-j", "2"), 0)
        self.assertLessEqual(set(seen.read_text().split()), set("abcd"))

    def test_the_dumpers_own_failure_stops_the_run(self):
        """A raise in the dumper, or a process that ends without a result and no signal, is no input's outcome."""
        d = self.tmp / "in"
        d.mkdir()
        tree = S.Node("MyThing", "Base", [S.Node("int", "i")])
        for name in ("a.assets", "b.assets", "c.assets"):
            (d / name).write_bytes(assets(tree, struct.pack("<i", 7)))
        real_send = unitypy_dump._send

        def buggy_l1(sf):
            raise AttributeError("'SerializedFile' object has no attribute '_m_target_platform'")

        def exit_without_result(conn, result):
            if result[1][0] == "b.assets":
                os._exit(1)
            real_send(conn, result)

        cases = {
            "a bug in the record code": (
                mock.patch.object(unitypy_dump, "serialized_l1", buggy_l1),
                "the dumper raised",
            ),
            "an exit without a result": (
                mock.patch.object(unitypy_dump, "_send", exit_without_result),
                "exited with status 1 and no result",
            ),
            # UnityPy read the value, and the oracle's own encoder refuses it: no `encode`
            # record (spec 4.3), the run stops naming the input and the object.
            "the canonical encoder refusing UnityPy's value": (
                mock.patch.object(C.ValueEncoder, "encode", side_effect=C.EncodeError("float for an int node")),
                "'a.assets': the dumper raised",
            ),
        }
        for label, (patch, message) in cases.items():
            for jobs in ("1", "2"):
                with self.subTest(label, jobs=jobs), patch, contextlib.redirect_stderr(io.StringIO()) as err:
                    with contextlib.redirect_stdout(io.StringIO()):
                        rc = unitypy_dump.main([str(d), "-o", str(self.tmp / "x.jsonl"), "-q", "-j", jobs])
                    self.assertEqual(rc, 1)
                    self.assertIn(message, err.getvalue())
                    if "encoder" in label:
                        self.assertIn(
                            'DumperError: file ["a.assets"] path_id 1: the canonical encoder refused the value '
                            "UnityPy read (float for an int node)",
                            err.getvalue(),
                        )
                    self.assertIn("no dump written", err.getvalue())
                    self.assertEqual(sorted(p.name for p in self.tmp.iterdir()), ["in"])
        # A build None cannot be pinned in is refused before any input runs.
        unpinnable = mock.patch.object(unitypy_dump, "pin_problem", lambda: "None cannot be pinned")
        with unpinnable, contextlib.redirect_stderr(io.StringIO()) as err, self.assertRaises(SystemExit):
            unitypy_dump.main([str(d), "-o", str(self.tmp / "x.jsonl"), "-q"])
        self.assertIn("None cannot be pinned", err.getvalue())

        # Every input crashing says more about the machine than the inputs: the dump is written, the exit is 3.
        def all_killed(job):
            os.kill(os.getpid(), signal.SIGKILL)

        with mock.patch.object(unitypy_dump, "dump_input", all_killed):
            self.assertEqual(dump(d, "-o", self.tmp / "all.jsonl", "-j", "2"), 3)
        self.assertEqual(
            [r["error"]["kind"] for r in records(self.tmp / "all.jsonl") if r["rec"] == "file"], ["crash"] * 3
        )

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
        # More references than None has in a fresh dumper process, measured there: this process's count
        # says nothing about that one. Bounded, so a surprise cannot grow the file without limit.
        probe = f"import sys; sys.path.insert(0, {str(HERE)!r}); import unitypy_dump; print(sys.getrefcount(None))"
        n = 2 * int(subprocess.run([sys.executable, "-B", "-c", probe], capture_output=True, check=True).stdout)
        self.assertLess(n, 1_000_000)
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
        installed = C.installed_packages()
        without_numpy = {k: v for k, v in installed.items() if k != "numpy"}
        for label, patch, said in (
            ("no Boost", mock.patch.object(unitypy_dump, "HAVE_BOOST", False), "UnityPyBoost is not loaded"),
            ("UnityPy", mock.patch.object(unitypy_dump.UnityPy, "__version__", "1.24.0"), "UnityPy is 1.24.0"),
            # Another build of UnityPyBoost beside UnityPy 1.25.2.
            ("Boost build", mock.patch.object(unitypy_dump, "BOOST_SHA256", "0" * 64), "UnityPyBoost has sha256 000"),
            # numpy absent: UnityPy then renders sprites with its pure-Python solver.
            ("numpy", mock.patch.object(C, "installed_packages", return_value=without_numpy), "numpy is not installed"),
            (
                "Pillow",
                mock.patch.object(C, "installed_packages", return_value={**installed, "pillow": "10.0.0"}),
                "pillow is 10.0.0, not the pinned 9.5.0",
            ),
            # Every version right, but a file that is not the pinned wheel's (test_refuses_a_patched_unitypy).
            (
                "a wheel's files",
                mock.patch.object(oracle_env, "installed_wheel_problems", return_value=(("numpy: x hashes as y",), {})),
                "numpy: x hashes as y (oracle_records.json)",
            ),
        ):
            with self.subTest(label):
                with patch, contextlib.redirect_stderr(io.StringIO()) as err, self.assertRaises(SystemExit):
                    unitypy_dump.main([str(tiny), "-o", str(out), "-q"])
                self.assertIn("not the pinned oracle", err.getvalue())
                self.assertIn(said, err.getvalue())
                self.assertFalse(out.exists())

    def test_refuses_a_patched_unitypy(self):
        """A copy of UnityPy with one line changed, imported before the installed one, is refused.

        With its dist-info the copy is the distribution Python finds, and its
        edited file no longer hashes as its RECORD says; without one, the
        installed distribution checks out, but `UnityPy` imports from
        elsewhere. A faithful copy with its dist-info passes, so the line is
        what is refused. The same edit in a package linked in beside the
        module, which Python imports in the module's place, is refused as a
        symlink. The version and the UnityPyBoost binary are the pinned ones
        throughout.
        """
        tiny = self.tmp / "tiny"
        tiny.write_bytes(b"tiny")
        site = Path(unitypy_dump.UnityPy.__file__).resolve().parents[1]
        info = site / f"unitypy-{C.ORACLE_UNITYPY}.dist-info"
        self.assertTrue(info.is_dir())

        def patched(text):
            self.assertEqual(text.count("is_stripped = None"), 1)
            return text.replace("is_stripped = None", "is_stripped = 7")

        def edit(files):
            reader = files / "ObjectReader.py"
            reader.write_text(patched(reader.read_text(encoding="utf-8")), encoding="utf-8")

        def link(files):
            # The patched module as a package elsewhere, its relative imports one level deeper.
            package = files.parents[2] / "linked" / "ObjectReader"
            package.mkdir(parents=True)
            text = re.sub(r"^(\s*from \.)", r"\1.", (files / "ObjectReader.py").read_text(encoding="utf-8"), flags=re.M)
            (package / "__init__.py").write_text(patched(text), encoding="utf-8")
            (files / "ObjectReader").symlink_to(package, target_is_directory=True)

        for n, (label, with_info, change, code, said) in enumerate(
            (
                ("a faithful copy with its dist-info", True, None, 0, ""),
                ("a patched copy with its dist-info", True, edit, 2, "UnityPy/files/ObjectReader.py hashes as"),
                ("a patched copy alone", False, edit, 2, "module UnityPy imports from"),
                ("a patched package linked in", True, link, 2, "UnityPy/files/ObjectReader is a symlink"),
            )
        ):
            with self.subTest(label):
                shadow = self.tmp / f"case{n}" / "shadow"
                shutil.copytree(site / "UnityPy", shadow / "UnityPy", ignore=shutil.ignore_patterns("__pycache__"))
                if with_info:
                    shutil.copytree(info, shadow / info.name)
                if change:
                    change(shadow / "UnityPy" / "files")
                out = self.tmp / f"patched{n}.jsonl"
                proc = dump_process(tiny, "-o", out, env={"PYTHONPATH": str(shadow)})
                self.assertEqual(proc.returncode, code, proc.stderr.decode(errors="replace"))
                self.assertEqual(out.exists(), code == 0)
                if code:
                    self.assertIn(b"not the pinned oracle", proc.stderr)
                    self.assertIn(said.encode(), proc.stderr)

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
        index_args = ["-q", "--index", str(root / "index.json"), "-o", str(out)]
        # An input that is not the file its entry pins stops the run: exit 1, no dump, no stats.
        with contextlib.redirect_stderr(io.StringIO()) as err, contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(unitypy_dump.main(index_args), 1)
        said = f"s/atlas_test: sha256 {SAMPLES['atlas_test']} differs from the index's {'0' * 64}"
        self.assertIn(said, err.getvalue())
        self.assertIn("no dump written", err.getvalue())
        self.assertEqual(sorted(p.name for p in self.tmp.iterdir()), ["corpus"])
        # --allow-index-mismatch dumps it with a warning, and the summary counts it.
        with contextlib.redirect_stderr(io.StringIO()) as err:
            code = unitypy_dump.main([*index_args, "--stats", str(stats), "--allow-index-mismatch"])
        self.assertEqual(code, 0)
        self.assertIn("warning: s/atlas_test: sha256", err.getvalue())
        tops = [r["file"] for r in records(out) if r["rec"] == "file" and len(r["file"]) == 1]
        self.assertEqual(tops, [["s/atlas_test"], ["s/banner_1"]])
        self.assertEqual(json.loads(stats.read_text())["summary"]["index_sha256_mismatch"], 1)
        # An index whose hashes all match needs no flag.
        index["files"][1]["sha256"] = SAMPLES["atlas_test"]
        (root / "index.json").write_text(json.dumps(index))
        out.unlink()
        self.assertEqual(dump("--index", root / "index.json", "-o", out, "--stats", stats), 0)
        self.assertNotIn("index_sha256_mismatch", json.loads(stats.read_text())["summary"])
        self.assertEqual(len([r for r in records(out) if r["rec"] == "file" and len(r["file"]) == 1]), 2)
        # The flag means nothing without an index.
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            unitypy_dump.main([str(src), "-o", str(self.tmp / "x.jsonl"), "--allow-index-mismatch"])

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

    def test_refuses_outputs_that_would_overwrite_each_other_or_an_input(self):
        """The dump, its .partial file and the stats are written in turn; none may be another one, or an input."""
        src, out = self.tmp / "in", self.tmp / "x.jsonl"
        src.mkdir()
        (src / "tiny").write_bytes(b"tiny")
        (src / "old.jsonl.partial").write_bytes(b"an input")
        out.write_bytes(b"an earlier dump")
        (self.tmp / "link.json").symlink_to(out)
        cases = {
            "--stats is the dump": (["-o", out, "--stats", out], "--stats names the dump"),
            "--stats is a symlink to the dump": (["-o", out, "--stats", self.tmp / "link.json"], "names the dump"),
            "--stats is the .partial file": (["-o", out, "--stats", f"{out}.partial"], "names the dump's .partial"),
            "the dump is an input": (["-o", src / "tiny"], "the dump would overwrite the input 'tiny'"),
            "the .partial file is an input": (["-o", src / "old.jsonl"], ".partial file would overwrite the input"),
            "--stats is an input": (["-o", out, "--stats", src / "tiny"], "--stats would overwrite the input 'tiny'"),
        }
        before = {p: p.read_bytes() for p in (out, src / "tiny", src / "old.jsonl.partial")}
        for label, (args, said) in cases.items():
            with self.subTest(label):
                with contextlib.redirect_stderr(io.StringIO()) as err, self.assertRaises(SystemExit) as cm:
                    unitypy_dump.main(["-q", str(src), *map(str, args)])
                self.assertEqual(cm.exception.code, 2)
                self.assertIn(said, err.getvalue())
                self.assertEqual({p: p.read_bytes() for p in before}, before)
                self.assertEqual(sorted(p.name for p in src.iterdir()), ["old.jsonl.partial", "tiny"])
                self.assertEqual(sorted(p.name for p in self.tmp.iterdir()), ["in", "link.json", "x.jsonl"])
        # Paths of their own are written.
        stats = self.tmp / "x.stats.json"
        self.assertEqual(dump(src, "-o", out, "--stats", stats), 0)
        self.assertEqual(records(out)[-1], {"rec": "end", "files": 2, "objects": 0})
        self.assertEqual(json.loads(stats.read_text())["summary"]["inputs"], 2)


if __name__ == "__main__":
    unittest.main()
