"""The oracle's pins (requirements-oracle.txt) and oracle_env.py's checks, on synthetic installs.

Standard library only.
"""

import base64
import contextlib
import hashlib
import importlib
import importlib.machinery
import io
import json
import os
import shutil
import subprocess
import sys
import sysconfig
import tempfile
import types
import unittest
import zipfile
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


def record_hash(data: bytes) -> str:
    return "sha256=" + base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()


# A synthetic wheel's files: a package, a top-level module, and its dist-info's METADATA.
PKG = "unityrs_fake_pkg"
FILES = {
    f"{PKG}/__init__.py": b"VALUE = 1\n",
    f"{PKG}/sub.py": b"def f():\n    return 2\n",
    "unityrs_fake_mod.py": b"X = 3\n",
}


def install(site: Path, files=FILES, extra_record=None, write=None):
    """`files` installed into `site` as an installer leaves them: on disk, in RECORD, plus what it writes itself.

    Returns the wheel's entries ({path: hash}), which the RECORD holds besides INSTALLER, REQUESTED and a launcher.
    """
    info = "fake_pkg-1.0.dist-info"
    metadata = b"Metadata-Version: 2.1\nName: fake-pkg\nVersion: 1.0\n"
    wheel_files = {**files, f"{info}/METADATA": metadata}
    for path, data in {**wheel_files, **(write or {})}.items():
        (site / path).parent.mkdir(parents=True, exist_ok=True)
        (site / path).write_bytes(data)
    entries = {path: record_hash(data) for path, data in wheel_files.items()}
    record = {
        **entries,
        f"{info}/INSTALLER": record_hash(b"pip\n"),
        f"{info}/REQUESTED": record_hash(b""),
        "../../../bin/fake": "sha256=launcher",
        f"{PKG}/__pycache__/__init__.cpython-311.pyc": "",
        **(extra_record or {}),
    }
    rows = [f"{path},{digest},1" for path, digest in record.items()] + [f"{info}/RECORD,,"]
    (site / info / "RECORD").write_text("\n".join(rows) + "\n")
    (site / info / "INSTALLER").write_text("pip\n")
    return entries


class PinTests(unittest.TestCase):
    def test_the_pins(self):
        pins = C.oracle_pins()
        text = C.ORACLE_REQUIREMENTS.read_text()
        self.assertEqual(len(pins), text.count("--hash=sha256:"), "one pin per hashed wheel")
        self.assertEqual(pins["unitypy"], C.ORACLE_UNITYPY)
        self.assertEqual(C.ORACLE_UNITYPY, "1.25.2")
        # The image holds numpy and UnityPy uses it when present, so the oracle pins it; and likewise
        # cffi (with its dependency pycparser), which Pillow imports, and zstandard, which fsspec imports.
        self.assertEqual(pins["numpy"], "2.4.6")
        self.assertEqual((pins["cffi"], pins["pycparser"], pins["zstandard"]), ("2.1.0", "2.21", "0.25.0"))
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

    def check(self, site, records, path=None):
        """wheel_problems on `site`, with sys.path holding `path` (default: just `site`) for the import check."""
        with mock.patch.object(sys, "path", [str(p) for p in (path or [site])]):
            importlib.invalidate_caches()
            try:
                return oracle_env.wheel_problems(records, [str(site)])
            finally:
                importlib.invalidate_caches()

    def test_record_digest(self):
        entries = {"b.py": "sha256=B", "a,b.py": "sha256=A"}
        record = '"a,b.py",sha256=A,3\nb.py,sha256=B,1\nx-1.dist-info/RECORD,,\n'
        self.assertEqual(oracle_env.parse_record(record), entries)
        # Sorted by path, so the RECORD's row order does not matter; any change to a path or hash does.
        self.assertEqual(oracle_env.record_digest(entries), oracle_env.record_digest(dict(reversed(entries.items()))))
        other = {**entries, "b.py": "sha256=C"}
        self.assertNotEqual(oracle_env.record_digest(entries), oracle_env.record_digest(other))
        self.assertRegex(oracle_env.record_digest(entries), r"^[0-9a-f]{64}$")

    def test_the_pinned_wheels_files_pass(self):
        site = self.tmp / "site"
        entries = install(site)
        # What the installer wrote (INSTALLER, REQUESTED, the launcher, hashless .pyc rows) is no part of the wheel.
        digest = oracle_env.record_digest(entries)
        self.assertEqual(self.check(site, {"fake-pkg": digest}), ([], {"fake-pkg": digest}))
        # __pycache__ holds what Python compiled; it is not checked.
        (site / PKG / "__pycache__").mkdir()
        (site / PKG / "__pycache__" / "sub.cpython-311.pyc").write_bytes(b"compiled")
        self.assertEqual(self.check(site, {"fake-pkg": digest})[0], [])

    def test_another_build_or_an_edited_file_is_refused(self):
        site = self.tmp / "site"
        digest = oracle_env.record_digest(install(site))
        records = {"fake-pkg": digest}
        edited = b"def f():\n    return 7\n"
        cases = {
            # One line of a module edited after installation; its RECORD row still holds the wheel's hash.
            "an edited file": (
                lambda: (site / PKG / "sub.py").write_bytes(edited),
                f"fake-pkg: {PKG}/sub.py hashes as {record_hash(edited)}, not as its RECORD's",
            ),
            # Unreadable, and so no module of that name to import either.
            "a missing file": (lambda: (site / "unityrs_fake_mod.py").unlink(), "unityrs_fake_mod.py cannot be read"),
            # An extension module beside a .py of the same name is what Python imports in its place.
            "a planted module": (
                lambda: (site / PKG / "sub.cpython-311-x86_64-linux-gnu.so").write_bytes(b"\x7fELF"),
                f"fake-pkg: {PKG}/sub.cpython-311-x86_64-linux-gnu.so is not one of its wheel's files",
            ),
        }
        for label, (change, said) in cases.items():
            with self.subTest(label):
                shutil.rmtree(site)
                install(site)
                change()
                problems, _ = self.check(site, records)
                self.assertEqual(len(problems), 2 if label == "a missing file" else 1, problems)
                self.assertIn(said, problems[0])
        # Another build of the same version: its RECORD, files and all, is consistent, but not the pinned one.
        shutil.rmtree(site)
        install(site, files={**FILES, f"{PKG}/sub.py": b"def f():\n    return 2  # another build\n"})
        problems, digests = self.check(site, records)
        self.assertEqual(len(problems), 1)
        self.assertIn("fake-pkg 1.0: its RECORD is not the pinned wheel's", problems[0])
        self.assertNotEqual(digests["fake-pkg"], digest)
        # A hash other than sha256 is not taken on trust.
        shutil.rmtree(site)
        entries = install(site, extra_record={f"{PKG}/sub.py": "md5=xyz"})
        problems, _ = self.check(site, {"fake-pkg": oracle_env.record_digest({**entries, f"{PKG}/sub.py": "md5=xyz"})})
        self.assertEqual(problems, [f"fake-pkg: {PKG}/sub.py has a md5 hash in RECORD; only sha256 is checked"])
        self.assertEqual(self.check(site, {"numpy": digest})[0], ["numpy is not installed"])

    def test_a_copy_earlier_on_the_path_is_refused(self):
        site, shadow = self.tmp / "site", self.tmp / "shadow"
        digest = oracle_env.record_digest(install(site))
        # A patched copy of the package and of the module, without a dist-info, where the import system looks first.
        (shadow / PKG).mkdir(parents=True)
        (shadow / PKG / "__init__.py").write_bytes(b"VALUE = 7\n")
        (shadow / "unityrs_fake_mod.py").write_bytes(b"X = 7\n")
        problems, _ = self.check(site, {"fake-pkg": digest}, path=[shadow, site])
        here = site.resolve()
        self.assertEqual(
            problems,
            [
                f"fake-pkg: module unityrs_fake_mod imports from {(shadow / 'unityrs_fake_mod.py').resolve()}, "
                f"not from {here}",
                f"fake-pkg: module {PKG} imports from {(shadow / PKG / '__init__.py').resolve()}, not from {here}",
            ],
        )
        self.assertEqual(self.check(site, {"fake-pkg": digest}, path=[site, shadow])[0], [])

    def test_a_symlink_is_refused(self):
        """No wheel installs a symlink, and the walk for stray files follows none, so each one is refused."""
        site, elsewhere = self.tmp / "site", self.tmp / "elsewhere"
        records = {"fake-pkg": oracle_env.record_digest(install(site))}

        def a_package_beside_a_module():
            # A folder `sub` linked in beside sub.py: Python imports the package in the module's place.
            (elsewhere / "sub").mkdir(parents=True)
            (elsewhere / "sub" / "__init__.py").write_bytes(b"def f():\n    return 7\n")
            (site / PKG / "sub").symlink_to(elsewhere / "sub", target_is_directory=True)
            loaders = (importlib.machinery.SourceFileLoader, importlib.machinery.SOURCE_SUFFIXES)
            spec = importlib.machinery.FileFinder(str(site / PKG), loaders).find_spec(f"{PKG}.sub")
            self.assertEqual(Path(spec.origin), site / PKG / "sub" / "__init__.py")

        def a_listed_file_as_a_link_to_its_bytes():
            elsewhere.mkdir()
            (site / PKG / "sub.py").rename(elsewhere / "sub.py")
            (site / PKG / "sub.py").symlink_to(elsewhere / "sub.py")

        def the_package_folder():
            elsewhere.mkdir()
            (site / PKG).rename(elsewhere / PKG)
            (site / PKG).symlink_to(elsewhere / PKG, target_is_directory=True)

        def one_in_pycache():
            (site / PKG / "__pycache__").mkdir()
            (site / PKG / "__pycache__" / "sub.cpython-311.pyc").symlink_to(self.tmp / "compiled.pyc")

        cases = {
            "a folder linked in beside a module": (a_package_beside_a_module, f"{PKG}/sub", 1),
            "a listed file linked to the same bytes": (a_listed_file_as_a_link_to_its_bytes, f"{PKG}/sub.py", 1),
            # Its modules then import from elsewhere too.
            "the package folder": (the_package_folder, PKG, 2),
            "a link in __pycache__": (one_in_pycache, f"{PKG}/__pycache__/sub.cpython-311.pyc", 1),
            "a dangling link": (lambda: (site / PKG / "x.py").symlink_to(self.tmp / "none.py"), f"{PKG}/x.py", 1),
        }
        for label, (plant, link, count) in cases.items():
            with self.subTest(label):
                shutil.rmtree(site)
                shutil.rmtree(elsewhere, ignore_errors=True)
                install(site)
                plant()
                problems, _ = self.check(site, records)
                self.assertIn(f"fake-pkg: {link} is a symlink, which no pinned wheel installs", problems)
                self.assertEqual(len(problems), count, problems)

    def test_installed_entries_leave_out_what_the_installer_writes(self):
        site = self.tmp / "site"
        entries = install(site)
        found = oracle_env._distribution("fake-pkg", [str(site)])
        self.assertEqual(oracle_env.installed_entries(found), entries)

    def test_wheel_entries_and_pin_records(self):
        wheels = self.tmp / "wheels"
        wheels.mkdir()
        entries = {"fake_pkg/__init__.py": record_hash(b"x"), "fake_pkg-1.0.dist-info/METADATA": "sha256=M"}

        def wheel(name, rows, member="fake_pkg-1.0.dist-info"):
            path = wheels / name
            with zipfile.ZipFile(path, "w") as zf:
                zf.writestr(f"{member}/METADATA", "Metadata-Version: 2.1\nName: Fake_Pkg\nVersion: 1.0\n\nbody: x\n")
                record = "".join(f"{p},{h},1\n" for p, h in rows.items()) + f"{member}/RECORD,,\n"
                zf.writestr(f"{member}/RECORD", record)
            return path, hashlib.sha256(path.read_bytes()).hexdigest()

        path, sha = wheel("fake_pkg-1.0-py3-none-any.whl", entries)
        self.assertEqual(oracle_env.wheel_entries(path), ("fake-pkg", "1.0", entries))
        pins = {"fake-pkg": ("1.0", sha)}
        with (
            mock.patch.object(oracle_env, "pinned_wheels", return_value=pins),
            mock.patch.object(C, "oracle_pins", return_value={"fake-pkg": "1.0"}),
        ):
            content = oracle_env.pin_records(wheels)
            self.assertEqual(
                content["records"],
                {
                    "fake-pkg": {
                        "version": "1.0",
                        "wheel_sha256": sha,
                        "files": 2,
                        "record_sha256": oracle_env.record_digest(entries),
                    }
                },
            )
            # Anything in the folder but the pinned wheels, or a pin without its wheel, is refused.
            (wheels / "stray.whl").write_bytes(b"not pinned")
            with self.assertRaisesRegex(ValueError, "stray.whl is not a pinned wheel"):
                oracle_env.pin_records(wheels)
            (wheels / "stray.whl").unlink()
            path.unlink()
            with self.assertRaisesRegex(ValueError, "no wheel for fake-pkg"):
                oracle_env.pin_records(wheels)
        # A wheel that installs files elsewhere (.data) is refused.
        path, _ = wheel("data.whl", {**entries, "fake_pkg-1.0.data/scripts/x": "sha256=S"})
        with self.assertRaisesRegex(ValueError, "a .data entry"):
            oracle_env.wheel_entries(path)

    def test_the_command_line(self):
        pins = C.oracle_pins()
        edited = ["unitypy: UnityPy/files/ObjectReader.py hashes as sha256=x, not as its RECORD's sha256=y"]
        cases = [
            (pins, [], [], 0, "is installed from its pinned wheel"),
            ({**pins, "numpy": "2.4.5"}, [], [], 1, "numpy is 2.4.5, not the pinned 2.4.6"),
            ({**pins, "scipy": "1.17.1"}, [], [], 0, "is installed from its pinned wheel"),
            ({**pins, "scipy": "1.17.1"}, ["--exact"], [], 1, "scipy 1.17.1 is installed but not pinned"),
            ({**pins, "pip": "24.0"}, ["--exact"], [], 0, "and nothing else is installed"),
            # Every version right, one file not the wheel's: refused, with or without --exact.
            (pins, [], edited, 1, edited[0]),
            (pins, ["--exact"], edited, 1, edited[0]),
        ]
        for installed, args, problems, code, said in cases:
            with self.subTest(args=args, said=said):
                out, err = io.StringIO(), io.StringIO()
                with (
                    mock.patch.object(C, "installed_packages", return_value=installed),
                    mock.patch.object(oracle_env, "wheel_problems", return_value=(problems, {})) as checked,
                    contextlib.redirect_stdout(out),
                    contextlib.redirect_stderr(err),
                ):
                    self.assertEqual(oracle_env.main(args), code)
                self.assertIn(said, out.getvalue() + err.getvalue())
                # The files are checked against oracle_records.json once the versions are right.
                if installed == pins:
                    checked.assert_called_once_with(C.oracle_records())
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            oracle_env.main(["--pin-records", str(self.tmp / "missing")])


class LoadedModulesTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-oracle-env-"))
        self.addCleanup(shutil.rmtree, self.tmp)

    def test_pinned_files_are_the_records_files(self):
        site = self.tmp / "site"
        entries = install(site)
        files = oracle_env.pinned_files({"fake-pkg": "unused"}, [str(site)])
        self.assertEqual(files, {(site / path).resolve() for path in entries})
        self.assertEqual(oracle_env.pinned_files({"numpy": "unused"}, [str(site)]), set())

    def test_only_the_standard_library_the_tools_and_the_pins_may_load(self):
        stdlib = Path(sysconfig.get_paths()["stdlib"])
        site, own = self.tmp / "site", self.tmp / "own"

        def module(name, file=None):
            m = types.ModuleType(name)
            if file is not None:
                m.__file__ = str(file)
            return m

        early = module("early", site / "early.py")
        # Copies of standard-library modules: one loaded at start-up, one in the tools' folder, one in a pin.
        early_csv = module("csv", site / "csv.py")
        modules = {
            "json": sys.modules["json"],
            "pinned": module("pinned", site / "pinned" / "__init__.py"),
            "own_tool": module("own_tool", own / "tool.py"),
            "early": early,
            "early_alias": early,  # one module under two names, as __mp_main__ is __main__
            "builtin": module("builtin"),
            "foreign": module("foreign", site / "foreign" / "__init__.py"),
            "site_in_stdlib": module("site_in_stdlib", stdlib / "site-packages" / "thing.py"),
            "csv": early_csv,
            "hashlib": module("hashlib", own / "hashlib.py"),
            "zlib": module("zlib", site / "pinned" / "zlib.py"),
            "json.encoder": module("json.encoder", site / "json" / "encoder.py"),
        }
        pinned = {(site / "pinned" / "__init__.py").resolve(), (site / "pinned" / "zlib.py").resolve()}
        check = oracle_env.LoadedModules(pinned, [own], [early, early_csv])
        why, shadows = (
            "which is neither the standard library nor a pinned wheel's file",
            "which shadows the standard library's",
        )
        expected = [
            f"module csv is loaded from {(site / 'csv.py').resolve()}, {shadows} csv",
            f"module foreign is loaded from {(site / 'foreign' / '__init__.py').resolve()}, {why}",
            f"module hashlib is loaded from {(own / 'hashlib.py').resolve()}, {shadows} hashlib",
            f"module json.encoder is loaded from {(site / 'json' / 'encoder.py').resolve()}, {shadows} json",
            f"module site_in_stdlib is loaded from {(stdlib / 'site-packages' / 'thing.py').resolve()}, {why}",
            f"module zlib is loaded from {(site / 'pinned' / 'zlib.py').resolve()}, {shadows} zlib",
        ]
        self.assertEqual(check.problems(modules), expected)
        # Each verdict is kept: a second check looks only at what is new.
        with mock.patch.object(check, "verdict", wraps=check.verdict) as verdict:
            more = {**modules, "later": module("later", site / "later.py")}
            self.assertEqual(len(check.problems(more)), 7)
        verdict.assert_called_once_with("later", (site / "later.py").resolve())
        # Real modules: frozen and pure-Python standard modules, extension modules (lib-dynload), and the tools'.
        here = oracle_env.LoadedModules(set(), [HERE], [])
        names = ("os", "json", "unittest", "zlib", "_csv", "_json", "oracle_env", __name__)
        self.assertEqual(here.problems({name: importlib.import_module(name) for name in names}), [])


class StartUpTests(unittest.TestCase):
    def test_what_puts_other_folders_first_is_refused(self):
        """PYTHONPATH, PYTHONHOME and PYTHONPLATLIBDIR, unless the interpreter ignores them, and a user site."""
        how = "unset it, or run python with -E or -I"
        self.assertEqual(oracle_env.start_up_problems({}, False, ""), [])
        self.assertEqual(oracle_env.start_up_problems({"PYTHONPATH": "", "PYTHONUTF8": "1"}, False, ""), [])
        every = {"PYTHONPATH": "/x", "PYTHONHOME": "/y", "PYTHONPLATLIBDIR": "lib64"}
        self.assertEqual(
            oracle_env.start_up_problems(every, False, ""),
            [f"PYTHONPATH is set; {how}", f"PYTHONHOME is set; {how}", f"PYTHONPLATLIBDIR is set; {how}"],
        )
        self.assertEqual(oracle_env.start_up_problems(every, True, ""), [])
        self.assertEqual(
            oracle_env.start_up_problems({}, True, "/u/site-packages"),
            ["the user site-packages folder /u/site-packages is on sys.path; run python with -s or -I"],
        )

    def test_the_user_site_is_the_one_site_added(self):
        """user_site() names the folder when site put it on sys.path at start-up, and None under -s."""
        with tempfile.TemporaryDirectory(prefix="unityrs-oracle-env-") as tmp:
            base = Path(tmp)
            folder = base / "lib" / f"python{sys.version_info[0]}.{sys.version_info[1]}" / "site-packages"
            folder.mkdir(parents=True)
            code = (
                f"import site, sys; sys.path.insert(0, {str(HERE)!r}); import oracle_env; "
                "print(site.ENABLE_USER_SITE, oracle_env.user_site(), site.getusersitepackages() in sys.path)"
            )
            env = {**os.environ, "PYTHONUSERBASE": str(base)}
            for flags in ([], ["-s"]):
                with self.subTest(flags=flags):
                    out = subprocess.run([sys.executable, *flags, "-c", code], env=env, capture_output=True, text=True)
                    self.assertEqual(out.returncode, 0, out.stderr)
                    enabled, found, on_path = out.stdout.split()
                    # A venv turns the user site off by itself, as -s does.
                    if enabled == "True":
                        self.assertEqual((found, on_path), (str(folder), "True"))
                    else:
                        self.assertEqual((found, on_path), ("None", "False"))
                    if flags:
                        self.assertEqual(enabled, "False")


class RecordsFileTests(unittest.TestCase):
    def test_the_records_pin_the_requirements_wheels(self):
        """oracle_records.json holds one record per pinned wheel: the same name, version and wheel sha256."""
        content = json.loads(C.ORACLE_RECORDS.read_text(encoding="utf-8"))
        wheels = oracle_env.pinned_wheels()
        self.assertEqual(set(wheels), set(C.oracle_pins()))
        self.assertEqual(
            {name: (r["version"], r["wheel_sha256"]) for name, r in content["records"].items()},
            wheels,
        )
        self.assertEqual(list(content["records"]), sorted(content["records"]))
        for name, digest in C.oracle_records().items():
            self.assertRegex(digest, r"^[0-9a-f]{64}$", name)
        # UnityPy's record includes UnityPyBoost, whose hash canonical.py also pins.
        self.assertEqual(content["records"]["unitypy"]["files"], 95)


if __name__ == "__main__":
    unittest.main()
