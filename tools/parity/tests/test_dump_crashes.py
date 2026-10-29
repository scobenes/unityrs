"""unitypy_dump.py when an input harms its process, and when the dumper itself fails.

A death by signal (the OOM killer, a UnityPyBoost crash), a MemoryError
anywhere in an input, a load error quoting bad bytes and the `None` leak each
cost only that input, which gets its record; finished results wait in bounded
number; the dumper's own failure stops the run with no dump. Needs the oracle
stack; skipped elsewhere.
"""

import contextlib
import hashlib
import io
import os
import pickle
import resource
import signal
import struct
import subprocess
import sys
import time
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

import canonical as C  # noqa: E402
import synth as S  # noqa: E402
from dump_harness import DumpCase, assets, dump, dump_process, dump_records, records, unitypy_dump  # noqa: E402


@unittest.skipIf(unitypy_dump is None, "needs UnityPy (the oracle image)")
class DumpCrashTests(DumpCase):
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

    def test_an_input_out_of_memory_is_one_crash_record(self):
        """A MemoryError the dumper meets (--memory-cap's RLIMIT_AS, G3b) is the machine's: the crash record."""
        d = self.tmp / "in"
        d.mkdir()
        for name in "abc":
            (d / name).write_bytes(b"tiny")
        cap = 6 << 30
        limits = self.tmp / "limits"
        real = unitypy_dump.dump_input

        def short_of_memory(job):
            with open(limits, "a") as fh:
                fh.write(f"{resource.getrlimit(resource.RLIMIT_AS)[0]}\n")
            if job[1] == "b":
                raise MemoryError
            return real(job)

        out = self.tmp / "m.jsonl"
        with mock.patch.object(unitypy_dump, "dump_input", short_of_memory):
            self.assertEqual(dump(d, "-o", out, "-j", "2", "--memory-cap", str(cap)), 0)
        self.assert_canonical(out)
        recs = [r for r in records(out) if r["rec"] == "file"]
        self.assertEqual([r["file"] for r in recs], [["a"], ["b"], ["c"]])
        self.assertEqual((recs[0]["error"], recs[2]["error"]), (None, None))
        self.assertEqual(recs[1]["error"], {"kind": "crash", "note": unitypy_dump.OUT_OF_MEMORY})
        self.assertEqual(limits.read_text().split(), [str(cap)] * 3)

    def test_a_memory_error_anywhere_in_an_input_is_its_crash_record(self):
        """A MemoryError inside an object's parse, its resave or the load is the cap's too, never an outcome."""
        d = self.tmp / "in"
        d.mkdir()
        good = assets(S.Node("MyThing", "Base", [S.Node("int", "i")]), struct.pack("<i", 7))
        for name in ("a", "b_parse", "c_resave", "d_load"):
            (d / name).write_bytes(good)
        reader = sys.modules["UnityPy.files.ObjectReader"].ObjectReader
        environment = unitypy_dump.UnityPy.Environment
        real_parse, real_load = reader.parse_as_dict, environment.load_file

        def parse(obj, *args, **kwargs):
            if obj.assets_file.name == "b_parse":
                raise MemoryError
            return real_parse(obj, *args, **kwargs)

        def write(*args, **kwargs):
            raise MemoryError

        def load(env, data, *args, name=None, **kwargs):
            if name == "d_load":
                raise MemoryError
            return real_load(env, data, *args, name=name, **kwargs)

        out = self.tmp / "m.jsonl"
        with (
            mock.patch.object(reader, "parse_as_dict", parse),
            mock.patch.object(environment, "load_file", load),
            mock.patch.object(unitypy_dump.TypeTreeHelper, "write_typetree", write),
        ):
            self.assertEqual(dump(d, "-o", out, "-j", "2", "--levels", "L0,L1,L2"), 0)
            d3 = self.tmp / "in3"
            d3.mkdir()
            (d3 / "c_resave").write_bytes(good)
            out3 = self.tmp / "m3.jsonl"
            # Its only input crashed: the dump is written, the exit status says so.
            self.assertEqual(dump(d3, "-o", out3), 3)
        self.assert_canonical(out)
        recs = records(out)
        files = [r for r in recs if r["rec"] == "file"]
        self.assertEqual([r["file"] for r in files], [["a"], ["b_parse"], ["c_resave"], ["d_load"]])
        crash = {"kind": "crash", "note": unitypy_dump.OUT_OF_MEMORY}
        self.assertEqual([r["error"] for r in files], [None, crash, None, crash])
        self.assertEqual([r["file"] for r in recs if r["rec"] == "object"], [["a"], ["c_resave"]])
        # L3 resaves: there the write's MemoryError fails the input too.
        self.assertEqual([r["error"] for r in records(out3) if r["rec"] == "file"], [crash])

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
                mock.patch.object(dump_records, "serialized_l1", buggy_l1),
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


if __name__ == "__main__":
    unittest.main()
