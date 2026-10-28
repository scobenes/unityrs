"""The canonical dump encoding, shared by the UnityPy dumper and the comparer.

docs/CANONICAL_DUMP.md is the specification; this module is its Python
reference. It needs only the standard library, so compare.py runs anywhere.
Typetree nodes are duck-typed (``m_Type``, ``m_Name``, ``m_Children``), so the
value encoder works on UnityPyBoost nodes and on plain test doubles alike.
"""

from __future__ import annotations

import hashlib
import json
import struct

FORMAT = "unityrs-canonical-dump"
VERSION = 1
LEVELS = ("L0", "L1", "L2", "L3")

# Raw-byte payloads longer than this are summarised as length + sha256. Inline
# hex keeps small diffs readable; the summary keeps texture and vertex blobs
# from doubling the dump size, and still separates any two different payloads.
INLINE_MAX = 64

FLOAT_WIDTH = {"float": "f32", "double": "f64"}

# Element types whose vectors pack as raw bytes ("u8s"): Font data, shader
# blobs and index buffers are megabytes of them, and as JSON integers they
# would dominate the dump.
U8_TYPES = frozenset(("UInt8", "char"))


class EncodeError(ValueError):
    """A value the canonical form cannot represent faithfully.

    Raised for a value whose Python type does not fit its typetree node, which
    means the dumper and UnityPy disagree about the value model; it is a bug in
    one of them, never an encoding to paper over.
    """


# ---------------------------------------------------------------- JSON layer


def dumps(obj) -> str:
    """Serialise one record exactly as the spec's JSON rules require.

    ``ensure_ascii=False`` with compact separators is byte-for-byte what
    serde_json's default writer emits for the same tree, and no float ever
    reaches JSON (floats are always hex-tagged), so number formatting cannot
    drift between producers.
    """
    return json.dumps(obj, ensure_ascii=False, separators=(",", ":"), allow_nan=False, check_circular=False)


# ------------------------------------------------------------ scalar encoders


def enc_raw(kind: str, raw: bytes):
    """Bytes, or a str that is not valid UTF-8, as its raw bytes."""
    if len(raw) <= INLINE_MAX:
        return {"t": kind, "hex": raw.hex()}
    return {"t": kind, "len": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}


def _raw_str(s: str) -> bytes | None:
    """The raw bytes behind a surrogate-escaped str, or None if it is valid UTF-8."""
    if s.isascii():
        return None
    try:
        s.encode("utf-8")
        return None
    except UnicodeEncodeError:
        pass
    try:
        return s.encode("utf-8", "surrogateescape")
    except UnicodeEncodeError as e:
        # A lone surrogate outside U+DC80..U+DCFF cannot come from decoding bytes.
        raise EncodeError(f"str holds a surrogate that no byte decodes to: {s!r}") from e


def enc_str(s: str):
    """A typetree string value: plain JSON when valid UTF-8, else its raw bytes."""
    raw = _raw_str(s)
    return s if raw is None else enc_raw("str", raw)


def enc_name(s):
    """A dict key or metadata string: like enc_str, but never summarised."""
    if s is None:
        return None
    raw = _raw_str(s)
    return s if raw is None else {"t": "str", "hex": raw.hex()}


def enc_hex(b) -> str | None:
    """Fixed metadata bytes (hashes, GUIDs) as lowercase hex, or None."""
    return None if b is None else bytes(b).hex()


def _check_floats(values, kind: str):
    if set(map(type, values)) - {float}:
        raise EncodeError(f"{kind} field holds a non-float: {values[:4]!r}")


def f32_hex(values) -> str:
    """Concatenated big-endian float32 bit patterns of already-widened floats.

    struct's 'f' narrows exactly for any value UnityPy widened from float32,
    NaN payloads included (the widening left the low 29 mantissa bits zero).
    The round trip below proves exactness, comparing bit patterns so NaNs and
    -0.0 count.
    """
    n = len(values)
    _check_floats(values, "f32")
    try:
        packed = struct.pack(f">{n}f", *values)
    except (OverflowError, struct.error) as e:
        raise EncodeError(f"f32 field holds a value outside float32: {e}") from e
    if struct.pack(f">{n}d", *values) != struct.pack(f">{n}d", *struct.unpack(f">{n}f", packed)):
        raise EncodeError("f32 field holds a value float32 cannot represent exactly")
    return packed.hex()


def u8_bytes(values) -> bytes:
    """A list of ints in 0..255 as bytes (bools are rejected, not coerced)."""
    if set(map(type, values)) - {int}:
        raise EncodeError(f"u8 vector holds a non-int: {values[:4]!r}")
    try:
        return bytes(values)
    except ValueError as e:
        raise EncodeError(f"u8 vector holds a value outside 0..255: {e}") from e


def f64_hex(values) -> str:
    _check_floats(values, "f64")
    return struct.pack(f">{len(values)}d", *values).hex()


# ------------------------------------------------------------- value encoder


def _element_node(node):
    """The element node of a vector node (children[0] is the Array), or None."""
    children = node.m_Children
    if children and children[0].m_Type == "Array" and len(children[0].m_Children) == 2:
        return children[0].m_Children[1]
    return None


class ValueEncoder:
    """Encodes UnityPy's ``parse_as_dict()`` result, guided by its typetree.

    The encoding follows the Python value; the node is consulted only where the
    value alone is ambiguous: the width of a float, and which child node types
    a dict entry, list element or tuple member. ``resolve_ref(type_dict)``
    returns the node of a ReferencedObject's data (UnityPy's
    ``get_ref_type_node``).

    One encoder serves one SerializedFile: it caches child maps by node id, and
    holding the node keeps that id from being reused.
    """

    def __init__(self, resolve_ref=None):
        self._resolve_ref = resolve_ref
        self._maps: dict[int, tuple[object, dict]] = {}

    def encode(self, value, node):
        t = type(value)
        if t is int or t is bool or value is None:
            return value
        if t is str:
            return enc_str(value)
        if t is float:
            return self._float(value, node)
        if t is dict:
            return self._dict(value, node)
        if t is list:
            return self._seq(value, node)
        if t is tuple:
            return self._tuple(value, node)
        if t is bytes or t is bytearray or t is memoryview:
            return enc_raw("bytes", bytes(value))
        raise EncodeError(f"unexpected {t.__name__} under {node.m_Type!r} {node.m_Name!r}")

    def _float(self, value, node):
        width = FLOAT_WIDTH.get(node.m_Type)
        if width == "f32":
            return {"t": "f32", "v": f32_hex([value])}
        if width == "f64":
            return {"t": "f64", "v": f64_hex([value])}
        raise EncodeError(f"float under non-float node {node.m_Type!r} {node.m_Name!r}")

    def _seq(self, value, node):
        elem = _element_node(node)
        if elem is None:
            raise EncodeError(f"list under non-vector node {node.m_Type!r} {node.m_Name!r}")
        if value:
            width = FLOAT_WIDTH.get(elem.m_Type)
            if width:
                return {"t": width + "s", "v": f32_hex(value) if width == "f32" else f64_hex(value)}
            if elem.m_Type in U8_TYPES:
                return enc_raw("u8s", u8_bytes(value))
        enc = self.encode
        return [enc(v, elem) for v in value]

    def _tuple(self, value, node):
        if node.m_Type == "pair" and len(node.m_Children) == 2 and len(value) == 2:
            first, second = node.m_Children
            return {"t": "tuple", "v": [self.encode(value[0], first), self.encode(value[1], second)]}
        # UnityPy's pure-Python reader (used when an object's byte_size is 0)
        # returns tuples for primitive arrays.
        elem = _element_node(node)
        if elem is None:
            raise EncodeError(f"tuple under {node.m_Type!r} {node.m_Name!r}")
        return {"t": "tuple", "v": [self.encode(v, elem) for v in value]}

    def _children(self, node) -> dict:
        hit = self._maps.get(id(node))
        if hit is not None:
            return hit[1]
        # A repeated child name keeps the dict key's first position but the last
        # child's value (PyDict_SetItem overwrites), so the last node types it.
        mapping = {child.m_Name: child for child in node.m_Children}
        self._maps[id(node)] = (node, mapping)
        return mapping

    def _dict(self, value, node):
        children = self._children(node)
        is_ref = node.m_Type == "ReferencedObject"
        out = []
        for key, item in value.items():
            child = children.get(key)
            if child is None:
                raise EncodeError(f"key {key!r} is not a child of {node.m_Type!r} {node.m_Name!r}")
            if is_ref and child.m_Type == "ReferencedObjectData":
                if self._resolve_ref is None:
                    raise EncodeError("ReferencedObject data without a ref-type resolver")
                try:
                    child = self._resolve_ref(value["type"])
                except (KeyError, TypeError, ValueError) as e:
                    raise EncodeError(f"cannot resolve ref type {value.get('type')!r}: {e}") from e
                if child is None:
                    raise EncodeError(f"no ref type for {value['type']!r}")
            out.append([enc_name(key), self.encode(item, child)])
        return {"t": "dict", "v": out}


# ------------------------------------------------------------- value decoder
#
# The comparer works on a decoded tree in which every canonical-form choice is
# undone: packed float lists become lists of F32/F64, and inline and summarised
# raw bytes both become Blob(len, sha256). Two encodings that decode equal hold
# the same value; a byte difference between them is a format violation only.


class F32(int):
    """A float32 by bit pattern."""

    __slots__ = ()

    def __repr__(self):
        return f"f32:{self:08x}"


class F64(int):
    """A float64 by bit pattern."""

    __slots__ = ()

    def __repr__(self):
        return f"f64:{self:016x}"


class Blob(tuple):
    """Raw bytes: kind 'bytes', 'u8s' (a UInt8 vector) or 'str' (not UTF-8): (kind, len, sha256, hex|None)."""

    __slots__ = ()

    def __new__(cls, kind, length, sha, hexdata=None):
        return tuple.__new__(cls, (kind, length, sha, hexdata))

    @property
    def kind(self):
        return self[0]

    def __eq__(self, other):
        return isinstance(other, Blob) and self[:3] == other[:3]

    def __ne__(self, other):
        return not self == other

    def __hash__(self):
        return hash(self[:3])

    def __repr__(self):
        if self[3] is not None:
            return f"{self[0]}:{self[3]}"
        return f"{self[0]}:len={self[1]},sha256={self[2][:16]}..."


class Tuple(list):
    """A Python tuple (a typetree pair)."""

    __slots__ = ()


class Dict(list):
    """An ordered dict as a list of (key, value) pairs."""

    __slots__ = ()


class FormatError(ValueError):
    """A dump line that does not follow the spec."""


def _blob(o, kind):
    if "hex" in o:
        raw = bytes.fromhex(o["hex"])
        return Blob(kind, len(raw), hashlib.sha256(raw).hexdigest(), o["hex"])
    return Blob(kind, o["len"], o["sha256"])


def _unpack(o, fmt, size, cls):
    raw = bytes.fromhex(o["v"])
    if not raw or len(raw) % size:
        raise FormatError(f"bad packed {o['t']} length {len(raw)}")
    return [cls(x) for x in struct.unpack(f">{len(raw) // size}{fmt}", raw)]


def decode(tv):
    """Decode one tagged value into the comparison model."""
    t = type(tv)
    if t is list:
        return [decode(v) for v in tv]
    if t is not dict:
        if t is float:
            raise FormatError("untagged JSON float in a value")
        return tv
    tag = tv.get("t")
    try:
        if tag == "dict":
            return Dict((decode_key(k), decode(v)) for k, v in tv["v"])
        if tag == "f32":
            return F32(int(tv["v"], 16))
        if tag == "f64":
            return F64(int(tv["v"], 16))
        if tag == "f32s":
            return _unpack(tv, "I", 4, F32)
        if tag == "f64s":
            return _unpack(tv, "Q", 8, F64)
        if tag == "tuple":
            return Tuple(decode(v) for v in tv["v"])
        if tag in ("bytes", "str", "u8s"):
            return _blob(tv, tag)
    except (KeyError, TypeError, ValueError) as e:
        raise FormatError(f"malformed {tag!r} value: {e}") from e
    raise FormatError(f"unknown value tag {tag!r}")


def decode_key(k):
    if type(k) is str:
        return k
    return decode(k)
