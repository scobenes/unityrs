"""What tools/fixtures/meshdecode.py builds its handler cases from, not a fixture script: trees as node rows, edits
of them, dict-mode values of their shape, and the vertex layouts `MeshHandler.get_streams` derives.

A tree is a list of `[level, type, name, meta_flag]` rows, the form the fixtures record and
`crates/unityrs-meshdecode/tests/oracle.rs` rebuilds as a `NodeList`: the cases' own, built by hand
(meshdecode_trees.py), or, for `meshdecode.py --tpk` alone, at run time, `tpk_rows(class_id, version)`, the node
UnityPy's own TPK gives that class at that version (`helpers/Tpk.py`'s `get_typetree_node`), which is never
written into the repository. `drop`, `add` and `retype` edit one at a field's path of names, a vector's element
reached through `Array`, `data`. `default_value` is a value of a node's shape as the dict-mode writer takes it
(zeros, empty strings and lists; tools/parity/tests' `test_rows_objects.default_value`), and `element` that of a
field's node, a vector's element for a vector. Run on its own it only imports
(tools/parity/tests/test_fixture_modules.py).
"""

from __future__ import annotations

import struct
import sys

M64 = (1 << 64) - 1


def splitmix(seed: int, n: int) -> bytes:
    """The first `n` bytes SplitMix64 gives from `seed`, each output little-endian (unityrs_testkit::rng::Rng)."""
    out = bytearray()
    state = seed & M64
    while len(out) < n:
        state = (state + 0x9E3779B97F4A7C15) & M64
        z = state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & M64
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & M64
        out += (z ^ (z >> 31)).to_bytes(8, "little")
    return bytes(out[:n])


def node_rows(node) -> list[list]:
    """A TypeTreeNode's rows, depth first."""
    out = []

    def walk(n, level):
        out.append([level, n.m_Type, n.m_Name, int(n.m_MetaFlag or 0)])
        for c in n.m_Children:
            walk(c, level + 1)

    walk(node, 0)
    return out


PROBE = "unityrs.meshdecode.no_writes"  # meshdecode.py's NoWrites answers it


def under_no_writes() -> bool:
    """Whether `meshdecode.py --tpk`'s audit hook is live in this process: it answers this audit event."""
    seen: list = []
    sys.audit(PROBE, seen)
    return seen == [True]


def tpk_rows(class_id: int, version: tuple) -> list[list]:
    """The TPK's tree of `class_id` at `version`, a 5-tuple: for `meshdecode.py --tpk` at run time only, under its
    hook."""
    if not under_no_writes():
        raise RuntimeError("the TPK's trees are read under meshdecode.py --tpk's hook only")
    from UnityPy.helpers import Tpk
    from UnityPy.helpers.UnityVersion import UnityVersion

    return node_rows(Tpk.get_typetree_node(class_id, UnityVersion.from_list(*version)))


def to_node(rows):
    from UnityPy.helpers.TypeTreeNode import TypeTreeNode

    return TypeTreeNode.from_list(
        [
            TypeTreeNode(m_Level=r[0], m_Type=r[1], m_Name=r[2], m_ByteSize=-1, m_Version=1, m_MetaFlag=r[3])
            for r in rows
        ]
    )


def find(rows: list[list], path: tuple) -> int:
    """The index of the row at `path`, names from the root's children down."""
    i = 0
    for name in path:
        level = rows[i][0]
        j = i + 1
        while j < len(rows) and rows[j][0] > level:
            if rows[j][0] == level + 1 and rows[j][2] == name:
                break
            j += 1
        else:
            raise KeyError(path)
        i = j
    return i


def end(rows: list[list], i: int) -> int:
    """One past the last row of row i's subtree."""
    j = i + 1
    while j < len(rows) and rows[j][0] > rows[i][0]:
        j += 1
    return j


def drop(rows: list[list], *path) -> list[list]:
    """The tree without the field at `path`."""
    i = find(rows, path)
    return rows[:i] + rows[end(rows, i) :]


def add(rows: list[list], path: tuple, field: list[list]) -> list[list]:
    """The tree with `field` (rows rooted at level 0) appended to the class at `path`."""
    i = find(rows, path)
    at = end(rows, i)
    return rows[:at] + [[r[0] + rows[i][0] + 1, *r[1:]] for r in field] + rows[at:]


def retype(rows: list[list], path: tuple, ty: str) -> list[list]:
    """The tree with the node at `path` of type `ty`."""
    i = find(rows, path)
    out = [list(r) for r in rows]
    out[i][1] = ty
    return out


def leaf(ty: str, name: str, meta: int = 0) -> list[list]:
    return [[0, ty, name, meta]]


def vec(name: str, elem: list[list], meta: int = 0) -> list[list]:
    return [[0, "vector", name, meta], [1, "Array", "Array", 0], [2, "int", "size", 0], *[[r[0] + 2, *r[1:]] for r in elem]]


WRITE = {
    "SInt8", "UInt8", "char", "short", "SInt16", "unsigned short", "UInt16", "int", "SInt32", "unsigned int",
    "UInt32", "Type*", "long long", "SInt64", "unsigned long long", "UInt64", "FileSize", "float", "double", "bool",
    "string", "TypelessData",
}  # fmt: skip


def default_value(node):
    """A value of `node`'s shape as the dict-mode typetree writer takes it: zeros, empty strings and lists."""
    if node.m_Type in WRITE:
        return {"string": "", "TypelessData": b"", "float": 0.0, "double": 0.0, "bool": False}.get(node.m_Type, 0)
    if node.m_Type == "pair":
        return tuple(default_value(child) for child in node.m_Children)
    if node.m_Children and node.m_Children[0].m_Type == "Array":
        return []
    return {child.m_Name: default_value(child) for child in node.m_Children}


def element(rows: list[list], *path):
    """The default value of the field at `path`, an array's element for an array."""
    node = to_node(rows)
    for name in path:
        node = next(child for child in node.m_Children if child.m_Name == name)
        if node.m_Children and node.m_Children[0].m_Type == "Array":
            node = node.m_Children[0].m_Children[1]
    return default_value(node)


def fit(template: dict, values: dict) -> dict:
    """`template` with the values it has a field for; a value for a field it lacks is an error."""
    unknown = set(values) - set(template)
    assert not unknown, f"no fields {unknown} in {sorted(template)}"
    return {k: values.get(k, v) for k, v in template.items()}


def write(rows: list[list], value: dict, endian: str = "<") -> bytes:
    from UnityPy.helpers import TypeTreeHelper
    from UnityPy.streams import EndianBinaryWriter

    writer = EndianBinaryWriter(endian=endian)
    TypeTreeHelper.write_typetree(value, to_node(rows), writer)
    return bytes(writer.bytes)


# --- vertex layouts

CODE_SIZE = {"f": 4, "e": 2, "B": 1, "b": 1, "H": 2, "h": 2, "I": 4, "i": 4}


def derived(count: int, channels: dict, sizes: dict) -> list[tuple[int, int]]:
    """The streams `get_streams` derives (`helpers/MeshHelper.py:256-281`): (offset, stride) per stream, from
    channels {index: (stream, offset, format, dimension)} and the component size of each format."""
    streams = 1 + max(c[0] for c in channels.values())
    out, offset = [], 0
    for s in range(streams):
        stride = sum((c[3] & 0xF) * sizes[c[2]] for c in channels.values() if c[0] == s and c[3] > 0)
        out.append((offset, stride))
        offset = (offset + count * stride + 15) & ~15
    return out


def lay_out(count: int, channels: dict, sizes: dict, rows: dict, endian: str = "<") -> bytes:
    """Vertex data holding `rows[chn]` (one tuple a vertex, packed by its struct code) at each channel's stream
    offset, vertex stride and channel offset, the rest zeros."""
    streams = derived(count, channels, sizes)
    total = max((o + count * s for o, s in streams), default=0)
    data = bytearray(-(-total // 16) * 16)
    for chn, (code, values) in rows.items():
        stream, offset, _, _ = channels[chn]
        base, stride = streams[stream]
        for v, row in enumerate(values):
            packed = struct.pack(f"{endian}{len(row)}{code}", *row)
            at = base + stride * v + offset
            data[at : at + len(packed)] = packed
    return bytes(data)
