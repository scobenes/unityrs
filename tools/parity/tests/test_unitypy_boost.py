"""Synthetic typetrees read through UnityPyBoost, encoded canonically.

These pin what the oracle reports, not what a file holds: where UnityPy loses
information (a float32 signalling NaN, a bool byte of 2), the test says so.
Needs the oracle stack (UnityPy with UnityPyBoost); skipped elsewhere.
"""

import struct
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))

import canonical as C  # noqa: E402

try:
    from UnityPy.helpers import TypeTreeHelper as TTH
    from UnityPy.helpers.TypeTreeNode import TypeTreeNode
    from UnityPy.streams import EndianBinaryReader, EndianBinaryWriter
except ImportError:  # pragma: no cover - outside the oracle image
    TTH = None

ALIGN = 0x4000
SPEC = HERE.parents[1] / "docs" / "CANONICAL_DUMP.md"


def N(t, name, children=(), meta=0):
    return TypeTreeNode(
        m_Level=0, m_Type=t, m_Name=name, m_ByteSize=-1, m_Version=1, m_Children=list(children), m_MetaFlag=meta
    )


def vec(name, elem, meta=0):
    return N("vector", name, [N("Array", "Array", [N("int", "size"), elem], meta=meta)])


def read(root, data, endian="<", assetsfile=None):
    """parse_as_dict's path: UnityPyBoost whenever byte_size is non-zero."""
    return TTH.read_typetree(
        root, EndianBinaryReader(data, endian=endian), as_dict=True, byte_size=len(data), assetsfile=assetsfile
    )


def resave(value, root, endian="<", assetsfile=None):
    w = EndianBinaryWriter(endian=endian)
    TTH.write_typetree(value, root, w, assetsfile)
    return bytes(w.bytes)


def example():
    """The spec's worked example (section 3): its tree and bytes."""
    root = N(
        "Base",
        "Base",
        [
            N("float", "f_snan"),
            N("float", "f_qnan_payload"),
            N("float", "f_negzero"),
            N("double", "d_snan"),
            N("bool", "b2", meta=ALIGN),
            N("string", "s_bad"),
            N("TypelessData", "blob", meta=ALIGN),
            vec("fv", N("float", "data")),
            vec("u8v", N("UInt8", "data"), meta=ALIGN),
            N(
                "map",
                "m",
                [
                    N(
                        "Array",
                        "Array",
                        [N("int", "size"), N("pair", "data", [N("string", "first"), N("SInt64", "second")])],
                    )
                ],
            ),
            N("int", "dup"),
            N("float", "dup"),
        ],
    )
    data = b"".join(
        [
            struct.pack("<I", 0x7F800001),
            struct.pack("<I", 0xFFC12345),
            struct.pack("<I", 0x80000000),
            struct.pack("<Q", 0x7FF0000000000001),
            b"\x02\0\0\0",
            struct.pack("<i", 3) + b"a\xffb\0",
            struct.pack("<i", 2) + b"\x00\x01\0\0",
            struct.pack("<i", 2) + struct.pack("<ff", 1.5, float("inf")),
            struct.pack("<i", 3) + b"\x00\x7f\xff\0",
            struct.pack("<i", 1) + struct.pack("<i", 1) + b"k\0\0\0" + struct.pack("<q", -5),
            struct.pack("<i", 7),
            struct.pack("<f", 2.0),
        ]
    )
    return root, data


@unittest.skipIf(TTH is None, "needs UnityPy (the oracle image)")
class ThroughBoost(unittest.TestCase):
    def test_oracle_is_boost(self):
        self.assertIsNotNone(TTH.read_typetree_boost, "UnityPyBoost must be loaded for the oracle")

    def test_spec_example_is_what_the_oracle_writes(self):
        root, data = example()
        line = C.dumps(C.ValueEncoder().encode(read(root, data), root))
        self.assertIn("```json\n" + line + "\n```", SPEC.read_text(), "the spec's worked example drifted")

    def test_float_bits(self):
        root, data = example()
        v = C.ValueEncoder().encode(read(root, data), root)
        got = dict(v["v"])
        # The file's float32 sNaN 7f800001 is quieted by CPython's widening.
        self.assertEqual(got["f_snan"], {"t": "f32", "v": "7fc00001"})
        self.assertEqual(got["f_qnan_payload"], {"t": "f32", "v": "ffc12345"})
        self.assertEqual(got["f_negzero"], {"t": "f32", "v": "80000000"})
        self.assertEqual(got["d_snan"], {"t": "f64", "v": "7ff0000000000001"})
        self.assertEqual(got["fv"], {"t": "f32s", "v": "3fc000007f800000"})

    def test_resave_shows_what_the_dict_lost(self):
        root, data = example()
        value = read(root, data)
        # The repeated name leaves a float where the first 'dup' needs an int.
        with self.assertRaises(struct.error):
            resave(value, root)
        clean = N("Base", "Base", [N("float", "f"), N("bool", "b", meta=ALIGN), N("float", "q")])
        raw = struct.pack("<I", 0x3F800000) + b"\x01\0\0\0" + struct.pack("<I", 0xFFC12345)
        self.assertEqual(resave(read(clean, raw), clean), raw)
        bool2 = raw[:4] + b"\x02" + raw[5:]
        self.assertNotEqual(resave(read(clean, bool2), clean), bool2, "a bool byte of 2 resaves as 1")
        snan = struct.pack("<I", 0x7F800001) + raw[4:]
        self.assertNotEqual(resave(read(clean, snan), clean), snan, "a float32 sNaN resaves quieted")

    def test_big_endian_reads_the_same_values(self):
        root = N(
            "Base",
            "Base",
            [N("UInt16", "u", meta=ALIGN), N("SInt64", "i"), vec("f", N("float", "data")), N("double", "d")],
        )
        le = (
            struct.pack("<H", 513)
            + b"\0\0"
            + struct.pack("<qi", -2, 2)
            + struct.pack("<ff", 1.0, -2.5)
            + struct.pack("<d", 3.25)
        )
        be = (
            struct.pack(">H", 513)
            + b"\0\0"
            + struct.pack(">qi", -2, 2)
            + struct.pack(">ff", 1.0, -2.5)
            + struct.pack(">d", 3.25)
        )
        enc = C.ValueEncoder().encode
        self.assertEqual(enc(read(root, le), root), enc(read(root, be, ">"), root))

    def test_shapes(self):
        root = N(
            "Base",
            "Base",
            [
                vec("vv", vec("data", N("float", "data"))),
                vec("s8", N("SInt8", "data"), meta=ALIGN),
                vec("empty", N("float", "data")),
                vec("strs", N("string", "data")),
                vec("blobs", N("TypelessData", "data")),
                N("PPtr<Object>", "ptr", [N("int", "m_FileID"), N("SInt64", "m_PathID")]),
            ],
        )
        data = b"".join(
            [
                struct.pack("<ii", 2, 1) + struct.pack("<f", 1.0) + struct.pack("<i", 0),
                struct.pack("<i", 2) + b"\xff\x01\0\0",
                struct.pack("<i", 0),
                struct.pack("<i", 2) + struct.pack("<i", 1) + b"a\0\0\0" + struct.pack("<i", 0),
                struct.pack("<i", 1) + struct.pack("<i", 1) + b"\x09",
                struct.pack("<iq", 1, -7),
            ]
        )
        self.assertEqual(
            C.dumps(C.ValueEncoder().encode(read(root, data), root)),
            '{"t":"dict","v":[["vv",[{"t":"f32s","v":"3f800000"},[]]],["s8",[-1,1]],["empty",[]],'
            '["strs",["a",""]],["blobs",[{"t":"bytes","hex":"09"}]],'
            '["ptr",{"t":"dict","v":[["m_FileID",1],["m_PathID",-7]]}]]}',
        )

    def test_referenced_objects_and_the_first_registry_only(self):
        foo = N("Foo", "data", [N("float", "x")])
        assetsfile = SimpleNamespace(
            ref_types=[SimpleNamespace(m_ClassName="Foo", m_NameSpace="", m_AssemblyName="Asm", node=foo)]
        )
        ref_obj = N(
            "ReferencedObject",
            "data",
            [
                N("SInt64", "rid"),
                N("ReferencedManagedType", "type", [N("string", "class"), N("string", "ns"), N("string", "asm")]),
                N("ReferencedObjectData", "data"),
            ],
        )
        registry = N("ManagedReferencesRegistry", "references", [N("int", "version"), vec("RefIds", ref_obj)])
        second = N("ManagedReferencesRegistry", "again", [N("int", "version")])
        root = N("Base", "Base", [registry, second, N("int", "tail")])

        def s(text):
            raw = text.encode()
            return struct.pack("<i", len(raw)) + raw + b"\0" * (-len(raw) % 4)

        data = b"".join(
            [
                struct.pack("<ii", 2, 2),
                struct.pack("<q", 1) + s("Foo") + s("") + s("Asm") + struct.pack("<f", 1.0),
                struct.pack("<q", 2) + s("") + s("") + s(""),
                struct.pack("<i", 9),
            ]
        )
        value = read(root, data, assetsfile=assetsfile)
        self.assertNotIn("again", value, "a second registry in the subtree is skipped")
        self.assertNotIn("data", value["references"]["RefIds"][1], "an empty class has no data")
        resolve = lambda t: TTH.get_ref_type_node({"type": t}, assetsfile)  # noqa: E731
        enc = C.ValueEncoder(resolve).encode(value, root)
        self.assertEqual(
            C.dumps(enc),
            '{"t":"dict","v":[["references",{"t":"dict","v":[["version",2],["RefIds",['
            '{"t":"dict","v":[["rid",1],["type",{"t":"dict","v":[["class","Foo"],["ns",""],["asm","Asm"]]}],'
            '["data",{"t":"dict","v":[["x",{"t":"f32","v":"3f800000"}]]}]]},'
            '{"t":"dict","v":[["rid",2],["type",{"t":"dict","v":[["class",""],["ns",""],["asm",""]]}]]}]]]}],'
            '["tail",9]]}',
        )

    def test_pure_python_path_returns_tuples(self):
        # byte_size 0 (falsy) routes UnityPy to its Python reader, whose
        # arrays of an align-flagged primitive come back as tuples.
        root = N("Base", "Base", [vec("v", N("int", "data", meta=ALIGN))])
        data = struct.pack("<iii", 2, 5, 6)
        value = TTH.read_typetree(
            root, EndianBinaryReader(data, endian="<"), as_dict=True, byte_size=0, check_read=False
        )
        self.assertEqual(C.ValueEncoder().encode(value, root), {"t": "dict", "v": [["v", {"t": "tuple", "v": [5, 6]}]]})


if __name__ == "__main__":
    unittest.main()
