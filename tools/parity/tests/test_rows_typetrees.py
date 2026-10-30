"""PARITY.md rows on typetrees, objects and versions, run against the oracle: an object with no tree, type
dependencies, managed references, file ids and the preload pass, object mode, GameObject's components, the TPK,
version strings, the base classes and records, and ObjectReader's unused defaults.

Sections 7 to 11, 14, 18 and 19.2. Every input is built here (`synth.py`), so nothing needs the corpus, and each test
names the rows it pins; a row's test column names the test back. Its class is `Rows`, as in `test_parity_rows.py`,
the module these tests were split from. Needs UnityPy 1.25.2 (the oracle image); skipped elsewhere.
"""

import re
import struct
import sys
import tempfile
import unittest
from abc import ABC
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from rows_support import ORACLE, TEXT_ASSET, load, quietly, text_asset_file, zip_with_a_directory  # noqa: E402

import synth as S  # noqa: E402

if ORACLE:
    import attrs
    import UnityPy
    from UnityPy import classes
    from UnityPy.classes import GameObject, PPtr, UnknownObject
    from UnityPy.helpers import Tpk, TypeTreeHelper
    from UnityPy.helpers.ContainerHelper import ContainerHelper
    from UnityPy.helpers.TypeTreeNode import TypeTreeNode
    from UnityPy.helpers.TypeTreeNode import clean_name as node_clean_name
    from UnityPy.helpers.UnityVersion import UnityVersion
    from UnityPy.streams import EndianBinaryReader


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class Rows(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-rows-"))
        self.addCleanup(lambda: __import__("shutil").rmtree(self.tmp))

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
        with self.assertRaisesRegex(ValueError, "^SerializedFile has no ref_types$"):
            TypeTreeHelper.get_ref_type_node({"type": {"class": "X", "ns": "", "asm": ""}}, None)  # no file
        # Below format 20 a SerializedFile has no `ref_types` at all: the pure-Python path raises AttributeError
        # reading it, and Boost, which requires a list before it looks at the class, raises for a null reference too.
        for cls in ("", "X"):
            data = struct.pack("<iq", 7, 1) + S.s32(cls) + S.s32("") * 2
            old = load(S.serialized_file([S.Type(114, root)], [S.Obj(1, 0, data)], version=19))[0]
            with self.subTest(cls=cls):
                self.assertFalse(hasattr(old, "ref_types"))
                with self.assertRaisesRegex(ValueError, "^Failed to get ref type node$"):
                    old.objects[1].parse_as_dict()
                reader = EndianBinaryReader(data, endian="<")
                with self.assertRaisesRegex(AttributeError, "^'SerializedFile' object has no attribute 'ref_types'$"):
                    TypeTreeHelper.read_typetree(
                        obj._get_typetree_node(), reader, as_dict=True, byte_size=0, check_read=False, assetsfile=old
                    )

    # Sections 14.1, 14.3: file ids and the preload pass.
    def test_negative_file_ids_index_from_the_end(self):
        sf = load(text_asset_file(), env=UnityPy.Environment(path=str(self.tmp)))[0]
        self.assertEqual(sf.externals, [])
        with self.assertRaises(IndexError):
            PPtr(m_FileID=-1, m_PathID=1, assetsfile=sf).deref()
        sf.externals = [SimpleNamespace(path="archive:/CAB-first/CAB-first"), SimpleNamespace(path="archive:/x/y")]
        missed = f" not found in {re.escape(str(self.tmp))}$"
        with self.assertRaisesRegex(FileNotFoundError, "^File cab-first" + missed):
            PPtr(m_FileID=-1, m_PathID=1, assetsfile=sf).deref()  # externals[-2]
        # A record nothing answers: find_file raises its own error, naming the last component lower-cased and
        # Environment.path, never deref's "Failed to resolve pointer" (which needs find_file to return None).
        for record in ("archive:/CAB-Miss/CAB-Miss", "assets/Missing.Assets", "Sub/Missing.assets"):
            sf.externals = [SimpleNamespace(path=record)]
            component = re.escape(record.rsplit("/")[-1].lower())
            with self.subTest(record), self.assertRaisesRegex(FileNotFoundError, f"^File {component}{missed}"):
                PPtr(m_FileID=1, m_PathID=1, assetsfile=sf).deref()
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
        # deref: no assetsfile raises ValueError before the path id is looked at; path id 0 raises ValueError; file
        # id 0 is the pointer's own file; a missing path id is KeyError.
        with self.assertRaisesRegex(ValueError, "^PPtr can't deref without an assetsfile!$"):
            PPtr(m_FileID=0, m_PathID=0).deref()
        with self.assertRaisesRegex(ValueError, "^PPtr can't deref with m_PathID == 0!$"):
            PPtr(m_FileID=0, m_PathID=0, assetsfile=sf).deref()
        # A file with no parent raises for an external pointer, naming the record.
        orphan = load(text_asset_file(), env=UnityPy.Environment(path=str(self.tmp)))[0]
        orphan.externals, orphan.parent = [SimpleNamespace(path="archive:/CAB-x/CAB-x")], None
        with self.assertRaisesRegex(
            FileNotFoundError, "^PPtr points to archive:/CAB-x/CAB-x but no container is set!$"
        ):
            PPtr(m_FileID=1, m_PathID=1, assetsfile=orphan).deref()
        self.assertIs(PPtr(m_FileID=0, m_PathID=1, assetsfile=sf).deref(), sf.objects[1])
        with self.assertRaisesRegex(KeyError, "^99$"):
            PPtr(m_FileID=0, m_PathID=99, assetsfile=sf).deref()

        # parse_preload_table: an entry whose range starts below 0, has a size of 0 or less, or runs past the table
        # is skipped, and so is a null PPtr; a target's path is set only where its path_dict has none; the table is
        # read once.
        def table(entries, pptrs):
            rows = [
                (p, SimpleNamespace(asset=SimpleNamespace(path_id=9), preloadIndex=i, preloadSize=n))
                for p, i, n in entries
            ]
            return ContainerHelper(SimpleNamespace(m_Container=rows, m_PreloadTable=pptrs))

        own = PPtr(m_FileID=0, m_PathID=1, assetsfile=sf)
        paths = sf._container.path_dict
        paths.pop(1, None)
        helper = table(
            [("neg", -1, 1), ("zero", 0, 0), ("past", 1, 2), ("null", 1, 1), ("ok", 0, 1), ("later", 0, 1)],
            [own, PPtr(m_FileID=0, m_PathID=0, assetsfile=sf)],
        )
        helper.parse_preload_table()
        self.assertEqual((paths.get(1), helper._preload_table), ("ok", None))
        paths[1] = "kept"
        table([("x", 0, 1)], [own]).parse_preload_table()
        self.assertEqual(paths.pop(1), "kept")
        # Any other failure escapes: a target that is a raw reader (AttributeError), a loader OSError.
        load(b"\x01" * 30, name="raw.bin", env=sf.environment)
        (self.tmp / "dir.assets").mkdir()
        for external, error in (("raw.bin", AttributeError), ("dir.assets", IsADirectoryError)):
            sf.externals = [SimpleNamespace(path=external)]
            with self.subTest(external), self.assertRaises(error):
                table([("x", 0, 1)], [PPtr(m_FileID=1, m_PathID=1, assetsfile=sf)]).parse_preload_table()
        sf.externals = []
        # deref's upper guard: a file id past the externals raises FileNotFoundError, before the parent is looked at.
        sf.externals = [SimpleNamespace(path="a"), SimpleNamespace(path="b")]
        for file_id, file in ((3, sf), (2, orphan)):
            with (
                self.subTest(file_id=file_id),
                self.assertRaisesRegex(FileNotFoundError, "^Failed to resolve pointer - invalid m_FileID!$"),
            ):
                PPtr(m_FileID=file_id, m_PathID=1, assetsfile=file).deref()
        # What the loop or find_file finds is used whatever it holds: a None (a zip's own key) raises
        # FileNotFoundError naming the file id; a raw reader (by key, or a registry hit on a .resS or on CAB ""), a
        # BundleFile or a WebFile has no `objects` and raises AttributeError.
        env = sf.environment
        load(b"\x02" * 30, name="dir/CAB-r.resS", env=env)
        load(S.fs_bundle([(b"CAB-b", text_asset_file(), 4)]), name="b.bundle", env=env)
        load(S.web_file([(b"w.txt", b"web")]), name="w.data", env=env)
        quietly(env.load_file, zip_with_a_directory(), name="z.zip")
        self.assertEqual(env.files["z.zip"], None)
        for record, error, message in (
            ("archive:/CAB-x/RAW.bin", AttributeError, "'EndianBinaryReader_Memoryview' object has no attribute"),
            ("archive:/CAB-r/CAB-r.resS", AttributeError, "'EndianBinaryReader_Memoryview' object has no attribute"),
            ("assets/", AttributeError, "'EndianBinaryReader_Streamable' object has no attribute 'objects'$"),
            ("b.bundle", AttributeError, "^'BundleFile' object has no attribute 'objects'$"),
            ("w.data", AttributeError, "^'WebFile' object has no attribute 'objects'$"),
            ("z.zip", FileNotFoundError, "^Failed to resolve pointer - 1 not found!$"),
        ):
            sf.externals = [SimpleNamespace(path=record)]
            with self.subTest(record), self.assertRaisesRegex(error, message):
                quietly(PPtr(m_FileID=1, m_PathID=1, assetsfile=sf).deref)
        sf.externals = []

    # Section 9: object-mode attribute names.
    def test_object_mode_names_use_the_boost_rule(self):
        root = S.Node("Base", "Base", [S.Node("int", "²ab"), S.Node("int", "2cd")])
        sf = load(S.serialized_file([S.Type(114, root)], [S.Obj(1, 0, struct.pack("<ii", 1, 2))]))[0]
        value = sf.objects[1].parse_as_object()
        self.assertEqual((getattr(value, "²ab"), value.x2cd), (1, 2))
        node = TypeTreeNode(m_Level=0, m_Type="int", m_Name="²ab", m_ByteSize=4, m_Version=1)
        self.assertEqual(node._clean_name, "²ab", "UnityPyBoost: ASCII digits only")
        self.assertEqual((node_clean_name("²ab"), TypeTreeHelper.clean_name("²ab")), ("x²ab", "x²ab"))

    # Sections 9.2, 14.1, 19.2: how a class node becomes an object, on UnityPyBoost and on the pure-Python path.
    def test_object_mode_builds_classes_as_parse_class_does(self):
        def tree(root_type, fields):
            nodes = [{"m_Level": 0, "m_Type": root_type, "m_Name": "Base", "m_ByteSize": -1, "m_Version": 1}]
            nodes += [
                {"m_Level": 1, "m_Type": "int", "m_Name": name, "m_ByteSize": 4, "m_Version": 1} for name in fields
            ]
            return TypeTreeNode.from_list(nodes)

        def parse(root_type, fields, boost):
            data = struct.pack(f"<{len(fields)}i", *range(1, len(fields) + 1))
            size = len(data) if boost else None  # no byte_size: the pure-Python path
            reader = EndianBinaryReader(data, "<")
            return TypeTreeHelper.read_typetree(tree(root_type, fields), reader, False, size, check_read=boost)

        # One class of each kind section 9.2.1 lists: a non-empty `__slots__` and no __dict__; a non-empty
        # `__slots__` and the __dict__ Object gives; and `()` (found through the MRO) with a __dict__, which holds
        # both the classes attrs builds with slots=False and those it builds with slots=True and no fields on
        # Object (`GameManager`). test_object_mode.py checks the rule over every class.
        self.assertEqual(classes.AssetDatabaseMetrics.__bases__, (object,))
        self.assertTrue(classes.AssetDatabaseMetrics.__slots__)
        self.assertEqual(classes.MarshallingTestObject.__mro__[1:3], (classes.Object, ABC))
        self.assertTrue(classes.MarshallingTestObject.__slots__)
        for slotted in (classes.EditorSettings, classes.StreamingInfo, classes.Vector3f):
            self.assertTrue(slotted.__slots__, slotted)
        for unslotted in (classes.MultiplayerManager, classes.TextAsset, classes.Texture2D, classes.GameManager):
            self.assertEqual(unslotted.__slots__, (), unslotted)
        self.assertEqual((classes.GameManager.__bases__[0], attrs.fields(classes.GameManager)), (classes.Object, ()))

        metrics = ["totalAssetCount", "nonProAssetCount", "nonProAssetsCreatedAfterProLicense"]  # 1 required
        roles = ["m_ActiveMultiplayerRole", "m_ActiveMultiplayerRoles"]  # both optional
        self.assertEqual([f.default is attrs.NOTHING for f in attrs.fields(classes.AssetDatabaseMetrics)], [1, 0, 0])
        cases = {
            # (class, fields): (UnityPyBoost's result, the pure-Python path's)
            ("AssetDatabaseMetrics", (*metrics,)): ("AssetDatabaseMetrics", "AssetDatabaseMetrics"),
            # A missing optional field takes its default on both paths, a missing required one gives UnknownObject.
            ("AssetDatabaseMetrics", (metrics[0],)): ("AssetDatabaseMetrics", "AssetDatabaseMetrics"),
            ("AssetDatabaseMetrics", (*metrics[1:],)): ("UnknownObject", "UnknownObject"),
            # An extra field: Boost gives up where `__slots__` is a non-empty tuple; Python's setattr fails without a
            # __dict__.
            ("AssetDatabaseMetrics", (*metrics, "m_Extra")): ("UnknownObject", AttributeError),
            ("MarshallingTestObject", ("m_Prop", "m_Extra")): ("UnknownObject", "MarshallingTestObject"),
            # Where `__slots__` is `()`, Boost keeps the extra as an attribute; Python wants every annotation present.
            ("MultiplayerManager", (*roles, "m_Extra")): ("MultiplayerManager", "MultiplayerManager"),
            ("MultiplayerManager", ("m_Extra",)): ("MultiplayerManager", "UnknownObject"),
            # A field-less class attrs built with slots=True on Object has `()` too, so it keeps the extra.
            ("GameManager", ("m_Extra",)): ("GameManager", "GameManager"),
            ("AssetDatabaseMetrics", (metrics[0], "m_Extra")): ("UnknownObject", "UnknownObject"),
            # A type the module lacks.
            ("NoSuchClass", ("a",)): ("UnknownObject", "UnknownObject"),
        }
        for (root, fields), results in cases.items():
            for boost, expected in zip((True, False), results, strict=True):
                with self.subTest(root, fields=fields, boost=boost):
                    if expected is AttributeError:
                        with self.assertRaisesRegex(AttributeError, "object has no attribute 'm_Extra'"):
                            parse(root, fields, boost)
                        continue
                    value = parse(root, fields, boost)
                    self.assertEqual(type(value).__name__, expected)
                    self.assertEqual(
                        {name: getattr(value, name) for name in fields},
                        dict(zip(fields, range(1, len(fields) + 1), strict=True)),
                    )
                    if expected == "UnknownObject":
                        # Boost always keeps the node; the pure-Python path drops it for a type the module lacks.
                        kept = boost or root != "NoSuchClass"
                        self.assertEqual(value.get_type(), root if kept else None)
        # ObjectReader passes byte_size, so every object-mode parse of a loaded file goes through Boost.
        root = S.Node("AssetDatabaseMetrics", "Base", [S.Node("int", name) for name in [*metrics, "m_Extra"]])
        sf = load(S.serialized_file([S.Type(114, root)], [S.Obj(1, 0, struct.pack("<4i", 1, 2, 3, 4))]))[0]
        parsed = sf.objects[1].parse_as_object()
        self.assertEqual((type(parsed), parsed.get_type(), parsed.m_Extra), (UnknownObject, "AssetDatabaseMetrics", 4))

    # Section 19.2: `GameObject.m_Components` raises on an empty list and on the old pair form.
    def test_game_object_components_raise(self):
        game_object = GameObject.__new__(GameObject)
        pointer = PPtr(m_FileID=0, m_PathID=5)
        object.__setattr__(game_object, "m_Component", [])
        with self.assertRaises(IndexError):
            _ = game_object.m_Components
        object.__setattr__(game_object, "m_Component", [(4, pointer)])
        with self.assertRaisesRegex(AttributeError, "m_GameObject"):
            _ = game_object.m_Components
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

    # Sections 10, 11: the fallback version, the version exceptions and UnityVersion's string form.
    def test_fallback_version_and_version_strings(self):
        from UnityPy import config
        from UnityPy.exceptions import UnityVersionFallbackError, UnityVersionFallbackWarning

        self.assertTrue(issubclass(UnityVersionFallbackError, Exception))
        self.assertTrue(issubclass(UnityVersionFallbackWarning, UserWarning))
        for value in (None, 2019):
            with (
                self.subTest(value=value),
                mock.patch.object(config, "FALLBACK_UNITY_VERSION", value),
                self.assertRaises(UnityVersionFallbackError),
            ):
                config.get_fallback_version()
        # The `once` filter UnityPy installs at import: one warning for three calls from three places, in a process
        # of its own because a test runner replaces the warning filters.
        code = (
            "import UnityPy.config as c\n"
            "c.FALLBACK_UNITY_VERSION = '2019.4.1f1'\n"
            "a = c.get_fallback_version()\n"
            "b = c.get_fallback_version()\n"
            "d = c.get_fallback_version()\n"
            "print(a, b, d)\n"
        )
        run = __import__("subprocess").run([sys.executable, "-B", "-c", code], capture_output=True, text=True)
        self.assertEqual(run.stdout, "2019.4.1f1 2019.4.1f1 2019.4.1f1\n")
        self.assertEqual(run.stderr.count("UnityVersionFallbackWarning"), 1, run.stderr)
        # str(): above 5.x the build and its dot go; 5.x and below the type and its number; the postfix stays.
        cases = {
            "2019.4.15f1": ("2019.4f1", ""),
            "2018.4.36": ("2018.4f0", ""),
            "5.6.7f1": ("5.6.7", ""),
            "2019.4.15f1-custom": ("2019.4f1-custom", "-custom"),
            "2022.3.1f1c1": ("2022.3f1c1", "c1"),
        }
        for text, (printed, postfix) in cases.items():
            with self.subTest(text):
                version = UnityVersion.from_str(text)
                self.assertEqual((str(version), version.postfix), (printed, postfix))

    # Sections 14.1, 14.3: the base classes, the math records and PPtr.
    def test_base_classes_and_records(self):
        import copy

        from UnityPy.classes import (
            ColorRGBA,
            Matrix3x4f,
            Matrix4x4f,
            Object,
            Quaternionf,
            TextAsset,
            Vector2f,
            Vector3f,
            Vector4f,
            float3,
            float4,
        )

        self.assertEqual(
            (Vector2f(), Vector3f(), Vector4f()), (Vector2f(0, 0), Vector3f(0, 0, 0), Vector4f(0, 0, 0, 0))
        )
        self.assertEqual((float3, float4), (Vector3f, Vector4f))
        self.assertTrue(issubclass(Quaternionf, Vector4f))
        self.assertEqual([f.name for f in attrs.fields(Matrix3x4f)], [f"e{r}{c}" for r in range(3) for c in range(4)])
        self.assertEqual([f.name for f in attrs.fields(Matrix4x4f)], [f"e{r}{c}" for r in range(4) for c in range(4)])
        self.assertEqual(ColorRGBA(), ColorRGBA(0, 0, 0, 1))
        self.assertEqual(ColorRGBA(rgba=0x80FF0040), ColorRGBA(128 / 255, 1.0, 0.0, 64 / 255))
        self.assertEqual(ColorRGBA(rgba=0x80FF0040).rgba, 0x80FF0040)
        # PPtr: truthy when the path id is not 0; equality and hash on the two ids alone, the hash that of the pair.
        a, b = PPtr(m_FileID=1, m_PathID=5), PPtr(m_FileID=1, m_PathID=5, assetsfile=object())
        self.assertEqual(
            (bool(a), bool(PPtr(m_FileID=1, m_PathID=0)), a == b, hash(a) == hash(b)), (True, False, True, True)
        )
        self.assertEqual(hash(a), hash((1, 5)))
        self.assertNotEqual(a, PPtr(m_FileID=2, m_PathID=5))
        self.assertEqual((a.file_id, a.path_id), (1, 5))
        # Object: keyword fields, no reader, save raises, copy.copy rebuilds; UnknownObject.get_type.
        text = TextAsset(m_Name="n", m_Script="s")
        self.assertIsNone((text.object_reader, text.assets_file)[1])
        with self.assertRaisesRegex(ValueError, "^ObjectReader not set$"):
            text.save()
        text.set_object_reader("reader")
        self.assertEqual(text.object_reader, "reader")
        extra = TextAsset(m_Name="n", m_Script="s")
        extra.m_Extra = 3
        copied = copy.copy(extra)
        self.assertEqual((type(copied), copied.m_Name, copied.m_Extra), (TextAsset, "n", 3))
        self.assertIsNone(UnknownObject(a=1).get_type())
        node = TypeTreeNode(m_Level=0, m_Type="Thing", m_Name="Base", m_ByteSize=0, m_Version=0)
        self.assertEqual(UnknownObject(node, a=1).get_type(), "Thing")
        # repr and str: Object's `<{class name}>`, which a generated class's attrs repr replaces.
        fresh = TextAsset(m_Name="n", m_Script="s")
        self.assertEqual(
            (Object.__repr__(fresh), repr(fresh)),
            ("<TextAsset>", "TextAsset(m_Name='n', m_Script='s', m_PathName=None)"),
        )
        # UnknownObject: its attributes but `__node__`, each repr longer than 100 cut to 97 characters and `...`.
        self.assertEqual(repr(UnknownObject(a=1)), "<UnknownObject<None> a=1>")
        long, edge = "x" * 120, "y" * 98
        self.assertEqual(
            repr(UnknownObject(node, a=1, b=long, c=edge)),
            f"<UnknownObject<Thing> a=1, b={repr(long)[:97]}..., c={edge!r}>",
        )
        self.assertEqual(
            [
                repr(Vector2f(1, 2.5)),
                repr(Vector3f(1, 2, 3)),
                repr(Vector4f(1, 2, 3, 4)),
                repr(Quaternionf(1, 2, 3, 4)),
            ],
            ["Vector2f(1, 2.5)", "Vector3f(1, 2, 3)", "Vector4f(1, 2, 3, 4)", "Quaternion(1, 2, 3, 4)"],
        )
        sf_bytes = S.serialized_file(
            [S.Type(49, TEXT_ASSET)], [S.Obj(1, 0, S.s32("n") + S.s32("s"))], externals=[S.External("archive:/C/d")]
        )
        sf, bundle = load(sf_bytes)[0], load(S.fs_bundle([(b"CAB-a", sf_bytes, 4)]), "b")[0]
        self.assertEqual(
            (repr(sf), repr(bundle), repr(bundle.files["CAB-a"])),
            ("<SerializedFile>", "<BundleFile>", "<SerializedFile>"),
        )
        self.assertEqual(
            (repr(sf.externals[0]), repr(sf.objects[1])), ("<FileIdentifier(archive:/C/d)>", "<ObjectReader TextAsset>")
        )
        self.assertEqual(
            (repr(UnityVersion.from_str("2019.4.15f1")), repr(UnityVersion.from_str("5.6.7f1"))),
            ("UnityVersion 2019.4f1", "UnityVersion 5.6.7"),
        )
        # TypeTreeNode, with UnityPyBoost: the C type's repr, which the subclass does not override; four fields, each
        # value's repr; its str the same.
        from UnityPy import UnityPyBoost

        self.assertTrue(issubclass(TypeTreeNode, UnityPyBoost.TypeTreeNode))
        self.assertNotIn("__repr__", vars(TypeTreeNode))
        flagged = TypeTreeNode(
            m_Level=1, m_Type="string", m_Name='it\'s "q"', m_ByteSize=-1, m_Version=1, m_MetaFlag=16384
        )
        self.assertEqual(
            [repr(node), str(node), repr(flagged)],
            [
                "TypeTreeNode(m_Level=0, m_Type='Thing', m_Name='Base', m_MetaFlag=None)",
                "TypeTreeNode(m_Level=0, m_Type='Thing', m_Name='Base', m_MetaFlag=None)",
                "TypeTreeNode(m_Level=1, m_Type='string', m_Name='it\\'s \"q\"', m_MetaFlag=16384)",
            ],
        )
        # ContainerHelper's str: `{path: pointer, …}` over items(), the path bare and the pointer as its str.
        pointers = [PPtr(m_FileID=0, m_PathID=7), PPtr(m_FileID=1, m_PathID=2)]
        container = ContainerHelper(
            [(p, SimpleNamespace(asset=a)) for p, a in zip(("a/path", "b"), pointers, strict=True)]
        )
        self.assertEqual(
            str(container),
            "{a/path: PPtr(m_FileID=0, m_PathID=7, assetsfile=None), b: PPtr(m_FileID=1, m_PathID=2, assetsfile=None)}",
        )

    # Section 11: the type character.
    def test_version_type_str_is_lower_case_for_known_types(self):
        upper = UnityVersion.from_str("2019.4.15F1")
        self.assertEqual((upper.type_str, str(upper)), ("f", "2019.4f1"))
        unknown = UnityVersion.from_str("2019.4.15q1")
        self.assertEqual((int(unknown.type), unknown.type_str, str(unknown)), (255, "q", "2019.4q1"))

    # Sections 8, 14.3: defaults that are never used.
    def test_object_reader_get_and_container_attributes(self):
        sf = load(text_asset_file())[0]
        reader = sf.objects[1]
        self.assertIsNone(reader.get("nope", 5))
        self.assertIsNone(reader.nope)
        info = SimpleNamespace(asset=SimpleNamespace(path_id=1))
        container = ContainerHelper([("a/path", info)])
        self.assertIs(container.__getattr__("a/path"), info.asset)
        with self.assertRaisesRegex(KeyError, "'x'"):
            hasattr(container, "x")
        with self.assertRaisesRegex(KeyError, "'x'"):
            getattr(container, "x", None)
        # The mapping protocol: `[]` and `in` on the last entry of a path, `len` counting repeats, iteration over
        # each path once; assignment and deletion raise.
        first, last = (SimpleNamespace(asset=SimpleNamespace(path_id=i)) for i in (2, 3))
        repeated = ContainerHelper([("a/path", info), ("b", first), ("b", last)])
        self.assertEqual(
            (len(repeated), sorted(repeated), "b" in repeated, "c" in repeated), (3, ["a/path", "b"], True, False)
        )
        self.assertIs(repeated["b"], last.asset)
        # items(): every (path, PPtr) in container order, repeats kept.
        self.assertEqual(list(repeated.items()), [("a/path", info.asset), ("b", first.asset), ("b", last.asset)])
        with self.assertRaisesRegex(NotImplementedError, "^Assigning to container is not allowed!$"):
            repeated["c"] = info.asset
        with self.assertRaisesRegex(NotImplementedError, "^Deleting from the container is not allowed!$"):
            del repeated["b"]


if __name__ == "__main__":
    unittest.main()
