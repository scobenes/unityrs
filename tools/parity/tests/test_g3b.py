"""g3b.py: each differing input's class, the crash re-run against real UnityPy included.

Needs the oracle stack (the re-run loads UnityPy); skipped elsewhere.
"""

from __future__ import annotations

import contextlib
import io
import json
import shutil
import struct
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

try:
    import unitypy_dump
except ImportError:  # pragma: no cover - outside the oracle image
    unitypy_dump = None

import g3b  # noqa: E402
import synth as S  # noqa: E402


def assets(tree, data):
    return S.serialized_file([S.Type(114, tree)], [S.Obj(1, 0, data)])


GOOD = assets(S.Node("MyThing", "Base", [S.Node("int", "i")]), struct.pack("<i", 7))
BAD_TYPE = assets(S.Node("MyThing", "Base", [S.Node(b"in\xfft", "i")]), bytes(4))
BAD_NAME = assets(S.Node("MyThing", "Base", [S.Node("int", b"a\xffb")]), bytes(4))


def file_rec(name, error=None, parsed_as="SerializedFile"):
    return {"rec": "file", "file": [name], "l0": {"parsed_as": parsed_as}, "l1": None, "error": error}


def crash(note):
    return file_rec(name=None, error={"kind": "crash", "note": note}, parsed_as="error")


def write(path, records):
    path.write_text("".join(json.dumps(r) + "\n" for r in records))


@unittest.skipIf(unitypy_dump is None, "needs UnityPy (the oracle image)")
class G3bTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-g3b-"))
        self.addCleanup(shutil.rmtree, self.tmp)

    def run_g3b(self, oracle, unityrs, *extra):
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            argv = [oracle, unityrs, "--inputs", self.tmp / "in", "-j", "2", *extra]
            status = g3b.main([str(a) for a in argv])
        return status, json.loads(out.getvalue())

    def test_each_difference_gets_its_class(self):
        d = self.tmp / "in"
        d.mkdir()
        files = {
            "agree": GOOD,
            "bad_name": BAD_NAME,
            "bad_type": BAD_TYPE,
            "differs": GOOD,
            "good_crashed": GOOD,
            "limit": GOOD,
            "oom": GOOD,
            "oom_at_a_count": GOOD,
            "wrong_signal": BAD_TYPE,
        }
        for name, data in files.items():
            (d / name).write_bytes(data)

        def died(sig):
            return f"the process dumping this input died of {sig}"

        oracle, unityrs = [], []
        for name in files:
            ok = file_rec(name)
            got = {
                "bad_name": crash(died("SIGABRT")),
                "bad_type": crash(died("SIGSEGV")),
                "differs": file_rec(name, parsed_as="error", error={"kind": "parse"}),
                "good_crashed": crash(died("SIGSEGV")),
                "oom": crash(unitypy_dump.OUT_OF_MEMORY),
                "oom_at_a_count": crash(unitypy_dump.OUT_OF_MEMORY),
                "wrong_signal": crash(died("SIGABRT")),
            }.get(name, ok)
            got["file"] = [name]
            oracle.append(got)
            mine = dict(ok)
            if name == "limit":
                # UnityPy reads it; unityrs refuses at a bound.
                mine = file_rec(name, parsed_as="error", error={"kind": "parse", "note": "limit TreeDepth: 9 > 8"})
            unityrs.append(mine)
        write(self.tmp / "a.jsonl", oracle)
        write(self.tmp / "b.jsonl", unityrs)

        # The re-run of oom_at_a_count meets its MemoryError where UnityPyBoost
        # reads an object (an allocation site); oom's re-run meets none.
        helper = sys.modules["UnityPy.helpers.TypeTreeHelper"]
        boost = helper.read_typetree_boost

        def short_of_memory(data, node, endian, as_dict, assetsfile, classes):
            if assetsfile.name == "oom_at_a_count":
                raise MemoryError
            return boost(data, node, endian, as_dict, assetsfile, classes)

        with mock.patch.object(helper, "read_typetree_boost", short_of_memory):
            status, report = self.run_g3b(self.tmp / "a.jsonl", self.tmp / "b.jsonl", "--list", self.tmp / "l.json")
        self.assertEqual(status, 1)
        self.assertEqual(
            report["classes"],
            {"agree": 1, "crash_non_utf8": 2, "limit": 1, "memory_bound": 1, "other": 3, "out_of_memory": 1},
        )
        self.assertEqual(
            report["kinds"],
            {
                "crash_non_utf8: m_Name: SIGABRT": 1,
                "crash_non_utf8: m_Type: SIGSEGV": 1,
                "limit: TreeDepth": 1,
                "memory_bound: an array's count, in UnityPyBoost": 1,
                "out_of_memory: re-run: no MemoryError": 1,
            },
        )
        listed = {r["input"]: (r["class"], r["detail"]) for r in json.loads((self.tmp / "l.json").read_text())}
        self.assertEqual(listed["good_crashed"], ("other", "oracle SIGSEGV; re-run: no node that is not UTF-8"))
        # A non-UTF-8 type gives SIGSEGV; the oracle's SIGABRT names another cause.
        self.assertEqual(listed["wrong_signal"], ("other", "oracle SIGABRT; re-run: m_Type"))
        self.assertEqual(listed["differs"], ("other", "records differ"))
        self.assertNotIn("agree", listed)

        # A re-run under a larger cap replaces the out-of-memory record; the
        # rest admitted, the verdict passes.
        write(self.tmp / "rerun.jsonl", [file_rec("oom")])
        keep = {"agree", "bad_name", "bad_type", "limit", "oom"}
        write(self.tmp / "a2.jsonl", [r for r in oracle if r["file"][0] in keep])
        write(self.tmp / "b2.jsonl", [r for r in unityrs if r["file"][0] in keep])
        status, report = self.run_g3b(self.tmp / "a2.jsonl", self.tmp / "b2.jsonl", "--rerun", self.tmp / "rerun.jsonl")
        self.assertEqual((status, report["classes"]), (0, {"agree": 2, "crash_non_utf8": 2, "limit": 1}))
        self.assertEqual(report["unadmitted"], [])


if __name__ == "__main__":
    unittest.main()
