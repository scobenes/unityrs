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
    from UnityPy.helpers.UnityVersion import UnityVersion
    from UnityPy.streams import EndianBinaryReader, EndianBinaryWriter

    import unitypy_dump
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


def read(root, data, endian="<", assetsfile=None, check_read=True):
    """parse_as_dict's path: UnityPyBoost whenever byte_size is non-zero."""
    return TTH.read_typetree(
        root,
        EndianBinaryReader(data, endian=endian),
        as_dict=True,
        byte_size=len(data),
        check_read=check_read,
        assetsfile=assetsfile,
    )


def read_python(root, data, check_read=True, assetsfile=None):
    """The byte_size 0 path: UnityPy's pure-Python reader, from the reader's position onwards."""
    return TTH.read_typetree(
        root,
        EndianBinaryReader(data, endian="<"),
        as_dict=True,
        byte_size=0,
        check_read=check_read,
        assetsfile=assetsfile,
    )


def s32(text):
    """An aligned typetree string."""
    raw = text.encode()
    return struct.pack("<i", len(raw)) + raw + b"\0" * (-len(raw) % 4)


def ref_tree():
    """A ReferencedObject field, and an assets file whose ref_types hold only Foo."""
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
    return ref_obj, assetsfile


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

    def test_referenced_objects_and_the_registry_scope(self):
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
        self.assertNotIn("again", value, "a second registry in the class that read one is skipped")
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

    def test_error_kinds(self):
        """Each L2 error kind from a real UnityPy failure, mapped by the oracle's error_kind (spec 4.3)."""
        two = N("Base", "Base", [N("int", "a"), N("int", "b")])
        aligned_tail = N("Base", "Base", [N("int", "i"), N("bool", "b", meta=ALIGN)])
        ref_obj, assetsfile = ref_tree()
        unknown_ref = struct.pack("<q", 1) + s32("Bar") + s32("") + s32("Asm") + struct.pack("<f", 1.0)
        boost = [
            ("fewer bytes", N("Base", "Base", [N("int", "a")]), struct.pack("<ii", 1, 2), ("check_read", 4)),
            ("the last alignment runs past the end", aligned_tail, struct.pack("<i", 7) + b"\1", ("check_read", 8)),
            ("an int past the end", two, struct.pack("<i", 1) + b"\0\0", ("out_of_bounds", None)),
            (
                "a bool past the end",
                N("Base", "Base", [N("int", "a"), N("bool", "b")]),
                b"\1\0\0\0",
                ("out_of_bounds", None),
            ),
            (
                "a string past the end",
                N("Base", "Base", [N("string", "s")]),
                struct.pack("<i", 9) + b"ab",
                ("out_of_bounds", None),
            ),
            (
                "bytes past the end",
                N("Base", "Base", [N("TypelessData", "s")]),
                struct.pack("<i", 9) + b"ab",
                ("out_of_bounds", None),
            ),
            ("a length past the end", N("Base", "Base", [N("string", "s")]), b"\1\0", ("out_of_bounds", None)),
            (
                "a negative string length",
                N("Base", "Base", [N("string", "s")]),
                struct.pack("<i", -1) + b"abcd",
                ("negative_length", None),
            ),
            (
                "a negative array length",
                N("Base", "Base", [vec("v", N("int", "data"))]),
                struct.pack("<i", -1) + b"abcd",
                ("negative_length", None),
            ),
            (
                "a negative byte-array length",
                N("Base", "Base", [N("TypelessData", "s")]),
                struct.pack("<i", -1) + b"abcd",
                ("negative_length", None),
            ),
            ("an unknown ref type", N("Base", "Base", [ref_obj]), unknown_ref, ("ref_type", None)),
            (
                "a pair with one child",
                N("Base", "Base", [N("pair", "p", [N("int", "first")])]),
                struct.pack("<ii", 1, 2),
                ("other", None),
            ),
        ]
        python = [
            ("any read at all", two, struct.pack("<ii", 11, 22) + b"rest", ("check_read", 8)),
            ("past the file's end", two, struct.pack("<i", 1) + b"\0\0", ("other", None)),
            (
                "a negative array length",
                N("Base", "Base", [vec("v", N("int", "data"))]),
                struct.pack("<i", -1),
                ("negative_length", None),
            ),
            ("an unknown ref type", N("Base", "Base", [ref_obj]), unknown_ref, ("ref_type", None)),
            (
                "an aligned SInt8 array",
                N("Base", "Base", [vec("v", N("SInt8", "data", meta=ALIGN))]),
                struct.pack("<i", 1) + b"\1",
                ("other", None),
            ),
        ]
        for path, cases, reader in (("boost", boost, read), ("python", python, read_python)):
            for label, root, data, expected in cases:
                with self.subTest(path=path, case=label), self.assertRaises(Exception) as cm:
                    reader(root, data, assetsfile=assetsfile)
                self.assertEqual(unitypy_dump.error_kind(cm.exception), expected, f"{path}: {label}: {cm.exception}")
            # A file without ref_types cannot resolve any ReferencedObject.
            with self.subTest(path=path, case="no ref_types"), self.assertRaises(Exception) as cm:
                reader(N("Base", "Base", [ref_obj]), unknown_ref, assetsfile=SimpleNamespace(ref_types=None))
            self.assertEqual(unitypy_dump.error_kind(cm.exception), ("ref_type", None), f"{path}: {cm.exception}")

    def test_check_read_values(self):
        """check_read keeps the value parse_as_dict(check_read=False) returns, in either direction."""
        aligned_tail = N("Base", "Base", [N("int", "i"), N("bool", "b", meta=ALIGN)])
        self.assertEqual(read(aligned_tail, struct.pack("<i", 7) + b"\1", check_read=False), {"i": 7, "b": True})
        # byte_size 0 reads on from byte_start, into whatever follows the object.
        two = N("Base", "Base", [N("int", "a"), N("int", "b")])
        self.assertEqual(read_python(two, struct.pack("<ii", 11, 22) + b"rest", check_read=False), {"a": 11, "b": 22})
        self.assertEqual(read_python(N("Base", "Base", []), b"rest"), {}, "a tree that reads nothing is no error")
        # The pure-Python reader reads a negative or out-of-range string length as "".
        string = N("Base", "Base", [N("string", "s")])
        for length in (-1, 99):
            self.assertEqual(read_python(string, struct.pack("<i", length) + b"abcd", check_read=False), {"s": ""})

    def test_registry_scope_is_the_class_that_holds_it(self):
        """A registry read by a class hides the later registries of that class and its later subtrees only."""

        def reg(name):
            return N("ManagedReferencesRegistry", name, [N("int", "version")])

        root = N(
            "Base",
            "Base",
            [N("A", "a", [reg("r1")]), N("B", "b", [reg("r2")]), reg("r3"), N("C", "c", [reg("r5")]), reg("r4")],
        )
        data = struct.pack("<iii", 1, 2, 3)
        expected = {"a": {"r1": {"version": 1}}, "b": {"r2": {"version": 2}}, "r3": {"version": 3}, "c": {}}
        self.assertEqual(read(root, data), expected)
        self.assertEqual(read_python(root, data, check_read=False), expected)
        # A registry inside the registry being read is skipped too.
        nested = N("ManagedReferencesRegistry", "r", [N("int", "version"), reg("inner")])
        root = N("Base", "Base", [nested, N("int", "tail")])
        expected = {"r": {"version": 1}, "tail": 2}
        self.assertEqual(read(root, struct.pack("<ii", 1, 2)), expected)
        self.assertEqual(read_python(root, struct.pack("<ii", 1, 2), check_read=False), expected)

    def test_pure_python_path_reads_every_registry_in_aligned_class_arrays(self):
        """Its array reader has no registry check for class elements whose node carries the align flag."""
        elem = N(
            "Elem",
            "data",
            [
                N("ManagedReferencesRegistry", "r1", [N("int", "v")]),
                N("ManagedReferencesRegistry", "r2", [N("int", "v")]),
            ],
            meta=ALIGN,
        )
        root = N("Base", "Base", [vec("arr", elem)])
        data = struct.pack("<iii", 1, 11, 22)
        self.assertEqual(read(root, data, check_read=False), {"arr": [{"r1": {"v": 11}}]})
        self.assertEqual(read_python(root, data, check_read=False), {"arr": [{"r1": {"v": 11}, "r2": {"v": 22}}]})

    def test_pure_python_path_reads_negative_typeless_lengths(self):
        """UnityPyBoost refuses a negative TypelessData length; the pure-Python reader slices with it."""
        root = N("Base", "Base", [N("TypelessData", "blob"), N("int", "after")])
        for n in (-1, -4):
            data = struct.pack("<ii", n, 0x55) + b"\1\2\3\4"
            with self.subTest(n=n), self.assertRaises(ValueError) as e:
                read(root, data, check_read=False)
            self.assertEqual(unitypy_dump.error_kind(e.exception), ("negative_length", None))
        # -4: nothing is read and the position moves back 4 bytes, so "after" re-reads the length.
        data = struct.pack("<ii", -4, 0x55) + b"\1\2\3\4"
        self.assertEqual(read_python(root, data, check_read=False), {"blob": b"", "after": -4})
        # -1: the rest of the data is read, and the next read fails.
        data = struct.pack("<ii", -1, 0x55) + b"\1\2\3\4"
        with self.assertRaises(Exception) as e:
            read_python(root, data, check_read=False)
        self.assertEqual(unitypy_dump.error_kind(e.exception), ("other", None))

    def test_pure_python_path_in_arrays_of_aligned_vectors_and_typeless_data(self):
        """Two more array-reader differences: a negative inner length, and TypelessData elements."""
        inner = N("vector", "data", [N("Array", "Array", [N("int", "size"), N("int", "data")])], meta=ALIGN)
        root = N("Base", "Base", [vec("vv", inner), N("int", "after")])
        data = struct.pack("<iii", 1, -3, 42)
        with self.assertRaises(ValueError) as e:
            read(root, data, check_read=False)
        self.assertEqual(unitypy_dump.error_kind(e.exception), ("negative_length", None))
        self.assertEqual(read_python(root, data, check_read=False), {"vv": [[]], "after": 42})

        root = N("Base", "Base", [vec("tl", N("TypelessData", "data", meta=ALIGN)), N("int", "after")])
        data = struct.pack("<iii", 1, 0, 42)
        self.assertEqual(read(root, data, check_read=False), {"tl": [b""], "after": 42})
        with self.assertRaises(TypeError) as e:
            read_python(root, data, check_read=False)
        self.assertEqual(unitypy_dump.error_kind(e.exception), ("other", None))

    def test_pure_python_path_aligns_class_element_arrays_once(self):
        """With an aligned class element it aligns after the array, not after each element as UnityPyBoost does."""
        elem = N("Elem", "data", [N("UInt8", "x")], meta=ALIGN)
        root = N("Base", "Base", [vec("arr", elem), N("int", "after")])
        data = struct.pack("<i", 2) + b"\1\0\0\0\2\0\0\0" + struct.pack("<i", 42)
        self.assertEqual(read(root, data), {"arr": [{"x": 1}, {"x": 2}], "after": 42})
        self.assertEqual(resave(read(root, data), root), data)  # the writer pads each element too
        self.assertEqual(read_python(root, data, check_read=False), {"arr": [{"x": 1}, {"x": 0}], "after": 2})

    def test_pure_python_path_swaps_16_bit_signedness(self):
        """In aligned arrays the pure-Python reader reads UInt16 as signed and SInt16 as unsigned."""
        root = N(
            "Base",
            "Base",
            [
                vec("u16", N("UInt16", "data", meta=ALIGN)),
                vec("s16", N("SInt16", "data", meta=ALIGN)),
                vec("us", N("unsigned short", "data", meta=ALIGN)),
                vec("ss", N("short", "data", meta=ALIGN)),
            ],
        )
        data = (struct.pack("<iHH", 2, 0xFFFF, 1) + struct.pack("<ihh", 2, -1, 1)) * 2
        self.assertEqual(read(root, data), {"u16": [65535, 1], "s16": [-1, 1], "us": [65535, 1], "ss": [-1, 1]})
        self.assertEqual(
            read_python(root, data, check_read=False),
            {"u16": (-1, 1), "s16": (65535, 1), "us": (-1, 1), "ss": (65535, 1)},
        )

    def test_version_packing(self):
        """UnityVersion.from_str, which engine_version_packed and version_packed record (spec 4.2)."""
        cases = {
            "2021.3.2f1": (2021, 3, 2, 3, 1),
            "2021.3.2F1": (2021, 3, 2, 3, 1),  # the type letter is lower-cased
            "2021.3.2ff1": (2021, 3, 2, 3, 0),  # one character is the type; "f1" is a postfix
            "2019.4.15 f1": (2019, 4, 15, 255, 0),  # a space is an unknown type
            "2021.3.2u1": (2021, 3, 2, 255, 1),
            "5.6.7": (5, 6, 7, 3, 0),  # no type letter packs as f
            "6000.3.17f1c1": (6000, 3, 17, 3, 1),
            "2020.1.0a12-x": (2020, 1, 0, 0, 12),
            "2017.1.0f300": (2017, 1, 0, 3, 300),  # a plain OR: 300 spills into the type byte
            "\u0662\u0660\u0662\u0661.3.2f1": (2021, 3, 2, 3, 1),  # any Unicode decimal digit
            "2021.3.2\n1": (2021, 3, 2, 255, 1),  # DOTALL: a newline is a type character
        }
        for text, (major, minor, build, typ, number) in cases.items():
            with self.subTest(text):
                packed = major << 48 | minor << 32 | build << 16 | typ << 8 | number
                self.assertEqual(int(UnityVersion.from_str(text)), packed)
        for text in ("2021.3", "x2021.3.2f1", ""):
            with self.subTest(text), self.assertRaises(ValueError):
                UnityVersion.from_str(text)

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
