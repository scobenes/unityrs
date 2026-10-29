"""Tests of the UnityPy side of the LZMA spike, on synthetic bundles only.
Run them in the dev container, from the checkout root:

    python3 -m unittest discover -s crates/unityrs-bench-lzma
"""

import argparse
import contextlib
import io
import lzma
import tempfile
import unittest
from pathlib import Path

import unitypy_lzma as bench


def quietly(command, **args):
    """Runs a subcommand and returns what it printed. UnityPy prints its own
    parse errors, which go to the same place."""
    out = io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
        command(argparse.Namespace(**args))
    return out.getvalue()


class Synth(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.dir = Path(cls.tmp.name)
        quietly(bench.cmd_synth, out_dir=str(cls.dir))

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def capture(self, name):
        with contextlib.redirect_stdout(io.StringIO()):
            return bench.capture(str(self.dir / f"{name}.bundle"))

    def test_unitypy_reads_every_header_shape(self):
        paths = bench.read_list(self.dir / "shapes.txt")
        self.assertEqual(len(paths), len(bench.SHAPES))
        for path in paths:
            with contextlib.redirect_stdout(io.StringIO()):
                blocks = bench.capture(path)
            self.assertEqual([index for index, *_ in blocks], [0, 2], path)
            for _, _, size, out in blocks:
                self.assertIsInstance(out, bytes, path)
                self.assertEqual(len(out), size, path)

    def test_unitypy_returns_what_a_truncated_stream_yields(self):
        [(index, _, size, out)] = self.capture("truncated")
        self.assertEqual((index, size), (0, 200_000))
        self.assertIsInstance(out, bytes)
        self.assertTrue(0 < len(out) < size, len(out))
        self.assertEqual(out, bench.sample(size)[: len(out)])

    def test_unitypy_returns_all_of_an_over_long_stream(self):
        for name, size in (("over-long-in-a-match", 100_000), ("over-long-between-symbols", 101_304)):
            [(_, _, blocks_info_size, out)] = self.capture(name)
            self.assertEqual(blocks_info_size, size)
            self.assertEqual(out, bench.sample(200_000), name)

    def test_unitypy_raises_on_lc_plus_lp_over_4(self):
        [(_, block, _, out)] = self.capture("props-0x5f")
        self.assertEqual(block[0], 0x5F)
        self.assertIsInstance(out, lzma.LZMAError)
        # The same stream under its true properties decodes: only lc + lp is wrong.
        small = bytes(b & 7 for b in bench.sample(200_000))
        self.assertEqual(bench.CompressionHelper.decompress_lzma(b"\x5d" + block[1:]), small)

    def test_unitypy_decodes_a_dictionary_size_lzma_rust2_refuses(self):
        [(_, block, size, out)] = self.capture("dict-0xfffffff1")
        self.assertEqual(int.from_bytes(block[1:5], "little"), 0xFFFFFFF1)
        self.assertEqual((size, out), (200_000, bench.sample(200_000)))

    def test_the_oracle_writes_a_row_for_every_block(self):
        oracle = self.dir / "contract.tsv"
        printed = quietly(bench.cmd_oracle, list=str(self.dir / "contract.txt"), out=str(oracle))
        self.assertIn(f"{len(bench.CONTRACT)} LZMA blocks", printed)
        rows = {Path(line.split("\t")[0]).stem: line.split("\t")[1:] for line in oracle.read_text().splitlines()}
        self.assertEqual(rows["props-0x5f"], ["0", "error", "LZMAError"])
        self.assertEqual(rows["over-long-in-a-match"][1], "200000")
        self.assertLess(int(rows["truncated"][1]), 200_000)
        self.assertEqual(rows["dict-0xfffffff1"][1], "200000")


class EngineVersions(unittest.TestCase):
    def test_unitypy_reads_these_engine_versions_as_other_versions(self):
        # unityfs.rs refuses each of them (Engine::parse). UnityPy packs the
        # fields into one integer and reads each back masked, and its \d
        # matches any Unicode digit; major 0 sends it to its fallback version.
        from UnityPy.helpers.UnityVersion import UnityVersion

        for text, read in (
            ("2019.4.14f65536", (2019, 4, 15)),
            ("67555.4.15f1", (2019, 4, 15)),
            ("2019.4.1５f1", (2019, 4, 15)),
            ("65536.0.0", (0, 0, 0)),
        ):
            version = UnityVersion.from_str(text)
            self.assertEqual((version.major, version.minor, version.build), read, text)


class Report(unittest.TestCase):
    def test_the_report_names_no_bundle_and_compares_with_liblzma(self):
        with tempfile.TemporaryDirectory() as tmp:
            runs, blocks = Path(tmp) / "runs.tsv", Path(tmp) / "blocks.tsv"
            runs.write_text(
                "liblzma\ts\t1\t0\t2\t3000000\t0.020000\n"
                "other\ts\t1\t0\t2\t3000000\t0.040000\n"
                "liblzma\ts\t8\t0\t2\t3000000\t0.004000\n"
                "other\ts\t8\t0\t2\t3000000\t0.010000\n"
            )
            blocks.write_text(
                "liblzma\ts\t0\t/x/secret_bundle_name\t0\t2000000\t0.010000\n"
                "liblzma\ts\t0\t/x/other_bundle\t0\t1000000\t0.010000\n"
                "other\ts\t0\t/x/secret_bundle_name\t0\t2000000\t0.020000\n"
                "other\ts\t0\t/x/other_bundle\t0\t1000000\t0.020000\n"
            )
            text = quietly(bench.cmd_report, runs=[str(runs)], per_block=[str(blocks)])
        self.assertNotIn("bundle", text)
        self.assertIn(f"| s | {bench.LARGEST} | 1 | 2.0 | 200.0 | 100.0 |", text)
        self.assertIn(f"| s | {bench.LARGEST}, seconds | 1 | | 0.010 s | 0.020 s |", text)
        self.assertIn(f"| s | {bench.LARGEST}, liblzma / this | 1 | | 1.00 | 2.00 |", text)
        # other: 75 MB/s against 150 on 1 core, 300 against 750 on 8.
        self.assertIn("| s | other | 2.00 | 2.50 | 4.0 | 13.33 | 3.33 |", text)
        self.assertIn("| s | 2 | 3.0 | 1.000 | 1.50 | 2.0 |", text)


if __name__ == "__main__":
    unittest.main()
