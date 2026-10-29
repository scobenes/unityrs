"""PARITY.md rows on the package, its files and the Environment, run against the oracle.

Detection and loading (WebFile, UnityArchive, ZIP), bundles and their save
flags, codecs and reads past the end, paths and the case-insensitive walk, the
CAB registry, split files and File's own methods, each on the smallest input
that shows it, built here (`synth.py`). Each test names the PARITY.md rows it
keeps honest, and each row's test column names the test back
(test_parity_rows.py checks both). Needs UnityPy 1.25.2 (the oracle image);
skipped elsewhere.
"""

import gzip
import io
import os
import random
import re
import struct
import sys
import unittest
import zipfile
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

import inventory  # noqa: E402
import synth as S  # noqa: E402
from parity_rows import ORACLE, TEXT_ASSET, RowsCase, load, quietly, text_asset_file  # noqa: E402

try:
    import lz4.block
    import UnityPy
    from UnityPy.classes import PPtr
    from UnityPy.enums import ArchiveFlags, ArchiveFlagsOld, ClassIDType, FileType
    from UnityPy.files import BundleFile, WebFile
    from UnityPy.helpers import ImportHelper, ResourceReader
    from UnityPy.helpers.ContainerHelper import ContainerHelper
    from UnityPy.streams import EndianBinaryReader
except ImportError:  # pragma: no cover - outside the oracle image (ORACLE is False)
    pass


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class FileRows(RowsCase):
    # Sections 4, 5.2, 6: WebFile gzip.
    def test_gzip_webfile_is_detected_and_never_parsed(self):
        plain = S.web_file([(b"a.txt", bytes(range(256)) * 2), (b"b.txt", b"world" * 40)])
        packed = gzip.compress(plain, mtime=0)
        self.assertEqual(list(load(plain)[0].files), ["a.txt", "b.txt"], "a plain WebFile loads")

        # Detection reads the gzip stream through the shared reader and leaves it where it stopped.
        kind, reader = ImportHelper.check_file_type(packed)
        self.assertEqual(kind, FileType.WebFile)
        self.assertNotEqual(reader.Position, 0)
        # So WebFile reads its magic past the start, takes the file for plain, and the signature fails.
        with self.assertRaisesRegex(ValueError, r"^Invalid WebFile signature: '\\x1f\\udc8b\\x08'"):
            load(packed)
        path = self.tmp / "web.data.gz"
        path.write_bytes(packed)
        with self.assertRaisesRegex(ValueError, "Invalid WebFile signature"):
            quietly(UnityPy.load, str(path))
        # Inside a bundle too.
        with self.assertRaisesRegex(ValueError, "Invalid WebFile signature"):
            load(S.fs_bundle([(b"inner.data.gz", packed, 0)]))
        # The decoder itself works on a fresh reader, and agrees with gzip.decompress plus the plain layout.
        self.assertEqual(list(WebFile(EndianBinaryReader(packed), None).files), ["a.txt", "b.txt"])
        self.assertEqual(gzip.decompress(packed), plain)

    # Sections 5.1, 18: UnityArchive.
    def test_unityarchive_loads_as_a_raw_file(self):
        data = S.fs_bundle([(b"CAB-test", text_asset_file(), 4)], signature=b"UnityArchive")
        self.assertEqual(ImportHelper.check_file_type(data)[0], FileType.ResourceFile)
        loaded, env = load(data, name="archive")
        self.assertIsInstance(loaded, EndianBinaryReader)
        self.assertIs(env.get_cab("archive"), loaded)
        with self.assertRaisesRegex(NotImplementedError, "UnityArchive"):
            BundleFile(EndianBinaryReader(data), env)

    # Sections 3, 5.2, 14.1: one CAB registry for the whole Environment.
    def test_same_named_cabs_share_one_registry_entry(self):
        env = UnityPy.Environment()

        def bundle(name, fill):
            return S.fs_bundle([(b"CAB-same", text_asset_file(name), 4), (b"CAB-same.resS", fill * 64, 0)])

        a, _ = load(bundle("A", b"\x11"), "a", env)
        b, _ = load(bundle("B", b"\x22"), "b", env)
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in env.objects], ["A", "B"])
        self.assertEqual(list(env.cabs), ["cab-same", "cab-same.ress"])
        self.assertIs(env.cabs["cab-same"], b.files["CAB-same"], "the last one loaded wins")
        # A's texture data is read from B's .resS.
        got = ResourceReader.get_resource_data("archive:/CAB-same/CAB-same.resS", a.files["CAB-same"], 0, 4)
        self.assertEqual(got, b"\x22" * 4)
        # Environment.container joins only the registered copy's container.
        info = SimpleNamespace(asset=SimpleNamespace(path_id=1), preloadIndex=0, preloadSize=0)
        a.files["CAB-same"]._container = ContainerHelper([("a/path", info)])
        b.files["CAB-same"]._container = ContainerHelper([("b/path", info)])
        self.assertEqual(list(env.container.keys()), ["b/path"])

    # Section 7.1: type dependencies sit inside the tree flag.
    def test_type_dependencies_need_the_tree_flag(self):
        with_tree = S.serialized_file([S.Type(49, TEXT_ASSET, deps=[7, 8])], [S.Obj(1, 0, S.s32("a") + S.s32("b"))])
        self.assertEqual(load(with_tree)[0].types[0].type_dependencies, (7, 8))
        stripped = text_asset_file(enable_type_tree=False)
        loaded = load(stripped)[0]
        self.assertIsNone(loaded.types[0].type_dependencies)
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in loaded.objects.values()], ["hello"])

        class WithDependencies(S.Type):
            def encode(self, version, enable_type_tree, is_ref, E):
                return super().encode(version, enable_type_tree, is_ref, E) + struct.pack(E + "i", 0)

        misread = S.serialized_file(
            [WithDependencies(49, TEXT_ASSET)], [S.Obj(1, 0, S.s32("a") + S.s32("b"))], enable_type_tree=False
        )
        with self.assertRaises(struct.error):
            load(misread)

    # Section 6: LZ4 blocks.
    def test_lz4_stored_size_is_a_capacity(self):
        entries = [(b"CAB-test", text_asset_file(), 4), (b"CAB-test.resS", b"\x11" * 64, 0)]
        size = sum(len(data) for _, data, _ in entries)
        loaded = load(S.fs_bundle(entries, stored_sizes=[size + 100]))[0]
        self.assertEqual(list(loaded.files), ["CAB-test", "CAB-test.resS"], "a block that decodes short loads")
        for stored in (size - 10, 0):
            with self.subTest(stored=stored), self.assertRaises(lz4.block.LZ4BlockError):
                load(S.fs_bundle(entries, stored_sizes=[stored]))

    # Sections 5.2, 13: reads past the end.
    def test_reads_past_the_end_return_short_data(self):
        sf = text_asset_file()
        data = sf + b"\x11" * 64
        rows = [(b"CAB-test", 0, len(sf), 4), (b"CAB-test.resS", len(sf), 64 + 1000, 0)]
        loaded = load(S.fs_bundle(rows, blocks=[data], codec=0))[0]
        self.assertEqual(loaded.files["CAB-test.resS"].Length, 64, "the entry is cut to the bytes that exist")
        got = ResourceReader.get_resource_data("CAB-test.resS", loaded.files["CAB-test"], 60, 500)
        self.assertEqual(got, b"\x11" * 4)
        reader = EndianBinaryReader(b"abcd")
        self.assertEqual((reader.read_bytes(10), reader.Position), (b"abcd", 10))
        with self.assertRaises(struct.error):
            reader.read_int()

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
        env = quietly(UnityPy.load, str(path))
        reader = env.files[str(path)]
        self.addCleanup(reader.dispose)
        self.assertEqual(type(reader).__name__, "EndianBinaryReader_Streamable")
        # With no NUL in reach the stream reader never raises and drops the last byte it read.
        stream = EndianBinaryReader(io.BytesIO(b"C" * 10))
        self.assertEqual((stream.read_string_to_null(), stream.Position), ("C" * 9, 10))
        stream = EndianBinaryReader(io.BytesIO(b"B" * 40000))
        self.assertEqual((len(stream.read_string_to_null()), stream.Position), (32766, 32767))
        memory = EndianBinaryReader(b"B" * 40000)
        self.assertEqual((len(memory.read_string_to_null()), memory.Position), (32767, 32767))
        # A string with its NUL in reach reads the same from both.
        for reader in (EndianBinaryReader(io.BytesIO(b"2021.3.2f1\0rest")), EndianBinaryReader(b"2021.3.2f1\0rest")):
            self.assertEqual((reader.read_string_to_null(), reader.Position), ("2021.3.2f1", 11))

    # Section 3: Environment.load and load_file.
    def test_load_keys_each_file_by_path_and_basename(self):
        (self.tmp / "sub").mkdir()
        path = self.tmp / "sub" / "asset"
        path.write_bytes(text_asset_file())
        env = UnityPy.Environment()
        quietly(env.load, [str(path)])
        self.assertEqual(list(env.files), [str(path), "asset"])
        self.assertEqual(len(env.objects), 2)
        self.chdir(self.tmp / "sub")
        env = UnityPy.Environment()
        quietly(env.load, ["asset"])
        self.assertEqual((list(env.files), len(env.objects)), (["asset"], 1))

    def test_load_file_tries_the_cwd_first(self):
        (self.tmp / "cwd").mkdir()
        (self.tmp / "env").mkdir()
        (self.tmp / "cwd" / "x").write_bytes(text_asset_file("from the cwd"))
        (self.tmp / "env" / "x").write_bytes(text_asset_file("from env.path"))
        self.chdir(self.tmp / "cwd")
        env = UnityPy.Environment(path=str(self.tmp / "env"))
        loaded = quietly(env.load_file, "x")
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in loaded.objects.values()], ["from the cwd"])
        (self.tmp / "cwd" / "x").unlink()
        loaded = quietly(env.load_file, "x")
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in loaded.objects.values()], ["from env.path"])

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
        self.assertFalse(os.path.exists(root), "nothing of that name on the local disk")
        self.assertEqual(UnityPy.Environment(fs=fs).path, "")
        # load_file tests a name with os.path and walks `path` with os.listdir, both on the local disk.
        for name in (f"{root}/x.assets", "x.assets", "missing"):
            with self.subTest(name), self.assertRaises(FileNotFoundError):
                quietly(UnityPy.Environment(fs=fs).load_file, name)
        with self.assertRaises(FileNotFoundError):
            quietly(UnityPy.Environment(fs=fs, path=root).load_file, "x.assets")
        # Where the local disk can list `path`, the walk finds nothing there and the name is None.
        self.assertIsNone(quietly(UnityPy.Environment(fs=fs, path=str(self.tmp)).load_file, "x.assets"))
        # The constructor takes a file through load_file, a folder through load_files' builtin open.
        for arg in (f"{root}/x.assets", root):
            with self.subTest(arg), self.assertRaises(FileNotFoundError):
                quietly(UnityPy.Environment, arg, fs=fs)
        with self.assertRaises(FileNotFoundError):
            quietly(UnityPy.Environment(fs=fs).load_folder, root)
        # load (fs.exists, fs.open) and a stream do read through the filesystem.
        env = UnityPy.Environment(fs=fs)
        quietly(env.load, [f"{root}/x.assets"])
        self.assertEqual(list(env.files), [f"{root}/x.assets", "x.assets"])
        with fs.open(f"{root}/x.assets", "rb") as stream:
            loaded = quietly(UnityPy.Environment(fs=fs).load_file, stream, name="x.assets")
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in loaded.objects.values()], ["in memory"])

    # Section 18: BundleFile.save flags.
    def test_bundle_save_flags(self):
        bundle = load(S.fs_bundle([(b"CAB-test", text_asset_file(), 4)]))[0]
        saved = bundle.save()
        again = load(saved)[0]
        self.assertEqual((int(again.dataflags), again._block_info_flags), (0x40, 0x40))
        empty = load(S.fs_bundle([]))[0]
        self.assertEqual(empty.files, {})
        with self.assertRaisesRegex(AttributeError, "_block_info_flags"):
            empty.save(packer="original")

    # Sections 12, 18, 19.2: save_fs XORs the flag set's whole encryption mask out when any bit of it is set.
    def test_bundle_save_xors_the_whole_encryption_mask(self):
        def header_flags(data):
            end = data.index(b"\0") + 1 + 4  # signature, format
            end = data.index(b"\0", data.index(b"\0", end) + 1) + 1  # player and engine versions
            return struct.unpack_from(">I", data, end + 16)[0]  # after the size and the two blocks-info sizes

        def saved(engine, extra):
            bundle = load(S.fs_bundle([(b"CAB-test", text_asset_file(), 4)], engine=engine))[0]
            bundle.dataflags = type(bundle.dataflags)(int(bundle.dataflags) | extra)
            return type(bundle.dataflags), bundle.save(packer="original")

        new, old = b"2021.3.2f1", b"2019.4.1f1"
        cases = {
            # ArchiveFlags' mask is 0x1400: one bit of it comes out as the other, which reads back as encrypted.
            (new, 0x1000): (ArchiveFlags, 0x0443, LookupError),
            (new, 0x0400): (ArchiveFlags, 0x1043, LookupError),
            (new, 0x1400): (ArchiveFlags, 0x0043, None),
            (new, 0x0200): (ArchiveFlags, 0x0243, None),  # BlockInfoNeedPaddingAtStart, kept
            # ArchiveFlagsOld's mask is 0x200 alone, which is cleared; its other bits are not encryption.
            (old, 0x0200): (ArchiveFlagsOld, 0x0043, None),
            (old, 0x1000): (ArchiveFlagsOld, 0x1043, None),
        }
        for (engine, extra), (flag_set, flags, error) in cases.items():
            with self.subTest(engine=engine, extra=hex(extra)):
                kind, data = saved(engine, extra)
                self.assertEqual((kind, hex(header_flags(data))), (flag_set, hex(flags)))
                if error is None:
                    self.assertEqual(list(load(data)[0].files), ["CAB-test"])
                else:
                    with self.assertRaisesRegex(error, "^The BundleFile is encrypted, but no key was provided"):
                        load(data)
        # The data blocks' own 0x100 bit is not in the mask, so `original` writes it back.
        bundle = load(S.fs_bundle([(b"CAB-test", text_asset_file(), 4)]))[0]
        bundle._block_info_flags |= 0x100
        again = load(bundle.save(packer="original"))[0]
        self.assertEqual((again._block_info_flags, list(again.files)), (0x102, ["CAB-test"]))

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
        with open(path, "rb") as stream:
            env = quietly(UnityPy.load, stream)
        self.assertEqual(sorted(type(f).__name__ for f in env.files.values()), ["NoneType", "SerializedFile"])

    # Sections 2, 2.1: the package's aliases and every re-export section 2.1 lists.
    def test_package_aliases_and_reexports(self):
        from UnityPy.helpers import ArchiveStorageManager

        self.assertIs(UnityPy.load, UnityPy.Environment)
        self.assertIs(UnityPy.AssetsManager, UnityPy.Environment)
        self.assertIs(UnityPy.set_assetbundle_decrypt_key, ArchiveStorageManager.set_assetbundle_decrypt_key)
        text = (HERE.parent.parent / "PARITY.md").read_text(encoding="utf-8")
        rows = next(body for header, body in inventory.tables(text) if header == ["package", "source", "members"])
        self.assertEqual(len(rows), 31)
        for package, _, members in rows:
            module = __import__(package.strip("`"), fromlist=["_"])
            for member in re.findall(r"`([A-Za-z_0-9]+)`", members):
                with self.subTest(package, member=member):
                    value = getattr(module, member)
                    self.assertFalse(isinstance(value, type(module)), "a re-export, not a submodule")
        self.assertIsInstance(UnityPy.files.BundleFile, type)
        self.assertTrue(callable(UnityPy.cli.update_tpk))
        # helpers and export re-export only their own submodules; export leaves ShaderConverter out.
        self.assertNotIn("ShaderConverter", UnityPy.export.__all__)
        self.assertEqual(len(UnityPy.helpers.__all__), 6)

    # Section 3: Environment.file, objects, assets, the CAB registry and find_file.
    def test_environment_registry_and_find_file(self):
        self.assertIsNone(UnityPy.Environment().typetree_generator)
        one = quietly(UnityPy.Environment, text_asset_file())
        self.assertIs(one.file, next(iter(one.files.values())), "set by the constructor for exactly one file")
        for env in (quietly(UnityPy.Environment, text_asset_file(), text_asset_file("b")), UnityPy.Environment()):
            self.assertFalse(hasattr(env, "file"))
        later = UnityPy.Environment()
        load(text_asset_file(), env=later)
        self.assertFalse(hasattr(later, "file"), "load_file never sets it")
        # objects and assets: depth-first in `files` order, dependencies left out.
        env = UnityPy.Environment()
        plain = load(text_asset_file("plain"), "plain.assets", env)[0]
        quietly(env.load_file, text_asset_file("dependency"), name="dep.assets", is_dependency=True)
        bundle = load(S.fs_bundle([(b"CAB-a", text_asset_file("in bundle"), 4), (b"CAB-a.resS", b"\x11" * 8, 0)]))[0]
        quietly(env.load_file, S.fs_bundle([(b"CAB-a", text_asset_file("in bundle"), 4)]), name="b")
        inner = env.files["b"].files["CAB-a"]
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in env.objects], ["plain", "in bundle"])
        self.assertEqual(env.assets, [plain, inner])
        self.assertEqual(len(bundle.files), 2)
        # simplify_name is the lower-cased basename, extension kept; the registry is keyed by it.
        from UnityPy.environment import simplify_name

        self.assertEqual(simplify_name("dir\\Sub/CAB-AbC.resS"), "cab-abc.ress")
        env = UnityPy.Environment()
        first, second = object(), object()
        env._container_index_built = True
        env.register_cab("dir/Sub/CAB-AbC.resS", first)
        self.assertFalse(env._container_index_built, "registering invalidates the container index")
        self.assertIs(env.get_cab("other\\cab-abc.RESS"), first)
        env.register_cab("elsewhere/cab-abc.ress", second)
        self.assertEqual((list(env.cabs), env.get_cab("CAB-ABC.RESS")), (["cab-abc.ress"], second))
        self.assertIsNone(env.get_cab("missing"))
        # find_file: the registry, then path/name, then a walk of `path` built once.
        d = self.tmp / "d"
        (d / "sub").mkdir(parents=True)
        (d / "Shared.Assets").write_bytes(text_asset_file("shared"))
        (d / "sub" / "other.assets").write_bytes(text_asset_file("other"))
        (d / "sub" / "xab.assets").write_bytes(text_asset_file("xab"))

        def found(env, name):
            got = quietly(env.find_file, name)
            return got if got is None or got is first else [o.parse_as_dict()["m_Name"] for o in got.objects.values()]

        env = UnityPy.Environment(path=str(d))
        env.register_cab("CAB-registered", first)
        self.assertIs(found(env, "cab-REGISTERED"), first)
        self.assertEqual(found(env, "Shared.Assets"), ["shared"], "path/name")
        self.assertEqual(found(env, "other.assets"), ["other"], "a suffix of a walked path")
        self.assertEqual(found(env, "ab.assets"), ["xab"], "a suffix with no separator boundary")
        # A name in another case misses the exact matches (the simplified one compares a bare name with full
        # paths) and matches a simplified path by suffix: that path names no file, so load_file gives None.
        self.assertIsNone(found(UnityPy.Environment(path=str(d)), "shared.assets"))
        with self.assertRaisesRegex(FileNotFoundError, "File missing.assets not found in"):
            found(env, "missing.assets")
        (d / "sub" / "late.assets").write_bytes(text_asset_file("late"))
        with self.assertRaises(FileNotFoundError):
            found(env, "late.assets")  # the walk is not built again
        self.assertEqual(found(UnityPy.Environment(path=str(d)), "late.assets"), ["late"])

    # Sections 3, 5.2: split files, folders, Environment.save and File's own attributes.
    def test_split_files_folders_and_save(self):
        from fsspec.implementations.memory import MemoryFileSystem
        from UnityPy.environment import reSplit
        from UnityPy.files import File

        self.assertEqual(reSplit.pattern, r"(.*?([^\/\\]+?))\.split\d+")
        self.assertEqual(reSplit.match("a/b.c.split12").groups(), ("a/b.c", "b.c"))
        d = self.tmp / "d"
        (d / "sub").mkdir(parents=True)
        data = text_asset_file("split")
        half = len(data) // 2
        (d / "big.split0").write_bytes(data[:half])
        (d / "big.split1").write_bytes(data[half:])
        (d / "big.split3").write_bytes(b"after a gap")
        (d / "nozero.split1").write_bytes(b"one")
        (d / "nozero.split2").write_bytes(b"two")
        env = UnityPy.Environment(path=str(d))
        # Up to the first missing part after split0; a missing split0 is skipped.
        self.assertEqual(env._load_split_file(str(d / "big")), data)
        self.assertEqual(env._load_split_file(str(d / "nozero")), b"onetwo")
        loaded = quietly(env.load_file, str(d / "big.split1"))
        self.assertEqual(
            (list(env.files), [o.parse_as_dict()["m_Name"] for o in loaded.objects.values()]),
            ([str(d / "big")], ["split"]),
        )
        # At most 999 parts, split0 to split998.
        fs = MemoryFileSystem()
        root = f"/{self.tmp.name}-parts"
        self.addCleanup(fs.rm, root, recursive=True)
        for i in range(1001):
            fs.pipe(f"{root}/p.split{i}", b"x")
        self.assertEqual(len(UnityPy.Environment(fs=fs)._load_split_file(f"{root}/p")), 999)
        # load_files (load_assets with the builtin open) merges split members once per base path. UnityPy never
        # closes what it opens there, so the files are closed here.
        (d / "sub" / "plain.assets").write_bytes(text_asset_file("plain"))
        opened, real_open = [], open  # the real one, taken before builtins.open is patched to `tracked`

        def tracked(path, mode="r", *args, **kw):
            stream = real_open(path, mode, *args, **kw)  # noqa: SIM115 - UnityPy reads it after the call
            opened.append(stream)
            return stream

        env = UnityPy.Environment()
        with mock.patch("builtins.open", tracked):
            quietly(env.load_files, [str(d / "big.split0"), str(d / "big.split1"), str(d / "sub" / "plain.assets")])
        self.assertEqual(list(env.files), [str(d / "big"), str(d / "sub" / "plain.assets")])
        # load_folder: every file fs.walk finds, in its order, through load_files.
        folder = self.tmp / "folder"
        (folder / "b").mkdir(parents=True)
        for name in ("z.assets", "a.assets", "b/m.assets"):
            (folder / name).write_bytes(text_asset_file(name))
        env = UnityPy.Environment()
        with mock.patch("builtins.open", tracked):
            quietly(env.load_folder, str(folder))
        for stream in opened:
            stream.close()
        walked = ["/".join([root, f]) for root, _, files in env.fs.walk(str(folder)) for f in files]
        self.assertEqual(list(env.files), walked)
        self.assertEqual(sorted(walked), sorted(str(folder / n) for n in ("z.assets", "a.assets", "b/m.assets")))
        # File: its attributes; mark_changed marks every File above; save writes the changed ones by basename.
        env = UnityPy.Environment()
        bundle = load(S.fs_bundle([(b"CAB-a", text_asset_file(), 4)]), "dir/b.bundle", env)[0]
        sf = bundle.files["CAB-a"]
        self.assertEqual(
            (bundle.name, bundle.cab_file, bundle.is_changed, bundle.parent, bundle.environment),
            ("b.bundle", "CAB-UnityPy_Mod.resS", False, env, env),
        )
        self.assertEqual((sf.name, sf.parent, sf.environment), ("CAB-a", bundle, env))
        self.assertTrue(isinstance(bundle, File) and isinstance(sf, File))
        sf.mark_changed()
        self.assertEqual((sf.is_changed, bundle.is_changed), (True, True))
        out = self.tmp / "out"
        out.mkdir()
        load(text_asset_file("unchanged"), "u.assets", env)
        quietly(env.save, out_path=str(out))
        self.assertEqual(sorted(p.name for p in out.iterdir()), ["b.bundle"])
        self.assertEqual((out / "b.bundle").read_bytes(), bundle.save(packer="none"))
        # File.container: the union of its File children's containers.
        info = SimpleNamespace(asset=PPtr(m_FileID=0, m_PathID=7), preloadIndex=0, preloadSize=0)
        sf._container = ContainerHelper([("a/path", info), ("b/path", info), ("a/path", info)])
        self.assertEqual(bundle.container, {"a/path": info.asset, "b/path": info.asset})
        self.assertEqual((sorted(sf.container.keys()), sf.container.values()), (["a/path", "b/path"], [info.asset]))

    # Sections 5.2, 19.2: File's iteration and dict-style methods.
    def test_file_iteration_and_dict_access(self):
        def two(name):
            objects = [S.Obj(1, 0, S.s32(name) + S.s32("s")), S.Obj(2, 0, S.s32(f"{name}2") + S.s32("s"))]
            return S.serialized_file([S.Type(49, TEXT_ASSET)], objects)

        bundle = load(S.fs_bundle([(b"CAB-a", two("a"), 4), (b"CAB-a.resS", b"\x11" * 16, 0)]), "b")[0]
        sf, raw = bundle.files["CAB-a"], bundle.files["CAB-a.resS"]
        self.assertIsInstance(raw, EndianBinaryReader)
        text_asset = [ClassIDType.TextAsset]
        # A raw reader is skipped: the bundle's objects are the SerializedFile's two.
        self.assertEqual(list(bundle.get_objects()), list(sf.objects.values()))
        self.assertEqual(list(bundle.get_assets()), [sf])
        self.assertEqual([o.path_id for o in bundle.get_filtered_objects(text_asset)], [1, 2])
        self.assertEqual(list(bundle.get_filtered_objects([ClassIDType.Texture2D])), [])
        # With no types, `return self.get_objects()` ends the generator: nothing.
        self.assertEqual((list(bundle.get_filtered_objects()), list(bundle.get_filtered_objects([]))), ([], []))
        # A SerializedFile's `files` is its objects: get_objects yields them through the ObjectReader branch,
        # get_filtered_objects takes neither branch, and get_assets' `return self` ends its generator.
        self.assertIs(sf.files, sf.objects)
        self.assertEqual(list(sf.get_objects()), list(sf.objects.values()))
        self.assertEqual((list(sf.get_filtered_objects(text_asset)), list(sf.get_assets())), ([], []))
        # A nested container is iterated as `f.objects`, which BundleFile and WebFile lack.
        inner = S.fs_bundle([(b"CAB-i", two("i"), 4)])
        nested = load(S.fs_bundle([(b"inner", inner, 4), (b"CAB-o", two("o"), 4)]), "n")[0]
        web = load(S.web_file([(b"inner", inner)]), "w")[0]
        self.assertIsInstance(web.files["inner"], BundleFile)
        for container in (nested, web):
            for call in (container.get_objects, lambda c=container: c.get_filtered_objects(text_asset)):
                with self.assertRaisesRegex(AttributeError, "'BundleFile' object has no attribute 'objects'"):
                    list(call())
        # get_assets recurses: the reference those raises are checked against.
        self.assertEqual([f.name for f in nested.get_assets()], ["CAB-i", "CAB-o"])
        # `get` is an attribute lookup; keys, items, values and [] are those of `files`.
        self.assertEqual((bundle.get("CAB-a"), bundle.get("nope", 5)), (None, 5))
        self.assertIs(bundle.get("files"), bundle.files)
        self.assertEqual(list(bundle.keys()), ["CAB-a", "CAB-a.resS"])
        self.assertEqual(list(bundle.values()), [sf, raw])
        self.assertEqual(list(bundle.items()), [("CAB-a", sf), ("CAB-a.resS", raw)])
        self.assertIs(bundle["CAB-a"], sf)
        self.assertEqual((list(sf.keys()), sf[2]), ([1, 2], sf.objects[2]))

    # Section 6: the codec helpers and tables.
    def test_compression_helpers(self):
        import lzma

        import brotli
        from UnityPy.enums import CompressionFlags as C
        from UnityPy.helpers import CompressionHelper as H

        data = bytes(range(256)) * 700  # 175 KiB, compressible
        self.assertEqual(sorted(H.DECOMPRESSION_MAP), [C.NONE, C.LZMA, C.LZ4, C.LZ4HC, C.LZHAM])
        self.assertEqual(sorted(H.COMPRESSION_MAP), [C.NONE, C.LZMA, C.LZ4, C.LZ4HC])
        self.assertEqual(H.COMPRESSION_MAP[C.LZ4HC], H.compress_lz4)
        self.assertEqual(H.DECOMPRESSION_MAP[C.LZ4HC], H.decompress_lz4)
        self.assertEqual(
            H.COMPRESSION_CHUNK_SIZE_MAP, {C.NONE: 0xFFFFFFFF, C.LZMA: 0xFFFFFFFF, C.LZ4: 0x20000, C.LZ4HC: 0x20000}
        )
        self.assertEqual((H.GZIP_MAGIC, H.BROTLI_MAGIC), (b"\x1f\x8b", b"brotli"))
        # LZHAM and any other codec value.
        with self.assertRaisesRegex(NotImplementedError, "LZHAM"):
            H.decompress_lzham(b"x", 1)
        bundle = load(S.fs_bundle([(b"CAB-test", text_asset_file(), 4)]))[0]
        with self.assertRaisesRegex(NotImplementedError, "LZHAM"):
            bundle.decompress_data(b"x", 1, 4)
        for codec in (5, 6, 7, 8, 63, 0x40 | 9):
            with self.subTest(codec=codec), self.assertRaisesRegex(ValueError, "^Unknown compression"):
                bundle.decompress_data(b"x", 1, codec)
        # LZMA: props 0x5D, an 8 MiB dictionary, lc 3 lp 0 pb 2, BT4, nice_len 123, an optional u64 size.
        filters = [
            {"id": lzma.FILTER_LZMA1, "dict_size": 1 << 23, "lc": 3, "lp": 0, "pb": 2, "mf": lzma.MF_BT4}
            | {"mode": lzma.MODE_NORMAL, "nice_len": 123}
        ]
        raw = lzma.compress(data, format=lzma.FORMAT_RAW, filters=filters)
        self.assertEqual(H.compress_lzma(data), b"\x5d" + (1 << 23).to_bytes(4, "little") + raw)
        self.assertEqual(H.compress_lzma(data, True), H.compress_lzma(data)[:5] + struct.pack("<Q", len(data)) + raw)
        self.assertEqual(H.decompress_lzma(H.compress_lzma(data)), data)
        self.assertEqual(H.decompress_lzma(H.compress_lzma(data, True), True), data)
        # LZ4: high compression at level 9, no size prefix.
        self.assertEqual(
            H.compress_lz4(data), lz4.block.compress(data, mode="high_compression", compression=9, store_size=False)
        )
        # brotli at the library's default quality, 11; gzip at level 9 with the time as the header's mtime.
        self.assertEqual(H.compress_brotli(data), brotli.compress(data, quality=11))
        with mock.patch("time.time", return_value=1000.0):
            first = H.compress_gzip(data)
        with mock.patch("time.time", return_value=2000.0):
            second = H.compress_gzip(data)
        self.assertEqual((first[4:8], second[4:8]), (struct.pack("<I", 1000), struct.pack("<I", 2000)))
        self.assertEqual((first[8], first[10:], H.decompress_gzip(first)), (2, second[10:], data))  # XFL 2: level 9
        # chunk_based_compress: LZ4 in 128 KiB chunks, LZMA and none in one; a chunk that grows keeps its bytes
        # with the codec bits cleared.
        packed, blocks = H.chunk_based_compress(data, 0x42)
        self.assertEqual([(size, flag) for size, _, flag in blocks], [(0x20000, 0x42), (len(data) - 0x20000, 0x42)])
        self.assertEqual(lz4.block.decompress(packed[: blocks[0][1]], 0x20000), data[:0x20000])
        self.assertEqual([(u, f) for u, _, f in H.chunk_based_compress(data, 1)[1]], [(len(data), 1)])
        self.assertEqual(H.chunk_based_compress(data, 0x40), (data, [(len(data), len(data), 0x40)]))
        noise = random.Random(3).randbytes(1000)
        self.assertEqual(H.chunk_based_compress(noise, 0x42), (noise, [(1000, 1000, 0x40)]))
        with self.assertRaisesRegex(NotImplementedError, "No compression function"):
            H.chunk_based_compress(data, 4)

    # Sections 5.2, 6: WebFile sniffs gzip at offset 0 and brotli at 0x20, given a reader at its start.
    def test_webfile_sniffs_its_codec(self):
        from UnityPy.helpers import CompressionHelper as H

        plain = S.web_file([(b"a.txt", b"hello")])
        gz = WebFile(EndianBinaryReader(gzip.compress(plain)), None)
        self.assertEqual((gz.packer, list(gz.files)), ("gzip", ["a.txt"]))
        marked = bytes(0x20) + b"brotli" + bytes(10)
        with mock.patch.object(H, "decompress_brotli", lambda data: plain) as stub:
            br = WebFile(EndianBinaryReader(marked), None)
        self.assertEqual((br.packer, list(br.files), stub(b"")), ("brotli", ["a.txt"], plain))
        self.assertEqual(WebFile(EndianBinaryReader(plain), None).packer, "none")


if __name__ == "__main__":
    unittest.main()
