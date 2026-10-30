"""Synthetic SerializedFiles (formats 1-22), UnityFS bundles and WebFiles for the oracle tests, built byte by byte.

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

    def __init__(self, type, name, children=(), meta=0, byte_size=-1, flags=0, version=1, ref_hash=0):
        self.type, self.name, self.children = type, name, list(children)
        self.meta, self.byte_size, self.flags = meta, byte_size, flags
        self.version, self.ref_hash = version, ref_hash


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


def old_nodes(root, version, endian="<"):
    """`TypeTreeNode.parse`'s layout (formats below 10, and 11): each node field by field, pre-order.

    `variable_count` (0) is written at version 2 only, `index` and `meta_flag` everywhere but version 3.
    """
    out, index = b"", 0

    def walk(n):
        nonlocal out, index
        out += _bytes(n.type) + b"\0" + _bytes(n.name) + b"\0" + struct.pack(endian + "i", n.byte_size)
        if version == 2:
            out += struct.pack(endian + "i", 0)
        if version != 3:
            out += struct.pack(endian + "i", index)
        index += 1
        out += struct.pack(endian + "ii", n.flags, n.version)
        if version != 3:
            out += struct.pack(endian + "i", n.meta)
        out += struct.pack(endian + "i", len(n.children))
        for child in n.children:
            walk(child)

    walk(root)
    return out


def blob(root, version, endian="<"):
    """parse_blob's layout: node count, string-buffer size, packed node rows, the local strings.

    `root` is a Node, or the rows themselves as (level, type, name) for shapes no Node tree gives.
    """
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

    if isinstance(root, Node):
        walk(root, 0)
    else:
        rows = [(Node(t, name), level) for level, t, name in root]
    fmt = endian + "hBBIIiii" + ("Q" if version >= 19 else "")
    body = b""
    for index, (n, level) in enumerate(rows):
        fields = [n.version, level, n.flags, offset(n.type), offset(n.name), n.byte_size, index, n.meta]
        body += struct.pack(fmt, *fields, *([n.ref_hash] if version >= 19 else []))
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
        if enable_type_tree or version < 13:
            out += blob(self.tree, version, E) if version >= 12 or version == 10 else old_nodes(self.tree, version, E)
            if version >= 21:
                if is_ref:
                    out += b"".join(_bytes(s) + b"\0" for s in self.ref_names)
                else:
                    out += struct.pack(E + "i", len(self.deps)) + struct.pack(f"{E}{len(self.deps)}i", *self.deps)
        return out


class Obj:
    """One object-table row and its data. `type_id` is a type index from format 16, else a class id."""

    def __init__(
        self, path_id, type_id, data, byte_size=None, script_type_index=-1, is_stripped=0, class_id=None, is_destroyed=0
    ):
        self.path_id, self.type_id, self.data = path_id, type_id, data
        self.byte_size = len(data) if byte_size is None else byte_size
        self.script_type_index, self.is_stripped = script_type_index, is_stripped
        self.class_id = type_id if class_id is None else class_id
        self.is_destroyed = is_destroyed


class External:
    """An external reference: `temp_empty` from format 6, `guid` and `type` from 5, then the path."""

    def __init__(self, path, temp_empty=b"", guid=bytes(16), type=0):
        self.path, self.temp_empty, self.guid, self.type = path, temp_empty, guid, type


def serialized_file(
    types,
    objects,
    version=22,
    unity_version=b"2021.3.2f1",
    enable_type_tree=True,
    ref_types=None,
    user_information=b"",
    endian="<",
    platform=19,
    big_id_enabled=0,
    script_types=(),
    externals=(),
):
    """A SerializedFile holding `objects` (Obj) typed by `types` (Type).

    Each member is written only from the format that reads it. Below format 9 the data follows the 16-byte
    header and the metadata, its endian byte first, sits at the end; `script_types` are (file index, id) pairs
    and `externals` External rows.
    """
    E = endian
    head_len = 48 if version >= 22 else 20 if version >= 9 else 0
    starts, data = [], b""
    for o in objects:
        data += b"\0" * (-len(data) % 8)
        starts.append(len(data))
        data += o.data
    data += b"\0" * max(0, 64 - len(data))

    meta = bytearray(_bytes(unity_version) + b"\0" if version >= 7 else b"")
    if version >= 8:
        meta += struct.pack(E + "i", platform)
    if version >= 13:
        meta += bytes([enable_type_tree])
    meta += struct.pack(E + "i", len(types))
    for t in types:
        meta += t.encode(version, enable_type_tree, False, E)
    if 7 <= version < 14:
        meta += struct.pack(E + "i", big_id_enabled)
    meta += struct.pack(E + "i", len(objects))
    for o, start in zip(objects, starts, strict=True):
        if version >= 14:
            meta += b"\0" * (-(head_len + len(meta)) % 4)
        meta += struct.pack(E + ("q" if version >= 14 or big_id_enabled else "i"), o.path_id)
        meta += struct.pack(E + ("q" if version >= 22 else "I"), start)
        meta += struct.pack(E + "Ii", o.byte_size, o.type_id)
        if version < 16:
            meta += struct.pack(E + "H", o.class_id)
        if version < 11:
            meta += struct.pack(E + "H", o.is_destroyed)
        elif version < 17:
            meta += struct.pack(E + "h", o.script_type_index)
        if version in (15, 16):
            meta += struct.pack(E + "b", o.is_stripped)
    if version >= 11:
        meta += struct.pack(E + "i", len(script_types))
        for file_index, identifier in script_types:
            meta += struct.pack(E + "i", file_index)
            if version >= 14:
                meta += b"\0" * (-(head_len + len(meta)) % 4)
            meta += struct.pack(E + ("q" if version >= 14 else "i"), identifier)
    meta += struct.pack(E + "i", len(externals))
    for e in externals:
        if version >= 6:
            meta += _bytes(e.temp_empty) + b"\0"
        if version >= 5:
            meta += e.guid + struct.pack(E + "i", e.type)
        meta += _bytes(e.path) + b"\0"
    if version >= 20:
        refs = ref_types or []
        meta += struct.pack(E + "i", len(refs))
        for t in refs:
            meta += t.encode(version, enable_type_tree, True, E)
    if version >= 5:
        meta += _bytes(user_information) + b"\0"
    if version < 9:
        meta = bytes([endian == ">"]) + meta
        file_size = 16 + len(data) + len(meta)
        return struct.pack(">IIII", len(meta), file_size, version, 16) + data + meta

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


def fs_bundle(entries, signature=b"UnityFS", engine=b"2021.3.2f1", codec=2, stored_sizes=None, blocks=None):
    """A UnityFS bundle (format 8, ArchiveFlags 0x43: blocks info and blocks LZ4, the info after the header).

    `entries` are (name, data, flags), laid out back to back as one block unless `blocks` (a list of
    byte strings) splits the data; `codec` compresses each block (0 none, 2 LZ4). `stored_sizes`
    overrides the uncompressed size each block records. The directory may also be given as
    (name, offset, size, flags) rows, for entries that do not match the data.
    """
    import lz4.block

    def compress(data, flags):
        return lz4.block.compress(data, store_size=False) if flags & 0x3F in (2, 3) else data

    rows = []
    data, offset = b"", 0
    for entry in entries:
        if len(entry) == 4:
            rows.append(entry)
            continue
        name, payload, flags = entry
        rows.append((name, offset, len(payload), flags))
        data += payload
        offset += len(payload)
    if blocks is None:
        blocks = [data] if data else []
    info = bytes(16) + struct.pack(">i", len(blocks))
    block_data = b""
    for i, block in enumerate(blocks):
        packed = compress(block, codec)
        stored = len(block) if stored_sizes is None else stored_sizes[i]
        info += struct.pack(">IIH", stored, len(packed), codec)
        block_data += packed
    info += struct.pack(">i", len(rows))
    for name, offset, size, flags in rows:
        info += struct.pack(">qqI", offset, size, flags) + name + b"\0"
    packed_info = compress(info, 2)
    head = signature + b"\0" + struct.pack(">I", 8) + b"5.x.x\0" + engine + b"\0"
    out = bytearray(head + struct.pack(">qIII", 0, len(packed_info), len(info), 0x43))
    out += bytes(-len(out) % 16)  # format 8 aligns after the header
    out += packed_info + block_data
    struct.pack_into(">q", out, len(head), len(out))
    return bytes(out)


def web_file(entries, signature=b"UnityWebData1.0"):
    """A plain (uncompressed) WebFile: signature, little-endian header length, (offset, size, name) rows, data."""
    head = signature + b"\0"
    head_length = len(head) + 4 + sum(12 + len(name) for name, _ in entries)
    out = bytearray(head + struct.pack("<i", head_length))
    offset, body = head_length, b""
    for name, payload in entries:
        out += struct.pack("<iii", offset, len(payload), len(name)) + name
        offset += len(payload)
        body += payload
    return bytes(out) + body
