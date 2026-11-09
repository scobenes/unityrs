"""W2's write matrix and the writer's model of Python's attributes, which
`walk.py` records into `write_matrix.json` and `py_attrs.json`
(docs/plans/core.md 3.11).

The matrix writes `Base { <node> f }` from `{"f": value}` for every node of
`W2_NODES` and every value of `W2_VALUES`, with UnityPy's `write_typetree`
(`helpers/TypeTreeHelper.py:407-458`): the written bytes, or the class of the
exception it raised. A value is built here as Python holds it, and on the Rust
side (`tests/walk_fixtures.rs`) from its spec.

The `class_...` nodes over a value that is not a dict take the class branch's
`getattr(value, child._clean_name)` (`helpers/TypeTreeHelper.py:447-455`), the name cleaned by
UnityPyBoost's rule (its `TypeTreeNode`, `UnityPyBoost/TypeTreeHelper.cpp:28-65`);
review round 16 found most of that branch pinned by nothing. Each of these
pins one of its rules: the `(int&)` prefix and the trailing `?` dropped, each
of ` .:-[]` cleaned to `_`, the attributes of a run (a `list`), of `bytes` and
of a `float` (`real`, an `F32`'s and an `F64`'s, from the value and from an
element of a run), and the registry scope each class ends. The values added
with them pin what the writer makes of a foreign value: a `str`'s characters
(an invalid UTF-8 sequence is one character a byte), a double narrowed to a
float (a signalling NaN quieted), an `int` widened to a double, and `-0.0`
false. `ref_object_data_only` pins which exception a `ReferencedObject`'s
data raises first: its `type` key (`KeyError`), or a `type` that is not a
dict (`AttributeError`, `getattr(typ, "class")`; `helpers/TypeTreeHelper.py:88-100`),
before the file's `ref_types`;
`pair_one_child` and `vector_no_data` the `IndexError` of a node missing a
child the writer takes.

`walk.py` imports this module; it records nothing when run on its own.
"""

from __future__ import annotations

import struct
from types import SimpleNamespace

from walk_build import ALIGN, base, leaf, pair, shift, tree, vec, write

U8 = lambda name, meta=0: leaf("UInt8", name, meta)  # noqa: E731
INT = lambda name, meta=0: leaf("int", name, meta)  # noqa: E731


def holds(child):
    """`Foo f` holding the one child `child` (rows rooted at level 0)."""
    return [[0, "Foo", "f", 0], *shift(child, 1)]


def registry_class(name):
    """`Bar <name> { ManagedReferencesRegistry real { int real } }`."""
    return [[0, "Bar", name, 0], [1, "ManagedReferencesRegistry", "real", 0], [2, "int", "real", 0]]


W2_NODES = {
    "SInt8": leaf("SInt8", "f"), "UInt8": leaf("UInt8", "f"), "char": leaf("char", "f"),
    "SInt16": leaf("SInt16", "f"), "UInt16": leaf("UInt16", "f"), "SInt32": leaf("SInt32", "f"),
    "UInt32": leaf("UInt32", "f"), "SInt64": leaf("SInt64", "f"), "UInt64": leaf("UInt64", "f"),
    "float": leaf("float", "f"), "double": leaf("double", "f"), "bool": leaf("bool", "f"),
    "string": leaf("string", "f"), "TypelessData": [[0, "TypelessData", "f", 0]],
    "pair": pair("f", INT("first"), leaf("string", "second")), "pair_u8": pair("f", U8("first"), U8("second")),
    "vector_int": vec("f", INT("data")), "vector_string": vec("f", leaf("string", "data")),
    "vector_aligned_u8": vec("f", U8("data", ALIGN)),
    "class_empty": [[0, "Foo", "f", 0]], "class_real": [[0, "Foo", "f", 0], [1, "SInt64", "real", 0]],
    "class_count": [[0, "Foo", "f", 0], [1, "bool", "count", 0]], "class_imag_float": [[0, "Foo", "f", 0], [1, "double", "imag", 0]],
    "class_numerator": [[0, "Foo", "f", 0], [1, "int", "numerator", 0], [1, "int", "denominator", 0]],
    "class_nested_attr": [[0, "Foo", "f", 0], [1, "Bar", "real", 0], [2, "int", "imag", 0]],
    "class_missing": [[0, "Foo", "f", 0], [1, "int", "nope", 0]],
    "class_method_attr": [[0, "Foo", "f", 0], [1, "Bar", "count", 0], [2, "int", "x", 0]],
    # Review round 16: the class branch over a value that is not a dict.
    "class_int_ref_real": holds(leaf("SInt64", "(int&)real")),
    "class_real_question": holds(leaf("SInt64", "real?")),
    "class_clean_space": holds([[0, "Bar", "bit length", 0]]),
    "class_clean_dot": holds([[0, "Bar", "bit.count", 0]]),
    "class_clean_colon": holds([[0, "Bar", "to:bytes", 0]]),
    "class_clean_dash": holds([[0, "Bar", "from-bytes", 0]]),
    "class_clean_open": holds([[0, "Bar", "as[integer_ratio", 0]]),
    "class_clean_close": holds([[0, "Bar", "is]integer", 0]]),
    "class_append": holds([[0, "Bar", "append", 0]]),
    "class_casefold": holds([[0, "Bar", "casefold", 0]]),
    "class_real_float": holds(leaf("float", "real")),
    "class_nested_real_float": holds([[0, "Bar", "real", 0], [1, "float", "real", 0]]),
    "vector_class_real_float": vec("f", [[0, "Bar", "data", 0], [1, "float", "real", 0]]),
    "class_two_registries": holds(registry_class("real") + registry_class("numerator")),
    "pair_bool": pair("f", leaf("bool", "first"), leaf("bool", "second")),
    "vector_bool": vec("f", leaf("bool", "data")),
    "ref_object_data_only": [[0, "ReferencedObject", "f", 0], [1, "ReferencedObjectData", "data", 0]],
    "pair_one_child": [[0, "pair", "f", 0], [1, "int", "first", 0]],
    "vector_no_data": [[0, "vector", "f", 0], [1, "Array", "Array", 0], [2, "int", "size", 0]],
}


def f32(bits):
    return struct.unpack(">f", bytes.fromhex(bits))[0]


def f64(bits):
    value = struct.unpack(">d", bytes.fromhex(bits))[0]
    assert struct.pack(">d", value).hex() == bits, bits
    return value


# (name, how the Rust side builds it, the Python value)
W2_VALUES = [
    ("bool_true", {"bool": True}, True), ("bool_false", {"bool": False}, False),
    ("int_0", {"int": 0}, 0), ("int_5", {"int": 5}, 5), ("int_m1", {"int": -1}, -1), ("int_300", {"int": 300}, 300),
    ("int_70000", {"int": 70000}, 70000), ("int_2_40", {"int": 2**40}, 2**40), ("int_min", {"int": -(2**63)}, -(2**63)),
    ("uint_big", {"uint": 2**63 + 5}, 2**63 + 5),
    ("f32_1_5", {"f32": "3fc00000"}, 1.5), ("f32_snan_quiet", {"f32": "7fc00001"}, f32("7fc00001")),
    ("f64_1e300", {"f64": struct.pack(">d", 1e300).hex()}, 1e300), ("f64_m0", {"f64": struct.pack(">d", -0.0).hex()}, -0.0),
    ("f64_0_1", {"f64": struct.pack(">d", 0.1).hex()}, 0.1), ("f64_inf", {"f64": struct.pack(">d", float("inf")).hex()}, float("inf")),
    ("str_ab", {"str": "6162"}, "ab"), ("str_empty", {"str": ""}, ""), ("str_invalid", {"str": "61ff62"}, b"a\xffb".decode("utf8", "surrogateescape")),
    ("str_multibyte", {"str": "c3a9e282ac"}, "é€"),
    ("bytes_12", {"bytes": "0102"}, b"\x01\x02"), ("bytes_empty", {"bytes": ""}, b""), ("bytes_big", {"bytes": "ff00ff"}, b"\xff\x00\xff"),
    ("prims_u8", {"prims_u8": "0102"}, [1, 2]), ("prims_i32", {"prims_i32": [-1, 7]}, [-1, 7]), ("prims_f32", {"prims_f32": ["3f800000"]}, [1.0]),
    ("prims_bool", {"prims_bool": [True, False]}, [True, False]), ("prims_empty", {"prims_i32": []}, []),
    ("list_ints", {"list": [{"int": 1}, {"int": 2}]}, [1, 2]), ("list_strs", {"list": [{"str": "78"}, {"str": "7979"}]}, ["x", "yy"]),
    ("list_empty", {"list": []}, []),
    ("tuple_ints", {"tuple": [{"int": 3}, {"str": "6869"}]}, (3, "hi")), ("tuple_u8s", {"tuple": [{"int": 1}, {"int": 2}]}, (1, 2)),
    ("dict_a", {"dict": [["a", {"int": 1}]]}, {"a": 1}), ("dict_empty", {"dict": []}, {}),
    ("dict_real", {"dict": [["real", {"int": 7}], ["x", {"int": 8}]]}, {"real": 7, "x": 8}),
    # Review round 16: foreign values the writer converts.
    ("f32_m0", {"f32": "80000000"}, f32("80000000")),
    ("f64_snan", {"f64": "7ff0000020000000"}, f64("7ff0000020000000")),
    ("int_2_24_1", {"int": 2**24 + 1}, 2**24 + 1),
    ("str_invalid_2", {"str": "e282"}, b"\xe2\x82".decode("utf8", "surrogateescape")),
    ("prims_f32_m0", {"prims_f32": ["80000000"]}, [f32("80000000")]),
    ("dict_type_int", {"dict": [["type", {"int": 5}]]}, {"type": 5}),
]


def write_matrix():
    """`Base { <node> f }` written from `{"f": value}`, for every pair."""
    cases = []
    for kname, rows in W2_NODES.items():
        root = tree(base(rows))
        for vname, _spec, value in W2_VALUES:
            cases.append([kname, vname, write({"f": value}, root, "<", SimpleNamespace(ref_types=[]))])
    nodes = {k: base(rows) for k, rows in W2_NODES.items()}
    return {"nodes": nodes, "values": {name: spec for name, spec, _ in W2_VALUES}, "cases": cases}


def py_attrs():
    out = {}
    for t, sample in ((int, 5), (bool, True), (float, 1.5), (str, "ab"), (bytes, b"ab"), (list, [1]), (tuple, (1,))):
        names = [a for a in dir(t) if not a.startswith("__")]
        out[t.__name__] = [[a, "method" if callable(getattr(sample, a)) else type(getattr(sample, a)).__name__] for a in names]
    return out
