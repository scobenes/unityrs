"""PARITY.md rows on objects, run against the oracle: ObjectReader, TypeTreeNode, the TPK and the classes.

Sections 8 to 11 and 14. Like `test_parity_rows.py`, every input is built here, so nothing needs the
corpus, and each test names the rows it pins; a row's test column names the test back. An object of a
class with no embedded tree is written from the TPK's node for the file's version by UnityPy's own
typetree writer (`tpk_object`), so its bytes are what the reader expects. Needs UnityPy 1.25.2 (the
oracle image); skipped elsewhere.
"""

import struct
import sys
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from test_parity_rows import ORACLE, TEXT_ASSET, load, quietly, text_asset_file  # noqa: E402

import synth as S  # noqa: E402

if ORACLE:
    import UnityPy
    from UnityPy.classes import MonoBehaviour, TextAsset
    from UnityPy.enums import BuildTarget, ClassIDType
    from UnityPy.helpers import Tpk, TypeTreeHelper
    from UnityPy.helpers.TypeTreeNode import TypeTreeNode
    from UnityPy.helpers.UnityVersion import UnityVersion
    from UnityPy.streams import EndianBinaryReader, EndianBinaryWriter

VERSION = "2021.3.2f1"  # synth.serialized_file's default engine version


def default_value(node):
    """A value of `node`'s shape as the dict-mode typetree writer takes it: zeros, empty strings and lists."""
    if node.m_Type in TypeTreeHelper.FUNCTION_WRITE_MAP:
        return {"string": "", "TypelessData": b"", "float": 0.0, "double": 0.0, "bool": False}.get(node.m_Type, 0)
    if node.m_Type == "pair":
        return tuple(default_value(child) for child in node.m_Children)
    if node.m_Children and node.m_Children[0].m_Type == "Array":
        return []
    return {child.m_Name: default_value(child) for child in node.m_Children}


def tpk_object(class_id, version=VERSION, endian="<", **fields):
    """The bytes of an object of `class_id` as the TPK's node for `version` lays it out, `fields` set."""
    node = Tpk.get_typetree_node(class_id, UnityVersion.from_str(version))
    value = default_value(node)
    unknown = set(fields) - set(value)
    assert not unknown, f"{unknown} are not fields of class {class_id}"
    value.update(fields)
    writer = EndianBinaryWriter(endian=endian)
    TypeTreeHelper.write_typetree(value, node, writer)
    return writer.bytes


def tpk_file(objects, **kw):
    """A format-22 file with no embedded trees, holding (path_id, class_id, data) objects."""
    class_ids = sorted({class_id for _, class_id, _ in objects})
    return S.serialized_file(
        [S.Type(c) for c in class_ids],
        [S.Obj(path_id, class_ids.index(c), data) for path_id, c, data in objects],
        enable_type_tree=False,
        **kw,
    )


def pptr(path_id, file_id=0):
    return {"m_FileID": file_id, "m_PathID": path_id}


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class ObjectRows(unittest.TestCase):
    # Section 8: ObjectReader's accessors, the object-mode readers, `nodes=`, the TPK fallback and the writers.
    def test_object_reader_accessors(self):
        sf = load(text_asset_file("nm", "x"))[0]
        obj = sf.objects[1]
        # version, version2 and platform are the file's.
        self.assertEqual(
            (obj.version, obj.version2, obj.platform), (UnityVersion.from_str(VERSION), 22, BuildTarget(19))
        )
        # read and parse_as_object are read_typetree(wrap=True): the generated class.
        for got in (obj.read(), obj.parse_as_object(), obj.read_typetree(wrap=True)):
            self.assertIsInstance(got, TextAsset)
            self.assertEqual((got.m_Name, got.m_Script), ("nm", "x"))
        self.assertIs(obj.get_class(), TextAsset)
        # Unknown attributes are the reader's, else None.
        self.assertEqual((obj.endian, obj.no_such_attribute), (obj.reader.endian, None))

        # nodes=: a TypeTreeNode, or a list (of dicts, or of TypeTreeNodes) linked by TypeTreeNode.from_list;
        # anything else raises ValueError; an empty list raises from_list's IndexError.
        tree = obj._get_typetree_node()
        self.assertEqual(obj.parse_as_dict(tree), {"m_Name": "nm", "m_Script": "x"})
        rows = [{"m_Level": n.m_Level, "m_Type": n.m_Type, "m_Name": n.m_Name} for n in tree.traverse()]
        self.assertEqual(obj.parse_as_dict(rows), {"m_Name": "nm", "m_Script": "x"})
        fresh = [TypeTreeNode(n.m_Level, n.m_Type, n.m_Name, n.m_ByteSize, n.m_Version) for n in tree.traverse()]
        self.assertEqual(obj.parse_as_dict(fresh), {"m_Name": "nm", "m_Script": "x"})
        for bad in ((), {}, 0, tuple(rows)):
            with self.subTest(nodes=bad), self.assertRaisesRegex(ValueError, "nodes must be a list"):
                obj.parse_as_dict(bad)
        with self.assertRaises(IndexError):
            obj.parse_as_dict([])
        # from_list links the nodes it is given in place, so a tree's own nodes gain their children twice.
        own = load(text_asset_file())[0].objects[1]._get_typetree_node()
        with self.assertRaises(EOFError):
            quietly(obj.parse_as_dict, list(own.traverse()))
        self.assertEqual(len(own.m_Children), 4)

        # The TPK fallback: no tree in the file, so Tpk.get_typetree_node(class_id, version).
        stripped = load(S.serialized_file([S.Type(49)], [S.Obj(1, 0, S.s32("a") + S.s32("b"))], enable_type_tree=False))
        no_tree = stripped[0].objects[1]
        real = Tpk.get_typetree_node
        with mock.patch.object(sys.modules["UnityPy.files.ObjectReader"], "get_typetree_node", wraps=real) as spy:
            self.assertEqual(no_tree.parse_as_dict(), {"m_Name": "a", "m_Script": "b"})
        spy.assert_called_once_with(49, UnityVersion.from_str(VERSION))

        # patch is save_typetree: the bytes written, set as the object's data, the file and its parents changed.
        bundle = load(S.fs_bundle([(b"CAB-a", text_asset_file("nm", "x"), 4)]))[0]
        inner = bundle.files["CAB-a"]
        target = inner.objects[1]
        value = {"m_Name": "patched", "m_Script": "y"}
        expected = S.s32("patched") + S.s32("y")
        self.assertFalse(bundle.is_changed)
        self.assertEqual(target.patch(value), expected)
        self.assertEqual((target.data, inner.is_changed, bundle.is_changed), (expected, True, True))
        self.assertEqual(load(text_asset_file())[0].objects[1].save_typetree(value), expected)
        # set_raw_data replaces the data, and write takes it, or else the original bytes: on save and reload
        # the patched object reads its new value, an untouched one its old.
        sf = load(
            S.serialized_file([S.Type(49, TEXT_ASSET)], [S.Obj(n, 0, S.s32(f"o{n}") + S.s32("s")) for n in (1, 2)])
        )[0]
        sf.objects[2].set_raw_data(S.s32("new") + S.s32("data!"))
        self.assertTrue(sf.is_changed)
        back = load(sf.save())[0]
        self.assertEqual(
            [o.parse_as_dict() for o in back.objects.values()],
            [{"m_Name": "o1", "m_Script": "s"}, {"m_Name": "new", "m_Script": "data!"}],
        )
        self.assertEqual(back.objects[2].byte_size, len(S.s32("new") + S.s32("data!")))

    # Sections 8, 14.1: get_class and the class map.
    def test_class_map(self):
        from UnityPy.classes.ClassIDTypeToClassMap import ClassIDTypeToClassMap

        self.assertEqual((len(ClassIDType), len(ClassIDTypeToClassMap)), (368, 358))
        unmapped = sorted(c.value for c in ClassIDType if c not in ClassIDTypeToClassMap)
        self.assertEqual(unmapped, [-1, 1000, 1121, 1122, 1123, 2000, 100000, 100001, 100002, 100011])
        self.assertEqual(ClassIDTypeToClassMap[ClassIDType.Texture2D].__name__, "Texture2D")
        # get_class: the map's class, None for an id it lacks (an unknown id is still a ClassIDType).
        sf = load(S.serialized_file([S.Type(1121, TEXT_ASSET)], [S.Obj(1, 0, S.s32("a") + S.s32("b"))]))[0]
        self.assertIsNone(sf.objects[1].get_class())
        unknown = load(S.serialized_file([S.Type(987654, TEXT_ASSET)], [S.Obj(1, 0, S.s32("a") + S.s32("b"))]))[0]
        self.assertEqual((unknown.objects[1].type, unknown.objects[1].get_class()), (987654, None))

    # Section 8: a MonoBehaviour with no tree, and Environment.typetree_generator.
    def test_monobehaviour_node_generator(self):
        script = tpk_object(115, m_Name="S", m_ClassName="Cls", m_Namespace="ns", m_AssemblyName="Asm.dll")
        plain_script = tpk_object(115, m_Name="S", m_ClassName="Cls", m_Namespace="", m_AssemblyName="Asm.dll")
        head = {"m_GameObject": pptr(0), "m_Enabled": 1, "m_Name": "mb"}

        def mono(script_ptr, extra=b"", script_data=script):
            data = tpk_object(114, m_GameObject=pptr(0), m_Enabled=1, m_Script=script_ptr, m_Name="mb") + extra
            return tpk_file([(1, 114, data), (5, 115, script_data)])

        class Generator:
            def __init__(self, node=None, error=None):
                self.node, self.error, self.calls = node, error, []

            def get_nodes_up(self, assembly, fullname):
                self.calls.append((assembly, fullname))
                if self.error:
                    raise self.error
                return self.node

        def parse(data, generator):
            env = UnityPy.Environment()
            env.typetree_generator = generator
            return load(data, env=env)[0].objects[1].parse_as_dict()

        tpk_head = dict(head, m_Script=pptr(5))
        # Without a generator, or with a null m_Script, or a generator that returns no node, generate raises
        # ValueError, which is swallowed: the object parses with the TPK's MonoBehaviour tree.
        self.assertEqual(parse(mono(pptr(5)), None), tpk_head)
        self.assertEqual(parse(mono(pptr(0)), Generator()), dict(head, m_Script=pptr(0)))
        none = Generator()
        self.assertEqual(parse(mono(pptr(5)), none), tpk_head)
        self.assertEqual(none.calls, [("Asm.dll", "ns.Cls")], "{namespace}.{class}")
        plain = Generator()
        parse(mono(pptr(5), script_data=plain_script), plain)
        self.assertEqual(plain.calls, [("Asm.dll", "Cls")], "the class alone with no namespace")
        # A ValueError from any step is swallowed, subclasses included: the generator's own, and the head
        # parse's, which the generator is never asked after (the TPK head then fails the main parse again).
        for error in (ValueError("generator broke"), UnicodeDecodeError("utf-8", b"\x80", 0, 1, "invalid")):
            with self.subTest(type(error).__name__):
                self.assertEqual(parse(mono(pptr(5)), Generator(error=error)), tpk_head)
        negative = tpk_object(114, m_GameObject=pptr(0), m_Enabled=1, m_Script=pptr(5), m_Name="mb")
        negative = negative[:-8] + struct.pack("<i", -1) + negative[-4:]  # m_Name's length
        env, unasked = UnityPy.Environment(), Generator()
        env.typetree_generator = unasked
        obj = load(tpk_file([(1, 114, negative), (5, 115, script)]), env=env)[0].objects[1]
        self.assertEqual(obj._get_typetree_node().m_Type, "MonoBehaviour")
        self.assertEqual(unasked.calls, [])
        with self.assertRaisesRegex(ValueError, "Negative length"):
            quietly(obj.parse_as_dict)
        # Every other failure escapes: a deref that finds no object or no external, a head parse that runs out
        # of data, or the generator's own.
        with self.assertRaisesRegex(KeyError, "^6$"):
            parse(mono(pptr(6)), Generator())
        with self.assertRaisesRegex(FileNotFoundError, "invalid m_FileID"):
            parse(mono(pptr(5, file_id=1)), Generator())
        cut = tpk_object(114, m_GameObject=pptr(0), m_Enabled=1, m_Script=pptr(5), m_Name="mb")[:-8]
        with self.assertRaises(EOFError):
            parse(tpk_file([(1, 114, cut), (5, 115, script)]), Generator())
        with self.assertRaisesRegex(RuntimeError, "^generator broke$"):
            parse(mono(pptr(5)), Generator(error=RuntimeError("generator broke")))
        # A file with no Environment fails before the head parse, reading the generator off None.
        orphan = UnityPy.files.SerializedFile(EndianBinaryReader(mono(pptr(5))), None).objects[1]
        with self.assertRaisesRegex(AttributeError, "typetree_generator"):
            quietly(orphan.parse_as_dict)
        # A node the generator returns is used as the object's tree.
        node = Tpk.get_typetree_node(114, UnityVersion.from_str(VERSION))
        extended = TypeTreeNode.from_list(
            [n.to_dict() | {"m_Children": []} for n in node.traverse()]
            + [{"m_Level": 1, "m_Type": "int", "m_Name": "extra", "m_ByteSize": 4, "m_Version": 1, "m_MetaFlag": 0}]
        )
        self.assertEqual(parse(mono(pptr(5), struct.pack("<i", 77)), Generator(extended)), dict(tpk_head, extra=77))
        # parse_monobehaviour_head reads the TPK's tree without check_read, so trailing bytes do not raise there,
        # while the plain parse with that tree does.
        obj = load(mono(pptr(5), b"\0" * 8))[0].objects[1]
        got = obj.parse_monobehaviour_head()
        self.assertIsInstance(got, MonoBehaviour)
        self.assertEqual((got.m_Name, got.m_Script.m_PathID), ("mb", 5))
        with self.assertRaisesRegex(ValueError, "Expected to read"):
            quietly(obj.parse_as_dict)

    # Sections 8, 9.3: peek_name and its process-wide cache of peek nodes.
    def test_peek_name(self):
        from UnityPy.helpers import TypeTreeNode as module

        def file(tree, *strings):
            return load(S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, b"".join(map(S.s32, strings)))]))[0]

        # A root key of its own, so the cache starts empty for it.
        name_first = S.Node("PeekProbe", "Base", [S.Node("string", "m_Name"), S.Node("string", "m_Script")])
        name_second = S.Node("PeekProbe", "Base", [S.Node("string", "m_Script"), S.Node("string", "m_Name")])
        key = ("Base", "PeekProbe", 1)
        self.addCleanup(module.NAME_PEEK_NODE_CACHE.pop, key, None)
        first = file(name_first, "the name", "x" * 4000).objects[1]
        # The peek node holds the children up to the first `m_Name` or `name`, read without check_read.
        peek, child = first._get_typetree_node().get_name_peek_node()
        self.assertEqual((child, [n.m_Name for n in peek.m_Children]), ("m_Name", ["m_Name"]))
        self.assertEqual(first.peek_name(), "the name")
        self.assertIs(module.NAME_PEEK_NODE_CACHE[key][0], peek)
        # The cache is keyed by the root's (m_Name, m_Type, m_Version) alone, so a later tree with that root and
        # another layout before its name reads with the cached node: the first string, not its name.
        second = file(name_second, "script2", "name2").objects[1]
        self.assertEqual((second.peek_name(), second.parse_as_dict()["m_Name"]), ("script2", "name2"))
        # A tree with no such child caches None, and peek_name returns None.
        nameless = S.Node("PeekProbeNone", "Base", [S.Node("string", "m_Script")])
        self.addCleanup(module.NAME_PEEK_NODE_CACHE.pop, ("Base", "PeekProbeNone", 1), None)
        self.assertIsNone(file(nameless, "s").objects[1].peek_name())
        self.assertIn(("Base", "PeekProbeNone", 1), module.NAME_PEEK_NODE_CACHE)
        # `name` counts as well as `m_Name`.
        lower = S.Node("PeekProbeLower", "Base", [S.Node("int", "i", byte_size=4), S.Node("string", "name")])
        self.addCleanup(module.NAME_PEEK_NODE_CACHE.pop, ("Base", "PeekProbeLower", 1), None)
        sf = load(S.serialized_file([S.Type(49, lower)], [S.Obj(1, 0, struct.pack("<i", 3) + S.s32("low"))]))[0]
        self.assertEqual(sf.objects[1].peek_name(), "low")

    # Sections 8, 9.3, 18: TypeTreeNode's traversal, dicts and writers, and dump_structure's text.
    def test_typetree_node_dumps(self):
        from UnityPy.helpers.TypeTreeNode import get_common_strings

        def row(level, type_, name, index, size=4, flags=0, meta=0, version=1):
            fields = {"m_Level": level, "m_Type": type_, "m_Name": name, "m_ByteSize": size, "m_Version": version}
            return fields | {"m_Index": index, "m_TypeFlags": flags, "m_MetaFlag": meta, "m_RefTypeHash": 0}

        rows = [row(0, "AABB", "Base", 0, 12), row(1, "int", "a", 1), row(2, "int", "a1", 2, 4, 1, 0x4000, 2)]
        rows.append(row(1, "int", "b", 3))
        root = TypeTreeNode.from_list(rows)
        # traverse: pre-order.
        self.assertEqual([n.m_Name for n in root.traverse()], ["Base", "a", "a1", "b"])
        # to_dict: the fields that are not None, m_Children (the node objects) included; to_dict_list: one per
        # node in pre-order.
        self.assertEqual(root.to_dict(), rows[0] | {"m_Children": root.m_Children})
        self.assertEqual(
            root.m_Children[1].to_dict(),
            rows[3] | {"m_Children": []},
        )
        self.assertEqual([d["m_Name"] for d in root.to_dict_list()], ["Base", "a", "a1", "b"])
        self.assertEqual(root.to_dict_list()[2], rows[2] | {"m_Children": []})
        # dump_structure: one line per node, the byte size in upper-case hex, the braces printed, and 17 spaces
        # after `Index{i},`: the one before the f-string's backslash continuation and the next source line's
        # 16-space indent; each level indents two more. dump_typetree_structure is it on the object's tree.
        line = "{}{} {} // ByteSize{{{}}}, Index{{{}}}," + " " * 17 + "Version{{{}}}, TypeFlags{{{}}}, MetaFlag{{{}}}"
        self.assertEqual(
            root.dump_structure(),
            "\n".join(
                [
                    line.format("  ", "AABB", "Base", "C", 0, 1, 0, 0),
                    line.format("    ", "int", "a", "4", 1, 1, 0, 0),
                    line.format("      ", "int", "a1", "4", 2, 2, 1, 16384),
                    line.format("    ", "int", "b", "4", 3, 1, 0, 0),
                ]
            ),
        )
        self.assertTrue(root.dump_structure(indent="").startswith("AABB Base // ByteSize{C}"))
        # A field that is None prints as `None`.
        self.assertEqual(
            TypeTreeNode(0, "int", "a", 4, 1, []).dump_structure(),
            "  int a // ByteSize{4}, Index{None}," + " " * 17 + "Version{1}, TypeFlags{None}, MetaFlag{None}",
        )
        obj = load(text_asset_file())[0].objects[1]
        self.assertEqual(obj.dump_typetree_structure(), obj._get_typetree_node().dump_structure())
        self.assertTrue(obj.dump_typetree_structure(indent="> ").startswith("> TextAsset Base"))
        # dump_blob: common strings by offset, except the one at offset 0 (AABB), written as a local string.
        self.assertEqual(get_common_strings()[0], "AABB")
        writer = EndianBinaryWriter(endian="<")
        root.dump_blob(writer, 22)
        count, size = struct.unpack_from("<ii", writer.bytes)
        self.assertEqual(writer.bytes[8 + count * 32 :], b"AABB\0a\0a1\0b\0")
        back = TypeTreeNode.parse_blob(EndianBinaryReader(writer.bytes, endian="<"), 22)
        self.assertEqual([n.m_Type for n in back.traverse()], ["AABB", "int", "int", "int"])
        # dump (the old format): the root's fields for every node, so four copies of the root's record.
        writer = EndianBinaryWriter(endian="<")
        root.dump(writer, 10)
        record = b"AABB\0Base\0" + struct.pack("<iiiiii", 12, 0, 0, 1, 0, 2)
        self.assertEqual(writer.bytes, record * 4)

    # Sections 8, 14.1, 19.2: ObjectReader.container, the container's path_dict, and PPtr's deref helpers.
    def test_object_container(self):
        def info(path_id, index=0, size=0, file_id=0):
            return {"preloadIndex": index, "preloadSize": size, "asset": pptr(path_id, file_id)}

        bundle = tpk_object(
            142,
            m_Name="ab",
            m_PreloadTable=[pptr(1), pptr(2), pptr(3)],
            m_Container=[
                ("a/one", info(1, 0, 3)),  # its preload range holds 1, 2 and 3
                ("a/two", info(2)),
                ("a/dup", info(1)),  # a second path for object 1
                ("a/external", info(4, file_id=1)),  # an entry in another file
            ],
        )
        text = [(n, 49, tpk_object(49, m_Name=f"t{n}", m_Script="")) for n in (1, 2, 3, 4)]
        data = tpk_file([(100, 142, bundle), *text], externals=[S.External(b"archive:/CAB-x/CAB-x")])
        sf, env = load(data)
        container = sf._container
        self.assertEqual(container.path_dict, {1: "a/dup", 2: "a/two", 4: "a/external"}, "before the preload pass")
        # container builds the Environment's index (the preload pass) first, then reads the path_dict by path id:
        # the last of two paths; a preload-only object its range's path; the other file's entry the own file's
        # object of that path id.
        self.assertFalse(env._container_index_built)
        self.assertEqual([sf.objects[n].container for n in (1, 2, 3, 4)], ["a/dup", "a/two", "a/one", "a/external"])
        self.assertTrue(env._container_index_built)
        self.assertIsNone(sf.objects[100].container)
        self.assertEqual(container["a/one"].path_id, 1)
        # A file with no Environment skips the pass, so a preload-only object has no path.
        from UnityPy.files import SerializedFile

        lone = SerializedFile(EndianBinaryReader(data), None)
        self.assertEqual([lone.objects[n].container for n in (1, 3)], ["a/dup", None])

        # PPtr.type is deref().type; deref_parse_as_dict and deref_parse_as_object parse the target.
        from UnityPy.classes import PPtr

        sf = load(text_asset_file("t", "s"))[0]
        ptr = PPtr(m_FileID=0, m_PathID=1, assetsfile=sf)
        self.assertEqual(ptr.type, ClassIDType.TextAsset)
        self.assertEqual(ptr.deref_parse_as_dict(), {"m_Name": "t", "m_Script": "s"})
        self.assertIsInstance(ptr.deref_parse_as_object(), TextAsset)
        with self.assertRaises(KeyError):
            _ = PPtr(m_FileID=0, m_PathID=9, assetsfile=sf).type

    # Section 9.2: the pure-Python path's class arrays, whose rules differ from a single class's.
    def test_pure_python_class_arrays(self):
        from UnityPy import classes

        def parse(element, fields, aligned, count=2):
            meta = 0x4000 if aligned else 0
            rows = [{"m_Level": 0, "m_Type": "ArrayHolder", "m_Name": "Base"}]
            rows += [
                {"m_Level": 1, "m_Type": "vector", "m_Name": "items"},
                {"m_Level": 2, "m_Type": "Array", "m_Name": "Array"},
            ]
            rows += [{"m_Level": 3, "m_Type": "int", "m_Name": "size"}]
            rows += [{"m_Level": 3, "m_Type": element, "m_Name": "data", "m_MetaFlag": meta}]
            rows += [{"m_Level": 4, "m_Type": "int", "m_Name": name, "m_MetaFlag": 0} for name in fields]
            root = TypeTreeNode.from_list([r | {"m_MetaFlag": r.get("m_MetaFlag", 0)} for r in rows])
            data = struct.pack("<i", count) + struct.pack(
                f"<{len(fields) * count}i", *range(1, len(fields) * count + 1)
            )
            holder = TypeTreeHelper.read_typetree(root, EndianBinaryReader(data, "<"), False, None, check_read=False)
            return holder.items

        def kinds(element, fields, aligned):
            try:
                items = parse(element, fields, aligned)
            except AttributeError:
                return AttributeError
            return [(type(i).__name__, i.get_type() if isinstance(i, classes.UnknownObject) else None) for i in items]

        metrics = ["totalAssetCount", "nonProAssetCount", "nonProAssetsCreatedAfterProLicense"]  # 1 required
        roles = ["m_ActiveMultiplayerRole", "m_ActiveMultiplayerRoles"]  # both optional
        cases = {
            # (element type, fields): (an aligned element: read_value_array's rules, an unaligned one: read_value's)
            ("AssetDatabaseMetrics", (*metrics,)): ("AssetDatabaseMetrics", "AssetDatabaseMetrics"),
            # A missing optional field: the array checks the annotations before it builds, so UnknownObject.
            ("AssetDatabaseMetrics", (metrics[0],)): ("UnknownObject", "AssetDatabaseMetrics"),
            ("AssetDatabaseMetrics", (*metrics[1:],)): ("UnknownObject", "UnknownObject"),
            ("AssetDatabaseMetrics", (*metrics, "m_Extra")): (AttributeError, AttributeError),
            ("MarshallingTestObject", ("m_Prop", "m_Extra")): ("MarshallingTestObject", "MarshallingTestObject"),
            ("MultiplayerManager", (*roles, "m_Extra")): ("MultiplayerManager", "MultiplayerManager"),
            ("MultiplayerManager", ("m_Extra",)): ("UnknownObject", "UnknownObject"),
            ("GameManager", ("m_Extra",)): ("GameManager", "GameManager"),
            # A type the module lacks: the array keeps the node, a single class drops it.
            ("NoSuchClass", ("a",)): ("UnknownObject:NoSuchClass", "UnknownObject:None"),
        }
        for (element, fields), expected in cases.items():
            for aligned, want in zip((True, False), expected, strict=True):
                with self.subTest(element, fields=fields, aligned=aligned):
                    got = kinds(element, fields, aligned)
                    if want is AttributeError:
                        self.assertIs(got, AttributeError)
                        continue
                    name, _, node_type = want.partition(":")
                    self.assertEqual([g[0] for g in got], [name, name])
                    if node_type:
                        self.assertEqual({g[1] for g in got}, {None if node_type == "None" else node_type})
        # Each element gets its own values.
        items = parse("AssetDatabaseMetrics", metrics, True)
        self.assertEqual([i.totalAssetCount for i in items], [1, 4])

    # Section 10: the shipped TPK, TpkFile's codecs, the node lookup and its cache.
    def test_tpk_file_and_lookup(self):
        import gzip  # noqa: F401 - keeps the codec imports together
        import io
        import lzma

        import brotli
        import lz4.block
        from UnityPy.helpers.Tpk import TpkCompressionType, TpkDataType, TpkFile, TpkJsonBlob, TpkTypeTreeBlob

        # The shipped resources/lzma.tpk: LZMA, type tree information, loaded at import into TPKTYPETREE.
        shipped = (Path(Tpk.__file__).parent.parent / "resources" / "lzma.tpk").read_bytes()
        head = TpkFile(io.BytesIO(shipped))
        self.assertEqual(
            (head.CompressionType, head.DataType, head.CompressedSize, head.UncompressedSize, len(shipped)),
            (TpkCompressionType.Lzma, TpkDataType.TypeTreeInformation, 187298, 1385610, 187318),
        )
        self.assertIsInstance(Tpk.TPKTYPETREE, TpkTypeTreeBlob)

        # TpkFile: magic TPK*, version 1, then the blob with no compression, LZ4, LZMA (the 5-byte props form
        # decompress_lzma reads) or brotli.
        text = "a json blob " * 10
        blob = bytes([len(text)]) + text.encode()  # a varint length below 128, then the UTF-8 text

        def tpk(codec, data, magic=0x2A4B5054, version=1, data_type=3, size=None):
            head = struct.pack("<IbbbbIII", magic, version, codec, data_type, 0, 0, len(data), size or len(blob))
            return io.BytesIO(head + data)

        filters = [{"id": lzma.FILTER_LZMA1, "dict_size": 1 << 16}]
        packed = {
            TpkCompressionType.NONE: blob,
            TpkCompressionType.Lz4: lz4.block.compress(blob, store_size=False),
            TpkCompressionType.Lzma: b"\x5d"
            + (1 << 16).to_bytes(4, "little")
            + lzma.compress(blob, format=lzma.FORMAT_RAW, filters=filters),
            TpkCompressionType.Brotli: brotli.compress(blob),
        }
        for codec, data in packed.items():
            with self.subTest(codec=codec.name):
                got = TpkFile(tpk(codec, data)).GetDataBlob()
                self.assertIsInstance(got, TpkJsonBlob)
                self.assertEqual(got.Text, text)
        with self.assertRaisesRegex(Exception, "^Invalid TPK magic bytes$"):
            TpkFile(tpk(0, blob, magic=0))
        with self.assertRaisesRegex(Exception, "^Invalid TPK version number$"):
            TpkFile(tpk(0, blob, version=2))
        with self.assertRaisesRegex(Exception, "^Invalid compressed size$"):
            TpkFile(io.BytesIO(tpk(0, blob).getvalue()[:-1]))
        with self.assertRaises(ValueError):
            TpkFile(tpk(4, blob))  # not a TpkCompressionType
        for data_type in (TpkDataType.ReferenceAssemblies, TpkDataType.EngineAssets):
            with (
                self.subTest(data_type=data_type.name),
                self.assertRaisesRegex(Exception, "^Unimplemented TpkDataType"),
            ):
                TpkFile(tpk(0, blob, data_type=data_type)).GetDataBlob()

        # get_typetree_node: KeyError for an id the TPK lacks; the versioned class through get_item_for_version,
        # which raises ValueError rather than return None; one node per (class_id, version), kept.
        version = UnityVersion.from_str(VERSION)
        with self.assertRaises(KeyError):
            Tpk.get_typetree_node(987654, version)
        with self.assertRaisesRegex(ValueError, "^Could not find exact version$"):
            Tpk.get_item_for_version(UnityVersion.from_str("1.0.0f1"), [(version, "x")])
        node = Tpk.get_typetree_node(49, version)
        self.assertIs(Tpk.get_typetree_node(49, version), node)
        self.assertIs(Tpk.CLASSES_CACHE[(49, version)], node)
        # generate_node: the release root, pre-order, m_Index counted from 0, no type flags.
        flat = list(node.traverse())
        self.assertEqual([n.m_Index for n in flat], list(range(len(flat))))
        self.assertEqual({n.m_TypeFlags for n in flat}, {None})
        self.assertEqual(
            [(n.m_Level, n.m_Type, n.m_Name) for n in flat[:2]], [(0, "TextAsset", "Base"), (1, "string", "m_Name")]
        )

    # Section 10: TypeTreeGenerator without TypeTreeGeneratorAPI, and the wrapper methods UnityPy adds.
    def test_typetree_generator(self):
        import os
        import tempfile

        from UnityPy.helpers.TypeTreeGenerator import TypeTreeGenerator

        with self.assertRaisesRegex(ImportError, "^TypeTreeGeneratorAPI isn't installed!$"):
            TypeTreeGenerator(VERSION)
        # The methods UnityPy adds, on an instance whose base calls are recorded (the base's own __init__ raises).
        calls = []

        class Recording(TypeTreeGenerator):
            def __init__(self):
                self.cache = {}

            def load_dll(self, data):
                calls.append(("dll", data))

            def load_il2cpp(self, il2cpp, metadata):
                calls.append(("il2cpp", il2cpp, metadata))

            def get_nodes(self, assembly, fullname):
                calls.append(("nodes", assembly, fullname))
                return [
                    TypeTreeNode(0, "Thing", "Base", 99, 5, m_MetaFlag=0, m_TypeFlags=7),
                    TypeTreeNode(1, "int", "x", 4, 3, m_MetaFlag=0x4000),
                ]

        gen = Recording()
        # get_nodes_up: `.dll` appended, byte size and version 0, the meta flags kept, cached by the name as asked.
        node = gen.get_nodes_up("Asm", "ns.Thing")
        self.assertEqual(calls, [("nodes", "Asm.dll", "ns.Thing")])
        self.assertEqual(
            [
                (n.m_Level, n.m_Type, n.m_Name, n.m_ByteSize, n.m_Version, n.m_MetaFlag, n.m_TypeFlags)
                for n in node.traverse()
            ],
            [(0, "Thing", "Base", 0, 0, 0, None), (1, "int", "x", 0, 0, 0x4000, None)],
        )
        self.assertIs(gen.get_nodes_up("Asm", "ns.Thing"), node)
        self.assertIsNot(gen.get_nodes_up("Asm.dll", "ns.Thing"), node, "keyed by the name as given")
        self.assertEqual(len(calls), 2)
        # load_local_dll_folder: every `.dll` in the folder, in listdir order; load_local_game: IL2CPP when the
        # root holds GameAssembly.dll, else the `*_Data/Managed` folder.
        with tempfile.TemporaryDirectory(prefix="unityrs-gen-") as root:
            managed = os.path.join(root, "Game_Data", "Managed")
            os.makedirs(managed)
            for name, data in (("a.dll", b"A"), ("b.txt", b"T"), ("c.DLL", b"C"), ("d.dll", b"D")):
                with open(os.path.join(managed, name), "wb") as f:
                    f.write(data)
            calls.clear()
            gen.load_local_game(root)
            order = [f for f in os.listdir(managed) if f.endswith(".dll")]
            self.assertEqual(calls, [("dll", {"a.dll": b"A", "d.dll": b"D"}[f]) for f in order])
            metadata = os.path.join(root, "Game_Data", "il2cpp_data", "Metadata")
            os.makedirs(metadata)
            with open(os.path.join(metadata, "global-metadata.dat"), "wb") as f:
                f.write(b"META")
            with open(os.path.join(root, "GameAssembly.dll"), "wb") as f:
                f.write(b"GA")
            calls.clear()
            gen.load_local_game(root)
            self.assertEqual(calls, [("il2cpp", b"GA", b"META")])

    # Sections 9.1, 19.1: a ReferencedObject array element with an empty class, on each path.
    def test_referenced_object_arrays(self):
        element = S.ref_object("data")
        element.meta = 0x4000  # aligned: the pure-Python path reads the array with read_value_array
        refs = S.vec("refs", element)
        root = S.Node("MonoBehaviour", "Base", [refs])
        foo = S.Node("Foo", "Base", [S.Node("float", "v")])
        typed = struct.pack("<q", 2) + S.s32("Foo") + S.s32("") + S.s32("Asm") + struct.pack("<f", 1.5)
        empty = struct.pack("<q", 1) + S.s32("") * 3
        data = struct.pack("<i", 2) + typed + empty
        sf = load(
            S.serialized_file(
                [S.Type(114, root)], [S.Obj(1, 0, data)], ref_types=[S.Type(-1, foo, ref_names=["Foo", "", "Asm"])]
            )
        )[0]
        obj = sf.objects[1]
        node = obj._get_typetree_node()
        # UnityPyBoost (what ObjectReader uses) skips the empty element's data, as the single-value path does.
        self.assertEqual(
            obj.parse_as_dict()["refs"],
            [
                {"rid": 2, "type": {"class": "Foo", "ns": "", "asm": "Asm"}, "data": {"v": 1.5}},
                {"rid": 1, "type": {"class": "", "ns": "", "asm": ""}},
            ],
        )
        # The pure-Python array path hands read_value the None that get_ref_type_node returns for it.
        with self.assertRaisesRegex(AttributeError, "'NoneType' object has no attribute 'm_MetaFlag'"):
            TypeTreeHelper.read_typetree(node, EndianBinaryReader(data, "<"), True, None, False, sf)
        # A single ReferencedObject with an empty class reads on that path, its data skipped.
        single = S.Node("MonoBehaviour", "Base", [S.ref_object("ref")])
        one = load(
            S.serialized_file(
                [S.Type(114, single)], [S.Obj(1, 0, empty)], ref_types=[S.Type(-1, foo, ref_names=["Foo", "", "Asm"])]
            )
        )[0]
        got = TypeTreeHelper.read_typetree(
            one.objects[1]._get_typetree_node(), EndianBinaryReader(empty, "<"), True, None, False, one
        )
        self.assertEqual(got, {"ref": {"rid": 1, "type": {"class": "", "ns": "", "asm": ""}}})
