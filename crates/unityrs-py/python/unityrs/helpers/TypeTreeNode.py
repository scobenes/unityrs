# Ported from UnityPy 1.25.2 (MIT): helpers/TypeTreeNode.py:1-370.
# UnityPyBoost TypeTreeHelper.cpp:28-65 and TypeTreeHelper.cpp:1130-1248.
# See THIRD_PARTY_LICENSES for the UnityPy and UnityPyBoost notices.
"""Mutable UnityPy-compatible type tree nodes, independent of attrs and UnityPy."""
from __future__ import annotations

import io
import re
from struct import Struct
from threading import Lock
from itertools import zip_longest
from ._bounded_tree import guard_for, charge_node, text_bound, walk, read_bytes, read_string

TYPETREENODE_KEYS = [
    "m_Level", "m_Type", "m_Name", "m_ByteSize", "m_Version", "m_Children",
    "m_TypeFlags", "m_VariableCount", "m_Index", "m_MetaFlag", "m_RefTypeHash",
]
_CONSTRUCTOR_KEYS = tuple(TYPETREENODE_KEYS)
SYSTEM_GLOBAL_LOCK = Lock()
NAME_PEEK_NODE_CACHE = {}
COMMONSTRING_CACHE = {}


def clean_name(name: str) -> str:
    if len(name) == 0:
        return name
    if name.startswith("(int&)"):
        name = name[6:]
    if name.endswith("?"):
        name = name[:-1]
    name = re.sub(r"[ \.:\-\[\]]", "_", name)
    if name in ["pass", "from"]:
        name += "_"
    if name[0].isdigit():
        name = f"x{name}"
    return name


def _boost_clean_name(name):
    # The extension uses a NUL-terminated UTF-8 string and ASCII isdigit.
    name = name.split("\0", 1)[0]
    if name.startswith("(int&)"):
        name = name[6:]
    if name.endswith("?"):
        name = name[:-1]
    name = re.sub(r"[ \.:\-\[\]]", "_", name)
    if name in ("pass", "from"):
        name += "_"
    if name and "0" <= name[0] <= "9":
        name = "x" + name
    return name


class TypeTreeNodeC:
    __slots__ = (*TYPETREENODE_KEYS, "_clean_name", "_cached_type", "_cached_align")

    def __init__(self, *args, **kwargs):
        keys = _CONSTRUCTOR_KEYS
        if len(args) + len(kwargs) > len(keys):
            kind = "keyword arguments" if not args else "arguments"
            raise TypeError(f"function takes at most 11 {kind} ({len(args) + len(kwargs)} given)")
        values = {}
        for i, key in enumerate(keys):
            if i < len(args):
                if key in kwargs:
                    raise TypeError(f"argument for function given by name ('{key}') and position ({i + 1})")
                value = args[i]
            elif key in kwargs:
                value = kwargs[key]
            elif i < 5:
                raise TypeError(f"function missing required argument '{key}' (pos {i + 1})")
            else:
                continue
            if i < 5:
                expected = str if key in ("m_Type", "m_Name") else int
                if not isinstance(value, expected):
                    raise TypeError(f"argument {i + 1} must be {expected.__name__}, not {type(value).__name__}")
            values[key] = value
        for key in kwargs:
            if key not in keys:
                raise TypeError(f"'{key}' is an invalid keyword argument for this function")
        for key in keys[6:]:
            value = values.get(key)
            if value is not None and not isinstance(value, int):
                raise TypeError(f"Expected int or None for {key}")
        for key in keys:
            value = values.get(key)
            if key == "m_Children" and value is None:
                value = []
            setattr(self, key, value)
        # Encoding rejects lone surrogates just as PyUnicode_AsUTF8 does.
        self.m_Name.encode("utf-8")
        self.m_Type.encode("utf-8")
        if self.m_MetaFlag is not None and not -(2**63) <= self.m_MetaFlag < 2**63:
            raise SystemError(f"<class '{type(self).__module__}.{type(self).__name__}'> returned a result with an exception set")
        self._clean_name = _boost_clean_name(self.m_Name)
        self._cached_type = self.m_Type
        self._cached_align = bool((self.m_MetaFlag or 0) & 0x4000)

    def __del__(self):
        # Python releases the owned field references during object destruction.
        pass

    def __repr__(self):
        return (f"TypeTreeNode(m_Level={self.m_Level!r}, m_Type={self.m_Type!r}, "
                f"m_Name={self.m_Name!r}, m_MetaFlag={self.m_MetaFlag!r})")


class TypeTreeNode(TypeTreeNodeC):
    def traverse(self, *, limits=None, budget=None, _guard=None):
        guard = guard_for(self, limits, budget, _guard)
        for node, _ in walk(self, guard):
            yield node

    @classmethod
    def parse(cls, reader, version, *, limits=None, budget=None, _guard=None):
        guard = guard_for(limits=limits, budget=budget, guard=_guard)
        guard.step(0, 4096, 1)
        dummy_node = cls(-1, "", "", 0, 0, [])
        dummy_root = cls(-1, "", "", 0, 0, [dummy_node])
        stack = [(dummy_root, 1)]
        while stack:
            parent, count = stack[-1]
            if count == 1:
                stack.pop()
            else:
                stack[-1] = (parent, count - 1)
            guard.step(parent.m_Level + 2, 2048, 1)
            ty, name = read_string(reader, guard), read_string(reader, guard)
            charge_node(guard, parent.m_Level + 1, ty, name)
            node = cls(
                m_Level=parent.m_Level + 1,
                m_Type=ty,
                m_Name=name,
                m_ByteSize=reader.read_int(),
                m_VariableCount=reader.read_int() if version == 2 else None,
                m_Index=reader.read_int() if version != 3 else None,
                m_TypeFlags=reader.read_int(),
                m_Version=reader.read_int(),
                m_MetaFlag=reader.read_int() if version != 3 else None,
            )
            parent.m_Children[-count] = node
            children_count = reader.read_int()
            if children_count > 0:
                guard.step(node.m_Level + 2, 512 + 16 * children_count, children_count)
                node.m_Children = [dummy_node] * children_count
                stack.append((node, children_count))
        return dummy_root.m_Children[0]

    @classmethod
    def parse_blob(cls, reader, version, *, limits=None, budget=None, _guard=None):
        guard = guard_for(limits=limits, budget=budget, guard=_guard)
        node_count = reader.read_int()
        stringbuffer_size = reader.read_int()
        node_struct, keys = _get_blob_node_struct(reader.endian, version)
        guard.step(0, 2048 * max(0, node_count), max(0, node_count))
        struct_data = read_bytes(reader, node_struct.size * node_count, guard)
        stringbuffer = read_bytes(reader, stringbuffer_size, guard)
        common = get_common_strings(_guard=guard)

        def read_string(value):
            if value & 0x80000000:
                offset = value & 0x7FFFFFFF
                return common.get(offset, str(offset))
            # EndianBinaryReader_Memoryview searches one 32767-byte window.
            end_at = value + 32767
            guard.step(0, 256 + 4 * min(32767, max(0, len(stringbuffer) - value)),
                       min(32767, max(0, len(stringbuffer) - value)))
            end = stringbuffer.find(b"\0", value, end_at)
            if end == -1:
                if end_at >= len(stringbuffer):
                    raise Exception("String not terminated")
                end = end_at
            return stringbuffer[value:end].decode("utf-8", "surrogateescape")

        nodes = []
        for raw in node_struct.iter_unpack(struct_data):
            guard.step(0, 2048, 1)
            ty, name = read_string(raw[3]), read_string(raw[4])
            charge_node(guard, 0, ty, name)
            nodes.append(cls(**dict(zip(keys[:3], raw[:3])),
                             **dict(zip(keys[5:], raw[5:])),
                             m_Type=ty, m_Name=name))
        return cls.from_list(nodes, _guard=guard)

    @classmethod
    def from_list(cls, nodes, *, limits=None, budget=None, _guard=None):
        guard = guard_for(limits=limits, budget=budget, guard=_guard)
        # Reject a large flat conversion before constructors encode any text.
        guard.step(0, 2048 + 2048 * len(nodes), len(nodes))
        fake_root = cls(-1, "", "", 0, 0, [])
        stack = [fake_root]
        parent = prev = fake_root
        if isinstance(nodes[0], dict):
            if "m_Level" not in nodes[0] or "m_Type" not in nodes[0] or "m_Name" not in nodes[0]:
                raise ValueError("Nodes must contain at least m_Level, m_Type and m_Name")
            patch_dict = {}
            if "m_ByteSize" not in nodes[0]:
                patch_dict["m_ByteSize"] = 0
            if "m_Version" not in nodes[0]:
                patch_dict["m_Version"] = 0
            converted = []
            for node in nodes:
                if isinstance(node, dict):
                    # Bound keyword copies and validation errors before **node.
                    count = dict.__len__(node)
                    guard.step(0, 256 * count, count)
                    for key in dict.__iter__(node):
                        if isinstance(key, str):
                            size = text_bound(key)
                            guard.step(0, size * 4, size)
                    charge_node(guard, 0,
                                node.get("m_Type", ""), node.get("m_Name", ""))
                converted.append(cls(**node, **patch_dict))
            nodes = converted
        for node in nodes:
            charge_node(guard, 0, node.m_Type, node.m_Name)
            if node.m_Level > prev.m_Level:
                stack.append(parent)
                parent = prev
            elif node.m_Level < prev.m_Level:
                while node.m_Level <= parent.m_Level:
                    parent = stack.pop()
            parent.m_Children.append(node)
            prev = node
        root = fake_root.m_Children[0]
        for _ in walk(root, guard):
            pass
        return root

    def get_name_peek_node(self, *, limits=None, budget=None):
        guard = guard_for(self, limits, budget)
        charge_node(guard, 0, self.m_Type, self.m_Name)
        with SYSTEM_GLOBAL_LOCK:
            key = (self.m_Name, self.m_Type, self.m_Version)
            if key in NAME_PEEK_NODE_CACHE:
                return NAME_PEEK_NODE_CACHE[key]
            result = None
            for i, child in enumerate(self.m_Children):
                charge_node(guard, 1, child.m_Type, child.m_Name)
                if child.m_Name in ("m_Name", "name"):
                    guard.step(0, 16 * (i + 1), i + 1)
                    result = (TypeTreeNode(self.m_Level, self.m_Type, self.m_Name,
                                           self.m_ByteSize, self.m_Version,
                                           self.m_Children[:i + 1]), child.m_Name)
                    source = getattr(self, "_tree_source", None)
                    if source is not None:
                        result[0]._tree_source = source
                    break
            NAME_PEEK_NODE_CACHE[key] = result
            return result

    def dump(self, writer, version, *, limits=None, budget=None, _guard=None):
        guard = guard_for(self, limits, budget, _guard)
        for node, _ in walk(self, guard):
            # UnityPy's old-format writer repeats root metadata per edge.
            charge_node(guard, 0, self.m_Type, self.m_Name)
            writer.write_string_to_null(self.m_Type)
            writer.write_string_to_null(self.m_Name)
            writer.write_int(self.m_ByteSize)
            if version == 2:
                assert self.m_VariableCount is not None
                writer.write_int(self.m_VariableCount)
            if version != 3:
                assert self.m_Index is not None
                writer.write_int(self.m_Index)
            writer.write_int(self.m_TypeFlags or 0)
            writer.write_int(self.m_Version)
            if version != 3:
                assert self.m_MetaFlag is not None
                writer.write_int(self.m_MetaFlag)
            writer.write_int(len(self.m_Children))

    def dump_blob(self, writer, version, *, limits=None, budget=None, _guard=None):
        guard = guard_for(self, limits, budget, _guard)
        common_offsets = {}
        for offset, string in get_common_strings(_guard=guard).items():
            size = text_bound(string)
            guard.step(0, 256 + size, 1 + size)
            common_offsets[string] = offset
        offsets = {}
        strings = io.BytesIO()
        records = io.BytesIO()
        node_struct, keys = _get_blob_node_struct(writer.endian, version)

        def write_string(string):
            offset = offsets.get(string)
            if offset is None:
                common_offset = common_offsets.get(string)
                if common_offset:
                    offset = common_offset | 0x80000000
                else:
                    offset = strings.tell()
                    strings.write(string.encode("utf-8", "surrogateescape") + b"\0")
                offsets[string] = offset
            return offset

        node_count = 0
        for node, _ in walk(self, guard):
            records.write(node_struct.pack(
                *[getattr(node, key) for key in keys[:3]],
                write_string(node.m_Type), write_string(node.m_Name),
                *[getattr(node, key) for key in keys[5:]],
            ))
            node_count += 1
        writer.write_int(node_count)
        writer.write_int(strings.tell())
        writer.write(records.getvalue())
        writer.write(strings.getvalue())

    def dump_structure(self, indent="  ", *, limits=None, budget=None, _guard=None):
        guard = guard_for(self, limits, budget, _guard)
        sb = []
        for node, depth in walk(self, guard):
            metadata = sum(int.bit_length(value) if isinstance(value, int) else text_bound(str(value))
                           for value in (node.m_ByteSize, node.m_Index, node.m_Version,
                                         node.m_TypeFlags, node.m_MetaFlag))
            size = 512 + text_bound(indent) + 8 * depth + metadata
            # Both the line list and the final join coexist during handoff.
            guard.step(0, size * 4, size)
            prefix = indent + "  " * depth
            sb.append(f"{prefix}{node.m_Type} {node.m_Name} // ByteSize{{{node.m_ByteSize:X}}}, Index{{{node.m_Index}}}, "
                      f"                Version{{{node.m_Version}}}, TypeFlags{{{node.m_TypeFlags}}}, MetaFlag{{{node.m_MetaFlag}}}")
        return "\n".join(sb)

    def to_dict(self):
        return {key: value for key, value in ((key, getattr(self, key)) for key in TYPETREENODE_KEYS)
                if value is not None}

    def to_dict_list(self, *, limits=None, budget=None, _guard=None):
        guard = guard_for(self, limits, budget, _guard)
        return [node.to_dict() for node, _ in walk(self, guard)]

    def __eq__(self, other):
        # Preserve the public error for non-node operands before walking.
        other.to_dict()
        left_guard, right_guard = guard_for(self), guard_for(other)
        for left, right in zip_longest(walk(self, left_guard), walk(other, right_guard)):
            if left is None or right is None or left[1] != right[1]:
                return False
            a, b = left[0].to_dict(), right[0].to_dict()
            a.pop("m_Children", None)
            b.pop("m_Children", None)
            if a != b or len(left[0].m_Children) != len(right[0].m_Children):
                return False
        return True

    def _native_rows(self, use_boost=True, *, limits=None, budget=None, _guard=None):
        """Stream actual edges; native checks bounds before encoding each row."""
        guard = guard_for(self, limits, budget, _guard)
        for node, depth in walk(self, guard):
            parse_type = node._cached_type if use_boost else node.m_Type
            aligned = node._cached_align if use_boost else bool((node.m_MetaFlag or 0) & 0x4000)
            # The dict reader uses current public names. Other stored metadata
            # never affects decoding and may contain Python ints wider than Rust.
            yield (depth, parse_type, node.m_Name, 0,
                   0, None, None, None, 0x4000 if aligned else 0, None)


def get_common_strings(version=None, *, _guard=None):
    if version in COMMONSTRING_CACHE:
        return COMMONSTRING_CACHE[version]
    from . import _tpk
    ret = _tpk.common_strings(version, _guard=guard_for(guard=_guard))
    COMMONSTRING_CACHE[version] = ret
    return ret


def _get_blob_node_struct(endian, version):
    struct_type = f"{endian}hBBIIiii"
    keys = ["m_Version", "m_Level", "m_TypeFlags", "m_TypeStrOffset", "m_NameStrOffset",
            "m_ByteSize", "m_Index", "m_MetaFlag"]
    if version >= 19:
        struct_type += "Q"
        keys.append("m_RefTypeHash")
    return Struct(struct_type), keys


__all__ = ("TypeTreeNode", "get_common_strings", "clean_name")
