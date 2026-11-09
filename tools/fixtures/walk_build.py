"""Building inputs and encoding outcomes: the helpers `walk.py` builds its
oracle fixtures' cases with and records UnityPy's outcomes through.

`walk.py` imports this module; it records nothing when run on its own.
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path
from types import SimpleNamespace

from UnityPy.helpers import TypeTreeHelper as TTH
from UnityPy.helpers.TypeTreeNode import TypeTreeNode
from UnityPy.streams import EndianBinaryReader, EndianBinaryWriter
from UnityPy.UnityPyBoost import read_typetree as boost_read

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools/parity"))

from unitypy_dump import error_kind  # noqa: E402

ALIGN = 0x4000


# --- Building inputs


def i32(*v, e="<"):
    return struct.pack(f"{e}{len(v)}i", *v)


def s(text, e="<"):
    raw = text if isinstance(text, bytes) else text.encode()
    return i32(len(raw), e=e) + raw + b"\0" * (-len(raw) % 4)


def shift(rows, by):
    return [[r[0] + by, *r[1:]] for r in rows]


def base(*children):
    rows = [[0, "Base", "Base", 0]]
    for c in children:
        rows.extend(shift(c, 1))
    return rows


def leaf(t, name, meta=0):
    return [[0, t, name, meta]]


def vec(name, elem_rows, meta=0, array_meta=0, ty="vector"):
    """A vector NAME whose element subtree is elem_rows (rooted at level 0)."""
    return [[0, ty, name, meta], [1, "Array", "Array", array_meta], [2, "int", "size", 0], *shift(elem_rows, 2)]


def pair(name, first, second, meta=0):
    return [[0, "pair", name, meta], *shift(first, 1), *shift(second, 1)]


def ref_object(name="obj", data_rows=None, meta=0):
    """`ReferencedObject { SInt64 rid; ReferencedManagedType type { string class, ns, asm }; ReferencedObjectData data }`."""
    rows = [
        [0, "ReferencedObject", name, meta],
        [1, "SInt64", "rid", 0],
        [1, "ReferencedManagedType", "type", 0],
        [2, "string", "class", 0],
        [2, "string", "ns", 0],
        [2, "string", "asm", 0],
        [1, "ReferencedObjectData", "data", 0],
    ]
    return rows


def ref_bytes(cls, ns="N", asm="A", rid=1):
    return struct.pack("<q", rid) + s(cls) + s(ns) + s(asm)


INT_TREE = [[0, "Data", "Base", 0], [1, "int", "x", 0]]
REF = [{"class": "C", "ns": "N", "asm": "A", "nodes": INT_TREE}]
# Classes named like the messages of the other L2 kinds, and of an UnityPyBoost bounds check: a reference to one
# the file does not hold is `ref_type` on both paths (CANONICAL_DUMP.md 4.3), though the pure-Python reader's
# message names it (review round 9: the oracle read such a message's substrings).
LOOKALIKE_CLASSES = [
    "out of bounds",
    "read_u8 out of bounds",
    "Negative length read from TypeTree",
    "Expected to read 3 bytes, but only read 99 bytes",
    "SerializedFile has no ref_types",
]


def tree(rows):
    return TypeTreeNode.from_list(
        [
            TypeTreeNode(m_Level=r[0], m_Type=r[1], m_Name=r[2], m_ByteSize=-1, m_Version=1, m_MetaFlag=r[3])
            for r in rows
        ]
    )


# --- Encoding outcomes


def enc(v):
    t = type(v)
    if t is bool:
        return v
    if t is int:
        assert -(2**63) <= v < 2**64, v
        return v
    if t is float:
        return {"f": struct.pack(">d", v).hex()}
    if t is str:
        return {"s": v.encode("utf8", "surrogateescape").hex()}
    if t in (bytes, bytearray, memoryview):
        return {"y": bytes(v).hex()}
    if t is list:
        return [enc(x) for x in v]
    if t is tuple:
        return {"t": [enc(x) for x in v]}
    if t is dict:
        return {"d": [[k, enc(x)] for k, x in v.items()]}
    raise TypeError(f"unexpected {t.__name__}")


def err(e):
    return {"err": type(e).__name__, "kind": error_kind(e)[0]}


def assets(spec):
    """The `assetsfile` a case reads with: None, an object without
    `ref_types`, or one whose `ref_types` is a list of ref types."""
    if spec is None:
        return None
    if spec == "absent":
        return SimpleNamespace()
    return SimpleNamespace(
        ref_types=[
            SimpleNamespace(
                m_ClassName=r["class"],
                m_NameSpace=r["ns"],
                m_AssemblyName=r["asm"],
                node=tree(r["nodes"]) if r["nodes"] is not None else None,
            )
            for r in spec
        ]
    )


def write(value, root, endian, assetsfile):
    w = EndianBinaryWriter(endian=endian)
    try:
        TTH.write_typetree(value, root, w, assetsfile)
    except Exception as e:  # noqa: BLE001 - the exception class is the outcome
        return {"err": type(e).__name__}
    return {"hex": bytes(w.bytes).hex()}


def run_case(c):
    root = tree(c["nodes"])
    data = bytes.fromhex(c["hex"])
    endian = c.get("endian", "<")
    start = c.get("start", 0)
    af = assets(c.get("ref_types", []))
    # The file each value is written against, when not the one it was read with.
    waf = assets(c["write_ref_types"]) if "write_ref_types" in c else af
    out = dict(c)
    try:
        value, n = boost_read(data[start:], root, endian, True, af)
    except Exception as e:  # noqa: BLE001
        out["boost"] = err(e)
    else:
        out["boost"] = {"ok": enc(value), "pos": n}
        out["boost_write"] = write(value, root, endian, waf)
    reader = EndianBinaryReader(data, endian)
    reader.Position = start
    try:
        value = TTH.read_value(root, reader, TTH.TypeTreeConfig(True, af, False))
    except Exception as e:  # noqa: BLE001
        out["python"] = err(e)
    else:
        out["python"] = {"ok": enc(value), "pos": reader.Position - start}
        out["python_write"] = write(value, root, endian, waf)
    return out
