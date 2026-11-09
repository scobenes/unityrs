"""Oracle fixtures for package B: the typetree readers and the writer (docs/plans/core.md 7.3).

Runs in the oracle image against UnityPy 1.25.2 with UnityPyBoost and writes
small JSON files under crates/unityrs-core/tests/fixtures/walk/, which
`cargo test -p unityrs-core --test walk_fixtures` replays without Python. Every
input is built here in code (the builders and the outcome encoders are
`walk_build.py`'s); no file is read.

    scripts/dev python tools/fixtures/walk.py           # write the fixtures
    scripts/dev python tools/fixtures/walk.py --check   # regenerate and compare
    scripts/dev python tools/fixtures/walk.py --boost-reach   # a survey, below

Files:
- `cases.json`: each case a typetree, its bytes and its ref types, read by
  UnityPyBoost's `read_typetree` (over the bytes from `start`) and by the
  pure-Python `read_value` (over all the bytes, from `start`), each value
  re-saved by `write_typetree` (against the case's `write_ref_types` when it
  has them, else its own). Case names start with the rule they pin
  (`r09_...`, `py_...`, `w5_...`; `mirror_...`, `walk_mirror.py`'s).
- `value_cases.json`: the 32 value cases of `tools/parity/value_cases.py` (G2), read as
  `parse_as_dict` reads them (`read_typetree` with `byte_size` and
  `check_read`), with UnityPy's L2 outcome and its L3 re-save.
- `write_matrix.json`: W2, every node kind written from every value variant;
  `py_attrs.json`: the non-dunder attributes of each Python type a value
  becomes, and which are data, for the writer's `getattr` (W2). Both from
  `walk_write.py`.
- `depth.json`: deep values (plan 3.10, "Bounds"): each shape of
  `DEPTH_SHAPES` (one kind of level nested, or every kind after every other)
  read by both readers and re-saved, at the least depth where
  the pure-Python reader raises `RecursionError`, one `A` less, and a quarter
  of it; the outcome only (`pos` or the exception), not the value.
- `elements.json`: arrays at and past the zero-size element bound, from
  `walk_elements.py`; `runs.json`: primitive runs of 2^26 + 1 elements,
  past the values bound, from `walk_runs.py` (each module's docs give its
  recipe and encoding).
- `reentry.json`: reads that hold many values open at once to the end of
  the bytes, from `walk_reentry.py` (likewise).

`--boost-reach` writes no fixture: for each depth shape it prints the most
`A`s UnityPyBoost reads before the C stack of the process's main thread
overflows (each try in a child process), which depends on that stack's size
(`ulimit -s`). `unityrs dump --unbounded ValueDepth` must read that deep
(`unityrs_dump::LIFTED_VALUE_DEPTH`).

A value is written with the Python types it has, so a test compares exactly
what UnityPy returned: a `bool` as JSON `true`/`false`, an `int` as a number,
a `float` as `{"f": <its double's 16 hex digits>}`, a `str` as `{"s": <hex of
its surrogate-escaped bytes>}`, `bytes` as `{"y": <hex>}`, a `list` as a JSON
array, a `tuple` as `{"t": [...]}` and a `dict` as `{"d": [[key, value], ...]}`
in insertion order. An outcome is `{"ok": value, "pos": n}` (`pos` the bytes
read) or `{"err": exception class, "kind": the dump's L2 kind}`; a write is
`{"hex": bytes}` or `{"err": exception class}`.
"""

from __future__ import annotations

import argparse
import json
import struct
import sys
from pathlib import Path
from types import SimpleNamespace

import UnityPy
from UnityPy.helpers import TypeTreeHelper as TTH
from UnityPy.streams import EndianBinaryReader, EndianBinaryWriter
from UnityPy.UnityPyBoost import read_typetree as boost_read

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-core/tests/fixtures/walk"
sys.path.insert(0, str(ROOT / "tools/parity"))
sys.path.insert(0, str(ROOT / "tools/fixtures"))

import child  # noqa: E402
from unitypy_dump import error_kind  # noqa: E402
from walk_build import (  # noqa: E402
    ALIGN,
    INT_TREE,
    LOOKALIKE_CLASSES,
    REF,
    assets,
    base,
    enc,
    err,
    i32,
    leaf,
    pair,
    ref_bytes,
    ref_object,
    run_case,
    s,
    shift,
    tree,
    vec,
    write,
)
from walk_elements import element_cases  # noqa: E402
from walk_mirror import mirror_cases  # noqa: E402
from walk_reentry import reentry_cases, reentry_ok_cases  # noqa: E402
from walk_runs import run_cases  # noqa: E402
from walk_write import py_attrs, write_matrix  # noqa: E402


# --- R cases

U8 = lambda name, meta=0: leaf("UInt8", name, meta)  # noqa: E731
INT = lambda name, meta=0: leaf("int", name, meta)  # noqa: E731


def type_rows(cls_rows):
    """A ReferencedObject whose `type` holds the given rows (at level 0 under it)."""
    return [
        [0, "ReferencedObject", "obj", 0],
        [1, "SInt64", "rid", 0],
        [1, "ReferencedManagedType", "type", 0],
        *shift(cls_rows, 2),
        [1, "ReferencedObjectData", "data", 0],
    ]


def non_str_class_cases():
    """R14: a `class` of every kind the reader makes, zero or empty and not."""
    ns_asm = [[0, "string", "ns", 0], [0, "string", "asm", 0]]
    tail = s("N") + s("A") + i32(9)
    kinds = [
        ("int_0", leaf("int", "class"), i32(0)),
        ("int_5", leaf("int", "class"), i32(5)),
        ("int_m1", leaf("int", "class"), i32(-1)),
        ("uint64_big", leaf("UInt64", "class"), struct.pack("<Q", 2**63 + 5)),
        ("bool_false", leaf("bool", "class", ALIGN), b"\0\0\0\0"),
        ("bool_true", leaf("bool", "class", ALIGN), b"\x01\0\0\0"),
        ("float_p0", leaf("float", "class"), struct.pack("<f", 0.0)),
        ("float_m0", leaf("float", "class"), struct.pack("<f", -0.0)),
        ("float_1", leaf("float", "class"), struct.pack("<f", 1.0)),
        ("double_p0", leaf("double", "class"), struct.pack("<d", 0.0)),
        ("double_m0", leaf("double", "class"), struct.pack("<d", -0.0)),
        ("double_1", leaf("double", "class"), struct.pack("<d", 1.0)),
        ("bytes_empty", [[0, "TypelessData", "class", ALIGN]], i32(0)),
        ("bytes_1", [[0, "TypelessData", "class", ALIGN]], i32(1) + b"\x07\0\0\0"),
        ("vector_empty", vec("class", leaf("int", "data")), i32(0)),
        ("vector_1", vec("class", leaf("int", "data")), i32(1, 3)),
        ("vector_str_empty", vec("class", leaf("string", "data")), i32(0)),
        ("vector_str_1", vec("class", leaf("string", "data")), i32(1) + s("x")),
        ("class_empty", [[0, "Foo", "class", 0]], b""),
        ("class_1", [[0, "Foo", "class", 0], [1, "int", "x", 0]], i32(4)),
        ("pair", pair("class", leaf("int", "first"), leaf("int", "second")), i32(1, 2)),
    ]
    out = []
    for name, rows, body in kinds:
        nodes = base(type_rows(rows + ns_asm), INT("tail"))
        out.append(
            dict(name=f"r14_class_{name}", nodes=nodes, hex=(struct.pack("<q", 1) + body + tail).hex(), ref_types=REF)
        )
    return out


def ref_type_cases():
    """R14, review round 16: a `type` read again, which the next data child resolves (`TypeTreeHelper.cpp:760`,
    `TypeTreeHelper.py:200-201`); and ref types told apart by their assembly alone, scanned and, past the scan,
    through unityrs's table of first indices."""
    c = []
    add = lambda name, nodes, data, **kw: c.append(dict(name=name, nodes=nodes, hex=data.hex(), **kw))  # noqa: E731
    again = [*ref_object(), [1, "ReferencedManagedType", "type", 0], [2, "string", "class", 0], [2, "string", "ns", 0],
             [2, "string", "asm", 0], [1, "ReferencedObjectData", "data2", 0]]
    refs = REF + [{"class": "B", "ns": "N", "asm": "A", "nodes": [[0, "D2", "Base", 0], [1, "double", "y", 0]]}]
    head = ref_bytes("C") + i32(42)
    add("r14_type_read_again", base(again, INT("tail")), head + s("B") + s("N") + s("A") + struct.pack("<d", 2.5) + i32(9), ref_types=refs)
    add("r14_type_read_again_empty_class", base(again, INT("tail")), head + s("") + s("N") + s("A") + i32(9), ref_types=refs)
    add("r14_type_read_again_unknown", base(again, INT("tail")), head + s("X") + s("N") + s("A") + i32(9), ref_types=refs)
    add("r14_type_read_again_aligned_element", base(vec("RefIds", [[r[0], r[1], r[2], ALIGN if r[0] == 0 else 0] for r in again]), INT("tail")),
        i32(1) + head + s("B") + s("N") + s("A") + struct.pack("<d", 2.5) + i32(9), ref_types=refs)
    by_asm = [dict(REF[0], asm="A1"), {"class": "C", "ns": "N", "asm": "A2", "nodes": [[0, "D2", "Base", 0], [1, "double", "y", 0]]}]
    add("r14_assembly_tells_apart", base(ref_object(), INT("tail")), ref_bytes("C", asm="A2") + struct.pack("<d", 2.5) + i32(9), ref_types=by_asm)
    add("r14_assembly_unknown", base(ref_object(), INT("tail")), ref_bytes("C", asm="A3") + i32(9), ref_types=by_asm)
    # Twelve ref types: the first lookup (`E`) scans them all, so the next is the table's.
    twelve = [{"class": f"C{i}", "ns": "N", "asm": "A", "nodes": INT_TREE} for i in range(9)] + by_asm + [{"class": "E", "ns": "N", "asm": "A", "nodes": INT_TREE}]
    e = ref_bytes("E") + i32(5)
    add("r14_assembly_tells_apart_past_the_scan", base(vec("RefIds", ref_object("data")), INT("tail")),
        i32(2) + e + ref_bytes("C", asm="A2", rid=2) + struct.pack("<d", 2.5) + i32(9), ref_types=twelve)
    add("r14_assembly_unknown_past_the_scan", base(vec("RefIds", ref_object("data")), INT("tail")),
        i32(2) + e + ref_bytes("C", asm="A3", rid=2) + i32(9), ref_types=twelve)
    return c


def r_cases():
    c = []
    add = lambda name, nodes, data, **kw: c.append(dict(name=name, nodes=nodes, hex=data.hex(), **kw))  # noqa: E731

    # R1: kind by exact match, PPtr< prefix, else a class.
    add("r01_every_primitive_name", base(*[leaf(t, f"f{i}") for i, t in enumerate(PRIMS)]), PRIM_BYTES)
    add("r01_unknown_and_pptr_are_classes", base([[0, "PPtr<Foo>", "p", 0], [1, "int", "m_FileID", 0], [1, "SInt64", "m_PathID", 0]], [[0, "Unknownish", "u", 0], [1, "UInt8", "a", 0]], [[0, "int ", "near", 0], [1, "UInt8", "b", 0]]), i32(1) + struct.pack("<q", 2) + b"\x05\x06")
    # R2: every read bounds-checked. An empty run or string after an alignment past the end fails at its count,
    # checked first (review round 16: no check of 0 bytes is ever the one that fails).
    add("r02_empty_run_count_past_end_after_align", base(U8("a", ALIGN), vec("v", leaf("int", "data"))), b"\x01\0\0")
    add("r02_empty_string_count_past_end_after_align", base(U8("a", ALIGN), leaf("string", "s")), b"\x01\0\0")
    add("r02_read_past_end", base(INT("a"), INT("b")), i32(1) + b"\0\0")
    add("r02_u8_past_end", base(INT("a"), U8("b")), i32(1))
    add("r02_bool_past_end", base(INT("a"), leaf("bool", "b")), i32(1))
    add("r02_bytes_past_end", base([[0, "TypelessData", "t", 0]]), i32(5) + b"ab")
    # R3: align from the object's start, no bounds check.
    add("r03_align_past_end", base(U8("a", ALIGN)), b"\x01")
    add("r03_align_within", base(U8("a", ALIGN), U8("b")), b"\x01\0\0\0\x02")
    # R4: bool any non-zero byte; char a u8; Type* u32; FileSize u64.
    add("r04_bool_bytes", base(leaf("bool", "a"), leaf("bool", "b"), leaf("bool", "c")), b"\x00\x02\xff")
    add("r04_char_typeptr_filesize", base(leaf("char", "c"), leaf("Type*", "t", ALIGN), leaf("FileSize", "f")), b"\xfe\0\0\0" + struct.pack("<IQ", 7, 2**64 - 2))
    # R5: float32 widening, sNaN quieted.
    add("r05_float_snan", base(leaf("float", "f"), leaf("float", "g")), struct.pack("<II", 0x7F800001, 0xFFBFFFFF))
    add("r05_float_values", base(leaf("float", "f"), leaf("double", "d"), vec("v", leaf("float", "data"))), struct.pack("<fd", -1.25, 2.5) + i32(2) + struct.pack("<II", 0x7F800001, 0x3F800000))
    # R6: strings always aligned; negative length before the cursor moves.
    add("r06_string_always_aligned", base(leaf("string", "s"), U8("c")), s("ab")[:6] + b"\0\0\x09")
    add("r06_empty_string", base(U8("a"), leaf("string", "s"), U8("c")), b"\x01" + i32(0) + b"\0\0\0\x09")
    add("r06_negative_string", base(leaf("string", "s")), i32(-5) + b"abcd")
    add("r06_invalid_utf8", base(leaf("string", "s")), s(b"\xff\xfe\xed\xa0\x80"))
    add("r06_string_with_align_flag", base(leaf("string", "s", ALIGN), U8("c")), i32(1) + b"x\0\0\0\x09")
    # R7: TypelessData aligned only by its own flag.
    add("r07_bytes_unaligned", base([[0, "TypelessData", "t", 0], [1, "int", "size", 0], [1, "UInt8", "data", 0]], U8("c")), i32(1) + b"\x07\x08")
    add("r07_bytes_aligned", base([[0, "TypelessData", "t", ALIGN], [1, "int", "size", 0], [1, "UInt8", "data", 0]], U8("c")), i32(1) + b"\x07\0\0\0\x08")
    add("r07_bytes_negative", base([[0, "TypelessData", "t", 0]]), i32(-1) + b"abcd")
    # R8: pair arity.
    add("r08_pair_one_child", base([[0, "pair", "p", 0], [1, "int", "first", 0]]), i32(1, 2))
    add("r08_pair_three_children", base([[0, "pair", "p", 0], [1, "int", "a", 0], [1, "int", "b", 0], [1, "int", "c", 0]]), i32(1, 2, 3))
    add("r08_pair_none", base([[0, "pair", "p", 0]]), i32(1))
    add("r08_pair_aligned", base(pair("p", U8("first"), U8("second"), ALIGN), U8("c")), b"\x01\x02\0\0\x03")
    # R9: the switch on the node's own kind first.
    arr = lambda: [[1, "Array", "Array", 0], [2, "int", "size", 0], [2, "UInt8", "data", 0]]  # noqa: E731
    add("r09_string_with_array_child", base([[0, "string", "s", 0], *arr()]), s("abc"))
    add("r09_int_with_array_child", base([[0, "int", "i", 0], *arr()]), i32(7))
    add("r09_bytes_with_array_child", base([[0, "TypelessData", "t", 0], *arr()]), i32(2) + b"ab")
    add("r09_pair_with_array_child", base([[0, "pair", "p", 0], *arr(), [1, "int", "second", 0]]), i32(1) + b"\x05" + b"\0\0\0" + i32(9))
    add("r09_unknown_with_array_child", base([[0, "Foo", "f", 0], *arr()]), i32(2) + b"\x01\x02")
    add("r09_pptr_with_array_child", base([[0, "PPtr<X>", "p", 0], *arr()]), i32(1) + b"\x01")
    add("r09_array_type_with_array_child", base([[0, "Array", "a", 0], *arr()]), i32(1) + b"\x01")
    add("r09_registry_type_with_array_child", base([[0, "ManagedReferencesRegistry", "r", 0], *arr()]), i32(1) + b"\x01")
    add("r09_data_type_with_array_child", base([[0, "ReferencedObjectData", "d", 0], *arr()]), i32(1) + b"\x01")
    add("r09_array_one_child", base([[0, "vector", "v", 0], [1, "Array", "Array", 0], [2, "int", "size", 0]]), i32(1, 2))
    add("r09_array_three_children", base([[0, "vector", "v", 0], [1, "Array", "Array", 0], [2, "int", "size", 0], [2, "int", "data", 0], [2, "int", "extra", 0]]), i32(2, 5, 6))
    add("r09_negative_length", base(vec("v", leaf("int", "data"))), i32(-2))
    add("r09_array_flag_aligns_whole", base(vec("v", U8("data"), array_meta=ALIGN), U8("c")), i32(1) + b"\x05\0\0\0\x09")
    add("r09_vector_flag_aligns_whole", base(vec("v", U8("data"), meta=ALIGN), U8("c")), i32(1) + b"\x05\0\0\0\x09")
    # R10: runs.
    add("r10_empty_array_of_3_child_pair", base(vec("v", [[0, "pair", "data", 0], [1, "int", "a", 0], [1, "int", "b", 0], [1, "int", "c", 0]])), i32(0))
    add("r10_empty_array_of_1_child_pair", base(vec("v", [[0, "pair", "data", 0], [1, "int", "a", 0]])), i32(0))
    add("r10_run_aligned_once", base(vec("v", leaf("SInt16", "data", ALIGN)), U8("c")), i32(3) + struct.pack("<3h", -1, 2, -3) + b"\0\0\x09")
    add("r10_every_run_kind", base(*[vec(f"v{i}", leaf(t, "data")) for i, t in enumerate(PRIMS)]), RUN_BYTES)
    add("r10_run_past_end", base(vec("v", leaf("int", "data"))), i32(3, 1, 2))
    add("r10_u8_run_past_end", base(vec("v", U8("data"))), i32(3) + b"\x01")
    add("r10_pair_run_children_align", base(vec("v", pair("data", U8("first", ALIGN), U8("second")))), i32(2) + b"\x01\0\0\0\x02\x03\0\0\0\x04")
    add("r10_big_endian_runs", base(vec("a", leaf("UInt16", "data")), vec("b", leaf("double", "data"))), i32(2, e=">") + struct.pack(">HH", 1, 2) + i32(1, e=">") + struct.pack(">d", 1.5), endian=">")
    # R11: other elements one by one.
    add("r11_strings_each_aligned", base(vec("v", leaf("string", "data"))), i32(2) + s("a") + s("bcd"))
    add("r11_struct_elements_align", base(vec("v", [[0, "Foo", "data", ALIGN], [1, "UInt8", "x", 0]])), i32(2) + b"\x01\0\0\0\x02\0\0\0")
    add("r11_nested_vectors", base(vec("v", vec("data", leaf("int", "data")))), i32(2, 1, 5, 0))
    add("r11_bytes_elements", base(vec("v", [[0, "TypelessData", "data", 0]])), i32(2, 1) + b"\x05" + i32(0))
    # R12: dicts with repeated names.
    add("r12_duplicate_names", base(INT("a"), INT("b"), U8("a"), leaf("float", "b")), i32(1, 2) + b"\x03" + struct.pack("<f", 4.0))
    add("r12_duplicate_after_vector", base(INT("x"), vec("v", U8("data")), INT("x")), i32(1, 2) + b"\x05\x06" + i32(3))
    # R13: the registry scope.
    reg = lambda name: [[0, "ManagedReferencesRegistry", name, 0], [1, "int", "version", 0]]  # noqa: E731
    add("r13_second_registry_skipped", base(reg("r1"), reg("r2"), INT("tail")), i32(1, 2, 3))
    add("r13_nested_registry_skipped", base([[0, "Foo", "f", 0], *shift(reg("r1"), 1), [1, "Bar", "inner", 0], *shift(reg("r2"), 2)], INT("tail")), i32(1, 2, 3))
    add("r13_scope_resets_after_class", base([[0, "Foo", "f", 0], *shift(reg("r1"), 1)], reg("r2"), INT("tail")), i32(1, 2, 3))
    add("r13_registry_dup_name_skipped", base(INT("r"), reg("r"), [[0, "Foo", "f", 0], *shift(reg("q"), 1), *shift(reg("r"), 1), *shift(INT("r"), 1)]), i32(1, 2, 3, 4, 5))
    add("r13_registry_array_elements", base(vec("v", [[0, "Foo", "data", 0], *shift(reg("r"), 1)]), reg("r2")), i32(2, 1, 2, 3))
    # R14: ReferencedObject.
    ok = ref_bytes("C") + i32(42) + i32(9)
    add("r14_match", base(ref_object(), INT("tail")), ok, ref_types=REF)
    add("r14_no_file", base(ref_object(), INT("tail")), ok, ref_types=None)
    add("r14_absent", base(ref_object(), INT("tail")), ok, ref_types="absent")
    add("r14_empty_list_match", base(ref_object(), INT("tail")), ok, ref_types=[])
    add("r14_no_match", base(ref_object(), INT("tail")), ref_bytes("D") + i32(9), ref_types=REF)
    add("r14_no_match_ns", base(ref_object(), INT("tail")), ref_bytes("C", ns="M") + i32(9), ref_types=REF)
    add("r14_empty_class_empty_list", base(ref_object(), INT("tail")), ref_bytes("") + i32(9), ref_types=[])
    add("r14_empty_class_list", base(ref_object(), INT("tail")), ref_bytes("") + i32(9), ref_types=REF)
    add("r14_empty_class_absent", base(ref_object(), INT("tail")), ref_bytes("") + i32(9), ref_types="absent")
    add("r14_empty_class_no_file", base(ref_object(), INT("tail")), ref_bytes("") + i32(9), ref_types=None)
    add("r14_matched_no_tree", base(ref_object(), INT("tail")), ref_bytes("C") + i32(9), ref_types=[dict(REF[0], nodes=None)])
    add("r14_second_ref_type_matches", base(ref_object(), INT("tail")), ref_bytes("C2") + struct.pack("<d", 2.5) + i32(9), ref_types=REF + [{"class": "C2", "ns": "N", "asm": "A", "nodes": [[0, "D2", "Base", 0], [1, "double", "y", 0]]}])
    # Two ref types of one (class, ns, asm), unequal trees: the first is the
    # match, in a short list (scanned) and in a list of twelve whose first
    # lookup scans all of it (past it, unityrs's RefIndex probes a table of
    # first indices).
    d_int = {"class": "D", "ns": "N", "asm": "A", "nodes": INT_TREE}
    d_double = {"class": "D", "ns": "N", "asm": "A", "nodes": [[0, "D2", "Base", 0], [1, "double", "y", 0]]}
    add("r14_first_of_a_repeated_ref_type", base(ref_object(), INT("tail")), ref_bytes("D") + i32(7) + i32(9), ref_types=[d_int, d_double])
    twelve = [{"class": f"C{i}", "ns": "N", "asm": "A", "nodes": INT_TREE} for i in range(9)] + [d_int, d_double, {"class": "E", "ns": "N", "asm": "A", "nodes": INT_TREE}]
    refs3 = ref_bytes("E") + i32(5) + ref_bytes("D", rid=2) + i32(7) + ref_bytes("D", rid=3) + i32(8)
    add("r14_first_of_a_repeated_ref_type_past_the_scan", base(vec("RefIds", ref_object("data")), INT("tail")), i32(3) + refs3 + i32(9), ref_types=twelve)
    no_type = [[0, "ReferencedObject", "obj", 0], [1, "SInt64", "rid", 0], [1, "ReferencedObjectData", "data", 0], [1, "ReferencedManagedType", "type", 0], [2, "string", "class", 0], [2, "string", "ns", 0], [2, "string", "asm", 0]]
    add("r14_no_type_yet", base(no_type), struct.pack("<q", 1) + s("C") + s("N") + s("A"), ref_types=REF)
    add("r14_no_type_yet_absent", base(no_type), struct.pack("<q", 1) + s("C") + s("N") + s("A"), ref_types="absent")
    for missing in ("class", "ns", "asm"):
        keep = [n for n in ("class", "ns", "asm") if n != missing]
        rows = type_rows([[0, "string", n, 0] for n in keep])
        add(f"r14_type_missing_{missing}", base(rows, INT("tail")), struct.pack("<q", 1) + s("C") + s("N") + i32(9), ref_types=REF)
        add(f"r14_type_missing_{missing}_absent", base(rows, INT("tail")), struct.pack("<q", 1) + s("C") + s("N") + i32(9), ref_types="absent")
    nondict = [[0, "ReferencedObject", "obj", 0], [1, "SInt64", "rid", 0], [1, "int", "type", 0], [1, "ReferencedObjectData", "data", 0]]
    add("r14_type_not_dict", base(nondict, INT("tail")), struct.pack("<q", 1) + i32(3) + i32(9), ref_types=REF)
    add("r14_type_not_dict_absent", base(nondict, INT("tail")), struct.pack("<q", 1) + i32(3) + i32(9), ref_types="absent")
    add("r14_nested_ref_reenters", base(ref_object(), INT("tail")), ref_bytes("Outer") + ref_bytes("C", rid=2) + i32(42) + i32(9), ref_types=REF + [{"class": "Outer", "ns": "N", "asm": "A", "nodes": [[0, "O", "Base", 0], *shift(ref_object("inner"), 1)]}])
    add("r14_array_of_ref_objects", base(vec("RefIds", ref_object("data")), INT("tail")), i32(2) + ref_bytes("C") + i32(1) + ref_bytes("") + i32(9), ref_types=REF)
    add("r14_aligned_array_element_empty_class", base(vec("RefIds", ref_object("data", meta=ALIGN)), INT("tail")), i32(1) + ref_bytes("") + i32(9), ref_types=REF)
    add("r14_aligned_array_element_no_tree", base(vec("RefIds", ref_object("data", meta=ALIGN)), INT("tail")), i32(1) + ref_bytes("C") + i32(9), ref_types=[dict(REF[0], nodes=None)])
    add("r14_aligned_array_element_match", base(vec("RefIds", ref_object("data", meta=ALIGN)), INT("tail")), i32(1) + ref_bytes("C") + i32(5) + i32(9), ref_types=REF)
    add("r14_registry_and_refids", base([[0, "ManagedReferencesRegistry", "references", 0], [1, "int", "version", 0], *shift(vec("RefIds", ref_object("data")), 1)], INT("tail")), i32(2, 1) + ref_bytes("C") + i32(3) + i32(9), ref_types=REF)
    c.extend(non_str_class_cases())
    c.extend(ref_type_cases())
    # R15: align after a flagged node (for an array, its Array), whatever other bits the flag holds (review round 16).
    add("r15_flagged_int_and_class", base(U8("a", ALIGN), [[0, "Foo", "f", ALIGN], [1, "UInt8", "x", 0]], U8("c")), b"\x01\0\0\0\x02\0\0\0\x03")
    add("r15_flag_beside_another_bit", base(U8("a", ALIGN | 0x8000), U8("c")), b"\x01\0\0\0\x03")
    add("r15_array_flag_beside_another_bit", base(vec("v", U8("data"), array_meta=ALIGN | 0x8000), U8("c")), i32(1) + b"\x05\0\0\0\x09")
    # R16: error kinds.
    add("r16_out_of_bounds", base(INT("a")), b"\x01")
    add("r16_negative_length", base(vec("v", INT("data"))), i32(-1))
    add("r16_ref_type", base(ref_object()), ref_bytes("X"), ref_types=REF)
    add("r16_other", base([[0, "pair", "p", 0]]), b"")
    for i, cls in enumerate(LOOKALIKE_CLASSES):
        add(f"r16_ref_type_named_like_another_kind_{i}", base(ref_object(), INT("tail")), ref_bytes(cls) + i32(9),
            ref_types=REF)
    return c


PRIMS = ["SInt8", "UInt8", "char", "short", "SInt16", "unsigned short", "UInt16", "int", "SInt32", "unsigned int",
         "UInt32", "Type*", "long long", "SInt64", "unsigned long long", "UInt64", "FileSize", "float", "double", "bool"]
PRIM_BYTES = (
    struct.pack("<bBB", -2, 254, 65)
    + struct.pack("<hhHH", -3, -4, 65535, 5)
    + struct.pack("<iiIII", -6, -7, 2**32 - 1, 8, 9)
    + struct.pack("<qqQQQ", -10, -11, 2**64 - 1, 12, 13)
    + struct.pack("<fd", 1.5, -2.5)
    + b"\x01"
)
RUN_BYTES = b"".join(
    i32(2) + struct.pack(f"<2{f}", *v)
    for f, v in [
        ("b", (-1, 2)), ("B", (255, 1)), ("B", (65, 66)), ("h", (-3, 3)), ("h", (-4, 4)), ("H", (65535, 1)),
        ("H", (2, 3)), ("i", (-5, 5)), ("i", (-6, 6)), ("I", (2**32 - 1, 7)), ("I", (8, 9)), ("I", (10, 11)),
        ("q", (-12, 12)), ("q", (-13, 13)), ("Q", (2**64 - 1, 14)), ("Q", (15, 16)), ("Q", (17, 18)),
        ("f", (1.5, -0.0)), ("d", (2.5, -3.5)), ("B", (0, 2)),
    ]
)


def py_cases():
    """The pure-Python reader's list (plan 3.10), each read by both readers."""
    c = []
    add = lambda name, nodes, data, **kw: c.append(dict(name=name, nodes=nodes, hex=data.hex(), **kw))  # noqa: E731
    add("py_pair_three_children", base([[0, "pair", "p", 0], [1, "int", "a", 0], [1, "int", "b", 0], [1, "int", "c", 0]], INT("t")), i32(1, 2, 3, 4))
    add("py_aligned_pair_array", base(vec("v", pair("data", INT("first"), U8("second"), ALIGN)), U8("c")), i32(2, 1) + b"\x02" + i32(3) + b"\x04" + b"\0\0\x09")
    add("py_ref_object_empty_ref_types", base(ref_object(), INT("tail")), ref_bytes("") + i32(9), ref_types=[])
    add("py_ref_object_absent_ref_types", base(ref_object(), INT("tail")), ref_bytes("") + i32(9), ref_types="absent")
    add("py_empty_string_between_u8s", base(U8("a"), leaf("string", "s"), U8("c")), b"\x01" + i32(0) + b"\x09\0\0\0")
    add("py_negative_string_length", base(leaf("string", "s"), U8("c")), i32(-1) + b"\x09")
    add("py_string_past_end", base(leaf("string", "s"), U8("c")), i32(9) + b"\x09")
    add("py_array_three_children", base([[0, "vector", "v", 0], [1, "Array", "Array", 0], [2, "int", "size", 0], [2, "int", "data", 0], [2, "UInt8", "extra", 0]]), i32(2, 5, 6))
    add("py_negative_inner_length_aligned_prim", base(vec("v", vec("data", leaf("int", "data", ALIGN), meta=ALIGN))), i32(1, -1))
    add("py_negative_inner_length_other", base(vec("v", vec("data", leaf("string", "data"), meta=ALIGN))), i32(1, -1, 9))
    add("py_negative_inner_length_unaligned_elem", base(vec("v", vec("data", leaf("int", "data")), meta=0)), i32(1, -1))
    add("py_aligned_vector_of_vectors", base(vec("v", vec("data", leaf("int", "data", ALIGN), meta=ALIGN))), i32(2, 1, 5, 2, 6, 7))
    add("py_aligned_vector_of_unaligned_vectors", base(vec("v", vec("data", leaf("int", "data"), meta=ALIGN))), i32(2, 1, 5, 0))
    add("py_aligned_class_elements", base(vec("v", [[0, "Foo", "data", ALIGN], [1, "UInt8", "x", 0], [1, "ManagedReferencesRegistry", "r", 0], [2, "int", "version", 0]]), U8("c")), i32(2) + b"\x01" + i32(3) + b"\x02" + i32(4) + b"\0\0\0\x09")
    add("py_aligned_class_elements_skip_registry_check", base([[0, "ManagedReferencesRegistry", "outer", 0], [1, "int", "version", 0]], vec("v", [[0, "Foo", "data", ALIGN], [1, "ManagedReferencesRegistry", "r", 0], [2, "int", "version", 0]])), i32(7, 1, 3))
    add("py_aligned_prim_arrays", base(*[vec(f"v{i}", leaf(t, "data", ALIGN)) for i, t in enumerate(PRIMS) if t != "SInt8"]), b"".join(i32(2) + RUNS_BY_TYPE[t] for t in PRIMS if t != "SInt8"))
    add("py_aligned_sint8_array", base(vec("v", leaf("SInt8", "data", ALIGN))), i32(2) + b"\x01\x02")
    add("py_aligned_string_array", base(vec("v", leaf("string", "data", ALIGN))), i32(2) + s("a") + s(""))
    add("py_typeless_minus_one", base([[0, "TypelessData", "t", 0]], U8("c")), i32(-1) + b"abcd")
    add("py_typeless_negative", base([[0, "TypelessData", "t", 0]], U8("c"), U8("d")), i32(-6) + b"\x09\x0a")
    add("py_typeless_negative_far", base([[0, "TypelessData", "t", 0]], INT("c")), i32(-100) + b"\x09")
    add("py_typeless_zero", base([[0, "TypelessData", "t", 0]], U8("c")), i32(0) + b"\x09")
    add("py_read_past_end", base(INT("a"), INT("b")), i32(1) + b"\x02")
    add("py_u8_past_end", base(INT("a"), U8("b")), i32(1))
    add("py_vector_of_bytes_negative", base(vec("v", [[0, "TypelessData", "data", 0]])), i32(2, -4, -4))
    add("py_odd_start", base(U8("a", ALIGN), U8("c")), b"\xee\x01\0\0\x09\0\0\0", start=1)
    add("py_odd_start_string", base(leaf("string", "s"), U8("c")), b"\xee" + i32(1) + b"x\0\0\0\x09", start=1)
    add("py_odd_start_run", base(vec("v", leaf("UInt8", "data", ALIGN)), U8("c")), b"\xee" + i32(1) + b"\x05\0\0\x09", start=1)
    add("py_vector_unaligned_u8_list", base(vec("v", U8("data")), vec("b", leaf("bool", "data")), vec("f", leaf("float", "data"))), i32(2) + b"\x01\x02" + i32(1) + b"\x01" + i32(1) + struct.pack("<I", 0x7F800001))
    add("py_vector_u8_after_negative_bytes", base([[0, "TypelessData", "t", 0]], vec("v", U8("data"))), i32(-12) + b"\0\0\0\0" + i32(2) + b"\x01\x02")
    add("py_registry_vector_type", base([[0, "ManagedReferencesRegistry", "r", 0], [1, "Array", "Array", 0], [2, "int", "size", 0], [2, "int", "data", 0]]), i32(1, 7))
    add("py_aligned_pair_array_three_children", base(vec("v", [[0, "pair", "data", ALIGN], [1, "int", "a", 0], [1, "int", "b", 0], [1, "int", "c", 0]])), i32(1, 1, 2))
    add("py_aligned_pair_vector_children", base(vec("v", pair("data", vec("first", leaf("int", "data")), leaf("string", "second"), ALIGN))), i32(1, 1, 5) + s("ab"))
    add("py_aligned_bool_tuple", base(vec("v", leaf("bool", "data", ALIGN))), i32(3) + b"\x00\x02\x01\0")
    add("py_aligned_float_tuple_snan", base(vec("v", leaf("float", "data", ALIGN))), i32(1) + struct.pack("<I", 0x7F800001))
    add("py_aligned_prim_array_past_end", base(vec("v", leaf("int", "data", ALIGN))), i32(3, 1))
    add("py_aligned_ref_object_absent_type_key", base(vec("v", no_type_rows(ALIGN))), i32(1) + struct.pack("<q", 1) + s("C") + s("N") + s("A"), ref_types=REF)
    c.extend(base_read_cases())
    # The empty-array and pair-child cases of pure_python_rules.py.
    import pure_python_rules as P  # noqa: PLC0415

    for i, (name, rows, data, _b, _p) in enumerate(P.CASES):
        nodes = [[lv, ty, nm, fl] for lv, ty, nm, fl in rows]
        add(f"py_rules_{i:02d} " + name, nodes, data)
    return c


WIDTHS = {"SInt16": 2, "UInt16": 2, "int": 4, "UInt32": 4, "float": 4, "SInt64": 8, "UInt64": 8, "double": 8}


def base_read_cases():
    """`FUNCTION_READ_MAP`'s readers are the base class's `unpack(fmt,
    read(n))`, not the memoryview reader's `unpack_from`: they differ at
    position -n, where `read(n)` is the empty slice `view[-n:0]`. A negative
    `TypelessData` length puts the position at -n, -n-1 and -1 before a
    primitive read directly, as a vector's element (after a length the
    memoryview reader's own `read_int` reads), and as an aligned pair's
    second child (`read_value_array`'s `key_func`/`value_func`)."""
    out = []
    tail = bytes(range(0x11, 0x29))  # 24 distinct bytes

    def add(name, rows, at, body):
        # `t`'s length moves the position from 4 to `at`.
        out.append(dict(name=name, nodes=base([[0, "TypelessData", "t", 0]], rows), hex=(i32(at - 4) + body).hex()))

    for t, n in WIDTHS.items():
        for at, label in ((-n, f"m{n}"), (-n - 1, f"m{n}_1"), (-1, "m1")):
            add(f"py_base_read_{t}_at_{label}", leaf(t, "c"), at, tail)
    for t in ("SInt16", "float", "UInt64"):
        n = WIDTHS[t]
        for at, label in ((-n, f"m{n}"), (-n - 1, f"m{n}_1"), (-1, "m1")):
            # The length, one element, then the element at `at`.
            add(f"py_base_read_{t}_element_at_{label}", vec("v", leaf(t, "data")), at - 4, tail + i32(1) + tail[: -at])
    for t in ("UInt16", "int", "double"):
        n = WIDTHS[t]
        for at, label in ((-n, f"m{n}"), (-n - 1, f"m{n}_1"), (-1, "m1")):
            # The length, one pair: `first` before `at`, `second` at it.
            rows = vec("v", pair("data", leaf(t, "first"), leaf(t, "second"), ALIGN))
            add(f"py_base_read_{t}_pair_second_at_{label}", rows, at - n - 4, tail + i32(1) + tail[: n - at])
    return out


# --- Depth (plan 3.10, "Bounds")

# Each shape nests its levels inside ref type `A` around a `ReferencedObject`
# whose data is the next `A`; the last holds a `Z`. A level is one of: a class
# (`Class`), a vector of the next level (`Vector`), an aligned vector of it
# (`Aligned`, `read_value_array`), an aligned vector of a class holding it
# (`ClassArray`), a pair whose `first` is it (`Pair`), and an aligned vector of
# pairs whose `first` is a class holding it (`PairArray`, the `lambda`). The
# Rust test (`walk_stack.rs`) builds the same rows and bytes from the same
# names; `pos` checks they agree.

# The tree levels each kind of level takes.
LEVELS = {"Class": 1, "Vector": 2, "Aligned": 2, "ClassArray": 3, "Pair": 1, "PairArray": 4}


def every_transition():
    """The six kinds of level in an order in which each follows each other
    kind once: an Euler circuit of the complete directed graph on them
    (Hierholzer's algorithm), 30 levels, closing on its first."""
    kinds = list(LEVELS)
    left = {k: [o for o in kinds if o != k] for k in kinds}
    stack, circuit = [kinds[0]], []
    while stack:
        if left[stack[-1]]:
            stack.append(left[stack[-1]].pop(0))
        else:
            circuit.append(stack.pop())
    return circuit[::-1][:-1]


def mixed(order, levels=248):
    """`order` repeated, a level each, as deep as `class` and `pair` go."""
    out, used = [], 0
    while True:
        for w in order:
            if used + LEVELS[w] > levels:
                return out
            out.append((w, 1))
            used += LEVELS[w]


DEPTH_SHAPES = {
    "ref": [],
    "class": [("Class", 248)],
    "vector": [("Vector", 124)],
    "aligned": [("Vector", 1), ("Aligned", 120)],
    "class_array": [("ClassArray", 82)],
    "pair": [("Pair", 248)],
    "pair_array": [("PairArray", 62)],
    # Every kind after every other, as a hostile tree can nest them.
    "mixed": mixed(every_transition()),
}


def depth_rows(wraps):
    out = [[0, "A", "Base", 0]]
    level, name, meta = 1, "x", 0
    tails = []
    for w in wraps:
        if w in ("Vector", "Aligned", "ClassArray", "PairArray"):
            out += [[level, "vector", name, meta], [level + 1, "Array", "Array", 0], [level + 2, "int", "size", 0]]
            level, name, meta = level + 2, "data", ALIGN if w != "Vector" else 0
        if w in ("Class", "ClassArray"):
            out.append([level, "C", name, meta])
            level, name, meta = level + 1, "c", 0
        elif w == "Pair":
            out.append([level, "pair", name, meta])
            tails.append([level + 1, "int", "second", 0])
            level, name, meta = level + 1, "first", 0
        elif w == "PairArray":
            out += [[level, "pair", name, meta], [level + 1, "C", "first", 0]]
            tails.append([level + 1, "int", "second", 0])
            level, name, meta = level + 2, "c", 0
    out += [
        [level, "ReferencedObject", name, meta],
        [level + 1, "ReferencedManagedType", "type", 0],
        [level + 2, "string", "class", 0],
        [level + 2, "string", "ns", 0],
        [level + 2, "string", "asm", 0],
        [level + 1, "ReferencedObjectData", "data", 0],
    ]
    return out + tails[::-1]


def depth_data(wraps, n):
    prefix = b"".join(i32(1) for w in wraps if w not in ("Class", "Pair"))
    suffix = b"".join(i32(5) for w in reversed(wraps) if w in ("Pair", "PairArray"))
    body = b"".join(prefix + s("Z" if i == n else "A") + s("n") + s("a") for i in range(1, n + 1))
    return body + i32(7) + suffix * n


def depth_outcome(read):
    try:
        value, pos = read()
    except Exception as e:  # noqa: BLE001 - the exception class is the outcome
        return err(e), None
    return {"ok": True, "pos": pos}, value


def depth_write(value, root, af, read_bytes):
    w = EndianBinaryWriter(endian="<")
    try:
        TTH.write_typetree(value, root, w, af)
    except Exception as e:  # noqa: BLE001
        return {"err": type(e).__name__}
    return {"equal": bytes(w.bytes) == read_bytes}


def depth_input(spec, n):
    wraps = [w for w, k in spec for _ in range(k)]
    rows = depth_rows(wraps)
    af = assets([
        {"class": "A", "ns": "n", "asm": "a", "nodes": rows},
        {"class": "Z", "ns": "n", "asm": "a", "nodes": [[0, "Z", "Base", 0], [1, "int", "z", 0]]},
    ])
    return tree(rows), af, depth_data(wraps, n)


def depth_python(root, af, data):
    reader = EndianBinaryReader(data, "<")
    value = TTH.read_value(root, reader, TTH.TypeTreeConfig(True, af, False))
    return value, reader.Position


def depth_case(name, spec, n, at):
    root, af, data = depth_input(spec, n)
    out = {"name": f"{name}_{n}", "shape": name, "wraps": [list(w) for w in spec], "n": n, "at": at}
    for reader, read in (
        ("boost", lambda: boost_read(data, root, "<", True, af)),
        ("python", lambda: depth_python(root, af, data)),
    ):
        out[reader], value = depth_outcome(read)
        if value is not None:
            out[f"{reader}_write"] = depth_write(value, root, af, data[: out[reader]["pos"]])
    return out


def recursion_error(spec, n):
    try:
        depth_python(*depth_input(spec, n))
    except RecursionError:
        return True
    return False


def depth_cases():
    """Each shape read `n` `A`s deep by both readers and re-saved: for the
    `n` either side of the least at which the pure-Python reader raises
    `RecursionError` (Python's 1,000 frames, less the frames of this script
    that call it), and at a quarter of that. UnityPyBoost recurses on the C
    stack, with no bound of its own at these depths."""
    out = []
    for name, spec in DEPTH_SHAPES.items():
        lo, hi = 1, 2  # lo reads, hi raises
        while not recursion_error(spec, hi):
            lo, hi = hi, hi * 2
        while hi - lo > 1:
            mid = (lo + hi) // 2
            lo, hi = (lo, mid) if recursion_error(spec, mid) else (mid, hi)
        # A case reads a frame or two deeper in this script than the search:
        # its `lo` is the last `n` it reads itself.
        while (case := depth_case(name, spec, lo, "last_read"))["python"].get("err") == "RecursionError":
            lo, hi = lo - 1, lo
        out += [depth_case(name, spec, max(1, hi // 4), "quarter"), case, depth_case(name, spec, hi, "first_raise")]
    return out


def boost_reach_child(name, n):
    root, af, data = depth_input(DEPTH_SHAPES[name], int(n))
    boost_read(data, root, "<", True, af)
    return 0


def boost_reach():
    """For each depth shape, the last `n` UnityPyBoost reads and the first at
    which its process dies or raises, each `n` tried in a child process."""

    def reads(name, n):
        argv = [sys.executable, __file__, "--boost-reach-child", name, str(n)]
        return child.run(argv, stdout=child.DEVNULL, stderr=child.DEVNULL).returncode == 0

    for name in DEPTH_SHAPES:
        lo, hi = 1, 2
        while hi <= 1 << 22 and reads(name, hi):
            lo, hi = hi, hi * 2
        if hi > 1 << 22:
            print(f"{name}: reads {lo} A's")
            continue
        while hi - lo > 1:
            mid = (lo + hi) // 2
            lo, hi = (mid, hi) if reads(name, mid) else (lo, mid)
        print(f"{name}: reads {lo} A's, not {hi}", flush=True)
    return 0


def no_type_rows(meta):
    return [[0, "ReferencedObject", "data", meta], [1, "SInt64", "rid", 0], [1, "ReferencedObjectData", "data", 0], [1, "ReferencedManagedType", "type", 0], [2, "string", "class", 0], [2, "string", "ns", 0], [2, "string", "asm", 0]]


RUNS_BY_TYPE = {
    "SInt8": struct.pack("<2b", -1, 2), "UInt8": bytes([255, 1, 0, 0]), "char": b"AB\0\0",
    "short": struct.pack("<2h", -3, 3), "SInt16": struct.pack("<2h", -4, 4), "unsigned short": struct.pack("<2H", 65535, 1),
    "UInt16": struct.pack("<2H", 2, 40000), "int": struct.pack("<2i", -5, 5), "SInt32": struct.pack("<2i", -6, 6),
    "unsigned int": struct.pack("<2I", 2**32 - 1, 7), "UInt32": struct.pack("<2I", 8, 9), "Type*": struct.pack("<2I", 10, 11),
    "long long": struct.pack("<2q", -12, 12), "SInt64": struct.pack("<2q", -13, 13), "unsigned long long": struct.pack("<2Q", 2**64 - 1, 14),
    "UInt64": struct.pack("<2Q", 15, 16), "FileSize": struct.pack("<2Q", 17, 18), "float": struct.pack("<2f", 1.5, -0.0),
    "double": struct.pack("<2d", 2.5, -3.5), "bool": b"\x00\x02\0\0",
}


def w_cases():
    c = []
    add = lambda name, nodes, data, **kw: c.append(dict(name=name, nodes=nodes, hex=data.hex(), **kw))  # noqa: E731
    # W1: the type name first (FUNCTION_WRITE_MAP), then `pair`, then
    # `ReferencedObject`, then an `Array` first child, then a class.
    add("w1_type_name_before_array_child", base([[0, "int", "i", 0], [1, "Array", "Array", 0], [2, "int", "size", 0], [2, "int", "data", 0]], [[0, "string", "s", 0], [1, "Array", "Array", 0], [2, "int", "size", 0], [2, "int", "data", 0]], [[0, "TypelessData", "t", 0], [1, "Array", "Array", 0], [2, "int", "size", 0], [2, "int", "data", 0]]), i32(7) + s("ab") + i32(1) + b"x")
    add("w1_pair_before_array_child", base([[0, "pair", "p", 0], [1, "Array", "Array", 0], [2, "int", "size", 0], [2, "int", "data", 0], [1, "int", "second", 0]]), i32(1, 5, 9))
    add("w1_ref_object_before_array_child", base([[0, "ReferencedObject", "o", 0], [1, "Array", "Array", 0], [2, "int", "size", 0], [2, "int", "data", 0]]), i32(1, 5))
    add("w1_array_child_before_class", base([[0, "Foo", "v", 0], [1, "Array", "Array", 0], [2, "int", "size", 0], [2, "int", "data", 0]], [[0, "Foo", "c", 0], [1, "int", "a", 0], [1, "Array", "Array", 0], [2, "int", "size", 0], [2, "int", "data", 0]]), i32(2, 5, 6, 3, 1, 4))
    # W1 by the whole name: a type name holding a NUL is a class here, and UnityPyBoost reads the name before the NUL.
    for name, elem, body in (("run", "int", i32(3, 1, 2, 3)), ("strings", "string", i32(2) + s("ab") + s("")), ("bytes", "TypelessData", i32(2, 1) + b"\x07" + i32(0))):
        add(f"w1_nul_type_name_{name}", base(vec("v", leaf(f"{elem}\0x", "data"))), body)
    add("w1_nul_type_name_child_writes", base(vec("v", [[0, "int\0x", "data", 0], [1, "int", "real", 0]])), i32(2, 5, 6))
    add("w3_strings_align", base(leaf("string", "s"), U8("c"), leaf("string", "t")), s("abc") + b"\x09" + b"\0\0\0" + s(""))
    add("w4_bytes_flag", base([[0, "TypelessData", "t", 0]], [[0, "TypelessData", "u", ALIGN]], U8("c")), i32(1) + b"\x01" + i32(1) + b"\x02\0\0\0\x09")
    add("w5_array_elements_align_each", base(vec("v", U8("data", ALIGN)), vec("p", pair("data", U8("first"), U8("second"), ALIGN)), U8("c")), i32(2) + b"\x01\x02\0\0" + i32(1) + b"\x03\x04\0\0\x09")
    add("w5_array_three_children", base([[0, "vector", "v", 0], [1, "Array", "Array", 0], [2, "int", "size", 0], [2, "int", "data", 0], [2, "UInt8", "extra", 0]]), i32(2, 5, 6))
    add("w5_vector_and_array_flags", base(vec("v", U8("data", ALIGN), meta=ALIGN, array_meta=ALIGN), U8("c")), i32(1) + b"\x05\0\0\0\x09")
    add("w6_registry_scope", base([[0, "ManagedReferencesRegistry", "r1", 0], [1, "int", "version", 0]], [[0, "ManagedReferencesRegistry", "r2", 0], [1, "int", "version", 0]], INT("t")), i32(1, 2, 3))
    add("w7_empty_ref_types", base(ref_object(), INT("tail")), ref_bytes("C") + i32(42) + i32(9), ref_types=[])
    # Fix pass 17's review: a value written against no file (`ValueError`), or a file below format 20, which has no
    # `ref_types` attribute (`AttributeError`, `files/SerializedFile.py:305-308`, `helpers/TypeTreeHelper.py:99`).
    for name, other in (("no_file", None), ("absent_ref_types", "absent")):
        add(f"w7_written_against_{name}", base(ref_object(), INT("tail")), ref_bytes("C") + i32(42) + i32(9), ref_types=REF, write_ref_types=other)
    add("w7_empty_class", base(ref_object(), INT("tail")), ref_bytes("") + i32(9), ref_types=REF)
    add("w7_matched_no_tree", base(ref_object(), INT("tail")), ref_bytes("C") + i32(9), ref_types=[dict(REF[0], nodes=None)])
    add("w7_match_writes", base(ref_object(), INT("tail")), ref_bytes("C") + i32(42) + i32(9), ref_types=REF)
    return c


# --- The value cases (G2)


def value_cases():
    try:
        import value_cases as V  # noqa: PLC0415
    except ImportError:
        sys.path.insert(0, str(ROOT / "tools/parity"))
        import value_cases as V  # noqa: PLC0415
    out = []
    for case in V.CASES:
        root = V.unitypy_tree(case["nodes"])
        endian = case.get("endian", "<")
        data = case["data"]
        af = SimpleNamespace(ref_types=[])

        def read(check, data=data, root=root, endian=endian, af=af):
            r = EndianBinaryReader(data, endian=endian)
            return TTH.read_typetree(root, r, as_dict=True, byte_size=len(data), check_read=check, assetsfile=af)

        rec = {"name": case["name"], "endian": endian, "nodes": case["nodes"], "hex": data.hex()}
        l2 = {"kind": None, "bytes_read": None, "value": None}
        value = None
        try:
            value = read(True)
        except Exception as e:  # noqa: BLE001
            kind, n = error_kind(e)
            l2.update(kind=kind, bytes_read=n)
            if kind == "check_read":
                value = read(False)
        if value is not None:
            l2["value"] = enc(value)
        rec["l2"] = l2
        rec["l3"] = write(value, root, endian, af) if l2["kind"] is None else None
        out.append(rec)
    return out


# --- Output


def compact(v):
    return json.dumps(v, sort_keys=True, separators=(",", ":"))


def dump_list(items):
    """A JSON array with one element per line, so a regenerated fixture diffs by case."""
    return "[\n" + ",\n".join(compact(i) for i in items) + "\n]\n"


def dump_matrix(m):
    head = "".join(f" {json.dumps(k)}: {compact(m[k])},\n" for k in ("nodes", "values"))
    return "{\n" + head + ' "cases": ' + dump_list(m["cases"]).rstrip("\n") + "\n}\n"


def fixtures():
    cases = [run_case(c) for c in r_cases() + reentry_ok_cases() + py_cases() + w_cases() + mirror_cases()]
    names = [c["name"] for c in cases]
    assert len(names) == len(set(names)), "case names repeat"
    return {
        "cases.json": dump_list(cases),
        "value_cases.json": dump_list(value_cases()),
        "write_matrix.json": dump_matrix(write_matrix()),
        "py_attrs.json": compact(py_attrs()) + "\n",
        "depth.json": dump_list(depth_cases()),
        "elements.json": dump_list(element_cases()),
        "runs.json": dump_list(run_cases()),
        "reentry.json": dump_list(reentry_cases()),
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true", help="regenerate and compare, write nothing")
    ap.add_argument("--boost-reach", action="store_true", help="print each depth shape's UnityPyBoost reach")
    ap.add_argument("--boost-reach-child", nargs=2, metavar=("SHAPE", "N"), help=argparse.SUPPRESS)
    args = ap.parse_args()
    if UnityPy.__version__ != "1.25.2" or TTH.read_typetree_boost is None:
        print(f"needs UnityPy 1.25.2 with UnityPyBoost, found {UnityPy.__version__}", file=sys.stderr)
        return 2
    if args.boost_reach_child:
        return boost_reach_child(*args.boost_reach_child)
    if args.boost_reach:
        return boost_reach()
    stale = []
    for name, text in fixtures().items():
        path = OUT / name
        if args.check:
            if not path.exists() or path.read_text() != text:
                stale.append(name)
        else:
            OUT.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
    if stale:
        print("stale fixtures: " + ", ".join(stale), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
