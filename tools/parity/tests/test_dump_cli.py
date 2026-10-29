"""unitypy_dump.py's command line: index mode, levels, and the guards on what it writes.

An input that is not the indexed file stops the run unless
--allow-index-mismatch; levels it is not asked for stay null; the dump and
--stats land outside the repository, each a file of its own and never an
input. Needs the oracle stack; skipped elsewhere.
"""

import contextlib
import hashlib
import io
import json
import os
import sys
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from dump_harness import SAMPLES, DumpCase, dump, records, samples_dir, unitypy_dump  # noqa: E402


@unittest.skipIf(unitypy_dump is None, "needs UnityPy (the oracle image)")
class DumpCliTests(DumpCase):
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
        """The dump, its .partial file and the stats are written in turn; none may be another, an input or the index.

        Each is compared by its path and by its (st_dev, st_ino), so a hard
        link is caught, and none may be a symlink: a .partial file linked to
        the stats or to an input would be written through.
        """
        src, out, stats = self.tmp / "in", self.tmp / "x.jsonl", self.tmp / "s.json"
        corpus = self.tmp / "corpus"
        src.mkdir()
        corpus.mkdir()
        (src / "tiny").write_bytes(b"tiny")
        (src / "old.jsonl.partial").write_bytes(b"an input")
        (corpus / "tiny").write_bytes(b"tiny")
        index = corpus / "index.json"
        sha = hashlib.sha256(b"tiny").hexdigest()
        index.write_text(json.dumps({"files": [{"path": "tiny", "kind": "unitypy-sample", "sha256": sha}]}))
        out.write_bytes(b"an earlier dump")
        stats.write_bytes(b"earlier stats")
        (self.tmp / "link.json").symlink_to(out)
        (self.tmp / "y.jsonl.partial").symlink_to(stats)
        (self.tmp / "z.jsonl.partial").symlink_to(src / "tiny")
        os.link(out, self.tmp / "hard-dump")
        os.link(src / "tiny", self.tmp / "hard-tiny")
        os.link(index, self.tmp / "hard-index")
        on_src = ["-q", str(src)]
        on_index = ["-q", "--index", str(index)]
        cases = {
            "--stats is the dump": (on_src, ["-o", out, "--stats", out], "--stats names the dump"),
            "--stats is a symlink to the dump": (
                on_src,
                ["-o", out, "--stats", self.tmp / "link.json"],
                f"--stats ({self.tmp / 'link.json'}) is a symlink",
            ),
            "--stats is a hard link to the dump": (
                on_src,
                ["-o", out, "--stats", self.tmp / "hard-dump"],
                "is the same file as the dump",
            ),
            "--stats is the .partial file": (
                on_src,
                ["-o", out, "--stats", f"{out}.partial"],
                "names the dump's .partial",
            ),
            "the dump is a symlink": (
                on_src,
                ["-o", self.tmp / "link.json"],
                "the dump (" + str(self.tmp / "link.json"),
            ),
            "the .partial file is a symlink to the stats": (
                on_src,
                ["-o", self.tmp / "y.jsonl", "--stats", stats],
                f"the dump's .partial file ({self.tmp / 'y.jsonl.partial'}) is a symlink",
            ),
            "the .partial file is a symlink to an input": (
                on_src,
                ["-o", self.tmp / "z.jsonl"],
                f"the dump's .partial file ({self.tmp / 'z.jsonl.partial'}) is a symlink",
            ),
            "the dump is an input": (on_src, ["-o", src / "tiny"], "the dump would overwrite the input 'tiny'"),
            "the dump is a hard link to an input": (
                on_src,
                ["-o", self.tmp / "hard-tiny"],
                "the dump would overwrite the input 'tiny'",
            ),
            "the .partial file is an input": (
                on_src,
                ["-o", src / "old.jsonl"],
                ".partial file would overwrite the input",
            ),
            "--stats is an input": (
                on_src,
                ["-o", out, "--stats", src / "tiny"],
                "--stats would overwrite the input 'tiny'",
            ),
            "--stats is a hard link to an input": (
                on_src,
                ["-o", out, "--stats", self.tmp / "hard-tiny"],
                "--stats would overwrite the input 'tiny'",
            ),
            "the dump is the index": (on_index, ["-o", index], f"the dump would overwrite the index {str(index)!r}"),
            "--stats is the index": (
                on_index,
                ["-o", out, "--stats", index],
                f"--stats would overwrite the index {str(index)!r}",
            ),
            "the dump is a hard link to the index": (
                on_index,
                ["-o", self.tmp / "hard-index"],
                "the dump would overwrite the index",
            ),
        }
        files = (out, stats, src / "tiny", src / "old.jsonl.partial", corpus / "tiny", index)
        before = {p: p.read_bytes() for p in files}
        names = sorted(p.name for p in self.tmp.iterdir())
        for label, (given, args, said) in cases.items():
            with self.subTest(label):
                with contextlib.redirect_stderr(io.StringIO()) as err, self.assertRaises(SystemExit) as cm:
                    unitypy_dump.main([*given, *map(str, args)])
                self.assertEqual(cm.exception.code, 2)
                self.assertIn(said, err.getvalue())
                self.assertEqual({p: p.read_bytes() for p in before}, before)
                self.assertEqual(sorted(p.name for p in src.iterdir()), ["old.jsonl.partial", "tiny"])
                self.assertEqual(sorted(p.name for p in self.tmp.iterdir()), names)
                self.assertTrue((self.tmp / "y.jsonl.partial").is_symlink())
        # Paths of their own are written: a .partial file a killed run left
        # is replaced, and the stats are a new file, so a hard link to the
        # old one keeps its bytes.
        Path(f"{out}.partial").write_bytes(b"left by a run that was killed")
        os.link(stats, self.tmp / "keep")
        self.assertEqual(dump(src, "-o", out, "--stats", stats), 0)
        self.assertEqual(records(out)[-1], {"rec": "end", "files": 2, "objects": 0})
        self.assertFalse(Path(f"{out}.partial").exists())
        self.assertEqual(json.loads(stats.read_text())["summary"]["inputs"], 2)
        self.assertEqual((self.tmp / "keep").read_bytes(), b"earlier stats")
        self.assertEqual(sorted(p.name for p in self.tmp.iterdir()), sorted([*names, "keep"]))

    def test_a_partial_file_made_as_the_run_starts_is_not_written_through(self):
        """The .partial file is made with O_EXCL and O_NOFOLLOW: one that appears after the checks stops the run."""
        tiny, out, victim = self.tmp / "tiny", self.tmp / "x.jsonl", self.tmp / "victim"
        tiny.write_bytes(b"tiny")
        victim.write_bytes(b"not the dump's")
        real = unitypy_dump.open_output

        def planted_first(path, compress):
            path.symlink_to(victim)
            return real(path, compress)

        with (
            mock.patch.object(unitypy_dump, "open_output", planted_first),
            contextlib.redirect_stderr(io.StringIO()) as err,
            self.assertRaises(SystemExit) as cm,
        ):
            unitypy_dump.main(["-q", str(tiny), "-o", str(out)])
        self.assertEqual(cm.exception.code, 2)
        self.assertIn(f"the dump's .partial file ({out}.partial) was made by something else", err.getvalue())
        self.assertEqual(victim.read_bytes(), b"not the dump's")
        self.assertTrue(Path(f"{out}.partial").is_symlink())
        self.assertFalse(out.exists())


if __name__ == "__main__":
    unittest.main()
