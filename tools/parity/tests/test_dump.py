"""unitypy_dump.py end to end: UnityPy's samples, determinism, L0 detection, the CLI rules.

The samples are read at run time from $UNITYRS_UNITYPY_SAMPLES (UnityPy's
tests/samples at commit 417d595), falling back to the corpus copy at
/corpus/unitypy-samples. They are game bundles and are never copied into this
repository. Needs the oracle stack; skipped elsewhere.
"""

import contextlib
import gzip
import hashlib
import io
import json
import os
import shutil
import struct
import sys
import tempfile
import unittest
import zipfile
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))

try:
    import unitypy_dump
except ImportError:  # pragma: no cover - outside the oracle image
    unitypy_dump = None

import compare  # noqa: E402

SAMPLES = {
    "atlas_test": "61ee5afb5f5c64f0a4f65f039d09b2509198363e713423289c895067ac215177",
    "banner_1": "6bd0c3bc36c97e5f1cd2de71bb58477b6c32ac2c254a5705884306c56b94417f",
    "char_118_yuki.ab": "6198cb71c7a820256208332bbd375d21b18ed1a36a12ad04be1eef2111b014a1",
    "xinzexi_2_n_tex": "a9f6e6bba7110cce647ef752d66655d13ce5850b013a60d80d4e8b08953e02ea",
    "xinzexi_2_n_tex_mesh": "8bb2082b9586b3f8f3bad2d22d33346ec0492053f2a76323aa8b4b4047368ca8",
}


def samples_dir():
    for candidate in (os.environ.get("UNITYRS_UNITYPY_SAMPLES"), "/corpus/unitypy-samples"):
        if candidate and all((Path(candidate) / name).is_file() for name in SAMPLES):
            return Path(candidate)
    return None


def records(path):
    opener = gzip.open if str(path).endswith(".gz") else open
    with opener(path, "rt", encoding="utf-8") as fh:
        return [json.loads(line) for line in fh]


def dump(*argv):
    """Run the dumper in-process, quietly (UnityPy prints load errors); returns its exit status."""
    with contextlib.redirect_stderr(io.StringIO()), contextlib.redirect_stdout(io.StringIO()):
        return unitypy_dump.main(["-q", *map(str, argv)])


@unittest.skipIf(unitypy_dump is None, "needs UnityPy (the oracle image)")
class DumpTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-parity-"))
        self.addCleanup(shutil.rmtree, self.tmp)

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
        files = {
            "tiny": b"A" * 19,
            "twenty": b"A" * 20,
            "fa_sig": b"\xfa" * 8 + b"\0" * 200,
            "u_sig": "ú".encode() * 8 + b"\0" * 200,
            "nul.zip": b"PK\x03\x04\0" + b"\0" * 200,
            "plain.zip": zbuf.getvalue(),
            "fake.resS": assets_header,
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
                "tiny": ("ResourceFile", "raw", None),
                "twenty": ("error", "error", "detect"),
                "fa_sig": ("ResourceFile", "raw", None),  # bytes FA never equal U+00FA
                "u_sig": ("BundleFile", "error", "parse"),  # C3 BA x8 does
                "nul.zip": ("ZIP", "zip", None),
                "plain.zip": ("ResourceFile", "raw", None),  # its 5th byte is 0x14, not NUL
                "fake.resS": ("AssetsFile", "raw", None),  # the name rule
                "fake.assets": ("AssetsFile", "error", "parse"),
            },
        )

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
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            unitypy_dump.main([*map(str, args), "-o", str(self.tmp / "x.jsonl"), "--levels", "L0,L3"])

    def test_refuses_output_inside_the_repository(self):
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            unitypy_dump.main([str(HERE), "-o", str(HERE / "dump.jsonl")])
        self.assertFalse((HERE / "dump.jsonl").exists())


if __name__ == "__main__":
    unittest.main()
