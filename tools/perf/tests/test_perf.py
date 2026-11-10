"""The perf tools: the summed-PSS sampler and its load accounting, UnityPyBoost's bench on synthetic inputs (the
oracle stack, as CI's parity job has it), perf.py's passes and folder rule, and the report's aggregates."""

from __future__ import annotations

import json
import struct
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
PERF = HERE.parent
ROOT = PERF.parents[1]
sys.path.insert(0, str(PERF))
sys.path.insert(0, str(ROOT / "tools" / "parity"))
sys.path.insert(0, str(ROOT / "tools" / "parity" / "tests"))

import child  # noqa: E402
import mem_peak  # noqa: E402
import perf  # noqa: E402
import report  # noqa: E402
import scratch  # noqa: E402
import synth as S  # noqa: E402


class SamplerTests(unittest.TestCase):
    def test_the_peak_is_the_childs_summed_pss(self):
        """A child that holds 64 MiB written to: its peak PSS is at least that; this process is not counted."""
        hold = "import time; b = b'x' * (64 << 20); time.sleep(0.4)"
        proc, rep = mem_peak.measure([sys.executable, "-c", hold], capture_output=True)
        self.assertEqual((proc.returncode, rep["exit"]), (0, 0))
        self.assertGreaterEqual(rep["peak_pss_sum_bytes"], 64 << 20)
        self.assertLess(rep["peak_pss_sum_bytes"], 256 << 20)
        self.assertGreaterEqual(rep["peak_rss_sum_bytes"], rep["peak_pss_sum_bytes"])
        self.assertEqual(rep["max_processes"], 1)
        self.assertGreater(rep["samples"], 2)
        load = rep["load"]
        self.assertEqual(len(load["loadavg_before"]), 3)
        self.assertGreaterEqual(load["others_cpu_s"], 0)
        self.assertGreaterEqual(load["box_busy_cores"], 0)

    def test_descendants_follow_the_parent_links(self):
        procs = {1: (0, 0), 10: (1, 0), 11: (10, 0), 12: (11, 0), 20: (1, 0), 30: (99, 0)}
        self.assertEqual(mem_peak.descendants(10, procs), {11, 12})
        self.assertEqual(mem_peak.descendants(1, procs), {10, 11, 12, 20})

    def test_others_count_from_first_sample_or_from_zero_when_started_later(self):
        """A process seen at the first sample counts what it used after it; one first seen later counts all
        it used (it started during the run); our own processes count nothing."""
        o = mem_peak.Others()
        o.sample({1: (0, 100), 2: (0, 50)}, ours={2})
        o.sample({1: (0, 300), 2: (0, 900), 3: (0, 40)}, ours={2})
        self.assertAlmostEqual(o.cpu_s(), (200 + 40) / mem_peak.TICK)


def bundle_and_assets(folder: Path) -> tuple[Path, int]:
    """A bundle holding a SerializedFile of 3 objects and a plain SerializedFile of 2, with an index naming them
    a bundle and a UnityPy sample: (the index, the objects)."""
    tree = S.Node("MyThing", "Base", [S.Node("int", "i"), S.Node("bool", "b", meta=S.ALIGN)])
    rows = [S.Obj(p, 0, struct.pack("<i", p) + b"\1\0\0\0") for p in (1, 2, 3)]
    sf = S.serialized_file([S.Type(114, tree)], rows)
    (folder / "b.bundle").write_bytes(S.fs_bundle([(b"CAB-0001", sf, 4)]))
    (folder / "s.assets").write_bytes(S.serialized_file([S.Type(114, tree)], rows[:2]))
    files = [{"path": "b.bundle", "kind": "bundle"}, {"path": "s.assets", "kind": "unitypy-sample"}]
    (folder / "index.json").write_text(json.dumps({"files": files}))
    return folder / "index.json", 5


class UnityPyBenchTests(unittest.TestCase):
    def test_serial_and_parallel_read_every_object_of_the_selection(self):
        folder = scratch.for_case(self, "unityrs-perf-")
        index, objects = bundle_and_assets(folder)
        size = (folder / "b.bundle").stat().st_size + (folder / "s.assets").stat().st_size
        script = str(PERF / "bench_unitypy.py")
        for extra in (["--phase", "serial"], ["--phase", "parallel", "-j", "2"]):
            with self.subTest(extra=extra):
                proc = child.run(
                    [sys.executable, script, str(index), *extra],
                    capture_output=True,
                    text=True,
                    preexec_fn=perf.limit_address_space,  # every UnityPy run under perf.py's ulimit -v
                )
                self.assertEqual(proc.returncode, 0, proc.stderr)
                out = json.loads(proc.stdout)
                self.assertEqual((out["bench"], out["unitypy"]), ("unitypy", "1.25.2"))
                self.assertEqual((out["inputs"], out["input_bytes"], out["objects"]), (2, size, objects))
                self.assertEqual((out["file_errors"], out["value_errors"]), (0, 0))
                self.assertGreater(out["mb_per_s"], 0)
        proc = child.run(
            [sys.executable, script, str(index), "--kind", "bundle"],
            capture_output=True,
            text=True,
            check=True,
            preexec_fn=perf.limit_address_space,
        )
        self.assertEqual(json.loads(proc.stdout)["objects"], 3, "--kind keeps the bundle only")


class PlanTests(unittest.TestCase):
    def test_passes(self):
        out = Path("/nowhere/out")
        timed, once = perf.plan("/i/index.json", 8, None, out)
        names = [p["name"] for p in timed]
        self.assertEqual(len(names), len(set(names)))
        self.assertEqual([p["name"] for p in timed if p["unitypy"]], ["unitypy-serial", "unitypy-parallel"])
        by = {p["name"]: p["argv"] for p in timed + once}
        self.assertTrue(by["unityrs-serial-nocounters"][0].endswith("target/perf-nocounters/release/examples/bench"))
        self.assertEqual(by["unityrs-parallel"][-2:], ["-j", "8"])
        self.assertNotIn("--pin", by["unityrs-tracked-serial"], "kept values would raise each input's peak")
        self.assertIn("--pin", by["unityrs-pin"])
        self.assertEqual(by["unityrs-lazy-r2-sample"][2:6], ["--kind", "bundle", "--sample", "20"])
        trial_timed, trial_once = perf.plan("/i/index.json", 4, 100, out)
        self.assertIn("100", trial_timed[0]["argv"])
        r2 = next(p for p in trial_once if p["name"] == "unityrs-lazy-r2-sample")["argv"]
        self.assertEqual(r2[2:6], ["--kind", "bundle", "--sample", "20"], "R2's rule whatever the trial's sample")
        self.assertNotIn("100", r2)
        many = next(p for p in timed if p["name"] == report.MANY_BLOCKS)
        self.assertEqual((many["argv"][1:], many["unitypy"]), (["--many-blocks"], False))
        trial_many = next(p for p in trial_timed if p["name"] == report.MANY_BLOCKS)
        self.assertEqual(trial_many["argv"][1:], ["--many-blocks"], "--sample does not apply")
        builds = perf.build_commands()
        self.assertEqual(builds[1][1], {"RUSTFLAGS": "--cfg unityrs_no_counters"})
        self.assertIn("-j", builds[0][0])

    def test_the_output_folder_is_outside_the_repository_and_empty(self):
        with self.assertRaises(SystemExit):
            perf.check_out(ROOT / "target" / "perf-out")
        folder = scratch.for_case(self, "unityrs-perf-")
        perf.check_out(folder / "new")
        self.assertTrue((folder / "new").is_dir())
        (folder / "new" / "x").write_text("")
        with self.assertRaises(SystemExit):
            perf.check_out(folder / "new")


def record(phase, wall, round_=1, bench="unityrs", **more):
    b = {
        "bench": bench,
        "inputs": 10,
        "input_bytes": 1000,
        "objects": 100,
        "wall_s": wall,
        "objects_per_s": 100 / wall,
        "mb_per_s": 1000 / 1e6 / wall,
        "file_errors": 0,
        "value_errors": 0,
        "decode_share": 0.5,
        "cpu_s": {"threads": wall, "decode": wall / 2},
        "clock": "thread_cpu",
        "counters": True,
        **more,
    }
    load = {"loadavg_before": [1.0, 1, 1], "loadavg_after": [2.0, 1, 1], "others_cores": 0.5, "box_busy_cores": 2}
    mem = {"peak_pss_sum_bytes": 5_000_000, "max_processes": 1, "load": load}
    return {"phase": phase, "round": round_, "bench": b, "mem": mem}


class ReportTests(unittest.TestCase):
    def records(self, unitypy_serial_wall):
        recs = []
        for r in (1, 2):
            recs += [
                record("unityrs-serial", 2.0 + r, r),
                record("unityrs-serial-nocounters", 2.0 + r, r),
                record("unityrs-serial-counting", 2.5 + r, r),
                record("unitypy-serial", unitypy_serial_wall + r, r, bench="unitypy"),
            ]
        return recs

    def test_ratios_cost_and_ranges(self):
        s = report.summarise(self.records(6.0), {"started_utc": "t", "repeat": 2, "jobs": 8, "sample": None})
        serial = s["r4"]["serial_unityrs_over_unitypy"]
        self.assertAlmostEqual(serial["median"], ((1 / 3 + 1 / 4) / 2) / ((1 / 7 + 1 / 8) / 2))
        self.assertAlmostEqual(serial["range"][0], 7 / 4)  # the slowest unityrs run over the fastest UnityPy one
        self.assertFalse(s["r4"]["blocker_slower_serially"])
        self.assertAlmostEqual(s["counters_cost_serial"]["median_change"], 0)
        self.assertAlmostEqual(s["r10_counting_over_unbounded"]["serial"]["median_change"], 4.0 / 3.5 - 1)
        self.assertAlmostEqual(s["r10_counting_over_unbounded"]["serial"]["median_cpu_change"], 4.0 / 3.5 - 1)
        self.assertEqual(s["counters_cost_serial"]["cpu_s"], [3.0, 4.0])
        self.assertEqual(s["phases"]["unityrs-serial"]["wall_s"], [3.0, 4.0])
        self.assertEqual(len(s["phases"]["unityrs-serial"]["per_run"]), 2)
        self.assertTrue(s["consistency"]["same_inputs_bytes_objects"])
        md = report.markdown(s)
        self.assertIn("| unityrs-serial | 2 | 3.00-4.00 |", md)
        self.assertNotIn("Blocker", md)
        json.dumps(s)

    def test_slower_serially_is_a_blocker(self):
        s = report.summarise(self.records(1.0), {"started_utc": "t", "repeat": 2, "jobs": 8, "sample": None})
        self.assertTrue(s["r4"]["blocker_slower_serially"])
        self.assertIn("Blocker", report.markdown(s))

    def test_runs_of_many_blocks_are_summarised_apart(self):
        """`bench --many-blocks` records (no corpus inputs) give their cases' ranges over the rounds, and leave the
        corpus passes' consistency and table alone."""
        recs = self.records(6.0)
        for r, ms in ((1, 40.0), (2, 30.0)):
            case = {"bytes": 64 << 20, "block": 16 << 10, "blocks": 4096, "reads": 5, "best_ms": ms,
                    "median_ms": ms + 5, "best_mb_per_s": 67.108864 / (ms / 1e3)}
            many = record(report.MANY_BLOCKS, ms / 100, r)
            many["bench"] = {"bench": "unityrs-many-block-runs", "wall_s": ms / 100, "cases": [case]}
            recs.append(many)
        s = report.summarise(recs, {"started_utc": "t", "repeat": 2, "jobs": 8, "sample": None})
        [c] = s["many_block_runs"]
        self.assertEqual((c["blocks"], c["best_ms"], c["median_ms"]), (4096, [30.0, 40.0], [35.0, 45.0]))
        self.assertEqual([r["round"] for r in c["per_run"]], [1, 2])
        self.assertNotIn(report.MANY_BLOCKS, s["phases"])
        self.assertTrue(s["consistency"]["same_inputs_bytes_objects"])
        self.assertIn("| 64 | 16 | 4096 | 30.0-40.0 | 35.0-45.0 |", report.markdown(s))
        none = report.summarise(self.records(6.0), {"started_utc": "t", "repeat": 2, "jobs": 8, "sample": None})
        self.assertIsNone(none["many_block_runs"])

    def test_a_pass_that_saw_other_objects_is_flagged(self):
        recs = self.records(6.0)
        recs[0]["bench"]["objects"] = 99
        s = report.summarise(recs, {"started_utc": "t", "repeat": 2, "jobs": 8, "sample": None})
        self.assertFalse(s["consistency"]["same_inputs_bytes_objects"])


if __name__ == "__main__":
    unittest.main()
