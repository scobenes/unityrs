"""callgrind.py's reading of callgrind output (docs/plans/bench.md section 4.2): the parse, the scope checks and
the per-object sums, on files callgrind 3.19 recorded of `unityrs-bench run texture-RGBA32` (tests/callgrind/, paths
replaced by /bench/..., trailing blanks dropped) and on edits of them; and one run through a stand-in valgrind. Standard library only."""

import json
import os
import re
import unittest
from pathlib import Path

import support  # noqa: F401  (the harness on the path)
import callgrind
import scratch

DATA = Path(__file__).resolve().parent / "callgrind"
SCOPE = (DATA / "scope.out").read_text(encoding="utf-8")
OUTSIDE = (DATA / "outside.out").read_text(encoding="utf-8")
NAME = "texture-RGBA32"
FN = "unityrs_bench::work::run_texture_rgba32"
OWN = "/bench/unityrs-bench"
LIBC = "/usr/lib/x86_64-linux-gnu/libc.so.6"

# A small file in the format, by hand: name compression across ob/cob and fn/cfn, a relative position, two events
# with Ir second, a call whose callee is defined in its cfn= line, and a function outside the scope with no cost.
SMALL = """# callgrind format
version: 1
creator: callgrind-3.19.0
positions: line
events: Dr Ir
summary: 7 2500

ob=(1) /bench/unityrs-bench
fl=(1) ???
fn=(1) unityrs_bench::main
cfn=(2) unityrs_bench::work::run_load_objects_lz4
calls=1 0
0 7 2500

fn=(2)
10 1 400
+2 1 100
cob=(2) /usr/lib/x86_64-linux-gnu/libc.so.6
cfi=(1)
cfn=(3) malloc
calls=4 0
0 2 900
cfn=(4) helper::h0123456789abcdef
calls=1 0
* 3 1100

ob=(2)
fn=(3)
0 2 900

ob=(1)
fn=(4)
0 3 1100

totals: 7 2500
"""


def scope_id(text: str) -> str:
    return re.search(r"^fn=\((\d+)\) " + re.escape(FN) + "$", text, re.M).group(1)


class ParseTests(unittest.TestCase):
    def test_a_recorded_scope_is_read_whole(self):
        prof = callgrind.parse(SCOPE)
        self.assertEqual(prof.totals, 163_871)
        self.assertEqual(prof.objects(), {OWN: 109_942, LIBC: 53_929})
        self.assertEqual(sum(prof.objects().values()), prof.totals)
        self.assertEqual(prof.entered(FN), 1)
        self.assertEqual(prof.inclusive(FN), prof.totals)
        self.assertEqual(prof.object_of(FN), OWN)

    def test_compression_positions_and_the_ir_column(self):
        prof = callgrind.parse(SMALL)
        fn = "unityrs_bench::work::run_load_objects_lz4"
        self.assertEqual(prof.own[("/bench/unityrs-bench", fn)], 500)
        self.assertEqual(prof.own[("/bench/unityrs-bench", "helper")], 1100)  # the hash is not part of a name
        self.assertEqual(prof.objects(), {"/bench/unityrs-bench": 1600, "/usr/lib/x86_64-linux-gnu/libc.so.6": 900})
        self.assertEqual(prof.inclusive(fn), 2500)
        self.assertEqual(prof.entered(fn), 1)
        self.assertNotIn(("/bench/unityrs-bench", "unityrs_bench::main"), prof.own)  # its line is a call's cost

    def test_malformed_files_are_refused(self):
        for text, why in ((SMALL.replace("totals: 7 2500\n", ""), "totals"),
                          (SMALL.replace("cfn=(3) malloc", "cfn=(9)"), "before its name"),
                          (SMALL.replace("events: Dr Ir", "events: Dr"), "no Ir"),
                          (SMALL.replace("calls=4 0", "jump=4 0"), "jump"),
                          (SMALL.replace("+2 1 100", "+2 1"), "no Ir field"),
                          (SMALL.replace("totals: 7 2500", "totals: 7 2600"), "sum to 2,500, not the file's total"),
                          (SMALL.replace("0 3 1100\n\ntotals", "0 3 1000\n\ntotals"), "sum to 2,400")):
            with self.assertRaisesRegex(callgrind.ParseError, why):
                callgrind.parse(text)

    def test_the_toggle_names_the_scope_the_bench_binary_defines(self):
        self.assertEqual(callgrind.scope_name("texture-ASTC_RGBA_4x4"), "run_texture_astc_rgba_4x4")
        self.assertEqual(callgrind.toggle(NAME), "*unityrs_bench::work::run_texture_rgba32*")


class ScopeTests(unittest.TestCase):
    def test_the_recorded_scope_passes(self):
        self.assertEqual(callgrind.scope_problems(callgrind.parse(SCOPE), NAME), [])
        self.assertEqual(callgrind.scope_problems(callgrind.parse(SMALL), "load-objects-lz4"),
                         ["unityrs_bench::work::run_load_objects_lz4's inclusive Ir is 2,500, under 10,000"])

    def test_collection_outside_the_scope_is_refused(self):
        """Recorded with a second toggle on the digest, after the scope: its cost is outside the scope's."""
        bad = callgrind.scope_problems(callgrind.parse(OUTSIDE), NAME)
        self.assertEqual(bad, ["collection was on outside the scope: the file's total is 7,013,076, the scope's "
                               "163,871"])

    def test_a_scope_entered_twice_is_refused(self):
        sid = scope_id(SCOPE)
        text = SCOPE.replace(f"cfn=({sid})\ncalls=1 0\n", f"cfn=({sid})\ncalls=2 0\n", 1)
        self.assertNotEqual(text, SCOPE)
        self.assertIn(f"{FN} was entered 2 times, not once", callgrind.scope_problems(callgrind.parse(text), NAME))

    def test_another_scope_entered_is_refused(self):
        text = SCOPE.replace("<unityrs_core::object::ObjectRef>::read_fields_mode",
                             "unityrs_bench::work::run_texture_dxt1", 1)
        self.assertIn("another scope was entered: unityrs_bench::work::run_texture_dxt1",
                      callgrind.scope_problems(callgrind.parse(text), NAME))

    def test_a_scope_never_entered_is_refused(self):
        self.assertEqual(callgrind.scope_problems(callgrind.parse(SCOPE), "texture-DXT1"),
                         ["unityrs_bench::work::run_texture_dxt1 was never entered"])


class CountTests(unittest.TestCase):
    def test_own_libc_and_allocation_calls_from_the_bench_binary(self):
        got = callgrind.counts(callgrind.parse(SCOPE), NAME)
        self.assertEqual(got, {"ir_own": 109_942, "ir_libc": 53_929, "calls": 1,
                               "allocs": {"malloc": 93, "calloc": 0, "realloc": 32, "free": 78}})

    def test_a_call_from_libc_itself_is_not_unityrs_s(self):
        got = callgrind.counts(callgrind.parse(SMALL), "load-objects-lz4")
        self.assertEqual(got["allocs"], {"malloc": 4, "calloc": 0, "realloc": 0, "free": 0})
        inner = SMALL.replace("ob=(2)\nfn=(3)\n0 2 900", "ob=(2)\nfn=(3)\n0 1 400\ncfn=(3)\ncalls=5 0\n0 1 500")
        inner = inner.replace("totals: 7 2500", "totals: 7 2000")
        self.assertEqual(callgrind.counts(callgrind.parse(inner), "load-objects-lz4")["allocs"]["malloc"], 4)

    def test_libc_is_known_by_its_file_name(self):
        for ob in (LIBC, "libc.so.6", "/lib64/libc-2.36.so"):
            self.assertTrue(callgrind.is_libc(ob), ob)
        for ob in ("/usr/lib/x86_64-linux-gnu/libcrypt.so.1", OWN, "/lib/libc++.so.1"):
            self.assertFalse(callgrind.is_libc(ob), ob)


FAKE = """#!/bin/sh
# A stand-in valgrind: writes the recorded file where --callgrind-out-file says, prints the bench binary's line.
for a in "$@"; do
  case "$a" in
    --callgrind-out-file=*) out="${a#--callgrind-out-file=}" ;;
    --toggle-collect=*) toggle="${a#--toggle-collect=}" ;;
  esac
done
[ "$toggle" = "*unityrs_bench::work::run_texture_rgba32*" ] || exit 3
[ -z "${HOME:-}" ] && [ -n "$VALGRIND_LIB" ] || exit 4
cp "$(dirname "$0")/recorded.out" "$out"
echo '{"workload":"texture-RGBA32","status":"ok","scope_s":0.1,"counts":{},"digest":"%s","alloc_bytes":135656,"scopes":%d,"cpu":"sse4.2"}'
"""

# A scope whose every instruction is libc's: its inclusive cost passes, its own object's count is 0.
ZERO = """version: 1
positions: line
events: Ir
ob=(1) /bench/unityrs-bench
fn=(1) unityrs_bench::main
cfn=(2) unityrs_bench::work::run_texture_rgba32
calls=1 0
0 20000
fn=(2)
cob=(2) /usr/lib/x86_64-linux-gnu/libc.so.6
cfn=(3) memcpy
calls=1 0
0 20000
ob=(2)
fn=(3)
0 20000
totals: 20000
"""
TOOLS = {"gcc": "gcc-12 12.2.0-14+deb12u1", "ld": "binutils 2.40-2", "libc": "libc6 2.36-9+deb12u14"}


class RunTests(unittest.TestCase):
    def prefix(self, recorded: str, scopes: int = 1) -> Path:
        root = scratch.for_case(self, "unityrs-bench-")
        bin_dir = root / "usr" / "bin"
        bin_dir.mkdir(parents=True)
        (bin_dir / "recorded.out").write_text(recorded, encoding="utf-8")
        script = bin_dir / "valgrind"
        script.write_text(FAKE % ("ab" * 32, scopes), encoding="utf-8")
        os.chmod(script, 0o755)
        (root / ".deb-sha256").write_text("cd" * 32 + "\n", encoding="utf-8")
        return root

    def test_a_run_reads_its_file_under_a_clean_environment(self):
        prefix = self.prefix(SCOPE)
        got = callgrind.run_once(Path("/bench/unityrs-bench"), prefix, NAME, Path("/bench/gate/f"), prefix)
        self.assertEqual(got["problems"], [])
        self.assertEqual((got["ir_own"], got["alloc_bytes"], got["digest"]), (109_942, 135_656, "ab" * 32))
        doc, problems = callgrind.measure(Path("/bench/unityrs-bench"), prefix, Path("/bench/gate"),
                                          [{"name": NAME, "fixture": "f"}], {}, "image@sha256:00", prefix,
                                          "rustc 1.99.1\nhost: x86_64-unknown-linux-gnu", TOOLS, say=lambda *_: None)
        self.assertEqual(problems, {})
        self.assertEqual(doc["fingerprint"], {"valgrind_deb_sha256": "cd" * 32, "container": "image@sha256:00",
                                              "rustc": "rustc 1.99.1\nhost: x86_64-unknown-linux-gnu", **TOOLS,
                                              "target": "x86_64-unknown-linux-gnu", "guest_cpu": "sse4.2"})
        self.assertEqual(doc["workloads"][NAME]["mode"], "calibrating")
        self.assertEqual(callgrind.gate_problems(json.loads(json.dumps(doc))), [])

    def test_a_rejected_run_records_no_counts(self):
        prefix = self.prefix(OUTSIDE)
        got = callgrind.run_once(Path("/bench/unityrs-bench"), prefix, NAME, Path("/bench/gate/f"), prefix)
        self.assertTrue(got["problems"][0].startswith("collection was on outside the scope"))
        self.assertNotIn("ir_own", got)

    def test_another_scope_entered_outside_collection_is_refused(self):
        """A setup-time call of another scope leaves no trace in the profile (collection is off there); the bench
        binary's process-wide count of scope entries does."""
        prefix = self.prefix(SCOPE, scopes=2)
        got = callgrind.run_once(Path("/bench/unityrs-bench"), prefix, NAME, Path("/bench/gate/f"), prefix)
        self.assertEqual(got["problems"], ["the process entered 2 scopes, not 1 (setup and digest included)"])
        self.assertNotIn("ir_own", got)

    def test_a_zero_own_count_is_refused_even_calibrating(self):
        self.assertEqual(callgrind.scope_problems(callgrind.parse(ZERO), NAME), [])
        prefix = self.prefix(ZERO)
        got = callgrind.run_once(Path("/bench/unityrs-bench"), prefix, NAME, Path("/bench/gate/f"), prefix)
        self.assertEqual(got["problems"], ["ir_own is 0: no instruction of the bench binary's own was counted"])

    def test_a_failing_valgrind_is_a_problem(self):
        prefix = self.prefix(SCOPE)
        got = callgrind.run_once(Path("/bench/unityrs-bench"), prefix, "texture-DXT1", Path("/bench/gate/f"), prefix)
        self.assertEqual(got["problems"][0][:6], "exit 3")


if __name__ == "__main__":
    unittest.main()
