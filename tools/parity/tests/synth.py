"""Synthetic SerializedFiles for the dumper tests, built byte by byte (formats 14-22).

The layout follows UnityPy 1.25.2's reader (files/SerializedFile.py,
files/ObjectReader.py, helpers/TypeTreeNode.py parse_blob): a big-endian header,
then metadata and object data in the file's endian. Strings may be bytes, so a
test can write names that are not valid UTF-8.
"""

from __future__ import annotations

import struct

ALIGN = 0x4000


class Node:
    """A typetree node; `type` and `name` are str, bytes, or an int string offset (high bit: common string)."""

    def __init__(self, type, name, children=(), meta=0, byte_size=-1, flags=0):
        self.type, self.name, self.children = type, name, list(children)
        self.meta, self.byte_size, self.flags = meta, byte_size, flags


def vec(name, elem, meta=0):
    return Node("vector", name, [Node("Array", "Array", [Node("int", "size"), elem], meta=meta, flags=1)])


def ref_object(name="data"):
    """A ReferencedObject: rid, its (class, ns, asm), and the data those name."""
    managed = Node("ReferencedManagedType", "type", [Node("string", s) for s in ("class", "ns", "asm")])
    return Node("ReferencedObject", name, [Node("SInt64", "rid"), managed, Node("ReferencedObjectData", "data")])


def s32(text, endian="<"):
    """An aligned typetree string."""
    raw = text if isinstance(text, bytes) else text.encode()
    return struct.pack(endian + "i", len(raw)) + raw + b"\0" * (-len(raw) % 4)


def _bytes(s):
    return s if isinstance(s, bytes) else s.encode()


def blob(root, version, endian="<"):
    """parse_blob's layout: node count, string-buffer size, packed node rows, the local strings."""
    rows, strings, offsets = [], bytearray(), {}

    def offset(s):
        if isinstance(s, int):
            return s
        raw = _bytes(s)
        if raw not in offsets:
            offsets[raw] = len(strings)
            strings.extend(raw + b"\0")
        return offsets[raw]

    def walk(n, level):
        rows.append((n, level))
        for child in n.children:
            walk(child, level + 1)

    walk(root, 0)
    fmt = endian + "hBBIIiii" + ("Q" if version >= 19 else "")
    body = b""
    for index, (n, level) in enumerate(rows):
        fields = [1, level, n.flags, offset(n.type), offset(n.name), n.byte_size, index, n.meta]
        body += struct.pack(fmt, *fields, *([0] if version >= 19 else []))
    return struct.pack(endian + "ii", len(rows), len(strings)) + body + bytes(strings)


class Type:
    def __init__(self, class_id, tree=None, script_type_index=-1, is_stripped=0, deps=(), ref_names=None):
        self.class_id, self.tree, self.script_type_index = class_id, tree, script_type_index
        self.is_stripped, self.deps, self.ref_names = is_stripped, list(deps), ref_names

    def encode(self, version, enable_type_tree, is_ref, E):
        out = struct.pack(E + "i", self.class_id)
        if version >= 16:
            out += bytes([self.is_stripped])
        if version >= 17:
            out += struct.pack(E + "h", self.script_type_index)
        if version >= 13:
            if (
                (is_ref and self.script_type_index >= 0)
                or (version < 16 and self.class_id < 0)
                or (version >= 16 and self.class_id == 114)
            ):
                out += bytes(range(16))  # script_id
            out += bytes(range(16, 32))  # old_type_hash
        if enable_type_tree:
            out += blob(self.tree, version, E)
            if version >= 21:
                if is_ref:
                    out += b"".join(_bytes(s) + b"\0" for s in self.ref_names)
                else:
                    out += struct.pack(E + "i", len(self.deps)) + struct.pack(f"{E}{len(self.deps)}i", *self.deps)
        return out


class Obj:
    """One object-table row and its data. `type_id` is a type index from format 16, else a class id."""

    def __init__(self, path_id, type_id, data, byte_size=None, script_type_index=-1, is_stripped=0, class_id=None):
        self.path_id, self.type_id, self.data = path_id, type_id, data
        self.byte_size = len(data) if byte_size is None else byte_size
        self.script_type_index, self.is_stripped = script_type_index, is_stripped
        self.class_id = type_id if class_id is None else class_id


def serialized_file(
    types,
    objects,
    version=22,
    unity_version=b"2021.3.2f1",
    enable_type_tree=True,
    ref_types=None,
    user_information=b"",
    endian="<",
):
    """A SerializedFile holding `objects` (Obj) typed by `types` (Type)."""
    E = endian
    head_len = 48 if version >= 22 else 20
    starts, data = [], b""
    for o in objects:
        data += b"\0" * (-len(data) % 8)
        starts.append(len(data))
        data += o.data
    data += b"\0" * max(0, 64 - len(data))

    meta = bytearray(_bytes(unity_version) + b"\0")
    meta += struct.pack(E + "i", 19)  # target_platform
    if version >= 13:
        meta += bytes([enable_type_tree])
    meta += struct.pack(E + "i", len(types))
    for t in types:
        meta += t.encode(version, enable_type_tree, False, E)
    meta += struct.pack(E + "i", len(objects))
    for o, start in zip(objects, starts, strict=True):
        meta += b"\0" * (-(head_len + len(meta)) % 4)
        meta += struct.pack(E + "q", o.path_id)
        meta += struct.pack(E + ("q" if version >= 22 else "I"), start)
        meta += struct.pack(E + "Ii", o.byte_size, o.type_id)
        if version < 16:
            meta += struct.pack(E + "H", o.class_id)
        if version < 17:
            meta += struct.pack(E + "h", o.script_type_index)
        if version in (15, 16):
            meta += struct.pack(E + "b", o.is_stripped)
    meta += struct.pack(E + "i", 0)  # script types
    meta += struct.pack(E + "i", 0)  # externals
    if version >= 20:
        refs = ref_types or []
        meta += struct.pack(E + "i", len(refs))
        for t in refs:
            meta += t.encode(version, enable_type_tree, True, E)
    meta += _bytes(user_information) + b"\0"

    data_offset = head_len + len(meta)
    data_offset += -data_offset % 16
    file_size = data_offset + len(data)
    flag = bytes([endian == ">"]) + b"\0\0\0"
    if version >= 22:
        header = (
            struct.pack(">IIII", 0, 0, version, 0) + flag + struct.pack(">Iqqq", len(meta), file_size, data_offset, 0)
        )
    else:
        header = struct.pack(">IIII", len(meta), file_size, version, data_offset) + flag
    body = header + bytes(meta)
    return body + b"\0" * (data_offset - len(body)) + data
