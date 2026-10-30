"""PARITY.md rows on paths, run against the oracle: the reader a path gets, the case-insensitive walk, a filesystem
other than the local one, a ZIP found by its content, and the folder walk's paths, links and folders it cannot list.

Sections 3, 4, 13 and 19.2. Every input is built here (`synth.py`), so nothing needs the corpus, and each test names
the rows it pins; a row's test column names the test back. Its class is `Rows`, as in `test_parity_rows.py`, the
module these tests were split from. Needs UnityPy 1.25.2 (the oracle image); skipped elsewhere.
"""

import io
import os
import re
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from rows_support import ORACLE, load, quietly, text_asset_file, zip_with_a_directory  # noqa: E402

if ORACLE:
    import UnityPy
    from UnityPy.classes import PPtr
    from UnityPy.enums import FileType
    from UnityPy.helpers import ImportHelper
    from UnityPy.streams import EndianBinaryReader


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class Rows(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-rows-"))
        self.addCleanup(lambda: __import__("shutil").rmtree(self.tmp))

    def chdir(self, path):
        cwd = os.getcwd()
        os.chdir(path)
        self.addCleanup(os.chdir, cwd)

    # Sections 3, 4, 13: what reader a path gets.
    def test_a_path_reads_strings_with_the_stream_rule(self):
        path = self.tmp / "twenty"
        path.write_bytes(b"A" * 20)
        kind, reader = ImportHelper.check_file_type(str(path))  # opens the file and leaves it open
        reader.dispose()
        self.assertEqual(kind, FileType.ResourceFile)
        with open(path, "rb") as stream:
            self.assertEqual(ImportHelper.check_file_type(stream)[0], FileType.ResourceFile)
        with self.assertRaisesRegex(Exception, "^String not terminated$"):
            ImportHelper.check_file_type(b"A" * 20)
        # Only an input of exactly 20 bytes reaches the memory reader's end: a longer one takes its first 20
        # bytes as the signature, and detection goes on.
        for size in (21, 127, 200):
            with self.subTest(size=size):
                self.assertEqual(ImportHelper.check_file_type(b"A" * size)[0], FileType.ResourceFile)
        env = quietly(UnityPy.load, str(path))
        reader = env.files[str(path)]
        self.addCleanup(reader.dispose)
        self.assertEqual(type(reader).__name__, "EndianBinaryReader_Streamable")
        # With no NUL in reach the stream reader drops the last byte it read. It raises only from a Position past
        # the end, where its loop, which stops at Position == Length alone, reads nothing; at the end it returns "".
        stream = EndianBinaryReader(io.BytesIO(b"C" * 10))
        self.assertEqual((stream.read_string_to_null(), stream.Position), ("C" * 9, 10))
        self.assertEqual((stream.read_string_to_null(), stream.Position), ("", 10))
        stream.Position = 9
        stream.align_stream()
        self.assertEqual(stream.Position, 12)
        past = r"^Unterminated string: \[b''\]$"
        with self.assertRaisesRegex(ValueError, past):
            stream.read_string_to_null()
        reader.Position = 21  # the 20-byte file opened by path
        with self.assertRaisesRegex(ValueError, past):
            reader.read_string_to_null()
        reader.Position = 20
        self.assertEqual(reader.read_string_to_null(), "")
        stream = EndianBinaryReader(io.BytesIO(b"B" * 40000))
        self.assertEqual((len(stream.read_string_to_null()), stream.Position), (32766, 32767))
        memory = EndianBinaryReader(b"B" * 40000)
        self.assertEqual((len(memory.read_string_to_null()), memory.Position), (32767, 32767))
        # A string with its NUL in reach reads the same from both.
        for reader in (EndianBinaryReader(io.BytesIO(b"2021.3.2f1\0rest")), EndianBinaryReader(b"2021.3.2f1\0rest")):
            self.assertEqual((reader.read_string_to_null(), reader.Position), ("2021.3.2f1", 11))

    # Sections 3, 4, 19.2: the case-insensitive walk takes `os.path.split`'s head and tail.
    def test_the_case_insensitive_walk_takes_head_and_tail(self):
        d = self.tmp / "d"
        (d / "Resources" / "Sub").mkdir(parents=True)
        for name in ("Resources/Foo.asset", "Resources/Sub/Bar.asset", "Top.asset"):
            (d / name).write_bytes(text_asset_file(name))
        walk = ImportHelper.find_sensitive_path
        found = str(d / "Resources" / "Foo.asset")
        # Exactly two components resolve; leading and trailing separators are stripped first.
        for path in ("resources/foo.asset", "RESOURCES/FOO.ASSET", "/resources/foo.asset", "resources/foo.asset/"):
            self.assertEqual(walk(str(d), path), found, path)
        # A single name (an empty head) or a deeper path does not, even in exact case; nor a full path.
        for path in ("Top.asset", "top.asset", "Resources/Sub/Bar.asset", f"{d}/resources/foo.asset"):
            self.assertIsNone(walk(str(d), path), path)
        # A directory the local disk does not have raises, `''` included.
        for missing in ("", str(d / "nowhere")):
            with self.assertRaises(FileNotFoundError):
                walk(missing, "resources/foo.asset")

        def names(path, name):
            env = UnityPy.Environment(path=path)
            loaded = quietly(env.load_file, name)
            return None if loaded is None else [o.parse_as_dict()["m_Name"] for o in loaded.objects.values()]

        self.chdir(self.tmp)
        # load_file hands the walk the name joined to `path`, so a record in another case is not found.
        self.assertIsNone(names(str(d), "resources/foo.asset"))
        self.assertIsNone(names(str(d), "top.asset"))
        self.assertEqual(names(str(d), "Resources/Foo.asset"), ["Resources/Foo.asset"])
        # An absolute name is handed over as given: `/a/b` is looked up as `path/A/B`.
        self.assertEqual(names(str(d), "/resources/foo.asset"), ["Resources/Foo.asset"])
        # A relative one-component `path` P and a bare name n are looked up as P/P/n.
        self.chdir(d)
        self.assertIsNone(names("Resources", "foo.asset"))
        (d / "Resources" / "resources").mkdir()
        (d / "Resources" / "resources" / "FOO.asset").write_bytes(text_asset_file("doubled"))
        self.assertEqual(names("Resources", "foo.Asset"), ["doubled"])
        # A `path` the walk cannot list raises instead of returning None.
        for path in ("", "nowhere"):
            with self.subTest(path=path), self.assertRaises(FileNotFoundError):
                names(path, "x.asset")

    # Sections 3, 19.2: a filesystem other than the local one.
    def test_a_non_local_fs_loads_only_through_load_and_streams(self):
        from fsspec.implementations.memory import MemoryFileSystem

        fs = MemoryFileSystem()  # one store per process: a folder of this test's own, removed after
        root = f"/{self.tmp.name}"
        fs.pipe(f"{root}/x.assets", text_asset_file("in memory"))
        self.addCleanup(fs.rm, root, recursive=True)
        # Section 3 states only the cases run here: each argument under a top-level folder the local disk lacks,
        # unless said otherwise. Every other case through a non-local fs is not characterised.
        for folder in (root, f"{root}-split", f"{root}-zip"):
            self.assertEqual(os.path.dirname(folder), "/")
            self.assertFalse(os.path.exists(folder), "nothing of that name on the local disk")
        self.assertEqual(UnityPy.Environment(fs=fs).path, "")
        # load_file tests a name with os.path and walks `path` with os.listdir, both on the local disk.
        for name in (f"{root}/x.assets", "x.assets", "missing"):
            with self.subTest(name), self.assertRaises(FileNotFoundError):
                quietly(UnityPy.Environment(fs=fs).load_file, name)
        with self.assertRaises(FileNotFoundError):
            quietly(UnityPy.Environment(fs=fs, path=root).load_file, "x.assets")
        # Where the local disk can list `path`, the walk finds nothing there and the name is None, with no error.
        self.assertIsNone(quietly(UnityPy.Environment(fs=fs, path=str(self.tmp)).load_file, "x.assets"))
        self.assertIsNone(quietly(UnityPy.Environment(fs=fs, path="/").load_file, f"{root[1:]}/x.assets"))
        # The constructor takes a file through load_file, a folder through load_files' builtin open: a file raises
        # naming its folder, which the local disk cannot list, and a folder naming its file.
        for arg, named in ((f"{root}/x.assets", root), (root, f"{root}/x.assets")):
            with self.subTest(arg), self.assertRaisesRegex(FileNotFoundError, f"No such file or directory: '{named}'$"):
                quietly(UnityPy.Environment, arg, fs=fs)
        # A file whose folder is an empty one the local disk can list loads nothing, with no error.
        fs.pipe(f"{self.tmp}/x.assets", text_asset_file("y"))
        self.addCleanup(fs.rm, f"{self.tmp}/x.assets")
        self.assertEqual(os.listdir(self.tmp), [])
        env = quietly(UnityPy.Environment, f"{self.tmp}/x.assets", fs=fs)
        self.assertEqual((env.path, env.files, env.cabs), (str(self.tmp), {}, {}))
        with self.assertRaisesRegex(FileNotFoundError, re.escape(f"No such file or directory: '{root}/x.assets'")):
            quietly(UnityPy.Environment(fs=fs).load_folder, root)
        # load (fs.exists, fs.open) and a stream do read through the filesystem.
        env = UnityPy.Environment(fs=fs)
        quietly(env.load, [f"{root}/x.assets"])
        self.assertEqual(list(env.files), [f"{root}/x.assets", "x.assets"])
        with fs.open(f"{root}/x.assets", "rb") as stream:
            loaded = quietly(UnityPy.Environment(fs=fs).load_file, stream, name="x.assets")
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in loaded.objects.values()], ["in memory"])
        # A .splitN name skips os.path and the builtin open: _load_split_file tests and reads its parts through fs,
        # so it loads, keyed by its base path, from load_file, the constructor, load_folder and find_file.
        split = f"{root}-split"
        whole = text_asset_file("split")
        fs.pipe({f"{split}/a.assets.split0": whole[:40], f"{split}/a.assets.split1": whole[40:]})
        self.addCleanup(fs.rm, split, recursive=True)

        def loaded(env):
            return env.path, list(env.files), list(env.cabs), [o.parse_as_dict()["m_Name"] for o in env.objects]

        for part in ("split0", "split1"):
            with self.subTest(part):
                env = quietly(UnityPy.Environment, f"{split}/a.assets.{part}", fs=fs)
                self.assertEqual(loaded(env), (split, [f"{split}/a.assets"], ["a.assets"], ["split"]))
                env = UnityPy.Environment(fs=fs)
                self.assertEqual(type(quietly(env.load_file, f"{split}/a.assets.{part}")).__name__, "SerializedFile")
                self.assertEqual(loaded(env), ("", [f"{split}/a.assets"], ["a.assets"], ["split"]))
                env = UnityPy.Environment(fs=fs, path=split)
                self.assertEqual(type(quietly(env.find_file, f"a.assets.{part}")).__name__, "SerializedFile")
                self.assertEqual(loaded(env), (split, [f"{split}/a.assets"], ["a.assets"], []))  # a dependency
        self.assertEqual(
            loaded(quietly(UnityPy.Environment, split, fs=fs))[1:], ([f"{split}/a.assets"], ["a.assets"], ["split"])
        )
        env = UnityPy.Environment(fs=fs)
        quietly(env.load_folder, split)
        self.assertEqual(loaded(env)[1:], ([f"{split}/a.assets"], ["a.assets"], ["split"]))
        # Any other file in the folder still meets the builtin open, from load_folder and from the constructor, a zip
        # too (which the constructor loads through fs when it is the argument itself).
        fs.pipe(f"{split}/sub/x.assets", text_asset_file("x"))
        for call in (UnityPy.Environment(fs=fs).load_folder, lambda folder: UnityPy.Environment(folder, fs=fs)):
            missing = re.escape(f"No such file or directory: '{split}/sub/x.assets'")
            with self.assertRaisesRegex(FileNotFoundError, missing):
                quietly(call, split)
        fs.pipe(f"{root}-zip/z.zip", zip_with_a_directory())
        self.addCleanup(fs.rm, f"{root}-zip", recursive=True)
        self.assertEqual(
            list(quietly(UnityPy.Environment, f"{root}-zip/z.zip", fs=fs).files), ["assets/", "assets/a.assets"]
        )
        with self.assertRaisesRegex(FileNotFoundError, re.escape(f"No such file or directory: '{root}-zip/z.zip'")):
            quietly(UnityPy.Environment, f"{root}-zip", fs=fs)

    # Sections 3, 4, 19.2: a ZIP is found by content, and only bytes and buffered streams load.
    def test_a_zip_loads_from_bytes_and_raises_from_a_path(self):
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr("inner.assets", text_asset_file())
        data = bytearray(buffer.getvalue())
        self.assertEqual(
            ImportHelper.check_file_type(bytes(data))[0], FileType.ResourceFile, "zipfile's fifth byte is 20"
        )
        data[4] = 0  # "version needed to extract" 0: the NUL ends the signature at four bytes
        data = bytes(data)
        self.assertEqual(ImportHelper.check_file_type(data)[0], FileType.ZIP)
        env = UnityPy.Environment()
        self.assertIsNone(quietly(env.load_file, data, name="z.bin"))
        self.assertEqual(
            {name: type(f).__name__ for name, f in env.files.items()},
            {"inner.assets": "SerializedFile", "z.bin": "NoneType"},
        )
        path = self.tmp / "z.bin"
        path.write_bytes(data)
        with self.assertRaisesRegex(TypeError, "^Unsupported type for loading zip file$"):
            quietly(UnityPy.Environment().load_file, str(path))
        with self.assertRaisesRegex(TypeError, "^Unsupported type for loading zip file$"):
            quietly(UnityPy.load, str(path))
        with self.assertRaisesRegex(TypeError, "^Unsupported type for loading zip file$"):
            quietly(UnityPy.Environment(path=str(self.tmp)).find_file, "z.bin")
        # The constructor (UnityPy.load is the constructor) hands a path to load_file unless its name ends in .zip or
        # .apk (case-sensitive): v.bin and Z.ZIP raise. A .zip or .apk path goes to load_zip_file, which opens a path
        # string itself, and its members load, as load_zip_file of any path does. Environment.load hands load_file
        # the LocalFileOpener fs.open gives, so it raises for every name, .zip and .apk included.
        self.assertIs(UnityPy.load, UnityPy.Environment)
        for name, loads in (("v.bin", False), ("Z.ZIP", False), ("y.zip", True), ("w.apk", True)):
            (self.tmp / name).write_bytes(data)
            with self.subTest(name, call="constructor"):
                if loads:
                    self.assertEqual(list(quietly(UnityPy.Environment, str(self.tmp / name)).files), ["inner.assets"])
                else:
                    with self.assertRaisesRegex(TypeError, "^Unsupported type for loading zip file$"):
                        quietly(UnityPy.Environment, str(self.tmp / name))
            with (
                self.subTest(name, call="load"),
                self.assertRaisesRegex(TypeError, "^Unsupported type for loading zip file$"),
            ):
                quietly(UnityPy.Environment().load, [str(self.tmp / name)])
            (self.tmp / name).unlink()
        env = UnityPy.Environment()
        quietly(env.load_zip_file, str(path))
        self.assertEqual(list(env.files), ["inner.assets"])
        with open(path, "rb") as stream:
            env = quietly(UnityPy.load, stream)
        self.assertEqual(sorted(type(f).__name__ for f in env.files.values()), ["NoneType", "SerializedFile"])
        # Every name namelist() gives loads, directory entries included (zip -r and shutil.make_archive write them):
        # each is an empty raw reader keyed by its name and registered as CAB "", the basename of a name ending in
        # "/", so the last one registered holds it.
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr("assets/", b"")
            archive.writestr("assets/bin/", b"")
            archive.writestr("assets/bin/a.assets", text_asset_file("a"))
        data = bytearray(buffer.getvalue())
        data[4] = 0
        env = UnityPy.Environment()
        quietly(env.load_file, bytes(data), name="dirs.bin")
        self.assertEqual(
            zipfile.ZipFile(io.BytesIO(bytes(data))).namelist(), ["assets/", "assets/bin/", "assets/bin/a.assets"]
        )
        self.assertEqual(
            {name: type(f).__name__ for name, f in env.files.items()},
            {
                "assets/": "EndianBinaryReader_Streamable",
                "assets/bin/": "EndianBinaryReader_Streamable",
                "assets/bin/a.assets": "SerializedFile",
                "dirs.bin": "NoneType",
            },
        )
        self.assertEqual((len(env.files["assets/"].bytes), len(env.files["assets/bin/"].bytes)), (0, 0))
        self.assertEqual(list(env.cabs), ["", "a.assets"])
        self.assertIs(env.cabs[""], env.files["assets/bin/"])
        # load_zip_file takes bytes, a bytearray or a memoryview, read-only or writable (each in an io.BytesIO), an
        # io.BufferedIOBase (an io.BytesIO, the builtin open's io.BufferedReader) or a path string fs finds; anything
        # else raises TypeError: a path fs does not find, fsspec's LocalFileOpener, an int. Through load_file each of
        # the first five loads, with name= (a bytearray and a writable memoryview need it, the load_file row).
        zipped = zip_with_a_directory()
        (self.tmp / "dirs.zip").write_bytes(zipped)
        members = ["assets/", "assets/a.assets"]

        def given():
            yield from (zipped, bytearray(zipped), memoryview(zipped), memoryview(bytearray(zipped)))
            yield io.BytesIO(zipped)

        with open(self.tmp / "dirs.zip", "rb") as opened:
            for value in (*given(), opened, str(self.tmp / "dirs.zip")):
                with self.subTest(type(value).__name__):
                    env = UnityPy.Environment()
                    quietly(env.load_zip_file, value)
                    self.assertEqual(list(env.files), members)
        for value in given():
            with self.subTest(type(value).__name__, through="load_file"):
                env = UnityPy.Environment()
                self.assertIsNone(quietly(env.load_file, value, name="z.bin"))
                self.assertEqual(list(env.files), [*members, "z.bin"])
        with UnityPy.Environment().fs.open(str(self.tmp / "dirs.zip"), "rb") as local_opener:
            for value in (str(self.tmp / "missing.zip"), local_opener, 3):
                with self.subTest(type(value).__name__), self.assertRaisesRegex(TypeError, "^Unsupported type"):
                    UnityPy.Environment().load_zip_file(value)
        # A zip in a folder goes through detection from the stream the builtin open gives (load_files), which
        # load_zip_file accepts: one whose fifth byte is NUL loads its members, registers CAB "" and holds None under
        # its own path, where load_file of that path raises; Python's own zip (fifth byte 20) is a raw file.
        folder = self.tmp / "zips"
        folder.mkdir()
        (folder / "z.bin").write_bytes(zipped)
        (folder / "plain.zip").write_bytes(buffer.getvalue())
        self.assertEqual(buffer.getvalue()[4], 20)
        opened, real_open = [], open  # load_files never closes what it opens

        def tracked(path, mode="r", *args, **kw):
            stream = real_open(path, mode, *args, **kw)  # noqa: SIM115 - UnityPy reads it after the call
            opened.append(stream)
            return stream

        loaded = {
            "assets/": "EndianBinaryReader_Streamable",
            "assets/a.assets": "SerializedFile",
            str(folder / "z.bin"): "NoneType",
            str(folder / "plain.zip"): "EndianBinaryReader_Streamable",
        }
        for how in ("constructor", "load_folder", "load_files"):
            with self.subTest(how), mock.patch("builtins.open", tracked):
                if how == "constructor":
                    env = quietly(UnityPy.Environment, str(folder))
                else:
                    env = UnityPy.Environment()
                    if how == "load_folder":
                        quietly(env.load_folder, str(folder))
                    else:
                        quietly(env.load_files, [str(folder / "z.bin"), str(folder / "plain.zip")])
                self.assertEqual({name: type(f).__name__ for name, f in env.files.items()}, loaded)
                self.assertEqual(sorted(env.cabs), ["", "a.assets", "plain.zip"])
        for stream in opened:
            stream.close()
        with self.assertRaisesRegex(TypeError, "^Unsupported type for loading zip file$"):
            quietly(UnityPy.Environment().load_file, str(folder / "z.bin"))

    # Section 3, the walk: the paths it yields, links, and folders it cannot list.
    def test_the_walk_yields_absolute_paths_and_takes_links_for_files(self):
        opened, real_open = [], open  # load_files never closes what it opens

        def tracked(path, mode="r", *args, **kw):
            stream = real_open(path, mode, *args, **kw)  # noqa: SIM115 - UnityPy reads it after the call
            opened.append(stream)
            return stream

        self.addCleanup(lambda: [stream.close() for stream in opened])
        patched = mock.patch("builtins.open", tracked)
        patched.start()
        self.addCleanup(patched.stop)

        def keys(folder):
            env = UnityPy.Environment()
            quietly(env.load_folder, folder)
            return list(env.files)

        # Absolute posix paths, whatever form the folder takes: joined to the cwd, `./` and a trailing `/` dropped,
        # `~` expanded, `..` kept. `files` holds them, find_file's walk lists them, and `path` stays as given.
        work = self.tmp / "work"
        (work / "rel" / "sub").mkdir(parents=True)
        (work / "rel" / "a.bin").write_bytes(text_asset_file("a"))
        (work / "rel" / "sub" / "b.bin").write_bytes(text_asset_file("b"))
        self.chdir(work)
        cwd = os.getcwd()
        absolute = [f"{cwd}/rel/a.bin", f"{cwd}/rel/sub/b.bin"]
        for folder in ("rel", "./rel", "rel/", f"{cwd}/rel"):
            with self.subTest(folder):
                self.assertEqual(keys(folder), absolute)
        self.assertEqual(keys("rel/../rel"), [f"{cwd}/rel/../rel/a.bin", f"{cwd}/rel/../rel/sub/b.bin"])
        with mock.patch.dict(os.environ, {"HOME": cwd}):
            self.assertEqual(keys("~/rel"), absolute)
        env = quietly(UnityPy.Environment, "rel")
        self.assertEqual((env.path, list(env.files), sorted(env.cabs)), ("rel", absolute, ["a.bin", "b.bin"]))
        env = UnityPy.Environment(path="rel")
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in quietly(env.find_file, "b.bin").objects.values()], ["b"])
        self.assertEqual(env.local_files, absolute)

        def names(file):
            return [o.parse_as_dict()["m_Name"] for o in file.objects.values()]

        # A `~name` the home lookup cannot expand stays as it is, relative, and so do the keys under it.
        self.assertEqual(os.path.expanduser("~no-such-user-r12"), "~no-such-user-r12")
        (work / "~no-such-user-r12").mkdir()
        (work / "~no-such-user-r12" / "t.bin").write_bytes(text_asset_file("t"))
        self.assertEqual(keys("~no-such-user-r12"), ["~no-such-user-r12/t.bin"])
        # The folder given is followed: `info` stats a link's target, so a link given as the folder walks its target
        # under the link's own path (find_file's walk of such a `path` too), a broken one yields nothing, and a link
        # to a file walks as that file, so load_folder opens `link/` and raises. The constructor's isdir and isfile
        # follow a link the same way: a link to a file loads under its own path, a broken one is ignored.
        (work / "rellink").symlink_to(work / "rel")
        (work / "brokenlink").symlink_to(work / "nothing")
        (work / "filelink").symlink_to(work / "rel" / "a.bin")
        linked = [f"{cwd}/rellink/a.bin", f"{cwd}/rellink/sub/b.bin"]
        self.assertEqual(keys("rellink"), linked)
        env = quietly(UnityPy.Environment, "rellink")
        self.assertEqual((env.path, list(env.files)), ("rellink", linked))
        env = UnityPy.Environment(path="rellink")
        self.assertEqual((names(quietly(env.find_file, "b.bin")), env.local_files), (["b"], linked))
        self.assertEqual(keys("brokenlink"), [])
        env = quietly(UnityPy.Environment, "brokenlink")
        self.assertEqual((env.path, env.files), (cwd, {}))
        with self.assertRaisesRegex(NotADirectoryError, re.escape(f"'{cwd}/filelink/'")):
            keys("filelink")
        env = quietly(UnityPy.Environment, "filelink")
        self.assertEqual((env.path, list(env.files), list(env.cabs)), (".", ["filelink"], ["filelink"]))
        # URL-style prefixes: fs strips file://, file:, local:// and local:, and load_file's os.path tests do not. A
        # folder or a zip so named loads, and so does a .splitN name, whose parts _load_split_file reads through fs.
        # load_file misses any other prefixed name: None where os.listdir lists Environment.path, FileNotFoundError
        # where Environment.path has a prefix. So a file argument with no folder part loads nothing, one with a
        # folder part raises, wherever the file lies, and find_file from a prefixed `path` (deref through it) raises
        # where its path/name step finds the file, a .splitN name aside, while a file only its walk finds loads.
        zipped = io.BytesIO()
        with zipfile.ZipFile(zipped, "w") as archive:
            archive.writestr("in.assets", text_asset_file("in"))
        (work / "x.zip").write_bytes(zipped.getvalue())
        (work / "x.bin").write_bytes(text_asset_file("x"))

        def unlistable(listed):
            return f"^\\[Errno 2\\] No such file or directory: '{re.escape(listed)}'$"

        for prefix in ("file://", "file:", "local://", "local:"):
            with self.subTest(prefix=prefix):
                self.assertEqual(keys(prefix + "rel"), absolute)
                env = quietly(UnityPy.Environment, prefix + "rel")
                self.assertEqual((env.path, list(env.files)), (prefix + "rel", absolute))
                env = quietly(UnityPy.Environment, prefix + "x.zip")
                self.assertEqual((env.path, list(env.files)), (cwd, ["in.assets"]))
                env = UnityPy.Environment()
                quietly(env.load_zip_file, prefix + "x.zip")
                self.assertEqual(list(env.files), ["in.assets"])
                env = UnityPy.Environment()
                self.assertEqual((quietly(env.load_file, prefix + "x.bin"), env.files), (None, {}))
                env = UnityPy.Environment(path=prefix + "rel")
                self.assertEqual(names(quietly(env.find_file, "b.bin")), ["b"])  # the walk
                with self.assertRaisesRegex(FileNotFoundError, unlistable(prefix + "rel")):
                    quietly(env.find_file, "a.bin")  # path/name
                with self.assertRaisesRegex(FileNotFoundError, unlistable(prefix + "rel")):
                    quietly(UnityPy.Environment(path=prefix + "rel").load_file, "missing.bin")
                env = UnityPy.Environment(path=prefix + "rel")
                main = load(text_asset_file("main"), "main.assets", env)[0]
                main.externals = [SimpleNamespace(path="archive:/CAB-x/a.bin")]
                with self.assertRaisesRegex(FileNotFoundError, unlistable(prefix + "rel")):
                    PPtr(m_FileID=1, m_PathID=1, assetsfile=main).deref()
        for argument in ("file:x.bin", "local:x.bin"):
            env = quietly(UnityPy.Environment, argument)
            self.assertEqual((env.path, env.files), (".", {}))
        # A folder part decides, not where the file lies: x.bin is in the cwd, out.bin outside it.
        (self.tmp / "out.bin").write_bytes(text_asset_file("out"))
        for argument, listed in (
            ("file://x.bin", "file:"),
            ("local://x.bin", "local:"),
            ("file:./x.bin", "file:."),
            (f"file:{cwd}/x.bin", f"file:{cwd}"),
            (f"file:{self.tmp}/out.bin", f"file:{self.tmp}"),
        ):
            with self.subTest(argument), self.assertRaisesRegex(FileNotFoundError, unlistable(listed)):
                quietly(UnityPy.Environment, argument)
        # A .splitN name loads through the split branch, keyed by its prefixed base path and registered as a CAB
        # under that path's simplified name (ntpath.basename does not split at `file:`).
        (work / "b.assets.split0").write_bytes(text_asset_file("b"))
        (work / "d").mkdir()
        whole = text_asset_file("d")
        (work / "d" / "a.assets.split0").write_bytes(whole[:40])
        (work / "d" / "a.assets.split1").write_bytes(whole[40:])

        def loaded(env):
            return env.path, list(env.files), sorted(env.cabs), [o.parse_as_dict()["m_Name"] for o in env.objects]

        self.assertEqual(
            loaded(quietly(UnityPy.Environment, "file:b.assets.split0")),
            (".", ["file:b.assets"], ["file:b.assets"], ["b"]),
        )
        self.assertEqual(
            loaded(quietly(UnityPy.Environment, f"file:{cwd}/d/a.assets.split0")),
            (f"file:{cwd}/d", [f"file:{cwd}/d/a.assets"], ["a.assets"], ["d"]),
        )
        env = UnityPy.Environment()
        self.assertEqual(type(quietly(env.load_file, "local://d/a.assets.split1")).__name__, "SerializedFile")
        self.assertEqual(loaded(env)[1:], (["local://d/a.assets"], ["a.assets"], ["d"]))
        # So find_file's path/name step from a prefixed `path` loads a .splitN name, and deref through it, where any
        # other name raises.
        (work / "d" / "b.bin").write_bytes(text_asset_file("b"))
        for prefix in ("file://", "file:", "local://", "local:"):
            folder = f"{prefix}{cwd}/d"
            for part in ("a.assets.split0", "a.assets.split1"):
                with self.subTest(prefix=prefix, part=part):
                    env = UnityPy.Environment(path=folder)
                    self.assertEqual(names(quietly(env.find_file, part)), ["d"])
                    self.assertEqual((list(env.files), list(env.cabs)), ([f"{folder}/a.assets"], ["a.assets"]))
            env = UnityPy.Environment(path=folder)
            main = load(text_asset_file("main"), "main.assets", env)[0]
            main.externals = [SimpleNamespace(path="archive:/CAB-x/a.assets.split0")]
            pointed = quietly(PPtr(m_FileID=1, m_PathID=1, assetsfile=main).deref)
            self.assertEqual(
                (pointed.parse_as_dict()["m_Name"], list(env.files)), ("d", ["main.assets", f"{folder}/a.assets"])
            )
            with self.subTest(prefix=prefix), self.assertRaisesRegex(FileNotFoundError, unlistable(folder)):
                quietly(UnityPy.Environment(path=folder).find_file, "b.bin")
        # Links under the folder are files: a link to a folder is never entered and raises when opened, a broken one
        # raises, a link to a file loads under its own path and name. find_file reaches a file under a linked folder
        # only as path/name.
        real = self.tmp / "real"
        real.mkdir()
        (real / "r.assets").write_bytes(text_asset_file("r"))
        (self.tmp / "target.assets").write_bytes(text_asset_file("t"))
        linked = {}
        for case, target in (("folder", real), ("broken", self.tmp / "nothing"), ("file", self.tmp / "target.assets")):
            linked[case] = self.tmp / f"{case}-link"
            linked[case].mkdir()
            (linked[case] / "Link.assets").symlink_to(target)
            walk = list(UnityPy.Environment().fs.walk(str(linked[case])))
            self.assertEqual(walk, [(str(linked[case]), [], ["Link.assets"])], case)
        with self.assertRaisesRegex(IsADirectoryError, re.escape(str(linked["folder"] / "Link.assets"))):
            keys(str(linked["folder"]))
        with self.assertRaisesRegex(FileNotFoundError, re.escape(str(linked["broken"] / "Link.assets"))):
            keys(str(linked["broken"]))
        env = UnityPy.Environment()
        quietly(env.load_folder, str(linked["file"]))
        self.assertEqual((list(env.files), list(env.cabs)), ([str(linked["file"] / "Link.assets")], ["link.assets"]))
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in env.objects], ["t"])
        with self.assertRaisesRegex(FileNotFoundError, "^File r.assets not found in "):
            quietly(UnityPy.Environment(path=str(linked["folder"])).find_file, "r.assets")
        found = quietly(UnityPy.Environment(path=str(linked["folder"])).find_file, "Link.assets/r.assets")
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in found.objects.values()], ["r"])
        # A folder the walk cannot list yields nothing, with no error: a missing one, an unreadable subfolder (which
        # only a user other than root cannot read). A file walks as itself under "", and opening path/ raises.
        self.assertEqual(keys(str(self.tmp / "missing")), [])
        perm = self.tmp / "perm"
        (perm / "locked").mkdir(parents=True)
        (perm / "ok.bin").write_bytes(text_asset_file("ok"))
        (perm / "locked" / "x.bin").write_bytes(text_asset_file("x"))
        os.chmod(perm / "locked", 0)
        self.addCleanup(os.chmod, perm / "locked", 0o755)
        readable = os.access(perm / "locked", os.R_OK)
        self.assertEqual(keys(str(perm)), [str(perm / "ok.bin"), *([str(perm / "locked" / "x.bin")] * readable)])
        with self.assertRaisesRegex(NotADirectoryError, re.escape(str(perm / "ok.bin") + "/")):
            keys(str(perm / "ok.bin"))


if __name__ == "__main__":
    unittest.main()
