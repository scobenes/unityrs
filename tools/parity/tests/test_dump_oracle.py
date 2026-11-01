"""unitypy_dump.py refuses to dump unless it is the pinned oracle.

UnityPy, UnityPyBoost and its binary, every package requirements-oracle.txt
pins installed from its wheel, no module loaded from outside the pins (at
import or while an input is dumped), and no PYTHONPATH or user site. Needs the
oracle stack; skipped elsewhere.
"""

import contextlib
import importlib
import io
import json
import os
import re
import shutil
import sys
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

import child  # noqa: E402
import canonical as C  # noqa: E402
import oracle_env  # noqa: E402
from dump_harness import DumpCase, dump, dump_process, dump_records, unitypy_dump  # noqa: E402


@unittest.skipIf(unitypy_dump is None, "needs UnityPy (the oracle image)")
class DumpOracleTests(DumpCase):
    def test_refuses_without_the_pinned_oracle(self):
        tiny = self.tmp / "tiny"
        tiny.write_bytes(b"tiny")
        out = self.tmp / "x.jsonl"
        installed = C.installed_packages()
        without_numpy = {k: v for k, v in installed.items() if k != "numpy"}
        for label, patch, said in (
            ("no Boost", mock.patch.object(unitypy_dump, "HAVE_BOOST", False), "UnityPyBoost is not loaded"),
            ("UnityPy", mock.patch.object(unitypy_dump.UnityPy, "__version__", "1.24.0"), "UnityPy is 1.24.0"),
            # Another build of UnityPyBoost beside UnityPy 1.25.2.
            ("Boost build", mock.patch.object(unitypy_dump, "BOOST_SHA256", "0" * 64), "UnityPyBoost has sha256 000"),
            # numpy absent: UnityPy then renders sprites with its pure-Python solver.
            ("numpy", mock.patch.object(C, "installed_packages", return_value=without_numpy), "numpy is not installed"),
            (
                "Pillow",
                mock.patch.object(C, "installed_packages", return_value={**installed, "pillow": "10.0.0"}),
                "pillow is 10.0.0, not the pinned 9.5.0",
            ),
            # Every version right, but a file that is not the pinned wheel's (test_refuses_a_patched_unitypy).
            (
                "a wheel's files",
                mock.patch.object(oracle_env, "installed_wheel_problems", return_value=(("numpy: x hashes as y",), {})),
                "numpy: x hashes as y (oracle_records.json)",
            ),
        ):
            with self.subTest(label):
                with patch, contextlib.redirect_stderr(io.StringIO()) as err, self.assertRaises(SystemExit):
                    unitypy_dump.main([str(tiny), "-o", str(out), "-q"])
                self.assertIn("not the pinned oracle", err.getvalue())
                self.assertIn(said, err.getvalue())
                self.assertFalse(out.exists())

    def test_refuses_a_patched_unitypy(self):
        """A copy of UnityPy with one line changed, imported before the installed one, is refused.

        With its dist-info the copy is the distribution Python finds, and its
        edited file no longer hashes as its RECORD says; without one, the
        installed distribution checks out, but `UnityPy` imports from
        elsewhere. A faithful copy with its dist-info passes, so the line is
        what is refused. The same edit in a package linked in beside the
        module, which Python imports in the module's place, is refused as a
        symlink. The version and the UnityPyBoost binary are the pinned ones
        throughout.
        """
        tiny = self.tmp / "tiny"
        tiny.write_bytes(b"tiny")
        site = Path(unitypy_dump.UnityPy.__file__).resolve().parents[1]
        info = site / f"unitypy-{C.ORACLE_UNITYPY}.dist-info"
        self.assertTrue(info.is_dir())

        def patched(text):
            self.assertEqual(text.count("is_stripped = None"), 1)
            return text.replace("is_stripped = None", "is_stripped = 7")

        def edit(files):
            reader = files / "ObjectReader.py"
            reader.write_text(patched(reader.read_text(encoding="utf-8")), encoding="utf-8")

        def link(files):
            # The patched module as a package elsewhere, its relative imports one level deeper.
            package = files.parents[2] / "linked" / "ObjectReader"
            package.mkdir(parents=True)
            text = re.sub(r"^(\s*from \.)", r"\1.", (files / "ObjectReader.py").read_text(encoding="utf-8"), flags=re.M)
            (package / "__init__.py").write_text(patched(text), encoding="utf-8")
            (files / "ObjectReader").symlink_to(package, target_is_directory=True)

        for n, (label, with_info, change, code, said) in enumerate(
            (
                ("a faithful copy with its dist-info", True, None, 0, ""),
                ("a patched copy with its dist-info", True, edit, 2, "UnityPy/files/ObjectReader.py hashes as"),
                ("a patched copy alone", False, edit, 2, "module UnityPy imports from"),
                ("a patched package linked in", True, link, 2, "UnityPy/files/ObjectReader is a symlink"),
            )
        ):
            with self.subTest(label):
                shadow = self.tmp / f"case{n}" / "shadow"
                shutil.copytree(site / "UnityPy", shadow / "UnityPy", ignore=shutil.ignore_patterns("__pycache__"))
                if with_info:
                    shutil.copytree(info, shadow / info.name)
                if change:
                    change(shadow / "UnityPy" / "files")
                out = self.tmp / f"patched{n}.jsonl"
                proc = dump_process(tiny, "-o", out, first=[shadow])
                self.assertEqual(proc.returncode, code, proc.stderr.decode(errors="replace"))
                self.assertEqual(out.exists(), code == 0)
                if code:
                    self.assertIn(b"not the pinned oracle", proc.stderr)
                    self.assertIn(said.encode(), proc.stderr)

    def test_refuses_a_module_from_outside_the_pins(self):
        """A module loaded from neither the standard library nor a pinned wheel, at import or in a dump, is refused.

        Pillow imports defusedxml when there is one, as it imports cffi and
        fsspec zstandard; one first on sys.path would run code that is not the
        oracle's. So would a copy of a standard-library module, such as json,
        which writes every line: it is refused even when it was loaded before
        the script started. test_samples is the check that the real dumps load
        none.
        """
        tiny, out = self.tmp / "tiny", self.tmp / "x.jsonl"
        tiny.write_bytes(b"tiny")
        planted = self.tmp / "planted"
        (planted / "defusedxml").mkdir(parents=True)
        (planted / "defusedxml" / "__init__.py").write_text("")
        (planted / "defusedxml" / "ElementTree.py").write_text("")
        (planted / "unityrs_planted.py").write_text("X = 1\n")
        stdlib = Path(json.__file__).resolve().parents[1]
        copies = self.tmp / "json_copy", self.tmp / "csv_copy"
        shutil.copytree(stdlib / "json", copies[0] / "json", ignore=shutil.ignore_patterns("__pycache__"))
        copies[1].mkdir()
        shutil.copy(stdlib / "csv.py", copies[1] / "csv.py")
        # At import, and at start-up.
        for label, folder, file, why, preload in (
            ("defusedxml", planted, "defusedxml/__init__.py", "which is neither the standard library nor", ()),
            ("json, imported by the script", copies[0], "json/__init__.py", "which shadows the standard library's", ()),
            ("json, at start-up", copies[0], "json/__init__.py", "which shadows the standard library's json", ["json"]),
            ("csv, imported by oracle_env", copies[1], "csv.py", "which shadows the standard library's csv", ()),
        ):
            with self.subTest(label):
                proc = dump_process(tiny, "-o", out, first=[folder], preload=preload)
                self.assertEqual(proc.returncode, 2, proc.stderr.decode(errors="replace"))
                self.assertIn(b"not the pinned oracle", proc.stderr)
                name = Path(file).parts[0].removesuffix(".py")
                said = f"module {name} is loaded from {(folder / file).resolve()}, {why}"
                self.assertIn(said.encode(), proc.stderr)
                self.assertFalse(out.exists())
        # While an input is dumped, in its own process: the run stops with no dump.
        dump_one = dump_records._dump_input

        def importing(job):
            importlib.import_module("unityrs_planted")
            return dump_one(job)

        with (
            mock.patch.object(dump_records, "_dump_input", importing),
            mock.patch.object(sys, "path", [str(planted), *sys.path]),
            contextlib.redirect_stderr(io.StringIO()) as err,
        ):
            self.assertEqual(unitypy_dump.main(["-q", str(tiny), "-o", str(out)]), 2)
        self.assertIn("refusing to dump: this is not the pinned oracle (while dumping 'tiny': module", err.getvalue())
        self.assertIn(f"unityrs_planted is loaded from {(planted / 'unityrs_planted.py').resolve()}", err.getvalue())
        self.assertNotIn("unityrs_planted", sys.modules)
        self.assertEqual(sorted(p.name for p in self.tmp.iterdir()), ["csv_copy", "json_copy", "planted", "tiny"])

    def test_refuses_to_run_with_pythonpath_or_a_user_site(self):
        """PYTHONPATH and a user site-packages folder put folders before the checks; with either the dumper refuses.

        PYTHONPATH comes before the standard library on sys.path, so a copy of
        json there would write every line, and a sitecustomize there would run
        at start-up. Under -E or -I the interpreter ignores it, and the dump
        is the one made without it. A user site (here through PYTHONUSERBASE)
        runs its .pth files at start-up; -s turns it off, and so does a venv.
        """
        tiny, out, clean = self.tmp / "tiny", self.tmp / "x.jsonl", self.tmp / "clean.jsonl"
        tiny.write_bytes(b"tiny")
        shadow = self.tmp / "shadow"
        shutil.copytree(
            Path(json.__file__).resolve().parent, shadow / "json", ignore=shutil.ignore_patterns("__pycache__")
        )
        self.assertEqual(dump_process(tiny, "-o", clean).returncode, 0)
        with_path = {"PYTHONPATH": str(shadow)}
        proc = dump_process(tiny, "-o", out, env=with_path)
        self.assertEqual(proc.returncode, 2, proc.stderr.decode(errors="replace"))
        said = b"not the pinned oracle (PYTHONPATH is set; unset it, or run python with -E or -I)"
        self.assertIn(said, proc.stderr)
        self.assertFalse(out.exists())
        for flag in ("-E", "-I"):
            with self.subTest(flag):
                proc = dump_process(tiny, "-o", out, env=with_path, flags=[flag])
                self.assertEqual(proc.returncode, 0, proc.stderr.decode(errors="replace"))
                self.assertEqual(out.read_bytes(), clean.read_bytes())
                out.unlink()
        base = self.tmp / "userbase"
        folder = base / "lib" / f"python{sys.version_info[0]}.{sys.version_info[1]}" / "site-packages"
        folder.mkdir(parents=True)
        with_user = {"PYTHONUSERBASE": str(base)}
        probe = [sys.executable, "-c", "import site; print(site.ENABLE_USER_SITE)"]
        # The probe's answer, never its failure (review round 9: a probe killed from outside read as "disabled").
        enabled = child.run(probe, env={**os.environ, **with_user}, capture_output=True, text=True, check=True).stdout
        self.assertIn(enabled, ("True\n", "False\n", "None\n"))
        proc = dump_process(tiny, "-o", out, env=with_user)
        if enabled.strip() == "True":
            self.assertEqual(proc.returncode, 2, proc.stderr.decode(errors="replace"))
            said = f"the user site-packages folder {folder} is on sys.path; run python with -s or -I"
            self.assertIn(said.encode(), proc.stderr)
            self.assertFalse(out.exists())
        else:  # a venv
            self.assertEqual(proc.returncode, 0, proc.stderr.decode(errors="replace"))
            out.unlink()
        proc = dump_process(tiny, "-o", out, env=with_user, flags=["-s"])
        self.assertEqual(proc.returncode, 0, proc.stderr.decode(errors="replace"))
        self.assertEqual(out.read_bytes(), clean.read_bytes())
        # The same input without the import is dumped.
        self.assertEqual(dump(tiny, "-o", out), 0)


if __name__ == "__main__":
    unittest.main()
