"""The oracle's pins (requirements-oracle.txt) and oracle_env.py's checks, on synthetic installs.

Standard library only.
"""

import contextlib
import io
import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))

import canonical as C  # noqa: E402
import oracle_env  # noqa: E402


def dist(site: Path, name: str, version: str, record: dict[str, str]):
    """A dist-info folder for `name` `version` in `site`, whose RECORD lists `record` ({path: hash})."""
    info = site / f"{name.replace('-', '_')}-{version}.dist-info"
    info.mkdir(parents=True)
    (info / "METADATA").write_text(f"Metadata-Version: 2.1\nName: {name}\nVersion: {version}\n")
    rows = [f"{path},{digest},1" for path, digest in record.items()] + [f"{info.name}/RECORD,,"]
    (info / "RECORD").write_text("\n".join(rows) + "\n")


class PinTests(unittest.TestCase):
    def test_the_pins(self):
        pins = C.oracle_pins()
        text = C.ORACLE_REQUIREMENTS.read_text()
        self.assertEqual(len(pins), text.count("--hash=sha256:"), "one pin per hashed wheel")
        self.assertEqual(pins["unitypy"], C.ORACLE_UNITYPY)
        self.assertEqual(C.ORACLE_UNITYPY, "1.25.2")
        # The image holds numpy and UnityPy uses it when present, so the oracle pins it.
        self.assertEqual(pins["numpy"], "2.4.6")
        self.assertEqual((pins["pillow"], pins["fmod-toolkit"]), ("9.5.0", "0.1.3"))

    def test_names_compare_as_pep_503_does(self):
        self.assertEqual(
            [C.normalise_package(n) for n in ("Pillow", "fmod_toolkit", "astc.encoder--py", "UnityPy")],
            ["pillow", "fmod-toolkit", "astc-encoder-py", "unitypy"],
        )

    def test_stack_problems(self):
        pins = {"unitypy": "1.25.2", "numpy": "2.4.6"}
        self.assertEqual(C.stack_problems({**pins, "django": "5.2"}, pins), [])
        self.assertEqual(
            C.stack_problems({"unitypy": "1.25.1"}, pins),
            ["numpy is not installed", "unitypy is 1.25.1, not the pinned 1.25.2"],
        )
        # --exact refuses extras, but not the installers pip freeze leaves out.
        installed = {**pins, "pip": "24.0", "setuptools": "79.0.1", "scipy": "1.17.1"}
        self.assertEqual(C.stack_problems(installed, pins, exact=True), ["scipy 1.17.1 is installed but not pinned"])


class EnvTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-oracle-env-"))
        self.addCleanup(shutil.rmtree, self.tmp)

    def test_installed_packages_and_exact(self):
        site = self.tmp / "site"
        dist(site, "UnityPy", "1.25.2", {})
        dist(site, "numpy", "2.4.6", {})
        dist(site, "pip", "24.0", {})
        installed = C.installed_packages([str(site)])
        self.assertEqual(installed, {"unitypy": "1.25.2", "numpy": "2.4.6", "pip": "24.0"})
        pins = {"unitypy": "1.25.2", "numpy": "2.4.6"}
        self.assertEqual(C.stack_problems(installed, pins, exact=True), [])
        dist(site, "scipy", "1.17.1", {})
        self.assertEqual(
            C.stack_problems(C.installed_packages([str(site)]), pins, exact=True),
            ["scipy 1.17.1 is installed but not pinned"],
        )

    def test_records(self):
        here, there = self.tmp / "here", self.tmp / "there"
        files = {"numpy/core.so": "sha256=AAAA", "numpy/__init__.py": "sha256=BBBB"}
        for site, launcher, installer in ((here, "sha256=L1", "sha256=pip"), (there, "sha256=L2", "sha256=uv")):
            dist(
                site,
                "numpy",
                "2.4.6",
                {**files, "../../../bin/f2py": launcher, "numpy-2.4.6.dist-info/INSTALLER": installer},
            )
        pins = {"numpy": "2.4.6"}
        # Launchers and INSTALLER differ by design; nothing else may.
        self.assertEqual(oracle_env.record_problems(pins, str(there), [str(here)]), [])
        shutil.rmtree(there)
        dist(there, "numpy", "2.4.6", {**files, "numpy/core.so": "sha256=CCCC"})
        problems = oracle_env.record_problems(pins, str(there), [str(here)])
        self.assertEqual(len(problems), 1)
        self.assertIn("numpy/core.so is sha256=AAAA here and sha256=CCCC", problems[0])
        self.assertIn("not installed", oracle_env.record_problems({"pillow": "9.5.0"}, str(there), [str(here)])[0])

    def test_the_command_line(self):
        pins = C.oracle_pins()
        cases = [
            (pins, [], 0, "is installed at its version"),
            ({**pins, "numpy": "2.4.5"}, [], 1, "numpy is 2.4.5, not the pinned 2.4.6"),
            ({**pins, "scipy": "1.17.1"}, [], 0, "is installed at its version"),
            ({**pins, "scipy": "1.17.1"}, ["--exact"], 1, "scipy 1.17.1 is installed but not pinned"),
            ({**pins, "pip": "24.0"}, ["--exact"], 0, "and nothing else is"),
        ]
        for installed, args, code, said in cases:
            with self.subTest(args=args, said=said):
                out, err = io.StringIO(), io.StringIO()
                with (
                    mock.patch.object(C, "installed_packages", return_value=installed),
                    contextlib.redirect_stdout(out),
                    contextlib.redirect_stderr(err),
                ):
                    self.assertEqual(oracle_env.main(args), code)
                self.assertIn(said, out.getvalue() + err.getvalue())


if __name__ == "__main__":
    unittest.main()
