"""scripts/unitypy_citations.py on trees built here: a stand-in for UnityPy's package, and a repository citing it.

No test needs UnityPy itself (CI's scripts job has none); CI's parity-py job
runs the script on the repository against the oracle's UnityPy 1.25.2.
"""

import hashlib
import sys
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1]
ROOT = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(ROOT / "tools" / "parity"))

import child  # noqa: E402
import scratch  # noqa: E402
import unitypy_citations as C  # noqa: E402

# Line counts of a stand-in UnityPy: two files share a name, as files/ and enums/ BundleFile.py do.
LINES = {
    "__init__.py": 4,
    "environment.py": 5,
    "files/BundleFile.py": 10,
    "enums/BundleFile.py": 3,
    "helpers/TypeTreeHelper.py": 8,
    "UnityPyBoost/TypeTreeHelper.cpp": 20,
}
HEADER = "// Ported from UnityPy 1.25.2 (MIT): UnityPy/environment.py. See THIRD_PARTY_LICENSES.\n"
# A citation in this file is written `name.py:{line}`, an f-string field for the number, so that the
# repository's own run of the check reads none of these made-up ones.


def write(root: Path, files: dict[str, str]):
    for rel, text in files.items():
        (root / rel).parent.mkdir(parents=True, exist_ok=True)
        (root / rel).write_text(text)


class CitationTests(unittest.TestCase):
    def report(self, files: dict[str, str]):
        with scratch.folder("unityrs-citations-") as tmp:
            write(Path(tmp), files)
            return C.check(Path(tmp), LINES)

    def assert_problem(self, report, text):
        self.assertTrue(any(text in p for p in report.problems), report.problems)

    def test_a_citation_within_its_file_passes(self):
        report = self.report(
            {
                "docs/x.md": (
                    f"`files/BundleFile.py:{3}-10`, `UnityPy/environment.py:{5}`, `UnityPy/files/BundleFile.py:{1}`\n"
                    f"`TypeTreeHelper.cpp:{20}`, `UnityPyBoost/TypeTreeHelper.cpp:{1}-20`, `TypeTreeHelper.py:{1}`\n"
                    f"`__init__.py:{4}`, `enums/BundleFile.py:{3}`\n"
                ),
            }
        )
        self.assertEqual(report.problems, [])
        self.assertEqual((report.checked, report.boost, report.files), (8, 2, 1))

    def test_a_line_that_does_not_exist_fails(self):
        cases = {
            f"`environment.py:{6}`": f"environment.py:{6} is not a range of environment.py's 5 lines",
            f"`files/BundleFile.py:{9}-11`": "is not a range of files/BundleFile.py's 10 lines",
            f"`environment.py:{0}`": f"environment.py:{0} is not a range",
            f"`environment.py:{4}-2`": f"environment.py:{4}-2 is not a range",
            f"`TypeTreeHelper.cpp:{21}`": "is not a range of UnityPyBoost/TypeTreeHelper.cpp's 20 lines",
        }
        for cite, message in cases.items():
            with self.subTest(cite):
                report = self.report({"src/a.py": f"# {cite}\n"})
                self.assertEqual(len(report.problems), 1, report.problems)
                self.assert_problem(report, f"src/a.py:{1}: {cite.strip('`')} ")
                self.assert_problem(report, message)

    def test_a_name_that_two_files_end_with_fails(self):
        report = self.report({"a.md": f"`BundleFile.py:{2}`\n"})
        self.assert_problem(report, "could be any of enums/BundleFile.py, files/BundleFile.py; write the path")

    def test_a_path_unitypy_has_no_file_at_is_not_checked(self):
        report = self.report(
            {
                # One to a line: a line naming a revision (`head_417d595` here) is skipped whole.
                "a.md": (
                    f"`Lib/gzip.py:{999}`\n`tools/parity/compare.py:{999}`\n`calls.py:{2}`\n`Helper.py:{999}`\n"
                    f"`json/__init__.py:{999}`\n`x/environment.py:{9}`\n`tests/head_417d595/files/BundleFile.py:{999}`\n"
                ),
            }
        )
        self.assertEqual((report.problems, report.checked), ([], 0))

    def test_a_line_about_another_revision_is_skipped(self):
        for line in (
            f"HEAD files/BundleFile.py:{999}",
            f"git 417d595: `environment.py:{99}`",
            f"head_417d595 environment.py:{9}",
        ):
            with self.subTest(line):
                report = self.report({"a.md": line + "\n"})
                self.assertEqual((report.problems, report.skipped_lines), ([], 1))
        # A run of hex data, a hex literal or a word is no revision.
        for line in (
            f"bytes 020d000a070c090f040b0a060f030406 at `environment.py:{99}`",
            f"flags 0x1400abc at `environment.py:{99}`",
            f"the head of `environment.py:{99}`",
        ):
            with self.subTest(line):
                report = self.report({"a.md": line + "\n"})
                self.assertEqual(report.skipped_lines, 0)
                self.assert_problem(report, f"environment.py:{99} is not a range")

    def test_a_crate_source_that_cites_unitypy_has_its_header(self):
        cite = f"//! Follows `environment.py:{2}`.\npub fn f() {{}}\n"
        report = self.report({"crates/demo/src/a.rs": cite, "crates/demo/src/b/c.rs": cite})
        self.assert_problem(report, "crates/demo/src/a.rs: cites UnityPy, but has no `// Ported from UnityPy 1.25.2")
        self.assert_problem(report, "crates/demo/src/b/c.rs: cites UnityPy")
        self.assertEqual(self.report({"crates/demo/src/a.rs": HEADER + cite}).problems, [])
        self.assertEqual(self.report({"crates/demo/src/a.rs": HEADER + cite}).ported, 1)
        for header in (
            HEADER.replace("UnityPy 1.25.2", "UnityPy"),
            HEADER.replace("(MIT)", "(Apache-2.0)").replace("UnityPy 1.25.2", "UnityPy 1.25"),
            "//! " + HEADER[3:],
            "\n" + "fn f() {}\n" + HEADER,
        ):
            with self.subTest(header):
                self.assert_problem(self.report({"crates/demo/src/a.rs": header + cite}), "has no `// Ported from")
        # Tests and test modules cite UnityPy to explain what they expect; they port nothing.
        for rel in (
            "crates/demo/src/tests.rs",
            "crates/demo/src/walk/linear_tests.rs",
            "crates/demo/src/env/tests/run.rs",
            "crates/demo/tests/run.rs",
            "crates/demo/examples/x.rs",
            "tools/fixtures/x.py",
        ):
            with self.subTest(rel):
                self.assertEqual(self.report({rel: cite}).problems, [])

    def test_vendored_and_built_files_are_not_read(self):
        bad = f"`environment.py:{99}`\n"
        report = self.report(
            {
                "tools/parity/tests/head_417d595/files/BundleFile.py": bad,
                "target/debug/x.rs": bad,
                ".wt/other/x.md": bad,
                "x.json": bad,
            }
        )
        self.assertEqual((report.problems, report.checked), ([], 0))
        self.assertEqual(len(self.report({"docs/x.md": bad, "scripts/run": bad}).problems), 2)


class SourceTests(unittest.TestCase):
    def test_the_package_and_the_pinned_boost_sources(self):
        with scratch.folder("unityrs-citations-") as tmp:
            write(Path(tmp), {"UnityPy/environment.py": "a\nb\n", "UnityPy/files/File.py": "x\n"})
            lines = C.unitypy_lines(Path(tmp) / "UnityPy", None)
        self.assertEqual(lines["environment.py"], 2)
        self.assertEqual(lines["files/File.py"], 1)
        self.assertEqual(lines["UnityPyBoost/TypeTreeHelper.cpp"], C.BOOST["UnityPyBoost/TypeTreeHelper.cpp"][0])

    def test_a_boost_copy_must_be_the_pinned_one(self):
        with scratch.folder("unityrs-citations-") as tmp:
            write(Path(tmp), {"UnityPy/environment.py": "a\n"})
            boost = Path(tmp) / "boost"
            boost.mkdir()
            for name in C.BOOST:
                (boost / Path(name).name).write_text("not the source\n")
            with self.assertRaisesRegex(ValueError, "is not UnityPy 1.25.2's UnityPyBoost/"):
                C.unitypy_lines(Path(tmp) / "UnityPy", boost)

    def test_the_pins_are_well_formed(self):
        for name, (count, sha) in C.BOOST.items():
            with self.subTest(name):
                self.assertTrue(name.startswith("UnityPyBoost/") and count > 0)
                self.assertRegex(sha, r"^[0-9a-f]{64}$")
        self.assertNotEqual(hashlib.sha256(b"").hexdigest(), C.BOOST["UnityPyBoost/Mesh.hpp"][1])

    def test_the_command_line(self):
        script = SCRIPTS / "unitypy_citations.py"
        with scratch.folder("unityrs-citations-") as tmp:
            write(Path(tmp), {"pkg/environment.py": "a\nb\n", "repo/a.md": f"`environment.py:{2}`\n"})
            args = [sys.executable, str(script), str(Path(tmp) / "repo"), "--unitypy", str(Path(tmp) / "pkg")]
            ok = child.run(args, capture_output=True, text=True)
            self.assertEqual((ok.returncode, ok.stderr), (0, ""))
            self.assertIn("1 citations of UnityPy 1.25.2 in 1 files name lines that exist", ok.stdout)
            write(Path(tmp), {"repo/a.md": f"`environment.py:{3}`\n"})
            bad = child.run(args, capture_output=True, text=True)
            self.assertEqual(bad.returncode, 1)
            self.assertIn(f"a.md:1: environment.py:{3} is not a range of environment.py's 2 lines", bad.stderr)
            (Path(tmp) / "boost").mkdir()
            usage = child.run([*args, "--boost", str(Path(tmp) / "boost")], capture_output=True, text=True)
            self.assertEqual(usage.returncode, 2)


if __name__ == "__main__":
    unittest.main()
