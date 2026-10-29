"""PARITY.md rows on type trees, objects, versions and the TPK, run against the oracle.

Trees from the TPK and from the file, managed references, file ids, the
object mode's classes and names, version strings, the base classes and
records, and ObjectReader's attributes, each on the smallest input that shows
it, built here (`synth.py`). Each test names the PARITY.md rows it keeps
honest, and each row's test column names the test back (test_parity_rows.py
checks both). Needs UnityPy 1.25.2 (the oracle image); skipped elsewhere.
"""

import struct
import sys
import unittest
from abc import ABC
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

import synth as S  # noqa: E402
from parity_rows import ORACLE, TEXT_ASSET, RowsCase, load, text_asset_file  # noqa: E402

try:
    import attrs
    import UnityPy
    from UnityPy import classes
    from UnityPy.classes import GameObject, PPtr, UnknownObject
    from UnityPy.helpers import Tpk, TypeTreeHelper
    from UnityPy.helpers.ContainerHelper import ContainerHelper
    from UnityPy.helpers.TypeTreeNode import TypeTreeNode, clean_name as node_clean_name
    from UnityPy.helpers.UnityVersion import UnityVersion
    from UnityPy.streams import EndianBinaryReader
except ImportError:  # pragma: no cover - outside the oracle image (ORACLE is False)
    pass


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class ObjectRows(RowsCase):
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
        # PPtr: truthy when the path id is not 0; equality and hash on the two ids alone.
        a, b = PPtr(m_FileID=1, m_PathID=5), PPtr(m_FileID=1, m_PathID=5, assetsfile=object())
        self.assertEqual(
            (bool(a), bool(PPtr(m_FileID=1, m_PathID=0)), a == b, hash(a) == hash(b)), (True, False, True, True)
        )
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


if __name__ == "__main__":
    unittest.main()
