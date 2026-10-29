"""PARITY.md rows a review disputed, run against the oracle on the smallest input that shows each.

Every input is built here (`synth.py`), so nothing needs the corpus. Each test names the
PARITY.md rows it keeps honest; a row's test column names the test back. Needs UnityPy
1.25.2 (the oracle image); skipped elsewhere.
"""

import contextlib
import gzip
import io
import os
import struct
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

import inventory  # noqa: E402
import synth as S  # noqa: E402

try:
    import lz4.block
    import UnityPy
    from UnityPy.classes import GameObject, PPtr, StreamingInfo, Texture2D
    from UnityPy.enums import FileType
    from UnityPy.export import Texture2DConverter
    from UnityPy.files import BundleFile, WebFile
    from UnityPy.helpers import ImportHelper, ResourceReader, Tpk, TypeTreeHelper
    from UnityPy.helpers.ContainerHelper import ContainerHelper
    from UnityPy.helpers.TypeTreeNode import TypeTreeNode
    from UnityPy.helpers.TypeTreeNode import clean_name as node_clean_name
    from UnityPy.helpers.UnityVersion import UnityVersion
    from UnityPy.streams import EndianBinaryReader

    ORACLE = UnityPy.__version__ == inventory.PINNED_VERSION
except ImportError:  # pragma: no cover - outside the oracle image
    ORACLE = False

TEXT_ASSET = S.Node("TextAsset", "Base", [S.Node("string", "m_Name"), S.Node("string", "m_Script")])


def text_asset_file(name="hello", script="world", **kw):
    """A format-22 SerializedFile holding one TextAsset (path id 1)."""
    return S.serialized_file([S.Type(49, TEXT_ASSET)], [S.Obj(1, 0, S.s32(name) + S.s32(script))], **kw)


def quietly(call, *args, **kw):
    """UnityPy prints a failing parse to stdout before re-raising it."""
    with contextlib.redirect_stdout(io.StringIO()):
        return call(*args, **kw)


def load(data, name="f.assets", env=None):
    env = env or UnityPy.Environment()
    return quietly(env.load_file, data, name=name), env


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class Rows(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-rows-"))
        self.addCleanup(lambda: __import__("shutil").rmtree(self.tmp))

    def chdir(self, path):
        cwd = os.getcwd()
        os.chdir(path)
        self.addCleanup(os.chdir, cwd)

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

    # Sections 8 and 10: an object with no tree anywhere.
    def test_no_tree_raises_the_tpk_error(self):
        def parse(class_id, **kw):
            sf = S.serialized_file(
                [S.Type(class_id, TEXT_ASSET)],
                [S.Obj(1, 0, S.s32("a") + S.s32("b"), class_id=class_id)],
                enable_type_tree=False,
                **kw,
            )
            return next(iter(load(sf)[0].objects.values())).parse_as_dict()

        self.assertEqual(parse(49), {"m_Name": "a", "m_Script": "b"}, "the TPK's TextAsset")
        with self.assertRaises(KeyError):
            parse(987654)  # not in the TPK
        with self.assertRaisesRegex(ValueError, "^Could not find exact version$"):
            parse(49, unity_version=b"2.0.0f1")  # before the TPK's first TextAsset
        self.assertIsNone(Tpk.TPKTYPETREE.ClassInformation[12][-1][1], "class 12 is gone from 2018.3")
        with self.assertRaisesRegex(ValueError, "^Could not find exact version$"):
            parse(12)
        with self.assertRaisesRegex(AssertionError, "has no ReleaseRootNode"):
            parse(18)  # EditorExtension, abstract: no release tree
        # TypeTreeError is unreachable: a node is always truthy, and the TPK returns one or raises.
        self.assertTrue(TypeTreeNode(m_Level=0, m_Type="X", m_Name="Base", m_ByteSize=0, m_Version=0))

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

    # Sections 9.1, 18: a null managed reference.
    def test_an_empty_managed_reference_does_not_resave(self):
        root = S.Node("MonoBehaviour", "Base", [S.Node("int", "x"), S.ref_object("ref")])
        foo = S.Node("Foo", "Base", [S.Node("float", "v")])
        empty = struct.pack("<iq", 7, 1) + S.s32("") * 3
        typed = struct.pack("<iq", 7, 2) + S.s32("Foo") + S.s32("") + S.s32("Asm") + struct.pack("<f", 1.5)
        refs = [S.Type(-1, foo, ref_names=["Foo", "", "Asm"])]
        sf = S.serialized_file([S.Type(114, root)], [S.Obj(1, 0, empty), S.Obj(2, 0, typed)], ref_types=refs)
        objects = load(sf)[0].objects
        value = objects[1].parse_as_dict()
        self.assertEqual(value["ref"], {"rid": 1, "type": {"class": "", "ns": "", "asm": ""}})
        with self.assertRaisesRegex(KeyError, "'data'"):
            objects[1].save_typetree(value)
        self.assertEqual(objects[2].save_typetree(objects[2].parse_as_dict()), objects[2].get_raw_data())

    # Sections 9.1: get_ref_type_node on an empty ref_types list.
    def test_ref_type_lookup_differs_on_an_empty_list(self):
        root = S.Node("MonoBehaviour", "Base", [S.Node("int", "x"), S.ref_object("ref")])
        empty = struct.pack("<iq", 7, 1) + S.s32("") * 3
        loaded = load(S.serialized_file([S.Type(114, root)], [S.Obj(1, 0, empty)], ref_types=[]))[0]
        self.assertEqual(loaded.ref_types, [])
        obj = loaded.objects[1]
        self.assertEqual(obj.parse_as_dict()["ref"], {"rid": 1, "type": {"class": "", "ns": "", "asm": ""}})
        reader = EndianBinaryReader(empty, endian="<")
        with self.assertRaisesRegex(ValueError, "^SerializedFile has no ref_types$"):
            TypeTreeHelper.read_typetree(
                obj._get_typetree_node(), reader, as_dict=True, byte_size=0, check_read=False, assetsfile=loaded
            )

    # Sections 5.2, 14.2, 17: a texture with no pixel data.
    def test_an_empty_texture_follows_an_empty_stream_path(self):
        env = UnityPy.Environment(path=str(self.tmp))
        sf = load(text_asset_file(), env=env)[0]

        def texture(stream, width=4):
            return SimpleNamespace(
                image_data=b"",
                m_StreamData=stream,
                object_reader=SimpleNamespace(assets_file=sf, version=(2021, 3, 2, 0), platform=0),
                m_Width=width,
                m_Height=width,
                m_TextureFormat=4,
                m_PlatformBlob=None,
                get_image_data=lambda: Texture2D.get_image_data(fake),
            )

        fake = texture(StreamingInfo(offset=0, size=0, path=""))
        self.assertTrue(fake.m_StreamData, "a StreamingInfo is truthy, even empty")
        # The empty path is looked up, misses, is loaded relative to env.path, and opens the directory.
        with self.assertRaises(IsADirectoryError):
            Texture2D.get_image_data(fake)
        with self.assertRaises(IsADirectoryError):
            ResourceReader.get_resource_data("", sf, 0, 0)
        # A 0x0 texture fetches its data first too, so it never reaches the empty-image shortcut.
        fake = texture(StreamingInfo(offset=0, size=0, path=""), width=0)
        with self.assertRaises(IsADirectoryError):
            Texture2DConverter.get_image_from_texture2d(fake)
        # Only a texture with no m_StreamData at all (pre-5.3 trees) gets the ValueError.
        fake = texture(None)
        with self.assertRaisesRegex(ValueError, "^No image data found$"):
            Texture2D.get_image_data(fake)
        image = Texture2DConverter.parse_image_data(b"", 0, 0, 4, (2021, 3, 2, 0), 0)
        self.assertEqual((image.mode, image.size), ("RGBA", (0, 0)), "parse_image_data's own shortcut")

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

    # Sections 14.1, 14.3: file ids and the preload pass.
    def test_negative_file_ids_index_from_the_end(self):
        sf = load(text_asset_file(), env=UnityPy.Environment(path=str(self.tmp)))[0]
        self.assertEqual(sf.externals, [])
        with self.assertRaises(IndexError):
            PPtr(m_FileID=-1, m_PathID=1, assetsfile=sf).deref()
        sf.externals = [SimpleNamespace(path="archive:/CAB-first/CAB-first"), SimpleNamespace(path="archive:/x/y")]
        with self.assertRaisesRegex(FileNotFoundError, "cab-first"):
            PPtr(m_FileID=-1, m_PathID=1, assetsfile=sf).deref()  # externals[-2]
        sf.externals = []

        def preload(pptr):
            info = SimpleNamespace(asset=SimpleNamespace(path_id=1), preloadIndex=0, preloadSize=1)
            helper = ContainerHelper(SimpleNamespace(m_Container=[("p", info)], m_PreloadTable=[pptr]))
            helper.parse_preload_table()
            return helper

        preload(PPtr(m_FileID=5, m_PathID=1, assetsfile=sf))  # FileNotFoundError: skipped
        preload(PPtr(m_FileID=0, m_PathID=99, assetsfile=sf))  # KeyError: skipped
        with self.assertRaises(IndexError):
            preload(PPtr(m_FileID=-1, m_PathID=1, assetsfile=sf))

    # Section 9: object-mode attribute names.
    def test_object_mode_names_use_the_boost_rule(self):
        root = S.Node("Base", "Base", [S.Node("int", "²ab"), S.Node("int", "2cd")])
        sf = load(S.serialized_file([S.Type(114, root)], [S.Obj(1, 0, struct.pack("<ii", 1, 2))]))[0]
        value = sf.objects[1].parse_as_object()
        self.assertEqual((getattr(value, "²ab"), value.x2cd), (1, 2))
        node = TypeTreeNode(m_Level=0, m_Type="int", m_Name="²ab", m_ByteSize=4, m_Version=1)
        self.assertEqual(node._clean_name, "²ab", "UnityPyBoost: ASCII digits only")
        self.assertEqual((node_clean_name("²ab"), TypeTreeHelper.clean_name("²ab")), ("x²ab", "x²ab"))

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

    # Section 19.2: `GameObject.m_Components` raises on an empty list and on the old pair form.
    def test_game_object_components_raise(self):
        game_object = GameObject.__new__(GameObject)
        pointer = PPtr(m_FileID=0, m_PathID=5)
        object.__setattr__(game_object, "m_Component", [])
        with self.assertRaises(IndexError):
            game_object.m_Components
        object.__setattr__(game_object, "m_Component", [(4, pointer)])
        with self.assertRaisesRegex(AttributeError, "m_GameObject"):
            game_object.m_Components
        object.__setattr__(game_object, "m_Component", None)
        self.assertEqual(game_object.m_Components, [])

    # Sections 9.3, 10: the TPK's version lookup and TypeTreeNode.from_list.
    def test_tpk_version_lookup_and_from_list(self):
        items = [(UnityVersion.from_str("5.0.0f1"), "a"), (UnityVersion.from_str("2020.1.0f1"), "b")]
        self.assertEqual(Tpk.get_item_for_version(UnityVersion.from_str("2021.3.2f1"), items), "b")
        with self.assertRaisesRegex(ValueError, "^Could not find exact version$"):
            Tpk.get_item_for_version(UnityVersion.from_str("4.0.0f1"), items)
        with self.assertRaisesRegex(ValueError, "^Could not find exact version$"):
            removed = [*items, (UnityVersion.from_str("2021.1.0f1"), None)]
            Tpk.get_item_for_version(UnityVersion.from_str("2021.3.2f1"), removed)
        rows = [{"m_Level": 0, "m_Type": "Base", "m_Name": "Base"}, {"m_Level": 1, "m_Type": "int", "m_Name": "a"}]
        node = TypeTreeNode.from_list(rows)
        self.assertEqual((node.m_ByteSize, node.m_Children[0].m_ByteSize), (0, 0))
        with self.assertRaises(TypeError):
            TypeTreeNode.from_list(
                [
                    {"m_Level": 0, "m_Type": "Base", "m_Name": "Base", "m_ByteSize": -1, "m_Version": 1},
                    {"m_Level": 1, "m_Type": "int", "m_Name": "a"},
                ]
            )


if __name__ == "__main__":
    unittest.main()
