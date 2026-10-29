"""scripts/unittest-suites on synthetic suites: the gate CI runs the Python suites through.

Standard library only.
"""

import os
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "unittest-suites"

PASSING = """
import unittest

class T(unittest.TestCase):
    def test_one(self):
        pass

    def test_two(self):
        pass
"""
SKIPPING = """
import unittest

class T(unittest.TestCase):
    def test_runs(self):
        pass

    def test_skips(self):
        self.skipTest("a sample is missing")
"""
FAILING = """
import unittest

class T(unittest.TestCase):
    def test_fails(self):
        self.fail("wrong")
"""
EXPECTED_FAILURE = """
import unittest

class T(unittest.TestCase):
    @unittest.expectedFailure
    def test_known(self):
        self.fail("known")
"""


class GateTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-gate-"))
        self.addCleanup(shutil.rmtree, self.tmp)

    def suite(self, name, files):
        folder = self.tmp / name
        for rel, body in files.items():
            (folder / rel).parent.mkdir(parents=True, exist_ok=True)
            (folder / rel).write_text(textwrap.dedent(body))
        folder.mkdir(exist_ok=True)
        return folder

    def gate(self, *args, actions=False):
        env = {k: v for k, v in os.environ.items() if k != "GITHUB_ACTIONS"}
        if actions:
            env["GITHUB_ACTIONS"] = "true"
        proc = subprocess.run(
            [sys.executable, str(SCRIPT), *map(str, args)], capture_output=True, text=True, env=env, cwd=self.tmp
        )
        return proc.returncode, proc.stdout + proc.stderr

    def test_a_suite_that_runs_its_tests_passes(self):
        ok = self.suite("ok", {"test_ok.py": PASSING})
        code, out = self.gate(f"{ok}=2")
        self.assertEqual(code, 0, out)
        self.assertIn("all 1 suites passed", out)

    def test_a_suite_that_runs_no_test_fails(self):
        # unittest on Python 3.11 ends both of these in "Ran 0 tests ... OK".
        cases = {
            "empty": {},
            "hidden below a folder discovery skips": {"sub/test_hidden.py": FAILING},
            "renamed away from test*.py": {"check_ok.py": PASSING},
        }
        for label, files in cases.items():
            with self.subTest(label):
                folder = self.suite(label.replace(" ", "_"), files)
                code, out = self.gate(f"{folder}=1")
                self.assertEqual(code, 1, out)
                self.assertIn("ran 0 tests, fewer than the 1 it holds", out)

    def test_a_suite_below_its_minimum_fails(self):
        ok = self.suite("ok", {"test_ok.py": PASSING})
        code, out = self.gate(f"{ok}=3")
        self.assertEqual(code, 1, out)
        self.assertIn("ran 2 tests, fewer than the 3 it holds", out)

    def test_a_skip_a_failure_or_an_expected_failure_fails(self):
        cases = {
            "skip": (SKIPPING, "OK (skipped=1)"),
            "failure": (FAILING, "unittest exited 1: FAILED (failures=1)"),
            "expected failure": (EXPECTED_FAILURE, "OK (expected failures=1)"),
        }
        for label, (body, said) in cases.items():
            with self.subTest(label):
                folder = self.suite(label.replace(" ", "_"), {"test_x.py": body})
                code, out = self.gate(f"{folder}=1")
                self.assertEqual(code, 1, out)
                self.assertIn(said, out)

    def test_every_suite_runs_and_each_failure_is_reported(self):
        bad = self.suite("bad", {"test_bad.py": FAILING})
        empty = self.suite("empty", {})
        ok = self.suite("ok", {"test_ok.py": PASSING})
        code, out = self.gate(f"{bad}=1", f"{empty}=1", f"{ok}=2", actions=True)
        self.assertEqual(code, 1, out)
        self.assertIn(f"::error::{bad}: unittest exited 1", out)
        self.assertIn(f"::error::{empty}: ran 0 tests", out)
        self.assertIn(f"::group::{ok}", out)
        self.assertIn("2 of 3 suites failed", out)

    def test_usage(self):
        ok = self.suite("ok", {"test_ok.py": PASSING})
        for args in ((), (str(ok),), (f"{ok}=0",), (f"{ok}=x",), ("=3",)):
            with self.subTest(args):
                self.assertEqual(self.gate(*args)[0], 2)


if __name__ == "__main__":
    unittest.main()
