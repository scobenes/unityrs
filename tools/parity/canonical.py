"""The canonical dump encoding, shared by the UnityPy dumper and the comparer.

docs/CANONICAL_DUMP.md is the specification; this module is its Python
reference. It needs only the standard library, so compare.py runs anywhere.
Typetree nodes are duck-typed (``m_Type``, ``m_Name``, ``m_Children``), so the
value encoder works on UnityPyBoost nodes and on plain test doubles alike.
"""

from __future__ import annotations

import hashlib
import json
import re
import struct

FORMAT = "unityrs-canonical-dump"
VERSION = 1
LEVELS = ("L0", "L1", "L2", "L3")

# The oracle stack a reference dump must come from (docs/CANONICAL_DUMP.md).
ORACLE_UNITYPY = "1.25.2"
ORACLE_PRODUCER = "unitypy_dump.py"

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


# ------------------------------------------------------------ canonical form
#
# The rules a single line can break whatever the other dump holds: section 4's
# member lists and the value encodings of section 3 that the value shows on its
# own. Packing by node type, float width and UInt8-vs-int need the typetree, so
# only the comparison with the reference can check them.

RECORD_MEMBERS = {
    "dump": ("rec", "format", "version", "levels", "producer"),
    "file": ("rec", "file", "l0", "l1", "error"),
    "object": ("rec", "file", "path_id", "l1", "l2", "l3"),
    "end": ("rec", "files", "objects"),
}
L0_MEMBERS = ("type", "parsed_as", "size", "sha256")
FILE_L1_MEMBERS = {
    "bundle": (
        "kind",
        "signature",
        "format",
        "version_player",
        "version_engine",
        "engine_version_packed",
        "flags",
        "flags_class",
        "block_alignment",
        "encrypted",
        "hash",
        "crc",
        "blocks_info",
        "blocks",
        "directory",
    ),
    "web": ("kind", "signature", "packer", "directory"),
    "serialized": (
        "kind",
        "header",
        "unity_version",
        "version_packed",
        "target_platform",
        "enable_type_tree",
        "big_id_enabled",
        "types",
        "object_order",
        "script_types",
        "externals",
        "ref_types",
        "user_information",
        "container",
    ),
}
SERIALIZED_HEADER_MEMBERS = ("metadata_size", "file_size", "version", "data_offset", "endian", "reserved", "unknown")
TYPE_MEMBERS = (
    "class_id",
    "is_stripped_type",
    "script_type_index",
    "script_id",
    "old_type_hash",
    "nodes",
    "type_dependencies",
    "class_name",
    "namespace",
    "assembly",
)
FILE_ERROR_MEMBERS = ("kind",)
OBJECT_SECTIONS = (
    ("L1", "l1", ("type_id", "class_id", "byte_start", "byte_size", "is_destroyed", "is_stripped")),
    ("L2", "l2", ("root_type", "source", "error", "value")),
    ("L3", "l3", ("raw_size", "raw_sha256", "resave", "resave_size", "resave_sha256", "first_diff")),
)
L2_ERROR_MEMBERS = ("kind", "bytes_read")
# The l1 kind each parse result carries; a file parsed as raw, zip or error has no l1.
FILE_L1_KIND = {"SerializedFile": "serialized", "BundleFile": "bundle", "WebFile": "web"}
# Name positions (the metadata-string rule) in the l1 of each kind: members, and
# (member, row length, name columns) for lists of rows.
L1_NAMES = {
    "bundle": (("signature", "version_player", "version_engine"), (("directory", 4, (3,)),)),
    "web": (("signature",), (("directory", 4, (3,)),)),
    "serialized": (("unity_version", "user_information"), (("externals", 4, (0, 3)), ("container", 5, (0,)))),
}
TYPE_NAMES = ("class_name", "namespace", "assembly")

_EVEN_HEX = re.compile(r"(?:[0-9a-f]{2})*")
_SHA256 = re.compile(r"[0-9a-f]{64}")
_PACKED = {
    "f32": re.compile(r"[0-9a-f]{8}"),
    "f64": re.compile(r"[0-9a-f]{16}"),
    "f32s": re.compile(r"(?:[0-9a-f]{8})+"),
    "f64s": re.compile(r"(?:[0-9a-f]{16})+"),
}
_RAW_TAGS = ("bytes", "u8s", "str")


def _is_utf8(raw: bytes) -> bool:
    try:
        raw.decode("utf-8")
    except UnicodeDecodeError:
        return False
    return True


def _is_hex_name(v) -> bool:
    """Whether v has the hex form of a name: {"t":"str","hex":H} with lowercase, even-length H."""
    return (
        type(v) is dict
        and list(v) == ["t", "hex"]
        and v["t"] == "str"
        and type(v["hex"]) is str
        and _EVEN_HEX.fullmatch(v["hex"]) is not None
    )


def check_name(v, path, out, nullable=True):
    """The metadata-string rule (section 4.2): a JSON string, or the hex of bytes that are not valid UTF-8."""
    if type(v) is str or (v is None and nullable):
        return
    if not _is_hex_name(v):
        out.append((path, "bad_name", v))
    elif _is_utf8(bytes.fromhex(v["hex"])):
        out.append((path, "hex_name_is_utf8", v))


def _row_names(rows, length, columns, path, out):
    """check_name on the given columns of a list of rows of `length` members."""
    if rows is None:
        return
    if type(rows) is not list:
        out.append((path, "malformed", rows))
        return
    for i, row in enumerate(rows):
        if type(row) is not list or len(row) != length:
            out.append((f"{path}[{i}]", "malformed", row))
            continue
        for col in columns:
            check_name(row[col], f"{path}[{i}][{col}]", out)


def _l1_names(l1, out):
    names, rows = L1_NAMES[l1["kind"]]
    for member in names:
        check_name(l1[member], f"l1.{member}", out)
    for member, length, columns in rows:
        _row_names(l1[member], length, columns, f"l1.{member}", out)
    for member in ("types", "ref_types") if l1["kind"] == "serialized" else ():
        for i, t in enumerate(l1[member] if type(l1[member]) is list else []):
            if type(t) is dict and list(t) == list(TYPE_MEMBERS):
                for name in TYPE_NAMES:
                    check_name(t[name], f"l1.{member}[{i}].{name}", out)
                _row_names(t["nodes"], 10, (1, 2), f"l1.{member}[{i}].nodes", out)


def _members(obj, expected, path, out, note=False) -> bool:
    """Whether obj is an object whose members are exactly `expected`, in order (plus a last `note`)."""
    if type(obj) is not dict:
        out.append((path, "not_object", obj))
        return False
    keys = list(obj)
    if note and keys and keys[-1] == "note":
        keys.pop()
    if keys != list(expected):
        out.append((path, "members", list(obj)))
        return False
    return True


def check_record(rec, levels) -> list:
    """Every section 2-4 rule this record breaks on its own, as (path, kind, detail) triples.

    `levels` is what its dump's header declares: a section of an undeclared
    level must be null, and one of a declared level must be there wherever
    the record has that level.
    """
    out = []
    kind = rec.get("rec")
    if not _members(rec, RECORD_MEMBERS.get(kind, ()) if type(kind) is str else (), "record", out):
        return out
    if kind in ("file", "object") and type(rec["file"]) is list:
        for i, name in enumerate(rec["file"]):
            check_name(name, f"file[{i}]", out, nullable=False)
    if kind == "file":
        l0_ok = _members(rec["l0"], L0_MEMBERS, "l0", out)
        parsed = rec["l0"]["parsed_as"] if l0_ok else None
        # parsed_as says whether the file has an error and which l1 it has.
        if l0_ok and (parsed == "error") != (rec["error"] is not None):
            out.append(("error", "error_mismatch", f"error {rec['error']!r} for parsed_as {parsed!r}"))
        wanted = FILE_L1_KIND.get(parsed) if type(parsed) is str else None
        l1 = rec["l1"]
        if l1 is not None and "L1" not in levels:
            out.append(("l1", "undeclared_level", "L1"))
        elif l1 is None:
            if wanted and "L1" in levels:
                out.append(("l1", "missing_section", f"L1 of a {parsed}"))
        else:
            l1_kind = l1.get("kind") if type(l1) is dict else None
            members = FILE_L1_MEMBERS.get(l1_kind) if type(l1_kind) is str else None
            if l0_ok and l1_kind != wanted:
                out.append(("l1.kind", "kind_mismatch", f"{l1_kind!r} for parsed_as {parsed!r}"))
            if _members(l1, members or (), "l1", out):
                if l1["kind"] == "serialized":
                    _members(l1["header"], SERIALIZED_HEADER_MEMBERS, "l1.header", out)
                    for name in ("types", "ref_types"):
                        if type(l1[name]) is list:
                            for i, t in enumerate(l1[name]):
                                _members(t, TYPE_MEMBERS, f"l1.{name}[{i}]", out)
                _l1_names(l1, out)
        if rec["error"] is not None:
            _members(rec["error"], FILE_ERROR_MEMBERS, "error", out, note=True)
    elif kind == "object":
        for level, name, members in OBJECT_SECTIONS:
            section = rec[name]
            if section is None:
                if level in levels:
                    out.append((name, "missing_section", level))
                continue
            if level not in levels:
                out.append((name, "undeclared_level", level))
            elif _members(section, members, name, out, note=name == "l3") and name == "l2":
                _check_l2(section, out)
    return out


def _check_l2(l2, out):
    error = l2["error"]
    error_kind = None
    if error is not None and _members(error, L2_ERROR_MEMBERS, "l2.error", out, note=True):
        error_kind = error["kind"]
    check_name(l2["root_type"], "l2.root_type", out)
    # A parse that finished has a value; so does check_read, which re-reads
    # without the check. Every other error has none.
    if (error is None or error_kind == "check_read") != (l2["value"] is not None):
        out.append(
            (
                "l2.value",
                "value_mismatch",
                f"value {'null' if l2['value'] is None else 'present'} for error {error_kind}",
            )
        )
    check_value(l2["value"], "l2.value", out)


def check_value(tv, path, out):
    """Append (path, kind, detail) for each section 3 rule the tagged value tv breaks."""
    t = type(tv)
    if t is list:
        if any(type(v) is dict and v.get("t") in ("f32", "f64") for v in tv):
            out.append((path, "unpacked", "a list of tagged floats; vectors of float and double pack"))
        for i, v in enumerate(tv):
            check_value(v, f"{path}[{i}]", out)
        return
    if t is float:
        out.append((path, "json_float", tv))
        return
    if t is not dict:
        return  # null, bool, int, str
    tag = tv.get("t")
    keys = list(tv)
    if type(tag) is not str:
        out.append((path, "unknown_tag", tag))
    elif tag in _RAW_TAGS:
        _check_raw(tv, tag, keys, path, out)
    elif tag in _PACKED or tag in ("dict", "tuple"):
        if keys != ["t", "v"]:
            out.append((path, "members", keys))
            return
        v = tv["v"]
        if tag == "dict":
            _check_dict(v, path, out)
        elif tag == "tuple":
            if type(v) is not list:
                out.append((path, "malformed", v))
                return
            for i, x in enumerate(v):
                check_value(x, f"{path}[{i}]", out)
        elif type(v) is not str or not _PACKED[tag].fullmatch(v):
            out.append((path, "empty_packed" if v == "" else "bad_hex", v))
    else:
        out.append((path, "unknown_tag", tag))


def _check_dict(entries, path, out):
    if type(entries) is not list:
        out.append((path, "malformed", entries))
        return
    seen = set()
    for entry in entries:
        if type(entry) is not list or len(entry) != 2:
            out.append((path, "malformed", entry))
            continue
        key, value = entry
        if type(key) is str:
            name, ident = key, ("str", key)
        elif _is_hex_name(key):
            name, ident = key["hex"], ("hex", key["hex"])
            if _is_utf8(bytes.fromhex(name)):
                out.append((f"{path}.{name}", "hex_name_is_utf8", key))
        else:
            out.append((path, "bad_key", key))
            continue
        if ident in seen:
            out.append((f"{path}.{name}", "duplicate_key", key))
        seen.add(ident)
        check_value(value, f"{path}.{name}", out)


def _check_raw(tv, tag, keys, path, out):
    if keys == ["t", "hex"]:
        h = tv["hex"]
        if type(h) is not str or not _EVEN_HEX.fullmatch(h):
            out.append((path, "bad_hex", h))
            return
        n, raw = len(h) // 2, bytes.fromhex(h)
        if n > INLINE_MAX:
            out.append((path, "inline_too_long", n))
    elif keys == ["t", "len", "sha256"]:
        n, sha, raw = tv["len"], tv["sha256"], None
        if type(n) is not int or n < 0:
            out.append((path, "bad_len", n))
            return
        if n <= INLINE_MAX:
            out.append((path, "summary_too_short", n))
        if type(sha) is not str or not _SHA256.fullmatch(sha):
            out.append((path, "bad_sha256", sha))
    else:
        out.append((path, "members", keys))
        return
    if tag == "u8s" and n == 0:
        out.append((path, "empty_packed", tv))
    if tag == "str" and raw is not None and _is_utf8(raw):
        out.append((path, "str_is_utf8", tv))
