"""PARITY.md rows on loading, run against the oracle: `.splitN` names, `Environment.save`, the local-disk tests of a
non-local filesystem, a `path` the local disk cannot list, and the readers an input gets.

Sections 3, 5.2, 13 and 19.2. Like `test_parity_rows.py`, every input is built here (`synth.py`), so nothing needs
the corpus, and each test names the rows it pins; a row's test column names the test back. Needs UnityPy 1.25.2
(the oracle image); skipped elsewhere.
"""

import io
import os
import re
import shutil
import struct
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from rows_support import ORACLE, load, quietly, text_asset_file, zip_with_a_directory  # noqa: E402

import synth as S  # noqa: E402

if ORACLE:
    import UnityPy
    from fsspec.implementations.local import LocalFileSystem
    from fsspec.implementations.memory import MemoryFileSystem
    from UnityPy.helpers.ImportHelper import find_sensitive_path
    from UnityPy.streams import EndianBinaryReader


def names(env):
    return [o.parse_as_dict()["m_Name"] for o in env.objects]


class ReadOnly:
    """An object with `read` and no `seek` or `tell`."""

    def __init__(self, data):
        self.data = data

    def read(self, *_):
        return self.data


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class LoadingRows(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-loading-rows-"))
        self.addCleanup(shutil.rmtree, self.tmp)

    def chdir(self, path):
        cwd = os.getcwd()
        os.chdir(path)
        self.addCleanup(os.chdir, cwd)

    def memory_fs(self, *folders):
        """The process's one `MemoryFileSystem`, and these folders of it removed after the test."""
        fs = MemoryFileSystem()
        for folder in folders:
            self.addCleanup(lambda f=folder: fs.exists(f) and fs.rm(f, recursive=True))
        return fs

    # Section 3, `Environment.load_file` and `Environment.load`: a `.splitN` name is read from the name as given.
    def test_split_names_are_read_from_the_name_as_given(self):
        d, cwd = self.tmp / "d", self.tmp / "cwd"
        d.mkdir()
        cwd.mkdir()
        self.chdir(cwd)
        whole = text_asset_file("split")
        (d / "a.assets.split0").write_bytes(whole[:40])
        (d / "a.assets.split1").write_bytes(whole[40:])
        (d / "plain.assets").write_bytes(text_asset_file("plain"))
        # A plain name is joined to `path`; a `.splitN` name is not: its parts are read from the cwd, where there are
        # none, so it loads b"": an empty raw reader keyed by the base path and registered as a CAB, not None.
        env = UnityPy.Environment(path=str(d))
        quietly(env.load_file, "plain.assets")
        self.assertEqual((list(env.files), names(env)), (["plain.assets"], ["plain"]))
        for name, base in (("a.assets.split0", "a.assets"), ("missing.assets.split0", "missing.assets")):
            with self.subTest(name):
                env = UnityPy.Environment(path=str(d))
                reader = quietly(env.load_file, name)
                self.assertEqual((type(reader).__name__, reader.Length), ("EndianBinaryReader_Memoryview", 0))
                self.assertEqual((list(env.files), list(env.cabs), names(env)), ([base], [base], []))
                self.assertIs(env.get_cab(base), reader)
                self.assertIs(quietly(env.find_file, base), reader)
        # With the parts in the cwd, those load.
        other = text_asset_file("from the cwd")
        (cwd / "a.assets.split0").write_bytes(other[:40])
        (cwd / "a.assets.split1").write_bytes(other[40:])
        env = UnityPy.Environment(path=str(d))
        self.assertEqual(type(quietly(env.load_file, "a.assets.split0")).__name__, "SerializedFile")
        self.assertEqual((list(env.files), names(env)), (["a.assets"], ["from the cwd"]))
        # load hands load_file the stream fs.open gives, so a part is not merged: part 0, 40 bytes, loads alone as a
        # raw stream reader, keyed by its path and by its basename, and registered under the part's name.
        part = str(d / "a.assets.split0")
        env = UnityPy.Environment(path=str(d))
        quietly(env.load, [part])
        self.assertEqual(list(env.files), [part, "a.assets.split0"])
        self.assertEqual(type(env.files[part]).__name__, "EndianBinaryReader_Streamable")
        self.assertEqual((list(env.cabs), names(env)), (["a.assets.split0"], []))

    # Section 3, `Environment.save`: the builtin open, on the local disk, of `out_path` joined to each basename.
    def test_environment_save_writes_by_basename_on_the_local_disk(self):
        self.chdir(self.tmp)
        env = UnityPy.Environment()
        load(text_asset_file("one"), "x/s.assets", env)[0].mark_changed()
        # It creates no folder: with the default `output` missing, opening the file raises.
        with self.assertRaisesRegex(FileNotFoundError, re.escape("No such file or directory: 'output/s.assets'")):
            quietly(env.save)
        self.assertFalse(os.path.exists("output"))
        # Two changed files with one basename write one file, the later in `files` order winning, with no error.
        os.mkdir("output")
        env = UnityPy.Environment()
        one = load(text_asset_file("one"), "x/s.assets", env)[0]
        two = load(text_asset_file("two"), "y/s.assets", env)[0]
        one.mark_changed()
        two.mark_changed()
        quietly(env.save)
        self.assertEqual(os.listdir("output"), ["s.assets"])
        self.assertEqual(Path("output/s.assets").read_bytes(), two.save(packer="none"))
        self.assertNotEqual(two.save(packer="none"), one.save(packer="none"))
        # With a MemoryFileSystem as `fs`, the file is written to the local disk, and `fs` holds nothing.
        out = self.tmp / "out"
        out.mkdir()
        fs = self.memory_fs(str(out))
        env = UnityPy.Environment(fs=fs)
        mem = load(text_asset_file("mem"), "m.assets", env)[0]
        mem.mark_changed()
        quietly(env.save, out_path=str(out))
        self.assertEqual((out / "m.assets").read_bytes(), mem.save(packer="none"))
        self.assertFalse(fs.exists(f"{out}/m.assets"))

    # Sections 3 and 4, `Environment.load_file` and `ImportHelper.find_sensitive_path`: what `os.listdir` raises.
    def test_a_path_the_local_disk_cannot_list(self):
        self.chdir(self.tmp)
        a_file = self.tmp / "a_file"
        a_file.write_bytes(b"x")
        locked = self.tmp / "locked"
        locked.mkdir()
        os.chmod(locked, 0)
        self.addCleanup(os.chmod, locked, 0o755)
        readable = os.access(locked, os.R_OK)  # root reads a folder of mode 000, and lists nothing in it
        cases = [(str(a_file), NotADirectoryError), (str(self.tmp / "missing"), FileNotFoundError)]
        cases += [("", FileNotFoundError), (str(locked), None if readable else PermissionError)]
        for path, error in cases:
            for call in (
                lambda p: UnityPy.Environment(path=p).load_file("missing.assets"),
                lambda p: find_sensitive_path(p, "a/b"),
            ):
                with self.subTest(path=path, error=error):
                    if error is None:
                        self.assertIsNone(quietly(call, path))
                        continue
                    with self.assertRaises(error) as raised:
                        quietly(call, path)
                    self.assertEqual((type(raised.exception), raised.exception.filename), (error, path))

    # Sections 3 and 19.2, `Environment.fs`: the local-disk tests of `load_file` that send the read to a non-local fs.
    def test_non_local_load_file_follows_local_split_parts_and_case_variants(self):
        local = self.tmp / "d"
        local.mkdir()
        (local / "x.assets.split0").write_bytes(b"local bytes")
        self.assertFalse((local / "x.assets").exists())
        fs = self.memory_fs(str(local))
        whole = text_asset_file("in memory, split")
        fs.pipe({f"{local}/x.assets.split0": whole[:40], f"{local}/x.assets.split1": whole[40:]})
        # The local disk holds `{name}.split0`: load_file reads the parts through fs, from a local `path` and from the
        # default `''` (an absolute name, not joined).
        for path, name, key in ((str(local), "x.assets", "x.assets"), (None, f"{local}/x.assets", f"{local}/x.assets")):
            with self.subTest(path=path):
                env = UnityPy.Environment(fs=fs, path=path)
                self.assertEqual(env.path, path or "")
                self.assertEqual(type(quietly(env.load_file, name)).__name__, "SerializedFile")
                self.assertEqual(
                    (list(env.files), list(env.cabs), names(env)), ([key], ["x.assets"], ["in memory, split"])
                )
        # `path` holds a case variant find_sensitive_path resolves (`P/p/X.assets` for `P/x.assets`): load_file opens
        # the variant's path through fs, which lacks it, and raises naming it, though fs holds `P/x.assets`.
        self.chdir(self.tmp)
        folder = "Up-" + self.tmp.name
        (self.tmp / folder / folder.lower()).mkdir(parents=True)
        (self.tmp / folder / folder.lower() / "X.assets").write_bytes(b"local variant")
        fs = self.memory_fs(f"/{folder}")
        fs.pipe(f"{folder}/x.assets", text_asset_file("in memory"))
        self.assertEqual(fs.find(f"/{folder}"), [f"/{folder}/x.assets"])
        variant = os.path.join(folder, folder.lower(), "X.assets")
        self.assertEqual(find_sensitive_path(folder, os.path.join(folder, "x.assets")), variant)
        with self.assertRaisesRegex(FileNotFoundError, f"^{re.escape('/' + variant)}$"):
            quietly(UnityPy.Environment(fs=fs, path=folder).load_file, "x.assets")
        # A `.zip` or `.apk` file argument goes to load_zip_file, which tests and opens it through fs.
        root = f"/{self.tmp.name}-zip"
        fs = self.memory_fs(root)
        for name in ("z.zip", "z.apk"):
            fs.pipe(f"{root}/{name}", zip_with_a_directory())
            with self.subTest(name):
                self.assertFalse(os.path.exists(f"{root}/{name}"))
                env = quietly(UnityPy.Environment, f"{root}/{name}", fs=fs)
                self.assertEqual((list(env.files), names(env)), (["assets/", "assets/a.assets"], ["a"]))

    # Sections 13 and 5.2: past the end, the stream reader's Position stops at the end, the memory reader's does not.
    def test_the_stream_reader_stops_at_the_end(self):
        data = b"0123456789"
        path = self.tmp / "ten.bin"
        path.write_bytes(data)

        def read_past(reader):
            reader.Position = 8
            return reader.read_bytes(5), reader.Position

        self.assertEqual(read_past(EndianBinaryReader(data)), (b"89", 13))
        self.assertEqual(read_past(EndianBinaryReader(io.BytesIO(data))), (b"89", 10))
        for opener in (open, LocalFileSystem().open):
            with opener(str(path), "rb") as stream, self.subTest(opener=opener):
                self.assertEqual(read_past(EndianBinaryReader(stream)), (b"89", 10))
        # A plain WebFile whose header length (1000) lies past its 57 bytes, its one entry's name length (2000) too.
        payload = b"DATA" * 4
        web = b"UnityWebData1.0\0" + struct.pack("<i", 1000) + struct.pack("<iii", 0, len(payload), 2000)
        web += b"entry.bin" + payload
        self.assertEqual(len(web), 57)
        # From bytes the memory reader's Position passes 1000, the header loop ends, and that entry loads.
        loaded = load(web, "w.data")[0]
        self.assertEqual(list(loaded.files), ["entry.binDATADATADATADATA"])
        # From a path or an io.BytesIO the stream reader's stays at 57: the loop reads on, and struct.error escapes.
        web_path = self.tmp / "w.data"
        web_path.write_bytes(web)
        for source in (str(web_path), io.BytesIO(web)):
            raised = self.assertRaisesRegex(struct.error, "^unpack requires a buffer of 4 bytes$")
            with self.subTest(source=type(source).__name__), raised:
                quietly(UnityPy.Environment().load_file, source, name="w.data")
        self.assertEqual(list(load(S.web_file([(b"a.bin", b"xyz")]), "g.data")[0].files), ["a.bin"])

    # Sections 3, 13 and 19.2: `EndianBinaryReader`'s inputs, and the loaders'.
    def test_endian_binary_reader_inputs(self):
        data = text_asset_file("read only")
        # An object with `read` and no `seek` or `tell`: __new__ reads it into memory and picks the memory reader,
        # whose __init__ then gets the object itself.
        message = "^memoryview: a bytes-like object is required, not 'ReadOnly'$"
        for call in (
            lambda: EndianBinaryReader(ReadOnly(data)),
            lambda: UnityPy.load(ReadOnly(data)),
            lambda: UnityPy.Environment().load_file(ReadOnly(data), name="r.assets"),
        ):
            with self.subTest(call=call), self.assertRaisesRegex(TypeError, message):
                quietly(call)
        # The reference: the same bytes passed as bytes.
        self.assertEqual(names(quietly(UnityPy.load, data)), ["read only"])
        # A memory reader handed to EndianBinaryReader fails the same way; a stream reader is wrapped, and reads.
        with self.assertRaisesRegex(
            TypeError, "^memoryview: a bytes-like object is required, not 'EndianBinaryReader_Memoryview'$"
        ):
            EndianBinaryReader(EndianBinaryReader(b"abcd"))
        wrapped = EndianBinaryReader(EndianBinaryReader(io.BytesIO(b"abcdefgh")))
        self.assertEqual(
            (type(wrapped).__name__, wrapped.read_bytes(4), wrapped.Position),
            ("EndianBinaryReader_Streamable", b"abcd", 4),
        )
        # The loaders take a reader of either kind as it is.
        for reader in (EndianBinaryReader(data), EndianBinaryReader(io.BytesIO(data))):
            with self.subTest(reader=type(reader).__name__):
                self.assertEqual(names(quietly(UnityPy.load, reader)), ["read only"])


if __name__ == "__main__":
    unittest.main()
