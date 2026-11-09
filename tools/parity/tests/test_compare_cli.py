"""compare.py on hand-built dumps: the header (levels, producer, the pinned oracle) and the command line."""

import gzip
import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import canonical as C  # noqa: E402
import compare  # noqa: E402
from compare_support import LEVELS, CompareCase, base_records, f32, header  # noqa: E402


class CompareTests(CompareCase):
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

    def test_an_encode_error_is_a_candidates_alone(self):
        # Only a producer under test writes `encode` (spec 4.3): in the candidate it is an L2
        # difference like any other; in the reference it is a format finding, even where the
        # candidate's line is the same.
        bad = base_records()
        bad[1]["l2"].update(error={"kind": "encode", "bytes_read": None, "note": "EncodeError: ..."}, value=None)
        bad[1]["l3"].update(resave="skipped", resave_size=None, resave_sha256=None, first_diff=None)
        good, pa = base_records(), self.write("bad.jsonl", bad)
        report, d = self.run_compare(self.write("good.jsonl", good), pa)
        self.assertIn(("L2", "l2.error", "type"), self.paths(d))
        self.assertNotIn("reference_encode", {k for _, _, k in self.paths(d)})
        for candidate in (pa, self.write("good.jsonl", good)):
            with self.subTest(candidate=candidate):
                report, d = self.run_compare(pa, candidate)
                self.assertIn(("format", "l2.error (reference)", "reference_encode"), self.paths(d))
                self.assertNotIn(("format", "l2.error (candidate)", "reference_encode"), self.paths(d))
                self.assertEqual(self.cli(pa, candidate), 1)

    def test_reference_must_come_from_the_pinned_oracle(self):
        recs = base_records()
        pb = self.write("b.jsonl", recs)
        pins = C.oracle_pins()
        oracle = {
            "name": C.ORACLE_PRODUCER,
            "unitypy": C.ORACLE_UNITYPY,
            "boost": True,
            "boost_sha256": C.ORACLE_BOOST_SHA256,
            "packages": pins,
            "records": C.oracle_records(),
            "python": "3.11",
        }
        self.assertEqual(self.cli(self.write("a.jsonl", recs, head=header(producer=oracle)), pb), 0)
        without_numpy = {k: v for k, v in pins.items() if k != "numpy"}
        changes = [
            ({"boost": False}, "the oracle is UnityPy"),
            ({"unitypy": "1.24.0"}, "the oracle is UnityPy"),
            ({"boost": 1}, "the oracle is UnityPy"),
            # Another UnityPyBoost build, or none recorded (a dump from before the hash was recorded).
            ({"boost_sha256": "0" * 64}, "the UnityPyBoost binary " + "0" * 64),
            ({"boost_sha256": None}, "the UnityPyBoost binary None"),
            # The stack around UnityPy: numpy missing (UnityPy then renders sprites without it), another
            # version, an extra package, or no record of the packages.
            ({"packages": without_numpy}, "numpy is not installed"),
            ({"packages": {**pins, "pillow": "10.0.0"}}, "pillow is 10.0.0, not the pinned 9.5.0"),
            ({"packages": {**pins, "scipy": "1.17.1"}}, "scipy 1.17.1 is installed but not pinned"),
            ({"packages": None}, "packages None"),
            # The pinned wheels' files: another build of a pinned version (another RECORD), or a dump
            # from before the dumper hashed every file (no records).
            (
                {"records": {**C.oracle_records(), "pillow": "0" * 64}},
                "the record digests of pillow are not the pinned ones",
            ),
            ({"records": {k: v for k, v in C.oracle_records().items() if k != "numpy"}}, "of numpy are not"),
            ({"records": None}, "records None"),
        ]
        for change, said in changes:
            with self.subTest(change):
                pa = self.write("a.jsonl", recs, head=header(producer={**oracle, **change}))
                self.assertEqual(self.cli(pa, pb), 2)
                self.assertIn(said, self.errors)
                self.assertEqual(self.cli(pb, pa), 0, "without --oracle, a candidate's producer is not checked")

    def test_every_gate_refuses_a_candidate_dumped_with_limits_lifted(self):
        # `unityrs dump --unbounded` names the limits it lifted in its producer's `unbounded`, and only such a dump
        # holds that member (CANONICAL_DUMP.md section 4.1). Lifted, an 18-deep nest the bounded dump refuses
        # (`limit ContainerDepth: 17 > 16`) agrees with the oracle's records, so `--oracle` (every gate's
        # comparison) refuses it as the candidate whatever its records hold (review round 16).
        recs = base_records()
        oracle = {
            "name": C.ORACLE_PRODUCER,
            "unitypy": C.ORACLE_UNITYPY,
            "boost": True,
            "boost_sha256": C.ORACLE_BOOST_SHA256,
            "packages": C.oracle_pins(),
            "records": C.oracle_records(),
            "python": "3.11",
        }
        pa = self.write("a.jsonl", recs, head=header(producer=oracle))
        unityrs = {"name": "unityrs", "unityrs": "0.0.1"}
        bounded = self.write("bounded.jsonl", recs, head=header(producer=unityrs))
        self.assertEqual(self.cli(pa, bounded, "--oracle"), 0, self.errors)
        # A producer that is no object names no limits: the candidate's own business, as before.
        text = self.write("text.jsonl", recs, head=header(producer="unbounded"))
        self.assertEqual(self.cli(pa, text, "--oracle"), 0, self.errors)
        for lifted in (["ContainerDepth"], ["TreeDepth", "ValueDepth"], []):
            with self.subTest(lifted=lifted):
                pb = self.write("lifted.jsonl", recs, head=header(producer={**unityrs, "unbounded": lifted}))
                self.assertEqual(self.cli(pa, pb, "--oracle"), 2)
                said = f"the candidate was dumped with limits lifted (its producer's unbounded: {lifted})"
                self.assertIn(said, self.errors)
                with self.assertRaisesRegex(compare.CompareError, "limits lifted"):
                    compare.run(pa, pb, oracle=True)
                # Without --oracle, no gate's comparison (two unityrs dumps), it compares.
                self.assertEqual(self.cli(pa, pb), 0, self.errors)

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
                path = self.tmp / name
                path.write_bytes(raw)
                self.assertEqual(self.cli(pa, path), 2)
                self.assertTrue(self.errors.startswith("compare: "), self.errors)

    def test_json_report_stays_out_of_the_repository(self):
        """The report quotes input names, so --json is refused inside the repository, through symlinks too."""
        a = self.write("a.jsonl", base_records())
        here = Path(__file__).resolve().parents[1]
        link = self.tmp / "link"
        link.symlink_to(here)
        for target in (here / "report.json", link / "report.json"):
            with self.subTest(target=target):
                try:
                    self.assertEqual(self.cli(a, a, "--json", target), 2)
                    self.assertIn("refusing to write --json into the repository", self.errors)
                    self.assertFalse((here / "report.json").exists())
                finally:
                    (here / "report.json").unlink(missing_ok=True)
        out = self.tmp / "report.json"
        self.assertEqual(self.cli(a, a, "--json", out), 0)
        self.assertEqual(json.loads(out.read_text())["counts"]["identical"], 4)

    def test_cli_exit_codes_and_report(self):
        a, b = base_records(), base_records()
        b[1]["l3"]["resave"] = "differs"
        pa, pb = self.write("a.jsonl", a), self.write("b.jsonl", b)
        self.assertEqual(self.cli(pa, pa), 0)
        self.assertEqual(self.cli(pa, pb, "--json", self.tmp / "r.json"), 1)
        self.assertIn("L3      Transform", self.output)
        report = json.loads((self.tmp / "r.json").read_text())
        self.assertEqual(report["by_path"][0]["path"], "l3.resave")


if __name__ == "__main__":
    unittest.main()
