"""tools/parity/value_cases.py: the verdicts, and the cases file `unityrs values` reads.

    python -m unittest discover -s tools/parity/tests

The verdict tests began as the rabex spike's script tests (removed with the
spike), run here on the copy. Needs UnityPy (the oracle image); skipped elsewhere.
"""

import contextlib
import io
import json
import os
import sys
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))

import scratch  # noqa: E402

try:
    import value_cases
except ImportError:  # pragma: no cover - outside the oracle image
    value_cases = None

I32 = range(-(2**31), 2**31)


@unittest.skipIf(value_cases is None, "needs UnityPy (the oracle image)")
class ValueCasesTests(unittest.TestCase):
    def test_the_copy_reads_this_folders_canonical_module(self):
        self.assertEqual(Path(value_cases.PARITY), HERE)
        self.assertEqual(Path(value_cases.C.__file__).resolve().parent, HERE)

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
        # Review round 11: an oracle whose encoder refused vouches for nothing, so the same refusal on both sides
        # is no match (CANONICAL_DUMP.md 4.3), nor is any other answer.
        refused = side("encode", None, None)
        self.assertEqual(v(refused, side("encode", None, None)), "reference_encode")
        self.assertEqual(v(refused, side(None, None)), "reference_encode")
        self.assertEqual(v(side(None, None), refused), "differs")

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

    def test_the_cases_file_is_what_unityrs_values_reads(self):
        # crates/unityrs-dump/src/values.rs `parse_cases`: a list of
        # {name, endian, nodes, hex}, each node [level, type, name, meta_flag]
        # with i32 integers, and every name distinct (the outcome lines are
        # matched to cases by name).
        cases = json.loads(json.dumps(value_cases.protocol_cases()))
        self.assertEqual(len(cases), len(value_cases.CASES))
        self.assertEqual(len({c["name"] for c in cases}), len(cases))
        for c, src in zip(cases, value_cases.CASES, strict=True):
            self.assertEqual(set(c), {"name", "endian", "nodes", "hex"})
            self.assertIn(c["endian"], ("<", ">"))
            self.assertEqual(bytes.fromhex(c["hex"]), src["data"])
            for row in c["nodes"]:
                self.assertEqual(len(row), 4, (c["name"], row))
                level, ty, name, meta = row
                self.assertIn(level, I32)
                self.assertIn(meta, I32)
                self.assertIsInstance(ty, str)
                self.assertIsInstance(name, str)


# A stand-in for `unityrs values`: it answers each case of the cases file with
# the oracle's own outcome, except as MODE says.
FAKE = """\
#!{python}
import json, sys
sys.path.insert(0, {parity!r})
import value_cases
MODE = {mode!r}
cases = {{c["name"]: c for c in value_cases.CASES}}
out = sys.stdout.buffer
for c in json.load(open(sys.argv[2])):
    o = value_cases.oracle(cases[c["name"]])
    line = {{"name": c["name"], "kind": o["kind"], "bytes_read": o["bytes_read"], "value": o["value"],
             "resave": o["resave"], "resave_hex": o["resave_hex"], "note": "\\u2028 kept \\u2029 \\x85"}}
    if MODE == "wrong" and c["name"] == value_cases.CASES[0]["name"]:
        line.update(kind="other", value=None)
    if MODE == "missing" and c["name"] == value_cases.CASES[-1]["name"]:
        continue
    if MODE == "encode" and c["name"] == value_cases.CASES[0]["name"]:
        line.update(kind="encode", value=None, resave="skipped", resave_hex=None)
    # As serde_json writes it: U+2028, U+2029 and U+0085 unescaped.
    out.write(json.dumps(line, ensure_ascii=False).encode() + b"\\n")
"""


@unittest.skipIf(value_cases is None, "needs UnityPy (the oracle image)")
class GateStatusTests(unittest.TestCase):
    def setUp(self):
        self.tmp = scratch.for_case(self, "unityrs-g2-")

    def run_with(self, mode, oracle=None):
        fake = self.tmp / f"fake-{mode}"
        fake.write_text(FAKE.format(python=sys.executable, parity=str(HERE), mode=mode))
        os.chmod(fake, 0o755)
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            with mock.patch.object(value_cases, "oracle", oracle or value_cases.oracle):
                status = value_cases.main([str(fake)])
        return status, out.getvalue(), err.getvalue()

    def test_every_case_matching_passes(self):
        # Each line carries U+2028, U+2029 and U+0085 inside a string: one line still.
        status, out, _ = self.run_with("same")
        self.assertEqual(status, 0)
        self.assertEqual(json.loads(out.splitlines()[-1])["l2"], {"match": len(value_cases.CASES)})

    def test_a_case_that_differs_fails(self):
        status, _, err = self.run_with("wrong")
        self.assertEqual(status, 1)
        self.assertIn(value_cases.CASES[0]["name"], err)

    def test_a_case_both_sides_refuse_to_encode_fails(self):
        """Review round 11: the oracle's `encode` and the same refusal from unityrs agreed, no re-save on either
        side was `not_resaved`, and G2 passed a case whose value neither side could write."""
        real, first = value_cases.oracle, value_cases.CASES[0]["name"]

        def oracle(case):
            if case["name"] != first:
                return real(case)
            out = {"kind": "encode", "bytes_read": None, "value": None, "resave": "skipped", "resave_hex": None}
            return {**out, "note": "the oracle's encoder refused it"}

        status, out, err = self.run_with("encode", oracle)
        self.assertEqual(status, 1)
        self.assertIn(f"G2 fails on 1 case(s): {first}", err)
        self.assertEqual(json.loads(out.splitlines()[-1])["l2"]["reference_encode"], 1)

    def test_a_case_left_unanswered_fails(self):
        status, _, err = self.run_with("missing")
        self.assertEqual(status, 1)
        self.assertIn("not answered", err)

    def test_the_answer_splits_at_newlines_only(self):
        names = ["a", "b"]
        out = '{"name":"a","note":"x\u2028y"}\n{"name":"b","note":"\u0085"}\n'.encode()
        self.assertEqual(list(value_cases.parse_outcomes(out, names)), names)
        with self.assertRaises(value_cases.ProtocolError):
            value_cases.parse_outcomes(out[:-1], names)
        with self.assertRaises(value_cases.ProtocolError):
            value_cases.parse_outcomes(out + out, names)

    def test_the_gate_fails_on_l2_and_l3_differences(self):
        f = value_cases.gate_failures
        self.assertEqual(f([{"case": "a", "l2": "match", "l3": "match"}]), [])
        self.assertEqual(f([{"case": "a", "l2": "match", "l3": "not_resaved"}]), [])
        self.assertEqual(f([{"case": "a", "l2": "match", "l3": "same_bytes"}]), [])
        self.assertEqual(f([{"case": "a", "l2": "f32_snan_quieted", "l3": "match"}]), ["a"])
        self.assertEqual(f([{"case": "a", "l2": "match", "l3": "differs"}]), ["a"])
        self.assertEqual(f([{"case": "a", "l2": "differs", "l3": "one_side"}]), ["a"])
        self.assertEqual(f([{"case": "a", "l2": "reference_encode", "l3": "not_resaved"}]), ["a"])


if __name__ == "__main__":
    unittest.main()
