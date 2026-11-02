"""The cases `walk.py` records for `WriteRules::MirrorReader` (docs/plans/core.md
3.11): trees each reader reads its own way, with bytes each reader that reads
them reads exactly, so the mirror must write back the bytes the reader read.

A value UnityPyBoost read is written back as UnityPyBoost reads, and one the
pure-Python reader read as that reader reads (`CANONICAL_DUMP.md` section 7):
a type name holding a NUL is a class to the pure-Python reader and the name
before the NUL to UnityPyBoost; an empty string is not aligned after; the
elements of a flagged array are read by `read_value_array`
(`helpers/TypeTreeHelper.py:266-361`), which aligns once after the run for
every element kind (a class, a `pair`, a vector, a `ReferencedObject`), reads
a flagged `pair`'s primitive children without their own flags, a flagged
class element's registry children with no registry check, and 16-bit
integers with their signedness swapped. Each case's bytes are exactly what
the pure-Python reader reads, from 0, padding as zeros (`ref_run` lays its
run out with UnityPy's own writer's primitives, which align from the start
as that reader aligns); UnityPyBoost reads some of them too, each exactly to
where it stops, and fails on the rest (`tests/walk_mirror.rs` checks both).
UnityPy's writer (`write_typetree`) of each value is recorded too, as for
every case.

`walk.py` imports this module; it records nothing when run on its own.
"""

from __future__ import annotations

import struct

from UnityPy.streams import EndianBinaryWriter
from walk_build import ALIGN, base, i32, leaf, pair, ref_object, vec

U8 = lambda name, meta=0: leaf("UInt8", name, meta)  # noqa: E731
FOO_X = [[0, "Foo", "data", ALIGN], [1, "UInt8", "x", 0]]
REF_U8 = [{"class": "C", "ns": "N", "asm": "A", "nodes": [[0, "Data", "Base", 0], [1, "UInt8", "x", 0]]}]


def ref_run(xs):
    """A flagged run of `ReferencedObject`s of class `C` (each `data` a `UInt8`), aligned once after, then `c`."""
    w = EndianBinaryWriter(endian="<")
    w.write_int(len(xs))
    for rid, x in enumerate(xs, 1):
        w.write_long(rid)
        for name in ("C", "N", "A"):
            w.write_aligned_string(name)
        w.write_u_byte(x)
    w.align_stream()
    w.write_u_byte(9)
    return bytes(w.bytes)


def mirror_cases():
    c = []
    add = lambda name, nodes, data, **kw: c.append(dict(name=name, nodes=nodes, hex=data.hex(), **kw))  # noqa: E731
    # A type name holding a NUL: a class with no children to the pure-Python reader, an `int` to UnityPyBoost
    # (both read these bytes; the zero-byte object is review round 11's: the mirror refused its pure-Python value).
    add("mirror_nul_type_name_zero_bytes", base(leaf("int\0x", "a")), i32(5, 6))
    add("mirror_nul_type_name_then_int", base(leaf("int\0x", "a"), leaf("int", "b")), i32(5, 7))
    # An empty string: not aligned after by the pure-Python reader (UnityPyBoost aligns, and reads past the end).
    add("mirror_empty_string", base(U8("c"), leaf("string", "s"), U8("d")), b"\x09" + i32(0) + b"\x07")
    # 16-bit runs: `read_short_array` unpacks `H`, `read_u_short_array` `h` (both readers).
    add("mirror_aligned_sint16_run", base(vec("v", leaf("SInt16", "data", ALIGN)), U8("c")), i32(1) + struct.pack("<h", -1) + b"\0\0\x09")
    add("mirror_aligned_uint16_run", base(vec("v", leaf("UInt16", "data", ALIGN)), U8("c")), i32(1) + struct.pack("<H", 40000) + b"\0\0\x09")
    # Flagged runs the pure-Python reader aligns once, where UnityPyBoost aligns each element.
    add("mirror_aligned_class_run", base(vec("v", FOO_X), U8("c")), i32(2) + b"\x01\x02\0\0\x09")
    registries = [[0, "Foo", "data", ALIGN], [1, "ManagedReferencesRegistry", "r1", 0], [2, "UInt8", "version", 0], [1, "ManagedReferencesRegistry", "r2", 0], [2, "UInt8", "version", 0]]
    add("mirror_aligned_class_run_registries", base(vec("v", registries)), i32(1) + b"\x01\0\0\0")
    add("mirror_aligned_pair_run", base(vec("v", pair("data", U8("first", ALIGN), U8("second", ALIGN), ALIGN)), U8("c")), i32(1) + b"\x05\x06\0\0\x09")
    class_first = pair("data", [[0, "Foo", "first", ALIGN], [1, "UInt8", "x", 0]], U8("second"), ALIGN)
    add("mirror_aligned_pair_run_class_first", base(vec("v", class_first), U8("c")), i32(1, 5, 6) + b"\x09")
    add("mirror_aligned_vector_run", base(vec("v", vec("data", U8("data"), meta=ALIGN)), U8("c")), i32(2, 1) + b"\x05" + i32(1) + b"\x06\0\0\x09")
    add("mirror_aligned_vector_run_aligned_inner", base(vec("v", vec("data", U8("data", ALIGN), meta=ALIGN)), U8("c")), i32(2, 1, 5, 1, 6) + b"\x09")
    add("mirror_aligned_bytes_run_empty", base(U8("c"), vec("v", [[0, "TypelessData", "data", ALIGN]]), U8("d")), b"\x09" + i32(0) + b"\0\0\0\x07")
    add("mirror_aligned_ref_object_run", base(vec("v", ref_object("data", meta=ALIGN)), U8("c")), ref_run([3, 4]), ref_types=REF_U8)
    return c
