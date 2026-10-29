"""The encoding rules of docs/CANONICAL_DUMP.md sections 2 and 3, on synthetic values.

Standard library only: these pin the byte format a Rust producer must match.
"""

import hashlib
import struct
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import canonical as C  # noqa: E402


class Node:
    """A duck-typed typetree node."""

    def __init__(self, m_Type, m_Name="x", children=()):
        self.m_Type, self.m_Name, self.m_Children = m_Type, m_Name, list(children)


def vec(name, elem):
    return Node("vector", name, [Node("Array", "Array", [Node("int", "size"), elem])])


def widened_f32(bits):
    """The Python float UnityPyBoost holds for a float32 bit pattern."""
    return struct.unpack("<f", struct.pack("<I", bits))[0]


def f64_of(bits):
    return struct.unpack("<d", struct.pack("<Q", bits))[0]


class JsonLayer(unittest.TestCase):
    def test_escapes_match_serde_json(self):
        s = 'q"b\\ \b\f\n\r\t \x00\x01\x1f \x7f é   \U0001f600 /'
        self.assertEqual(
            C.dumps([s]),
            '["q\\"b\\\\ \\b\\f\\n\\r\\t \\u0000\\u0001\\u001f \x7f é   \U0001f600 /"]',
        )

    def test_compact_ordered_and_float_free(self):
        self.assertEqual(C.dumps({"rec": "end", "files": 2, "objects": -1}), '{"rec":"end","files":2,"objects":-1}')
        self.assertEqual(C.dumps([2**64 - 1, -(2**63)]), "[18446744073709551615,-9223372036854775808]")
        with self.assertRaises(ValueError):
            C.dumps([float("nan")])


class Scalars(unittest.TestCase):
    def test_str_valid_and_invalid(self):
        self.assertEqual(C.enc_str("plain"), "plain")
        self.assertEqual(C.enc_str("héllo"), "héllo")
        self.assertEqual(C.enc_str(""), "")
        bad = b"a\xffb".decode("utf-8", "surrogateescape")
        self.assertEqual(C.enc_str(bad), {"t": "str", "hex": "61ff62"})
        # Encoded surrogates and overlong forms are invalid UTF-8 in Rust too.
        for raw in (b"\xed\xa0\x80", b"\xc0\xaf"):
            self.assertEqual(C.enc_str(raw.decode("utf-8", "surrogateescape")), {"t": "str", "hex": raw.hex()})

    def test_raw_rule_threshold(self):
        at = bytes(range(64))
        over = bytes(range(65))
        self.assertEqual(C.enc_raw("bytes", at), {"t": "bytes", "hex": at.hex()})
        self.assertEqual(
            C.enc_raw("bytes", over), {"t": "bytes", "len": 65, "sha256": hashlib.sha256(over).hexdigest()}
        )
        long_bad = (b"\xff" * 65).decode("utf-8", "surrogateescape")
        self.assertEqual(C.enc_str(long_bad)["len"], 65)
        self.assertEqual(C.enc_name(long_bad), {"t": "str", "hex": "ff" * 65}, "names are never summarised")

    def test_lone_surrogate_is_an_error(self):
        with self.assertRaises(C.EncodeError):
            C.enc_str("\ud800")

    def test_f32_bits(self):
        cases = {
            0x3F800000: "3f800000",
            0x80000000: "80000000",  # -0.0
            0x7F800000: "7f800000",
            0xFF800000: "ff800000",
            0x00000001: "00000001",  # smallest subnormal
            0x7F7FFFFF: "7f7fffff",
            0x7FC00000: "7fc00000",
            0xFFC12345: "ffc12345",  # quiet NaN, sign and payload
        }
        for bits, want in cases.items():
            self.assertEqual(C.f32_hex([widened_f32(bits)]), want)

    def test_f32_rejects_values_float32_cannot_hold(self):
        for bad in ([0.1], [1e39], [1], [True]):
            with self.assertRaises(C.EncodeError, msg=bad):
                C.f32_hex(bad)

    def test_f64_bits_keep_signalling_nan(self):
        self.assertEqual(C.f64_hex([f64_of(0x7FF0000000000001)]), "7ff0000000000001")
        self.assertEqual(C.f64_hex([-0.0, 1.0]), "80000000000000003ff0000000000000")

    def test_u8_bytes(self):
        self.assertEqual(C.u8_bytes([0, 127, 255]), b"\x00\x7f\xff")
        for bad in ([256], [-1], [True], [1.0]):
            with self.assertRaises(C.EncodeError, msg=bad):
                C.u8_bytes(bad)


class Values(unittest.TestCase):
    def enc(self, value, node, resolve=None):
        return C.ValueEncoder(resolve).encode(value, node)

    def test_golden_line(self):
        # Pinned bytes for a Rust producer: every tag in one record.
        root = Node(
            "Base",
            "Base",
            [
                Node("int", "i"),
                Node("bool", "b"),
                Node("float", "f"),
                Node("double", "d"),
                Node("string", "s"),
                Node("TypelessData", "td"),
                vec("fs", Node("float", "data")),
                vec("ds", Node("double", "data")),
                vec("u8", Node("UInt8", "data")),
                vec("c", Node("char", "data")),
                vec("s8", Node("SInt8", "data")),
                vec("empty", Node("float", "data")),
                vec("m", Node("pair", "data", [Node("string", "first"), Node("int", "second")])),
                Node("Inner", "t", [Node("int", "t")]),
            ],
        )
        value = {
            "i": -3,
            "b": False,
            "f": widened_f32(0x3F800000),
            "d": 0.5,
            "s": "név",
            "td": b"\x01\x02",
            "fs": [widened_f32(0x7FC00001), -0.0],
            "ds": [2.0],
            "u8": [1, 2],
            "c": [255],
            "s8": [-1, 1],
            "empty": [],
            "m": [("a", 1)],
            "t": {"t": 7},
        }
        line = C.dumps({"rec": "x", "value": self.enc(value, root)})
        self.assertEqual(
            line,
            '{"rec":"x","value":{"t":"dict","v":[["i",-3],["b",false],["f",{"t":"f32","v":"3f800000"}],'
            '["d",{"t":"f64","v":"3fe0000000000000"}],["s","név"],["td",{"t":"bytes","hex":"0102"}],'
            '["fs",{"t":"f32s","v":"7fc0000180000000"}],["ds",{"t":"f64s","v":"4000000000000000"}],'
            '["u8",{"t":"u8s","hex":"0102"}],["c",{"t":"u8s","hex":"ff"}],["s8",[-1,1]],["empty",[]],'
            '["m",[{"t":"tuple","v":["a",1]}]],["t",{"t":"dict","v":[["t",7]]}]]}}',
        )

    def test_dict_order_is_the_values_order(self):
        root = Node("Base", "Base", [Node("int", "a"), Node("int", "b")])
        self.assertEqual(self.enc({"b": 2, "a": 1}, root), {"t": "dict", "v": [["b", 2], ["a", 1]]})

    def test_repeated_name_is_typed_by_the_last_child(self):
        root = Node("Base", "Base", [Node("int", "dup"), Node("float", "dup")])
        self.assertEqual(self.enc({"dup": 2.0}, root)["v"], [["dup", {"t": "f32", "v": "40000000"}]])

    def test_list_and_tuple_stay_apart(self):
        pair = Node("pair", "p", [Node("int", "first"), Node("int", "second")])
        self.assertEqual(self.enc((1, 2), pair), {"t": "tuple", "v": [1, 2]})
        self.assertEqual(self.enc([1, 2], vec("v", Node("int", "data"))), [1, 2])
        # The pure-Python reader's tuple arrays stay tuples, element-typed and unpacked.
        self.assertEqual(
            self.enc((1.0,), vec("v", Node("float", "data"))), {"t": "tuple", "v": [{"t": "f32", "v": "3f800000"}]}
        )

    def test_bool_vectors_and_int_vectors_are_not_packed(self):
        self.assertEqual(self.enc([True, False], vec("v", Node("bool", "data"))), [True, False])
        self.assertEqual(self.enc([300], vec("v", Node("UInt16", "data"))), [300])

    def test_u8_vector_is_never_bytes(self):
        enc_list = self.enc([1], vec("v", Node("UInt8", "data")))
        enc_bytes = self.enc(b"\x01", Node("TypelessData", "td"))
        self.assertNotEqual(enc_list, enc_bytes)
        self.assertNotEqual(C.decode(enc_list), C.decode(enc_bytes))

    def test_mismatches_raise(self):
        with self.assertRaises(C.EncodeError):
            self.enc(1.0, Node("int"))
        with self.assertRaises(C.EncodeError):
            self.enc({"zz": 1}, Node("Base", "Base", [Node("int", "a")]))
        with self.assertRaises(C.EncodeError):
            self.enc([1], Node("Base"))
        with self.assertRaises(C.EncodeError):
            self.enc(object(), Node("int"))

    def test_referenced_object_data_uses_the_resolved_node(self):
        data_node = Node("Foo", "data", [Node("float", "x")])
        ref = Node(
            "ReferencedObject",
            "data",
            [
                Node("SInt64", "rid"),
                Node(
                    "ReferencedManagedType",
                    "type",
                    [Node("string", "class"), Node("string", "ns"), Node("string", "asm")],
                ),
                Node("ReferencedObjectData", "data"),
            ],
        )
        seen = []

        def resolve(t):
            seen.append(t)
            return data_node

        value = {"rid": 1, "type": {"class": "Foo", "ns": "", "asm": "A"}, "data": {"x": 1.0}}
        out = self.enc(value, ref, resolve)
        self.assertEqual(out["v"][2], ["data", {"t": "dict", "v": [["x", {"t": "f32", "v": "3f800000"}]]}])
        self.assertEqual(seen, [{"class": "Foo", "ns": "", "asm": "A"}])
        with self.assertRaises(C.EncodeError):
            self.enc(value, ref, None)


class Decoding(unittest.TestCase):
    def test_inline_and_summary_payloads_compare_equal(self):
        raw = bytes(range(10))
        inline = C.decode({"t": "bytes", "hex": raw.hex()})
        summary = C.decode({"t": "bytes", "len": 10, "sha256": hashlib.sha256(raw).hexdigest()})
        self.assertEqual(inline, summary)
        self.assertNotEqual(inline, C.decode({"t": "u8s", "hex": raw.hex()}))

    def test_packed_floats_unpack_to_scalars(self):
        self.assertEqual(
            C.decode({"t": "f32s", "v": "3f80000080000000"}),
            [C.decode({"t": "f32", "v": "3f800000"}), C.decode({"t": "f32", "v": "80000000"})],
        )
        self.assertIs(type(C.decode({"t": "f64s", "v": "3ff0000000000000"})[0]), C.F64)

    def test_kinds_survive(self):
        v = C.decode({"t": "dict", "v": [["a", {"t": "tuple", "v": [1, True]}], [{"t": "str", "hex": "ff"}, []]]})
        self.assertIs(type(v), C.Dict)
        self.assertIs(type(v[0][1]), C.Tuple)
        self.assertIs(type(v[0][1][1]), bool)
        self.assertEqual(v[1][0].kind, "str")

    def test_malformed(self):
        for bad in ({"t": "nope"}, {"t": "f32s", "v": "3f80"}, {"t": "dict"}, 1.5):
            with self.assertRaises(C.FormatError, msg=bad):
                C.decode(bad)


class CanonicalForm(unittest.TestCase):
    """check_value and check_record: the rules a single line can break."""

    def kinds(self, tv):
        out = []
        C.check_value(tv, "v", out)
        return [(path, kind) for path, kind, _ in out]

    def test_encoder_output_is_canonical(self):
        root = Node("Base", "Base", [Node("float", "f"), vec("fs", Node("float", "data")), Node("TypelessData", "b")])
        value = {"f": 1.0, "fs": [1.0, 2.0], "b": bytes(65)}
        self.assertEqual(self.kinds(C.ValueEncoder().encode(value, root)), [])
        bad_name = (b"\xff" * 70).decode("utf-8", "surrogateescape")
        self.assertEqual(self.kinds({"t": "dict", "v": [[C.enc_name(bad_name), C.enc_str(bad_name)]]}), [])

    def test_violations(self):
        h65 = bytes(65).hex()
        cases = [
            ([{"t": "f32", "v": "3f800000"}], [("v", "unpacked")]),
            ([{"t": "f64", "v": "3ff0000000000000"}, 1], [("v", "unpacked")]),
            ({"t": "tuple", "v": [{"t": "f32", "v": "3f800000"}]}, []),  # tuples are never packed
            (1.5, [("v", "json_float")]),
            ({"t": "f32", "v": "3F800000"}, [("v", "bad_hex")]),
            ({"t": "f32", "v": "3f80"}, [("v", "bad_hex")]),
            ({"t": "f64s", "v": "3ff00000"}, [("v", "bad_hex")]),
            ({"t": "f32s", "v": ""}, [("v", "empty_packed")]),
            ({"v": "3f800000", "t": "f32"}, [("v", "members")]),
            ({"t": "f32", "v": "3f800000", "x": 1}, [("v", "members")]),
            ({"t": "bytes", "hex": h65}, [("v", "inline_too_long")]),
            ({"t": "bytes", "len": 64, "sha256": "00" * 32}, [("v", "summary_too_short")]),
            ({"t": "bytes", "len": 65, "sha256": "AB" * 32}, [("v", "bad_sha256")]),
            ({"t": "bytes", "len": True, "sha256": "00" * 32}, [("v", "bad_len")]),
            ({"t": "bytes", "hex": "abc"}, [("v", "bad_hex")]),
            ({"t": "u8s", "hex": ""}, [("v", "empty_packed")]),
            ({"t": "str", "hex": "616263"}, [("v", "str_is_utf8")]),
            ({"t": "str", "hex": "ff"}, []),
            ({"t": "nope", "v": 1}, [("v", "unknown_tag")]),
            ({"t": ["dict"], "v": []}, [("v", "unknown_tag")]),
            ({"t": "dict", "v": [["a", 1], ["a", 2]]}, [("v.a", "duplicate_key")]),
            ({"t": "dict", "v": [[{"t": "str", "hex": "61"}, 1]]}, [("v.61", "hex_name_is_utf8")]),
            ({"t": "dict", "v": [[1, 2]]}, [("v", "bad_key")]),
            ({"t": "dict", "v": [["a"]]}, [("v", "malformed")]),
            ({"t": "dict", "v": [["a", [{"t": "f32", "v": "00000000"}]]]}, [("v.a", "unpacked")]),
            ({"t": "dict", "v": [["a", {"t": "tuple", "v": [1.0]}]]}, [("v.a[0]", "json_float")]),
        ]
        for tv, want in cases:
            with self.subTest(tv=tv):
                self.assertEqual(self.kinds(tv), want)

    def test_record_members(self):
        end = {"rec": "end", "files": 0, "objects": 0}
        self.assertEqual(C.check_record(end, []), [])
        self.assertEqual([k for _, k, _ in C.check_record({"rec": "end", "objects": 0, "files": 0}, [])], ["members"])
        obj = {
            "rec": "object",
            "file": ["a"],
            "path_id": 1,
            "l1": None,
            "l2": {
                "root_type": "T",
                "source": "tpk",
                "error": {"kind": "other", "bytes_read": None, "note": "x"},
                "value": None,
            },
            "l3": None,
        }
        self.assertEqual(C.check_record(obj, ["L0", "L2"]), [])
        self.assertEqual([(p, k) for p, k, _ in C.check_record(obj, ["L0"])], [("l2", "undeclared_level")])
        obj["l2"]["error"] = {"note": "x", "kind": "other", "bytes_read": None}
        self.assertEqual([(p, k) for p, k, _ in C.check_record(obj, ["L0", "L2"])], [("l2.error", "members")])


if __name__ == "__main__":
    unittest.main()
