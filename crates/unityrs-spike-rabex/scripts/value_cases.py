#!/usr/bin/env python3
"""Synthetic typetree cases: UnityPyBoost against rabex (docs/spikes/rabex.md).

    python value_cases.py RABEX_DUMP_BINARY [--json OUT]

The corpus exercises one engine line, so each case here targets one place
where reading rabex's source suggested it parts from UnityPyBoost
(TypeTreeHelper.cpp): alignment rules, negative lengths, ReferencedObject,
node shapes rabex classifies differently. A case is a tree and its bytes; the
oracle side reads it with `read_typetree` (the Boost path, as `parse_as_dict`
does) and resaves with `write_typetree`; the rabex side is `rabex-dump
values`. Both encode with the canonical rules, so a value matches when the
text matches. Both sides also return their re-saved bytes, and L3 compares
those bytes with each other and with the input (`l3_verdict`).
"""

import argparse
import json
import struct
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path
from types import SimpleNamespace

PARITY = Path(__file__).resolve().parents[3] / "tools" / "parity"
sys.path.insert(0, str(PARITY))

import canonical as C  # noqa: E402
from UnityPy.helpers import TypeTreeHelper as TTH  # noqa: E402
from UnityPy.helpers.TypeTreeNode import TypeTreeNode  # noqa: E402
from UnityPy.streams import EndianBinaryReader, EndianBinaryWriter  # noqa: E402
from unitypy_dump import error_kind  # noqa: E402

ALIGN = 0x4000


def i32(*v, e="<"):
    return struct.pack(f"{e}{len(v)}i", *v)


def s(text, e="<"):
    raw = text if isinstance(text, bytes) else text.encode()
    return i32(len(raw), e=e) + raw + b"\0" * (-len(raw) % 4)


def vec(level, name, elem_rows, meta=0, array_meta=0):
    """Rows of vector NAME whose element subtree is elem_rows (at level 0)."""
    return [
        [level, "vector", name, meta],
        [level + 1, "Array", "Array", array_meta],
        [level + 2, "int", "size", 0],
        *[[level + 2 + r[0], r[1], r[2], r[3]] for r in elem_rows],
    ]


def base(*children):
    rows = [[0, "Base", "Base", 0]]
    for c in children:
        rows.extend([[r[0] + 1, *r[1:]] for r in c])
    return rows


def leaf(t, name, meta=0):
    return [[0, t, name, meta]]


CASES = [
    # --- Alignment rules
    dict(
        name="typeless_data_align",
        why="TypelessData with the align flag: Boost aligns after it; rabex's Untyped path never aligns",
        nodes=base(
            [[0, "TypelessData", "blob", ALIGN], [1, "int", "size", 0], [1, "UInt8", "data", 0]], leaf("int", "after")
        ),
        data=i32(1) + b"\xaa\0\0\0" + i32(7),
    ),
    dict(
        name="empty_struct_align",
        why="a childless struct with the align flag: Boost reads {} and aligns; rabex visits unit and does not",
        nodes=base(leaf("UInt8", "a"), leaf("Foo", "e", ALIGN), leaf("int", "after")),
        data=b"\x01\0\0\0" + i32(7),
    ),
    dict(
        name="empty_struct_align_last",
        why="the same struct as the last field, then 4 spare bytes: both under-read (check_read) to the same value, "
        "Boost after 4 bytes and rabex after 1, so only bytes_read tells them apart",
        nodes=base(leaf("UInt8", "a"), leaf("Foo", "e", ALIGN)),
        data=b"\x01\0\0\0" + b"\xff\xff\xff\xff",
    ),
    dict(
        name="map_array_align",
        why="map whose Array node carries the align flag: Boost aligns (node or Array); rabex checks map or pair",
        nodes=base(
            [
                [0, "map", "m", 0],
                [1, "Array", "Array", ALIGN],
                [2, "int", "size", 0],
                [2, "pair", "data", 0],
                [3, "UInt8", "first", 0],
                [3, "UInt8", "second", 0],
            ],
            leaf("int", "after"),
        ),
        data=i32(1) + b"\x01\x02\0\0" + i32(7),
    ),
    dict(
        name="primitive_element_align",
        why="vector<UInt8> whose element node is aligned: Boost aligns once after the array; rabex after every element",
        nodes=base(vec(0, "v", [[0, "UInt8", "data", ALIGN]]), leaf("int", "after")),
        data=i32(2) + b"\x01\x02\0\0" + i32(7),
    ),
    dict(
        name="pair_element_align",
        why="vector<pair> whose pair node is aligned: Boost aligns once after the array; rabex after every pair",
        nodes=base(
            vec(0, "v", [[0, "pair", "data", ALIGN], [1, "UInt8", "first", 0], [1, "UInt8", "second", 0]]),
            leaf("int", "after"),
        ),
        data=i32(2) + b"\x01\x02\x03\x04" + i32(7),
    ),
    dict(
        name="struct_align",
        why="an aligned struct: both align after it",
        nodes=base([[0, "Foo", "f", ALIGN], [1, "UInt8", "a", 0]], leaf("int", "after")),
        data=b"\x05\0\0\0" + i32(7),
    ),
    dict(
        name="array_node_align",
        why="vector<UInt8> with an aligned Array node (Unity's usual shape): both align",
        nodes=base(vec(0, "v", [[0, "UInt8", "data", 0]], array_meta=ALIGN), leaf("int", "after")),
        data=i32(3) + b"\x01\x02\x03\0" + i32(7),
    ),
    # --- Lengths and bounds
    dict(
        name="negative_string_length",
        why="string length -1: Boost raises 'Negative length'; rabex reads 2^32-1 as a u32 length and refuses it "
        "as above its 1 GiB read limit (any length up to 1 GiB is allocated before the read fails)",
        nodes=base(leaf("string", "s")),
        data=i32(-1) + b"\0" * 8,
    ),
    dict(
        name="negative_vector_length",
        why="vector<int> length -1: Boost raises 'Negative length'; rabex reads 2^32-1, runs out of bytes on the "
        "third element, and reports 'did not exhaust sequence' in place of the end-of-data error",
        nodes=base(vec(0, "v", [[0, "int", "data", 0]])),
        data=i32(-1) + i32(1, 2),
    ),
    dict(
        name="string_past_end",
        why="a string longer than the object: both report out of bounds",
        nodes=base(leaf("string", "s")),
        data=i32(100) + b"abcd",
    ),
    dict(
        name="short_read",
        why="the tree reads fewer bytes than the object: check_read on both",
        nodes=base(leaf("int", "a")),
        data=i32(1, 2),
    ),
    # Truncated arrays. rabex counts elements down before reading each one,
    # and on any error with elements still to come reports "did not exhaust
    # sequence", so the end-of-data error is lost unless the last element fails.
    dict(
        name="vector_u8_truncated",
        why="vector<UInt8> of 8 with 2 bytes left: Boost out of bounds; rabex masks the end of data (third element)",
        nodes=base(vec(0, "v", [[0, "UInt8", "data", 0]])),
        data=i32(8) + b"\x01\x02",
    ),
    dict(
        name="vector_float_truncated",
        why="vector<float> of 4 with one element left: Boost out of bounds; rabex masks the end of data",
        nodes=base(vec(0, "v", [[0, "float", "data", 0]])),
        data=i32(4) + b"\0\0\x80\x3f",
    ),
    dict(
        name="vector_int_truncated",
        why="vector<int> of 3 with 2 elements: the last element fails, so both report out of bounds",
        nodes=base(vec(0, "v", [[0, "int", "data", 0]])),
        data=i32(3) + i32(1, 2),
    ),
    dict(
        name="vector_struct_truncated",
        why="vector<Foo{int}> of 3 with 2 elements: the last element fails, so both report out of bounds",
        nodes=base(vec(0, "v", [[0, "Foo", "data", 0], [1, "int", "a", 0]])),
        data=i32(3) + i32(1, 2),
    ),
    dict(
        name="vector_string_truncated",
        why="vector<string> of 2 whose second string runs past the end: both report out of bounds",
        nodes=base(vec(0, "v", [[0, "string", "data", 0]])),
        data=i32(2) + s("ab") + i32(100),
    ),
    dict(
        name="map_truncated",
        why="map<int,int> of 2 with one pair: the last pair fails, so both report out of bounds",
        nodes=base(
            [
                [0, "map", "m", 0],
                [1, "Array", "Array", 0],
                [2, "int", "size", 0],
                [2, "pair", "data", 0],
                [3, "int", "first", 0],
                [3, "int", "second", 0],
            ]
        ),
        data=i32(2) + i32(1, 2),
    ),
    # --- Shapes
    dict(
        name="referenced_object_registry",
        why="ManagedReferencesRegistry/ReferencedObject: Boost reads them; rabex returns 'unimplemented'",
        nodes=base(
            [
                [0, "ManagedReferencesRegistry", "references", 0],
                [1, "int", "version", 0],
                *[
                    [r[0] + 1, *r[1:]]
                    for r in vec(
                        0,
                        "RefIds",
                        [
                            [0, "ReferencedObject", "data", 0],
                            [1, "SInt64", "rid", 0],
                            [1, "ReferencedManagedType", "type", 0],
                            [2, "string", "class", 0],
                            [2, "string", "ns", 0],
                            [2, "string", "asm", 0],
                            [1, "ReferencedObjectData", "data", 0],
                        ],
                    )
                ],
            ],
            leaf("int", "tail"),
        ),
        data=i32(2, 1) + struct.pack("<q", 2) + s("") + s("") + s("") + i32(9),
    ),
    dict(
        name="array_first_child_with_siblings",
        why="a node whose first child is an Array but which has more children: Boost reads an array; rabex a struct",
        nodes=base(
            [
                [0, "Foo", "f", 0],
                [1, "Array", "Array", 0],
                [2, "int", "size", 0],
                [2, "int", "data", 0],
                [1, "int", "extra", 0],
            ]
        ),
        data=i32(1, 5),
    ),
    dict(
        name="type_named_node",
        why="a struct node whose type is literally 'Type': rabex reads nothing (TodoType); Boost reads its children",
        nodes=base([[0, "Type", "t", 0], [1, "int", "x", 0]], leaf("int", "after")),
        data=i32(5, 7),
    ),
    dict(
        name="pair_outside_map",
        why="a pair node on its own: a tuple on both",
        nodes=base([[0, "pair", "p", 0], [1, "int", "first", 0], [1, "string", "second", 0]]),
        data=i32(3) + s("hi"),
    ),
    dict(
        name="map_string_keys",
        why="map<string,int> with a repeated key: a list of tuples, both entries kept",
        nodes=base(
            [
                [0, "map", "m", 0],
                [1, "Array", "Array", 0],
                [2, "int", "size", 0],
                [2, "pair", "data", 0],
                [3, "string", "first", 0],
                [3, "int", "second", 0],
            ]
        ),
        data=i32(2) + s("k") + i32(1) + s("k") + i32(2),
    ),
    dict(
        name="duplicate_child_names",
        why="int dup then float dup: first position, last value, last node types it",
        nodes=base(leaf("int", "dup"), leaf("float", "dup")),
        data=i32(1) + struct.pack("<f", 2.0),
    ),
    dict(
        name="nested_vectors",
        why="vector<vector<SInt16>>, vector<SInt8>, vector<bool>, vector<char>",
        nodes=base(
            vec(0, "vv", vec(0, "data", [[0, "SInt16", "data", 0]], array_meta=ALIGN)),
            vec(0, "s8", [[0, "SInt8", "data", 0]], array_meta=ALIGN),
            vec(0, "b", [[0, "bool", "data", 0]], array_meta=ALIGN),
            vec(0, "c", [[0, "char", "data", 0]], array_meta=ALIGN),
        ),
        data=i32(2)
        + i32(1)
        + struct.pack("<h", -3)
        + b"\0\0"
        + i32(0)
        + i32(2)
        + b"\xff\x01\0\0"
        + i32(2)
        + b"\x01\x00\0\0"
        + i32(3)
        + b"abc\0",
    ),
    dict(
        name="wide_and_odd_ints",
        why="UInt64 max, FileSize, Type*, char scalar, SInt8",
        nodes=base(
            leaf("UInt64", "u"),
            leaf("FileSize", "fs"),
            leaf("Type*", "tp"),
            leaf("char", "c"),
            leaf("SInt8", "i8", ALIGN),
        ),
        data=struct.pack("<QQI", 2**64 - 1, 5, 6) + b"z\xfe\0\0",
    ),
    dict(
        name="empty_float_vector",
        why="an empty vector<float> is [] on both",
        nodes=base(vec(0, "v", [[0, "float", "data", 0]])),
        data=i32(0),
    ),
    # --- Values UnityPy changes
    dict(
        name="bool_byte_2",
        why="a bool byte of 2 reads True on both; both resave 1 (differs)",
        nodes=base(leaf("bool", "b", ALIGN)),
        data=b"\x02\0\0\0",
    ),
    dict(
        name="f32_signalling_nan",
        why="float32 sNaN: UnityPyBoost quiets it (7fc00001); rabex keeps the file's bits",
        nodes=base(leaf("float", "f")),
        data=struct.pack("<I", 0x7F800001),
    ),
    dict(
        name="invalid_utf8_string",
        why="a non-UTF-8 string: both read the bytes; rabex's serializer writes them without the alignment",
        nodes=base(leaf("string", "s"), leaf("int", "after")),
        data=s(b"a\xffb") + i32(7),
    ),
    dict(
        name="invalid_utf8_dirty_pad",
        why="the same string with its alignment byte set to 01: both read past the pad; UnityPy re-saves a zero "
        "pad, rabex no pad, so neither round-trips and the two re-saves differ",
        nodes=base(leaf("string", "s"), leaf("int", "after")),
        data=i32(3) + b"a\xffb\x01" + i32(7),
    ),
    dict(
        name="big_endian_mix",
        why="big-endian float, double, string, vector<float> and map",
        endian=">",
        nodes=base(
            leaf("float", "f"),
            leaf("double", "d"),
            leaf("string", "s"),
            vec(0, "fv", [[0, "float", "data", 0]]),
            [
                [0, "map", "m", 0],
                [1, "Array", "Array", 0],
                [2, "int", "size", 0],
                [2, "pair", "data", 0],
                [3, "int", "first", 0],
                [3, "string", "second", 0],
            ],
        ),
        data=struct.pack(">fd", 1.5, -2.25)
        + s("be", ">")
        + i32(2, e=">")
        + struct.pack(">ff", 1.0, 2.0)
        + i32(1, e=">")
        + i32(4, e=">")
        + s("x", ">"),
    ),
]


def unitypy_tree(rows):
    nodes = [
        TypeTreeNode(m_Level=r[0], m_Type=r[1], m_Name=r[2], m_ByteSize=-1, m_Version=1, m_MetaFlag=r[3], m_TypeFlags=0)
        for r in rows
    ]
    return TypeTreeNode.from_list(nodes)


def oracle(case):
    root = unitypy_tree(case["nodes"])
    endian = case.get("endian", "<")
    data = case["data"]
    assetsfile = SimpleNamespace(ref_types=[])
    out = {"kind": None, "bytes_read": None, "value": None, "resave": "skipped", "resave_hex": None}

    def read(check):
        r = EndianBinaryReader(data, endian=endian)
        return TTH.read_typetree(root, r, as_dict=True, byte_size=len(data), check_read=check, assetsfile=assetsfile)

    try:
        value = read(True)
    except Exception as e:  # noqa: BLE001
        kind, n = error_kind(e)
        out.update(kind=kind, bytes_read=n, note=f"{type(e).__name__}: {e}")
        if kind != "check_read":
            return out
        value = read(False)
    try:
        out["value"] = C.dumps(C.ValueEncoder(lambda t: None).encode(value, root))
    except C.EncodeError as e:
        out.update(kind="encode", note=str(e))
        return out
    if out["kind"] is None:
        w = EndianBinaryWriter(endian=endian)
        try:
            TTH.write_typetree(value, root, w, assetsfile)
            out["resave"] = "equal" if bytes(w.bytes) == data else "differs"
            out["resave_hex"] = bytes(w.bytes).hex()
        except Exception as e:  # noqa: BLE001
            out["resave"] = f"error: {type(e).__name__}"
    return out


def l2_verdict(o, r):
    """L2 for one case, from the two sides' read outcomes.

    The canonical L2 error record is `{kind, bytes_read}` (`bytes_read` an
    integer only for `check_read`), and `compare.py` compares both members,
    so two `check_read`s match only when both reads ended at the same byte.
    "f32_snan_quieted" is the by-design difference: UnityPy quiets a
    signalling NaN, and rabex keeps its bits.
    """
    if (o["kind"], o["bytes_read"], o["value"]) == (r["kind"], r["bytes_read"], r["value"]):
        return "match"
    if (
        (o["kind"], o["bytes_read"]) == (r["kind"], r["bytes_read"])
        and o["value"] is not None
        and r["value"] is not None
        and o["value"].replace("7fc00001", "7f800001") == r["value"]
    ):
        return "f32_snan_quieted"
    return "differs"


def l3_verdict(unitypy, rabex, data):
    """L3 over a case's two re-saved byte strings (None where that side did not re-save).

    L3 (DESIGN section 6) asks that a re-save return the input's bytes. When
    both sides re-saved: "match" when both re-saves equal the input; "same_bytes"
    when the two re-saves are byte-equal to each other but not to the input
    (neither round-trips, and they agree on how); "differs" when the two
    re-saves differ from each other. When neither re-saved (both skipped after
    a read error, or both writers refused), there is nothing to compare:
    "not_resaved". When only one side re-saved (the other could not read the
    case, which L2 already counts), "one_side".
    """
    if unitypy is None and rabex is None:
        return "not_resaved"
    if unitypy is None or rabex is None:
        return "one_side"
    if unitypy != rabex:
        return "differs"
    return "match" if unitypy == data else "same_bytes"


def summary(table):
    """Counts for the doc: L2 over every case, L3 over the cases both sides re-saved."""
    l2 = Counter(t["l2"] for t in table)
    l3 = Counter(t["l3"] for t in table)
    return {
        "cases": len(table),
        "l2": dict(l2),
        "l3_resaved_by_both": l3["match"] + l3["same_bytes"] + l3["differs"],
        "l3": dict(l3),
        "l3_same_bytes": sorted(t["case"] for t in table if t["l3"] == "same_bytes"),
        "l3_differs": sorted(t["case"] for t in table if t["l3"] == "differs"),
        "l3_not_resaved": sorted(t["case"] for t in table if t["l3"] == "not_resaved"),
        "l3_one_side": sorted(t["case"] for t in table if t["l3"] == "one_side"),
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("binary", help="path to rabex-dump")
    ap.add_argument("--json", help="write the table as JSON here")
    args = ap.parse_args()
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "cases.json"
        path.write_text(
            json.dumps(
                [
                    {"name": c["name"], "endian": c.get("endian", "<"), "nodes": c["nodes"], "hex": c["data"].hex()}
                    for c in CASES
                ]
            )
        )
        lines = subprocess.run([args.binary, "values", str(path)], check=True, capture_output=True, text=True).stdout
    rabex = {r["name"]: r for r in map(json.loads, lines.splitlines())}
    table = []
    for c in CASES:
        o, r = oracle(c), rabex[c["name"]]
        hexes = [x["resave_hex"] for x in (o, r)]
        l3 = l3_verdict(*(bytes.fromhex(h) if h is not None else None for h in hexes), c["data"])
        table.append(
            {
                "case": c["name"],
                "why": c["why"],
                "l2": l2_verdict(o, r),
                "l3": l3,
                "unitypy": {k: o.get(k) for k in ("kind", "bytes_read", "resave", "resave_hex", "value")},
                "rabex": {k: r.get(k) for k in ("kind", "bytes_read", "resave", "resave_hex", "value", "note")},
            }
        )
    for t in table:
        print(
            f"{t['case']:34} L2 {t['l2']:17} L3 {t['l3']:11} unitypy={t['unitypy']['kind']}/{t['unitypy']['resave']}"
            f"  rabex={t['rabex']['kind']}/{t['rabex']['resave']}"
        )
    print(json.dumps(summary(table)))
    if args.json:
        Path(args.json).write_text(json.dumps(table, indent=1))


if __name__ == "__main__":
    main()
