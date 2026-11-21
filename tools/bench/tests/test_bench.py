"""bench.py's checks: `check --wall-run` (plan section 5.2) against a fake GitHub API and history, each refusal
and the change that has nothing to verify; and the fixtures' manifest check, which re-hashes every file.
Standard library only."""

import io
import json
import types
import unittest
import zipfile

from support import bench, scratch_tree, wall

REPO = "owner/repo"


def zipped(data: bytes) -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        z.writestr("wall.json", data)
    return buf.getvalue()


class WallRunTests(unittest.TestCase):
    def setUp(self):
        self.doc = wall()
        self.root = scratch_tree(self, self.doc)
        self.path = self.root / "benchmarks" / "wall.json"
        self.run = {"path": bench.WORKFLOW, "event": "workflow_dispatch", "head_branch": "main",
                    "conclusion": "success", "head_sha": self.doc["commit"]}
        self.artifacts = {"artifacts": [{"name": "bench-wall-12345-1", "id": 9}]}
        self.artifact = zipped(self.path.read_bytes())
        self.changed_wall, self.measured = True, ""
        # The run's commit's sources: the checkout's, and a helper only that commit imported.
        self.at_run = {**bench.checkout_sources(), "tools/perf/old_helper.py": "",
                       "tools/bench/py_workloads.py": "import old_helper\n"}

    def gh(self, path: str) -> bytes:
        if path == f"repos/{REPO}/actions/runs/12345/attempts/1":
            return json.dumps(self.run).encode()
        if path == f"repos/{REPO}/actions/runs/12345/artifacts?per_page=100":
            return json.dumps(self.artifacts).encode()
        if path == f"repos/{REPO}/actions/artifacts/9/zip":
            return self.artifact
        raise AssertionError(path)

    def git(self, *args):
        if args[:2] == ("diff", "--quiet"):
            return (1 if self.changed_wall else 0), ""
        if args[:2] == ("diff", "--name-only"):
            self.assertEqual(args[2], str(self.run.get("head_sha")))
            self.assertEqual(args[4:], ("--", *bench.measured_paths(bench.checkout_sources(), self.at_run)))
            self.assertIn("tools/perf/old_helper.py", args[5:])
            self.assertIn("tools/perf/mem_peak.py", args[5:])
            return 0, self.measured
        raise AssertionError(args)

    def check(self, run_id=None):
        return bench.check_wall_run(run_id, "base", REPO, gh=self.gh, git=self.git, wall_path=self.path,
                                    say=lambda _: None, sources_at=self.sources_at)

    def sources_at(self, sha):
        if sha != self.doc["commit"]:
            raise bench.child.CalledProcessError(128, ["git", "archive"])
        return self.at_run

    def test_the_named_runs_artifact_passes(self):
        self.assertEqual(self.check(), [])
        self.assertEqual(self.check("12345"), [])

    def test_a_change_that_leaves_wall_json_alone_has_nothing_to_verify(self):
        self.changed_wall = False
        self.run = {}
        self.assertEqual(self.check(), [])

    def test_each_property_of_the_run_is_checked(self):
        for key, value in (("path", ".github/workflows/ci.yml"), ("event", "push"), ("head_branch", "topic"),
                           ("conclusion", "failure"), ("head_sha", "f" * 40)):
            with self.subTest(key):
                self.setUp()
                self.run[key] = value
                bad = self.check()
                self.assertTrue(bad and any(key in b or "workflow" in b or "commit" in b for b in bad), bad)

    def test_a_change_to_a_measured_path_fails(self):
        self.measured = "crates/unityrs-core/src/lib.rs\n"
        bad = self.check()
        self.assertEqual(len(bad), 2)  # against the base and against the head
        self.assertIn("crates/unityrs-core/src/lib.rs", bad[0])

    def test_an_artifact_that_differs_or_is_missing_fails(self):
        self.artifact = zipped(self.path.read_bytes() + b" ")
        self.assertEqual(self.check(), ["benchmarks/wall.json is not byte-equal to run 12345's artifact "
                                        "bench-wall-12345-1"])
        self.artifacts = {"artifacts": []}
        self.assertEqual(self.check(), ["run 12345: 0 artifacts named bench-wall-12345-1"])

    def test_another_cpus_results_file_is_checked_by_its_own_name(self):
        self.path = self.path.rename(self.path.with_name("wall-other.json"))
        self.assertEqual([p.name for p in bench.wall_paths(self.root)], ["wall-other.json"])
        self.assertEqual(self.check(), [])
        self.artifact = zipped(self.path.read_bytes() + b" ")
        self.assertEqual(self.check("777"), ["run 777 is not wall-other.json's run 12345",
                                             "benchmarks/wall-other.json is not byte-equal to run 12345's artifact "
                                             "bench-wall-12345-1"])

    def test_another_run_id_or_an_emulated_run_fails(self):
        self.assertEqual(self.check("777"), ["run 777 is not wall.json's run 12345"])
        self.path.write_text(bench.dump_json(wall(emulated=True)), encoding="utf-8")
        self.artifact = zipped(self.path.read_bytes())
        self.assertTrue(any("emulated" in b for b in self.check()))


class MeasuredPathTests(unittest.TestCase):
    def test_the_workflow_build_config_and_every_imported_helper_are_measured(self):
        paths = bench.measured_paths()
        for path in (bench.WORKFLOW, ".cargo/", "crates/", "rust-toolchain.toml", "tools/perf/mem_peak.py",
                     "tools/perf/bench_unitypy.py", "tools/parity/child.py", "tools/parity/scratch.py",
                     "tools/parity/unitypy_dump.py"):
            self.assertIn(path, paths)
        self.assertFalse([p for p in paths if p.startswith("tools/bench/") and p != "tools/bench/"], paths)

    def test_imports_are_followed_from_source_at_any_depth_and_trees_are_unioned(self):
        sources = {
            "tools/bench/bench.py": "import json\n\ndef f():\n    import b\n",
            "tools/bench/b.py": "from c import x\nimport pkg.sub, lone\nimport UnityPy\n",
            "tools/parity/c.py": "x = 1\n",
            "tools/parity/unused.py": "",
            "tools/perf/pkg/__init__.py": "",
            "tools/perf/pkg/sub.py": "from c import x\n",
            "tools/bench/lone.py": "",
        }
        got = bench.local_imports(["tools/bench/bench.py"], sources)
        self.assertEqual(got, {"tools/bench/bench.py", "tools/bench/b.py", "tools/parity/c.py", "tools/perf/pkg/",
                               "tools/perf/pkg/sub.py", "tools/perf/pkg/__init__.py", "tools/bench/lone.py"})
        other = {"tools/bench/py_workloads.py": "import gone\n", "tools/perf/gone.py": ""}
        paths = bench.measured_paths(sources, other)
        self.assertIn("tools/parity/c.py", paths)
        self.assertIn("tools/perf/gone.py", paths)
        self.assertIn("tools/perf/pkg/", paths)


class ChildResultTests(unittest.TestCase):
    def proc(self, code, out, err):
        return types.SimpleNamespace(returncode=code, stdout=out, stderr=err)

    def test_a_crash_a_kill_a_failed_setup_or_no_valid_result_stops_the_run_with_stderr_kept_locally(self):
        log = scratch_tree(self) / "logs" / "0-x-unityrs.stderr"
        good = {"status": "ok", "scope_s": 1.0, "counts": {"objects": 3}, "digest": "ab" * 32}
        ok = json.dumps(good)
        self.assertEqual(bench.child_result(self.proc(0, ok + "\n", ""), "x unityrs", log), good)
        self.assertFalse(log.exists())
        secret = "/private/secret"
        bad_results = [
            {**good, "status": f"ValueError: {secret}"}, {**good, "scope_s": secret}, {**good, "scope_s": -1},
            {**good, "extra": secret}, {**good, "counts": {secret: 1}}, {**good, "counts": {"objects": True}},
            {**good, "digest": secret}, {**good, "status": "raised:ValueError"}, {k: good[k] for k in ("status",)},
        ]
        procs = [self.proc(-9, ok, "Killed " + secret), self.proc(-11, "", "Segmentation fault\n"),
                 self.proc(1, ok, "Traceback\n"), self.proc(0, "", ""), self.proc(0, "not json", ""),
                 self.proc(0, ok.replace("1.0", "NaN"), ""), self.proc(0, ok.replace("1.0", "Infinity"), ""),
                 *(self.proc(0, json.dumps(r), "") for r in bad_results)]
        for proc in procs:
            with self.assertRaises(bench.ChildFailed) as cm:
                bench.child_result(proc, "x unityrs", log)
            self.assertNotIn(secret, str(cm.exception))
        self.assertEqual(log.read_text(encoding="utf-8"), "Traceback\n")  # the last stderr written


class ManifestTests(unittest.TestCase):
    def test_every_file_is_rehashed_against_the_recorded_manifest(self):
        root = scratch_tree(self)
        folder = root / "fixtures"
        folder.mkdir()
        (folder / "a.bundle").write_bytes(b"alpha")
        import hashlib

        recorded = {"files": [{"name": "a.bundle", "bytes": 5, "sha256": hashlib.sha256(b"alpha").hexdigest()}],
                    "inventories": {"a.bundle": {"kind": "x"}}}
        (folder / "manifest.json").write_text(json.dumps(recorded), encoding="utf-8")
        self.assertEqual(bench.manifest_problems(folder, recorded), [])
        (folder / "a.bundle").write_bytes(b"alphA")
        self.assertEqual(bench.manifest_problems(folder, recorded), ["a.bundle: the file on disk is not the recorded one"])
        (folder / "manifest.json").write_text("{}", encoding="utf-8")
        self.assertEqual(bench.manifest_problems(folder, recorded),
                         ["a.bundle: recorded, not made", "a.bundle: the file on disk is not the recorded one"])


if __name__ == "__main__":
    unittest.main()
